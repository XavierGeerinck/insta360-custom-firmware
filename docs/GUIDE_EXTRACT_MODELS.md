# Insta360 Wave — AI Model Extraction Guide

How to extract all AI/ML models from the Insta360 Wave device.

---

## Overview

The Wave has models in two locations:

| Location | Partition | Access via USB File Transfer | Access via ADB |
|---|---|---|---|
| `/usr/bin/tea_models/` | rootfs (SquashFS) | No | Yes |
| `/model/` | Separate ext4 (4.1 GB) | No | Yes |
| `/usr/lib/s2t_data/` | rootfs (SquashFS) | No | Yes |

**USB File Transfer mode only exposes `/mnt/udisk/`** (the user storage partition with recordings). The model and rootfs partitions require root access via ADB.

---

## Step-by-Step Extraction

### Prerequisites

```bash
brew install android-platform-tools
pip3 install hidapi  # or: pip3 install --break-system-packages hidapi
```

### 1. Enable ADB

Use the checked-in helper script from the repository root:

If you are using the standalone copy instead of the checked-in helper:

```bash
python3 enable_adb.py
sleep 3
adb devices  # Should show PSP_<serial>
```

If you need a standalone copy outside this repo, this is the helper script contents:

```python
#!/usr/bin/env python3
import hid

h = hid.device()
h.open(0x18F0, 0x4E40)  # Insta360 Wave
h.set_nonblocking(1)
while h.read(64): pass  # drain

pkt = bytearray(64)
pkt[0] = 0x03  # Report ID
pkt[1] = 0xF0  # HID_MSG_TYPE_OF_SWT_FACTORY
pkt[2] = 0x00
pkt[4] = 0xF0
pkt[5] = 0xC0  # ADB enable
h.write(bytes(pkt))
h.close()
print("ADB enabled. Run: adb devices")
```

```bash
python3 ./waved/scripts/enable_adb.py
sleep 3
adb devices  # Should show PSP_<serial>
```

If the device rebooted or switched USB modes, run the helper again before pulling files.

### 2. Pull All Models

```bash
# Create output directory
mkdir -p extracted_models

# ASR models from /model partition (4.1 GB, ~10 min over USB 2.0)
adb pull /model/ extracted_models/model/

# TEA SDK audio models from rootfs (75.5 MB)
adb pull /usr/bin/tea_models/ extracted_models/tea_models/

# Chinese conversion data (1 MB)
adb pull /usr/lib/s2t_data/ extracted_models/s2t_data/

# Inference runtime libraries
mkdir -p extracted_models/runtime
adb pull /usr/lib/libonnxruntime.so.1.15.1 extracted_models/runtime/
adb pull /usr/lib/libnnsdk.so extracted_models/runtime/
adb pull /usr/lib/libasr_psp.so extracted_models/runtime/
adb pull /usr/lib/libasr_reverb.so extracted_models/runtime/
adb pull /usr/lib/libTEAProcessSdk.so extracted_models/runtime/
```

### 3. Verify

```bash
# Check file count (should be 173 for /model, 15 for tea_models)
find extracted_models/model/ -type f | wc -l
find extracted_models/tea_models/ -type f | wc -l

# Check model version
cat extracted_models/model/model_version.txt
# Expected: 20250613_V2.0.0.1
```

### 4. Restore Normal USB Mode

```python
#!/usr/bin/env python3
import hid
h = hid.device()
h.open(0x18F0, 0x4E40)
h.set_nonblocking(1)
while h.read(64): pass
pkt = bytearray(64)
pkt[0] = 0x03
pkt[1] = 0xF0
pkt[2] = 0x00
pkt[4] = 0xF0
pkt[5] = 0xE0  # User mode (restore normal USB)
h.write(bytes(pkt))
h.close()
print("Normal USB mode restored.")
```

---

## Extracted Model Inventory

**Model version:** `20250613_V2.0.0.1`

### ASR Models (`/model/`) — 4.1 GB total

#### Chinese ASR (1.2 GB)

| Directory | Files | Format | Architecture |
|---|---|---|---|
| `asr_ch/offline_sensevoice/` | 17 `.nb` + config | VeriSilicon NBG | SenseVoice (Alibaba) |
| `asr_ch/online_zipformer/` | 1 `.nb` + tokenizer | VeriSilicon NBG | Zipformer (k2-fsa) |

The SenseVoice model is split into 15 sub-graph files (`svs6_sp_*` and `svs12_sp_*`) for the NPU — the `6` and `12` variants likely correspond to different batch sizes or chunk lengths.

#### English ASR (1.1 GB)

