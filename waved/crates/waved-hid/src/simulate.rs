//! Simulated HID service — logs reports to tracing.
use tokio::sync::broadcast;
use tracing::{info, warn};
use waved_state::{ButtonType, DeviceEvent};

pub struct SimHidService;

impl SimHidService {
    pub async fn run(mut events_rx: broadcast::Receiver<DeviceEvent>) {
        info!("[SIM] hid_srv started (logging reports)");
        loop {
            match events_rx.recv().await {
                Ok(DeviceEvent::ButtonPress(btn)) => {
                    let desc = match btn {
                        ButtonType::MuteToggle => "TELEPHONE:mute_toggle",
                        ButtonType::VolumeUp => "CONSUMER:volume_up",
                        ButtonType::VolumeDown => "CONSUMER:volume_down",
                        ButtonType::MuteLongPress => "TELEPHONE:mute_long",
                    };
                    info!("[SIM] HID report: {desc} (press + release)");
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(n)) => { warn!("[SIM] hid_srv lagged by {n} events"); }
            }
        }
    }
}
