//! UAC2 routing — bridges the kernel `f_uac2` gadget endpoint to the playback
//! chain (host → speaker) and the post-AEC mic frame back upstream
//! (mic → host). See `docs/AUDIO_HARDWARE.md` "UAC2 routing (SP#6)".

pub mod bridge;
pub mod upstream;
pub mod control;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Atomic counters surfaced into `/tmp/audio_stats.json` via `stats_collector`.
/// Each component (bridge, upstream, control) writes its own subset.
#[derive(Debug, Default)]
pub struct UacStats {
    /// `true` when the host has `hw:1,0c` open and is writing audio.
    pub downstream_active: AtomicBool,
    /// `true` when the host has `hw:1,0p` open and is reading audio.
    pub upstream_active: AtomicBool,
    /// Periods dropped on the downstream side because `play_batch` queue
    /// depth exceeded `buffer_periods` (host clock outran device clock).
    pub downstream_drops: AtomicU64,
    /// Periods dropped on the upstream side because the host couldn't drain
    /// fast enough (device clock outran host clock).
    pub upstream_drops: AtomicU64,
    /// ALSA xruns on the downstream `readi` path.
    pub downstream_underruns: AtomicU64,
    /// ALSA xruns on the upstream `writei` path.
    pub upstream_underruns: AtomicU64,
    /// Counter: host SET_CUR events on volume/mute (Mac → device).
    pub host_set_volume_events: AtomicU64,
    /// Counter: device-initiated mixer writes (touch UI → host mirror).
    pub device_set_volume_events: AtomicU64,
}

impl UacStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Snapshot for the stats collector. Single-shot read of every atomic.
    pub fn snapshot(&self) -> UacStatsSnapshot {
        UacStatsSnapshot {
            downstream_active: self.downstream_active.load(Ordering::Relaxed),
            upstream_active: self.upstream_active.load(Ordering::Relaxed),
            downstream_drops: self.downstream_drops.load(Ordering::Relaxed),
            upstream_drops: self.upstream_drops.load(Ordering::Relaxed),
            downstream_underruns: self.downstream_underruns.load(Ordering::Relaxed),
            upstream_underruns: self.upstream_underruns.load(Ordering::Relaxed),
            host_set_volume_events: self.host_set_volume_events.load(Ordering::Relaxed),
            device_set_volume_events: self.device_set_volume_events.load(Ordering::Relaxed),
        }
    }
}

/// Plain serialisable snapshot — what the stats writer emits as JSON.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UacStatsSnapshot {
    pub downstream_active: bool,
    pub upstream_active: bool,
    pub downstream_drops: u64,
    pub upstream_drops: u64,
    pub downstream_underruns: u64,
    pub upstream_underruns: u64,
    pub host_set_volume_events: u64,
    pub device_set_volume_events: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_snapshot_starts_zero() {
        let s = UacStats::default();
        let snap = s.snapshot();
        assert!(!snap.downstream_active);
        assert!(!snap.upstream_active);
        assert_eq!(snap.downstream_drops, 0);
        assert_eq!(snap.host_set_volume_events, 0);
    }

    #[test]
    fn stats_snapshot_reflects_writes() {
        let s = UacStats::default();
        s.downstream_active.store(true, Ordering::Relaxed);
        s.downstream_drops.fetch_add(3, Ordering::Relaxed);
        s.host_set_volume_events.fetch_add(7, Ordering::Relaxed);
        let snap = s.snapshot();
        assert!(snap.downstream_active);
        assert_eq!(snap.downstream_drops, 3);
        assert_eq!(snap.host_set_volume_events, 7);
    }
}
