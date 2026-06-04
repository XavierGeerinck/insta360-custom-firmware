pub mod wpa;

#[cfg(feature = "simulate")]
pub mod simulate;

use tokio::sync::broadcast;
use tracing::{debug, info, warn};
use waved_config::WavedConfig;
use waved_state::{DeviceEvent, WifiState};

pub struct WifiService {
    wifi_tx: std::sync::Arc<tokio::sync::watch::Sender<WifiState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
    ntp_servers: Vec<String>,
}

impl WifiService {
    pub fn new(
        wifi_tx: std::sync::Arc<tokio::sync::watch::Sender<WifiState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
        config: &WavedConfig,
    ) -> Self {
        Self {
            wifi_tx,
            events_tx,
            ntp_servers: config.ntp.servers.clone(),
        }
    }

    pub async fn run(self) {
        info!("wifi_srv started");
        let mut was_connected = false;

        loop {
            let state = match wpa::status().await {
                Some((ssid, ip, signal)) => {
                    if !was_connected {
                        info!("wifi connected: {ssid} ({ip})");
                        let _ = self.events_tx.send(DeviceEvent::WifiConnected(ssid.clone()));
                        was_connected = true;
                        // Errata #6: NTP sync on WiFi connect
                        self.sync_ntp().await;
                    }
                    WifiState {
                        connected: true,
                        ssid,
                        ip_addr: ip,
                        signal_strength: signal,
                    }
                }
                None => {
                    if was_connected {
                        info!("wifi disconnected");
                        let _ = self.events_tx.send(DeviceEvent::WifiDisconnected);
                        was_connected = false;
                    }
                    WifiState::default()
                }
            };
            let _ = self.wifi_tx.send(state);

            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    }

    async fn sync_ntp(&self) {
        for server in &self.ntp_servers {
            info!("attempting NTP sync with {server}");
            match tokio::process::Command::new("ntpd")
                .args(["-q", "-p", server])
                .status()
                .await
            {
                Ok(status) if status.success() => {
                    info!("NTP sync successful via {server}");
                    return;
                }
                Ok(status) => {
                    debug!("NTP sync via {server} failed with status: {status}");
                }
                Err(e) => {
                    warn!("NTP command failed for {server}: {e}");
                }
            }
        }
        warn!("NTP sync failed with all servers");
    }
}
