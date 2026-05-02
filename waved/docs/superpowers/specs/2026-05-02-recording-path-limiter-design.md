# Recording-Path Limiter — Design

**Status:** approved 2026-05-02
**Owner:** Xavier
**Related:** [mic-volume-pipeline](2026-05-01-mic-volume-pipeline-design.md) (this is the follow-up resolving its accepted "no soft-knee on the recording path" limitation), SP#5 playback DSP (provides the proven `LookaheadLimiter` we reuse)

## Problem

The mic-volume-pipeline ships with `apply_mic_volume` clamping output to ±1.0 hard. On-device acceptance testing surfaced two coupled symptoms:

1. **Crackle on speech peaks at high software gain.** With `mic_volume = 400` (4×, the previous max) and any post-DSP signal exceeding 0.25, the gain stage saturates to ±1.0 and produces audible clip distortion.
2. **Can't get loud enough without crackle.** Mac slider at 100 % is "louder but not loud enough"; pushing PDM Gain higher *upstream* of the gain stage just makes the gain stage clip earlier (and at PDM Gain max it also risks ADC clipping). There is no headroom path that reaches normal speech loudness without distortion.

Both symptoms are the same architectural gap: `apply_mic_volume`'s hard clamp is a clip, not a limiter. We need soft-knee dynamic range compression on the recording path so that gain can safely exceed unity ceiling and peaks are compressed instead of mangled.

## Goals

- Software gain can safely exceed today's 4× cap. Mac users get notably more loudness without distortion.
- Speech peaks above the limiter ceiling are compressed cleanly, not hard-clipped.
- Both broadcast consumers (recording WAV, UAC2 uplink) get the same treatment — single application point, just like the gain stage.
- Stereo recording (rare, only used in `Stereo` beam mode) is treated the same way as mono — no path that produces clip distortion.
- Stats expose the limiter's per-block reduction so on-device debugging can confirm it's working.

## Non-goals

