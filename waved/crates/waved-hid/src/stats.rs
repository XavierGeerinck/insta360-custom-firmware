//! Atomic stats surfaced into `/tmp/audio_stats.json` via the
//! waved-audio stats_collector.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct HidStats {
    pub device_present: AtomicBool,
    pub events_volume_step: AtomicU64,
    pub events_mute: AtomicU64,
    pub events_phone_mute: AtomicU64,
    pub events_play_pause: AtomicU64,
    pub events_next: AtomicU64,
    pub events_prev: AtomicU64,
    pub leds_off_hook: AtomicU64,
    pub leds_ringing: AtomicU64,
    pub leds_mute: AtomicU64,
    pub call_off_hook: AtomicBool,
    pub call_ringing: AtomicBool,
    pub call_host_muted: AtomicBool,
    pub write_errors: AtomicU64,
    pub glue_drops: AtomicU64,
}

impl HidStats {
    pub fn new() -> Arc<Self> { Arc::new(Self::default()) }

    pub fn snapshot(&self) -> HidStatsSnapshot {
        HidStatsSnapshot {
            device_present: self.device_present.load(Ordering::Relaxed),
            events_emitted: HidEventCounts {
                volume_step: self.events_volume_step.load(Ordering::Relaxed),
                mute: self.events_mute.load(Ordering::Relaxed),
                phone_mute: self.events_phone_mute.load(Ordering::Relaxed),
                play_pause: self.events_play_pause.load(Ordering::Relaxed),
                next: self.events_next.load(Ordering::Relaxed),
                prev: self.events_prev.load(Ordering::Relaxed),
            },
            leds_received: HidLedCounts {
                off_hook: self.leds_off_hook.load(Ordering::Relaxed),
                ringing: self.leds_ringing.load(Ordering::Relaxed),
                mute: self.leds_mute.load(Ordering::Relaxed),
            },
            call_off_hook: self.call_off_hook.load(Ordering::Relaxed),
            call_ringing: self.call_ringing.load(Ordering::Relaxed),
            call_host_muted: self.call_host_muted.load(Ordering::Relaxed),
            write_errors: self.write_errors.load(Ordering::Relaxed),
            glue_drops: self.glue_drops.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HidStatsSnapshot {
    pub device_present: bool,
    pub events_emitted: HidEventCounts,
    pub leds_received: HidLedCounts,
    pub call_off_hook: bool,
    pub call_ringing: bool,
    pub call_host_muted: bool,
    pub write_errors: u64,
    pub glue_drops: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HidEventCounts {
    pub volume_step: u64,
    pub mute: u64,
    pub phone_mute: u64,
    pub play_pause: u64,
    pub next: u64,
    pub prev: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HidLedCounts {
    pub off_hook: u64,
    pub ringing: u64,
    pub mute: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_starts_zero() {
        let s = HidStats::default();
        let snap = s.snapshot();
        assert!(!snap.device_present);
        assert_eq!(snap.events_emitted.volume_step, 0);
    }

    #[test]
    fn snapshot_reflects_writes() {
        let s = HidStats::default();
        s.device_present.store(true, Ordering::Relaxed);
        s.events_volume_step.fetch_add(7, Ordering::Relaxed);
        s.leds_off_hook.fetch_add(2, Ordering::Relaxed);
        s.call_off_hook.store(true, Ordering::Relaxed);
        let snap = s.snapshot();
        assert!(snap.device_present);
        assert_eq!(snap.events_emitted.volume_step, 7);
        assert_eq!(snap.leds_received.off_hook, 2);
        assert!(snap.call_off_hook);
    }
}
