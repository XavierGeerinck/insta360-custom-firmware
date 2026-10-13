# Compute Hardware — CPU, NPU, DSP

Reference for non-CPU compute resources on the Insta360 Wave (Amlogic A113X2,
AV400 audio platform).

## Summary

| Resource | What it is | Stock firmware uses it for | Suitable for waved |
|----------|------------|----------------------------|--------------------|
| 4× Cortex-A55 @ up to 2.016 GHz, NEON + `asimddp` + `asimdhp` | CPU complex | Everything the NPU/DSP doesn't handle | AEC, beamforming, DOA, orchestration |
| **Vivante VIP NPU** (GC-class, rev 0x9002, npu_version 6) | Dedicated neural accelerator | Target-native ASR/diarization NBGs | **Production integration parked 2026-10-12** (see below). Erf, native FP16 Conv and quantized U8 FC run; general models remain unqualified, and measured gains are small |
| HiFi-class audio DSP at `/dev/audiodsp0` | 8 MB firmware region, chr dev 272 | Unknown (likely stock AEC / NR / DRC) | Out of scope unless we get Cadence/Tensilica toolchain |

## Vivante NPU

### Status: parked (2026-10-12)

Noise reduction is not worth the NPU's cost today. What decided it, all
measured on the Wave:

| Workload | CPU | NPU |
|---|---|---|
| Stock CTC model, 41 M MACs, fp16, vendor-compiled (PoC 15A) | 4.66 ms, stock ONNX Runtime 1.15.1, default thread pool | 2.79 ms `Run()`; 4.49 ms with output copy, 5.15 ms with fp16→fp32 conversion |
| U8 `[481]` Erf lookup (PoC 13G) | 3.3 µs | 68 µs `Run()`, 86 µs with copies |

- Even a vendor-compiled model only ties the CPU end to end. The earlier
  custom Conv/MatMul controls used FP32 weights, which REA now proves this
  NN checker rejects when an output tensor is present. PoC 16 compiled and
  ran FP16 Conv and quantized U8 FC; full denoising models and their
  end-to-end cost remain unqualified.
- DeepFilterNet's 84 % of a core (8.5 ms per 10 ms hop, tract, fp32) is far
  above its arithmetic: DeepFilterNet3 publishes ≈ 0.35 GMAC per second of
  audio, ≈ 3.5 M per hop, which the rate above puts near 0.4–1.6 ms. The
  runtime looks like the bottleneck, so the CPU is tried first (TODO C13).
- The NPU earns its keep on large, continuous models (stock ASR,
  diarization, punctuation), which waved doesn't run on the device.

Research continuation and remaining routes:

1. **E1 (recovered 2026-10-13).** REA recovered the exact
   OpenVX FP32-weight rejection and conditional NN/TP format paths. Both
   firmware Conv/FC admission tables contain FP16/FP16 and asymmetric
   U8/U8. See
   the [REA notebook](../../docs/npu/lab-notebook/2026-10-13-rea-op-dtype-recovery.md).
2. **E2/E3 (two controls passed 2026-10-13).** FP16 FIR Conv `[16,1,1,1]`
   and U8 FC `16→4` exported native VPMN v2 NBGs for `0x1000000e`;
   compile logs select `VXNNE_OPERATION_TARGET_NN`. Each ran once with
   all output bytes matching its CPU oracle, canaries intact and captured
   health unchanged. See [PoC 16](../../docs/npu/lab-notebook/2026-10-13-poc16-native-fp16-conv-u8-fc.md).
3. **ONNX native backend (PoC 17 passed 2026-10-13).** Actual ONNX
   initializers now lower into the two fixed native profiles. Two FP16 Conv
   models and two U8 QLinearMatMul models compiled on NN and returned all
   CPU-oracle bytes, including changed weights, Conv bias and a non-symmetric
   FC layout control. The explicit `wave-native` session requires the exact
   qualified package; strict mode creates no CPU session, and hybrid
   failure latches CPU fallback without retry. All eight staging roots were
   removed and captured health stayed unchanged. See the
   [native backend guide](../../docs/npu/onnx/native-backend.md) and
   [PoC 17 notebook](../../docs/npu/lab-notebook/2026-10-13-poc17-onnx-native-backend.md).
