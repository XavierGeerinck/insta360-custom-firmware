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
3. Recover only the bounded format and loader contracts needed to validate
   stock graphs and investigate alternate graph representations. This is now
   an evidence-gated research program under
   [`docs/npu/`](../../docs/npu/README.md), not product-ID patching or raw
   command mutation.

### Integration path for waved (SP#4 target)

The production target remains a Rust-owned inference wrapper with a CPU
fallback, but no arbitrary-ONNX NPU route is proven yet. The active research
order is:

1. recover and validate the exact stock runtime and NBG contracts;
2. determine whether the private export-data graph path can represent a
   minimal safe graph;
3. obtain or identify a genuine Wave-targeted compiler if compiled NBG is
   required;
4. characterize only the operations needed by a streaming denoiser;
5. integrate after quality, deadline, stability, cleanup, and fallback gates
   pass.

DeepFilterNet remains a CPU/reference candidate, not a promise that its full
ONNX graph maps to this NPU. ONNX Runtime with VSI-NPU and direct TIM-VX are
candidate front ends only after their target/compiler and operation coverage
are demonstrated on the Wave.

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

The live-reference v1.1.6.6 image has no dedicated NS/dereverb model in
`/model/`. Firmware v3 adds AES/ANS model-loading APIs in
`libmodel_infer.so`, but PoC 1 static analysis shows that DSO is a
CPU/MNN-style interpreter: it has no NPU/OpenVX/GAL dependencies or imports,
and the signal-processing caller consumes a 513-float ANS mask in its CPU
audio pipeline. It is a useful workload and quality reference, not an NPU
access path. See the
[`backend map`](../../docs/npu/abi/backend-map.md#firmware-v3-audio-api).

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

### Direct inference proof through stock libnnsdk (validated 2026-07-23)

The context proof above now has a full inference control. The
`waved-npu-probe --run-stock-ctc` path calls the exact Wave
`libnnsdk.so` exports recovered from the stock `libasr_reverb.so`
caller:

```text
aml_module_create
aml_module_input_set
aml_module_output_get  (perfMode 1 = run, 2 = retrieve)
aml_module_destroy
```

It uses the unmodified firmware graph
`/model/asr_de/de_online/ctc_online.nb` (SHA-256
`5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66`).
The graph header identifies product `0x1000000e`, matching this Wave.
Runtime tensor metadata and the observed result were:

```text
input:  input/output[0], FP16 [256, 16, 1], 8,192 bytes
output: uid_20000_sub_uid_1_out_0, FP32 [5200, 16, 1], 332,800 bytes

iteration 1: 3.501 ms
iteration 2: 2.184 ms
iteration 3: 2.263 ms
output sha256 (all runs):
e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4
aml_module_destroy: 0
```

The input is deterministic and zero-filled. The probe validates the
retrieved tensor count and byte length against NBG metadata, rejects an
all-zero output, and requires the full output hash to match across all
iterations. Two complete invocations passed. The existing `waved`
process remained PID `14049`; the observed `nanoqFreq` (`838860800`) and
`enableNN` (`255`) values were unchanged.

This proves that our own process can load, execute, and retrieve output
from a Wave-targeted NBG through the stock NPU stack. It does **not**
prove that the available host SDK can compile a custom graph for this
chip.

The custom two-input add graph makes the remaining boundary explicit:

| NBG | Version | Product field | Device result |
|-----|---------|---------------|---------------|
| host `sample-add.nb` | `0x0001001e` | `0x10000020` | rejected |
| stock `ctc_online.nb` | `0x00020000` | `0x1000000e` | executes |

Direct loading of the add graph reaches `aml_module_create` but fails
with `hardware version is not in product list`. Therefore the runtime
FFI and device access are solved; the custom-model blocker is the lack
of a Wave-targeted VIP9000 compiler/archmodel.

Reproduce without replacing a service or model:

```bash
cross build -p waved-npu-probe --release \
  --target aarch64-unknown-linux-gnu
adb push target/aarch64-unknown-linux-gnu/release/waved-npu-probe \
  /data/waved/waved-npu-probe.poc
adb shell 'LD_LIBRARY_PATH=/data/waved/lib:/usr/lib \
  VIVANTE_SDK_DIR=/usr \
  /data/waved/waved-npu-probe.poc --run-stock-ctc'
```

### Reproducible NPU lab baseline (validated 2026-07-28)

The direct-inference control now runs through a source-controlled host
harness rather than an ad-hoc ADB command. The harness pins the probe,
`libnnsdk.so`, `libovxlib.so`, and model hashes; records bounded pre/post
state; refuses mismatches before execution; recovers the native exit status;
and compares semantic results while ignoring timing, PID, uptime, and pointer
variation.

Two new process lifecycles passed with definition SHA-256
`9cf0a286874a1f2062383977b9a26136efba12e768ac14f4e26eef9b47e1df6a`.
All six iterations returned the same 332,800-byte output digest shown above,
and both `aml_module_destroy` calls returned zero. The observed iteration
times were:

```text
control 1: 8.198, 2.241, 2.290 ms
control 2: 3.539, 2.155, 2.160 ms
median: 2.266 ms (descriptive, not a benchmark)
```

On this boot the read-only galcore values were:

```text
nanoqFreq=838860800
enableNN=255
recovery=1
```

They were identical before and after both controls. ADB remained responsive,
the required hashes were unchanged, `/dev/galcore` remained present, and the
bounded holder scan found no holder at either snapshot point. The empty
holder result is point-in-time evidence, not a claim that stock services
never use the NPU.

The full reproducibility procedure, exact artifact sizes and hashes, raw-log
digests, non-claims, and safety boundary are tracked in:

- [`docs/npu/README.md`](../../docs/npu/README.md)
- [`docs/npu/reproduction.md`](../../docs/npu/reproduction.md)
- [PoC 0 lab notebook](../../docs/npu/lab-notebook/2026-07-28-poc0-reproducible-lab.md)
- [live device manifest](../../docs/npu/manifests/wave-live-v1.1.6.6.json)

This strengthens the inference proof; it does not change the custom-compiler
blocker described below.

### Runtime ABI and backend map (PoC 1, 2026-07-28)

Host-only analysis of four exact firmware families recovered the narrow
Wave-facing runtime contract and locked every relied-upon AArch64 structure
size and offset in Rust tests. The exact bytes of the seven project-facing
wrapper functions are identical across the corpus:

```text
aml_module_create
aml_module_destroy
aml_module_input_set
aml_module_output_get
aml_util_getInputTensorInfo
aml_util_getOutputTensorInfo
aml_util_freeTensorInfo
```

The public Amlogic header is not layout-compatible with the private Wave
create/input/output ABI. In particular, stock callers allocate a
`0x8e0`-byte create configuration; the first fields are:

```text
+0x00 typeSize = 0x8e0
+0x04 model byte length
+0x08 file path pointer
+0x10 in-memory model pointer
+0x18 model type
+0x1c NBG source/type
```

The active hardware dispatch is:

```text
/dev/galcore -> VIPUNIFY (this Wave)
/dev/vipcore -> VIPLITE
/dev/adla0   -> ADLA
/dev/mali0   -> eligible TFLite GPU/CPU delegate fallback
```

VIPUNIFY has two graph families:

| `nbgType` | Source | Construction |
|---:|---|---|
| 0 | NBG file | Parse compiled NBG, create one OpenVX NBG node, bind IO, verify |
| 1 | NBG memory | Same compiled-NBG path |
| 2 | private export-data file | Parse nodes/tensors/connections/parameters, construct OpenVX graph, verify |
| 3 | private export-data memory | Same reconstructed-graph path |

The export-data path is a genuine second custom-model research route. PoC 1
recovered its section markers and construction lifecycle but not a complete
safe input grammar, so no handcrafted graph has been executed. The
compiled-NBG route still requires a real `0x1000000e` target; changing a
foreign NBG’s product field remains invalid.

One error detail affects every future binding: `aml_module_input_set`
discards its backend status and returns zero after dispatch, while
`aml_module_output_get` can encode some invalid-context failures as a
non-null integer-like pointer. Project code must validate context, tensor
metadata, returned counts and lengths, output content, cleanup, and the
external watchdog rather than trusting one wrapper return value.

Full reproducible evidence:

- [PoC 1 ABI index](../../docs/npu/abi/README.md)
- [recovered prototypes and layouts](../../docs/npu/abi/prototypes.md)
- [stock CTC call graph](../../docs/npu/abi/stock-ctc-call-graph.md)
- [backend map](../../docs/npu/abi/backend-map.md)
- [error taxonomy](../../docs/npu/abi/error-taxonomy.md)
- [PoC 1 notebook](../../docs/npu/lab-notebook/2026-07-28-poc1-runtime-abi.md)

The repository’s current `third_party/TIM-VX` checkout is exactly commit
`702715d714277376d7ff466b7b72833ff57564c1` (commit date 2026-03-30).
Sections below that say “TIM-VX v1.2.22” preserve results from the earlier
checkout used for those experiments; they must not be read as the identity
of the current tree. A later PoC must rerun any TIM-VX claim against the exact
current commit before graduating it.

### NBG v2 format and stock corpus (PoC 2, 2026-07-28)

A strict host-only parser now classifies every available stock graph without
calling a firmware library or executing model bytes. The configured corpus
contains 137 stock files, the exact live-tested CTC reference, and one
foreign-target legacy add negative control:

```text
139 files
3,785,683,152 bytes

138 × version 0x00020000, target 0x1000000e
  1 × version 0x0001001e, target 0x10000020
```

A public VIPLite 2.0.0 linker header at Ameba Pro2 BSP commit
`b83b89f3842cf2938c8ce8adec9dee1864b7d72a` independently matches the exact
Wave v2 accesses recovered in PoC 1. The Wave envelope is:

```text
48-byte nbg_format_header
12-byte nbg_sub_header
18 or 20 × 44-byte nbg_section_header
typed section payloads
final 512-byte runtime connection region
```

Every v2 graph uses format class 0, one core, 32-bit patch addresses,
192-byte NN commands, 192-byte TP commands, and 262,144 bytes of VIP SRAM.
Serialized memory-pool requirements range from 161,280 to 57,445,376 bytes.
No graph contains an AXI-SRAM section.

The 20 observed section types include input/output descriptors, layer and
operation metadata, NN/TP command sections, patches, PPU instructions,
dynamic I/O, load state, coefficient data, their tables, network info,
memory metadata, and checksums. Public names are now known, but PoC 2
semantically decodes only bounded tensor/network/memory/checksum metadata.
Accelerator commands, coefficients, relocations, state, and connection data
remain opaque and fingerprinted.

Tensor metadata across all configured graphs is:

```text
FP32   21 records
FP16  421 records
UINT8   3 records (foreign v1 negative control)
INT32  42 records
```

All 487 I/O records are public type `TENSOR`. All 484 v2 records use
quantization format `NONE`; this does not describe internal activation or
weight precision.

Each v2 graph stores five section checksums for NN, TP, coefficient data,
PPU instructions, and load state. All 690 values exactly equal standard
CRC-32 of their referenced section bytes. The separate header `crc_sum`
field is preserved but its coverage/algorithm is not yet reproduced.

For the exact live CTC hash, the static parser recovers the same FP16
`[256,16,1]` input and FP32 `[5200,16,1]` output sizes observed through
`libnnsdk.so`, plus a 167,680-byte memory-pool requirement and 256 KiB VIP
SRAM requirement.

This closes the stock-format inventory gate. It does not produce a
Wave-targeted graph, decode NPU instructions, or make target-byte patching
valid. Full evidence:

- [PoC 2 format result](../../docs/npu/format/README.md)
- [field map and confidence ledger](../../docs/npu/format/field-map.md)
- [generated corpus summary](../../docs/npu/format/corpus-summary.md)
- [PoC 2 notebook](../../docs/npu/lab-notebook/2026-07-28-poc2-nbg-corpus-format.md)

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

### eYs3D eCV5546 VIP9000 SDK — massive de-risking (2026-04-23)

The `eYs3D/eCV5546_rootfs` repo on GitHub ships the complete aarch64
VeriSilicon VIP9000 userspace SDK at versions **6.4.15.9** and
**6.4.18.5** under
`initramfs/prebuilt/vip9000sdk/<ver>/`. This includes everything we
concluded was missing above:

- `libNNVXCBinary.so` (87 MB) and `libNNGPUBinary.so` (33 MB) —
  pre-compiled kernel binaries for every NN op
- `libOvx12VXCBinary.so`, `libOvxGPUVXCBinary.so` — OpenVX 1.2 kernels
- `libArchModelSw.so` with **actual VIP9000 archmodel** (vs TIM-VX's
  GC7000-only prebuilt)
- `libOpenVX.so.1.3.0` (6.8 MB) — OpenVX runtime *built with the NN
  binary subsystem enabled*
- `libovxlib.so`, `libGAL.so`, `libVSC.so`, `libCLC.so`, `libSPIRV_viv.so`,
  `libGLSLC.so`, `libtim-vx.so` — full aarch64 Vivante stack
- `include/CL/cl_viv_vx_ext.h` — the OpenCL Vivante extension header
- Full `include/ovxlib/` tree — complete userspace API

Plus the companion `eYs3D/eCV5546_linux` kernel tree ships the galcore
**driver source** at 6.4.18.5 and 6.4.15.9 under
`drivers/media/platform/eys3d/vip9000/v<ver>/`. Its
`gc_feature_database.h` identifies our exact chip: **`vip9000nano-di+`**
(ChipID `0x9000`, Rev `0x9002`, ProductID `0x5090009`).

**Our chip matches.** License-wise, these are proprietary VeriSilicon
binaries that eYs3D publishes with their product's redistribution
permission — same binaries Insta360 ships on every Wave (just from a
different redistributor, complete rather than stripped).

### Why the SDK alone doesn't unblock us: user/kernel version skew

Attempting to use the 6.4.18.5 userspace against our device's 6.4.19
galcore kernel driver (tested 2026-04-23) produces consistent failures:

- Full 6.4.18.5 stack → `HAL user 6.4.18.886827 / HAL kernel
  6.4.19.946407` warning, then `vsi_nn_AddTensor` returns
  `VSI_NN_TENSOR_ID_NA` at every tensor create. Tensor-allocation ioctl
  between 6.4.18 libovxlib and 6.4.19 galcore is incompatible.
- Pure 6.4.18.5 libOpenVX with device libGAL/libVSC → hangs at context
  create.
- Device userspace with only 6.4.18.5 `libNN*Binary.so` blobs added →
  silent Compile fail. Reason via `LD_DEBUG=libs`: Insta360's libOpenVX
  **never dlopens** `libNNVXCBinary.so` — it was built without the NN
  binary subsystem. Dead code in their binary.

**Net**: the 6.4.18.5 SDK is complete and right for this chip family,
but *not* ABI-compatible with 6.4.19 galcore. We either:

1. **Replace galcore with 6.4.18.5 kernel driver.** Source is public in
   `eYs3D/eCV5546_linux`. Build as module, `rmmod galcore && insmod
   galcore.ko`. Then full 6.4.18.5 stack works end-to-end. Risk: NPU
   driver replacement on a running device; if it misbehaves, might
   require reboot. Mitigation: galcore is a loadable module, not
   statically compiled.
2. **Obtain matching 6.4.19 userspace from Insta360/Amlogic.** Business
   ask — they clearly have it since stock ASR works with full NN binary
   loading.
3. **Find another public 6.4.19 BSP.** Not yet surveyed; Amlogic's own
   GitHub / linux-meson trees are candidates.

**What this means for "own the models"**:

| Path | State | Notes |
|------|-------|-------|
| Host AoT compile, device run (OSS stack) | ❌ blocked | Host SDK's archmodel is GC7000-class + CPU simulator, no VIP9000 |
| Device-side JIT compile via libOpenVX / libVSC | ❌ blocked | Device ships the compiler (`libVSC.so`) but **zero shader sources** (`.vx`, `.cl`, `cl_viv_vx_ext.h`) — stripped from image because stock only loads pre-compiled NBGs |
| `libnnsdk.so` with chip-native SDK | ❌ NDA | Needs VeriSilicon Acuity Toolkit for VIP9000 — proprietary |
| TIM-VX evk-board SDKs (A311D, S905D3) | — (untested) | Different NPU IP family from VIP9000 — very unlikely to produce compatible NBGs |

**Root cause** across all three blocked OSS paths: the VIP9000-specific
bits (archmodel data + shader sources + chip-native kernel compilers) only
ship inside proprietary BSP drops from VeriSilicon / Amlogic / Insta360.
The device's runtime libraries (`libOpenVX.so`, `libVSC.so`,
`libovxlib.so`) know the chip, but the *data files* they need to compile
new graphs for it aren't on the device.

