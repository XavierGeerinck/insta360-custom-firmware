# Waved Audio — SP#3 Beamforming Design

**Date:** 2026-04-20
**Depends on:** SP#1 capture foundation v2 (2026-04-16) + SP#1.5 software-reference playback (2026-04-20) + SP#2 AEC (2026-04-20, ERLE 22.7 dB speech-band on device)
**Status:** spec — implementation plan to be created via `writing-plans`

---

## 1. Context

SP#2 delivers a working single-channel DSP pipeline: `AudioFrame` (8 raw mic + 2 reference) → mono mix of channel 0 → AEC → NS → `ProcessedAudioFrame.mic_mono_f32`. Seven of the eight mic channels are discarded in the mono-mix step (`mix_to_mono` picks channel 0 and ignores the rest).

SP#3 turns those seven discarded channels into useful spatial information:

1. Replace the "channel 0 only" mono mix with a **delay-and-sum (DAS) beamformer** steered to the current direction of arrival (DOA). Each of five audio-mode presets (Omni, Cardioid, Supercardioid, Figure-8, Stereo) becomes a fixed weight matrix applied to the 8 delayed channels.
2. Wire up the currently state-only `Command::SetAudioMode` so changing the mode on the UI actually swaps the active weight matrix.
3. Populate `AudioState::doa_horizontal` and `AudioState::doa_pitch` — continuously, so the Wave Controller UI can render the "mic is pointing here" indicator while also feeding the beamformer's steering direction.
4. Bump `AudioConfig` to version 3, adding `mic_geometry` (8-entry table of physical mic positions + ALSA channel permutation) as the version-bump payload.

**What's not currently real about SP#3 that it needs to make real:**
- `AudioMode` has a default (`NoiseReduction`) that overlaps conceptually with `NrLevel` — this spec removes the overlap.
- `ProcessedAudioFrame` is mono-only; Stereo mode has no path to reach downstream consumers as stereo — this spec adds a sibling field.
- Beamforming math requires known physical mic positions; we currently have none encoded in software — this spec adds them via teardown-measured geometry plus a tap-harness for ALSA channel → physical mic permutation.

## 2. Decisions (locked in brainstorming)

1. **Geometry source: physical teardown (done).** 8-mic 3D cylindrical array. Cylinder diameter 52.9 mm (radius 26.45 mm). Two rings of 4 mics each at mic radius 25.5 mm from the cylinder axis:
   - Top ring: 4 mics at z = 0, azimuths **0° / 90° / 180° / 270°**.
   - Side ring: 4 mics at z = −78.2 mm, azimuths **45° / 135° / 225° / 315°** (offset from top ring for finer angular coverage).
2. **Beamforming algorithm: DAS only.** Five fixed weight matrices, one per preset. Deterministic, CPU-trivial, no adaptation that can misbehave. MVDR is a possible SP#4.5 follow-up if real-room testing shows DAS insufficient; SP#4's DeepFilterNet handles non-stationary noise better than any beamformer anyway.
3. **Mode semantics:**
   - Remove `NoiseReduction` from `AudioMode`. Default becomes `Omni`. Noise suppression is controlled entirely by the orthogonal `NrLevel` (Off/Low/Med/High).
   - Stereo mode outputs two channels via a new sibling field `mic_stereo_f32: Option<(Vec<f32>, Vec<f32>)>` on `ProcessedAudioFrame`. `None` for the four mono presets; `Some((L, R))` for Stereo. Meeting recorder writes stereo when present; uplink always takes `mic_mono_f32` (Teams / Zoom are mono).
   - Stereo preset = two cardioids at ±45° from current DOA. DOA-following, not fixed L/R.