4. **Remaining model-specific work.** Other shapes, multi-layer graphs,
   additional operator/dtype pairs and one-weight serializer differentials
   need separate qualification. The two first-run timings are correctness
   observations, not performance measurements.
5. **Toolchain requests (need the owner to send them).** Sunplus C3V "NPU
   Toolkit" page offers a vSimulator SDK CL1307462B (2026-04-09) and Acuity
   6.51.16 on request; Amlogic or `ML_Support@verisilicon.com` for the A113X2
   (`VIP9000NANOS_PID0X1000000E`) SDK. Either pairs with public acuitylite
   6.57.1 (ovxlib 1.2.30).
6. **E0, Khadas `aml_npu_sdk`.** Public; its Acuity device file
   `VIPNANOQI_PID0X1E` targets `VIP9000NANOS_PID0X1000000E`, but its 6.4.8
   simulator writes NBG v1 (`0x0001000F`). The earlier rejection
   (`sample-add.nb`) failed on the product field first, so whether the driver
   loads a v1 file with the right product is unknown. It is a simulator-path
   export, which the lab rules exclude for Acuity 6.30.22: needs the owner's
   explicit OK.

No fit found: Mesa Teflon (etnaviv instead of galcore, VIPNano-QI/SI+ and
RK3588 only, no NBG); Synaptics SyNAP and Realtek Ameba document only their
own chips; NXP i.MX has no public x86 NBG compiler; TI, MediaTek and Rockchip
have no Vivante NPU.

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

### Runtime-to-driver trace (validated 2026-07-28)

Two metadata-only traces of the exact stock CTC control observed the same
software-visible transaction:

```text
ioctl request:       30000
HAL interface:       376 bytes
hardware type:       4 = VIP (adjacent public enum)
core:                0
calls per run:       73
syscall/HAL failures: 0
```

Graph creation produced 46 initialization/allocation calls. Each of three
executions produced exactly:

```text
COMMIT → EVENT_COMMIT → USER_SIGNAL
```

Graph teardown produced 18 unlock/release/event calls. The runtime opened
`/dev/galcore`, probed absent `/dev/galcore1`, and retained the primary
descriptor until normal process exit. Both runs preserved the known
332,800-byte output digest, returned cleanup zero, and left captured NPU
parameters and artifact hashes unchanged.

The tracer reads only the fixed driver envelope/interface prefix. It does
not capture addresses, command unions, buffers, model data, tensors, or
audio. See the
[`PoC 3 trace report`](../../docs/npu/trace/README.md) and
[`runtime sequence`](../../docs/npu/trace/runtime-sequence.md).

### Custom graph compiler and execution status (updated 2026-10-13)

The observed custom-model paths have different admission boundaries:

| Operation/path | Node selection | Graph verification | NBG export | Execution |
|---|---:|---:|---:|---:|
| Firmware Erf LUT/user kernel | OpenVX pass | Pass | Pass | Pass |
| Add built-in | OpenVX pass | Not tested | Fail | Not reached |
| MatrixMul built-in, FP32 July control | OpenVX pass | Fail | Fail | Not reached |
| Conv2d built-in, FP32 July control | OpenVX pass | Fail | Fail | Not reached |
| Conv2d built-in, FP16 PoC 16 | NN target pass | Pass | Pass | Exact 32-byte oracle pass |
| FullyConnected built-in, asymmetric U8 PoC 16 | NN target pass (optimized to Conv) | Pass | Pass | Exact 4-byte oracle pass |
| ONNX FP16 Conv and U8 QLinearMatMul, PoC 17 fixed profiles | NN target pass | Pass, all four | Pass, all four | Four one-run byte-exact oracle passes |
| SignalFrame source kernel | CPU fallback after shader failure | Fail | Fail | Not reached |

The first successful artifact was generated by repository-controlled TIM-VX source
through the exact Wave stack:

```text
graph:       UINT8 [4] -> Erf -> UINT8 [4]
NBG:         VPMN v2
target:      0x1000000e
bytes:       5,840
sha256:      6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee
checksums:   3 / 3 valid
VIP SRAM:    43,520 bytes
```

It executed once with zero input and once with nonzero input:

