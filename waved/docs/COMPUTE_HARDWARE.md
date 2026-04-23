# Compute Hardware — CPU, NPU, DSP

Reference for non-CPU compute resources on the Insta360 Wave (Amlogic A113X2,
AV400 audio platform).

## Summary

| Resource | What it is | Stock firmware uses it for | Suitable for waved |
|----------|------------|----------------------------|--------------------|
| 4× Cortex-A55 @ ~1.2 GHz, NEON + `asimddp` | CPU complex | Everything the NPU/DSP doesn't handle | AEC, beamforming, DOA, orchestration |
| **Vivante VIP NPU** (GC-class, rev 0x9002, npu_version 6) | Dedicated neural accelerator | ASR (WeNet-style CTC + encoders, all languages), speaker diarization | **SP#4 neural NS + dereverb** (target) |
| HiFi-class audio DSP at `/dev/audiodsp0` | 8 MB firmware region, chr dev 272 | Unknown (likely stock AEC / NR / DRC) | Out of scope unless we get Cadence/Tensilica toolchain |

## Vivante NPU

### Kernel + driver evidence

```text
dmesg | grep galcore
  galcore: loading out-of-tree module taints kernel.
  npu_version: 6
  galcore irq number is 19.
  Galcore version 6.4.19.5.946407
  [galcore]: Warning: chipRevision mismatch, database chipRevision=0x9001 register read chipRevision=0x9002
```

- Char device: `/dev/galcore` (major 199). User-space issues ioctls against
  this to submit command buffers.
- IP is a VIPNano-class core. "Input buffers: 29 × 1648 bytes" from the boot
  log indicates a real on-chip SRAM working set (not a software fallback).

### Userspace stack

Under `/usr/lib` (and `/usr/lib64`, identical copies):

| Library | Role |
|---------|------|
| `libGAL.so` | Vivante HAL (kernel ioctl wrapper) |
| `libVSC.so`, `libVSC_Lite.so` | Vivante Shader Compiler |
| `libOpenVX.so`, `libOpenVXU.so` | Khronos OpenVX graph API |
| `libovxlib.so` | VeriSilicon's tensor-op wrapper on top of OpenVX |
| `libOpenCL.so` | Vivante OpenCL runtime (`Vivante OpenCL Platform`, version `6.4.11:505228`) |

The normal inference entry point is **ovxlib**: you call `vxCreateContext()`
/ `vxCreateGraph()` and feed tensors. Models are shipped as pre-compiled
`.nb` binary graphs.

### Model format: `.nb`

`.nb` = "Network Binary" — VeriSilicon's compiled graph format produced by
the **Acuity toolkit** (`acuity-toolkit`). The compiler is host-side only
(x86 Linux), not on-device. Typical flow:

```
ONNX / TFLite  ─►  acuity_convert  ─►  case code (C) + .nb
                                           │
                                           ▼
                                      ovxlib loader + NPU execution
```

Quantisation during compilation is typical (int8 symmetric, sometimes int16
for sensitive layers). Activations and weights are fused + scheduled for
the VIP's internal tiling.

Stock firmware ships `.nb` files under `/model/`:

```
/model/asr_{ch,en,de,fr,ja}/{offline,online}/*.nb   — encoder / CTC / decoders
/model/diarization/campp_118_cn_en_fp16.nb          — speaker embedding (fp16)
/model/punc_model/*.nb                              — punctuation
/model/vad/silero_vad_v5.onnx                       — raw ONNX (CPU path)
```

The presence of `*_fp16.nb` confirms the NPU also handles fp16 paths, not
only int8.

### Toolchain access (the one real gap)

The Acuity toolkit is under NDA from VeriSilicon and usually reaches
licensees via the Amlogic BSP. To compile our own ONNX → `.nb` we need
one of:

1. **Acuity toolkit** from Amlogic/VeriSilicon — the native path. Check
   `/data`, `/system/vendor`, any SDK drops from Insta360 for a bundled
   copy; sometimes it ships inside the board support package.
