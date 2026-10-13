//! The Bluetooth module on `/dev/ttyS1` (a Telink SoC running its own
//! Bluetooth stack): docs/BLUETOOTH_PROTOCOL.md.
//!
//! - [`frame`]: `75 45|46 LEN CMD DATA SUM` framing and the byte parser.
//! - [`command`]: what the host sends, byte-exact with stock.
//! - [`event`]: what the module sends, decoded.
//! - [`state`]: events folded into `BluetoothState` (the phone, the module's
//!   name, the switch).
//! - [`call`]: the call, and every sign that it is over.
//! - [`control`]: bus commands (switch, pair, disconnect, answer, decline)
//!   and the 2-minute pairing window.
//! - [`startup`]: stock's start-up sequence and the module's name.
//! - [`volume`]: the phone's volume on the Wave's speaker.
//! - [`ota`]: `BT.bin` / `DONGLE.bin` updates (TODO F6), run only by the
//!   `waved-bt-flash` tool, never by the service.
//!
//! [`BtService`] sends stock's start-up sequence ([`startup`]), answers every
//! frame the module repeats (`75 45`) with stock's acknowledgement and keeps
//! `BluetoothState`. Given the bus (`with_commands`) it switches Bluetooth on
//! and off, pairs, stops pairing, disconnects and answers or declines calls;
//! given the speaker (`with_speaker`) it follows the phone's volume.
//!
//! Privacy: the phone book (`0xB0`) is dropped unread, and no log line at
//! any level carries a phone's name, MAC or a number (`log_line`): module log
//! lines show only as known fixed messages, undecoded frames as their length.

pub mod call;
pub mod command;
pub mod control;
pub mod event;
pub mod frame;
pub mod ota;
pub mod startup;
pub mod state;
pub mod volume;

#[cfg(feature = "simulate")]
pub mod simulate;

pub use startup::Settings;

use call::Calls;
use command::{ack_for_frame, Command};
use control::{Control, Response};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use event::Event;
use frame::{Frame, Prefix, PREFIX_ACK, PREFIX_LEGACY, PREFIX_REQUEST};
use startup::Startup;
use state::Tracker;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, trace, warn, Level};
use waved_state::{AudioState, BluetoothState, Command as BusCommand, DeviceEvent, StateBus};
use waved_uart::UartMux;

/// The module's UART and speed (stock `g_uart_bt_device`, `bttool`).
pub const BT_PORT: &str = "/dev/ttyS1";
pub const BT_BAUD: u32 = 921_600;

/// Stock polls the module every 5 s (`bt_heartbeat_thread`).
const STATUS_POLL_MS: u64 = 5_000;

/// How often the loop looks at the bus and the clocks when the module is
/// quiet.
const TICK: Duration = Duration::from_millis(100);

impl Settings {
    /// From config: `[bluetooth] enabled` and `device.name`; with no device
    /// name, stock's name from `/proc/cpuinfo`'s serial.
    pub fn new(enabled: bool, device_name: &str) -> Self {
        let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        let serial = startup::serial_suffix(&cpuinfo);
        Self { enabled, name: startup::module_name(device_name, serial.as_deref()) }
    }
}

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

/// The service without the UART or the clock: module frames, bus commands
/// and the time in; frames to send, state changes and log lines out.
#[derive(Debug, Default)]
struct Core {
    tracker: Tracker,
    control: Control,
    calls: Calls,
    startup: Startup,
    seen: Seen,
    /// What `SetName` sends, for stock's renaming while the module is unnamed.
    name: String,
    /// The module has sent something: the line works, polling makes sense.
    heard: bool,
    next_poll_ms: u64,
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
    /// An info line for a call change the frame does not name by itself.
    note: Option<&'static str>,
    /// The speaker volume the phone asks for (`0xD5`, [`volume`]).
    speaker: Option<u8>,
}

