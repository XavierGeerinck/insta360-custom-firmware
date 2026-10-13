//! The Bluetooth module on `/dev/ttyS1` (a Telink SoC running its own
//! Bluetooth stack): docs/BLUETOOTH_PROTOCOL.md.
//!
//! - [`frame`]: `75 45|46 LEN CMD DATA SUM` framing and the byte parser.
//! - [`command`]: what the host sends, byte-exact with stock.
//! - [`event`]: what the module sends, decoded.
//! - [`state`]: events folded into `BluetoothState`, the call included.
//! - [`control`]: bus commands (pair, disconnect, answer, decline) and the
//!   2-minute pairing window.
//! - [`ota`]: `BT.bin` / `DONGLE.bin` updates (TODO F6).
//!
//! [`BtService`] asks read-only questions at start, answers every frame the
//! module repeats (`75 45`) with stock's acknowledgement and keeps
//! `BluetoothState`. Given the bus (`with_commands`) it pairs, stops pairing,
//! disconnects and answers or declines calls. It does not switch Bluetooth on
//! or off, rename the module or save its flash.
//!
//! Privacy: the phone book (`0xB0`) is dropped unread, and no log line at
//! any level carries a phone's name, MAC or a number (`log_line`): module log
//! lines show only as known fixed messages, undecoded frames as their length.

pub mod command;
pub mod control;
pub mod event;
pub mod frame;
pub mod ota;
pub mod state;

#[cfg(feature = "simulate")]
pub mod simulate;

use command::{ack_for_frame, Command};
use control::{Control, Response};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use event::Event;
use frame::{Frame, Prefix, PREFIX_ACK, PREFIX_LEGACY, PREFIX_REQUEST};
use state::Tracker;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, trace, warn, Level};
use waved_state::{BluetoothState, Command as BusCommand, DeviceEvent};
use waved_uart::UartMux;

/// The module's UART and speed (stock `g_uart_bt_device`, `bttool`).
pub const BT_PORT: &str = "/dev/ttyS1";
pub const BT_BAUD: u32 = 921_600;

/// Stock polls the module every 5 s (`bt_heartbeat_thread`).
const STATUS_POLL: Duration = Duration::from_secs(5);

/// How often the loop looks at the bus and the pairing countdown when the
/// module is quiet.
const TICK: Duration = Duration::from_millis(100);

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

/// The service without the UART or the clock: module frames and bus
/// commands in, frames to send and state changes out.
#[derive(Debug, Default)]
struct Core {
    tracker: Tracker,
    control: Control,
    seen: Seen,
}

/// What one module frame asks of the service.
#[derive(Debug, Default)]
struct Handled {
    /// Stock's acknowledgement, for a `75 45` frame other than `0xE0`.
    acks: Vec<Frame>,
    /// `BluetoothState` changed (notify watchers).
    changed: bool,
    /// A connect / disconnect to broadcast.
    event: Option<DeviceEvent>,
    /// The log line and its level: info only when the frame told us
    /// something new, so a frame the module repeats never repeats at info.
    log: Option<(Level, String)>,
}

/// What the module said that `BluetoothState` does not keep.
#[derive(Debug, Default)]
struct Seen {
    version: Option<String>,
    build_time: Option<String>,
}

impl Seen {
    /// Whether `event` brings a version or build time not seen before.
    fn news(&mut self, event: &Event) -> bool {
        let (slot, value) = match event {
            Event::Version(v) => (&mut self.version, v),
            Event::BuildTime(t) => (&mut self.build_time, t),
            _ => return false,
        };
        if slot.as_ref() == Some(value) {
            return false;
        }
        *slot = Some(value.clone());
        true
    }
}

impl Core {
    /// A module frame: its acknowledgement ([`command::ack_for_frame`]),
    /// whether `s` changed, a connect / disconnect to broadcast and the log
    /// line.
    fn frame(&mut self, frame: &Frame, s: &mut BluetoothState) -> Handled {
        let acks = ack_for_frame(frame).into_iter().collect();
        let event = event::decode(frame);
        let applied = self.tracker.apply(&event, s);
        let window = self.control.after_event(s);
        let news = applied.changed || applied.event.is_some() || self.seen.news(&event);
        let log = log_line(&event).map(|(level, line)| match level {
            Level::INFO if !news => (Level::DEBUG, line),
            _ => (level, line),
        });
        Handled { acks, changed: applied.changed || window, event: applied.event, log }
    }

    fn command(&mut self, command: &BusCommand, now_ms: u64, unix_time: u32, s: &mut BluetoothState) -> Response {
        self.control.command(command, now_ms, unix_time, s)
    }

