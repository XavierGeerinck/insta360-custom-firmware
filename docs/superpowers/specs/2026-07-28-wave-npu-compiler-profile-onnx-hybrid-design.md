# Wave NPU Compiler/Profile Recovery and Hybrid ONNX Design

**Date:** 2026-07-28

**Status:** Approved for implementation planning

**Primary outcome:** Recover the minimum Wave-specific compiler and hardware
profile contract required to compile and execute a fixed-shape convolutional
spectral-mask model, while ONNX Runtime provides general model loading,
verified NPU partitioning, and CPU fallback.

## 1. Executive Summary

The Insta360 Wave has a working VeriSilicon NPU runtime and accepts VPMN v2
NBGs for product `0x1000000e`. Earlier PoCs generated and executed a small
custom Erf graph through the exact device stack. Fixed MatrixMul and Conv2d
graphs reached OpenVX operator selection but failed graph verification and NBG
generation. The exact firmware lacks `libCLC.so`; a version-adjacent 6.4.18
compiler library is ABI-incompatible with the Wave's `6.4.19:946407` runtime
and is quarantined.

This design does not attempt to clone the complete proprietary Acuity
toolchain. It recovers the smaller contract needed by the Wave:

- the exact hardware feature/profile record;
- the configuration format consumed by an offline/no-kernel compiler;
- the compatible graph compiler boundary;
- the tensor, quantization, layout, and operator constraints relevant to a
  small audio mask model;
- reliable ONNX-to-TIM-VX partitioning with honest CPU fallback;
- evidence and safety gates for every compiler and device experiment.

The open ONNX Runtime VSINPU execution provider supplies the general ONNX
frontend. Unsupported graph regions run on the CPU. The designated audio mask
subgraph must compile and execute on the NPU in strict qualification mode
before the project claims NPU acceleration.

## 2. Verified Starting Point

### 2.1 Live target

The connected Wave has the following verified identity:

```text
kernel:          Linux 5.4.242-ab575
galcore runtime: 6.4.19.5.946407
device node:     /dev/galcore, major 199, minor 0
NBG target:      0x1000000e
nanoqFreq:       838860800
enableNN:        255
recovery:        1
```

The runtime reports EVIS2, subgroup size 4, no stream processor, and no VA40.
These values describe exposed runtime capabilities; they are not a complete
physical microarchitecture specification.

### 2.2 Proven custom execution

The exact Wave stack generated a 5,840-byte VPMN v2 Erf NBG with target
`0x1000000e`. Two bounded executions passed:

```text
[0, 0, 0,   0] -> [0, 0, 0, 0]
[0, 1, 2, 255] -> [0, 1, 1, 1]
```

The device health envelope remained unchanged.

### 2.3 Current compiler boundary

- Exact-device Erf NBG export works.
- Fixed MatrixMul and Conv2d select OpenVX kernels.
- MatrixMul and Conv2d fail `Graph::Compile()` and `CompileToBinary()`.
- SignalFrame reaches a source-kernel path that requires `libCLC.so`.
- No exact `6.4.19:946407` CLC or matching public offline compiler package was
  found in the local corpus or targeted public-repository searches as of
  2026-07-28.
- The 6.4.18 CLC candidate relocates but is not ABI-compatible and must not be
  invoked again.

### 2.4 Recovery assets

The repository contains:

- exact AArch64 `libArchModelSw.so`, `libNNArchPerf.so`, `libVSC.so`,
  `libGAL.so`, `libOpenVX.so`, and `libovxlib.so`;
- an adjacent 6.4.18 VIP9000 SDK;
- an x86 TIM-VX simulator SDK;
- TIM-VX source and public headers;
- ONNX Runtime 1.22 with the VSINPU execution provider;
- 138 stock VPMN v2 NBGs for target `0x1000000e`;
- associated stock ONNX models;
- a strict NBG parser and earlier runtime/compiler probes;
- exact hashes, manifests, device health collection, and bounded execution
  harnesses.