**Unblocking action is business/external, not technical:**
- Acuity Toolkit license from VeriSilicon (covers archmodel + compiler)
- Amlogic A113X2 BSP drop (probably includes archmodel + shader source
  tree under Amlogic NDA)
- Insta360 engineering handoff — they built the stock firmware and have
  both.

Until one of those lands, NPU-targeted models require pre-compiled NBGs
produced externally. Implementation-side SP#4 work (Rust wrapper, NS
integration, acceptance tests) can still proceed against a CPU backend
— see `TODO.md` for the status matrix.

### Path 1 (replace galcore) — feasibility survey (2026-04-28)

After deciding on Path 1 (rebuild galcore at 6.4.18.5 to match the
public userspace), surveyed what's actually needed.

**Device state captured:**

- Kernel: `Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT Fri Nov 28 20:02:25 CST 2025 aarch64`
  built with `aarch64-none-linux-gnu-gcc 10.2.1` (Arm Toolchain 10.2-2020.11).
- `/proc/config.gz`: pulled to `/tmp/waved-galcore-survey/config`. Key flags:
  `CONFIG_MODULES=y`, `CONFIG_MODULE_UNLOAD=y`, `CONFIG_MODVERSIONS=y`,
  no `CONFIG_MODULE_SIG*` (modules are not signed). `CONFIG_LOCALVERSION=""`,
  `CONFIG_LOCALVERSION_AUTO=y` — the `-ab575` suffix is git-derived from the
  Amlogic build tree (likely the build number, given the path below).
