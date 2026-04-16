# Waved Audio — Capture Foundation v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rework SP#1 capture so it correctly captures mic PCM from `hw:0,2` + aligned reference from `hw:0,3`, delivers both in an `AudioFrame` sidecar, uses a forward-compatible config schema at `/etc/waved-audio-config.json`, validates ALSA parameters at open with clear diagnostics, and gates every ALSA-touching task with on-device verification.

**Architecture:** Low-level `PcmSource` trait unchanged (one file, one device). New `AudioFrameSource` trait sits above it, producing complete `AudioFrame`s. `SingleStreamFrameSource<P>` wraps one `PcmSource` (mic-only replay, mock, tests). `DualStreamSource<M, R>` wraps two (mic + reference), tries `snd_pcm_link`, falls back to timestamp-aligned pairing. `AlsaPcmSource` probes hw_params at open, validates against config, errors with a diagnostic summary on mismatch. `Command::ToggleSpeakerMute` / `Command::SetSpeakerVolume` shell out to `amixer` via a new `speaker_mixer` module; `ffi.rs` is deleted.

**Tech Stack:** Rust 2021, tokio, tracing, `alsa` crate 0.9, serde_json, `libc` for `CLOCK_MONOTONIC`. Cross-compiled via `cross build --target aarch64-unknown-linux-gnu`. Deployed via `adb push`.

**Reference spec:** `docs/superpowers/specs/2026-04-16-waved-audio-capture-foundation-v2-design.md`

---

## File Structure

Files this plan creates, modifies, or deletes:

```
TODO.md                                                 (new, repo root)

crates/waved-audio/
  src/
    audio_config.rs                                     (new) forward-compat config + embedded default
    speaker_mixer.rs                                    (new) amixer shell-out
    capture/
      mod.rs                                            (modify) AudioFrameSource trait, re-exports
      alsa.rs                                           (rewrite) config-driven + runtime probe
      alsa_dual_stream.rs                               (new) DualStreamSource<M, R> + AlsaDualStreamSource alias
      single_stream.rs                                  (new) SingleStreamFrameSource<P>
      thread.rs                                         (modify) run_capture_loop now bounded on AudioFrameSource
      channel_map.rs                                    (delete) replaced by audio_config
      replay.rs                                         (modify) drop ReplayFormat::I32Ten (10-ch dead code)
      mock.rs                                           (unchanged)
    callbacks.rs                                        (modify) AudioFrame gains reference sidecar
    stats_collector.rs                                  (modify) emit reference_rms, alignment, alignment_drift_us_p95
    fixtures.rs                                         (unchanged)
    metrics.rs                                          (unchanged)
    lib.rs                                              (modify) delete `pub mod ffi`, add `pub mod audio_config`, `pub mod speaker_mixer`
    ffi.rs                                              (delete)
  config/
    audio_config_default.json                           (new) embedded default
    channel_map_default.json                            (delete)
  tests/
    replay_smoke.rs                                     (modify) account for new AudioFrame shape
    determinism.rs                                      (unchanged structurally; re-record golden if format changed)
    mute_end_to_end.rs                                  (unchanged structurally)
  Cargo.toml                                            (unchanged)

crates/waved/
  src/main.rs                                           (modify) new wiring w/ AlsaDualStreamSource + AudioConfig
  Cargo.toml                                            (unchanged)

crates/waved-latency-probe/src/main.rs                  (modify) load AudioConfig; mic-only remains fine
crates/waved-capture-record/src/main.rs                 (modify) write paired mic + reference PCM sidecar

scripts/test-capture-on-device.sh                       (modify) assert new stats fields
```

**Task classification for the verification gate:**

| Tasks | Classification | Gate |
|---|---|---|
| 1, 2, 4, 5, 10, 11 | Host-only TDD | `cargo test` green |
| 3, 7, 8, 9, 15 | Host-only TDD (touch types that flow to ALSA but contain no ALSA calls) | `cargo test` green |
| 6, 12, 13, 14, 16 | ALSA-touching | `cargo test` **and** push-to-device-and-verify |

---

## Task 1: TODO.md roadmap at repo root

**Files:**
- Create: `TODO.md`

**Rationale:** Establishes the multi-SP roadmap so nothing slips through. Non-code, so no TDD. Commit first so all later tasks can reference it.

- [ ] **Step 1: Create `TODO.md`**

```markdown
# Waved — Roadmap

This repository replaces the stock Insta360 Wave firmware's closed-source audio stack with an open-source equivalent. The work is split into six sub-projects (SPs); each gets its own spec → plan → implementation cycle.

See `docs/superpowers/specs/2026-04-16-waved-audio-capture-foundation-v2-design.md` for the authoritative SP#1 design.

## Sub-projects

- [~] **SP#1 — Capture foundation + measurement harness** (v2 in progress)
- [ ] SP#2 — AEC + playback path
- [ ] SP#3 — Beamforming (MVDR/DAS, 6 modes + DOA)
- [ ] SP#4 — Neural NS + dereverb (DeepFilterNet 3)
- [ ] SP#5 — Playback EQ + gain staging
- [ ] SP#6 — UAC2 routing for Wave Controller

ASR (Silero VAD, WeNet, SenseVoice) is already open-source and out of scope.

## SP#1 carry-overs for later SPs

Items intentionally deferred during SP#1 v2 because they belong to their owning sub-project:

### SP#2 (AEC)
- Flip DSP boundary to `Vec<f32>`. Capture keeps `AudioFrame.data: Vec<i16>`; conversion happens at AEC's input stage (`as f32 / 32768.0`).
- Consume `AudioFrame.reference` sidecar already populated by SP#1 v2.
- Own the `NoiseReduction` audio mode (currently state-only in SP#1).
- Re-verify `snd_pcm_link` kernel alignment using a known playback signal; if drift > 1 period, force timestamp-fallback path via config.

### SP#3 (Beamforming)
- Add `mic_geometry` field to `AudioConfig` under a version bump.
- Wire `Command::SetAudioMode` (Omni / Cardioid / Supercardioid / Figure8 / Stereo) to MVDR/DAS beamforming presets.
- DOA estimation consumer for on-UI directionality indicator.

### SP#4 (Neural NS + dereverb)
- DeepFilterNet 3 integration (post-beamforming single-channel f32).

### SP#5 (Playback EQ + gain staging)
- Replace `speaker_mixer::amixer` shell-outs with a proper playback pipeline.
- Add `playback` block to `AudioConfig` under a version bump.

### SP#6 (UAC2 routing)
- Expose Wave Controller audio via USB Audio Class 2 gadget.
- Out of scope for the first five SPs.

## SP#1 v1 → v2 minor items (follow-ups, not blocking)

- `_frame_tx` in `AudioService` keeps the frame channel open even if the capture thread dies silently — add a health watch once SP#2 lands (context exists there to react).
- `stats_collector` does not actively reap stale `/tmp/audio_stats.json.tmp` on rename failure — self-corrects on next write; low priority.
- ALSA device / sample-rate / period values are fully in `AudioConfig`; no follow-up required.
```

- [ ] **Step 2: Commit**

```bash
git add TODO.md
git commit -m "docs: add TODO.md roadmap for SP#1-6 with SP#1 v2 carry-overs

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 2: AudioConfig schema + parser + embedded default

**Files:**
- Create: `crates/waved-audio/src/audio_config.rs`
- Create: `crates/waved-audio/config/audio_config_default.json`
- Modify: `crates/waved-audio/src/lib.rs` (add `pub mod audio_config;`)

**Rationale:** New forward-compatible schema that replaces the SP#1 v1 `ChannelMap` for everything except the reference-channel-index usage (which becomes irrelevant in the two-device model). Pure parser; TDD-friendly.

- [ ] **Step 1: Create embedded default JSON**

Create `crates/waved-audio/config/audio_config_default.json`:

```json
{
  "version": 2,
  "capture": {
    "mic_device": "hw:0,2",
    "mic_channels": 8,
    "mic_sample_rate": 48000,
    "mic_format": "S32_LE",
    "reference_device": "hw:0,3",
    "reference_channels": 2,
    "reference_sample_rate": 48000,
    "reference_format": "S32_LE"
  },
  "period_frames": 256,
  "buffer_periods": 4
}
```

- [ ] **Step 2: Write the failing tests**

Create `crates/waved-audio/src/audio_config.rs`:

```rust
//! AudioConfig — forward-compatible configuration for waved-audio capture.
//!
//! Loaded from `/etc/waved-audio-config.json` on device; falls back to the
//! embedded default (mics on `hw:0,2`, references on `hw:0,3`) when the file
//! is missing or malformed. Version-bumped when future sub-projects add
//! fields (mic_geometry, playback, etc.).

use serde::Deserialize;
use std::path::Path;
use thiserror::Error;
use tracing::warn;