`libArchModelSw.so` exposes architecture-model APIs such as
`archShowConfigurations`, `archIsFeatureAvailable`,
`archSwLibInitFromDriver`, and `archSetManualFeatures`. Its diagnostics show a
feature-database lookup keyed by chip ID, chip version, product ID, eco ID,
and customer ID. This is evidence that the effective Acuity target profile is
at least partly compiled into architecture/performance libraries rather than
stored only in a standalone file.

TIM-VX's platform examples also document `VIV_GPU_FILE` as the hardware
configuration supplied to a no-kernel NBG compiler. The expected family name
is `VIP9000NANOQ_PLUS`; the Wave-specific product is `0x1000000e`.

## 3. Goals

### 3.1 Primary goal

Produce an operationally validated Wave profile and compiler path that can
compile and execute one fixed Conv graph and then a tiny convolutional
spectral-mask ONNX subgraph on the NPU.

### 3.2 ONNX goal

Provide broad ONNX file compatibility through ONNX Runtime:

- supported, preflight-approved partitions may run through VSINPU;
- unsupported partitions run on the CPU;
- strict mode requires a designated mask partition to be NPU-resident;
- audit mode reports placement without running inference.

General ONNX support means general parsing and hybrid execution. It does not
mean that every ONNX operator will run on the NPU.

### 3.3 Audio goal

Run a bounded microphone-noise-suppression pipeline:

```text
audio
  -> CPU window/STFT/normalization
  -> ONNX mask estimator
       -> verified subgraph on NPU
       -> unsupported surrounding nodes on CPU
  -> CPU mask application/iSTFT/overlap-add
  -> enhanced audio
```

The first mask model is fixed-shape, convolutional, and stateless. Recurrent
state may later be expressed as explicit tensor inputs and outputs, but GRU,
LSTM, FFT, and complex-number operator recovery are outside the first
milestone.

## 4. Non-Goals

This phase does not:

- reproduce the complete Acuity user interface, training pipeline, or model
  importer;
- promise all-NPU execution for arbitrary ONNX;
- reverse the transistor-level microarchitecture;
- replace the `galcore` driver;
- replay raw ioctl streams;
- patch NBG target identifiers or executable payloads;
- overwrite stock libraries or models;
- flash firmware, change boot configuration, stop stock services, reload
  modules, write sysfs parameters, or automatically reset the NPU;
- call the quarantined 6.4.18 CLC compiler entry points;
- deploy a production audio feature before correctness, quality, latency, and
  soak gates pass.

## 5. Approaches Considered

### 5.1 ONNX Runtime plus hardened VSINPU — selected

Use ONNX Runtime for parsing, optimization, shape handling, graph
partitioning, and CPU fallback. Use VSINPU to lower eligible partitions to
TIM-VX. Recover only the Wave-specific profile and compiler boundary beneath
TIM-VX.

Advantages:

- shortest route to useful ONNX compatibility;
- the current source registers 55 ONNX operator builders, with eligibility
  still conditional on opset, attributes, shape, type, and quantization;
- unsupported operators naturally form CPU partitions;
- model authors can use normal ONNX tooling;
- the recovered profile remains useful for a later offline compiler.

Risks:

- current VSINPU failure propagation is insufficient;
- operator registration does not prove a particular shape/type works;
- CPU/NPU partition boundaries may erase latency gains;
- runtime compilation still depends on a compatible backend.

### 5.2 Offline ONNX-to-NBG conversion — later extension

Partition and compile models on the development machine, then deploy
validated NBGs.

Advantages:

- deterministic deployment artifacts;
- lower device startup work;
- easier production allowlisting.

Risks:

- requires a compatible no-kernel compiler;
- hybrid orchestration must marshal tensors between NBG and CPU partitions;
- NBG compiler/runtime compatibility remains version-sensitive.

### 5.3 Custom ONNX-to-NBG compiler — deferred

Implement graph lowering, tiling, scheduling, allocation, shader generation,
and serialization independently.

This provides maximum control but is much larger than the selected goal. It
is considered only if the open frontend and available compiler components
reach a demonstrated hard limit.

## 6. System Architecture

### 6.1 Component map