```text
[0,0,0,0]     -> [0,0,0,0]
[0,1,2,255]   -> [0,1,1,1]
```

The nonzero runner initialized its output buffer to `0xa5`, so the returned
bytes are not an untouched allocation. Both runs were exactly once, bounded
by native and host watchdogs, and left the captured device health unchanged.

The live hardware-capability query returned:

```text
EVIS1=0
EVIS2=1
subgroup size=4
VA40=0
stream processor=0
eco ID=0x04000000
customer/target ID=0x1000000e
```

The exact backend tables are:

```text
default:      SP(5), EVIS(4), CL(3), VX(2), CPU(1)
VX-first:     SP(4), VX(3), EVIS(2), CL(1), CPU(0)
```

Erf is a VX-first operation, explaining why OpenVX won despite EVIS2
availability.

The firmware contains EVIS/OpenCL source resources inside `libovxlib.so`, but
its `libOpenVX.so` dynamically requires `libCLC.so` to compile them. No exact
CLC is installed. A `6.4.18:886827` candidate relocated and resolved all
three required compiler symbols against the Wave's `6.4.19:946407` stack,
but an isolated compile reached invalid kernel references and aborted during
teardown. That CLC is quarantined and must never have its compiler entry
points called again.

### Learned Erf-mask noise suppression (validated 2026-07-29)

The narrow Erf route now executes a real learned noise-suppression partition,
not only a four-element numerical control:

```text
CPU: causal STFT, noise estimate, learned affine logits
  -> Wave NPU: UINT8 [481] Erf
  -> CPU: learned candidate + DFN candidate + causal selector + synthesis
```

The exact 5,840-byte NBG has SHA-256
`96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94`.
It was produced identically by two independent compile-only processes. Six
qualification vectors matched 2,886/2,886 CPU-oracle bytes, and a learned
fixture vector returned 63 distinct values.

The complete 12-second run executed 1,201 frames:

| Result | Value |
|---|---:|
| NPU/CPU tensor differences | 0 / 577,681 bytes |
| NPU/CPU waveform differences | 0 / 576,000 samples |
| NPU-derived candidate selected | 803 / 1,201 frames |
| Mean / maximum graph time | 91.0167 / 1,682 µs |
| Median SI-SDR improvement | +4.8443 dB |
| Noise-only attenuation | 9.8283 dB |

A separate 10,000-execution fixed-runner soak covered 4,810,000 bytes with
all ten rolling oracle checkpoints, output canaries, and pre/post health
checks passing. Its mean graph time was 84.8758 µs and maximum was 1,649 µs.

PoC 13G then measured the same U8 `[481]` transform on the Wave CPU and NPU.
Over 10,000 paired steady-state frames:

| Path | Mean | p50 | p95 | p99 |
|---|---:|---:|---:|---:|
| Cortex-A55 LUT | 3.303 µs | 3.333 µs | 3.458 µs | 3.500 µs |
| NPU `Run()` | 68.330 µs | 65.750 µs | 74.458 µs | 86.083 µs |
| NPU copy/run/read | 86.338 µs | 83.625 µs | 93.000 µs | 106.959 µs |

For Erf alone, CPU is 20.686 times faster than NPU `Run()` and 26.137 times
faster than the complete NPU boundary by ratio of means. This is fixed-cost
domination for a tiny lookup, not a general CPU-versus-NPU conclusion.
Larger partitions need independent benchmarks.

PoC 14 attempted that larger boundary. A host-qualified
`Multiply -> Add -> Erf` partition retained all denoiser quality gates, but
the exact Wave elementwise checker rejected its first Multiply input pair:

```text
ASYM UINT8, SYMM PC INT8
```

The pinned checker source has 59 ADD/Multiply pairs and no per-channel pair.
The failure occurred before backend selection or NBG emission; no graph ran,
health remained unchanged, and coefficient-only retries were skipped. The
affine partition therefore remains blocked. PoC 16 separately proves a
native FP16 FIR Conv; expansion of the learned affine model into Conv
remains unqualified.

This proves the fixed partition and an offline Python/lab hybrid pipeline.
It does not prove arbitrary ONNX, broader Conv/MatMul coverage, full-model NPU
residency, or the pending Rust `NpuInfer` service integration and end-to-end
real-time/thermal qualification.