4. **DOA: GCC-PHAT pairwise + least-squares fit, VAD-gated, α=0.9 smoothed.** Compute TDOA on 4 well-separated mic pairs per frame (every 10 ms), solve the overdetermined steering-vector system for (azimuth, elevation), smooth exponentially. Smoothing keeps the UI pointer from flickering and keeps the beam's steering direction stable across frames.
5. **Pipeline topology: Beamform → AEC → NS.** Single post-beam AEC (the Speex state already in place). DAS attenuates the echo path the beam doesn't admit; α=0.9 DOA smoothing mitigates AEC re-convergence when beam rotates. Keeps today's CPU budget (important for SP#4 DeepFilterNet headroom on the A53).
6. **ALSA channel → physical mic mapping: tap harness (one-shot).** Device records while operator taps each of the 8 mic holes in a documented order; the resulting PCM reveals the permutation via per-channel peak energy. Result is committed to the repo as a fixed 8-entry table in `AudioConfig::mic_geometry`. Non-destructive, ~5 min, no turntable needed.
7. **Validation criteria (acceptance tests, ship with `scripts/test-beamforming.sh`):**
   - **Front-to-back ratio** (Cardioid preset): ≥ 10 dB attenuation at 180° vs 0° on a swept sine source. (Textbook cardioid is 25 dB; 10 dB is the "actually working" floor given our coarse 8-mic geometry.)
   - **DOA accuracy:** ≤ 10° error for a speech source at 1 m, measured against tape-mark ground truth at azimuths 0° / 90° / 180° / 270°.
   - **Beam CPU budget:** < 5% of one A53 core. Leaves headroom for AEC (~2%) + SP#4 DeepFilterNet (~20% budgeted).

## 3. Scope

**In scope:**
- Coordinate system + geometry (top ring + side ring in Cartesian) encoded as `MicGeometry` in `AudioConfig v3`.
- `waved-audio::beam` module: DAS per-mode weight-matrix generation, per-frame application.
- `waved-audio::doa` module: GCC-PHAT pair cross-correlation + least-squares direction fit + EWMA smoothing + VAD gate.
- `ProcessedAudioFrame` gains `mic_stereo_f32: Option<(Vec<f32>, Vec<f32>)>`.
- `AudioMode` enum loses `NoiseReduction`; default changes to `Omni`. `Command::SetAudioMode` handler in `AudioService` installs the active weight matrix.
- `AudioConfig` bumped to version 3 with `mic_geometry` section. Forward-compatible: older JSON files (v2) continue to work via `serde(default)` on `mic_geometry`, falling back to the teardown-measured defaults embedded in code.
- Calibration harness: new binary `waved-mic-map` in `crates/waved-device-test/` (or a new `crates/waved-mic-map/`) that produces the 8-entry ALSA→physical permutation from a tap recording.
- On-device acceptance test script `scripts/test-beamforming.sh`: plays a swept sine through a Bluetooth speaker moved to known positions, verifies front-to-back ratio and DOA accuracy.
- Host unit tests: DAS math against a synthetic 8-channel source at a known angle — verify the cardioid peak lies within 5° of the true direction and the rear is attenuated ≥ 10 dB in simulation.

**Out of scope (later SPs):**
- MVDR / adaptive beamforming (possible SP#4.5).
- DeepFilterNet neural noise suppression (SP#4).
- Automatic gain control (SP#5).
- Per-channel AEC. Single post-beam AEC only (see §2.5).
- Turntable-based or chirp-based geometry calibration — teardown gives us the positions; tap gives us the channel map. Both suffice.

## 4. Non-goals

- **We do not rewrite DOA as SRP-PHAT.** Single dominant speaker is the target use case; GCC-PHAT is the natural fit and 10× cheaper. SRP-PHAT is the upgrade path if we ever need multi-speaker disambiguation.
- **We do not make the stereo output interleaved.** `Option<(Vec<f32>, Vec<f32>)>` keeps the existing mono consumers branch-free (they always read `mic_mono_f32`); only the meeting recorder needs to learn about the new field.
- **We do not ship back-compat for the removed `NoiseReduction` mode.** It was the default, it overlaps with `NrLevel`, and the UI screen that used to say "Noise Reduction" renames to "Omni + NS". No end-user feature is lost; anyone noise-suppressing was already layering `NrLevel` on top.

## 5. Data model changes

### 5.1 `ProcessedAudioFrame` (`crates/waved-audio/src/lib.rs`)

```rust
pub struct ProcessedAudioFrame {
    pub mic_mono_f32: Vec<f32>,                              // unchanged
    pub mic_stereo_f32: Option<(Vec<f32>, Vec<f32>)>,        // NEW — Some() iff mode = Stereo
    pub reference_f32: Vec<f32>,                              // unchanged
    pub sample_rate: u32,
    pub timestamp_ms: u64,
    pub raw_mic_i16: Vec<i16>,
}
```

Invariant: `mic_stereo_f32.is_some()` implies both channels have the same length as `mic_mono_f32`. The mono channel is still populated in Stereo mode (it's the L+R sum scaled by 0.5), so downstream consumers that only care about mono (stats, uplink) never branch.

### 5.2 `AudioMode` (`crates/waved-state/src/lib.rs`)

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AudioMode {
    #[default]
    Omni,                  // NEW default (was NoiseReduction)
    Cardioid,
    Supercardioid,
    Figure8,
    Stereo,
    // NoiseReduction removed
}
```

Persisted state files that contain `"mode": "noise_reduction"` will fail to deserialize. Mitigation: `AudioService::load_state` catches the deserialize error, logs a warning, and writes back `Omni` as the new default. (One-time migration — matches how we've handled `AudioConfig` version bumps.)

### 5.3 `AudioConfig` v3 (`crates/waved-audio/src/audio_config.rs`)

```rust
pub struct AudioConfig {
    pub version: u32,           // now 3
    pub capture: CaptureConfig,
    pub playback: Option<PlaybackSection>,
    pub period_frames: usize,
    pub buffer_periods: u32,
    #[serde(default)]
    pub mic_geometry: Option<MicGeometry>,   // NEW
}

pub struct MicGeometry {
    /// 8 physical mic positions in device-fixed Cartesian (mm).
    /// +X forward, +Y left, +Z up. Origin at center of top ring.
    pub positions_mm: [Position3D; 8],
    /// permutation[alsa_ch] = physical_index in positions_mm
    /// produced once per hardware revision by `waved-mic-map`.
    pub alsa_to_physical: [u8; 8],
}

pub struct Position3D { pub x: f32, pub y: f32, pub z: f32 }
```

**Default `MicGeometry`** (embedded in code; used when JSON omits it):

| idx | ring | azimuth | x (mm) | y (mm) | z (mm) |
|-----|------|---------|--------|--------|--------|
| 0 | top | 0° | 25.5 | 0 | 0 |
| 1 | top | 90° | 0 | 25.5 | 0 |
| 2 | top | 180° | −25.5 | 0 | 0 |
| 3 | top | 270° | 0 | −25.5 | 0 |
| 4 | side | 45° | 18.03 | 18.03 | −78.2 |
| 5 | side | 135° | −18.03 | 18.03 | −78.2 |
| 6 | side | 225° | −18.03 | −18.03 | −78.2 |
| 7 | side | 315° | 18.03 | −18.03 | −78.2 |

Default `alsa_to_physical = [0,1,2,3,4,5,6,7]` (identity) until overridden by tap-harness output. Identity is a guess; the tap run confirms or corrects it and the correction is committed to the repo's default config.

## 6. Module layout

```
crates/waved-audio/
  src/
    lib.rs                   — re-export beam + doa; extend ProcessedAudioFrame
    beam/
      mod.rs                 — BeamFormer trait, DasBeamFormer impl
      weights.rs             — generate_weights(mode, geometry, doa) → [f32; 8] per preset
      delays.rs              — fractional-delay line (linear interp, 1ms tail buffer)
    doa/
      mod.rs                 — DoaEstimator, EWMA smoother
      gcc_phat.rs            — pairwise cross-correlation (FFT) → TDOA
      solve.rs               — least-squares direction from TDOAs
      vad.rs                 — simple energy-based VAD gate (speex NS already gives us a VAD flag — reuse it)
    audio_service.rs         — wire beam before AEC; handle SetAudioMode
    audio_config.rs          — v3 with MicGeometry
crates/waved-mic-map/
  Cargo.toml                 — new crate
  src/main.rs                — tap-harness CLI (runs on device)
scripts/
  test-beamforming.sh        — on-device acceptance tests (FBR + DOA accuracy + CPU)
  tap-mic-map.sh             — host-side wrapper that runs waved-mic-map remotely over adb
```

## 7. Algorithm details

### 7.1 DAS mechanics

For a plane-wave source at direction **d** = (sin θ cos φ, sin θ sin φ, cos θ) (elevation θ, azimuth φ), the per-mic time-of-arrival relative to origin is:

```
τ_i = -(d · p_i) / c        (c = 343 m/s, p_i = mic position in meters)
```

Each channel is **delayed by −τ_i** (so the source's wavefront aligns across channels) then weighted:

```
y[n] = Σ_i w_i · x_i[n − τ_i / T_s]      (T_s = 1/48000 s)
```

Delays are fractional (max delay across our geometry ≈ 51 μm / 343 m·s⁻¹ ≈ 150 μs ≈ 7.2 samples at 48 kHz). We implement a 32-sample linear-interpolation delay line per channel — tiny, cache-warm, no FFT.

**Weight matrices per preset** (applied after delay alignment):

| Preset | Weights `w_i` | Notes |
|--------|---------------|-------|
| Omni | `[1/8] × 8` | No steering; isotropic sum. |
| Cardioid | Derived analytically; first-order cardioid pointed along DOA. | Uses top-ring pair (front mic + back mic along DOA axis) with a differential term; other mics weighted at the computed taps for 3D pattern. |
| Supercardioid | Weights maximize directivity index. | Tighter main lobe, small rear lobe. |
| Figure-8 | `w_front = +1/2, w_back = −1/2`, others 0 | Pressure-gradient pattern. |
| Stereo | Two parallel DAS outputs at φ ± 45°, each with cardioid weights. | Packed into `mic_stereo_f32`. |

Weight matrices are pre-computed on DOA change (not per-sample). A single atomic swap of an `Arc<[[f32; 8]]>` covers the "user changes mode" and "DOA drifted" cases without a lock.

### 7.2 DOA

Per frame:

1. Gate on VAD (Speex preprocessor exposes a speech-probability flag already; reuse it — threshold 0.5).
2. For 4 selected mic pairs (2 top-ring diametrically opposed + 2 top-to-side diagonals), compute GCC-PHAT cross-correlation over the 10 ms frame. Peak lag → TDOA per pair.
3. Least-squares solve `A d = τ` where `A` has rows `(p_i − p_j) / c` and `τ` is the TDOA vector. Yields unit direction **d**.
4. Convert to (azimuth, elevation). Exponentially smooth with α = 0.9: `φ_out = 0.9 φ_prev + 0.1 φ_new` (angle-aware — use unit-vector slerp or atan2 on smoothed (sin, cos)).
5. Write `AudioState.doa_horizontal = φ` (degrees 0..360) and `AudioState.doa_pitch = 90 − θ` (degrees, -90..90, 0 = horizon).

Smoothed DOA feeds both the UI and the beamformer's weight-matrix recomputation.

### 7.3 Fallback when VAD is quiet

When VAD is below threshold, freeze the DOA (keep the last speaking direction). This keeps the beam pointing where the user was talking during pauses rather than drifting to whatever HVAC hum is loudest. Beam continues to attenuate off-axis noise during the pause.

## 8. Calibration harness (`waved-mic-map`)

Flow:

1. Operator runs `scripts/tap-mic-map.sh` on host.
2. Script pushes `waved-mic-map` binary to device, runs it while printing prompts.
3. Device: waved is paused (`systemctl stop waved`); `waved-mic-map` opens `hw:0,2` directly at 48 kHz × 8 ch and records 12 s of audio.
4. Operator taps mic hole 0 (top-ring 0°), waits 1 s, taps hole 1 (top-ring 90°), ... through hole 7 (side-ring 315°), following on-screen prompts.
5. `waved-mic-map` segments the recording into 8 × 1.5 s windows, computes per-channel energy in each, and emits: `for each tap slot (physical index), the ALSA channel with highest energy`.
6. Result: 8-entry `alsa_to_physical` array, printed and written to `/tmp/mic_map.json`. Host script pulls the file and instructs operator to commit it into `audio_config_default.json` (or hand-edit).

**Why not fully automated?** Because the operator has to physically tap the holes anyway; having them confirm visually is cheaper than a chirp-source rig and gives us immediate sanity-check output ("all 8 taps detected a distinct channel ✓").

**Robustness:** the harness also warns if any two taps resolved to the same ALSA channel (operator mistake — retry) and if any channel had zero energy (dead mic or missed tap).

## 9. Validation plan

### 9.1 Host unit tests (`cargo test -p waved-audio`)

- `beam::tests::cardioid_points_at_doa`: synthesize 8-channel plane wave at φ=45°, apply Cardioid weights with DOA=45°, assert main output ≥ 0.9 × reference.
- `beam::tests::cardioid_rejects_rear`: same source at φ=45°, apply Cardioid weights with DOA=225° (180° off-axis), assert output ≤ 0.3 × reference (10 dB attenuation).
- `beam::tests::stereo_produces_two_channels`: Stereo mode on plane-wave input, assert `mic_stereo_f32.is_some()` and L ≠ R.
- `doa::tests::gcc_phat_synthetic`: known TDOAs on 4 pairs → least-squares recovers direction within 2° in noise-free simulation.
- `audio_config::tests::v2_config_loads_as_v3`: read a v2 JSON (no `mic_geometry` field), confirm it parses, confirm `mic_geometry` defaults to the teardown values.

### 9.2 On-device acceptance tests (`scripts/test-beamforming.sh`)

Runs three sub-tests; all three must pass for SP#3 acceptance.

**9.2.1 Front-to-back ratio (Cardioid).** Bluetooth speaker plays a 1 kHz tone while positioned 1 m away at φ=0°. Script reads post-beam RMS from `AudioService` stats broadcast for 5 s. Operator moves speaker to φ=180°, script reads again. **Pass:** 20·log10(RMS_front / RMS_back) ≥ 10 dB.

**9.2.2 DOA accuracy.** Operator places speaker at each of 4 tape-marked positions (0°/90°/180°/270° at 1 m). Script reads `AudioState.doa_horizontal` after 2 s of speech-band signal at each position. **Pass:** |measured − ground-truth| ≤ 10° at all 4 positions.

**9.2.3 CPU budget.** Script starts waved, enables Supercardioid mode, runs a 30 s silence capture, samples `/proc/<pid>/stat` at 1 Hz, computes average CPU %. **Pass:** < 5% on an A53 core (reported per-mode: the four mono presets should be essentially identical since only the weights change; Stereo will be ~2× the beam cost because two DAS outputs).

### 9.3 Subjective validation (not a blocker, but in the test script)

Records 30 s of free speech while operator walks around the device. Output saved as WAV; operator listens post-test to confirm the beam "follows" them (voice stays loud; HVAC/computer-fan noise from a fixed angle gets attenuated as they move).

## 10. Rollout / migration

1. `AudioConfig` v3 is backward-compatible via `serde(default)` on `mic_geometry`. Existing on-device `/etc/waved-audio-config.json` keeps working; the embedded default supplies the geometry.
2. Persisted `AudioState` with `mode: "noise_reduction"` is migrated to `mode: "omni"` on first load (one-shot, logged warning). UI screen "Noise Reduction" is renamed to "Omni + NS" in SP#3's UI changes.
3. Tap-harness output is committed as a one-time change to `audio_config_default.json` in the same PR as the SP#3 code. This is the only "ship a literal physical measurement" step, and it's idempotent (re-running the tap on a new device will either match or reveal a hardware change requiring a new commit).
4. No wire-protocol changes. `Command::SetAudioMode` is already defined; SP#3 just wires up its handler in `AudioService`. `AudioState` fields (`doa_horizontal`, `doa_pitch`) are already defined; SP#3 starts populating them.

## 11. Open questions deferred to implementation

- Whether `waved-mic-map` lives as a new crate or a binary in `waved-device-test`. Both work; implementation plan picks based on final file-size.
- Exact weight derivation for Cardioid / Supercardioid on this specific 3D geometry — standard textbook formulas exist for horizontal linear/circular arrays; extending cleanly to the 2-ring arrangement may want a small analytic writeup during implementation. Fallback if we get stuck: compute weights numerically by solving `min ||W||² s.t. Wᵀd_target = 1, Wᵀd_null = 0` for each preset (one-time offline solve, constants baked into code).
- VAD source: Speex's `speech_prob` flag vs. a dedicated energy-threshold VAD. Prefer Speex for consistency with the existing pipeline; benchmark during implementation if it's flaky in low-SNR recordings.