- `/lib/modules/5.4.242/build` and `Module.symvers` are **not** on the device.
  `/usr/src` and `/usr/include/linux` absent.
- Stock galcore.ko pulled. `vermagic=5.4.242-ab575 SMP preempt mod_unload modversions aarch64`.
  Banner string: `$VERSION$6.4.19:946407$` → exact version is **6.4.19.5.946407**
  (5-component, not 4). Module is loadable+unloadable (refcount 0, taint G/O).
- `dmesg` shows `chipRevision mismatch, database chipRevision=0x9001 register
  read chipRevision=0x9002` — stock galcore's chip database is slightly stale
  for our die rev, but it works anyway.

**Stock galcore originates from an Amlogic Yocto-style PSP build:**

`strings galcore-stock.ko` reveals source paths under
`/home/jenkins/workspace/PSP_Releasebuild/575/projectCode/SOC/output/a5_av400_psp_a64_release/build/npu-1.0/`
— the `575` matches the `-ab575` LOCALVERSION suffix, and `a5_av400` is the
Amlogic A5 reference platform. The platform file used was
`hal/os/linux/kernel/platform/amlogic/gc_hal_kernel_platform_amlogic.c`.
DT compatible string: `amlogic, galcore`. Imported clocks: `npu_axi_clk`,
`npu_core_clk`. IRQ 19.