Full evidence:

- [PoC 5 compiler notebook](../../docs/npu/lab-notebook/2026-07-28-poc5-custom-compiler.md)
- [PoC 6/7 execution notebook](../../docs/npu/lab-notebook/2026-07-28-poc6-7-custom-nbg-execution.md)
- [PoC 8 audio boundary](../../docs/npu/lab-notebook/2026-07-28-poc8-audio-denoiser-boundary.md)
- [PoC 9 architecture/profile evidence](../../docs/npu/profile/README.md)
- [PoC 13 Erf-mask result](../../docs/npu/models/erf-mask/README.md)
- [PoC 13 completion audit](../../docs/npu/models/erf-mask/completion-audit.md)
- [PoC 13G CPU/NPU benchmark](../../docs/npu/lab-notebook/2026-07-29-poc13g-wave-cpu-vs-npu.md)
- [PoC 14A affine host contract](../../docs/npu/lab-notebook/2026-07-29-poc14a-affine-host.md)
- [PoC 14B compiler negative](../../docs/npu/lab-notebook/2026-07-29-poc14b-affine-compiler-negative.md)
- [PoC 16 native FP16 Conv and U8 FC](../../docs/npu/lab-notebook/2026-10-13-poc16-native-fp16-conv-u8-fc.md)

### Offline compiler acquisition status (validated 2026-07-28)

PoC 9D statically qualified the Radxa-linked Allwinner
`ubuntu-npu:v2.0.10.2` package without loading its OCI image, importing a
native extension, executing a compiler, or contacting the Wave.

Positive static results:

- the vendor image contains
  `VIP9000NANOS_PID0X1000000E.config`;
- its five identity values exactly match the PoC 9A Wave database row;
- standalone x86-64 `vcompiler` and `vcCompiler` binaries genuinely consume
  `VIV_GPU_FILE`;
- the five-field parser, its three call sites, and its database lookup are
  byte-locked in `vcompiler`.

Closed compatibility result:

- the two consumers are GLSL/OpenCL shader compilers, not graph-to-NBG
  compilers;
- their version is `6.4.18:967862`, below the no-kernel `>=6.4.22` gate;
- Acuity 6.30.22 uses `VSIMULATOR_CONFIG` and an embedded
  `6.4.6:769531` simulator SDK to reach `network_binary.nb`;
- the native `gen_nbg` input/output and allocation-ownership ABI is not
  completely proven;
- fixed Conv compilation using this unqualified host package remains prohibited.

The next valid acquisition is a dedicated x86-64 Vivante no-kernel SDK
`>=6.4.22` whose OpenVX/ovxlib NBG path directly consumes `VIV_GPU_FILE`,
with ovxlib `>=1.2.26` and a complete hash-pinned dependency/licence/ABI
set. See the
[PoC 9D notebook](../../docs/npu/lab-notebook/2026-07-28-poc9d-compiler-acquisition-static-qualification.md).

### ONNX audit and CPU fallback (validated 2026-07-28)

PoC 9E/9F adds a separate host-only ONNX control plane. It:

- bounds and validates ordinary ONNX models;
- recursively inventories control-flow subgraphs;
- computes deterministic executable graph signatures;
- emits exact CPU/NPU placement reasons;
- runs a deterministic ONNX Runtime CPU oracle;
- implements `audit`, `cpu`, `hybrid`, and `strict` modes.

The current policy is bound to the unqualified compiler-profile manifest and
contains no approved NPU signatures. Consequently:

```text
cpu:     one CPU session, zero NPU attempts
hybrid:  one CPU session, zero NPU attempts
strict:  refuse before session construction
```

A fixed Conv/Sigmoid spectral-mask control produced byte-identical CPU and
hybrid output archives. The real Silero VAD model passed non-executing
admission with 51 graphs and 689 total nodes; all placement remains CPU.

This establishes general host ONNX admission and a fallback API, not ONNX
execution on the Wave NPU. See the
[PoC 9E/9F evidence](../../docs/npu/onnx/README.md).

### Open NBG serializer recovery (PoC 12A, validated 2026-07-28)

Because the exact no-kernel SDK is unavailable, PoC 12A pursued a
serializer-first host-static route against the firmware already on hand.
It produced four reproducible evidence layers:

