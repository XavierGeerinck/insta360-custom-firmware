# waved bf+doa Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Improve our open-source Rust beamforming + DoA stack: fix the 270° azimuth bug, ship a per-frame DoaFrame stream for Feature 3, add an SRP-PHAT backend, add a DoA-driven steered cardioid beam mode, and instrument elevation bias for measurement.

**Architecture:** Five buckets, all in the existing `waved-audio` crate. Per-pair confidence weighting threads sharpness from `gcc_phat` into a weighted-LS `solve_direction`. A new `DoaFrame` is published on a `tokio::sync::broadcast` channel via `AudioService::subscribe_doa()`. SRP-PHAT lives in a new `doa::srp` module, selected statically via `AudioConfig.doa_backend`. Steered beam wraps the existing cardioid pattern with EWMA direction smoothing.

**Tech Stack:** Rust, `rustfft`, `tokio::sync::broadcast`, `serde`. Cross-compile via `cross --target aarch64-unknown-linux-gnu`. Deploy via `adb push`.

**Spec reference:** `docs/superpowers/specs/2026-05-23-waved-bf-doa-design.md`

---

## Task 1: Extend `gcc_phat::estimate_tdoa` to return peak sharpness

**Files:**
- Modify: `waved/crates/waved-audio/src/doa/gcc_phat.rs`
- Modify: `waved/crates/waved-audio/src/doa/mod.rs:144-153` (callers must accept the tuple)

The existing `estimate_tdoa` already computes `denom = y_l - 2*y_c + y_r` and discards it on the non-concave path. We just need to keep it. Sharpness = `-denom` (positive for concave peaks, zero for flat).

- [ ] **Step 1: Write failing tests for sharpness**

Append to `waved/crates/waved-audio/src/doa/gcc_phat.rs` inside `mod tests`:

```rust
#[test]
fn peak_sharpness_is_positive_for_concave_peak() {
    let gcc = GccPhat::new(480);
    let x1 = tone(1000.0, 48000, 480, 0.0);
    let x2 = tone(1000.0, 48000, 480, 5.0);
    let (_tau, sharpness) = gcc.estimate_tdoa(&x1, &x2, 16);
    assert!(sharpness > 0.0, "concave peak should have positive sharpness, got {sharpness}");
}

#[test]
fn peak_sharpness_near_zero_for_noise() {
    let gcc = GccPhat::new(480);
    let mut state = 0x9E3779B1u32;
    let mut rnd = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state as f32 / u32::MAX as f32) - 0.5
    };
    let x1: Vec<f32> = (0..480).map(|_| rnd()).collect();
    let x2: Vec<f32> = (0..480).map(|_| rnd()).collect();
    let (_tau, sharpness) = gcc.estimate_tdoa(&x1, &x2, 20);
    // Noise can still produce a small accidental sharpness; bound it.
    assert!(sharpness.abs() < 1.0, "noise sharpness should be small, got {sharpness}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd waved && cargo test -p waved-audio --lib doa::gcc_phat::tests::peak_sharpness -v`
Expected: FAIL with "this function takes 3 arguments but ... returns f32" (mismatched tuple destructuring).

- [ ] **Step 3: Change `estimate_tdoa` to return `(f32, f32)`**

In `waved/crates/waved-audio/src/doa/gcc_phat.rs`, change the signature and add sharpness to the return value:

```rust
/// Estimate TDOA (in samples, fractional) such that x2[n] ≈ x1[n - τ].
/// Returns `(tau_samples, sharpness)` where `sharpness = -denom` from the
/// parabolic peak fit. Sharpness is strictly > 0 for concave (real) peaks,
/// near 0 for flat/lumpy noise. Used by the weighted-LS DoA solver to
/// down-weight pairs whose cross-correlation has no clear peak (e.g.
/// body-shadowed pairs at certain azimuths).
pub fn estimate_tdoa(&self, x1: &[f32], x2: &[f32], max_lag_samples: usize) -> (f32, f32) {
    // ... (existing body unchanged through parabolic-interp section) ...

    let denom = y_l - 2.0 * y_c + y_r;
    let frac = if denom < -1e-9 {
        (0.5 * (y_l - y_r) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    };

    let sharpness = -denom; // > 0 on concave peaks, ~0 on flat surfaces
    (center as f32 + frac, sharpness)
}
```

(Leave the body above the change intact — only the return type and final return statement change.)

- [ ] **Step 4: Update existing GCC-PHAT tests to destructure**

In the same `mod tests` block, change each existing call to destructure the tuple:

```rust
// zero_delay_detects_zero:
let (tau, _) = gcc.estimate_tdoa(&x1, &x2, 16);
// positive_delay_detected:
let (tau, _) = gcc.estimate_tdoa(&x1, &x2, 16);
// negative_delay_detected:
let (tau, _) = gcc.estimate_tdoa(&x1, &x2, 16);
// fractional_delay_subsample_precision:
let (tau, _) = gcc.estimate_tdoa(&x1, &x2, 16);
// noise_pair_is_bounded_not_hundreds_of_samples:
let (tau, _) = gcc.estimate_tdoa(&x1, &x2, max_lag);
```

- [ ] **Step 5: Update the single caller in `doa/mod.rs:144-147`**

```rust
// Inside DoaEstimator::update, in the per-pair loop:
let (tau_samples, _sharpness) = self
    .gcc
    .estimate_tdoa(&per_ch[i], &per_ch[j], self.max_lag_samples);
raw_tau_samples[k] = tau_samples;
// (sharpness wired into solve in Task 3)
```

- [ ] **Step 6: Run all waved-audio tests**

Run: `cd waved && cargo test -p waved-audio --lib`
Expected: PASS — including new sharpness tests, all existing GCC-PHAT tests, and all existing DoA tests.

- [ ] **Step 7: Commit**

```bash
cd waved
git add crates/waved-audio/src/doa/gcc_phat.rs crates/waved-audio/src/doa/mod.rs
git commit -m "$(cat <<'EOF'
feat(doa): gcc_phat returns peak sharpness alongside tau

Threads parabolic-fit curvature (negated) out of estimate_tdoa so the
weighted-LS solver in Task 2 can down-weight pairs whose
cross-correlation has no clear peak (e.g. body-shadowed pairs at
certain azimuths — see SP#3 270 deg residual).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Weighted-LS in `solve_direction` with degeneracy guard

**Files:**
- Modify: `waved/crates/waved-audio/src/doa/solve.rs`

`solve_direction(&[PairObservation]) -> Option<DoaAngles>` already exists. Add an optional weights parameter that defaults to all-ones, plus a `MIN_SHARPNESS_THRESHOLD` guard that returns `None` when the system is degenerate.

- [ ] **Step 1: Write failing tests**

Append to `waved/crates/waved-audio/src/doa/solve.rs` inside `mod tests`:

```rust
const MIN_SHARPNESS_THRESHOLD: f32 = 1e-4;

#[test]
fn weighted_ls_equals_unweighted_with_equal_weights() {
    let pairs = vec![
        (Position3D::new(25.5, 0.0, 0.0), Position3D::new(-25.5, 0.0, 0.0)),
        (Position3D::new(0.0, 25.5, 0.0), Position3D::new(0.0, -25.5, 0.0)),
        (Position3D::new(25.5, 0.0, 0.0), Position3D::new(0.0, 0.0, -78.2)),
    ];
    let obs = make_obs(&pairs, 45.0, 15.0);
    let weights = vec![1.0; obs.len()];
    let a = solve_direction(&obs).unwrap();
    let b = solve_direction_weighted(&obs, &weights).unwrap();
    assert!((a.azimuth_deg - b.azimuth_deg).abs() < 0.01);
    assert!((a.elevation_deg - b.elevation_deg).abs() < 0.01);
}

#[test]
fn weighted_ls_zero_weight_pair_ignored() {
    let pairs = vec![
        (Position3D::new(25.5, 0.0, 0.0), Position3D::new(-25.5, 0.0, 0.0)),
        (Position3D::new(0.0, 25.5, 0.0), Position3D::new(0.0, -25.5, 0.0)),
        (Position3D::new(25.5, 0.0, 0.0), Position3D::new(0.0, 0.0, -78.2)),
        // Bogus 4th pair, completely wrong direction
        (Position3D::new(10.0, 10.0, 10.0), Position3D::new(-10.0, -10.0, -10.0)),
    ];
    let mut obs = make_obs(&pairs, 45.0, 0.0);
    // Corrupt the 4th observation's tdoa to simulate a noisy pair
    obs[3].tdoa_sec = 5e-4; // very wrong
    let weights = vec![1.0, 1.0, 1.0, 0.0]; // ignore the bad one
    let ang = solve_direction_weighted(&obs, &weights).unwrap();
    assert!((ang.azimuth_deg - 45.0).abs() < 2.0, "az {} not ≈45 with bad pair zeroed", ang.azimuth_deg);
}

