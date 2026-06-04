//! Simulated WiFi service — always connected, NTP always synced.
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tracing::info;
use waved_state::{DeviceEvent, WifiState};

pub struct SimWifiService {
    wifi_tx: Arc<watch::Sender<WifiState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl SimWifiService {
    pub fn new(wifi_tx: Arc<watch::Sender<WifiState>>, events_tx: broadcast::Sender<DeviceEvent>) -> Self {
        info!("[SIM] WiFi service created (always connected)");
        Self { wifi_tx, events_tx }
    }

    pub async fn run(self, _commands: tokio::sync::broadcast::Receiver<waved_state::Command>) {
        info!("[SIM] wifi_srv started");
        let state = WifiState {
            connected: true, ssid: "SimNetwork".to_string(),
            ip_addr: "192.168.1.100".to_string(), signal_strength: -45,
        };
        let _ = self.wifi_tx.send(state);
        let _ = self.events_tx.send(DeviceEvent::WifiConnected("SimNetwork".to_string()));
        info!("[SIM] WiFi connected to SimNetwork (192.168.1.100)");
        info!("[SIM] NTP sync: using system time");
        loop { tokio::time::sleep(std::time::Duration::from_secs(60)).await; }
    }
}