```text
21 exact serializer/load exports
5 exact VPMN v2 byte-ownership maps
5 / 5 byte-identical unchanged round trips
1 finite English-CTC ONNX/NBG coefficient ledger
```

The static call evidence spans graph generation, network serialization,
section memory, weight/bias handling, NN/TP command and state emission,
patching, relocation, loading, and release. Twenty targets are complete
function bodies. `vxoBinaryGraph_SaveNetworkBinaryGraph` is the sole
manifest-approved four-byte AArch64 export alias into the
`SaveBinaryEntrance` path.

The lossless image is intentionally not a mutable writer. It retains original
bytes for every decoded, alignment, padding, and opaque range; reconstructs
only an unchanged image; and refuses a semantic-change marker. English,
German, French, and Japanese CTC graphs plus the generated Erf graph all
match their original size, every byte, and SHA-256 after reconstruction.

The adjacent English CTC ONNX weight `[512,4999]` and bias `[4999]` do not
occur under the finite direct/FP16/BF16/transpose/block/lane transform
registry in admitted NBG sections. The PoC 12A coefficient gate is
`not-ready`.
Coefficient packing, NN/TP instruction emission, relocation construction
and custom Conv generation through an open emitter remain unproven, as
does general ONNX-to-NBG compilation. PoC 16 later proved native FP16 Conv
generation through the exact vendor serializer.

See the
[PoC 12A notebook](../../docs/npu/lab-notebook/2026-07-28-poc12a-open-nbg-serializer.md)
and the
[format evidence index](../../docs/npu/format/README.md).

### Controlled serializer differential (PoC 12B/12C, validated 2026-07-29)

PoC 12B made exactly three compile-only calls through the stock Wave
serializer for one fixed constant-input U8 Erf graph. A1 and A2 used
`[0,1,2,3]`; B used `[0,1,2,4]`. A1/A2 are byte-identical 5,904-byte NBGs.
B has the same size and directory descriptors. The captured device returned
zero for all three calls, and cleanup restored the exact pre-run state. No
graph was loaded or executed.

The PoC 12B interpretation remains frozen at its reviewed finite registry,
which excluded section 20. Its top-level classification is therefore still
`multi-region-unresolved`.

PoC 12C separately combined those bytes with bounded analysis of the exact
Wave `libOpenVX.so` save path and the full corpus. It proves:

```text
header crc_sum
  reflected IEEE CRC-32, initial value 0
  section payloads in directory order
  header/directory/alignment gaps excluded
  section-40 checksum-value words treated as zero

dynamic I/O
  section 20 = data region
  section 26 = repeated u32 offset, u32 size
  offsets are region-relative and 64-byte aligned
```

All 138 v2 graphs match the recovered header checksum. All 111 graphs with
dynamic I/O contain both sections; 8,939 entries are positive, ordered,
non-overlapping, and in bounds, and all 83,024 bytes outside mapped entries
are zero. In the controlled graph, table entry `(0,4)` maps the source
constant to section 20. Replacing its fourth byte in an in-memory copy of A
and recomputing `crc_sum` reproduces B exactly.

This qualifies a read/validation codec and one exact reconstruction. It is
not an arbitrary dynamic-I/O writer, coefficient packer, operation encoder,
general ONNX compiler, or NPU execution result. See the
[PoC 12B notebook](../../docs/npu/lab-notebook/2026-07-29-poc12b-erf-constant-differential.md),
[PoC 12C notebook](../../docs/npu/lab-notebook/2026-07-29-poc12c-dyna-io-crc-profile.md),
and
[machine profile](../../docs/npu/format/dyna-io-crc-profile.json).

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

### Toolchain access (the remaining broad-model gap)

The Acuity toolkit is under NDA from VeriSilicon and usually reaches
licensees via the Amlogic BSP. To compile our own ONNX → `.nb` we need
one of:

1. **Acuity toolkit** from Amlogic/VeriSilicon — the native path. Check
   `/data`, `/system/vendor`, any SDK drops from Insta360 for a bundled
   copy; sometimes it ships inside the board support package.