```text
                       DEVELOPMENT HOST

  ONNX model
      |
      v
  +----------------------+       +-----------------------+
  | capability auditor   |------>| partition manifest    |
  | shape/type/QDQ audit |       | signatures + evidence |
  +----------+-----------+       +-----------------------+
             |
             v
  +----------------------+       +-----------------------+
  | ONNX Runtime         |------>| CPU ExecutionProvider |
  | hardened VSINPU      |       +-----------------------+
  +----------+-----------+
             |
             v
  +----------------------+       +-----------------------+
  | TIM-VX graph         |<------| Wave profile          |
  | compiler adapter     |       | VIV_GPU_FILE candidate|
  +----------+-----------+       +-----------------------+
             |
             v
  +----------------------+       +-----------------------+
  | NBG validator/cache  |------>| evidence manifest     |
  +----------+-----------+       +-----------------------+
             |
             | reviewed, hash-pinned artifact
             v

                         INSTA360 WAVE

  supervised worker -> exact OpenVX/libGAL -> /dev/galcore -> NPU
           |
           +-------------------------------> CPU fallback
```

### 6.2 Wave profile extractor

Purpose:

- recover the target configuration and its provenance;
- produce a machine-readable profile;
- generate a candidate compiler configuration without inventing values.

Inputs:

- exact architecture/performance ELF files;
- decompilation and exported-symbol evidence;
- exact hardware identity results;
- stock NBG header and section statistics;
- adjacent SDK/profile grammar evidence;
- bounded read-only architecture API output after ABI validation.

Outputs:

- `wave-profile-evidence.json`;
- `wave-profile.schema.json`;
- a candidate `VIP9000NANOQ_PLUS_PID0X1000000E.config`;
- an evidence report mapping every generated field to its source.

Each profile field has:

```text
name
value
representation
evidence_class
source_artifact
source_location
confidence
validation_state
notes
```

Allowed evidence classes are:

- `observed-live`;
- `static-verified`;
- `differentially-verified`;
- `inferred`;
- `unknown`.

An unknown field remains unknown. A generator may omit it or use a
documented compiler default, but it may not silently guess.

### 6.3 Compiler adapter

The compiler adapter presents one logical interface for:

- exact target-side TIM-VX compilation;
- an isolated offline/no-kernel compiler;
- compile-only preflight;
- NBG parsing and compatibility validation.

Every backend declares:

- executable and library hashes;
- architecture and ABI;
- compiler/runtime version;
- accepted profile schema;
- whether it opens `/dev/galcore`;
- whether it emits NBG;
- supported timeout and isolation controls.

Compiler candidates run in disposable processes with:

- fixed inputs;
- read-only dependency roots where practical;
- a private output directory;
- `SIGALRM`;
- parent-death signaling;
- a host-side deadline;
- captured stdout, stderr, signal, exit status, and output hashes.

The adapter accepts an NBG only when:

- magic and version parse;
- target equals `0x1000000e`;
- sizes, offsets, counts, and section bounds validate;
- input/output descriptors match the requested graph;
- checksums validate;
- no unexpected files or device-state changes occurred.

### 6.4 ONNX capability auditor

The auditor runs before session construction and never executes inference.

It reports:

- model and initializer hashes;
- opset and shape-inference result;
- static and dynamic dimensions;
- input/output data types;
- QDQ and QLinear groups;
- registered VSINPU operator mappings;
- per-node shape/type/opset eligibility;
- predicted NPU and CPU partitions;
- CPU/NPU boundary tensors;
- tensor byte counts;
- the designated mask partition;
- whether strict mode would accept the model.

The auditor creates stable graph signatures from canonical topology, tensor
metadata, constants, attributes, profile identity, and runtime/compiler
identity. A signature changes when any execution-relevant property changes.

### 6.5 Hardened VSINPU

The existing provider is modified only within the repository build. Stock
device libraries are not replaced.

Required behavior:

- `Graph::Compile()` failure becomes a real error;
- `Graph::Run()` is impossible unless compilation succeeded;
- copy and output failures propagate;
- partition placement is emitted as structured evidence;
- preflight-approved signatures are distinguished from merely registered
  operators;
