# Audio Hardware — 8-mic Array, DOA, Beamforming

## Mic Array Geometry

Two stacked rings of 4 MEMS mics each. All 8 mics sit on the **diagonal** azimuths
(45° / 135° / 225° / 315°) — there are **no** cardinal-axis mics. Origin at the
centre of the top-ring plane; +X forward, +Y left, +Z up. Units: mm.

| Physical idx | Ring   | Azimuth | (x, y, z) mm                |
|--------------|--------|---------|-----------------------------|
| 0            | TOP    | 45°     | (+18.03, +18.03, 0)         |
| 1            | TOP    | 135°    | (−18.03, +18.03, 0)         |
| 2            | TOP    | 225°    | (−18.03, −18.03, 0)         |
| 3            | TOP    | 315°    | (+18.03, −18.03, 0)         |
| 4            | BOTTOM | 45°     | (+18.03, +18.03, −78.2)     |
| 5            | BOTTOM | 135°    | (−18.03, +18.03, −78.2)     |
| 6            | BOTTOM | 225°    | (−18.03, −18.03, −78.2)     |
| 7            | BOTTOM | 315°    | (+18.03, −18.03, −78.2)     |

Ring radius R = 25.5 mm → xy = R·(1/√2) ≈ 18.03 mm. Bottom ring offset
z = −78.2 mm.

> **Do not "rotate to cardinal".** An earlier hypothesis was that the physical
> layout was actually aligned with the ±X/±Y axes and the estimator just had
> a 45° bias. It does not — rotating the geometry produced a ~−49° mean DOA
> offset in the 4-point validation, vs ~−6° for the diagonal layout.
> The earlier beam sweep that motivated the rotation (seemingly peaking at
> 45°) was within the cardioid main-lobe noise floor (~1.4 dB loss at 45°
> off-axis for a first-order cardioid with 51 mm aperture).

## ALSA → Physical Permutation

`hw:0` exposes 8 channels in a scrambled order relative to physical positions.
Measured on device `PSP_0000000000000000` with the `waved-mic-map` tap harness:

```rust
alsa_to_physical: [5, 4, 7, 6, 1, 0, 3, 2]
```

i.e. ALSA ch 0 → physical 5 (BOT 135°), ALSA ch 1 → physical 4 (BOT 45°), etc.
Every estimator must permute using `geom.position_for_alsa(alsa_ch)` or apply
the mapping directly — never assume ALSA index = physical index.

## DOA Pipeline

```
ALSA 8ch interleaved f32
   ↓ permute to per-physical-channel buffers
   ↓ Energy VAD on physical channel 0 (RMS > 0.0003)
   ↓ GCC-PHAT for 4 pairs: (0,2), (1,3), (0,4), (2,6)
   ↓ least-squares 3D solve (τ·c = (p_i − p_j) · d̂)
   ↓ EWMA smoothing on unit-vector az, linear on el
   → azimuth_deg ∈ [−180, 180], elevation_deg
```

**Pairs** (`crates/waved-audio/src/doa/mod.rs`):

| Pair  | Baseline                    | Observes          |
|-------|-----------------------------|-------------------|
| (0,2) | top 45°  ↔ top 225°         | +X+Y horizontal diagonal |
| (1,3) | top 135° ↔ top 315°         | −X+Y horizontal diagonal |
| (0,4) | top 45°  ↔ bot 45°          | pure-Z column, +X+Y side |
| (2,6) | top 225° ↔ bot 225°         | pure-Z column, −X−Y side |

Two top-ring pairs span the XY plane; two pure-Z (same-XY, different-Z)
pairs give elevation. We deliberately do **not** use the longer 3D
diagonals like (0,6) or (1,7) — see next section.

### Cross-ring pair acoustic shadow (why not (0,6)/(1,7))

Earlier iterations used the 3D diagonals `(0,6) = top 45°↔bot 225°` and
`(1,7) = top 135°↔bot 315°` for elevation because their 90 mm baselines
are the longest available and looked best on paper. On-device 4-compass
validation with `WAVED_DOA_DEBUG=100` showed the two diagonals are
*mutually inconsistent* — they disagree on elevation by 15–25° at every
azimuth, sometimes with opposite sign:

| Source az | (0,6) implies el | (1,7) implies el | Truth |
|-----------|------------------|------------------|-------|
| 0° (front)  | +17°           | −8°              | 0°    |
| 90° (left)  | +50°           | +35°             | 0°    |
| 180° (back) | 0°             | +45°             | 0°    |
| 270° (right)| 0°             | −38°             | 0°    |

Root cause is body diffraction. The Wave is a ~40 × 40 × 80 mm block of
electronics/plastic between the top and bottom rings. At audio
wavelengths (340 mm @ 1 kHz, 85 mm @ 4 kHz) that block is a non-trivial
acoustic obstruction. Whenever the source is on one hemisphere, the
bottom mic on the opposite-hemisphere side of a 3D-diagonal pair is in
the body's shadow — it receives diffracted signal with smeared phase,
and GCC-PHAT lands on the wrong lag. Because the two diagonals are
rotated 90° from each other, they experience opposite shadow patterns
and give contradictory readings.

Pure-Z pairs (0,4), (2,6) sidestep this: both mics in each pair share
XY, so they see the same acoustic side of the device. Any body-shadow
effect applies to both equally and cancels out of the time-difference
estimate. The trade-off is a shorter 78 mm baseline (vs 93 mm for the
diagonals) — acceptable because τ_max at the edge of elevation is still
~11 samples at 48 kHz, well within `max_lag_samples = 20`.

This is a device-intrinsic property, not a per-unit manufacturing
issue. Every Wave has the same body geometry.

### Elevation bias measurement (F2)

**Diagnostic instrumentation:** `WAVED_DOA_DEBUG=N` now emits per-pair
sharpness (`sh=`) and per-pair implied elevation (`el_p=`) for each
of the 4 pairs in the regular debug line. Pure-Z pairs (0,4)/(2,6)
emit a real `el_p`; XY-horizontal pairs (0,2)/(1,3) emit NaN
(elevation is not meaningfully isolable from these geometries).

Example log line (production format):

```
doa_debug az=178.2 el=3.1 rms0=0.00012 |
  (0,2)raw=-0.03 exp=-0.05 sh=1834.2 el_p=+nan
  (1,3)raw=+0.44 exp=+0.42 sh=1721.9 el_p=+nan
  (0,4)raw=-1.21 exp=-1.30 sh=1532.6 el_p=+12.4
  (2,6)raw=-0.18 exp=-0.25 sh=1488.1 el_p=+8.7
```

**Measurement sweep:** `scripts/test-doa-elevation.sh` walks an
operator-positioned loudspeaker through 4 azimuths × 4 elevations
(-30°, 0°, +30°, +60° relative to top-ring plane). Output CSV:
`elevation-measurements-YYYYMMDD.csv` with columns
`truth_az, truth_el, measured_az, measured_el, sharp_0_2, sharp_1_3,
sharp_0_4, sharp_2_6`.

**Outcome decision (per F2 spec §3.4):**
- If bias is **consistent across azimuth** and matches a simple
  geometric offset → document as test-rig artifact, NO code fix.
- If bias **varies wildly per azimuth** → root cause is algorithmic;
  queue an F2b fix.

**Measured (F2, 2026-05-27, device PSP_0000000000000000, GCC-PHAT,
broadband stimulus).** Single-azimuth sweep at az=0° (front), 4
elevations. **Truth elevations were eyeballed, not goniometer-set —
treat as ±several degrees**, so the numbers below characterise the
*shape* of the response, not a calibration:

| truth el | measured el | measured az |
|----------|-------------|-------------|
| −30°     | ≈ −15°      | ≈ 357°      |
| 0°       | ≈ +7°       | ≈ 355°      |
| +30°     | ≈ +45°      | ≈ 351°      |
| +60°     | ≈ +52°      | ≈ 352°      |

Conclusions robust to the eyeball uncertainty:

1. **Sign and ordering are always correct** — the estimate is
   monotonic in truth elevation (below → negative, level → ~0, above →
   positive). Elevation *is* observable.
2. **It saturates at steep angles.** Local slope falls from ~1.3 in
   the 0→+30° band to ~0.23 in the +30→+60° band; +60° barely reads
   higher than +30°. The negative side compresses similarly
   (−30°→−15°). The (0,4) pure-Z pair's `el_p` sticks at ~+33–37° for
   *both* +30° and +60° truth — the vertical aperture runs out of τ.