/// What a tick of the clock asks of the service.
#[derive(Debug, Default, PartialEq)]
struct Ticked {
    frames: Vec<Command>,
    changed: bool,
    note: Option<&'static str>,
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
    /// Stock's start-up ([`startup`]) and the switch as config has it.
    /// Returns whether `s` changed.
    fn start(&mut self, settings: &Settings, s: &mut BluetoothState) -> bool {
        self.name = settings.name.clone();
        self.startup = Startup::stock(settings);
        self.next_poll_ms = STATUS_POLL_MS;
        self.tracker.set_enabled(settings.enabled, s);
        self.calls.set_enabled(settings.enabled);
        let changed = s.bt_on != settings.enabled;
        s.bt_on = settings.enabled;
        changed
    }

    /// A module frame: its acknowledgement ([`command::ack_for_frame`]),
    /// whether `s` changed, a connect / disconnect to broadcast, the log
    /// line and the speaker volume the phone asks for.
    fn frame(&mut self, frame: &Frame, now_ms: u64, s: &mut BluetoothState) -> Handled {
        self.heard = true;
        self.startup.heard(frame.cmd);
        let acks = ack_for_frame(frame).into_iter().collect();
        let event = event::decode(frame);
        let applied = self.tracker.apply(&event, s);
        let call = self.calls.event(&event, now_ms, s);
        let window = self.control.after_event(s);
        let speaker = match &event {
            Event::PhoneVolume { kind, level, max } => volume::speaker_target(*kind, *level, *max, s),
            _ => None,
        };
        let news = applied.changed || call.changed || applied.event.is_some() || self.seen.news(&event);
        let log = log_line(&event).map(|(level, line)| match level {
            Level::INFO if !news => (Level::DEBUG, line),
            _ => (level, line),
        });
        Handled {
            acks,
            changed: applied.changed || call.changed || window,
            event: applied.event,
            log,
            note: call.why,
            speaker,
        }
    }

    fn command(&mut self, command: &BusCommand, now_ms: u64, unix_time: u32, s: &mut BluetoothState) -> Response {
        let mut out = self.control.command(command, now_ms, unix_time, s);
        match command {
            // Stock's call buttons act on the screen at once (`call`).
            BusCommand::BtAnswerCall if !out.frames.is_empty() => out.changed |= self.calls.answered(now_ms, s),
            BusCommand::BtDeclineCall if !out.frames.is_empty() => {
                out.changed |= self.calls.hung_up(now_ms, s)
            }
            BusCommand::SetBluetooth(on) if out.changed => {
                let applied = self.tracker.set_enabled(*on, s);
                self.calls.set_enabled(*on);
                out.event = applied.event;
            }
            _ => {}
        }
        out
    }

    /// The clocks: the start-up sequence, the pairing window, the call and
    /// the 5 s status poll once the module has answered.
    fn tick(&mut self, now_ms: u64, s: &mut BluetoothState) -> Ticked {
        let mut frames = self.startup.poll(now_ms);
        let window = self.control.tick(now_ms, s);
        let call = self.calls.tick(now_ms, s);
        if self.heard && now_ms >= self.next_poll_ms {
            self.next_poll_ms = now_ms + STATUS_POLL_MS;
            if self.startup.done() {
                // Stock names the module again while it still has its own
                // default name (bt_heartbeat_thread 75122-75127).
                if s.advertised_name.contains(startup::UNNAMED) {
                    frames.extend([Command::SetName(self.name.clone()), Command::GetName]);
                }
                frames.push(Command::GetStatus);
            }
        }
        Ticked { frames, changed: window || call.changed, note: call.why }
    }
}

/// Where the phone's volume goes: the speaker's volume and mute on the bus,
/// as any other volume change (persisted by waved-settings). The change is
/// marked as not the Wave's own, so the HID volume watch does not step the
/// computer's volume for it (`StateBus::host_pending_speaker_vol`).
pub struct Speaker {
    commands: broadcast::Sender<BusCommand>,
    audio: watch::Receiver<AudioState>,
    echo: Arc<AtomicI32>,
}

