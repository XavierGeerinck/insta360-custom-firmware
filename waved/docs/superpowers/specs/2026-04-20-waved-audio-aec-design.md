# Waved Audio — SP#2 Acoustic Echo Cancellation Design

**Date:** 2026-04-20
**Depends on:** SP#1 capture foundation v2 (committed 2026-04-16) + SP#1.5 software-reference playback (committed 2026-04-20, this session)
**Status:** spec — implementation plan in `docs/superpowers/plans/2026-04-20-waved-audio-aec.md`

---

## 1. Context

SP#1 and SP#1.5 together deliver an `AudioFrame` with two paired slots:

```rust
pub struct AudioFrame {
    pub data: Vec<i16>,           // 8-ch mic, interleaved
    pub channels: u16,            // 8
    pub reference: Option<Vec<i16>>,  // 2-ch reference (what was sent to speakers)
    pub reference_channels: u16,      // 2
    pub sample_rate: u32,         // 48000
    pub timestamp_ms: u64,        // monotonic since capture start
}
```

The reference channel is populated by `PlaybackService`: whatever waved writes to the playback PCM (`hw:0,1`, default silence) is broadcast to `MicWithSoftwareReferenceSource`, which attaches the freshest batch to each mic period. Validated on device at 99.9% real-time, 0 xruns, sub-millisecond alignment jitter (far tighter than any AEC filter's tap length).

**What's missing:** actual echo cancellation. Today the reference slot is populated but untouched — downstream consumers (stats, meeting recording, VU) see raw mic with all the speaker echo in it. Under normal conferencing conditions (speaker playing remote-participant audio at moderate volume, mic 10-30 cm from speaker) this echo would make outgoing audio unusable.

SP#2 closes that gap: process each `AudioFrame` through an echo canceller that uses `reference` to remove echo from `data`, then surface the cleaned signal to downstream consumers.

## 2. Scope

**In scope:**
- DSP library vendoring + Rust binding (see §5)
- `waved_audio::aec` module wrapping the DSP's state machine
- Pipeline integration at the `AudioService` frame-processing boundary
- `AudioFrame` boundary flip to `Vec<f32>` — echo cancellation works natively in float (see §4)
- `NoiseReduction` audio mode wired to enable/disable AEC + noise suppression at runtime
- Host tests with deterministic synthetic echo (known reference + known echo delay + measurable ERLE)
- Smoke test on device: measured echo suppression ≥ 20 dB with a 1 kHz reference tone played through the speaker

**Out of scope (later SPs):**
- Beamforming across the 8-mic array (SP#3)
- Automatic gain control on the mic path (SP#4)
- Dynamic-range compression / EQ / limiter (SP#5)
- Uplink delay measurement and adaptive latency tuning (SP#6)

## 3. Non-goals

- We do **not** ship per-channel AEC. For reasons described in §7 we run AEC on a single mono mix of the mic array (the same signal that SP#3 beamforming will consume first when it lands). For SP#2 that mix is a simple channel-0 selection; SP#3 replaces it with the beamformed signal.
- We do **not** implement our own adaptive filter. Nothing we could write in a reasonable time approaches the tuning of speexdsp or webrtc-aec3.
- We do **not** build AEC configuration into `/etc/waved-audio-config.json`. The knobs that matter (filter length, echo suppression aggressiveness) are tied to the chosen library; we expose two compile-time constants plus the `NoiseReduction`/`NoiseReductionOff` runtime toggle.

## 4. Data model — boundary flip to f32

Today `AudioFrame.data` and `AudioFrame.reference` are `Vec<i16>`. i16 is the right wire format for capture (matches the ALSA readi we do post-`>> 16`) and for meeting recording (small on-disk footprint), but DSP filters multiply and accumulate — doing that in i16 requires either i32 accumulators with explicit scaling or frequent conversion to f32 anyway.

**Decision:** introduce an intermediate `ProcessedAudioFrame` that holds the DSP pipeline's working representation:

```rust
pub struct ProcessedAudioFrame {
    pub mic_mono_f32: Vec<f32>,      // post-AEC, post-NS, mono, 48kHz, [-1.0, 1.0]
    pub reference_f32: Vec<f32>,     // input to AEC, kept for debugging / re-runs
    pub sample_rate: u32,
    pub timestamp_ms: u64,
    pub raw_mic_i16: Vec<i16>,       // original 8-ch mic data, for meeting recording + stats
}
```

Rationale for carrying `raw_mic_i16`:

1. Stats/VU consumers want per-channel RMS from the raw mic. Running stats against the AEC-processed mono signal would show "VU drops when speaker is loud" which is exactly what AEC is supposed to produce — not what the user wants to see on the bar graph.
2. Meeting recording should capture raw mic for diagnostic replay and for retroactive reprocessing when we tune DSP later.
3. Beamforming (SP#3) runs on raw 8-ch, not post-AEC mono.

The `mic_mono_f32` field is what goes out to meeting encoders / uplink.

**Where the flip happens:** `AudioService::run` today does `apply_mic_mute(&mut frame.data, muted)` then broadcasts a clone. SP#2 intercepts between mute and broadcast:

```
mpsc<AudioFrame>     raw mic + reference
   │
   v
AudioService::run
   │
   ├── apply_mic_mute (mutates data in place; unchanged)
   ├── convert i16 → f32 scaling by 1.0/32768.0
   ├── mono mix (channel 0 for SP#2; beamform output in SP#3)
   ├── EchoCanceller::process(mic_mono_f32, reference_f32)
   ├── NoiseSuppressor::process(mic_mono_f32)  [optional, driven by audio_mode]
   ├── build ProcessedAudioFrame
   │
   ├── compute VU from frame.data (raw)         [stats unchanged]
   ├── broadcast raw AudioFrame to stats_tee    [unchanged]
   ├── broadcast ProcessedAudioFrame to meeting [new channel]
   │
   └── loop
```

**`stats_collector` keeps its current broadcast** — it wants RAW mic data for per-channel metrics. Only the meeting / uplink path moves to the processed frame.

## 5. DSP library selection

### Option A — speexdsp (`speexdsp-sys` + thin Rust wrapper)

- C99, ~30 kB static, battle-tested (FreeSWITCH, Mumble, Linphone).
- AEC: NLMS adaptive filter. Tunable tail length. Pre-allocates; no runtime heap.
- Pair with `SpeexPreprocess` for noise suppression + residual echo suppression.
- API works in **i16**, so we either (a) keep the i16 boundary and do less f32 work, or (b) convert back and forth per frame.
- Bindings: no current well-maintained high-level crate. `speexdsp-sys` is available but stale; we'd vendor our own minimal wrapper (~200 LOC). Acceptable.

### Option B — webrtc-audio-processing-2 / aec3 (`webrtc-audio-processing` crate)

- C++ (aec3 is the newer linear filter replacing aec2). Much larger (~500 kB+ static).
- Integrated processing chain: high-pass, AEC, NS, AGC, voice activity detection, all in one `AudioProcessing` object.
- Operates on 10 ms float frames at 48 kHz (= 480 samples/channel).
- Bindings: `webrtc-audio-processing` crate (v0.4 covers aec2, v0.5 adds aec3). Reasonably maintained.
- Requires C++ toolchain + `-std=c++17`; cross-compilation via `cross` has been a pain point in other projects (doable, but adds setup).

### Option C — custom NLMS

Wildly out of scope for SP#2 timebox. Listed only for completeness.

**Decision: speexdsp.**

Rationale:
1. Size matters on a 440 MB /data partition and a constrained SoC. 30 kB > 500 kB is a meaningful margin when we're also carrying LVGL.
2. C99 cross-compiles trivially via `cross`. No C++ ABI concerns.
3. The AEC algorithm quality difference matters less than the hardware echo path (speaker → mic geometry, enclosure resonance). SP#2's goal is a working cancellation baseline, not best-in-class; speexdsp reliably delivers 20–30 dB ERLE on typical conferencing paths.
4. i16 native means our `ProcessedAudioFrame`'s `mic_mono_f32` comes from one i16→f32 conversion after AEC, not pre-post-pre wrapping around a C++ float API. Simpler boundary.

**Migration path:** if SP#4 or later demands aec3-quality, the `EchoCanceller` trait stays the same; we swap the implementation. The `ProcessedAudioFrame` boundary is already float, so no downstream code changes.

## 6. Module layout

```
crates/waved-audio/
  Cargo.toml                               — add speexdsp-sys = "0.2" (vendored C)
  build.rs                                 — link against vendored libspeexdsp
  src/
    aec/
      mod.rs                               — public EchoCanceller trait + factory
      speex.rs                             — SpeexEchoCanceller + SpeexPreprocessor
      passthrough.rs                       — PassthroughCanceller (for tests / disabled mode)
    processed_frame.rs                     — ProcessedAudioFrame + conversions
    lib.rs                                 — pub mod aec; pub mod processed_frame;

crates/waved-audio/vendor/speexdsp/        — vendored C sources (pinned commit)
```

## 7. AEC on mono, not per-channel

Running AEC 8 times (once per mic channel) would:
- Burn 8× the CPU. At 48 kHz 1024-period, each speex AEC instance is ~2–4 ms of CPU; 8 instances on a single-core budget is marginal.
- Reuse the same reference signal, producing 8 filters that converge to similar weights (wasted work).
- Still leave downstream consumers with an 8-ch signal they don't want (meeting upload expects mono).

The correct pipeline is:
1. SP#2: mic[ch0] → AEC → mono_f32 (placeholder mono source — pick ch0)
2. SP#3: beamform(mic[0..8]) → beamformed_mono → AEC → mono_f32 (replace ch0 selection)

This preserves the `EchoCanceller` API across SP#2→SP#3.

## 8. Configuration

Two compile-time constants in `waved_audio::aec::speex`:

| Constant | Value | Why |
|---|---|---|
| `FILTER_TAIL_MS` | 200 | Covers typical room reverb; speexdsp recommends 100–500 ms |
| `SUPPRESS_DEFAULT_DB` | 35 | Initial echo suppression target |

Runtime toggle: `Command::SetAudioMode(AudioMode)` already exists; SP#2 wires it through:

| AudioMode | Behavior |
|---|---|
| `NoiseReduction` | AEC on, Speex preprocessor NS on |
| `NoiseReductionOff` | AEC on, NS off |
| `Raw` | AEC bypass (PassthroughCanceller) |
| `Stereo` (future) | TBD — probably same as `Raw` for now |

The audio_mode lives on `AudioState`; `AudioService::run` already mutates it on `Command::SetAudioMode`. SP#2 adds: rebuild the cancellation chain on mode change.

## 9. Testability

**Host-only tests** (no ALSA):

1. `PassthroughCanceller::process(mic, ref) == mic` (sanity).
2. `SpeexEchoCanceller::process(silence, silence) ~= silence` (low RMS on output).
3. **Synthetic echo test**:
   ```
   reference = sine(1kHz, 1s)
   echo = delay(reference, 20ms) * 0.5        // simulated acoustic path
   mic = noise(low) + echo
   cancelled = canceller.process(mic, reference)
   assert ERLE(mic, cancelled) > 20 dB
   ```
   Deterministic: seed the noise RNG. Runs in CI.

4. **NS effect test**: feed noise-only mic (no reference signal), assert NS attenuates by ≥ 8 dB with `NoiseReduction` on, ≤ 2 dB with it off.

**Device smoke test** (`scripts/test-aec-on-device.sh`):

1. Start waved.
2. `amixer sset "5707_A Master" 50` (moderate volume).
3. Spawn a `waved-aec-probe` helper (new binary) that plays a 1 kHz tone through `PlaybackService`.
4. Capture 3 s of `audio_stats.json` with the tone active.
5. Read back:
   - Assert `reference_rms[0]` > 0.1 (tone is reaching the reference tap).
   - Assert `mic_post_aec_rms` < `mic_pre_aec_rms` by ≥ 20 dB (new stats fields).
6. Stop tone, assert `reference_rms` decays to < 0.01 within 500 ms.

## 10. Failure modes to address in the plan

- **Filter divergence** under microphone clipping. speexdsp handles this internally; verify by feeding a clipped mic signal in tests.
- **Reference latency mismatch**. Software reference is tight (§1), but if ALSA's playback buffer shifts unexpectedly (underruns), the filter may chase its tail. Speex's adaptive algorithm should reconverge within ~200 ms; add a test that measures reconvergence after an injected 50 ms shift.
- **Channel count drift**. Reference is 2-ch, AEC wants mono. Mix L+R to mono at the entry to `SpeexEchoCanceller::process`; don't rely on the playback layer to pre-mix.
- **CPU headroom**. Measure on device — if the AEC alone pushes the capture thread past 80 % of its period, we need a plan (smaller filter, process every other period, move AEC off-thread via a second mpsc).

## 11. Open questions

- Do we need to expose post-AEC per-channel stats, or is mono enough? Probably mono until SP#4.
- Should `stats_collector` also receive `ProcessedAudioFrame` to report ERLE? Yes — one new metric, cheap. Add to §4 pipeline diagram if we do.
- Does the meeting upload format need to switch to float? Mumble-style codecs want i16 in; we convert `mic_mono_f32 → i16` at the meeting-encode boundary. No format change elsewhere.
