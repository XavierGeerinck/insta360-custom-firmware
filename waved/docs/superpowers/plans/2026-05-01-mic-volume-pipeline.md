# Mic Volume Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire `[audio].mic_volume` (TOML) and the UAC2 host mic mixer to actually change the audio level on both recording and UAC2 uplink, fixing today's silent recordings.

**Architecture:** Two-stage gain. PDM Gain (hardware preamp, ALSA `numid=9`) is set once at daemon startup to a fixed operating point of `24` (~+12 dB lift) so DSP tuning sees a stable input level forever. A software linear-percent gain (`apply_mic_volume`) runs at the end of `AudioService::process_frame`, before `processed_tx.send`, so both consumers (recording WAV and UAC2 uplink) get the same treatment from one knob.

**Tech Stack:** Rust, tokio (broadcast/watch), ALSA via `alsa` crate, serde/toml for config, hound for WAV.

**Spec:** `docs/superpowers/specs/2026-05-01-mic-volume-pipeline-design.md`

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `waved/crates/waved-audio/src/lib.rs` | modify | Add `apply_mic_volume` free fn; call it at end of `process_frame` for mic mono and stereo |
| `waved/crates/waved-audio/src/audio_config.rs` | modify | Add `pdm_gain: u8` to `CaptureConfig`, validate 0..=48 |
| `waved/crates/waved-audio/config/audio_config_default.json` | modify | Add `"pdm_gain": 24` |
| `waved/crates/waved-audio/src/capture/pdm_gain.rs` | create | `init(card: &str, value: u8)` — one-shot ALSA mixer write |
| `waved/crates/waved-audio/src/capture/mod.rs` | modify | `pub mod pdm_gain;` |
| `waved/crates/waved-audio/src/uac/control.rs` | modify | Parameterize `read_volume`/`write_volume` with `max_pct`; mic uses 400, speaker stays 100; one-shot mic mixer sync at spawn |
| `waved/crates/waved-audio/src/stats_collector.rs` | modify | Add `mic_gain_linear: f32` and `mic_gain_db: f32` to `AudioStats`; populate from `AudioState.mic_volume` |
| `waved/crates/waved/src/main.rs` | modify | Seed `AudioState.mic_volume` from `WavedConfig.audio.mic_volume`; call `pdm_gain::init` once at capture-init time |
| `waved/scripts/test-mic-volume.sh` | create | On-device acceptance: record, measure peak/RMS, exercise UAC2 slider |
| `waved/docs/AUDIO_HARDWARE.md` | modify | Document two-stage gain, PDM Gain operating point, `mic_volume` semantic |

---

## Task 1: `apply_mic_volume` helper + unit tests

**Files:**
- Modify: `waved/crates/waved-audio/src/lib.rs` (add free fn near top of file, before `pub struct AudioService`)
- Test: same file, `#[cfg(test)] mod tests` already exists or add one

- [ ] **Step 1: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` block in `waved/crates/waved-audio/src/lib.rs` (or create one if absent):

```rust
#[test]
fn apply_mic_volume_unity_is_identity() {
    let mut s = vec![0.1, -0.2, 0.5, -0.5];
    apply_mic_volume(&mut s, 100);
    assert_eq!(s, vec![0.1, -0.2, 0.5, -0.5]);
}

#[test]
fn apply_mic_volume_doubles_at_200() {
    let mut s = vec![0.4, -0.3, 0.0];
    apply_mic_volume(&mut s, 200);
    assert!((s[0] - 0.8).abs() < 1e-6, "expected 0.8, got {}", s[0]);
    assert!((s[1] - (-0.6)).abs() < 1e-6, "expected -0.6, got {}", s[1]);
    assert_eq!(s[2], 0.0);
}

#[test]
fn apply_mic_volume_silences_at_zero() {
    let mut s = vec![0.5, -0.3, 1.0];
    apply_mic_volume(&mut s, 0);
    assert_eq!(s, vec![0.0, 0.0, 0.0]);
}

#[test]
fn apply_mic_volume_clamps_to_plus_minus_unity() {
    let mut s = vec![0.6, -0.6, 0.7, -0.7];
    apply_mic_volume(&mut s, 200);
    assert_eq!(s, vec![1.0, -1.0, 1.0, -1.0]);
}

