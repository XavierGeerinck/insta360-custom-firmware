# Insta360 Wave NPU Research

> **Production integration parked (2026-10-12).** Noise reduction moves to
> the CPU. The measured case is in
> [`waved/docs/COMPUTE_HARDWARE.md`](../../waved/docs/COMPUTE_HARDWARE.md#status-parked-2026-10-12):
> a vendor-compiled 41 M-MAC fp16 model ran 2.8 ms on the NPU against 4.7 ms
> on the CPU, but 4.5–5.2 ms once its output was copied and converted, and
> every NPU call costs about 70–90 µs. That section also lists where to resume
> (E1–E3 compile matrix, vendor toolchain requests, the Khadas v1 probe).

**REA continuation (2026-10-13).** A current stock-CTC control passed, and
the exact OpenVX NN checker is now statically proven to reject FP32 weight
tensors when an output tensor is present. Two replacement custom graphs
now compile and execute on the NN target: FP16 FIR Conv and quantized U8
FC. Both passed one exact CPU-oracle comparison with unchanged captured
device health. See the [REA recovery notebook](lab-notebook/2026-10-13-rea-op-dtype-recovery.md)
and [PoC 16 results](lab-notebook/2026-10-13-poc16-native-fp16-conv-u8-fc.md).

**ONNX native backend (2026-10-13).** PoC 17 imported real ONNX parameters
for two FP16 Conv and two U8 QLinearMatMul controls, compiled each on the NN
engine and ran each once. All returned bytes agree with ORT CPU and an
independent oracle. Changed coefficients/bias produce different correct
NBGs and readbacks; all eight staging roots are removed and captured health
is unchanged. Use the explicit `wave-native` backend with an exact qualified
package; see [commands and supported profiles](onnx/native-backend.md) and
the [PoC 17 notebook](lab-notebook/2026-10-13-poc17-onnx-native-backend.md).

**Trained MarbleNet trial (PoC18, 2026-10-13).** The real pointwise and residual
blocks exported on NN and passed numerical qualification. The trained
depthwise K29/dilation2 block was rejected during kernel initialization,
so the complete-model NPU compile and benchmark were skipped. A file VAD
frontend provides an explicit CPU fallback; all800 frozen host frame
decisions match the original FP32 reference. See the
[MarbleNet guide](onnx/marblenet.md) and
[PoC18 notebook](lab-notebook/2026-10-13-poc18-trained-marblenet.md).

**MarbleNet compatibility (PoC19, 2026-10-13).** The rejected trained layer
now passes native export and inference through exact TP zero padding followed
by unchanged NN convolution. The first trained strided block also passes.
The complete candidate preserves every folded parameter bit and all four
original CPU outputs. Full NN qualification remains blocked by a USB warning
between runs and subsequent ADB loss; Wi-Fi restoration and final live state
are unverified. See the
[PoC19 notebook](lab-notebook/2026-10-13-poc19-marblenet-compatibility.md).

This directory is the evidence hub for learning how to run custom neural
networks on the Insta360 Wave's VeriSilicon-backed NPU safely and
reproducibly.

The immediate result is concrete:

- code we control can open `/dev/galcore`;
- code we control can load the Wave's `libnnsdk.so`;
- an unmodified, firmware-targeted NBG can be loaded, executed, read back,
  and destroyed;
- two independently captured PoC 0 controls produced the same 332,800-byte
  output digest;
- the device remained responsive and all captured NPU parameters and
  required artifact hashes were unchanged;
- PoC 1 recovered and test-locked the private runtime ABI, backend dispatch,
  NBG loader lifecycle, and cleanup/error boundaries;
- PoC 2 classified all 139 configured graphs, resolved the public v2 section
  schema, recovered tensor/network/memory metadata, and verified all 690
  stored section CRC-32 records;
- PoC 3 captured two equivalent stock runtime/driver traces: 73 successful
  galcore calls per run, a stable phase-correlated command sequence, and the
  unchanged stock output digest;
- the available host simulator still emits the wrong NBG product target;
- the exact Wave stack generated an untouched 5,840-byte VPMN v2 Erf NBG for
  target `0x1000000e`, with one U8 `[4]` input/output and all stored
  checksums valid;
- two separately reviewed executions proved zero and nonzero input/output
  for that generated NBG;
- the live hardware reports EVIS2, subgroup size 4, no stream processor, and
  no VA40 support;
- general custom models are **not yet unlocked**: the July Add and FP32
  MatrixMul/Conv controls failed export or verification. PoC 16 now proves
  native FP16 Conv and U8 FC, with broader graph coverage still unqualified;
- the Wave firmware omits the exact `libCLC.so` needed by its embedded
  EVIS/OpenCL source path; a version-adjacent compiler crossed relocation but
  failed its program-object ABI and is permanently quarantined.
- PoC 9A recovered the exact five-key architecture lookup, the unique
  target-`0x1000000e` Wave database row, its masked revision-family fallback,
  and a strict evidence profile without calling a compiler or device API.
- PoC 9C recovered the x86 simulator's five-field config parser and
  358-record catalog, accepted the live revision in an isolated 13-case
  matrix, and emitted a strict Wave candidate that is explicitly refused for
  compiler use because a compatible no-kernel graph-to-NBG
  `VIV_GPU_FILE` consumer had not been qualified.
- PoC 9D acquired and fully hash-pinned the Radxa-linked Allwinner
  `ubuntu-npu:v2.0.10.2` image, recovered its exact vendor Wave config and a
  genuine no-kernel parser, and then closed the package as incompatible for
  graph NBG generation: its shader compilers are `6.4.18:967862`, while
  Acuity uses an embedded `6.4.6:769531` simulator SDK.
- PoC 9E/9F added bounded general ONNX admission, recursive control-flow
  inventory, deterministic graph signatures, a CPU numerical oracle, and
  fail-closed hybrid/strict modes; its default NPU placement remains disabled.
- PoC 12A modeled five exact NBGs with total byte ownership and
  byte-identical unchanged re-emission while binding 21 serializer/load
  targets to the exact Wave binaries.
- PoC 12B captured three compile-only stock-serializer outputs for a
  one-byte Erf constant differential; A1/A2 are byte-identical, and the
  sealed finite-registry result remains conservatively unresolved.
- PoC 12C separately recovered and enforced the v2 header CRC order,
  decoded the section-20/26 dynamic-I/O table geometry, validated 138/138
  v2 graphs and 8,939 table entries, and exactly reconstructed the
  controlled B graph in memory without writing or executing it.
- PoCs 13A–13F produced a working learned NPU-assisted noise-cancellation
  PoC: the Wave executed 1,201 U8 `[481]` Erf partitions, all 577,681 output
  bytes matched an independent CPU oracle, and those returned tensors
  reconstructed a 576,000-sample waveform with +4.8443 dB median SI-SDR
  improvement and 9.8283 dB noise attenuation.
- The fixed partition also passed a 10,000-execution numerical and health
  soak. Production Rust `NpuInfer` integration and full service-level
  latency, memory, reset, and thermal qualification remain incomplete.
- PoC 13G measured the exact partition on both Wave compute engines. The
  Cortex-A55 lookup was 20.686 times faster than NPU `Run()` and 26.137
  times faster than the complete NPU boundary by ratios of means.
- PoC 14A froze and quality-qualified a larger learned
  `Multiply -> Add -> Erf` partition, but PoC 14B closed it as a clean
  compiler negative: the exact Wave checker rejects its asymmetric-U8 by
  symmetric-per-channel-I8 Multiply signature before NBG emission. No
  coefficient differential exists, so semantic affine emission and expansion
  of that learned model into Conv remain unqualified.
- PoC 16 recovered the dtype boundary with REA, rebuilt a hash-bound TIM-VX
  wrapper against the exact device SDK, and exported valid 1,800-byte FP16
  Conv and 2,056-byte U8 FC NBGs. Both compile logs name the NN target;
  each model ran once, returning all 32 or 4 oracle bytes exactly. All four
  temporary staging roots were removed and captured health stayed unchanged.
- PoC 17 adds actual ONNX-to-native lowering for the two fixed profiles and
  explicit qualification-bound sessions. Four changed-parameter controls
  passed one NN compile and one byte-exact inference each. Strict loading
  creates no CPU session; hybrid failure latches fallback without a retry.

The detailed program design is in
[`2026-07-28-wave-npu-reverse-engineering-program-design.md`](../superpowers/specs/2026-07-28-wave-npu-reverse-engineering-program-design.md).
PoC 0's implementation plan is in
[`2026-07-28-wave-npu-poc0-reproducible-lab.md`](../superpowers/plans/2026-07-28-wave-npu-poc0-reproducible-lab.md).
PoC 1's ABI evidence starts at [`abi/README.md`](abi/README.md), and its
implementation plan is
[`2026-07-28-wave-npu-poc1-runtime-abi.md`](../superpowers/plans/2026-07-28-wave-npu-poc1-runtime-abi.md).
PoC 2's format evidence starts at
[`format/README.md`](format/README.md), and its implementation plan is
[`2026-07-28-wave-npu-poc2-nbg-corpus-format.md`](../superpowers/plans/2026-07-28-wave-npu-poc2-nbg-corpus-format.md).
PoC 3's dynamic evidence starts at [`trace/README.md`](trace/README.md), and
its implementation plan is
[`2026-07-28-wave-npu-poc3-runtime-driver-trace.md`](../superpowers/plans/2026-07-28-wave-npu-poc3-runtime-driver-trace.md).
PoCs 5 through 8 are consolidated in the
[`custom compiler notebook`](lab-notebook/2026-07-28-poc5-custom-compiler.md),
[`custom NBG execution notebook`](lab-notebook/2026-07-28-poc6-7-custom-nbg-execution.md),
and
[`audio denoiser boundary notebook`](lab-notebook/2026-07-28-poc8-audio-denoiser-boundary.md).
PoC 9A starts at the
[`architecture profile`](profile/README.md) and its
[`architecture database notebook`](lab-notebook/2026-07-28-poc9a-architecture-database-recovery.md).
PoC 9C continues in the
[`configuration parser evidence`](profile/viv-gpu-file-grammar.json) and
[`parser-recovery notebook`](lab-notebook/2026-07-28-poc9c-viv-gpu-file-parser-recovery.md).
PoC 9D continues in the
[`compiler-acquisition evidence`](profile/compiler-acquisition-evidence.json)
and
[`static-qualification notebook`](lab-notebook/2026-07-28-poc9d-compiler-acquisition-static-qualification.md).
PoC 9E/9F starts at the
[`ONNX audit and fallback evidence`](onnx/README.md) and its
[`lab notebook`](lab-notebook/2026-07-28-poc9e-9f-onnx-cpu-fallback.md).
PoC 12A–12C continue in the
[`format and serializer evidence`](format/README.md), the
[`PoC 12A notebook`](lab-notebook/2026-07-28-poc12a-open-nbg-serializer.md),
the
[`PoC 12B notebook`](lab-notebook/2026-07-29-poc12b-erf-constant-differential.md),
and the
[`PoC 12C notebook`](lab-notebook/2026-07-29-poc12c-dyna-io-crc-profile.md).
PoCs 13A–13F are consolidated in the
[`Erf-mask result and reproduction guide`](models/erf-mask/README.md), with
the exact research-versus-production boundary in the
[`completion audit`](models/erf-mask/completion-audit.md).

## Why this is a staged program

The Wave does not expose a documented custom-model SDK. Its stock software
contains several useful layers, but they solve different parts of the
problem:

```text
audio model or ONNX
        │
        ├── strict host audit ──> ONNX Runtime CPU fallback — proven
        │
        └── Wave NPU routes
              ├── exact compiler for general ops — blocked
              ├── native Erf / FP16 Conv / U8 FC export — proven
              └── private export-data graph — incomplete
                         │
                         ▼
              Wave-targeted NBG or verified export-data graph
                         │
                         │ TIM-VX import or private libnnsdk ABI
                         ▼
              VeriSilicon userspace runtime
                         │
                         │ /dev/galcore ioctls
                         ▼
              galcore kernel driver
                         │
                         ▼
                      Wave NPU
```

The broad compiled-NBG blocker is above the runtime. The host SDK available
in this repository produces target `0x10000020`, while the Wave requires
`0x1000000e`. The exact device stack can emit and execute the required
target for Erf, FP16 Conv and U8 FC. The earlier Add and FP32 MatrixMul/Conv
controls failed export or verification, and additional model graphs remain
unqualified.
We do not patch a foreign product field because changing an identifier cannot
make target-dependent kernels and command streams correct for different
silicon.

PoC 9D added a second host package to the inventory. It contains a correct
vendor-authored Wave profile, but its two `VIV_GPU_FILE` executables are
shader compilers below the no-kernel version gate. Its Acuity graph exporter
uses an even older simulator SDK. A profile match and an NBG emitter existing
in the same image do not prove that the emitter uses that profile or produces
device-compatible code.

PoC 1 found a second branch in the exact stock runtime. `nbgType` 2/3 parses
a private export-data representation, creates individual OpenVX tensors and
nodes, then verifies the graph. That branch may avoid a precompiled NBG, but
its format, operation parameters, ownership rules, and target behavior are
not yet sufficiently recovered to execute a handcrafted graph. It is a
research route, not an unlock claim.

This research follows the same evidence-first spirit as Maderix's
[Apple Neural Engine investigation](https://maderix.substack.com/p/inside-the-m4-apple-neural-engine)
and [ANE tools](https://github.com/maderix/ANE): recover boundaries, preserve
raw observations, build parsers rather than relying on offsets, and graduate
from tiny controlled graphs to useful models. The Wave stack is not Apple's
ANE, so none of the private ABI or binary-format details are assumed to
transfer.

## Program status

| PoC | Question | Status | Exit gate |
|---|---|---|---|
| 0 — Reproducible lab | Can every later experiment be hash-pinned, bounded, and compared? | **Passed 2026-07-28** | Two stock CTC controls are semantically equivalent and leave captured state unchanged. |
| 1 — Runtime ABI and call graph | What are the exact loader, tensor, execution, and cleanup contracts? | **Passed 2026-07-28** | Recovered ABI map is backed by exact function bytes, callers, focused decompilation, public-header deltas, and layout tests. |
| 2 — NBG corpus and format | Which sections encode target, tensors, operations, constants, and relocation data? | **Passed 2026-07-28** | The strict host parser classifies all 139 configured graphs, agrees with live CTC metadata, and validates 690/690 section checksums. |
| 3 — Runtime and driver trace | What runtime calls and driver interactions occur for a known graph? | **Passed 2026-07-28** | Two bounded traces have the same 73-call phase-correlated GAL sequence, output digest, loaded paths, driver events, syscall counts, cleanup, and health. |
| 4 — Firmware v3 audio loader | Does `libmodel_infer.so` provide a useful CPU, OpenCL, or NPU path for audio models? | **Static backend gate passed in PoC 1** | It is classified as a CPU/MNN-style reference; dynamic format/quality characterization remains optional. |
| 5 — Wave custom-graph compiler | Can exact Wave userspace emit a legitimate target-native custom graph? | **Partial pass, extended 2026-10-13** | Erf, FP16 Conv and U8 FC emit valid NBGs. Earlier Add/FP32 MatrixMul/Conv controls and general shader compilation remain blocked. |
| 6 — First generated-NBG execution | Can that exact generated artifact be imported, run, read back, and cleaned up? | **Passed 2026-07-28** | One zero-input execution passed with unchanged health. |
| 7 — Nonzero characterization | Does nonzero data traverse the generated graph and overwrite the output buffer correctly? | **Passed 2026-07-28** | `[0,1,2,255] -> [0,1,1,1]` for the U8 Erf graph in one bounded execution. |
| 8 — Minimal audio-denoiser boundary | Can fixed MatrixMul or Conv2d filtering export or execute through public TIM-VX? | **Closed FP32 negative 2026-07-28** | Both FP32 controls failed export and direct graph verification. PoC 16 separately proves the FP16 FIR Conv. |
| 9A — Architecture database recovery | What target identity/profile is embedded in the exact architecture libraries? | **Passed host-only 2026-07-28** | Five-key lookup, unique Wave row, revision mask, ABI bounds, strict evidence profile, and explicit unknowns are reproducible. |
| 9C — Configuration parser recovery | What grammar and target catalog does the available Vivante simulator implement? | **Passed isolated-host gate 2026-07-28** | Five ordered fields, exact catalog, live-revision acceptance, unsafe vendor edge cases, and strict candidate/refusal outputs are reproducible. |
| 9D — Compiler acquisition/static qualification | Does a trusted public package contain a compatible no-kernel Wave NBG compiler? | **Closed blocked gate 2026-07-28** | Exact vendor Wave config and real parser recovered; the only `VIV_GPU_FILE` consumers are `6.4.18` shader compilers and Acuity's NBG path is an older simulator backend. Zero compiler/device calls. |
| 9E/9F — ONNX frontend and CPU fallback | Can general ONNX models be admitted, signed, explained, and run on CPU under fail-closed hybrid semantics? | **Passed host-only 2026-07-28** | Three deterministic fixtures, a real Silero audit, CPU/hybrid equality, zero NPU attempts, strict refusal, and 35 focused tests are reproducible. |
| 12A — Open NBG serializer recovery | Can exact existing NBGs be modeled and re-emitted losslessly while statically bounding the firmware serializer? | **Passed host-only 2026-07-28** | Five exact graphs have total byte ownership and byte-identical re-emission; 21 serializer/load targets are byte-bound; bounded ONNX correlation leaves coefficient packing explicitly `not-ready`. |
| 12B — Controlled Erf constant differential | Does the stock serializer emit deterministic evidence for a one-byte source-constant change? | **Passed live capture 2026-07-29; interpretation sealed** | Three compile-only calls produced identical A1/A2 and a structurally stable B; the original bounded registry remains `multi-region-unresolved`, and cleanup restored the exact device baseline. |
| 12C — DYNAIO and file-CRC qualification | Can the section-20/26 geometry and header checksum be uniquely recovered without another device run? | **Passed host-only 2026-07-29** | Exact Wave save-path evidence, the controlled differential, and 138/138 corpus matches prove the CRC order; 8,939 table entries pass strict geometry, and in-memory A→B reconstruction is exact. |
| 13A–13F — Learned Erf-mask denoiser | Can a learned, quality-qualified noise mask use one sealed NPU partition with exact fallback? | **Passed 2026-07-29** | Training, compile, strict package, six vectors, 1,201-frame waveform, 10,000-frame soak, and same-frame CPU fallback passed. |
| 13G — Wave CPU/NPU benchmark | What is the same-device cost of the exact U8 `[481]` Erf partition? | **Passed R2 2026-07-29** | 10,000 paired frames were exact; Cortex-A55 was 20.686× faster than NPU `Run()` and 26.137× faster than NPU total by ratio of means. |
| 14A — Learned affine host contract | Does a finite integer `Multiply -> Add -> Erf` partition retain the denoiser quality gates? | **Passed host-only 2026-07-29** | The U8/per-channel-I8 candidate, six vectors, ONNX graph, CPU oracle, and nine-file package regenerate byte-identically and pass all 1,230-item quality gates; execution remains unapproved. |
| 14B/C — Affine producer differential | Can exact Wave userspace emit deterministic baseline/weight/bias NBG controls? | **Closed clean negative 2026-07-29** | A1 was rejected at `MULTIPLY` type checking (`ASYM UINT8, SYMM PC INT8`) before NBG emission. Known-equivalent retries were skipped, health and CLC gates held, and PoC 14C has no differential to decode. |
| 16 — REA-guided native dtype controls | Can FP16 Conv and quantized U8 FC compile and execute on the NN engine? | **Passed 2026-10-13** | Both target-native NBGs pass exact tensor/CRC qualification and one canary-protected CPU-oracle execution; captured health is unchanged and staging is removed. |
| 17 — ONNX native backend | Can actual ONNX coefficients lower through the qualified Conv/FC routes and run with exact package evidence? | **Passed 2026-10-13** | Two FP16 Conv and two U8 QLinearMatMul models compile on NN and return every oracle byte. Explicit native strict/hybrid sessions preserve default CPU/refusal behavior. |
| 18 — Trained MarbleNet | Can the real VAD backbone compile and match FP32 speech decisions? | **Partial native pass; full NPU blocked 2026-10-13** | Trained pointwise/residual blocks qualified; depthwise K29/rate2 rejected. Full model uses explicit CPU fallback, with800/800 frozen host decisions agreeing. |
| 19 — MarbleNet compatibility | Can an exact rewrite remove the trained convolution rejection? | **Layer qualified; full NN pending, 2026-10-13** | Exact Pad28/28 + original K29/rate2 and first strided block passed. USB warning/ADB loss stopped full testing and blocked Wi-Fi restoration; encompassing failed interval retained. |
| 9B / 9D continuation — Remaining compiler adapter | Can the missing no-kernel NBG consumer and exact VSINPU compiler path be proven? | Pending, separately gated | Acquire x86-64 no-kernel SDK `>=6.4.22` plus ovxlib `>=1.2.26`, prove its dependency/I/O ABI, then compile one fixed Conv before any NPU signature is approved. |
| Production integration | Can `waved` schedule the NPU safely with fallback and observability? | Pending | Full audio acceptance passes without privileged recovery behavior. |

PoC 0 does not claim that custom models run. It establishes the controlled
lab required to make every later claim reproducible.

PoC 1 also remains host-only. It proves the ABI and code paths present in
specific hashed binaries; it does not make a new graph safe to execute.

PoC 2 is also host-only. It names sections from pinned public source and
decodes bounded metadata, but its original result left accelerator commands,
coefficients, patches, load states, dynamic I/O, and the connection region
opaque. PoC 12C later decoded dynamic-I/O table geometry and the file-level
checksum; per-entry semantics and mutation safety remain unresolved. Format
knowledge and valid checksums do not make a mutated or foreign graph safe.

PoC 3 is dynamic but observational. It identifies the 32-byte request
envelope, 376-byte live interface, command/hardware/status prefix, lifecycle
sequence, and runtime libraries without capturing command-union payloads.
It does not turn those observations into a direct-driver programming API or
authorize replay.

PoC 5 is a narrow compiler result. It proves one firmware-provided Erf export,
not arbitrary source compilation or general operation coverage. The
version-adjacent CLC experiment is a negative ABI result and carries an
explicit no-further-calls quarantine.

PoCs 6 and 7 are positive live executions of one exact graph. They do not
turn the harness into a general model runner and do not establish latency,
streaming, or learned-model quality.

PoC 8 is a useful negative model-portability result. `matrixmul` and `conv2d`
node selection must not be reported as graph verification or inference.

PoC 9A is host-only. It reads two statically demonstrated architecture tables
from hash-pinned ELF load segments and cross-checks them with prior live
facts. It does not load the libraries, generate `VIV_GPU_FILE`, invoke a
compiler, or authorize PoC 9B device/API work.

PoC 9C is host-only with one narrowly isolated x86 parser exception. It never
contacts the Wave and never loads a compiler. The generated
`.candidate.config` proves deterministic rendering from live facts; it does
not prove that the absent no-kernel SDK consumes the same grammar.

PoC 9D is entirely static. It never loads the acquired OCI image, imports an
Acuity native extension, runs a compiler, or contacts the Wave. It
corroborates PoC 9C's grammar in a real `VIV_GPU_FILE` consumer, but that
consumer accepts shader source and does not emit graph NBG. The exact vendor
profile is evidence, not authorization to use the embedded simulator or the
quarantined adjacent CLC.

PoC 9E/9F is also host-only. Its default `hybrid` mode means one classified
CPU-only ONNX Runtime session after NPU preflight finds no approved
signature. It makes zero NPU attempts. The real Silero audit validates and
counts its 51-graph, 689-node control-flow tree without running it. Neither
the audit nor the Conv/Sigmoid fixture proves a Wave NPU operator.

PoC 17 separately enables `--backend wave-native` for its exact qualified
packages. It extracts ONNX constants, generates native source and validates
the complete compiler/NBG/oracle/run/cleanup chain. This does not populate
the historical PoC 9 registry or qualify arbitrary ONNX graphs.

PoC 12A is host-only and serializer-first. It proves lossless unchanged
representation, not semantic mutation: all five selected NBGs reconstruct
byte-for-byte and any marked change is refused. Its bounded static evidence
ties 21 generation/load exports to exact function bytes, including one
explicit four-byte export alias. A finite ONNX/NBG search found no exact
weight or bias occurrence, so it does not authorize coefficient repacking,
custom NBG construction, or NPU execution.

PoC 12B is the only new live step in this serializer sequence. It made
exactly three compile-only calls against one fixed graph definition and
performed no graph load or execution. Its captured analysis is frozen at
the finite registry reviewed before the run; later discoveries do not
retroactively turn that evidence into a broader claim.

PoC 12C is host-only. It can validate section-20/26 ranges, recompute the
header CRC, and reproduce one controlled A-to-B change in memory. It does
not expose an operation encoder, coefficient packer, arbitrary dynamic-I/O
writer, general ONNX compiler, or approved NPU placement signature.

PoCs 13A–13F prove a working learned NPU-assisted noise-suppression research
pipeline for one exact Erf NBG. PoC 13G adds the missing same-device
performance comparison: offloading Erf alone is slower than the Cortex-A55
lookup, although both paths fit comfortably inside a 10 ms audio hop. This
does not predict the performance of a larger affine or convolutional
partition.

## PoC 0 evidence summary

Both controls used definition SHA-256
`9cf0a286874a1f2062383977b9a26136efba12e768ac14f4e26eef9b47e1df6a`.

| Field | Control 1 | Control 2 |
|---|---:|---:|
| Run ID | `poc0-first-control-20260728` | `poc0-second-control-20260728` |
| Host-side ADB duration | 340 ms | 182 ms |
| Mean of three native iterations | 4.243 ms | 2.618 ms |
| Output bytes | 332,800 | 332,800 |
| Output SHA-256 | `e621b213…52d4` | `e621b213…52d4` |
| Graph cleanup | 0 | 0 |
| Post-run ADB responsive | yes | yes |
| NPU parameters unchanged | yes | yes |
| Required hashes unchanged | yes | yes |

The six individual native observations were 8.198, 2.241, 2.290, 3.539,
2.155, and 2.160 ms. Their descriptive median is 2.266 ms and their range is
2.155–8.198 ms. This is a control observation, not a benchmark: the graph is
an ASR CTC component with zero-filled input, the sample is tiny, and no
thermal or scheduler controls were imposed.

See the full
[`PoC 0 lab notebook`](lab-notebook/2026-07-28-poc0-reproducible-lab.md)
and the tracked
[`live-device manifest`](manifests/wave-live-v1.1.6.6.json).
Raw envelopes and native logs are kept under `artifacts/npu/` and are
intentionally gitignored.

## Repository map

```text
docs/npu/
  README.md                         this status and architecture guide
  reproduction.md                  operator procedure
  manifests/
    README.md                       manifest policy
    wave-live-v1.1.6.6.json         hash-pinned live baseline
  lab-notebook/
    README.md                       notebook policy
    000-template.md                 required experiment record
    2026-07-28-poc0-...md           PoC 0 evidence
    2026-07-28-poc1-...md           PoC 1 evidence
    2026-07-28-poc2-...md           PoC 2 evidence
    2026-07-28-poc3-...md           PoC 3 live trace evidence
    2026-07-28-poc5-...md           compiler, caps, selector, CLC evidence
    2026-07-28-poc6-7-...md         generated-NBG execution evidence
    2026-07-28-poc8-...md           audio model portability boundary
    2026-07-28-poc9a-...md          architecture-profile recovery
    2026-07-28-poc9c-...md          simulator-config parser recovery
    2026-07-28-poc9d-...md          compiler acquisition/static qualification
    2026-07-28-poc9e-9f-...md       ONNX audit and CPU fallback
    2026-07-28-poc12a-...md         lossless serializer recovery
    2026-07-29-poc12b-...md         controlled live constant differential
    2026-07-29-poc12c-...md         host DYNAIO and CRC qualification
  abi/
    README.md                       PoC 1 result and reproduction
    prototypes.md                   recovered FFI and layouts
    stock-ctc-call-graph.md          live path lifecycle
    backend-map.md                  runtime and v3 audio backends
    error-taxonomy.md               failure and recovery contract
    runtime-abi-inventory.json       generated exact ELF evidence
    firmware-matrix.md              generated cross-firmware matrix
    ghidra-evidence.json             generated focused call/decompile evidence
  format/
    README.md                       PoC 2/12A–12C format and serializer result
    field-map.md                    byte layout and confidence ledger
    corpus-summary.md               generated corpus distributions
    stock-nbg-inventory.json        generated per-file evidence
    serializer-*.json               static, ownership, and round-trip evidence
    onnx-nbg-correlation.json        bounded coefficient search ledger
    erf-constant-differential.json   sealed PoC 12B interpretation
    dyna-io-crc-profile.json         PoC 12C machine evidence
  trace/
    README.md                       PoC 3 result and reproduction
    galcore-interface.md            ioctl envelope and command ledger
    runtime-sequence.md             phase-correlated transaction map
    repeatability.md                live acceptance analysis
    normalized-run-{1,2}.json       pointer-free generated evidence
    repeatability.json              machine comparison
  profile/
    README.md                       PoC 9A result and reproduction
    wave-profile.schema.json        strict evidence-profile schema
    wave-profile-evidence.json      generated field-level Wave profile
    arch-profile-ghidra-evidence.json
                                    focused architecture decompilation
    viv-gpu-file-grammar.json        recovered simulator grammar
    wave-viv-gpu-file.candidate.*   strict candidate and refusal manifest
    compiler-acquisition-evidence.json
                                    acquired package qualification ledger
    no-kernel-sdk-acquisition-checklist.md
                                    exact missing-package request and gate
  onnx/
    README.md                       PoC 9E/9F result and reproduction
    hybrid-policy.json              compiler-bound empty NPU allowlist
    spectral-mask-*.json            audit and CPU/hybrid run evidence
    spectral-mask-*.npz             deterministic CPU/hybrid outputs
    silero-vad-audit.json            bounded real-model audit

waved/scripts/npu/
  README.md                         harness developer guide
  experiments.json                  sole executable experiment source
  npu_lab.py                        host runner
  abi_corpus.json                   exact static-analysis corpus
  abi_inventory.py                  deterministic ELF inventory
  ghidra_targets.json               bounded decompiler targets
  ghidra_abi.py                     deterministic Ghidra export
  arch_profile_targets.json         exact architecture/profile corpus
  arch_profile.py                   strict host-only profile recovery
  compiler_profile.py               simulator-config parser recovery
  compiler_acquisition_targets.json exact package and refusal contract
  compiler_acquisition.py           inert archive/OCI/compiler qualifier
  nbg_corpus.json                   exact NBG corpus definition
  nbg_format.py                     strict non-executing NBG parser
  nbg_serializer_targets.json       exact PoC 12A artifact/function policy
  nbg_serializer_recovery.py        bounded serializer static evidence
  nbg_image.py                      immutable lossless NBG image
  nbg_correlate.py                  finite ONNX/NBG correlation engine
  nbg_constant_diff.py              sealed PoC 12B differential analyzer
  nbg_dyna_profile.py               PoC 12C CRC/DYNAIO qualifier
  trace_normalize.py                strict pointer-free trace normalizer
  npu_compile_*.py                  fixed compile-only experiment harnesses
  npu_*execution_lab.py             fixed one-shot execution harnesses
  npu_*audio_denoiser_lab.py        fixed PoC 8 graph harnesses
  tests/                             host contract tests

waved/tools/npu-compiler/
  tim_vx_compile_*.cc               immutable compile-only graph definitions
  wave_npu_*.cc                     fixed capability/execution probes
  build-device-*.sh                 pinned offline AArch64 builds
  verify-device-*-policy.sh         source and ELF policy gates
  stage-device-*.sh                 hash-pinned non-overwriting tmpfs staging

waved/tools/npu-trace/
  gal_trace.c                       metadata-only ioctl interposer
  build.sh                          reproducible AArch64 build
  stage.sh                          exact non-overwriting tmpfs staging
  stock-ctc-trace.sh                fixed framed trace wrapper

waved/tools/onnx-hybrid/
  pyproject.toml, uv.lock            pinned host ONNX environment
  src/wave_onnx_hybrid/              auditor, runtime, fixtures, and CLI
  fixtures/                          deterministic ONNX and NPZ controls
  tests/                             admission and fallback contract tests

artifacts/npu/                       raw, local, gitignored run evidence
```

Stable hardware facts also live in
[`waved/docs/COMPUTE_HARDWARE.md`](../../waved/docs/COMPUTE_HARDWARE.md).
Research notebooks may contain provisional observations; facts move into an
authoritative document only after their evidence and limits are clear.

## Evidence language

Every important statement should be classed as one of:

- **Observed live** — returned by the connected device during a bounded,
  recorded read or experiment.
- **Static verified** — derived from a specific hashed binary, source file,
  or firmware artifact without executing it.
- **Inferred** — the evidence supports the explanation, but an alternative is
  still plausible.
- **Unknown** — no adequate evidence exists yet.

The distinction matters. For example:

- “The tested stock CTC graph executes” is observed live.
- “The graph target field is `0x1000000e`” is both statically parsed and
  observed in probe output.
- “A custom generated Erf NBG executes nonzero input” is observed live in
  PoC 7 and backed by the exact artifact and runner hashes.
- “The tested Conv2d denoiser ran” is false: PoC 8D stopped at graph
  verification before `Graph::Run`.
- “The v3 `libmodel_infer.so` AES/ANS path is CPU/MNN-style and does not
  import the NPU stack” is static verified.
- “The private export-data branch can run an arbitrary handcrafted graph”
  remains unknown.
- “The default PoC 9 ONNX hybrid runner tried the Wave NPU” is false: its
  empty approval registry records zero NPU attempts and one CPU session.
- “Explicit ONNX native lowering executes the four PoC 17 controls” is
  observed live, with exact NN artifacts, oracle readback and cleanup.
- “The v2 header checksum and section-20/26 table geometry are recovered” is
  static verified and controlled-differential verified in PoC 12C.
- “Any ONNX initializer can now be written into section 20” remains
  unsupported.
- “Changing a product ID makes a foreign NBG safe” is unsupported and is not
  tested.

## Safety boundary

Allowed without a new device-mutation review:

- host-side firmware and binary analysis;
- ADB state checks and bounded reads;
- read-only sysfs inspection;
- execution of a separately reviewed source-controlled experiment whose
  executable, graph/model, arguments, environment, timeout, artifact hashes,
  expected result, attempt count, and stop conditions are pinned;
- invocation of the exact hash-pinned, non-overwriting staging script named
  by that reviewed plan, only in its unique disposable `/tmp/wave-npu-*`
  root.

Requires explicit approval before the specific action:

- firmware flashing or upgrade;
- replacing a system library or stock model;
- loading or unloading a kernel module;
- stopping a normal service;
- rebooting;
- writing sysfs;
- replaying raw driver commands;
- executing an unverified target graph;
- changing boot configuration.

Never part of this program:

- blind mutation fuzzing against `/dev/galcore`;
- executing a graph whose product bytes were patched to bypass validation;
- overwriting a stock model;
- automatic NPU-driver reset after a hang.

## Noise-cancellation target

The first useful custom workload remains neural noise suppression. PoCs 5
through 7 now prove one exact custom NBG compile-and-execute route. PoC 8
shows that this narrow Erf result does not yet generalize to the MatrixMul or
Conv2d primitives needed by even a minimal denoiser. Candidate models must:

- operate on bounded streaming frames;
- have a CPU quality/reference implementation;
- use operations and dtypes already proven on this NPU;
- preserve recurrent state explicitly if needed;
- meet the Wave audio deadline with margin;
- fail closed to the existing CPU path;
- never require reboot, driver reset, or stock-service replacement for
  recovery.

The existing DeepFilterNet-based CPU backend remains the correctness and
fallback baseline. The next NPU milestone is a compiler-compatible fixed
Conv control, followed by a tiny spectral-mask network—not an immediate full
DeepFilterNet port. PoC 9E/9F now provides the fixed Conv/Sigmoid ONNX
control, exact input/output validation, and CPU oracle for that later gate,
but it deliberately keeps the control on CPU. PoC 12C removes two serializer
unknowns but does not supply the missing Conv operation encoding or
coefficient packing. NPU acceleration is an optimization, not a reason to
make audio capture fragile.

## Start here

1. Read [`reproduction.md`](reproduction.md),
   [`abi/README.md`](abi/README.md), and
   [`format/README.md`](format/README.md), then
   [`trace/README.md`](trace/README.md), the
   [`PoC 5 compiler notebook`](lab-notebook/2026-07-28-poc5-custom-compiler.md),
   the
   [`PoC 6/7 execution notebook`](lab-notebook/2026-07-28-poc6-7-custom-nbg-execution.md),
   and the
   [`PoC 8 audio notebook`](lab-notebook/2026-07-28-poc8-audio-denoiser-boundary.md),
   then the
   [`PoC 9A/9C profile result`](profile/README.md), and finally the
   [`PoC 9E/9F ONNX result`](onnx/README.md) and
   [`PoC 12A serializer notebook`](lab-notebook/2026-07-28-poc12a-open-nbg-serializer.md),
   [`PoC 12B live differential`](lab-notebook/2026-07-29-poc12b-erf-constant-differential.md),
   and
   [`PoC 12C qualification`](lab-notebook/2026-07-29-poc12c-dyna-io-crc-profile.md).
2. Run the host-only unit and ABI-layout tests.
3. Regenerate or verify static evidence before changing a recovered
   declaration.
4. List the committed device experiments.
5. Take a read-only snapshot.
6. Run only a reviewed definition when preflight hashes match.
7. Compare repeated envelopes and record the conclusion and limits.
