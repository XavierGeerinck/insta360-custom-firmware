pub mod ntp;
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
        let mut clock_synchronized = false;
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = tick.tick() => self.poll(&mut last_ssid, &mut clock_synchronized).await,
                cmd = commands.recv() => {
                    if let Ok(cmd) = cmd { self.handle_command(cmd).await; }
                }
            }
        }
    }

    async fn poll(&self, last_ssid: &mut Option<String>, clock_synchronized: &mut bool) {
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
                    let _ = self
                        .events_tx
                        .send(DeviceEvent::WifiConnected(ssid.clone()));
                    *last_ssid = Some(ssid.clone());
                    *clock_synchronized = self.sync_ntp().await;
                }
                WifiState {
                    connected: true,
                    ssid,
                    ip_addr: ip,
                    signal_strength: signal,
                    clock_synchronized: *clock_synchronized,
                }
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

    async fn sync_ntp(&self) -> bool {
        for server in &self.ntp_servers {
            info!("attempting NTP sync with {server}");
            match ntp::synchronize(server).await {
                Ok(()) => {
                    info!("NTP sync successful via {server}");
                    return true;
                }
                Err(error) => {
                    debug!("NTP sync via {server} failed: {error}");
                }
            }
        }
        warn!("NTP sync failed with all servers");
        false
    }
}
