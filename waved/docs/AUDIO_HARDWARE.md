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

### Mac → Wave control gaps (kernel `f_uac2` 5.4 limitation)

macOS's behaviour with the Wave's UAC2 gadget on this kernel:

| Mac action | What happens | Why |
|------------|--------------|-----|
| Plays audio (Music/Spotify) | Streams over UAC2 OUT terminal, hits `UacBridge` → DSP → speaker. Works. | data path is straightforward |
| Sound prefs Input slider | Dispatches UAC2 SET_CUR(VOLUME) to capture FU. We see `host set mic volume → N` in the log. Wave's mic_volume in `AudioState` updates. | macOS dispatches Input gain via UAC2 regardless of advertised range |
| F10 (mute key) | Dispatches UAC2 SET_CUR(MUTE) to **capture FU** (mic), not playback. macOS picks the headset semantic for any bidirectional UAC2 device. | confirmed in log: `host set mic mute → true/false` |
| F11 / F12 (volume keys) | **Nothing.** Mac doesn't dispatch UAC2 SET_CUR(VOLUME) for either FU, even with the volume range widened to ‑64 dB. | kernel 5.4 `f_uac2` likely doesn't expose a Master channel control in the playback FU's `bmaControls`; macOS USBAudio.kext keys on Master to enable HW Output volume. Without it, neither HW (UAC2) nor SW (host attenuation) gets engaged |
| Sound prefs Output slider drag | Same — slider is interactive but no UAC2 traffic and no audible level change. | same root cause |

The mute case is mitigated in `UacControl` by redirecting capture-FU mute
events onto `Command::ToggleSpeakerMute` by default (configurable via
`uac.host_mute_target` in `audio_config.json`: `"speaker"` (default,
music-listener UX), `"mic"` (headset UX, original behaviour), or
`"both"`). Mac F10 then silences the speaker — which is what most users
want from a music-output speakerphone.

The volume-key case has no clean device-side workaround on this kernel.
Practical paths in increasing engineering cost:

1. **Wave's touch UI for speaker volume** — works today via the SP#6
   follow-up that reads `speaker_volume` in the playback loop and scales
   the post-DSP buffer. Volume control just lives on the device, not
   the Mac.

2. **`scripts/wave-volume-mac.lua` (Hammerspoon shim).** Catches Mac
   F10/F11/F12 system media keys and translates each press into
   `adb shell amixer -c 1 cset numid={1,2} ...`. UacControl picks up
   the mixer change and the SP#6 gain code applies it. Per-Mac install
   (Hammerspoon + Accessibility permission), runs only when Wave is the
   active output device. See the script header for setup. This is the
   recommended workaround until/unless the kernel patch lands.

3. **Stock-style `uac_hid` pattern** — write Consumer-page HID INPUT
   reports (`VolumeUp` / `VolumeDown`) to `/dev/hidg0` from waved when
   the touch-UI volume changes. Mac receives a media-key event, scales
   its system Output volume host-side, and audio arrives pre-attenuated.
   This is the reverse direction (Wave→Mac) and only useful if you
   want Mac's slider to track Wave's touch-UI changes. Doesn't help
   Mac F11/F12. Not yet ported.

4. **Backport Master Volume control to `f_uac2`.** The right long-term
   answer: ~10–20 lines of C in `drivers/usb/gadget/function/f_uac2.c`
   to advertise a Master channel control in the playback FU's
   `bmaControls`, which is what macOS USBAudio.kext keys on. Requires
   the vendor's kernel source (likely available via the GPL but not
   yet sourced) and a kernel-module deploy path.

We tried disabling `p_volume_present` / `p_mute_present` /
`c_mute_present` to coax macOS into software-side attenuation. macOS
silently no-ops instead — it does not fall back to SW attenuation for
UAC2 devices the way it does for HDMI / built-in speakers. Don't
disable those flags (also kills the F10 mute redirect).

### `f_uac2` configfs gotcha — playback volume range

