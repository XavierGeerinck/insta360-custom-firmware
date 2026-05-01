# Mic Volume Pipeline — Design

**Status:** approved 2026-05-01
**Owner:** Xavier
**Related:** SP#5 (playback DSP), SP#6 (UAC2 routing — same `processed_tx` shared stream)

## Problem

`[audio].mic_volume` exists in `WavedConfig` and `AudioState.mic_volume` is updated
by `Command::SetMicVolume`, but **no code applies it to the audio signal**. The
UAC2 host mixer mirrors the value to `hw:1 PCM Capture Volume` for the host's
own view; the bytes we send upstream are unchanged. The hardware preamp (PDM
Gain, ALSA `numid=9`, range 0–48) is also at `0` by default, so raw mic floor
sits ~−75 dBFS and recordings come out at ~−46 dBFS even without DSP gating.

The fix is to add a real gain stage that both control paths (TOML boot default,
UAC2 host mixer at runtime) drive, and to set the hardware preamp to a sensible
fixed operating point at startup.

## Goals

- Recordings made with default settings are audible at normal listening volume.
- The Mac's UAC2 mic slider produces the audible change a user expects.
- Recording WAV and UAC2 uplink share the same gain — one knob, both paths.
- DSP tuning (AEC, DFN3) sees a stable, consistent input level so it doesn't
  re-adapt every time the user moves the volume knob.

## Non-goals

- Limiter or soft-knee on the recording path (hard-clamp only).
- On-device mic-volume settings screen.
- Persisting host-driven runtime changes across reboot.
- Fixing DFN3 over-adaptation behavior beyond "do not change PDM Gain
  dynamically" (handled by design; deeper DFN3 tuning is its own project).
- Migrating the existing TOML `mic_volume = 200` default — it now formally
  means "200 % linear", which matches the boost we wanted anyway.

## Decisions

| Decision | Value | Rationale |
|---|---|---|
| Gain placement | Hybrid: hardware static + software dynamic | Hardware preamp gives best SNR (gain before ADC). Software gain keeps DSP tuning stable because it sits *after* NS — DFN3 never sees the user's knob movements. |
| `mic_volume` semantic | Linear percent. `100` = unity (0 dB). Range 0..=400. | Readable in TOML (`200` = 2× = +6 dB). No migration needed. Easy mental model for non-audio engineers. |
| PDM Gain default | `24` (mid-range, ~+12 dB lift) | Halfway gives DFN3 a clean noise floor to model against; software gain takes it the rest of the way without ever touching hardware. |
| PDM Gain home | JSON audio config (`CaptureConfig.pdm_gain`) | Not user-editable preference — it's DSP-tuning territory, same kind of value as AEC params and DFN3 thresholds. |
| Software gain placement | `process_frame` output → `processed_tx.send()` | Single application point; both `meeting_tx` (recording) and `UacUpstreamSink` (uplink) get the same treatment automatically. |
| UI exposure | None on-device | Mac drives runtime via UAC2 mixer (already wired). On-device duplication not worth the LVGL work. |
| Persistence | TOML = boot default; runtime UAC2 changes are *not* written back | Hosts manage their own per-session mic level. Avoids file-write churn on every UAC2 mixer event. |
| UAC2 mixer range | Mixer exposes `0..=400` to the host with default `200` | Both control paths (TOML, UAC2) share the same value space. Otherwise host's "100% slider" = `100` would silently drop 6 dB vs. boot default. |

## Architecture

### Two-stage gain

```
┌──────────────────────────────────────────────────────────────────┐
│ Boot (one-shot)                                                  │
│   TOML  → WavedConfig.audio.mic_volume → AudioState.mic_volume   │
│   JSON  → CaptureConfig.pdm_gain        → ALSA numid=9           │
└──────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────┐
│ Per frame (audio_srv loop)                                       │
│                                                                  │
│   capture → beam → AEC → NS → ProcessedAudioFrame                │
│                                       │                          │
│                                       ▼                          │
│              apply_mic_volume(&mut .mic_mono_f32,                │
│                                state.mic_volume)                 │
│                                       │                          │
│                                       ▼                          │
│                          processed_tx.send(processed)            │
│                                  ├── meeting_tx  (recording WAV) │
│                                  └── UacUpstreamSink (uplink)    │
└──────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────┐
│ Runtime (Mac turns mic knob in System Settings)                  │
│                                                                  │
│   UAC2 kernel mixer event                                        │
│     → uac::control polls (existing)                              │
│     → Command::SetMicVolume(v)        v ∈ 0..=400 (mixer range)  │
│     → audio_srv main loop                                        │
│     → audio_tx.send_modify(|s| s.mic_volume = v)                 │
│     → next frame's gain stage reads new value (no buffering)     │
└──────────────────────────────────────────────────────────────────┘
```

### File layout

