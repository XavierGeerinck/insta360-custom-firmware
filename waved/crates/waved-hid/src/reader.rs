//! HID Reader — blocking std::thread owning the read side of /dev/hidg0.
//! Parses host Set_Report writes (Reports 0x05 / 0x06 / 0x07) into
//! AudioState.call_*, and hands vendor report 0x03 (the Wave Controller
//! PC link, `crate::pclink`) to its channel. It runs for the process
//! lifetime: a node that fails, goes away or is replaced (the gadget switch
//! of USB storage) is opened again (`ReadLoop`, `crate::node`).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tracing::{debug, info, warn};

use waved_state::AudioState;

use crate::node::{DevNode, Handle, Node, Opened, Outage, Reopening, Unavailable, READ_WAIT, REOPEN_EVERY};
use crate::stats::HidStats;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedReport {
    OffHook(bool),
    Mute(bool),
    Ringing(bool),
}

/// Decode a single `[report_id, byte]` write into a `LedReport`. Returns
/// `None` if the report id is unrecognised (silently drop unknown reports
/// so the host can write future reports without breaking us).
pub fn parse_led(buf: &[u8]) -> Option<LedReport> {
    if buf.len() < 2 {
        return None;
    }
    let bit = (buf[1] & 0x01) != 0;
    match buf[0] {
        0x05 => Some(LedReport::OffHook(bit)),
        0x06 => Some(LedReport::Mute(bit)),
        0x07 => Some(LedReport::Ringing(bit)),
        _ => None,
    }
}

/// Where one read from `/dev/hidg0` goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Led(LedReport),
    /// Report 0x03: the Wave Controller PC link.
    Vendor,
    Unknown,
}

pub fn route(buf: &[u8]) -> Route {
    if buf.first() == Some(&crate::pclink::vendor::REPORT_ID) {
        return Route::Vendor;
    }
    parse_led(buf).map_or(Route::Unknown, Route::Led)
}

/// A vendor read as the 64-byte report the PC link expects (zero-padded).
pub fn vendor_report(buf: &[u8]) -> Vec<u8> {
    let mut report = buf[..buf.len().min(crate::pclink::vendor::REPORT_LEN)].to_vec();
    report.resize(crate::pclink::vendor::REPORT_LEN, 0);
    report
}

/// Apply a parsed `LedReport`: bump the matching counter and store the
/// bit into both `HidStats` mirrors and the shared `AudioState`.
pub fn apply_led(led: LedReport, stats: &HidStats, audio_tx: &watch::Sender<AudioState>) {
    match led {
        LedReport::OffHook(b) => {
            stats.leds_off_hook.fetch_add(1, Ordering::Relaxed);
            stats.call_off_hook.store(b, Ordering::Relaxed);
            audio_tx.send_modify(|s| s.call_off_hook = b);
        }
        LedReport::Mute(b) => {
            stats.leds_mute.fetch_add(1, Ordering::Relaxed);
            stats.call_host_muted.store(b, Ordering::Relaxed);
            audio_tx.send_modify(|s| s.call_host_muted = b);
        }
        LedReport::Ringing(b) => {
            stats.leds_ringing.fetch_add(1, Ordering::Relaxed);
            stats.call_ringing.store(b, Ordering::Relaxed);
            audio_tx.send_modify(|s| s.call_ringing = b);
        }
    }
}

/// What one turn of the reader did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// A report of this many bytes is in the buffer.
    Report(usize),
    /// Nothing came for `READ_WAIT`.
    Idle,
    /// No node to read: wait this long, then turn again.
    Down(Duration),
}

/// The reader thread's loop, one turn at a time.
pub struct ReadLoop<N: Node> {
    node: Reopening<N>,
    device: String,
    outage: Outage,
    /// Check that the path still names the node before the next read. Off
    /// while reports keep coming (they prove the node alive), on after a
    /// quiet `READ_WAIT`: a read on a node the gadget switch replaced may
    /// never return anything (Linux 5.4 `f_hid`).
    verify: bool,
    opened_once: bool,
}