- strict mode verifies that the designated partition is claimed and compiled;
- audit mode performs no inference;
- worker deadlines and cancellation are explicit.

The provider cannot safely hand a claimed partition back to ONNX Runtime
after `Compile()` fails. Therefore predictable fallback is decided before
claiming a partition. An unexpected compile failure aborts session creation;
the wrapper then creates one CPU-only session. It does not retry NPU
compilation.

### 6.6 Audio inference shell

The audio shell owns:

- channel selection and framing;
- windowing and STFT;
- feature normalization;
- ONNX input construction;
- mask validation and clamping;
- mask application;
- inverse STFT and overlap-add;
- recurrent-state handoff when later enabled;
- CPU reference/fallback;
- timing and quality measurements.

The first neural subgraph uses only verified fixed-shape operations:

- Conv represented in a supported tensor layout;
- Add, Sub, Mul, or Div;
- Reshape, Transpose, and Concat when verified;
- simple activations such as Relu, Tanh, or Sigmoid;
- FP16 or calibrated QDQ INT8/U8.

The first milestone excludes dynamic dimensions, control flow, external
initializers, unsupported `int64` tensors, FFT operators, and native recurrent
operators.

The initial integration ABI is mono 48 kHz audio with a 480-sample, 10 ms
hop, matching the current Wave audio pipeline. PoC 11 freezes and hashes the
analysis window, FFT size, feature normalization, context-frame count, tensor
layout, and mask interpretation before generating or executing the model.

## 7. Hybrid Execution Semantics

### 7.1 Modes

`hybrid`
: Preflight-approved partitions may run on NPU. Unsupported regions run on
  CPU. Unexpected NPU session-construction failure produces one CPU-only
  session.

`strict`
: The designated mask partition must pass preflight, be claimed by VSINPU,
  compile, and execute on NPU. Otherwise model loading or the run fails.

`audit`
: Perform model analysis, partitioning, and eligibility reporting without
  inference.

`cpu`
: Run the deterministic ONNX Runtime CPU reference.

### 7.2 Pre-compilation fallback

The following conditions make a node or partition CPU-only before VSINPU
claims it:

- unsupported operator or opset;
- missing or dynamic shape;
- unsupported data type;
- invalid or unsupported quantization;
- unsupported attributes;
- unverified profile/compiler combination;
- missing approved graph signature;
- excessive or unsafe tensor size;
- a prior explicit deny record for the same signature and environment.

### 7.3 Session-construction fallback

For an unexpected compile failure:

1. propagate the compiler error;
2. discard the incomplete VSINPU session;
3. record the signature and evidence;
4. construct one CPU-only session;
5. do not issue a second NPU compile attempt.

The fallback is reported as CPU-only. It is not reported as a successful
hybrid or NPU run.

### 7.4 Inference-time fallback

NPU inference runs in a supervised worker. On a reported error, deadline,
malformed output, canary failure, or device-health change:

- recompute the current frame on CPU only when the remaining frame budget is
  at least the measured CPU-reference p99 plus a 1 ms guard;
- otherwise emit the delay-aligned unprocessed input for that frame;
- emit silence only when the input frame itself is unavailable or invalid;
- move subsequent frames to CPU;
- disable further NPU submissions;
- require an explicit clean health check before NPU use can be re-enabled.

There is no automatic NPU retry, module reload, or reset.

## 8. PoC Sequence

### 8.1 PoC 9A — Architecture database recovery

Question:

> What target profile and feature database are embedded in the exact
> architecture/performance libraries?

Work:

- inventory symbols, relocations, strings, read-only data, and call graphs;
- decompile profile lookup and configuration display functions;
- compare exact AArch64 and adjacent/x86 implementations;
- map structure offsets and enum values;
- correlate fields with exact hardware identity and NBG evidence.

Pass:

- a parseable evidence profile exists;
- every field has provenance;
- critical identity fields are corroborated;
- unknowns are explicit.

This PoC is host-only.

### 8.2 PoC 9B — Architecture API ABI