    fn tick(&mut self, now_ms: u64, s: &mut BluetoothState) -> bool {
        self.control.tick(now_ms, s)
    }
}

pub struct BtService {
    bt_rx: Receiver<waved_uart::Frame>,
    uart: Arc<UartMux>,
    bt_tx: Arc<watch::Sender<BluetoothState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
    commands: Option<broadcast::Receiver<BusCommand>>,
}

impl BtService {
    pub fn new(
        bt_rx: Receiver<waved_uart::Frame>,
        uart: Arc<UartMux>,
        bt_tx: Arc<watch::Sender<BluetoothState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        Self { bt_rx, uart, bt_tx, events_tx, commands: None }
    }

    /// Take `BtPair`, `BtStopPairing`, `BtDisconnect`, `BtAnswerCall` and
    /// `BtDeclineCall` from the bus. Without it the service stays passive.
    pub fn with_commands(mut self, commands: broadcast::Receiver<BusCommand>) -> Self {
        self.commands = Some(commands);
        self
    }

    pub fn run(mut self) {
        info!("bt_srv started");
        for c in &STARTUP {
            self.send(&c.frame());
        }

        let start = Instant::now();
        let now_ms = || start.elapsed().as_millis() as u64;
        let mut core = Core::default();
        let mut next_poll = STATUS_POLL;
        // Poll only once the module has answered: until then the line may be
        // at the wrong speed and every byte we send is noise to it.
        let mut heard = false;
        loop {
            match self.bt_rx.recv_timeout(TICK) {
                Ok(raw) => {
                    heard = true;
                    self.handle(&raw, &mut core);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    warn!("bt channel closed, stopping");
                    return;
                }
            }
            while let Some(command) = self.next_command() {
                let (now, unix) = (now_ms(), unix_time());
                let mut out = Response::default();
                self.bt_tx.send_if_modified(|s| {
                    out = core.command(&command, now, unix, s);
                    out.changed
                });
                if out.acted() {
                    info!("bt: {command:?}");
                }
                for c in &out.frames {
                    self.send(&c.frame());
                }
            }
            let now = now_ms();
            self.bt_tx.send_if_modified(|s| core.tick(now, s));
            if heard && now >= next_poll.as_millis() as u64 {
                self.send(&Command::GetStatus.frame());
                next_poll = Duration::from_millis(now) + STATUS_POLL;
            }
        }
    }

    /// The next bus command, if any; `None` once the bus is gone too.
    fn next_command(&mut self) -> Option<BusCommand> {
        let commands = self.commands.as_mut()?;
        loop {
            match commands.try_recv() {
                Ok(command) => return Some(command),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Empty) => return None,
                Err(broadcast::error::TryRecvError::Closed) => {
                    self.commands = None;
                    return None;
                }
            }
        }
    }

    fn send(&self, frame: &Frame) {
        self.uart.send(frame.encode(), false);
    }

    fn handle(&self, raw: &waved_uart::Frame, core: &mut Core) {
        let Some(frame) = from_uart(raw) else { return };
        let mut out = Handled::default();
        self.bt_tx.send_if_modified(|s| {
            out = core.frame(&frame, s);
            out.changed
        });
        for ack in &out.acks {
            self.send(ack);
        }
        log_event(out.log);
        if let Some(e) = out.event {
            let _ = self.events_tx.send(e);
        }
    }
}

/// `time(NULL)` for button events, as stock stamps them.
fn unix_time() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

fn log_event(line: Option<(Level, String)>) {
    match line {
        Some((Level::INFO, line)) => info!("{line}"),
        Some((Level::DEBUG, line)) => debug!("{line}"),
        Some((_, line)) => trace!("{line}"),
        None => {}
    }
}

/// The module's own fixed messages about pairing, reconnecting and its radio
/// (strings in its firmware, 0.0.0.28 and 0.0.0.33). A module log line shows
/// only as the message it starts with, never as received: the rest of a line
/// can carry the phone's address (`a2dp_snk_disconnect_req addr …`), and
/// other lines its name and MAC (`app_set_bt_last_addr`).
const MODULE_MESSAGES: &[&str] = &[
    "BT set pairing mode by user",
    "Warning! bt set pairing but bt has been connected",
    "Warning! bt set pairing while bt is reconnecting",
    "Warning! bt is reconnecting",
    "time out ,app_bt_pairing_exit",
    "app_exit_pairing_handle",
    "RECONN_START!",
    "RECONN_CANCEL!",
    "RECONN_ERROR!",
    "BT_RECONN_TIMEOUT!",
    "BT_RECONN_ADDR_ERROR!",
    "bt stop connback",
    "app_bt_start_connback_func",
    "Connback time out !",
    "Connback error! Page scan",
    "bt rf close done",
    "bt rf close",
    "bt rf open done",
    "bt rf open",
    "system_force_disconnect",
    "app_bt_disconnect error",
    "a2dp_snk_disconnect_req",
    "ACL DISCONN CMP, reason",
    "app_bt_clean_last_dev",
    "bt clean info",
    "set pairred_mode_switch3",
    "delete pairred_mode_switch3",
];