2. **TIM-VX** (VeriSilicon's open-source tensor interface) plus the exact
   matching `6.4.19:946407` shader/program compiler components. TIM-VX graph
   construction, firmware Erf, native FP16 Conv and quantized U8 FC are
   proven. The earlier FP32 MatrixMul/Conv controls failed; additional graph
   coverage and EVIS source compilation remain unqualified. An ONNX Runtime
   or TFLite front end still needs compatible lowering for each operation.
3. Continue the bounded serializer-first route. PoC 12A proves exact
   unchanged representation and bounds the writer/load families; PoC 12C
   adds the header checksum and dynamic-I/O table codec. Coefficient packing
   and command emission remain unrecovered. The next gate is a deterministic
   semantic mapping corroborated across multiple controlled tensors or
   graphs—not product-ID patching or raw command mutation.

### Integration path for waved (SP#4 target)

The production target remains a Rust-owned inference wrapper with a CPU
fallback. General host ONNX admission is proven, but arbitrary ONNX-to-Wave-
NPU compilation is not. The active research order is:

1. preserve generated Erf, FP16 Conv and U8 FC as numerical controls;
2. use explicit `wave-native` only for exact PoC 17 qualified packages;
   keep the general ONNX path on deterministic CPU fallback;
3. extend the qualified TIM-VX route through separately checked models, or
   obtain a compatible broader compiler/serializer route;
4. build a model-specific graph from qualified dtype/operator pairs;
5. characterize only the operations needed by a tiny spectral-mask model;
6. keep CPU framing, overlap-add, monitoring, and fallback;
7. integrate only after quality, deadline, stability, cleanup, and fallback
   gates pass.

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

**Compiler quirk, now resolved more precisely**: this firmware ships no
`libCLC.so`. A host-side link shim was sufficient to produce
`libtim-vx.so`, and precompiled NBG import does not need CLC. It is incorrect
to infer that the compiler functionality is folded into `libOpenCL.so`:
live PoC 5E showed the exact source-kernel path dynamically loading three CLC
compiler entry points and failing because the library is absent. Context
creation and precompiled-NBG execution work; EVIS/OpenCL source compilation
does not.

No stock service needs to be stopped for the accepted PoCs. A point-in-time
holder scan is part of every preflight. If a holder exists, the experiment
stops for review rather than killing the process.

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

This established the stock compiled-NBG control. PoCs 5 through 8 later
narrowed the custom-compiler blocker: one generated Erf NBG now compiles and
executes, while general built-in operations and source-kernel compilation
remain blocked.

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
PoC 12C later adds strict dynamic-I/O table geometry. Accelerator commands,
coefficients, relocations, state, per-entry dynamic-I/O roles, and connection
data remain opaque and fingerprinted.

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
CRC-32 of their referenced section bytes. PoC 12C also reproduces and
enforces the separate header `crc_sum`: reflected IEEE CRC-32, initialized
to zero and updated over section payloads in directory order, with
section-40 checksum-value words treated as zero because the producer fills
them afterward. All 138 v2 values match.

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
| Host AoT compile, device run (OSS stack) | ❌ blocked | Host SDK emits foreign target `0x10000020`; it is a simulator control, not a Wave compiler. |
| Device-side compile via exact OpenVX/ovxlib | ⚠️ native primitives pass | Erf, FP16 Conv and U8 FC export for `0x1000000e` and execute correctly. Earlier Add/FP32 MatrixMul/Conv controls failed; broader models remain unqualified. |
| EVIS/OpenCL source-kernel JIT | ❌ blocked | `libovxlib.so` embeds sources, but exact `libCLC.so` is absent; a 6.4.18 candidate is ABI-incompatible and quarantined. |
| `libnnsdk.so` with chip-native SDK | ❌ NDA | Needs VeriSilicon Acuity Toolkit for VIP9000 — proprietary |
| TIM-VX evk-board SDKs (A311D, S905D3) | — (untested) | Different NPU IP family from VIP9000 — very unlikely to produce compatible NBGs |

**Updated boundary (2026-10-13):** target identity, graph construction and
native NN export are available for the qualified FP16 Conv/U8 FC controls.
FP32 weight rejection explains the earlier NN Conv boundary. The exact
compiler/program package needed for EVIS source JIT is still missing;
relocation coverage from 6.4.18 was insufficient for the 6.4.19 compiler ABI.
General model lowering needs further operator and graph qualification.

**External SDK routes for broader compiler coverage:**
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

This is retained as a historical feasibility survey, not an active or
authorized procedure. PoC 5F later showed user-space compiler ABI corruption
before any reason to replace the kernel driver, and the research safety
boundary prohibits module replacement without a separate explicit review.
No command in this section should be run as part of the accepted PoCs.

The earlier survey asked what would be needed to rebuild galcore at 6.4.18.5
to match the public userspace.

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
3. Forced module loading was considered and rejected. A version/CRC mismatch
   can panic the device and is outside the research boundary.

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

### TIM-VX compile/import path (updated 2026-07-28)

The April host-simulator path was only a partial map. The July PoCs
established which branches actually work:

```
HOST x86_64 simulator
  graph->CompileToBinary
  -> foreign target 0x10000020
  -> never execute

DEVICE aarch64 + exact Wave ovxlib/OpenVX/GAL
  fixed TIM-VX Erf graph
  -> CompileToBinary
  -> target 0x1000000e NBG
  -> ops::NBG import
  -> Graph::Compile / Graph::Run
  -> verified nonzero output
```

**Key moves:**
- Build TIM-VX for device with `-DTIM_VX_USE_EXTERNAL_OVXLIB=ON` and
  `-DOVXLIB_LIB=/path/to/device/libovxlib.so` — libtim-vx.so (2.2 MB, was
  8.6 MB bundled) delegates all `vsi_nn_*` calls to the **device's own**
  libovxlib, which has the correct kernel-contract ioctl for chip rev
  0x9002.
- Compile only through a fixed, source/ELF-policy-checked graph. Erf is the
  sole successful export family so far.
- Strict-parse and hash the untouched output before a separate execution
  review.
- Import the accepted NBG through `tim::vx::ops::NBG`; precompiled NBG
  execution needs no CLC.

**NPU identification** (from `VSI_NN_LOG_LEVEL=5`):
- product: **VIP9000Nano-S**, pid `0x1000000e`
- stock OVXLIB on device: **1.2.15** (our TIM-VX headers are 1.2.22 —
  close enough for the stable C++ surface we use).

**Runtime env requirement**: accepted probes pin `VIVANTE_SDK_DIR=/usr`
rather than relying on arbitrary nonempty values.

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
  too new for the earlier parser configuration; the accepted generated Erf
  artifact has independently validated v2 metadata).
