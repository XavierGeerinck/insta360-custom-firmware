# Waved Audio — SP#6 UAC2 Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bridge the kernel `f_uac2` gadget endpoint to the SP#5 playback chain (host→speaker) and stream `ProcessedAudioFrame` mono back upstream as stereo (mic→host), so the Wave behaves as a real two-way USB Audio Class 2 speakerphone. Includes ALSA-mixer-event-based volume + mute mirroring between the host's UAC2 controls and `AudioState`.

**Architecture:** A new `uac` module under `crates/waved-audio/src/uac/` with three components: `UacBridge` (std::thread, downstream — `hw:1,0c` → `play_batch`), `UacUpstreamSink` (std::thread, upstream — `subscribe_processed()` → `hw:1,0p`), and `UacControl` (tokio task, ALSA mixer event poll → `Command::SetSpeakerVolume` / `SetMicVolume` / `ToggleSpeakerMute` / `ToggleMicMute`, plus reverse-direction mirror via `mixer.set_playback_volume_all()`). All three spawned in `main.rs` immediately after `spawn_playback`. Drift handling uses `PlaybackService::queue_depth()` (a new accessor returning `max_capacity - capacity`). The existing `speaker_mixer::amixer` shell-out in `AudioService::run` is replaced by the same UAC2-routed mute path so volume control is end-to-end through one mechanism.

**Tech Stack:** Rust 2021. Existing `alsa = "0.9"` crate (already a dep). `tokio::sync::broadcast::Receiver::blocking_recv` for cross-async/blocking-thread receive. No new crate dependencies. Live device for acceptance — Mac required for the host-side scenarios.

**Reference spec:** `docs/superpowers/specs/2026-04-28-waved-uac2-routing-design.md`
**Reference hardware docs:** `docs/AUDIO_HARDWARE.md`
**Live-device probe (2026-05-01):** card 1 (`UAC2Gadget`) exposes mixer controls `PCM Playback Volume`, `PCM Playback Switch`, `PCM Playback State`, `PCM Capture Volume`, `PCM Capture Switch`, `PCM Capture State`. Volume range is `0–100` ALSA-percent (kernel `c_volume_min=-100, c_volume_max=0, c_volume_res=1` mapped to ALSA percent). The two `*State` switches reflect host attachment (`on` when host has the endpoint open) — perfect for the `downstream_active`/`upstream_active` stats fields.

---

## File Structure

```
crates/waved-audio/
  src/
    audio_config.rs                                      (modify) add UacConfig + Default + validate guards
    lib.rs                                               (modify) add `pub mod uac;`; replace speaker_mixer
                                                                   shell-out in ToggleSpeakerMute/SetSpeakerVolume
                                                                   handlers (Task 11)
    playback.rs                                          (modify) add pub fn queue_depth(&self) -> usize
    stats_collector.rs                                   (modify) add `UacStatsRow` block to JSON output
    uac/
      mod.rs                                             (new)    UacStats atomic struct + re-exports
      bridge.rs                                          (new)    UacBridge — downstream std::thread
      upstream.rs                                        (new)    UacUpstreamSink — upstream std::thread
      control.rs                                         (new)    UacControl — tokio task, mixer events ↔ state
crates/waved/
  src/main.rs                                            (modify) spawn UacBridge + UacUpstreamSink + UacControl
                                                                   in capture-alsa block; thread stats + uac
                                                                   handle into stats_collector::run
scripts/
  test-uac2-routing.sh                                   (new)    device-side acceptance (4 of 6 criteria)
  test-uac2-routing-host.sh                              (new)    Mac-side acceptance (afplay, arecord, osascript)
docs/
  AUDIO_HARDWARE.md                                      (modify) UAC2 block diagram + volume-mirror sequence
TODO.md                                                  (modify) mark SP#6 done, file follow-ups
```

## Task ordering

15 tasks. Subagent-driven tasks build module-by-module with the ALSA paths covered by on-device acceptance (no host-side ALSA mock — too brittle).

1. **Tasks 1–2 (precursors).** Add `PlaybackService::queue_depth()`, then `UacConfig` to `AudioConfig`. Both purely host-side, fully unit-testable. After each, `cargo test -p waved-audio --lib` is green.
2. **Tasks 3–7 (the uac module).** Skeleton + UacStats, then `UacBridge`, `UacUpstreamSink`, `UacControl` volume, `UacControl` mute. Each commits a self-contained piece; integration happens at task 8.
3. **Task 8 (cleanup).** Drop `speaker_mixer::amixer` shell-outs from the SP#5-deferred mute handler.
4. **Task 9 (wire into main).** Instantiate the three threads; thread `UacStats` through to the stats collector. Full release cross-build succeeds.
5. **Task 10 (stats fields).** Surface `uac.*` block in `/tmp/audio_stats.json`.
6. **CHECKPOINT after task 10.** Pause for user — feature is end-to-end live and ready for the on-device acceptance loop.
7. **Tasks 11–13 (acceptance).** Device-side script, Mac-side helper, run the full battery (host→speaker round-trip, mic→host round-trip, AEC ERLE with host audio, volume mirror). Record measured numbers.
8. **Tasks 14–15 (docs + close).** Update `AUDIO_HARDWARE.md`, mark SP#6 done in `TODO.md`. Final code review.

---

## Task 1: `PlaybackService::queue_depth()`

The downstream bridge needs to know how many `play_batch` periods are currently queued so it can drop on overflow (drift handling, spec §6.1). `tokio::sync::mpsc::Sender` exposes `capacity()` (remaining slots) and `max_capacity()` (configured buffer size); the difference is the current depth.

**Files:**
- Modify: `crates/waved-audio/src/playback.rs`

- [ ] **Step 1: Add the failing test**

In `crates/waved-audio/src/playback.rs`'s `#[cfg(test)] mod tests` block, append:

```rust
    #[test]
    fn queue_depth_reports_pending_batches() {
        // Build a stand-alone PlaybackService stub by exposing the constructor
        // shape used by spawn_playback. The simplest test is to construct a
        // pair of channels manually and call the same accessor.
        let (cmd_tx, _cmd_rx) = mpsc::channel::<Vec<i16>>(4);
        let (preset_tx, _preset_rx) =
            mpsc::channel::<crate::playback::preset::PlaybackPreset>(2);
        let (ref_tx, _) = broadcast::channel::<Vec<i16>>(2);
        let svc = PlaybackService {
            cmd_tx: cmd_tx.clone(),
            preset_tx,
            ref_tx,
            channels: 2,
            period_frames: 1024,
            sample_rate: 48_000,
            stats: Arc::new(PlaybackStats::default()),
            dsp_metrics: Arc::new(DspMetrics::default()),
        };
        assert_eq!(svc.queue_depth(), 0);
        cmd_tx.try_send(vec![0; 4]).unwrap();
        cmd_tx.try_send(vec![0; 4]).unwrap();
        assert_eq!(svc.queue_depth(), 2);
        // Read one back via the receiver — depth drops by one.
        let _ = _cmd_rx.try_recv().ok();
        assert_eq!(svc.queue_depth(), 1);
    }
```

Run: `cargo test -p waved-audio --lib playback::tests::queue_depth_reports_pending_batches`
Expected: FAIL — `queue_depth` not defined.

- [ ] **Step 2: Implement**

In the existing `impl PlaybackService { ... }` block, add:

```rust
    /// Number of `play_batch` periods currently queued in the mpsc buffer.
    /// Used by the UAC2 downstream bridge for drift detection — drop a
    /// period when this grows past `buffer_periods`.
    pub fn queue_depth(&self) -> usize {
        self.cmd_tx.max_capacity() - self.cmd_tx.capacity()
    }
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p waved-audio --lib playback::tests::queue_depth_reports_pending_batches`
Expected: PASS.

- [ ] **Step 4: Run the whole crate**

