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
   ↓ GCC-PHAT for 4 pairs: (0,2), (1,3), (0,6), (1,7)
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

## Diagnostic: `WAVED_DOA_DEBUG`

Set at startup to log raw-vs-expected τ per pair every N frames:

```
WAVED_DOA_DEBUG=100 waved
```

Unset or `=0` → off (production default). Log target
`waved_audio::doa::debug`, info level. Example line:

```
doa_debug az=178.2 el=3.1 rms0=0.00012 | (0,2)raw=-0.03 exp=-0.05  (1,3)raw=+0.44 exp=+0.42  (0,6)raw=-1.21 exp=-1.30  (1,7)raw=-0.18 exp=-0.25
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
  with `ERLE_THRESHOLD_DB=20.7`.
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

## Deploy

```bash
cross build --release --target aarch64-unknown-linux-gnu \
    --features "ui,capture-alsa" -p waved
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "killall waved 2>/dev/null; sleep 2; \
           (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &)"
```
