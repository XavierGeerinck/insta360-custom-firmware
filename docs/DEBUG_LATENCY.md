# Insta360 Wave — Audio Latency Bug Analysis

**Symptom:** In multi-person meetings, after ~60 seconds of silence, it takes ~20 seconds of talking before your audio comes through to remote participants.

**Root cause:** Multiple stages in the Roomix TEA SDK audio pipeline independently adapt to classify the silent speaker as "noise/echo", and each stage takes seconds to re-converge when speech resumes. The stages compound to ~20 seconds of effective muting.

---

## Audio Pipeline Architecture

The Insta360 Wave uses an 8-stage audio processing pipeline from the **Roomix TEA SDK** (v3.10.1.1153). The full pipeline configuration is embedded in `libTEAProcessSdk.so` as a JSON blob at binary offset `0x375688`.

Two pipeline modes exist:
- **`preprocess_pipe_meeting`** (mode 0) — Used in "Noise Reduction" mode. This is the default for conference calls.
- **`preprocess_pipe_live`** (mode 1) — Used in beamforming modes (Omni, Cardioid, Super-cardioid, Figure-8, Stereo).

Both share the same fundamental issue.

### Pipeline Flow (Meeting Mode)

```
8 MEMS Mics (48kHz/32bit/10ch)
        │
        ▼
┌─────────────────────────────────────────────────────┐
│  Stage 1: AEC (Thread 1, Core 1)                    │
│  Kalman filter (500ms) + NLMS (180ms)               │
│  NNEC mode 3 (neural echo cancellation, aggressive) │
│  ec_aggr: 1.5                                       │
│  ← PROBLEM: Adapts to classify you as echo path     │
├─────────────────────────────────────────────────────┤
│  Stage 7: WPE (Thread 7, Core 2)                    │
│  Dereverberation (FFT 1024)                         │
├─────────────────────────────────────────────────────┤
│  Stage 2: EXTRACT (Thread 2, Core 2)                │
│  Neural pre-mask (premask_reverb_best_model.bin)     │
│  array_gain: 17.0, nn_aggr: 1.0                    │
├─────────────────────────────────────────────────────┤
│  Stage 3: ARRAY_SE (Thread 3, Core 3)               │
│  MVDR beamforming (5 iterations)                    │
│  NNEC mode 3, ns_aggr: 0.0                         │
│  ← PROBLEM: Beamformer steers away from you         │
├─────────────────────────────────────────────────────┤
│  Stage 4: NNPOST (Thread 4, Core 2)                 │
│  RAFA neural noise suppression                      │
│  agc_attack: 0.5, agc_release: 0.5, agc_alpha: 0.1 │
│  ← PROBLEM: Slow AGC convergence after silence      │
├─────────────────────────────────────────────────────┤
│  Stage 8: PEQ (Thread 8, Core 3)                    │
│  8-band parametric EQ                               │
├─────────────────────────────────────────────────────┤
│  Stage 5: MIXER_AGC (Thread 5, Core 3)              │
│  Automatic Mic Mixing (AMM)                         │
│  agc_max_gain: 11.5, agc_min_gain: -1.5            │
│  ← PROBLEM: AMM attenuates silent channels          │
├─────────────────────────────────────────────────────┤
│  Stage 6: DOA (Thread 6, Core 3)                    │
│  Direction of Arrival estimation                    │
└─────────────────────────────────────────────────────┘
        │
        ▼
  3A Output (48kHz/16bit/2ch) → USB/BT/Speaker
```

---

## Culprit 1: AEC with Aggressive NNEC (Primary)

### Configuration

```json
{
  "name": "AEC",
  "params": {
    "aec": 1,
    "kalman": 1,
    "aec_nlms_filter_len_ms": 180,
    "aec_kalman_filter_len_ms": 500,
    "nnec": 3,
    "ec_aggr": 1.5,
    "rwpe_enable": 0
  }
}
```

### What Happens