impl Speaker {
    pub fn from_bus(bus: &StateBus) -> Self {
        Self {
            commands: bus.commands.clone(),
            audio: bus.audio.subscribe(),
            echo: Arc::clone(&bus.host_pending_speaker_vol),
        }
    }

    /// Bring the speaker to `target` percent; returns what was sent.
    fn apply(&self, target: u8) -> Vec<BusCommand> {
        let (volume, muted) = {
            let audio = self.audio.borrow();
            (audio.speaker_volume, audio.speaker_muted)
        };
        let commands = volume::speaker_commands(target, volume, muted);
        for command in &commands {
            if let BusCommand::SetSpeakerVolume(v) = command {
                self.echo.store(i32::from(*v), Ordering::Release);
            }
            let _ = self.commands.send(command.clone());
        }
        commands
    }
}

pub struct BtService {
    bt_rx: Receiver<waved_uart::Frame>,
    uart: Arc<UartMux>,
    bt_tx: Arc<watch::Sender<BluetoothState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
    commands: Option<broadcast::Receiver<BusCommand>>,
    settings: Settings,
    speaker: Option<Speaker>,
}

impl BtService {
    pub fn new(
        bt_rx: Receiver<waved_uart::Frame>,
        uart: Arc<UartMux>,
        bt_tx: Arc<watch::Sender<BluetoothState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        Self {
            bt_rx,
            uart,
            bt_tx,
            events_tx,
            commands: None,
            settings: Settings::new(true, ""),
            speaker: None,
        }
    }

    /// Take `SetBluetooth`, `BtPair`, `BtStopPairing`, `BtDisconnect`,
    /// `BtAnswerCall` and `BtDeclineCall` from the bus. Without it the
    /// service only follows the module.
    pub fn with_commands(mut self, commands: broadcast::Receiver<BusCommand>) -> Self {
        self.commands = Some(commands);
        self
    }

    /// The switch and the module's name from config (`Settings::new`).
    /// Without it: Bluetooth on, stock's name.
    pub fn with_settings(mut self, settings: Settings) -> Self {
        self.settings = settings;
        self
    }

    /// Follow the phone's volume on the speaker (`0xD5`). Without it the
    /// phone's volume reports are only logged.
    pub fn with_speaker(mut self, speaker: Speaker) -> Self {
        self.speaker = Some(speaker);
        self
    }

