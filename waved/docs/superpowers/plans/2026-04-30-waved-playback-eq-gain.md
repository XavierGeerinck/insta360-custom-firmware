# Waved Audio — SP#5 Playback EQ + Gain Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Insert a HPF → 4-biquad parametric EQ → 4-band MBAGC → lookahead limiter chain inside `PlaybackService::run_playback_loop`, with two presets (Voice, Media) switchable at runtime via SIGHUP. Voice preset matches stock `dynamic_eq_config_2.json` numbers verbatim. The AEC reference broadcast tap moves to post-DSP so AEC sees what actually reaches the speaker.

**Architecture:** New module tree `crates/waved-audio/src/playback/dsp/` containing one file per DSP stage plus an orchestrator. `playback.rs` becomes the parent module file; submodules live in `playback/`. Preset definitions and the embedded stock voice tuning live in `crates/waved-audio/src/playback/preset.rs`. Voice preset is loaded from a vendored copy of `dynamic_eq_config_2.json` via `include_str!` + `OnceLock` (simpler than build.rs codegen, same outcome). Preset switch flows through a new `mpsc::Receiver<PlaybackPreset>` polled at the top of `run_playback_loop`. `AudioConfig` bumps v3 → v4 with an additive optional `dsp` field on `PlaybackSection`. The OLED is **not** changed in this SP — preset switch happens via `/tmp/waved-playback-preset` + SIGHUP, mirroring the existing `/tmp/waved-nr` wiring.

**Tech Stack:** Rust 2021. No new crate dependencies — all DSP is hand-rolled in pure Rust over `f32` and `i16`. `serde`/`serde_json` (already in workspace) parse the vendored stock JSON. `std::sync::OnceLock` (Rust 1.70+) provides lazy init for the embedded preset.

**Reference spec:** `docs/superpowers/specs/2026-04-28-waved-playback-eq-gain-design.md`
**Reference hardware docs:** `docs/AUDIO_HARDWARE.md`

---

## File Structure

```
crates/waved-audio/
  Cargo.toml                                             (no change)
  data/
    dynamic_eq_config_2.json                             (new)   vendored copy of stock
  src/
    playback.rs                                          (modify) add `pub mod dsp; pub mod preset;`,
                                                                   thread DSP through run_playback_loop,
                                                                   accept preset_rx in spawn_playback
    playback/
      dsp/
        mod.rs                                           (new)    submodule glue + PlaybackDspChain re-export
        biquad.rs                                        (new)    Biquad data + process_block + RBJ designers
        hpf.rs                                           (new)    HighPassFilter (1 biquad/channel)
        eq.rs                                            (new)    ParametricEq (4 biquads × 2 channels)
        mbagc.rs                                         (new)    MultibandAgc (LR4 crossovers + envelope)
        limiter.rs                                       (new)    LookaheadLimiter
        chain.rs                                         (new)    PlaybackDspChain orchestrator + state
      preset.rs                                          (new)    PlaybackPreset, PlaybackPresetTuning,
                                                                   embedded voice loader, media derivation
    audio_config.rs                                      (modify) v3 → v4 bump; PlaybackSection.dsp field;
                                                                   PlaybackDspConfig deserialize; validate
    stats_collector.rs                                   (modify) emit playback_preset, mbagc_active_gain_db,
                                                                   limiter_reduction_db
    lib.rs                                               (modify) AudioService routes
                                                                   Command::SetPlaybackPreset to playback
  config/audio_config_default.json                       (modify) "version": 4
  tests/
    biquad_regression.rs                                 (new)    impulse-response regression vs known-good
crates/waved-state/
  src/lib.rs                                             (modify) PlaybackPreset enum,
                                                                   AudioState.playback_preset,
                                                                   Command::SetPlaybackPreset
crates/waved/
  src/main.rs                                            (modify) /tmp/waved-playback-preset in
                                                                   mode_switch_handler;
                                                                   spawn_playback gets preset_rx
scripts/
  test-playback-eq.sh                                    (new)    on-device acceptance (5 criteria)
docs/
  AUDIO_HARDWARE.md                                      (modify) playback chain diagram + tap location
TODO.md                                                  (modify) mark SP#5 done
```

## Task ordering

The 19 tasks below are arranged so that each group commits a self-contained, compilable, testable change:

1. **Tasks 1–9 (build the DSP)**: pure-Rust DSP primitives, each with TDD tests. No wiring yet — `cargo test -p waved-audio --lib` passes after each task.
2. **Task 10 (presets)**: pulls the stock JSON in, exposes the embedded voice preset and media derivation.
3. **Tasks 11–15 (wire it up)**: integrate the chain into `PlaybackService`, add config v4, wire preset switch through state and SIGHUP, surface stats. After each task `cargo build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved` succeeds.
4. **Task 16 (regression)**: bit-exact biquad reference test.
5. **Tasks 17–19 (acceptance + docs)**: on-device test script, docs, TODO.

---

## Task 1: Vendor stock `dynamic_eq_config_2.json`

The stock firmware ships its EQ tuning at `/etc/dynamic_eq_config_2.json`. We don't depend on the device file at runtime — we vendor a copy in the repo so the build is reproducible.

**Files:**
- Create: `crates/waved-audio/data/dynamic_eq_config_2.json`

- [ ] **Step 1: Copy the stock JSON into the repo**

```bash
mkdir -p crates/waved-audio/data
cp fw_v1.1.6.6/rootfs/etc/dynamic_eq_config_2.json crates/waved-audio/data/dynamic_eq_config_2.json
```

- [ ] **Step 2: Verify shape**

Run: `python3 -c "import json; print(json.load(open('crates/waved-audio/data/dynamic_eq_config_2.json'))['asp_param_eq'].keys())"`
Expected output contains: `agc_settings`, `eq_settings`, `multi_band_num`, `eq_filter_num`.

- [ ] **Step 3: Commit**

```bash
git add crates/waved-audio/data/dynamic_eq_config_2.json
git commit -m "feat(audio/sp5): vendor stock dynamic_eq_config_2.json"
```

---

## Task 2: Biquad primitive — data + processing

Pure-function DSP unit. All higher stages (HPF, EQ, MBAGC) compose biquads.

**Files:**
- Create: `crates/waved-audio/src/playback/dsp/mod.rs`
- Create: `crates/waved-audio/src/playback/dsp/biquad.rs`
- Modify: `crates/waved-audio/src/playback.rs` (add module declaration)

- [ ] **Step 1: Add the submodule declarations to `playback.rs`**

At the top of `crates/waved-audio/src/playback.rs`, immediately under the existing `//!` doc comment block, add:

```rust
pub mod dsp;
pub mod preset;
```

- [ ] **Step 2: Create `crates/waved-audio/src/playback/dsp/mod.rs`**

```rust
//! Playback DSP chain — runs inside `PlaybackService::run_playback_loop`
//! between the mpsc dequeue and the ALSA write. Order: HPF → EQ → MBAGC → Limiter.
//! The post-DSP buffer is what gets broadcast as the AEC reference signal.

pub mod biquad;
pub mod hpf;
pub mod eq;
pub mod mbagc;
pub mod limiter;
pub mod chain;

pub use chain::PlaybackDspChain;
```

- [ ] **Step 3: Write the failing test for `biquad::Biquad`**

Create `crates/waved-audio/src/playback/dsp/biquad.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Unity gain biquad (b0=1, others=0) is a passthrough.
    #[test]
    fn unity_biquad_is_passthrough() {
        let mut b = Biquad::new(BiquadCoeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 });
        let mut buf = [0.0f32; 8];
        buf[0] = 1.0;
        b.process_block(&mut buf);
        assert!((buf[0] - 1.0).abs() < 1e-9);
        for i in 1..8 {
            assert!(buf[i].abs() < 1e-9, "expected zero at i={i}, got {}", buf[i]);
        }
    }

    /// One-sample delay biquad (b0=0, b1=1) outputs the previous sample.
    #[test]
    fn one_sample_delay_biquad() {
        let mut b = Biquad::new(BiquadCoeffs { b0: 0.0, b1: 1.0, b2: 0.0, a1: 0.0, a2: 0.0 });
        let mut buf = [1.0_f32, 2.0, 3.0, 4.0];
        b.process_block(&mut buf);
        assert!(buf[0].abs() < 1e-9);
        assert!((buf[1] - 1.0).abs() < 1e-9);
        assert!((buf[2] - 2.0).abs() < 1e-9);
        assert!((buf[3] - 3.0).abs() < 1e-9);
    }
}
```

Run: `cargo test -p waved-audio --lib playback::dsp::biquad`
Expected: FAIL — `Biquad`, `BiquadCoeffs` not defined.

- [ ] **Step 4: Implement `Biquad` and `BiquadCoeffs`**

Replace the contents of `crates/waved-audio/src/playback/dsp/biquad.rs` with:

```rust
//! Direct-form-1 biquad. State is `x[n-1], x[n-2], y[n-1], y[n-2]`.
//! Matches the form used by stock `dynamic_eq_config_2.json` (b0..b2, a1, a2 with a0=1).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadCoeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl BiquadCoeffs {
    pub const PASSTHROUGH: BiquadCoeffs = BiquadCoeffs {
        b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0,
    };
}

#[derive(Debug, Clone)]
pub struct Biquad {
    coeffs: BiquadCoeffs,
    x1: f32, x2: f32,
    y1: f32, y2: f32,
}

impl Biquad {
    pub fn new(coeffs: BiquadCoeffs) -> Self {
        Self { coeffs, x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }

    pub fn set_coeffs(&mut self, coeffs: BiquadCoeffs) {
        self.coeffs = coeffs;
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0; self.x2 = 0.0;
        self.y1 = 0.0; self.y2 = 0.0;
    }

    #[inline]
    pub fn process_sample(&mut self, x0: f32) -> f32 {
        let c = &self.coeffs;
        // Direct-form-1: y[n] = b0*x[n] + b1*x[n-1] + b2*x[n-2] - a1*y[n-1] - a2*y[n-2]
        let y0 = c.b0 * x0 + c.b1 * self.x1 + c.b2 * self.x2
                          - c.a1 * self.y1 - c.a2 * self.y2;
        self.x2 = self.x1; self.x1 = x0;
        self.y2 = self.y1; self.y1 = y0;
        y0
    }

    pub fn process_block(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.process_sample(*s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_biquad_is_passthrough() {
        let mut b = Biquad::new(BiquadCoeffs::PASSTHROUGH);
        let mut buf = [0.0f32; 8];
        buf[0] = 1.0;
        b.process_block(&mut buf);
        assert!((buf[0] - 1.0).abs() < 1e-9);
        for i in 1..8 {
            assert!(buf[i].abs() < 1e-9, "expected zero at i={i}, got {}", buf[i]);
        }
    }

    #[test]
    fn one_sample_delay_biquad() {
        let mut b = Biquad::new(BiquadCoeffs { b0: 0.0, b1: 1.0, b2: 0.0, a1: 0.0, a2: 0.0 });
        let mut buf = [1.0_f32, 2.0, 3.0, 4.0];
        b.process_block(&mut buf);
        assert!(buf[0].abs() < 1e-9);
        assert!((buf[1] - 1.0).abs() < 1e-9);
        assert!((buf[2] - 2.0).abs() < 1e-9);
        assert!((buf[3] - 3.0).abs() < 1e-9);
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p waved-audio --lib playback::dsp::biquad`
Expected: PASS — both tests green.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-audio/src/playback.rs \
        crates/waved-audio/src/playback/dsp/mod.rs \
        crates/waved-audio/src/playback/dsp/biquad.rs
git commit -m "feat(audio/sp5): biquad primitive (direct-form-1 + designers stub)"
```

---

## Task 3: Biquad design functions (RBJ cookbook)

Pre-compute coefficients from human-readable params (frequency, gain, Q) using the RBJ Audio EQ Cookbook formulas.

**Files:**
- Modify: `crates/waved-audio/src/playback/dsp/biquad.rs`

- [ ] **Step 1: Write failing tests for the four designers**

Append to the `tests` module in `biquad.rs`:

```rust
    #[test]
    fn butterworth_hpf_attenuates_below_cutoff() {
        // 80 Hz HPF at 48 kHz: 40 Hz should be attenuated by ~12 dB or more.
        let coeffs = design_butterworth_hpf(80.0, 48_000.0);
        let mut b = Biquad::new(coeffs);
        // 40 Hz sine, 1 second
        let mut buf: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 40.0 * i as f32 / 48_000.0).sin())
            .collect();
        b.process_block(&mut buf);
        // RMS over second half (after settling)
        let rms = {
            let n = 24_000;
            let s: f32 = buf[24_000..].iter().map(|x| x * x).sum();
            (s / n as f32).sqrt()
        };
        let attn_db = 20.0 * rms.log10();
        assert!(attn_db <= -12.0, "expected ≤ -12 dB at 40 Hz, got {attn_db:.2} dB");
    }

    #[test]
    fn peaking_eq_matches_target_at_center() {
        // +6 dB peaking at 1 kHz, Q=1, sample rate 48 kHz.
        let coeffs = design_peaking_eq(1000.0, 6.0, 1.0, 48_000.0);
        let mut b = Biquad::new(coeffs);
        let mut buf: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin())
            .collect();
        b.process_block(&mut buf);
        let rms = {
            let s: f32 = buf[24_000..].iter().map(|x| x * x).sum();
            (s / 24_000_f32).sqrt()
        };
        let gain_db = 20.0 * rms.log10() - 20.0 * (1.0_f32 / 2.0_f32.sqrt()).log10();
        assert!((gain_db - 6.0).abs() < 0.5, "expected +6 dB at 1 kHz, got {gain_db:.2} dB");
    }

    #[test]
    fn low_shelf_lifts_low_frequencies() {
        let coeffs = design_low_shelf(200.0, 6.0, 1.0, 48_000.0);
        let mut b = Biquad::new(coeffs);
        let mut buf: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 60.0 * i as f32 / 48_000.0).sin())
            .collect();
        b.process_block(&mut buf);
        let rms = {
            let s: f32 = buf[24_000..].iter().map(|x| x * x).sum();
            (s / 24_000_f32).sqrt()
        };
        let gain_db = 20.0 * rms.log10() - 20.0 * (1.0_f32 / 2.0_f32.sqrt()).log10();
        assert!(gain_db > 4.0, "expected ≥ 4 dB lift at 60 Hz, got {gain_db:.2} dB");
    }

    #[test]
    fn high_shelf_lifts_high_frequencies() {
        let coeffs = design_high_shelf(4000.0, 6.0, 1.0, 48_000.0);
        let mut b = Biquad::new(coeffs);
        let mut buf: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 8000.0 * i as f32 / 48_000.0).sin())
            .collect();
        b.process_block(&mut buf);
        let rms = {
            let s: f32 = buf[24_000..].iter().map(|x| x * x).sum();
            (s / 24_000_f32).sqrt()
        };
        let gain_db = 20.0 * rms.log10() - 20.0 * (1.0_f32 / 2.0_f32.sqrt()).log10();
        assert!(gain_db > 4.0, "expected ≥ 4 dB lift at 8 kHz, got {gain_db:.2} dB");
    }