- TOML/JSON-configurable limiter params (YAGNI: single use case, SP#5 voice defaults are correct for this).
- MBAGC / multi-band compression on the recording path (out of scope; playback DSP has it, recording does not need it for this project).
- EQ on the recording path.
- Fixing the cardioid back-rejection issue (separate project, task #28).

## Decisions

| Decision | Value | Rationale |
|---|---|---|
| Software-gain ceiling | 400 → **800** (8×, +18 dB) | Doubles today's headroom. Limiter behind it makes the new ceiling safe. Easy to bump to 1600 later if needed. |
| `apply_mic_volume` hard clamp | **Removed** | Limiter replaces it. Output may transiently exceed ±1.0 between gain stage and limiter — that's fine, the next sample-time the limiter compresses it. |
| Limiter unit | **Reuse SP#5's `LookaheadLimiter`** as-is | Already proven, has tests, exposes `last_reduction_db` for stats. No changes to the unit itself. |
| Limiter params | **Hardcoded** SP#5 voice defaults: ceiling=-1 dBFS, lookahead=96 samples (~2 ms), release=50 ms | Single content type (speech from this device's mic). YAGNI on a config knob nobody would tune. |
| Mono path | New `recording_limiter_mono: LookaheadLimiter` field on `AudioService`, channels=1 | Always-on; the canonical recording path. |
| Stereo path | Separate `recording_limiter_stereo: LookaheadLimiter`, channels=2, shared gain across L/R | Preserves stereo image when `Stereo` beam mode is active. |
| UAC2 mixer scale | Mic `max_pct`: 400 → **800** in `uac/control.rs` | Mac slider 100 % maps to internal 800; 50 % stays at 200 (boot default unchanged). |
| TOML default `mic_volume` | Stays **200** | Boot default loudness unchanged for users who don't move the slider. |
| Stats | Emit `recording_limiter_reduction_db: f32` | Parity with playback's `limiter_reduction_db`. Operator can verify on-device. |
| Latency added on recording path | ~2 ms (96 samples @ 48 kHz) | Imperceptible. Not a real-time path constraint. |

## Architecture

### Per-frame chain (changes in **bold**)

```
capture → beam → AEC → NS → ProcessedAudioFrame
                              │
                              ▼
              apply_mic_volume(mic_mono_f32, mic_volume)   (multiply only — no clamp)
                              │
                              ▼
   recording_limiter_mono.process_interleaved(mic_mono_f32) ── NEW
                              │
                              ▼
              if mic_stereo_f32 = Some((L, R)):
                  apply_mic_volume(L); apply_mic_volume(R)
                  interleave [L, R] → recording_limiter_stereo.process_interleaved → de-interleave
                              │
                              ▼
              processed_tx.send(processed)
                ├── meeting_tx (recording WAV)
                └── UacUpstreamSink (Mac uplink)
```

### File layout

| File | Change |
|---|---|
| `waved-audio/src/lib.rs` | `apply_mic_volume`: cap raised 400→800, hard clamp removed. `AudioService` gains `recording_limiter_mono` and `recording_limiter_stereo` fields. `apply_gain` runs gain then limiter on each path. Adds an interleave/de-interleave step around the stereo limiter. |
| `waved-audio/src/uac/control.rs` | Mic-side `max_pct` argument 400→800 at all call sites. Speaker stays 100. |
| `waved-audio/src/stats_collector.rs` | Add `recording_limiter_reduction_db: f32` to `AudioStats`. Populate from `min(mono.last_reduction_db(), stereo.last_reduction_db())` — the most-negative value, i.e. whichever limiter attenuated harder during the block. |
| `waved-audio/src/playback/dsp/limiter.rs` | Reused as-is. Already public. No edits. |
| `waved/scripts/test-mic-volume.sh` | Update expected `mic_gain_linear` / `mic_gain_db` for the new ceiling (100 % slider → 8.0 / +18.06 dB). Add a stage that confirms `recording_limiter_reduction_db` is 0 at idle and < 0 under loud input. |
| `waved/docs/AUDIO_HARDWARE.md` | Update "Mic Gain Pipeline" section: new 0..=800 mic ceiling, mention the recording-side limiter (mirroring playback), note the 2 ms lookahead latency. |

### Code-shape highlights

`apply_mic_volume` becomes:

```rust
pub fn apply_mic_volume(samples: &mut [f32], pct: u16) {
    let pct = pct.min(800);              // was 400
    let g = pct as f32 / 100.0;
    for s in samples.iter_mut() {
        *s = *s * g;                     // hard clamp removed
    }
}
```

Limiter construction (in `AudioService::new`):

```rust
const VOICE_LIMITER: LimiterParams = LimiterParams::DEFAULT;  // -1 dBFS, 96, 50 ms
recording_limiter_mono:   LookaheadLimiter::new(VOICE_LIMITER, 1, sample_rate as f32),
recording_limiter_stereo: LookaheadLimiter::new(VOICE_LIMITER, 2, sample_rate as f32),
```

Stereo interleave shape (in `apply_gain` helper):

```rust
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
```

`AudioStats` gains:

```rust
/// Peak attenuation (dB, negative) the recording limiter applied during
/// the last collection block. 0.0 when no limiting was needed.
pub recording_limiter_reduction_db: f32,
```

## Error handling

| Condition | Behavior |
|---|---|
| `mic_volume > 800` | Clamped to 800 in `apply_mic_volume` (canonical guard). |
| `mic_volume = 0` | Gain produces zeros; limiter sees zeros; output zeros. No special case. |
| NaN/Inf in upstream f32 | Trust DSP to produce finite samples. Limiter doesn't sanitize — same as before. |
| Limiter startup blip | First 96 frames after construction emit silence (lookahead fill). One-shot at daemon boot; imperceptible. |
| Sample-rate change mid-run | Not a real case. `sample_rate` is fixed in `AudioService`. |
| Stereo channel-length mismatch | Guarded by existing `ProcessedAudioFrame::with_stereo` assertions. |

## Testing

### Existing test updates (`waved-audio/src/lib.rs`)

- `apply_mic_volume_clamps_to_plus_minus_unity` — **delete** (hard clamp removed).
- `apply_mic_volume_clamps_input_pct_above_400` — **rename and retarget** to
  `apply_mic_volume_caps_pct_at_800`. Assert `pct=900` and `pct=800` produce
  identical output.
- Three other tests (`unity_is_identity`, `doubles_at_200`, `silences_at_zero`)
  unchanged — still pass.

### New unit tests

- `apply_mic_volume_no_longer_clamps_output` — `pct=800` on `[0.5]` produces
  `[4.0]` (verifies the hard clamp is gone).
- `process_frame_limiter_holds_peak_below_ceiling` — set `mic_volume=800`,
  feed a sine close to ±0.5 (so post-gain peak is ~4.0), run through
  `process_frame`. After lookahead warmup, output peak must be
  ≤ 10^(-1/20) ≈ 0.891 (-1 dBFS).
- `process_frame_quiet_signal_passes_unchanged_post_warmup` — `mic_volume=200`,
  input small enough that post-gain stays well below ceiling. Output equals
  input × 2 within tolerance (limiter idle).
- `audio_stats_includes_recording_limiter_reduction_db` — field defaults to
  0.0 when limiter is idle; populated from the limiter snapshot.

(`LookaheadLimiter` itself is already covered by SP#5 unit tests in
`playback/dsp/limiter.rs`. We do not duplicate them.)

### On-device acceptance updates

In `scripts/test-mic-volume.sh`:

| Mac slider | Internal `mic_volume` | Expected `mic_gain_linear` | Expected `mic_gain_db` |
|---|---|---|---|
| 100 % | **800** (was 400) | **8.0** (was 4.0) | **+18.06 dB** (was +12.04) |
| 50 % (boot default) | 200 (unchanged) | 2.0 | +6.02 dB |
| 25 % | 100 | 1.0 | 0 dB |

Add a new stage that confirms the limiter is actually working:

1. Set Mac slider to 100 % (loud).
2. Speak loudly close to the mic for ~3 s.
3. Read `recording_limiter_reduction_db` from `/tmp/audio_stats.json`.
4. Assert `< 0` (some attenuation occurred).

At idle (silence into the mic), `recording_limiter_reduction_db` must remain
`0.0`.

## Open follow-ups (deferred — not part of this project)

- TOML/JSON-configurable limiter params, if a future use case needs them.
- MBAGC-style compression on the recording path.
- Recording-side EQ.
- Cardioid rear-rejection investigation (task #28).
