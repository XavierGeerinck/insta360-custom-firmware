//! The Bluetooth module on `/dev/ttyS1` (a Telink SoC running its own
//! Bluetooth stack): docs/BLUETOOTH_PROTOCOL.md.
//!
//! - [`frame`]: `75 45|46 LEN CMD DATA SUM` framing and the byte parser.
//! - [`command`]: what the host sends, byte-exact with stock.
//! - [`event`]: what the module sends, decoded.
//! - [`state`]: events folded into `BluetoothState`.
//! - [`ota`]: `BT.bin` / `DONGLE.bin` updates (TODO F6).
//!
//! [`BtService`] is deliberately passive: it asks read-only questions, answers
//! every frame with stock's acknowledgement and keeps `BluetoothState`. It
//! does not switch Bluetooth on, rename the module or save its flash; that is
//! I1, after a device session (doc, "First device session").

pub mod command;
pub mod event;
pub mod frame;
pub mod ota;
pub mod state;

#[cfg(feature = "simulate")]
pub mod simulate;

use command::{ack_for, Command};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use event::Event;
use frame::{Frame, Prefix, PREFIX_ACK, PREFIX_LEGACY, PREFIX_REQUEST};
use state::{Applied, Tracker};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, trace, warn};
use waved_state::{BluetoothState, DeviceEvent};
use waved_uart::UartMux;

/// The module's UART and speed (stock `g_uart_bt_device`, `bttool`).
pub const BT_PORT: &str = "/dev/ttyS1";
pub const BT_BAUD: u32 = 921_600;

/// Stock polls the module every 5 s (`bt_heartbeat_thread`).
const STATUS_POLL: Duration = Duration::from_secs(5);

/// Read-only questions asked once at start.
const STARTUP: [Command; 5] = [
    Command::GetVersion,
    Command::GetBuildTime,
    Command::GetStatus,
    Command::GetName,
    Command::GetPhoneMac,
];

/// A frame from the `waved-uart` mux as a codec frame (`data` there holds
/// `CMD DATA`). `None` for an empty frame or a header this port never carries.
pub fn from_uart(raw: &waved_uart::Frame) -> Option<Frame> {
    let prefix = match raw.header {
        PREFIX_REQUEST => Prefix::Request,
        PREFIX_ACK => Prefix::Ack,
        PREFIX_LEGACY => Prefix::Legacy,
        _ => return None,
    };
    let (&cmd, data) = raw.data.split_first()?;
    Some(Frame { prefix, cmd, data: data.to_vec() })
}

pub struct BtService {
    bt_rx: Receiver<waved_uart::Frame>,
    uart: Arc<UartMux>,
    bt_tx: Arc<watch::Sender<BluetoothState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl BtService {
    pub fn new(
        bt_rx: Receiver<waved_uart::Frame>,
        uart: Arc<UartMux>,
        bt_tx: Arc<watch::Sender<BluetoothState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        Self { bt_rx, uart, bt_tx, events_tx }
    }

    pub fn run(self) {
        info!("bt_srv started");
        for c in &STARTUP {
            self.send(&c.frame());
        }

        let mut tracker = Tracker::default();
        // Poll only once the module has answered: until then the line may be
        // at the wrong speed and every byte we send is noise to it.
        let mut heard = false;
        loop {
            match self.bt_rx.recv_timeout(STATUS_POLL) {
                Ok(raw) => {
                    heard = true;
                    self.handle(&raw, &mut tracker);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if heard {
                        self.send(&Command::GetStatus.frame());
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    warn!("bt channel closed, stopping");
                    return;
                }
            }
        }
    }

    fn send(&self, frame: &Frame) {
        self.uart.send(frame.encode(), false);
    }

    fn handle(&self, raw: &waved_uart::Frame, tracker: &mut Tracker) {
        let Some(frame) = from_uart(raw) else { return };
        if let Some(ack) = ack_for(frame.cmd) {
            self.send(&ack);
        }
        let event = event::decode(&frame);
        log_event(&event);

        let mut applied = Applied::default();
        self.bt_tx.send_if_modified(|s| {
            applied = tracker.apply(&event, s);
            applied.changed
        });
        if let Some(e) = applied.event {
            let _ = self.events_tx.send(e);
        }
    }
}

fn log_event(event: &Event) {
    match event {
        Event::Version(v) if !v.is_empty() => info!("bt module firmware {v}"),
        Event::BuildTime(t) if !t.is_empty() => info!("bt module built {t}"),
        Event::Name(n) => info!("bt module name {n:?}"),
        // Never log the caller's number.
        Event::CallNumber(_) => info!("bt: call ringing"),
        Event::CallCleared => info!("bt: call number cleared"),
        Event::Phone(e) => info!("bt: phone event {e:?}"),
        Event::ConnectRequest { name, .. } => info!("bt: {name:?} asks to connect"),
        Event::WorkStatus { status, .. } => debug!("bt work status {:#04x}", status.0),
        Event::Log(line) => debug!("bt module log: {line}"),
        Event::Other { cmd, data } => trace!("bt frame {cmd:#04x} {data:02x?}"),
        other => debug!("bt: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uart(header: [u8; 2], data: &[u8]) -> waved_uart::Frame {
        waved_uart::Frame {
            header,
            target: waved_uart::frame::FrameTarget::Bluetooth,
            data: data.to_vec(),
        }
    }

    #[test]
    fn converts_mux_frames() {
        assert_eq!(
            from_uart(&uart(PREFIX_REQUEST, &[0xAA, 0x01])),
            Some(Frame::request(0xAA, &[0x01]))
        );
        assert_eq!(from_uart(&uart(PREFIX_ACK, &[0xA9])), Some(Frame::ack(0xA9, &[])));
        assert_eq!(from_uart(&uart(PREFIX_LEGACY, &[0x10])).map(|f| f.prefix), Some(Prefix::Legacy));
        assert_eq!(from_uart(&uart(PREFIX_REQUEST, &[])), None);
        assert_eq!(from_uart(&uart([0x00, 0x00], &[0xAA])), None);
    }

    #[test]
    fn mux_and_codec_agree_on_the_wire() {
        // A stock frame must survive the waved-uart parser unchanged.
        let raw = Command::Heartbeat { battery_percent: 85, charging: true, wifi: true }.encode();
        let mut parser = waved_uart::FrameParser::new();
        let parsed: Vec<_> = raw.iter().filter_map(|&b| parser.feed(b)).collect();
        assert_eq!(parsed.len(), 1);
        let frame = from_uart(parsed[0].as_ref().expect("checksum as stock")).unwrap();
        assert_eq!(frame, Frame::request(0xB3, &[7, 1, 1]));
    }

    #[test]
    fn startup_only_asks() {
        // Nothing that changes the module: no open/close, name, pairing or flash.
        for c in &STARTUP {
            assert!(
                matches!(
                    c,
                    Command::GetVersion
                        | Command::GetBuildTime
                        | Command::GetStatus
                        | Command::GetName
                        | Command::GetPhoneMac
                ),
                "{c:?}"
            );
        }
    }
}