Question:

> Can the exported architecture APIs be called with a statically proven ABI
> to render the exact live configuration?

Work:

- reconstruct function prototypes and dependent structures;
- validate against the host/x86 counterpart;
- intercept the architecture logging callback;
- use canaries and bounded allocations;
- prepare a separately reviewed read-only device query only if the host ABI
  passes.

Pass:

- host calls are deterministic and memory-safe under sanitizers;
- the device query, if authorized, exits cleanly;
- the report agrees with independently observed identity fields;
- device health is unchanged.

No guessed structure is passed to an exact device library.

### 8.3 PoC 9C — `VIV_GPU_FILE` format recovery

Question:

> What configuration grammar and fields does the no-kernel compiler accept?

Work:

- locate configuration parser entry points and diagnostic strings;
- recover tokenization, types, required fields, defaults, and validation;
- test parser behavior in an isolated host process;
- compare documented TIM-VX examples and adjacent SDK behavior;
- generate a Wave candidate from verified profile fields.

Pass:

- the grammar is documented and tested;
- malformed inputs fail cleanly;
- the generated file is deterministic;
- every emitted field maps to evidence or a documented compiler default.

This PoC is host-only and does not compile a graph on the device.

### 8.4 PoC 9D — Offline compiler compatibility

Question:

> Can an available isolated compiler consume the Wave profile and emit a
> structurally compatible Wave-targeted NBG?

Work:

- test only tiny fixed Erf and Conv controls;
- capture compiler version and dependency hashes;
- perform size-only and binary-output compilation;
- parse every candidate without executing it;
- compare with the exact generated Erf NBG and stock corpus invariants;
- repeat identical compiles to characterize determinism.

Pass:

- no crash or forbidden dependency use;
- target is exactly `0x1000000e`;
- NBG structure and checksums validate;
- tensor metadata matches the requested graph;
- nondeterminism is absent or fully isolated to explained fields.

Failure does not authorize patching NBG payloads, replacing libraries, or
executing an incompatible candidate.

### 8.5 PoC 9E — ONNX capability auditor

Question:

> Can model placement be predicted and explained without running inference?

Work:

- build synthetic ONNX controls for relevant supported and unsupported ops;
- perform static shape and QDQ analysis;
- canonicalize graph signatures;
- emit partition manifests;
- calculate boundary tensors and transfer volume.

Pass:

- placement is deterministic;
- every rejected node has an exact reason;
- strict and hybrid decisions match the manifest;
- CPU-only models cannot be mislabeled NPU-capable.

### 8.6 PoC 9F — VSINPU hardening

Question:

> Does every compile, copy, run, and output error propagate without executing
> an invalid graph?

Work:

- enforce provider compile state;
- add audit, hybrid, strict, and CPU modes;
- expose structured placement and execution evidence;
- implement session-level CPU fallback;
- add deadlines and worker supervision;
- inject compile and runtime failures in host tests.

Pass:

- an uncompiled graph cannot call `Run()`;
- expected unsupported nodes remain CPU;
- unexpected compile failure creates one CPU-only session;
- strict mode fails when the designated partition is not NPU-resident;
- error-injection tests are deterministic.

### 8.7 PoC 10A — First Conv control

Question:

> Can one fixed convolution compile and produce numerically correct output on
> the Wave NPU?

Work:

- compile a fixed-shape Conv with known constants;
- parse and approve the NBG;
- create a separate device execution plan;
- execute once with canary-protected buffers;
- compare every output with the CPU oracle;
- capture pre/post health.

Pass:

- compilation and one execution succeed;
- the intended partition is proven NPU-resident;
- output meets the numerical tolerance;
- health remains unchanged;
- no timeout, holder leak, or kernel alert occurs.

### 8.8 PoC 10B — Hybrid ONNX control

Question:

> Can a deliberately mixed ONNX graph run one verified partition on NPU and
> one unsupported partition on CPU?

Work:

- construct a fixed mixed graph;
- audit placement;
- require exactly the expected NPU and CPU partitions;
- execute under a separate reviewed plan;
- compare hybrid and CPU-only outputs;
- inject one controlled preflight rejection.