**eYs3D source only ships an `eys3d` platform file**, not Amlogic's. So
"build galcore 6.4.18.5" splits into two sub-paths:

- **Path 1A**: take eYs3D's 6.4.18.5 source, swap `platform/eys3d/*` for an
  Amlogic platform file ported from a public Amlogic galcore source. Two
  candidate sources for the Amlogic platform file:
  - `aml-streambox/nanoq` (Yocto migration of `aml-comp/hardware/aml-5.4/amlogic/npu/nanoq`),
    galcore version 6.4.11.2, full Amlogic platform file (22 KB).
  - `khadas/android_vendor_amlogic_npu` (Goldfinger branch), 6.3.2.5 — older.
  - `khadas/android_common_driver_modules_npu_driver` (VIM4), 6.4.8.7.7.1.1 — closest.
  Risk: platform-file API may have shifted between 6.4.11 → 6.4.18 (struct
  layouts in `gckPLATFORM_OPERATIONS`); needs source diff.

- **Path 1B**: forget version-matching userspace. Use `aml-streambox/nanoq`
  galcore 6.4.11.2 directly (it has Amlogic platform built in), then find a
  matching 6.4.11.2 userspace. Most likely source: `aml-streambox/aml_npu_common`
  (sibling repo, same Yocto migration). Risk: 6.4.11.2 userspace doesn't
  ship the same VIP9000Nano-DI+ archmodel as our chip needs.

