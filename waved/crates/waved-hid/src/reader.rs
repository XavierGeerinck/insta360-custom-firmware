//! HID Reader — blocking std::thread owning the read side of /dev/hidg0.
//! Parses host Set_Report writes (Reports 0x05 / 0x06 / 0x07) into
//! AudioState.call_*, and hands vendor report 0x03 (the Wave Controller
//! PC link, `crate::pclink`) to its channel.

use std::io::Read;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::sync::watch;
use tracing::{info, warn};

use waved_state::AudioState;

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

#[derive(Debug, Clone)]
pub struct HidReaderConfig {
    pub device: String,
}

pub struct HidReader;

impl HidReader {
    /// Spawn a blocking std::thread that reads /dev/hidg0 and forwards
    /// LED reports into AudioState. Returns immediately; the thread runs
    /// until EOF or read error.
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
            let mut f = match std::fs::OpenOptions::new().read(true).open(&cfg.device) {
                Ok(f) => {
                    info!("hid-reader: opened {} for read", cfg.device);
                    f
                }
                Err(e) => {
                    warn!(
                        "hid-reader: cannot open {} for read: {} — LED mirror disabled",
                        cfg.device, e
                    );
                    return;
                }
            };
            let mut buf = [0u8; 64];
            loop {
                match f.read(&mut buf) {
                    Ok(0) => {
                        warn!("hid-reader: EOF on {}, exiting", cfg.device);
                        return;
                    }
                    Ok(n) => match route(&buf[..n]) {
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
                    Err(e) => {
                        warn!("hid-reader: read error: {} — exiting", e);
                        return;
                    }
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