- ORT 1.22's VSI-NPU EP would hit the same query at runtime (on top of
  its own separate cross-compile bugs).

Stock firmware avoids all of this because `libnnsdk.so` was compiled
against the exact chip/driver combo it ships with. The `vsi_nn_*` API
surface (which our NPU probe shows works) is the one that initializes
cleanly.

**Viable/research NPU paths:**
1. Call `libnnsdk.so` directly from Rust FFI (same as stock). Proved
   reachable in `waved-npu-probe`. Requires `.nb` files, which require
   VeriSilicon Acuity Toolkit (NDA) to produce from ONNX.
2. Import a hash-pinned exact-target NBG through the external-ovxlib TIM-VX
   build. Proven for the generated Erf graph.
3. Compile a fixed graph through exact device userspace. Proven only for the
   firmware Erf path; broader ops need the matching compiler package.
4. Recover the private `nbgType=2/3` export-data representation. Its loader
   exists, but its complete safe grammar is still unknown.

**Dead paths:**
- Foreign host-simulator NBG execution — wrong target and semantics.
- Bundled-ovxlib TIM-VX runtime — kernel contract mismatch.
- ORT 1.22.0 + VSI-NPU EP — cross-compile bugs + same runtime issue.
- The `6.4.18:886827` CLC compiler against Wave `6.4.19:946407` — observed
  program-object ABI corruption; quarantined.

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

This command is read-only. If a holder is present, stop the experiment and
review ownership. Stopping a stock service is not an automatic lab action and
requires a separate explicit decision.

## Related docs

- [AUDIO_HARDWARE.md](AUDIO_HARDWARE.md) — 8-mic array + DOA (A55-only).
- `/etc/init.d/S60waved` watchdog behaviour — in `AUDIO_HARDWARE.md`.