Stock `/etc/init.d/S89usbgadget` writes `p_volume_min=-100`,
`p_volume_max=0`, `p_volume_res=1` (units of 1/256 dB) — so the device
advertises only `~0.4 dB` of HW Output volume range. macOS treats that
as "not a real HW Output volume", silently disables hardware Output
volume control, and **does not** dispatch UAC2 SET_CUR for Mac volume /
mute keys. Mac volume/mute then has no effect on the speakerphone (Mac
also doesn't fall back to software-side attenuation for this case).

The capture (Input) side is unaffected — macOS dispatches UAC2 SET_CUR
for Input gain regardless of advertised range, since mic preamp gain is
conventionally HW-controlled.

**Fix:** advertise a useful playback range. `scripts/patch-uac2-volume-range.sh`
patches the init script to `p_volume_min=-16384` / `p_volume_res=256`
(= -64 dB to 0 dB in 1 dB steps), backs up the original to
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

`screens/audio_mode.rs` overlays `bf_doa.png` on the carousel-centered card
to show direction of arrival in real time, matching the original firmware's
"preview before selecting" behavior. Three pipeline facts to keep authoritative:

### State plumbing
`DoaUpdate.vad_active` (audio thread) → `AudioState.doa_active` (state bus)
→ `AudioStatsSnapshot.doa_active` (stats JSON) → UI hysteresis (next).
Set in `waved-audio/src/lib.rs` inside the `audio_tx.send_modify` block.

### Asset orientation
`bf_doa.png` (124×124 PNG under `/etc/pspapp/lvgl/icon/beamforming/`) draws
its "source dot" on the **LVGL-LEFT** edge with the wave arcs opening
rightward. This matters for any code that rotates the asset — see the
`az_to_lvgl_rotation` helper in `screens/audio_mode.rs` for the +270° offset
that puts the dot at LVGL-TOP for `az=0` (front of device).

LVGL coords = visual coords on this device (the FB flush 180° flip cancels
with the OLED's 180° physical mount — see `docs/ui/COORDINATE_SYSTEM.md`),
so the rotation formula is the natural identity mapping plus the asset offset:

```
rotation_lvgl = (az_deg.rem_euclid(360.0).round() * 10 + 2700) % 3600
```

### VAD threshold + UI hysteresis
EnergyVad default is **0.0003** (~−70 dBFS). It is permissive on purpose —
it catches quiet music and conversational speech but also room noise.
Per-frame thresholding alone cannot cleanly separate the two on this device:
silence-floor and phone-music both land in the 0.0003–0.0005 band on a
quiet desk.

The UI layer compensates with temporal hysteresis on `audio.doa_active`:
require 3 consecutive `true` ticks to engage, 2 consecutive `false` ticks
to release. `AudioModeScreen.{doa_show, doa_streak}` in
`screens/audio_mode.rs`. At ~50 ms UI ticks this gives ~150 ms engage
latency / ~100 ms release latency.

### Carousel-centered tracking
DOA arrow follows the **carousel-centered** card, not the selected mode —
this is what makes the "preview before tapping" UX work. A
`LV_EVENT_SCROLL_END` callback on the carousel maintains an
`Arc<AtomicUsize>` with the index of the child closest to the carousel's
viewport midline. On selection commit, `update_state` calls
`lv_obj_scroll_to_view(LV_ANIM_ON)` so centered tracks selection after
every tap, while still letting the user scroll-preview without committing.

### Measured mic-level reference (one device, quiet desk)
| Source                              | Mic ch0 RMS (int16) | float RMS  |
|-------------------------------------|---------------------|------------|
| Silence (typical)                   | ~9                  | ~0.00027   |
| Silence (occasional noise burst)    | ~40                 | ~0.0012    |
| Phone music ~30 cm away             | ~13                 | ~0.0004    |
| Voice (loud, ~30 cm)                | trips at 0.003      | ≥ 0.003    |

These are device- and room-dependent — values above are an order-of-magnitude
reference for picking thresholds, not a calibration target.

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