| Component | Location | Responsibility |
|---|---|---|
| Software gain stage | `waved-audio/src/lib.rs` (in `run` loop, after `process_frame`, before `processed_tx.send`) | Apply gain to `processed.mic_mono_f32`, hard-clamp to ±1.0 |
| `apply_mic_volume(samples, pct)` free fn | `waved-audio/src/lib.rs` | Pure — easy to unit-test |
| PDM Gain initializer | `waved-audio/src/capture/pdm_gain.rs` (new) | One-shot ALSA mixer write at capture thread startup |
| JSON schema field | `waved-audio/src/audio_config.rs` (`CaptureConfig`) | New `pdm_gain: u8`, default 24, validated 0..=48 |
| JSON default | `waved-audio/config/audio_config_default.json` | Add `"pdm_gain": 24` |
| TOML → state seed | `waved/src/main.rs` (where `AudioState` is constructed) | Replace default `mic_volume` with `cfg.audio.mic_volume` |
| UAC2 mixer setup | `waved-audio/src/uac/control.rs` (existing mixer init) | Set PCM Capture Volume range to `0..=400`, default `200` |
| Stats | `waved-audio/src/stats_collector.rs` | Emit `mic_gain_linear` (f32) and `mic_gain_db` (f32) |
| Hardware doc | `waved/docs/AUDIO_HARDWARE.md` | Two-stage gain explanation; PDM Gain operating point rationale; `mic_volume` semantic |

## Schema additions

### JSON (`audio_config_default.json`)

```json
"capture": {
  "...existing fields...",
  "pdm_gain": 24
}
```

### JSON validation (in `AudioConfig::validate`)

```rust
if self.capture.pdm_gain > 48 {
    return Err(ConfigError::Invalid(format!(
        "pdm_gain must be in [0, 48], got {}", self.capture.pdm_gain
    )));
}
```

### TOML

Unchanged. `mic_volume = 200` already exists as default and is now formally
documented as "200 % linear (2× / +6 dB)". No migration.

### Software-gain math

```rust
/// Apply user mic volume to a slice of mono f32 samples in-place.
/// `pct` is linear percent: 100 = unity, 200 = 2×, 0 = silence.
/// Values above 400 are clamped to 400 to prevent catastrophic gain.
/// Output samples are hard-clamped to [-1.0, 1.0].
pub fn apply_mic_volume(samples: &mut [f32], pct: u16) {
    let pct = pct.min(400);
    let g = pct as f32 / 100.0;
    for s in samples.iter_mut() {
        *s = (*s * g).clamp(-1.0, 1.0);
    }
}
```

## Error handling

| Failure mode | Behavior |
|---|---|
| ALSA mixer write fails at PDM Gain init | Log warn, continue. Capture still works at whatever mixer value the kernel had. |
| JSON `pdm_gain` outside 0..=48 | Validation rejects → falls back to embedded default (`pdm_gain: 24`). |
| TOML `mic_volume > 400` | Clamped on use in `apply_mic_volume`. No TOML-side validation. |
| `mic_volume = 0` | Legitimate — produces silence. No special case. |
| Sample clipping after gain | Hard clamp to ±1.0. No soft-knee — out of scope. |
| NaN/Inf in upstream samples | Not handled here. Trust DSP to produce finite values; otherwise it is a different bug. |

## Testing

### Unit (`waved-audio`)

- `apply_mic_volume_unity_is_identity` — `pct=100` doesn't change samples
- `apply_mic_volume_doubles_at_200` — `[0.4]` → `[0.8]`
- `apply_mic_volume_silences_at_zero` — `[0.5, -0.3]` → `[0.0, 0.0]`
- `apply_mic_volume_clamps_to_plus_minus_unity` — `[0.6]` at `pct=200` → `[1.0]`; symmetric on negative
- `apply_mic_volume_clamps_input_pct_above_400` — `pct=500` treated as `400`
- `audio_config_validates_pdm_gain_range` — `49` rejected, `48` accepted, `0` accepted
- `audio_config_embedded_default_has_pdm_gain_24`

### Integration

- `set_mic_volume_command_affects_next_processed_frame` — send `Command::SetMicVolume(200)`, then drive a frame through `simulate.rs` path; assert subsequent `processed_tx` frame has 2× amplitude on `mic_mono_f32`.
- `boot_seeds_audio_state_from_toml_mic_volume` — construct daemon services with TOML `mic_volume=300`; assert `AudioState.mic_volume=300`.

### On-device acceptance (script in `scripts/`)

1. Boot defaults (TOML `mic_volume=200`, JSON `pdm_gain=24`). Record 5 s normal-distance speech.
   - WAV peak ~ −12 to −6 dBFS
   - WAV RMS ~ −25 to −18 dBFS
   - (Compare to pre-fix baseline of ~−46 dBFS peak.)
2. Drive UAC2 mixer to internal `mic_volume = 100` (half the boot default,
   reachable from the Mac slider) → next recording 6 dB quieter than step 1.
3. Drive UAC2 mixer to internal `mic_volume = 400` (max) → next recording
   6 dB louder than step 1.
4. Reboot device → `AudioState.mic_volume` returns to TOML `200` (runtime
   changes not persisted).
5. `/tmp/audio_stats.json` exposes `mic_gain_linear` and `mic_gain_db`. Useful
   for in-field debugging.

## Open follow-ups (deferred — not part of this project)

- Limiter on the recording path (parallels playback DSP's limiter).
- DFN3 noise-floor adaptation tuning if the static PDM Gain operating point
  still produces over-gating in the field.
- On-device mic-volume settings screen if users ask for one.
- Persisting host-driven UAC2 mixer changes across reboot.
