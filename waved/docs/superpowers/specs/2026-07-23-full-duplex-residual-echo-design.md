# Full-duplex residual-echo suppression design

**Date:** 2026-07-23

## Context

A live call through `Insta360 Wave USB` exposed two coupled symptoms:

- the near-end speaker initially sounded far away at the configured
  `mic_volume = 200` (+6 dB);
- after increasing microphone gain, the far end heard an audible copy of
  their own voice.

Both USB streams were active during the echo test. The playback reference was
non-zero, no UAC2 frames were dropped, the capture path reported no XRUNs, and
the limiter did not engage. The problem is therefore inside uplink processing,
not application routing or USB transport.

Initial diagnosis used `rms_per_channel[0] / mic_post_aec_rms` as ERLE. That
comparison is invalid after the microphone-gain work: `mic_post_aec_rms` is
calculated from the final `ProcessedAudioFrame`, after `apply_mic_volume` and
the recording limiter, while the raw microphone value is pre-gain. A +12 dB
or +18 dB microphone boost makes the reported ERLE appear lower by the same
amount.

Correcting for gain puts the existing linear Speex AEC around 24–29 dB on the
controlled tone and voice-like fixtures. Reference alignment is therefore not
the first defect to fix. The remaining echo is small before gain but is
amplified together with near-end speech. The pipeline has no always-on
residual-echo suppressor:

```text
beamformed mic -> Speex linear AEC -> optional DFN noise suppression
               -> mic gain -> limiter -> USB uplink / recording
```

`SpeexNoiseSuppressor` already links a Speex preprocessor to the echo state,
but it runs only as a fallback when neural noise suppression is requested and
DFN initialization fails. `NrLevel::Off` has no preprocessor, and a successful
DFN path is not linked to Speex's residual-echo estimate.

SpeexDSP provides the missing full-duplex stage. Its preprocessor consumes the
linear canceller's residual estimate and has separate far-end-only and
near-end-active suppression floors. The vendored defaults are -40 dB and
-15 dB. Applied before user gain, this removes residual echo without delaying
the microphone or lowering all near-end speech whenever the far end talks.

The device has been returned to a clean diagnostic baseline:

- `mic_volume = 400` (+12 dB);
- hardware PDM gain 24;
- normal `NrLevel::Off`;
- no `/tmp/waved-nr` or `/tmp/waved-volume` override.

## Goals

- Prevent the far end from hearing an audible copy of its own speech at normal
  and maximum supported Wave speaker volume.
- Preserve natural full-duplex conversation, including simultaneous speech
  and first syllables after silence.
- Keep near-end speech loud enough to sound close at normal meeting distance.
- Add residual-echo suppression before microphone gain so user gain cannot
  re-amplify the uncancelled tail.
- Measure pre-AEC, post-linear-AEC, post-residual, and final-uplink levels from
  the same processed frame.
- Add no microphone buffering or algorithmic lookahead.
- Keep added capture-to-USB latency at or below 5 ms and total device
  microphone latency below 50 ms.
- Preserve recording/UAC parity: the meeting WAV and USB uplink continue to
  consume the same final processed signal.

## Non-goals

- Replacing Speex AEC with WebRTC AEC3 in this change.
- Adding a fixed reference delay without same-frame evidence that alignment is
  wrong.
- Ducking or muting the microphone while the far end speaks.
- Converting the device to half-duplex or adding a voice-activated gate.
- Adding upstream AGC, microphone EQ, or changing hardware PDM gain.
- Changing beam patterns, DOA, playback EQ, Wi-Fi, recording, or webhook
  behavior.
- Exposing residual-echo tuning in `config.toml` or the setup portal. These
  values are DSP tuning in the versioned audio JSON, not user preferences.

## Considered approaches

### Speex residual-echo suppression before user gain

Run one Speex preprocessor linked to the same `SpeexEchoState` immediately
after linear cancellation. Disable ordinary denoising in this stage when DFN
is not falling back, but retain Speex's residual-echo estimator and its
near-end-active suppression floor.

This is the selected approach. It fills the exact missing pipeline stage,
uses the already-vendored implementation, has no lookahead, and is designed
for double-talk.

### Replace the AEC with WebRTC AEC3

