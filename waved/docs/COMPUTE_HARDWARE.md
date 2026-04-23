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
`libovxlib.so` + our cross-built `libtim-vx.so`, call `vxCreateContext()`
and `vsi_nn_CreateContext()`, and get back non-null handles. Release
is clean.

**TIM-VX cross-compile (SP#4 Task 2, 2026-04-23)**: built from
`third_party/TIM-VX` v1.2.22 against `third_party/device-sdk/`
(TIM-VX's x86_64 prebuilt headers at 6.4.15 paired with aarch64 .so
files pulled from `/usr/lib` on device at 6.4.19). The resulting
`libtim-vx.so` loads on-device with every transitive symbol resolved
— no ABI mismatch between 6.4.15 headers and 6.4.19 runtime on this
chip revision (0x9002). Recipe: `scripts/build-tim-vx.sh`.

**One quirk**: this chip ships no `libCLC.so`. TIM-VX's build expects
one (the OpenCL C compiler, used for JIT kernel compilation). The
functionality is folded into `libOpenCL.so` on this stack; a symlink
`third_party/device-sdk/lib/libCLC.so → libOpenCL.so` satisfies the
link step and the resulting library works at runtime. No kill-stock dance
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

### Host prebuilt SDK is simulator-only (2026-04-23)

Pursuing the AoT path (compile on host, load on device via `ops::NBG`)
ran into one more wall: **TIM-VX's `prebuilt-sdk/x86_64_linux/` is a CPU
simulator, not a chip-native compiler.** Evidence:

- `strings libOpenVX.so.1` shows `vSimCustomizedFeature`,
  `vivSetVSimulatorTarget`, `gVsimulatorTargetID`,
  `AddCallBackForSimulator`, `isVSimulatorInRunMode` — all simulator
  machinery.
- `strings libArchModelSw.so` enumerates only `GC8000NANOULTRA31_vip1`,
  `GCNANOULTRA31`, `GCNANOULTRA31_VIP2`, `vip7Version: 7` — NPU
  archmodels for older GC7000-class chips, **no VIP9000**.

On-device NPU is VIP9000Nano-S (pid `0x1000000e`), which this archmodel
library doesn't know about. The host `CompileToBinary` still produces a
syntactically valid NBG (magic `VPMN`, 2944 bytes) but the compiled
kernels inside target simulator semantics. On-device
`tim::vx::ops::NBG::Compile()` fails at `compute_node:447 Create node[0]
NBG fail` when trying to instantiate the underlying vx op against real
silicon.

Also: TIM-VX v1.2.22's bundled NBG format version is `0x0001001e`, but
its own standalone `nbg_parser.c` only accepts up to `0x00010014` —
internal inconsistency. For our own loader path (`ops::NBG` directly, no
standalone parser) this isn't the blocker; the chip-archmodel mismatch
above is.

**What this means for "own the models"**:

| Path | Unblocked? | Notes |
|------|-----------|-------|
| Host AoT compile, device run (OSS stack) | ✗ | Host SDK lacks VIP9000 archmodel |
| Device-side compile + cache .nb | open | Requires device's libOpenVX to have the compiler + kernel cache; untested |
| `libnnsdk.so` with chip-native SDK | ✗ NDA | Needs VeriSilicon Acuity Toolkit for this chip |
| A311D prebuilt SDK from TIM-VX | untested | VIP9000 is different IP family from A311D's GCNANOULTRA31 — unlikely to help but cheap to try |

Short of getting a VIP9000-native SDK (VeriSilicon/Amlogic/Insta360
relationship), fully open-source host-side NPU compilation is not
viable on this silicon.

### TIM-VX ahead-of-time compile path (mapped 2026-04-23)

Probing during SP#4 established the working path:

```
HOST x86_64                         DEVICE aarch64
─────────────────                   ──────────────────────
TIM-VX (prebuilt SDK, simulator)    TIM-VX (USE_EXTERNAL_OVXLIB=ON)
  |                                   |
  Construct graph in code             Load .nb via ops::NBG
  graph->CompileToBinary(.nb) ─────► graph->Run()  (device libovxlib)
```

**Key moves:**
- Build TIM-VX on host with its x86_64 prebuilt SDK — shaders + OpenCL
  compile machinery are present → `CompileToBinary()` produces a `.nb`
  that bakes in the NPU instructions for the target chip.
- Build TIM-VX for device with `-DTIM_VX_USE_EXTERNAL_OVXLIB=ON` and
  `-DOVXLIB_LIB=/path/to/device/libovxlib.so` — libtim-vx.so (2.2 MB, was
  8.6 MB bundled) delegates all `vsi_nn_*` calls to the **device's own**
  libovxlib, which has the correct kernel-contract ioctl for chip rev
  0x9002 (see next section for the earlier mismatch).
- Load the .nb at device runtime via `tim::vx::ops::NBG` + `ops::NBG` —
  no shader compilation required on-device.

**NPU identification** (from `VSI_NN_LOG_LEVEL=5`):
- product: **VIP9000Nano-S**, pid `0x1000000e`
- stock OVXLIB on device: **1.2.15** (our TIM-VX headers are 1.2.22 —
  close enough for the stable C++ surface we use).

**Runtime env requirement**: every process that loads libovxlib must set
`VIVANTE_SDK_DIR` (any non-empty path works — the var is checked for
presence, not content, when only running pre-compiled .nb).

### TIM-VX runtime ABI mismatch (for reference; fixed by USE_EXTERNAL_OVXLIB)

Before `TIM_VX_USE_EXTERNAL_OVXLIB=ON`, TIM-VX v1.2.22's bundled ovxlib
copy issued a `query_hardware_caps` ioctl that galcore 6.4.19 on chip rev
0x9002 rejected:

```
tim_vx_smoke: create context... E [vsi_nn_context.c:query_hardware_caps:55]
  CHECK STATUS(-10:The supplied parameter information does not match the
  kernel contract.)
OK (context object returned but poisoned)
tim_vx_smoke: create graph... OK
E [tensor.cc:Init:447]Create tensor fail!
```

TIM-VX's hardware-capability query emits an ioctl that galcore 6.4.19
on chip rev 0x9002 rejects as a contract mismatch. The context is
returned non-null (no fail-fast in TIM-VX) but tensor creation fails
downstream. Also: stock `.nb` files use a newer NBG format than
TIM-VX v1.2.22 parses (`NBG parser is forward compatible with NBG
format, please update NBG parser in time`), so `tim::vx::ops::NBG`
can't load them either.

This was a **chip/driver/TIM-VX triple mismatch** not patchable from
userspace. Fixed by delegating to the device's own libovxlib (see above).
Symptoms when misconfigured:
- TIM-VX `Context::Create()` returns a poisoned handle.
- `ops::NBG` can't load stock `.nb` (unrelated: stock `.nb` format is
  too new for TIM-VX v1.2.22's NBG parser, but we never need to load
  stock `.nb` — we ship our own compiled via CompileToBinary).
- ORT 1.22's VSI-NPU EP would hit the same query at runtime (on top of
  its own separate cross-compile bugs).

Stock firmware avoids all of this because `libnnsdk.so` was compiled
against the exact chip/driver combo it ships with. The `vsi_nn_*` API
surface (which our NPU probe shows works) is the one that initializes
cleanly.

**Viable NPU paths remain:**
1. Call `libnnsdk.so` directly from Rust FFI (same as stock). Proved
   reachable in `waved-npu-probe`. Requires `.nb` files, which require
   VeriSilicon Acuity Toolkit (NDA) to produce from ONNX.
2. Call `libovxlib.so` directly via `vsi_nn_*` API. Same `.nb` /
   Acuity requirement.
3. Newer TIM-VX from upstream main (post-v1.2.22) — may have updated
   the kernel-contract query. Untested.

**Dead paths:**
- TIM-VX v1.2.22 runtime — kernel contract mismatch.
- ORT 1.22.0 + VSI-NPU EP — cross-compile bugs + same runtime issue.

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
