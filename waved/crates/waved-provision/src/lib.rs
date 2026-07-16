pub mod ap;
pub mod dhcp;
pub mod http;

use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::{info, warn};
use waved_state::{Command, ProvisioningState, StateBus};

const PROV_TIMEOUT: Duration = Duration::from_secs(300); // 5 min

/// Derive a stable 8-hex-char provisioning passphrase from a seed (the MAC's
/// numeric form). Deterministic so the on-screen value matches across restarts
/// within a session; not a secret beyond the provisioning window.
pub fn gen_passphrase(seed: u32) -> String {
    format!("{seed:08x}")
}

pub struct ProvisioningService {
    bus: Arc<StateBus>,
    mac: String,
    mac_seed: u32,
}

impl ProvisioningService {
    pub fn new(bus: Arc<StateBus>, mac: String) -> Self {
        let mac_seed = mac
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .collect::<String>()
            .get(0..8)
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .unwrap_or(0);
        Self { bus, mac, mac_seed }
    }

    pub async fn run(self, mut commands: broadcast::Receiver<Command>) {
        info!("provision_srv started");
        loop {
            match commands.recv().await {
                Ok(Command::OpenWifiProvisioning) => self.provision().await,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    fn set(&self, state: ProvisioningState) {
        let _ = self.bus.provisioning.send(state);
    }

    async fn provision(&self) {
        // Subscribe to the command bus BEFORE the long scan/AP-bringup phase: a
        // broadcast receiver only sees messages sent after it subscribes, so a
        // Cancel pressed during Scanning/start() would otherwise be lost and the
        // radio could stay in AP mode until the 5-min timeout.
        let mut cancel_rx = self.bus.commands.subscribe();

        // 1. Scan as STA.
        self.set(ProvisioningState::Scanning);
        let ssids = waved_wifi::wpa::scan().await.unwrap_or_default();

        // 2. Bring up AP.
        let passphrase = gen_passphrase(self.mac_seed);
        let ap = ap::ApController::new(&self.mac, passphrase.clone());
        if let Err(e) = ap.start().await {
            warn!("AP start failed: {e}");
            self.set(ProvisioningState::Failed { reason: "AP start failed".into() });
            let _ = ap.stop().await;
            return;
        }
        self.set(ProvisioningState::ApActive {
            ssid: ap.ssid.clone(),
            passphrase,
            url: format!("http://{}", ap::AP_IP),
        });

        // 3. Serve the form until creds arrive, timeout, or cancel.
        let creds = tokio::select! {
            c = self.serve(&ssids) => c,
            _ = tokio::time::sleep(PROV_TIMEOUT) => None,
            _ = wait_for_cancel(&mut cancel_rx) => None,
        };

        // 4. Tear down AP, restore STA.
        let _ = ap.stop().await;

        // 5. Connect (or report idle/failure).
        match creds {
            Some((ssid, psk)) => {
                self.set(ProvisioningState::Connecting { ssid: ssid.clone() });
                match waved_wifi::wpa::connect(&ssid, &psk).await {
                    Ok(()) => self.set(ProvisioningState::Connected { ssid }),
                    Err(e) => {
                        self.set(ProvisioningState::Failed { reason: format!("connect failed: {e}") });
                    }
                }
            }
            None => self.set(ProvisioningState::Idle),
        }
    }

    /// One-shot HTTP server on AP_IP:80. Returns the submitted (ssid, psk).
    async fn serve(&self, ssids: &[String]) -> Option<(String, String)> {
        let listener = match TcpListener::bind((ap::AP_IP, 80)).await {
            Ok(l) => l,
            Err(e) => {
                warn!("provisioning bind failed: {e}");
                return None;
            }
        };
        loop {
            let (mut sock, _) = listener.accept().await.ok()?;
            let mut buf = vec![0u8; 4096];
            let n = sock.read(&mut buf).await.ok()?;
            let raw = String::from_utf8_lossy(&buf[..n]).to_string();
            let req = match http::parse_request(&raw) {
                Some(r) => r,
                None => continue,
            };
            if req.method == "POST" && req.path == "/connect" {
                let form = http::parse_form(&req.body);
                let ssid = form.get("ssid").cloned().unwrap_or_default();
                let psk = form.get("psk").cloned().unwrap_or_default();
                let _ = sock.write_all(http::render_ack(&ssid).as_bytes()).await;
                let _ = sock.shutdown().await;
                if !ssid.is_empty() {
                    return Some((ssid, psk));
                }
            } else {
                let _ = sock.write_all(http::render_page(ssids).as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        }
    }

}

/// Resolve when a `CancelWifiProvisioning` command arrives on `rx` (or the
/// channel closes). Takes a pre-subscribed receiver so a cancel sent during the
/// scan/AP-bringup phase isn't missed.
async fn wait_for_cancel(rx: &mut broadcast::Receiver<Command>) {
    loop {
        match rx.recv().await {
            Ok(Command::CancelWifiProvisioning) => return,
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passphrase_is_8_chars_hex() {
        let p = gen_passphrase(0xA1B2C3D4);
        assert_eq!(p.len(), 8);
        assert!(p.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
