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
use tracing::{debug, info, trace, warn, Level};
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
    match log_line(event) {
        Some((Level::INFO, line)) => info!("{line}"),
        Some((Level::DEBUG, line)) => debug!("{line}"),
        Some((_, line)) => trace!("{line}"),
        None => {}
    }
}

/// The log line for a module event, and its level. No info or debug line
/// carries a phone's name, its MAC or a number: those reach the state (and
/// the screen) only. Module log lines (`0xF0`) can name the phone, so they
/// stay at trace. The phone book is never logged.
fn log_line(event: &Event) -> Option<(Level, String)> {
    let info = |line: String| Some((Level::INFO, line));
    let debug = |line: String| Some((Level::DEBUG, line));
    let trace = |line: String| Some((Level::TRACE, line));
    match event {
        Event::Version(v) if !v.is_empty() => info(format!("bt module firmware {v}")),
        Event::BuildTime(t) if !t.is_empty() => info(format!("bt module built {t}")),
        // The module's own advertised name, not a phone's.
        Event::Name(n) => info(format!("bt module name {n:?}")),
        Event::CallNumber(_) => info("bt: call ringing".into()),
        Event::CallCleared => info("bt: call number cleared".into()),
        Event::Phone(e) => info(format!("bt: phone event {e:?}")),
        Event::ConnectRequest { .. } => info("bt: a phone asks to connect".into()),
        Event::DeviceDisconnected { .. } => info("bt: a phone dropped".into()),
        Event::PhoneMac(_) => debug("bt: phone MAC received".into()),
        Event::WorkStatus { status, .. } => debug(format!("bt work status {:#04x}", status.0)),
        Event::Heartbeat { phone } => trace(format!(
            "bt heartbeat{}",
            if phone.is_some() { " naming the phone" } else { "" }
        )),
        Event::Log(line) => trace(format!("bt module log: {line}")),
        Event::PhoneBookEntry => None,
        Event::Other { cmd, data } => trace(format!("bt frame {cmd:#04x} {data:02x?}")),
        // The rest carries no phone's name, MAC or number: signal levels,
        // volumes, the dongle, versions, OTA and the module's own MAC.
        other @ (Event::Status { .. }
        | Event::LinkReport { .. }
        | Event::PhoneVolume { .. }
        | Event::ModuleInfo { .. }
        | Event::Version(_)
        | Event::BuildTime(_)
        | Event::AudioModeSwitched(_)
        | Event::DongleControl { .. }
        | Event::DongleVersion(_)
        | Event::DongleBuildTime(_)
        | Event::FlashSaved
        | Event::Done(_)
        | Event::Ota(_)) => debug(format!("bt: {other:?}")),
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

    mod logs {
        use super::super::log_line;
        use crate::event::{decode, Event, Link, Phone, PhoneEvent, WorkStatus};
        use crate::frame::Frame;
        use tracing::Level;

        const NAME: &str = "Xavier's iPhone";
        const NUMBER: &str = "+32470123456";
        const MAC: [u8; 6] = [0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0x57];

        /// Every way a MAC, a phone's name or a number could show up in a line.
        fn leaks(line: &str) -> Vec<String> {
            let hex_lower: Vec<String> = MAC.iter().map(|b| format!("{b:02x}")).collect();
            let hex_upper: Vec<String> = MAC.iter().map(|b| format!("{b:02X}")).collect();
            let decimal: Vec<String> = MAC.iter().map(|b| b.to_string()).collect();
            let needles = [
                NAME.to_string(),
                "iPhone".to_string(),
                NUMBER.to_string(),
                "470123456".to_string(),
                hex_lower.join(":"),
                hex_upper.join(":"),
                hex_lower.join(""),
                hex_upper.join(""),
                hex_upper.join(" "),
                decimal.join(", "),
                "ba:ad".to_string(),
                "BA AD".to_string(),
                "186, 173".to_string(),
            ];
            needles.into_iter().filter(|n| line.contains(n.as_str())).collect()
        }

        fn phone() -> Option<Phone> {
            Some(Phone { name: NAME.into(), mac: Some(MAC) })
        }

        /// Events that carry a phone's name, MAC or a number.
        fn personal() -> Vec<Event> {
            let mut log = b"\x01app_set_bt_last_addr A1B2C3D4E557 name ".to_vec();
            log.extend_from_slice(NAME.as_bytes());
            vec![
                Event::WorkStatus { status: WorkStatus(0x01), phone: phone() },
                Event::Heartbeat { phone: phone() },
                Event::CallNumber(NUMBER.into()),
                Event::ConnectRequest { mac: MAC, name: NAME.into() },
                Event::DeviceDisconnected { mac: MAC },
                Event::PhoneMac(MAC),
                decode(&Frame::request(0xF0, &log)),
                Event::Other { cmd: 0x42, data: NUMBER.as_bytes().to_vec() },
            ]
        }

        #[test]
        fn no_info_or_debug_line_names_a_phone_or_a_number() {
            for event in personal() {
                if let Some((level, line)) = log_line(&event) {
                    if level != Level::TRACE {
                        assert!(leaks(&line).is_empty(), "{level} {line:?} leaks {:?}", leaks(&line));
                    }
                }
            }
        }

        #[test]
        fn module_logs_are_trace_only() {
            let line = decode(&Frame::request(0xF0, b"\x01BT set pairing mode by user"));
            assert_eq!(log_line(&line).map(|(level, _)| level), Some(Level::TRACE));
            let raw = decode(&Frame::request(0xF0, &[0x02, 0xE4, 0x7F]));
            assert_eq!(log_line(&raw).map(|(level, _)| level), Some(Level::TRACE));
        }

        #[test]
        fn the_phone_book_is_never_logged() {
            let entry = decode(&Frame::request(0xB0, b"\x01\0\0\0\x06+32470\x03A\0n\0n\0"));
            assert_eq!(entry, Event::PhoneBookEntry);
            assert_eq!(log_line(&entry), None);
        }

        #[test]
        fn calls_and_connections_still_show_at_info() {
            let level = |e: Event| log_line(&e).map(|(level, _)| level);
            assert_eq!(level(Event::CallNumber(NUMBER.into())), Some(Level::INFO));
            assert_eq!(level(Event::CallCleared), Some(Level::INFO));
            assert_eq!(level(Event::Phone(PhoneEvent::CallActive)), Some(Level::INFO));
            assert_eq!(level(Event::ConnectRequest { mac: MAC, name: NAME.into() }), Some(Level::INFO));
            assert_eq!(level(Event::DeviceDisconnected { mac: MAC }), Some(Level::INFO));
            assert_eq!(level(Event::Version("0.0.0.28".into())), Some(Level::INFO));
            // The module's own name is no phone's: it may show.
            let (_, line) = log_line(&Event::Name("Insta360 Wave BT 0A1B2C".into())).unwrap();
            assert!(line.contains("Insta360 Wave BT 0A1B2C"));
            // Link reports carry signal levels only.
            let report = Event::LinkReport { dongle: Link::Absent, phone: Link::Connected(-48), counters: None };
            assert_eq!(level(report), Some(Level::DEBUG));
        }
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