    pub fn run(mut self) {
        info!(
            "bt_srv started, Bluetooth {}",
            if self.settings.enabled { "on" } else { "off" }
        );
        let start = Instant::now();
        let now_ms = || start.elapsed().as_millis() as u64;
        let mut core = Core::default();
        let settings = self.settings.clone();
        self.bt_tx.send_if_modified(|s| core.start(&settings, s));
        loop {
            match self.bt_rx.recv_timeout(TICK) {
                Ok(raw) => self.handle(&raw, now_ms(), &mut core),
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
                if let Some(e) = out.event {
                    let _ = self.events_tx.send(e);
                }
            }
            let now = now_ms();
            let mut ticked = Ticked::default();
            self.bt_tx.send_if_modified(|s| {
                ticked = core.tick(now, s);
                ticked.changed
            });
            if let Some(note) = ticked.note {
                info!("{note}");
            }
            for c in &ticked.frames {
                let frame = c.frame();
                // The start-up sequence and the 5 s poll: the command byte
                // only (the device pass checks the order).
                if frame.cmd != command::id::GET_STATUS {
                    debug!("bt: sends {:#04x}", frame.cmd);
                }
                self.send(&frame);
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

    fn handle(&self, raw: &waved_uart::Frame, now_ms: u64, core: &mut Core) {
        let Some(frame) = from_uart(raw) else { return };
        let mut out = Handled::default();
        self.bt_tx.send_if_modified(|s| {
            out = core.frame(&frame, now_ms, s);
            out.changed
        });
        for ack in &out.acks {
            self.send(ack);
        }
        log_event(out.log);
        if let Some(note) = out.note {
            info!("{note}");
        }
        if let Some(e) = out.event {
            let _ = self.events_tx.send(e);
        }
        if let (Some(target), Some(speaker)) = (out.speaker, self.speaker.as_ref()) {
            let sent = speaker.apply(target);
            if !sent.is_empty() {
                info!("bt: the phone's volume sets the speaker to {target} %");
            }
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

/// The module's own fixed messages (strings in its firmware, 0.0.0.28 and
/// 0.0.0.33): pairing, reconnecting, the radio, calls, call audio, volume and
/// the phone book's link. A module log line shows only as the message it
/// starts with, never as received: the rest of a line can carry the phone's
/// address (`a2dp_snk_disconnect_req addr …`), and other lines its name and
/// MAC (`app_set_bt_last_addr`).
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
    // Calls (`call::call_hint` acts on the first three).
    "HFP  Call active",
    "HFP call end",
    "HFP RFCOMM_CLOSED",
    "HFP  incoming",
    "HFP outgoing",
    "AT RING",
    "+CIEV hf-state",
    "hf statusCall =",
    "hf statusCallSetup =",
    "hf_dev->statusCallSetup set 0",
    // Call audio (the SCO link).
    "SCO CONN CMP",
    "SCO DISCONN CMP, reason",
    "app_hci_evt_sco_connect",
    "app_hci_evt_sco_disconn",
    // Volume.
    "app_bt_hfp_hf_vol_change",
    "app_bt_avrcp_ct_vol_change",
    "AVRCP_CTYPE_CONTROL @remote vol change",
    "AVRCP_VEN_RSP_CHANGED, @remote vol change",
    "app_set_avrcp_cur_music_vol",
    "MAX A2DP VOL!",
    "MIN A2DP VOL!",
    "MAX HFP VOL!",
    "MIN HFP VOL!",
    // The phone book's link (never its contents).
    "PBAP dev closed, open PBAP",
    "PBAP START CONNECTING",
    "APP_PBAP_CONNECTED",
    "APP_PBAP_DISCONNECTED",
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
        Event::PbapStatus(ready) => debug(format!("bt: phone book link status {ready} (never asked for)")),
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
        use super::super::{Core, Settings};
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

        fn settings(enabled: bool) -> Settings {
            Settings { enabled, name: "Insta360 Wave BT 0A1B2C".into() }
        }

        /// The service started with Bluetooth on, as config's default.
        fn started() -> (Core, BluetoothState) {
            let (mut core, mut s) = (Core::default(), BluetoothState::default());
            core.start(&settings(true), &mut s);
            (core, s)
        }

        #[test]
        fn pairing_a_phone_from_the_bus_to_the_state() {
            let (mut core, mut s) = started();
            let out = core.command(&BusCommand::BtPair, 0, 0, &mut s);
            assert_eq!(hex(&out.frames.iter().map(|c| c.frame()).collect::<Vec<_>>()), [
                "75 45 02 D8 00 DA",
                "75 45 02 A3 00 A5",
                "75 45 02 A7 00 A9"
            ]);
            assert_eq!(s.pairing_left_s, 120);

            // The module: pairing mode (seen on the device), acknowledged.
            let h = core.frame(&wire(&[0x75, 0x45, 0x03, 0xB2, 0x08, 0x00, 0xBD]), 0, &mut s);
            assert_eq!(hex(&h.acks), ["75 46 02 B2 00 B4"]);
            assert!(h.changed && s.pairing && s.pairing_left_s == 120);
            assert_eq!(h.event, None);

            // A phone connects: the window closes.
            core.tick(30_000, &mut s);
            assert_eq!(s.pairing_left_s, 90);
            let h = core.frame(&wire(&[0x75, 0x45, 0x03, 0xB2, 0x01, 0x00, 0xB6]), 0, &mut s);
            assert!(h.changed && s.connected && !s.pairing);
            assert_eq!(s.pairing_left_s, 0);
            assert_eq!(h.event, Some(DeviceEvent::BtConnected(String::new())));
        }

        #[test]
        fn leaving_the_pairing_screen_ends_the_window() {
            let (mut core, mut s) = started();
            core.command(&BusCommand::BtPair, 0, 0, &mut s);
            core.frame(&wire(&[0x75, 0x45, 0x03, 0xB2, 0x08, 0x00, 0xBD]), 0, &mut s);
            let out = core.command(&BusCommand::BtStopPairing, 5_000, 0, &mut s);
            assert!(out.frames.is_empty(), "no command leaves pairing mode");
            assert!(out.changed);
            assert_eq!(s.pairing_left_s, 0);
            assert!(s.pairing, "the module's own bit 3 stays until it says otherwise");
            assert!(!core.tick(6_000, &mut s).changed);
        }

        #[test]
        fn a_call_from_the_module_to_answer() {
            let (mut core, mut s) = started();
            let ring = [0x75, 0x45, 0x0C, 0xB1, 0x2B, 0x33, 0x32, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0xF1];
            let h = core.frame(&wire(&ring), 0, &mut s);
            assert_eq!(hex(&h.acks), ["75 46 02 B1 00 B3"]);
            assert!(h.changed);
            assert_eq!(s.call, BtCall::Ringing { number: "+3212345678".into() });
            let out = core.command(&BusCommand::BtAnswerCall, 0, 0x6720_0000, &mut s);
            assert_eq!(hex(&out.frames.iter().map(|c| c.frame()).collect::<Vec<_>>()), [
                "75 45 07 D0 03 00 00 20 67 00 61"
            ]);
            // Up at once, as stock's pickup button (libui 27958).
            assert!(out.changed && s.call.is_active());
            let h = core.frame(&wire(&[0x75, 0x45, 0x02, 0xD1, 0x00, 0xD3]), 0, &mut s);
            assert_eq!(hex(&h.acks), ["75 46 01 D1 D2"]);
            assert!(!h.changed, "the module's call up changes nothing more");
            assert!(s.call.is_active());
        }

        #[test]
        fn the_phone_book_is_acknowledged_and_changes_nothing() {
            let (mut core, mut s) = started();
            let entry = Frame::request(0xB0, b"\x01\0\0\0\x06+32470\x03A\0n\0n\0");
            let h = core.frame(&entry, 0, &mut s);
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
            let (mut core, mut s) = started();
            for echo in [[0x75, 0x46, 0x02, 0xA9, 0x00, 0xAB], [0x75, 0x46, 0x02, 0xC5, 0x00, 0xC7]] {
                let h = core.frame(&wire(&echo), 0, &mut s);
                assert!(h.acks.is_empty(), "{echo:02X?}");
                assert!(!h.changed);
            }
            // The version comes back in the reply itself: decoded, not acknowledged.
            let h = core.frame(&Frame::ack(0x91, b"\x000.0.0.28"), 0, &mut s);
            assert!(h.acks.is_empty());
            assert_eq!(h.log.map(|(_, line)| line), Some("bt module firmware 0.0.0.28".into()));
            // The module's own frames still are, the name and the status included.
            let h = core.frame(&name_frame("Insta360 Wave BT 0A1B2C"), 0, &mut s);
            assert_eq!(hex(&h.acks), ["75 46 02 C6 00 C8"]);
            let h = core.frame(&wire(&[0x75, 0x45, 0x02, 0xAA, 0x01, 0xAD]), 0, &mut s);
            assert_eq!(hex(&h.acks), ["75 46 02 AA 00 AC"]);
            assert!(core.frame(&Frame::request(0xE0, &[]), 0, &mut s).acks.is_empty());
        }

        #[test]
        fn the_module_name_shows_at_info_only_when_it_changes() {
            let (mut core, mut s) = started();
            let level = |core: &mut Core, s: &mut BluetoothState, f: &Frame| core.frame(f, 0, s).log.map(|(l, _)| l);
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
            let (mut core, mut s) = started();
            for f in &frames {
                core.frame(f, 0, &mut s);
                let again = core.frame(f, 0, &mut s).log;
                assert!(
                    !matches!(again, Some((Level::INFO, _))),
                    "{:#04x} repeated at info: {again:?}",
                    f.cmd
                );
            }
        }

        #[test]
        fn a_new_call_after_the_last_one_logs_again() {
            let (mut core, mut s) = started();
            let ring = Frame::request(0xB1, b"0471");
            let cleared = Frame::request(0xB1, &[]);
            let level = |core: &mut Core, s: &mut BluetoothState, f: &Frame| core.frame(f, 0, s).log.map(|(l, _)| l);
            assert_eq!(level(&mut core, &mut s, &ring), Some(Level::INFO));
            assert_eq!(level(&mut core, &mut s, &ring), Some(Level::DEBUG));
            assert_eq!(level(&mut core, &mut s, &cleared), Some(Level::INFO));
            assert_eq!(level(&mut core, &mut s, &ring), Some(Level::INFO));
        }
    }

    mod service {
        use super::super::{Core, Settings, Ticked, STATUS_POLL_MS};
        use crate::call::{CALL_AUDIO_GRACE_MS, RING_TIMEOUT_MS};
        use crate::command::{id, Command};
        use crate::frame::Frame;
        use waved_state::{BluetoothState, BtCall, Command as BusCommand, DeviceEvent};

        fn hex(frames: &[Command]) -> Vec<String> {
            frames
                .iter()
                .map(|c| c.encode().iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "))
                .collect()
        }

        fn start(enabled: bool) -> (Core, BluetoothState) {
            let (mut core, mut s) = (Core::default(), BluetoothState::default());
            let changed = core.start(&Settings { enabled, name: "Wave Boardroom".into() }, &mut s);
            assert_eq!(changed, enabled, "bt_on starts false");
            assert_eq!(s.bt_on, enabled);
            (core, s)
        }

        /// The start-up as the module answers it at once.
        fn answered_startup(core: &mut Core, s: &mut BluetoothState) -> Vec<String> {
            let mut all = hex(&core.tick(0, s).frames);
            core.frame(&Frame::request(id::NAME_SET, &[0]), 0, s);
            all.extend(hex(&core.tick(100, s).frames));
            let mut name = vec![0];
            name.extend_from_slice(b"Wave Boardroom");
            core.frame(&Frame::request(id::NAME, &name), 100, s);
            all.extend(hex(&core.tick(200, s).frames));
            all
        }

        #[test]
        fn it_starts_like_stock_with_the_device_name() {
            let (mut core, mut s) = start(true);
            let frames = answered_startup(&mut core, &mut s);
            assert_eq!(
                frames[0],
                "75 45 18 8B 57 61 76 65 20 42 6F 61 72 64 72 6F 6F 6D 00 00 00 00 00 00 00 00 00 FB",
                "SetName: the device name, NUL-padded to stock's 23 bytes"
            );
            assert_eq!(&frames[1..], [
                "75 45 02 CC 00 CE",    // OpenBt
                "75 45 02 D8 01 DB",    // AutoReconnect on
                "75 45 02 E0 00 E2",    // SaveFlash
                "75 45 01 91 92",       // GetVersion
                "75 45 01 93 94",       // GetBuildTime
                "75 45 01 A9 AA",       // GetStatus
                "75 45 01 C5 C6",       // GetName
                "75 45 03 EA 01 00 EE", // PtaMode
                "75 45 01 C7 C8",       // GetPhoneMac
            ]);
            assert_eq!(s.advertised_name, "Wave Boardroom");
        }

        #[test]
        fn switched_off_in_config_it_starts_closed() {
            let (mut core, mut s) = start(false);
            let frames = answered_startup(&mut core, &mut s);
            assert_eq!(&frames[1..3], ["75 45 01 CB CC", "75 45 02 D8 00 DA"]);
            // While off a phone the module still reports does not connect.
            let h = core.frame(&Frame::request(0xB2, &[0x03]), 300, &mut s);
            assert!(!s.connected && !s.phone_audio);
            assert_eq!(h.event, None);
        }

        #[test]
        fn the_switch_from_the_bus() {
            let (mut core, mut s) = start(true);
            core.frame(&Frame::request(0xB2, &[0x07]), 0, &mut s);
            core.frame(&Frame::request(0xB1, b"0471"), 0, &mut s);
            assert!(s.connected && s.call.is_ringing());
            let out = core.command(&BusCommand::SetBluetooth(false), 1_000, 0, &mut s);
            assert_eq!(hex(&out.frames), ["75 45 01 CB CC"]);
            assert!(out.changed);
            assert_eq!(out.event, Some(DeviceEvent::BtDisconnected));
            assert!(!s.bt_on && !s.connected && !s.call_audio);
            assert_eq!(s.call, BtCall::Idle);
            // Off: the module's reports bring nothing back, nothing rings.
            for f in [Frame::request(0xB2, &[0x07]), Frame::request(0xAA, &[1]), Frame::request(0xB1, b"0471")] {
                let h = core.frame(&f, 2_000, &mut s);
                assert!(!h.changed && h.event.is_none(), "{:#04x}", f.cmd);
            }
            // On again: OpenBt, and the next report counts.
            let out = core.command(&BusCommand::SetBluetooth(true), 3_000, 0, &mut s);
            assert_eq!(hex(&out.frames), ["75 45 02 CC 00 CE"]);
            assert_eq!(out.event, None);
            let h = core.frame(&Frame::request(0xAA, &[1]), 3_100, &mut s);
            assert_eq!(h.event, Some(DeviceEvent::BtConnected(String::new())));
        }

        #[test]
        fn the_phones_volume_reaches_the_speaker_while_it_plays() {
            let (mut core, mut s) = start(true);
            // Not playing: stock ignores it ("not bt or dongle path").
            let h = core.frame(&Frame::request(0xD5, &[0, 0, 8]), 0, &mut s);
            assert_eq!(h.speaker, None);
            assert_eq!(h.acks[0].encode(), [0x75, 0x46, 0x01, 0xD5, 0xD6], "stock's empty ack");
            core.frame(&Frame::request(0xB2, &[0x03]), 0, &mut s);
            assert_eq!(core.frame(&Frame::request(0xD5, &[0, 0, 8]), 0, &mut s).speaker, Some(50));
            assert_eq!(core.frame(&Frame::request(0xD5, &[1, 1, 16]), 0, &mut s).speaker, Some(100));
            assert_eq!(core.frame(&Frame::request(0xD5, &[0, 7, 8]), 0, &mut s).speaker, None, "unknown scale");
        }

        #[test]
        fn a_call_answered_on_the_phone_ends_on_its_own() {
            // Device 2026-10-13: answered on the iPhone, no 0xD1, no empty
            // 0xB1. With an in-band ring the call audio closes; without it,
            // the minute runs out.
            let (mut core, mut s) = start(true);
            core.frame(&Frame::request(0xB2, &[0x01]), 0, &mut s);
            core.frame(&Frame::request(0xB1, b"0471"), 0, &mut s);
            core.frame(&Frame::request(0xB2, &[0x07]), 500, &mut s);
            core.frame(&Frame::request(0xB2, &[0x01]), 4_000, &mut s);
            let t = core.tick(4_000 + CALL_AUDIO_GRACE_MS, &mut s);
            assert!(t.changed);
            assert_eq!(t.note, Some("bt: the call audio closed: call over here"));
            assert_eq!(s.call, BtCall::Idle);

            core.frame(&Frame::request(0xB1, b"0472"), 10_000, &mut s);
            let t = core.tick(10_000 + RING_TIMEOUT_MS, &mut s);
            assert_eq!(t.note, Some("bt: no news of the ring for 60 s: ring over"));
            assert_eq!(s.call, BtCall::Idle);
        }

        #[test]
        fn the_modules_log_ends_a_call() {
            let (mut core, mut s) = start(true);
            core.frame(&Frame::request(0xB1, b"0471"), 0, &mut s);
            let mut line = vec![1];
            line.extend_from_slice(b"HFP  Call active");
            let h = core.frame(&Frame::request(0xF0, &line), 1_000, &mut s);
            assert!(h.changed && s.call.is_active());
            assert_eq!(h.note, Some("bt: the module reports the call up"));
            let mut line = vec![1];
            line.extend_from_slice(b"HFP call end");
            assert!(core.frame(&Frame::request(0xF0, &line), 9_000, &mut s).changed);
            assert_eq!(s.call, BtCall::Idle);
        }

        #[test]
        fn decline_on_the_wave_leaves_at_once() {
            let (mut core, mut s) = start(true);
            core.frame(&Frame::request(0xB1, b"0471"), 0, &mut s);
            let out = core.command(&BusCommand::BtDeclineCall, 1_000, 0x6720_0000, &mut s);
            assert_eq!(hex(&out.frames), ["75 45 07 D0 00 00 00 20 67 00 5E"]);
            assert!(out.changed);
            assert_eq!(s.call, BtCall::Idle);
            // The module repeating the number does not bring the screen back.
            assert!(!core.frame(&Frame::request(0xB1, b"0471"), 1_500, &mut s).changed);
        }

        #[test]
        fn the_module_is_named_again_while_it_has_its_default_name() {
            // Stock's heartbeat: SetName and GetName while "XXXXXX" (75122).
            let (mut core, mut s) = start(true);
            answered_startup(&mut core, &mut s);
            s.advertised_name = "Insta360 Wave BT XXXXXX".into();
            let t = core.tick(STATUS_POLL_MS, &mut s);
            assert_eq!(t.frames, [Command::SetName("Wave Boardroom".into()), Command::GetName, Command::GetStatus]);
            s.advertised_name = "Wave Boardroom".into();
            let t = core.tick(2 * STATUS_POLL_MS, &mut s);
            assert_eq!(t.frames, [Command::GetStatus]);
            assert_eq!(core.tick(2 * STATUS_POLL_MS + 100, &mut s), Ticked::default());
        }

        #[test]
        fn no_status_poll_before_the_module_answers() {
            let (mut core, mut s) = start(true);
            core.tick(0, &mut s);
            let t = core.tick(10 * STATUS_POLL_MS, &mut s);
            assert!(!t.frames.contains(&Command::GetStatus), "{t:?}");
        }

        #[test]
        fn the_service_never_flashes() {
            // F6 runs only from the waved-bt-flash tool: nothing the service
            // sends, at start, on any bus command or on any tick, is an OTA
            // frame.
            const OTA: [u8; 9] = [0x80, 0x9D, 0x9F, 0xA1, 0xA2, 0xD9, 0xDB, 0xDD, 0xDF];
            let (mut core, mut s) = start(true);
            let mut frames: Vec<Command> = Vec::new();
            let mut now = 0;
            for _ in 0..50 {
                frames.extend(core.tick(now, &mut s).frames);
                now += 1_000;
            }
            core.frame(&Frame::request(0xB2, &[0x07]), now, &mut s);
            core.frame(&Frame::request(0xB1, b"0471"), now, &mut s);
            for command in [
                BusCommand::BtAnswerCall,
                BusCommand::BtDeclineCall,
                BusCommand::BtPair,
                BusCommand::BtStopPairing,
                BusCommand::BtDisconnect,
                BusCommand::SetBluetooth(false),
                BusCommand::SetBluetooth(true),
                BusCommand::InstallUpdate,
                BusCommand::FactoryReset,
            ] {
                frames.extend(core.command(&command, now, 0, &mut s).frames);
                frames.extend(core.tick(now, &mut s).frames);
                now += 10_000;
            }
            for f in frames.iter().map(Command::frame) {
                assert!(!OTA.contains(&f.cmd), "{:#04x}", f.cmd);
            }
        }
    }
}