Pass:

- the manifest proves both placements;
- outputs meet tolerance;
- fallback is correctly labeled;
- no unexpected NPU retry occurs;
- device health remains unchanged.

### 8.9 PoC 11 — Audio mask model

Question:

> Can a small fixed convolutional mask network improve noisy speech while
> meeting the Wave's real-time budget?

Work:

- define the fixed feature tensor and hop;
- train or obtain a legally usable small model;
- export deterministic ONNX;
- audit and qualify its mask partition;
- validate first on synthetic spectra;
- run recorded speech/noise through the complete CPU/NPU hybrid pipeline;
- compare with the same model on CPU.

Pass:

- the mask partition is NPU-resident in strict mode;
- NPU/hybrid output matches CPU within tolerance;
- audio-quality, latency, and soak gates pass;
- CPU fallback produces continuous, bounded audio behavior.

## 9. Error Taxonomy

Every failure receives one stable category:

- `PROFILE_UNKNOWN_FIELD`;
- `PROFILE_IDENTITY_MISMATCH`;
- `PROFILE_CONFIG_REJECTED`;
- `COMPILER_DEPENDENCY_MISMATCH`;
- `COMPILER_CRASH`;
- `COMPILER_DEADLINE`;
- `COMPILER_NO_OUTPUT`;
- `NBG_PARSE_FAILURE`;
- `NBG_TARGET_MISMATCH`;
- `NBG_METADATA_MISMATCH`;
- `ONNX_SHAPE_UNSUPPORTED`;
- `ONNX_TYPE_UNSUPPORTED`;
- `ONNX_OPERATOR_UNSUPPORTED`;
- `ONNX_QUANTIZATION_UNSUPPORTED`;
- `PARTITION_NOT_PREFLIGHTED`;
- `PARTITION_COMPILE_FAILURE`;
- `PARTITION_NOT_NPU_RESIDENT`;
- `NPU_RUN_FAILURE`;
- `NPU_RUN_DEADLINE`;
- `NPU_OUTPUT_INVALID`;
- `NPU_HEALTH_CHANGED`;
- `CPU_FALLBACK_FAILURE`;
- `AUDIO_QUALITY_GATE_FAILURE`;
- `AUDIO_REALTIME_GATE_FAILURE`.

Expected research failures are results, not reasons to broaden device access.

## 10. Safety Boundary

### 10.1 Allowed without a new device execution plan

- static ELF and NBG analysis;
- source inspection;
- decompilation;
- host parser and compiler-format tests;
- ONNX generation, auditing, and CPU execution;
- isolated host/x86 architecture-library calls;
- documentation and manifest generation;
- read-only ADB identity and health collection already covered by an existing
  reviewed plan.

### 10.2 Requires a separate reviewed plan

- each new target-side compiler or graph execution;
- the first exact architecture API query;
- a new compiler/library candidate on the device;
- a new graph topology, shape, type, quantization, constant set, or expected
  output;
- any new path that opens `/dev/galcore`.

### 10.3 Outside this design

- flashing or updating firmware;
- replacing stock libraries or models;
- stopping stock services;
- rebooting;
- sysfs or module writes;
- loading or unloading `galcore`;
- raw ioctl replay;
- executing an unverified or target-patched NBG;
- calling the quarantined CLC;
- blind fuzzing;
- automatic NPU reset or retry.

## 11. Validation Contract

### 11.1 Profile and compiler

- Critical identity fields match live hardware and stock NBG evidence.
- Unknown fields remain marked unknown.
- The generated profile is accepted by the isolated compiler.
- Output target is exactly `0x1000000e`.
- Two identical compiles are byte-identical or have fully explained
  nondeterministic fields.
- Conv compilation and controlled execution pass before the profile is called
  operationally recovered.

### 11.2 ONNX and placement

- Synthetic models cover each audio-relevant operator, shape, type, and
  quantization mode used by the target model.
