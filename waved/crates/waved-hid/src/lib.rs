#[cfg(feature = "simulate")]
pub mod simulate;

pub mod events;
pub mod report;
pub mod stats;

use std::path::Path;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
use waved_state::{ButtonType, DeviceEvent};

const HID_GADGET_PATH: &str = "/dev/hidg0";

// Stock-correct Report IDs (were swapped prior to SP#7(d')).
const REPORT_ID_CONSUMER:  u8 = 0x01;

// TODO(Task 5): replace these raw literals with `report::CONSUMER_*` constants
// and route through `report::event_to_press_release(button_to_event(b))`.
// Kept in this shape for now to land report.rs cleanly before the HidService
// refactor in Task 5.
fn build_hid_report(button: &ButtonType) -> (Vec<u8>, Vec<u8>) {
    // Stock Consumer page bit mapping:
    //   bit0 VolumeUp, bit1 VolumeDown, bit2 Mute.
    let press = match button {
        ButtonType::MuteToggle    => vec![REPORT_ID_CONSUMER, 0b0000_0100], // Mute
        ButtonType::MuteLongPress => vec![REPORT_ID_CONSUMER, 0b0000_0100], // Mute (PhoneMute deferred to watch.rs)
        ButtonType::VolumeUp      => vec![REPORT_ID_CONSUMER, 0b0000_0001],
        ButtonType::VolumeDown    => vec![REPORT_ID_CONSUMER, 0b0000_0010],
    };
    let release = vec![press[0], 0x00];
    (press, release)
}

pub struct HidService;

impl HidService {
    pub async fn run(mut events_rx: broadcast::Receiver<DeviceEvent>) {
        info!("hid_srv started");

        if !Path::new(HID_GADGET_PATH).exists() {
            warn!("{HID_GADGET_PATH} not found, HID disabled (not running on device?)");
            loop {
                match events_rx.recv().await {
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("hid_srv lagged by {n} events");
                    }
                }
            }
        }

        loop {
            match events_rx.recv().await {
                Ok(DeviceEvent::ButtonPress(btn)) => {
                    if let Err(e) = Self::send_hid_report(&btn).await {
                        error!("HID write error: {e}");
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("hid_srv lagged by {n} events");
                }
            }
        }
    }

    async fn send_hid_report(button: &ButtonType) -> std::io::Result<()> {
        let (report, release) = build_hid_report(button);

        debug!("sending HID report: {:02x?}", report);

        let mut file = OpenOptions::new()
            .write(true)
            .open(HID_GADGET_PATH)
            .await?;
        file.write_all(&report).await?;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        file.write_all(&release).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_toggle_report() {
        // Stock Consumer Mute = Report 0x01, bit2.
        let (press, release) = build_hid_report(&ButtonType::MuteToggle);
        assert_eq!(press, vec![0x01, 0b0000_0100]);
        assert_eq!(release, vec![0x01, 0x00]);
    }

    #[test]
    fn volume_up_report() {
        // Stock Consumer VolumeUp = Report 0x01, bit0 (was Report 0x02, bit0 pre-swap).
        let (press, release) = build_hid_report(&ButtonType::VolumeUp);
        assert_eq!(press, vec![0x01, 0b0000_0001]);
        assert_eq!(release, vec![0x01, 0x00]);
    }

    #[test]
    fn volume_down_report() {
        // Stock Consumer VolumeDown = Report 0x01, bit1 (was Report 0x02, bit1 pre-swap).
        let (press, release) = build_hid_report(&ButtonType::VolumeDown);
        assert_eq!(press, vec![0x01, 0b0000_0010]);
        assert_eq!(release, vec![0x01, 0x00]);
    }

    #[test]
    fn mute_long_press_report() {
        // Stock Consumer Mute = Report 0x01, bit2 (was Report 0x01, bit0 pre-swap).
        let (press, release) = build_hid_report(&ButtonType::MuteLongPress);
        assert_eq!(press, vec![0x01, 0b0000_0100]);
        assert_eq!(release, vec![0x01, 0x00]);
    }
}
