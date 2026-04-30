# Waved Audio — SP#5 Playback EQ + Gain Staging Design

**Date:** 2026-04-28
**Depends on:** SP#1 capture foundation v2 (2026-04-16) + SP#2 AEC (2026-04-20) + SP#3 beamforming (2026-04-23) + SP#4 neural NS (2026-04-23)
**Status:** spec — implementation plan to be created via `writing-plans`

---

## 1. Context

After SP#4 the capture chain is complete: 8-ch capture → beamform → AEC → DFN3/speex NS → mono f32. Playback is the weakest link. `PlaybackService` (`crates/waved-audio/src/playback.rs`) writes raw S16 batches straight to `hw:0,1` (TDM-B → `acm8625s` smart amp). There is no high-pass filter, no equaliser, no dynamics processing, and no limiter. Test-tone (SIGUSR1) and file-play (SIGUSR2) sources work because they're already in-band; anything outside the amp's comfortable response (sub-80 Hz rumble, sibilance peaks, full-scale square waves) goes straight to the drivers.

Stock firmware solves this with `Insta360_Speaker::setDownlinkEqMode(int)` driving a multi-stage DSP chain (`asp_param_eq` block in `/etc/dynamic_eq_config_2.json`): HPF → broadband AGC → 4-band MBAGC → 4-biquad parametric EQ → DRC limiter, with optional reverb. Default mode is 2 (`get eq mode=2` in dmesg at boot). Closed-source: lives inside `libInstaAudio.so`.

SP#5 builds the open replacement. Two presets — **voice** (matches the stock conferencing tuning byte-for-byte) and **media** (voice with MBAGC bypassed and EQ flattened). Runtime preset switch via the same `Command + /tmp/waved-…` SIGHUP pattern that SP#4 uses for `NrLevel`.

### Speaker hardware (validated 2026-04-28)

The Wave has three visible speakers — two side, one bottom — driven by **one** stereo `acm8625s` smart amp (single chip on I²C bus 3 addr 0x35; full I²C scan confirms no other amps). Device-tree:

```
auge_sound/aml-audio-card,dai-link@1 (TDM-B → acm8625s):
  dai-tdm-slot-num     = 2
  dai-tdm-slot-tx-mask = [1, 1]    ← both slots transmit independent data
  dai-tdm-slot-rx-mask = [1, 1]    ← both slots receive (IV-sense per channel)
  dai-tdm-slot-width   = 32
  format               = i2s
```

Both slots being active rules out PBTL mono. The amp gets independent L and R PCM. The bottom driver has no second amp wired to it and is most likely a **passive radiator** — driven by chamber pressure from the active L/R drivers, tuned to a low-frequency resonance for bass extension. Listening tests at normal distance couldn't localise stereo separation, which is consistent with a small enclosure where acoustic crosstalk dominates the direct sound — but it doesn't change the data-path: the firmware writes 2-channel S16 and the amp routes each to its driver. SP#5 designs symmetrically (same EQ on L and R) but keeps a per-channel coefficient array so a future asymmetric tuning can drop in without an interface change.

## 2. Decisions (locked in brainstorming)

1. **DSP chain shape: HPF → 4-biquad parametric EQ → 4-band MBAGC → brick-wall limiter.** Matches stock minus broadband AGC and reverb. Broadband AGC is rejected: speech-band MBAGC already does the soft-talker boost, and a second AGC layer interacts poorly with the conferencing AEC (chases each AEC residual, generates pumping). Reverb is rejected: it generates self-echo that fights the AEC reference signal, and a speakerphone is the wrong device for "room ambience" anyway.

2. **Two presets: `Voice` and `Media`.** Same chain shape; differ in coefficient tables. Voice is the active mode for conferencing (host audio is far-end speech). Media bypasses MBAGC and uses a flatter EQ for music playback over UAC2. Default = Voice (matches stock). Switched at runtime; switch is online (no DSP teardown).

