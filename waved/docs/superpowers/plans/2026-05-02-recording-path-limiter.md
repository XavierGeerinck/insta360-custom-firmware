# Recording-Path Limiter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow software gain to safely exceed today's 4× cap by inserting SP#5's `LookaheadLimiter` on the recording path between gain and `processed_tx.send()`. Resolves the "louder slider produces crackle, can't reach normal speech loudness without distortion" wall hit during mic-volume-pipeline acceptance.

**Architecture:** Reuse SP#5's `LookaheadLimiter` unchanged. `apply_mic_volume` becomes a pure multiply (no clamp). `AudioService` gains two stateful limiter fields (one mono, one 2-channel stereo with shared gain). `apply_gain` runs the gain stage then the appropriate limiter on each path. Software-gain ceiling rises 400 → 800 (8× / +18 dB) end-to-end including UAC2 mixer mapping.

**Tech Stack:** Rust, tokio (broadcast/watch), existing `playback::dsp::limiter::LookaheadLimiter` (lookahead 96 samples = ~2 ms at 48 kHz, ceiling -1 dBFS, release 50 ms).

**Spec:** `docs/superpowers/specs/2026-05-02-recording-path-limiter-design.md`

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `waved/crates/waved-audio/src/lib.rs` | modify | `apply_mic_volume` cap 400→800, hard clamp removed; add `recording_limiter_mono` and `recording_limiter_stereo` to `AudioService` (constructed in `new`); `apply_gain` runs gain then limiter on mono and stereo paths |
| `waved/crates/waved-audio/src/uac/control.rs` | modify | Mic-side `max_pct` argument 400→800 at all 3 call sites; speaker stays 100 |
| `waved/crates/waved-audio/src/stats_collector.rs` | modify | Add `recording_limiter_reduction_db: f32` to `AudioStats`; populate from `min(mono.last_reduction_db(), stereo.last_reduction_db())` |
| `waved/crates/waved-audio/src/playback/dsp/limiter.rs` | unchanged | Reused as-is. Already public. |
| `waved/scripts/test-mic-volume.sh` | modify | Update expected `mic_gain_linear`/`mic_gain_db` for the new ceiling; add a stage that confirms `recording_limiter_reduction_db` < 0 under loud input and = 0 at idle |
| `waved/docs/AUDIO_HARDWARE.md` | modify | Update "Mic Gain Pipeline" section: 0..=800 mic ceiling, recording-side limiter step, ~2 ms lookahead-latency note |

---

## Task 1: `apply_mic_volume` — raise cap 400→800, remove hard clamp

**Files:**
- Modify: `waved/crates/waved-audio/src/lib.rs` (function at line 147; tests in `#[cfg(test)] mod tests` block)

- [ ] **Step 1: Update existing tests in `lib.rs`**

In `waved/crates/waved-audio/src/lib.rs`, edit the existing `#[cfg(test)] mod tests` block (current tests are around lines 630–660):

**Delete** the test `apply_mic_volume_clamps_to_plus_minus_unity` (the hard clamp it asserts is being removed).

**Rename + retarget** `apply_mic_volume_clamps_input_pct_above_400`:

Old:
```rust
#[test]
fn apply_mic_volume_clamps_input_pct_above_400() {
    let mut a = vec![0.1];
    let mut b = vec![0.1];
    apply_mic_volume(&mut a, 500);
    apply_mic_volume(&mut b, 400);
    assert_eq!(a, b);
}
```

New:
```rust
#[test]
fn apply_mic_volume_caps_pct_at_800() {
    let mut a = vec![0.001];
    let mut b = vec![0.001];
    apply_mic_volume(&mut a, 900);
    apply_mic_volume(&mut b, 800);
    assert_eq!(a, b, "pct above 800 should produce same output as 800");
}
```

**Add** a new test next to the others:
```rust
#[test]
fn apply_mic_volume_no_longer_clamps_output() {
    let mut s = vec![0.5_f32];
    apply_mic_volume(&mut s, 800);
    // Hard clamp removed; limiter handles ceiling downstream.
    // 0.5 × 8 = 4.0 — should pass through.
    assert!((s[0] - 4.0).abs() < 1e-6, "expected 4.0, got {}", s[0]);
}
```

(The other three tests — `apply_mic_volume_unity_is_identity`, `apply_mic_volume_doubles_at_200`, `apply_mic_volume_silences_at_zero` — keep unchanged.)

- [ ] **Step 2: Run tests to verify failures**

Run: `cargo test -p waved-audio --lib apply_mic_volume`
Expected: 3 of the new/updated tests fail; the unchanged 3 still pass:
- `apply_mic_volume_no_longer_clamps_output` — FAIL: assertion `(s[0] - 4.0).abs() < 1e-6` fails because current code clamps to 1.0.
- `apply_mic_volume_caps_pct_at_800` — FAIL: at `pct=900` and `pct=800`, current code clamps both to 400, so `a == b` still holds with values 0.004; this test passes spuriously today. To make it actually fail before implementation, you can include a sanity assertion line. Add immediately above the equality check: `assert!(a[0] > 0.005, "values should reflect 8× gain (0.001 × 8 = 0.008)");` — that line will FAIL on the unmodified code (which produces ~0.004 = clamped to 4×).