AEC3 offers a more modern echo model and residual suppressor. It also brings a
large dependency, a different frame contract, new CPU and memory costs, and a
substantially larger validation surface on the Cortex-A55 target. It remains a
fallback only if the selected Speex design cannot pass full-duplex speech
acceptance within the latency and CPU budgets.

### Far-end-triggered gain ducking

Reducing microphone gain whenever reference energy is present would make echo
quieter quickly. It would also lower the near-end speaker during double-talk,
clip conversational interruptions, and create walkie-talkie behavior. It is
explicitly rejected.

### Lower static microphone or speaker gain

Lower levels reduce the audible residual but recreate the original
far-away-voice symptom or make the Wave speaker too quiet. Static level
reduction remains a troubleshooting aid, not a product fix.

## Design

### Processing order

The uplink path becomes:

```text
8-channel capture
  -> beamforming
  -> Speex linear AEC
  -> Speex residual-echo suppression
  -> optional noise suppression (DFN, or Speex denoise fallback)
  -> user microphone gain
  -> existing lookahead limiter
  -> USB uplink + meeting recording
```

The residual stage processes the current AEC output in place. It does not
queue microphone frames, delay the microphone to match playback, or add
lookahead.

### Composite Speex AEC stage

Introduce a bounded component that owns the coupled Speex objects:

- `SpeexEchoCanceller`;
- one linked `SpeexPreprocessor`;
- residual-suppression tuning;
- per-frame stage metrics.

The preprocessor is configured with:

- `SPEEX_PREPROCESS_SET_ECHO_STATE` pointing to the owned echo state;
- `SPEEX_PREPROCESS_SET_DENOISE = 0` for the normal residual-only path;
- `SPEEX_PREPROCESS_SET_ECHO_SUPPRESS = 40`;
- `SPEEX_PREPROCESS_SET_ECHO_SUPPRESS_ACTIVE = 15`.

The JSON values are positive attenuation magnitudes. Speex stores them as
negative dB internally. Validation accepts `0..=60` dB for each value and
requires the near-end-active attenuation not to exceed the far-end-only
attenuation.

The composite owns both native allocations so their relationship cannot be
broken by an unrelated refactor. The preprocessor is destroyed before the
echo state. Construction returns an error rather than exposing a partially
linked object.

Its public operation is one frame-level call:

1. receive beamformed mono microphone and mono playback reference;
2. run linear echo cancellation;
3. snapshot post-linear-AEC RMS;
4. run residual-echo suppression on the same output buffer;
5. snapshot post-residual RMS;
6. return the residual-suppressed frame for optional noise suppression.

`NrLevel` does not disable residual echo handling. “Off” continues to mean
ordinary environmental noise suppression is off; echo control remains part
of the conferencing transport.

### Noise-suppression interaction

For `NrLevel::Off`, the composite Speex stage runs with denoise disabled and
the output proceeds directly to user gain.

For `NrLevel::{Low, Med, High}`:

- a successful DFN initialization runs DFN after Speex residual suppression;
- if DFN initialization fails, the same linked Speex preprocessor enables its
  denoise function rather than constructing a second echo-linked
  preprocessor.

This avoids applying two independent residual suppressors and preserves the
existing DFN attenuation-level behavior.

Runtime NR changes retain the current rebuild semantics in this scope. The
separate roadmap item for prebuilding DFN remains responsible for eliminating
the existing model-rebuild frame gap.

### Audio JSON configuration

Add a versioned optional `aec` block to the audio JSON:

```json
{
  "version": 5,
  "aec": {
    "residual_echo_suppress_db": 40,
    "residual_echo_suppress_active_db": 15
  }
}
```

The embedded default advances from version 4 to version 5. The loader accepts
versions 3, 4, and 5; a version 3 or 4 file, or any file without the optional
block, receives these defaults. Unknown fields continue to follow the audio
configuration's existing forward-compatibility policy. Invalid values reject
the external file and use the embedded validated default, with the existing
configuration fallback reporting.

These fields are deliberately absent from `config.toml` and the captive setup
form. They are device DSP calibration, analogous to PDM gain and playback
filter coefficients.

### Same-frame AEC telemetry

The current stats collector combines the latest raw frame with the latest
final processed frame received on independent broadcasts. That is suitable
for general health telemetry but not for ERLE.