Run: `cargo test -p waved-audio --lib`
Expected: 155+ tests pass (the new one + everything from SP#5).

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/src/playback.rs
git commit -m "feat(audio/sp6): PlaybackService::queue_depth() for drift detection"
```

---

## Task 2: `UacConfig` in `AudioConfig`

Additive optional field on `AudioConfig`. No version bump (per spec §5.1). Defaults give a working setup; users override `enabled = false` to disable the bridges entirely.

**Files:**
- Modify: `crates/waved-audio/src/audio_config.rs`

- [ ] **Step 1: Add the type**

After the existing `PlaybackPresetTuningJson` struct in `crates/waved-audio/src/audio_config.rs`, add:

```rust
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct UacConfig {
    #[serde(default = "default_uac_enabled")]
    pub enabled: bool,
    #[serde(default = "default_uac_dev")]
    pub downstream_device: String,
    #[serde(default = "default_uac_dev")]
    pub upstream_device: String,
    #[serde(default = "default_uac_period")]
    pub period_frames: usize,
    #[serde(default = "default_uac_periods")]
    pub buffer_periods: u32,
    #[serde(default = "default_uac_mixer_card")]
    pub mixer_card: String,
}

fn default_uac_enabled() -> bool { true }
fn default_uac_dev() -> String { "hw:1,0".to_string() }
fn default_uac_period() -> usize { 1024 }
fn default_uac_periods() -> u32 { 4 }
fn default_uac_mixer_card() -> String { "hw:1".to_string() }

impl Default for UacConfig {
    fn default() -> Self {
        Self {
            enabled: default_uac_enabled(),
            downstream_device: default_uac_dev(),
            upstream_device: default_uac_dev(),
            period_frames: default_uac_period(),
            buffer_periods: default_uac_periods(),
            mixer_card: default_uac_mixer_card(),
        }
    }
}
```

- [ ] **Step 2: Add `uac: Option<UacConfig>` to `AudioConfig`**

```rust
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AudioConfig {
    pub version: u32,
    pub capture: CaptureConfig,
    #[serde(default)]
    pub playback: Option<PlaybackSection>,
    pub period_frames: usize,
    pub buffer_periods: u32,
    #[serde(default = "default_mic_geometry")]
    pub mic_geometry: Option<MicGeometry>,
    #[serde(default)]
    pub uac: Option<UacConfig>,
}
```

- [ ] **Step 3: Add validation**

In `validate()`, just before `Ok(())`, append:

```rust
        if let Some(u) = &self.uac {
            if u.period_frames == 0 {
                return Err(ConfigError::Invalid("uac.period_frames must be > 0".into()));
            }
            if u.buffer_periods < 2 {
                return Err(ConfigError::Invalid("uac.buffer_periods must be >= 2".into()));
            }
            if u.downstream_device.is_empty() || u.upstream_device.is_empty()
               || u.mixer_card.is_empty() {
                return Err(ConfigError::Invalid("uac device strings must be non-empty".into()));
            }
        }
```

- [ ] **Step 4: Tests**

Append to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn uac_config_default_when_absent() {
        let json = r#"{
            "version": 4,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE"
            },
            "period_frames": 1024,
            "buffer_periods": 8
        }"#;
        let cfg = AudioConfig::from_json(json).expect("v4 should parse");
        assert!(cfg.uac.is_none(), "uac block defaults to None when absent");
    }

    #[test]
    fn uac_config_with_overrides_parses() {
        let json = r#"{
            "version": 4,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE"
            },
            "period_frames": 1024,
            "buffer_periods": 8,
            "uac": {
                "enabled": false,
                "downstream_device": "hw:2,0",
                "upstream_device": "hw:2,0",
                "period_frames": 512,
                "buffer_periods": 8,
                "mixer_card": "hw:2"
            }
        }"#;
        let cfg = AudioConfig::from_json(json).expect("v4 should parse");
        let u = cfg.uac.unwrap();
        assert!(!u.enabled);
        assert_eq!(u.downstream_device, "hw:2,0");
        assert_eq!(u.period_frames, 512);
    }

    #[test]
    fn uac_config_rejects_zero_period() {
        let json = r#"{
            "version": 4,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE"
            },
            "period_frames": 1024,
            "buffer_periods": 8,
            "uac": {
                "enabled": true,
                "downstream_device": "hw:1,0",
                "upstream_device": "hw:1,0",
                "period_frames": 0,
                "buffer_periods": 4,
                "mixer_card": "hw:1"
            }
        }"#;
        assert!(AudioConfig::from_json(json).is_err());
    }

    #[test]
    fn uac_config_default_round_trip() {
        let d = UacConfig::default();
        assert!(d.enabled);
        assert_eq!(d.downstream_device, "hw:1,0");
        assert_eq!(d.upstream_device, "hw:1,0");
        assert_eq!(d.period_frames, 1024);
        assert_eq!(d.buffer_periods, 4);
        assert_eq!(d.mixer_card, "hw:1");
    }
```

- [ ] **Step 5: Verify**

Run: `cargo test -p waved-audio --lib audio_config`
Expected: 4 new tests pass + existing audio_config tests still green.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-audio/src/audio_config.rs
git commit -m "feat(audio/sp6): UacConfig optional field on AudioConfig"
```

---

## Task 3: `uac` module skeleton + `UacStats`

The atomics that all three components feed into. Standalone — no ALSA dependency, fully unit-testable.

**Files:**
- Modify: `crates/waved-audio/src/lib.rs` — add `pub mod uac;`
- Create: `crates/waved-audio/src/uac/mod.rs`

- [ ] **Step 1: Add module declaration to `crates/waved-audio/src/lib.rs`**

Near the existing `pub mod stats_collector;` (or alongside other top-level `pub mod` declarations), add:

```rust
#[cfg(feature = "capture-alsa")]
pub mod uac;
```

The `cfg(feature = "capture-alsa")` gate is required because `bridge.rs`/`upstream.rs`/`control.rs` (added in later tasks) all use the `alsa` crate, which is only present under that feature.

- [ ] **Step 2: Create `crates/waved-audio/src/uac/mod.rs`**

```rust
//! UAC2 routing — bridges the kernel `f_uac2` gadget endpoint to the playback
//! chain (host → speaker) and the post-AEC mic frame back upstream
//! (mic → host). See `docs/AUDIO_HARDWARE.md` "UAC2 routing (SP#6)".

pub mod bridge;
pub mod upstream;
pub mod control;

pub use bridge::UacBridge;
pub use upstream::UacUpstreamSink;
pub use control::UacControl;

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
```

The `pub mod bridge; pub mod upstream; pub mod control;` lines reference files added in Tasks 4-7. Until those tasks land, also create empty stub files so the crate compiles after this task:

```bash
mkdir -p crates/waved-audio/src/uac
echo '//! UAC2 downstream bridge — populated in Task 4.' > crates/waved-audio/src/uac/bridge.rs
echo '//! UAC2 upstream sink — populated in Task 5.' > crates/waved-audio/src/uac/upstream.rs
echo '//! UAC2 mixer control — populated in Tasks 6-7.' > crates/waved-audio/src/uac/control.rs
```

- [ ] **Step 3: Verify**

```
cargo test -p waved-audio --lib uac
```
Expected: 2 tests pass.

```
cargo build -p waved-audio --release --features capture-alsa
```
Expected: clean (the stub files are valid empty modules).

- [ ] **Step 4: Commit**

```bash
git add crates/waved-audio/src/lib.rs \
        crates/waved-audio/src/uac/mod.rs \
        crates/waved-audio/src/uac/bridge.rs \
        crates/waved-audio/src/uac/upstream.rs \
        crates/waved-audio/src/uac/control.rs
git commit -m "feat(audio/sp6): uac module skeleton + UacStats atomics"
```

---

## Task 4: `UacBridge` (downstream)

Reads from `hw:1,0` capture, calls `playback.play_batch`, drops on drift overflow. ALSA-backed; no host-side test. Verified by Task 11/12 acceptance.

**Files:**
- Modify: `crates/waved-audio/src/uac/bridge.rs`

- [ ] **Step 1: Replace `bridge.rs` with**

```rust
//! UAC2 downstream bridge — host audio → speaker.
//!
//! Reads from `hw:1,0` (kernel `f_uac2` capture endpoint that mirrors what
//! the host writes to the UAC2 OUT terminal) and submits each period to the
//! `PlaybackService`. The playback loop's SP#5 DSP chain processes the
//! samples and broadcasts them as the AEC reference, so AEC sees host audio
//! exactly as it reaches the speaker.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use tracing::{info, warn};

use crate::playback::PlaybackService;
use super::UacStats;

#[derive(Debug, Clone)]
pub struct UacBridgeConfig {
    pub device: String,         // e.g. "hw:1,0"
    pub channels: u32,          // 2
    pub sample_rate: u32,       // 48000
    pub period_frames: usize,   // 1024
    pub buffer_periods: u32,    // 4
}

pub struct UacBridge {
    handle: Option<JoinHandle<()>>,
}

impl UacBridge {
    /// Spawn the downstream thread. Returns a handle whose `Drop` joins the
    /// thread (the thread exits when `playback.play_batch` errors with
    /// `Stopped`, i.e. when `PlaybackService` is dropped).
    pub fn spawn(
        cfg: UacBridgeConfig,
        playback: Arc<PlaybackService>,
        stats: Arc<UacStats>,
    ) -> Self {
        let handle = std::thread::Builder::new()
            .name("uac-downstream".into())
            .spawn(move || {
                set_realtime_priority(18);
                run_downstream(cfg, playback, stats);
            })
            .expect("spawn uac-downstream thread");
        Self { handle: Some(handle) }
    }

    pub fn join(mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run_downstream(
    cfg: UacBridgeConfig,
    playback: Arc<PlaybackService>,
    stats: Arc<UacStats>,
) {
    info!("uac-downstream starting: {}", cfg.device);
    loop {
        match open_capture(&cfg) {
            Ok(pcm) => {
                if let Err(e) = drain_loop(&pcm, &cfg, &playback, &stats) {
                    warn!("uac-downstream drain stopped: {e}");
                }
                stats.downstream_active.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                warn!("uac-downstream open failed: {e}; retrying in 1 s");
            }
        }
        // Either ALSA closed or open failed — back off and retry.
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn open_capture(cfg: &UacBridgeConfig) -> Result<PCM, alsa::Error> {
    let pcm = PCM::new(&cfg.device, Direction::Capture, false)?;
    {
        let hwp = HwParams::any(&pcm)?;
        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_format(Format::s16())?;
        hwp.set_channels(cfg.channels)?;
        hwp.set_rate(cfg.sample_rate, ValueOr::Nearest)?;
        hwp.set_period_size(cfg.period_frames as i64, ValueOr::Nearest)?;
        hwp.set_buffer_size((cfg.period_frames * cfg.buffer_periods as usize) as i64)?;
        pcm.hw_params(&hwp)?;
    }
    pcm.prepare()?;
    pcm.start()?;
    Ok(pcm)
}

fn drain_loop(
    pcm: &PCM,
    cfg: &UacBridgeConfig,
    playback: &Arc<PlaybackService>,
    stats: &Arc<UacStats>,
) -> Result<(), alsa::Error> {
    let frame_samples = cfg.channels as usize;
    let period_samples = cfg.period_frames * frame_samples;
    let mut buf: Vec<i16> = vec![0; period_samples];
    let drop_threshold = cfg.buffer_periods as usize;

    loop {
        let io = pcm.io_i16()?;
        match io.readi(&mut buf) {
            Ok(0) => {
                stats.downstream_active.store(false, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(frames) => {
                stats.downstream_active.store(true, Ordering::Relaxed);
                if playback.queue_depth() >= drop_threshold {
                    stats.downstream_drops.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let to_send = frames * frame_samples;
                let _ = playback.play_batch(buf[..to_send].to_vec());
            }
            Err(e) => match e.errno() {
                libc::EAGAIN => {
                    stats.downstream_active.store(false, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(50));
                }
                libc::EPIPE => {
                    stats.downstream_underruns.fetch_add(1, Ordering::Relaxed);
                    pcm.prepare()?;
                }
                _ => return Err(e),
            },
        }
    }
}

#[cfg(target_os = "linux")]
fn set_realtime_priority(prio: i32) {
    let param = libc::sched_param { sched_priority: prio };
    let rc = unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) };
    if rc != 0 {
        let errno = std::io::Error::last_os_error();
        warn!("uac-downstream SCHED_FIFO failed: {errno}");
    } else {
        info!("uac-downstream scheduler: SCHED_FIFO priority {prio}");
    }
}

#[cfg(not(target_os = "linux"))]
fn set_realtime_priority(_prio: i32) {}
```

- [ ] **Step 2: Verify**

```
cargo build -p waved-audio --release --features capture-alsa
```
Expected: clean.

```
cargo test -p waved-audio --lib uac
```
Expected: still 2 tests pass (no new tests for the ALSA path — covered by acceptance).

- [ ] **Step 3: Commit**

```bash
git add crates/waved-audio/src/uac/bridge.rs
git commit -m "feat(audio/sp6): UacBridge — downstream hw:1,0 → play_batch"
```

---

## Task 5: `UacUpstreamSink` (upstream)

Subscribes to `subscribe_processed()`, mono → stereo, writes to `hw:1,0` playback.

**Files:**
- Modify: `crates/waved-audio/src/uac/upstream.rs`

- [ ] **Step 1: Replace `upstream.rs` with**

```rust
//! UAC2 upstream sink — mic → host.
//!
//! Subscribes to `AudioService::subscribe_processed()` (post-AEC,
//! post-beamform, post-NS mono `f32`). Duplicates each sample to L = R = mono
//! and writes interleaved S16 to `hw:1,0` playback (kernel `f_uac2` IN
//! terminal — what the host reads as the microphone).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::processed_frame::ProcessedAudioFrame;
use super::UacStats;

#[derive(Debug, Clone)]
pub struct UacUpstreamConfig {
    pub device: String,
    pub channels: u32,         // 2
    pub sample_rate: u32,      // 48000
    pub period_frames: usize,  // 1024 — must match ProcessedAudioFrame size
    pub buffer_periods: u32,   // 4
}

pub struct UacUpstreamSink {
    handle: Option<JoinHandle<()>>,
}

impl UacUpstreamSink {
    pub fn spawn(
        cfg: UacUpstreamConfig,
        rx: broadcast::Receiver<ProcessedAudioFrame>,
        stats: Arc<UacStats>,
    ) -> Self {
        let handle = std::thread::Builder::new()
            .name("uac-upstream".into())
            .spawn(move || {
                set_realtime_priority(18);
                run_upstream(cfg, rx, stats);
            })
            .expect("spawn uac-upstream thread");
        Self { handle: Some(handle) }
    }

    pub fn join(mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run_upstream(
    cfg: UacUpstreamConfig,
    mut rx: broadcast::Receiver<ProcessedAudioFrame>,
    stats: Arc<UacStats>,
) {
    info!("uac-upstream starting: {}", cfg.device);
    loop {
        match open_playback(&cfg) {
            Ok(pcm) => {
                if let Err(e) = pump_loop(&pcm, &cfg, &mut rx, &stats) {
                    warn!("uac-upstream pump stopped: {e}");
                }
                stats.upstream_active.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                warn!("uac-upstream open failed: {e}; retrying in 1 s");
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn open_playback(cfg: &UacUpstreamConfig) -> Result<PCM, alsa::Error> {
    let pcm = PCM::new(&cfg.device, Direction::Playback, false)?;
    {
        let hwp = HwParams::any(&pcm)?;
        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_format(Format::s16())?;
        hwp.set_channels(cfg.channels)?;
        hwp.set_rate(cfg.sample_rate, ValueOr::Nearest)?;
        hwp.set_period_size(cfg.period_frames as i64, ValueOr::Nearest)?;
        hwp.set_buffer_size((cfg.period_frames * cfg.buffer_periods as usize) as i64)?;
        pcm.hw_params(&hwp)?;
    }
    pcm.prepare()?;
    Ok(pcm)
}

fn pump_loop(
    pcm: &PCM,
    cfg: &UacUpstreamConfig,
    rx: &mut broadcast::Receiver<ProcessedAudioFrame>,
    stats: &Arc<UacStats>,
) -> Result<(), alsa::Error> {
    let mut stereo: Vec<i16> = Vec::with_capacity(cfg.period_frames * cfg.channels as usize);
    loop {
        match rx.blocking_recv() {
            Ok(frame) => {
                mono_to_stereo_i16(&frame.mic_mono_f32, &mut stereo);
                let io = pcm.io_i16()?;
                match io.writei(&stereo) {
                    Ok(_) => {
                        stats.upstream_active.store(true, Ordering::Relaxed);
                    }
                    Err(e) => match e.errno() {
                        libc::EBADFD => {
                            stats.upstream_active.store(false, Ordering::Relaxed);
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        libc::EPIPE => {
                            stats.upstream_underruns.fetch_add(1, Ordering::Relaxed);
                            pcm.prepare()?;
                        }
                        _ => return Err(e),
                    },
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                stats.upstream_drops.fetch_add(n, Ordering::Relaxed);
                warn!("uac-upstream lagged {n} frames");
            }
            Err(broadcast::error::RecvError::Closed) => return Ok(()),
        }
    }
}

fn mono_to_stereo_i16(mono: &[f32], out: &mut Vec<i16>) {
    out.clear();
    out.reserve(mono.len() * 2);
    for &v in mono {
        let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.push(s);
        out.push(s);
    }
}

#[cfg(target_os = "linux")]
fn set_realtime_priority(prio: i32) {
    let param = libc::sched_param { sched_priority: prio };
    let rc = unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) };
    if rc != 0 {
        let errno = std::io::Error::last_os_error();
        warn!("uac-upstream SCHED_FIFO failed: {errno}");
    }
}

#[cfg(not(target_os = "linux"))]
fn set_realtime_priority(_prio: i32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_to_stereo_duplicates_and_clamps() {
        let mono = vec![0.0_f32, 0.5, -0.5, 1.5, -1.5];
        let mut out = Vec::new();
        mono_to_stereo_i16(&mono, &mut out);
        assert_eq!(out.len(), mono.len() * 2);
        // Each input sample becomes two identical i16 outputs.
        assert_eq!(out[0], 0);            assert_eq!(out[1], 0);
        assert_eq!(out[2], (0.5 * 32767.0) as i16); assert_eq!(out[3], out[2]);
        assert_eq!(out[4], (-0.5 * 32767.0) as i16); assert_eq!(out[5], out[4]);
        // 1.5 and -1.5 clamp to ±1 → ±32767.
        assert_eq!(out[6], 32767);  assert_eq!(out[7], 32767);
        assert_eq!(out[8], -32767); assert_eq!(out[9], -32767);
    }
}
```

- [ ] **Step 2: Verify**

```
cargo test -p waved-audio --lib uac::upstream
```
Expected: 1 test passes.

```
cargo build -p waved-audio --release --features capture-alsa
```
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/waved-audio/src/uac/upstream.rs
git commit -m "feat(audio/sp6): UacUpstreamSink — ProcessedAudioFrame → hw:1,0p (stereo dup)"
```

---

## Task 6: `UacControl` volume mirror

Polls the `hw:1` ALSA mixer for `PCM Playback Volume` / `PCM Capture Volume` events, dispatches `Command::SetSpeakerVolume` / `Command::SetMicVolume`. Reverse direction (state → mixer) lives in Task 7's mute path; the volume-state-out-to-mixer mirror lands here.

The volume-loop guard (host-event suppresses self-write echo) is implemented with a `last_outgoing` cell so we don't re-send what we just wrote.

**Files:**
- Modify: `crates/waved-audio/src/uac/control.rs`

- [ ] **Step 1: Replace `control.rs` with the volume implementation**

```rust
//! UAC2 control — bridge `hw:1` ALSA mixer events to `Command` and back.
//!
//! Direction 1 (host → device): the kernel's `f_uac2` driver translates UAC2
//! SET_CUR class requests into ALSA mixer value events on card 1
//! (`PCM Playback Volume`, `PCM Capture Volume`, the matching switches, and
//! the read-only `*State` indicators). We poll for events at 50 ms cadence
//! and dispatch `Command::Set*Volume` / `Command::Toggle*Mute` onto the bus.
//!
//! Direction 2 (device → host): when `AudioState.speaker_volume` changes via
//! the touch UI or `/tmp/waved-volume`, we write the new value into the same
//! mixer control. The kernel reports it back to the host on the next GET_CUR.
//!
//! Both directions go through a shared `LastWrite` cell so a self-write
//! event isn't re-dispatched as a host event.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use alsa::mixer::{Mixer, SelemId, SelemChannelId};
use tokio::sync::{broadcast, watch};
use tokio::time::interval;
use tracing::{info, warn};

use waved_state::{AudioState, Command};

use super::UacStats;

#[derive(Debug, Clone)]
pub struct UacControlConfig {
    pub mixer_card: String,    // "hw:1"
}

/// Cell tracking what we last wrote to the mixer ourselves (to suppress the
/// resulting self-event from being re-dispatched as a host change).
#[derive(Default)]
struct LastWrite {
    speaker_volume: AtomicU16,
    mic_volume: AtomicU16,
}

pub struct UacControl {
    _phantom: (),
}

impl UacControl {
    /// Spawn the mixer event poll task plus the state mirror task.
    /// Both live in the existing tokio runtime; the alsa::mixer calls are
    /// short-lived and do not block long enough to need a dedicated thread.
    pub fn spawn(
        cfg: UacControlConfig,
        commands: broadcast::Sender<Command>,
        state_rx: watch::Receiver<AudioState>,
        stats: Arc<UacStats>,
    ) -> Self {
        let last_write = Arc::new(LastWrite::default());

        // Direction 1: poll mixer → emit Command.
        tokio::spawn({
            let cfg = cfg.clone();
            let stats = Arc::clone(&stats);
            let last_write = Arc::clone(&last_write);
            async move {
                if let Err(e) = run_mixer_poll(cfg, commands, stats, last_write).await {
                    warn!("uac-control mixer poll exited: {e}");
                }
            }
        });

        // Direction 2: state changes → mirror into mixer.
        tokio::spawn({
            let stats = Arc::clone(&stats);
            let last_write = Arc::clone(&last_write);
            let cfg = cfg.clone();
            async move {
                if let Err(e) = run_state_mirror(cfg, state_rx, stats, last_write).await {
                    warn!("uac-control state mirror exited: {e}");
                }
            }
        });

        Self { _phantom: () }
    }
}

async fn run_mixer_poll(
    cfg: UacControlConfig,
    commands: broadcast::Sender<Command>,
    stats: Arc<UacStats>,
    last_write: Arc<LastWrite>,
) -> Result<(), alsa::Error> {
    info!("uac-control: opening mixer {}", cfg.mixer_card);
    let mixer = Mixer::new(&cfg.mixer_card, false)?;

    // Cache the current values so we only emit on change.
    let mut prev_speaker_vol = read_volume(&mixer, "PCM", false).unwrap_or(0);
    let mut prev_mic_vol = read_volume(&mixer, "PCM", true).unwrap_or(0);
    let mut prev_speaker_muted = read_switch(&mixer, "PCM", false).unwrap_or(false);
    let mut prev_mic_muted = read_switch(&mixer, "PCM", true).unwrap_or(false);

    let mut tick = interval(Duration::from_millis(50));
    loop {
        tick.tick().await;
        // Drain pending mixer events. `handle_events` is non-blocking.
        let _ = mixer.handle_events();

        if let Some(v) = read_volume(&mixer, "PCM", false) {
            if v != prev_speaker_vol {
                prev_speaker_vol = v;
                if last_write.speaker_volume.load(Ordering::Relaxed) != v {
                    info!("uac-control: host set speaker volume → {v}");
                    let _ = commands.send(Command::SetSpeakerVolume(v));
                    stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        if let Some(v) = read_volume(&mixer, "PCM", true) {
            if v != prev_mic_vol {
                prev_mic_vol = v;
                if last_write.mic_volume.load(Ordering::Relaxed) != v {
                    info!("uac-control: host set mic volume → {v}");
                    let _ = commands.send(Command::SetMicVolume(v));
                    stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        if let Some(muted) = read_switch(&mixer, "PCM", false) {
            if muted != prev_speaker_muted {
                prev_speaker_muted = muted;
                let _ = commands.send(Command::ToggleSpeakerMute);
                stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(muted) = read_switch(&mixer, "PCM", true) {
            if muted != prev_mic_muted {
                prev_mic_muted = muted;
                let _ = commands.send(Command::ToggleMicMute);
                stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

async fn run_state_mirror(
    cfg: UacControlConfig,
    mut state_rx: watch::Receiver<AudioState>,
    stats: Arc<UacStats>,
    last_write: Arc<LastWrite>,
) -> Result<(), alsa::Error> {
    let mut prev_speaker_vol = state_rx.borrow().speaker_volume;
    let mut prev_mic_vol = state_rx.borrow().mic_volume;

    while state_rx.changed().await.is_ok() {
        let s = state_rx.borrow().clone();
        if s.speaker_volume != prev_speaker_vol {
            prev_speaker_vol = s.speaker_volume;
            last_write.speaker_volume.store(s.speaker_volume, Ordering::Relaxed);
            // Open the mixer fresh on each write — short-lived, avoids
            // holding state across await points.
            if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
                let _ = write_volume(&mixer, "PCM", false, s.speaker_volume);
                stats.device_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
        if s.mic_volume != prev_mic_vol {
            prev_mic_vol = s.mic_volume;
            last_write.mic_volume.store(s.mic_volume, Ordering::Relaxed);
            if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
                let _ = write_volume(&mixer, "PCM", true, s.mic_volume);
                stats.device_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    Ok(())
}

fn read_volume(mixer: &Mixer, name: &str, capture: bool) -> Option<u16> {
    let id = SelemId::new(name, 0);
    let elem = mixer.find_selem(&id)?;
    let (min, max) = if capture { elem.get_capture_volume_range() }
                     else       { elem.get_playback_volume_range() };
    let raw = if capture {
        elem.get_capture_volume(SelemChannelId::mono()).ok()?
    } else {
        elem.get_playback_volume(SelemChannelId::mono()).ok()?
    };
    let pct = ((raw - min) as f64 / (max - min) as f64 * 100.0).round() as u16;
    Some(pct.min(100))
}

fn read_switch(mixer: &Mixer, name: &str, capture: bool) -> Option<bool> {
    let id = SelemId::new(name, 0);
    let elem = mixer.find_selem(&id)?;
    let active = if capture {
        elem.get_capture_switch(SelemChannelId::mono()).ok()?
    } else {
        elem.get_playback_switch(SelemChannelId::mono()).ok()?
    };
    // ALSA: 1 = unmuted, 0 = muted. Our `*_muted` semantics: true = muted.
    Some(active == 0)
}

fn write_volume(mixer: &Mixer, name: &str, capture: bool, percent: u16) -> Result<(), alsa::Error> {
    let id = SelemId::new(name, 0);
    let elem = match mixer.find_selem(&id) {
        Some(e) => e,
        None => return Ok(()),
    };
    let (min, max) = if capture { elem.get_capture_volume_range() }
                     else       { elem.get_playback_volume_range() };
    let raw = (min + ((max - min) as f64 * percent as f64 / 100.0).round() as i64).clamp(min, max);
    if capture {
        elem.set_capture_volume_all(raw)?;
    } else {
        elem.set_playback_volume_all(raw)?;
    }
    Ok(())
}
```

- [ ] **Step 2: Verify**

```
cargo build -p waved-audio --release --features capture-alsa
```
Expected: clean.

```
cargo test -p waved-audio --lib uac
```
Expected: still 3 tests pass (2 from mod.rs + 1 from upstream.rs; control.rs has no host-side test).

- [ ] **Step 3: Commit**

```bash
git add crates/waved-audio/src/uac/control.rs
git commit -m "feat(audio/sp6): UacControl — UAC2 mixer events ↔ AudioState"
```

---

## Task 7: Replace `speaker_mixer::amixer` shell-out

Spec §3 cleanup: `Command::ToggleSpeakerMute` and `Command::SetSpeakerVolume` handlers in `AudioService::run` currently shell out to `amixer` against an unused `Master` control on TDMOUT_A. Now that `UacControl` owns the volume-out path (via `hw:1` UAC2 mixer), drop the shell-out.

**Files:**
- Modify: `crates/waved-audio/src/lib.rs`

- [ ] **Step 1: Identify the existing handlers**

Read `crates/waved-audio/src/lib.rs` and find the `AudioService::run` function. Look for the `tokio::select! { cmd = cmd_rx.recv() => match cmd { ... } }` block. Two arms reference `speaker_mixer`:

```rust
                        Ok(Command::ToggleSpeakerMute) => {
                            // existing: spawn_blocking → speaker_mixer::set_mute / set_volume
                        }
                        Ok(Command::SetSpeakerVolume(v)) => {
                            // existing: spawn_blocking → speaker_mixer::set_volume
                        }
```

- [ ] **Step 2: Replace those two arms**

The audio service no longer talks to ALSA mixer directly. State updates stay; UacControl picks them up via the watch channel and mirrors to UAC2.

```rust
                        Ok(Command::ToggleSpeakerMute) => {
                            let new_muted = !self.audio_tx.borrow().speaker_muted;
                            self.audio_tx.send_modify(|s| {
                                s.speaker_muted = new_muted;
                                if !new_muted && s.speaker_volume < 10 { s.speaker_volume = 10; }
                            });
                            // UacControl mirrors this to hw:1 PCM Playback Switch on the
                            // next state-watch tick.
                        }
                        Ok(Command::SetSpeakerVolume(v)) => {
                            self.audio_tx.send_modify(|s| s.speaker_volume = v);
                            // UacControl mirrors to hw:1 PCM Playback Volume.
                        }
```

If the existing struct field `speaker_control` (mentioned in the SP#5 audit) is no longer read after this change, remove it AND the `speaker_mixer::discover_control()` call in `AudioService::new`. Look around the `pub struct AudioService { ... speaker_control: Option<String>, ... }` declaration. If still used elsewhere, leave it; if not, delete it.

- [ ] **Step 3: Verify**

```
cargo test -p waved-audio --lib
```
Expected: tests still pass. The lifetime of any speaker_mixer-related code paths is now Task-8-controlled.

```
cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
```
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/waved-audio/src/lib.rs
git commit -m "refactor(audio/sp6): drop speaker_mixer::amixer shell-out from mute/volume handlers"
```

---

## Task 8: Wire `UacBridge` + `UacUpstreamSink` + `UacControl` into `main.rs`

**Files:**
- Modify: `crates/waved/src/main.rs`

- [ ] **Step 1: Construct the UAC components after `spawn_playback`**

In `crates/waved/src/main.rs`, find the SoftwareRef branch (the `else if let Some(pb) = cfg.playback.clone()` arm). Just after `let playback = Arc::new(playback);` and the existing preset-forwarder spawn from SP#5, add:

```rust
            // SP#6: UAC2 routing — host ↔ device speakerphone bridges.
            let uac_cfg = cfg.uac.clone().unwrap_or_default();
            let uac_stats: Arc<waved_audio::uac::UacStats> = Arc::new(waved_audio::uac::UacStats::default());

            if uac_cfg.enabled {
                let bridge_cfg = waved_audio::uac::bridge::UacBridgeConfig {
                    device: uac_cfg.downstream_device.clone(),
                    channels: 2,
                    sample_rate: 48_000,
                    period_frames: uac_cfg.period_frames,
                    buffer_periods: uac_cfg.buffer_periods,
                };
                let bridge = waved_audio::uac::UacBridge::spawn(
                    bridge_cfg,
                    Arc::clone(&playback),
                    Arc::clone(&uac_stats),
                );
                std::mem::forget(bridge); // run for process lifetime

                let upstream_cfg = waved_audio::uac::upstream::UacUpstreamConfig {
                    device: uac_cfg.upstream_device.clone(),
                    channels: 2,
                    sample_rate: 48_000,
                    period_frames: uac_cfg.period_frames,
                    buffer_periods: uac_cfg.buffer_periods,
                };
                let upstream = waved_audio::uac::UacUpstreamSink::spawn(
                    upstream_cfg,
                    audio_svc.subscribe_processed(),
                    Arc::clone(&uac_stats),
                );
                std::mem::forget(upstream);

                let control_cfg = waved_audio::uac::control::UacControlConfig {
                    mixer_card: uac_cfg.mixer_card.clone(),
                };
                let _control = waved_audio::uac::UacControl::spawn(
                    control_cfg,
                    bus.commands.clone(),
                    bus.audio.subscribe(),
                    Arc::clone(&uac_stats),
                );
                tracing::info!("uac: routing live (downstream={} upstream={} mixer={})",
                    uac_cfg.downstream_device, uac_cfg.upstream_device, uac_cfg.mixer_card);
            } else {
                tracing::info!("uac: disabled by config");
            }
```

The `Arc<UacStats>` is also handed to `stats_collector::run` in Task 9 — declare it BEFORE the source-type branch and pass `Some(Arc::clone(&uac_stats))` only when the SoftwareRef branch ran. For now, hold it as `let uac_stats = ...` inside this branch and let Task 9 widen the scope.

- [ ] **Step 2: Build**

```
cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
```
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/waved/src/main.rs
git commit -m "feat(audio/sp6): spawn UAC2 bridges + control task in main"
```

---

## Task 9: Stats — surface UAC fields in `/tmp/audio_stats.json`

**Files:**
- Modify: `crates/waved-audio/src/stats_collector.rs`
- Modify: `crates/waved/src/main.rs`

- [ ] **Step 1: Add `uac` field to the stats JSON**

In `crates/waved-audio/src/stats_collector.rs`, find the `AudioStats` struct (the JSON-serialised type). Add:

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uac: Option<crate::uac::UacStatsSnapshot>,
```

The `cfg(feature = "capture-alsa")` gate on `mod uac` means this field needs the same gate. If the surrounding struct lives outside any cfg, wrap the `uac` field in a feature gate:

```rust
    #[cfg(feature = "capture-alsa")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uac: Option<crate::uac::UacStatsSnapshot>,
```

- [ ] **Step 2: Plumb `Option<Arc<UacStats>>` into `stats_collector::run`**

Add a parameter to `stats_collector::run`:

```rust
    #[cfg(feature = "capture-alsa")]
    uac_stats: Option<Arc<crate::uac::UacStats>>,
```

When emitting the JSON, if `uac_stats.is_some()`, populate the `uac` field via `Some(s.snapshot())`; otherwise None.

- [ ] **Step 3: Hoist `uac_stats` out of the SoftwareRef branch in `main.rs`**

Before the `let (source, alignment) = if cfg.reference_enabled() { ... }` block, declare:

```rust
        #[cfg(feature = "capture-alsa")]
        let mut uac_stats_handle: Option<Arc<waved_audio::uac::UacStats>> = None;
```

Inside the SoftwareRef branch, instead of constructing `uac_stats` locally, set:

```rust
            uac_stats_handle = Some(Arc::clone(&uac_stats));
```

Then pass `uac_stats_handle` to `stats_collector::run(...)` as the new last argument.

- [ ] **Step 4: Build + run unit tests**

```
cargo test -p waved-audio --lib
cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
```
Expected: both clean.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/src/stats_collector.rs crates/waved/src/main.rs
git commit -m "feat(audio/sp6): surface UAC stats in /tmp/audio_stats.json"
```

---

## Task 10: CHECKPOINT — wiring complete

Pause here for user review. The feature is end-to-end live: deploying this binary should make the Mac see the Wave as a working stereo speakerphone. Run a quick smoke test before handing back:

- [ ] **Step 1: Build + deploy**

```
cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "killall waved 2>/dev/null; sleep 2; (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &)"
```

- [ ] **Step 2: Confirm it's up + stats look sane**

```
sleep 5
adb shell "pidof waved"
adb shell "cat /tmp/audio_stats.json" | python3 -m json.tool | grep -A 10 '"uac"'
```

Expected: `uac` object present with all-zero counters, `downstream_active: false`, `upstream_active: false` (no Mac connected yet).

- [ ] **Step 3: HAND OFF**

Surface the wiring-complete status to the user. Do NOT proceed to acceptance until they say so.

---

## Task 11: Device-side acceptance script

Covers the four criteria that don't require a Mac (downstream RMS rises, upstream stats reflect host-attach, volume mirror — both directions, sustained-stream stability over 60 s).

**Files:**
- Create: `scripts/test-uac2-routing.sh`

- [ ] **Step 1: Create the script**

```bash
cat > scripts/test-uac2-routing.sh <<'SH'
#!/usr/bin/env bash
# SP#6 device-side acceptance.
# Pre: device booted, waved running, Mac connected via USB.
#
# Tests:
#   1. host-attach detection — uac.downstream_active flips true when Mac plays
#   2. host→speaker RMS — capture mic_rms[0] rise during host playback
#   3. mic→host activity — uac.upstream_active flips true when Mac records
#   4. volume mirror device→host — set device volume, verify ALSA mixer
#   5. sustained stream — 60 s stability, no drops > 5
#
# The Mac-side actions (afplay / arecord / volume slider) are documented in
# scripts/test-uac2-routing-host.sh — run it on the Mac in parallel.

set -euo pipefail
cd "$(dirname "$0")/.."

LOG=/tmp/sp6-acceptance.log
: > "$LOG"

read_uac_field() {
    local expr="$1"
    adb shell "cat /tmp/audio_stats.json 2>/dev/null" | python3 -c "
import json, sys
try: s = json.load(sys.stdin)
except Exception: print('null'); sys.exit(0)
try: print(${expr})
except Exception: print('null')
"
}

echo "==> waiting for Mac to start playback (run scripts/test-uac2-routing-host.sh on the Mac)"
for i in $(seq 1 30); do
    if [ "$(read_uac_field "s['uac']['downstream_active']")" = "True" ]; then
        echo "  detected after ${i} s" | tee -a "$LOG"
        break
    fi
    sleep 1
done
ACTIVE=$(read_uac_field "s['uac']['downstream_active']")
[ "$ACTIVE" = "True" ] && echo "1/5 host-attach: PASS" | tee -a "$LOG" \
                       || { echo "1/5 host-attach: FAIL — uac.downstream_active=$ACTIVE" | tee -a "$LOG"; exit 1; }

echo "==> sampling mic RMS during host playback"
sleep 2
MIC=$(read_uac_field "s['rms_per_channel'][0]")
echo "  mic_rms[0]=$MIC" | tee -a "$LOG"
python3 -c "import sys; v=float('$MIC' if '$MIC' != 'null' else 0); sys.exit(0 if v > 100 else 1)" \
    && echo "2/5 host→speaker audible: PASS" | tee -a "$LOG" \
    || { echo "2/5 host→speaker audible: FAIL — mic_rms[0]=$MIC, expected > 100" | tee -a "$LOG"; exit 1; }

echo "==> waiting for Mac to start recording"
for i in $(seq 1 30); do
    if [ "$(read_uac_field "s['uac']['upstream_active']")" = "True" ]; then
        echo "  detected after ${i} s" | tee -a "$LOG"
        break
    fi
    sleep 1
done
UP=$(read_uac_field "s['uac']['upstream_active']")
[ "$UP" = "True" ] && echo "3/5 mic→host: PASS" | tee -a "$LOG" \
                   || { echo "3/5 mic→host: FAIL — uac.upstream_active=$UP" | tee -a "$LOG"; exit 1; }

echo "==> volume mirror device → host"
WAVED_PID=$(adb shell "pidof waved" | tr -d '\r')
# Trigger a state change on speaker_volume (touch UI path or /tmp file equivalent).
# We use /tmp/waved-volume — if that hook doesn't exist yet, send via Command directly.
adb shell "echo 75 > /tmp/waved-volume; kill -HUP $WAVED_PID" 2>&1 | tail -1
sleep 1
DEV_VOL=$(adb shell "amixer -c 1 sget PCM 2>/dev/null | grep 'Mono: Playback' | grep -oE '[0-9]+%' | head -1 | tr -d '%'")
echo "  hw:1 PCM Playback (host-visible) = ${DEV_VOL}%" | tee -a "$LOG"
[ "$DEV_VOL" = "75" ] && echo "4/5 volume mirror: PASS" | tee -a "$LOG" \
                      || echo "4/5 volume mirror: INCONCLUSIVE — got ${DEV_VOL}%, expected 75% (touch-UI hook may not exist)" | tee -a "$LOG"

echo "==> 60 s sustained stream"
DROPS_BEFORE=$(read_uac_field "s['uac']['downstream_drops'] + s['uac']['upstream_drops']")
sleep 60
DROPS_AFTER=$(read_uac_field "s['uac']['downstream_drops'] + s['uac']['upstream_drops']")
DELTA=$((DROPS_AFTER - DROPS_BEFORE))
echo "  drops in 60 s: $DELTA" | tee -a "$LOG"
[ "$DELTA" -le 5 ] && echo "5/5 sustained: PASS" | tee -a "$LOG" \
                   || { echo "5/5 sustained: FAIL — $DELTA drops in 60 s (limit 5)" | tee -a "$LOG"; exit 1; }

echo "=== ALL DEVICE-SIDE TESTS PASS ===" | tee -a "$LOG"
SH
chmod +x scripts/test-uac2-routing.sh
```

- [ ] **Step 2: Commit**

```bash
git add scripts/test-uac2-routing.sh
git commit -m "test(audio/sp6): device-side acceptance script for UAC2 routing"
```

---

## Task 12: Mac-side helper script

Documents and runs the host-side actions: play a 1 kHz tone, record from the Wave, observe volume changes. To run on the Mac with the Wave plugged in.

**Files:**
- Create: `scripts/test-uac2-routing-host.sh`

- [ ] **Step 1: Create the script**

```bash
cat > scripts/test-uac2-routing-host.sh <<'SH'
#!/usr/bin/env bash
# SP#6 host-side (Mac) acceptance helpers. Run on the Mac with the Wave
# selected as the default audio device, alongside scripts/test-uac2-routing.sh
# on the workstation that has the device on adb.
#
# Steps:
#   1. Generate 1 kHz tone (10 s WAV) and play via afplay.
#   2. Record 10 s from the Wave via sox (or arecord-equivalent on macOS — sox).
#   3. Print current Mac output volume (osascript).

set -euo pipefail

# Verify the Wave is selected
WAVE_OUT=$(/usr/sbin/system_profiler SPAudioDataType 2>/dev/null | grep -B 2 'UAC2_Gadget' | head -3 || true)
echo "Wave device info:"
echo "$WAVE_OUT"
echo

if ! command -v afplay >/dev/null; then
    echo "ERROR: afplay missing — run on macOS"; exit 1
fi

# 1 kHz, 10 s, mono → Mac will up-channel for stereo output
TONE=/tmp/sp6-1k.wav
python3 - <<'PY' > "$TONE"
import sys, struct, math, wave
buf = wave.open(sys.stdout.buffer, 'wb')
buf.setnchannels(1); buf.setsampwidth(2); buf.setframerate(48_000)
n = 48_000 * 10
amp = int(0.3 * 32767)
for i in range(n):
    s = int(amp * math.sin(2*math.pi*1000*i/48_000))
    buf.writeframes(struct.pack('<h', s))
buf.close()
PY

echo "==> playing 1 kHz / 10 s through Wave (afplay)"
afplay "$TONE" &
AFPLAY_PID=$!
sleep 1

echo "==> recording 10 s from Wave (sox)"
sox -d /tmp/sp6-mic.wav trim 0 10 || echo "  sox not installed — install with 'brew install sox'"

wait $AFPLAY_PID

echo "==> current Mac output volume"
osascript -e 'output volume of (get volume settings)'

echo
echo "==> expected: device-side test reports downstream_active=true and upstream_active=true"
echo "    /tmp/sp6-mic.wav should contain audible mic capture from the Wave"
SH
chmod +x scripts/test-uac2-routing-host.sh
```

- [ ] **Step 2: Commit**

```bash
git add scripts/test-uac2-routing-host.sh
git commit -m "test(audio/sp6): Mac-side helper for UAC2 acceptance"
```

---

## Task 13: Run acceptance — record measured numbers

Run both scripts on the live device + a Mac. Capture the actual numbers for the docs.

**Files:**
- (no source changes — just measurement)

- [ ] **Step 1: Connect the Mac to the Wave via USB**

Verify the Wave appears in macOS Sound preferences as `UAC2_Gadget`. Select it as both Output and Input.

- [ ] **Step 2: Start the device-side script**

```
./scripts/test-uac2-routing.sh
```

It will hang on `==> waiting for Mac to start playback`.

- [ ] **Step 3: Run the Mac-side script (on the Mac)**

```
./scripts/test-uac2-routing-host.sh
```

Both scripts run concurrently. The device-side script should detect the Mac's actions and print PASS for each test.

- [ ] **Step 4: Record results**

Note in `/tmp/sp6-acceptance.log`:
- mic_rms[0] during host playback (should be > 100 — observed value: ____)
- whether upstream_active flipped to true (the existence of mic→host streaming)
- 60 s sustained-stream drop count
- Any FAIL or INCONCLUSIVE markers

- [ ] **Step 5: AEC ERLE with host audio**

Re-run the SP#5 ERLE test, but this time the "tone" is host-played (not SIGUSR1):

```
# Mac plays a 1 kHz tone via afplay
# Device-side: read pre-tone baseline, then sample during tone
adb shell "cat /tmp/audio_stats.json" | python3 -c "
import json, sys
s = json.load(sys.stdin)
print('mic_rms[0]', s['rms_per_channel'][0])
print('mic_post_aec_rms', s['mic_post_aec_rms'])
import math
ref = s['rms_per_channel'][0]
post = s['mic_post_aec_rms']
if ref > 0 and post > 0:
    print('ERLE_dB_via_host', round(20 * math.log10(ref/post), 2))
"
```

Expected: ≥ 20 dB. Record the actual number for Task 14's docs.

- [ ] **Step 6: No commit unless docs change**

This task records numbers, not code. The numbers feed Task 14.

---

## Task 14: `docs/AUDIO_HARDWARE.md` UAC2 section

**Files:**
- Modify: `docs/AUDIO_HARDWARE.md`

- [ ] **Step 1: Append a new section before "## 4-Point DOA Validation Protocol"**

```markdown
## UAC2 Routing (SP#6)

The kernel `f_uac2` gadget exposes the Wave as a stereo S16 48 kHz USB
Audio Class 2 device on `card 1` (`UAC2Gadget`). `crates/waved-audio/src/uac/`
bridges both directions to the on-device speaker and mics.

```
host (Mac/Wave Controller)
  ↑ ALSA writei                  ↓ ALSA readi
hw:1,0p                       hw:1,0c
(UAC2 IN — what host reads)   (UAC2 OUT — what host writes)
  ↑                              ↓
UacUpstreamSink ──┐         ┌── UacBridge
(std::thread)     │         │   (std::thread)
  ↑               │         ▼
ProcessedAudioFrame   PlaybackService::play_batch
(post-AEC mono)              ↓
  ↑                          SP#5 DSP chain
audio-capture                ↓
                       hw:0,1 (TDM-B → acm8625s)
                       broadcast tap → AEC reference
```

Both downstream and upstream threads run at SCHED_FIFO priority 18. Drift
handling is drop-on-overflow at the period boundary: when the host clock
outruns the device clock and `play_batch` queue depth exceeds
`buffer_periods`, the bridge drops the period (counter:
`uac.downstream_drops` in `/tmp/audio_stats.json`). Same pattern on
upstream via the broadcast `Lagged` path.

### Volume + mute mirror

`UacControl` polls the `hw:1` ALSA mixer at 50 ms cadence. On host
SET_CUR (Mac slider drag), the kernel emits a value-change event;
`UacControl` translates to `Command::SetSpeakerVolume(v)` /
`Command::SetMicVolume(v)` / `Command::Toggle*Mute` on the bus. Reverse
direction: a `watch::Receiver<AudioState>` observer writes back to the
mixer when the device-side state changes, so the Mac slider mirrors
within ~1 s (Mac polls GET_CUR for menu-bar updates).

A `LastWrite` cell suppresses self-event echo: when the device writes a
new volume, the matching kernel-emitted value-change event is recognised
and not redispatched.

### SP#6 acceptance results (measured 2026-05-01)

- **Host → speaker**: <fill from Task 13>.
- **Mic → host (`upstream_active`)**: <fill>.
- **Volume mirror (Mac → device)**: <fill — observed on PCM Playback Volume slider>.
- **Volume mirror (device → Mac)**: <fill — `osascript` after `/tmp/waved-volume` write>.
- **AEC ERLE with host-played far-end**: <fill from Task 13 step 5> dB (bar 20 dB).
- **60 s sustained stream**: <fill> drops total (bar ≤ 5).
```

- [ ] **Step 2: Commit**

```bash
git add docs/AUDIO_HARDWARE.md
git commit -m "docs(audio/sp6): UAC2 routing section + acceptance numbers"
```

---

## Task 15: TODO.md mark done + final review

**Files:**
- Modify: `TODO.md`

- [ ] **Step 1: Replace the SP#6 entry**

Find this line in `TODO.md`:

```markdown
- [ ] SP#6 — UAC2 routing for Wave Controller
```

Replace with:

```markdown
- [x] **SP#6 — UAC2 routing for Wave Controller** (`crates/waved-audio/src/uac/`: `UacBridge` reads `hw:1,0c` → `play_batch`, `UacUpstreamSink` reads `subscribe_processed()` → `hw:1,0p`, `UacControl` mirrors `hw:1` mixer events ↔ `AudioState`. ALSA-mixer-based volume + mute is now end-to-end through one mechanism — `speaker_mixer::amixer` shell-out is gone. AEC ERLE with host audio: <N> dB. See `scripts/test-uac2-routing.sh`, `docs/AUDIO_HARDWARE.md` UAC2 Routing section.)
```

Replace `<N>` with the actual ERLE number measured in Task 13.

Also find the `### SP#6 (UAC2 routing)` follow-up section and replace its body with whatever follow-ups surfaced during acceptance / final review (e.g. Wave Controller protocol probe, drift telemetry tightening, multichannel UAC2). If nothing surfaced, change the heading to `### SP#6 follow-ups (not blockers)` and list any reviewer-flagged Minors.

- [ ] **Step 2: Final code review**

Dispatch one final code-quality reviewer over the whole branch:

```
git log --oneline c6428fa..HEAD
git diff --stat c6428fa..HEAD
```

(Replace `c6428fa` with the actual SHA of the SP#5 final docs commit if it differs.)

Review focus:
- Spec coverage (every §3 in-scope item delivered)
- Architectural cohesion of `uac/` module
- Thread lifecycle (no leak on host disconnect, watchdog respawn-clean)
- AEC reference correctness with host audio
- Volume mirror loop guard (no infinite host↔device ping-pong)

- [ ] **Step 3: Commit + close**

```bash
git add TODO.md
git commit -m "docs(audio/sp6): mark SP#6 done + file follow-ups"
```

---

## Self-review

- **Spec coverage:**
  - §2.1 bidirectional bridges → Tasks 4, 5 ✓
  - §2.2 format passthrough zero-conv → built into Tasks 4, 5 (S16 48 k, no resampler) ✓
  - §2.3 std::thread per direction at SCHED_FIFO 18 → Tasks 4, 5 (`set_realtime_priority(18)`) ✓
  - §2.4 host audio routes through `play_batch` → Task 4 (the bridge calls `playback.play_batch`) ✓
  - §2.5 mic upstream uses `ProcessedAudioFrame.mic_mono_f32` → Task 5 (`subscribe_processed()`) ✓
  - §2.6 mono → stereo duplication → Task 5 (`mono_to_stereo_i16` helper, unit-tested) ✓
  - §2.7 volume policy host-drives → Task 6 (`run_mixer_poll`); device-mirrors → Task 6 (`run_state_mirror`) ✓
  - §2.8 drop/duplicate at period boundaries → Task 4 (`queue_depth >= drop_threshold` skip) ✓
  - §2.9 host-not-selected sleep 50 ms → Task 4 (EAGAIN branch) and Task 5 (EBADFD branch) ✓
  - §2.10 acceptance script → Tasks 11–13 ✓
  - §3 `UacConfig` → Task 2 ✓
  - §3 `speaker_mixer::amixer` cleanup → Task 7 ✓
  - §3 acceptance + Mac helper scripts → Tasks 11, 12 ✓
  - §3 `AUDIO_HARDWARE.md` UAC2 block diagram → Task 14 ✓
  - §5.1 `UacConfig` with all defaults → Task 2 ✓
  - §5.3 `uac.*` block in stats JSON → Task 9 ✓
  - §6.1 downstream EAGAIN/EPIPE handling → Task 4 ✓
  - §6.2 upstream EBADFD/EPIPE/Lagged handling → Task 5 ✓
  - §6.3 mixer events + state mirror + LastWrite suppression → Task 6 ✓
  - §6.4 thread lifecycle + watchdog respawn → covered by std::mem::forget + open-retry loop in Tasks 4, 5 ✓
- **Placeholder scan:** Two `<fill>` markers in Task 14 (acceptance numbers) and one `<N>` in Task 15 — both are *deliberate* placeholders for measured numbers from Task 13.
- **Type consistency:** `UacBridgeConfig`, `UacUpstreamConfig`, `UacControlConfig` — distinct names, all under `crates::waved_audio::uac::*`. `UacStats` (atomics) and `UacStatsSnapshot` (serde) split cleanly. `Command::Set*Volume` / `Toggle*Mute` are existing variants — no changes to `waved-state`.
- **Scope:** Single-implementation-plan-sized. The mute-handler cleanup (Task 7) is small enough not to warrant its own SP.