3. **Voice preset numbers imported verbatim from stock `/etc/dynamic_eq_config_2.json`.** Day-1 the device sounds the same as stock for conferencing — no measurement work needed to ship. Re-tuning is a follow-up task (4 hours with a USB measurement mic, well-defined protocol, not gating SP#5 release). Translation step: stock's biquad coefficients are direct-form-1 (b0, b1, b2, a1, a2 with `a0 = 1`); we use the same form via `aec/biquad.rs`. Bit-exact regression test ships against a known-impulse host-side reference.

4. **Media preset = Voice with MBAGC bypassed and EQ flat.** Single bool in `PlaybackPresetTuning` (`mbagc_enabled`); EQ band gains all set to 0 dB. HPF stays (sub-bass below 80 Hz still wastes amp power on any program material). Limiter stays.

5. **DSP placed inside `PlaybackService::run_playback_loop`, broadcast tap moves to post-DSP.** This is the architectural choice the AEC requires — the reference signal must be what actually comes out of the speaker, including EQ and limiting, otherwise cancellation degrades. Single chokepoint, all sources benefit, no per-source DSP duplication. Trait shape unchanged for callers; only the loop body adds two lines (`chain.process(&mut buf)` then `ref_tx.send(buf.clone())`).

6. **Per-channel EQ array, default symmetric.** `[BiquadParams; 4]` × 2 channels in the data model; same coefficients on both channels by default. Asymmetric override is config-only (no UI exposure). Cost is one extra copy of 5 f32 coefficients per band in the config struct; runtime cost is identical.

7. **Limiter ceiling −1 dBFS, 96-sample lookahead (2 ms @ 48 kHz), no oversampling.** Speech and conferencing music don't have meaningful inter-sample peaks above 8 kHz program material; oversampling would buy <0.5 dB ceiling tightening for ~3× the CPU. Lookahead is short enough that the post-DSP AEC reference still aligns inside the existing tolerance.

8. **Preset switch is online, gain-ramped.** Coefficient swap happens at the next period boundary. MBAGC envelopes reset to neutral gain on switch (avoids stale state from the other preset's noise floor). 5 ms cosine ramp on the output gain prevents zipper artefacts during the swap. Total switch latency (request → audible change): ≤ 20 ms.

9. **Acceptance criteria (`scripts/test-playback-eq.sh`):**
   - **EQ accuracy.** Voice preset frequency response measured through speaker → USB measurement mic, pink noise sweep, target curve from stock `dynamic_eq_config_2.json` within ±3 dB across 200 Hz–8 kHz.
   - **Soft-talk amplification.** −40 dBFS speech in (voice preset, MBAGC active), output within −20 ± 3 dBFS at the post-DSP tap. Confirms MBAGC reaches the soft-talker target.
   - **Limiter true-peak ceiling.** Full-scale square wave at 1 kHz in, output post-DSP peak ≤ −1 dBFS over a 1 s window.
   - **Preset switch click test.** A↔B rapid switch under continuous −12 dBFS pink noise; spectral flux at the switch boundary ≤ baseline noise + 0.5 dB.
   - **AEC ERLE non-regression.** Re-run `scripts/test-aec-on-device.sh`; ERLE within −2 dB of the SP#2 baseline (22.7 dB speech-band → ≥ 20.7 dB).
   - **CPU delta.** Whole waved process under voice preset at silence ≤ pre-SP#5 baseline + 1.5 %. Bar is below SP#3 beam delta because EQ is cheaper than beamforming.

## 3. Scope

**In scope:**
- New module tree `crates/waved-audio/src/playback/dsp/` with one file per DSP stage (`hpf.rs`, `eq.rs`, `mbagc.rs`, `limiter.rs`) and an orchestrator `chain.rs` exposing `PlaybackDspChain::process(buf: &mut [i16])`.
- Reuse `aec/biquad.rs` for HPF and EQ stages — already battle-tested, no duplicate biquad math.
- New `crates/waved-audio/src/playback/preset.rs` defining `PlaybackPreset { Voice, Media }` and `PlaybackPresetTuning` (HPF cutoff + 4 biquads × 2 ch + MBAGC params + limiter params).
- Voice tuning constants imported from `/etc/dynamic_eq_config_2.json`, baked into the binary via a `build.rs` that parses the JSON and emits a `pub const VOICE_PRESET: PlaybackPresetTuning` table. Media tuning derived from voice in code (one helper).
- `AudioConfig` v3 → v4 bump (`crates/waved-audio/src/audio_config.rs`): adds optional `dsp: Option<PlaybackDspConfig>` to the existing `PlaybackSection`. When absent, the embedded voice preset is used. When present, JSON values override.
- `crates/waved-state/src/lib.rs`: add `Command::SetPlaybackPreset(PlaybackPreset)` and `AudioState.playback_preset: PlaybackPreset` (default `Voice`).
- `crates/waved/src/main.rs`: extend `mode_switch_handler` to read `/tmp/waved-playback-preset` (`voice` | `media`) on SIGHUP and dispatch `Command::SetPlaybackPreset`. Mirrors the existing `/tmp/waved-nr` wiring.
- `PlaybackService::run_playback_loop` modified: instantiates a `PlaybackDspChain` from the active preset, processes each batch in place before the ALSA write, broadcasts the post-DSP buffer as the reference signal. Preset switch via an `mpsc::Receiver<PlaybackPreset>` polled at the top of the loop.
- Acceptance script `scripts/test-playback-eq.sh` covering the five criteria in §2.9.
- `docs/AUDIO_HARDWARE.md` extended with the playback chain block diagram and the AEC tap location.
- Host-side biquad regression test (`crates/waved-audio/tests/biquad_regression.rs`): impulse → known reference output to within 1 LSB, confirming our biquad form matches the stock coefficients' assumption.

**Out of scope (later SPs / follow-ups):**
- Re-measurement and re-tuning of presets (separate task, gated on having a USB measurement mic in hand).
- Loudness compensation curve (Fletcher-Munson). Possible SP#5.5.
- Per-driver crossover or biamp configuration. There is no second amp; the passive radiator handles low-frequency reinforcement passively. No crossover to design.
- Reverb / ambience effects. Rejected.
- Broadband AGC. Rejected.
- Speaker protection beyond the limiter (excursion modelling using IV-sense). The smart amp's on-chip protection handles this; software duplicating it would be redundant.

## 4. Non-goals

- **We do not match stock byte-for-byte through the full chain.** Voice EQ + MBAGC is matched; broadband AGC and reverb are intentional omissions. If a side-by-side listening test reveals an audible regression that isn't explained by AGC/reverb absence, we investigate — but the goal is *equivalent conferencing quality*, not *bit-identical output*.
- **We do not expose every DSP knob to the UI.** The OLED gets a single preset switcher (Voice / Media). All sub-knobs are config-only. Stock had a hidden adb-level interface for tuning; we'll have the same shape via `audio_config.json`.
- **We do not replace the existing `speaker_mixer::amixer` shell-outs in this SP.** TODO.md SP#5 lists this as a sub-goal; we descope it. The amixer path manages the AML-AUGESOUND `Master Playback Volume` (TDMOUT_A — the *unused* TDM port on this hardware). It's a no-op in production. Cleanup belongs in a separate refactor PR alongside SP#6 once the new pipeline owns volume control end-to-end.

## 5. Data model changes

### 5.1 `AudioConfig` v4

```rust
pub struct AudioConfig {
    pub version: u32,                         // bumped from 3 to 4
    pub capture: CaptureConfig,
    pub playback: Option<PlaybackSection>,    // existing
    pub period_frames: usize,
    pub buffer_periods: u32,
    pub mic_geometry: Option<MicGeometry>,
}

pub struct PlaybackSection {
    pub device: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub format: SampleFormat,
    pub period_frames: usize,
    pub buffer_periods: u32,
    pub dsp: Option<PlaybackDspConfig>,       // v4 — NEW
}

pub struct PlaybackDspConfig {
    /// Per-preset tunings. If absent, embedded defaults are used.
    #[serde(default)]
    pub voice: Option<PlaybackPresetTuning>,
    #[serde(default)]
    pub media: Option<PlaybackPresetTuning>,
}

pub struct PlaybackPresetTuning {
    pub hpf_cutoff_hz: f32,
    /// 4 biquads, per channel. Default symmetric (eq[0] == eq[1]).
    pub eq: [[BiquadParams; 4]; 2],
    pub mbagc: MbagcParams,
    pub limiter: LimiterParams,
}

pub struct BiquadParams {
    pub kind: BiquadKind,            // Peaking | LowShelf | HighShelf | Bypass
    pub center_freq_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

pub struct MbagcParams {
    pub enabled: bool,                       // false in Media preset
    pub crossover_freq_hz: [f32; 3],         // 4 bands → 3 crossovers
    pub target_dbfs: [f32; 4],               // per-band target level
    pub max_gain_db: [f32; 4],
    pub knee_db: [f32; 4],
    pub release_ms: [f32; 4],
}

pub struct LimiterParams {
    pub ceiling_dbfs: f32,                   // default −1.0
    pub lookahead_samples: usize,            // default 96
    pub release_ms: f32,                     // default 50.0
}
```

Validation in `AudioConfig::validate()`:
- `version` accepts 3 (legacy, no DSP) and 4. v3 silently uses embedded voice preset.
- `dsp.voice.eq.len() == 2` (two channels), `dsp.voice.eq[0].len() == 4` (four bands).
- MBAGC crossover frequencies strictly increasing.
- HPF cutoff in [20, 200] Hz.
- Limiter ceiling in [-3, 0] dBFS.

### 5.2 `AudioState` + `Command`

```rust
// crates/waved-state/src/lib.rs
pub struct AudioState {
    // ... existing fields
    pub playback_preset: PlaybackPreset,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackPreset {
    #[default]
    Voice,
    Media,
}

pub enum Command {
    // ... existing variants
    SetPlaybackPreset(PlaybackPreset),
}
```

### 5.3 `/tmp/audio_stats.json` extensions

Stats writer adds:
- `playback_preset`: `"voice"` | `"media"`
- `mbagc_active_gain_db`: `[f32; 4]` — current per-band gain (snapshot)
- `limiter_reduction_db`: f32 — current attenuation, peak over the 500 ms window

These are diagnostic; consumers downstream (the meeting service) ignore them.

## 6. Algorithms

### 6.1 HPF

Butterworth 2nd-order high-pass, cutoff = `voice.hpf_cutoff_hz` (default 80 Hz). Implemented as a single biquad in direct-form-1, coefficients computed once at preset-switch time. Standard RBJ cookbook formula; same impl as `aec/biquad.rs`.

### 6.2 Parametric EQ

4 biquad bands per channel, each with its own kind (Peaking / LowShelf / HighShelf), center frequency, gain, Q. Cascade order: bands applied in array order. Coefficients pre-computed at preset switch; per-sample work is 5 multiplies + 4 adds per band per channel. Total: 80 multiplies per stereo sample.

### 6.3 MBAGC (4-band)

Linkwitz-Riley 4th-order crossover splits the signal into 4 bands at the configured crossover frequencies. Each band has its own envelope follower (peak detect + IIR smoothing, attack 10 ms, release per-band). Per-band gain = `clamp(target_dbfs - envelope_dbfs, -knee_db, max_gain_db)`. Bands are summed back to one signal.

In the Media preset (`mbagc.enabled = false`), the crossover network is bypassed entirely (input → HPF → EQ → limiter; MBAGC stage is a passthrough). Costs nothing at runtime when disabled.

The 4-band crossovers from stock `dynamic_eq_config_2.json` are:
- Band 0 (low): < 300 Hz
- Band 1 (mid-low): 300–4000 Hz
- Band 2 (mid-high): 4000–7000 Hz
- Band 3 (high): > 7000 Hz

Per-band targets (from stock):
- `mbagc_target = [8203, 16406, 1160, 580]` (in i16 scale → [−12, −6, −29, −35] dBFS)
- `mbagc_max_gain_db = [6, 20, 10, 10]`

These numbers go into `VOICE_PRESET` verbatim.

### 6.4 Limiter

Lookahead peak limiter. Buffer holds `lookahead_samples` of the input. Each sample, peak-search the buffer; if peak > ceiling, compute attenuation = `ceiling / peak`; apply to the *delayed* sample (the one that's about to be emitted). Release follows an exponential curve with `release_ms` time constant. No oversampling — speech and music in the conferencing band don't generate inter-sample peaks worth chasing.

The 96-sample lookahead at 48 kHz = 2 ms. Total DSP latency added to playback: HPF (1-2 samples), EQ (1-2 samples), MBAGC (negligible — envelope-follower delay only), limiter (96 samples) ≈ 100 samples ≈ 2.1 ms. AEC's existing `MicWithSoftwareReferenceSource` aligns by frame count so this is automatically handled; the 2 ms shift falls well inside Speex AEC's tolerance window.

### 6.5 Preset switch

On `Command::SetPlaybackPreset`, the loop:
1. Computes new coefficient tables from the target preset's `PlaybackPresetTuning` (one-time CPU cost: ~200 multiplies, called off-thread).
2. At the next period boundary, atomically swaps the coefficient arrays.
3. Resets MBAGC envelope state to neutral gain (0 dB per band).
4. Limiter envelope state is preserved (avoids a momentary peak after switch).
5. Output gain ramped from 1.0 → 0.0 → 1.0 over 5 ms at the boundary (cosine envelope) so any coefficient discontinuity is masked.

Total user-visible switch latency: ≤ 20 ms (one period at the default 1024-frame period @ 48 kHz = 21.3 ms).

## 7. Risks + mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Stock biquad coefficient interpretation differs from RBJ direct-form-1 | EQ doesn't match stock | Bit-exact host-side regression test against impulse response of stock (computed in double precision); failure means we add a translation layer |
| Preset switch click | Audible artefact | 5 ms cosine gain ramp on switch (§6.5); acceptance test 4 |
| AEC ERLE regression from non-flat reference signal | Echo audible to far-end | AEC tap moves to post-DSP (§5 decision); acceptance test 5 holds ERLE ≥ 20.7 dB |
| MBAGC pumping under sustained noise | Noise pumps in/out of audibility | Per-band release set to stock's 200 ms (slow enough to mask pumping in normal program material); host-side listening test before ship |
| Limiter brick-walls all signal | Sounds compressed/lifeless | Voice preset target levels leave ~5 dB headroom under the limiter ceiling; limiter triggers only on actual overshoot |
| Build-time dynamic_eq_config_2.json absent on a fresh checkout | Build fails | `build.rs` falls back to a hardcoded copy of the JSON (stored alongside `build.rs`) if the device file isn't readable; CI uses the fallback |

## 8. Open questions

- **Reuse stock `mbagc_target` units.** Stock's targets are i16-scale numbers (e.g., 8203). We assume they're peak-amplitude targets in i16 LSB. Confirm by playing pink noise at known dBFS through the stock chain (host-side simulator built from the JSON) and comparing the measured envelope; if interpretation is wrong, the conversion factor is a single dBFS offset.
- **Per-channel asymmetric tuning.** Reserved in the data model. Whether to ship asymmetric defaults depends on a measurement pass (out of scope here). Default is symmetric.
- **Does Wave Controller send any UAC2 control surface that overrides the on-device preset choice?** Probably not — UAC2 doesn't have a "preset" concept — but worth a USB packet capture once SP#6 lands.

## 9. Self-review

- [x] No `TBD` / `TODO` placeholders left in the spec.
- [x] No internal contradictions: AEC tap location consistent across §1, §2.5, §6.4, §7.
- [x] Scope is single-implementation-plan-sized: one new module tree, one config bump, one SIGHUP wiring, one acceptance script. No multi-system fanout.
- [x] Ambiguity check: "voice tuning matches stock" defined as "import the JSON values verbatim, regression-test against impulse response", not "sounds the same to a listener".
- [x] Acceptance criteria are measurable, scripted, and have numeric thresholds.