**Same chip family is supported by these public galcore sources** (found
via `vip9000nano-di+` / VIP9000NANODI_PLUS_PID0X1000000B grep across GitHub):
`STMicroelectronics/gcnano-binaries`, `eYs3D/eCV5546_linux`,
`radxa/allwinner-bsp`, `D-Robotics/x5-kernel`, `eswincomputing/linux-stable`,
`YuzukiHD/TinyVision`. None Amlogic-specific, so platform code would still
need adaptation, but the **chip-feature database** (archmodel data) for our
exact die rev is broadly available.

**The CONFIG_MODVERSIONS=y constraint is the harder of the two issues.**
Every kernel symbol galcore imports needs a CRC matching the device's
`Module.symvers`. Without that file or the device kernel's full build tree,
options are:

1. Source Insta360's GPL kernel release (mandatory under GPL-2.0 — they
   distribute galcore as a module, so `-ab575` kernel source must be
   obtainable). Ask Insta360 / look for a published GPL drop.
2. Reconstruct: build kernel 5.4.242 from Amlogic upstream + apply
   `CONFIG_LOCALVERSION="-ab575"` + use device's `/proc/config.gz`. If the
   config matches, symbol CRCs match (CRCs are deterministic from preprocessed
   source + config). Risk: any out-of-tree patches Amlogic/Insta360 applied
   that touched exported symbols would shift CRCs.