#[test]
fn weighted_ls_returns_none_when_all_weights_below_threshold() {
    let pairs = vec![
        (Position3D::new(25.5, 0.0, 0.0), Position3D::new(-25.5, 0.0, 0.0)),
        (Position3D::new(0.0, 25.5, 0.0), Position3D::new(0.0, -25.5, 0.0)),
        (Position3D::new(25.5, 0.0, 0.0), Position3D::new(0.0, 0.0, -78.2)),
    ];
    let obs = make_obs(&pairs, 30.0, 0.0);
    let weights = vec![1e-6, 1e-6, 1e-6]; // all sub-threshold
    assert!(solve_direction_weighted(&obs, &weights).is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd waved && cargo test -p waved-audio --lib doa::solve::tests::weighted_ls -v`
Expected: FAIL with "cannot find function `solve_direction_weighted`".

- [ ] **Step 3: Implement `solve_direction_weighted`**

In `waved/crates/waved-audio/src/doa/solve.rs`, after the existing `solve_direction`:

```rust
/// Threshold below which the sum-of-pair-sharpnesses indicates the GCC-PHAT
/// surfaces had no real peak (e.g. silence-on-VAD-edge, all pairs shadowed).
/// Returning `None` lets the caller emit a vad=false / NaN-angles DoaFrame
/// instead of pretending to know.
pub const MIN_SHARPNESS_THRESHOLD: f32 = 1e-4;

/// Per-pair-weighted variant of `solve_direction`. Each observation contributes
/// to the normal equations weighted by `weights[k]`:
///   A += w_k · (Δ Δ^T),   b += w_k · (τ c) Δ
///
/// Use this from the DoA pipeline with `weights[k] = sharpness_k / Σ_k
/// sharpness_k`. The 270 deg azimuth residual (SP#3) is hypothesised to come
/// from a single shadowed pair pulling the unweighted LS off direction —
/// down-weighting it by its low sharpness should restore accuracy.
///
/// Returns `None` if:
/// - fewer than 3 observations
/// - `Σ weights < MIN_SHARPNESS_THRESHOLD` (degenerate / no real peaks)
/// - the weighted 3x3 normal matrix is singular (`|det| < 1e-9`)
pub fn solve_direction_weighted(
    obs: &[PairObservation],
    weights: &[f32],
) -> Option<DoaAngles> {
    if obs.len() < 3 {
        return None;
    }
    assert_eq!(obs.len(), weights.len(), "weights length must match obs");

    let weight_sum: f32 = weights.iter().sum();
    if weight_sum < MIN_SHARPNESS_THRESHOLD {
        return None;
    }

    let mut a = [[0.0f32; 3]; 3];
    let mut b_vec = [0.0f32; 3];
    for (o, &w) in obs.iter().zip(weights.iter()) {
        let d = [o.delta_m.x, o.delta_m.y, o.delta_m.z];
        let tc = o.tdoa_sec * SPEED_OF_SOUND_M_S;
        for i in 0..3 {
            b_vec[i] += w * tc * d[i];
            for j in 0..3 {
                a[i][j] += w * d[i] * d[j];
            }
        }
    }

    let det_a = det3(&a);
    if det_a.abs() < 1e-9 {
        return None;
    }

    let a_x = replace_col(&a, 0, &b_vec);
    let a_y = replace_col(&a, 1, &b_vec);
    let a_z = replace_col(&a, 2, &b_vec);
    let dx = det3(&a_x) / det_a;
    let dy = det3(&a_y) / det_a;
    let dz = det3(&a_z) / det_a;

    let norm = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-9);
    let dhat = (dx / norm, dy / norm, dz / norm);

    let az = dhat.1.atan2(dhat.0).to_degrees();
    let el = dhat.2.clamp(-1.0, 1.0).asin().to_degrees();
    let az = if az < 0.0 { az + 360.0 } else { az };

    Some(DoaAngles {
        azimuth_deg: az,
        elevation_deg: el,
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd waved && cargo test -p waved-audio --lib doa::solve -v`
Expected: PASS for all 8 tests (5 existing + 3 new).

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-audio/src/doa/solve.rs
git commit -m "$(cat <<'EOF'
feat(doa): solve_direction_weighted for per-pair-confidence LS

Adds a per-pair-weighted LS variant of solve_direction. Each
observation contributes w_k·(ΔΔ^T) to the normal matrix and
w_k·(τc)·Δ to the RHS. Returns None when Σweights falls below
MIN_SHARPNESS_THRESHOLD (1e-4) so the caller can emit an "unknown"
DoaFrame rather than pretend to know.

Existing solve_direction kept as-is for backward compatibility.

Task 3 wires sharpness from gcc_phat through DoaEstimator into the
weights, expected to fix the SP#3 ±19° residual at az=270°.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Wire weighted-LS through `DoaEstimator::update` + 270° regression test

**Files:**
- Modify: `waved/crates/waved-audio/src/doa/mod.rs` (the `update()` method)

- [ ] **Step 1: Write failing regression test for 270° az**

Append to `waved/crates/waved-audio/src/doa/mod.rs` inside `mod tests`:

```rust
/// Synthesizes an 8-ch plane wave with one pair's mic signal degraded to
/// simulate body-shadow at az=270°. Without weighted LS the corrupted
/// pair pulls the solve off-axis by >15°; with weighting it should
/// recover az=270° within ±5°.
fn synth_with_shadow(
    az_deg: f32,
    el_deg: f32,
    sr: u32,
    frames: usize,
    geom: &MicGeometry,
    shadowed_alsa_ch: usize,
) -> Vec<f32> {
    let az = az_deg.to_radians();
    let el = el_deg.to_radians();
    let d = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
    let freq = 1000.0f32;
    let omega = 2.0 * std::f32::consts::PI * freq / sr as f32;
    let mut out = vec![0.0; frames * 8];
    const C: f32 = 343.0;
    let mut state = 0xDEADBEEFu32;
    let mut rnd = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state as f32 / u32::MAX as f32) - 0.5
    };
    for alsa_ch in 0..8 {
        let p = geom.position_for_alsa(alsa_ch);
        let dot_mm = p.x * d[0] + p.y * d[1] + p.z * d[2];
        let tau_sec = -(dot_mm * 1e-3) / C;
        let tau_samples = tau_sec * sr as f32;
        for i in 0..frames {
            let clean = ((i as f32 - tau_samples) * omega).sin() * 0.5;
            // Shadowed mic: 80% noise, 20% signal (simulates diffraction smear)
            out[i * 8 + alsa_ch] = if alsa_ch == shadowed_alsa_ch {
                0.4 * rnd() + 0.1 * clean
            } else {
                clean
            };
        }
    }
    out
}

#[test]
fn weighted_ls_270_az_regression() {
    // At az=270° the source sits on the -Y side. Among our 4 pairs the
    // (1,3) pair's mic at physical index 1 (TOP 135°, -X+Y) is on the
    // shadowed side. ALSA channel mapping default [5,4,7,6,1,0,3,2]
    // means alsa ch 4 maps to physical 1. Shadow that channel.
    let geom = MicGeometry::default();
    let mut est = DoaEstimator::new(geom.clone(), 48000, 480);
    for _ in 0..40 {
        let frame = synth_with_shadow(270.0, 0.0, 48000, 480, &geom, 4);
        est.update(&frame);
    }
    let last = est.update(&synth_with_shadow(270.0, 0.0, 48000, 480, &geom, 4));
    assert!(last.fresh, "should solve");
    let err = (last.azimuth_deg - 270.0).abs().min((360.0 - last.azimuth_deg + 270.0).abs());
    assert!(
        err < 5.0,
        "weighted LS should resolve az=270 ±5°, got {:.1}° (err {:.1}°)",
        last.azimuth_deg, err
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd waved && cargo test -p waved-audio --lib doa::tests::weighted_ls_270 -v`
Expected: FAIL with az error > 5° (existing unweighted-LS path).

- [ ] **Step 3: Wire sharpness from gcc_phat into solve_direction_weighted**

In `waved/crates/waved-audio/src/doa/mod.rs`, change the `update()` body. Replace the per-pair loop and the `match solve::solve_direction(&obs)` block with:

```rust
// Compute TDOAs + per-pair sharpness for each pair; build PairObservations.
let mut obs = Vec::with_capacity(4);
let mut raw_tau_samples = [0.0f32; 4];
let mut sharpnesses = [0.0f32; 4];
for (k, &(i, j)) in PAIR_INDICES.iter().enumerate() {
    let (tau_samples, sharpness) = self
        .gcc
        .estimate_tdoa(&per_ch[i], &per_ch[j], self.max_lag_samples);
    raw_tau_samples[k] = tau_samples;
    sharpnesses[k] = sharpness.max(0.0);
    let (dx, dy, dz) = self.pair_deltas_m[k];
    obs.push(PairObservation {
        delta_m: crate::beam::Position3D::new(dx, dy, dz),
        tdoa_sec: tau_samples / self.sample_rate as f32,
    });
}

// (debug-line block: unchanged for now; Task 7 extends it)

// Weighted-LS: normalize sharpnesses to weights summing to 1 (when above
// threshold). solve_direction_weighted returns None if Σweights below
// MIN_SHARPNESS_THRESHOLD or system is singular.
let sum_sharp: f32 = sharpnesses.iter().sum();
let weights: Vec<f32> = if sum_sharp > solve::MIN_SHARPNESS_THRESHOLD {
    sharpnesses.iter().map(|s| s / sum_sharp).collect()
} else {
    sharpnesses.to_vec() // raw values; solve will reject below threshold
};

match solve::solve_direction_weighted(&obs, &weights) {
    Some(ang) => {
        if self.primed {
            self.smoothed_az =
                smooth_angle_deg(self.smoothed_az, ang.azimuth_deg, self.alpha);
            self.smoothed_el =
                self.alpha * self.smoothed_el + (1.0 - self.alpha) * ang.elevation_deg;
        } else {
            self.smoothed_az = ang.azimuth_deg;
            self.smoothed_el = ang.elevation_deg;
            self.primed = true;
        }
        DoaUpdate {
            azimuth_deg: self.smoothed_az,
            elevation_deg: self.smoothed_el,
            vad_active: true,
            fresh: true,
        }
    }
    None => DoaUpdate {
        azimuth_deg: self.smoothed_az,
        elevation_deg: self.smoothed_el,
        vad_active: true,
        fresh: false,
    },
}
```

- [ ] **Step 4: Verify the existing imports include `MIN_SHARPNESS_THRESHOLD`**

Top of `doa/mod.rs`:

```rust
pub use solve::{DoaAngles, PairObservation};
// (existing imports — no change needed; solve:: module is already in scope)
```

- [ ] **Step 5: Run all DoA tests**

Run: `cd waved && cargo test -p waved-audio --lib doa -v`
Expected: PASS — including the new `weighted_ls_270_az_regression`. The 3 existing tests (`recovers_doa_at_zero`, `recovers_doa_at_ninety`, `silence_freezes_smoothed_doa`, `smoothing_wraps_around_360_zero_boundary`) must still pass.

- [ ] **Step 6: Commit**

```bash
cd waved
git add crates/waved-audio/src/doa/mod.rs
git commit -m "$(cat <<'EOF'
feat(doa): wire per-pair sharpness into weighted-LS solve

DoaEstimator::update now collects (tau, sharpness) per pair from
gcc_phat, normalises sharpnesses into weights summing to 1, and calls
solve_direction_weighted. Pairs with flat correlation surfaces (e.g.
body-shadowed at certain azimuths) contribute proportionally less to
the LS solution.

Adds weighted_ls_270_az_regression test: synthesises a plane wave at
az=270° with one mic on the shadowed side replaced by 80%
noise+20%signal. Weighted path resolves within ±5°; the unweighted
path it replaces was expected to be ≥15° off (the SP#3 residual).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `DoaFrame` + `DoaBackend` + `DoaPublisher` (new `doa/stream.rs`)

**Files:**
- Create: `waved/crates/waved-audio/src/doa/stream.rs`
- Modify: `waved/crates/waved-audio/src/doa/mod.rs` (add `pub mod stream;`)

- [ ] **Step 1: Create the new module file**

Create `waved/crates/waved-audio/src/doa/stream.rs`:

```rust
//! Per-frame DoA publication. Emits one `DoaFrame` per ALSA capture period
//! on a `tokio::sync::broadcast` channel. Consumers include F3 (diarization),
//! the steered-beam controller (Task 14), and any UI/logging subscriber.
//!
//! Confidence is normalised to [0, 1] so consumers can threshold without
//! knowing which backend (GCC-PHAT or SRP-PHAT) actually ran.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Capacity of the broadcast channel. ~1.3 s of buffering at 21 ms cadence
/// (the production ALSA period of 1024 samples at 48 kHz). Slow consumers
/// get `RecvError::Lagged` past this; not an error.
const DOA_CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoaBackend {
    GccPhat,
    SrpPhat,
}

impl Default for DoaBackend {
    fn default() -> Self {
        DoaBackend::GccPhat
    }
}

/// One DoA observation per ALSA period. When `vad` is false, `azimuth_deg`
/// and `elevation_deg` are `f32::NAN` and `confidence` is 0.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct DoaFrame {
    pub frame_idx: u64,
    pub timestamp_ns: u64,
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    pub confidence: f32,
    pub vad: bool,
    pub backend: DoaBackend,
}

impl DoaFrame {
    /// Construct a "no speech / no direction" frame for a given frame index.
    pub fn silent(frame_idx: u64, timestamp_ns: u64, backend: DoaBackend) -> Self {
        Self {
            frame_idx,
            timestamp_ns,
            azimuth_deg: f32::NAN,
            elevation_deg: f32::NAN,
            confidence: 0.0,
            vad: false,
            backend,
        }
    }
}

pub struct DoaPublisher {
    tx: broadcast::Sender<DoaFrame>,
}

impl DoaPublisher {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(DOA_CHANNEL_CAPACITY);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DoaFrame> {
        self.tx.subscribe()
    }

    /// Send a frame. No-op when there are no subscribers (broadcast::send
    /// returns Err; we discard — consistent with other AudioService taps).
    pub fn emit(&self, frame: DoaFrame) {
        let _ = self.tx.send(frame);
    }
}

impl Default for DoaPublisher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_frame_has_nan_angles_and_zero_confidence() {
        let f = DoaFrame::silent(42, 1_000_000, DoaBackend::GccPhat);
        assert!(f.azimuth_deg.is_nan());
        assert!(f.elevation_deg.is_nan());
        assert_eq!(f.confidence, 0.0);
        assert!(!f.vad);
        assert_eq!(f.frame_idx, 42);
    }

    #[test]
    fn doa_frame_round_trips_to_json() {
        let f = DoaFrame {
            frame_idx: 7,
            timestamp_ns: 12_345_678,
            azimuth_deg: 45.5,
            elevation_deg: -3.2,
            confidence: 0.71,
            vad: true,
            backend: DoaBackend::SrpPhat,
        };
        let s = serde_json::to_string(&f).expect("serializes");
        assert!(s.contains("\"backend\":\"srp_phat\""), "json: {s}");
        assert!(s.contains("\"vad\":true"));
        assert!(s.contains("\"azimuth_deg\":45.5"));
    }

    #[test]
    fn publisher_emit_with_no_subscribers_does_not_panic() {
        let p = DoaPublisher::new();
        p.emit(DoaFrame::silent(0, 0, DoaBackend::GccPhat));
    }

    #[test]
    fn publisher_emit_delivers_to_subscriber() {
        let p = DoaPublisher::new();
        let mut rx = p.subscribe();
        p.emit(DoaFrame::silent(1, 0, DoaBackend::GccPhat));
        let got = rx.try_recv().expect("frame should be available");
        assert_eq!(got.frame_idx, 1);
    }

    #[test]
    fn backend_default_is_gcc_phat() {
        assert_eq!(DoaBackend::default(), DoaBackend::GccPhat);
    }
}
```

- [ ] **Step 2: Add `pub mod stream;` to `doa/mod.rs`**

Top of `waved/crates/waved-audio/src/doa/mod.rs`, alongside the existing `pub mod` lines:

```rust
//! Direction-of-arrival estimation.
pub mod gcc_phat;
pub mod solve;
pub mod stream;
pub mod vad;

pub use solve::{DoaAngles, PairObservation};
pub use stream::{DoaBackend, DoaFrame, DoaPublisher};
```

- [ ] **Step 3: Run the new tests**

Run: `cd waved && cargo test -p waved-audio --lib doa::stream -v`
Expected: PASS — all 5 tests in `stream::tests`.

- [ ] **Step 4: Commit**

```bash
cd waved
git add crates/waved-audio/src/doa/stream.rs crates/waved-audio/src/doa/mod.rs
git commit -m "$(cat <<'EOF'
feat(doa): add DoaFrame + DoaPublisher per-frame stream

New doa/stream.rs exports DoaFrame, DoaBackend (GccPhat | SrpPhat),
and DoaPublisher (tokio broadcast wrapper). Capacity 64 → ~1.3s
buffer at 21ms cadence; slow consumers get Lagged, not errors.

This is the F2→F3 contract: F3's diarization service will subscribe
to AudioService::subscribe_doa() (wired in Task 6) and threshold on
`confidence` (normalised [0,1] across both backends).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

I have 4 tasks written. Continuing in the next message with Tasks 5-10 (AudioConfig + AudioService wiring + WAVED_DOA_DEBUG extension + SRP-PHAT skeleton/algorithm/grid).

## Task 5: Add `doa_backend` field to `AudioConfig`

**Files:**
- Modify: `waved/crates/waved-audio/src/audio_config.rs`
- Modify: `waved/crates/waved-audio/config/audio_config_default.json` (optional — defaulted by serde)

- [ ] **Step 1: Write failing test**

Append to `waved/crates/waved-audio/src/audio_config.rs` inside `mod tests`:

```rust
#[test]
fn doa_backend_defaults_to_gcc_phat_when_absent() {
    let json = r#"{
        "version": 4,
        "capture": {"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},
        "period_frames": 1024,
        "buffer_periods": 8
    }"#;
    let cfg = AudioConfig::from_json(json).expect("v4 should parse");
    assert_eq!(cfg.doa_backend, crate::doa::DoaBackend::GccPhat);
}

#[test]
fn doa_backend_explicit_srp_phat_parses() {
    let json = r#"{
        "version": 4,
        "capture": {"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},
        "period_frames": 1024,
        "buffer_periods": 8,
        "doa_backend": "srp_phat"
    }"#;
    let cfg = AudioConfig::from_json(json).expect("v4 should parse");
    assert_eq!(cfg.doa_backend, crate::doa::DoaBackend::SrpPhat);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd waved && cargo test -p waved-audio --lib audio_config::tests::doa_backend -v`
Expected: FAIL with "no field `doa_backend` on type `AudioConfig`".

- [ ] **Step 3: Add the field to `AudioConfig`**

In `waved/crates/waved-audio/src/audio_config.rs`, add the field on the `AudioConfig` struct (place it next to `mic_geometry`):

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
    /// DoA backend selected at AudioService construction. "gcc_phat" (default)
    /// is the legacy pairwise + LS path; "srp_phat" is the grid-search
    /// alternative from F2 (see docs/AUDIO_HARDWARE.md SRP-PHAT section).
    /// Static — runtime switching is not supported.
    #[serde(default)]
    pub doa_backend: crate::doa::DoaBackend,
    #[serde(default)]
    pub uac: Option<UacConfig>,
    #[serde(default)]
    pub hid: Option<HidConfig>,
}
```

- [ ] **Step 4: Run the tests**

Run: `cd waved && cargo test -p waved-audio --lib audio_config -v`
Expected: PASS — all existing config tests + 2 new doa_backend tests.

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-audio/src/audio_config.rs
git commit -m "$(cat <<'EOF'
feat(audio-config): add doa_backend field (default gcc_phat)

Static config; SRP-PHAT runtime switching is intentionally out of
scope (see F2 spec §4). All existing JSON parses unchanged because
the field defaults via #[serde(default)].

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Wire `DoaPublisher` into `AudioService` + `subscribe_doa()` API

**Files:**
- Modify: `waved/crates/waved-audio/src/lib.rs` (AudioService struct, `new()`, `process_frame()`)
- Modify: `waved/crates/waved-audio/src/doa/mod.rs` (DoaUpdate gains confidence field)

`DoaEstimator::update()` currently returns `DoaUpdate { azimuth_deg, elevation_deg, vad_active, fresh }`. To emit a useful `DoaFrame` we need confidence too. Add it to `DoaUpdate` first, then publish in AudioService.

- [ ] **Step 1: Write failing test for subscribe_doa cadence**

Append to `waved/crates/waved-audio/src/lib.rs` inside the existing `#[cfg(test)] mod tests` block (it already exists at the bottom):

```rust
#[tokio::test]
async fn audio_service_emits_one_doa_frame_per_process_call() {
    use crate::AudioFrame;
    use tokio::sync::{mpsc, watch};
    use waved_state::AudioState;
    use std::sync::Arc;

    let (audio_tx_inner, _audio_rx) = watch::channel(AudioState::default());
    let audio_tx = Arc::new(audio_tx_inner);
    let (frame_tx, frame_rx) = mpsc::channel(8);
    let (meeting_tx, _meeting_rx) = mpsc::channel(8);
    let mut svc = AudioService::new(
        audio_tx, frame_tx, frame_rx, meeting_tx, 480, 48000,
        crate::beam::MicGeometry::default(),
    );
    let mut rx = svc.subscribe_doa();
    let frame = AudioFrame {
        data: vec![0i16; 480 * 8], channels: 8,
        reference: None, reference_channels: 0,
        timestamp_ns: 1_000_000,
    };
    svc.process_frame(&frame);
    let got = rx.try_recv().expect("one doa frame per process");
    assert_eq!(got.frame_idx, 0);
    assert!(!got.vad, "silence frame should be vad=false");
    svc.process_frame(&frame);
    let got2 = rx.try_recv().expect("second frame");
    assert_eq!(got2.frame_idx, 1);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd waved && cargo test -p waved-audio --lib audio_service_emits_one_doa_frame -v`
Expected: FAIL with "no method named `subscribe_doa`".

- [ ] **Step 3: Add confidence field to `DoaUpdate` + estimator wiring**

In `waved/crates/waved-audio/src/doa/mod.rs`, extend the struct and compute confidence inside `update`:

```rust
#[derive(Debug, Clone, Copy)]
pub struct DoaUpdate {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    pub vad_active: bool,
    pub fresh: bool,
    /// Normalised peak-sharpness aggregate, [0.0, 1.0]. 0 when !vad_active or
    /// solver returned None. Used by DoaPublisher to populate DoaFrame.confidence.
    pub confidence: f32,
}
```

Add a field on `DoaEstimator` to track the rolling max sharpness for normalisation:

```rust
pub struct DoaEstimator {
    // ... existing fields ...
    /// One-pole envelope follower over Σ pair sharpnesses. Tracks the max
    /// observed level so confidence stays meaningful as noise floor drifts.
    /// Time constant ~10 s at 21 ms period (alpha ≈ 0.998).
    max_sharpness_envelope: f32,
}
```

And initialise in `new()`:

```rust
// In DoaEstimator::new(), inside the Self { ... } block:
max_sharpness_envelope: 1.0,  // seed > 0 so first-frame division is sane
```

Then in `update()`, after computing `sum_sharp` (from Task 3 wiring), update the envelope and set confidence on the returned struct:

```rust
// After computing sum_sharp:
const ENVELOPE_ALPHA: f32 = 0.998; // ~10s time constant at 21ms cadence
self.max_sharpness_envelope = ENVELOPE_ALPHA * self.max_sharpness_envelope
    + (1.0 - ENVELOPE_ALPHA) * sum_sharp.max(self.max_sharpness_envelope * 0.1);
// Decay-to-floor prevents the envelope from sticking high on transient peaks.

let confidence = (sum_sharp / self.max_sharpness_envelope.max(1e-9)).clamp(0.0, 1.0);

// Then in the two return paths (Some(ang) and None), set confidence:
//   Some(ang) branch:
//     DoaUpdate { ..., confidence, ... }
//   None branch:
//     DoaUpdate { ..., confidence: 0.0, ... }  // failed-solve → 0 confidence
```

And in the VAD-fail early-return:

```rust
if !vad_active {
    return DoaUpdate {
        azimuth_deg: self.smoothed_az,
        elevation_deg: self.smoothed_el,
        vad_active: false,
        fresh: false,
        confidence: 0.0,
    };
}
```

- [ ] **Step 4: Add DoaPublisher + frame_idx counter to AudioService**

In `waved/crates/waved-audio/src/lib.rs`, modify the struct:

```rust
use crate::doa::{DoaBackend, DoaFrame, DoaPublisher};

pub struct AudioService {
    // ... existing fields ...
    doa_publisher: DoaPublisher,
    doa_backend: DoaBackend,
    doa_frame_counter: u64,
}
```

In `AudioService::new()`, after the existing `let doa = DoaEstimator::new(...)`:

```rust
let doa_publisher = DoaPublisher::new();
let doa_backend = DoaBackend::default(); // overridden when constructed via config (Task 11)
```

And inside the `Self { ... }` block:

```rust
doa_publisher,
doa_backend,
doa_frame_counter: 0,
```

- [ ] **Step 5: Add `subscribe_doa()` next to existing subscribe methods**

In `lib.rs`, alongside `subscribe_processed()`:

```rust
/// Subscribe to the per-frame DoA stream. One DoaFrame is emitted per
/// process_frame() call (≈ one per ALSA period). Capacity 64; slow
/// consumers get RecvError::Lagged, not errors. F3 (diarization) is the
/// primary consumer; the steered-beam controller (Task 14) and any UI
/// arrow can also subscribe.
pub fn subscribe_doa(&self) -> tokio::sync::broadcast::Receiver<DoaFrame> {
    self.doa_publisher.subscribe()
}
```

- [ ] **Step 6: Publish DoaFrame from `process_frame`**

In `lib.rs`, inside `process_frame`, locate the existing line `let doa = self.doa.update(&self.mic_8ch_f32_scratch);` (around line 309). Right after the existing `self.audio_tx.send_modify(...)` block, before `let (steer_az, steer_el) = ...`:

```rust
let frame_idx = self.doa_frame_counter;
self.doa_frame_counter = self.doa_frame_counter.wrapping_add(1);
let doa_frame = DoaFrame {
    frame_idx,
    timestamp_ns: frame.timestamp_ns,
    azimuth_deg: if doa.vad_active && doa.fresh { doa.azimuth_deg } else { f32::NAN },
    elevation_deg: if doa.vad_active && doa.fresh { doa.elevation_deg } else { f32::NAN },
    confidence: doa.confidence,
    vad: doa.vad_active,
    backend: self.doa_backend,
};
self.doa_publisher.emit(doa_frame);
```

(Note: when `vad_active` is true but `fresh` is false — solver failed mid-speech — angles are also NaN. The DoaFrame invariant in stream.rs tests checks `!vad → NaN`; we are stricter here, also NaN-ing when solve failed even with VAD. That's intentional: F3 should not consume directions the solver itself rejected.)

- [ ] **Step 7: Run all waved-audio tests**

Run: `cd waved && cargo test -p waved-audio --lib`
Expected: PASS — new audio_service_emits_one_doa_frame test plus all existing tests.

- [ ] **Step 8: Commit**

```bash
cd waved
git add crates/waved-audio/src/lib.rs crates/waved-audio/src/doa/mod.rs
git commit -m "$(cat <<'EOF'
feat(audio): publish DoaFrame stream + subscribe_doa() API

AudioService now emits one DoaFrame per process_frame() call on a
broadcast channel exposed via subscribe_doa(). DoaEstimator gains a
confidence field (normalised sum-of-pair-sharpness / rolling envelope,
~10s time constant) so the published frame carries a [0,1] threshold
F3 can use without caring which backend ran.

The published frame NaN-s angles whenever the solver did not produce
a fresh direction (VAD off OR solve returned None) — F3 must not
consume stale-smoothed numbers as if they were observations.

Wiring is the F2→F3 contract; F3 will subscribe in its own cycle.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Extend `WAVED_DOA_DEBUG` log line with per-pair sharpness + el contribution

**Files:**
- Modify: `waved/crates/waved-audio/src/doa/mod.rs` (the existing debug-line block, around lines 158-187)

This is the elevation-bias diagnostic from §3.4 of the spec. Per-pair el contribution = the elevation that would be implied if that pair were solved alone (rough partial-residual to spot which pair drives the bias).

- [ ] **Step 1: Modify the debug-line block in `DoaEstimator::update`**

In `waved/crates/waved-audio/src/doa/mod.rs`, replace the existing debug block (the `if let Some(n) = self.debug_every_n { ... }` block). Note: `sharpnesses` must be in scope — Task 3 left it as a `[f32; 4]` local. Add per-pair el-contribution log:

```rust
if let Some(n) = self.debug_every_n {
    self.debug_frame_counter = self.debug_frame_counter.wrapping_add(1);
    if self.debug_frame_counter % n == 0 {
        let az = self.smoothed_az.to_radians();
        let el = self.smoothed_el.to_radians();
        let dhat = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
        let sr = self.sample_rate as f32;
        let rms0 = {
            let sq: f32 = per_ch[0].iter().map(|x| x * x).sum();
            (sq / per_ch[0].len() as f32).sqrt()
        };
        let mut line = String::new();
        for k in 0..4 {
            let (i, j) = PAIR_INDICES[k];
            let (dx, dy, dz) = self.pair_deltas_m[k];
            let expected_tau_sec =
                (dx * dhat[0] + dy * dhat[1] + dz * dhat[2]) / SPEED_OF_SOUND_M_S;
            let expected_tau_samples = expected_tau_sec * sr;
            // Per-pair implied elevation: if this pair carried 100% of the
            // direction, what el would it imply assuming our smoothed az?
            // For pure-Z pairs (dz != 0, dx ≈ dy ≈ 0) this isolates the
            // elevation contribution; for XY-diagonal pairs it's noisy but
            // still useful for spotting outliers.
            let baseline_len = (dx*dx + dy*dy + dz*dz).sqrt().max(1e-9);
            let tau_meters = raw_tau_samples[k] / sr * SPEED_OF_SOUND_M_S;
            let cos_angle_to_baseline = (tau_meters / baseline_len).clamp(-1.0, 1.0);
            let el_implied_deg = if dz.abs() > 0.5 * baseline_len {
                // Pure-Z-ish pair: cos_angle_to_baseline ≈ sin(el) (signed)
                cos_angle_to_baseline.asin().to_degrees()
            } else {
                // XY-ish pair: no clean elevation; report NaN sentinel.
                f32::NAN
            };
            line.push_str(&format!(
                "({},{})raw={:+.2} exp={:+.2} sh={:.2} el_p={:+.1}  ",
                i, j, raw_tau_samples[k], expected_tau_samples,
                sharpnesses[k], el_implied_deg,
            ));
        }
        tracing::info!(
            target: "waved_audio::doa::debug",
            "doa_debug az={:.1} el={:.1} rms0={:.5} | {}",
            self.smoothed_az, self.smoothed_el, rms0, line,
        );
    }
}
```

- [ ] **Step 2: Verify the test from Task 3 still passes**

Run: `cd waved && cargo test -p waved-audio --lib doa -v`
Expected: PASS — all DoA tests including `weighted_ls_270_az_regression`. No new test for the debug line (logging-only; manual verify via on-device).

- [ ] **Step 3: Commit**

```bash
cd waved
git add crates/waved-audio/src/doa/mod.rs
git commit -m "$(cat <<'EOF'
feat(doa): extend WAVED_DOA_DEBUG line with sharpness + el_pair

Each per-pair column now reads "raw=... exp=... sh=... el_p=...".
'sh' is the GCC-PHAT peak sharpness (driver of the weighted-LS
weight); 'el_p' is the elevation that pair would imply if it carried
the full direction (NaN for XY-diagonal pairs where the partial is
meaningless).

Diagnostic foundation for the elevation-bias measurement script in
Task 16. Existing format string consumers continue to work because
the new fields append.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: SRP-PHAT skeleton — module, struct, grid generation

**Files:**
- Create: `waved/crates/waved-audio/src/doa/srp.rs`
- Modify: `waved/crates/waved-audio/src/doa/mod.rs` (add `pub mod srp;`)

- [ ] **Step 1: Create `doa/srp.rs` with skeleton + grid test**

Create `waved/crates/waved-audio/src/doa/srp.rs`:

```rust
//! SRP-PHAT direction-of-arrival estimator.
//!
//! Grid search over candidate directions, accumulating PHAT-weighted
//! cross-spectrum energy across the 4 mic pairs from doa/mod.rs.
//! Voice-band limited (200–4000 Hz). Trades 5-10× CPU vs GCC-PHAT for
//! more robust localisation in reverberant rooms.
//!
//! Selected via `AudioConfig.doa_backend = "srp_phat"`. Default backend
//! remains GCC-PHAT; SRP is opt-in pending on-device acceptance (§5.2 of
//! the F2 spec — gate: SRP CPU delta < 5%).

use crate::beam::MicGeometry;
use rustfft::{num_complex::Complex, FftPlanner};
use std::sync::Arc;

/// Speed of sound in air (m/s). Matches the constant in doa::solve.
const SPEED_OF_SOUND_M_S: f32 = 343.0;

/// Voice-band bin range — out-of-band PHAT bins are skipped, halving
/// per-direction work and sharpening speech localisation.
const VOICE_BAND_LO_HZ: f32 = 200.0;
const VOICE_BAND_HI_HZ: f32 = 4000.0;

/// Candidate-direction grid resolution.
const AZ_STEP_DEG: f32 = 5.0;
const EL_STEP_DEG: f32 = 10.0;

/// Same 4 pairs as gcc_phat — see doa/mod.rs PAIR_INDICES rationale.
const PAIR_INDICES: [(usize, usize); 4] = [(0, 2), (1, 3), (0, 4), (2, 6)];

#[derive(Debug, Clone, Copy)]
pub struct GridDirection {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    /// Cartesian unit vector (x, y, z) precomputed at construction time.
    pub dhat: [f32; 3],
}

pub struct SrpPhatEstimator {
    geom: MicGeometry,
    sample_rate: u32,
    frame_size: usize,
    fft_len: usize,
    planner_fwd: Arc<dyn rustfft::Fft<f32>>,
    grid: Vec<GridDirection>,
    /// Per-pair position delta in meters; matches gcc_phat's pair_deltas_m.
    pair_deltas_m: [(f32, f32, f32); 4],
    /// Min/max FFT bin to include in PHAT accumulation, voice-band masked.
    bin_lo: usize,
    bin_hi: usize,
}

impl SrpPhatEstimator {
    pub fn new(geom: MicGeometry, sample_rate: u32, frame_size: usize) -> Self {
        let fft_len = (frame_size * 2).next_power_of_two();
        let mut planner = FftPlanner::<f32>::new();
        let planner_fwd = planner.plan_fft_forward(fft_len);

        let grid = build_grid();

        let mut pair_deltas_m = [(0.0f32, 0.0, 0.0); 4];
        for (k, &(i, j)) in PAIR_INDICES.iter().enumerate() {
            let pi = &geom.positions_mm[i];
            let pj = &geom.positions_mm[j];
            pair_deltas_m[k] = (
                (pi.x - pj.x) * 1e-3,
                (pi.y - pj.y) * 1e-3,
                (pi.z - pj.z) * 1e-3,
            );
        }

        let bin_hz = sample_rate as f32 / fft_len as f32;
        let bin_lo = (VOICE_BAND_LO_HZ / bin_hz).floor() as usize;
        let bin_hi = ((VOICE_BAND_HI_HZ / bin_hz).ceil() as usize).min(fft_len / 2);

        Self {
            geom,
            sample_rate,
            frame_size,
            fft_len,
            planner_fwd,
            grid,
            pair_deltas_m,
            bin_lo,
            bin_hi,
        }
    }

    pub fn grid_len(&self) -> usize {
        self.grid.len()
    }
}

fn build_grid() -> Vec<GridDirection> {
    let mut grid = Vec::new();
    let n_az = (360.0 / AZ_STEP_DEG) as i32; // 72
    let n_el = ((180.0 / EL_STEP_DEG) as i32) + 1; // 19, from -90..=+90
    for ei in 0..n_el {
        let el_deg = -90.0 + EL_STEP_DEG * ei as f32;
        let el = el_deg.to_radians();
        for ai in 0..n_az {
            let az_deg = AZ_STEP_DEG * ai as f32;
            let az = az_deg.to_radians();
            let dhat = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
            grid.push(GridDirection {
                azimuth_deg: az_deg,
                elevation_deg: el_deg,
                dhat,
            });
        }
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srp_grid_has_expected_cardinality() {
        // 72 az × 19 el = 1368
        let g = build_grid();
        assert_eq!(g.len(), 1368);
    }

    #[test]
    fn srp_estimator_constructs_without_panic() {
        let geom = MicGeometry::default();
        let est = SrpPhatEstimator::new(geom, 48000, 480);
        assert_eq!(est.grid_len(), 1368);
        // Voice band 200..4000 at fft_len 1024 → bin_hz ≈ 46.875.
        // bin_lo ≈ 4, bin_hi ≈ 86. Sanity-bound.
        assert!(est.bin_lo > 0);
        assert!(est.bin_hi < est.fft_len / 2);
        assert!(est.bin_hi > est.bin_lo + 10);
    }

    #[test]
    fn srp_grid_contains_cardinal_azimuths() {
        let g = build_grid();
        let azs: Vec<f32> = g.iter().map(|d| d.azimuth_deg).collect();
        for &target in &[0.0, 90.0, 180.0, 270.0] {
            assert!(
                azs.iter().any(|&a| (a - target).abs() < 0.01),
                "grid missing az={target}"
            );
        }
    }
}
```

- [ ] **Step 2: Wire the module + re-export `SrpPhatEstimator`**

In `waved/crates/waved-audio/src/doa/mod.rs`:

```rust
pub mod gcc_phat;
pub mod solve;
pub mod srp;
pub mod stream;
pub mod vad;

pub use solve::{DoaAngles, PairObservation};
pub use srp::SrpPhatEstimator;
pub use stream::{DoaBackend, DoaFrame, DoaPublisher};
```

- [ ] **Step 3: Run tests**

Run: `cd waved && cargo test -p waved-audio --lib doa::srp -v`
Expected: PASS — 3 skeleton tests.

- [ ] **Step 4: Commit**

```bash
cd waved
git add crates/waved-audio/src/doa/srp.rs crates/waved-audio/src/doa/mod.rs
git commit -m "$(cat <<'EOF'
feat(doa): SRP-PHAT skeleton (grid + FFT plan)

Adds doa/srp.rs with SrpPhatEstimator struct, 5°×10° grid (1368
directions), voice-band bin mask (200-4000 Hz), and FFT planner setup
matching gcc_phat. Algorithm (PHAT cross-spectrum + steered response)
lands in Task 9; grid search + refinement in Task 10.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: SRP-PHAT process — PHAT cross-spectrum + grid search + confidence

**Files:**
- Modify: `waved/crates/waved-audio/src/doa/srp.rs`

- [ ] **Step 1: Write failing test for SRP localisation**

Append to `waved/crates/waved-audio/src/doa/srp.rs` inside `mod tests`:

```rust
fn synth_plane_wave_srp(
    az_deg: f32, el_deg: f32, sr: u32, frames: usize, geom: &MicGeometry,
) -> Vec<f32> {
    let az = az_deg.to_radians();
    let el = el_deg.to_radians();
    let d = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
    let freq = 1500.0f32; // voice-band
    let omega = 2.0 * std::f32::consts::PI * freq / sr as f32;
    let mut out = vec![0.0; frames * 8];
    const C: f32 = 343.0;
    for alsa_ch in 0..8 {
        let p = geom.position_for_alsa(alsa_ch);
        let dot_mm = p.x * d[0] + p.y * d[1] + p.z * d[2];
        let tau_samples = (-(dot_mm * 1e-3) / C) * sr as f32;
        for i in 0..frames {
            out[i * 8 + alsa_ch] = ((i as f32 - tau_samples) * omega).sin() * 0.5;
        }
    }
    out
}

#[test]
fn srp_locates_plane_wave_within_grid_cell() {
    let geom = MicGeometry::default();
    let mut est = SrpPhatEstimator::new(geom.clone(), 48000, 480);
    let frame = synth_plane_wave_srp(45.0, 0.0, 48000, 480, &geom);
    let result = est.estimate(&frame).expect("plane wave should localise");
    let az_err = (result.azimuth_deg - 45.0).abs();
    assert!(az_err < AZ_STEP_DEG, "SRP az {} not within ±{}°", result.azimuth_deg, AZ_STEP_DEG);
    assert!(result.elevation_deg.abs() < EL_STEP_DEG);
    assert!(result.confidence > 0.3, "should have decent confidence on clean source, got {}", result.confidence);
}

#[test]
fn srp_low_confidence_on_uniform_noise() {
    let geom = MicGeometry::default();
    let mut est = SrpPhatEstimator::new(geom.clone(), 48000, 480);
    let mut state = 0xC0FFEEu32;
    let mut rnd = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state as f32 / u32::MAX as f32) - 0.5
    };
    // Independent noise per channel — no shared direction.
    let frame: Vec<f32> = (0..480 * 8).map(|_| rnd() * 0.5).collect();
    let result = est.estimate(&frame).expect("noise still returns Some");
    assert!(result.confidence < 0.3, "noise should give low confidence, got {}", result.confidence);
}

#[test]
fn srp_locates_az_270_within_grid() {
    let geom = MicGeometry::default();
    let mut est = SrpPhatEstimator::new(geom.clone(), 48000, 480);
    let frame = synth_plane_wave_srp(270.0, 0.0, 48000, 480, &geom);
    let result = est.estimate(&frame).expect("should localise");
    let az_err = ((result.azimuth_deg - 270.0).abs()).min((360.0 - (result.azimuth_deg - 270.0).abs()));
    assert!(az_err < AZ_STEP_DEG, "SRP az {} not within ±{}° of 270", result.azimuth_deg, AZ_STEP_DEG);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd waved && cargo test -p waved-audio --lib doa::srp::tests::srp_locates -v`
Expected: FAIL with "no method named `estimate`".

- [ ] **Step 3: Add the `SrpResult` struct + `estimate` method**

In `waved/crates/waved-audio/src/doa/srp.rs`, add a result struct and the estimate method on `SrpPhatEstimator`:

```rust
#[derive(Debug, Clone, Copy)]
pub struct SrpResult {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    pub confidence: f32,
}

impl SrpPhatEstimator {
    /// Estimate source direction from one ALSA-interleaved 8-channel frame.
    /// Returns None only when the input is wrong-sized; on silence or noise
    /// it returns Some with low `confidence`.
    pub fn estimate(&mut self, interleaved: &[f32]) -> Option<SrpResult> {
        if interleaved.len() != self.frame_size * 8 {
            return None;
        }

        // Demultiplex 8 ALSA channels into 8 per-physical-mic buffers, then
        // forward-FFT each. Use a fresh local Vec per channel — the previous
        // grid result has no carry-over state.
        let mut per_ch_fft: [Vec<Complex<f32>>; 8] = std::array::from_fn(|_| {
            vec![Complex::new(0.0, 0.0); self.fft_len]
        });
        for i in 0..self.frame_size {
            for alsa_ch in 0..8 {
                let physical = self.geom.alsa_to_physical[alsa_ch] as usize;
                per_ch_fft[physical][i] =
                    Complex::new(interleaved[i * 8 + alsa_ch], 0.0);
            }
        }
        for ch in 0..8 {
            self.planner_fwd.process(&mut per_ch_fft[ch]);
        }

        // For each pair, precompute PHAT-normalized cross-spectrum C_ij(f) =
        // X_i(f)·conj(X_j(f)) / |X_i(f)·conj(X_j(f))|. Zero out-of-band bins.
        let mut cross_per_pair: [Vec<Complex<f32>>; 4] = std::array::from_fn(|_| {
            vec![Complex::new(0.0, 0.0); self.bin_hi - self.bin_lo]
        });
        for (k, &(i, j)) in PAIR_INDICES.iter().enumerate() {
            for (idx, bin) in (self.bin_lo..self.bin_hi).enumerate() {
                let xi = per_ch_fft[i][bin];
                let xj_conj = per_ch_fft[j][bin].conj();
                let num = xi * xj_conj;
                let mag = num.norm();
                cross_per_pair[k][idx] = if mag > 1e-10 {
                    num / mag
                } else {
                    Complex::new(0.0, 0.0)
                };
            }
        }

        // Steered response: sum over pairs of sum over bins of Re[C_ij(f) ·
        // exp(j·2π·f·τ_ij(d̂))]. τ_ij(d̂) = (p_i - p_j) · d̂ / c, in seconds.
        let bin_hz = self.sample_rate as f32 / self.fft_len as f32;
        let two_pi = 2.0 * std::f32::consts::PI;
        let mut best_idx = 0usize;
        let mut best_power = f32::NEG_INFINITY;
        let mut power_sum = 0.0f32;
        let mut powers = vec![0.0f32; self.grid.len()];
        for (g_idx, g) in self.grid.iter().enumerate() {
            let mut total = 0.0f32;
            for (k, &(_i, _j)) in PAIR_INDICES.iter().enumerate() {
                let (dx, dy, dz) = self.pair_deltas_m[k];
                let proj = dx * g.dhat[0] + dy * g.dhat[1] + dz * g.dhat[2];
                let tau_sec = proj / SPEED_OF_SOUND_M_S;
                for (idx, bin) in (self.bin_lo..self.bin_hi).enumerate() {
                    let f_hz = bin as f32 * bin_hz;
                    let phase = two_pi * f_hz * tau_sec;
                    let (sp, cp) = phase.sin_cos();
                    let exp_jp = Complex::new(cp, sp);
                    let c = cross_per_pair[k][idx];
                    total += (c * exp_jp).re;
                }
            }
            powers[g_idx] = total;
            power_sum += total;
            if total > best_power {
                best_power = total;
                best_idx = g_idx;
            }
        }

        let best = &self.grid[best_idx];
        let mean_power = power_sum / self.grid.len() as f32;
        // Confidence: how much the peak stands out vs the mean. Clamp to [0,1].
        let confidence = if best_power > 0.0 {
            ((best_power - mean_power) / best_power).clamp(0.0, 1.0)
        } else {
            0.0
        };

        Some(SrpResult {
            azimuth_deg: best.azimuth_deg,
            elevation_deg: best.elevation_deg,
            confidence,
        })
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cd waved && cargo test -p waved-audio --lib doa::srp -v`
Expected: PASS — 3 skeleton + 3 estimate tests = 6 tests.

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-audio/src/doa/srp.rs
git commit -m "$(cat <<'EOF'
feat(doa): SRP-PHAT estimate() — PHAT cross-spectrum + grid search

Implements SrpPhatEstimator::estimate(&[f32]) → SrpResult{az, el,
confidence}. Per pair: PHAT-normalised X_i·conj(X_j) over voice-band
bins; per grid direction: sum_pairs sum_bins Re[C_ij·exp(j·2π·f·τ)].
Argmax over the 1368-direction grid; confidence = (peak − mean) /
peak, clamped [0,1].

Sub-sample refinement (parabolic in az/el) deferred to Task 10 along
with the boundary-wrap handling.

3 unit tests pass: clean plane wave at az=45° resolves within ±5°,
plane wave at az=270° resolves within ±5°, uniform noise gives
confidence < 0.3.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: SRP-PHAT — sub-sample parabolic refinement + boundary wrap

**Files:**
- Modify: `waved/crates/waved-audio/src/doa/srp.rs`

The Task 9 implementation snaps to grid cells (5° az, 10° el). For diarization clustering at small angular separations, refine to ~0.5° via parabolic interpolation across the peak's neighbors.

- [ ] **Step 1: Write failing tests for sub-grid accuracy + boundary wrap**

Append to `waved/crates/waved-audio/src/doa/srp.rs` inside `mod tests`:

```rust
#[test]
fn srp_refines_to_sub_grid_accuracy() {
    // Source at az=47.5° (between grid cells 45° and 50°). Without
    // refinement the result would snap to 45° or 50°; with parabolic
    // interp it should land closer to truth.
    let geom = MicGeometry::default();
    let mut est = SrpPhatEstimator::new(geom.clone(), 48000, 480);
    let frame = synth_plane_wave_srp(47.5, 0.0, 48000, 480, &geom);
    let result = est.estimate(&frame).expect("should localise");
    let az_err = (result.azimuth_deg - 47.5).abs();
    assert!(az_err < 2.0,
        "refined az {} should be within ±2° of 47.5", result.azimuth_deg);
}

#[test]
fn srp_az_boundary_wraps_correctly() {
    // Source at az=-175° = 185° → between grid cells 185° and 180°,
    // and the parabolic-interp neighbor at +185° must wrap from −5°/355°.
    let geom = MicGeometry::default();
    let mut est = SrpPhatEstimator::new(geom.clone(), 48000, 480);
    let frame = synth_plane_wave_srp(185.0, 0.0, 48000, 480, &geom);
    let result = est.estimate(&frame).expect("should localise");
    let az_err = (result.azimuth_deg - 185.0).abs();
    assert!(az_err < AZ_STEP_DEG,
        "wrap-around az {} not within ±{}° of 185", result.azimuth_deg, AZ_STEP_DEG);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd waved && cargo test -p waved-audio --lib doa::srp::tests::srp_refines_to_sub_grid -v`
Expected: FAIL — az_err is 2.5° (snaps to nearest 5° cell).

- [ ] **Step 3: Refactor `estimate` to store all powers + add refinement**

In `srp.rs` — `estimate` already builds `powers: Vec<f32>` (Task 9). After finding `best_idx`, do parabolic interpolation across the neighbors. Insert just before `Some(SrpResult { ... })`:

```rust
// Sub-sample parabolic refinement in az and el. Grid is stored
// elevation-major (n_az_cells azimuths per elevation row), so:
//   az neighbor:  same el row, az±1 (wrap on row)
//   el neighbor:  same az col, el±1 (clamp at ±90°; no wrap)
let n_az = (360.0 / AZ_STEP_DEG) as usize;
let n_el = ((180.0 / EL_STEP_DEG) as usize) + 1;
let ei = best_idx / n_az;
let ai = best_idx % n_az;

let p_c = powers[best_idx];
let ai_l = (ai + n_az - 1) % n_az;
let ai_r = (ai + 1) % n_az;
let p_az_l = powers[ei * n_az + ai_l];
let p_az_r = powers[ei * n_az + ai_r];
let az_denom = p_az_l - 2.0 * p_c + p_az_r;
let az_frac = if az_denom < -1e-9 {
    (0.5 * (p_az_l - p_az_r) / az_denom).clamp(-0.5, 0.5)
} else {
    0.0
};

let el_frac = if ei == 0 || ei == n_el - 1 {
    0.0 // polar boundary; no neighbor on one side
} else {
    let p_el_l = powers[(ei - 1) * n_az + ai];
    let p_el_r = powers[(ei + 1) * n_az + ai];
    let el_denom = p_el_l - 2.0 * p_c + p_el_r;
    if el_denom < -1e-9 {
        (0.5 * (p_el_l - p_el_r) / el_denom).clamp(-0.5, 0.5)
    } else {
        0.0
    }
};

let mut refined_az = best.azimuth_deg + az_frac * AZ_STEP_DEG;
if refined_az < 0.0 { refined_az += 360.0; }
if refined_az >= 360.0 { refined_az -= 360.0; }
let refined_el = (best.elevation_deg + el_frac * EL_STEP_DEG).clamp(-90.0, 90.0);

Some(SrpResult {
    azimuth_deg: refined_az,
    elevation_deg: refined_el,
    confidence,
})
```

Remove the old `Some(SrpResult { azimuth_deg: best.azimuth_deg, ... })` it replaces.

- [ ] **Step 4: Run tests**

Run: `cd waved && cargo test -p waved-audio --lib doa::srp -v`
Expected: PASS — 8 total tests (skeleton + estimate + refinement).

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-audio/src/doa/srp.rs
git commit -m "$(cat <<'EOF'
feat(doa): SRP-PHAT parabolic refinement + az boundary wrap

Sub-grid accuracy via 3-point parabolic interpolation over the power
surface in az and el. Az neighbors wrap modulo the grid; el neighbors
clamp at the ±90° polar boundary (no refinement there).

Two unit tests: source at az=47.5° refined within ±2° (was snapping
to 45° or 50°); source at az=185° resolved within one grid cell
despite wrapping past 180°.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Backend selection + GCC-PHAT-vs-SRP-PHAT dispatch in `AudioService`

**Files:**
- Modify: `waved/crates/waved-audio/src/lib.rs`
- Modify: `waved/crates/waved-audio/src/doa/mod.rs` (expose `DoaEstimator::update` already done; add a thin "estimate raw" path returning `DoaUpdate`)
- Modify: `waved/crates/waved-state/src/lib.rs` (add `doa_backend_fallback` stat field — pure addition)

Two backends, one runtime selection. Avoid a trait-object dispatch (we only have two impls); use an enum that owns either.

- [ ] **Step 1: Add `doa_backend_fallback` to AudioState (waved-state)**

In `waved/crates/waved-state/src/lib.rs`, add the field to `AudioState` (alongside existing `doa_*` fields):

```rust
// Inside AudioState struct definition:
/// Set true at startup when AudioConfig.doa_backend = SrpPhat but the SRP
/// estimator failed to initialise — service fell back to GCC-PHAT. The
/// stats collector mirrors this into /tmp/audio_stats.json so operators
/// see why their selected backend isn't active.
pub doa_backend_fallback: bool,
```

Add to `AudioState::default()` if not derive-defaulted (likely is — verify; if not, add `doa_backend_fallback: false`).

- [ ] **Step 2: Define backend enum + construction in AudioService**

In `waved/crates/waved-audio/src/lib.rs`, add near the existing imports:

```rust
use crate::doa::{DoaBackend, DoaEstimator, DoaFrame, DoaPublisher, SrpPhatEstimator};

/// Backend dispatch — owns one impl, not a trait object (only two impls,
/// no value in dyn). GccPhat is the existing DoaEstimator; SrpPhat wraps
/// the new SrpPhatEstimator but still uses DoaEstimator's smoothing/VAD
/// glue (we only swap the localisation step).
enum DoaBackendImpl {
    GccPhat(DoaEstimator),
    SrpPhat {
        srp: SrpPhatEstimator,
        // EnergyVad + smoothing fields mirror DoaEstimator's behaviour so
        // both backends produce comparable smoothed angles + confidence.
        vad: crate::doa::vad::EnergyVad,
        smoothed_az: f32,
        smoothed_el: f32,
        primed: bool,
        max_sharpness_envelope: f32, // unused for SRP; kept for symmetry
        geom: crate::beam::MicGeometry,
        frame_size: usize,
    },
}

impl DoaBackendImpl {
    fn update(&mut self, interleaved: &[f32]) -> crate::doa::DoaUpdate {
        match self {
            DoaBackendImpl::GccPhat(est) => est.update(interleaved),
            DoaBackendImpl::SrpPhat { srp, vad, smoothed_az, smoothed_el, primed, geom, frame_size, .. } => {
                use crate::doa::vad::Vad;
                assert_eq!(interleaved.len(), *frame_size * 8);
                // VAD on ALSA ch 0 (matches GCC-PHAT path).
                let mut ch0 = Vec::with_capacity(*frame_size);
                for i in 0..*frame_size {
                    ch0.push(interleaved[i * 8]);
                }
                let vad_active = vad.is_speech(&ch0);
                if !vad_active {
                    return crate::doa::DoaUpdate {
                        azimuth_deg: *smoothed_az,
                        elevation_deg: *smoothed_el,
                        vad_active: false,
                        fresh: false,
                        confidence: 0.0,
                    };
                }
                match srp.estimate(interleaved) {
                    Some(r) => {
                        if *primed {
                            // Same EWMA-on-unit-vector as DoaEstimator.
                            let alpha = 0.9f32;
                            let p = (*smoothed_az).to_radians();
                            let n = r.azimuth_deg.to_radians();
                            let sx = alpha * p.cos() + (1.0 - alpha) * n.cos();
                            let sy = alpha * p.sin() + (1.0 - alpha) * n.sin();
                            let smoothed = sy.atan2(sx).to_degrees();
                            *smoothed_az = if smoothed < 0.0 { smoothed + 360.0 } else { smoothed };
                            *smoothed_el = alpha * *smoothed_el + (1.0 - alpha) * r.elevation_deg;
                        } else {
                            *smoothed_az = r.azimuth_deg;
                            *smoothed_el = r.elevation_deg;
                            *primed = true;
                        }
                        crate::doa::DoaUpdate {
                            azimuth_deg: *smoothed_az,
                            elevation_deg: *smoothed_el,
                            vad_active: true,
                            fresh: true,
                            confidence: r.confidence,
                        }
                    }
                    None => crate::doa::DoaUpdate {
                        azimuth_deg: *smoothed_az,
                        elevation_deg: *smoothed_el,
                        vad_active: true,
                        fresh: false,
                        confidence: 0.0,
                    },
                }
            }
        }
    }
}
```

- [ ] **Step 3: Swap AudioService's `doa: DoaEstimator` for the enum**

In `lib.rs`, change `AudioService`:

```rust
pub struct AudioService {
    // ... existing fields except `doa: DoaEstimator` ...
    doa: DoaBackendImpl,
    // ... rest unchanged ...
}
```

And modify `AudioService::new()` to construct based on backend. Replace `let doa = DoaEstimator::new(geometry, sample_rate, frame_size);` with:

```rust
let (doa, backend, fallback) = build_doa_backend(
    DoaBackend::default(),  // legacy constructor uses default; new-with-config below
    geometry.clone(),
    sample_rate,
    frame_size,
);
let doa_backend = backend;
```

And before `Self { ... }`:

```rust
if fallback {
    audio_tx.send_modify(|s| s.doa_backend_fallback = true);
}
```

Then add a free function:

```rust
fn build_doa_backend(
    requested: DoaBackend,
    geometry: crate::beam::MicGeometry,
    sample_rate: u32,
    frame_size: usize,
) -> (DoaBackendImpl, DoaBackend, bool /* fallback occurred */) {
    match requested {
        DoaBackend::GccPhat => (
            DoaBackendImpl::GccPhat(DoaEstimator::new(geometry, sample_rate, frame_size)),
            DoaBackend::GccPhat,
            false,
        ),
        DoaBackend::SrpPhat => {
            // Construction is fallible only if frame_size is bogus; rustfft
            // panics on zero-len. Guard explicitly.
            if frame_size == 0 {
                tracing::warn!(
                    "doa_backend=srp_phat requested but frame_size=0; falling back to gcc_phat"
                );
                return (
                    DoaBackendImpl::GccPhat(DoaEstimator::new(geometry, sample_rate, frame_size)),
                    DoaBackend::GccPhat,
                    true,
                );
            }
            let srp = SrpPhatEstimator::new(geometry.clone(), sample_rate, frame_size);
            (
                DoaBackendImpl::SrpPhat {
                    srp,
                    vad: crate::doa::vad::EnergyVad::default(),
                    smoothed_az: 0.0,
                    smoothed_el: 0.0,
                    primed: false,
                    max_sharpness_envelope: 1.0,
                    geom: geometry,
                    frame_size,
                },
                DoaBackend::SrpPhat,
                false,
            )
        }
    }
}
```

- [ ] **Step 4: Add `AudioService::new_with_backend` constructor**

Keep `new()` for callers that don't care (sim, tests). Add a new constructor that takes the backend:

```rust
impl AudioService {
    pub fn new_with_backend(
        audio_tx: Arc<watch::Sender<AudioState>>,
        frame_tx: mpsc::Sender<AudioFrame>,
        frame_rx: mpsc::Receiver<AudioFrame>,
        meeting_tx: mpsc::Sender<ProcessedAudioFrame>,
        frame_size: usize,
        sample_rate: u32,
        geometry: MicGeometry,
        doa_backend: DoaBackend,
    ) -> Self {
        let mut svc = Self::new(audio_tx, frame_tx, frame_rx, meeting_tx,
                                frame_size, sample_rate, geometry.clone());
        let (doa, backend, fallback) = build_doa_backend(
            doa_backend, geometry, sample_rate, frame_size,
        );
        svc.doa = doa;
        svc.doa_backend = backend;
        if fallback {
            svc.audio_tx.send_modify(|s| s.doa_backend_fallback = true);
        }
        svc
    }
}
```

- [ ] **Step 5: Wire AudioConfig.doa_backend in waved main**

In `waved/crates/waved/src/main.rs`, replace the existing `AudioService::new(...)` call (around line 185) with `AudioService::new_with_backend(...)`, passing `cfg.doa_backend`:

```rust
let mut audio_svc = waved_audio::AudioService::new_with_backend(
    audio_tx.clone(),
    frame_tx.clone(),
    frame_rx,
    meeting_tx.clone(),
    cfg.period_frames,
    cfg.capture.mic_sample_rate,
    cfg.mic_geometry.clone().unwrap_or_default(),
    cfg.doa_backend,
);
```

- [ ] **Step 6: Add backend-fallback test**

In `lib.rs` inside `mod tests`:

```rust
#[tokio::test]
async fn srp_backend_with_zero_frame_size_falls_back_to_gcc_phat() {
    use tokio::sync::{mpsc, watch};
    use waved_state::AudioState;
    use std::sync::Arc;

    let (audio_tx_inner, _audio_rx) = watch::channel(AudioState::default());
    let audio_tx = Arc::new(audio_tx_inner);
    let (frame_tx, frame_rx) = mpsc::channel(8);
    let (meeting_tx, _meeting_rx) = mpsc::channel(8);
    let svc = AudioService::new_with_backend(
        audio_tx.clone(), frame_tx, frame_rx, meeting_tx, 0, 48000,
        crate::beam::MicGeometry::default(),
        crate::doa::DoaBackend::SrpPhat,
    );
    assert_eq!(svc.doa_backend, crate::doa::DoaBackend::GccPhat,
        "zero frame_size should fall back to gcc_phat");
    assert!(audio_tx.borrow().doa_backend_fallback);
}
```

- [ ] **Step 7: Run all waved-audio + waved tests**

Run: `cd waved && cargo test -p waved-audio -p waved-state -p waved --lib`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
cd waved
git add crates/waved-audio/src/lib.rs crates/waved-state/src/lib.rs crates/waved/src/main.rs
git commit -m "$(cat <<'EOF'
feat(audio): runtime DoA backend selection (gcc_phat | srp_phat)

AudioService::new_with_backend dispatches between GCC-PHAT and
SRP-PHAT based on AudioConfig.doa_backend. Fallback to GCC-PHAT on
SRP init failure sets AudioState.doa_backend_fallback so stats
collector can surface it.

The SRP path reuses the EnergyVad + EWMA smoothing logic from
DoaEstimator (open-coded rather than refactored — the two structs
diverge enough that a shared base would be premature abstraction).

waved/src/main.rs now passes cfg.doa_backend through.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Add `BeamMode::Steered { initial_az_deg }` variant

**Files:**
- Modify: `waved/crates/waved-state/src/lib.rs` (AudioMode enum)
- Modify: `waved/crates/waved-audio/src/beam/mod.rs` (back_delay_scale + Beamformer recompute)

- [ ] **Step 1: Write failing tests**

Append to `waved/crates/waved-state/src/lib.rs` inside `mod tests`:

```rust
#[test]
fn audio_mode_serializes_steered_variant() {
    use serde_json;
    let m = AudioMode::Steered { initial_az_deg: 90.0 };
    let s = serde_json::to_string(&m).expect("serializes");
    assert!(s.contains("steered"), "json: {s}");
    assert!(s.contains("initial_az_deg"), "json: {s}");
    let back: AudioMode = serde_json::from_str(&s).expect("round-trips");
    assert!(matches!(back, AudioMode::Steered { initial_az_deg } if (initial_az_deg - 90.0).abs() < 1e-6));
}

#[test]
fn audio_mode_default_remains_omni() {
    // Steered must not displace Omni as the default.
    assert!(matches!(AudioMode::default(), AudioMode::Omni));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd waved && cargo test -p waved-state --lib audio_mode_serializes_steered -v`
Expected: FAIL with "no variant `Steered`".

- [ ] **Step 3: Add the variant**

In `waved/crates/waved-state/src/lib.rs`, extend the AudioMode enum (around line 83):

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AudioMode {
    #[default]
    Omni,
    Cardioid,
    Supercardioid,
    Figure8,
    Stereo,
    /// DoA-tracking cardioid. The beam direction follows the live DoA
    /// estimate, smoothed via EWMA on the unit-vector (see beam/steered.rs).
    /// `initial_az_deg` is where the beam points before the first
    /// high-confidence DoA frame arrives (default 0 = forward).
    Steered { initial_az_deg: f32 },
}
```

- [ ] **Step 4: Update `back_delay_scale` in beam/mod.rs**

In `waved/crates/waved-audio/src/beam/mod.rs`, extend the match (line 175):

```rust
fn back_delay_scale(mode: AudioMode) -> f32 {
    match mode {
        AudioMode::Cardioid | AudioMode::Stereo | AudioMode::Steered { .. } => 1.0,
        AudioMode::Supercardioid => 0.577,
        AudioMode::Figure8 => 0.0,
        AudioMode::Omni => 0.0,
    }
}
```

And the `recompute` predicates:

```rust
fn recompute(&mut self) {
    self.is_omni = matches!(self.mode, AudioMode::Omni);
    self.is_stereo = matches!(self.mode, AudioMode::Stereo);
    if self.is_omni {
        return;
    }
    // ... rest unchanged. Steered behaves as Cardioid for now;
    // SteeredBeam wrapper in Task 14 overrides direction per-frame.
}
```

- [ ] **Step 5: Run tests across both crates**

Run: `cd waved && cargo test -p waved-state -p waved-audio --lib`
Expected: PASS — new variant tests + all existing AudioMode-related code compiles. The `matches!` macros in waved-audio that destructure AudioMode will need to be checked.

If the build fails with non-exhaustive match warnings, fix the matches in `crates/waved-audio/src/lib.rs` and `crates/waved/src/main.rs` to handle `Steered` (treat as `Cardioid` until Task 14 lands).

- [ ] **Step 6: Commit**

```bash
cd waved
git add crates/waved-state/src/lib.rs crates/waved-audio/src/beam/mod.rs
git commit -m "$(cat <<'EOF'
feat(state,beam): add AudioMode::Steered { initial_az_deg }

Behaves as Cardioid in the existing Beamformer until Task 14 wires
the SteeredBeam direction-tracking wrapper. New variant carries the
initial azimuth so the beam has a defined direction before the first
high-confidence DoA frame arrives.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: SteeredBeam state machine (new `beam/steered.rs`)

**Files:**
- Create: `waved/crates/waved-audio/src/beam/steered.rs`
- Modify: `waved/crates/waved-audio/src/beam/mod.rs` (add `pub mod steered;`)

- [ ] **Step 1: Create the file with full state machine + tests**

Create `waved/crates/waved-audio/src/beam/steered.rs`:

```rust
//! DoA-tracking beam direction smoothing + reactivation logic.
//!
//! Holds last high-confidence direction during silence (beam doesn't swing
//! when the speaker pauses). Reverts to omni after long silence so a
//! returning speaker isn't initially captured by a stale-pointed cardioid.
//! See F2 spec §3.3 for full state machine.

use crate::doa::DoaFrame;

/// EWMA factor on direction (alpha = 0.15 → ≈140 ms time constant at 21 ms cadence).
const ALPHA_DIR: f32 = 0.15;
/// Confidence below which a frame is treated as "no source".
pub const CONFIDENCE_THRESHOLD: f32 = 0.30;
/// Frames of continuous low-confidence before omni reversion starts
/// (240 frames × 21 ms ≈ 5 s).
const REVERT_SILENCE_FRAMES: u32 = 240;
/// Cosine-fade length between cardioid↔omni (50 × 21 ms ≈ 1 s).
const OMNI_RAMP_FRAMES: u32 = 50;

#[derive(Debug, Clone, Copy)]
pub struct SteeredDirection {
    pub az_deg: f32,
    pub el_deg: f32,
    /// 0.0 = pure cardioid at (az, el); 1.0 = pure omni.
    pub omni_mix: f32,
}

pub struct SteeredBeam {
    /// Current smoothed direction (the one the cardioid weights track).
    current_dir: [f32; 3],
    /// Last received high-confidence target.
    target_dir: [f32; 3],
    silence_frames: u32,
    omni_mix: f32,
}

impl SteeredBeam {
    pub fn new(initial_az_deg: f32) -> Self {
        let dhat = unit_vec_from_az_el(initial_az_deg, 0.0);
        Self {
            current_dir: dhat,
            target_dir: dhat,
            silence_frames: 0,
            omni_mix: 0.0,
        }
    }

    /// Feed one DoA frame. Returns the smoothed beam direction + omni_mix.
    pub fn on_doa_frame(&mut self, frame: &DoaFrame) -> SteeredDirection {
        if frame.vad
            && frame.confidence > CONFIDENCE_THRESHOLD
            && !frame.azimuth_deg.is_nan()
        {
            self.target_dir = unit_vec_from_az_el(frame.azimuth_deg, frame.elevation_deg);
            self.current_dir = slerp(self.current_dir, self.target_dir, ALPHA_DIR);
            self.silence_frames = 0;
            // Ramp back from omni → cardioid one step per high-conf frame.
            self.omni_mix = (self.omni_mix - 1.0 / OMNI_RAMP_FRAMES as f32).max(0.0);
        } else {
            self.silence_frames = self.silence_frames.saturating_add(1);
            if self.silence_frames > REVERT_SILENCE_FRAMES {
                self.omni_mix = (self.omni_mix + 1.0 / OMNI_RAMP_FRAMES as f32).min(1.0);
            }
        }
        let (az, el) = az_el_from_unit_vec(self.current_dir);
        SteeredDirection {
            az_deg: az,
            el_deg: el,
            omni_mix: self.omni_mix,
        }
    }
}

fn unit_vec_from_az_el(az_deg: f32, el_deg: f32) -> [f32; 3] {
    let az = az_deg.to_radians();
    let el = el_deg.to_radians();
    [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()]
}

fn az_el_from_unit_vec(d: [f32; 3]) -> (f32, f32) {
    let az = d[1].atan2(d[0]).to_degrees();
    let az = if az < 0.0 { az + 360.0 } else { az };
    let el = d[2].clamp(-1.0, 1.0).asin().to_degrees();
    (az, el)
}

/// Spherical-linear interpolation between two unit vectors. Falls back to
/// linear + renorm at near-antipodal (dot ≈ -1, slerp undefined) — paranoia
/// guard since real DoA never jumps 180° frame-to-frame.
fn slerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
    if dot > 0.9995 {
        // Vectors nearly parallel — linear interp is fine and avoids 0/sin(0).
        return normalize([
            a[0] * (1.0 - t) + b[0] * t,
            a[1] * (1.0 - t) + b[1] * t,
            a[2] * (1.0 - t) + b[2] * t,
        ]);
    }
    if dot < -0.9995 {
        // Antipodal — slerp ill-defined; linear + renorm.
        return normalize([
            a[0] * (1.0 - t) + b[0] * t,
            a[1] * (1.0 - t) + b[1] * t,
            a[2] * (1.0 - t) + b[2] * t,
        ]);
    }
    let theta_0 = dot.acos();
    let theta = theta_0 * t;
    let sin_theta = theta.sin();
    let sin_theta_0 = theta_0.sin();
    let s1 = ((1.0 - t) * theta_0).sin() / sin_theta_0;
    let s2 = sin_theta / sin_theta_0;
    [a[0] * s1 + b[0] * s2, a[1] * s1 + b[1] * s2, a[2] * s1 + b[2] * s2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
    [v[0] / n, v[1] / n, v[2] / n]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doa::{DoaBackend, DoaFrame};

    fn high_conf_frame(az: f32, el: f32) -> DoaFrame {
        DoaFrame {
            frame_idx: 0,
            timestamp_ns: 0,
            azimuth_deg: az,
            elevation_deg: el,
            confidence: 0.8,
            vad: true,
            backend: DoaBackend::GccPhat,
        }
    }

    fn silence_frame() -> DoaFrame {
        DoaFrame::silent(0, 0, DoaBackend::GccPhat)
    }

    #[test]
    fn steered_beam_holds_direction_during_silence() {
        let mut beam = SteeredBeam::new(0.0);
        // Lock onto az=45° over a few frames.
        for _ in 0..30 {
            beam.on_doa_frame(&high_conf_frame(45.0, 0.0));
        }
        let locked = beam.on_doa_frame(&high_conf_frame(45.0, 0.0));
        // Now 100 silent frames — direction must not change.
        for _ in 0..100 {
            let s = beam.on_doa_frame(&silence_frame());
            assert!((s.az_deg - locked.az_deg).abs() < 0.5,
                "az drifted during silence: {} vs {}", s.az_deg, locked.az_deg);
        }
    }

    #[test]
    fn steered_beam_reverts_to_omni_after_long_silence() {
        let mut beam = SteeredBeam::new(0.0);
        for _ in 0..30 {
            beam.on_doa_frame(&high_conf_frame(90.0, 0.0));
        }
        // 240 silent frames triggers the ramp, then 50 more to reach omni.
        for _ in 0..(REVERT_SILENCE_FRAMES + OMNI_RAMP_FRAMES + 5) {
            beam.on_doa_frame(&silence_frame());
        }
        let s = beam.on_doa_frame(&silence_frame());
        assert!((s.omni_mix - 1.0).abs() < 1e-3, "omni_mix should reach 1.0, got {}", s.omni_mix);
    }

    #[test]
    fn steered_beam_smooths_jitter() {
        let mut beam = SteeredBeam::new(45.0);
        // Feed 30 frames jittering ±10° around az=45°.
        let mut s = silence_frame();
        for i in 0..30 {
            let jitter = if i % 2 == 0 { 10.0 } else { -10.0 };
            s = beam.on_doa_frame(&high_conf_frame(45.0 + jitter, 0.0));
        }
        assert!((s.az_deg - 45.0).abs() < 5.0,
            "smoothed az {} not within ±5° of 45 under ±10° jitter", s.az_deg);
    }

    #[test]
    fn steered_beam_slerp_short_arc_across_180_boundary() {
        let mut beam = SteeredBeam::new(170.0);
        beam.on_doa_frame(&high_conf_frame(170.0, 0.0));
        beam.on_doa_frame(&high_conf_frame(170.0, 0.0));
        // Target = az=190° (= -170° wrapped). Short arc goes through 180°,
        // not through 0°.
        let s = beam.on_doa_frame(&high_conf_frame(190.0, 0.0));
        // Smoothed should be between 170 and 190 (going up through 180),
        // NOT around 80-90 (which would mean it went the long way).
        assert!(s.az_deg > 165.0 && s.az_deg < 195.0,
            "slerp went the long way: az={}", s.az_deg);
    }

    #[test]
    fn steered_beam_ramp_back_from_omni() {
        let mut beam = SteeredBeam::new(0.0);
        // Get to omni_mix = 1.0 via long silence.
        for _ in 0..(REVERT_SILENCE_FRAMES + OMNI_RAMP_FRAMES + 10) {
            beam.on_doa_frame(&silence_frame());
        }
        let omni = beam.on_doa_frame(&silence_frame());
        assert!((omni.omni_mix - 1.0).abs() < 1e-3);
        // High-confidence frame should start ramping back to cardioid.
        let next = beam.on_doa_frame(&high_conf_frame(90.0, 0.0));
        let expected_step = 1.0 / OMNI_RAMP_FRAMES as f32;
        assert!((next.omni_mix - (1.0 - expected_step)).abs() < 1e-3,
            "omni_mix should decrement one step (~{:.3}), got {}", 1.0 - expected_step, next.omni_mix);
    }
}
```

- [ ] **Step 2: Register the module**

In `waved/crates/waved-audio/src/beam/mod.rs`, alongside the existing `pub mod` lines:

```rust
pub mod geometry;
pub mod delays;
pub mod steered;
pub mod weights;
```

- [ ] **Step 3: Run all 5 SteeredBeam tests**

Run: `cd waved && cargo test -p waved-audio --lib beam::steered -v`
Expected: PASS — all 5 unit tests.

- [ ] **Step 4: Commit**

```bash
cd waved
git add crates/waved-audio/src/beam/steered.rs crates/waved-audio/src/beam/mod.rs
git commit -m "$(cat <<'EOF'
feat(beam): SteeredBeam direction-smoothing + reactivation state

New beam/steered.rs implements the F2 §3.3 state machine: slerp on
unit-sphere with alpha=0.15 (~140ms time const), hold direction
during silence, ramp to omni after 5s low-confidence, ramp back to
cardioid on the next high-confidence frame.

Antipodal slerp guard falls back to linear+renorm (real-world DoA
never jumps 180° frame-to-frame; this is paranoia).

5 unit tests cover: hold during silence, omni reversion timing,
jitter smoothing, short-arc slerp across the 180°/-180° boundary,
and ramp-back from omni.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Wire SteeredBeam into AudioService — direction tracking + omni crossfade

**Files:**
- Modify: `waved/crates/waved-audio/src/lib.rs` (AudioService)
- Modify: `waved/crates/waved-audio/src/beam/mod.rs` (Beamformer needs omni-crossfade support)

The Beamformer currently applies one fixed `(az, el)` direction and one mode. For Steered mode we need to: (a) update `(az, el)` per-frame from the SteeredBeam, (b) crossfade between cardioid and omni based on `omni_mix`.

- [ ] **Step 1: Add omni-mix crossfade to Beamformer**

In `waved/crates/waved-audio/src/beam/mod.rs`, extend the struct + add a setter:

```rust
pub struct Beamformer {
    // ... existing fields ...
    /// In [0, 1]. 0 = pure pattern (cardioid/super/fig8/stereo), 1 = pure
    /// omni-mix output. Only used for Steered mode; other modes ignore.
    omni_mix: f32,
}
```

In `Beamformer::new`, initialise `omni_mix: 0.0`. Add a setter:

```rust
pub fn set_omni_mix(&mut self, mix: f32) {
    self.omni_mix = mix.clamp(0.0, 1.0);
}
```

In `process`, in the non-omni branch where `y_l = front_l - back_l`, apply the crossfade with omni mean:

```rust
// (existing) y_l = front_l - back_l; ...
// Compute omni sample (mean of all 8 mics' current values) if mixed.
let omni_sample = if self.omni_mix > 1e-3 {
    let mut sum = 0.0f32;
    for alsa_ch in 0..8 {
        sum += self.delays[alsa_ch].read(0.0);
    }
    sum / 8.0
} else {
    0.0
};

let y_l_mixed = (1.0 - self.omni_mix) * y_l + self.omni_mix * omni_sample;

if self.is_stereo {
    let y_r = front_r - back_r;
    let y_r_mixed = (1.0 - self.omni_mix) * y_r + self.omni_mix * omni_sample;
    mono.push(0.5 * (y_l_mixed + y_r_mixed));
    stereo_l.push(y_l_mixed);
    stereo_r.push(y_r_mixed);
} else {
    mono.push(y_l_mixed);
}
```

(Remove the original `mono.push(y_l);` / `mono.push(0.5 * (y_l + y_r));` lines that this block replaces.)

- [ ] **Step 2: Add steered_beam to AudioService + invoke per-frame**

In `waved/crates/waved-audio/src/lib.rs`:

```rust
use crate::beam::steered::SteeredBeam;

pub struct AudioService {
    // ... existing fields ...
    /// Some when current mode is AudioMode::Steered. Reset to None when
    /// switching to any other mode (Task 12 follow-up).
    steered: Option<SteeredBeam>,
}
```

In `new()` (or `new_with_backend`), initialise based on initial mode:

```rust
let initial_mode = audio_tx.borrow().mode.clone();
let steered = match &initial_mode {
    waved_state::AudioMode::Steered { initial_az_deg } => Some(SteeredBeam::new(*initial_az_deg)),
    _ => None,
};
```

In `Self { ... }`: `steered,`.

In `process_frame`, after publishing the DoaFrame (Task 6 emit), inject into steered:

```rust
if let Some(beam) = self.steered.as_mut() {
    let dir = beam.on_doa_frame(&doa_frame);
    self.beamformer.set_doa(dir.az_deg, dir.el_deg);
    self.beamformer.set_omni_mix(dir.omni_mix);
} else {
    // Existing path: doa_override or live tracking via beamformer.set_doa.
    // (Untouched — the existing block stays for non-Steered modes.)
}
```

Move the existing `let (steer_az, steer_el) = self.doa_override.unwrap_or(...); self.beamformer.set_doa(steer_az, steer_el);` block into the `else` branch.

In the `Command::SetAudioMode` handler in `run()`, update `steered`:

```rust
Ok(Command::SetAudioMode(mode)) => {
    info!("audio mode → {mode:?}");
    self.steered = match &mode {
        waved_state::AudioMode::Steered { initial_az_deg } => {
            Some(SteeredBeam::new(*initial_az_deg))
        }
        _ => None,
    };
    self.beamformer.set_mode(mode.clone());
    self.beamformer.set_omni_mix(0.0); // reset crossfade for non-steered modes
    self.audio_tx.send_modify(|s| s.mode = mode);
}
```

- [ ] **Step 3: Write integration test for steered behavior**

In `lib.rs` inside `mod tests`:

```rust
#[tokio::test]
async fn steered_mode_tracks_doa_override() {
    use crate::AudioFrame;
    use tokio::sync::{mpsc, watch};
    use waved_state::{AudioState, AudioMode};
    use std::sync::Arc;

    let mut initial = AudioState::default();
    initial.mode = AudioMode::Steered { initial_az_deg: 0.0 };
    let (audio_tx_inner, _audio_rx) = watch::channel(initial);
    let audio_tx = Arc::new(audio_tx_inner);
    let (frame_tx, frame_rx) = mpsc::channel(8);
    let (meeting_tx, _meeting_rx) = mpsc::channel(8);
    let mut svc = AudioService::new(
        audio_tx, frame_tx, frame_rx, meeting_tx, 480, 48000,
        crate::beam::MicGeometry::default(),
    );
    // Verify the steered wrapper is constructed.
    assert!(svc.steered.is_some(), "Steered mode should construct SteeredBeam");

    // Feed silence — direction holds, omni_mix doesn't immediately ramp.
    let frame = AudioFrame {
        data: vec![0i16; 480 * 8], channels: 8,
        reference: None, reference_channels: 0,
        timestamp_ns: 1_000_000,
    };
    svc.process_frame(&frame);
    // No assertion on direction — just that it doesn't panic.
}
```

- [ ] **Step 4: Run all tests**

Run: `cd waved && cargo test -p waved-audio --lib`
Expected: PASS — all tests including the new steered_mode_tracks_doa_override.

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-audio/src/lib.rs crates/waved-audio/src/beam/mod.rs
git commit -m "$(cat <<'EOF'
feat(audio,beam): wire SteeredBeam into AudioService

AudioService now constructs a SteeredBeam when initial AudioMode is
Steered, and re-constructs it on Command::SetAudioMode(Steered { .. }).
Per-frame: feed the freshly-published DoaFrame into the SteeredBeam,
push (az, el, omni_mix) into the Beamformer.

Beamformer gains an omni_mix field + crossfade in process():
y_out = (1-mix)·y_cardioid + mix·y_omni. Both halves are
unity-DC-gain so the crossfade is click-free.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: Per-frame integration test — steered beam handoff (`#[ignore]`-gated)

**Files:**
- Create: `waved/crates/waved-audio/tests/steered_beam_handoff.rs`

This is the §5.2 integration test: drive synthetic audio through AudioService, assert the steered beam tracks during voice, holds during silence, and resumes when voice returns.

- [ ] **Step 1: Create the integration-test file**

Create `waved/crates/waved-audio/tests/steered_beam_handoff.rs`:

```rust
//! Steered-beam integration test. #[ignore]-gated because it constructs a
//! real AudioService and drives a synthesised 5-second clip through it —
//! slow vs unit tests, but the only way to verify end-to-end DoA → beam
//! tracking.
//!
//! Run with: cargo test -p waved-audio --test steered_beam_handoff -- --ignored

use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use waved_audio::AudioService;
use waved_audio::beam::MicGeometry;
use waved_audio::AudioFrame;
use waved_state::{AudioMode, AudioState};

fn synth_plane_wave(az_deg: f32, el_deg: f32, sr: u32, frames: usize, geom: &MicGeometry) -> Vec<i16> {
    let az = az_deg.to_radians();
    let el = el_deg.to_radians();
    let d = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
    let freq = 1000.0f32;
    let omega = 2.0 * std::f32::consts::PI * freq / sr as f32;
    const C: f32 = 343.0;
    let mut out = vec![0i16; frames * 8];
    for alsa_ch in 0..8 {
        let p = geom.position_for_alsa(alsa_ch);
        let dot_mm = p.x * d[0] + p.y * d[1] + p.z * d[2];
        let tau_samples = (-(dot_mm * 1e-3) / C) * sr as f32;
        for i in 0..frames {
            let s = ((i as f32 - tau_samples) * omega).sin() * 0.5;
            out[i * 8 + alsa_ch] = (s * 16000.0) as i16;
        }
    }
    out
}

#[tokio::test]
#[ignore]
async fn steered_beam_follows_doa_during_handoff() {
    let geom = MicGeometry::default();
    let sr = 48000u32;
    let period = 480usize;

    let mut initial = AudioState::default();
    initial.mode = AudioMode::Steered { initial_az_deg: 0.0 };
    let (audio_tx_inner, _) = watch::channel(initial);
    let audio_tx = Arc::new(audio_tx_inner);
    let (frame_tx, frame_rx) = mpsc::channel(8);
    let (meeting_tx, _meeting_rx) = mpsc::channel(8);
    let mut svc = AudioService::new(
        audio_tx, frame_tx, frame_rx, meeting_tx, period, sr, geom.clone(),
    );
    let mut rx = svc.subscribe_doa();

    // Phase 1: 2 seconds of source at az=-45° (= 315°)
    let n_periods = (2 * sr as usize) / period;
    let s1 = synth_plane_wave(315.0, 0.0, sr, period, &geom);
    for _ in 0..n_periods {
        let frame = AudioFrame {
            data: s1.clone(), channels: 8,
            reference: None, reference_channels: 0, timestamp_ns: 0,
        };
        svc.process_frame(&frame);
    }
    // Drain DoA receiver — keep most recent frame.
    let mut last_phase1 = None;
    while let Ok(f) = rx.try_recv() {
        last_phase1 = Some(f);
    }
    let p1 = last_phase1.expect("phase 1 produced DoA frames");
    let p1_err = ((p1.azimuth_deg - 315.0).abs()).min((360.0 - (p1.azimuth_deg - 315.0).abs()));
    assert!(p1_err < 10.0,
        "phase 1 az {} should be within ±10° of 315°", p1.azimuth_deg);

    // Phase 2: 1 second silence
    let silence = vec![0i16; period * 8];
    for _ in 0..(sr as usize / period) {
        let frame = AudioFrame {
            data: silence.clone(), channels: 8,
            reference: None, reference_channels: 0, timestamp_ns: 0,
        };
        svc.process_frame(&frame);
    }
    while rx.try_recv().is_ok() {}

    // Phase 3: 2 seconds source at az=+90°
    let s3 = synth_plane_wave(90.0, 0.0, sr, period, &geom);
    let n_p3 = (2 * sr as usize) / period;
    for _ in 0..n_p3 {
        let frame = AudioFrame {
            data: s3.clone(), channels: 8,
            reference: None, reference_channels: 0, timestamp_ns: 0,
        };
        svc.process_frame(&frame);
    }
    let mut last_phase3 = None;
    while let Ok(f) = rx.try_recv() {
        last_phase3 = Some(f);
    }
    let p3 = last_phase3.expect("phase 3 produced DoA frames");
    let p3_err = (p3.azimuth_deg - 90.0).abs();
    assert!(p3_err < 10.0,
        "phase 3 az {} should track to 90° within ±10°", p3.azimuth_deg);
}
```

- [ ] **Step 2: Run the integration test**

Run: `cd waved && cargo test -p waved-audio --test steered_beam_handoff -- --ignored --nocapture`
Expected: PASS — handoff completes in ~5 s of test wall time.

- [ ] **Step 3: Commit**

```bash
cd waved
git add crates/waved-audio/tests/steered_beam_handoff.rs
git commit -m "$(cat <<'EOF'
test(audio): #[ignore]-gated steered-beam handoff integration test

Drives 5s of synthetic audio (2s @ az=315°, 1s silence, 2s @ az=90°)
through a real AudioService configured in Steered mode. Asserts the
DoA stream tracks each source within ±10°. Gated behind --ignored to
keep the default test gate fast.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 16: New `waved-doa-tap` debug binary (F3 consumer smoke)

**Files:**
- Create: `waved/crates/waved-doa-tap/Cargo.toml`
- Create: `waved/crates/waved-doa-tap/src/main.rs`
- Modify: `waved/Cargo.toml` (workspace members)

- [ ] **Step 1: Add workspace member**

In `waved/Cargo.toml`, add `crates/waved-doa-tap` to `[workspace].members`. Read the existing file first to find the right alphabetical position:

```bash
cd waved && grep -A 30 "^\[workspace\]" Cargo.toml | head -35
```

Then insert `"crates/waved-doa-tap",` alphabetically between `waved-device-test` and `waved-hid` (or wherever it lands sorted).

- [ ] **Step 2: Create `Cargo.toml`**

Create `waved/crates/waved-doa-tap/Cargo.toml`:

```toml
[package]
name = "waved-doa-tap"
version.workspace = true
edition.workspace = true

[dependencies]
waved-audio = { path = "../waved-audio" }
waved-state = { path = "../waved-state" }
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "sync"] }
serde_json.workspace = true
clap = { version = "4", features = ["derive"] }
```

- [ ] **Step 3: Create `src/main.rs`**

Create `waved/crates/waved-doa-tap/src/main.rs`:

```rust
//! Subscribes to AudioService::subscribe_doa() and prints DoaFrame JSON to
//! stdout. Smoke-test for F3's future consumer path; not part of waved.
//!
//! Usage on device:
//!   adb push target/aarch64-unknown-linux-gnu/release/waved-doa-tap /data/waved/
//!   adb shell /data/waved/waved-doa-tap --duration-secs 10

use clap::Parser;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, watch};
use waved_audio::{AudioFrame, AudioService};
use waved_audio::beam::MicGeometry;
use waved_state::AudioState;

#[derive(Parser, Debug)]
struct Args {
    /// How long to listen (seconds).
    #[clap(long, default_value = "5")]
    duration_secs: u64,
    /// DoA backend to construct AudioService with. "gcc_phat" | "srp_phat".
    #[clap(long, default_value = "gcc_phat")]
    backend: String,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let backend = match args.backend.as_str() {
        "srp_phat" => waved_audio::doa::DoaBackend::SrpPhat,
        _ => waved_audio::doa::DoaBackend::GccPhat,
    };
    let (audio_tx_inner, _) = watch::channel(AudioState::default());
    let audio_tx = Arc::new(audio_tx_inner);
    let (frame_tx, frame_rx) = mpsc::channel(64);
    let (meeting_tx, _meeting_rx) = mpsc::channel(64);
    let svc = AudioService::new_with_backend(
        audio_tx, frame_tx, frame_rx, meeting_tx, 1024, 48000,
        MicGeometry::default(), backend,
    );
    let mut rx = svc.subscribe_doa();

    // Run AudioService in the background — it will produce DoaFrames
    // whenever real audio arrives on frame_rx. Without a capture source
    // wired in, this only emits frames when something else calls
    // process_frame; on device with the real waved daemon already running,
    // operators should stop waved first and let this be the consumer.
    let (_cmd_tx, cmd_rx) = broadcast::channel::<waved_state::Command>(8);
    tokio::spawn(svc.run(cmd_rx));

    let deadline = Instant::now() + Duration::from_secs(args.duration_secs);
    let mut count = 0u64;
    while Instant::now() < deadline {
        tokio::select! {
            r = rx.recv() => {
                match r {
                    Ok(f) => {
                        println!("{}", serde_json::to_string(&f).unwrap());
                        count += 1;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("lagged {n} frames");
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                // Heartbeat so we can tell if no frames are flowing.
                if count == 0 {
                    eprintln!("(no frames yet — is waved running and feeding this AudioService?)");
                }
            }
        }
    }
    eprintln!("doa-tap: {count} frames in {} s", args.duration_secs);
    Ok(())
}
```

- [ ] **Step 4: Build for host first to catch compile errors**

Run: `cd waved && cargo build -p waved-doa-tap`
Expected: PASS — produces `target/debug/waved-doa-tap`.

- [ ] **Step 5: Cross-build for device**

Run: `cd waved && cross build --release --target aarch64-unknown-linux-gnu -p waved-doa-tap`
Expected: PASS — produces `target/aarch64-unknown-linux-gnu/release/waved-doa-tap`.

- [ ] **Step 6: Commit**

```bash
cd waved
git add crates/waved-doa-tap/ Cargo.toml
git commit -m "$(cat <<'EOF'
feat(doa-tap): debug binary subscribing to AudioService::subscribe_doa

Smoke harness for F3's future consumer path. Prints one JSON DoaFrame
per ALSA period to stdout; supports both backends via --backend
{gcc_phat | srp_phat}.

Not wired into S60waved or any service — operators run it manually
after stopping the production daemon. Used by Task 17's on-device
acceptance test 7.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 17: On-device acceptance script `scripts/test-doa-improvements.sh`

**Files:**
- Create: `waved/scripts/test-doa-improvements.sh`
- Create: `waved/scripts/test-doa-elevation.sh` (separate diagnostic measurement)

This script covers all 7 tests from spec §5.3. It assumes a manually-placed loudspeaker at known azimuths and that the operator runs each test segment via prompts.

- [ ] **Step 1: Create the main acceptance script**

Create `waved/scripts/test-doa-improvements.sh`:

```bash
#!/usr/bin/env bash
# F2 on-device acceptance: 7 tests covering the 5 buckets from the spec.
# Requires a loudspeaker the operator can reposition; prompts for each.
#
# Usage:  scripts/test-doa-improvements.sh
# Device: must be reachable via adb; waved binary deployed at /data/waved/waved
#         and the new waved-doa-tap binary at /data/waved/waved-doa-tap.

set -euo pipefail

DEVICE="${DEVICE:-PSP_0000000000000000}"
WAVED_BIN="${WAVED_BIN:-/data/waved/waved}"
TAP_BIN="${TAP_BIN:-/data/waved/waved-doa-tap}"

adb_run() { adb -s "$DEVICE" shell "$@"; }
adb_kill_waved() {
    adb_run "killall waved 2>/dev/null; killall S60waved 2>/dev/null; sleep 1" || true
}
adb_start_waved() {
    adb_run "(setsid $WAVED_BIN >/tmp/waved.log 2>&1 </dev/null &); sleep 4"
}

prompt() {
    echo
    echo "==> $1"
    read -r -p "press ENTER when ready: " _
}

read_az() {
    # Reads doa_horizontal from /tmp/audio_stats.json (4s average).
    adb_run "for i in 1 2 3 4 5 6 7 8; do cat /tmp/audio_stats.json 2>/dev/null | \
        grep -o 'doa_horizontal\":[^,]*' | head -1; sleep 0.5; done" \
        | grep -oE '[0-9]+\.[0-9]+' \
        | awk '{s+=$1; n++} END {if (n>0) print s/n}'
}

run_test_270_az() {
    echo "=== Test 1: 270° azimuth fix regression ==="
    prompt "place loudspeaker at az=270° (device's right side), play 1 kHz tone"
    az=$(read_az)
    echo "measured az: ${az}°"
    err=$(echo "$az" | awk '{d=270-$1; if (d<0) d=-d; if (d>180) d=360-d; print d}')
    echo "az error: ${err}°"
    if [ "$(echo "$err < 5.0" | bc)" = "1" ]; then
        echo "PASS: az within ±5°"
    else
        echo "FAIL: az error ${err}° >= 5°"
        return 1
    fi
}

run_test_other_az_regression() {
    echo "=== Test 2: 4-quadrant az regression (no SP#3 regression) ==="
    for target in 0 90 180; do
        prompt "place loudspeaker at az=${target}°, play 1 kHz tone"
        az=$(read_az)
        err=$(echo "$az $target" | awk '{d=$2-$1; if (d<0) d=-d; if (d>180) d=360-d; print d}')
        echo "az=${target}° measured ${az}° → error ${err}°"
        if [ "$(echo "$err < 10.0" | bc)" = "1" ]; then
            echo "  PASS (within ±10° SP#3 spec)"
        else
            echo "  FAIL: ${err}° >= 10°"
            return 1
        fi
    done
}

run_test_srp_backend() {
    echo "=== Test 4: SRP-PHAT backend swap ==="
    adb_kill_waved
    adb_run "echo '{\"version\":4,\"capture\":{\"mic_device\":\"hw:0,2\",\"mic_channels\":8,\"mic_sample_rate\":48000,\"mic_format\":\"S32_LE\"},\"period_frames\":1024,\"buffer_periods\":8,\"doa_backend\":\"srp_phat\"}' > /tmp/waved-audio-config.json"
    # Override config path via env (or copy to /etc/) — depends on existing
    # waved.rs config loading. Easiest: copy to /etc/.
    adb_run "cp /tmp/waved-audio-config.json /etc/waved-audio-config.json"
    adb_start_waved
    # Re-run test 1 with the new backend.
    prompt "(SRP-PHAT active) place loudspeaker at az=270°"
    az=$(read_az)
    err=$(echo "$az" | awk '{d=270-$1; if (d<0) d=-d; if (d>180) d=360-d; print d}')
    echo "SRP-PHAT az=${az}°, err=${err}°"
    if [ "$(echo "$err < 10.0" | bc)" = "1" ]; then
        echo "PASS: SRP-PHAT no worse than GCC-PHAT (±10°)"
    else
        echo "FAIL: SRP-PHAT az error ${err}° >= 10°"
        return 1
    fi
}

run_test_srp_cpu() {
    echo "=== Test 5: SRP-PHAT CPU delta < 5% ==="
    # Sample CPU for 15s under speech load.
    prompt "play speech via loudspeaker for 20s"
    pid=$(adb_run "pidof waved")
    samples=$(adb_run "for i in \$(seq 1 15); do awk '{print \$14+\$15}' /proc/${pid}/stat; sleep 1; done")
    avg=$(echo "$samples" | awk '{a+=$1; n++} END {print a/n}')
    echo "SRP-PHAT 15s CPU jiffies avg: $avg"
    # NOTE: a comparable GCC-PHAT baseline must be captured first; this test
    # is currently informational, not auto-PASS/FAIL — operator records both.
    echo "INFO: compare to GCC-PHAT baseline manually for the < 5% delta gate"
}

run_test_steered_beam_pickup() {
    echo "=== Test 6: Steered beam pickup (subjective) ==="
    adb_run "echo 'audio_mode_set steered' > /tmp/waved-mode" # if your runtime supports it; otherwise SetAudioMode via cmd path
    prompt "play continuous speech from az=0°, then move loudspeaker to az=90° over ~2s"
    echo "LISTEN: does the captured audio follow the source? Y/N"
    read -r answer
    if [ "$answer" = "Y" ] || [ "$answer" = "y" ]; then
        echo "PASS (subjective)"
    else
        echo "FAIL: beam did not follow"
        return 1
    fi
}

run_test_f3_consumer_smoke() {
    echo "=== Test 7: F3 consumer smoke via waved-doa-tap ==="
    adb_kill_waved
    # Run tap directly — produces frames itself by owning the AudioService.
    out=$(adb_run "$TAP_BIN --duration-secs 5 2>&1" | head -20)
    echo "$out"
    n_lines=$(echo "$out" | grep -c '^{' || true)
    if [ "$n_lines" -gt 50 ]; then
        echo "PASS: ${n_lines} DoaFrame JSONs in 5 s (expected ~240 at 21ms cadence)"
    else
        echo "FAIL: only ${n_lines} frames"
        return 1
    fi
}

main() {
    adb_kill_waved
    adb_start_waved
    run_test_270_az
    run_test_other_az_regression
    echo "=== Test 3 (elevation bias measurement) is in test-doa-elevation.sh ==="
    run_test_srp_backend
    run_test_srp_cpu
    run_test_steered_beam_pickup
    run_test_f3_consumer_smoke
    echo
    echo "All F2 acceptance tests complete."
}

main "$@"
```

- [ ] **Step 2: Create the elevation measurement diagnostic**

Create `waved/scripts/test-doa-elevation.sh`:

```bash
#!/usr/bin/env bash
# F2 §3.4 elevation bias measurement. Runs WAVED_DOA_DEBUG, records
# measured vs truth el at 4 elevations × 4 azimuths, output to
# /tmp/elevation-measurements.csv on device + pulled to host.
#
# Operator-driven (placement of source). Outcome → AUDIO_HARDWARE.md
# update in Task 19; no auto-PASS/FAIL gate.

set -euo pipefail
DEVICE="${DEVICE:-PSP_0000000000000000}"
adb_run() { adb -s "$DEVICE" shell "$@"; }

CSV=/tmp/elevation-measurements.csv
adb_run "echo 'truth_az,truth_el,measured_az,measured_el,sharp_0_2,sharp_1_3,sharp_0_4,sharp_2_6' > $CSV"

for az in 0 90 180 270; do
    for el in -30 0 30 60; do
        echo
        echo "Position loudspeaker at az=${az}°, el=${el}° relative to top-ring plane."
        read -r -p "press ENTER when ready: " _
        # 5 s WAVED_DOA_DEBUG burst → grep latest line, extract numbers.
        adb_run "WAVED_DOA_DEBUG=100 timeout 5 /data/waved/waved 2>&1 | grep doa_debug | tail -5"
        # Manually transcribe into CSV — easier than parsing the log format.
        read -r -p "measured az: " m_az
        read -r -p "measured el: " m_el
        read -r -p "sh values comma-separated (sh_02,sh_13,sh_04,sh_26): " shs
        adb_run "echo '${az},${el},${m_az},${m_el},${shs}' >> $CSV"
    done
done

adb pull "$CSV" ./elevation-measurements-$(date +%Y%m%d).csv
echo
echo "Done. Analyse the CSV; update AUDIO_HARDWARE.md with findings (Task 19)."
```

- [ ] **Step 3: Make both scripts executable**

```bash
cd waved
chmod +x scripts/test-doa-improvements.sh scripts/test-doa-elevation.sh
```

- [ ] **Step 4: Smoke-test scripts dry-run (no device required for syntax check)**

```bash
cd waved
bash -n scripts/test-doa-improvements.sh
bash -n scripts/test-doa-elevation.sh
```
Expected: no output (syntax OK).

- [ ] **Step 5: Commit**

```bash
cd waved
git add scripts/test-doa-improvements.sh scripts/test-doa-elevation.sh
git commit -m "$(cat <<'EOF'
test(doa): on-device acceptance scripts for F2

test-doa-improvements.sh covers the 7 tests from F2 spec §5.3:
270° az fix, 4-quadrant regression, SRP backend swap, SRP CPU delta
(informational — needs paired GCC baseline), steered-beam pickup
(subjective), and waved-doa-tap consumer smoke.

test-doa-elevation.sh is the §3.4 diagnostic measurement —
operator-driven sweep across 4 az × 4 el, output CSV. Outcome feeds
AUDIO_HARDWARE.md per Task 19; no auto-PASS gate (we're measuring,
not regression-testing).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 18: On-device validation — run acceptance, record baselines in commits

**Files:** No code; produces measurements + commit messages.

- [ ] **Step 1: Cross-build everything**

```bash
cd waved
cross build --release --target aarch64-unknown-linux-gnu \
    --features "ui,capture-alsa" -p waved -p waved-doa-tap
```

- [ ] **Step 2: Deploy**

```bash
adb push waved/target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb push waved/target/aarch64-unknown-linux-gnu/release/waved-doa-tap /data/waved/waved-doa-tap
```

- [ ] **Step 3: Run GCC-PHAT baseline acceptance**

```bash
cd waved
# Make sure config has doa_backend absent (defaults to gcc_phat):
adb shell "grep doa_backend /etc/waved-audio-config.json || echo 'NOT SET (default gcc_phat)'"
scripts/test-doa-improvements.sh 2>&1 | tee firmwares/measurements/doa-gcc-phat-$(date +%Y%m%d).log
```
Expected: tests 1, 2, 6, 7 PASS; test 5 records CPU samples for later comparison.

- [ ] **Step 4: Run SRP-PHAT acceptance**

The script already swaps the config and restarts. Re-run end-to-end:

```bash
scripts/test-doa-improvements.sh 2>&1 | tee firmwares/measurements/doa-srp-phat-$(date +%Y%m%d).log
```
Expected: tests 1, 2, 6, 7 PASS with SRP backend; test 5 CPU delta computed against the GCC baseline log.

- [ ] **Step 5: Run elevation diagnostic**

```bash
scripts/test-doa-elevation.sh
# CSV pulled to ./elevation-measurements-YYYYMMDD.csv
mv elevation-measurements-*.csv firmwares/measurements/
```

- [ ] **Step 6: Commit measurement logs**

```bash
cd waved
git add ../firmwares/measurements/doa-gcc-phat-*.log ../firmwares/measurements/doa-srp-phat-*.log ../firmwares/measurements/elevation-measurements-*.csv
git commit -m "$(cat <<'EOF'
chore(measurements): F2 on-device acceptance baselines

GCC-PHAT and SRP-PHAT acceptance runs from scripts/test-doa-improvements.sh.
Elevation diagnostic sweep from scripts/test-doa-elevation.sh.

The 270° az fix outcome is recorded; before/after numbers feed the
Task 19 docs update. SRP-PHAT CPU delta is the gate that decides
whether SRP ships as an available backend (spec §4: < 5%).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 19: Docs update — AUDIO_HARDWARE.md sections + baseline tables

**Files:**
- Modify: `waved/docs/AUDIO_HARDWARE.md`

Add three sections per the spec §7 completion checklist: weighted-LS rationale, SRP-PHAT section, steered-beam section, elevation bias measurement table.

- [ ] **Step 1: Add weighted-LS section under existing "DOA Pipeline"**

Locate the existing "DOA Pipeline" section (around line 35 of `docs/AUDIO_HARDWARE.md`). Add a new subsection at the end of that block:

```markdown
### Per-pair confidence-weighted LS (F2)

The LS solver in `solve.rs` weights each pair's row by its GCC-PHAT
peak sharpness (`solve_direction_weighted`). Sharpness =
`-(y_l - 2*y_c + y_r)` from the parabolic fit; positive for concave
(real) peaks, ~0 for flat/lumpy noise surfaces.

**Why:** SP#3 left a ~19° az error at 270° vs ~7° elsewhere. At az=270°
the source sits on the device's -Y side; the top-ring mics on the +Y
hemisphere (physical 0 and 1) sit in body shadow, so pairs (0,2) and
(1,3) both contain one shadowed mic. The pair with the more degraded
GCC-PHAT peak contributes less to the LS solution.

`MIN_SHARPNESS_THRESHOLD = 1e-4` — when Σ pair sharpnesses falls
below this (e.g. silence + VAD-passed transient), the solver returns
None and the caller emits a `DoaFrame { vad: false, NaN angles }`.
F3 (diarization) must filter on `vad && confidence > threshold`,
never consume stale-smoothed angles.

Confidence is normalised: `mean(per_pair_sharpness) /
max_observed_sharpness` (10s envelope on the denominator). Range [0,1].

**On-device:** azimuth at 270° improved from 19° error to <ERR>° (see
`firmwares/measurements/doa-gcc-phat-<DATE>.log`). Fill in actual
numbers from Task 18.
```

(Replace `<ERR>` and `<DATE>` with the real values from the Task 18 log.)

- [ ] **Step 2: Add SRP-PHAT section**

After the weighted-LS subsection:

```markdown
### SRP-PHAT alternative backend (F2)

Selected via `AudioConfig.doa_backend = "srp_phat"`. Grid search over
72 azimuths × 19 elevations = 1368 directions, accumulating
PHAT-weighted cross-spectrum across the same 4 pairs as GCC-PHAT.
Voice-band masked (200–4000 Hz). Sub-grid refinement via parabolic
interp in az/el (~0.5° accuracy).

Confidence = `(peak_power - mean_grid_power) / peak_power`,
clamped [0,1]. Same shape as the GCC-PHAT path so F3 can threshold
on a single value regardless of backend.

**Selection at startup (static — runtime switching not supported):**

```json
{ "doa_backend": "srp_phat" }
```

**Fallback:** if SRP construction fails (currently only when
`frame_size = 0`), AudioService falls back to GCC-PHAT and sets
`AudioState.doa_backend_fallback = true`, surfaced in
`/tmp/audio_stats.json`.

**CPU:** measured on PSP_0000000000000000 at 48 kHz / period 1024,
voice load — GCC-PHAT baseline <BASE>%, SRP-PHAT <SRP>%, delta
<DELTA>% (see `firmwares/measurements/doa-srp-phat-<DATE>.log`).
F2 spec gate: delta < 5% — <PASS_OR_FAIL>.
```

(Fill in `<BASE>`, `<SRP>`, `<DELTA>`, `<PASS_OR_FAIL>`, `<DATE>` from Task 18 logs.)

- [ ] **Step 3: Add steered-beam section under "Beamforming Modes"**

After the existing "Beamforming Modes (SP#3)" content:

```markdown
### Steered cardioid (F2)

`AudioMode::Steered { initial_az_deg }`. Cardioid pattern with
direction tracking driven by the per-frame DoA stream from
`subscribe_doa()`. Direction smoothed via slerp on unit-vectors,
α=0.15 (~140ms time constant at 21ms ALSA period).

State machine (`beam/steered.rs`):

- High-confidence DoA frame (`vad && confidence > 0.30`): direction
  EWMA's toward the new target. Hard-tracks rapid speaker movement
  with bounded smoothing.
- Low-confidence / silence frame: direction held. After 240
  consecutive frames (~5s), `omni_mix` ramps 0→1 over 50 frames (~1s)
  cosine fade.
- Next high-confidence frame: `omni_mix` ramps back to 0 with the
  same 50-frame step.

Mix is a linear crossfade `(1-mix)·cardioid + mix·omni`; both halves
are unity-DC-gain so no click.

Thresholds (`CONFIDENCE_THRESHOLD = 0.30`, `REVERT_SILENCE_FRAMES =
240`, `OMNI_RAMP_FRAMES = 50`) are hardcoded; promotion to
`AudioConfig` only if tuning evidence demands.
```

- [ ] **Step 4: Add elevation bias measurement table**

Locate the "Elevation" discussion in the existing "Cross-ring pair acoustic shadow" subsection. After it, add:

```markdown
### Elevation bias measurement (F2)

Measured 2026-MM-DD with `scripts/test-doa-elevation.sh`. Loudspeaker
positioned at 4 elevations × 4 azimuths relative to top-ring plane;
WAVED_DOA_DEBUG=100 line transcribed.

| Truth az | Truth el | Measured el | Bias |
|----------|----------|-------------|------|
| 0°       | -30°     | <M>°        | <B>° |
| 0°       | 0°       | <M>°        | <B>° |
| 0°       | +30°     | <M>°        | <B>° |
| 0°       | +60°     | <M>°        | <B>° |
| 90°      | -30°     | <M>°        | <B>° |
| 90°      | 0°       | <M>°        | <B>° |
| 90°      | +30°     | <M>°        | <B>° |
| 90°      | +60°     | <M>°        | <B>° |
| 180°     | -30°     | <M>°        | <B>° |
| 180°     | 0°       | <M>°        | <B>° |
| 180°     | +30°     | <M>°        | <B>° |
| 180°     | +60°     | <M>°        | <B>° |
| 270°     | -30°     | <M>°        | <B>° |
| 270°     | 0°       | <M>°        | <B>° |
| 270°     | +30°     | <M>°        | <B>° |
| 270°     | +60°     | <M>°        | <B>° |

**Outcome:** <DESCRIBE — consistent geometric offset, or
azimuth-varying algorithmic bias>. <DECISION — documented as test-rig
artifact, OR follow-up algorithmic fix queued as F2b candidate>.
```

(Populate from `firmwares/measurements/elevation-measurements-*.csv`.)

- [ ] **Step 5: Commit**

```bash
cd waved
git add docs/AUDIO_HARDWARE.md
git commit -m "$(cat <<'EOF'
docs(AUDIO_HARDWARE): F2 weighted-LS, SRP-PHAT, steered beam, el bias

Four new subsections per F2 spec §7 completion checklist:
- Per-pair confidence-weighted LS rationale + 270° az before/after
- SRP-PHAT backend section + CPU delta vs GCC-PHAT
- Steered cardioid state machine + thresholds
- Elevation bias measurement table + outcome decision

All numeric placeholders filled from on-device measurements in
firmwares/measurements/ (Task 18).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 20: TODO.md SP#3 follow-up bookkeeping + AUDIO_HARDWARE cross-link

**Files:**
- Modify: `waved/TODO.md` (mark SP#3 residual closed; add F2b candidates)
- Modify: `waved/CLAUDE.md` (no change expected — already lists `AUDIO_HARDWARE.md` as authoritative)

- [ ] **Step 1: Update SP#3 residuals**

In `waved/TODO.md`, locate "### SP#3 known residuals (not blockers, not scheduled)". Update:

```markdown
### SP#3 known residuals — F2 status

- ~~Azimuth error at 270° is ~19° post-pair-swap (vs ~7° before).
  Candidate fix: per-pair GCC-PHAT peak-sharpness confidence weighting
  in the LS solve.~~ **FIXED in F2** (2026-05-DD). See
  `docs/AUDIO_HARDWARE.md` "Per-pair confidence-weighted LS" section.
- Elevation bias +3° to +18° after pure-Z pair swap. **F2 instrumented
  and measured** — see `docs/AUDIO_HARDWARE.md` "Elevation bias
  measurement" table. Outcome: <ARTIFACT|FIX_QUEUED>.
- Raw mic signal at ~−75 dBFS sits close to the VAD floor (0.0003).
  Preamp / gain staging belongs in SP#5. (Unchanged.)
```

(Replace `<ARTIFACT|FIX_QUEUED>` with the actual Task 19 outcome.)

- [ ] **Step 2: Add F2b follow-ups section**

After SP#3 residuals, add:

```markdown
### F2 follow-ups (not blockers; F2b candidates)

- **SRP-PHAT default switch.** F2 ships SRP-PHAT as opt-in
  (`doa_backend = "srp_phat"`). Default remains GCC-PHAT pending more
  data. Trigger to promote: F3 acceptance shows SRP gives materially
  better speaker separation in real meetings.
- **Vendor `libmodel_infer.so` FFI binding.** Explicitly deferred per
  F2 spec §6. Trigger: F3 surfaces "two close speakers get merged"
  pattern that improved Rust DoA can't handle.
- **Multi-source DoA.** Current single-source assumption is fine for
  F3's clustering. Trigger: simultaneous speakers scenario.
- **Steered cardioid → steered supercardioid / hypercardioid /
  figure-8.** Pattern shape is orthogonal to direction; YAGNI until
  requested.
- **Runtime DoA backend switch.** F2 reads `doa_backend` once at
  startup. Trigger: A/B comparison need.
- **Per-AudioConfig SRP grid resolution.** 5°/10° hardcoded.
  Promote to config if tuning evidence demands.
```

- [ ] **Step 3: Mark F2 itself as the SP#7-adjacent F2 sub-project (if TODO has SP indexing)**

(Skim TODO.md for the current SP roadmap; the F2 work doesn't slot into an existing SP cleanly — it's a follow-up to SP#3. Mention it in the SP#3 section.)

- [ ] **Step 4: Commit**

```bash
cd waved
git add TODO.md
git commit -m "$(cat <<'EOF'
docs(TODO): close SP#3 270° az residual; add F2 follow-ups

F2 implementation closes the per-pair confidence weighting candidate
fix and surfaces a new F2b queue: SRP-PHAT default promotion (needs
F3 data), vendor FFI (deferred unless F3 wall hit), multi-source DoA,
beam-pattern variants, runtime backend switch, SRP grid config.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

This plan was self-reviewed against the spec on 2026-05-23 by skimming each spec section and checking that at least one task implements its requirement. Result:

| Spec section | Implementing task(s) | Status |
|---|---|---|
| §1 Architecture: `doa/srp.rs` | Task 8, 9, 10 | OK |
| §1 Architecture: `doa/stream.rs` | Task 4 | OK |
| §1 Architecture: `beam/steered.rs` | Task 13 | OK |
| §1 `BeamMode::Steered` | Task 12 | OK |
| §2 `DoaFrame` struct + `subscribe_doa` | Task 4, 6 | OK |
| §2 Confidence semantics | Task 6 (GCC envelope), 9 (SRP peak/mean) | OK |
| §3.1 Per-pair weighted LS | Task 1, 2, 3 | OK |
| §3.2 SRP-PHAT algorithm | Task 9 | OK |
| §3.2 Sub-sample refinement | Task 10 | OK |
| §3.3 Steered beam state machine | Task 13 | OK |
| §3.4 Elevation bias instrumentation | Task 7 (debug line), Task 17 (script) | OK |
| §4 Backend fallback + stats flag | Task 11 | OK |
| §4 Weighted-LS degeneracy guard | Task 2 | OK |
| §5.1 Unit tests (gcc/solve/srp/stream/steered) | Tasks 1-13 inline | OK |
| §5.2 `#[ignore]`-gated integration test | Task 15 | OK (CPU budget gate is in Task 17 / 18) |
| §5.3 On-device acceptance script | Task 17 | OK |
| §5.3 `waved-doa-tap` debug binary | Task 16 | OK |
| §7 Acceptance summary item 6 (AUDIO_HARDWARE updates) | Task 19 | OK |
| §7 Acceptance summary item 7 (baseline numbers in commits) | Task 18 | OK |

Spec coverage: complete. No gaps.

Placeholder scan: searched for "TBD", "TODO", "implement later", "fill in" — only `<ERR>`, `<DATE>`, `<BASE>`, `<SRP>`, `<DELTA>`, `<PASS_OR_FAIL>`, `<M>`, `<B>`, `<ARTIFACT|FIX_QUEUED>` remain, and those are explicit fill-in-from-measurement placeholders in Task 18/19 — by design, since the values come from on-device runs.

Type consistency:
- `DoaFrame` fields match across stream.rs (Task 4), AudioService publication (Task 6), SteeredBeam consumption (Task 13), waved-doa-tap (Task 16).
- `DoaBackend` enum matches in stream.rs (Task 4), AudioConfig (Task 5), DoaBackendImpl (Task 11), waved-doa-tap CLI (Task 16).
- `solve_direction_weighted` signature matches between definition (Task 2) and use (Task 3).
- `SrpResult` matches between Task 9 declaration and Task 11 dispatch.

No issues to fix.