The AEC (Acoustic Echo Canceller) uses a dual-filter architecture:
- **Kalman filter** with a 500ms window — models the room impulse response
- **NLMS filter** with a 180ms window — adaptive echo subtraction
- **NNEC mode 3** — the most aggressive neural echo cancellation mode

When others are talking through the speaker and you're silent for ~60 seconds:
1. The Kalman filter continuously adapts its model of the acoustic path
2. After 60s, it has thoroughly learned the room's echo characteristics
3. Your microphone position's signal is modeled as part of the echo path
4. **`ec_aggr: 1.5`** (aggressiveness > 1.0) means the AEC over-cancels — it subtracts more than just the estimated echo

When you start talking:
- The AEC's double-talk detector (DTD) needs to recognize that near-end speech is present
- With NNEC=3, the neural model is heavily biased toward echo suppression
- **Estimated recovery: 2-5 seconds** for the AEC to stop cancelling your voice

### Why ec_aggr: 1.5 Is Too High

An `ec_aggr` of 1.0 means cancel exactly the estimated echo. At 1.5, the AEC cancels **150% of the estimated echo** — intentionally over-suppressing to handle non-linear echo components. This trades voice quality for echo suppression, and in your scenario, it means your returning voice gets significantly attenuated while the AEC re-adapts.

---

## Culprit 2: NNPOST / RAFA Neural Noise Suppression (Major)

### Configuration

```json
{
  "name": "NNPOST",
  "params": {
    "nn_model_file": "rafa-best_model-11000_model.bin",
    "nnpost_aggr": 1.0,
    "mask_power_factor": 0.75,
    "agc_gain_apply": 1,
    "agc_target_level": 0.8,
    "agc_max_gain_in_dB": 18.0,
    "agc_threshold": 0.001,
    "agc_attack": 0.5,
    "agc_release": 0.5,
    "agc_alpha": 0.1,
    "feature_agc_enable": 1
  }
}
```

### What Happens

The RAFA neural network creates a time-frequency mask that is applied to the signal. The mask determines how much of each frequency bin to pass through (1.0 = pass, 0.0 = suppress).

