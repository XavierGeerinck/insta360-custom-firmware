//! The power key (TODO E2): find its input node, read KEY_POWER edges and turn
//! them into short and long presses. The UI runtime receives them as
//! `DeviceEvent::PowerKey` and decides what they mean (`display::DisplayPower`).

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tracing::{info, warn};
use waved_state::{DeviceEvent, PowerKeyPress};

/// Holding the key this long asks to power off (docs/mockups/README.md).
pub const POWER_KEY_HOLD_MS: u64 = 2_000;
pub const SYS_INPUT: &str = "/sys/class/input";
pub const DEV_INPUT: &str = "/dev/input";
/// The input device that advertises KEY_POWER (docs/AUDIO_HARDWARE.md).
pub const POWER_KEY_DEVICE: &str = "vad_keypad";
/// `struct input_event` on 64-bit Linux: timeval (16) + type (2) + code (2) + value (4).
pub const INPUT_EVENT_SIZE: usize = 24;

const EV_KEY: u16 = 0x01;
const KEY_POWER: u16 = 116;

/// The `/dev/input/eventN` node of the power key: by device name first, then
/// by the KEY_POWER capability bit.
pub fn find_power_key(sys_input: &Path, dev_input: &Path) -> Option<PathBuf> {
    let mut nodes: Vec<String> = std::fs::read_dir(sys_input)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("event"))
        .collect();
    nodes.sort_by_key(|name| name["event".len()..].parse::<u32>().unwrap_or(u32::MAX));
    let read = |node: &str, file: &str| std::fs::read_to_string(sys_input.join(node).join("device").join(file)).ok();
    let by_name = nodes
        .iter()
        .find(|node| read(node, "name").is_some_and(|name| name.trim() == POWER_KEY_DEVICE));
    let by_capability = || {
        nodes
            .iter()
            .find(|node| read(node, "capabilities/key").is_some_and(|keys| has_key(&keys, KEY_POWER)))
    };
    by_name.or_else(by_capability).map(|node| dev_input.join(node))
}

