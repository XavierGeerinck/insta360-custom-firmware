use std::path::Path;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
use waved_state::{ButtonType, DeviceEvent};

const HID_GADGET_PATH: &str = "/dev/hidg0";

const REPORT_ID_TELEPHONE: u8 = 0x01;
const REPORT_ID_CONSUMER: u8 = 0x02;

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
        let report = match button {
            ButtonType::MuteToggle => {
                vec![REPORT_ID_TELEPHONE, 0x01]
            }
            ButtonType::VolumeUp => {
                vec![REPORT_ID_CONSUMER, 0x01]
            }
            ButtonType::VolumeDown => {
                vec![REPORT_ID_CONSUMER, 0x02]
            }
            ButtonType::MuteLongPress => {
                vec![REPORT_ID_TELEPHONE, 0x01]
            }
        };

        debug!("sending HID report: {:02x?}", report);

        let mut file = OpenOptions::new()
            .write(true)
            .open(HID_GADGET_PATH)
            .await?;
        file.write_all(&report).await?;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let release = vec![report[0], 0x00];
        file.write_all(&release).await?;

        Ok(())
    }
}