impl<N: Node> ReadLoop<N> {
    pub fn new(node: N, device: &str, every: Duration) -> Self {
        Self {
            node: Reopening::new(node, every),
            device: device.to_owned(),
            outage: Outage::default(),
            verify: true,
            opened_once: false,
        }
    }

    /// One read, or the wait before the next try while there is no node.
    pub fn turn(&mut self, now: Instant, buf: &mut [u8]) -> Turn {
        let trusted = !self.verify && self.node.is_open();
        let got = if trusted {
            self.node.open_handle().map(|h| (h, Opened::Already)).ok_or(Unavailable::Waiting)
        } else {
            self.node.get(now)
        };
        let h = match got {
            Ok((h, opened)) => {
                match opened {
                    Opened::Fresh if !self.opened_once => info!("hid-reader: opened {} for read", self.device),
                    Opened::Replaced => {
                        info!("hid-reader: {} is a new node (the gadget was switched); reading it", self.device)
                    }
                    _ => {}
                }
                self.opened_once = true;
                h
            }
            Err(Unavailable::Waiting) => return Turn::Down(self.node.wait(now)),
            Err(Unavailable::OpenFailed(e)) => {
                let what = format!("cannot open {} for read: {e}", self.device);
                self.failed(what);
                return Turn::Down(self.node.wait(now));
            }
        };
        match h.read_report(buf, READ_WAIT) {
            Ok(Some(n)) => {
                self.verify = false;
                self.worked();
                Turn::Report(n)
            }
            Ok(None) => {
                self.verify = true;
                self.worked();
                Turn::Idle
            }
            Err(e) => {
                self.node.failed(now);
                self.verify = true;
                let what = format!("read from {} failed: {e}", self.device);
                self.failed(what);
                Turn::Down(self.node.wait(now))
            }
        }
    }

    fn failed(&mut self, what: String) {
        if self.outage.failed() {
            warn!("hid-reader: {what}; opening it again every second, quietly until it works");
        } else {
            debug!("hid-reader: {what}");
        }
    }

    fn worked(&mut self) {
        if let Some(n) = self.outage.ended() {
            info!("hid-reader: reading {} again after {n} failures", self.device);
        }
    }
}

#[derive(Debug, Clone)]
pub struct HidReaderConfig {
    pub device: String,
}

pub struct HidReader;

impl HidReader {
    /// Spawn a blocking std::thread that reads /dev/hidg0 and forwards
    /// LED reports into AudioState. Returns immediately; the thread runs
    /// for the process lifetime.
    pub fn spawn(
        cfg: HidReaderConfig,
        audio_tx: Arc<watch::Sender<AudioState>>,
        stats: Arc<HidStats>,
    ) {
        Self::spawn_with_vendor(cfg, audio_tx, stats, None);
    }