During your 60 seconds of silence:
1. The RAFA model learns your frequency profile as "noise" (since it's just ambient/room tone from your position)
2. The spectral mask converges to heavily suppress frequencies matching your voice characteristics
3. The integrated AGC drops gain because there's no speech energy from your direction

When you start talking:
- **`agc_alpha: 0.1`** — This is the envelope smoothing factor. At 0.1, each frame only updates the gain estimate by 10%. To move from a "silence" gain to a "speech" gain requires dozens of frames
- **`agc_attack: 0.5`** — Attack time of 0.5 seconds. The AGC takes 0.5s per "step" to increase gain
- To ramp from `agc_threshold: 0.001` back to `agc_target_level: 0.8`, with alpha=0.1 and attack=0.5s, the math works out to:

```
Gain convergence time ≈ -ln(threshold/target) / (alpha × frame_rate)
                      ≈ -ln(0.001/0.8) / (0.1 × 50)
                      ≈ 6.68 / 5
                      ≈ 1.3 seconds (theoretical minimum)
```

But in practice, the neural mask also needs to re-adapt, and the `mask_power_factor: 0.75` applies a power curve that further slows the opening. **Estimated recovery: 5-10 seconds.**

---

## Culprit 3: MIXER_AGC with Automatic Mic Mixing (Compounding)

### Configuration

```json
{
  "name": "MIXER_AGC",
  "params": {
    "agc": 1,
    "amm": 1,
    "amm_fake": 0,
    "agc_max_gain": 11.5,
    "agc_min_gain": -1.5,
    "agc_post_gain": 0.0
  }
}
```

### What Happens

AMM (Automatic Mic Mixing) dynamically distributes gain across microphone channels based on speech activity. This is the final stage before output.

During your silence:
1. AMM reduces your channel's contribution to the mix
2. Active speakers get more gain allocation
3. Your channel drops to `agc_min_gain: -1.5 dB` (or effectively muted in the mix)

When you start talking:
- AMM needs to detect speech energy on your channel (already reduced by upstream AEC + NNPOST)
- It must ramp from -1.5 dB to up to +11.5 dB — a range of 13 dB
- This ramp happens only after the upstream stages have started passing your audio through
- **Estimated recovery: 5-10 seconds** (on top of upstream delays)

---

## Culprit 4: ARRAY_SE Beamforming (Contributing)

### Configuration

```json
{
  "name": "ARRAY_SE",
  "params": {
    "mvdr_switch": 1,
    "mvdr_iters": 5,
    "ns_aggr": 0.0,
    "nnec": 3,
    "initial_echo_clip": 1
  }
}
```

### What Happens

MVDR (Minimum Variance Distortionless Response) beamforming uses 5 iterations per frame to compute optimal spatial filtering. The beamformer steers toward active sound sources.

During your silence:
- The beamformer's spatial covariance matrix is dominated by other speakers
- Your direction gets classified as a noise/interference direction
- The MVDR steering vector points away from you

When you start talking:
- The covariance matrix needs multiple frames to incorporate your direction
- With 5 iterations per 20ms frame, and a 6-mic array, re-steering takes ~10-20 frames
- **Estimated recovery: 0.5-2 seconds** (relatively fast, but adds to the total)

---

## Combined Timeline

```
T=0:   You start talking after 60s of silence

T=0-2s:   ┌─ AEC: Still cancelling you as "echo" (nnec=3, ec_aggr=1.5)
           │  Your voice is subtracted from the signal.
           │  DTD slowly detecting near-end speech.
           └─ Beamformer: Starting to re-steer (MVDR covariance updating)

T=2-5s:   ┌─ AEC: DTD has detected double-talk, reducing echo subtraction
           │  But Kalman filter still partially adapted to your path.
           ├─ NNPOST/RAFA: Neural mask starting to open
           │  agc_alpha=0.1 means slow gain ramp
           └─ Your voice is partially coming through, very quiet

T=5-10s:  ┌─ AEC: Mostly recovered. Echo path re-estimated.
           ├─ NNPOST: Mask ~50% open. AGC ramping (attack=0.5s per step)
           └─ MIXER_AGC/AMM: Detecting speech energy, starting to increase
              your channel gain. But input is still quiet from upstream.

T=10-15s: ┌─ NNPOST: Mask ~80% open. AGC near target.
           └─ AMM: Your channel gaining weight in the mix.
              Voice audible but still quieter than others.

T=15-20s: ┌─ All stages fully converged.
           └─ Your voice at normal volume.

Total effective muting: ~15-20 seconds
```

---

## Evidence from the Configuration

### Meeting Pipeline vs Live Pipeline Comparison

| Parameter | Meeting Pipe | Live Pipe | Impact |
|---|---|---|---|
| `ec_aggr` | **1.5** | **2.0** | Live is even more aggressive! |
| `nnec` (AEC) | 3 | 3 | Same — max aggressiveness |
| `nnec` (ARRAY_SE) | 3 | **0** | Live disables neural echo in array stage |
| `ns_aggr` (ARRAY_SE) | **0.0** | **1.0** | Live has full noise suppression |
| `agc_gain_apply` (NNPOST) | **1** (applied) | **0** (not applied) | Meeting pipe applies AGC in NNPOST |
| `agc_max_gain` (MIXER) | 11.5 | **15.0** | Live allows higher gain recovery |
| `use_az_mask` (NNPOST) | 0 | **1** | Live uses azimuth mask |
| `disable_NNMASK` (NNPOST) | not set | **1** | Live disables neural mask! |
| `process_pcm_dB_gain` | 13.0 | **18.0** | Live has 5dB more input gain |

**Key observation:** The live pipeline (`preprocess_pipe_live`) has `"disable_NNMASK": 1` and `"agc_gain_apply": 0` in NNPOST, meaning it **skips the neural noise mask and NNPOST AGC entirely**. This would significantly reduce the latency issue. The meeting pipeline applies both.

---

## Recommended Fixes

### For Insta360 (firmware update)

1. **Reduce `ec_aggr`** from 1.5 to **0.8-1.0**
   - Reason: 1.5 over-suppresses near-end speech during double-talk
   - Risk: Slightly more residual echo, but much faster voice recovery

2. **Increase `agc_alpha`** from 0.1 to **0.3-0.5**
   - Reason: Faster envelope tracking means faster gain recovery
   - Risk: More gain pumping on short pauses (acceptable trade-off)

3. **Decrease `agc_attack`** from 0.5s to **0.1-0.2s**
   - Reason: Faster ramp-up when speech resumes
   - Risk: Slightly more breathing noise amplification

4. **Add proper DTD (Double-Talk Detector)** to the AEC
   - The current pipeline relies on the neural model (NNEC) to handle double-talk
   - A dedicated DTD would freeze AEC adaptation during silence near active speakers
   - This prevents the Kalman filter from adapting to classify the near-end as echo

5. **Increase `agc_min_gain`** in MIXER_AGC from -1.5 to **2.0-4.0**
   - Reason: Don't fully attenuate silent channels; maintain a baseline
   - Risk: Slightly higher noise floor from inactive mics

6. **Set `cache_clear_policy: 1`** in NNPOST (currently 0 in meeting pipe)
   - The live pipe has `cache_clear_policy: 1` in EXTRACT
   - This clears the neural cache on mode changes, preventing stale noise estimates

### For Users (Workaround)

1. **Switch to a beamforming mode** (Omni, Cardioid, etc.) instead of "Noise Reduction"
   - Beamforming modes use `preprocess_pipe_live` which has `disable_NNMASK: 1`
   - This bypasses the RAFA neural mask and NNPOST AGC that cause most of the delay
   - Go to: **Settings → Audio Mode → Original → Omni**

2. **Use Omni mode specifically** for multi-person round-table meetings
   - Cardioid/Super-cardioid modes focus on one direction, which could cause similar issues for speakers at other positions
   - Omni (mode 2) picks up all directions equally

3. **Avoid extended silence** — if you know you'll speak after a pause, a brief "mhm" or any sound every 30s keeps the pipeline adapted to your voice

4. **If you have device access** (telnet/UART), you could modify the config:
   ```bash
   # The TEA SDK config is embedded in the binary, but the device
   # reads /etc/audio_config.json at startup. While this doesn't
   # control the TEA pipeline directly, switching aglo_type might help:

   # Check current mode
   cat /etc/audio_config.json
   # {"audio_config":{"aglo_type":1,"insta_eq_en":0,"insta_eq_pregain":1.0,"insta_reverb_en":0}}

   # aglo_type: 0=bypass, 1=tea, 2=sky
   # Bypass mode (0) would skip all processing — good for testing
   ```

---

## Full Pipeline Configuration (Extracted)

Extracted from `libTEAProcessSdk.so` at offset `0x375688` (12,078 bytes):

### Meeting Pipeline (`preprocess_pipe_meeting`, mode 0)

```json
{
  "node_name": "preprocess_pipe_meeting",
  "process_pipeline_mode": 0,
  "frame_duration_ms": 20,
  "input_sample_rate": 48000,
  "output_sample_rate": 48000,
  "process_sample_rate": 32000,
  "process_bit_depth": 32,
  "process_pcm_dB_gain": 13.0,
  "process_channel_num": 6,
  "common_fft_size": 1024,
  "common_hop_size": 640,

  "AEC": {
    "aec": 1, "kalman": 1,
    "aec_nlms_filter_len_ms": 180,
    "aec_kalman_filter_len_ms": 500,
    "nnec": 3, "ec_aggr": 1.5
  },
  "WPE": {
    "use_wpe": 1, "wpe_fft_size": 1024,
    "wpe_reverb_det": 0
  },
  "EXTRACT": {
    "array_gain": 17.0, "nn_aggr": 1.0,
    "cache_clear_policy": 1,
    "nn_model_file": "premask_reverb_best_model-220000_model.bin"
  },
  "ARRAY_SE": {
    "ns": 0, "mvdr_switch": 1, "mvdr_iters": 5,
    "nnec": 3, "ns_aggr": 0.0,
    "array_gain": 8.0, "initial_echo_clip": 1
  },
  "NNPOST": {
    "nn_model_file": "rafa-best_model-11000_model.bin",
    "nnpost_aggr": 1.0, "mask_power_factor": 0.75,
    "agc_gain_apply": 1, "agc_target_level": 0.8,
    "agc_max_gain_in_dB": 18.0, "agc_threshold": 0.001,
    "agc_attack": 0.5, "agc_release": 0.5, "agc_alpha": 0.1,
    "feature_agc_enable": 1
  },
  "PEQ": {
    "peq_enable": 1, "peq_sample_rate": 32000,
    "peq_filter_nums": 8
  },
  "MIXER_AGC": {
    "agc": 1, "amm": 1,
    "agc_max_gain": 11.5, "agc_min_gain": -1.5
  }
}
```

### Live Pipeline (`preprocess_pipe_live`, mode 1)

```json
{
  "node_name": "preprocess_pipe_live",
  "process_pipeline_mode": 1,
  "process_pcm_dB_gain": 18.0,

  "AEC": {
    "aec": 1, "kalman": 1,
    "aec_nlms_filter_len_ms": 180,
    "aec_kalman_filter_len_ms": 500,
    "nnec": 3, "ec_aggr": 2.0
  },
  "CUSTOM (Beamforming)": {
    "az_enable": 1, "aggr": 0.85,
    "models": "insta360_az_taer_{cardioid,supercardioid,figure8}_{low,high,vad}_rotate_model.bin"
  },
  "EXTRACT": {
    "array_gain": 17.0, "nn_aggr": 1.0,
    "nn_model_file": "insta360_psp_pre_chimera_model.bin"
  },
  "ARRAY_SE": {
    "ns": 1, "mvdr_switch": 1, "mvdr_iters": 5,
    "nnec": 0, "ns_aggr": 1.0
  },
  "NNPOST": {
    "nn_model_file": "rafa-best_model-11000_model.bin",
    "disable_NNMASK": 1,
    "agc_gain_apply": 0,
    "agc_attack": 0.5, "agc_release": 0.5, "agc_alpha": 0.1
  },
  "MIXER_AGC": {
    "agc": 1, "amm": 1,
    "agc_max_gain": 15.0, "agc_min_gain": -1.5
  }
}
```

---

## Key Difference: Why "Original/Beamforming" Mode May Work Better

| Feature | Meeting Pipe (Noise Reduction) | Live Pipe (Beamforming) |
|---|---|---|
| Neural noise mask (RAFA) | **Applied** (`disable_NNMASK` not set) | **Disabled** (`disable_NNMASK: 1`) |
| NNPOST AGC | **Applied** (`agc_gain_apply: 1`) | **Not applied** (`agc_gain_apply: 0`) |
| Input gain | 13.0 dB | **18.0 dB** (5 dB hotter) |
| Max recovery gain | 11.5 dB | **15.0 dB** |
| Neural echo in array | NNEC=3 (aggressive) | **NNEC=0** (disabled) |

The live pipeline **skips the two slowest-converging stages** (neural noise mask and NNPOST AGC), which is why switching to any beamforming mode (Omni recommended for meetings) should significantly reduce the 20-second delay.

---

*Analysis based on Ghidra decompilation of libInstaAudio.so (1,634 functions) and embedded pipeline configuration extracted from libTEAProcessSdk.so (offset 0x375688, 12,078 bytes).*