Adjusted test body:
```rust
#[test]
fn apply_mic_volume_caps_pct_at_800() {
    let mut a = vec![0.001];
    let mut b = vec![0.001];
    apply_mic_volume(&mut a, 900);
    apply_mic_volume(&mut b, 800);
    assert!(a[0] > 0.005, "expected ~0.008 at pct=800, got {}", a[0]);
    assert_eq!(a, b, "pct above 800 should produce same output as 800");
}
```

After this adjustment, both new/changed tests fail before the implementation change.

- [ ] **Step 3: Update `apply_mic_volume`**

In `waved/crates/waved-audio/src/lib.rs`, replace the existing function (around line 147):

Old:
```rust
pub fn apply_mic_volume(samples: &mut [f32], pct: u16) {
    let pct = pct.min(400);
    let g = pct as f32 / 100.0;
    for s in samples.iter_mut() {
        *s = (*s * g).clamp(-1.0, 1.0);
    }
}
```

New:
```rust
/// Apply user mic volume to a slice of mono f32 samples in-place.
/// `pct` is linear percent: 100 = unity, 200 = 2×, 0 = silence.
/// Values above 800 are clamped to 800 to prevent catastrophic gain.
/// The output is **not** clamped to [-1.0, 1.0]; the recording-path
/// limiter (see `apply_gain`) is responsible for taming peaks above
/// the limiter's ceiling.
pub fn apply_mic_volume(samples: &mut [f32], pct: u16) {
    let pct = pct.min(800);
    let g = pct as f32 / 100.0;
    for s in samples.iter_mut() {
        *s = *s * g;
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p waved-audio --lib apply_mic_volume`
Expected: 5 tests pass (4 unchanged + 1 new).

Also run the full lib test suite to confirm nothing else regressed:
Run: `cargo test -p waved-audio --lib`
Expected: PASS (the integration test `process_frame_applies_mic_volume_to_mono` from the prior project still passes — it asserts a 2× ratio, not absolute clamp behavior).

- [ ] **Step 5: Update the matching `.min(400)` in `stats_collector.rs`**

The stats collector clamps `mic_volume` for its `mic_gain_linear` computation. Find the line in `waved/crates/waved-audio/src/stats_collector.rs` (currently line 199):

Old:
```rust
    let mic_gain_linear = (audio_state.mic_volume.min(400) as f32) / 100.0;
```

New:
```rust
    let mic_gain_linear = (audio_state.mic_volume.min(800) as f32) / 100.0;
```

Run: `cargo test -p waved-audio --lib stats`
Expected: PASS — the existing `audio_stats_includes_mic_gain_when_volume_set` test passes `mic_volume = 200` which is well below both 400 and 800; behavior unchanged.

- [ ] **Step 6: Commit**

```bash
git add waved/crates/waved-audio/src/lib.rs waved/crates/waved-audio/src/stats_collector.rs
git commit -m "feat(audio): raise apply_mic_volume cap 400→800, drop hard clamp

The hard clamp at ±1.0 was the source of clip distortion observed during
mic-volume-pipeline acceptance. The recording-path limiter (next task)
takes responsibility for ceiling enforcement so gain can multiply freely.
Also bumps stats_collector's matching .min(400) → .min(800)."
```

---

## Task 2: `AudioService` gains `recording_limiter_mono` + `recording_limiter_stereo`; `apply_gain` runs gain → limiter

**Files:**
- Modify: `waved/crates/waved-audio/src/lib.rs` (`AudioService` struct, `new`, `apply_gain` helper)

- [ ] **Step 1: Add the limiter imports near the top of `lib.rs`**

Find the existing `use` block in `waved/crates/waved-audio/src/lib.rs` and add:

```rust
use crate::playback::dsp::limiter::{LimiterParams, LookaheadLimiter};
```