    /// Like `spawn`, and vendor reports 0x03 go to `vendor_tx` (the PC link,
    /// `pclink::service`) instead of being dropped.
    pub fn spawn_with_vendor(
        cfg: HidReaderConfig,
        audio_tx: Arc<watch::Sender<AudioState>>,
        stats: Arc<HidStats>,
        vendor_tx: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    ) {
        std::thread::spawn(move || {
            let mut reader = ReadLoop::new(DevNode::reader(&cfg.device), &cfg.device, REOPEN_EVERY);
            let mut buf = [0u8; 64];
            loop {
                match reader.turn(Instant::now(), &mut buf) {
                    Turn::Report(n) => match route(&buf[..n]) {
                        Route::Led(led) => apply_led(led, &stats, &audio_tx),
                        Route::Vendor => match &vendor_tx {
                            Some(tx) if tx.send(vendor_report(&buf[..n])).is_ok() => {}
                            _ => {
                                stats.glue_drops.fetch_add(1, Ordering::Relaxed);
                            }
                        },
                        Route::Unknown => {
                            stats.glue_drops.fetch_add(1, Ordering::Relaxed);
                        }
                    },
                    Turn::Idle => {}
                    // Never less than 10 ms: nothing spins while the node is missing.
                    Turn::Down(wait) => std::thread::sleep(wait.max(Duration::from_millis(10))),
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::watch;
    use waved_state::AudioState;

    #[test]
    fn vendor_reports_go_to_the_pc_link_and_leds_stay_leds() {
        let mut vendor = [0u8; 64];
        vendor[0] = 0x03;
        vendor[1] = 0xEC;
        assert_eq!(route(&vendor), Route::Vendor);
        assert_eq!(route(&[0x06, 0x01]), Route::Led(LedReport::Mute(true)));
        assert_eq!(route(&[0x09, 0x01]), Route::Unknown);
        assert_eq!(route(&[0x03]), Route::Vendor, "short vendor reads are still vendor reads");
    }

    #[test]
    fn vendor_reads_are_padded_to_64_bytes() {
        let mut short = vec![0x03, 0x00, 0x01];
        short.resize(10, 0xAA);
        let padded = vendor_report(&short);
        assert_eq!(padded.len(), 64);
        assert_eq!(&padded[..10], &short[..]);
        assert!(padded[10..].iter().all(|&b| b == 0));
    }

    #[test]
    fn parse_off_hook_on() {
        assert_eq!(parse_led(&[0x05, 0x01]), Some(LedReport::OffHook(true)));
    }

    #[test]
    fn parse_off_hook_off() {
        assert_eq!(parse_led(&[0x05, 0x00]), Some(LedReport::OffHook(false)));
    }

    #[test]
    fn parse_mute_on() {
        assert_eq!(parse_led(&[0x06, 0x01]), Some(LedReport::Mute(true)));
    }

    #[test]
    fn parse_ringing_on() {
        assert_eq!(parse_led(&[0x07, 0x01]), Some(LedReport::Ringing(true)));
    }

    #[test]
    fn parse_unknown_report_id_returns_none() {
        assert_eq!(parse_led(&[0x03, 0xff]), None);
    }

    #[test]
    fn parse_short_buf_returns_none() {
        assert_eq!(parse_led(&[0x05]), None);
        assert_eq!(parse_led(&[]), None);
    }

    #[test]
    fn parse_ignores_padding_bits() {
        // Per stock descriptor, bits 1-7 of the LED byte are padding.
        assert_eq!(parse_led(&[0x05, 0xfe]), Some(LedReport::OffHook(false)));
        assert_eq!(parse_led(&[0x05, 0xff]), Some(LedReport::OffHook(true)));
    }

    #[test]
    fn apply_led_off_hook_updates_state_and_stats() {
        let (tx, rx) = watch::channel(AudioState::default());
        let stats = HidStats::default();
        apply_led(LedReport::OffHook(true), &stats, &tx);
        let s = rx.borrow();
        assert!(s.call_off_hook);
        assert!(!s.call_ringing,    "Off-hook LED must not touch call_ringing");
        assert!(!s.call_host_muted, "Off-hook LED must not touch call_host_muted");
        assert_eq!(stats.leds_off_hook.load(Ordering::Relaxed), 1);
        assert!(stats.call_off_hook.load(Ordering::Relaxed));
    }

    #[test]
    fn apply_led_ringing_updates_state_and_stats() {
        let (tx, rx) = watch::channel(AudioState::default());
        let stats = HidStats::default();
        apply_led(LedReport::Ringing(true), &stats, &tx);
        let s = rx.borrow();
        assert!(s.call_ringing);
        assert!(!s.call_off_hook,   "Ringing LED must not touch call_off_hook");
        assert!(!s.call_host_muted, "Ringing LED must not touch call_host_muted");
        assert_eq!(stats.leds_ringing.load(Ordering::Relaxed), 1);
        assert!(stats.call_ringing.load(Ordering::Relaxed));
    }

    use crate::node::fake::{FakeNode, Read};

    fn read_loop(node: &FakeNode) -> ReadLoop<FakeNode> {
        ReadLoop::new(node.clone(), "/dev/hidg0", REOPEN_EVERY)
    }

    #[test]
    fn a_node_replaced_under_a_waiting_read_is_noticed_and_the_new_one_read() {
        let node = FakeNode::present();
        node.state().reads.push_back(Read::Report(vec![0x05, 0x01]));
        let mut r = read_loop(&node);
        let mut buf = [0u8; 64];
        let t0 = Instant::now();
        assert_eq!(r.turn(t0, &mut buf), Turn::Report(2));
        // The switch: reads on the old node just wait (5.4 f_hid).
        node.unbind();
        assert_eq!(r.turn(t0, &mut buf), Turn::Idle);
        let Turn::Down(wait) = r.turn(t0, &mut buf) else { panic!("the path is checked after a quiet wait") };
        assert_eq!(wait, REOPEN_EVERY);
        assert!(matches!(r.turn(t0 + Duration::from_millis(500), &mut buf), Turn::Down(_)));
        assert_eq!(node.opens(), 2, "no open within the back-off");
        node.bind();
        node.state().reads.push_back(Read::Report(vec![0x07, 0x01, 0x00]));
        assert_eq!(r.turn(t0 + REOPEN_EVERY, &mut buf), Turn::Report(3));
        assert_eq!(&buf[..2], &[0x07, 0x01]);
    }

    #[test]
    fn a_node_switched_between_two_quiet_waits_is_read_without_a_back_off() {
        let node = FakeNode::present();
        let mut r = read_loop(&node);
        let mut buf = [0u8; 64];
        let t0 = Instant::now();
        assert_eq!(r.turn(t0, &mut buf), Turn::Idle);
        node.unbind();
        node.bind();
        node.state().reads.push_back(Read::Report(vec![0x06, 0x01]));
        assert_eq!(r.turn(t0, &mut buf), Turn::Report(2));
        assert_eq!(node.opens(), 2);
    }

    #[test]
    fn a_read_error_opens_the_node_again_after_the_back_off_and_nothing_spins() {
        let node = FakeNode::present();
        node.state().reads.push_back(Read::Fail(libc::EIO));
        let mut r = read_loop(&node);
        let mut buf = [0u8; 64];
        let t0 = Instant::now();
        assert_eq!(r.turn(t0, &mut buf), Turn::Down(REOPEN_EVERY));
        // A missing node: every turn within the back-off only waits.
        node.unbind();
        for ms in [0, 10, 500, 999] {
            assert!(matches!(r.turn(t0 + Duration::from_millis(ms), &mut buf), Turn::Down(d) if d > Duration::ZERO));
        }
        assert_eq!(node.opens(), 1);
        assert_eq!(r.turn(t0 + REOPEN_EVERY, &mut buf), Turn::Down(REOPEN_EVERY), "tried, still missing");
        assert_eq!(node.opens(), 2);
        node.bind();
        node.state().reads.push_back(Read::Report(vec![0x05, 0x00]));
        assert_eq!(r.turn(t0 + 2 * REOPEN_EVERY, &mut buf), Turn::Report(2));
    }

    #[test]
    fn the_reader_reads_the_new_node_after_a_gadget_switch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hidg0");
        std::fs::write(&path, [0x05, 0x01]).unwrap();
        let (tx, rx) = watch::channel(AudioState::default());
        let stats = HidStats::new();
        HidReader::spawn(
            HidReaderConfig { device: path.to_string_lossy().into_owned() },
            Arc::new(tx),
            Arc::clone(&stats),
        );
        let wait_until = |what: &dyn Fn(&AudioState) -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
            while !what(&rx.borrow()) && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            what(&rx.borrow())
        };
        assert!(wait_until(&|s| s.call_off_hook), "the first node's report");
        // The switch: the old node ends, a new one comes at the same path.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, [0x07, 0x01]).unwrap();
        assert!(wait_until(&|s| s.call_ringing), "the new node's report");
    }

    #[test]
    fn apply_led_mute_updates_call_host_muted_only() {
        let (tx, rx) = watch::channel(AudioState::default());
        let stats = HidStats::default();
        apply_led(LedReport::Mute(true), &stats, &tx);
        let s = rx.borrow();
        assert!(s.call_host_muted);
        assert!(!s.call_off_hook, "Mute LED must not touch call_off_hook");
        assert!(!s.call_ringing,  "Mute LED must not touch call_ringing");
        assert!(!s.speaker_muted, "Mute LED must not touch speaker_muted");
        assert!(!s.mic_muted,     "Mute LED must not touch mic_muted");
    }
}
