pub mod ap;
pub mod dhcp;
pub mod http;

use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::sync::watch;
use tracing::{info, warn};
use waved_state::{Command, ProvisioningState, StateBus};

const PROV_TIMEOUT: Duration = Duration::from_secs(300); // 5 min

/// Generate a new WPA2-compatible eight-hex passphrase for each AP session.
pub fn gen_passphrase() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

pub struct ProvisioningService {
    bus: Arc<StateBus>,
    mac: String,
}

impl ProvisioningService {
    pub fn new(bus: Arc<StateBus>, mac: String) -> Self {
        Self { bus, mac }
    }

    pub async fn run(self, mut commands: broadcast::Receiver<Command>) {
        info!("provision_srv started");
        loop {
            match commands.recv().await {
                Ok(Command::OpenWifiProvisioning) => {
                    self.provision().await;
                    // The primary receiver queues commands while provision()
                    // owns the service. Drop duplicate Open/Cancel messages so
                    // a tap during the active session cannot start another AP.
                    while commands.try_recv().is_ok() {}
                }
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
        if let Err(error) = waved_wifi::wpa::ensure_station().await {
            warn!("station preparation failed: {error}");
        }
        let mut ssids = waved_wifi::wpa::scan().await.unwrap_or_default();
        ssids.sort();
        ssids.dedup();

        if matches!(cancel_rx.try_recv(), Ok(Command::CancelWifiProvisioning)) {
            self.set(ProvisioningState::Idle);
            return;
        }

        // 2. Bring up AP.
        let passphrase = gen_passphrase();
        let ap = ap::ApController::new(&self.mac, passphrase.clone());
        if let Err(e) = ap.start().await {
            warn!("AP start failed: {e}");
            self.set(ProvisioningState::Failed {
                reason: "AP start failed".into(),
            });
            let _ = ap.stop().await;
            return;
        }

        let dhcp = match dhcp::Server::bind() {
            Ok(server) => server,
            Err(error) => {
                warn!("DHCP bind failed: {error}");
                let _ = ap.stop().await;
                self.set(ProvisioningState::Failed {
                    reason: "DHCP start failed".into(),
                });
                return;
            }
        };
        let (dhcp_shutdown, dhcp_rx) = watch::channel(false);
        let mut dhcp_task = tokio::spawn(dhcp.serve(dhcp_rx));
        self.set(ProvisioningState::ApActive {
            ssid: ap.ssid.clone(),
            passphrase,
            url: format!("http://{}", ap::AP_IP),
        });

        // 3. Serve the form until creds arrive, timeout, or cancel.
        enum Outcome {
            Credentials(String, String),
            Cancelled,
            Failed(String),
        }
        let outcome = tokio::select! {
            credentials = self.serve(&ssids) => match credentials {
                Some((ssid, psk)) => Outcome::Credentials(ssid, psk),
                None => Outcome::Failed("setup page stopped".into()),
            },
            _ = tokio::time::sleep(PROV_TIMEOUT) => Outcome::Cancelled,
            _ = wait_for_cancel(&mut cancel_rx) => Outcome::Cancelled,
            result = &mut dhcp_task => {
                let reason = match result {
                    Ok(Ok(())) => "DHCP server stopped".to_string(),
                    Ok(Err(error)) => format!("DHCP server failed: {error}"),
                    Err(error) => format!("DHCP task failed: {error}"),
                };
                Outcome::Failed(reason)
            },
        };

        // 4. Tear down AP, restore STA.
        let _ = dhcp_shutdown.send(true);
        if !dhcp_task.is_finished() {
            match dhcp_task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => warn!("DHCP shutdown failed: {error}"),
                Err(error) => warn!("DHCP task join failed: {error}"),
            }
        }
        if let Err(error) = ap.stop().await {
            warn!("AP cleanup/STA restore failed: {error}");
        }

        // 5. Connect (or report idle/failure).
        match outcome {
            Outcome::Credentials(ssid, psk) => {
                self.set(ProvisioningState::Connecting { ssid: ssid.clone() });
                match waved_wifi::wpa::connect(&ssid, &psk).await {
                    Ok(()) => {
                        match waved_wifi::wpa::wait_for_connection(&ssid, Duration::from_secs(45))
                            .await
                        {
                            Ok(_) => self.set(ProvisioningState::Connected { ssid }),
                            Err(error) => self.set(ProvisioningState::Failed {
                                reason: format!("connection timed out: {error}"),
                            }),
                        }
                    }
                    Err(e) => {
                        self.set(ProvisioningState::Failed {
                            reason: format!("connect failed: {e}"),
                        });
                    }
                }
            }
            Outcome::Cancelled => self.set(ProvisioningState::Idle),
            Outcome::Failed(reason) => {
                warn!("provisioning session failed: {reason}");
                self.set(ProvisioningState::Failed {
                    reason: "provisioning service failed".into(),
                });
            }
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
        let p = gen_passphrase();
        assert_eq!(p.len(), 8);
        assert!(p.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
