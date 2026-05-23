# waved bf+doa Improvements — Design

**Goal:** Improve our open-source Rust beamforming + DoA stack to (a) fix
known SP#3 accuracy residuals, (b) ship a per-frame DoA stream that
Feature 3 (DoA-based diarization) will consume, (c) add an alternative
SRP-PHAT backend, (d) ship a steered-cardioid beam mode that tracks the
estimated source direction.

**Why this and not vendor FFI:** Stock firmware's `libmodel_infer.so`
exposes `bf_*` and `doa_srp_*` APIs that are likely better-tuned than
ours, but binding a 2.7 MB stripped closed binary cuts against the
project's open-source-firmware goal, and the diarization use case from
F3 is robust enough to small DoA errors that our improved Rust path
should be sufficient. Vendor FFI stays a documented escape hatch for
when F3 surfaces a real gap (e.g., "two speakers sitting next to each
other on a couch get merged"). See [[feedback_diarization_scope]],
[[project_sp4_npu_status]].

**Scope:** Improvements live in `crates/waved-audio`. No new crate. Five
F2 items, all in one cycle per user direction:

1. 270° azimuth bug fix (per-pair confidence-weighted LS solve).
2. Elevation bias investigation (instrument + measure; decide-then-ship).
3. Per-frame `DoaFrame` stream API (`AudioService::subscribe_doa()`).
4. SRP-PHAT alternative DoA backend (runtime-selectable via config; default = GCC-PHAT).
5. DoA-driven steered cardioid beam mode (`BeamMode::Steered`).

---

## 1. Architecture & Components

All work stays in the existing `waved-audio` crate. Three internal
modules grow / appear:

```
crates/waved-audio/src/
├── doa/
│   ├── mod.rs           # existing — DoA orchestrator
│   ├── solve.rs         # MODIFY — per-pair confidence-weighted LS solve
│   ├── gcc_phat.rs      # MODIFY — emit per-pair peak sharpness alongside τ
│   ├── srp.rs           # NEW — SRP-PHAT grid-search DoA backend
│   ├── vad.rs           # existing
│   └── stream.rs        # NEW — per-frame DoaFrame publisher (broadcast)
├── beam/
│   ├── mod.rs           # MODIFY — add BeamMode::Steered variant
│   ├── steered.rs       # NEW — DoA-tracking direction smoothing + reactivation
│   ├── delays.rs        # existing
│   ├── geometry.rs      # existing
│   └── weights.rs       # existing
```

**Three new responsibilities:**

1. **`doa::stream::DoaPublisher`** — a
   `tokio::sync::broadcast::Sender<DoaFrame>`. Emits one frame per ALSA
   capture period (~21 ms at 48 kHz/1024). `AudioService::subscribe_doa()`
   mirrors the existing `subscribe_processed()` / `subscribe_reference()`
   patterns. F3 (diarization) is the first consumer; UI/logging can
   also subscribe.

2. **`doa::srp::SrpPhatEstimator`** — alternative DoA backend. Selected
   via `AudioConfig.doa_backend: "gcc-phat" (default) | "srp-phat"`
   (static config; no runtime switching). Both backends emit into the
   same `DoaFrame` stream — consumers don't care which one ran.

3. **`beam::steered::SteeredBeam`** — wraps existing cardioid weights,
   applies direction smoothing (slerp / EWMA) on DoA stream input. New
   `BeamMode::Steered { initial_az_deg }` variant. Holds last
   confident direction during silence; reverts to omni after >5 s of
   low-confidence frames.

**Two existing modules grow narrowly:**

- `doa::gcc_phat::estimate_tdoa` returns `(tau_samples, sharpness)`
  instead of just `tau`. Sharpness is the negated parabolic-fit
  curvature `-(y_l - 2·y_c + y_r)`, already computed during the
  existing concavity check.
- `doa::solve::solve_direction` accepts a per-pair weight vector; the
  LS becomes weighted-LS (W·A·d̂ = W·b instead of A·d̂ = b). Per-pair
  weight = normalized sharpness.

Total estimated change: ~600 LoC added, ~80 LoC modified.

---

## 2. Data Flow & Per-Frame Stream Contract

The carrier between F2 (this) and F3 (diarization) is one struct
emitted once per ALSA period:

```rust
// crates/waved-audio/src/doa/stream.rs
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct DoaFrame {
    pub frame_idx: u64,        // monotonic from AudioService startup
    pub timestamp_ns: u64,     // CLOCK_MONOTONIC at capture, propagated from AudioFrame
    pub azimuth_deg: f32,      // [-180, 180], NaN if !vad
    pub elevation_deg: f32,    // [-90, 90], NaN if !vad
    pub confidence: f32,       // [0.0, 1.0] — aggregated per-pair sharpness, 0 if !vad
    pub vad: bool,             // EnergyVad output on this frame
    pub backend: DoaBackend,   // GccPhat | SrpPhat — so consumer can log which ran
}

pub struct DoaPublisher {
    tx: broadcast::Sender<DoaFrame>,  // capacity 64 (~1.3s buffer at 21ms cadence)
}
```

**Flow:**

```
AudioService::process_frame(AudioFrame)
    ↓ (existing path: AEC, NS, beam, processed_tx, audio_tx)
    ↓
DoaOrchestrator::tick(audio_frame)
    ├─ EnergyVad (existing)
    ├─ if backend == "gcc-phat":  gcc_phat + weighted-LS (existing path, modified)
    ├─ if backend == "srp-phat":  srp_phat grid search (new path)
    ├─ smoothing (existing EWMA on az unit-vec, linear on el)
    └─ DoaPublisher::emit(DoaFrame{...})
                ↓
        broadcast::Sender (lagging-receiver-safe)
                ↓
        F3 DiarizationService subscriber  ← future
        UI azimuth-arrow subscriber       ← already exists (reads stats_collector today;
                                            may migrate to subscribe_doa() in F2b)
        Steered beam subscriber           ← NEW (consumes via same channel)
```

**Backpressure:** `broadcast` channel; if a receiver lags > 64 frames
it gets `RecvError::Lagged` and skips. F3 must handle Lagged like any
other consumer in the codebase (same pattern as `subscribe_processed`).

**Subscription API on AudioService:**

```rust
impl AudioService {
    pub fn subscribe_doa(&self) -> broadcast::Receiver<DoaFrame> {
        self.doa_publisher.subscribe()
    }
}
```

**Confidence semantics (load-bearing — F3 will threshold on this):**

- **GCC-PHAT backend:** `confidence = mean(per_pair_sharpness) /
  max_observed_sharpness`, clamped [0, 1]. `max_observed_sharpness` is
  a slow-decaying envelope (one-pole, time-constant 10 s) so
  confidence stays meaningful as ambient noise floor drifts.
- **SRP-PHAT backend:** `confidence = (peak_power - mean_grid_power) /
  peak_power`, clamped [0, 1]. Same normalized shape: 1.0 = sharp
  single peak, 0.0 = flat response.

Either way, confidence is *normalized 0–1* and *comparable across
backends*. F3 can use a single threshold (e.g., 0.30) without caring
which backend ran.

**Existing surfaces unchanged or narrowly extended:**

- `/tmp/audio_stats.json` still updates every 500 ms with the latest
  `doa_horizontal` field (read from the same `DoaFrame` the publisher
  emits). New optional field: `doa_backend_fallback: bool` (see §4).
- `WAVED_DOA_DEBUG=N` env-var trigger unchanged; the log *line* gains
  per-pair sharpness + per-pair el contribution columns (§3.4). Anyone
  parsing the existing line will need to skip the new fields — they
  append, not insert.

---

## 3. Algorithms

### 3.1 Per-pair confidence-weighted LS solve (the 270° az fix)

**Current bug:** `solve_direction` solves `A · d̂ = b` where each row is
`(p_i - p_j)/|p_i - p_j|` (baseline unit vector) and `b_k = τ_k · c /
|p_i - p_j|`. All pairs weighted equally. When pair (1,3) gets a noisy
τ (e.g., at az 270° one mic is shadowed by the device body), its bad
row pulls the LS solution off the true direction. Measured impact:
±19° az error at 270° vs ±7° at other azimuths.

**Fix:** weighted LS. Each pair contributes weight `w_k ∈ [0, 1]`
proportional to its GCC-PHAT peak sharpness.

```rust
// Per-pair sharpness = parabolic-fit curvature, already computed during the
// existing concavity check. gcc_phat now returns (tau_samples, sharpness).
sharpness_k = -denom_k    // we already compute denom_k; just stop discarding it
weight_k    = sharpness_k.max(0.0) / sum(all sharpnesses)   // normalize across pairs

// Weighted normal equations: (A^T W A) d̂ = A^T W b
// where W = diag(weight_0, weight_1, weight_2, weight_3)
let wa: Mat = a.transpose() * w_diag * a;
let wb: Vec = a.transpose() * w_diag * b;
let d_hat = wa.qr().solve(wb);
```

**Why this should help at 270°:** at that direction, the source (on
the device's right, −Y) sits in body shadow relative to the mics on
the +Y side of the top ring (physical mics 0 and 1). Both XY pairs
(0,2) and (1,3) therefore contain one shadowed mic, and at least one
of them is likely to produce a degraded GCC-PHAT peak with reduced
sharpness. Per-pair confidence weighting lets the LS solve down-weight
that pair, giving more vote to the pure-Z pairs (0,4)/(2,6) which —
despite both their mics being on the front face of the ring — still
produce a sharper cross-correlation because both elements see the
source through symmetric body geometry.

Whether this specifically resolves the observed 19° at 270° is a
hypothesis; the on-device regression in §5.3 test 1 confirms or
refutes it pre-merge. If it doesn't, that's a real result — the next
diagnostic step is per-pair logging via `WAVED_DOA_DEBUG` (extended in
§3.4) to see which pair the LS is over-trusting.

**Failure mode to guard:** all pairs equally poor (e.g., source
directly above device with all elevation pairs flat). Sum of
sharpnesses → 0, weights become ill-defined.

```rust
if sum_sharpness < MIN_SHARPNESS_THRESHOLD { // 1e-4
    // Don't pretend to know.
    return DoaResult::Unknown;
}
```

Caller emits `DoaFrame { vad: false, azimuth_deg: NaN, elevation_deg:
NaN, confidence: 0.0, ... }`.

### 3.2 SRP-PHAT backend

**Algorithm:**

```
For each candidate direction d̂ in grid G (5° az × 10° el = 1368 dirs):
    P(d̂) = 0
    For each pair (i, j) in pairs:
        τ_expected = (p_i - p_j) · d̂ / c     # in samples
        # PHAT cross-spectrum (frequency-domain, voice band 200-4000 Hz)
        # P_PHAT(f) = X_i(f) · conj(X_j(f)) / |X_i(f) · conj(X_j(f))|
        # Steered response = sum_f Re[P_PHAT(f) · exp(j·2π·f·τ_expected)]
        P(d̂) += steered_response(pair=(i,j), tau_expected=τ_expected)
peak_dir = argmax P(d̂)
```

**Grid:** 72 azimuths × 19 elevations = 1368 candidate directions.
**Pairs:** same 4 used by GCC-PHAT — (0,2), (1,3), (0,4), (2,6). Same
body-shadow reasoning applies (cross-ring 3D-diagonal pairs excluded).

**Voice-band weighting:** PHAT spectrum is band-limited to 200–4000 Hz
before steering. Out-of-band bins skipped — saves ~half the per-bin
work, sharpens speech-source localization.

**FFT reuse:** GCC-PHAT path already FFTs each pair's mic signals. The
SRP backend computes the FFTs once per period and reuses them across
all 1368 directions. The grid loop is cheap arithmetic — only ~5
complex muls per pair per direction.

**Estimated cost:** 4 pairs × 1368 dirs × ~50 voice-band bins × ~5 ops
≈ 1.4 M ops/period. At 21 ms period that's ~67 MFLOPs/s, well within
Cortex-A55 NEON budget. **Target CPU delta: < 5% per spec convention;
budget implementation against 3% to leave headroom.**

**Subsample refinement:** after finding peak grid cell, parabolic
interp in az/el to refine to ~0.5° accuracy. Same parabolic-fit
machinery as `gcc_phat::peak_interp`.

### 3.3 Steered beam direction smoothing + reactivation

```rust
struct SteeredBeamState {
    target_dir: UnitVec3,       // last received high-confidence DoA
    current_dir: UnitVec3,      // smoothed actual beam direction
    confidence_ewma: f32,       // smoothed confidence
    silence_frames: u32,        // counter since last high-confidence DoA
    omni_mix: f32,              // [0, 1]; 0 = pure cardioid, 1 = omni
}

const ALPHA_DIR: f32 = 0.15;             // direction EWMA (≈140 ms time const at 21 ms cadence)
const CONFIDENCE_THRESHOLD: f32 = 0.30;  // F3 will likely match this
const REVERT_SILENCE_FRAMES: u32 = 240;  // ≈5 s at 21 ms cadence
const OMNI_RAMP_FRAMES: u32 = 50;        // ≈1 s cosine fade between cardioid ↔ omni

on DoaFrame:
    if vad && confidence > CONFIDENCE_THRESHOLD:
        target_dir = doa_frame.direction
        current_dir = slerp(current_dir, target_dir, ALPHA_DIR)   // smooth on unit-sphere
        silence_frames = 0
        omni_mix = (omni_mix - 1.0/OMNI_RAMP_FRAMES).max(0.0)     // ramp back to cardioid
    else:
        silence_frames += 1
        // current_dir is held — beam does NOT swing during silence
        if silence_frames > REVERT_SILENCE_FRAMES:
            omni_mix = (omni_mix + 1.0/OMNI_RAMP_FRAMES).min(1.0) // ramp to omni
```

**Why slerp not lerp:** azimuth angles are circular. Lerping az = 170°
toward az = −170° goes the wrong way around (through 0°). slerp on the
unit vector goes the short way across the back.

**Output:** beam weights = `(1 - omni_mix) * cardioid_weights(current_dir)
+ omni_mix * omni_weights`. Linear crossfade between the
cardioid-at-current-direction and omni. No clicks; both are
unity-DC-gain.

**Slerp through antipodal direction guard:** if target ≈ −current
(slerp undefined), fall back to lerp + renormalize. Real-world DoA
never makes 180° jumps frame-to-frame (upstream EWMA + smoothing), so
this is paranoia code, but cheap.

### 3.4 Elevation bias investigation

**Diagnostic-first, fix-second.** The +3° → +18° elevation bias from
SP#3 may be (a) a real geometric offset (source actually above ring),
(b) the same algorithmic shadow issue manifesting in elevation pairs,
or (c) something else. Decide based on data.

Plan:

1. **Instrument:** extend `WAVED_DOA_DEBUG=N` line to emit per-pair el
   contribution + per-pair sharpness. (Existing log already has
   `(0,4)raw=... exp=...`; add the derived el component per pair.)
2. **Measurement script:** new `scripts/test-doa-elevation.sh` —
   4-position vertical sweep (source at el = −30°, 0°, +30°, +60°
   relative to top-ring plane), record measured-vs-truth across all 4
   azimuth quadrants. Compare to current production figures.
3. **Decide based on data:**
   - If bias is consistent across azimuth and matches a simple
     geometric offset (e.g., source-actually-above-ring-by-30 mm) →
     document in `AUDIO_HARDWARE.md`, NO code change. The bias is an
     artifact of the test rig, not the estimator.
   - If bias varies wildly per azimuth → root cause is algorithmic
     (possibly the same pair-shadow issue, manifesting in elevation
     pairs). Implement a fix candidate based on what the data reveals.

The plan commits to ship the diagnostics + measurement; it does NOT
commit to a code fix. If the data says "this is a real bias and we
should leave it alone," that's a valid completion. Future F2b can pick
up an algorithmic fix if measurement justifies it.

---

## 4. Error Handling & Edge Cases

**Backend selection failure:**

- `AudioConfig.doa_backend = "srp-phat"` but SRP-PHAT can't initialize
  (e.g., FFT planner OOM, geometry degenerate) → log `WARN`, fall back
  to GCC-PHAT, set `audio_stats.doa_backend_fallback = true`. Don't
  crash AudioService.
- Unknown `doa_backend` value → parse error at config load, fail fast
  (same as existing `AudioConfig::validate`).

**Weighted LS degenerates:**

- All pair sharpnesses below `MIN_SHARPNESS_THRESHOLD = 1e-4` (per §3.1) → emit
  `DoaFrame { vad: false, az/el: NaN, confidence: 0.0, ... }`.
  Last-confident-direction handling lives in the steered-beam
  consumer, not in DoA itself. DoA reports honest "I don't know"
  rather than holding stale state.
- Weight matrix nearly singular (e.g., 3 of 4 pairs zero-weighted,
  single pair can't solve 3D) → same path: emit unknown frame. Detect
  by checking residual `|W·A·d̂ - W·b| > tolerance` post-solve and
  discarding.

**SRP grid edge cases:**

- Peak at grid boundary (az = ±180°): wrap parabolic neighbors via
  modular arithmetic. Elevation at ±90°: don't refine (no neighbor on
  the polar side); accept ±0.5 grid cell error.
- Multiple equal peaks (broadband white noise, no real source): pick
  first; low confidence (peak-vs-mean ratio near 1.0 → confidence near
  0). Consumer should drop the frame.

**Steered beam edge cases:**

- Beam mode switched from `Steered` → `Cardioid` mid-talk: existing
  `set_audio_mode` path handles it; new path = drop steered state,
  snap to cardioid pattern. No crossfade between modes (mode switches
  are user-initiated, click is acceptable, matches existing
  semantics).
- DoA confidence threshold tuning: `CONFIDENCE_THRESHOLD = 0.30` is a
  starting value. If on-device acceptance shows the beam jitters
  during real meetings, raise to 0.5. Hardcoded in v1; promoted to
  `AudioConfig.steered_confidence_threshold` only if tuning evidence
  demands it.
- Reverting steered beam to omni after 5 s silence: the
  `OMNI_RAMP_FRAMES = 50` (~1 s) cosine fade is *one-way* — once at
  `omni_mix = 1.0`, the next high-confidence DoA frame starts a
  similar 50-frame ramp *back to cardioid*. Avoids a single noisy
  frame snapping the beam from omni → narrow-cardioid in one period
  (audible artifact).

**DoaPublisher edge cases:**

- No subscribers: `broadcast::send` returns `Err(SendError)` — discard
  silently. Per the existing pattern in `audio_tx`, missing consumers
  are not an error.
- Channel full (capacity 64): broadcast auto-drops oldest for slow
  receivers, emits `Lagged(n)` on next recv. F3's docstring will note:
  "if you see `Lagged`, you missed `n` frames — that's a bug in your
  consumer, not in DoA."

**SRP-PHAT CPU breach:**

- If on-device measurement shows SRP > 5% CPU delta, **don't ship as
  available backend**. Treat as design failure of this section,
  escalate via the SP#3 follow-up loop. Don't ship a backend that
  violates the budget even when it's opt-in — operators will enable it
  without reading the fine print.

**Backend switching at runtime:**

- **Not supported.** `doa_backend` is read once at AudioService
  construction. Changing it requires waved restart. Avoids mid-flight
  state migration between two different algorithms. If the user later
  wants A/B comparison live, that's an F2b ask.

---

## 5. Testing

### 5.1 Unit tests (default `cargo test -p waved-audio`)

**`doa::gcc_phat`:**

- `peak_sharpness_is_positive_for_concave_peak` — `y_l=0, y_c=1,
  y_r=0` → sharpness > 0.
- `peak_sharpness_is_zero_for_flat_correlation` — `y_l=y_c=y_r=0.5` →
  sharpness ≈ 0.
- Existing `noise_pair_is_bounded_not_hundreds_of_samples` extended to
  also assert returned sharpness < min threshold.

**`doa::solve`:**

- `weighted_ls_falls_back_to_unweighted_with_equal_weights` — weights
  = [1,1,1,1] → output bit-exact matches existing `solve_direction` on
  same input.
- `weighted_ls_ignores_zero_weight_pair` — synthetic 4-pair system;
  corrupt pair 3 with a 5-sample τ error, weight=0 → solution matches
  noise-free 3-pair solve.
- `weighted_ls_degenerates_cleanly` — all weights below threshold →
  returns `None` (caller emits "unknown" frame).
- **Regression test for 270° az fix:** synthetic 8-channel signal
  from az=270°, el=0°, with pair (1,7) τ corrupted (simulating body
  shadow), weighted by reduced sharpness → solved az within ±5° of
  truth. Same input with equal weights → solved az error matches the
  known ~19° to lock the regression in.

**`doa::srp`:**

- `srp_finds_known_direction_on_synthetic_signal` — synthetic
  plane-wave 8-ch input at az=45°, el=20° → SRP peak at the correct
  grid cell, refined to ±0.5°.
- `srp_grid_boundary_az_wraps` — synthetic source at az=−175° → peak
  found correctly (parabolic neighbor at az=+180° is correctly used).
- `srp_confidence_low_on_uniform_noise` — uncorrelated noise →
  confidence < 0.1.
- `srp_voice_band_skips_dc` — manually verify only bins 200–4000 Hz
  contribute (count operations or check intermediate tensor).
- `srp_falls_back_to_gccphat_on_init_failure` — inject FFT init
  failure → AudioService comes up with `backend=GccPhat` + stats flag
  set.

**`doa::stream`:**

- `doa_publisher_emits_one_frame_per_period` — drive 10 periods → 10
  frames on the broadcast receiver.
- `doa_publisher_no_subscribers_does_not_panic` — drop receiver, drive
  periods → no error propagates.
- `doa_frame_serializes_to_json` — round-trip via `serde_json`, field
  shape sanity check (so `/tmp/audio_stats.json` consumers don't
  break silently).
- `doa_frame_vad_false_has_nan_angles` — invariant:
  `!vad → az.is_nan() && el.is_nan()`.

**`beam::steered`:**

- `steered_beam_holds_direction_during_silence` — feed 1
  high-confidence DoA, then 100 low-confidence frames → `current_dir`
  unchanged.
- `steered_beam_reverts_to_omni_after_long_silence` — feed 1
  high-confidence DoA, then `REVERT_SILENCE_FRAMES +
  OMNI_RAMP_FRAMES` low-confidence → `omni_mix == 1.0`.
- `steered_beam_smooths_jitter` — feed 10 frames jittering ±10° around
  az=45° → smoothed direction stays within ±2° of 45° (EWMA settles).
- `steered_beam_slerp_short_arc` — current=170°, target=−170° → next
  smoothed direction crosses 180° (short arc), not 0° (long arc).
  Critical correctness test for the angle-wrap path.
- `steered_beam_ramp_back_from_omni` — at `omni_mix=1.0`, feed
  high-confidence frame → `omni_mix` decreases by `1/OMNI_RAMP_FRAMES`
  per frame, no snap.

### 5.2 Integration tests (`#[ignore]`-gated)

- `srp_phat_cpu_under_budget` — real AudioService with
  `backend=SrpPhat`, drive 5 s of pink noise via existing fixture
  machinery, sample `/proc/self/stat` CPU% → assert delta vs GccPhat
  run < 5%. **This is the gate that decides if SRP-PHAT actually
  ships** — failure aborts merge.
- `steered_beam_follows_doa_during_handoff` — synthetic 5-s clip: 2 s
  at az=−45°, 1 s silence, 2 s at az=+90°. Assert beam direction
  tracks within ±5° of source during voice + holds last during silence
  + ramps to new source within ~200 ms of voice resuming.

### 5.3 On-device acceptance

Single new script `scripts/test-doa-improvements.sh` covering all five
F2 items:

| Test | Acceptance gate |
|---|---|
| 1. 270° az fix regression | Loud-speaker at az=270°, measured az within ±5° (was ±19°) |
| 2. 4-quadrant az error (other angles) | No regression: each cardinal az within ±10° (matches SP#3 acceptance) |
| 3. Elevation bias measurement | Diagnostic only — record measured-vs-truth at 4 elevations × 4 azimuths. Outcome documented in `AUDIO_HARDWARE.md`. |
| 4. SRP-PHAT backend swap | Switch `doa_backend=srp-phat`, rerun tests 1–3, assert no worse than GCC-PHAT |
| 5. SRP-PHAT CPU delta | `/proc/<pid>/stat` sampled 15 s under speech load — delta < 5% per existing `cpu_delta.py` convention |
| 6. Steered beam pickup | Move source mid-recording, listen for natural follow (subjective gate, no false positives) |
| 7. F3 consumer smoke | New `waved-doa-tap` debug binary (NOT part of waved) subscribes to `subscribe_doa()`, prints DoA frames at 21 ms cadence, sanity-checks F3's future consumer path |

### 5.4 What's NOT tested

- **Multi-source localization.** GCC-PHAT and the LS solve assume a
  single dominant source. Two simultaneous sources at different
  directions → DoA reports a meaningless average. Out of scope; F2b
  candidate.
- **Reverberation robustness.** SRP-PHAT *should* be more robust than
  GCC-PHAT in reverb, but we don't have a reverberant test rig.
  Anecdotal acceptance only.
- **Beam steering polar pattern accuracy.** Existing
  `test-beamforming.sh` covers static patterns; steered cardioid
  pattern is "the cardioid pattern, just rotated." Confirmed via test
  6 by listening, not by polar-sweep regression.

---

## 6. Out of Scope (F2b candidates, F3 boundary)

**Deferred to a future cycle (F2b, brainstormed separately when triggered):**

- **Vendor `libmodel_infer.so` FFI binding.** Open-source-first; only
  revisit if F3 surfaces "two close speakers get merged" pattern that
  our improved DoA can't handle.
- **Multi-source DoA.** Current DoA assumes one dominant source;
  multi-source localization (e.g., MUSIC, ESPRIT, or SRP local maxima
  ranking) is its own design. F3 will detect "different speakers at
  different times" via DoA clustering; "two speakers talking
  simultaneously" is explicitly handled by F3 by gating on
  `confidence` (uncertain frames excluded from clustering), not by F2
  separating them.
- **Reverberation-controlled benchmarks.** Needs a treated room or
  simulated RIRs; not in this cycle's test harness.
- **Beam pattern other than cardioid for steered mode.** Supercardioid
  / hypercardioid / figure-8 steered variants. Pattern shape is
  orthogonal to direction; could add but YAGNI until requested.
- **Per-`AudioConfig` SRP grid resolution.** Hardcoded 5° az × 10° el
  in v1. Promotion to config only if user asks.
- **Runtime backend switching.** DoA backend chosen once at startup.
  No mid-flight migration.
- **DoA-driven AEC reference selection.** Could imagine pointing AEC's
  reference channel toward away-from-source mics. Real value would be
  marginal at best; current AEC ERLE is already 23–26 dB.

**F3 boundary (what F2 ships vs what F3 owns):**

| Surface | F2 owns | F3 owns |
|---|---|---|
| `DoaFrame` struct + `subscribe_doa()` API | yes | — |
| Confidence threshold for inclusion | F2 picks a default; F3 may override locally | F3 owns the per-meeting threshold tuning |
| Clustering DoA samples → speaker IDs | — | yes |
| Per-meeting numeric IDs (reset on meeting start) | — | yes |
| Persistence to SQLite (`diarization_segment` table) | — | yes |
| Naming / rename UI / pyannote rescore | — | **EXPLICITLY OUT** per [[feedback_diarization_scope]] |

**Explicit non-promises:**

- F2 does not commit to ±1° azimuth accuracy. Targets: 270° az ≤ ±5°
  (was ±19°), other azimuths no worse than current ±7°.
- F2 does not commit to fixing elevation bias. It commits to
  *measuring* it and documenting.
- F2 does not commit to SRP-PHAT being the *default*. It commits to
  making it *available* and tested; choice of default is a follow-on
  data-driven decision.
- F2's per-frame stream cadence matches the ALSA period (~21 ms
  today). If `period_frames` changes in production, cadence changes —
  F3 must not assume a hardcoded period.

---

## 7. Acceptance Summary

F2 is "done" when all of the following hold on the device
`PSP_0000000000000000` running on-tree `main`:

1. `cargo test -p waved-audio --lib` — all unit tests in §5.1 pass.
2. `cargo test -p waved-audio -- --ignored` — both integration tests
   in §5.2 pass.
3. `scripts/test-doa-improvements.sh` — all 7 tests in §5.3 pass.
4. `WAVED_DOA_DEBUG=100 /data/waved/waved` shows per-pair sharpness +
   per-pair el contribution in the debug line.
5. A `waved-doa-tap` smoke run prints `DoaFrame` JSON to stdout at
   21 ms cadence, schema matching §2.
6. `docs/AUDIO_HARDWARE.md` updated with: elevation bias measurement
   table, weighted-LS rationale, SRP-PHAT section, steered-beam section.
7. Baseline numbers recorded in commit messages (270° az before/after,
   SRP-PHAT vs GCC-PHAT per-azimuth, CPU deltas).