2. **TIM-VX** (VeriSilicon's open-source tensor-interface, github.com/VeriSilicon/TIM-VX)
   plus **tim-vx-delegate** for TFLite, or the `onnxruntime-vsi-npu`
   execution provider. This is the viable open-source path: build
   onnxruntime against TIM-VX, which calls into the same `libovxlib.so`
   / `libGAL.so` / `/dev/galcore` stack already on-device. Trade-off:
   op coverage vs. Acuity is incomplete for bleeding-edge architectures.
3. Reverse-engineer `.nb` (don't — it's versioned against the galcore
   driver and schema-shifts between NPU generations).

### Integration path for waved (SP#4 target)

Recommended default: build **onnxruntime + VSI-NPU execution provider**
against the on-device `libovxlib.so` / `libGAL.so`, cross-compiled for
aarch64. Expose it via a Rust wrapper crate in `crates/waved-audio/src/ns/`.
Model candidate: **DeepFilterNet 3** — ships a cleanly-exportable ONNX
and is explicitly designed for edge inference (int8 quantisable, ~2M
params, 10 ms frames).

If the VSI-NPU EP lacks an op DFN3 needs, fall back to the Acuity path
(needs toolkit access) or `tract` CPU inference (eats 1–2 A55 cores).

## Audio DSP (`/dev/audiodsp0`)

- Boot log: `audio_dsp: [dsp]register dsp to char device(272)` + 8 MB
  firmware reservation at physaddr `0x30000000–0x30800000` labelled
  `linux,dsp_fw`.
- Char device major 272, minor 0.
- Typical Amlogic audio-platform DSP is a Cadence HiFi-class core. The
  Tensilica Xtensa toolchain is vendor-locked; writing our own firmware
  is a significant effort.
- **Out of scope** for waved unless/until we need more than the NPU +
  A55 can deliver. NPU should cover NS + dereverb with plenty of
  headroom.

## Stock audio stack (what uses the NPU today)

The stock NPU consumer is `/usr/bin/audioserver` (~398 KB ELF). Its
library graph:

```
/usr/bin/audioserver
├─ libInstaAudio.so           — Insta360 audio pipeline glue
├─ libTEAProcessSdk.so (8.1M) — Tencent Enhanced Audio SDK
│                               (NS / enhancement / AEC, proprietary)
├─ libbva_audio.so     (1.9M) — secondary audio component
├─ liblibspai_mic_sigp.so     — SPAI mic-signal processing
├─ libonnxruntime.so.1.15.1   — CPU-only ORT build, no VSI-NPU EP
└─ libnnsdk.so ──► libovxlib.so ──► /dev/galcore ──► NPU
```

Model-to-backend mapping:

| Model path | Format | Backend |
|------------|--------|---------|
| `/model/vad/silero_vad_v5.onnx` | ONNX | `libonnxruntime` (CPU) |
| `/model/asr_*/*.nb` | Vivante `.nb` | `libnnsdk` → NPU |
| `/model/diarization/campp_118_cn_en_fp16.nb` | fp16 `.nb` | `libnnsdk` → NPU |
| `/model/punc_model/*.nb`, `/model/punc_mino/*.nb` | `.nb` | `libnnsdk` → NPU |

**There is no dedicated NS/dereverb model in `/model/`.** Stock noise
suppression almost certainly lives inside `libTEAProcessSdk.so` (Tencent
TEA), running on the A55 cores. The NPU is reserved for ASR, punctuation,
and diarization.

### What this means for waved SP#4

The on-device `libonnxruntime.so.1.15.1` cannot reach the NPU — it's a
vanilla CPU build. Our choices for NPU inference:

1. **Build our own ORT with the VSI-NPU EP.** VeriSilicon maintains the
   provider at github.com/microsoft/onnxruntime under
   `onnxruntime/core/providers/vsinpu`. It depends on TIM-VX, which
   depends on `libovxlib.so` + `/dev/galcore` (both on-device). Output:
   a drop-in `libonnxruntime.so.1.16+` or our own crate-level wrapper.
2. **Call `libovxlib.so` via TIM-VX directly from Rust.** TIM-VX
   (github.com/VeriSilicon/TIM-VX) is VeriSilicon's open-source C++ API.
   Smaller footprint than ORT, but we'd have to construct the DFN3
   graph layer-by-layer instead of loading an ONNX.
3. **Reuse stock `libnnsdk.so` directly.** It exports the standard
   VeriSilicon `vsi_nn_*` symbols. Simplest at runtime, but we still
   need `.nb` files to feed it — which brings back the Acuity toolkit
   dependency.

Route 1 is the default recommendation: biggest model-import surface
(any ONNX), open source, fits existing ML-ops tooling.

### NPU proof-of-life (validated 2026-04-23)

Our own process can open `/dev/galcore`, dlopen `libOpenVX.so` +
`libovxlib.so`, call `vxCreateContext()` and `vsi_nn_CreateContext()`,
and get back non-null handles. Release is clean. No kill-stock dance
required in the steady state (we happen to have killed `audioserver`
indirectly via earlier watchdog kills, so this measurement doesn't
tell us whether stock holds an exclusive NPU lock during normal boot —
worth a re-check once we start SP#4 with everything running).

Reproduce:

```bash
cross build -p waved-npu-probe --release --target aarch64-unknown-linux-gnu
adb push target/aarch64-unknown-linux-gnu/release/waved-npu-probe /data/waved/
adb shell /data/waved/waved-npu-probe
```

Expected output (on this unit, firmware build Nov 28 2025):

```text
[1] /dev/galcore                        OK
[2] dlopen libOpenVX.so                  OK
[3] vxCreateContext / vxReleaseContext   OK
[4] vxCreateContext()                    OK — handle ~0x7f8...
[5] vxReleaseContext()                   OK — status 0
[7] dlopen libovxlib.so                  OK
[8] vsi_nn_CreateContext()               OK — handle ~0x559...
RESULT: OK — NPU reachable from waved's process.
```

### Finding which stock process holds the NPU

If a future probe run fails at step 1 or 4, audit which stock process
has `/dev/galcore` open:

```bash
adb shell "for p in /proc/[0-9]*/fd; do
  pid=\${p%/fd}; pid=\${pid##*/}
  if ls -la \$p 2>/dev/null | grep -q galcore; then
    echo \"pid=\$pid comm=\$(cat /proc/\$pid/comm 2>/dev/null)\"
  fi
done"
```

Same pattern as the ALSA / S60waved watchdog dance — kill the
conflicting stock service (likely `audioserver`) before launching
waved's NPU path.

## Related docs

- [AUDIO_HARDWARE.md](AUDIO_HARDWARE.md) — 8-mic array + DOA (A55-only).
- `/etc/init.d/S60waved` watchdog behaviour — in `AUDIO_HARDWARE.md`.