3. `insmod --force` — the kernel rejects cross-vermagic by default, but
   `modprobe --force-modversions --force-vermagic` exists. Will likely
   panic at first symbol the kernel can't reconcile.

**Net assessment of Path 1:**

- The galcore source side is workable but not trivial: need to merge the
  eYs3D 6.4.18.5 codebase with an Amlogic-style platform file.
- The kernel build side is the bigger blocker: `CONFIG_MODVERSIONS=y`
  without device kernel source effectively forces us into option 1 (Insta360
  GPL drop) or option 2 (rebuild upstream + pray on CRCs). Option 2 has
  decent odds — kernel 5.4.242 is the LTS upstream version and Amlogic
  upstreams most of their meson tree.

**Two adjacent paths worth considering before committing further to Path 1:**

- **Path 4 (userspace ABI shim).** The mismatch is "user 6.4.18.886827 vs
  kernel 6.4.19.946407 → `vsi_nn_AddTensor` ioctl mismatch". Inspect
  `gc_hal_driver.h` ioctl numbers/structs in eYs3D 6.4.18.5 vs whatever
  6.4.19 source we can find. If the delta is small (one struct field added),
  patching the userspace `libGAL.so` is far cheaper than rebuilding the
  kernel module.
- **Path 5 (Insta360 GPL request).** Insta360 distributes `galcore.ko` as
  a binary under GPL-2.0 (eYs3D Makefile carries SPDX `GPL-2.0-only`).
  Under GPL we can request the kernel source corresponding to
  `5.4.242-ab575`. This is the cleanest unblock — it gives us the build
  tree (Module.symvers), a canonical Amlogic platform file matching our
  device, and a galcore source at exactly 6.4.19.5.946407 which removes
  all skew.

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