3. **Absolute accuracy is poor (10–15°+), azimuth is unaffected** —
   az stayed ~351–357° (front) across the whole elevation sweep.

**Outcome decision (§3.4): GEOMETRIC — document as coarse, no code
fix.** The dominant effect (saturation + the per-pair `el_p` ceiling
~+35°) is a vertical-aperture limit of the cross-ring Z-pairs
(0,4)/(2,6), which have small `dz`; τ saturates at steep elevation and
observability collapses. This is not fixable in software. Treat
`elevation_deg` as a **coarse, low-confidence band** (below / level /
above), never a calibrated angle. The small near-level offset (~+7°,
consistent with the SP#3 note `+3° to +18°`) plus the noisier (2,6)
pair's overshoot *could* be partially calibrated, but (a) truth here
was eyeballed so no curve is fittable, and (b) only one azimuth was
sampled, so §3.4's "consistent across azimuth" test was not fully
exercised. A real algorithmic-vs-geometric verdict on the residual
offset would need a goniometric rig and the full 4-azimuth sweep
before any F2b is queued.

> **Caveat on this run:** truth elevations eyeballed; single azimuth
> (az=0°) only; `rms0 ≈ 0.002` (moderate level, stable estimates).
> Raw bursts in `/tmp/doa_e{0,30,60,en30}.log` on-device at capture time.

### F3 diarization acceptance (DoA-based, measured 2026-05-27)

Diarization is **purely DoA-clustered** — `waved-meeting/src/diarization.rs`
buckets confident DoA frames into azimuth clusters
(`CLUSTER_TOLERANCE_DEG = 30`, nearest-centroid within tolerance) and
emits a numeric `speaker_id` per cluster. **No names, no embedding/NPU
rescore** — see the project diarization-scope decision. Gating:
`CONFIDENCE_THRESHOLD = 0.30`, `MIN_SEGMENT_DURATION_MS = 200`,
`MAX_SEGMENT_SILENCE_MS = 500` (a >0.5 s talk gap closes a segment).

**On-device walkthrough** (`scripts/test-diarization.sh`, single operator
talking at distinct azimuths with pauses while moving). Result, meeting
`160608fa` (17 s, Omni): **4 segments, 2 distinct speakers, sequence
`1 1 2 1` → PASS** (spec wants ≥ 2). Segments:

| speaker | start–end ms | az |
|---------|--------------|------|
| 1 | 3392–4608   | 343°   |
| 1 | 8555–9750   | 2.6°   |
| 2 | 14848–15467 | 307.8° |
| 1 | 15488–16214 | 330°   |

The DoA→cluster→`speaker_id`→SQLite (`diarization_segments`) path is
proven end-to-end. Two caveats for the next run:

- The "behind" talk registered at **~307° (rear-right), not 180°** — the
  front/right hemisphere captured everything. A controlled loudspeaker at
  180° read accurately in the F2 azimuth sweep (2.6° error), so this is
  most likely operator placement (walked behind-and-beside, not dead
  centre). **Front/back separation was not cleanly stress-tested** here;
  redo with marked floor positions if it matters.
- **Select "latest meeting" by `rowid`, not `created_at`.** The device
  clock sits at the 1970 epoch, so `meetings.created_at`
  (`DEFAULT datetime('now')`) is non-monotonic and `ORDER BY created_at`
  can return a stale meeting (this bit the first query — a 2 s
  `NoiseReduction` ghost — before switching to `rowid`).

### Sign Conventions (read carefully)

- **GCC-PHAT** (`gcc_phat.rs`): `estimate_tdoa(x1, x2, ..)` returns τ (samples,
  fractional) such that **x2[n] ≈ x1[n − τ]**. Positive τ means x1 leads x2.
- **solve_direction** (`solve.rs`): the equation is `τ · c = (p_i − p_j) · d̂`
  where d̂ points **toward** the source. Positive τ therefore means mic i
  **leads** mic j (the source is on mic i's side of the pair). The inline
  comment in `solve.rs` reads "delayed" but the math treats it as "leads" —
  unit tests pin the behaviour down; do not trust the comment alone.

### GCC-PHAT Outlier Guard

Parabolic sub-sample interpolation is only valid on a concave peak (y_c > y_l
and y_c > y_r), which forces `denom = y_l − 2y_c + y_r < 0`. When pairs are
effectively uncorrelated (e.g. noisy cross-ring pairs, no speech) the PHAT
correlation surface is flat/lumpy and the denom can sit near zero with
arbitrary sign — the old `|denom| < 1e-9` guard produced τ values of
±hundreds of samples.

Current guard: `denom < −1e-9` (strictly concave) AND clamp fractional offset
to ±0.5. Regression test: `gcc_phat::tests::noise_pair_is_bounded_not_hundreds_of_samples`.

### VAD Threshold

`EnergyVad` default is **0.0003** (~−70 dBFS RMS). Raw Wave mic signals sit
around −75 dBFS without preamp gain; the previous default of 0.003 (−50 dBFS)
silently rejected every frame and froze the DOA.

If you measure that room noise passes VAD in silence, raise the threshold —
don't lower it below 0.0001.

### Maximum Lag

`max_lag_samples = 20` at 48 kHz. Physical: max pairwise distance is the
top-to-bottom diagonal ≈ 90 mm → τ_max = 90 mm / 343 m/s ≈ 12.6 samples.
20 gives ~60% headroom for numerical noise and peak-search-window symmetry.

### Per-pair confidence-weighted LS (F2)

The LS solver in `solve.rs` weights each pair's row by its GCC-PHAT
peak sharpness (`solve_direction_weighted`). Sharpness =
`-(y_l - 2*y_c + y_r)` from the parabolic fit, clamped to ≥ 0;
positive for concave (real) peaks, ~0 for flat/lumpy noise surfaces.

Empirical signal/noise discrimination measured in
`gcc_phat::tests::peak_sharpness_discriminates_broadband_signal_from_noise`:
broadband signal sharpness ≈ 1900, broadband noise sharpness ≈ 185
→ 10× ratio (well above the 5× gate for the weighting to be useful).

**Wiring in `DoaEstimator::update`:** raw sharpness values are passed
to `solve_direction_weighted` unscaled. The LS solution is
scale-invariant in weights, and raw values avoid normal-matrix
determinant collapse on near-singular pair geometries — normalizing
4 raw sharpnesses to Σ=1 shrinks the determinant by ~16-64× and
risks tripping the 1e-9 singularity guard.

`MIN_SHARPNESS_THRESHOLD = 1e-4` — when Σ pair sharpnesses falls
below this (silence-on-VAD-edge), the solver returns `None` and the
caller emits a `DoaFrame { vad: false, NaN angles, confidence: 0.0 }`.

**Scope note on the 270° azimuth residual:** the original SP#3 270°
bug is caused by SIMULTANEOUS shadowing of BOTH XY-horizontal pairs
(both top-ring mics on the +Y side are blocked from a -Y source).
Weighted-LS cannot exploit symmetric degradation. Validation that
weighted-LS at least preserves accuracy when ONE pair is corrupted
is in `doa::tests::update_with_weighted_solver_recovers_az_on_recoverable_scenario`.
The actual 270° production fix comes from SRP-PHAT (below).

**On-device measurement** (device `PSP_0000000000000000`, **broadband
noise** — see stimulus note below, 8–10 sample circular mean of
`doa_horizontal`):

| Target az | GCC-PHAT measured | err | SRP-PHAT measured | err |
|-----------|-------------------|-----|-------------------|-----|
| 0° (+X front) | 359.0° | 1.0° | — | — |
| 90° (+Y left) | 74.0° | 16.0° | — | — |
| 180° (−X back) | 182.6° | 2.6° | — | — |
| 270° (−Y right) | 283.2° | 13.2° | 263.1° | **6.9°** |

The X-axis (0°/180°) is excellent (≤3°). **Both Y-axis points are
compressed ~13–16° *toward the front* (0°)** under GCC-PHAT — a
symmetric bias, not noise (per-sample spread was ±1°). This is the
same body/geometry effect that makes 270° hard: off the X-axis, both
XY-horizontal pairs degrade together. **SRP-PHAT roughly halves the
270° error (13.2° → 6.9°), crossing the ±10° gate** — confirming SRP
is the real fix for the Y-axis weakness (at a CPU cost, below). SRP is
noisier per-sample (spread ±10°, grid-limited) but far less biased.

> **Stimulus note (important):** a pure 1 kHz tone is *pathological*
> for any PHAT-based DoA — PHAT normalises every bin to unit
> magnitude, so single-frequency energy carries no usable
> cross-correlation peak and the estimate locks onto room reflections
> (1 kHz ≈ 34 cm → strong indoor standing waves). The same 0° source
> read 295° (65° err) with a tone vs 359° (1° err) with broadband.
> **Always drive DoA acceptance with broadband noise or speech**, never
> a tone. (The `scripts/test-doa-*.sh` prompts were corrected to say
> "broadband" accordingly.)

### SRP-PHAT alternative backend (F2)

Selected via `AudioConfig.doa_backend = "srp_phat"`. Grid search over
72 azimuths × 19 elevations = 1368 directions, accumulating
PHAT-weighted cross-spectrum across the same 4 pairs as GCC-PHAT
((0,2), (1,3), (0,4), (2,6)). Voice-band masked (200–4000 Hz).
Sub-grid refinement via parabolic interp in az/el (~0.5° accuracy).

Confidence = `peak_power / (n_pairs × n_bins)` (theoretical-ceiling
ratio), clamped [0, 1]. Plane wave at the right grid cell gives
confidence ≈ 0.95; uniform per-channel noise gives ≈ 0.16. Same
normalised shape as GCC-PHAT path — F3 can threshold on a single
value regardless of backend.

**Selection at startup (static — runtime switching not supported):**

```json
{ "doa_backend": "srp_phat" }
```

**Fallback:** if SRP construction fails (currently only when
`frame_size = 0` — `rustfft` panics otherwise), AudioService falls
back to GCC-PHAT and sets `AudioState.doa_backend_fallback = true`,
surfaced in `/tmp/audio_stats.json`.

**CPU (measured, device `PSP_0000000000000000`, broadband load, waved
`utime+stime` over 12 s, USER_HZ=100):**

| Backend | jiffies / 12 s | ≈ % of one core |
|---------|----------------|-----------------|
| GCC-PHAT | 699 | ~58% |
| SRP-PHAT | 1355 | ~113% |

**SRP-PHAT costs +93.8% CPU — nearly 2× GCC-PHAT** (the 1368-direction
grid search dwarfs GCC's 4 pairwise correlations). This blows past the
F2 spec's optimistic "< 5%" gate by ~19×. The gate was unrealistic;
the real decision is **GCC-PHAT stays the default**, and SRP-PHAT
ships as an **opt-in** backend (`doa_backend: "srp_phat"`) for cases
that need the Y-axis accuracy *and* have a spare core — it fits on
this device (one extra core absorbs the ~113% load) but should not be
the always-on default.

## Diagnostic: `WAVED_DOA_DEBUG`

Set at startup to log raw-vs-expected τ per pair every N frames:

```
WAVED_DOA_DEBUG=100 waved
```

Unset or `=0` → off (production default). Log target
`waved_audio::doa::debug`, info level. Example line:

```
doa_debug az=178.2 el=3.1 rms0=0.00012 | (0,2)raw=-0.03 exp=-0.05 sh=1834.2 el_p=+nan  (1,3)raw=+0.44 exp=+0.42 sh=1721.9 el_p=+nan  (0,4)raw=-1.21 exp=-1.30 sh=1532.6 el_p=+12.4  (2,6)raw=-0.18 exp=-0.25 sh=1488.1 el_p=+8.7
```

`raw` = GCC-PHAT output, `exp` = τ predicted from the current smoothed
direction and the pair's baseline. Large disagreement on a single pair
points at a bad channel / permutation error.

## Beamforming Modes (SP#3)

First-order cardioid family + MVDR. Main lobe of a cardioid `(1+cos θ)/2`
is **broad** — only ~1.4 dB attenuation at 45° off-axis. Consequences:

- Beam-pattern sweeps with narrow-band tones at this aperture (51 mm, ~0.15λ
  at 1 kHz) will not show sharp azimuthal peaks. Use broadband pink noise
  and expect modest front-to-back ratios (in-room target ≥ 4 dB, analytic
  ≥ 10 dB — see `beam::tests::cardioid_achieves_ten_db_front_to_back`).
- Don't read beam-pattern "plateau" as "geometry must be wrong". Validate
  geometry via DOA (phase) tests — TDoA is 10× more sensitive to mic
  placement than cardioid directivity.

Acceptance test harness: `scripts/test-beamforming.sh`.

### CPU baseline (Cortex-A55 @ ~1.2 GHz, 4 cores)

Measured per-thread on silence, `/proc/<pid>/stat` sampled over 15 s. Whole
waved process — not per core.

| Mode          | Total CPU | Δ vs Omni |
|---------------|-----------|-----------|
| Omni          | ~8.99%    | 0 (baseline) |
| Cardioid      | ~9.36%    | +0.37% |
| Supercardioid | ~9.28%    | +0.29% |
| Figure-8      | ~9.23%    | +0.24% |

The beamformer itself is essentially free (<0.5% delta). The ~9% floor
is tokio runtime overhead + ALSA capture + AEC + per-frame channel
fanout — **none of which is beam-related**. The spec's "Beam CPU < 5%"
applies to the *delta*, not the absolute waved figure. `test-beamforming.sh`
Test 3 now reports the delta explicitly.

Per-thread breakdown at Omni (15 s, silence):

| Thread            | Share |
|-------------------|-------|
| tokio-rt-worker × 4 | ~7.8% |
| audio-capture     | ~1.3% |
| audio-playback    | ~0.3% |
| ui-lvgl           | ~0.2% |

### Steered cardioid (F2)

`AudioMode::Steered { initial_az_deg }`. Cardioid pattern with
direction tracking driven by the per-frame DoA stream from
`AudioService::subscribe_doa()`. Direction smoothed via slerp on
unit-vectors, α=0.15 (~140 ms time constant at 21 ms ALSA period).

State machine (`crates/waved-audio/src/beam/steered.rs`):

- **High-confidence DoA frame** (`vad && confidence > 0.30`):
  direction EWMA's toward the new target. Hard-tracks rapid speaker
  movement with bounded smoothing.
- **Low-confidence / silence frame**: direction held. After 240
  consecutive frames (~5 s), `omni_mix` ramps 0→1 over 50 frames
  (~1 s) cosine fade.
- **Next high-confidence frame**: `omni_mix` ramps back to 0 with
  the same 50-frame step.

Mix is a linear crossfade in `Beamformer::process`:
`(1 - mix) · cardioid + mix · omni`. Both halves are unity-DC-gain
so the crossfade is click-free.

Thresholds (`CONFIDENCE_THRESHOLD = 0.30`, `REVERT_SILENCE_FRAMES =
240`, `OMNI_RAMP_FRAMES = 50`) are hardcoded in `steered.rs`;
promotion to `AudioConfig` only if tuning evidence demands.

**On-device test:** test 6 of `scripts/test-doa-improvements.sh`
(subjective — operator listens to moving-source recording).

### Milestone-2 changes (2026-10-05/06)

- **Fixed aim.** Focused (Cardioid), Narrow (Supercardioid) and Two sides (Figure8)
  steer to `audio.beam_aim_deg`, in device degrees (the DoA frame: 0° in front of the
  screen, counter-clockwise, 90° on the listener's left). Stereo steers to (0°, 0°).
  `SetDoaOverride` still overrides both. The UI converts to screen degrees in
  `waved-ui/src/kit/direction.rs`.
- **Whole room (Omni) aligns on the talker.** Summing the eight mics unaligned cancelled
  the voice's 2–6 kHz range for any source off the vertical axis (simulated: −12 dB at
  4 kHz from the side, −25 dB with the mouth 30° up), and callers heard a muffled,
  distant voice. Omni now delay-and-sums on the talker tracked by `SteeredBeam`
  (`Beamformer::align_delays`; flat within 1.5 dB to 8 kHz on axis) and fades to the
  plain average (`omni_mix` 1) while no direction is confident.
- **The directional modes lost bass** (first-order differential: −16 dB at 125 Hz,
  −10 dB at 250 Hz, simulated). Now compensated: "Directional-mode bass" below.

`/tmp/audio_stats.json` carries `beam: { az_deg, el_deg, omni_mix }` from
the latest processed frame (C19): where the beam points and how far it leans
on the plain average. In Whole room `omni_mix` 0 means the mics are lined up
on the talker, 1 the plain average (−12 to −25 dB at 4 kHz off-axis); in
Steered it is the omni blend above. The fixed modes report their aim and 0.

### Directional-mode bass (C18, simulated 2026-10-06)

A first-order differential `front − back(t − τ)` responds like
`2·sin(ω·D/2)`, `D` = acoustic front-to-back delay + back delay. At 48 kHz,
aimed at 0° el 0: Cardioid D = 10.1 samples, Supercardioid 8.0, Figure-8
5.0, a Stereo side 12.2 for the talker in the middle (14.3 on its own axis).
Below its peak that is a 6 dB/octave high-pass, and its first null is at
fs/D (4.8 kHz for Cardioid, 3.9 kHz for a Stereo side and for a Cardioid
aimed at a mic diagonal).

`beam::bass::BassShelf` undoes the slope on each pattern output (not on
Whole room, not on Steered's omni blend): a one-pole low shelf, at most
+12 dB at DC (for mic self-noise), pole at 85 Hz, zero at `ω·D = 2.2`. A flat
make-up gain (`bass::presence_makeup`, before the omni blend) then puts the
talker at Whole room's level over 1–3 kHz, so switching mode keeps the
level. It is derived from `2·|sin(π·f·D/fs)|` × the shelf, averaged over the
band, for a talker at most 30° up (steeper, the pattern's null moves into the
band and levelling it lifted 1 kHz by +7 dB at el 60; now +4.5).

Shelf and make-up together are capped at +18 dB at low frequencies against
Whole room (`bass::tune`): content that is not a plane wave (self-noise, mic
mismatch, structure-borne echo, knocks) otherwise reached +18 to +24 dB ahead
of the hard clip before the echo canceller. The shelf's DC boost is
`min(12, 18 − make-up)`: it keeps the gain above its zero and raises its
pole instead, and the two are iterated to agreement.

| Mode | Make-up | Shelf at DC | Total |
|---|---|---|---|
| Cardioid / Steered | +6.2 dB | +11.8 dB | +18.0 dB |
| Supercardioid | +8.1 dB | +9.9 dB | +18.0 dB |
| Figure-8 | +12.4 dB | +5.6 dB | +18.0 dB |
| Stereo L or R | +5.3 dB | +12.0 dB | +17.3 dB |

Front talker, el 0, mono output vs plain average (`beam::tests`):

| Mode | 50 | 125 | 250 | 500 | 1k | 2k | 3k | 4k Hz |
|---|---|---|---|---|---|---|---|---|
| Cardioid, before | −23.6 | −15.7 | −9.7 | −3.8 | +1.8 | +5.7 | +5.3 | −0.4 |
| Cardioid / Steered, after | −6.8 | −2.5 | −1.2 | −0.7 | −0.1 | +0.4 | −1.2 | −7.4 |
| Supercardioid, after | −8.5 | −3.4 | −1.6 | −1.0 | −0.5 | +0.2 | −0.1 | −2.0 |
| Figure-8, after | −11.9 | −5.4 | −2.3 | −1.0 | −0.5 | +0.0 | +0.4 | +0.4 |
| Stereo L or R, after | −6.0 | −1.7 | −0.5 | +0.1 | +0.8 | +0.7 | −3.5 | −26.9 |

Figure-8, with the shallowest shelf, is within 4 dB of 1 kHz from 250 Hz to
4 kHz and about −5 dB at 125 Hz.
Cardioid stays within ±3 dB of 1 kHz from 125 Hz to 3 kHz; its 4 kHz point
is the pattern's own null, which no low shelf can lift without breaking the
2 kHz side (a monotonic filter can't both cut 2 kHz and keep 4 kHz). A mode
change crossfades the old and new patterns over 10 ms (`MODE_FADE_MS`);
leaving Stereo, L and R fade into the new mono output.

## Full-duplex AEC pipeline

The conferencing uplink runs the following order for every microphone frame:

```
8-channel capture → beamforming → Speex linear AEC
    → linked Speex residual-echo suppression
    → optional environmental noise suppression
    → user mic gain → recording limiter → UAC2 / meeting recording
```

Residual echo suppression is independent of the user-facing noise-reduction
setting and remains active for `NrLevel::Off`. In residual-only mode, Speex's
spectral engine stays enabled because that switch also gates echo suppression,
while environmental noise attenuation is explicitly set to 0 dB. If DFN3 is
unavailable for Low/Med/High, the already-linked Speex preprocessor enables its
15 dB environmental denoise path instead of constructing a second echo-linked
native state.

Audio-config version 5 adds:

```json
"aec": {
  "residual_echo_suppress_db": 40,
  "residual_echo_suppress_active_db": 15
}
```

Both values are positive attenuation magnitudes, limited to 0–60 dB, with the
active/double-talk value no greater than the far-end-only value. Versions 3
and 4 remain accepted and receive the 40/15 defaults.

Speex preprocessing has one analysis-frame of inherent delay. The production
candidate preserves the proven 1024-sample/200 ms primary linear echo state
and runs the residual estimator plus its linked preprocessor at 128 samples
with a 128 ms shadow tail. Host cross-correlation measures a fixed 2.67 ms
shift at 48 kHz, with no Rust-side queue. The preprocessor is primed with
silence at construction. Host tests require silent-reference near-end loss
≤0.5 dB and synthetic double-talk loss ≤3 dB.

Native alternatives were rejected rather than weakening those gates. Linking
a 128-sample preprocessor directly to the 1024-sample echo state corrupted
native memory because Speex requires matching linked frame sizes. One matched
1024-sample state was CPU-efficient but shifted output by a full 21.33 ms
frame and broke first-frame service tests. One matched 512-sample state cut
that delay to 10.67 ms but used more CPU because it ran twice as many
transforms. A 200 ms shadow tail passed quality but was needlessly expensive;
128 ms is the shortest tested tail that still keeps synthetic double-talk loss
within 3 dB.

`/tmp/audio_stats.json` contains an `aec` object captured from the exact
processed frame:

- `mic_pre_aec_rms`
- `mic_post_linear_aec_rms`
- `mic_post_residual_echo_rms`
- `mic_post_noise_suppression_rms`
- `mic_uplink_rms`
- `linear_erle_db` and `effective_echo_suppression_db`
- residual-stage active/tuning fields and `noise_suppression_backend`

Derived suppression values are omitted when the same frame's reference is
below the activity floor. The legacy top-level `mic_post_aec_rms` name is kept
for compatibility but means the **final uplink after all DSP, user gain, and
limiting**. It must not be compared with a raw mic channel to calculate ERLE.

Use `scripts/test-aec-on-device.sh` for the quick tone convergence check.
`scripts/test-aec-conferencing.sh` requires `SPEECH_FIXTURE=/path/to/speech.wav`
and is the real full-duplex gate. Both scripts read the same-frame metrics.

On device `PSP_0000000000000000` at 100% speaker volume, an eight-second
speech-shaped far-end fixture measured 27.0 dB linear ERLE and 36.1 dB final
effective suppression after the configured 4× microphone gain. Reference RMS
decayed to zero and all XRUN/raw/processed/UAC drop counters stayed at zero.
An identical six-second playback CPU sample measured 1,894 process ticks
versus 1,717 for the saved pre-residual build (+177 ticks, approximately
+29.5 percentage points of one core with USER_HZ=100). This exceeds the
original +5-point provisional delta target, although the four-core device
retained real-time integrity. Sustained bidirectional load and the live
listener echo/delay report remain acceptance gates.

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
Switched at runtime via `Command::SetPlaybackPreset` or
`/tmp/waved-playback-preset` (`voice` | `media`) + SIGHUP — same wiring as
`/tmp/waved-nr` for SP#4.

### SP#5 acceptance results (measured 2026-04-30)

- **AEC ERLE non-regression: 23.2 dB** (SP#2 baseline 22.7 dB; SP#5 bar
  20.7 dB). The post-DSP tap actually delivers a *more* accurate reference
  than the pre-DSP tap — AEC sees what the speaker emits including EQ and
  limiting, so cancellation tightens slightly. Test: `scripts/test-aec-on-device.sh`
  with `LINEAR_ERLE_THRESHOLD_DB=20.7`. This historical result predates the
  residual stage; rerun the current same-frame acceptance after deployment.
- **Bit-exact biquad regression**: `cargo test -p waved-audio --test biquad_regression`
  pins direct-form-1 + RBJ peaking against a Python f64 reference impulse
  response within 1e-6.
- **Voice EQ stats sanity** (test 1 of `scripts/test-playback-eq.sh`): pass —
  `playback_preset="voice"`, `mbagc_active_gain_db=[6.0, 20.0, 10.0, 10.0]`.
- **Soft-talk MBAGC** (test 2): pass — total per-band gain 46.0 dB on
  −40 dBFS multi-tone input (bar ≥ 4 dB).

### Known caveats (acceptance test)

- **`limiter_reduction_db` snapshot is last-block, not peak-hold.** Test 3
  of `scripts/test-playback-eq.sh` reads stats *after* SIGUSR2 file-play
  finishes; by then playback is silent and the limiter envelope has
  released back to 1.0 (`reduction_db = 0.0`). The limiter still functions
  correctly during the actual square wave — just not visible in stats
  post-hoc. Fix is a small change to `LookaheadLimiter` (peak-hold with slow
  decay window matching the 500 ms stats cadence). Future SP#5.5 cleanup.
- **CPU baseline 11.5% in `scripts/cpu_delta.py` is pre-SP#4.** With DFN3
  enabled by default, the post-SP#4 baseline at silence is much higher
  (~43% observed). Re-baselining belongs with the SP#4 follow-up that
  pre-builds DFN3 at startup.
- **Preset switch is a hard swap, no cosine gain ramp.** Spec §2.8 called
  for a 5 ms ramp at the boundary; current `PlaybackDspChain::set_preset`
  swaps coefficients directly at the next period and resets MBAGC envelopes.
  Test 4 (manual rapid voice↔media flip under pink noise) passed without an
  audible click on this hardware, but the risk is real if presets ever
  differ more aggressively. See `TODO.md` SP#5 follow-ups.
- **`MbagcParams.knee_db` is currently a dead field.** It is populated from
  the stock voice preset (`[5.0, 5.0, 5.0, 5.0]`) and round-trips through
  config, but `band_gain_linear` does a hard clamp instead of a soft-knee
  transition. Voice preset therefore diverges slightly from stock at the
  gain-curve knee. See `TODO.md` SP#5 follow-ups.

**Local playback (milestone 2).** `play_local` input (Files playback) is mixed into the
host stream with a saturating add *before* HPF/EQ/MBAGC/limiter, so the speaker volume,
the limiter and the AEC reference all cover it. The saturating add can clip loud host
audio before the volume stage.

## UAC2 Routing (SP#6)

The kernel `f_uac2` gadget exposes the Wave as a stereo S16 48 kHz USB
Audio Class 2 device on `card 1` (`UAC2Gadget`). `crates/waved-audio/src/uac/`
bridges both directions to the on-device speaker and mics.

```
host (Mac/Wave Controller)
  ↑ ALSA writei                  ↓ ALSA readi
hw:1,0p                       hw:1,0c
(UAC2 IN — what host reads)   (UAC2 OUT — what host writes)
  ↑                              ↓
UacUpstreamSink ──┐         ┌── UacBridge
(std::thread)     │         │   (std::thread)
  ↑               │         ▼
ProcessedAudioFrame   PlaybackService::play_batch
(post-AEC mono)              ↓
  ↑                          SP#5 DSP chain
audio-capture                ↓
                       hw:0,1 (TDM-B → acm8625s)
                       broadcast tap → AEC reference
```

Both `uac-downstream` and `uac-upstream` threads run at SCHED_FIFO
priority 18 (one less than the speaker playback thread's 20, so
speaker-clock wins on contention). Drift handling is drop-on-overflow at
the period boundary: when the host clock outruns the device clock and
`play_batch` queue depth exceeds `buffer_periods`, the bridge drops the
period (`uac.downstream_drops` in `/tmp/audio_stats.json`). Same pattern
on upstream via the `broadcast::Receiver::Lagged` path.

### Volume + mute mirror

`UacControl` polls the `hw:1` ALSA mixer at 50 ms cadence. On a host
SET_CUR (Mac slider drag), the kernel emits a value-change event;
`UacControl` translates it to `Command::SetSpeakerVolume(v)` /
`Command::SetMicVolume(v)` / `Command::Toggle*Mute` on the bus. Reverse
direction: a `watch::Receiver<AudioState>` observer writes back to the
mixer when the device-side state changes, so the Mac slider mirrors
within ~1 s.

A `LastWrite` cell suppresses self-event echo: when the device writes a
new volume, the matching kernel-emitted value-change event is recognised
and not redispatched onto the bus (no infinite ping-pong).

`run_playback_loop` reads `speaker_volume` / `speaker_muted` from the
state watch once per batch and applies the result as a scalar to the
post-DSP buffer (mute → 0, otherwise `volume_pct/100`, linear). The AEC
reference broadcast taps **after** the gain so the canceller sees what
actually reaches the mics.

### MCU heartbeat on `/dev/ttyS1` — and the "button code" myth

The MCU emits a **single heartbeat byte `0x4B` roughly every 0.5 s** on
`/dev/ttyS1` (115200 baud), outside the `0xDC 0xCD` frame protocol used for
motor/battery commands. That is the *only* unframed traffic on the wire.

⚠️ **There is no MCU raw-byte button protocol.** An earlier revision documented
`0x4A`/`0x49`/`0x48` as Volume Up / Volume Down / Mute "press events measured
on-device via strace", and `UartMux` injected synthetic button frames for any
standalone byte in `0x48..=0x4A`. On-device capture (strace of the `ttyS1`
reader, correlated with `waved.log` and `uac::control`) disproved this:

- The heartbeat byte gets **individual-bit corruption with low bits cleared**.
  Captured variants of `0x4B` (`0100 1011`) included `0x4A` (`…1010`),
  `0x49` (`…1001`), `0x09` (`0000 1001`) and `0x08` (`0000 1000`). The `0x08`/
  `0x09` values — outside any "button" range — prove these are line corruption,
  not intentional messages.
- All anomalies stay locked to the **0.5 s heartbeat grid**; none are
  human-timed. Pressing and holding the physical Volume Up button **five times
  produced zero `0x4A` bytes** and the Mac volume only drifted *down* (from
  `0x49` corruptions) — i.e. physical buttons do **not** reach `waved` over
  `ttyS1` at all.
- Neither input device advertises volume keys: `vad_keypad` (`event0`) reports
  only `KEY_POWER`; `ILITEK_TDDI` (`event1`) reports only touch codes.

Because the button codes `0x48..=0x4A` sit one or two cleared bits away from the
constant `0x4B` heartbeat, every corrupted heartbeat that landed on `0x49`/`0x4A`
was forwarded to the Mac as a HID Consumer Volume key — the **"ghost volume"**
bug. The raw-byte interception in `UartMux` has been removed; unframed bytes
(heartbeat and its corruptions) are now discarded by the frame parser. The
framed-button slot (`McuService::handle_frame`, cmd `0x40..=0x4F`) remains for
the simulation/legacy protocol but carries no traffic on real hardware.

If genuine device-side volume buttons are wanted, the real hardware input path
must be identified first (it is **not** `ttyS1`, `event0`, or `event1`).

### HID control surface (SP#7d)

The device exposes a `functions/hid.0` USB-gadget interface with a combined
**Vendor (Report 0x03) + Telephony_V4 (Reports 0x02/0x05/0x06/0x07) +
Consumer (Report 0x01)** report descriptor, copied byte-for-byte from stock
fw v1.1.6.6. The Rust implementation lives in `crates/waved-hid/`:

- `HidService::run` (`lib.rs`) — owns `/dev/hidg0` write side. Bridges
  `DeviceEvent::ButtonPress(ButtonType)` from `waved-mcu` and drains an
  `mpsc<HidEvent>` from state-driven sources. Writes press-release pairs.
- `watch.rs` glue tasks — subscribe to `audio_tx`:
  - `spawn_volume_watch` — fires `ConsumerVolumeStep` events when
    `speaker_volume` changes. **Scaled to Mac's ~16-step range:**
    `mac_steps = max(1, round(delta × 16/100))`. Without scaling a
    50-unit slider drag would fire 50 HID steps → 312% Mac jump; with
    scaling it fires 8 steps → proportional 50% change. Rate-capped at
    `hid.volume_step_rate_hz` (default 40 Hz).
  - `spawn_mute_watch` — one `ConsumerMute` per `speaker_muted` edge;
    additionally one `TelephonyPhoneMute` when `call_off_hook == true`.
- `reader.rs` — blocking std::thread reads `/dev/hidg0`, parses LED
  Set_Reports (0x05/0x06/0x07) into `AudioState.call_off_hook /
  call_host_muted / call_ringing`.

Vendor channel (Report 0x03) is declared but not handled (SP#7(a) future).

Report IDs match the stock descriptor: **0x01 = Consumer, 0x02 = Telephony**.
This is the opposite of the original `waved-hid` constants prior to SP#7d,
which had the IDs swapped — see §4.2 of `docs/superpowers/specs/2026-05-21-
waved-hid-control-interface-design.md`.

Configfs setup lives in `scripts/patch-stock-usbgadget.sh`; reboot required
for the new gadget config to come up.

Stats: `/tmp/audio_stats.json` includes a `hid` block (`device_present`,
`events_emitted`, `leds_received`, `call_*`, `write_errors`, `glue_drops`).

### Mac → Wave control (kernel `f_uac2` 5.4 nomenclature inversion)

**The mapping is inverted compared to what the ALSA control names suggest.**
`f_uac2` names its mixer controls from the *gadget's* perspective: an audio
stream the host writes (= the gadget receives, = the device's *speaker*) is
called "Capture" on the gadget side, while a stream the host reads (= the
gadget produces, = the device's *mic*) is called "Playback". So:

| configfs    | ALSA control          | Direction       | What Mac calls it |
|-------------|-----------------------|-----------------|-------------------|
| `c_volume_*`| `PCM Capture Volume`  | host → gadget   | **speaker**       |
| `p_volume_*`| `PCM Playback Volume` | gadget → host   | **mic**           |

This was verified by reading the stock firmware: `handle_usb_msg_threadLoop`
in `libins_entry.so` subscribes to `snd_ctl_event_elem`, matches on
`"PCM Capture Volume"`, and pushes the value into `audio_server_spk_volume_policy`
(speaker policy). It then maps to `g_is_usb_spk_work` / `g_is_usb_mic_work`
state — confirming "Capture" on this gadget IS the host→gadget speaker path.

`UacControl` in `uac/control.rs` honours this — `read_volume("PCM", capture=true, …)`
returns the speaker volume Mac sent, `capture=false` would be the mic gain
(which kernel 5.4 doesn't actually expose, see below).

| Mac action | What happens | Why |
|------------|--------------|-----|
| Plays audio (Music/Spotify) | Streams over UAC2 OUT terminal, hits `UacBridge` → DSP → speaker. Works. | data path is straightforward |
| Sound prefs Output slider | Dispatches UAC2 SET_CUR(VOLUME) on the playback (speaker) FU → kernel writes `PCM Capture Volume`. We see `host set speaker volume → N` in the log. Wave's `speaker_volume` in `AudioState` updates and the OLED Volume screen reflects it. | works with patched `c_volume_min` (see below) |
| F11 / F12 (volume keys) | Same as the slider — only **if** the gadget advertises a usable volume range (≥ several dB) on the playback FU. With stock `c_volume_min=-100` (1/256 dB = ‑0.39 dB), Mac silently falls back to its own digital scaling. With `c_volume_min=-10240` (=‑40 dB) Mac uses the FU. | see "`c_volume` range" below |
| Sound prefs Input slider | Stock writes to `p_volume_*` (which would be the mic FU), but kernel 5.4 `f_uac2` doesn't surface `PCM Playback Volume` even when `p_volume_min/max/res` are configured. So no mic-gain control reaches userspace. | known kernel 5.4 limitation |
| F10 (mute key) | Dispatches UAC2 SET_CUR(MUTE) to capture FU (= speaker side after the inversion). `UacControl` reads `PCM Capture Switch` and dispatches via `host_mute_target`. | see `uac.host_mute_target` below |

The `uac.host_mute_target` config (in `audio_config.json`) routes Mac F10:
`"speaker"` (default, music-listener UX), `"mic"` (headset UX, original
behaviour), or `"both"`. Mac F10 then silences the speaker — which is
what most users want from a music-output speakerphone.

**`c_volume` range — units are 1/256 dB, not dB.** Stock
`/etc/init.d/S89usbgadget` writes `c_volume_min=-100, c_volume_max=0,
c_volume_res=1`, which kernel interprets as ‑0.39 dB to 0 dB — a useless
range. macOS sees this and silently bypasses the FU, using its own digital
volume scaling for the system volume keys. The init.d patch in
`scripts/patch-stock-usbgadget.sh` sets `c_volume_min=-10240` (=‑40 dB)
and `c_volume_res=256` (=1 dB step), giving Mac a real range it will drive.
The same gotcha applies to `p_volume_*` for the mic side (already patched
to `-16384` / `256`).

**Feedback-loop suppression.** When Mac F11/F12 lowers `PCM Capture Volume`,
`uac/control.rs` dispatches `Command::SetSpeakerVolume(v)`, which mutates
`AudioState.speaker_volume`. The HID `volume_watch` task in `waved-hid`
would normally then emit a `ConsumerVolumeStep` HID report back to Mac
(its job is to mirror Wave's volume changes to Mac so the touch UI works) —
which Mac interprets as another F11 press, dropping the FU value again,
ad infinitum. `StateBus::host_pending_speaker_vol` is a shared `AtomicI32`
echo-suppress marker: `uac/control.rs` stamps the host value before
dispatching, and `volume_watch` checks it and skips emission for matching
changes. Sentinel `-1` = no pending host echo.

If/when the kernel exposes `PCM Playback Volume` (the mic-gain side), the
same pattern can mirror that direction.

We tried disabling `p_volume_present` / `p_mute_present` / `c_mute_present`
to coax macOS into software-side attenuation. macOS silently no-ops
instead — it does not fall back to SW attenuation for UAC2 devices the
way it does for HDMI / built-in speakers. Don't disable those flags
(also kills the F10 mute redirect).

### `f_uac2` configfs gotcha — volume range units are 1/256 dB

Stock `/etc/init.d/S89usbgadget` writes `p_volume_min=-100`,
`p_volume_max=0`, `p_volume_res=1`. These look like dB but the kernel
interprets them as 1/256 dB units — so the device advertises only
`~0.4 dB` of HW volume range. macOS treats that as "not a real HW
volume", silently bypasses the FU, and does not dispatch UAC2 SET_CUR
for system volume keys.

**Fix:** advertise a useful range. `scripts/patch-stock-usbgadget.sh`
patches the init script to:
- `p_volume_min=-16384` / `p_volume_res=256` (= ‑64 dB to 0 dB in 1 dB steps) for the mic FU
- `c_volume_min=-10240` / `c_volume_res=256` (= ‑40 dB to 0 dB in 1 dB steps) for the speaker FU

…backs up the original to
`S89usbgadget.bak.<ts>`, and is idempotent. After patching, reboot —
the gadget needs to come up fresh for macOS to re-read descriptors.

`uac.upstream_active` and `uac.downstream_active` flags are the
authoritative host-attach indicators — they read the kernel's
`PCM Playback State` and `PCM Capture State` mixer switches every 50 ms.
The bridges' own I/O success was an unreliable proxy (writei to a UAC2 IN
endpoint succeeds even with no host reading; the kernel's State control
only flips `on` when the host has the endpoint open).

This SP also retired the `speaker_mixer::amixer` shell-out from the
`Command::ToggleSpeakerMute` / `Command::SetSpeakerVolume` handlers in
`AudioService::run`. Volume control is now end-to-end through one
mechanism — `UacControl` → UAC2 mixer.

### SP#6 acceptance results (measured 2026-05-01)

- **Host-attach detection latency:** `downstream_active` flips true
  6 s after the Mac's `afplay` start; `upstream_active` flips true 1 s
  after `sox -d` start (50 ms polling cadence + macOS open latency).
- **Host → speaker audible:** `mic_rms[0] = 102.1` during 1 kHz
  afplay through the Wave (bar > 100 = mics confirm the speaker
  emitted audible signal).
- **Volume mirror device → host:** `amixer -c 1 sset PCM 75%` →
  `amixer -c 1 sget PCM` reports `Mono: Playback 75 [75%]`.
- **AEC ERLE with host-played music** (Apple Music continuous): median
  **26.6 dB**, range 21.8–29.5 dB across 6 samples (bar ≥ 20 dB; SP#5
  1 kHz reference was 23.2 dB — diverse program material gives the AEC
  more spectral lock-in). The "remote party doesn't hear themselves"
  gate from spec §2.10 holds with comfortable margin.
- **60 s sustained stream:** 0 drops total once the host is actively
  draining (host-disconnect lag was 58k frames pre-attach, drops to 0
  while the Mac consumes).

### Known minor

- `scripts/test-uac2-routing-host.sh`'s `system_profiler` grep for
  `UAC2_Gadget` doesn't match — the Mac may report the device under a
  different name (e.g. `Insta360 Wave`). Cosmetic only; the actual
  acceptance signal is what `/tmp/audio_stats.json` reports on the
  device side.

## Uplink Level and CPU (device pass, 2026-10-06)

Calls sounded "very far" and choppy. Measured on the device, with stage RMS
from `/tmp/audio_stats.json` while speaking ~1 m away:

- **Level.** The beam carries conversational speech at about −55 to −65 dBFS.
  DeepFilterNet treats speech that quiet as noise. Offline, on a `say` sentence
  plus a 1.7 LSB noise floor, it removed 10.5 dB at −60 dBFS (NR Low, 12 dB cap)
  and 20 dB at High, but nothing at −40 dBFS or louder. The user mic level was
  applied after the denoiser, so the uplink sat near −58 dBFS. Fix:
  `aec.uplink_makeup_gain_db` (`crate::makeup`, soft knee above half scale)
  between residual-echo suppression and noise suppression. At +24 dB the
  denoiser lost ≈ 2.7 dB; after the echo canceller fix below the default is
  +18 dB (speech ≈ −26 dBFS into the denoiser and, at mic level 100 %, out).
- **CPU.** DeepFilterNet skips its network on quiet input and runs it fully on
  speech. During speech the audio worker rose from ≈ 11 % to ≈ 84 % of one
  core (NR Off: ≈ 17 %). The whole DSP chain runs in series on that worker, so
  21 ms frames finish late and the UAC2 uplink underruns: 183 underruns in 20 s
  of streamed speech. NR Off gave none.
- **Uplink cushion.** `uac::upstream` now writes `cushion::PREFILL_PERIODS` (2)
  periods of silence whenever the stream (re)starts. That cut the underruns
  from 183 to 78 under the same load. It does not cover a frame that arrives
  more than ≈ 43 ms late.
- **Drop bursts of 78–140 frames** follow the end of a host stream: `writei`
  blocks until the control poll clears `upstream_active` (≈ 1.6 s). They are
  inaudible.
- **Echo canceller at 8 kHz.** The linear AEC cut near-end speech by 11–17 dB
  with a silent reference. `speex_echo_state_init` assumes 8 kHz until
  `SPEEX_ECHO_SET_SAMPLING_RATE`, and its DC notch (radius 0.9 at "8 kHz")
  high-passed the 48 kHz signal near 760 Hz, which is most of a voice. Setting
  the rate brought the loss to ≈ 0.7 dB. ERLE figures measured before this fix
  (SP#5/SP#6) were taken with that filter in place.
- Noise reduction is **Off** on the device for now: DeepFilterNet on the CPU
  can't keep up during speech. Running it on the NPU is deferred (TODO).
- **Stereo alignment (C15).** Stereo adds the raw beam side to the processed
  mid, which lags the beam. Measured on the host through
  `AudioService::process_frame` (`mid_latency_matches_an_impulse_through_the_service`):
  the Speex residual stage shifts by one analysis frame (128 samples at the
  1024 period, 240 at 480; the linear canceller adds none); DeepFilterNet adds
  480 (its model) plus the zeros its hop queue inserted while filling (448 at
  1024, none at 480), so 1056 samples (22 ms) with NR on at 1024. The fallback
  standalone Speex denoiser shifts by a whole period. `stereo_align::SideDelay`
  holds the raw pair back by `mid_latency_samples()`; side and mid now line up
  within 4 samples with NR Off and Low (they were 128 / 1056 apart). The
  recording limiter's 96-sample lookahead comes after the rebuild and delays
  mono and pair alike.

## Uplink Voice Tone (device pass, 2026-10-07)

After the level fix, calls sounded "roomy" and "very bassy, not clear on
speech". Measured offline on a 32 s raw 8-channel recording of the user
talking in their room (`arecord -D hw:0,2 -c 8 -r 48000 -f S32_LE`, waved
stopped), replayed through `AudioService::process_frame`:

- **The voice is bass-heavy at the mics.** The fundamental (105–117 Hz) sits
  about 16 dB over 400–630 Hz. All 8 mics agree within ±2 dB, and the codec's
  PDM filter modes 1–4 measure identical on a speaker white-noise stimulus
  (mode 0 is worse at HF), so neither a dull mic nor the codec is the cause.
- **Stock firmware EQs its mic path.** `/etc/dynamic_eq_config.json` (read by
  libInstaAudio; `Insta360_SetUplinkEqEnable`): type 4 at 150 Hz Q 0.7
  (high-pass), type 0 at 90 Hz −6 dB Q 2.0 (peaking), type 2 at 8 kHz +5 dB
  Q 0.7 (high shelf); mbagc off. waved had no mic EQ.
- **The Speex residual stage cuts treble with no far end.** Its MMSE gain
  treats the quiet treble of distant speech as noise: −3 dB at 2.5–5 kHz,
  −10 dB at 5–8 kHz, −7 dB at 8–12 kHz with a silent reference. The stage now
  passes the linear output through (delayed by its analysis frame) unless the
  far end played in the last 500 ms or Speex is the active denoiser.
- **Whole room's talker alignment lost treble.** Aligning the eight mics on
  the tracked talker came out 5–6 dB under the plain average above 2.5 kHz,
  at every elevation tried (−40° to +40°). The inter-mic lags measured by
  GCC-PHAT on the recording do not fit the geometry: the best plane-wave fit
  (az 224°, el −58°) misses by 3.3 samples RMS, and ALSA ch2 → ch6 is
  17 samples where no mic pair is more than 13 apart. Delay-and-sum on the
  measured lags also loses to the plain average. Whole room now stays on the
  plain average. The calibration that followed (see "Array calibration"
  below) found the hardware sound: the impossible lags came from GCC-PHAT on
  a recording where the direction estimate itself had little to hold on to.
- **Picked by ear** from level-matched clips: dereverb + a voice EQ
  (`crate::dereverb`, `crate::uplink_voice`) after echo control, before the
  make-up gain. Dereverb: Lebart late-reverb suppression, T60 0.6 s, 50 ms
  delay, −15 dB floor, 1024/256 STFT (768 samples, 16 ms; the 1536/384
  prototype measured within 0.15 dB per octave band). EQ: 4th-order
  Butterworth high-pass at 160 Hz, low shelf 300 Hz −5 dB, peak 3 kHz +5 dB
  Q 1, high shelves 2 kHz +7 dB and 8 kHz +5 dB, then +5 dB to restore the
  level the bass cut removes. The Stereo side pair gets the same EQ.
  `aec.uplink_voice: false` turns the stage off.

Effect on the recording (octave bands, firmware chain vs before, level
matched): 80–160 Hz about −13 dB, 2.5–5 kHz +17 dB, 5–8 kHz +19 dB,
8–12 kHz +27 dB. The replayed firmware chain matches the chosen prototype
within 0.5 dB from 160 Hz to 12 kHz.

## Array calibration (device pass, 2026-10-08)

Raw `hw:0,2` captures with waved stopped; analysis scripts were scratch.

**Hardware.** The top-ring holes face **up** (top surface); the bottom-ring
holes face **out** (side wall). Ruler: 77 mm from a top hole to the hole below
it (the table above uses 78.2 mm; 1.2 mm is 0.17 samples, not worth a change).

| Test | Result |
|------|--------|
| Pen clicks 1 cm from each hole | Earliest channel per hole matches `alsa_to_physical` for all eight |
| 10 s silence | Every channel within ±1.8 dB of the median in every band (20 Hz–16 kHz) |
| 32 s room speech | Within ±2.5 dB in every band |
| Broadband noise, 1 m, front/left/back/right | GCC-PHAT arrivals match the free-field model within ~1 sample, including ±2.5-sample in-ring delays; top and bottom ring arrive together for a level source (bottom − top +0.1 to +1.3 samples) |

The near-field clicks fit a bottom-ring lag of +2.8 samples (90 % CI 2.0–3.8),
but the far-field noise shows ≤ 1 sample, and the far field is what the beam
and DoA see: **no per-channel timing correction is applied.**

**Direction estimate.** On the noise, `doa` read front 351°, left 88°, back 175°,
right 255° (errors −9/−2/−5/−15°, IQR ≤ 7°, confidence 1.0), and 346°/+49° for
a source above and in front. On counted speech, seated at 1 m (−59/−54 dBFS
at the mics), it produced a direction for only 15 of 123 VAD frames from the
front and 100 of 201 from the left, with an 89° spread. This is why the wedge
rarely shows: the estimator is accurate when it commits but rejects most
speech frames. **Open.**

**Whole room alignment.** On the noise, the delay-and-sum on the true
direction beats the plain average by +3 dB at 2.5–8 kHz (measured delays add
another 0.5–1.5 dB). On the seated speech the gain is 0 dB below 5 kHz and
+2 dB at 5–8 kHz, while steering at a wrong direction costs 3–6 dB. With the
estimate as unreliable as above on speech, Whole room stays on the plain
average.

## Microphone check (milestone 2)

`waved-audio/src/mic_check.rs` watches raw, pre-mute frames in 3 s windows at start-up.
A channel that is exactly zero counts as not responding; levels are judged against the
median of all channels. The result is `AudioState.mic_health`, and the UI shows
"Microphones not responding" for `NotResponding`. On the device it reports `Ok` about
3 s after boot (2026-10-06).

From 2026-10-06 most boots flagged a subset of the odd ALSA channels (ch1, 3, 5, 7) as
"much quieter" and the rest read `Ok`. The calibration above shows the elements matched
in silence and on speech, so the flag reflects what the array hears in its first seconds
(motor, handling), not a weak mic. The 2026-10-08 boot read `Ok`.

## Mic Gain Pipeline

Two stages, set independently:

1. **Hardware preamp — PDM Gain** (`amixer -c 0 ... numid=9`, range 0–48).
   Set once at daemon startup from `audio_config.capture.pdm_gain` (default
   `24`, ~+12 dB lift). Static across the run so DFN3's noise-floor
   estimator doesn't re-adapt every time the user moves the volume knob.

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

Boot order:
```
TOML  → WavedConfig.audio.mic_volume → AudioState.mic_volume   (seeded in main)
JSON  → CaptureConfig.pdm_gain        → ALSA numid=9            (one-shot)
```

The UAC2 mixer (`hw:1 PCM Capture Volume`) maps its raw range to our
`0..=800` so the host's "max slider" corresponds to internal `mic_volume=800`
(+18 dB), and the boot default of `200` sits at 25 % of the host's range.

If recordings come back too quiet or too clipped, **change `pdm_gain`
in the JSON config and reboot — do not change it dynamically.** Dynamic
changes were the cause of the DFN3 over-gating regression observed during
SP#5 testing.

Acceptance script: `scripts/test-mic-volume.sh`.

## 4-Point DOA Validation Protocol

With speaker at top-ring level, 1 m out:

| Source az | Accepted |
|-----------|----------|
| 0° (front)  | error ≤ 10° |
| 90° (left)  | error ≤ 10° |
| 180° (back) | error ≤ 15° (back pairs noisier) |
| 270° (right)| error ≤ 10° |

Measured DOA is wrapped into [0, 360) before comparison. Mean offset
across the 4 points should be close to zero; a consistent non-zero offset
means either the ALSA permutation is wrong or the geometry rotation is
off by some multiple of 45°.

## Audio-Mode DOA Preview (UI)

The Home/Recording dial wedge and the Audio mode picker show the direction of
arrival; the picker's aim drag sets `beam_aim_deg`. Plumbing:
`DoaUpdate.vad_active` (audio thread) → `AudioState.doa_active` (state bus)
→ `AudioStatsSnapshot.doa_active` (stats JSON) and the UI snapshot.

### Direction convention (measured on the device, 2026-10-06)
`doa_horizontal` and `beam_aim_deg` are **device degrees**: 0° in front of the
screen, counter-clockwise seen from above, 90° on the listener's left. Speaking
~1 m away with the screen facing the listener read: front ≈ 355°, left ≈ 92°,
behind ≈ 183°, right ≈ 260° (live from `/tmp/audio_stats.json`).

The dial is a top-down map seen by the listener: front at the bottom, their left
on the left. Screen degrees (0° up, clockwise) = device degrees + 180°
(`waved-ui/src/kit/direction.rs`). Drawing device degrees directly (the first
M2 build) showed a front-left talker at the back-right.

The legacy carousel UI (`screens/audio_mode.rs`, `bf_doa.png`) and its
hysteresis were deleted with the runtime switch; the notes below on the VAD
threshold still apply.

### VAD threshold + UI hysteresis
EnergyVad default is **0.0003** (~−70 dBFS). It is permissive on purpose —
it catches quiet music and conversational speech but also room noise.
Per-frame thresholding alone cannot cleanly separate the two on this device:
silence-floor and phone-music both land in the 0.0003–0.0005 band on a
quiet desk.

The legacy UI compensated with temporal hysteresis (3 `true` ticks to engage,
2 `false` to release). The M2 screens don't yet; on the device pass the gate
read open in almost every 0.5 s sample of a quiet office, so the wedge follows
room noise between talkers.

### Measured mic-level reference (one device, quiet desk)
| Source                              | Mic ch0 RMS (int16) | float RMS  |
|-------------------------------------|---------------------|------------|
| Silence (typical)                   | ~9                  | ~0.00027   |
| Silence (occasional noise burst)    | ~40                 | ~0.0012    |
| Phone music ~30 cm away             | ~13                 | ~0.0004    |
| Voice (loud, ~30 cm)                | trips at 0.003      | ≥ 0.003    |

These are device- and room-dependent — values above are an order-of-magnitude
reference for picking thresholds, not a calibration target.

## Recording bit depth — why there is no "HD" (24-bit) tier

Investigated 2026-05-27 after the stock 1.2.0.6 diff surfaced an "HD recording
mode". Conclusion: **meeting recordings are 16-bit lossless WAV and that is the
right ceiling on this pipeline.** Evidence:

- **The PDM ADC is genuine ~24-bit.** A raw `arecord -D hw:0,2 -f S32_LE -c 8`
  capture reaches **bit 27** in magnitude, the low byte is populated **99.6 %**
  of samples, and levels exceed 16-bit full-scale by ~6380×. The hardware has
  the resolution.
- **The firmware throws it away at ingest.** The capture layer downconverts
  S32 → i16 by keeping only the top 16 bits — `(s >> 16) as i16` in
  `capture/single_stream.rs`, `capture/alsa_dual_stream.rs`,
  `capture/mic_software_ref.rs`. `AudioFrame.data` is `Vec<i16>`, so everything
  downstream (beamform, the **i16-only speexdsp AEC**, the recorder) is 16-bit.
  A second hard quantization is `lib.rs` `(src * 32768.0) as i16` feeding the AEC.
- **Stock made the identical choice.** In `libInstaAudio.so` the algorithm
  bases carry `mInBitWidth = 0x20` (32-bit in) and `mOutBitWidth = 0x10`
  (16-bit out); the record path is an `Insta360_Mp3_FileOutDevice` at
  `mBitWidth = 0x10` driving a **LAME MP3** encoder. Stock's "HD"
  (`is_high_rate_encode_en`) is a higher **MP3 bitrate**, not more PCM bits —
  stock never carries 24-bit to a recording either.

So our **Standard 16-bit lossless WAV already beats stock's HD lossy MP3** on
fidelity. A 24-bit WAV tier was prototyped and dropped: on this i16 pipeline the
low 8 bits are always zero (verified — the container is 24-bit but the content
is promoted 16-bit), so it adds ~50 % file size with no resolution. Real 24-bit
would require an end-to-end f32 pipeline including an f32-domain AEC (speexdsp is
i16-only) — a larger effort with **no stock precedent**, deferred.

**Proper check for any future HD work:** don't trust the WAV header alone —
read the 24-bit samples and confirm the **low byte is populated** (`!= 0` for a
large fraction). All-zero low bytes mean promoted 16-bit, not genuine 24-bit.

## Field-Testing Gotcha: init.d Watchdog

`/etc/init.d/S60waved` respawns `waved` without environment variables.
Manual env-var injection (e.g. `WAVED_DOA_DEBUG=100`) requires killing
the watchdog first:

```bash
adb shell "kill -9 \$(pidof S60waved 2>/dev/null || echo 330); \
           killall waved 2>/dev/null; sleep 1; \
           (WAVED_DOA_DEBUG=100 setsid /data/waved/waved \
              >/tmp/waved.log 2>&1 </dev/null &)"
```

Without the watchdog kill, the env var gets stripped on the next respawn.

When starting the init script manually over ADB, let BusyBox detach it before
the ADB session exits:

```bash
adb shell "start-stop-daemon -S -b -x /bin/sh -- /etc/init.d/S60waved start"
```

Launching `S60waved start` as an ordinary shell background job can leave the
first `waved` process alive while its restart-loop subshell is reaped as the ADB
session closes. `start-stop-daemon -b` was verified on-device to retain exactly
one `/bin/sh /etc/init.d/S60waved start` watchdog and one `waved` process across
the scripted five-second display restart.

## Deploy

```bash
cross build --release --target aarch64-unknown-linux-gnu \
    --features "ui,capture-alsa" -p waved
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "killall waved 2>/dev/null; sleep 2; \
           (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &)"
```