#[test]
fn apply_mic_volume_clamps_input_pct_above_400() {
    let mut a = vec![0.1];
    let mut b = vec![0.1];
    apply_mic_volume(&mut a, 500);
    apply_mic_volume(&mut b, 400);
    assert_eq!(a, b);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p waved-audio --lib apply_mic_volume`
Expected: FAIL with "cannot find function `apply_mic_volume`".

- [ ] **Step 3: Implement `apply_mic_volume`**

Add near the top of `waved/crates/waved-audio/src/lib.rs` (just before `pub struct AudioService`):

```rust
/// Apply user mic volume to a slice of mono f32 samples in-place.
/// `pct` is linear percent: 100 = unity, 200 = 2×, 0 = silence.
/// Values above 400 are clamped to 400 to prevent catastrophic gain.
/// Output samples are hard-clamped to [-1.0, 1.0] so downstream i16
/// quantization never wraps.
pub fn apply_mic_volume(samples: &mut [f32], pct: u16) {
    let pct = pct.min(400);
    let g = pct as f32 / 100.0;
    for s in samples.iter_mut() {
        *s = (*s * g).clamp(-1.0, 1.0);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p waved-audio --lib apply_mic_volume`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-audio/src/lib.rs
git commit -m "feat(audio): apply_mic_volume free fn (linear-percent gain + clamp)"
```

---

## Task 2: `AudioService::process_frame` applies gain at chain end

**Files:**
- Modify: `waved/crates/waved-audio/src/lib.rs` (`process_frame`, both 8-ch and 1-ch paths)
- Test: same file, embedded `#[cfg(test)]` module

**Why in `process_frame` and not the run loop:** keeps gain inside the testable DSP unit. The 8-ch path and the legacy 1-ch path both end in a `ProcessedAudioFrame`; we apply gain to both right before returning.

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` block in `waved/crates/waved-audio/src/lib.rs`:

```rust
#[test]
fn process_frame_applies_mic_volume_to_mono() {
    use crate::callbacks::AudioFrame;
    use std::sync::Arc;
    use tokio::sync::{mpsc, watch};
    use waved_state::AudioState;

    // Build a service with mic_volume = 200 in initial state.
    let mut state = AudioState::default();
    state.mic_volume = 200;
    let (audio_tx, _audio_rx) = watch::channel(state);
    let (frame_tx, frame_rx) = mpsc::channel(8);
    let (meeting_tx, _meeting_rx) = mpsc::channel(8);
    let mut svc = AudioService::new(
        Arc::new(audio_tx),
        frame_tx,
        frame_rx,
        meeting_tx,
        256,
        48_000,
        crate::beam::MicGeometry::default(),
    );

    // Mono frame (1 channel) so process_frame takes the legacy path:
    // its mic_mono_scratch is just the raw samples copied in, AEC is
    // disabled, NS is off by default → output mic_mono_f32 reflects
    // the i16 input scaled by 1/32768, then multiplied by gain.
    let raw = vec![1000i16; 256];
    let frame = AudioFrame::mic_only(raw, 1, 48_000, 0);
    let processed = svc.process_frame(&frame);

    // 1000/32768 = 0.0305 base; ×2 = 0.0610.
    let expected = (1000.0_f32 / 32768.0) * 2.0;
    let max = processed.mic_mono_f32.iter().cloned().fold(0.0_f32, f32::max);
    assert!(
        (max - expected).abs() < 1e-3,
        "expected ~{expected}, got max {max}",
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p waved-audio --lib process_frame_applies_mic_volume_to_mono`
Expected: FAIL — `max` is half the expected value (no gain applied yet).

- [ ] **Step 3: Apply gain at the end of each `process_frame` branch**

In `waved/crates/waved-audio/src/lib.rs`, the existing `process_frame` (around lines 241–321) has two branches that each produce a `ProcessedAudioFrame`. Edit each one so it routes through a shared `apply_gain` helper before returning.

**8-ch path — replace lines that currently look like this:**

Old:
```rust
            let mut p = ProcessedAudioFrame::from_raw(frame, &aec_out, &self.ref_mono_scratch);
            if let Some((l, r)) = beam_out.stereo {
                p = p.with_stereo(l, r);
            }
            p
```

New:
```rust
            let mut p = ProcessedAudioFrame::from_raw(frame, &aec_out, &self.ref_mono_scratch);
            if let Some((l, r)) = beam_out.stereo {
                p = p.with_stereo(l, r);
            }
            apply_gain(self, p)
```

**Legacy path — replace the line that currently looks like this:**

Old:
```rust
            ProcessedAudioFrame::from_raw(frame, &aec_out, &self.ref_mono_scratch)
```

New:
```rust
            let p = ProcessedAudioFrame::from_raw(frame, &aec_out, &self.ref_mono_scratch);
            apply_gain(self, p)
```

**Add the helper as a free fn outside any `impl` block.** Place it directly after `process_frame` (or near the top of the file with other free fns — either is fine):

```rust
fn apply_gain(svc: &AudioService, mut processed: ProcessedAudioFrame) -> ProcessedAudioFrame {
    let mic_vol = svc.audio_tx.borrow().mic_volume;
    apply_mic_volume(&mut processed.mic_mono_f32, mic_vol);
    if let Some((l, r)) = processed.mic_stereo_f32.as_mut() {
        apply_mic_volume(l, mic_vol);
        apply_mic_volume(r, mic_vol);
    }
    processed
}
```

`audio_tx` is a private field of `AudioService`; `apply_gain` lives in the same module so visibility is fine. Free fn (not method) sidesteps split-borrow issues if `process_frame`'s receiver is mid-mutation when gain runs.

- [ ] **Step 4: Run all `waved-audio` lib tests**

Run: `cargo test -p waved-audio --lib`
Expected: PASS — including the new test and all pre-existing ones.

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-audio/src/lib.rs
git commit -m "feat(audio): apply mic_volume gain at end of process_frame

Both 8-ch and legacy paths route through a shared gain helper that
reads AudioState.mic_volume and scales mic_mono_f32 (and mic_stereo_f32
when present) before returning. processed_tx and meeting_tx — and
therefore both recording and UAC2 uplink — see the gained signal."
```

---

## Task 3: Seed `AudioState.mic_volume` from TOML at startup

**Files:**
- Modify: `waved/crates/waved/src/main.rs` (after `let bus = Arc::new(StateBus::new());` at line 48)
- Test: a new lightweight unit test in `waved-state` that confirms the helper does what we expect, since main is hard to test directly

- [ ] **Step 1: Write the failing test**

Add a new test module to `waved/crates/waved-state/src/lib.rs` inside the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn audio_state_seed_helper_writes_mic_volume() {
    let bus = StateBus::new();
    // Simulate the seed call main.rs will make.
    bus.audio.send_modify(|s| s.mic_volume = 250);
    assert_eq!(bus.audio.borrow().mic_volume, 250);
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p waved-state --lib audio_state_seed_helper_writes_mic_volume`
Expected: PASS — this just confirms the API surface; the actual seed lives in main.rs.

- [ ] **Step 3: Add the seed call to `main.rs`**

In `waved/crates/waved/src/main.rs`, immediately after `let bus = Arc::new(StateBus::new());` (line 48), add:

```rust
    // Seed mutable audio state from TOML so apply_mic_volume sees the
    // user-configured boot value before the first frame is processed.
    bus.audio.send_modify(|s| {
        s.mic_volume = config.audio.mic_volume;
    });
```

- [ ] **Step 4: Build the daemon to confirm it compiles**

Run: `cargo build -p waved`
Expected: builds cleanly.

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved/src/main.rs waved/crates/waved-state/src/lib.rs
git commit -m "feat(waved): seed AudioState.mic_volume from TOML at startup

The post-DSP gain stage reads mic_volume from the audio watch state.
Seeding it from WavedConfig means the very first processed frame is
gained correctly, without waiting for a host UAC2 mixer event."
```

---

## Task 4: `pdm_gain` config field + JSON schema validation

**Files:**
- Modify: `waved/crates/waved-audio/src/audio_config.rs` (`CaptureConfig` struct + `validate`)
- Modify: `waved/crates/waved-audio/config/audio_config_default.json`
- Test: same `audio_config.rs`

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block in `waved/crates/waved-audio/src/audio_config.rs`:

```rust
#[test]
fn embedded_default_has_pdm_gain_24() {
    let cfg = AudioConfig::default_embedded();
    assert_eq!(cfg.capture.pdm_gain, 24);
}

#[test]
fn pdm_gain_above_48_is_rejected() {
    let json = r#"{
        "version": 4,
        "capture": {
            "mic_device": "hw:0,2",
            "mic_channels": 8,
            "mic_sample_rate": 48000,
            "mic_format": "S32_LE",
            "pdm_gain": 49
        },
        "period_frames": 1024,
        "buffer_periods": 8
    }"#;
    assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
}

#[test]
fn pdm_gain_at_48_is_accepted() {
    let json = r#"{
        "version": 4,
        "capture": {
            "mic_device": "hw:0,2",
            "mic_channels": 8,
            "mic_sample_rate": 48000,
            "mic_format": "S32_LE",
            "pdm_gain": 48
        },
        "period_frames": 1024,
        "buffer_periods": 8
    }"#;
    let cfg = AudioConfig::from_json(json).expect("48 should validate");
    assert_eq!(cfg.capture.pdm_gain, 48);
}

#[test]
fn pdm_gain_absent_uses_serde_default() {
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
    let cfg = AudioConfig::from_json(json).expect("missing pdm_gain should default");
    assert_eq!(cfg.capture.pdm_gain, 24);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p waved-audio --lib pdm_gain`
Expected: FAIL — field doesn't exist yet.

- [ ] **Step 3: Add the field with serde default + validation**

In `waved/crates/waved-audio/src/audio_config.rs`, modify `CaptureConfig`:

```rust
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
    /// PDM hardware preamp gain (ALSA `numid=9` on `hw:0`). Range 0..=48.
    /// Set once at daemon startup; static across the device's run so DSP
    /// tuning sees a stable input level. See the mic-volume-pipeline spec.
    #[serde(default = "default_pdm_gain")]
    pub pdm_gain: u8,
}

fn default_pdm_gain() -> u8 { 24 }
```

In `validate`, add (immediately after the existing checks for `mic_channels` and `mic_sample_rate`, before the reference-device block):

```rust
        if self.capture.pdm_gain > 48 {
            return Err(ConfigError::Invalid(format!(
                "pdm_gain must be in [0, 48], got {}", self.capture.pdm_gain
            )));
        }
```

- [ ] **Step 4: Update the embedded JSON default**

Edit `waved/crates/waved-audio/config/audio_config_default.json`:

```json
{
  "version": 4,
  "capture": {
    "mic_device": "hw:0,2",
    "mic_channels": 8,
    "mic_sample_rate": 48000,
    "mic_format": "S32_LE",
    "pdm_gain": 24
  },
  "playback": {
    "device": "hw:0,1",
    "channels": 2,
    "sample_rate": 48000,
    "format": "S16_LE",
    "period_frames": 1024,
    "buffer_periods": 4
  },
  "period_frames": 1024,
  "buffer_periods": 8
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p waved-audio --lib`
Expected: PASS — including all four new `pdm_gain` tests and all existing config tests.

- [ ] **Step 6: Commit**

```bash
git add waved/crates/waved-audio/src/audio_config.rs waved/crates/waved-audio/config/audio_config_default.json
git commit -m "feat(audio-config): pdm_gain field on CaptureConfig (default 24)

Hardware preamp operating point. Validated 0..=48; serde-defaults to 24
when absent so older config JSONs still parse."
```

---

## Task 5: PDM Gain init module — one-shot ALSA mixer write

**Files:**
- Create: `waved/crates/waved-audio/src/capture/pdm_gain.rs`
- Modify: `waved/crates/waved-audio/src/capture/mod.rs` (add `pub mod pdm_gain;`)

**Note on the underlying ALSA API:** unlike the named simple-element controls used by `uac::control` (`PCM Capture Volume`), PDM Gain is a *card-level* control identified by `numid=9`. The `alsa::ctl` API is the right tool — `alsa::Mixer` only sees Selem controls. We use `alsa::Ctl` + `alsa::ctl::ElemId::new(numid)` and write an integer value.

**Fallback if the alsa-rs ctl API differs from what's shown below:** PDM Gain init runs exactly once at startup, so shelling out to `amixer` is acceptable. If the `alsa::ctl` import paths or method signatures don't compile against the version of the `alsa` crate this repo uses, replace the body of `init` with `std::process::Command::new("amixer").args(["-c", card.trim_start_matches("hw:"), "cset", "numid=9", &value.to_string()]).status()?` and a non-zero-exit check. Document this fallback in a code comment.

- [ ] **Step 1: Write the failing test**

Create `waved/crates/waved-audio/src/capture/pdm_gain.rs` with the test scaffolding first:

```rust
//! PDM Gain hardware preamp init — one-shot ALSA mixer write at daemon
//! startup so the ADC sees a consistent level forever. See the
//! mic-volume-pipeline spec for why we don't move this dynamically.

use alsa::ctl::{Ctl, ElemId, ElemIface, ElemValue};
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum PdmGainError {
    #[error("alsa: {0}")]
    Alsa(#[from] alsa::Error),
    #[error("control numid={0} not found on card {1}")]
    NotFound(u32, String),
}

/// Set the PDM Gain mixer control on `card` (e.g. `"hw:0"`) to `value`
/// (validated 0..=48 by the caller). Logs and returns the error rather
/// than panicking — capture can still run with whatever gain was
/// previously set.
pub fn init(card: &str, value: u8) -> Result<(), PdmGainError> {
    const PDM_GAIN_NUMID: u32 = 9;

    let ctl = Ctl::new(card, false)?;
    let mut id = ElemId::new(ElemIface::Mixer);
    id.set_numid(PDM_GAIN_NUMID);

    let mut elem = ElemValue::new(alsa::ctl::ElemType::Integer)?;
    elem.set_id(&id);
    elem.set_integer(0, value as i32)
        .ok_or(PdmGainError::NotFound(PDM_GAIN_NUMID, card.to_string()))?;

    ctl.elem_write(&mut elem)?;
    info!("pdm_gain: set numid=9 on {card} → {value}");
    Ok(())
}

/// Convenience: log a warning on failure and continue. Capture is still
/// viable with the prior mixer value.
pub fn init_or_warn(card: &str, value: u8) {
    if let Err(e) = init(card, value) {
        warn!("pdm_gain init on {card} failed: {e}; capture continues at prior gain");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_or_warn_does_not_panic_on_missing_card() {
        // No ALSA card by this name on the host — function must handle
        // the error and return without panicking.
        init_or_warn("hw:99", 24);
    }

    #[test]
    fn init_returns_error_on_missing_card() {
        let result = init("hw:99", 24);
        assert!(result.is_err(), "expected error on missing card, got {result:?}");
    }
}
```

- [ ] **Step 2: Wire into `capture/mod.rs`**

Edit `waved/crates/waved-audio/src/capture/mod.rs` and add:

```rust
pub mod pdm_gain;
```

(Placement: alongside other `pub mod` lines. The exact location doesn't matter; alphabetical is fine.)

- [ ] **Step 3: Run the tests**

Run: `cargo test -p waved-audio --lib pdm_gain`
Expected: PASS — both tests succeed because they only assert error handling, not real ALSA behavior.

- [ ] **Step 4: Commit**

```bash
git add waved/crates/waved-audio/src/capture/pdm_gain.rs waved/crates/waved-audio/src/capture/mod.rs
git commit -m "feat(audio/capture): pdm_gain::init for one-shot ALSA preamp set

Card-level numid=9 control on hw:0. Errors are logged but non-fatal —
capture continues at whatever gain was last written."
```

---

## Task 6: Wire PDM Gain init in daemon main

**Files:**
- Modify: `waved/crates/waved/src/main.rs` (the `capture-alsa` block, after `AudioConfig::load_or_default`)

- [ ] **Step 1: Locate the existing audio-config load**

In `waved/crates/waved/src/main.rs`, find the block (currently around line 157):

```rust
        let (cfg, cfg_src) = AudioConfig::load_or_default(Path::new("/etc/waved-audio-config.json"));
```

- [ ] **Step 2: Add the PDM Gain init call**

Immediately after the `cfg_src_str` `match` block (around line 165), insert:

```rust
        // Set the hardware preamp once. Static across the device's run
        // so DSP (AEC, DFN3) sees a stable input level. See
        // docs/superpowers/specs/2026-05-01-mic-volume-pipeline-design.md.
        waved_audio::capture::pdm_gain::init_or_warn("hw:0", cfg.capture.pdm_gain);
```

- [ ] **Step 3: Build the daemon**

Run: `cargo build -p waved --features capture-alsa`
Expected: builds cleanly.

- [ ] **Step 4: Commit**

```bash
git add waved/crates/waved/src/main.rs
git commit -m "feat(waved): init PDM Gain at startup from audio-config

Hooks pdm_gain::init_or_warn into the capture-alsa init path so the
hardware preamp is set before the first frame is captured."
```

---

## Task 7: UAC2 mixer scale parameterization (mic 0..=400, speaker 100) + initial sync

**Files:**
- Modify: `waved/crates/waved-audio/src/uac/control.rs`

**Why:** The existing `read_volume`/`write_volume` hardcode 0..=100 percent. Spec mandates the mic value space is 0..=400 (linear-percent). Speaker stays 0..=100. We add a `max_pct` param and update both callsites. We also write the current `mic_volume` to the mixer once at spawn so the host's view starts in sync with our boot default of 200.

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `waved/crates/waved-audio/src/uac/control.rs` (or create one if absent — there is no existing test module in this file, so add one at the bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pct_to_raw_unity_at_max_pct_is_max_raw() {
        // The conversion logic embedded in write_volume / read_volume:
        // pct ∈ [0, max_pct], raw ∈ [min_raw, max_raw].
        assert_eq!(pct_to_raw(0, 0, 64, 100), 0);
        assert_eq!(pct_to_raw(100, 0, 64, 100), 64);
        assert_eq!(pct_to_raw(50, 0, 64, 100), 32);

        assert_eq!(pct_to_raw(0, 0, 64, 400), 0);
        assert_eq!(pct_to_raw(400, 0, 64, 400), 64);
        assert_eq!(pct_to_raw(200, 0, 64, 400), 32);
        assert_eq!(pct_to_raw(100, 0, 64, 400), 16);
    }

    #[test]
    fn raw_to_pct_inverse() {
        assert_eq!(raw_to_pct(0, 0, 64, 100), 0);
        assert_eq!(raw_to_pct(64, 0, 64, 100), 100);
        assert_eq!(raw_to_pct(64, 0, 64, 400), 400);
        assert_eq!(raw_to_pct(32, 0, 64, 400), 200);
    }

    #[test]
    fn pct_to_raw_clamps_above_max_pct() {
        assert_eq!(pct_to_raw(500, 0, 64, 400), 64);
        assert_eq!(pct_to_raw(150, 0, 64, 100), 64);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p waved-audio --lib --features capture-alsa pct_to_raw`
Expected: FAIL — functions don't exist yet.

- [ ] **Step 3: Extract pure conversion helpers + parameterize read/write**

In `waved/crates/waved-audio/src/uac/control.rs`, replace the existing `read_volume` and `write_volume` with parameterized versions that delegate to pure functions:

```rust
/// Convert ALSA raw mixer value to our percent space [0, max_pct].
fn raw_to_pct(raw: i64, min: i64, max: i64, max_pct: u16) -> u16 {
    let span = (max - min).max(1) as f64;
    let pct = ((raw - min) as f64 / span * max_pct as f64).round() as i64;
    pct.clamp(0, max_pct as i64) as u16
}

/// Convert our percent value [0, max_pct] back to ALSA raw range.
fn pct_to_raw(pct: u16, min: i64, max: i64, max_pct: u16) -> i64 {
    let span = (max - min) as f64;
    let raw = min + (span * pct as f64 / max_pct as f64).round() as i64;
    raw.clamp(min, max)
}

fn read_volume(mixer: &Mixer, name: &str, capture: bool, max_pct: u16) -> Option<u16> {
    let id = SelemId::new(name, 0);
    let elem = mixer.find_selem(&id)?;
    let (min, max) = if capture { elem.get_capture_volume_range() }
                     else       { elem.get_playback_volume_range() };
    let raw = if capture {
        elem.get_capture_volume(SelemChannelId::mono()).ok()?
    } else {
        elem.get_playback_volume(SelemChannelId::mono()).ok()?
    };
    Some(raw_to_pct(raw, min, max, max_pct))
}

fn write_volume(mixer: &Mixer, name: &str, capture: bool, percent: u16, max_pct: u16) -> Result<(), alsa::Error> {
    let id = SelemId::new(name, 0);
    let elem = match mixer.find_selem(&id) {
        Some(e) => e,
        None => return Ok(()),
    };
    let (min, max) = if capture { elem.get_capture_volume_range() }
                     else       { elem.get_playback_volume_range() };
    let raw = pct_to_raw(percent, min, max, max_pct);
    if capture {
        elem.set_capture_volume_all(raw)?;
    } else {
        elem.set_playback_volume_all(raw)?;
    }
    Ok(())
}
```

- [ ] **Step 4: Update all callsites with the right `max_pct`**

In the same file, update every `read_volume`/`write_volume` call. Speaker uses `100`, mic uses `400`.

In `run_mixer_poll`:

Old:
```rust
    let mut prev_speaker_vol = read_volume(&mixer, "PCM", false).unwrap_or(0);
    let mut prev_mic_vol = read_volume(&mixer, "PCM", true).unwrap_or(0);
```
New:
```rust
    let mut prev_speaker_vol = read_volume(&mixer, "PCM", false, 100).unwrap_or(0);
    let mut prev_mic_vol = read_volume(&mixer, "PCM", true, 400).unwrap_or(0);
```

Old (the inner `read_volume` calls inside the loop):
```rust
        if let Some(v) = read_volume(&mixer, "PCM", false) {
```
```rust
        if let Some(v) = read_volume(&mixer, "PCM", true) {
```
New:
```rust
        if let Some(v) = read_volume(&mixer, "PCM", false, 100) {
```
```rust
        if let Some(v) = read_volume(&mixer, "PCM", true, 400) {
```

In `run_state_mirror`:

Old:
```rust
                let _ = write_volume(&mixer, "PCM", false, s.speaker_volume);
```
```rust
                let _ = write_volume(&mixer, "PCM", true, s.mic_volume);
```
New:
```rust
                let _ = write_volume(&mixer, "PCM", false, s.speaker_volume, 100);
```
```rust
                let _ = write_volume(&mixer, "PCM", true, s.mic_volume, 400);
```

- [ ] **Step 5: Add the initial sync write at spawn**

Inside `run_state_mirror`, immediately after the existing `let mut prev_mic_vol = state_rx.borrow().mic_volume;` line, add:

```rust
    // Push the seeded boot value into the UAC2 mixer so the host's view
    // starts in sync with AudioState.mic_volume (which is itself seeded
    // from TOML). Without this, the mixer keeps whatever the kernel
    // initialised it to and the host's first read will not match.
    if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
        let _ = write_volume(&mixer, "PCM", true, prev_mic_vol, 400);
        let _ = write_volume(&mixer, "PCM", false, prev_speaker_vol, 100);
        last_write.mic_volume.store(prev_mic_vol, Ordering::Relaxed);
        last_write.speaker_volume.store(prev_speaker_vol, Ordering::Relaxed);
    }
```

- [ ] **Step 6: Run all tests**

Run: `cargo test -p waved-audio --lib --features capture-alsa`
Expected: PASS — including the new `pct_to_raw` / `raw_to_pct` tests and all existing UAC tests. (If a `--features capture-alsa` test target isn't already configured, fall back to `cargo test -p waved-audio --features capture-alsa`.)

- [ ] **Step 7: Commit**

```bash
git add waved/crates/waved-audio/src/uac/control.rs
git commit -m "feat(audio/uac): parameterize mixer percent scale (mic 0..400)

Mic value space spans 0..400 (linear-percent matching apply_mic_volume),
speaker stays 0..100. Adds an initial mixer sync at spawn so the host's
view starts at the boot mic_volume seeded from TOML."
```

---

## Task 8: Stats — emit `mic_gain_linear` and `mic_gain_db`

**Files:**
- Modify: `waved/crates/waved-audio/src/stats_collector.rs` (`AudioStats` struct, `compute_stats_with_processed`, the test that constructs an `AudioStats`)

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)]` block in `waved/crates/waved-audio/src/stats_collector.rs`:

```rust
#[test]
fn audio_stats_includes_mic_gain_when_volume_set() {
    use waved_state::AudioState;
    let frame = AudioFrame::mic_only(vec![0i16; 8 * 256], 8, 48_000, 0);
    let cs = CaptureStats::new();
    let mut state = AudioState::default();
    state.mic_volume = 200; // 2× linear, +6 dB
    let stats = compute_stats_with_processed(
        &frame, &cs, ChannelMapSource::File, None, "none", Some(123.4), &state, None,
        #[cfg(feature = "capture-alsa")]
        None,
    );
    assert!((stats.mic_gain_linear - 2.0).abs() < 1e-6, "expected 2.0, got {}", stats.mic_gain_linear);
    assert!((stats.mic_gain_db - 6.0205994).abs() < 0.01, "expected ~6.02 dB, got {}", stats.mic_gain_db);
}

#[test]
fn audio_stats_mic_gain_zero_volume_emits_minus_inf_dB_as_neg_120() {
    use waved_state::AudioState;
    let frame = AudioFrame::mic_only(vec![0i16; 8 * 256], 8, 48_000, 0);
    let cs = CaptureStats::new();
    let mut state = AudioState::default();
    state.mic_volume = 0;
    let stats = compute_stats_with_processed(
        &frame, &cs, ChannelMapSource::File, None, "none", None, &state, None,
        #[cfg(feature = "capture-alsa")]
        None,
    );
    assert_eq!(stats.mic_gain_linear, 0.0);
    // Convention: clamp -inf to -120 dB so JSON stays finite.
    assert_eq!(stats.mic_gain_db, -120.0);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p waved-audio --lib audio_stats_includes_mic_gain`
Expected: FAIL — `mic_gain_linear` and `mic_gain_db` fields don't exist.

- [ ] **Step 3: Add the fields and computation**

In `waved/crates/waved-audio/src/stats_collector.rs`, modify `AudioStats`:

```rust
#[derive(Serialize, Debug, Clone)]
pub struct AudioStats {
    pub last_updated_ms: u64,
    pub channel_map_source: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames_emitted: u64,
    pub frames_dropped: u64,
    pub xrun_count: u64,
    pub rms_per_channel: Vec<f32>,
    pub clipping_rate_per_channel: Vec<f32>,
    pub dc_offset_per_channel: Vec<f32>,
    pub correlation_matrix: Vec<Vec<f32>>,
    pub last_write_error: Option<String>,
    pub reference_rms: Vec<f32>,
    pub alignment: String,
    pub mic_post_aec_rms: Option<f32>,
    pub doa_horizontal: f32,
    pub doa_pitch: f32,
    pub mode: AudioMode,
    pub playback_preset: String,
    pub mbagc_active_gain_db: [f32; 4],
    pub limiter_reduction_db: f32,
    /// Linear mic gain currently applied post-DSP. 1.0 = unity, 2.0 = +6 dB.
    /// Tracks `AudioState.mic_volume / 100`.
    pub mic_gain_linear: f32,
    /// Same as `mic_gain_linear` but expressed in dB; -120.0 when muted.
    pub mic_gain_db: f32,
    #[cfg(feature = "capture-alsa")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uac: Option<crate::uac::UacStatsSnapshot>,
}
```

In `compute_stats_with_processed`, add the computation just before the final `AudioStats { ... }` literal:

```rust
    let mic_gain_linear = (audio_state.mic_volume.min(400) as f32) / 100.0;
    let mic_gain_db = if mic_gain_linear > 0.0 {
        20.0 * mic_gain_linear.log10()
    } else {
        -120.0
    };
```

And populate the new fields in the `AudioStats { ... }` literal:

```rust
        mic_gain_linear,
        mic_gain_db,
```

- [ ] **Step 4: Run all stats tests**

Run: `cargo test -p waved-audio --lib stats`
Expected: PASS — including the two new tests and all pre-existing ones (any pre-existing test that constructs an `AudioStats` literal will fail compilation; if so, add `mic_gain_linear: 0.0, mic_gain_db: -120.0` to those literals).

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-audio/src/stats_collector.rs
git commit -m "feat(audio/stats): emit mic_gain_linear and mic_gain_db

Surfaces the post-DSP mic gain in /tmp/audio_stats.json so we can verify
on-device that mic_volume changes are taking effect. -120 dB is the
'muted' sentinel since log10(0) is -inf."
```

---

## Task 9: On-device acceptance script + AUDIO_HARDWARE.md

**Files:**
- Create: `waved/scripts/test-mic-volume.sh`
- Modify: `waved/docs/AUDIO_HARDWARE.md`

**This task has no automated tests** — it's an operator-run smoke test. The `.sh` script is what we run on the device after deploy; the docs explain why the values are what they are.

- [ ] **Step 1: Write the on-device acceptance script**

Create `waved/scripts/test-mic-volume.sh`:

```bash
#!/usr/bin/env bash
# On-device acceptance: confirms mic_volume + pdm_gain produce audible
# recordings and that UAC2 mixer events propagate.
#
# Usage on device (via adb):
#   adb push waved/scripts/test-mic-volume.sh /data/test-mic-volume.sh
#   adb shell sh /data/test-mic-volume.sh
#
# Pre-conditions: waved is running with the new mic_volume code; default
# TOML mic_volume=200 and JSON pdm_gain=24 already in place.

set -e

REC_DIR=/tmp/mic-volume-acceptance
mkdir -p "$REC_DIR"

# 1. Boot defaults: record 5 s, measure peak/RMS.
echo "=== test 1: boot defaults (mic_volume=200, pdm_gain=24) ==="
arecord -c 1 -r 48000 -f S16_LE -d 5 "$REC_DIR/boot.wav"
sox "$REC_DIR/boot.wav" -n stat 2>&1 | grep -E "Maximum|RMS"

# 2. Halve mic_volume via the runtime command path so the gain stage
#    sees 100 (unity). 6 dB quieter than test 1.
echo "=== test 2: mic_volume=100 (unity) ==="
amixer -c 1 cset name='PCM Capture Volume' 25%   # 25% of 0..400 = 100
sleep 1
arecord -c 1 -r 48000 -f S16_LE -d 5 "$REC_DIR/unity.wav"
sox "$REC_DIR/unity.wav" -n stat 2>&1 | grep -E "Maximum|RMS"

# 3. Max mic_volume (400). 6 dB louder than test 1.
echo "=== test 3: mic_volume=400 (max) ==="
amixer -c 1 cset name='PCM Capture Volume' 100%
sleep 1
arecord -c 1 -r 48000 -f S16_LE -d 5 "$REC_DIR/max.wav"
sox "$REC_DIR/max.wav" -n stat 2>&1 | grep -E "Maximum|RMS"

# 4. Stats sanity.
echo "=== /tmp/audio_stats.json mic_gain fields ==="
grep -E '"mic_gain_(linear|db)"' /tmp/audio_stats.json || true

echo
echo "Pass criteria:"
echo "  test 1 peak ~ -12 to -6 dBFS at normal speaking distance"
echo "  test 2 peak ~ 6 dB below test 1"
echo "  test 3 peak ~ 6 dB above test 1 (or clipped to 0 dBFS for loud sources)"
echo "  reboot test: re-run script after 'killall waved; reboot' equivalent;"
echo "    test 1 reproduces (TOML default re-applied, runtime change gone)"
```

- [ ] **Step 2: Make it executable**

```bash
chmod +x waved/scripts/test-mic-volume.sh
```

- [ ] **Step 3: Update `AUDIO_HARDWARE.md`**

In `waved/docs/AUDIO_HARDWARE.md`, add a new section near the existing "VAD Threshold" / "Maximum Lag" cluster (or under a new "Mic Gain Pipeline" heading at the top of the audio-tuning material):

```markdown
## Mic Gain Pipeline

Two stages, set independently:

1. **Hardware preamp — PDM Gain** (`amixer -c 0 ... numid=9`, range 0–48).
   Set once at daemon startup from `audio_config.capture.pdm_gain` (default
   `24`, ~+12 dB lift). Static across the run so DFN3's noise-floor
   estimator doesn't re-adapt every time the user moves the volume knob.

2. **Software gain — `apply_mic_volume`** at the end of `process_frame`.
   Linear-percent value driven by `AudioState.mic_volume`. `100` = unity,
   `200` = 2× / +6 dB, range 0..=400. Applied to `mic_mono_f32` and
   `mic_stereo_f32`, so both meeting recording and the UAC2 uplink share
   one knob.

Boot order:
```
TOML  → WavedConfig.audio.mic_volume → AudioState.mic_volume   (seeded in main)
JSON  → CaptureConfig.pdm_gain        → ALSA numid=9            (one-shot)
```

The UAC2 mixer (`hw:1 PCM Capture Volume`) maps its raw range to our
`0..=400` so the host's "max slider" corresponds to internal `mic_volume=400`,
and the boot default of `200` sits in the middle of the host's range.

If recordings come back too quiet or too clipped, **change `pdm_gain`
in the JSON config and reboot — do not change it dynamically.** Dynamic
changes were the cause of the DFN3 over-gating regression observed during
SP#5 testing.
```

- [ ] **Step 4: Commit**

```bash
git add waved/scripts/test-mic-volume.sh waved/docs/AUDIO_HARDWARE.md
git commit -m "docs(audio): mic gain pipeline + on-device acceptance script

Adds AUDIO_HARDWARE.md section explaining the two-stage gain design and
why pdm_gain is static. test-mic-volume.sh is the smoke script we run
after deploying the daemon."
```

---

## Final integration check

After Task 9, run the full waved-audio test suite to make sure the cumulative changes haven't broken anything:

```bash
cargo test -p waved-audio --lib --features capture-alsa
cargo test -p waved-state --lib
cargo build -p waved --features capture-alsa
```

Then deploy and run `scripts/test-mic-volume.sh` on the device.

If the on-device test 1 still produces a recording quieter than −20 dBFS peak with normal-distance speech, that's a signal the `pdm_gain=24` default is too low for this hardware revision. **Do not raise it dynamically.** Edit the JSON default, rebuild, redeploy. (See AUDIO_HARDWARE.md note on DFN3 adaptation.)
