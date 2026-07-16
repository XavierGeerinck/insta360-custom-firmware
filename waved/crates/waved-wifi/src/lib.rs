pub mod wpa;

#[cfg(feature = "simulate")]
pub mod simulate;

use tokio::sync::broadcast;
use tracing::{debug, info, warn};
use waved_config::WavedConfig;
use waved_state::{Command, DeviceEvent, WifiState};

/// True when the connected SSID differs from the last observed one
/// (or this is the first connect). Drives disconnect+reconnect events
/// on roam without a full drop.
pub fn ssid_changed(last: Option<&str>, current: &str) -> bool {
    last != Some(current)
}

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

    pub async fn run(self, mut commands: broadcast::Receiver<Command>) {
        info!("wifi_srv started");
        if let Err(e) = wpa::ensure_station().await {
            warn!("station initialization failed: {e}");
        }
        let mut last_ssid: Option<String> = None;
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = tick.tick() => self.poll(&mut last_ssid).await,
                cmd = commands.recv() => {
                    if let Ok(cmd) = cmd { self.handle_command(cmd).await; }
                }
            }
        }
    }

    async fn poll(&self, last_ssid: &mut Option<String>) {
        if let Err(e) = wpa::ensure_station().await {
            warn!("station recovery failed: {e}");
        }
        let state = match wpa::status().await {
            Some((ssid, ip, signal)) => {
                if ssid_changed(last_ssid.as_deref(), &ssid) {
                    if last_ssid.is_some() {
                        let _ = self.events_tx.send(DeviceEvent::WifiDisconnected);
                    }
                    info!("wifi connected: {ssid} ({ip})");
                    let _ = self.events_tx.send(DeviceEvent::WifiConnected(ssid.clone()));
                    *last_ssid = Some(ssid.clone());
                    self.sync_ntp().await;
                }
                WifiState { connected: true, ssid, ip_addr: ip, signal_strength: signal }
            }
            None => {
                if last_ssid.is_some() {
                    info!("wifi disconnected");
                    let _ = self.events_tx.send(DeviceEvent::WifiDisconnected);
                    *last_ssid = None;
                }
                WifiState::default()
            }
        };
        let _ = self.wifi_tx.send(state);
    }

    async fn handle_command(&self, cmd: Command) {
        if let Command::WifiConnect { ssid, password } = cmd {
            match wpa::connect(&ssid, &password).await {
                Ok(()) => info!("wifi connect requested for {ssid}"),
                Err(e) => warn!("wifi connect failed: {e}"),
            }
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