Each `ProcessedAudioFrame` will carry an `AecFrameMetrics` snapshot generated
inside the processing call:

- `reference_rms`;
- `mic_pre_aec_rms` from the exact beamformed input;
- `mic_post_linear_aec_rms`;
- `mic_post_residual_echo_rms`;
- `mic_post_noise_suppression_rms`;
- `mic_uplink_rms` after user gain and limiter;
- `linear_erle_db`;
- `effective_echo_suppression_db`, measured from pre-AEC mic to final uplink.

ERLE-derived fields are present only when reference RMS exceeds the
fixture-calibrated activity floor. They are diagnostics during arbitrary
double-talk, not proof of cancellation; acceptance computes them in
far-end-only windows.

`/tmp/audio_stats.json` exposes this snapshot from one processed frame. The
existing `mic_post_aec_rms` field remains for compatibility but is documented
as the legacy final-processed value. New scripts use `mic_uplink_rms` and the
explicit stage metrics instead.

The stats must also expose:

- `residual_echo_suppression_active`;
- the configured far-end-only and near-end-active suppression magnitudes;
- the active environmental noise-suppression backend.

### Gain calibration

Residual suppression is validated before microphone level is changed again.
After echo acceptance passes, test `mic_volume` values 400, 600, and 800 at
30–50 cm with normal conversational speech.

Select the lowest value that meets all of:

- conversational uplink RMS between -24 and -18 dBFS over speech-active
  windows;
- speech peaks below -3 dBFS;
- limiter reduction normally 0 dB and never more than 3 dB on normal speech;
- the final-gain echo acceptance below remains satisfied.

Write the selected value to the test device's `config.toml`. Changing the
project-wide default requires the separate repeatable stock-vs-Waved fixture
already listed in `TODO.md`; this change does not infer a fleet-wide gain from
one room and one device.

If none of the three software-gain values meets the speech target without
violating echo or limiter limits, stop and design upstream AGC separately.
Do not compensate by changing PDM gain or adding microphone buffering here.

### Full-duplex behavior

The residual suppressor uses Speex's active-speech probability to blend from
the far-end-only suppression floor toward the less aggressive
near-end-active floor. It must not make stream-active state, reference RMS, or
a binary VAD decision directly control microphone gain.

During simultaneous speech:

- the near-end signal remains on the normal microphone path;
- user gain remains unchanged;
- the residual floor relaxes to preserve the near-end voice;
- no frame is muted, dropped, or held for a future decision.

This preserves interruptions, backchannels, and first syllables.

### Latency invariants

The change adds no algorithmic microphone delay:

- Speex residual processing operates on the current frame in place;
- the playback reference is not buffered further;
- the microphone frame is not delayed to await classification;
- the existing 96-sample recording limiter remains the only uplink
  lookahead.

Release gates are:

- capture-to-USB latency increase no greater than 5 ms versus the current
  production binary under the same fixture;
- total Wave device microphone latency below 50 ms;
- no measurable first-syllable truncation after silence;
- a live listener reports neither self-echo nor perceptible conversational
  delay.

If the echo target cannot be met inside those limits, the implementation does
not add buffering or a half-duplex gate to force a pass.

### Failure handling

- Residual-stage construction failure logs a specific error, exposes
  `residual_echo_suppression_active = false`, and retains linear AEC rather
  than producing a silent microphone.
- A missing or silent reference makes residual processing effectively
  transparent; near-end-only speech must not be attenuated by more than
  0.5 dB.
- Invalid residual tuning rejects the external audio JSON and uses the
  embedded default through the existing fallback path.
- DFN failure enables Speex denoise on the existing linked preprocessor and
  keeps residual suppression active.
- NaN or non-finite per-frame metrics are omitted from JSON rather than
  causing stats serialization to fail.
- Existing capture, processed-channel, UAC2, limiter, and recording-integrity
  failures retain their current behavior.

## Testing

### Unit tests

- Residual-only construction disables denoise, links the echo state, and
  applies the configured suppression magnitudes.
- The near-end-active attenuation cannot exceed far-end-only attenuation.
- External audio JSON without `aec` gets the embedded defaults.
- Invalid attenuation values reject the external JSON.
- Native ownership drops the preprocessor before the echo state.
- A synthetic far-end echo converges through linear AEC and is further
  reduced by the residual stage.