- Unsupported operators demonstrably form CPU partitions.
- A mixed control proves one NPU and one CPU partition.
- Strict mode rejects CPU placement of the designated mask partition.
- Manifests distinguish registered, eligible, claimed, compiled, executed,
  and CPU-fallback nodes.

### 11.3 Numerical correctness

- Integer outputs match exactly or within one quantization unit where a
  documented rounding rule differs.
- FP16 and FP32 tests use explicit absolute and relative tolerances derived
  from the CPU oracle and recorded in each test manifest.
- Canary regions remain intact.
- Unexpected NaN or infinity fails the run.
- NPU, hybrid, and CPU paths use identical stored inputs.

### 11.4 Audio quality

- NPU/hybrid output matches the same ONNX model on CPU within numerical
  tolerance.
- Median SI-SDR improvement is at least 3 dB on the fixed synthetic-noise
  corpus.
- Median STOI for enhanced output is no more than `0.02` below the
  unprocessed noisy-input baseline.
- Quality results record corpus identity, license, preprocessing, sample
  rate, signal levels, and metric implementation.

### 11.5 Real-time and stability

- Sustained real-time factor is below `0.8`.
- p99 processing time is below the configured audio hop duration.
- A minimum 10,000-frame soak produces no deadline, holder leak, monotonic
  memory growth, kernel alert, or health-envelope change.
- The CPU fallback independently meets the same `0.8` real-time-factor and
  one-hop p99 limits.

### 11.6 Failure injection

Tests cover:

- unsupported operator;
- dynamic shape;
- profile identity mismatch;
- rejected profile config;
- compiler crash and deadline;
- malformed or wrong-target NBG;
- partition compile failure;
- worker exit;
- NPU deadline;
- bad output and canary failure;
- device disconnection;
- CPU fallback failure.

Each case must produce its documented fallback or fail-closed state without an
NPU retry.

## 12. Evidence and Documentation

Every PoC creates:

- an approved plan before any new device graph;
- a hash-pinned input manifest;
- a machine-readable result;
- raw stdout/stderr and timing;
- compiler, profile, runtime, model, and device hashes;
- pre/post device health for every device-touching PoC;
- a lab notebook with hypothesis, method, result, interpretation, non-claims,
  and next gate;
- deterministic reproduction commands;
- unit and integration tests.

Generated binary evidence remains outside normal source commits when large or
device-specific. Tracked manifests refer to exact hashes and evidence
locations.

## 13. Completion Criteria

This design is complete when all of the following are true:

1. A versioned Wave profile exists with field-level provenance.
2. The profile is accepted by an isolated compatible compiler.
3. A fixed Conv graph compiles for `0x1000000e`.
4. The Conv executes once on the Wave and matches its CPU oracle.
5. The hardened VSINPU provider cannot run an uncompiled graph.
6. The capability auditor produces deterministic partition manifests.
7. A mixed ONNX graph proves NPU and CPU placement in one inference.
8. Strict mode proves the mask subgraph is NPU-resident.
9. A tiny audio mask model passes numerical, quality, real-time, and soak
   gates.
10. All failure-injection cases fall back or fail closed as specified.

Passing criteria 1-4 demonstrates minimum operational compiler/profile
recovery. Passing criteria 1-8 demonstrates usable hybrid ONNX acceleration.
Passing all criteria demonstrates the first qualified NPU-assisted
noise-suppression model.

## 14. References

- VeriSilicon TIM-VX:
  <https://github.com/VeriSilicon/TIM-VX>
- TIM-VX platform/no-kernel compiler example:
  <https://github.com/VeriSilicon/TIM-VX/blob/main/samples/platform_sample/README>
- VeriSilicon VIP9000:
  <https://www.verisilicon.com/en/IPPortfolio/VivanteVIP9000>
- ONNX Runtime VSINPU execution provider:
  <https://github.com/microsoft/onnxruntime/tree/main/onnxruntime/core/providers/vsinpu>
- Maderix, Inside the M4 Apple Neural Engine:
  <https://maderix.substack.com/p/inside-the-m4-apple-neural-engine?r=1afltp>
- Maderix ANE research:
  <https://github.com/maderix/ANE>