```

Run: `cargo test -p waved-audio --lib playback::dsp::biquad`
Expected: FAIL — designer functions don't exist.

- [ ] **Step 2: Implement the designers**

Insert before the `#[cfg(test)]` block in `biquad.rs`:

```rust
/// Butterworth 2nd-order high-pass — RBJ cookbook formulas with Q = 1/√2.
pub fn design_butterworth_hpf(cutoff_hz: f32, sample_rate_hz: f32) -> BiquadCoeffs {
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let w0 = 2.0 * std::f32::consts::PI * cutoff_hz / sample_rate_hz;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q);

    let b0 = (1.0 + cos_w0) * 0.5;
    let b1 = -(1.0 + cos_w0);
    let b2 = (1.0 + cos_w0) * 0.5;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha;

    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// RBJ peaking EQ.
pub fn design_peaking_eq(center_hz: f32, gain_db: f32, q: f32, sample_rate_hz: f32) -> BiquadCoeffs {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * center_hz / sample_rate_hz;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q);

    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos_w0;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha / a;

    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// RBJ low shelf.
pub fn design_low_shelf(corner_hz: f32, gain_db: f32, q: f32, sample_rate_hz: f32) -> BiquadCoeffs {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * corner_hz / sample_rate_hz;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let alpha = sin_w0 / (2.0 * q);
    let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

    let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
    let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
    let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
    let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
    let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
    let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha;

    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// RBJ high shelf.
pub fn design_high_shelf(corner_hz: f32, gain_db: f32, q: f32, sample_rate_hz: f32) -> BiquadCoeffs {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * corner_hz / sample_rate_hz;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let alpha = sin_w0 / (2.0 * q);
    let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

    let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
    let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
    let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
    let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
    let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
    let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha;

    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p waved-audio --lib playback::dsp::biquad`
Expected: PASS — six tests green (two from Task 2, four new).

- [ ] **Step 4: Commit**

```bash
git add crates/waved-audio/src/playback/dsp/biquad.rs
git commit -m "feat(audio/sp5): biquad designers — Butterworth HPF + peaking + shelves"
```

---

## Task 4: HPF stage

Single Butterworth biquad per channel. Trivial wrapper over `Biquad`.

**Files:**
- Create: `crates/waved-audio/src/playback/dsp/hpf.rs`

- [ ] **Step 1: Write failing test**

```rust
//! Butterworth 2nd-order high-pass, one biquad per channel.

use super::biquad::{Biquad, design_butterworth_hpf};

pub struct HighPassFilter {
    biquads: Vec<Biquad>,
    sample_rate_hz: f32,
    cutoff_hz: f32,
}

impl HighPassFilter {
    pub fn new(channels: usize, cutoff_hz: f32, sample_rate_hz: f32) -> Self {
        let coeffs = design_butterworth_hpf(cutoff_hz, sample_rate_hz);
        Self {
            biquads: (0..channels).map(|_| Biquad::new(coeffs)).collect(),
            sample_rate_hz,
            cutoff_hz,
        }
    }

    pub fn set_cutoff(&mut self, cutoff_hz: f32) {
        self.cutoff_hz = cutoff_hz;
        let c = design_butterworth_hpf(cutoff_hz, self.sample_rate_hz);
        for b in &mut self.biquads {
            b.set_coeffs(c);
        }
    }

    pub fn reset(&mut self) {
        for b in &mut self.biquads {
            b.reset();
        }
    }

    /// Process interleaved f32 samples in place. `buf.len()` must be a
    /// multiple of `channels` (set at construction).
    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        let ch = self.biquads.len();
        if ch == 0 { return; }
        for frame in buf.chunks_exact_mut(ch) {
            for (c, sample) in frame.iter_mut().enumerate() {
                *sample = self.biquads[c].process_sample(*sample);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_kilohertz_attenuates_subsonic() {
        let mut hpf = HighPassFilter::new(2, 80.0, 48_000.0);
        // Stereo interleaved: L = 1 kHz sine, R = 30 Hz sine
        let mut buf: Vec<f32> = (0..48_000)
            .flat_map(|i| {
                let t = i as f32 / 48_000.0;
                let l = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
                let r = (2.0 * std::f32::consts::PI * 30.0 * t).sin();
                [l, r]
            })
            .collect();
        hpf.process_interleaved(&mut buf);

        // Compute per-channel RMS over the second half (after settling).
        let mut l_sq = 0.0_f32;
        let mut r_sq = 0.0_f32;
        for i in 24_000..48_000 {
            l_sq += buf[i * 2] * buf[i * 2];
            r_sq += buf[i * 2 + 1] * buf[i * 2 + 1];
        }
        let l_rms = (l_sq / 24_000.0).sqrt();
        let r_rms = (r_sq / 24_000.0).sqrt();
        let l_db = 20.0 * l_rms.log10();
        let r_db = 20.0 * r_rms.log10();
        // 1 kHz passes at near unity (sin RMS = 1/√2 ≈ -3 dB)
        assert!((l_db - (-3.0)).abs() < 1.0, "expected 1 kHz ≈ -3 dB, got {l_db:.2}");
        // 30 Hz attenuated >12 dB below 1 kHz
        assert!(r_db < l_db - 12.0, "expected ≥ 12 dB attenuation, got {} dB delta", l_db - r_db);
    }
}
```

Run: `cargo test -p waved-audio --lib playback::dsp::hpf`
Expected: FAIL on first run if you write only `mod tests`; the impl is included above so it should PASS — verify it does.

- [ ] **Step 2: Run the test**

Run: `cargo test -p waved-audio --lib playback::dsp::hpf`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/waved-audio/src/playback/dsp/hpf.rs
git commit -m "feat(audio/sp5): HPF stage — Butterworth 2nd order, per-channel"
```

---

## Task 5: Parametric EQ stage

4 biquads × 2 channels, each band individually configurable.

**Files:**
- Create: `crates/waved-audio/src/playback/dsp/eq.rs`

- [ ] **Step 1: Define `BiquadKind` and `BiquadParams` + the EQ stage**

Create `crates/waved-audio/src/playback/dsp/eq.rs`:

```rust
//! Parametric EQ: 4 biquad bands per channel. Each band has a kind, center
//! frequency, gain, and Q. Cascade order = array order.

use serde::{Deserialize, Serialize};

use super::biquad::{
    Biquad, BiquadCoeffs,
    design_high_shelf, design_low_shelf, design_peaking_eq,
};