| Directory | Files | Format | Architecture |
|---|---|---|---|
| `asr_en/reverb-asr-10s/` | 38 `.nb` + config | VeriSilicon NBG | Reverb-ASR (WeNet) |
| `asr_en/reverb_asr_online/` | 2 `.nb` + 1 `.onnx` + config | Mixed | Reverb-ASR online |

The English model has both 5-second and 10-second chunk variants (`reverb_5s_split_*` and `reverb_10s_*`), each split into ~19 sub-graphs for NPU execution. The online model has a `ctc.onnx` (9.8 MB) that runs on CPU via ONNX Runtime and an `encoder.nb` (177 MB) that runs on NPU.

#### German ASR (603 MB)

| Directory | Files | Format | Architecture |
|---|---|---|---|
| `asr_de/de_offline/` | 28 `.nb` | VeriSilicon NBG | WeNet |
| `asr_de/de_online/` | 2 `.nb` | VeriSilicon NBG | WeNet |

Split into `encoder_split_6_*` (6-chunk) and `encoder_split_12_*` (12-chunk) variants.

#### French ASR (604 MB)

| Directory | Files | Format | Architecture |
|---|---|---|---|
| `asr_fr/fr_offline/` | 28 `.nb` | VeriSilicon NBG | WeNet |
| `asr_fr/fr_online/` | 2 `.nb` | VeriSilicon NBG | WeNet |

Same split structure as German.

#### Japanese ASR (75 MB)

| Directory | Files | Format | Architecture |
|---|---|---|---|
| `asr_ja/ja_online/` | 2 `.nb` | VeriSilicon NBG | WeNet |

Online streaming only — no offline model. Smallest language model.

#### Speaker Diarization (17 MB)

| File | Format | Architecture |
|---|---|---|
| `diarization/campp_118_cn_en_fp16.nb` | VeriSilicon NBG (FP16) | CAM++ (Alibaba 3D-Speaker) |

Speaker embedding extraction. FP16 quantized for NPU. Supports Chinese + English speakers.

#### Voice Activity Detection (2.2 MB)

| File | Format | Architecture |
|---|---|---|
| `vad/silero_vad_v5.onnx` | ONNX | Silero VAD v5 |

The only standard ONNX model — runs on CPU via ONNX Runtime. Open source: https://github.com/snakers4/silero-vad

#### Punctuation Models (550 MB)

| Directory | File | Size | Format | Architecture |
|---|---|---|---|---|
| `punc_model/` | `model_quant.onnx` | 269 MB | ONNX | CT-Transformer (online punctuation) |
| `punc_mino/` | `model_quantized.onnx` | 266 MB | ONNX | PuncMino (offline punctuation) |

Both are standard ONNX models running on CPU. They're the largest ONNX files.

### TEA SDK Audio Models (`/usr/bin/tea_models/`) — 75.5 MB

| File | Size | Format | Purpose |
|---|---|---|---|
| `rafa-best_model-11000_model.bin` | 28 MB | TNNE | Neural noise suppression |
| `insta360_psp_mchpre_chimera_model.bin` | 13 MB | TNNE | Multi-channel source separation |
| `insta360_nnpost_solo_steedy.bin` | 6.1 MB | TNNE | Solo speaker processing |
| `insta360_az_taer_*_low_rotate_model.bin` | 5.1 MB × 3 | Binary | Beamforming (low freq) |
| `insta360_az_taer_*_high_rotate_model.bin` | 2.5 MB × 3 | Binary | Beamforming (high freq) |
| `insta360_psp_pre_chimera_model.bin` | 2.5 MB | TNNE | Source separation pre-mask |
| `premask_reverb_best_model-220000_model.bin` | 2.5 MB | TNNE | Dereverberation mask |
| `nnec_two_stage_32k.bin` | 628 KB | TNNE | Neural echo cancellation |
| `insta360_az_taer_*_vad_rotate_model.bin` | 4.3 KB × 3 | Binary | Beamforming VAD |

---

## Model Formats

### VeriSilicon NBG (`.nb`)

Pre-compiled binary graphs for the Amlogic NPU (VeriSilicon Vivante). Cannot be converted back to ONNX — they are hardware-specific compiled binaries containing:
- Layer operations mapped to NPU hardware units
- Weight data in quantized format (FP16 or INT8)
- Memory allocation plans for NPU SRAM
- DMA transfer schedules

**To use these models**, you need the same Amlogic A5 NPU hardware + `libnnsdk.so` ADLA runtime.

**To recreate them**, you need the original ONNX models + VeriSilicon Acuity Toolkit (NDA required from Amlogic/VeriSilicon).

### ONNX (`.onnx`)

Standard Open Neural Network Exchange format. Can be loaded by ONNX Runtime on any platform.

