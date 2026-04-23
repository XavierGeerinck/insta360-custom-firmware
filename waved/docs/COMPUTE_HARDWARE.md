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

## Seeing what stock uses the NPU for

Stock firmware (`/data/main_processor` or whichever binary) links
`libovxlib.so` / `libGAL.so`. Quick audit:

```bash
adb shell "for p in /proc/[0-9]*; do
  if grep -q libovxlib /proc/\${p##*/}/maps 2>/dev/null; then
    echo \"\${p##*/}: \$(cat \$p/comm)\"
  fi
done"
```

This is how to find which stock process is holding the NPU context — an
`/dev/galcore` open can serialise access, so we may need to kill stock
ASR before waved can claim the NPU (same pattern as the S60waved
watchdog interaction for ALSA).

## Related docs

- [AUDIO_HARDWARE.md](AUDIO_HARDWARE.md) — 8-mic array + DOA (A55-only).
- `/etc/init.d/S60waved` watchdog behaviour — in `AUDIO_HARDWARE.md`.