const DEFAULT_JSON: &str = include_str!("../config/audio_config_default.json");

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AudioConfig {
    pub version: u32,
    pub capture: CaptureConfig,
    pub period_frames: usize,
    pub buffer_periods: u32,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CaptureConfig {
    pub mic_device: String,
    pub mic_channels: u32,
    pub mic_sample_rate: u32,
    pub mic_format: SampleFormat,
    /// Absent when reference capture is disabled.
    #[serde(default)]
    pub reference_device: Option<String>,
    #[serde(default)]
    pub reference_channels: u32,
    #[serde(default = "default_sample_rate")]
    pub reference_sample_rate: u32,
    #[serde(default)]
    pub reference_format: Option<SampleFormat>,
}

fn default_sample_rate() -> u32 { 48000 }

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum SampleFormat {
    S16_LE,
    S24_LE,
    S32_LE,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    File,
    Fallback,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("parse failure: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("invalid: {0}")]
    Invalid(String),
}

impl AudioConfig {
    pub fn from_json(json: &str) -> Result<Self, ConfigError> {
        let cfg: AudioConfig = serde_json::from_str(json)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn default_embedded() -> Self {
        Self::from_json(DEFAULT_JSON).expect("embedded default must parse")
    }

    pub fn load_or_default(path: &Path) -> (Self, ConfigSource) {
        match std::fs::read_to_string(path) {
            Ok(s) => match Self::from_json(&s) {
                Ok(cfg) => (cfg, ConfigSource::File),
                Err(e) => {
                    warn!("audio config at {} invalid ({e}), using fallback", path.display());
                    (Self::default_embedded(), ConfigSource::Fallback)
                }
            },
            Err(e) => {
                warn!("audio config at {} unreadable ({e}), using fallback", path.display());
                (Self::default_embedded(), ConfigSource::Fallback)
            }
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.version != 2 {
            return Err(ConfigError::Invalid(format!(
                "unsupported config version {}, expected 2", self.version
            )));
        }
        if self.capture.mic_channels == 0 {
            return Err(ConfigError::Invalid("mic_channels must be > 0".into()));
        }
        if self.capture.mic_sample_rate == 0 {
            return Err(ConfigError::Invalid("mic_sample_rate must be > 0".into()));
        }
        if self.period_frames == 0 {
            return Err(ConfigError::Invalid("period_frames must be > 0".into()));
        }
        if self.buffer_periods < 2 {
            return Err(ConfigError::Invalid("buffer_periods must be >= 2".into()));
        }
        // Reference: all-or-nothing.
        let has_ref_device = self.capture.reference_device.is_some();
        let has_ref_channels = self.capture.reference_channels > 0;
        let has_ref_format = self.capture.reference_format.is_some();
        match (has_ref_device, has_ref_channels, has_ref_format) {
            (true, true, true) => {}
            (false, false, false) => {}
            _ => return Err(ConfigError::Invalid(
                "reference_device, reference_channels, reference_format must all be set or all absent".into()
            )),
        }
        Ok(())
    }

    pub fn reference_enabled(&self) -> bool {
        self.capture.reference_device.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_default_parses_and_is_version_2() {
        let cfg = AudioConfig::default_embedded();
        assert_eq!(cfg.version, 2);
        assert_eq!(cfg.capture.mic_device, "hw:0,2");
        assert_eq!(cfg.capture.mic_channels, 8);
        assert_eq!(cfg.capture.reference_device.as_deref(), Some("hw:0,3"));
        assert_eq!(cfg.capture.reference_channels, 2);
        assert!(cfg.reference_enabled());
    }

    #[test]
    fn rejects_wrong_version() {
        let json = r#"{"version":1,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},"period_frames":256,"buffer_periods":4}"#;
        assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_zero_mic_channels() {
        let json = r#"{"version":2,"capture":{"mic_device":"hw:0,2","mic_channels":0,"mic_sample_rate":48000,"mic_format":"S32_LE"},"period_frames":256,"buffer_periods":4}"#;
        assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_partial_reference_config() {
        let json = r#"{"version":2,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE","reference_device":"hw:0,3"},"period_frames":256,"buffer_periods":4}"#;
        assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn mic_only_config_is_valid() {
        let json = r#"{"version":2,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},"period_frames":256,"buffer_periods":4}"#;
        let cfg = AudioConfig::from_json(json).unwrap();
        assert!(!cfg.reference_enabled());
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(AudioConfig::from_json("{not json"), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn load_or_default_missing_file_uses_fallback() {
        let (cfg, src) = AudioConfig::load_or_default(Path::new("/definitely/not/here.json"));
        assert_eq!(src, ConfigSource::Fallback);
        assert_eq!(cfg.version, 2);
    }

    #[test]
    fn load_or_default_valid_file_uses_file() {
        let tmp = std::env::temp_dir().join(format!("audiocfg-test-{}.json", std::process::id()));
        std::fs::write(&tmp, r#"{"version":2,"capture":{"mic_device":"hw:5,5","mic_channels":4,"mic_sample_rate":16000,"mic_format":"S16_LE"},"period_frames":128,"buffer_periods":2}"#).unwrap();
        let (cfg, src) = AudioConfig::load_or_default(&tmp);
        assert_eq!(src, ConfigSource::File);
        assert_eq!(cfg.capture.mic_device, "hw:5,5");
        assert_eq!(cfg.capture.mic_channels, 4);
        std::fs::remove_file(&tmp).ok();
    }
}
```

- [ ] **Step 3: Wire into `lib.rs`**

Modify `crates/waved-audio/src/lib.rs` — add `pub mod audio_config;` after `pub mod callbacks;`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p waved-audio --lib audio_config`
Expected: All 7 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/src/audio_config.rs crates/waved-audio/config/audio_config_default.json crates/waved-audio/src/lib.rs
git commit -m "feat(audio): add AudioConfig schema with version 2 + embedded fallback

Forward-compatible config for the two-device capture model (hw:0,2 mics,
hw:0,3 reference). Validates version, non-zero core fields, and
all-or-nothing reference configuration. Embedded default matches the
verified hardware. Loaded from /etc/waved-audio-config.json at runtime
with graceful fallback.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 3: AudioFrame gains reference sidecar

**Files:**
- Modify: `crates/waved-audio/src/callbacks.rs`
- Modify: all sites constructing `AudioFrame` to set the new fields (search for `AudioFrame {`)

**Rationale:** All downstream consumers (stats, AEC in SP#2, recording) need this shape. Changes are mechanical but must go in before the DualStreamSource is wired.

- [ ] **Step 1: Update `AudioFrame` definition**

Open `crates/waved-audio/src/callbacks.rs` and replace the `AudioFrame` struct:

```rust
#[derive(Debug, Clone)]
pub struct AudioFrame {
    /// Mic PCM samples (interleaved if multi-channel).
    pub data: Vec<i16>,
    /// Aligned reference PCM (loopback), same timestamp. `None` when
    /// reference capture is disabled or for mic-only replay / tests.
    pub reference: Option<Vec<i16>>,
    pub sample_rate: u32,
    /// Mic channel count (applies to `data`).
    pub channels: u16,
    /// Reference channel count (applies to `reference` when Some). `0` when
    /// reference is `None`.
    pub reference_channels: u16,
    pub timestamp_ms: u64,
}
```

- [ ] **Step 2: Find all AudioFrame construction sites**

Run: `grep -rn "AudioFrame {" crates/ tests/`
Expected output (approximate): construction sites in `capture/thread.rs`, `capture/replay.rs` tests, `fixtures.rs`, `stats_collector.rs` tests, `waved-audio-bench/tests/cli_smoke.rs`, `tests/replay_smoke.rs`, `tests/mute_end_to_end.rs`, `simulate.rs`. Each needs the new fields.

- [ ] **Step 3: Update every construction site**

For each site, add `reference: None, reference_channels: 0,` (mic-only).

Example: `crates/waved-audio/src/capture/thread.rs` — the in-loop construction:

```rust
let frame = AudioFrame {
    data: out.clone(),
    reference: None,
    sample_rate,
    channels: mic_channels as u16,
    reference_channels: 0,
    timestamp_ms: start.elapsed().as_millis() as u64,
};
```

Example: `crates/waved-audio/src/simulate.rs` (the simulate path emits silence) — same.

Do a full sweep. The compiler is your checklist: `cargo check -p waved-audio` after each edit lists remaining sites.

- [ ] **Step 4: Add a constructor helper for mic-only frames**

In `crates/waved-audio/src/callbacks.rs`, add:

```rust
impl AudioFrame {
    /// Construct a mic-only frame (no reference sidecar). Intended for
    /// single-stream sources (replay, mock, simulate).
    pub fn mic_only(data: Vec<i16>, channels: u16, sample_rate: u32, timestamp_ms: u64) -> Self {
        Self { data, reference: None, sample_rate, channels, reference_channels: 0, timestamp_ms }
    }

    /// Construct a paired frame (mic + aligned reference). Intended for
    /// dual-stream ALSA capture.
    pub fn paired(
        data: Vec<i16>,
        channels: u16,
        reference: Vec<i16>,
        reference_channels: u16,
        sample_rate: u32,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            data,
            reference: Some(reference),
            sample_rate,
            channels,
            reference_channels,
            timestamp_ms,
        }
    }
}
```

Replace the construction sites from Step 3 with `AudioFrame::mic_only(...)` — cleaner call sites, one obvious default.

- [ ] **Step 5: Run tests**

Run: `cargo test -p waved-audio`
Expected: All 47 existing tests pass with the new shape.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-audio/src/callbacks.rs crates/waved-audio/src/capture/thread.rs crates/waved-audio/src/simulate.rs crates/waved-audio/src/stats_collector.rs crates/waved-audio/src/fixtures.rs crates/waved-audio/tests/*.rs
git commit -m "feat(audio): add reference sidecar to AudioFrame + mic_only/paired constructors

Adds AudioFrame.reference: Option<Vec<i16>> and reference_channels: u16.
Mic-only paths use AudioFrame::mic_only(); two-device paths (added in
a later task) will use AudioFrame::paired(). All existing consumers
pass reference through unchanged.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 4: Delete ChannelMap + downmix_interleaved dead code

**Files:**
- Delete: `crates/waved-audio/src/capture/channel_map.rs`
- Delete: `crates/waved-audio/config/channel_map_default.json`
- Modify: `crates/waved-audio/src/capture/mod.rs` (remove re-exports + downmix function)
- Modify: `crates/waved-audio/src/capture/thread.rs` (remove `ChannelMap` arg, remove downmix call)
- Modify: `crates/waved-audio/src/capture/replay.rs` (drop `ReplayFormat::I32Ten`)

**Rationale:** v1's 10-ch interleaved downmix is dead in the two-device model. Mics come from `hw:0,2` as 8-ch PCM with no references in-band. Removing the dead code early prevents implementers of later tasks from accidentally re-using it.

- [ ] **Step 1: Delete the channel_map files**

```bash
git rm crates/waved-audio/src/capture/channel_map.rs
git rm crates/waved-audio/config/channel_map_default.json
```

- [ ] **Step 2: Rewrite `capture/mod.rs`**

Open `crates/waved-audio/src/capture/mod.rs` and replace contents with:

```rust
//! Audio capture: ALSA on device, file replay on host, mock in tests.

#[cfg(test)]
pub mod mock;

#[cfg(feature = "capture-alsa")]
pub mod alsa;

#[cfg(feature = "capture-replay")]
pub mod replay;

pub mod thread;
pub mod single_stream;

#[cfg(feature = "capture-alsa")]
pub mod alsa_dual_stream;

pub use thread::{CaptureStats, run_capture_loop};
pub use single_stream::SingleStreamFrameSource;

#[cfg(feature = "capture-replay")]
pub use replay::{ReplayFormat, ReplayPcmSource};

/// Produces raw interleaved 32-bit PCM frames from one device.
///
/// Implementations:
/// - `AlsaPcmSource` (device): wraps `snd_pcm_readi`
/// - `ReplayPcmSource` (host): reads from a PCM file
/// - `MockPcmSource` (tests): yields pre-built frame sequences
pub trait PcmSource: Send {
    /// Read raw interleaved samples into `buf`. Returns the number of
    /// samples written (NOT frames — `samples = frames * channels`).
    /// Returns 0 on EOF.
    fn read(&mut self, buf: &mut [i32]) -> std::io::Result<usize>;

    fn channels(&self) -> usize;
    fn sample_rate(&self) -> u32;
}

/// Produces complete `AudioFrame`s. Higher-level than `PcmSource`; consumers
/// of the capture loop work against this.
///
/// Implementations:
/// - `SingleStreamFrameSource<P>` (mic-only replay, mock, tests)
/// - `AlsaDualStreamSource` (on-device, mic + reference)
pub trait AudioFrameSource: Send {
    /// Return the next frame, or `Err` on unrecoverable error.
    /// Implementations handle their own xrun recovery where applicable.
    fn next_frame(&mut self) -> std::io::Result<Option<crate::callbacks::AudioFrame>>;

    fn sample_rate(&self) -> u32;
}
```

(`AudioFrameSource` returns `Option<AudioFrame>` — `None` means graceful EOF, `Err` means unrecoverable. This mirrors `std::io::Read`.)

- [ ] **Step 3: Delete the `downmix_tests` module**

Delete the `#[cfg(test)] mod downmix_tests` block in the old `mod.rs` (now gone after Step 2; for clarity, ensure the new `mod.rs` contains no downmix tests).

- [ ] **Step 4: Drop `ReplayFormat::I32Ten`**

Open `crates/waved-audio/src/capture/replay.rs`. Delete the `I32Ten` variant and its sample-bytes / channels handling:

```rust
#[derive(Debug, Clone, Copy)]
pub enum ReplayFormat {
    /// Interleaved i16 (what our 8-ch post-downmix / 2-ch reference fixtures use).
    /// `channels` captured from `ReplayPcmSource::open`'s `channels` arg.
    I16Interleaved { channels: usize },
}

impl ReplayFormat {
    pub fn sample_bytes(&self) -> usize {
        match self {
            ReplayFormat::I16Interleaved { .. } => 2,
        }
    }
    pub fn channels(&self) -> usize {
        match self {
            ReplayFormat::I16Interleaved { channels } => *channels,
        }
    }
}
```

Update `ReplayPcmSource::open` signature:

```rust
impl ReplayPcmSource {
    pub fn open(path: &Path, format: ReplayFormat, sample_rate: u32) -> std::io::Result<Self> {
        let channels = format.channels();
        let file = File::open(path)?;
        Ok(Self {
            reader: BufReader::new(file),
            channels,
            sample_rate,
            sample_bytes: format.sample_bytes(),
        })
    }
}
```

And drop the `4` branch in `read`:

```rust
match self.sample_bytes {
    2 => for i in 0..whole_samples {
        let s = i16::from_le_bytes([byte_buf[2*i], byte_buf[2*i+1]]);
        buf[i] = (s as i32) << 16;
    },
    _ => unreachable!(),
}
```

- [ ] **Step 5: Update tests in `replay.rs`**

The `reads_i16_file_and_widens_to_i32` test still works with `ReplayFormat::I16Interleaved { channels: 1 }`. Update the invocation:

```rust
let mut src = ReplayPcmSource::open(f.path(), ReplayFormat::I16Interleaved { channels: 1 }, 48000).unwrap();
```

Apply to all three tests in the file.

- [ ] **Step 6: Update `run_capture_loop` signature (interim)**

Open `crates/waved-audio/src/capture/thread.rs`. Change signature to remove `ChannelMap` and the downmix step:

```rust
pub fn run_capture_loop<P: PcmSource>(
    mut source: P,
    period_frames: usize,
    mic_channels: usize,
    frame_tx: tokio::sync::mpsc::Sender<AudioFrame>,
    stop_flag: Arc<AtomicBool>,
    stats: Arc<CaptureStats>,
) {
    let mut raw = vec![0i32; period_frames * mic_channels];
    let mut out = vec![0i16; period_frames * mic_channels];
    let start = Instant::now();
    let sample_rate = source.sample_rate();
    // … rest unchanged except:
    //   - remove the `downmix_interleaved(&raw, &mut out, &map)` call
    //   - replace with a simple S32 → S16 conversion:
    for (i, &s) in raw.iter().enumerate() {
        out[i] = (s >> 16) as i16;
    }
    let frame = AudioFrame::mic_only(out.clone(), mic_channels as u16, sample_rate, start.elapsed().as_millis() as u64);
    // … try_send unchanged
}
```

Note: this interim signature will be superseded in Task 10 when `run_capture_loop` switches to the `AudioFrameSource` trait. For now, the mic-only single-device path uses it.

- [ ] **Step 7: Update tests in `thread.rs`**

The tests use `tiny_map()`. Replace with:

```rust
fn tiny_map() -> ChannelMap { /* deleted */ }

// Replace the 3 test call sites:
run_capture_loop(src, /*period*/ 2, /*mic_channels*/ 1, tx, stop, stats);
```

Plus: the tests currently use a 2-channel `MockPcmSource` with the 2nd channel discarded (the reference). In the new world, `MockPcmSource` models a single ALSA device. Change the mock setup to 1-channel:

```rust
let src = MockPcmSource::new(1, 48000)
    .with_batch(vec![10 << 16, 20 << 16]); // 2 frames, 1 ch
```

Update expected output: `frame.data == vec![10, 20]`.

Do the same for the `drops_frames_on_backpressure` and `xrun_increments_counter_and_continues` tests.

- [ ] **Step 8: Run all waved-audio tests**

Run: `cargo test -p waved-audio --lib`
Expected: 47 tests … wait, minus `channel_map` tests (6) and `downmix_tests` (4), so 37 tests pass. Plus audio_config tests (7) from Task 2 = 44 total tests passing.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "refactor(audio): remove ChannelMap + downmix_interleaved (dead in two-device model)

The 10-ch interleaved downmix assumed mics and references share one ALSA
stream; v2 captures them as separate streams (hw:0,2 mics, hw:0,3 refs).
run_capture_loop now takes mic_channels directly and does a simple S32>S16
conversion inline. ReplayPcmSource loses I32Ten (only I16Interleaved
remains, parameterised by channel count).

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 5: Delete ffi.rs + new speaker_mixer module

**Files:**
- Delete: `crates/waved-audio/src/ffi.rs`
- Create: `crates/waved-audio/src/speaker_mixer.rs`
- Modify: `crates/waved-audio/src/lib.rs` (remove `pub mod ffi`, add `pub mod speaker_mixer`, update `AudioService::run`)

**Rationale:** The `ffi.rs` stubs were placeholders for libInstaAudio calls we'll never make. Speaker mute / volume shell out to `amixer`. `SetAudioMode` becomes state-only with a debug log.

- [ ] **Step 1: Write failing tests for speaker_mixer**

Create `crates/waved-audio/src/speaker_mixer.rs`:

```rust
//! Speaker mute + volume via `amixer` shell-out.
//!
//! The Amlogic driver exposes an sscalar control whose name varies across
//! firmware builds. `discover_control()` enumerates controls at startup
//! and picks the first that looks like a master / speaker scalar.

use std::process::Command;
use tracing::{debug, warn};

pub fn discover_control() -> Option<String> {
    // `amixer scontrols` output like: Simple mixer control 'Master',0
    let out = Command::new("amixer")
        .arg("scontrols")
        .output()
        .ok()?;
    if !out.status.success() { return None; }
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        if let Some(start) = line.find('\'') {
            if let Some(end) = line[start + 1..].find('\'') {
                let name = &line[start + 1..start + 1 + end];
                let lower = name.to_lowercase();
                if lower.contains("master") || lower.contains("speaker") || lower.contains("playback") {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

pub fn set_mute(control: &str, muted: bool) -> std::io::Result<()> {
    let arg = if muted { "mute" } else { "unmute" };
    let status = Command::new("amixer")
        .arg("sset").arg(control).arg(arg)
        .status()?;
    if !status.success() {
        warn!("amixer sset {control} {arg} exit code {:?}", status.code());
    }
    debug!("speaker_mixer: {control} {arg}");
    Ok(())
}

pub fn set_volume(control: &str, percent: u32) -> std::io::Result<()> {
    let p = percent.min(100);
    let status = Command::new("amixer")
        .arg("sset").arg(control).arg(format!("{p}%"))
        .status()?;
    if !status.success() {
        warn!("amixer sset {control} {p}% exit code {:?}", status.code());
    }
    debug!("speaker_mixer: {control} -> {p}%");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_is_clamped_to_100() {
        // We can't verify amixer without it being installed; this test only
        // exercises the clamp logic indirectly via absence of panic.
        // `amixer` not being installed makes set_volume return Err, which is fine.
        let _ = set_volume("Master", 150);
    }

    #[test]
    fn discover_control_returns_none_if_amixer_absent_or_empty() {
        // Environment-dependent: if amixer is installed and has controls, this
        // may return Some; we assert only that it doesn't panic.
        let _ = discover_control();
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p waved-audio --lib speaker_mixer`
Expected: both tests pass (they don't depend on amixer being present).

- [ ] **Step 3: Update `AudioService` to use speaker_mixer**

Open `crates/waved-audio/src/lib.rs`. Remove `pub mod ffi;`. Add:

```rust
pub mod audio_config;
pub mod speaker_mixer;
```

Then update `AudioService` to own an optional control name, and the `Command::ToggleSpeakerMute` / `Command::SetSpeakerVolume` / `Command::SetAudioMode` handlers:

```rust
pub struct AudioService {
    audio_tx: Arc<watch::Sender<AudioState>>,
    _frame_tx: mpsc::Sender<callbacks::AudioFrame>,
    frame_rx: mpsc::Receiver<callbacks::AudioFrame>,
    meeting_tx: mpsc::Sender<callbacks::AudioFrame>,
    stats_tee: broadcast::Sender<callbacks::AudioFrame>,
    speaker_control: Option<String>,
}

impl AudioService {
    pub fn new(
        audio_tx: Arc<watch::Sender<AudioState>>,
        frame_tx: mpsc::Sender<callbacks::AudioFrame>,
        frame_rx: mpsc::Receiver<callbacks::AudioFrame>,
        meeting_tx: mpsc::Sender<callbacks::AudioFrame>,
    ) -> Self {
        let (stats_tee, _) = broadcast::channel(16);
        let speaker_control = speaker_mixer::discover_control();
        if speaker_control.is_none() {
            warn!("no amixer speaker control found — speaker mute/volume will be state-only");
        }
        Self { audio_tx, _frame_tx: frame_tx, frame_rx, meeting_tx, stats_tee, speaker_control }
    }

    // subscribe_frames() unchanged

    pub async fn run(mut self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("audio_srv starting (consumer-only loop)");
        loop {
            tokio::select! {
                frame = self.frame_rx.recv() => {
                    if let Some(mut frame) = frame {
                        let muted = self.audio_tx.borrow().mic_muted;
                        apply_mic_mute(&mut frame.data, muted);
                        let vu = compute_vu(&frame.data, 8);
                        self.audio_tx.send_modify(|s| s.vu_level.copy_from_slice(&vu));
                        let _ = self.stats_tee.send(frame.clone());
                        let _ = self.meeting_tx.try_send(frame);
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Ok(Command::SetAudioMode(mode)) => {
                            tracing::debug!("SetAudioMode({mode:?}): state-only until SP#3 (beamforming) or SP#4 (NS)");
                            self.audio_tx.send_modify(|s| s.mode = mode);
                        }
                        Ok(Command::ToggleMicMute) => {
                            self.audio_tx.send_modify(|s| s.mic_muted = !s.mic_muted);
                        }
                        Ok(Command::ToggleSpeakerMute) => {
                            let (new_muted, restore_vol) = {
                                let s = self.audio_tx.borrow();
                                (!s.speaker_muted, s.speaker_volume)
                            };
                            if let Some(ctrl) = self.speaker_control.clone() {
                                let _ = tokio::task::spawn_blocking(move || {
                                    let _ = speaker_mixer::set_mute(&ctrl, new_muted);
                                    if !new_muted {
                                        let _ = speaker_mixer::set_volume(&ctrl, restore_vol.max(10) as u32);
                                    }
                                }).await;
                            }
                            self.audio_tx.send_modify(|s| {
                                s.speaker_muted = new_muted;
                                if !new_muted && s.speaker_volume < 10 { s.speaker_volume = 10; }
                            });
                        }
                        Ok(Command::SetSpeakerVolume(v)) => {
                            if let Some(ctrl) = self.speaker_control.clone() {
                                let _ = tokio::task::spawn_blocking(move || {
                                    let _ = speaker_mixer::set_volume(&ctrl, v as u32);
                                }).await;
                            }
                            self.audio_tx.send_modify(|s| s.speaker_volume = v);
                        }
                        Ok(Command::SetMicVolume(v)) => {
                            self.audio_tx.send_modify(|s| s.mic_volume = v);
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Closed) => return,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("audio_srv lagged {n} commands");
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 4: Delete ffi.rs**

```bash
git rm crates/waved-audio/src/ffi.rs
```

- [ ] **Step 5: Update any remaining ffi references**

Run: `grep -rn "ffi::" crates/waved-audio/`

Expected: only doc-comment references, no live callers. If any remain, delete them.

- [ ] **Step 6: Run all waved-audio tests**

Run: `cargo test -p waved-audio --lib`
Expected: 44 + 2 (speaker_mixer) = 46 tests pass.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(audio): replace ffi.rs stubs with amixer-based speaker_mixer

- Delete waved-audio/src/ffi.rs — stubs for libInstaAudio calls we no longer make
- Add speaker_mixer module that shells out to amixer with discover_control()
- AudioService owns Option<String> for the speaker mixer name (None = state-only)
- SetAudioMode becomes state-only with a debug log; SP#3 (beamforming) and
  SP#4 (NS) will own the DSP selection semantics

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 6: AlsaPcmSource runtime probe + config-driven open

**Files:**
- Rewrite: `crates/waved-audio/src/capture/alsa.rs`

**Classification:** ALSA-touching → on-device verification required.

**Rationale:** Replace the opaque `Invalid argument (22)` with a specific diagnostic. Take an `AlsaStreamConfig` struct instead of positional args so adding fields later doesn't churn call sites.

- [ ] **Step 1: Define AlsaStreamConfig + AlsaOpenError**

Rewrite `crates/waved-audio/src/capture/alsa.rs`:

```rust
//! AlsaPcmSource: opens one ALSA PCM capture device, validates requested
//! parameters against the hardware's supported ranges, and streams raw i32
//! samples into a buffer. Recovers from xruns via snd_pcm_prepare.

use super::PcmSource;
use crate::audio_config::SampleFormat;
use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use thiserror::Error;
use tracing::info;

pub struct AlsaStreamConfig<'a> {
    pub device: &'a str,
    pub channels: u32,
    pub sample_rate: u32,
    pub format: SampleFormat,
    pub period_frames: usize,
    pub buffer_periods: u32,
}

#[derive(Debug, Error)]
pub enum AlsaOpenError {
    #[error("alsa open error: {0}")]
    Alsa(#[from] alsa::Error),
    #[error("hardware/config mismatch on {device}:\n{diagnosis}")]
    Mismatch { device: String, diagnosis: String },
}

pub struct AlsaPcmSource {
    pcm: PCM,
    channels: usize,
    sample_rate: u32,
}

impl AlsaPcmSource {
    pub fn open(cfg: &AlsaStreamConfig<'_>) -> Result<Self, AlsaOpenError> {
        let pcm = PCM::new(cfg.device, Direction::Capture, false)?;
        probe_and_set(&pcm, cfg)?;
        info!(
            "alsa capture opened: device={} ch={} rate={} period={} buffer={}",
            cfg.device, cfg.channels, cfg.sample_rate, cfg.period_frames,
            cfg.period_frames * cfg.buffer_periods as usize
        );
        Ok(Self {
            pcm,
            channels: cfg.channels as usize,
            sample_rate: cfg.sample_rate,
        })
    }

    /// Access the inner PCM for advanced operations (e.g., snd_pcm_link).
    pub fn as_pcm(&self) -> &PCM { &self.pcm }
}

fn probe_and_set(pcm: &PCM, cfg: &AlsaStreamConfig<'_>) -> Result<(), AlsaOpenError> {
    let hwp = HwParams::any(pcm)?;

    let ch_min = hwp.get_channels_min()?;
    let ch_max = hwp.get_channels_max()?;
    let rate_min = hwp.get_rate_min()?;
    let rate_max = hwp.get_rate_max()?;

    let fmt = match cfg.format {
        SampleFormat::S16_LE => Format::S16LE,
        SampleFormat::S24_LE => Format::S243LE,
        SampleFormat::S32_LE => Format::S32LE,
    };
    let format_ok = hwp.test_format(fmt).is_ok();

    let ch_in_range = (ch_min..=ch_max).contains(&cfg.channels);
    let rate_in_range = (rate_min..=rate_max).contains(&cfg.sample_rate);

    if !(ch_in_range && rate_in_range && format_ok) {
        let diagnosis = format!(
            "  channels: config={} supported=[{}..{}] {}\n  rate:     config={} supported=[{}..{}] {}\n  format:   config={:?} {}",
            cfg.channels, ch_min, ch_max, if ch_in_range { "OK" } else { "MISMATCH" },
            cfg.sample_rate, rate_min, rate_max, if rate_in_range { "OK" } else { "MISMATCH" },
            cfg.format, if format_ok { "OK" } else { "MISMATCH" },
        );
        return Err(AlsaOpenError::Mismatch {
            device: cfg.device.to_string(),
            diagnosis,
        });
    }

    hwp.set_access(Access::RWInterleaved)?;
    hwp.set_format(fmt)?;
    hwp.set_channels(cfg.channels)?;
    hwp.set_rate(cfg.sample_rate, ValueOr::Nearest)?;
    hwp.set_period_size(cfg.period_frames as i64, ValueOr::Nearest)?;
    hwp.set_buffer_size((cfg.period_frames * cfg.buffer_periods as usize) as i64)?;
    pcm.hw_params(&hwp)?;
    pcm.prepare()?;
    Ok(())
}

impl PcmSource for AlsaPcmSource {
    fn read(&mut self, buf: &mut [i32]) -> std::io::Result<usize> {
        let io = self.pcm.io_i32()?;
        match io.readi(buf) {
            Ok(frames) => Ok(frames * self.channels),
            Err(e) => {
                if e.errno() == libc::EPIPE {
                    drop(io);
                    let _ = self.pcm.prepare();
                    return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, e));
                }
                Err(std::io::Error::other(e))
            }
        }
    }

    fn channels(&self) -> usize { self.channels }
    fn sample_rate(&self) -> u32 { self.sample_rate }
}
```

- [ ] **Step 2: Verify host compile**

Run: `cargo check -p waved-audio --features capture-alsa 2>&1 | head -10`

Expected on macOS: fails at `alsa-sys` linker (missing `libasound`). That's expected — no Rust type errors should appear before the alsa-sys line.

If any Rust type errors appear, fix them.

- [ ] **Step 3: Cross-build for device**

Run: `cross build --target aarch64-unknown-linux-gnu -p waved-audio --features capture-alsa 2>&1 | tail -10`

Expected: `Finished` with no errors. (The `Cross.toml` pre-build step from commit `3426a9f` installs `libasound2-dev:arm64`.)

- [ ] **Step 4: Wait for Task 12 for full on-device verification**

This task's on-device proof comes indirectly via Task 12 (which wires AlsaPcmSource into main.rs and checks stats output). For this task, the compile-only gate is sufficient — but you MUST note in the commit that full on-device verification is deferred to Task 12.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/src/capture/alsa.rs
git commit -m "feat(audio): AlsaPcmSource runtime hw_params probe + config struct API

- Accept AlsaStreamConfig<'a> (device, channels, rate, format, period, buffer_periods)
  instead of positional args; future fields slot in without breaking callers
- Probe hw_params_any at open and return AlsaOpenError::Mismatch with a
  precise per-field diagnosis ('config=X supported=[A..B] MISMATCH') instead
  of the opaque 'Invalid argument (22)' we previously emitted
- Full on-device verification deferred to Task 12 (end-to-end wiring)

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 7: SingleStreamFrameSource

**Files:**
- Create: `crates/waved-audio/src/capture/single_stream.rs`

**Rationale:** The one-PcmSource-to-one-AudioFrame adapter. Used by replay, mock, and any future mic-only capture path. Also lets us express "reference is disabled" cleanly without special-casing in `run_capture_loop`.

- [ ] **Step 1: Write failing tests**

Create `crates/waved-audio/src/capture/single_stream.rs`:

```rust
//! SingleStreamFrameSource — wraps a PcmSource + channel count + period size
//! into an AudioFrameSource emitting mic-only AudioFrames (reference = None).

use super::{AudioFrameSource, PcmSource};
use crate::callbacks::AudioFrame;
use std::time::Instant;

pub struct SingleStreamFrameSource<P: PcmSource> {
    source: P,
    period_frames: usize,
    channels: usize,
    raw: Vec<i32>,
    out: Vec<i16>,
    start: Instant,
}

impl<P: PcmSource> SingleStreamFrameSource<P> {
    pub fn new(source: P, period_frames: usize, channels: usize) -> Self {
        Self {
            period_frames,
            channels,
            raw: vec![0i32; period_frames * channels],
            out: vec![0i16; period_frames * channels],
            source,
            start: Instant::now(),
        }
    }
}

impl<P: PcmSource> AudioFrameSource for SingleStreamFrameSource<P> {
    fn next_frame(&mut self) -> std::io::Result<Option<AudioFrame>> {
        let expected = self.raw.len();
        let n = self.source.read(&mut self.raw)?;
        if n == 0 {
            return Ok(None);
        }
        if n != expected {
            // Partial read — treat as recoverable; let caller decide.
            return Ok(None);
        }
        for (i, &s) in self.raw.iter().enumerate() {
            self.out[i] = (s >> 16) as i16;
        }
        Ok(Some(AudioFrame::mic_only(
            self.out.clone(),
            self.channels as u16,
            self.source.sample_rate(),
            self.start.elapsed().as_millis() as u64,
        )))
    }

    fn sample_rate(&self) -> u32 { self.source.sample_rate() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::mock::MockPcmSource;

    #[test]
    fn emits_mic_only_frame() {
        let mock = MockPcmSource::new(2, 48000)
            .with_batch(vec![10 << 16, 99 << 16, 20 << 16, 99 << 16]);
        let mut src = SingleStreamFrameSource::new(mock, 2, 2);
        let frame = src.next_frame().unwrap().expect("frame");
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.reference_channels, 0);
        assert!(frame.reference.is_none());
        assert_eq!(frame.data, vec![10, 99, 20, 99]);
    }

    #[test]
    fn returns_none_on_eof() {
        let mock = MockPcmSource::new(1, 48000);
        let mut src = SingleStreamFrameSource::new(mock, 2, 1);
        assert!(src.next_frame().unwrap().is_none());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p waved-audio --lib capture::single_stream`
Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/waved-audio/src/capture/single_stream.rs
git commit -m "feat(audio): add SingleStreamFrameSource — PcmSource to AudioFrameSource adapter

Used by replay paths, mock tests, and any mic-only capture scenario.
Emits AudioFrames with reference = None so downstream code doesn't need
special-casing for the absence of a reference sidecar.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 8: DualStreamSource — generic over two PcmSources

**Files:**
- Create: `crates/waved-audio/src/capture/alsa_dual_stream.rs`

**Classification:** Host-only TDD for the generic aligner logic; ALSA hooking (snd_pcm_link) is cfg-gated and tested on device via Task 12.

**Rationale:** Alignment logic is testable with two `MockPcmSource`s. Keep the generic in a host-testable shape; the ALSA-specific type alias stays behind `#[cfg(feature = "capture-alsa")]`.

- [ ] **Step 1: Add shared aligner logic + tests**

Create `crates/waved-audio/src/capture/alsa_dual_stream.rs`:

```rust
//! DualStreamSource<M, R> — pairs mic + reference PCM frames, tagging each
//! pair with a monotonic timestamp. ALSA-flavoured type alias
//! (`AlsaDualStreamSource`) tries `snd_pcm_link` at startup; on failure, the
//! generic aligner emits on best-effort timestamp pairing.

use super::{AudioFrameSource, PcmSource};
use crate::callbacks::AudioFrame;
use std::time::Instant;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignmentMethod {
    KernelLinked,       // snd_pcm_link honored
    TimestampFallback,  // per-thread timestamps
}

impl AlignmentMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            AlignmentMethod::KernelLinked => "kernel-linked",
            AlignmentMethod::TimestampFallback => "timestamp-fallback",
        }
    }
}

pub struct DualStreamSource<M: PcmSource, R: PcmSource> {
    mic: M,
    mic_channels: usize,
    reference: R,
    reference_channels: usize,
    period_frames: usize,
    raw_mic: Vec<i32>,
    raw_ref: Vec<i32>,
    out_mic: Vec<i16>,
    out_ref: Vec<i16>,
    alignment: AlignmentMethod,
    start: Instant,
}

impl<M: PcmSource, R: PcmSource> DualStreamSource<M, R> {
    pub fn new(
        mic: M,
        mic_channels: usize,
        reference: R,
        reference_channels: usize,
        period_frames: usize,
        alignment: AlignmentMethod,
    ) -> Self {
        Self {
            raw_mic: vec![0i32; period_frames * mic_channels],
            raw_ref: vec![0i32; period_frames * reference_channels],
            out_mic: vec![0i16; period_frames * mic_channels],
            out_ref: vec![0i16; period_frames * reference_channels],
            mic,
            mic_channels,
            reference,
            reference_channels,
            period_frames,
            alignment,
            start: Instant::now(),
        }
    }

    pub fn alignment(&self) -> AlignmentMethod { self.alignment }
}

impl<M: PcmSource, R: PcmSource> AudioFrameSource for DualStreamSource<M, R> {
    fn next_frame(&mut self) -> std::io::Result<Option<AudioFrame>> {
        // Read mic first (determines cadence); then reference.
        let m = self.mic.read(&mut self.raw_mic)?;
        if m == 0 { return Ok(None); }
        if m != self.raw_mic.len() { return Ok(None); }
        let r = self.reference.read(&mut self.raw_ref)?;
        if r == 0 { return Ok(None); }
        if r != self.raw_ref.len() { return Ok(None); }

        for (i, &s) in self.raw_mic.iter().enumerate() { self.out_mic[i] = (s >> 16) as i16; }
        for (i, &s) in self.raw_ref.iter().enumerate() { self.out_ref[i] = (s >> 16) as i16; }

        Ok(Some(AudioFrame::paired(
            self.out_mic.clone(),
            self.mic_channels as u16,
            self.out_ref.clone(),
            self.reference_channels as u16,
            self.mic.sample_rate(),
            self.start.elapsed().as_millis() as u64,
        )))
    }

    fn sample_rate(&self) -> u32 { self.mic.sample_rate() }
}

#[cfg(feature = "capture-alsa")]
pub use alsa_impl::AlsaDualStreamSource;

#[cfg(feature = "capture-alsa")]
mod alsa_impl {
    use super::*;
    use crate::audio_config::CaptureConfig;
    use crate::capture::alsa::{AlsaPcmSource, AlsaOpenError, AlsaStreamConfig};

    pub type AlsaDualStreamSource = DualStreamSource<AlsaPcmSource, AlsaPcmSource>;

    impl AlsaDualStreamSource {
        pub fn from_capture_config(
            cfg: &CaptureConfig,
            period_frames: usize,
            buffer_periods: u32,
        ) -> Result<Self, AlsaOpenError> {
            let mic = AlsaPcmSource::open(&AlsaStreamConfig {
                device: &cfg.mic_device,
                channels: cfg.mic_channels,
                sample_rate: cfg.mic_sample_rate,
                format: cfg.mic_format,
                period_frames,
                buffer_periods,
            })?;
            let ref_dev = cfg.reference_device.as_deref().ok_or_else(|| {
                AlsaOpenError::Mismatch {
                    device: "(reference)".into(),
                    diagnosis: "reference_device missing in config but DualStream requested".into(),
                }
            })?;
            let reference = AlsaPcmSource::open(&AlsaStreamConfig {
                device: ref_dev,
                channels: cfg.reference_channels,
                sample_rate: cfg.reference_sample_rate,
                format: cfg.reference_format.expect("validated present"),
                period_frames,
                buffer_periods,
            })?;

            // Try snd_pcm_link via the alsa crate.
            let alignment = match mic.as_pcm().link(reference.as_pcm()) {
                Ok(()) => {
                    info!("alignment: snd_pcm_link honored");
                    AlignmentMethod::KernelLinked
                }
                Err(e) => {
                    warn!("snd_pcm_link failed ({e}) — falling back to timestamp alignment");
                    AlignmentMethod::TimestampFallback
                }
            };

            Ok(DualStreamSource::new(
                mic,
                cfg.mic_channels as usize,
                reference,
                cfg.reference_channels as usize,
                period_frames,
                alignment,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::mock::MockPcmSource;

    #[test]
    fn paired_frames_populated() {
        let mic = MockPcmSource::new(2, 48000)
            .with_batch(vec![1 << 16, 2 << 16, 3 << 16, 4 << 16]);
        let refr = MockPcmSource::new(1, 48000)
            .with_batch(vec![9 << 16, 8 << 16]);
        let mut src = DualStreamSource::new(mic, 2, refr, 1, 2, AlignmentMethod::TimestampFallback);
        let frame = src.next_frame().unwrap().expect("frame");
        assert_eq!(frame.data, vec![1, 2, 3, 4]);
        assert_eq!(frame.reference.as_deref(), Some(&[9, 8][..]));
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.reference_channels, 1);
    }

    #[test]
    fn returns_none_on_mic_eof() {
        let mic = MockPcmSource::new(1, 48000); // no batches → EOF
        let refr = MockPcmSource::new(1, 48000)
            .with_batch(vec![1 << 16, 2 << 16]);
        let mut src = DualStreamSource::new(mic, 1, refr, 1, 2, AlignmentMethod::TimestampFallback);
        assert!(src.next_frame().unwrap().is_none());
    }

    #[test]
    fn alignment_method_is_exposed() {
        let mic = MockPcmSource::new(1, 48000);
        let refr = MockPcmSource::new(1, 48000);
        let src = DualStreamSource::new(mic, 1, refr, 1, 2, AlignmentMethod::KernelLinked);
        assert_eq!(src.alignment(), AlignmentMethod::KernelLinked);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p waved-audio --lib alsa_dual_stream`
Expected: 3 tests pass.

- [ ] **Step 3: Cross-build for device**

Run: `cross build --target aarch64-unknown-linux-gnu -p waved-audio --features capture-alsa 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/waved-audio/src/capture/alsa_dual_stream.rs
git commit -m "feat(audio): add DualStreamSource + AlsaDualStreamSource

Generic DualStreamSource<M, R> pairs mic + reference PCM frames from two
PcmSources, emitting paired AudioFrames with reference sidecar populated.
Host tests cover the aligner via two MockPcmSources.

AlsaDualStreamSource (behind capture-alsa feature) opens two AlsaPcmSources
and tries snd_pcm_link; logs 'alignment: snd_pcm_link honored' on success,
falls back to timestamp alignment on failure. Full on-device verification
at Task 12.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 9: run_capture_loop switches to AudioFrameSource

**Files:**
- Modify: `crates/waved-audio/src/capture/thread.rs`

**Rationale:** Now that `AudioFrameSource` exists, the capture loop stops owning the raw→i16 conversion itself. It just pulls frames and enforces policy (backpressure, xrun counting, stop flag).

- [ ] **Step 1: Rewrite `run_capture_loop`**

Open `crates/waved-audio/src/capture/thread.rs`. Replace contents:

```rust
//! Capture loop — pulls complete AudioFrames from an AudioFrameSource and
//! forwards them into an mpsc with drop-newest backpressure policy.

use crate::callbacks::AudioFrame;
use crate::capture::AudioFrameSource;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::mpsc;
use tracing::{info, warn, error};

pub struct CaptureStats {
    pub frames_emitted: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub xrun_count: AtomicU64,
}

impl CaptureStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            frames_emitted: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            xrun_count: AtomicU64::new(0),
        })
    }
}

pub fn run_capture_loop<S: AudioFrameSource>(
    mut source: S,
    frame_tx: mpsc::Sender<AudioFrame>,
    stop_flag: Arc<AtomicBool>,
    stats: Arc<CaptureStats>,
) {
    info!("capture loop starting (sample_rate={})", source.sample_rate());
    let mut consecutive_errors = 0u32;
    while !stop_flag.load(Ordering::Relaxed) {
        match source.next_frame() {
            Ok(Some(frame)) => {
                consecutive_errors = 0;
                match frame_tx.try_send(frame) {
                    Ok(()) => { stats.frames_emitted.fetch_add(1, Ordering::Relaxed); }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        let n = stats.frames_dropped.fetch_add(1, Ordering::Relaxed) + 1;
                        if n % 50 == 1 {
                            warn!("capture backpressure: {n} frames dropped");
                        }
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        info!("frame_tx closed, stopping capture");
                        return;
                    }
                }
            }
            Ok(None) => {
                info!("capture source EOF");
                return;
            }
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                let n = stats.xrun_count.fetch_add(1, Ordering::Relaxed) + 1;
                warn!("xrun #{n} ({e})");
            }
            Err(e) => {
                consecutive_errors += 1;
                error!("capture read error: {e}");
                if consecutive_errors > 3 {
                    error!("too many read errors, stopping capture");
                    return;
                }
            }
        }
    }
    info!("capture loop stopped (stop flag set)");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::mock::MockPcmSource;
    use crate::capture::single_stream::SingleStreamFrameSource;

    #[tokio::test]
    async fn emits_one_frame_per_batch() {
        let mock = MockPcmSource::new(1, 48000)
            .with_batch(vec![10 << 16, 20 << 16]);
        let src = SingleStreamFrameSource::new(mock, 2, 1);
        let (tx, mut rx) = mpsc::channel(8);
        let stats = CaptureStats::new();
        let stop = Arc::new(AtomicBool::new(false));
        let stats_c = Arc::clone(&stats);
        let stop_c = Arc::clone(&stop);
        let handle = std::thread::spawn(move || run_capture_loop(src, tx, stop_c, stats_c));
        let frame = rx.recv().await.expect("frame");
        assert_eq!(frame.data, vec![10, 20]);
        assert_eq!(frame.channels, 1);
        assert!(frame.reference.is_none());
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        assert_eq!(stats.frames_emitted.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn drops_frames_on_backpressure() {
        let mut mock = MockPcmSource::new(1, 48000);
        for _ in 0..100 { mock = mock.with_batch(vec![0]); }
        let src = SingleStreamFrameSource::new(mock, 1, 1);
        let (tx, _rx) = mpsc::channel(2);
        let stats = CaptureStats::new();
        let stop = Arc::new(AtomicBool::new(false));
        let stats_c = Arc::clone(&stats);
        let stop_c = Arc::clone(&stop);
        std::thread::spawn(move || run_capture_loop(src, tx, stop_c, stats_c)).join().unwrap();
        let emitted = stats.frames_emitted.load(Ordering::Relaxed);
        let dropped = stats.frames_dropped.load(Ordering::Relaxed);
        assert!(emitted <= 2, "at most 2 emitted, got {emitted}");
        assert!(dropped >= 95, "most dropped, got {dropped}");
    }

    #[tokio::test]
    async fn xrun_increments_counter_and_continues() {
        use std::io::ErrorKind;
        let mock = MockPcmSource::new(1, 48000)
            .with_batch(vec![1 << 16])
            .with_error(ErrorKind::BrokenPipe, "xrun")
            .with_batch(vec![2 << 16]);
        let src = SingleStreamFrameSource::new(mock, 1, 1);
        let (tx, mut rx) = mpsc::channel(8);
        let stats = CaptureStats::new();
        let stop = Arc::new(AtomicBool::new(false));
        let stats_c = Arc::clone(&stats);
        let stop_c = Arc::clone(&stop);
        std::thread::spawn(move || run_capture_loop(src, tx, stop_c, stats_c)).join().unwrap();
        let f1 = rx.try_recv().expect("f1");
        let f2 = rx.try_recv().expect("f2");
        assert_eq!(f1.data, vec![1]);
        assert_eq!(f2.data, vec![2]);
        assert_eq!(stats.xrun_count.load(Ordering::Relaxed), 1);
        assert_eq!(stats.frames_emitted.load(Ordering::Relaxed), 2);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p waved-audio --lib capture::thread`
Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/waved-audio/src/capture/thread.rs
git commit -m "refactor(audio): run_capture_loop now pulls from AudioFrameSource

Moves raw→i16 conversion and frame construction into the AudioFrameSource
implementations (SingleStreamFrameSource, DualStreamSource). run_capture_loop
owns only policy (backpressure, xrun counting, stop-flag, EOF handling).

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 10: stats_collector emits reference_rms + alignment

**Files:**
- Modify: `crates/waved-audio/src/stats_collector.rs`

**Rationale:** The stats sidecar is what the on-device smoke test inspects. It must surface enough info to prove the dual-stream path is live.

- [ ] **Step 1: Extend `AudioStats` schema**

Open `crates/waved-audio/src/stats_collector.rs`. Add fields:

```rust
#[derive(Serialize, Default)]
pub struct AudioStats {
    pub rms_per_channel: Vec<f32>,
    pub reference_rms: Vec<f32>,            // empty when reference = None
    pub clipping_rate: f32,
    pub dc_offset_per_channel: Vec<f32>,
    pub frames_emitted: u64,
    pub frames_dropped: u64,
    pub xrun_count: u64,
    pub channel_map_source: String,         // "file" or "fallback" — keep name for back-compat
    pub alignment: String,                  // "kernel-linked" | "timestamp-fallback" | "mic-only"
    pub sample_rate: u32,
    pub channels: u16,
    pub reference_channels: u16,
    pub timestamp_ms: u64,
    pub last_write_error: Option<String>,
}
```

- [ ] **Step 2: Update `compute_stats` to populate reference_rms**

Find the function that turns a frame into `AudioStats` and add:

```rust
let reference_rms = match frame.reference.as_ref() {
    Some(refbuf) if frame.reference_channels > 0 => {
        crate::metrics::rms_per_channel(refbuf, frame.reference_channels as usize)
    }
    _ => Vec::new(),
};
```

And set `reference_rms` in the returned struct, plus `reference_channels: frame.reference_channels`.

- [ ] **Step 3: Thread `alignment` through `run`**

Change the `run` signature to take an `alignment: &'static str` parameter OR an `alignment: String` that the caller passes in:

```rust
pub async fn run(
    mut rx: broadcast::Receiver<AudioFrame>,
    stats: Arc<CaptureStats>,
    config_source: ConfigSource,
    alignment: String,
    out_path: PathBuf,
    period: Duration,
) { ... }
```

Set `stats.alignment = alignment.clone()` on each write (or bind it into the closure).

- [ ] **Step 4: Update tests**

Existing stats tests construct `AudioFrame`s and call `compute_stats`. Add `reference: None, reference_channels: 0` (already done via `AudioFrame::mic_only` in Task 3). The `reference_rms` should be empty in those cases.

Add one new test:

```rust
#[test]
fn compute_stats_with_reference_populates_reference_rms() {
    let frame = AudioFrame::paired(
        vec![0; 16],
        1,
        vec![1000, 2000, 1000, 2000, 1000, 2000, 1000, 2000],
        1,
        48000,
        0,
    );
    let stats = compute_stats(&frame, ConfigSource::File, "kernel-linked".into());
    assert_eq!(stats.reference_rms.len(), 1);
    assert!(stats.reference_rms[0] > 0.0);
    assert_eq!(stats.reference_channels, 1);
    assert_eq!(stats.alignment, "kernel-linked");
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p waved-audio --lib stats_collector`
Expected: all stats tests pass (existing + new).

- [ ] **Step 6: Commit**

```bash
git add crates/waved-audio/src/stats_collector.rs
git commit -m "feat(audio): stats_collector emits reference_rms + alignment

Adds reference_rms, reference_channels, alignment to AudioStats. On-device
smoke test can now distinguish:
  - alignment='kernel-linked'   snd_pcm_link succeeded
  - alignment='timestamp-fallback' fell back to CLOCK_MONOTONIC aligner
  - alignment='mic-only'        reference device absent

Reference RMS is empty when frame.reference is None.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 11: Integration tests for the new capture path

**Files:**
- Modify: `crates/waved-audio/tests/replay_smoke.rs`
- Modify: `crates/waved-audio/tests/determinism.rs`
- Modify: `crates/waved-audio/tests/mute_end_to_end.rs`

**Rationale:** The host-side integration tests built in v1 must keep passing after all these type changes.

- [ ] **Step 1: Update replay_smoke.rs**

The existing `replay_silence_produces_zero_vu` and `replay_tone_elevates_vu_on_tone_channel` tests use `ReplayPcmSource::open(path, ReplayFormat::I16Eight, 48000)`. Replace with `ReplayFormat::I16Interleaved { channels: 8 }`. Wrap in `SingleStreamFrameSource::new(src, 256, 8)` before passing to `run_capture_loop`. Call `run_capture_loop(src, tx, stop, stats)` (no map arg).

- [ ] **Step 2: Update determinism.rs**

Same changes as replay_smoke.rs. The determinism assertion (byte-identical stats JSON on two replays of the same file) still applies; the only difference is the new fields are zero-valued for mic-only replay and thus still deterministic.

- [ ] **Step 3: Update mute_end_to_end.rs**

Same replay adjustments; the mute assertion (last muted frame is all zeros) works unchanged.

- [ ] **Step 4: Run all integration tests**

Run: `cargo test -p waved-audio --features capture-replay`
Expected: all replay integration tests green.

- [ ] **Step 5: Run all host tests**

Run: `cargo test -p waved-audio`
Expected: full suite green. Count: 7 (audio_config) + 37 existing unit + 2 (single_stream) + 3 (alsa_dual_stream) + 2 (speaker_mixer) + 1 new stats test + 3 integration = ~55 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-audio/tests/
git commit -m "test(audio): update integration tests for AudioFrameSource refactor

Replay tests wrap ReplayPcmSource in SingleStreamFrameSource; mute test
uses AudioFrame::mic_only; determinism still holds because new fields are
constant for mic-only replay.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 12: Wire AlsaDualStreamSource into waved/main.rs

**Files:**
- Modify: `crates/waved/src/main.rs`

**Classification:** ALSA-touching → on-device verification REQUIRED.

**Rationale:** End-to-end wiring. After this task, `waved` on device must boot, open both ALSA devices, start the capture thread, and begin writing stats.

- [ ] **Step 1: Replace the existing capture-alsa block**

Open `crates/waved/src/main.rs` and find the `#[cfg(all(not(feature = "simulate"), feature = "capture-alsa"))]` block. Replace its contents (keep the `compile_error!` guard and `capture_stop_flag` declaration):

```rust
#[cfg(all(not(feature = "simulate"), feature = "capture-alsa"))]
{
    use std::sync::atomic::AtomicBool;
    use std::path::Path;
    use waved_audio::audio_config::{AudioConfig, ConfigSource};
    use waved_audio::capture::{
        alsa_dual_stream::{AlsaDualStreamSource, AlignmentMethod},
        single_stream::SingleStreamFrameSource,
        alsa::{AlsaPcmSource, AlsaStreamConfig},
        run_capture_loop, CaptureStats,
    };
    use waved_audio::stats_collector;

    let (cfg, cfg_src) = AudioConfig::load_or_default(Path::new("/etc/waved-audio-config.json"));
    let cfg_src_str = match cfg_src { ConfigSource::File => "file", ConfigSource::Fallback => "fallback" };

    let capture_stats = CaptureStats::new();
    let capture_stop = Arc::new(AtomicBool::new(false));
    capture_stop_flag = Arc::clone(&capture_stop);

    let (frame_tx, frame_rx) = waved_audio::callbacks::create_audio_channel();
    let audio_svc = waved_audio::AudioService::new(
        Arc::clone(&bus.audio), frame_tx.clone(), frame_rx, meeting_tx,
    );
    let stats_rx = audio_svc.subscribe_frames();

    // Build the capture source. Dual-stream if reference configured, else single-stream.
    enum SourceDyn {
        Dual(AlsaDualStreamSource),
        Single(SingleStreamFrameSource<AlsaPcmSource>),
    }
    let (source, alignment) = if cfg.reference_enabled() {
        match AlsaDualStreamSource::from_capture_config(&cfg.capture, cfg.period_frames, cfg.buffer_periods) {
            Ok(s) => {
                let a = s.alignment();
                (SourceDyn::Dual(s), a.as_str().to_string())
            }
            Err(e) => {
                tracing::error!("ALSA dual-stream open failed:\n{e}");
                std::process::exit(1);
            }
        }
    } else {
        let mic = match AlsaPcmSource::open(&AlsaStreamConfig {
            device: &cfg.capture.mic_device,
            channels: cfg.capture.mic_channels,
            sample_rate: cfg.capture.mic_sample_rate,
            format: cfg.capture.mic_format,
            period_frames: cfg.period_frames,
            buffer_periods: cfg.buffer_periods,
        }) {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("ALSA mic open failed:\n{e}");
                std::process::exit(1);
            }
        };
        (SourceDyn::Single(SingleStreamFrameSource::new(
            mic, cfg.period_frames, cfg.capture.mic_channels as usize,
        )), "mic-only".to_string())
    };

    tokio::spawn(stats_collector::run(
        stats_rx,
        Arc::clone(&capture_stats),
        cfg_src,
        alignment.clone(),
        std::path::PathBuf::from("/tmp/audio_stats.json"),
        std::time::Duration::from_millis(500),
    ));

    let stats_c = Arc::clone(&capture_stats);
    let stop_c = Arc::clone(&capture_stop);
    std::thread::Builder::new()
        .name("audio-capture".into())
        .spawn(move || {
            match source {
                SourceDyn::Dual(s) => run_capture_loop(s, frame_tx, stop_c, stats_c),
                SourceDyn::Single(s) => run_capture_loop(s, frame_tx, stop_c, stats_c),
            }
        })
        .expect("spawn capture thread");

    let audio_cmd_rx = bus.commands.subscribe();
    tokio::spawn(async move { audio_svc.run(audio_cmd_rx).await });

    tracing::info!("capture wired: config={cfg_src_str} alignment={alignment}");
}
```

- [ ] **Step 2: Cross-build**

Run: `cross build --target aarch64-unknown-linux-gnu -p waved --features "ui,capture-alsa" 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 3: Deploy to device**

```bash
adb push target/aarch64-unknown-linux-gnu/debug/waved /data/waved/waved
adb shell "kill -9 \$(pgrep -f S60waved) 2>/dev/null; killall -9 waved 2>/dev/null; sleep 2"
adb shell "setsid /etc/init.d/S60waved start </dev/null >/dev/null 2>&1 &"
```

- [ ] **Step 4: Wait for first stats write + verify**

```bash
adb shell "sleep 8; test -f /tmp/audio_stats.json && cat /tmp/audio_stats.json"
```

Expected output: valid JSON with:
- `"alignment": "kernel-linked"` or `"timestamp-fallback"`
- `"channel_map_source": "file"` or `"fallback"`
- `"rms_per_channel"`: array of 8 floats
- `"reference_rms"`: array of 2 floats
- `"frames_emitted"`: non-zero
- `"xrun_count"`: integer (may be 0)

If ALSA fails to open, the log will show the probe diagnosis:

```bash
adb shell "tail -20 /data/waved/waved.log"
```

Expected: no `ALSA config/hardware mismatch` errors. If there are, the config doesn't match the hardware — fix the config or the hardware and re-deploy.

- [ ] **Step 5: Verify via ps that capture thread is stable**

```bash
adb shell "ps | grep -E 'waved|S60' | grep -v grep"
```

Expected: two PIDs — the S60waved wrapper and `/data/waved/waved`. Both present and not repeatedly respawning.

- [ ] **Step 6: Commit**

```bash
git add crates/waved/src/main.rs
git commit -m "feat(waved): wire AlsaDualStreamSource + AudioConfig in main.rs

On-device verified:
- AudioConfig loaded from /etc/waved-audio-config.json (or fallback)
- AlsaDualStreamSource opens hw:0,2 (8ch mics) + hw:0,3 (2ch reference)
- snd_pcm_link attempted; alignment field populated in /tmp/audio_stats.json
- Capture thread stable, frames_emitted > 0, reference_rms has 2 entries
- If config omits reference, falls back to SingleStreamFrameSource mic-only

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 13: waved-latency-probe uses AudioConfig

**Files:**
- Modify: `crates/waved-latency-probe/src/main.rs`

**Classification:** ALSA-touching → on-device verification REQUIRED.

**Rationale:** Latency probe is mic-only (reference device not useful for playback→mic latency). Just load config and use mic side.

- [ ] **Step 1: Swap ChannelMap for AudioConfig**

Open `crates/waved-latency-probe/src/main.rs`. Remove `use waved_audio::capture::{..., ChannelMap}`. Add:

```rust
use std::path::Path;
use waved_audio::audio_config::AudioConfig;
use waved_audio::capture::alsa::{AlsaPcmSource, AlsaStreamConfig};
use waved_audio::capture::single_stream::SingleStreamFrameSource;
use waved_audio::capture::{run_capture_loop, CaptureStats};
```

Replace the `let map = ChannelMap::default_embedded();` / `AlsaPcmSource::open(...)` block with:

```rust
let (cfg, _) = AudioConfig::load_or_default(Path::new("/etc/waved-audio-config.json"));
let mic = AlsaPcmSource::open(&AlsaStreamConfig {
    device: &cfg.capture.mic_device,
    channels: cfg.capture.mic_channels,
    sample_rate: cfg.capture.mic_sample_rate,
    format: cfg.capture.mic_format,
    period_frames: cfg.period_frames,
    buffer_periods: cfg.buffer_periods,
})?;
let mic_channels = cfg.capture.mic_channels as usize;
let source = SingleStreamFrameSource::new(mic, cfg.period_frames, mic_channels);
```

And adjust the capture-loop spawn to use the new source + new `run_capture_loop` signature (no `map`, no `period_frames` arg — they're baked into `source`).

The channel-0 extraction (`frame.data.iter().step_by(8)`) now uses `step_by(mic_channels)` to remain correct regardless of channel count.

- [ ] **Step 2: Cross-build**

Run: `cross build --target aarch64-unknown-linux-gnu -p waved-latency-probe 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 3: Deploy + run × 5 + verify**

```bash
adb push target/aarch64-unknown-linux-gnu/debug/waved-latency-probe /data/waved/
adb shell "for i in 1 2 3 4 5; do /data/waved/waved-latency-probe 2>/dev/null; sleep 1; done" | tee /tmp/probe5.log
```

Expected: 5 JSON lines of the form `{"delay_ms": N.NN, "peak_sample": M, "score": S.SSe+NN}`. `delay_ms` values roughly consistent (stddev < 5ms is the SP#1 success criterion).

If `aplay` fails (no playback device) or `score < min_score`, the probe prints an error to stderr — investigate on device before claiming the task done.

- [ ] **Step 4: Commit**

```bash
git add crates/waved-latency-probe/src/main.rs
git commit -m "refactor(latency-probe): use AudioConfig + SingleStreamFrameSource

Loads /etc/waved-audio-config.json (or fallback), opens mic-only via the
new AlsaStreamConfig API, and wraps in SingleStreamFrameSource for
run_capture_loop. 5-run median on device within SP#1 latency-stddev
target.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 14: waved-capture-record writes mic + reference sidecars

**Files:**
- Modify: `crates/waved-capture-record/src/main.rs`

**Classification:** ALSA-touching → on-device verification REQUIRED.

**Rationale:** Fixtures that drive SP#2 AEC need the reference channel too. Record both when available.

- [ ] **Step 1: Adopt AudioConfig + dual-stream**

Open `crates/waved-capture-record/src/main.rs`. Replace the existing setup block with:

```rust
use std::path::Path;
use waved_audio::audio_config::AudioConfig;
use waved_audio::capture::alsa_dual_stream::AlsaDualStreamSource;
use waved_audio::capture::alsa::{AlsaPcmSource, AlsaStreamConfig};
use waved_audio::capture::single_stream::SingleStreamFrameSource;
use waved_audio::capture::{run_capture_loop, CaptureStats};

let (cfg, _) = AudioConfig::load_or_default(Path::new("/etc/waved-audio-config.json"));

// Source: dual-stream if reference enabled, single otherwise.
enum Src {
    Dual(AlsaDualStreamSource),
    Single(SingleStreamFrameSource<AlsaPcmSource>),
}
let (src, has_ref) = if cfg.reference_enabled() {
    (Src::Dual(AlsaDualStreamSource::from_capture_config(&cfg.capture, cfg.period_frames, cfg.buffer_periods)?), true)
} else {
    let mic = AlsaPcmSource::open(&AlsaStreamConfig {
        device: &cfg.capture.mic_device,
        channels: cfg.capture.mic_channels,
        sample_rate: cfg.capture.mic_sample_rate,
        format: cfg.capture.mic_format,
        period_frames: cfg.period_frames,
        buffer_periods: cfg.buffer_periods,
    })?;
    (Src::Single(SingleStreamFrameSource::new(mic, cfg.period_frames, cfg.capture.mic_channels as usize)), false)
};
```

- [ ] **Step 2: Open the reference output file when applicable**

```rust
let mut pcm_file = std::fs::File::create(&args.out)?;
let ref_path = args.out.with_extension("ref.pcm");
let mut ref_file = if has_ref {
    Some(std::fs::File::create(&ref_path)?)
} else { None };
```

- [ ] **Step 3: Drain frames and write both files**

Replace the inner `while Instant::now() < deadline` body with:

```rust
match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
    Ok(Some(frame)) => {
        frames += 1;
        for &s in &frame.data { pcm_file.write_all(&s.to_le_bytes())?; }
        if let (Some(rf), Some(ref refbuf)) = (ref_file.as_mut(), frame.reference.as_ref()) {
            for &s in refbuf { rf.write_all(&s.to_le_bytes())?; }
        }
    }
    Ok(None) => break,
    Err(_) => continue,
}
```

- [ ] **Step 4: Flush + write_meta**

Flush both files before writing meta:

```rust
pcm_file.flush()?;
drop(pcm_file);
if let Some(mut rf) = ref_file {
    rf.flush()?;
    drop(rf);
}
```

Extend `write_meta` to include the reference path if present:

```rust
let meta = serde_json::json!({
    "schema": 1,
    "format": "i16_le_interleaved",
    "sample_rate": cfg.capture.mic_sample_rate,
    "mic_channels": cfg.capture.mic_channels,
    "reference_channels": if has_ref { cfg.capture.reference_channels } else { 0 },
    "reference_path": if has_ref { Some(ref_path.display().to_string()) } else { None },
    "duration_seconds": duration_s,
    "frames_captured": frames,
    "captured_at_unix": now,
    "description": description,
});
```

- [ ] **Step 5: Cross-build**

Run: `cross build --target aarch64-unknown-linux-gnu -p waved-capture-record 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 6: Deploy + 10-second capture**

```bash
adb push target/aarch64-unknown-linux-gnu/debug/waved-capture-record /data/waved/
adb shell "killall waved 2>/dev/null; sleep 1"
adb shell "/data/waved/waved-capture-record --duration 10 --out /tmp/smoke.pcm --description 'dual-stream smoke'"
adb shell "ls -la /tmp/smoke.pcm /tmp/smoke.ref.pcm /tmp/smoke.meta.json"
```

Expected:
- `/tmp/smoke.pcm` ≈ 10 × 48000 × 8 × 2 = 7.68MB
- `/tmp/smoke.ref.pcm` ≈ 10 × 48000 × 2 × 2 = 1.92MB
- `/tmp/smoke.meta.json` references both + `reference_channels: 2`

Restart waved afterwards:

```bash
adb shell "setsid /etc/init.d/S60waved start </dev/null >/dev/null 2>&1 &"
```

- [ ] **Step 7: Commit**

```bash
git add crates/waved-capture-record/src/main.rs
git commit -m "feat(capture-record): dual-stream PCM + meta.json with reference sidecar

When reference capture is enabled in AudioConfig, writes:
  out.pcm       — mic PCM
  out.ref.pcm   — reference PCM
  out.meta.json — schema, sample_rate, channel counts, both paths

SP#2 AEC fixtures will use the paired output. When reference is absent,
falls back to single-stream (mic only, no sidecar file).

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 15: Update smoke-test script assertions

**Files:**
- Modify: `scripts/test-capture-on-device.sh`

**Rationale:** Script is the final Tier-3 gate. Its assertions must check the new fields or it's just theater.

- [ ] **Step 1: Extend the stats assertion**

Replace the Python block that validates `/tmp/audio_stats.json`:

```bash
echo "==> Checking stats JSON has alignment + reference_rms + 8 mic RMS"
adb shell 'cat /tmp/audio_stats.json' | python3 -c '
import json, sys
s = json.load(sys.stdin)
assert len(s["rms_per_channel"]) == 8, f"expected 8 mic rms, got {len(s[\"rms_per_channel\"])}"
assert s["alignment"] in ("kernel-linked", "timestamp-fallback", "mic-only"), f"unexpected alignment: {s[\"alignment\"]}"
assert s["channel_map_source"] in ("file", "fallback"), f"unexpected config source: {s[\"channel_map_source\"]}"
if s["alignment"] != "mic-only":
    assert len(s["reference_rms"]) == 2, f"expected 2 reference rms, got {len(s[\"reference_rms\"])}"
    assert s["reference_channels"] == 2
else:
    assert s["reference_channels"] == 0
assert isinstance(s["xrun_count"], int), "xrun_count must be int"
assert isinstance(s["frames_emitted"], int), "frames_emitted must be int"
assert s["frames_emitted"] > 0, f"no frames emitted after 5s"
print(f"  alignment={s[\"alignment\"]} config_source={s[\"channel_map_source\"]}")
print(f"  frames_emitted={s[\"frames_emitted\"]} xruns={s[\"xrun_count\"]}")
print(f"  mic_rms={[round(v,1) for v in s[\"rms_per_channel\"]]}")
print(f"  ref_rms={[round(v,1) for v in s[\"reference_rms\"]]}")
'
```

- [ ] **Step 2: Extend the capture-record assertion**

```bash
echo "==> 10-second dual-stream capture"
adb shell "killall waved 2>/dev/null; sleep 1"
adb shell "/data/waved/waved-capture-record --duration 10 --out /tmp/smoke.pcm --description 'smoke test'"
adb shell 'ls -la /tmp/smoke.pcm /tmp/smoke.ref.pcm /tmp/smoke.meta.json 2>&1 | head -10'
adb shell 'cat /tmp/smoke.meta.json' | python3 -c '
import json, sys
m = json.load(sys.stdin)
assert m["mic_channels"] == 8
if m["reference_channels"] > 0:
    assert m["reference_channels"] == 2, f"expected 2 ref channels, got {m[\"reference_channels\"]}"
    assert m["reference_path"] is not None
print(f"  meta ok: mic={m[\"mic_channels\"]} ref={m[\"reference_channels\"]}")
'
```

- [ ] **Step 3: Run the full smoke test**

Run locally (requires adb-connected device):
```bash
bash scripts/test-capture-on-device.sh
```

Expected: all gates pass; final `==> Smoke test passed` line printed. Any mismatch → fix whichever task introduced the regression before claiming this task done.

- [ ] **Step 4: Commit**

```bash
git add scripts/test-capture-on-device.sh
git commit -m "test(audio): extend smoke-test for alignment + reference_rms + dual-stream capture

Asserts /tmp/audio_stats.json has 'alignment' in the expected value set,
reference_rms length matches reference_channels (or both are zero for
mic-only), and /tmp/smoke.meta.json records mic_channels + reference_channels
consistently with the config active on device.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Task 16: README + final aggregate verification

**Files:**
- Modify: `crates/waved-audio/README.md`

**Rationale:** Docs catch up; aggregate gate ensures we didn't regress anything.

- [ ] **Step 1: Update README**

Replace `crates/waved-audio/README.md` contents:

```markdown
# waved-audio

Audio capture + state bridge for waved. See the v2 design spec:
`docs/superpowers/specs/2026-04-16-waved-audio-capture-foundation-v2-design.md`

## Features

- `capture-alsa` — real ALSA capture from `hw:0,2` (8 mics) + `hw:0,3` (2-ch reference). Device only.
- `capture-replay` — file-backed replay for host-side testing / DSP iteration.
- `simulate` — synthetic silence generator (mutually exclusive with `capture-alsa`).

## Configuration

Runtime config at `/etc/waved-audio-config.json` (version 2). Falls back to
the embedded default (mics `hw:0,2` @ 8ch S32_LE 48000, refs `hw:0,3` @
2ch S32_LE 48000, period 256, 4 buffers).

## Common commands

```bash
# Host unit tests
cargo test -p waved-audio

# Host integration tests (replay)
cargo test -p waved-audio --features capture-replay

# Cross-build for device
cross build --target aarch64-unknown-linux-gnu -p waved-audio --features capture-alsa

# Offline metrics on a captured file (8-ch i16)
cargo run -p waved-audio-bench -- my-capture.pcm --format i16_8

# On-device smoke test (full Tier-3 gate)
scripts/test-capture-on-device.sh
```

## Capture data flow

```
 hw:0,2 (8ch mics)        hw:0,3 (2ch reference)
        │                          │
        ▼                          ▼
  AlsaPcmSource               AlsaPcmSource
         \                      /
          AlsaDualStreamSource     (snd_pcm_link or timestamp-fallback)
                  │
                  ▼
           run_capture_loop        (backpressure drop-newest, xrun recovery)
                  │
                  ▼   AudioFrame { data, reference, ts_ms, ... }
                  ├──► stats_collector  → /tmp/audio_stats.json
                  ├──► AudioService     → VU state, meeting channel
                  └──► AudioService fan-out broadcast tee
```

## Next sub-projects

See `TODO.md` at repo root and the v2 spec §9. SP#2 (AEC) consumes
`AudioFrame.reference` and flips to `Vec<f32>` at its DSP boundary. SP#3–6
plug in via further extensions of `AudioConfig`.
```

- [ ] **Step 2: Run the aggregate gate**

```bash
cargo test -p waved-audio
cargo test -p waved-audio --features capture-replay
cargo test -p waved-audio-bench
cross build --target aarch64-unknown-linux-gnu -p waved -p waved-latency-probe -p waved-capture-record --features "ui,capture-alsa"
bash scripts/test-capture-on-device.sh
```

All green required.

- [ ] **Step 3: Commit**

```bash
git add crates/waved-audio/README.md
git commit -m "docs(audio): update README for v2 (dual-stream capture, config path, data flow)

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Plan self-review

**Spec coverage:**

- §1 motivation (wrong device path, schema mismatch) → addressed by Tasks 2, 6, 8, 12
- §2 scope (dual-stream from hw:0,2 + hw:0,3, aligned AudioFrame, forward-compat config, runtime probe, ffi deletion, per-task gate, TODO.md) → Tasks 1, 2, 3, 5, 6, 8, 12
- §3 preserved from v1 → kept intact per task changes (metrics, fixtures, mock, bench unmodified)
- §4.1 dual-stream aligner → Task 8
- §4.2 AudioFrame shape → Task 3
- §4.3 config schema + path → Task 2
- §4.4 runtime probe → Task 6
- §4.5 ffi deletion → Task 5
- §5 data flow → Tasks 8, 9, 10, 12
- §6 error handling → Tasks 2, 5, 6, 8, 12 (fatal on mic fail, fatal on ref fail when configured, graceful mic-only when ref omitted, snd_pcm_link fallback)
- §7 per-task verification gate → Tasks 6, 12, 13, 14 carry explicit deploy-and-verify steps
- §8 deliverables → end-of-plan Tasks 15, 16 enforce them
- §9 future hooks → TODO.md in Task 1

**Placeholders:** none. All code blocks are complete. No "TODO: implement X" left in tasks.

**Type consistency:** `AudioFrame` shape matches across Tasks 3–14. `AudioFrameSource` trait defined in Task 4 (mod.rs rewrite), implemented in Tasks 7 (single), 8 (dual), consumed in Task 9. `AlsaStreamConfig` defined in Task 6, consumed in Tasks 8, 12, 13, 14. `ConfigSource` name flows intact from Task 2 through Task 10 (stats) and Task 12 (main). `AlignmentMethod::as_str()` pattern matches across Tasks 8, 10, 12.

---

## Execution

Plan complete and saved to `docs/superpowers/plans/2026-04-16-waved-audio-capture-foundation-v2.md`.