Files in this format:
- `vad/silero_vad_v5.onnx` (2.2 MB)
- `asr_en/reverb_asr_online/ctc.onnx` (9.8 MB)
- `punc_model/model_quant.onnx` (269 MB)
- `punc_mino/model_quantized.onnx` (266 MB)

**These are portable** — you can run them on any machine with ONNX Runtime.

### TNNE (`.bin` with `tnne` header)

Roomix TEA SDK's custom neural network format. PyTorch-trained models compiled for their SDK.

**Header format:**
```
tnne\r\n
num_layers: <N>
training: pytorch

<layer definitions>
```

### Versioned Binary (`.bin` with `version:0.1` header)

Beamforming coefficient files.

**Header format:**
```
version:0.1
<timestamp>
<binary coefficient data>
```

---

## Using the Extracted ONNX Models

### Silero VAD

```python
import onnxruntime as ort
import numpy as np

session = ort.InferenceSession("extracted_models/model/vad/silero_vad_v5.onnx")

# 16kHz audio, 512-sample chunks
audio_chunk = np.random.randn(1, 512).astype(np.float32)
h = np.zeros((2, 1, 64)).astype(np.float32)
c = np.zeros((2, 1, 64)).astype(np.float32)
sr = np.array([16000], dtype=np.int64)

output, h_out, c_out = session.run(None, {
    "input": audio_chunk, "h": h, "c": c, "sr": sr
})
speech_prob = output[0][0]
print(f"Speech probability: {speech_prob:.3f}")
```

### Punctuation Model

```python
import onnxruntime as ort

session = ort.InferenceSession("extracted_models/model/punc_model/model_quant.onnx")
# Check input/output names
for inp in session.get_inputs():
    print(f"Input: {inp.name} shape={inp.shape} type={inp.type}")
for out in session.get_outputs():
    print(f"Output: {out.name} shape={out.shape} type={out.type}")
```

---

## Automation Script

Complete script to extract everything in one go:

```bash
#!/bin/bash
# extract_all_models.sh — Pull all AI models from Insta360 Wave
set -e

OUTDIR="${1:-insta360_wave_models}"
mkdir -p "$OUTDIR"

echo "=== Step 1: Enable ADB ==="
python3 -c "
import hid
h = hid.device()
h.open(0x18F0, 0x4E40)
h.set_nonblocking(1)
while h.read(64): pass
pkt = bytearray(64)
pkt[0]=0x03; pkt[1]=0xF0; pkt[2]=0x00; pkt[4]=0xF0; pkt[5]=0xC0
h.write(bytes(pkt))
h.close()
print('ADB enabled')
"
sleep 5
adb wait-for-device

echo "=== Step 2: Pull ASR models (4.1 GB, ~10 min) ==="
adb pull /model/ "$OUTDIR/model/"

echo "=== Step 3: Pull TEA SDK models (75 MB) ==="
adb pull /usr/bin/tea_models/ "$OUTDIR/tea_models/"

echo "=== Step 4: Pull S2T data (1 MB) ==="
adb pull /usr/lib/s2t_data/ "$OUTDIR/s2t_data/"

echo "=== Step 5: Pull runtime libraries ==="
mkdir -p "$OUTDIR/runtime"
for lib in libonnxruntime.so.1.15.1 libnnsdk.so libasr_psp.so libasr_reverb.so libTEAProcessSdk.so; do
    adb pull "/usr/lib/$lib" "$OUTDIR/runtime/" 2>/dev/null || true
done

echo "=== Step 6: Restore normal USB ==="
python3 -c "
import hid
h = hid.device()
try:
    h.open(0x18F0, 0x4E40)
    h.set_nonblocking(1)
    while h.read(64): pass
    pkt = bytearray(64)
    pkt[0]=0x03; pkt[1]=0xF0; pkt[2]=0x00; pkt[4]=0xF0; pkt[5]=0xE0
    h.write(bytes(pkt))
    h.close()
    print('Normal USB restored')
except:
    print('Device already in normal mode')
"

echo "=== Done! ==="
echo "Models extracted to: $OUTDIR"
du -sh "$OUTDIR"
find "$OUTDIR" -type f | wc -l
echo "files total"
```

```bash
chmod +x extract_all_models.sh
./extract_all_models.sh my_wave_models
```

---

## Notes

- **ADB mode is not persistent** — it resets on device reboot. Re-run the enable script after each reboot.
- **USB 2.0 speed** — The Wave connects at High Speed (480 Mbps), so 4 GB takes ~10 minutes.
- **No impact on device** — Pulling files is read-only. The device continues operating normally.
- **Model partition is read-only** — Mounted as ext4 RO. You cannot modify models via ADB without remounting.
- **`.nb` files are hardware-specific** — They only run on the Amlogic A5 NPU. The `.onnx` files are portable.