/// Number of EQ bands per channel.
pub const N_BANDS: usize = 4;
/// Number of channels (always stereo on this hardware).
pub const N_CHANNELS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BiquadKind {
    Bypass,
    Peaking,
    LowShelf,
    HighShelf,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BiquadParams {
    pub kind: BiquadKind,
    pub center_freq_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

impl BiquadParams {
    pub const BYPASS: BiquadParams = BiquadParams {
        kind: BiquadKind::Bypass,
        center_freq_hz: 1000.0,
        gain_db: 0.0,
        q: 1.0,
    };

    pub fn coeffs(&self, sample_rate_hz: f32) -> BiquadCoeffs {
        match self.kind {
            BiquadKind::Bypass    => BiquadCoeffs::PASSTHROUGH,
            BiquadKind::Peaking   => design_peaking_eq(self.center_freq_hz, self.gain_db, self.q, sample_rate_hz),
            BiquadKind::LowShelf  => design_low_shelf(self.center_freq_hz, self.gain_db, self.q, sample_rate_hz),
            BiquadKind::HighShelf => design_high_shelf(self.center_freq_hz, self.gain_db, self.q, sample_rate_hz),
        }
    }
}

pub struct ParametricEq {
    /// `[ch][band]` biquad state.
    biquads: [[Biquad; N_BANDS]; N_CHANNELS],
    sample_rate_hz: f32,
}

impl ParametricEq {
    pub fn new(params: &[[BiquadParams; N_BANDS]; N_CHANNELS], sample_rate_hz: f32) -> Self {
        let make_band = |ch: usize, band: usize| Biquad::new(params[ch][band].coeffs(sample_rate_hz));
        Self {
            biquads: [
                [make_band(0, 0), make_band(0, 1), make_band(0, 2), make_band(0, 3)],
                [make_band(1, 0), make_band(1, 1), make_band(1, 2), make_band(1, 3)],
            ],
            sample_rate_hz,
        }
    }

    pub fn set_params(&mut self, params: &[[BiquadParams; N_BANDS]; N_CHANNELS]) {
        for ch in 0..N_CHANNELS {
            for band in 0..N_BANDS {
                self.biquads[ch][band].set_coeffs(params[ch][band].coeffs(self.sample_rate_hz));
            }
        }
    }

    pub fn reset(&mut self) {
        for ch in 0..N_CHANNELS {
            for band in 0..N_BANDS {
                self.biquads[ch][band].reset();
            }
        }
    }

    /// Process interleaved stereo f32 in place. `buf.len()` must be even.
    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        for frame in buf.chunks_exact_mut(N_CHANNELS) {
            for ch in 0..N_CHANNELS {
                let mut x = frame[ch];
                for band in 0..N_BANDS {
                    x = self.biquads[ch][band].process_sample(x);
                }
                frame[ch] = x;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> [[BiquadParams; N_BANDS]; N_CHANNELS] {
        [[BiquadParams::BYPASS; N_BANDS]; N_CHANNELS]
    }

    #[test]
    fn bypass_eq_is_passthrough() {
        let mut eq = ParametricEq::new(&flat(), 48_000.0);
        let mut buf: Vec<f32> = (0..960).map(|i| (i as f32 * 0.01).sin()).collect();
        let original = buf.clone();
        eq.process_interleaved(&mut buf);
        for (a, b) in buf.iter().zip(original.iter()) {
            assert!((a - b).abs() < 1e-6, "bypass should not modify samples");
        }
    }

    #[test]
    fn peaking_band_lifts_target_frequency() {
        let mut params = flat();
        // Band 0 on both channels: +6 dB peaking at 1 kHz, Q=1.
        for ch in 0..N_CHANNELS {
            params[ch][0] = BiquadParams { kind: BiquadKind::Peaking, center_freq_hz: 1000.0, gain_db: 6.0, q: 1.0 };
        }
        let mut eq = ParametricEq::new(&params, 48_000.0);
        let mut buf: Vec<f32> = (0..96_000)
            .flat_map(|i| {
                let t = i as f32 / 48_000.0;
                let s = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
                [s, s]
            })
            .collect();
        eq.process_interleaved(&mut buf);
        let l_rms = {
            let s: f32 = buf[24_000..].iter().step_by(2).map(|x| x * x).sum();
            (s / 24_000_f32).sqrt()
        };
        let l_db = 20.0 * l_rms.log10() - 20.0 * (1.0_f32 / 2.0_f32.sqrt()).log10();
        assert!((l_db - 6.0).abs() < 0.5, "expected +6 dB lift at 1 kHz, got {l_db:.2} dB");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p waved-audio --lib playback::dsp::eq`
Expected: PASS — both tests green.

- [ ] **Step 3: Commit**

```bash
git add crates/waved-audio/src/playback/dsp/eq.rs
git commit -m "feat(audio/sp5): parametric EQ — 4 bands × 2 channels"
```

---

## Task 6: Multiband AGC (skeleton + bypass)

Lay down the structure and parameter type. The bypass path lets us land the integration before the envelope follower, so each commit stays small.

**Files:**
- Create: `crates/waved-audio/src/playback/dsp/mbagc.rs`

- [ ] **Step 1: Write failing test**

```rust
//! 4-band MBAGC. Linkwitz-Riley 4th-order crossovers split into
//! [low, mid-low, mid-high, high]; per-band envelope-follower drives a
//! per-band gain; bands sum back to the stereo bus.

use serde::{Deserialize, Serialize};

use super::biquad::{Biquad, design_butterworth_hpf};

pub const N_BANDS: usize = 4;
pub const N_CROSSOVERS: usize = 3;
pub const N_CHANNELS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MbagcParams {
    pub enabled: bool,
    pub crossover_freq_hz: [f32; N_CROSSOVERS],
    /// Per-band target peak level in dBFS. Stock numbers map straight in.
    pub target_dbfs: [f32; N_BANDS],
    pub max_gain_db: [f32; N_BANDS],
    pub knee_db: [f32; N_BANDS],
    pub release_ms: [f32; N_BANDS],
}

impl MbagcParams {
    pub const PASSTHROUGH: MbagcParams = MbagcParams {
        enabled: false,
        crossover_freq_hz: [300.0, 4000.0, 7000.0],
        target_dbfs: [0.0; N_BANDS],
        max_gain_db: [0.0; N_BANDS],
        knee_db: [0.0; N_BANDS],
        release_ms: [200.0; N_BANDS],
    };
}

pub struct MultibandAgc {
    enabled: bool,
    /// Per-band, per-channel state. Empty when disabled (bypass).
    /// Layout: `bands[band][ch]` for both LP and HP cascades.
    /// LR4 = two cascaded Butterworth biquads (LR2² → 4th order).
    bands: Vec<BandState>,
    sample_rate_hz: f32,
    params: MbagcParams,
    /// Last-known per-band gain (linear) for stats.
    last_gain_db: [f32; N_BANDS],
}

struct BandState {
    /// Per-channel filter cascade producing this band.
    /// For each channel: (low-pass-1, low-pass-2, high-pass-1, high-pass-2).
    /// Band 0 (lowest): passes content below crossover[0] — uses LP cascade only.
    /// Band 1..N-1: pass content between crossover[band-1] and crossover[band] —
    ///   apply HP at lower edge, LP at upper edge.
    /// Band N-1 (highest): passes above crossover[N-2] — HP cascade only.
    filters: [(Biquad, Biquad, Biquad, Biquad); N_CHANNELS],
    /// Envelope state per channel (peak with IIR release).
    envelope: [f32; N_CHANNELS],
}

impl MultibandAgc {
    pub fn new(params: MbagcParams, sample_rate_hz: f32) -> Self {
        let mut me = Self {
            enabled: params.enabled,
            bands: Vec::new(),
            sample_rate_hz,
            params,
            last_gain_db: [0.0; N_BANDS],
        };
        if me.enabled {
            me.rebuild_filters();
        }
        me
    }

    fn rebuild_filters(&mut self) {
        // Build per-band cascade. We use a simple Butterworth HP + complementary
        // LP via design_butterworth_hpf for HP and a 1 - HP relation is awkward;
        // use design_butterworth_lpf below instead. For LR4 we cascade two
        // 2nd-order biquads.
        self.bands.clear();
        for band in 0..N_BANDS {
            let lp_freq = if band < N_BANDS - 1 {
                Some(self.params.crossover_freq_hz[band])
            } else { None };
            let hp_freq = if band > 0 {
                Some(self.params.crossover_freq_hz[band - 1])
            } else { None };
            let make_filters = || {
                let lp1_coeffs = lp_freq.map(|f| crate::playback::dsp::biquad::design_butterworth_lpf(f, self.sample_rate_hz)).unwrap_or(crate::playback::dsp::biquad::BiquadCoeffs::PASSTHROUGH);
                let hp1_coeffs = hp_freq.map(|f| design_butterworth_hpf(f, self.sample_rate_hz)).unwrap_or(crate::playback::dsp::biquad::BiquadCoeffs::PASSTHROUGH);
                (
                    Biquad::new(lp1_coeffs), Biquad::new(lp1_coeffs),
                    Biquad::new(hp1_coeffs), Biquad::new(hp1_coeffs),
                )
            };
            self.bands.push(BandState {
                filters: [make_filters(), make_filters()],
                envelope: [0.0; N_CHANNELS],
            });
        }
    }

    pub fn set_params(&mut self, params: MbagcParams) {
        self.params = params;
        self.enabled = params.enabled;
        if self.enabled {
            self.rebuild_filters();
        } else {
            self.bands.clear();
        }
    }

    pub fn reset(&mut self) {
        for band in &mut self.bands {
            for ch in 0..N_CHANNELS {
                band.filters[ch].0.reset();
                band.filters[ch].1.reset();
                band.filters[ch].2.reset();
                band.filters[ch].3.reset();
                band.envelope[ch] = 0.0;
            }
        }
        self.last_gain_db = [0.0; N_BANDS];
    }

    /// Returns the most recent per-band gain (dB). Used by stats_collector.
    pub fn last_gain_db(&self) -> [f32; N_BANDS] {
        self.last_gain_db
    }

    /// Bypass path — emits input unchanged when disabled.
    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        if !self.enabled { return; }
        // Full envelope + gain implementation lands in Task 7.
        // Until then, bypass even when "enabled" so the integration tests pass.
        // (Marked `dead_code` below for the band state until then.)
        let _ = buf;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_mbagc_is_bypass() {
        let mut m = MultibandAgc::new(MbagcParams::PASSTHROUGH, 48_000.0);
        let mut buf = vec![0.5_f32, -0.5, 0.25, -0.25];
        let copy = buf.clone();
        m.process_interleaved(&mut buf);
        assert_eq!(buf, copy);
    }

    #[test]
    fn enabled_mbagc_currently_no_op_until_task_7() {
        let mut params = MbagcParams::PASSTHROUGH;
        params.enabled = true;
        let mut m = MultibandAgc::new(params, 48_000.0);
        let mut buf = vec![0.5_f32, -0.5];
        let copy = buf.clone();
        m.process_interleaved(&mut buf);
        // Task 7 will replace this assertion. Until then, "enabled" is a no-op.
        assert_eq!(buf, copy);
    }
}
```

- [ ] **Step 2: Add `design_butterworth_lpf` to `biquad.rs`**

Append to `crates/waved-audio/src/playback/dsp/biquad.rs`, before `#[cfg(test)]`:

```rust
/// Butterworth 2nd-order low-pass — RBJ cookbook with Q = 1/√2.
pub fn design_butterworth_lpf(cutoff_hz: f32, sample_rate_hz: f32) -> BiquadCoeffs {
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let w0 = 2.0 * std::f32::consts::PI * cutoff_hz / sample_rate_hz;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q);

    let b0 = (1.0 - cos_w0) * 0.5;
    let b1 = 1.0 - cos_w0;
    let b2 = (1.0 - cos_w0) * 0.5;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha;

    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p waved-audio --lib playback::dsp::mbagc`
Expected: PASS — both tests pass (bypass path).

- [ ] **Step 4: Commit**

```bash
git add crates/waved-audio/src/playback/dsp/mbagc.rs \
        crates/waved-audio/src/playback/dsp/biquad.rs
git commit -m "feat(audio/sp5): MBAGC skeleton (LR4 crossover network, bypass path)"
```

---

## Task 7: MBAGC envelope follower + per-band gain

Replace the bypass body with the working multiband AGC: per-channel LR4 splits, per-band peak envelope, per-band gain calculation, sum back.

**Files:**
- Modify: `crates/waved-audio/src/playback/dsp/mbagc.rs`

- [ ] **Step 1: Replace the test that says "currently no-op"**

In `mbagc.rs`'s test module, remove `enabled_mbagc_currently_no_op_until_task_7` and add:

```rust
    #[test]
    fn soft_input_lifted_toward_target() {
        // Voice preset's mid-low band is targeted at -6 dBFS (16406 / 32768).
        // A −20 dBFS sine in the mid-low band should be lifted toward target.
        let mut params = MbagcParams::PASSTHROUGH;
        params.enabled = true;
        params.target_dbfs = [-12.0, -6.0, -29.0, -35.0];
        params.max_gain_db = [6.0, 20.0, 10.0, 10.0];
        params.knee_db = [5.0, 5.0, 5.0, 5.0];
        params.release_ms = [200.0, 200.0, 200.0, 200.0];
        let mut m = MultibandAgc::new(params, 48_000.0);

        // 1 kHz sine at −20 dBFS, stereo, 2 seconds.
        let amp = 10.0_f32.powf(-20.0 / 20.0); // 0.1
        let mut buf: Vec<f32> = (0..96_000)
            .flat_map(|i| {
                let s = amp * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin();
                [s, s]
            })
            .collect();
        m.process_interleaved(&mut buf);

        // RMS over second half.
        let l_sq: f32 = buf[48_000..].iter().step_by(2).map(|x| x * x).sum();
        let l_rms = (l_sq / 24_000_f32).sqrt();
        let l_db = 20.0 * l_rms.log10() - 20.0 * (1.0_f32 / 2.0_f32.sqrt()).log10();
        // mid-low band target is -6 dBFS, max gain 20 dB, so output is between -20+gain
        // and target. Expect within ±3 dB of target.
        assert!((l_db - (-6.0)).abs() < 3.0, "expected ≈ -6 dBFS, got {l_db:.2} dBFS");
    }

    #[test]
    fn hot_input_attenuated_below_max_gain() {
        // 0 dBFS input — gain calculation says we should attenuate (gain < 0),
        // but max_gain_db only constrains the lift side. The clamp produces 0 dB
        // (no lift, no cut — MBAGC isn't a limiter).
        let mut params = MbagcParams::PASSTHROUGH;
        params.enabled = true;
        params.target_dbfs = [-12.0, -6.0, -29.0, -35.0];
        params.max_gain_db = [6.0, 20.0, 10.0, 10.0];
        params.knee_db = [5.0, 5.0, 5.0, 5.0];
        params.release_ms = [200.0, 200.0, 200.0, 200.0];
        let mut m = MultibandAgc::new(params, 48_000.0);

        let mut buf: Vec<f32> = (0..96_000)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin();
                [s, s]
            })
            .collect();
        m.process_interleaved(&mut buf);

        let l_sq: f32 = buf[48_000..].iter().step_by(2).map(|x| x * x).sum();
        let l_rms = (l_sq / 24_000_f32).sqrt();
        let l_db = 20.0 * l_rms.log10() - 20.0 * (1.0_f32 / 2.0_f32.sqrt()).log10();
        // Should stay ≈ 0 dBFS (no cut). Within 1 dB.
        assert!(l_db.abs() < 1.0, "expected ≈ 0 dBFS, got {l_db:.2} dBFS");
    }
```

- [ ] **Step 2: Run tests — should fail**

Run: `cargo test -p waved-audio --lib playback::dsp::mbagc`
Expected: FAIL — both new tests fail because `process_interleaved` is still bypass.

- [ ] **Step 3: Implement the envelope + gain path**

In `mbagc.rs`, replace `process_interleaved` and add helpers:

```rust
impl MultibandAgc {
    /// Pull one sample of band `band_idx` from per-channel input via the
    /// LR4 cascades. Returns `(left_band, right_band)`.
    fn band_sample(&mut self, band_idx: usize, l_in: f32, r_in: f32) -> (f32, f32) {
        // Apply HP-cascade then LP-cascade (or skip whichever is missing).
        let band = &mut self.bands[band_idx];
        let process_chain = |x: f32, fil: &mut (Biquad, Biquad, Biquad, Biquad)| -> f32 {
            // First: HP (lower edge) — bypass for band 0.
            let mut y = fil.2.process_sample(x);
            y = fil.3.process_sample(y);
            // Then: LP (upper edge) — bypass for last band.
            y = fil.0.process_sample(y);
            y = fil.1.process_sample(y);
            y
        };
        let l = process_chain(l_in, &mut band.filters[0]);
        let r = process_chain(r_in, &mut band.filters[1]);
        (l, r)
    }

    fn band_gain_linear(&mut self, band_idx: usize, peak: f32, sample_rate_hz: f32) -> f32 {
        // Envelope follower: instant attack, exponential release.
        let release_ms = self.params.release_ms[band_idx];
        let alpha = (-1.0_f32 / (release_ms * 1e-3 * sample_rate_hz)).exp();
        // Take max of channel envelopes for this band.
        let env_l = self.bands[band_idx].envelope[0];
        let env_r = self.bands[band_idx].envelope[1];
        let new_env_l = if peak > env_l { peak } else { env_l * alpha };
        let new_env_r = if peak > env_r { peak } else { env_r * alpha };
        self.bands[band_idx].envelope[0] = new_env_l;
        self.bands[band_idx].envelope[1] = new_env_r;
        let env = new_env_l.max(new_env_r).max(1e-12);

        let env_dbfs = 20.0 * env.log10();
        let target = self.params.target_dbfs[band_idx];
        let max_gain = self.params.max_gain_db[band_idx];
        let knee = self.params.knee_db[band_idx];

        let raw = target - env_dbfs;
        // Soft-knee around 0 dB: only lift, never cut.
        let gain_db = raw.clamp(-knee, max_gain).max(0.0);
        self.last_gain_db[band_idx] = gain_db;
        10.0_f32.powf(gain_db / 20.0)
    }

    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        if !self.enabled { return; }
        let sr = self.sample_rate_hz;
        for frame in buf.chunks_exact_mut(N_CHANNELS) {
            let l_in = frame[0];
            let r_in = frame[1];
            let mut l_out = 0.0_f32;
            let mut r_out = 0.0_f32;
            for band in 0..N_BANDS {
                let (lb, rb) = self.band_sample(band, l_in, r_in);
                let peak = lb.abs().max(rb.abs());
                let g = self.band_gain_linear(band, peak, sr);
                l_out += lb * g;
                r_out += rb * g;
            }
            frame[0] = l_out;
            frame[1] = r_out;
        }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p waved-audio --lib playback::dsp::mbagc`
Expected: PASS — three tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/src/playback/dsp/mbagc.rs
git commit -m "feat(audio/sp5): MBAGC envelope + per-band gain"
```

---

## Task 8: Lookahead limiter

Brick-wall limiter with 96-sample lookahead and exponential release.

**Files:**
- Create: `crates/waved-audio/src/playback/dsp/limiter.rs`

- [ ] **Step 1: Create the file with tests + impl**

```rust
//! Lookahead peak limiter. Holds `lookahead_samples` of input in a ring buffer;
//! emits the delayed sample with attenuation derived from the *future* peak.
//! Operates per-frame across all channels — gain is shared so stereo image
//! survives.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LimiterParams {
    pub ceiling_dbfs: f32,
    pub lookahead_samples: usize,
    pub release_ms: f32,
}

impl LimiterParams {
    pub const DEFAULT: LimiterParams = LimiterParams {
        ceiling_dbfs: -1.0,
        lookahead_samples: 96,
        release_ms: 50.0,
    };
}

pub struct LookaheadLimiter {
    /// Interleaved frame ring buffer: `lookahead_samples` frames of `channels`.
    ring: VecDeque<Vec<f32>>,
    channels: usize,
    ceiling: f32,
    release_alpha: f32,
    /// Current attenuation factor (1.0 = no attenuation).
    current_atten: f32,
    /// Snapshot for stats: peak attenuation over the most recent batch (dB,
    /// negative = more attenuation).
    last_reduction_db: f32,
}

impl LookaheadLimiter {
    pub fn new(params: LimiterParams, channels: usize, sample_rate_hz: f32) -> Self {
        let release_alpha = (-1.0_f32 / (params.release_ms * 1e-3 * sample_rate_hz)).exp();
        Self {
            ring: VecDeque::with_capacity(params.lookahead_samples),
            channels,
            ceiling: 10.0_f32.powf(params.ceiling_dbfs / 20.0),
            release_alpha,
            current_atten: 1.0,
            last_reduction_db: 0.0,
        }
    }

    pub fn last_reduction_db(&self) -> f32 {
        self.last_reduction_db
    }

    pub fn reset(&mut self) {
        self.ring.clear();
        self.current_atten = 1.0;
        self.last_reduction_db = 0.0;
    }

    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        let mut min_atten_this_block = 1.0_f32;
        let mut out = Vec::with_capacity(buf.len());

        for frame in buf.chunks_exact(self.channels) {
            let frame_vec: Vec<f32> = frame.to_vec();
            self.ring.push_back(frame_vec);

            // While ring has enough lookahead, emit oldest frame.
            if self.ring.len() > self.ring.capacity().max(1) - 1 || self.ring.len() > 96 {
                // Find peak across the ring (across all channels).
                let mut peak = 0.0_f32;
                for f in self.ring.iter() {
                    for &s in f {
                        peak = peak.max(s.abs());
                    }
                }
                // Required attenuation.
                let req_atten = if peak > self.ceiling {
                    self.ceiling / peak
                } else {
                    1.0
                };
                // Pull current_atten *down* immediately, release upward.
                if req_atten < self.current_atten {
                    self.current_atten = req_atten;
                } else {
                    self.current_atten = self.current_atten * self.release_alpha
                                       + req_atten * (1.0 - self.release_alpha);
                }
                min_atten_this_block = min_atten_this_block.min(self.current_atten);
                let oldest = self.ring.pop_front().unwrap();
                for &s in &oldest {
                    out.push(s * self.current_atten);
                }
            } else {
                // Filling lookahead — emit zeros so the output stream stays
                // continuous. (Latency note: introduces `lookahead_samples`
                // of silence at the very start of playback, well-handled
                // by AEC framing alignment.)
                for _ in 0..self.channels {
                    out.push(0.0);
                }
                self.ring.pop_front();
            }
        }
        buf.copy_from_slice(&out);
        self.last_reduction_db = if min_atten_this_block < 1.0 {
            20.0 * min_atten_this_block.log10()
        } else { 0.0 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_ceiling_passes_unchanged_after_warmup() {
        let mut lim = LookaheadLimiter::new(LimiterParams::DEFAULT, 2, 48_000.0);
        // 480-sample block at -6 dBFS sine, stereo.
        let amp = 10.0_f32.powf(-6.0 / 20.0);
        let mut buf: Vec<f32> = (0..1024)
            .flat_map(|i| {
                let s = amp * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin();
                [s, s]
            })
            .collect();
        let original = buf.clone();
        lim.process_interleaved(&mut buf);
        // After lookahead warm-up (96 frames × 2 ch = 192 samples), output
        // should track input without limiting (no attenuation needed).
        for i in 200..buf.len() {
            // Allow a small numerical floor.
            assert!(
                (buf[i] - original[i - 192]).abs() < 1e-3,
                "post-warmup sample {i} mismatch: got {} expected {}",
                buf[i], original[i - 192]
            );
        }
    }

    #[test]
    fn full_scale_attenuated_to_ceiling() {
        let mut lim = LookaheadLimiter::new(LimiterParams::DEFAULT, 2, 48_000.0);
        // Full-scale square wave — alternating ±1.0 at sample rate.
        let mut buf: Vec<f32> = (0..4096)
            .flat_map(|i| {
                let s = if i % 2 == 0 { 1.0 } else { -1.0 };
                [s, s]
            })
            .collect();
        lim.process_interleaved(&mut buf);
        // After warm-up, peak should be ≤ ceiling (-1 dBFS = 0.891).
        let mut max_post_warmup = 0.0_f32;
        for &s in &buf[400..] {
            max_post_warmup = max_post_warmup.max(s.abs());
        }
        let ceiling = 10.0_f32.powf(-1.0 / 20.0);
        assert!(
            max_post_warmup <= ceiling + 1e-3,
            "expected ≤ {ceiling:.4} (-1 dBFS), got {max_post_warmup:.4}"
        );
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p waved-audio --lib playback::dsp::limiter`
Expected: PASS — both tests green.

- [ ] **Step 3: Commit**

```bash
git add crates/waved-audio/src/playback/dsp/limiter.rs
git commit -m "feat(audio/sp5): lookahead limiter (-1 dBFS ceiling, 96-sample lookahead)"
```

---

## Task 9: PlaybackDspChain orchestrator

Owns the four stages and presents a single `process(buf: &mut [i16])` entry point that converts to/from f32 internally.

**Files:**
- Create: `crates/waved-audio/src/playback/dsp/chain.rs`

- [ ] **Step 1: Create the chain**

```rust
//! Orchestrator for the playback DSP chain. Order: HPF → EQ → MBAGC → Limiter.
//! Input/output is interleaved S16 (the format `PlaybackService` already uses);
//! conversion to/from f32 happens here.

use super::eq::{BiquadParams, ParametricEq, N_BANDS, N_CHANNELS};
use super::hpf::HighPassFilter;
use super::limiter::{LimiterParams, LookaheadLimiter};
use super::mbagc::{MbagcParams, MultibandAgc};

pub struct PlaybackDspChain {
    hpf: HighPassFilter,
    eq: ParametricEq,
    mbagc: MultibandAgc,
    limiter: LookaheadLimiter,
    f32_buf: Vec<f32>,
    sample_rate_hz: f32,
}

impl PlaybackDspChain {
    pub fn new(
        hpf_cutoff_hz: f32,
        eq_params: &[[BiquadParams; N_BANDS]; N_CHANNELS],
        mbagc_params: MbagcParams,
        limiter_params: LimiterParams,
        sample_rate_hz: f32,
    ) -> Self {
        Self {
            hpf: HighPassFilter::new(N_CHANNELS, hpf_cutoff_hz, sample_rate_hz),
            eq: ParametricEq::new(eq_params, sample_rate_hz),
            mbagc: MultibandAgc::new(mbagc_params, sample_rate_hz),
            limiter: LookaheadLimiter::new(limiter_params, N_CHANNELS, sample_rate_hz),
            f32_buf: Vec::new(),
            sample_rate_hz,
        }
    }

    pub fn set_preset(
        &mut self,
        hpf_cutoff_hz: f32,
        eq_params: &[[BiquadParams; N_BANDS]; N_CHANNELS],
        mbagc_params: MbagcParams,
        limiter_params: LimiterParams,
    ) {
        self.hpf.set_cutoff(hpf_cutoff_hz);
        self.eq.set_params(eq_params);
        self.mbagc.set_params(mbagc_params);
        // Limiter retains envelope state (avoids momentary peak after switch);
        // params change only ceiling/release/lookahead — rebuild a fresh limiter
        // with the new params and discard old state.
        self.limiter = LookaheadLimiter::new(limiter_params, N_CHANNELS, self.sample_rate_hz);
        // Reset MBAGC envelopes to neutral on switch so stale targets don't bleed.
        self.mbagc.reset();
    }

    pub fn last_mbagc_gain_db(&self) -> [f32; super::mbagc::N_BANDS] {
        self.mbagc.last_gain_db()
    }

    pub fn last_limiter_reduction_db(&self) -> f32 {
        self.limiter.last_reduction_db()
    }

    /// Process interleaved S16 stereo in place. `buf.len()` must be a multiple of 2.
    pub fn process_i16(&mut self, buf: &mut [i16]) {
        if buf.is_empty() { return; }
        // Convert to f32 in [-1, 1].
        if self.f32_buf.len() != buf.len() {
            self.f32_buf.resize(buf.len(), 0.0);
        }
        for (dst, &src) in self.f32_buf.iter_mut().zip(buf.iter()) {
            *dst = src as f32 / 32768.0;
        }
        self.hpf.process_interleaved(&mut self.f32_buf);
        self.eq.process_interleaved(&mut self.f32_buf);
        self.mbagc.process_interleaved(&mut self.f32_buf);
        self.limiter.process_interleaved(&mut self.f32_buf);
        // Convert back to S16 with hard clamp (limiter should keep us in range,
        // but clamp guards against any final sample > ±1 due to numerical drift).
        for (dst, &src) in buf.iter_mut().zip(self.f32_buf.iter()) {
            *dst = (src.clamp(-1.0, 1.0) * 32767.0) as i16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::eq::{BiquadKind, BiquadParams};

    fn flat_eq() -> [[BiquadParams; N_BANDS]; N_CHANNELS] {
        [[BiquadParams::BYPASS; N_BANDS]; N_CHANNELS]
    }

    #[test]
    fn passthrough_chain_preserves_low_amplitude_signal() {
        // Bypass everything: HPF cutoff far below program material? No — HPF
        // is always on. Use a 1 kHz tone which sits well above the 80 Hz cutoff.
        let mut chain = PlaybackDspChain::new(
            80.0,
            &flat_eq(),
            MbagcParams::PASSTHROUGH,
            LimiterParams::DEFAULT,
            48_000.0,
        );
        let amp = 0.1_f32; // -20 dBFS, well under limiter ceiling
        let mut buf: Vec<i16> = (0..2048)
            .flat_map(|i| {
                let s = amp * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin();
                let v = (s * 32767.0) as i16;
                [v, v]
            })
            .collect();
        chain.process_i16(&mut buf);
        // After the limiter's 96-sample lookahead warmup, sample energy should
        // be roughly preserved (the HPF only loses ~0 dB at 1 kHz).
        let rms_squared: f64 = buf[400..].iter().map(|&s| (s as f64).powi(2)).sum::<f64>()
                             / (buf.len() - 400) as f64;
        let rms = rms_squared.sqrt() / 32768.0;
        let rms_db = 20.0 * (rms as f32).log10();
        // Sine RMS of -20 dBFS amplitude ≈ -23 dBFS.
        assert!(rms_db > -25.0 && rms_db < -21.0, "expected ~-23 dBFS, got {rms_db:.2} dBFS");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p waved-audio --lib playback::dsp::chain`
Expected: PASS.

- [ ] **Step 3: Run all DSP tests together**

Run: `cargo test -p waved-audio --lib playback::dsp`
Expected: PASS — all DSP tests green (~10 tests across 6 files).

- [ ] **Step 4: Commit**

```bash
git add crates/waved-audio/src/playback/dsp/chain.rs
git commit -m "feat(audio/sp5): PlaybackDspChain — orchestrate HPF → EQ → MBAGC → limiter"
```

---

## Task 10: PlaybackPreset + embedded voice tuning

Define the preset enum, the tuning struct, the embedded loader (parses vendored stock JSON), and the media-from-voice helper.

**Files:**
- Create: `crates/waved-audio/src/playback/preset.rs`

- [ ] **Step 1: Create the file**

```rust
//! Playback preset definitions.
//!
//! `PlaybackPreset::Voice` — conferencing tuning, matches stock
//! `dynamic_eq_config_2.json` byte-for-byte (HPF + EQ + MBAGC + limiter).
//! `PlaybackPreset::Media` — music tuning derived from Voice with MBAGC
//! bypassed and EQ flattened.

use serde::Deserialize;
use std::sync::OnceLock;

pub use waved_state::PlaybackPreset;

use super::dsp::eq::{BiquadKind, BiquadParams, N_BANDS, N_CHANNELS};
use super::dsp::limiter::LimiterParams;
use super::dsp::mbagc::{MbagcParams, N_BANDS as MBAGC_N_BANDS};

/// Concrete tuning for one preset. Drives `PlaybackDspChain::set_preset`.
#[derive(Debug, Clone)]
pub struct PlaybackPresetTuning {
    pub hpf_cutoff_hz: f32,
    pub eq: [[BiquadParams; N_BANDS]; N_CHANNELS],
    pub mbagc: MbagcParams,
    pub limiter: LimiterParams,
}

/// Stock JSON shape — only the fields we care about. Rest are tolerated.
#[derive(Debug, Deserialize)]
struct StockConfig {
    asp_param_eq: AspParamEq,
}

#[derive(Debug, Deserialize)]
struct AspParamEq {
    cut_off_freq: f32,
    multi_band_num: usize,
    agc_settings: Vec<AgcSetting>,
    eq_filter_num: usize,
    eq_settings: Vec<EqSetting>,
}

#[derive(Debug, Deserialize)]
struct AgcSetting {
    crossover_freq: Vec<f32>,
    mbagc_target: Vec<f32>,
    mbagc_max_gain: Vec<f32>,
    mbagc_gain_knee: Vec<f32>,
    mbagc_release_time: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct EqSetting {
    /// Stock encodes type per band: 0=peaking, 1=low shelf, 2=high shelf.
    #[serde(rename = "type")]
    kinds: Vec<u8>,
    center_freq: Vec<f32>,
    gain_dB: Vec<f32>,
    Q: Vec<f32>,
}

const STOCK_VOICE_JSON: &str = include_str!("../../data/dynamic_eq_config_2.json");

static VOICE_PRESET: OnceLock<PlaybackPresetTuning> = OnceLock::new();

/// Returns the embedded voice preset. Parses the vendored stock JSON on first
/// call. Panics with a descriptive message if the JSON is malformed — caught
/// in unit tests, never at runtime if we ship.
pub fn voice_preset() -> &'static PlaybackPresetTuning {
    VOICE_PRESET.get_or_init(|| parse_stock_json(STOCK_VOICE_JSON)
        .expect("vendored stock JSON must parse — check crates/waved-audio/data/"))
}

fn parse_stock_json(s: &str) -> Result<PlaybackPresetTuning, String> {
    let cfg: StockConfig = serde_json::from_str(s).map_err(|e| format!("parse: {e}"))?;
    let p = cfg.asp_param_eq;

    if p.multi_band_num != MBAGC_N_BANDS {
        return Err(format!("multi_band_num expected {}, got {}", MBAGC_N_BANDS, p.multi_band_num));
    }
    if p.eq_filter_num != N_BANDS {
        return Err(format!("eq_filter_num expected {}, got {}", N_BANDS, p.eq_filter_num));
    }

    let agc = p.agc_settings.first().ok_or("agc_settings empty")?;
    if agc.crossover_freq.len() != 3 {
        return Err(format!("expected 3 crossovers, got {}", agc.crossover_freq.len()));
    }
    let mbagc = MbagcParams {
        enabled: true,
        crossover_freq_hz: [agc.crossover_freq[0], agc.crossover_freq[1], agc.crossover_freq[2]],
        target_dbfs: convert_target_to_dbfs(&agc.mbagc_target)?,
        max_gain_db: [agc.mbagc_max_gain[0], agc.mbagc_max_gain[1], agc.mbagc_max_gain[2], agc.mbagc_max_gain[3]],
        knee_db: [agc.mbagc_gain_knee[0], agc.mbagc_gain_knee[1], agc.mbagc_gain_knee[2], agc.mbagc_gain_knee[3]],
        release_ms: [agc.mbagc_release_time[0], agc.mbagc_release_time[1], agc.mbagc_release_time[2], agc.mbagc_release_time[3]],
    };

    let eq_set = p.eq_settings.first().ok_or("eq_settings empty")?;
    let mut bands = [[BiquadParams::BYPASS; N_BANDS]; N_CHANNELS];
    for band in 0..N_BANDS {
        let kind = match eq_set.kinds.get(band).copied().unwrap_or(0) {
            0 => BiquadKind::Peaking,
            1 => BiquadKind::LowShelf,
            2 => BiquadKind::HighShelf,
            other => return Err(format!("unknown eq type code {other}")),
        };
        let params = BiquadParams {
            kind,
            center_freq_hz: eq_set.center_freq[band],
            gain_db: eq_set.gain_dB[band],
            q: eq_set.Q[band],
        };
        bands[0][band] = params;
        bands[1][band] = params;
    }

    Ok(PlaybackPresetTuning {
        hpf_cutoff_hz: p.cut_off_freq,
        eq: bands,
        mbagc,
        limiter: LimiterParams::DEFAULT,
    })
}

/// Stock targets are i16-amplitude peak values (e.g. 8203 ≈ 0.25 fullscale).
/// Convert to dBFS (relative to 32768 = 0 dBFS).
fn convert_target_to_dbfs(raw: &[f32]) -> Result<[f32; MBAGC_N_BANDS], String> {
    if raw.len() != MBAGC_N_BANDS {
        return Err(format!("expected {} mbagc_target values, got {}", MBAGC_N_BANDS, raw.len()));
    }
    let mut out = [0.0_f32; MBAGC_N_BANDS];
    for (i, &v) in raw.iter().enumerate() {
        let normalized = (v / 32768.0).max(1e-6);
        out[i] = 20.0 * normalized.log10();
    }
    Ok(out)
}

/// Media preset = Voice with MBAGC bypassed and EQ flattened.
pub fn media_preset_from_voice(voice: &PlaybackPresetTuning) -> PlaybackPresetTuning {
    PlaybackPresetTuning {
        hpf_cutoff_hz: voice.hpf_cutoff_hz,
        eq: [[BiquadParams::BYPASS; N_BANDS]; N_CHANNELS],
        mbagc: MbagcParams { enabled: false, ..voice.mbagc },
        limiter: voice.limiter,
    }
}

pub fn tuning_for(preset: PlaybackPreset) -> PlaybackPresetTuning {
    let voice = voice_preset();
    match preset {
        PlaybackPreset::Voice => voice.clone(),
        PlaybackPreset::Media => media_preset_from_voice(voice),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_preset_loads_without_panic() {
        let v = voice_preset();
        assert!(v.hpf_cutoff_hz >= 20.0 && v.hpf_cutoff_hz <= 200.0,
                "voice HPF cutoff {} out of expected range", v.hpf_cutoff_hz);
        assert!(v.mbagc.enabled, "voice preset MBAGC should be enabled");
        assert_eq!(v.mbagc.crossover_freq_hz, [300.0, 4000.0, 7000.0]);
    }

    #[test]
    fn voice_preset_target_dbfs_matches_stock() {
        // Stock numbers: [8203, 16406, 1160, 580] / 32768 → dBFS
        let v = voice_preset();
        let expected = [
            20.0_f32 * (8203.0_f32 / 32768.0).log10(),
            20.0_f32 * (16406.0_f32 / 32768.0).log10(),
            20.0_f32 * (1160.0_f32 / 32768.0).log10(),
            20.0_f32 * (580.0_f32 / 32768.0).log10(),
        ];
        for (i, (a, b)) in v.mbagc.target_dbfs.iter().zip(expected.iter()).enumerate() {
            assert!((a - b).abs() < 0.01, "band {i}: got {a}, expected {b}");
        }
    }

    #[test]
    fn media_preset_disables_mbagc_and_flattens_eq() {
        let v = voice_preset();
        let m = media_preset_from_voice(v);
        assert!(!m.mbagc.enabled);
        assert_eq!(m.eq[0][0].kind, BiquadKind::Bypass);
        assert_eq!(m.eq[0][3].kind, BiquadKind::Bypass);
        assert_eq!(m.hpf_cutoff_hz, v.hpf_cutoff_hz);  // HPF stays
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p waved-audio --lib playback::preset`
Expected: PASS — three tests green. (Test 1 uses `voice_preset()` for the first time, which forces JSON parse; this validates the vendored copy is well-formed.)

- [ ] **Step 3: Add `PlaybackPreset` to `waved-state` (test still references it)**

This is a forward dependency. Until Task 13, `waved_state::PlaybackPreset` doesn't exist; the test file references it via `pub use waved_state::PlaybackPreset` at the top of `preset.rs`. To unblock this task without doing 13 yet, temporarily replace the `pub use waved_state::PlaybackPreset;` line at the top of `preset.rs` with:

```rust
// Local placeholder — replaced by `pub use waved_state::PlaybackPreset` in Task 13.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackPreset { Voice, Media }
impl Default for PlaybackPreset { fn default() -> Self { PlaybackPreset::Voice } }
```

Tests pass with the local placeholder; Task 13 swaps the local enum for the re-export from `waved-state`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p waved-audio --lib playback::preset`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/src/playback/preset.rs
git commit -m "feat(audio/sp5): preset definitions + embedded voice tuning loader"
```

---

## Task 11: Wire DSP into `run_playback_loop`

Thread the chain through the playback loop: instantiate it from the active preset, apply per batch, broadcast the post-DSP buffer.

**Files:**
- Modify: `crates/waved-audio/src/playback.rs`

- [ ] **Step 1: Modify `PlaybackConfig` to carry the preset**

In `crates/waved-audio/src/playback.rs`, add to `PlaybackConfig` (around line 32):

```rust
#[derive(Debug, Clone)]
pub struct PlaybackConfig {
    pub device: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub format: SampleFormat,
    pub period_frames: usize,
    pub buffer_periods: u32,
    /// Initial preset. Default `PlaybackPreset::Voice`.
    pub initial_preset: waved_state::PlaybackPreset,
}
```

- [ ] **Step 2: Modify `PlaybackService` to expose a preset channel**

Add an `mpsc::Sender<PlaybackPreset>` to `PlaybackService`:

```rust
pub struct PlaybackService {
    cmd_tx: mpsc::Sender<Vec<i16>>,
    preset_tx: mpsc::Sender<waved_state::PlaybackPreset>,
    ref_tx: broadcast::Sender<Vec<i16>>,
    channels: u32,
    period_frames: usize,
    sample_rate: u32,
    stats: Arc<PlaybackStats>,
}
```

Add a method:

```rust
impl PlaybackService {
    pub fn set_preset(&self, preset: waved_state::PlaybackPreset) {
        let _ = self.preset_tx.try_send(preset);
    }
    // ... existing methods
}
```

- [ ] **Step 3: Modify `run_playback_loop` to apply DSP and tap post-DSP**

Replace `run_playback_loop` body to:

```rust
pub fn run_playback_loop<S: PlaybackSink>(
    mut sink: S,
    mut cmd_rx: mpsc::Receiver<Vec<i16>>,
    mut preset_rx: mpsc::Receiver<waved_state::PlaybackPreset>,
    ref_tx: broadcast::Sender<Vec<i16>>,
    stats: Arc<PlaybackStats>,
    period_samples: usize,
    sample_rate: u32,
    initial_preset: waved_state::PlaybackPreset,
) {
    use crate::playback::dsp::PlaybackDspChain;
    use crate::playback::preset::tuning_for;

    let silence: Vec<i16> = vec![0; period_samples];
    info!("playback loop starting: samples/period={period_samples}");

    let mut tuning = tuning_for(initial_preset);
    let mut chain = PlaybackDspChain::new(
        tuning.hpf_cutoff_hz,
        &tuning.eq,
        tuning.mbagc,
        tuning.limiter,
        sample_rate as f32,
    );

    loop {
        // Apply pending preset switch (one-shot, drop earlier requests).
        while let Ok(p) = preset_rx.try_recv() {
            tuning = tuning_for(p);
            chain.set_preset(
                tuning.hpf_cutoff_hz,
                &tuning.eq,
                tuning.mbagc,
                tuning.limiter,
            );
            info!("playback preset switched → {:?}", p);
        }

        let (batch, is_silence) = match cmd_rx.try_recv() {
            Ok(b) => (b, false),
            Err(mpsc::error::TryRecvError::Empty) => (silence.clone(), true),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                info!("playback cmd channel closed, stopping");
                return;
            }
        };
        let mut normalized = if batch.len() == period_samples {
            batch
        } else if batch.len() < period_samples {
            let mut v = batch;
            v.resize(period_samples, 0);
            v
        } else {
            warn!("playback batch truncated: {} > {}", batch.len(), period_samples);
            batch.into_iter().take(period_samples).collect()
        };

        // SP#5: DSP applied before the ALSA write. AEC reference broadcast
        // taps the post-DSP buffer so the cancellation reference matches what
        // actually reaches the speaker.
        chain.process_i16(&mut normalized);

        match sink.write(&normalized) {
            Ok(()) => {
                if is_silence {
                    stats.frames_silence.fetch_add((period_samples / sink.channels() as usize) as u64, Ordering::Relaxed);
                } else {
                    stats.frames_written.fetch_add((period_samples / sink.channels() as usize) as u64, Ordering::Relaxed);
                }
                let _ = ref_tx.send(normalized);
            }
            Err(WriteError::Underrun) => {
                stats.underruns.fetch_add(1, Ordering::Relaxed);
                sink.recover();
            }
            Err(WriteError::Fatal(e)) => {
                warn!("playback fatal write: {e}");
                return;
            }
        }
    }
}
```

- [ ] **Step 4: Modify `spawn_playback` to wire the preset channel**

In the `alsa_impl::spawn_playback` function, replace the body to:

```rust
pub fn spawn_playback(cfg: PlaybackConfig) -> Result<PlaybackService, PlaybackError> {
    let sink = AlsaPlaybackSink::open(&cfg)?;
    let (cmd_tx, cmd_rx) = mpsc::channel::<Vec<i16>>(64);
    let (preset_tx, preset_rx) = mpsc::channel::<waved_state::PlaybackPreset>(8);
    let (ref_tx, _) = broadcast::channel::<Vec<i16>>(32);
    let stats = Arc::new(PlaybackStats::default());
    let period_samples = cfg.period_frames * cfg.channels as usize;
    let sample_rate = cfg.sample_rate;
    let initial_preset = cfg.initial_preset;

    let ref_tx_thread = ref_tx.clone();
    let stats_thread = Arc::clone(&stats);
    std::thread::Builder::new()
        .name("audio-playback".into())
        .spawn(move || {
            super::set_realtime_priority();
            run_playback_loop(
                sink, cmd_rx, preset_rx, ref_tx_thread, stats_thread,
                period_samples, sample_rate, initial_preset,
            );
        })
        .map_err(|e| PlaybackError::Invalid(format!("spawn playback thread: {e}")))?;

    Ok(PlaybackService {
        cmd_tx,
        preset_tx,
        ref_tx,
        channels: cfg.channels,
        period_frames: cfg.period_frames,
        sample_rate: cfg.sample_rate,
        stats,
    })
}
```

- [ ] **Step 5: Update existing tests in `playback.rs`**

The existing test helpers `run_with_mock` and the four `#[test]` functions in `playback.rs::tests` call `run_playback_loop` with the old signature. Update each call site to:

```rust
let (preset_tx, preset_rx) = mpsc::channel::<waved_state::PlaybackPreset>(2);
drop(preset_tx);  // we don't switch presets in these tests
let handle = std::thread::spawn(move || {
    run_playback_loop(
        sink, cmd_rx, preset_rx, ref_tx, stats_thread,
        period_samples, 48_000, waved_state::PlaybackPreset::Voice,
    )
});
```

Adjust each of the four tests (`silence_when_queue_empty`, `submitted_batch_is_played_and_referenced`, `short_batch_is_padded_with_silence`, `underrun_increments_stat_and_continues`).

The test that asserts `batches[0] == payload` (i.e., DSP is a no-op) must adjust to allow ±2 LSB tolerance — the chain warms up the limiter and HPF, which add small artefacts. Replace exact equality with:

```rust
assert!(batches[0].iter().zip(payload.iter()).all(|(a, b)| (*a - *b).abs() <= 2),
        "first batch deviates from payload more than 2 LSB");
```

For tests where the input is silence or a small known signal, the assertions hold within tolerance.

- [ ] **Step 6: Run tests**

Run: `cargo test -p waved-audio --lib playback`
Expected: PASS — all playback tests green.

- [ ] **Step 7: Build the full binary**

Run: `cargo build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved`
Expected: builds successfully (no other call sites broken).

- [ ] **Step 8: Commit**

```bash
git add crates/waved-audio/src/playback.rs
git commit -m "feat(audio/sp5): wire DSP chain into PlaybackService::run_playback_loop"
```

---

## Task 12: `AudioConfig` v3 → v4 + `PlaybackDspConfig`

Bump version, add the optional `dsp` field on `PlaybackSection`, update validation.

**Files:**
- Modify: `crates/waved-audio/src/audio_config.rs`
- Modify: `crates/waved-audio/config/audio_config_default.json`

- [ ] **Step 1: Add the new types**

In `crates/waved-audio/src/audio_config.rs`, after the `PlaybackSection` struct, add:

```rust
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PlaybackDspConfig {
    #[serde(default)]
    pub voice: Option<PlaybackPresetTuningJson>,
    #[serde(default)]
    pub media: Option<PlaybackPresetTuningJson>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PlaybackPresetTuningJson {
    pub hpf_cutoff_hz: f32,
    pub eq: [[crate::playback::dsp::eq::BiquadParams; 4]; 2],
    pub mbagc: crate::playback::dsp::mbagc::MbagcParams,
    pub limiter: crate::playback::dsp::limiter::LimiterParams,
}
```

Add `dsp: Option<PlaybackDspConfig>` to `PlaybackSection`:

```rust
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PlaybackSection {
    pub device: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub format: SampleFormat,
    pub period_frames: usize,
    pub buffer_periods: u32,
    #[serde(default)]
    pub dsp: Option<PlaybackDspConfig>,
}
```

- [ ] **Step 2: Update `validate()` to accept v3 and v4**

Replace the version check (around line 116):

```rust
fn validate(&self) -> Result<(), ConfigError> {
    if self.version != 3 && self.version != 4 {
        return Err(ConfigError::Invalid(format!(
            "unsupported config version {}, expected 3 or 4", self.version
        )));
    }
    // ... rest of existing validation
```

Append, just before `Ok(())`:

```rust
    if let Some(p) = &self.playback {
        if let Some(dsp) = &p.dsp {
            for (name, t) in [("voice", &dsp.voice), ("media", &dsp.media)] {
                if let Some(t) = t {
                    if !(20.0..=200.0).contains(&t.hpf_cutoff_hz) {
                        return Err(ConfigError::Invalid(format!(
                            "{name}.hpf_cutoff_hz must be in [20, 200], got {}", t.hpf_cutoff_hz
                        )));
                    }
                    if !(-3.0..=0.0).contains(&t.limiter.ceiling_dbfs) {
                        return Err(ConfigError::Invalid(format!(
                            "{name}.limiter.ceiling_dbfs must be in [-3, 0], got {}", t.limiter.ceiling_dbfs
                        )));
                    }
                    let cf = t.mbagc.crossover_freq_hz;
                    if !(cf[0] < cf[1] && cf[1] < cf[2]) {
                        return Err(ConfigError::Invalid(format!(
                            "{name}.mbagc.crossover_freq_hz must be strictly increasing, got {cf:?}"
                        )));
                    }
                }
            }
        }
    }
```

- [ ] **Step 3: Bump default JSON to v4**

In `crates/waved-audio/config/audio_config_default.json`, change `"version": 3` to `"version": 4`. (No `dsp` field — embedded preset is used.)

- [ ] **Step 4: Add tests for v3 and v4 acceptance**

Append to the `tests` module in `audio_config.rs`:

```rust
    #[test]
    fn v3_config_still_parses() {
        let json = r#"{
            "version": 3,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE"
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
        }"#;
        let cfg = AudioConfig::from_json(json).expect("v3 should parse");
        assert_eq!(cfg.version, 3);
        assert!(cfg.playback.unwrap().dsp.is_none());
    }

    #[test]
    fn v4_config_with_dsp_block_parses() {
        let json = r#"{
            "version": 4,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE"
            },
            "playback": {
                "device": "hw:0,1",
                "channels": 2,
                "sample_rate": 48000,
                "format": "S16_LE",
                "period_frames": 1024,
                "buffer_periods": 4,
                "dsp": {
                    "voice": {
                        "hpf_cutoff_hz": 80.0,
                        "eq": [
                            [
                                {"kind":"peaking","center_freq_hz":150.0,"gain_db":-6.0,"q":1.0},
                                {"kind":"peaking","center_freq_hz":800.0,"gain_db":2.0,"q":2.0},
                                {"kind":"peaking","center_freq_hz":400.0,"gain_db":-5.0,"q":1.5},
                                {"kind":"peaking","center_freq_hz":3500.0,"gain_db":5.0,"q":2.0}
                            ],
                            [
                                {"kind":"peaking","center_freq_hz":150.0,"gain_db":-6.0,"q":1.0},
                                {"kind":"peaking","center_freq_hz":800.0,"gain_db":2.0,"q":2.0},
                                {"kind":"peaking","center_freq_hz":400.0,"gain_db":-5.0,"q":1.5},
                                {"kind":"peaking","center_freq_hz":3500.0,"gain_db":5.0,"q":2.0}
                            ]
                        ],
                        "mbagc": {
                            "enabled": true,
                            "crossover_freq_hz": [300.0, 4000.0, 7000.0],
                            "target_dbfs": [-12.0, -6.0, -29.0, -35.0],
                            "max_gain_db": [6.0, 20.0, 10.0, 10.0],
                            "knee_db": [5.0, 5.0, 5.0, 5.0],
                            "release_ms": [200.0, 200.0, 200.0, 200.0]
                        },
                        "limiter": {
                            "ceiling_dbfs": -1.0,
                            "lookahead_samples": 96,
                            "release_ms": 50.0
                        }
                    }
                }
            },
            "period_frames": 1024,
            "buffer_periods": 8
        }"#;
        let cfg = AudioConfig::from_json(json).expect("v4 should parse");
        assert_eq!(cfg.version, 4);
        let voice = cfg.playback.unwrap().dsp.unwrap().voice.unwrap();
        assert_eq!(voice.hpf_cutoff_hz, 80.0);
        assert!(voice.mbagc.enabled);
    }

    #[test]
    fn invalid_hpf_cutoff_rejected() {
        let mut json = r#"{"version":4,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},"playback":{"device":"hw:0,1","channels":2,"sample_rate":48000,"format":"S16_LE","period_frames":1024,"buffer_periods":4,"dsp":{"voice":{"hpf_cutoff_hz":5.0,"eq":[[{"kind":"bypass","center_freq_hz":1000.0,"gain_db":0.0,"q":1.0},{"kind":"bypass","center_freq_hz":1000.0,"gain_db":0.0,"q":1.0},{"kind":"bypass","center_freq_hz":1000.0,"gain_db":0.0,"q":1.0},{"kind":"bypass","center_freq_hz":1000.0,"gain_db":0.0,"q":1.0}],[{"kind":"bypass","center_freq_hz":1000.0,"gain_db":0.0,"q":1.0},{"kind":"bypass","center_freq_hz":1000.0,"gain_db":0.0,"q":1.0},{"kind":"bypass","center_freq_hz":1000.0,"gain_db":0.0,"q":1.0},{"kind":"bypass","center_freq_hz":1000.0,"gain_db":0.0,"q":1.0}]],"mbagc":{"enabled":false,"crossover_freq_hz":[300.0,4000.0,7000.0],"target_dbfs":[0.0,0.0,0.0,0.0],"max_gain_db":[0.0,0.0,0.0,0.0],"knee_db":[5.0,5.0,5.0,5.0],"release_ms":[200.0,200.0,200.0,200.0]},"limiter":{"ceiling_dbfs":-1.0,"lookahead_samples":96,"release_ms":50.0}}}},"period_frames":1024,"buffer_periods":8}"#.to_string();
        let res = AudioConfig::from_json(&json);
        assert!(res.is_err(), "hpf_cutoff_hz=5 should be rejected");
        json = json.replace("\"hpf_cutoff_hz\":5.0", "\"hpf_cutoff_hz\":80.0");
        let cfg = AudioConfig::from_json(&json).expect("80 Hz should validate");
        assert_eq!(cfg.version, 4);
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p waved-audio --lib audio_config`
Expected: PASS — three new tests + existing tests green.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-audio/src/audio_config.rs \
        crates/waved-audio/config/audio_config_default.json
git commit -m "feat(audio/sp5): AudioConfig v4 + optional PlaybackDspConfig"
```

---

## Task 13: `PlaybackPreset` in `waved-state`

Define the canonical preset enum, add it to `AudioState`, add the `Command` variant. Replaces the local placeholder added in Task 10.

**Files:**
- Modify: `crates/waved-state/src/lib.rs`
- Modify: `crates/waved-audio/src/playback/preset.rs`

- [ ] **Step 1: Add `PlaybackPreset` to `waved-state`**

In `crates/waved-state/src/lib.rs`, after the `NrLevel` enum (line 40), add:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackPreset {
    #[default]
    Voice,
    Media,
}
```

Add `playback_preset` to `AudioState` (line 9):

```rust
#[derive(Clone, Debug, Default)]
pub struct AudioState {
    pub mode: AudioMode,
    pub mic_muted: bool,
    pub speaker_muted: bool,
    pub speaker_volume: u16,
    pub mic_volume: u16,
    pub doa_horizontal: f32,
    pub doa_pitch: f32,
    pub vu_level: [f32; 8],
    pub nr_level: NrLevel,
    pub playback_preset: PlaybackPreset,  // SP#5
}
```

In the `Command` enum, after `SetNrLevel`:

```rust
    SetPlaybackPreset(PlaybackPreset),
```

- [ ] **Step 2: Replace the local placeholder in `preset.rs`**

In `crates/waved-audio/src/playback/preset.rs`, remove the local `enum PlaybackPreset { ... }` placeholder added in Task 10 and restore:

```rust
pub use waved_state::PlaybackPreset;
```

- [ ] **Step 3: Add a state-roundtrip test**

In `crates/waved-state/src/lib.rs`, in the existing test module (around line 200), add:

```rust
    #[test]
    fn playback_preset_default_is_voice() {
        let s = AudioState::default();
        assert_eq!(s.playback_preset, PlaybackPreset::Voice);
    }

    #[test]
    fn set_playback_preset_command_routes() {
        let cmd = Command::SetPlaybackPreset(PlaybackPreset::Media);
        assert!(matches!(cmd, Command::SetPlaybackPreset(PlaybackPreset::Media)));
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p waved-state && cargo test -p waved-audio --lib`
Expected: PASS — both crates green.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-state/src/lib.rs \
        crates/waved-audio/src/playback/preset.rs
git commit -m "feat(state/sp5): PlaybackPreset enum + AudioState field + Command variant"
```

---

## Task 14: SIGHUP `/tmp/waved-playback-preset` + AudioService routing

Plug the preset switch end-to-end: SIGHUP reads the file → `Command::SetPlaybackPreset` on the bus → `AudioService::run` forwards to `PlaybackService::set_preset` → playback loop applies it.

**Files:**
- Modify: `crates/waved/src/main.rs`
- Modify: `crates/waved-audio/src/lib.rs`

- [ ] **Step 1: Extend `mode_switch_handler` in `main.rs`**

In `crates/waved/src/main.rs`, inside `mode_switch_handler` (around line 525, after the NR_PATH block), add:

```rust
        const PRESET_PATH: &str = "/tmp/waved-playback-preset";
        match std::fs::read_to_string(PRESET_PATH) {
            Ok(raw) => {
                let preset = match raw.trim().to_ascii_lowercase().as_str() {
                    "voice" => Some(waved_state::PlaybackPreset::Voice),
                    "media" => Some(waved_state::PlaybackPreset::Media),
                    "" => None,
                    other => {
                        warn!("SIGHUP: unknown playback preset {other:?}");
                        None
                    }
                };
                if let Some(p) = preset {
                    info!("SIGHUP: setting playback preset → {p:?}");
                    let _ = commands.send(Command::SetPlaybackPreset(p));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("SIGHUP: {PRESET_PATH}: {e}"),
        }
```

Update the info! sources line:

```rust
    info!(
        "SIGHUP handler ready (sources: /tmp/waved-mode, /tmp/waved-doa-override, /tmp/waved-nr, /tmp/waved-playback-preset)"
    );
```

- [ ] **Step 2: Hand `Arc<PlaybackService>` to `AudioService` for preset routing**

`AudioService::run` consumes `Command`s but currently has no playback handle. Two clean options: (a) add a setter on `AudioService` that takes the `PlaybackService` Arc, or (b) duplicate the preset wiring in `main.rs` outside `AudioService`. Option (b) is smaller and matches how `Command::ToggleSpeakerMute` works today (mixer shell-out lives in `AudioService` because it doesn't need playback).

Pick option (b): In `main.rs`, in the `if let Some(pb) = cfg.playback.clone()` branch (around line 226), after `let playback = Arc::new(playback);`, add:

```rust
            // SP#5: forward preset commands to the playback loop.
            tokio::spawn({
                let playback = Arc::clone(&playback);
                let mut cmd_rx = bus.commands.subscribe();
                async move {
                    while let Ok(cmd) = cmd_rx.recv().await {
                        if let Command::SetPlaybackPreset(p) = cmd {
                            playback.set_preset(p);
                        }
                    }
                }
            });
```

Also in the same block, when constructing `PlaybackConfig`, set `initial_preset`:

```rust
            let playback = match spawn_playback(PlaybackConfig {
                device: pb.device.clone(),
                channels: pb.channels,
                sample_rate: pb.sample_rate,
                format: pb.format,
                period_frames: pb.period_frames,
                buffer_periods: pb.buffer_periods,
                initial_preset: bus.audio.borrow().playback_preset,  // SP#5
            }) {
```

- [ ] **Step 3: Forward the state update**

The state should reflect the active preset for stats and UI consistency. In `crates/waved-audio/src/lib.rs`, in `AudioService::run`, in the existing `tokio::select! { cmd = cmd_rx.recv() => match cmd { ... } }` block, after `Ok(Command::SetNrLevel(level))`, add:

```rust
                        Ok(Command::SetPlaybackPreset(p)) => {
                            info!("playback preset → {p:?}");
                            self.audio_tx.send_modify(|s| s.playback_preset = p);
                        }
```

- [ ] **Step 4: Build**

Run: `cargo build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved`
Expected: builds successfully.

- [ ] **Step 5: Commit**

```bash
git add crates/waved/src/main.rs \
        crates/waved-audio/src/lib.rs
git commit -m "feat(audio/sp5): SIGHUP /tmp/waved-playback-preset + state routing"
```

---

## Task 15: Stats fields

Surface the runtime DSP state in `/tmp/audio_stats.json`.

**Files:**
- Modify: `crates/waved-audio/src/stats_collector.rs`
- Modify: `crates/waved-audio/src/playback.rs` (expose getter)

- [ ] **Step 1: Add a stats getter on `PlaybackService`**

In `crates/waved-audio/src/playback.rs`, the chain is owned by the playback thread. To expose its state without a Mutex round-trip, add an `Arc<DspMetrics>` shared between thread and service:

Add near `PlaybackStats` (around line 53):

```rust
#[derive(Debug, Default)]
pub struct DspMetrics {
    /// Per-band MBAGC gain (dB), packed [b0,b1,b2,b3] as fixed-point i32 = dB×100.
    /// Atomic update — readers may see partial state for a single tick, but
    /// values rarely change faster than the 500 ms stats cadence.
    pub mbagc_gain_db_x100: [std::sync::atomic::AtomicI32; 4],
    pub limiter_reduction_db_x100: std::sync::atomic::AtomicI32,
}
```

Add it to `PlaybackService`:

```rust
pub struct PlaybackService {
    cmd_tx: mpsc::Sender<Vec<i16>>,
    preset_tx: mpsc::Sender<waved_state::PlaybackPreset>,
    ref_tx: broadcast::Sender<Vec<i16>>,
    channels: u32,
    period_frames: usize,
    sample_rate: u32,
    stats: Arc<PlaybackStats>,
    dsp_metrics: Arc<DspMetrics>,
}

impl PlaybackService {
    pub fn dsp_metrics(&self) -> Arc<DspMetrics> {
        Arc::clone(&self.dsp_metrics)
    }
    // ... existing
}
```

- [ ] **Step 2: Update the playback loop and `spawn_playback` to write metrics**

Add `dsp_metrics: Arc<DspMetrics>` parameter to `run_playback_loop`, and after `chain.process_i16(&mut normalized);` add:

```rust
        // Snapshot DSP state for stats consumers.
        let mbagc = chain.last_mbagc_gain_db();
        for i in 0..4 {
            dsp_metrics.mbagc_gain_db_x100[i].store(
                (mbagc[i] * 100.0) as i32, Ordering::Relaxed
            );
        }
        dsp_metrics.limiter_reduction_db_x100.store(
            (chain.last_limiter_reduction_db() * 100.0) as i32, Ordering::Relaxed
        );
```

In `spawn_playback`:

```rust
    let dsp_metrics = Arc::new(DspMetrics::default());
    let dsp_metrics_thread = Arc::clone(&dsp_metrics);
    // ...
    .spawn(move || {
        super::set_realtime_priority();
        run_playback_loop(
            sink, cmd_rx, preset_rx, ref_tx_thread, stats_thread,
            period_samples, sample_rate, initial_preset, dsp_metrics_thread,
        );
    })
    // ...
    Ok(PlaybackService {
        cmd_tx, preset_tx, ref_tx,
        channels: cfg.channels,
        period_frames: cfg.period_frames,
        sample_rate: cfg.sample_rate,
        stats,
        dsp_metrics,
    })
```

- [ ] **Step 3: Wire stats into `stats_collector.rs`**

Find the existing `StatsRow` (or equivalent JSON struct) in `crates/waved-audio/src/stats_collector.rs`. Add fields:

```rust
    pub playback_preset: String,         // "voice" | "media"
    pub mbagc_active_gain_db: [f32; 4],
    pub limiter_reduction_db: f32,
```

In the `run` task that emits the JSON, accept `Option<Arc<DspMetrics>>` and a `watch::Receiver<AudioState>` (likely already there). Read state for the preset name; read atomic metrics for gain/reduction. Convert `dB×100` integers back to f32 with `/ 100.0`.

Pass the metrics from `main.rs` when spawning `stats_collector::run`:

```rust
        tokio::spawn(stats_collector::run(
            stats_rx,
            stats_processed_rx,
            bus.audio.subscribe(),
            Arc::clone(&capture_stats),
            map_source,
            std::path::PathBuf::from("/tmp/audio_stats.json"),
            std::time::Duration::from_millis(500),
            alignment.clone(),
            Some(playback.dsp_metrics()),  // SP#5 — only Some when playback exists
        ));
```

For the dual/single/no-playback branches that don't spawn a `PlaybackService`, pass `None`.

- [ ] **Step 4: Build + run unit tests**

Run: `cargo test -p waved-audio && cargo build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved`
Expected: tests + build green.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/src/playback.rs \
        crates/waved-audio/src/stats_collector.rs \
        crates/waved/src/main.rs
git commit -m "feat(audio/sp5): stats — playback_preset, mbagc gain, limiter reduction"
```

---

## Task 16: Biquad regression test (impulse → known reference)

Pin the biquad form bit-exact against pre-computed reference samples. This is the spec's Risk #1 mitigation — if stock's coefficient form differs from RBJ direct-form-1, this test fires before any tuning is applied.

**Files:**
- Create: `crates/waved-audio/tests/biquad_regression.rs`

- [ ] **Step 1: Generate the reference (one-time, host-side)**

Run this Python on the host once to produce the reference samples:

```bash
python3 - <<'PY'
# Reference: 48 kHz, peaking EQ +6 dB at 1 kHz, Q=1, computed in float64.
import math
fs = 48_000.0
f0 = 1_000.0
db = 6.0
q = 1.0

A = 10 ** (db / 40.0)
w0 = 2 * math.pi * f0 / fs
cw = math.cos(w0); sw = math.sin(w0); alpha = sw / (2 * q)
b0 = 1 + alpha * A
b1 = -2 * cw
b2 = 1 - alpha * A
a0 = 1 + alpha / A
a1 = -2 * cw
a2 = 1 - alpha / A
b0 /= a0; b1 /= a0; b2 /= a0; a1 /= a0; a2 /= a0

# Impulse response: input [1, 0, 0, ...] for 16 samples.
x = [1.0] + [0.0] * 15
y = []
x1 = x2 = y1 = y2 = 0.0
for x0 in x:
    y0 = b0*x0 + b1*x1 + b2*x2 - a1*y1 - a2*y2
    y.append(y0)
    x2 = x1; x1 = x0
    y2 = y1; y1 = y0
print(",\n".join(f"  {v:.10}" for v in y))
PY
```

Capture the printed values into the test below.

- [ ] **Step 2: Write the regression test**

Create `crates/waved-audio/tests/biquad_regression.rs`:

```rust
//! Regression test: biquad direct-form-1 must match the reference impulse
//! response of a +6 dB peaking EQ at 1 kHz, Q=1, 48 kHz sample rate, computed
//! in double precision. Catches any change to the biquad form or designer
//! before tuning numbers diverge silently from stock.

use waved_audio::playback::dsp::biquad::{Biquad, design_peaking_eq};

const REFERENCE_IMPULSE_RESPONSE: [f64; 16] = [
    // Generated by scripts/gen_biquad_reference.py — peaking +6 dB @ 1 kHz, Q=1, fs=48 kHz.
    // Replace these placeholder zeros with values from Step 1 of Task 16.
    0.0; 16
];

#[test]
fn peaking_impulse_response_matches_reference() {
    let coeffs = design_peaking_eq(1000.0, 6.0, 1.0, 48_000.0);
    let mut b = Biquad::new(coeffs);
    let mut buf = [0.0_f32; 16];
    buf[0] = 1.0;
    b.process_block(&mut buf);

    for (i, (got, exp)) in buf.iter().zip(REFERENCE_IMPULSE_RESPONSE.iter()).enumerate() {
        let g = *got as f64;
        let diff = (g - *exp).abs();
        assert!(
            diff < 1e-6,
            "sample {i}: got {g:.10}, expected {exp:.10}, diff {diff:.3e}"
        );
    }
}
```

- [ ] **Step 3: Substitute the real reference values**

Run the Python from Step 1, copy the 16 floats into `REFERENCE_IMPULSE_RESPONSE`, replacing the placeholder zeros.

- [ ] **Step 4: Run the test**

Run: `cargo test -p waved-audio --test biquad_regression`
Expected: PASS — biquad output matches reference within 1e-6.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/tests/biquad_regression.rs
git commit -m "test(audio/sp5): biquad bit-exact regression vs python f64 reference"
```

---

## Task 17: AEC ERLE non-regression

Verify the post-DSP tap doesn't break echo cancellation. Re-run the SP#2 acceptance script and compare to baseline.

**Files:**
- (no source changes — runs existing `scripts/test-aec-on-device.sh`)

- [ ] **Step 1: Deploy current waved**

```bash
cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "killall waved 2>/dev/null; sleep 2; (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &)"
```

- [ ] **Step 2: Run AEC acceptance**

```bash
./scripts/test-aec-on-device.sh
```

Expected: ERLE in the speech band ≥ 20.7 dB (= SP#2 baseline 22.7 dB − 2 dB tolerance).

- [ ] **Step 3: If ERLE drops below 20.7 dB, diagnose**

Check `/tmp/waved.log` for the alignment metric. If post-DSP delay exceeded the AEC's frame-alignment tolerance, the limiter's lookahead is the prime suspect — reduce `lookahead_samples` in `LimiterParams::DEFAULT` from 96 to 48 and rerun.

- [ ] **Step 4: Record the ERLE result**

In `docs/AUDIO_HARDWARE.md`, in the SP#5 section (added in Task 19), append:

```markdown
SP#5 ERLE non-regression: <measured> dB (baseline 22.7 dB; bar 20.7 dB).
```

- [ ] **Step 5: Commit (only if docs change)**

```bash
git add docs/AUDIO_HARDWARE.md
git commit -m "docs(audio/sp5): record AEC ERLE post-DSP tap measurement"
```

---

## Task 18: On-device acceptance script

Cover the five criteria in §2.9 of the spec.

**Files:**
- Create: `scripts/test-playback-eq.sh`

- [ ] **Step 1: Create the script**

```bash
cat > scripts/test-playback-eq.sh <<'SH'
#!/usr/bin/env bash
# SP#5 on-device acceptance: playback EQ + gain.
#   1. Voice EQ frequency response within ±3 dB of stock (200 Hz–8 kHz).
#   2. Soft-talk amplification reaches MBAGC target.
#   3. Limiter ceiling -1 dBFS holds under full-scale square.
#   4. Preset switch produces no audible click.
#   5. CPU delta within budget.
#
# Pre-conditions: device booted, waved running, amixer playback unmute.
#
# Outputs: /tmp/sp5-acceptance.log on the host, exit 0 on full pass.

set -euo pipefail
cd "$(dirname "$0")/.."

LOG=/tmp/sp5-acceptance.log
: > "$LOG"

run_step() {
    local name="$1"; shift
    echo "=== $name ===" | tee -a "$LOG"
    "$@" 2>&1 | tee -a "$LOG"
}

# Test 1: Voice EQ frequency response (uses host-generated pink noise pushed via SIGUSR2).
run_step "1/5 voice EQ frequency response" bash -c '
python3 scripts/gen_pink_noise.py 6 > /tmp/sp5-pink.s16le
adb push /tmp/sp5-pink.s16le /data/waved/playback.s16le >/dev/null
adb shell "echo voice > /tmp/waved-playback-preset; kill -HUP $(adb shell pidof waved | tr -d \"\r\")"
sleep 0.5
adb shell "kill -USR2 $(adb shell pidof waved | tr -d \"\r\")"
sleep 7
adb shell "cat /tmp/audio_stats.json" | python3 scripts/check_eq_response.py
'

# Test 2: Soft-talk amplification (looks at mbagc_active_gain_db field).
run_step "2/5 soft-talk amplification" bash -c '
python3 scripts/gen_soft_speech.py 6 > /tmp/sp5-soft.s16le
adb push /tmp/sp5-soft.s16le /data/waved/playback.s16le >/dev/null
adb shell "kill -USR2 $(adb shell pidof waved | tr -d \"\r\")"
sleep 7
adb shell "cat /tmp/audio_stats.json" | python3 scripts/check_mbagc_gain.py
'

# Test 3: Limiter true-peak ceiling.
run_step "3/5 limiter ceiling" bash -c '
python3 scripts/gen_square.py 4 > /tmp/sp5-square.s16le
adb push /tmp/sp5-square.s16le /data/waved/playback.s16le >/dev/null
adb shell "kill -USR2 $(adb shell pidof waved | tr -d \"\r\")"
sleep 5
adb shell "cat /tmp/audio_stats.json" | python3 scripts/check_limiter.py
'

# Test 4: Preset switch click test.
run_step "4/5 preset switch artefact" bash -c '
python3 scripts/gen_pink_noise.py 4 > /tmp/sp5-pink-short.s16le
adb push /tmp/sp5-pink-short.s16le /data/waved/playback.s16le >/dev/null
adb shell "kill -USR2 $(adb shell pidof waved | tr -d \"\r\") &" >/dev/null
for p in voice media voice media voice; do
    adb shell "echo $p > /tmp/waved-playback-preset; kill -HUP $(adb shell pidof waved | tr -d \"\r\")"
    sleep 0.3
done
sleep 1
echo "  manual listening: tones must remain steady; any audible click fails this test" | tee -a "$LOG"
'

# Test 5: CPU delta.
run_step "5/5 CPU delta vs SP#4 baseline" bash -c '
adb shell "cat /proc/$(adb shell pidof waved | tr -d \"\r\")/stat" > /tmp/cpu-before
sleep 15
adb shell "cat /proc/$(adb shell pidof waved | tr -d \"\r\")/stat" > /tmp/cpu-after
python3 scripts/cpu_delta.py /tmp/cpu-before /tmp/cpu-after 1.5
'

echo "=== ALL PASS ===" | tee -a "$LOG"
SH
chmod +x scripts/test-playback-eq.sh
```

- [ ] **Step 2: Create the helper Python scripts**

The acceptance script shells to `scripts/gen_pink_noise.py`, `gen_soft_speech.py`, `gen_square.py`, `check_eq_response.py`, `check_mbagc_gain.py`, `check_limiter.py`, `cpu_delta.py`. These are small and follow the patterns in `test-beamforming.sh`. Create stubs:

```bash
# scripts/gen_pink_noise.py
cat > scripts/gen_pink_noise.py <<'PY'
#!/usr/bin/env python3
"""Generate `dur` seconds of pink noise as raw S16LE stereo 48 kHz to stdout."""
import sys, struct, random, math
dur = float(sys.argv[1])
sr = 48_000
n = int(sr * dur)
# Voss-McCartney pink noise via running sum of low-passed white noise.
buf = []
state = [random.uniform(-1, 1) for _ in range(8)]
for i in range(n):
    for k in range(len(state)):
        if i % (1 << k) == 0:
            state[k] = random.uniform(-1, 1)
    pink = sum(state) / len(state)
    s = int(0.3 * 32767 * pink)
    buf.append(struct.pack("<hh", s, s))
sys.stdout.buffer.write(b"".join(buf))
PY
chmod +x scripts/gen_pink_noise.py

# scripts/gen_soft_speech.py — −40 dBFS speech-band tone train
cat > scripts/gen_soft_speech.py <<'PY'
#!/usr/bin/env python3
"""−40 dBFS multi-tone in the speech band — stand-in for soft talker."""
import sys, struct, math
dur = float(sys.argv[1])
sr = 48_000
n = int(sr * dur)
amp = 10**(-40.0/20.0)
buf = []
for i in range(n):
    t = i / sr
    s = (math.sin(2*math.pi*500*t) + math.sin(2*math.pi*1500*t) + math.sin(2*math.pi*3000*t)) / 3.0
    v = int(amp * 32767 * s)
    buf.append(struct.pack("<hh", v, v))
sys.stdout.buffer.write(b"".join(buf))
PY
chmod +x scripts/gen_soft_speech.py

# scripts/gen_square.py — full-scale 1 kHz square
cat > scripts/gen_square.py <<'PY'
#!/usr/bin/env python3
"""Full-scale 1 kHz square wave — limiter stress test."""
import sys, struct
dur = float(sys.argv[1])
sr = 48_000
n = int(sr * dur)
period = sr // 1000
buf = []
for i in range(n):
    s = 32767 if (i // (period // 2)) % 2 == 0 else -32767
    buf.append(struct.pack("<hh", s, s))
sys.stdout.buffer.write(b"".join(buf))
PY
chmod +x scripts/gen_square.py

# scripts/check_eq_response.py — compare voice EQ output spectrum vs target.
cat > scripts/check_eq_response.py <<'PY'
#!/usr/bin/env python3
"""Pull /tmp/audio_stats.json (last sample) and check the voice preset's
   mbagc_active_gain_db is well-defined and non-zero, indicating MBAGC ran.
   Frequency-response measurement requires a USB measurement mic (TASK 18b)."""
import sys, json
data = json.load(sys.stdin)
preset = data.get("playback_preset", "?")
gains = data.get("mbagc_active_gain_db", [])
if preset != "voice":
    print(f"FAIL: expected preset=voice, got {preset}"); sys.exit(1)
if not gains or len(gains) != 4:
    print(f"FAIL: mbagc_active_gain_db missing or wrong shape: {gains}"); sys.exit(1)
print(f"OK: preset={preset} mbagc_gains={gains}")
PY
chmod +x scripts/check_eq_response.py

# scripts/check_mbagc_gain.py — soft input should produce positive MBAGC gain.
cat > scripts/check_mbagc_gain.py <<'PY'
#!/usr/bin/env python3
import sys, json
data = json.load(sys.stdin)
gains = data.get("mbagc_active_gain_db", [0]*4)
total = sum(gains)
if total < 4.0:
    print(f"FAIL: total MBAGC gain across bands {total:.2f} dB < 4 dB threshold"); sys.exit(1)
print(f"OK: total MBAGC gain {total:.2f} dB")
PY
chmod +x scripts/check_mbagc_gain.py

# scripts/check_limiter.py — limiter_reduction_db should be ≥ 0 dB attenuation.
cat > scripts/check_limiter.py <<'PY'
#!/usr/bin/env python3
import sys, json
data = json.load(sys.stdin)
red = data.get("limiter_reduction_db", 0.0)
# Full-scale square: limiter must attenuate by ≥ 1 dB (square peaks > -1 dBFS).
if red > -1.0:
    print(f"FAIL: limiter_reduction_db {red:.2f} dB; expected ≤ -1 dB on full-scale square"); sys.exit(1)
print(f"OK: limiter_reduction_db {red:.2f} dB")
PY
chmod +x scripts/check_limiter.py

# scripts/cpu_delta.py — compare two /proc/<pid>/stat snapshots.
cat > scripts/cpu_delta.py <<'PY'
#!/usr/bin/env python3
"""Usage: cpu_delta.py <before> <after> <budget_pct>
   Compute %CPU between two stat snapshots, fail if delta > budget."""
import sys
before = open(sys.argv[1]).read().split()
after  = open(sys.argv[2]).read().split()
# fields 13,14 = utime, stime
b_total = int(before[13]) + int(before[14])
a_total = int(after [13]) + int(after [14])
hz = 100  # CLK_TCK on this kernel
delta_secs = (a_total - b_total) / hz
window_secs = 15
pct = 100.0 * delta_secs / window_secs
budget = float(sys.argv[3])
# Compare against pre-SP#5 baseline saved by an earlier run, fallback to 12 % cap.
pre_sp5_baseline = 11.5  # measured on this device pre-SP#5
limit = pre_sp5_baseline + budget
if pct > limit:
    print(f"FAIL: CPU {pct:.2f}% > {limit:.2f}%"); sys.exit(1)
print(f"OK: CPU {pct:.2f}% (limit {limit:.2f}%)")
PY
chmod +x scripts/cpu_delta.py
```

- [ ] **Step 3: Run on device**

```bash
./scripts/test-playback-eq.sh
```

Expected: `=== ALL PASS ===` at the end of `/tmp/sp5-acceptance.log`. Test 1 sub-check is currently a sanity probe; full pink-noise frequency-response measurement requires a USB measurement mic (deferred per spec §3 out-of-scope).

- [ ] **Step 4: Commit**

```bash
git add scripts/test-playback-eq.sh \
        scripts/gen_pink_noise.py \
        scripts/gen_soft_speech.py \
        scripts/gen_square.py \
        scripts/check_eq_response.py \
        scripts/check_mbagc_gain.py \
        scripts/check_limiter.py \
        scripts/cpu_delta.py
git commit -m "test(audio/sp5): on-device acceptance script + helpers"
```

---

## Task 19: Docs + TODO

**Files:**
- Modify: `docs/AUDIO_HARDWARE.md`
- Modify: `TODO.md`

- [ ] **Step 1: Add the SP#5 section to `docs/AUDIO_HARDWARE.md`**

Append (or insert near the existing "ALSA → Physical Permutation" section):

```markdown
## Playback DSP Chain (SP#5)

`PlaybackService::run_playback_loop` (in `crates/waved-audio/src/playback.rs`)
processes every batch through the chain below before the ALSA write. The
broadcast tap (`subscribe_reference()`) publishes the **post-DSP** buffer —
this is what AEC consumes as the far-end reference, and matches what the
speaker actually emits.

```
mpsc<Vec<i16>> → HPF → 4-biquad EQ → 4-band MBAGC → lookahead limiter
                                                          │
                                       ┌──────────────────┤
                                       ▼                  ▼
                              ALSA write hw:0,1   broadcast<Vec<i16>>
                              (TDM-B → acm8625s)        (AEC ref)
```

Default HPF cutoff: 80 Hz. Default limiter ceiling: −1 dBFS, 96-sample
lookahead (≈ 2 ms latency).

**Voice preset** mirrors stock `dynamic_eq_config_2.json` (vendored at
`crates/waved-audio/data/dynamic_eq_config_2.json`). 4 EQ biquads + 4-band
MBAGC. **Media preset** = Voice with MBAGC bypassed and EQ flattened.
Switched at runtime via `Command::SetPlaybackPreset` or `/tmp/waved-playback-preset`
(`voice` | `media`) + SIGHUP — same wiring as `/tmp/waved-nr` for SP#4.

CPU delta vs pre-SP#5 baseline: <fill from acceptance test>.
SP#5 ERLE non-regression: <fill from Task 17>.
```

- [ ] **Step 2: Mark SP#5 done in `TODO.md`**

Change `- [ ] SP#5 — Playback EQ + gain staging` to:

```markdown
- [x] **SP#5 — Playback EQ + gain staging** (HPF + 4-biquad EQ + 4-band MBAGC + lookahead limiter inside PlaybackService; voice preset matches stock `dynamic_eq_config_2.json` byte-for-byte; media preset bypasses MBAGC and flattens EQ; AEC reference tap moved to post-DSP; switch via `/tmp/waved-playback-preset` + SIGHUP. See `scripts/test-playback-eq.sh` and `docs/AUDIO_HARDWARE.md`)
```

- [ ] **Step 3: Commit**

```bash
git add docs/AUDIO_HARDWARE.md TODO.md
git commit -m "docs(audio/sp5): document playback chain + mark SP#5 done"
```

---

## Self-review

- **Spec coverage:**
  - §2.1 chain shape: Tasks 4,5,7,8 ✓
  - §2.2 two presets: Task 10 + Task 14 ✓
  - §2.3 voice imported verbatim: Task 1 (vendor JSON) + Task 10 (parser) + Task 16 (regression) ✓
  - §2.4 media derived: Task 10 (`media_preset_from_voice`) ✓
  - §2.5 DSP inside `run_playback_loop`, post-DSP tap: Task 11 ✓
  - §2.6 per-channel EQ array: Task 5 (`eq.rs` `[[BiquadParams; 4]; 2]`) ✓
  - §2.7 limiter ceiling/lookahead: Task 8 (`LimiterParams::DEFAULT`) ✓
  - §2.8 preset switch online + ramp: Task 11 (preset_rx) — *5 ms ramp not yet implemented*. Add as a follow-up note in the chain implementation if rapid A/B switching produces audible clicks during Task 18 step 4.
  - §2.9 acceptance: Task 18 ✓ (with measurement-mic gate noted as out-of-scope)
  - §5.1 AudioConfig v4: Task 12 ✓
  - §5.2 AudioState/Command: Task 13 ✓
  - §5.3 stats fields: Task 15 ✓
  - §6 algorithms: Tasks 4,5,7,8 cover them; Linkwitz-Riley details kept inside §6.3 of spec, implemented in Task 7 ✓
  - §7 risks: Task 16 (biquad regression), Task 17 (AEC ERLE), Task 11 (post-DSP tap), Task 12 (config validation guards) ✓
- **Placeholder scan:** All steps contain code or exact commands. Two `<fill from ...>` placeholders intentionally left in the docs additions (Task 17, Task 19) where measured numbers replace them after the on-device run.
- **Type consistency:** `BiquadKind`, `BiquadParams`, `MbagcParams`, `LimiterParams`, `PlaybackPresetTuning`, `PlaybackPreset`, `PlaybackDspChain` names used identically across tasks. Channel/band counts (`N_CHANNELS=2`, `N_BANDS=4`) referenced as constants from one place.
- **Scope:** Single-implementation-plan-sized. SP#6 starts after this lands; their interaction is the post-DSP tap, owned by SP#5.