/// A module log line (`0xF0`): a known message at debug, else its length at
/// trace (see [`MODULE_MESSAGES`]).
fn module_log(line: &str) -> (Level, String) {
    let line = line.trim_start_matches(|c: char| c == '@' || c == '#' || c.is_whitespace());
    let known = MODULE_MESSAGES
        .iter()
        .filter(|message| line.starts_with(**message))
        .max_by_key(|message| message.len());
    match known {
        Some(message) => (Level::DEBUG, format!("bt module: {message}")),
        None => (Level::TRACE, format!("bt module log line, {} bytes", line.len())),
    }
}

/// The log line for a module event, and its level. No line at any level
/// carries a phone's name, its MAC or a number: those reach the state (and
/// the screen) only. Module log lines (`0xF0`) show only as known messages
/// ([`MODULE_MESSAGES`]); undecoded frames only as their length. The phone
/// book is never logged.
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
        Event::Log(line) => Some(module_log(line)),
        Event::PhoneBookEntry => None,
        Event::Other { cmd, data } => trace(format!("bt frame {cmd:#04x}, {} bytes", data.len())),
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
        fn no_line_at_any_level_names_a_phone_or_a_number() {
            let module_line = |text: &str| {
                let mut data = vec![0x01];
                data.extend_from_slice(text.as_bytes());
                decode(&Frame::request(0xF0, &data))
            };
            let mut events = personal();
            events.extend([
                module_line(&format!("@ app_set_bt_last_addr A1B2C3D4E557 name {NAME}")),
                // A known message followed by the phone's address: only the
                // message may show.
                module_line("a2dp_snk_disconnect_req addr A1B2C3D4E557"),
                module_line(&format!("RECONN_START! {NAME}")),
                decode(&Frame::request(0xF0, &[0x02, 0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0x57])),
                Event::Other { cmd: 0xC8, data: vec![0x00, 0x06, 0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0x57] },
            ]);
            for event in events {
                if let Some((level, line)) = log_line(&event) {
                    assert!(leaks(&line).is_empty(), "{level} {line:?} leaks {:?}", leaks(&line));
                }
            }
        }

        #[test]
        fn module_messages_about_pairing_and_reconnecting_show_at_debug() {
            let module_line = |text: &str| {
                let mut data = vec![0x01];
                data.extend_from_slice(text.as_bytes());
                log_line(&decode(&Frame::request(0xF0, &data)))
            };
            let debug = |text: &str| Some((Level::DEBUG, format!("bt module: {text}")));
            assert_eq!(module_line("BT set pairing mode by user"), debug("BT set pairing mode by user"));
            assert_eq!(module_line("RECONN_START!"), debug("RECONN_START!"));
            assert_eq!(module_line("@ bt stop connback\r\n"), debug("bt stop connback"));
            // The longest match wins.
            assert_eq!(module_line("bt rf close done"), debug("bt rf close done"));
            assert_eq!(module_line("bt rf close"), debug("bt rf close"));
            assert_eq!(module_line("time out ,app_bt_pairing_exit "), debug("time out ,app_bt_pairing_exit"));
            // Anything else: its length at trace, never its text.
            assert_eq!(
                module_line("@ stack_overflow_check_init 0x1234"),
                Some((Level::TRACE, "bt module log line, 32 bytes".into()))
            );
            let raw = decode(&Frame::request(0xF0, &[0x02, 0xE4, 0x7F]));
            assert_eq!(log_line(&raw).map(|(level, _)| level), Some(Level::TRACE));
        }

        #[test]
        fn an_undecoded_frame_shows_its_length_only() {
            let other = Event::Other { cmd: 0x42, data: NUMBER.as_bytes().to_vec() };
            assert_eq!(log_line(&other), Some((Level::TRACE, "bt frame 0x42, 12 bytes".into())));
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

    mod core {
        use super::super::Core;
        use crate::frame::{Frame, Parser};
        use tracing::Level;
        use waved_state::{BluetoothState, BtCall, Command as BusCommand, DeviceEvent};

        fn wire(bytes: &[u8]) -> Frame {
            let mut p = Parser::default();
            let frames: Vec<Frame> = bytes.iter().filter_map(|&b| p.feed(b)).collect();
            assert_eq!(frames.len(), 1);
            frames.into_iter().next().unwrap()
        }

        fn hex(frames: &[Frame]) -> Vec<String> {
            frames
                .iter()
                .map(|f| f.encode().iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "))
                .collect()
        }

        #[test]
        fn pairing_a_phone_from_the_bus_to_the_state() {
            let (mut core, mut s) = (Core::default(), BluetoothState::default());
            let out = core.command(&BusCommand::BtPair, 0, 0, &mut s);
            assert_eq!(hex(&out.frames.iter().map(|c| c.frame()).collect::<Vec<_>>()), [
                "75 45 02 D8 00 DA",
                "75 45 02 A3 00 A5",
                "75 45 02 A7 00 A9"
            ]);
            assert_eq!(s.pairing_left_s, 120);

            // The module: pairing mode (seen on the device), acknowledged.
            let h = core.frame(&wire(&[0x75, 0x45, 0x03, 0xB2, 0x08, 0x00, 0xBD]), &mut s);
            assert_eq!(hex(&h.acks), ["75 46 02 B2 00 B4"]);
            assert!(h.changed && s.pairing && s.pairing_left_s == 120);
            assert_eq!(h.event, None);

            // A phone connects: the window closes.
            core.tick(30_000, &mut s);
            assert_eq!(s.pairing_left_s, 90);
            let h = core.frame(&wire(&[0x75, 0x45, 0x03, 0xB2, 0x01, 0x00, 0xB6]), &mut s);
            assert!(h.changed && s.connected && !s.pairing);
            assert_eq!(s.pairing_left_s, 0);
            assert_eq!(h.event, Some(DeviceEvent::BtConnected(String::new())));
        }

        #[test]
        fn leaving_the_pairing_screen_ends_the_window() {
            let (mut core, mut s) = (Core::default(), BluetoothState::default());
            core.command(&BusCommand::BtPair, 0, 0, &mut s);
            core.frame(&wire(&[0x75, 0x45, 0x03, 0xB2, 0x08, 0x00, 0xBD]), &mut s);
            let out = core.command(&BusCommand::BtStopPairing, 5_000, 0, &mut s);
            assert!(out.frames.is_empty(), "no command leaves pairing mode");
            assert!(out.changed);
            assert_eq!(s.pairing_left_s, 0);
            assert!(s.pairing, "the module's own bit 3 stays until it says otherwise");
            assert!(!core.tick(6_000, &mut s));
        }

        #[test]
        fn a_call_from_the_module_to_answer() {
            let (mut core, mut s) = (Core::default(), BluetoothState::default());
            let ring = [0x75, 0x45, 0x0C, 0xB1, 0x2B, 0x33, 0x32, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0xF1];
            let h = core.frame(&wire(&ring), &mut s);
            assert_eq!(hex(&h.acks), ["75 46 02 B1 00 B3"]);
            assert!(h.changed);
            assert_eq!(s.call, BtCall::Ringing { number: "+3212345678".into() });
            let out = core.command(&BusCommand::BtAnswerCall, 0, 0x6720_0000, &mut s);
            assert_eq!(hex(&out.frames.iter().map(|c| c.frame()).collect::<Vec<_>>()), [
                "75 45 07 D0 03 00 00 20 67 00 61"
            ]);
            let h = core.frame(&wire(&[0x75, 0x45, 0x02, 0xD1, 0x00, 0xD3]), &mut s);
            assert_eq!(hex(&h.acks), ["75 46 01 D1 D2"]);
            assert!(s.call.is_active());
        }

        #[test]
        fn the_phone_book_is_acknowledged_and_changes_nothing() {
            let (mut core, mut s) = (Core::default(), BluetoothState::default());
            let entry = Frame::request(0xB0, b"\x01\0\0\0\x06+32470\x03A\0n\0n\0");
            let h = core.frame(&entry, &mut s);
            assert_eq!(hex(&h.acks), ["75 46 02 B0 00 B2"]);
            assert!(!h.changed);
            assert_eq!(h.event, None);
            assert_eq!(h.log, None);
        }

        fn name_frame(name: &str) -> Frame {
            let mut data = vec![0x00];
            data.extend_from_slice(name.as_bytes());
            Frame::request(0xC6, &data)
        }

        #[test]
        fn replies_to_our_requests_are_not_acknowledged() {
            // Capture s1-pair-20261013.bin: the module answers GetStatus and
            // GetName with `75 46 A9 00` / `75 46 C5 00` once each.
            let (mut core, mut s) = (Core::default(), BluetoothState::default());
            for echo in [[0x75, 0x46, 0x02, 0xA9, 0x00, 0xAB], [0x75, 0x46, 0x02, 0xC5, 0x00, 0xC7]] {
                let h = core.frame(&wire(&echo), &mut s);
                assert!(h.acks.is_empty(), "{echo:02X?}");
                assert!(!h.changed);
            }
            // The version comes back in the reply itself: decoded, not acknowledged.
            let h = core.frame(&Frame::ack(0x91, b"\x000.0.0.28"), &mut s);
            assert!(h.acks.is_empty());
            assert_eq!(h.log.map(|(_, line)| line), Some("bt module firmware 0.0.0.28".into()));
            // The module's own frames still are, the name and the status included.
            let h = core.frame(&name_frame("Insta360 Wave BT 0A1B2C"), &mut s);
            assert_eq!(hex(&h.acks), ["75 46 02 C6 00 C8"]);
            let h = core.frame(&wire(&[0x75, 0x45, 0x02, 0xAA, 0x01, 0xAD]), &mut s);
            assert_eq!(hex(&h.acks), ["75 46 02 AA 00 AC"]);
            assert!(core.frame(&Frame::request(0xE0, &[]), &mut s).acks.is_empty());
        }

        #[test]
        fn the_module_name_shows_at_info_only_when_it_changes() {
            let (mut core, mut s) = (Core::default(), BluetoothState::default());
            let level = |core: &mut Core, s: &mut BluetoothState, f: &Frame| core.frame(f, s).log.map(|(l, _)| l);
            let name = name_frame("Insta360 Wave BT 0A1B2C");
            assert_eq!(level(&mut core, &mut s, &name), Some(Level::INFO));
            for _ in 0..4 {
                assert_eq!(level(&mut core, &mut s, &name), Some(Level::DEBUG), "a repeat");
            }
            let renamed = name_frame("Wave Boardroom");
            assert_eq!(level(&mut core, &mut s, &renamed), Some(Level::INFO));
        }

        #[test]
        fn nothing_at_info_repeats_per_frame() {
            // Every frame that logs at info, sent twice in a row as a module
            // that is not acknowledged sends it: the second copy is not info.
            let ring = b"+32470123456".to_vec();
            let mut connected = vec![0x01, 0x00, 0x05];
            connected.extend_from_slice(b"Pixel");
            connected.extend_from_slice(&[0x00, 0x06, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
            let mut request = vec![0x06, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x05];
            request.extend_from_slice(b"Pixel");
            let frames = [
                Frame::ack(0x91, b"\x000.0.0.28"),
                Frame::ack(0x93, b"\x002025-11-28 20:15:49"),
                name_frame("Insta360 Wave BT 0A1B2C"),
                Frame::request(0xB2, &connected),
                Frame::request(0xB1, &ring),
                Frame::request(0xD1, &[0x00]),
                Frame::request(0xD1, &[0x01]),
                Frame::request(0xB1, &[]),
                Frame::request(0xD6, &request),
                Frame::request(0xD1, &[0x02]),
                Frame::request(0xE1, &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66]),
            ];
            let (mut core, mut s) = (Core::default(), BluetoothState::default());
            for f in &frames {
                core.frame(f, &mut s);
                let again = core.frame(f, &mut s).log;
                assert!(
                    !matches!(again, Some((Level::INFO, _))),
                    "{:#04x} repeated at info: {again:?}",
                    f.cmd
                );
            }
        }

        #[test]
        fn a_new_call_after_the_last_one_logs_again() {
            let (mut core, mut s) = (Core::default(), BluetoothState::default());
            let ring = Frame::request(0xB1, b"0471");
            let cleared = Frame::request(0xB1, &[]);
            let level = |core: &mut Core, s: &mut BluetoothState, f: &Frame| core.frame(f, s).log.map(|(l, _)| l);
            assert_eq!(level(&mut core, &mut s, &ring), Some(Level::INFO));
            assert_eq!(level(&mut core, &mut s, &ring), Some(Level::DEBUG));
            assert_eq!(level(&mut core, &mut s, &cleared), Some(Level::INFO));
            assert_eq!(level(&mut core, &mut s, &ring), Some(Level::INFO));
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