- Near-end-only speech with a silent reference changes by at most 0.5 dB.
- A synthetic double-talk mixture retains near-end energy within 3 dB while
  reducing the far-end-correlated component.
- `AecFrameMetrics` values come from one frame and do not include user gain in
  `mic_post_linear_aec_rms` or `mic_post_residual_echo_rms`.
- `mic_uplink_rms` reflects user gain and limiter.
- Existing `mic_post_aec_rms` serialization remains compatible.

### Existing host suites

Run:

- `cargo test -p waved-audio`;
- `cargo test -p waved-state`;
- `cargo test -p waved`;
- audio-configuration parsing and fallback tests;
- UAC2 upstream/downstream tests;
- meeting-recording tests that consume `ProcessedAudioFrame`.

### On-device controlled tests

Update the AEC acceptance scripts to use same-frame metrics and run both tone
and speech material. Tone-only success is insufficient because DFN can remove
a tone while preserving voice-like echo.

At `speaker_volume = 100` and each candidate microphone gain:

1. establish silence and near-end-only baselines;
2. play the existing voice-like fixture twice, allowing linear AEC to
   converge;
3. measure linear ERLE before user gain;
4. measure effective echo suppression after residual processing, gain, and
   limiter;
5. play a distinct near-end speech fixture simultaneously from a fixed
   30–50 cm position;
6. verify near-end retention, far-end-correlated residual, first-syllable
   preservation, and final uplink level;
7. repeat after speaker mute/unmute and a 50-to-100 volume transition;
8. verify zero capture XRUNs, raw drops, processed drops, and UAC2 drops.

Run a sustained 30-minute bidirectional speech/playback test and record CPU,
memory, drop counters, limiter reduction, and residual-stage metrics. Added
CPU consumption must remain within 5 percentage points of the production
baseline.

### Latency verification

Measure the current production binary and candidate binary with the same
capture-to-UAC host fixture and physical placement. Report median and p95 over
at least 20 trials.

The candidate fails if:

- median or p95 increases by more than 5 ms;
- total measured device microphone latency reaches 50 ms;
- a silence-to-speech burst loses its leading edge.

### Live-call acceptance

Use the same calling application with both input and output explicitly set to
`Insta360 Wave USB`.

Test:

- far-end-only speech while the near end stays silent;
- near-end-only speech at 30–50 cm;
- both people reading different phrases simultaneously;
- rapid interruptions in both directions;
- speaker volume 50 and 100;
- the selected final microphone gain.

The far-end listener must report:

- no audible self-echo;
- near-end speech sounds close and natural;
- simultaneous speech remains intelligible;
- no delayed, clipped, or walkie-talkie turn transitions.

## Acceptance criteria

- Same-frame linear ERLE is at least 20 dB on the voice-like far-end-only
  fixture.
- Effective echo suppression after residual processing, selected microphone
  gain, and limiter is at least 25 dB on that fixture.
- Near-end level loss during controlled double-talk is no more than 3 dB
  relative to the same near-end fixture with a silent reference.
- Near-end-only speech changes by no more than 0.5 dB when the reference is
  silent.
- Conversational uplink level is -24 to -18 dBFS RMS with peaks below -3 dBFS.
- Normal speech causes at most 3 dB limiter reduction.
- Added capture-to-USB latency is no more than 5 ms and total device
  microphone latency stays below 50 ms.
- No first-syllable loss is measurable or reported.
- No capture XRUNs, raw-frame drops, processed-frame drops, or active UAC2
  drops are introduced.
- Sustained CPU increase is no more than 5 percentage points.
- Meeting WAV and USB uplink remain sample-equivalent consumers of the final
  processed mono signal.
- A live full-duplex call passes listener checks for self-echo, near-end
  loudness, simultaneous speech, and conversational delay.

## Rollback

The implementation remains isolated behind the composite Speex stage. If
device acceptance fails, restore the previous linear-AEC path, keep the
corrected telemetry, and leave the device at the clean +12 dB/PDM-24
configuration. Do not ship gain ducking, added microphone buffering, or an
unvalidated WebRTC replacement as an emergency substitute.