/// A sysfs key bitmap: hex words, most significant first, 64 bits each (aarch64).
pub fn has_key(bitmap: &str, code: u16) -> bool {
    let words: Vec<&str> = bitmap.split_whitespace().collect();
    let index = usize::from(code / 64);
    let Some(word) = words.len().checked_sub(index + 1).and_then(|i| words.get(i)) else {
        return false;
    };
    u64::from_str_radix(word, 16).is_ok_and(|bits| bits & (1 << (code % 64)) != 0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

pub fn parse_event(raw: &[u8; INPUT_EVENT_SIZE]) -> InputEvent {
    InputEvent {
        kind: u16::from_le_bytes([raw[16], raw[17]]),
        code: u16::from_le_bytes([raw[18], raw[19]]),
        value: i32::from_le_bytes([raw[20], raw[21], raw[22], raw[23]]),
    }
}

/// `Some(pressed)` for a KEY_POWER press or release; auto-repeat and every
/// other event are `None`.
pub fn power_edge(event: InputEvent) -> Option<bool> {
    match (event.kind, event.code, event.value) {
        (EV_KEY, KEY_POWER, 1) => Some(true),
        (EV_KEY, KEY_POWER, 0) => Some(false),
        _ => None,
    }
}

/// Press and release edges → `Short` (released before the hold time) or
/// `Long` (reported once, as soon as the hold time passes).
#[derive(Clone, Debug)]
pub struct PowerKeyDecoder {
    hold_ms: u64,
    pressed_at: Option<u64>,
    long_sent: bool,
}

impl PowerKeyDecoder {
    pub fn new(hold_ms: u64) -> Self {
        Self {
            hold_ms,
            pressed_at: None,
            long_sent: false,
        }
    }

    pub fn on_edge(&mut self, pressed: bool, now_ms: u64) -> Option<PowerKeyPress> {
        if pressed {
            self.pressed_at = Some(now_ms);
            self.long_sent = false;
            return None;
        }
        let pressed_at = self.pressed_at.take()?;
        if std::mem::take(&mut self.long_sent) {
            None
        } else if now_ms.saturating_sub(pressed_at) >= self.hold_ms {
            Some(PowerKeyPress::Long)
        } else {
            Some(PowerKeyPress::Short)
        }
    }

    /// When `on_time` should run next, while the key is held.
    pub fn deadline_ms(&self) -> Option<u64> {
        match (self.pressed_at, self.long_sent) {
            (Some(at), false) => Some(at + self.hold_ms),
            _ => None,
        }
    }

    pub fn on_time(&mut self, now_ms: u64) -> Option<PowerKeyPress> {
        let deadline = self.deadline_ms()?;
        if now_ms >= deadline {
            self.long_sent = true;
            Some(PowerKeyPress::Long)
        } else {
            None
        }
    }
}

/// Decode edges until the sender hangs up, waking at the hold deadline.
pub fn run_decoder(edges: mpsc::Receiver<bool>, hold_ms: u64, mut emit: impl FnMut(PowerKeyPress)) {
    let start = Instant::now();
    let now = || start.elapsed().as_millis() as u64;
    let mut decoder = PowerKeyDecoder::new(hold_ms);
    loop {
        let received = match decoder.deadline_ms() {
            Some(deadline) => edges.recv_timeout(Duration::from_millis(deadline.saturating_sub(now()))),
            None => edges.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let press = match received {
            Ok(pressed) => decoder.on_edge(pressed, now()),
            Err(RecvTimeoutError::Timeout) => decoder.on_time(now()),
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if let Some(press) = press {
            emit(press);
        }
    }
}

/// Read `node` on one thread, decode on another, send presses to `events`.
pub fn spawn(node: PathBuf, events: broadcast::Sender<DeviceEvent>) -> io::Result<()> {
    let mut file = File::open(&node)?;
    info!("power key: reading {}", node.display());
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("power-key-read".into())
        .spawn(move || {
            let mut raw = [0u8; INPUT_EVENT_SIZE];
            loop {
                if let Err(error) = file.read_exact(&mut raw) {
                    warn!("power key: read failed, no more presses: {error}");
                    return;
                }
                if let Some(pressed) = power_edge(parse_event(&raw)) {
                    if tx.send(pressed).is_err() {
                        return;
                    }
                }
            }
        })?;
    std::thread::Builder::new().name("power-key".into()).spawn(move || {
        run_decoder(rx, POWER_KEY_HOLD_MS, |press| {
            info!("power key: {press:?} press");
            let _ = events.send(DeviceEvent::PowerKey(press));
        })
    })?;
    Ok(())
}

/// Find the power key and start reading it. Logs and returns false when the
/// device has none or it can't be opened.
pub fn start(events: broadcast::Sender<DeviceEvent>) -> bool {
    let Some(node) = find_power_key(Path::new(SYS_INPUT), Path::new(DEV_INPUT)) else {
        warn!("power key: no input device advertises KEY_POWER; the key does nothing");
        return false;
    };
    match spawn(node, events) {
        Ok(()) => true,
        Err(error) => {
            warn!("power key: not started: {error}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn input_tree(devices: &[(&str, &str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (node, name, keys) in devices {
            let device = dir.path().join(node).join("device");
            std::fs::create_dir_all(device.join("capabilities")).unwrap();
            std::fs::write(device.join("name"), format!("{name}\n")).unwrap();
            std::fs::write(device.join("capabilities/key"), format!("{keys}\n")).unwrap();
        }
        std::fs::create_dir_all(dir.path().join("input0")).unwrap(); // not an event node
        dir
    }

    fn raw(kind: u16, code: u16, value: i32) -> [u8; INPUT_EVENT_SIZE] {
        let mut raw = [0u8; INPUT_EVENT_SIZE];
        raw[16..18].copy_from_slice(&kind.to_le_bytes());
        raw[18..20].copy_from_slice(&code.to_le_bytes());
        raw[20..24].copy_from_slice(&value.to_le_bytes());
        raw
    }

    const TOUCH_KEYS: &str = "400 0 0 0 0 0"; // BTN_TOUCH (330)
    const POWER_KEYS: &str = "10000000000000 0"; // KEY_POWER (116)

    #[test]
    fn the_power_key_node_is_found_by_name_then_by_capability() {
        let dev = Path::new("/dev/input");
        let wave = input_tree(&[("event1", "ILITEK_TDDI", TOUCH_KEYS), ("event0", "vad_keypad", POWER_KEYS)]);
        assert_eq!(find_power_key(wave.path(), dev), Some(dev.join("event0")));
        let renamed = input_tree(&[("event1", "ILITEK_TDDI", TOUCH_KEYS), ("event3", "gpio-keys", POWER_KEYS)]);
        assert_eq!(find_power_key(renamed.path(), dev), Some(dev.join("event3")));
        let none = input_tree(&[("event1", "ILITEK_TDDI", TOUCH_KEYS)]);
        assert_eq!(find_power_key(none.path(), dev), None);
        assert_eq!(find_power_key(Path::new("/nonexistent/sys"), dev), None);
    }

    #[test]
    fn key_bitmaps_are_read_most_significant_word_first() {
        assert!(has_key(POWER_KEYS, 116));
        assert!(!has_key(POWER_KEYS, 115));
        assert!(has_key(TOUCH_KEYS, 330));
        assert!(!has_key(TOUCH_KEYS, 116));
        assert!(!has_key("0", 116));
        assert!(!has_key("", 116));
        assert!(!has_key("zz 0", 116));
    }

    #[test]
    fn input_events_decode_from_the_64_bit_layout() {
        let press = parse_event(&raw(1, 116, 1));
        assert_eq!(press, InputEvent { kind: 1, code: 116, value: 1 });
        assert_eq!(power_edge(press), Some(true));
        assert_eq!(power_edge(parse_event(&raw(1, 116, 0))), Some(false));
        assert_eq!(power_edge(parse_event(&raw(1, 116, 2))), None, "auto-repeat");
        assert_eq!(power_edge(parse_event(&raw(1, 115, 1))), None, "another key");
        assert_eq!(power_edge(parse_event(&raw(0, 0, 0))), None, "EV_SYN");
    }

    #[test]
    fn short_and_long_presses_split_at_two_seconds() {
        let mut d = PowerKeyDecoder::new(POWER_KEY_HOLD_MS);
        assert_eq!(d.on_edge(true, 0), None);
        assert_eq!(d.on_edge(false, 1_999), Some(PowerKeyPress::Short));

        assert_eq!(d.on_edge(true, 10_000), None);
        assert_eq!(d.deadline_ms(), Some(12_000));
        assert_eq!(d.on_time(11_999), None);
        assert_eq!(d.on_time(12_000), Some(PowerKeyPress::Long));
        assert_eq!(d.deadline_ms(), None, "a long press is reported once");
        assert_eq!(d.on_edge(false, 15_000), None);

        // The timer ran late: the release still counts as a long press.
        assert_eq!(d.on_edge(true, 20_000), None);
        assert_eq!(d.on_edge(false, 22_000), Some(PowerKeyPress::Long));

        // A release without a press, and a missed release.
        assert_eq!(d.on_edge(false, 30_000), None);
        d.on_edge(true, 40_000);
        d.on_edge(true, 45_000);
        assert_eq!(d.on_edge(false, 45_100), Some(PowerKeyPress::Short));
    }

    #[test]
    fn the_decoder_thread_reports_a_held_key_before_release() {
        let (tx, rx) = mpsc::channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let thread = std::thread::spawn(move || run_decoder(rx, 50, move |p| sink.lock().unwrap().push(p)));
        tx.send(true).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(*seen.lock().unwrap(), vec![PowerKeyPress::Long]);
        tx.send(false).unwrap();
        tx.send(true).unwrap();
        tx.send(false).unwrap();
        drop(tx);
        thread.join().unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![PowerKeyPress::Long, PowerKeyPress::Short]);
    }
}