(Place it alongside other `use crate::...` lines. Order doesn't matter; alphabetical is fine.)

- [ ] **Step 2: Add fields to `AudioService`**

Find `pub struct AudioService { ... }` (around line 142). Add at the end of the struct, before the closing brace:

```rust
    /// Recording-path peak limiter for the mono mic signal. Replaces the
    /// hard ±1.0 clamp that used to live in `apply_mic_volume`. SP#5
    /// voice defaults: -1 dBFS ceiling, 96-sample lookahead, 50 ms release.
    recording_limiter_mono: LookaheadLimiter,
    /// Same shape, but for the stereo mic pair (only exercised when beam
    /// mode is `Stereo`). Shared gain across L/R preserves stereo image.
    recording_limiter_stereo: LookaheadLimiter,
```

- [ ] **Step 3: Construct the limiters in `AudioService::new`**

Find `impl AudioService { pub fn new(...) -> Self { ... } }` (around line 173). At the end of the body, add the two limiters to the returned struct literal. The existing struct literal looks roughly like:

```rust
        Self {
            audio_tx,
            _frame_tx: frame_tx,
            frame_rx,
            meeting_tx,
            stats_tee,
            processed_tx,
            frame_size,
            sample_rate,
            // ... other fields ...
        }
```

Add the two new fields:

```rust
            recording_limiter_mono: LookaheadLimiter::new(
                LimiterParams::DEFAULT, 1, sample_rate as f32,
            ),
            recording_limiter_stereo: LookaheadLimiter::new(
                LimiterParams::DEFAULT, 2, sample_rate as f32,
            ),
```

(Place them at the end of the struct literal, in the same order as the field declarations from Step 2.)

- [ ] **Step 4: Build to confirm `AudioService` still compiles**

Run: `cargo build -p waved-audio --lib`
Expected: builds cleanly. (No tests yet — wiring is next.)

- [ ] **Step 5: Update `apply_gain` to run the limiters after gain**

Find `apply_gain` (around line 446):

Old:
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

The limiters are mutable fields on `AudioService`, so `apply_gain` needs `&mut svc` instead of `&svc`. Update the signature and body:

New:
```rust
fn apply_gain(svc: &mut AudioService, mut processed: ProcessedAudioFrame) -> ProcessedAudioFrame {
    let mic_vol = svc.audio_tx.borrow().mic_volume;
    // Mono: gain → limiter.
    apply_mic_volume(&mut processed.mic_mono_f32, mic_vol);
    svc.recording_limiter_mono.process_interleaved(&mut processed.mic_mono_f32);
    // Stereo (rare; only Stereo beam mode): gain on each channel, then
    // shared-gain limiter on the interleaved pair to preserve image.
    if let Some((l, r)) = processed.mic_stereo_f32.as_mut() {
        apply_mic_volume(l, mic_vol);
        apply_mic_volume(r, mic_vol);
        let mut interleaved: Vec<f32> = Vec::with_capacity(l.len() * 2);
        for i in 0..l.len() {
            interleaved.push(l[i]);
            interleaved.push(r[i]);
        }
        svc.recording_limiter_stereo.process_interleaved(&mut interleaved);
        for (i, chunk) in interleaved.chunks_exact(2).enumerate() {
            l[i] = chunk[0];
            r[i] = chunk[1];
        }
    }
    processed
}
```

- [ ] **Step 6: Update both `apply_gain` callsites in `process_frame`**

Find the two `apply_gain(self, p)` calls in `process_frame` (one in the 8-ch path, one in the legacy path — Task 2 of the previous project added them). Change both to `apply_gain(self, p)` → `apply_gain(self, p)` is already the form, but `self` is now used as `&mut self`. The borrow checker will require `&mut self` access. Fortunately `process_frame` is already `pub fn process_frame(&mut self, ...)` so passing `self` mutably is fine.

If the compiler reports issues with split borrows (e.g., "cannot borrow `*self` as mutable more than once"), replace the call sites with explicit two-line forms that avoid holding intermediate borrows. Example for the legacy path:

Old:
```rust
            let p = ProcessedAudioFrame::from_raw(frame, &aec_out, &self.ref_mono_scratch);
            apply_gain(self, p)
```

New (only if a borrow conflict arises):
```rust
            let p = ProcessedAudioFrame::from_raw(frame, &aec_out, &self.ref_mono_scratch);
            apply_gain(self, p)
```

(In practice, the existing form should work — `apply_gain` takes the frame by value, so the only mutable borrow on `self` is the one for `apply_gain` itself.)

- [ ] **Step 7: Add the integration test for the limiter**

Append to the `#[cfg(test)] mod tests` block in `waved/crates/waved-audio/src/lib.rs`:

```rust
#[test]
fn process_frame_limiter_holds_peak_below_ceiling() {
    use crate::callbacks::AudioFrame;
    use std::sync::Arc;
    use tokio::sync::{mpsc, watch};
    use waved_state::AudioState;

    // mic_volume = 800 (max). Feed an i16 sine that reaches ±16384 (= ±0.5
    // in f32 after /32768). Post-gain peak is ~4.0, far above the limiter's
    // -1 dBFS ceiling (≈0.891).
    let mut state = AudioState::default();
    state.mic_volume = 800;
    let (audio_tx, _audio_rx) = watch::channel(state);
    let (frame_tx, frame_rx) = mpsc::channel(8);
    let (meeting_tx, _meeting_rx) = mpsc::channel(8);
    let mut svc = AudioService::new(
        Arc::new(audio_tx), frame_tx, frame_rx, meeting_tx,
        256, 48_000, crate::beam::MicGeometry::default(),
    );

    // Push enough frames through to flush the 96-sample lookahead.
    let mut max_post_warmup = 0.0_f32;
    for frame_idx in 0..6_u32 {
        let mut raw = vec![0i16; 256];
        for (i, s) in raw.iter_mut().enumerate() {
            let phase = (frame_idx as usize * 256 + i) as f32;
            *s = (16384.0 * (2.0 * std::f32::consts::PI * 1000.0 * phase / 48_000.0).sin()) as i16;
        }
        let frame = AudioFrame::mic_only(raw, 1, 48_000, frame_idx as u64);
        let processed = svc.process_frame(&frame);
        // Skip the first 96 output samples (lookahead warmup).
        let start = if frame_idx == 0 { 96 } else { 0 };
        for &v in &processed.mic_mono_f32[start..] {
            max_post_warmup = max_post_warmup.max(v.abs());
        }
    }
    let ceiling = 10.0_f32.powf(-1.0 / 20.0); // ≈0.891
    assert!(
        max_post_warmup <= ceiling + 0.02,
        "post-warmup peak {max_post_warmup} exceeded -1 dBFS ceiling {ceiling}"
    );
    // Also confirm the limiter actually did something — output isn't silent.
    assert!(
        max_post_warmup > 0.5,
        "post-warmup peak {max_post_warmup} suspiciously low (limiter eating the signal?)"
    );
}

#[test]
fn process_frame_quiet_signal_passes_with_only_gain_post_warmup() {
    use crate::callbacks::AudioFrame;
    use std::sync::Arc;
    use tokio::sync::{mpsc, watch};
    use waved_state::AudioState;

    // Quiet signal: i16 amplitude ±200 (= ±0.0061 in f32). At mic_volume=200
    // (boot default), post-gain ≈ ±0.0122 — well below the -1 dBFS ceiling.
    // Limiter should be idle and the output should reflect 2× gain.
    let mut state = AudioState::default();
    state.mic_volume = 200;
    let (audio_tx, _audio_rx) = watch::channel(state);
    let (frame_tx, frame_rx) = mpsc::channel(8);
    let (meeting_tx, _meeting_rx) = mpsc::channel(8);
    let mut svc = AudioService::new(
        Arc::new(audio_tx), frame_tx, frame_rx, meeting_tx,
        256, 48_000, crate::beam::MicGeometry::default(),
    );

    // Two frames to flush warmup, then measure the second.
    let mut last_processed = None;
    for frame_idx in 0..2_u32 {
        let raw = vec![200i16; 256];
        let frame = AudioFrame::mic_only(raw, 1, 48_000, frame_idx as u64);
        last_processed = Some(svc.process_frame(&frame));
    }
    let p = last_processed.unwrap();
    // 200 / 32768 × 2 = 0.0122. Tolerance allows for AEC residual.
    let max = p.mic_mono_f32.iter().cloned().fold(0.0_f32, f32::max);
    assert!(max > 0.008 && max < 0.018,
        "expected ~0.012 (gain-only), got {max} (limiter may have engaged)");
}
```

- [ ] **Step 8: Run tests**

Run: `cargo test -p waved-audio --lib`
Expected: PASS — both new tests + the existing 168+. If `process_frame_quiet_signal_passes_with_only_gain_post_warmup` is flaky (AEC residual fluctuations), tighten the tolerance numbers based on observed values.

- [ ] **Step 9: Commit**

```bash
git add waved/crates/waved-audio/src/lib.rs
git commit -m "feat(audio): recording-path limiter on AudioService (mono + stereo)

apply_gain now runs apply_mic_volume → LookaheadLimiter on each path.
Mono limiter is always-on; stereo limiter is exercised when Stereo beam
mode populates mic_stereo_f32. Shared gain across L/R preserves image."
```

---

## Task 3: UAC2 mixer scale 400→800 for the mic side

**Files:**
- Modify: `waved/crates/waved-audio/src/uac/control.rs`

This is mechanical — every `read_volume`/`write_volume` callsite for the mic side currently passes `400`. Bump them all to `800`. Speaker (`100`) stays.

- [ ] **Step 1: Update test assertions in `uac/control.rs`**

Find the `#[cfg(test)] mod tests` block at the bottom of `waved/crates/waved-audio/src/uac/control.rs`. Update the assertions that depend on `max_pct=400` to use `800`:

In `pct_to_raw_unity_at_max_pct_is_max_raw`, replace the `400` block with `800`:

Old:
```rust
        assert_eq!(pct_to_raw(0, 0, 64, 400), 0);
        assert_eq!(pct_to_raw(400, 0, 64, 400), 64);
        assert_eq!(pct_to_raw(200, 0, 64, 400), 32);
        assert_eq!(pct_to_raw(100, 0, 64, 400), 16);
```

New:
```rust
        assert_eq!(pct_to_raw(0, 0, 64, 800), 0);
        assert_eq!(pct_to_raw(800, 0, 64, 800), 64);
        assert_eq!(pct_to_raw(400, 0, 64, 800), 32);
        assert_eq!(pct_to_raw(200, 0, 64, 800), 16);
```

In `raw_to_pct_inverse`:

Old:
```rust
        assert_eq!(raw_to_pct(64, 0, 64, 400), 400);
        assert_eq!(raw_to_pct(32, 0, 64, 400), 200);
```

New:
```rust
        assert_eq!(raw_to_pct(64, 0, 64, 800), 800);
        assert_eq!(raw_to_pct(32, 0, 64, 800), 400);
```

In `pct_to_raw_clamps_above_max_pct`:

Old:
```rust
        assert_eq!(pct_to_raw(500, 0, 64, 400), 64);
```

New (clamp at the new ceiling):
```rust
        assert_eq!(pct_to_raw(900, 0, 64, 800), 64);
```

(The `assert_eq!(pct_to_raw(150, 0, 64, 100), 64);` line in the same test is for the speaker-side scale and stays unchanged.)

- [ ] **Step 2: Update mic-side callsites**

In `waved/crates/waved-audio/src/uac/control.rs`, find every callsite where the `capture` argument to `read_volume`/`write_volume` is `true` (these are the mic-side calls) and bump the `max_pct` argument from `400` to `800`.

Expected callsites (match the existing structure from Task 7 of the prior project):

In `run_mixer_poll`:

Old:
```rust
    let mut prev_mic_vol = read_volume(&mixer, "PCM", true, 400).unwrap_or(0);
```
```rust
        if let Some(v) = read_volume(&mixer, "PCM", true, 400) {
```

New:
```rust
    let mut prev_mic_vol = read_volume(&mixer, "PCM", true, 800).unwrap_or(0);
```
```rust
        if let Some(v) = read_volume(&mixer, "PCM", true, 800) {
```

In `run_state_mirror`:

Old (initial sync block):
```rust
        let _ = write_volume(&mixer, "PCM", true, prev_mic_vol, 400);
```
```rust
                let _ = write_volume(&mixer, "PCM", true, s.mic_volume, 400);
```

New:
```rust
        let _ = write_volume(&mixer, "PCM", true, prev_mic_vol, 800);
```
```rust
                let _ = write_volume(&mixer, "PCM", true, s.mic_volume, 800);
```

(Speaker callsites — `capture=false`, `max_pct=100` — stay unchanged.)

- [ ] **Step 3: Run the unit tests**

Run: `cargo test -p waved-audio --lib`
Expected: PASS — the updated `pct_to_raw`/`raw_to_pct` tests pass with the new constants. (Note: the uac module is gated behind `#[cfg(feature = "capture-alsa")]`; on macOS without ALSA headers these tests don't compile, but the conversion math is verifiable by inspection. A Linux/device build will exercise them.)

Run: `cargo check -p waved-audio --lib`
Expected: clean compile (uac module gated out under default features).

- [ ] **Step 4: Commit**

```bash
git add waved/crates/waved-audio/src/uac/control.rs
git commit -m "feat(audio/uac): bump mic max_pct 400→800 to match new gain ceiling

Recording-path limiter now lets software gain safely exceed 4×.
UAC2 mic mixer maps Mac slider 100% to internal 800; 50% stays at 200
(boot default unchanged)."
```

---

## Task 4: Stats — emit `recording_limiter_reduction_db`

**Files:**
- Modify: `waved/crates/waved-audio/src/playback.rs` (`DspMetrics` struct — add new atomic)
- Modify: `waved/crates/waved-audio/src/lib.rs` (`AudioService` — add optional `dsp_metrics`; `apply_gain` — write limiter reduction)
- Modify: `waved/crates/waved-audio/src/stats_collector.rs` (`AudioStats` field; `compute_stats_with_processed` reads from `DspMetrics`)
- Modify: `waved/crates/waved/src/main.rs` (wire `playback.dsp_metrics()` into `AudioService` via setter)

The recording limiter's reduction value lives on `AudioService` (per Task 2). To surface it in `/tmp/audio_stats.json`, we extend the existing `DspMetrics` struct (which already plumbs `limiter_reduction_db_x100` for playback) with a parallel atomic for recording. `AudioService` writes to the atomic after each `apply_gain`; `compute_stats_with_processed` reads from the same `dsp_metrics: Option<&DspMetrics>` argument it already accepts. No new function-signature parameter needed.

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `waved/crates/waved-audio/src/stats_collector.rs`:

```rust
#[test]
fn audio_stats_recording_limiter_reduction_db_zero_when_metrics_absent() {
    use waved_state::AudioState;
    let frame = AudioFrame::mic_only(vec![0i16; 8 * 256], 8, 48_000, 0);
    let cs = CaptureStats::new();
    let state = AudioState::default();
    let stats = compute_stats_with_processed(
        &frame, &cs, ChannelMapSource::File, None, "none",
        Some(123.4), &state, None,
        #[cfg(feature = "capture-alsa")]
        None,
    );
    assert_eq!(stats.recording_limiter_reduction_db, 0.0);
}

#[test]
fn audio_stats_recording_limiter_reduction_db_reads_from_dsp_metrics() {
    use std::sync::atomic::Ordering;
    use waved_state::AudioState;
    let frame = AudioFrame::mic_only(vec![0i16; 8 * 256], 8, 48_000, 0);
    let cs = CaptureStats::new();
    let state = AudioState::default();
    let m = crate::playback::DspMetrics::default();
    // Limiter pulled signal down by 3.5 dB → store as -350 (×100).
    m.recording_limiter_reduction_db_x100.store(-350, Ordering::Relaxed);
    let stats = compute_stats_with_processed(
        &frame, &cs, ChannelMapSource::File, None, "none",
        None, &state, Some(&m),
        #[cfg(feature = "capture-alsa")]
        None,
    );
    assert!((stats.recording_limiter_reduction_db - (-3.5)).abs() < 1e-6,
        "got {}", stats.recording_limiter_reduction_db);
}
```

- [ ] **Step 2: Run tests to verify failures**

Run: `cargo test -p waved-audio --lib audio_stats_recording_limiter`
Expected: FAIL — field on `AudioStats` and atomic on `DspMetrics` don't exist yet.

- [ ] **Step 3: Add the atomic to `DspMetrics`**

In `waved/crates/waved-audio/src/playback.rs`, find `pub struct DspMetrics { ... }` (currently line 68). Add a third atomic field:

```rust
#[derive(Debug, Default)]
pub struct DspMetrics {
    /// Per-band MBAGC gain (dB), packed `[b0, b1, b2, b3]` as fixed-point
    /// i32 = `dB × 100`. Atomic updates — readers may see partial state for
    /// a single 500 ms stats tick, but values rarely move that fast.
    pub mbagc_gain_db_x100: [AtomicI32; 4],
    /// Limiter peak attenuation over the most recent block (`dB × 100`,
    /// negative = attenuating).
    pub limiter_reduction_db_x100: AtomicI32,
    /// Recording-path limiter peak attenuation over the most recent block
    /// (`dB × 100`, negative = attenuating). Written by the audio service
    /// after `apply_gain`; 0 when no limiting was needed.
    pub recording_limiter_reduction_db_x100: AtomicI32,
}
```

(The existing `#[derive(Default)]` covers the new field — `AtomicI32` defaults to 0.)

- [ ] **Step 4: Add the field to `AudioStats` and read it in `compute_stats_with_processed`**

In `waved/crates/waved-audio/src/stats_collector.rs`, add a field to `AudioStats` (immediately after `mic_gain_db`):

Old:
```rust
    /// Same as `mic_gain_linear` but expressed in dB.
    /// -120.0 when `mic_volume` is 0 (sentinel for -inf; kept finite for JSON).
    pub mic_gain_db: f32,
    #[cfg(feature = "capture-alsa")]
```

New:
```rust
    /// Same as `mic_gain_linear` but expressed in dB.
    /// -120.0 when `mic_volume` is 0 (sentinel for -inf; kept finite for JSON).
    pub mic_gain_db: f32,
    /// Peak attenuation (dB, negative) the recording limiter applied during
    /// the last collection block. 0.0 when no limiting was needed.
    pub recording_limiter_reduction_db: f32,
    #[cfg(feature = "capture-alsa")]
```

In the function body of `compute_stats_with_processed` (around line 183, where `dsp_metrics` is already destructured for `limiter_reduction_db`), extend that block to also read the new atomic. Find the existing block:

```rust
    let (mbagc_active_gain_db, limiter_reduction_db) = if let Some(m) = dsp_metrics {
        let gains = [
            m.mbagc_gain_db_x100[0].load(Ordering::Relaxed) as f32 / 100.0,
            m.mbagc_gain_db_x100[1].load(Ordering::Relaxed) as f32 / 100.0,
            m.mbagc_gain_db_x100[2].load(Ordering::Relaxed) as f32 / 100.0,
            m.mbagc_gain_db_x100[3].load(Ordering::Relaxed) as f32 / 100.0,
        ];
        let limiter = m.limiter_reduction_db_x100.load(Ordering::Relaxed) as f32 / 100.0;
        (gains, limiter)
    } else {
        ([0.0_f32; 4], 0.0_f32)
    };
```

Replace with (extending the tuple to three values):

```rust
    let (mbagc_active_gain_db, limiter_reduction_db, recording_limiter_reduction_db) =
        if let Some(m) = dsp_metrics {
            let gains = [
                m.mbagc_gain_db_x100[0].load(Ordering::Relaxed) as f32 / 100.0,
                m.mbagc_gain_db_x100[1].load(Ordering::Relaxed) as f32 / 100.0,
                m.mbagc_gain_db_x100[2].load(Ordering::Relaxed) as f32 / 100.0,
                m.mbagc_gain_db_x100[3].load(Ordering::Relaxed) as f32 / 100.0,
            ];
            let limiter = m.limiter_reduction_db_x100.load(Ordering::Relaxed) as f32 / 100.0;
            let rec_limiter = m.recording_limiter_reduction_db_x100.load(Ordering::Relaxed) as f32 / 100.0;
            (gains, limiter, rec_limiter)
        } else {
            ([0.0_f32; 4], 0.0_f32, 0.0_f32)
        };
```

In the `AudioStats { ... }` literal, add the new field (immediately after `mic_gain_db`):

```rust
        recording_limiter_reduction_db,
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p waved-audio --lib audio_stats_recording_limiter`
Expected: PASS — both new tests.

Run: `cargo test -p waved-audio --lib stats`
Expected: PASS — the two new + all pre-existing stats tests. (`compute_stats_with_processed`'s signature is unchanged, so existing test callers don't need updates.)

- [ ] **Step 6: Add `dsp_metrics` to `AudioService` and wire `apply_gain`**

In `waved/crates/waved-audio/src/lib.rs`, add a new field to `pub struct AudioService` (next to the limiter fields from Task 2):

```rust
    /// Optional handle to the shared DSP metrics. Set by main once both the
    /// audio and playback services exist (they share the same Arc). When
    /// present, `apply_gain` writes the recording limiter's reduction into
    /// it after each frame; the stats collector reads from the same Arc.
    dsp_metrics: Option<Arc<crate::playback::DspMetrics>>,
```

Initialise to `None` in `AudioService::new`'s returned struct literal:

```rust
            dsp_metrics: None,
```

Add a setter on `impl AudioService`:

```rust
    /// Wire the shared DSP metrics handle. Call once at startup, after
    /// `PlaybackService` is constructed (since playback owns the Arc).
    pub fn set_dsp_metrics(&mut self, metrics: Arc<crate::playback::DspMetrics>) {
        self.dsp_metrics = Some(metrics);
    }
```

In `apply_gain` (Task 2), after running both limiters and before returning `processed`, write the worst-case reduction to the atomic:

```rust
    if let Some(metrics) = svc.dsp_metrics.as_ref() {
        let mono = svc.recording_limiter_mono.last_reduction_db();
        let stereo = if processed.mic_stereo_f32.is_some() {
            svc.recording_limiter_stereo.last_reduction_db()
        } else {
            0.0
        };
        let worst = mono.min(stereo);
        metrics.recording_limiter_reduction_db_x100.store(
            (worst * 100.0) as i32,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
```

Place this block immediately before the final `processed` return.

- [ ] **Step 7: Wire the metrics handle in `main.rs`**

In `waved/crates/waved/src/main.rs`, find where `dsp_metrics_handle` is set (currently line 275, inside the `capture-alsa` block, immediately after `PlaybackService` construction):

```rust
            dsp_metrics_handle = Some(playback.dsp_metrics());
```

Immediately after that line, add:

```rust
            audio_svc.set_dsp_metrics(playback.dsp_metrics());
```

(`audio_svc` is the `AudioService` instance constructed earlier in the same block. The `Arc` from `playback.dsp_metrics()` is cheap to clone — both services share the same metrics struct.)

- [ ] **Step 8: Run all tests + build**

Run: `cargo test -p waved-audio --lib`
Expected: PASS — including the two new tests.

Run: `cargo build -p waved`
Expected: clean build.

- [ ] **Step 9: Commit**

```bash
git add waved/crates/waved-audio/src/playback.rs waved/crates/waved-audio/src/lib.rs waved/crates/waved-audio/src/stats_collector.rs waved/crates/waved/src/main.rs
git commit -m "feat(audio/stats): emit recording_limiter_reduction_db via DspMetrics

Surfaces the recording-path limiter's per-block peak attenuation in
/tmp/audio_stats.json so on-device acceptance can confirm the limiter
is doing real work under loud input. Threaded via DspMetrics for parity
with playback's limiter_reduction_db; AudioService and PlaybackService
share the same Arc."
```

---

## Task 5: On-device acceptance script + AUDIO_HARDWARE.md

**Files:**
- Modify: `waved/scripts/test-mic-volume.sh`
- Modify: `waved/docs/AUDIO_HARDWARE.md`

- [ ] **Step 1: Update the script's expected gain values**

In `waved/scripts/test-mic-volume.sh`, update the four `check_stage` invocations to reflect the new ceiling. Old (around line 60):

```bash
run check_stage "host capture 50% (boot default → mic_volume=200)"   50 2.0  6.02
run check_stage "host capture 25% (→ mic_volume=100, unity)"          25 1.0  0.0
run check_stage "host capture 100% (→ mic_volume=400, +12 dB)"       100 4.0 12.04
run check_stage "host capture 0%  (→ mic_volume=0,   −120 dB sentinel)" 0 0.0 -120.0
```

New:
```bash
run check_stage "host capture 50% (boot default → mic_volume=200)"   50 2.0  6.02
run check_stage "host capture 25% (→ mic_volume=100, unity)"          25 1.0  0.0
run check_stage "host capture 100% (→ mic_volume=800, +18 dB)"       100 8.0 18.06
run check_stage "host capture 0%  (→ mic_volume=0,   −120 dB sentinel)" 0 0.0 -120.0
```

- [ ] **Step 2: Add a limiter-effectiveness stage**

After the four existing stages and before "reset to boot default", add a new section:

```bash
echo
echo "=== recording limiter (idle) ==="
amixer -c 1 sset 'PCM' 50% capture > /dev/null
sleep 1
RED_IDLE=$(grep -oE '"recording_limiter_reduction_db":[^,}]*' /tmp/audio_stats.json | sed -E 's/.*: *([0-9.-]+).*/\1/')
echo "  idle reduction: $RED_IDLE dB (expected ~0.0 with no input)"
if awk -v v="$RED_IDLE" 'BEGIN { exit (v > -0.1 ? 0 : 1) }'; then
    echo "  PASS"
    PASS=$((PASS+1))
else
    echo "  FAIL (limiter is reducing at idle — investigate)"
    FAIL=$((FAIL+1))
fi

echo
echo "=== recording limiter (loud — manual: speak loudly close to the mic for 3s) ==="
amixer -c 1 sset 'PCM' 100% capture > /dev/null
echo "  *** speak loudly now, 3 second window ***"
sleep 3
RED_LOUD=$(grep -oE '"recording_limiter_reduction_db":[^,}]*' /tmp/audio_stats.json | sed -E 's/.*: *([0-9.-]+).*/\1/')
echo "  loud reduction: $RED_LOUD dB (expected < 0 if limiter engaged)"
if awk -v v="$RED_LOUD" 'BEGIN { exit (v < -0.1 ? 0 : 1) }'; then
    echo "  PASS"
    PASS=$((PASS+1))
else
    echo "  WARN (limiter not engaged — your input may not have been loud enough; not a hard fail)"
fi
```

(The "loud" stage is a soft assert because it depends on operator action — speaking loudly enough to push the limiter. A FAIL at idle is real; a WARN at loud is expected if no one spoke.)

- [ ] **Step 3: Update `AUDIO_HARDWARE.md`**

In `waved/docs/AUDIO_HARDWARE.md`, find the existing "## Mic Gain Pipeline" section (added in the prior project). Update the "Software gain" bullet to mention the new ceiling and the limiter:

Find:
```markdown
2. **Software gain — `apply_mic_volume`** at the end of `process_frame`.
   Linear-percent value driven by `AudioState.mic_volume`. `100` = unity,
   `200` = 2× / +6 dB, range 0..=400. Applied to `mic_mono_f32` and
   `mic_stereo_f32`, so both meeting recording and the UAC2 uplink share
   one knob.
```

Replace with:
```markdown
2. **Software gain — `apply_mic_volume`** then **recording-path limiter** at
   the end of `process_frame`. Gain is a linear-percent value driven by
   `AudioState.mic_volume`: `100` = unity, `200` = 2× / +6 dB, range
   0..=800 (8× / +18 dB max). The output of the gain stage is no longer
   clamped to ±1.0 — instead, a `LookaheadLimiter` (-1 dBFS ceiling, 96
   sample lookahead, 50 ms release; same unit as the playback DSP from
   SP#5) compresses peaks above the ceiling. Two limiter instances live
   on `AudioService`: a mono limiter for `mic_mono_f32` and a 2-channel
   stereo limiter for `mic_stereo_f32` (shared gain across L/R preserves
   stereo image). Both meeting recording and the UAC2 uplink see the same
   limited signal.

   Recording-path latency added by the lookahead: ~2 ms (96 samples at
   48 kHz). Imperceptible.
```

Update the UAC2 mixer mapping note to use the new range:

Find:
```markdown
The UAC2 mixer (`hw:1 PCM Capture Volume`) maps its raw range to our
`0..=400` so the host's "max slider" corresponds to internal `mic_volume=400`,
and the boot default of `200` sits in the middle of the host's range.
```

Replace with:
```markdown
The UAC2 mixer (`hw:1 PCM Capture Volume`) maps its raw range to our
`0..=800` so the host's "max slider" corresponds to internal `mic_volume=800`
(+18 dB), and the boot default of `200` sits at 25 % of the host's range.
```

- [ ] **Step 4: Push and run on-device**

```bash
adb push waved/scripts/test-mic-volume.sh /data/test-mic-volume.sh
adb shell "chmod +x /data/test-mic-volume.sh && sh /data/test-mic-volume.sh"
```

Expected: 6/6 pass at idle (4 gain stages + idle limiter + PDM Gain). The "loud" stage is informational.

- [ ] **Step 5: Commit**

```bash
git add waved/scripts/test-mic-volume.sh waved/docs/AUDIO_HARDWARE.md
git commit -m "docs(audio): recording-path limiter — script + Mic Gain Pipeline update

Acceptance script verifies new 800% ceiling values and the limiter's
idle/loud reduction. AUDIO_HARDWARE.md documents the limiter step,
new range, and ~2 ms lookahead latency."
```

---

## Final integration check

After Task 5, full test pass plus a deploy:

```bash
cargo test -p waved-audio --lib
cargo build -p waved-audio --lib
cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
adb shell "killall waved 2>/dev/null; sleep 2"
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "(setsid /data/waved/waved >>/data/waved/waved.log 2>&1 </dev/null &)"
sleep 5
adb shell "sh /data/test-mic-volume.sh"
```

If the loud-input limiter stage doesn't engage during normal speech at Mac slider 100 %, that's the signal: the limiter ceiling at -1 dBFS is generous, and signal isn't reaching that level. If you want even more apparent loudness without distortion, the next lever is raising the ceiling further (e.g., -3 dBFS) — but that risks audible distortion on i16 quantization. Better path: bump `pdm_gain` in JSON config back up (we now know clipping comes from the gain stage, not ADC, so a small `pdm_gain` lift like 30 is fine).
