# Insta360 Wave — Complete Audio Processing Pipeline

**Source:** Ghidra decompilation of `libInstaAudio.so` (1,634 functions), `libTEAProcessSdk.so` (8.5 MB), `libbva_audio.so` (1.9 MB), and embedded JSON config at binary offset `0x375688`.

**SDK:** Roomix TEA SDK v3.10.1.1153 (`dev/tea_process_x2_sdk_v7u_qoroliang.Roomix-dev`)

---

## Table of Contents

1. [High-Level Architecture](#1-high-level-architecture)
2. [Hardware Audio Path](#2-hardware-audio-path)
3. [Software Processing Chains](#3-software-processing-chains)
4. [Pipeline Mode 0: Meeting (Noise Reduction)](#4-pipeline-mode-0-meeting-noise-reduction)
5. [Pipeline Mode 1: Live (Beamforming)](#5-pipeline-mode-1-live-beamforming)
6. [Stage-by-Stage Deep Dive](#6-stage-by-stage-deep-dive)
7. [AGC Chain Analysis](#7-agc-chain-analysis)
8. [ASR Audio Path](#8-asr-audio-path)
9. [Audio Mode Switching](#9-audio-mode-switching)
10. [Dynamic EQ & Reverb Post-Processing](#10-dynamic-eq--reverb-post-processing)
11. [Debug & Dump Infrastructure](#11-debug--dump-infrastructure)
12. [Full Embedded Configuration](#12-full-embedded-configuration)

---

## 1. High-Level Architecture

```mermaid
graph TB
    subgraph Hardware
        MEMS["8 MEMS Microphones<br/>5m pickup range"]
        ADC["ADC<br/>48kHz / 32-bit / 10-ch"]
        TAS_A["TAS5707 Amp A"]
        TAS_B["TAS5707 Amp B"]
        LCD["1.82&quot; Touchscreen<br/>400×304"]
        NPU["Amlogic NPU<br/>VeriSilicon 350MHz"]
        USB_C["USB-C Port"]
        BT_MOD["BT Module<br/>SDIO"]
        DONGLE["2.4GHz Dongle"]
    end

    subgraph "Audio Input Sources"
        MIC_IN["Mic Array Input<br/>hw:0,2 / 48kHz/32bit/10ch"]
        BT_IN["BT SCO Input<br/>16kHz/16bit/1ch"]
        USB_IN["UAC 2.0 Input<br/>48kHz/16bit/2ch"]
        DONGLE_IN["Dongle Input<br/>48kHz/16bit/2ch"]
        FILE_IN["File Playback<br/>48kHz/16bit/2ch"]
    end

    subgraph "TEA SDK Processing"
        TEA["TEA Processing Pipeline<br/>Roomix SDK v3.10.1.1153<br/>32kHz / 20ms frames"]
    end

    subgraph "Audio Output Paths"
        OUT_3A["3A Output<br/>48kHz/16bit/2ch"]
        OUT_ASR["ASR Output<br/>32kHz/16bit/1ch"]
    end

    subgraph "Output Destinations"
        SPK["Speaker<br/>TAS5707 A+B"]
        USB_OUT["UAC 2.0 Output<br/>48kHz/16bit/2ch"]
        BT_OUT["BT SCO Output"]
        DONGLE_OUT["Dongle Output"]
        ASR_ENGINE["ASR Engine<br/>WeNet / SenseVoice"]
        RECORDER["Meeting Recorder<br/>Opus encoder"]
    end

    MEMS --> ADC --> MIC_IN
    MIC_IN --> TEA
    BT_IN --> TEA
    USB_IN --> TEA
    DONGLE_IN --> TEA
    FILE_IN --> TEA

    TEA --> OUT_3A
    TEA --> OUT_ASR

    OUT_3A --> SPK
    OUT_3A --> USB_OUT
    OUT_3A --> BT_OUT
    OUT_3A --> DONGLE_OUT

    OUT_ASR --> ASR_ENGINE
    OUT_ASR --> RECORDER

    NPU -.->|"CAM++ FP16<br/>Speaker Diarization"| ASR_ENGINE
    NPU -.->|"NNEC / RAFA<br/>Neural Models"| TEA
```

---

## 2. Hardware Audio Path

```mermaid
graph LR
    subgraph "Microphone Array (Physical Layout)"
        M0["Mic 0"]
        M1["Mic 1"]
        M2["Mic 2"]
        M3["Mic 3"]
        M4["Mic 4"]
        M5["Mic 5"]
        M6["Mic 6"]
        M7["Mic 7"]
        REF1["Ref Ch 8"]
        REF2["Ref Ch 9"]
    end

    subgraph "ALSA Devices"
        C0D0c["pcmC0D0c<br/>Capture"]
        C0D0p["pcmC0D0p<br/>Playback"]
        C0D1p["pcmC0D1p<br/>Playback"]
        C0D3c["pcmC0D3c<br/>Web/CGI capture"]
        C1D0c["pcmC1D0c<br/>UAC Capture"]
        C1D0p["pcmC1D0p<br/>UAC Playback"]
    end

    M0 & M1 & M2 & M3 & M4 & M5 & M6 & M7 & REF1 & REF2 --> C0D0c

    C0D0p --> TAS["TAS5707 A+B<br/>Speaker Output"]
    C1D0p --> USB["USB Audio Out"]
```

### ALSA Configuration (`/etc/asound.conf`)

| PCM Device | Direction | Sample Rate | Channels | Purpose |
|---|---|---|---|---|
| `hw:0,0` | Capture | 48kHz | 10 | Raw microphone array |
| `hw:0,0` | Playback | 48kHz | 8 | Speaker output (2→8 upmix) |
| `hw:0,1` | Playback | 48kHz | 2 | Default playback |
| `hw:0,2` | Capture | 48kHz | 2 | Processed mic input |
| `hw:0,3` | Capture | 48kHz | varies | CGI/Web recording |
| `hw:0,4` | Capture | 48kHz | 2 | AVS input |
| `hw:1,0,0` | Both | 48kHz | 2 | USB Audio Class 2.0 |

### Channel Mapping

From `/etc/channel_map_config_default.json`, the 10 input channels map to:
- **Channels 0-5**: 6 active microphone signals (used for beamforming)
- **Channel 6-7**: Additional microphones
- **Channel 8-9**: Reference channels (speaker loopback for AEC)

The TEA SDK processes **6 channels** (`process_channel_num: 6`) with `mic_selective_mode: 7`.

### Amplifier Configuration

Two TAS5707 digital amplifier chips:
- Volume range: 0-511 (stored in `/data/system_volume`)
- Default volume: 150/511
- Master volume from `/etc/EXT_AMP.ini`: 230/511
- ALSA control: `snd_ctl_elem_*` on card 1

---

## 3. Software Processing Chains

```mermaid
flowchart TB
    subgraph "Input Selection"
        direction LR
        USB_PATH["USB Path<br/>select_path bit 0"]
        BT_PATH["BT Path<br/>select_path bit 1"]
        DONGLE_PATH["Dongle Path<br/>select_path bit 2"]
        FILE_PATH["File Path<br/>select_path bit 3"]
    end

    subgraph "Channel Mapping"
        CHMAP["Insta360_MicChannelMapProccess<br/>10ch → 6ch + 2 ref"]
    end

    subgraph "TEA SDK Core"
        direction TB
        AEC_S["AEC<br/>Kalman + NLMS + NNEC"]
        WPE_S["WPE / CUSTOM<br/>Dereverberation / Beamforming"]
        EXTRACT_S["EXTRACT<br/>Neural Pre-mask"]
        ARRAY_S["ARRAY_SE<br/>MVDR Beamforming + NS"]
        NNPOST_S["NNPOST<br/>RAFA Neural Noise Suppression"]
        PEQ_S["PEQ<br/>8-band Parametric EQ"]
        MIXER_S["MIXER_AGC<br/>Automatic Mic Mixing"]
        DOA_S["DOA<br/>Direction of Arrival"]

        AEC_S --> WPE_S --> EXTRACT_S --> ARRAY_S --> NNPOST_S --> PEQ_S --> MIXER_S
        NNPOST_S --> DOA_S
    end

    subgraph "Post-Processing"
        EQ["Dynamic EQ<br/>Insta360_EqProccess<br/>/etc/dynamic_eq_config.json"]
        REVERB["Reverb Processing<br/>Insta360_ReverbProccess<br/>/etc/reverb_config.json"]
        UL_AGC["Uplink AGC<br/>aspMtagcProcessApiInt16"]
        DL_AGC["Downlink AGC<br/>aspMtagcProcessApiInt16"]
    end

    subgraph "Output Split"
        OUT_3A_S["3A Output<br/>Insta360_TeaSdk3AOut<br/>48kHz/16bit/2ch"]
        OUT_ASR_S["ASR Output<br/>Insta360_TeaSdkAsrOut<br/>32kHz/16bit/1ch"]
    end

    USB_PATH & BT_PATH & DONGLE_PATH & FILE_PATH --> CHMAP
    CHMAP --> AEC_S

    MIXER_S --> EQ --> REVERB --> UL_AGC --> OUT_3A_S
    MIXER_S --> OUT_ASR_S
    DL_AGC --> OUT_3A_S
```

---

## 4. Pipeline Mode 0: Meeting (Noise Reduction)

Active when Audio Mode = "Noise Reduction" (Mode 1 in `setWorkMode()`).

```mermaid
flowchart TD
    INPUT["10-ch Mic Input<br/>48kHz / 32-bit"]

    subgraph "Thread 1 / Core 1"
        AEC["🔇 AEC<br/>Kalman 500ms + NLMS 180ms<br/>NNEC=3, ec_aggr=1.5<br/>Removes speaker echo"]
    end

    subgraph "Thread 7 / Core 2"
        WPE["🏠 WPE<br/>Weighted Prediction Error<br/>FFT 1024, reverb_det=0<br/>Removes room reverb"]
    end

    subgraph "Thread 2 / Core 2"
        EXTRACT["🧠 EXTRACT<br/>Neural Pre-mask<br/>premask_reverb_best_model.bin<br/>array_gain=17dB, nn_aggr=1.0"]
    end

    subgraph "Thread 3 / Core 3"
        ARRAY["📡 ARRAY_SE<br/>MVDR Beamforming (5 iter)<br/>ns=0, nnec=3, ns_aggr=0.0<br/>Spatial filtering"]
    end

    subgraph "Thread 4 / Core 2"
        NNPOST["🤖 NNPOST<br/>RAFA Neural Noise Suppression<br/>rafa-best_model.bin<br/>agc_alpha=0.1, attack=0.5s<br/>mask_power=0.75"]
    end

    subgraph "Thread 8 / Core 3"
        PEQ["🎛️ PEQ<br/>8-band Parametric EQ<br/>32kHz processing"]
    end

    subgraph "Thread 5 / Core 3"
        MIXER["🎚️ MIXER_AGC<br/>Automatic Mic Mixing<br/>AGC: -1.5 to +11.5 dB"]
    end

    subgraph "Thread 6 / Core 2"
        DOA["🧭 DOA<br/>Direction of Arrival<br/>Uploaded via UART"]
    end

    INPUT --> AEC
    AEC --> WPE
    WPE --> EXTRACT
    AEC --> EXTRACT
    EXTRACT --> ARRAY
    ARRAY --> NNPOST
    NNPOST --> PEQ
    PEQ --> MIXER
    PEQ --> DOA
    MIXER --> OUTPUT_3A["3A Output → USB/BT/Speaker"]
    DOA --> OUTPUT_DOA["DOA angles → UI/PUC Camera"]

    style AEC fill:#ff6b6b,color:#fff
    style NNPOST fill:#ff6b6b,color:#fff
    style MIXER fill:#ffa502,color:#fff
    style ARRAY fill:#ffa502,color:#fff
```

### Meeting Pipeline Parameters

| Stage | Key Parameters | Threading |
|---|---|---|
| **AEC** | `aec=1, kalman=1, nnec=3, ec_aggr=1.5, kalman_len=500ms, nlms_len=180ms` | Thread 1, Core 1 |
| **WPE** | `use_wpe=1, wpe_fft_size=1024, wpe_reverb_det=0` | Thread 7, Core 2 |
| **EXTRACT** | `array_gain=17.0, nn_aggr=1.0, cache_clear_policy=1, model=premask_reverb_best` | Thread 2, Core 2 |
| **ARRAY_SE** | `ns=0, mvdr_switch=1, mvdr_iters=5, nnec=3, ns_aggr=0.0, array_gain=8.0` | Thread 3, Core 3 |
| **NNPOST** | `model=rafa-best, aggr=1.0, mask_power=0.75, agc_apply=1, agc_alpha=0.1, attack=0.5, release=0.5` | Thread 4, Core 2 |
| **PEQ** | `8 filters, 32kHz, dynamic_headroom=0` | Thread 8, Core 3 |
| **MIXER_AGC** | `agc=1, amm=1, max_gain=11.5, min_gain=-1.5` | Thread 5, Core 3 |
| **DOA** | `upload_with_uart=1, upload_with_teasdk=0` | Thread 6, Core 2 |

---

## 5. Pipeline Mode 1: Live (Beamforming)

Active when Audio Mode = "Original" with any beamforming pattern (Modes 2-6 in `setWorkMode()`).

```mermaid
flowchart TD
    INPUT["10-ch Mic Input<br/>48kHz / 32-bit<br/>+5dB more gain (18dB vs 13dB)"]

    subgraph "Thread 1 / Core 1"
        AEC2["🔇 AEC<br/>Kalman 500ms + NLMS 180ms<br/>NNEC=3, ec_aggr=2.0<br/>Even more aggressive echo cancel"]
    end

    subgraph "Thread 7 / Core 2"
        CUSTOM["🎯 CUSTOM (Beamforming)<br/>Neural TAER Models<br/>Cardioid / Super / Figure-8<br/>azimuth steering, aggr=0.85"]
    end

    subgraph "Thread 2 / Core 2"
        EXTRACT2["🧠 EXTRACT<br/>Neural Pre-mask<br/>insta360_psp_pre_chimera_model.bin<br/>array_gain=17dB"]
    end

    subgraph "Thread 3 / Core 3"
        ARRAY2["📡 ARRAY_SE<br/>MVDR Beamforming (5 iter)<br/>ns=1, nnec=0, ns_aggr=1.0<br/>Full noise suppression"]
    end

    subgraph "Thread 4 / Core 2"
        NNPOST2["🤖 NNPOST<br/>RAFA model loaded but:<br/>disable_NNMASK=1 ✅<br/>agc_gain_apply=0 ✅<br/>Neural mask BYPASSED"]
    end

    subgraph "Thread 5 / Core 3"
        MIXER2["🎚️ MIXER_AGC<br/>Automatic Mic Mixing<br/>AGC: -1.5 to +15.0 dB<br/>Higher max gain"]
    end

    subgraph "Thread 6 / Core 3"
        DOA2["🧭 DOA<br/>Direction of Arrival"]
    end

    INPUT --> AEC2
    AEC2 --> CUSTOM
    CUSTOM --> EXTRACT2
    EXTRACT2 --> ARRAY2
    ARRAY2 --> NNPOST2
    NNPOST2 --> MIXER2
    NNPOST2 --> DOA2
    MIXER2 --> OUTPUT2["3A Output → USB/BT/Speaker"]
    DOA2 --> OUTPUT_DOA2["DOA angles → UI/PUC Camera"]

    style AEC2 fill:#ff6b6b,color:#fff
    style NNPOST2 fill:#2ed573,color:#fff
    style MIXER2 fill:#ffa502,color:#fff
    style CUSTOM fill:#5352ed,color:#fff
```

### Live Pipeline Parameters

| Stage | Key Parameters | Difference from Meeting |
|---|---|---|
| **AEC** | `nnec=3, ec_aggr=2.0` | Even more aggressive (2.0 vs 1.5) |
| **CUSTOM** | `az_enable=1, aggr=0.85, 9 neural models` | Replaces WPE; directional beamforming |
| **EXTRACT** | `model=insta360_psp_pre_chimera_model.bin` | Different model (Chimera vs Reverb-premask) |
| **ARRAY_SE** | `ns=1, nnec=0, ns_aggr=1.0` | NS enabled, NNEC disabled |
| **NNPOST** | `disable_NNMASK=1, agc_gain_apply=0` | **Neural mask and AGC bypassed** |
| **MIXER_AGC** | `agc_max_gain=15.0` | Higher max gain (15 vs 11.5 dB) |
| Global | `process_pcm_dB_gain=18.0` | 5 dB more input gain (18 vs 13 dB) |

---

## 6. Stage-by-Stage Deep Dive

### 6.1 AEC (Acoustic Echo Cancellation)

```mermaid
flowchart LR
    subgraph "AEC Filter Architecture"
        MIC["Mic Signal<br/>x(n)"]
        REF["Speaker Reference<br/>d(n) from Ch 8-9"]

        KALMAN["Kalman Filter<br/>500ms impulse response<br/>Models room acoustics"]
        NLMS["NLMS Filter<br/>180ms impulse response<br/>Adaptive echo subtraction"]
        NNEC_M["NNEC Mode 3<br/>Neural Echo Cancellation<br/>nnec_two_stage_32k.bin"]

        SUB["Subtract<br/>echo estimate"]

        HPF["HPF<br/>(if hpf_policy=1)"]

        REF --> KALMAN
        REF --> NLMS
        MIC --> SUB
        KALMAN --> |"echo estimate"| SUB
        NLMS --> |"residual echo"| SUB
        SUB --> NNEC_M
        NNEC_M --> HPF
        HPF --> OUT_AEC["Clean signal<br/>(echo removed)"]
    end
```

**Parameters:**
```json
{
  "aec": 1,
  "keep_hf": 0,
  "gain": 0.0,
  "hpf_policy": 0,
  "kalman": 1,
  "aec_nlms_filter_len_ms": 180,
  "aec_kalman_filter_len_ms": 500,
  "nnec": 3,
  "ec_aggr": 1.5,
  "rwpe_enable": 0
}
```

| Parameter | Value | Meaning |
|---|---|---|
| `aec` | 1 | AEC enabled |
| `kalman` | 1 | Kalman filter enabled |
| `aec_kalman_filter_len_ms` | 500 | 500ms echo tail modeling |
| `aec_nlms_filter_len_ms` | 180 | 180ms adaptive filter |
| `nnec` | 3 | Neural echo cancellation, mode 3 (most aggressive) |
| `ec_aggr` | 1.5 (meeting) / 2.0 (live) | Over-cancellation factor (>1.0 = aggressive) |
| `keep_hf` | 0 | Don't preserve high frequencies through AEC |
| `hpf_policy` | 0 | No high-pass filter |
| `rwpe_enable` | 0 | Residual WPE disabled |

**Neural model:** `nnec_two_stage_32k.bin` — Two-stage neural echo cancellation at 32kHz.

---

### 6.2 WPE (Weighted Prediction Error) — Meeting Only

```mermaid
flowchart LR
    IN_WPE["AEC Output"] --> WPE_PROC["WPE Dereverberation<br/>FFT 1024<br/>Prediction filter orders:<br/>klg_minus_0=4<br/>klg_minus_1=4<br/>klg_minus_2=6<br/>klg_minus_3=8"]
    WPE_PROC --> OUT_WPE["Dereverberated Signal"]
```

**Parameters:**
```json
{
  "use_wpe": 1,
  "wpe_fft_size": 1024,
  "wpe_reverb_det": 0,
  "klg_minus_0": 4,
  "klg_minus_1": 4,
  "klg_minus_2": 6,
  "klg_minus_3": 8
}
```

The WPE filter removes late reverberation by predicting the reverberant component from past frames. The `klg_minus_*` parameters control prediction filter order per sub-band.

---

### 6.3 CUSTOM (Neural Beamforming) — Live Only

```mermaid
flowchart TD
    IN_CUSTOM["AEC Output<br/>6-ch"]

    subgraph "TAER Neural Beamforming"
        STEERING["Azimuth Steering<br/>vertical: 0°-180°"]

        subgraph "Model Selection by Pattern"
            CARD["Cardioid<br/>cardiod_{low,high,vad}_rotate"]
            SUPER["Supercardioid<br/>supercardiod_{low,high,vad}_rotate"]
            FIG8["Figure-8<br/>figure8_{low,high,vad}_rotate"]
        end

        SELECTOR["Mode Selector<br/>LiveEffectMode 0-4"]
    end

    IN_CUSTOM --> STEERING
    STEERING --> SELECTOR
    SELECTOR --> |"Mode 0"| OMNI["Omnidirectional<br/>(no model)"]
    SELECTOR --> |"Mode 1"| CARD
    SELECTOR --> |"Mode 2"| SUPER
    SELECTOR --> |"Mode 3"| FIG8
    SELECTOR --> |"Mode 4"| STEREO["Stereo<br/>(L/R split)"]

    CARD & SUPER & FIG8 & OMNI & STEREO --> OUT_CUSTOM["Beamformed Signal"]
```

**Parameters:**
```json
{
  "first_node": 0,
  "az_enable": 1,
  "pre_gain": 0,
  "post_gain": 0,
  "aggr": 0.85,
  "vertical_start": 0,
  "vertical_end": 180
}
```

Each beamforming pattern has **3 neural models:**
- `*_low_rotate_model.bin` — Low-frequency beamforming
- `*_high_rotate_model.bin` — High-frequency beamforming
- `*_vad_rotate_model.bin` — VAD-guided adaptive beamforming

Total: **9 neural beamforming models** for the 3 directional patterns.

---

### 6.4 EXTRACT (Neural Pre-Mask)

```mermaid
flowchart LR
    IN_EX["Multi-ch Signal"] --> NN_MASK["Neural Pre-Mask<br/>Meeting: premask_reverb_best_model.bin<br/>Live: insta360_psp_pre_chimera_model.bin"]
    NN_MASK --> |"Spectral mask × signal"| GAIN["Array Gain<br/>+17 dB"]
    GAIN --> OUT_EX["Enhanced Signal"]
```

**Parameters:**
```json
{
  "array_gain": 17.0,
  "nn_aggr": 1.0,
  "cache_clear_policy": 1,
  "input_channel": 1,
  "nn_model_file": "premask_reverb_best_model-220000_model.bin"
}
```

| Parameter | Value | Meaning |
|---|---|---|
| `array_gain` | 17.0 dB | Gain applied after masking |
| `nn_aggr` | 1.0 | Neural mask aggressiveness (1.0 = full) |
| `cache_clear_policy` | 1 | Clear neural cache on mode changes |
| `input_channel` | 1 | Channel to extract |

---

### 6.5 ARRAY_SE (Array Signal Enhancement)

```mermaid
flowchart LR
    IN_ARR["Pre-masked Signal<br/>6 channels"]

    subgraph "MVDR Beamformer"
        COV["Spatial Covariance<br/>Matrix Estimation"]
        MVDR["MVDR Filter<br/>5 iterations"]
        NS_POST["Post-filter NS<br/>(if ns=1)"]
    end

    IN_ARR --> COV --> MVDR --> NS_POST --> OUT_ARR["Single-channel<br/>Enhanced Signal"]
```

**Meeting pipeline:**
```json
{
  "ns": 0, "just_est_mask": 1,
  "hard_mask_thd": 0.0000001,
  "array_gain": 8.0,
  "nnmask6mic_mode": 0,
  "initial_echo_clip": 1,
  "array_ns": 5,
  "nnec": 3,
  "ns_aggr": 0.0,
  "mvdr_iters": 5, "mvdr_switch": 1,
  "use_dbss_mask": 0, "use_az_mask": 0
}
```

**Live pipeline:**
```json
{
  "ns": 1,
  "nnec": 0,
  "ns_aggr": 1.0,
  "scale_factor": 1
}
```

| Parameter | Meeting | Live | Effect |
|---|---|---|---|
| `ns` | 0 | 1 | Noise suppression off/on |
| `nnec` | 3 | 0 | Neural echo in array: aggressive / disabled |
| `ns_aggr` | 0.0 | 1.0 | NS aggressiveness: none / full |
| `mvdr_switch` | 1 | 1 | MVDR beamforming enabled in both |
| `mvdr_iters` | 5 | 5 | 5 iterations per frame in both |

---

### 6.6 NNPOST (Neural Post-Processing)

```mermaid
flowchart TD
    IN_NN["Array Output"]

    subgraph "NNPOST Processing"
        RAFA["RAFA Neural Model<br/>rafa-best_model-11000.bin"]
        MASK_APPLY{"disable_NNMASK?"}
        MASK_YES["Apply Spectral Mask<br/>mask_power_factor=0.75"]
        MASK_NO["Skip Mask<br/>(pass through)"]

        AGC_CHECK{"agc_gain_apply?"}
        AGC_YES["Feature AGC<br/>target=0.8, max=18dB<br/>alpha=0.1, attack=0.5s"]
        AGC_NO["Skip AGC"]
    end

    IN_NN --> RAFA
    RAFA --> MASK_APPLY
    MASK_APPLY --> |"Meeting: not set"| MASK_YES
    MASK_APPLY --> |"Live: =1"| MASK_NO
    MASK_YES --> AGC_CHECK
    MASK_NO --> AGC_CHECK
    AGC_CHECK --> |"Meeting: =1"| AGC_YES
    AGC_CHECK --> |"Live: =0"| AGC_NO
    AGC_YES --> OUT_NN["Output"]
    AGC_NO --> OUT_NN

    style MASK_YES fill:#ff6b6b,color:#fff
    style AGC_YES fill:#ff6b6b,color:#fff
    style MASK_NO fill:#2ed573,color:#fff
    style AGC_NO fill:#2ed573,color:#fff
```

**Meeting pipeline (problematic):**
```json
{
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
  "feature_agc_enable": 1,
  "cache_clear_policy": 0,
  "group_num": 32,
  "group_size": 8,
  "smooth_high_freq": 112
}
```

**Live pipeline (faster recovery):**
```json
{
  "nn_model_file": "rafa-best_model-11000_model.bin",
  "disable_NNMASK": 1,
  "agc_gain_apply": 0,
  "low_cplx_mode": 1
}
```

**This is the key difference.** The meeting pipeline applies the RAFA neural mask AND the AGC, both of which converge slowly after silence. The live pipeline bypasses both.

---

### 6.7 PEQ (Parametric Equalizer) — Meeting Only

```mermaid
flowchart LR
    IN_PEQ["NNPOST Output"]

    subgraph "8-Band PEQ at 32kHz"
        F0["Band 0: Peak 150Hz<br/>Q=1000, -3dB"]
        F1["Band 1: Peak 7kHz<br/>Q=700, -8dB"]
        F2["Band 2: Peak 13kHz<br/>Q=700, +3dB"]
        F3["Band 3: LPF 90Hz<br/>Q=700, 0dB"]
        F4["Band 4: Peak 100Hz<br/>Q=2000, -10dB"]
        F5["Band 5: Peak 500Hz<br/>Q=2000, -8dB"]
        F6["Band 6: Peak 3.5kHz<br/>Q=2000, -5dB"]
        F7["Band 7: Peak 8.5kHz<br/>Q=2000, -5dB"]
    end

    IN_PEQ --> F0 & F1 & F2 & F3 & F4 & F5 & F6 & F7 --> OUT_PEQ["EQ'd Output"]
```

**Filter configuration:**
```json
{
  "peq_enable": 1,
  "process_ch": 1,
  "peq_sample_rate": 32000,
  "dynamic_headroom": 0,
  "peq_filter_nums": 8,
  "peq_filter_params": [
    {"id": 0, "p": [2, 150, 1000, -3000, 1]},
    {"id": 1, "p": [4, 7000, 700, -8000, 1]},
    {"id": 2, "p": [4, 13000, 700, 3000, 1]},
    {"id": 3, "p": [1, 90, 700, 0, 1]},
    {"id": 4, "p": [2, 100, 2000, -10000, 1]},
    {"id": 5, "p": [2, 500, 2000, -8000, 1]},
    {"id": 6, "p": [2, 3500, 2000, -5000, 1]},
    {"id": 7, "p": [2, 8500, 2000, -5000, 1]}
  ]
}
```

Filter type codes: `1=LPF, 2=Peak, 4=High-shelf`. Gain in millidB (÷1000 for dB).

---

### 6.8 MIXER_AGC (Automatic Mic Mixing)

```mermaid
flowchart LR
    IN_MIX["PEQ Output<br/>(multi-channel)"]

    subgraph "Automatic Mic Mixing"
        AMM["AMM Algorithm<br/>Distributes gain across<br/>active mic channels"]
        AGC_FINAL["Final AGC<br/>Meeting: -1.5 to +11.5 dB<br/>Live: -1.5 to +15.0 dB"]
    end

    IN_MIX --> AMM --> AGC_FINAL --> OUT_MIX["Mixed Output<br/>(mono or stereo)"]
```

**Parameters:**
```json
{
  "agc": 1,
  "keep_hf": 0,
  "amm": 1,
  "amm_fake": 0,
  "agc_max_gain": 11.5,
  "agc_min_gain": -1.5,
  "agc_post_gain": 0.0,
  "stereo_enable": 0,
  "stereo_post_gain": 4.0,
  "stereo_min_gain_dB": -7.0,
  "stereo_max_gain_dB": 7.0
}
```

| Parameter | Meeting | Live |
|---|---|---|
| `agc_max_gain` | 11.5 dB | 15.0 dB |
| `agc_min_gain` | -1.5 dB | -1.5 dB |
| `amm` | 1 (enabled) | 1 (enabled) |
| `stereo_enable` | 0 | 0 |

---

### 6.9 DOA (Direction of Arrival)

```mermaid
flowchart LR
    SIGNAL["Multi-ch Signal"] --> DOA_EST["DOA Estimation<br/>Horizontal angle: 0°-360°<br/>Pitch angle: -90° to +90°"]
    DOA_EST --> |"UART"| MCU_DOA["MCU / PUC Camera<br/>Speaker tracking"]
    DOA_EST --> |"TEA SDK callback"| UI_DOA["UI DOA Indicator<br/>Beamforming screen"]
    DOA_EST --> |"Database"| DB_DOA["audio_mic_pickup_horizon_angle<br/>audio_mic_pickup_pitch_angle<br/>audio_mic_pickup_level<br/>audio_mic_pickup_quality"]
```

**Parameters:**
```json
{
  "upload_with_uart": 1,
  "upload_with_teasdk": 0
}
```

DOA data is sent to the MCU via UART for PUC camera speaker tracking (auto-framing mode).

---

## 7. AGC Chain Analysis

There are **multiple AGC stages** in the pipeline, each operating independently:

```mermaid
flowchart TD
    subgraph "AGC Stage 1: NNPOST Feature AGC"
        AGC1["agc_target_level: 0.8<br/>agc_max_gain_in_dB: 18.0<br/>agc_threshold: 0.001<br/>agc_attack: 0.5s<br/>agc_release: 0.5s<br/>agc_alpha: 0.1<br/><br/>Meeting: APPLIED ❌<br/>Live: BYPASSED ✅"]
    end

    subgraph "AGC Stage 2: MIXER_AGC"
        AGC2["agc_max_gain: 11.5 / 15.0 dB<br/>agc_min_gain: -1.5 dB<br/>AMM: enabled<br/><br/>Both modes: APPLIED"]
    end

    subgraph "AGC Stage 3: Dynamic EQ AGC"
        AGC3["agc_target: 23198<br/>agc_max_gain: 8.0<br/>agc_gain_knee: 5.0<br/>agc_release_time: 1000ms<br/><br/>Only in Mode 1: APPLIED<br/>(EQ enabled flag)"]
    end

    subgraph "AGC Stage 4: Reverb AGC"
        AGC4["agc_target: -6.0 dB<br/>agc_gain: 0.0<br/>agc_release_time: 200ms<br/><br/>Only in Mode 1: APPLIED<br/>(Reverb enabled flag)"]
    end

    subgraph "AGC Stage 5: Uplink Multi-band AGC"
        AGC5["4 bands: 300/4000/7000 Hz crossover<br/>Per-band targets + gains<br/>mbagc_release_time: 200ms<br/><br/>Config-dependent"]
    end

    AGC1 --> AGC2 --> AGC3 --> AGC4 --> AGC5

    style AGC1 fill:#ff6b6b,color:#fff
    style AGC2 fill:#ffa502,color:#fff
```

**In Noise Reduction mode (Mode 1), audio passes through up to 5 AGC stages.** Each independently estimates and applies gain, and each has its own convergence time. After silence, all 5 must independently ramp back up.

---

## 8. ASR Audio Path

```mermaid
flowchart TD
    TEA_OUT["TEA SDK 3A Output"]

    subgraph "ASR Audio Thread"
        TS["Insta360_TeaSdk3AOut<br/>get3AOutFrameIndex()<br/>get3AOutTimeStamp()"]
        PUSH["push3AOutFrame()"]
        RESAMPLE["LinearResample<br/>48kHz → 16kHz<br/>Sinc filter"]
    end

    subgraph "ASR Engine (libasr_psp.so)"
        VAD["Silero VAD v5<br/>silero_vad_v5.onnx<br/>Voice Activity Detection"]
        DECODER["WeNet ASR Decoder<br/>Online: Zipformer / Reverb<br/>Offline: SenseVoice"]
        CONTEXT["Context Graph<br/>Custom hotwords<br/>/data/common_settings/asr/hotword.list"]
        PUNC["Punctuation Model<br/>CT-Transformer Online<br/>PuncMino Offline"]
        DIAR["Speaker Diarization<br/>CAM++ (NPU, FP16)<br/>campp_118_cn_en_fp16.nb"]
    end

    subgraph "Output"
        SUBTITLE["Real-time Subtitles<br/>→ LVGL UI display"]
        ASR_JSON["ASR JSON Result<br/>→ audio_asr_total.json"]
        CLOUD_ASR["Cloud ASR Notice<br/>→ Insta360 API"]
    end

    TEA_OUT --> TS --> PUSH --> RESAMPLE
    RESAMPLE --> VAD --> DECODER
    DECODER --> CONTEXT
    DECODER --> PUNC
    DECODER --> DIAR

    PUNC --> SUBTITLE
    PUNC --> ASR_JSON
    DIAR --> ASR_JSON
    ASR_JSON --> CLOUD_ASR
```

### ASR Model Selection

```mermaid
flowchart TD
    LANG{"Language Setting"}

    LANG --> |"Chinese (Simplified)"| CN["Offline: SenseVoice<br/>/model/asr_ch/offline_sensevoice<br/>Online: Zipformer<br/>/model/asr_ch/online_zipformer"]
    LANG --> |"English"| EN["Offline: Reverb-ASR<br/>/model/asr_en/reverb-asr-10s<br/>Online: Reverb-ASR<br/>/model/asr_en/reverb_asr_online"]
    LANG --> |"German"| DE["Offline: /model/asr_de/de_offline<br/>Online: /model/asr_de/de_online"]
    LANG --> |"French"| FR["Offline: /model/asr_fr/fr_offline<br/>Online: /model/asr_fr/fr_online"]
    LANG --> |"Japanese"| JA["Offline: SenseVoice (shared)<br/>Online: /model/asr_ja/ja_online"]
    LANG --> |"Cantonese"| YUE["Offline: SenseVoice (shared)<br/>Online: Zipformer (shared)<br/>+ OpenCC conversion"]

    CN & EN & DE & FR & JA & YUE --> COMMON["Common Models:<br/>VAD: silero_vad_v5.onnx<br/>Diarization: campp_118_cn_en_fp16.nb<br/>Punctuation: punc_model / punc_mino<br/>Hotwords: hotword.list"]
```

---

## 9. Audio Mode Switching

```mermaid
stateDiagram-v2
    [*] --> Mode1: Default / "Noise Reduction"

    Mode1: Mode 1 - Noise Reduction
    Mode1: pipe_mode=0 (meeting)
    Mode1: EQ=enabled, Reverb=enabled

    Mode2: Mode 2 - Omni
    Mode2: pipe_mode=1, LiveEffect=0
    Mode2: EQ=disabled, Reverb=disabled

    Mode3: Mode 3 - Cardioid
    Mode3: pipe_mode=1, LiveEffect=1

    Mode4: Mode 4 - Supercardioid
    Mode4: pipe_mode=1, LiveEffect=2

    Mode5: Mode 5 - Figure-8
    Mode5: pipe_mode=1, LiveEffect=3

    Mode6: Mode 6 - Stereo
    Mode6: pipe_mode=1, LiveEffect=4

    Mode1 --> Mode2: setWorkMode(2)
    Mode1 --> Mode3: setWorkMode(3)
    Mode1 --> Mode4: setWorkMode(4)
    Mode1 --> Mode5: setWorkMode(5)
    Mode1 --> Mode6: setWorkMode(6)
    Mode2 --> Mode1: setWorkMode(1)
    Mode3 --> Mode1: setWorkMode(1)
    Mode4 --> Mode1: setWorkMode(1)
    Mode5 --> Mode1: setWorkMode(1)
    Mode6 --> Mode1: setWorkMode(1)
    Mode2 --> Mode3: SetLivePipeEffectMode
    Mode3 --> Mode4: SetLivePipeEffectMode
    Mode4 --> Mode5: SetLivePipeEffectMode
    Mode5 --> Mode6: SetLivePipeEffectMode
```

**Key code** (`Insta360_TeaSdk::setWorkMode`, line 18712):
- Mode 1: `SetProcessPipeWorkingMode(0)` + enable EQ + enable Reverb
- Modes 2-6: `SetProcessPipeWorkingMode(1)` + `SetLivePipeEffectMode(mode-2)` + disable EQ + disable Reverb

---

## 10. Dynamic EQ & Reverb Post-Processing

Only active in Mode 1 (Noise Reduction). Singletons `Insta360_EqProccess` and `Insta360_ReverbProccess`.

### Dynamic EQ (`/etc/dynamic_eq_config.json`)

```mermaid
flowchart LR
    IN_EQ["MIXER_AGC Output"]

    subgraph "3-Band IIR EQ"
        HPF_S{"HPF enabled?"}
        HPF_S --> |"hpf_switch=0"| NO_HPF["Bypass"]

        EQ1["Band 1: Peak 150Hz<br/>0 dB, Q=0.7"]
        EQ2["Band 2: Peak 8kHz<br/>+5 dB, Q=0.7"]
        EQ3["Band 3: LPF 90Hz<br/>-6 dB, Q=2.0"]
    end

    subgraph "4-Band Multi-AGC"
        MBAGC["Crossover: 300/4000/7000 Hz<br/>Band targets: 8203/16406/1160/580<br/>Max gains: 6/20/10/10 dB<br/>Release: 200ms per band"]
    end

    IN_EQ --> HPF_S
    NO_HPF --> EQ1 & EQ2 & EQ3
    EQ1 & EQ2 & EQ3 --> MBAGC --> OUT_EQ["EQ Output"]
```

### Reverb Processing (`/etc/reverb_config.json`)

```mermaid
flowchart LR
    IN_REV["EQ Output"]

    subgraph "Reverb Processor"
        REV_AGC["AGC<br/>target=-6dB, release=200ms"]
        APF["All-Pass Filter Reverb<br/>RT60=1.0s, d=0.2<br/>g=0.85, rev_rate=0.5"]
    end

    IN_REV --> REV_AGC --> APF --> OUT_REV["Final Output"]
```

---

## 11. Debug & Dump Infrastructure

```mermaid
flowchart TD
    subgraph "Debug Flags"
        FLAG1["/tmp/AUDIO_VERBOSE"]
        FLAG2["/tmp/DUMP_ASR_TRANS_AUDIO"]
        FLAG3["/data/DOA_ENABLE"]
        FLAG4["Debug Screen Toggle<br/>PAGE_DEBUG (ID 88)"]
    end

    subgraph "Dump Outputs"
        D1["/mnt/udisk/audio_dump/<br/>Full pipeline PCM dumps"]
        D2["/mnt/udisk/asr_dump/<br/>ASR audio input dumps"]
        D3["/uplink_agc_in_dump.pcm"]
        D4["/uplink_agc_out_dump.pcm"]
        D5["/downlink_agc_in_dump.pcm"]
        D6["/downlink_agc_out_dump.pcm"]
        D7["/userdata/roomix/dump/<br/>TEA SDK internal dumps"]
    end

    FLAG1 --> D1
    FLAG2 --> D2
    FLAG4 --> D1 & D2

    subgraph "Activation"
        A1["Insta360_SetDumpEnable(1)"]
        A2["Debug screen: Audio DUMP toggles"]
        A3["Touch file: /tmp/DUMP_ASR_TRANS_AUDIO"]
    end
```

### How to Enable Audio Dumps

If you have root access (telnet/UART):

```bash
# Enable full audio pipeline dump
touch /tmp/AUDIO_VERBOSE

# Enable ASR audio dump
touch /tmp/DUMP_ASR_TRANS_AUDIO

# Enable DOA debug output
touch /data/DOA_ENABLE

# Dumps will appear in /mnt/udisk/audio_dump/ and /mnt/udisk/asr_dump/
# when a USB drive is connected or the internal storage is used

# View TEA SDK logs
tail -f /opt/logs/psp.log | grep AUDIOSERVER
```

---

## 12. Full Embedded Configuration

The complete TEA SDK pipeline configuration is embedded in `libTEAProcessSdk.so` at binary offset **`0x375688`** (12,078 bytes). It defines both the meeting and live pipelines with all parameters for all 8 processing stages.

### Extraction Command

```bash
python3 -c "
with open('libTEAProcessSdk.so', 'rb') as f:
    data = f.read()
start = None
for i, b in enumerate(data):
    if 32 <= b <= 126 or b in (9, 10, 13):
        if start is None: start = i
    else:
        if start and i - start > 500:
            region = data[start:i].decode('ascii', errors='replace')
            if '\"aec\"' in region and '\"ns\"' in region:
                print(region)
                break
        start = None
"
```

### Global Processing Parameters

| Parameter | Value | Unit |
|---|---|---|
| `frame_duration_ms` | 20 | ms |
| `input_sample_rate` | 48000 | Hz |
| `output_sample_rate` | 48000 | Hz |
| `process_sample_rate` | 32000 | Hz |
| `process_bit_depth` | 32 | bits |
| `process_channel_num` | 6 | channels |
| `common_fft_size` | 1024 | samples |
| `common_hop_size` | 640 | samples |
| `process_pcm_dB_gain` | 13.0 (meeting) / 18.0 (live) | dB |

### Thread-to-Core Assignment

```mermaid
gantt
    title Pipeline Thread Distribution Across CPU Cores
    dateFormat X
    axisFormat %s

    section Core 1
    AEC (Thread 1)          :0, 1

    section Core 2
    WPE/CUSTOM (Thread 7)   :0, 1
    EXTRACT (Thread 2)      :1, 2
    NNPOST (Thread 4)       :2, 3
    DOA (Thread 6)          :3, 4

    section Core 3
    ARRAY_SE (Thread 3)     :0, 1
    PEQ (Thread 8)          :1, 2
    MIXER_AGC (Thread 5)    :2, 3
```

The pipeline uses 3 of the 4 available Cortex-A53 cores (Core 0 reserved for Linux kernel + application). Processing is pipelined: AEC runs on Core 1 while ARRAY_SE runs on Core 3, allowing parallel execution of independent stages.

---

### Neural Model Summary

| Model | File | Stage | Purpose | Size |
|---|---|---|---|---|
| NNEC | `nnec_two_stage_32k.bin` | AEC | Neural echo cancellation | ~2 MB |
| Premask Reverb | `premask_reverb_best_model-220000_model.bin` | EXTRACT (meeting) | Dereverberation mask | ~3 MB |
| Chimera | `insta360_psp_pre_chimera_model.bin` | EXTRACT (live) | Multi-channel separation mask | ~3 MB |
| RAFA | `rafa-best_model-11000_model.bin` | NNPOST | Noise suppression mask | ~2 MB |
| TAER Cardioid ×3 | `insta360_az_taer_cardiod_{low,high,vad}_rotate_model.bin` | CUSTOM (live) | Cardioid beamforming | ~1 MB each |
| TAER Supercardioid ×3 | `insta360_az_taer_supercardiod_{low,high,vad}_rotate_model.bin` | CUSTOM (live) | Supercardioid beamforming | ~1 MB each |
| TAER Figure-8 ×3 | `insta360_az_taer_figure8_{low,high,vad}_rotate_model.bin` | CUSTOM (live) | Figure-8 beamforming | ~1 MB each |
| Solo Steady | `insta360_nnpost_solo_steedy.bin` | NNPOST | Solo speaker steady-state | ~1 MB |
| MCH Pre Chimera | `insta360_psp_mchpre_chimera_model.bin` | EXTRACT | Multi-channel pre-processing | ~3 MB |

**Total: 15 neural models**, all running on the ARM Cortex-A53 CPU (ONNX Runtime) or VeriSilicon NPU (`.nb` format for CAM++ diarization).

---

*Document generated from Ghidra decompilation and binary analysis. Pipeline configuration extracted from libTEAProcessSdk.so at offset 0x375688.*
