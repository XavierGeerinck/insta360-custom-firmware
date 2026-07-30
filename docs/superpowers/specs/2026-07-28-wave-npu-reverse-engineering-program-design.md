# Wave NPU Reverse-Engineering and Custom-Model Program

**Date:** 2026-07-28

**Status:** Implemented through PoC 8; compiler/model program continues

**Primary outcome:** A reproducible, software-visible description of the
Insta360 Wave NPU stack plus a safe toolchain for compiling, executing,
measuring, and deploying custom neural-audio models.

## 1. Executive Summary

This program reverse-engineers the Insta360 Wave's Vivante neural
accelerator incrementally. It follows the useful research pattern
demonstrated by Maderix's Apple Neural Engine work:

1. inventory the software and hardware surfaces;
2. obtain direct runtime access;
3. characterize the compiled-program format;
4. trace the runtime-to-driver transaction;
5. produce controlled custom programs;
6. measure operations, memory, precision, and throughput;
7. deploy an application model with objective acceptance tests.

The project already completed the direct-access step. A Rust process can
load the Wave's stock `libnnsdk.so`, execute a firmware-matched NBG, read
its output, and cleanly destroy the graph. The current custom add graph
does not execute because the available host simulator emits product
`0x10000020`, while the Wave accepts product `0x1000000e`.

The original central gate was to produce and execute a legitimate
Wave-targeted custom NBG. That gate has now passed for one deterministic U8
Erf graph:

```text
generated NBG:
  VPMN v2, target 0x1000000e, 5,840 bytes

zero execution:
  [0,0,0,0] -> [0,0,0,0]

nonzero execution:
  [0,1,2,255] -> [0,1,1,1]
```

Patching the product identifier in an incompatible NBG does not satisfy
the gate. The compiler must produce a graph whose instructions,
descriptors, kernels, and metadata are actually valid for the Wave.

The current gate is broader compiler and operation coverage. Fixed
MatrixMul and Conv2d denoiser graphs select OpenVX but fail both NBG export
and ordinary graph verification before inference. SignalFrame reaches the
firmware's source-kernel path, but exact `libCLC.so` is absent; a
version-adjacent CLC is ABI-incompatible and quarantined.

The first real workload remains microphone noise suppression. The model
ladder starts with a fixed convolution control, then a tiny causal
spectral-mask network composed only of verified operations, and only later a
higher-quality model such as a compatible DeepFilterNet variant. VAD,
residual echo suppression, dereverberation, and speaker embeddings remain
independent model PoCs.

## 2. Definition of "Reverse the Entire NPU"

The program covers the complete software-visible programming stack:

- physical device and kernel-driver identity;
- device nodes, sysfs status, memory mappings, and ioctl behavior;
- user-space runtime libraries and their ABI;
- model lifecycle: parse, create, bind, execute, retrieve, destroy;
- NBG container versions, sections, tensor descriptors, and target IDs;
- compiler and archmodel selection for product `0x1000000e`;
- supported operations, shapes, layouts, quantization, and precision;
- dispatch overhead, concurrency, memory cliffs, and throughput;
- fault boundaries and cleanup behavior;
- model packaging and production integration.

It does not promise:

- transistor-level microarchitecture recovery;
- destructive decapsulation or electrical probing;
- a complete undocumented hardware instruction-set specification when
  the public software stack only exposes graph-level programs;
- replacement of the working `galcore` kernel driver;
- bypassing cryptographic protections or extracting third-party secrets.

This is the same practical boundary visible in the ANE precedent:
software-visible direct programming and empirical characterization are
the goal, even if the accelerator's lowest-level internal ISA remains
opaque.

## 3. Terminology

- **NPU:** The Wave's Vivante VIP9000 Nano-family neural accelerator.
- **Product ID:** The hardware target embedded in NBG files. The live
  Wave's accepted stock graphs use `0x1000000e`.
- **NBG:** VeriSilicon Network Binary Graph, normally beginning with
  `VPMN`.
- **NN SDK:** Amlogic's `libnnsdk.so` wrapper.
- **OVXLIB:** VeriSilicon's `libovxlib.so` graph runtime.
- **GAL:** The `libGAL.so` user library and `galcore.ko` kernel driver.
- **Archmodel:** The compiler-side description of a specific Vivante
  product's capabilities and scheduling constraints.
- **Custom model:** A model whose graph or weights we control, compiled
  for and executed on the Wave NPU.
- **Noise cancellation:** In this program, microphone noise suppression
  and residual-echo/dereverberation processing. It does not mean
  loudspeaker active-noise-control feedback.

## 4. Verified Starting Point

### 4.1 Live device

The connected unit currently reports:

```text
kernel:       Linux 5.4.242-ab575
galcore:      6.4.19.5.946407
device node:  /dev/galcore
NBG product:  0x1000000e
nanoqFreq:    838860800
enableNN:     255
```

`/dev/adla0` is absent on this unit. ADLA-related strings and functions
inside `libnnsdk.so` are not evidence of a usable second device.

### 4.2 Direct inference already proven

The committed `waved-npu-probe`:

- opens `/dev/galcore`;
- loads `/usr/lib/libnnsdk.so`;
- resolves ABI-verified runtime and tensor-metadata symbols;
- loads `/model/asr_de/de_online/ctc_online.nb`;
- submits one deterministic FP16 input;
- executes and retrieves one FP32 output;
- verifies shape, nonzero bytes, and a stable full-output digest;
- destroys the graph with native status zero.

The stock graph uses:

```text
NBG version: 0x00020000
product:     0x1000000e
input:       FP16 [256, 16, 1]
output:      FP32 [5200, 16, 1]
```

### 4.3 Custom compiler boundary

The host-generated add graph uses:

```text
NBG version: 0x0001001e
product:     0x10000020
```

The Wave runtime reaches graph creation and returns:

```text
hardware version is not in product list
set_graph_inoutputs fail
aml_module_create returned NULL
```

This separated runtime access from the original host simulator. Later PoCs
proved a narrow exact-device compile route for Erf, then showed that the
same exposed interface does not generalize to Add, MatrixMul, or Conv2d.

### 4.4 Useful local assets

The repository contains:

- several extracted Wave firmware generations;
- exact Wave runtime libraries;
- a large corpus of firmware NBGs and associated ONNX models;
- TIM-VX and its NBG parser;
- a host-generated add NBG;
- a stock CTC NBG;
- a CPU DeepFilterNet backend behind `NpuInfer`;
- the current device inference probe;
- prior static-analysis and compute-hardware reports.

Firmware v3 adds `libmodel_infer.so`, which exports generic and
model-specific acoustic noise-suppression and echo-suppression entry
points. The live unit's older root filesystem does not contain this
library. It can be studied statically and, after its ABI is recovered,
staged from `/data` without flashing the device.

## 5. Technical Strategies Considered

### 5.1 Runtime-first differential analysis — primary

Use exact firmware libraries, the stock NBG corpus, known model
topologies, controlled inputs, and runtime instrumentation. Build the
format and operation map from independently checkable differences.

Advantages:

- begins from a proven execution route;
- generates useful artifacts before custom compilation is solved;
- keeps tests small and attributable;
- has the lowest device risk;
- supports every later compiler and model experiment.

Limitation:

- stock graphs alone may not expose every operation or cleanly isolate
  one variable.

### 5.2 Compiler-first recovery — required secondary route

Recover Wave target selection in TIM-VX, `libArchModelSw.so`,
`libVSC.so`, simulator configuration, or another compatible SDK.

Advantages:

- directly unlocks arbitrary controlled graphs;
- enables a clean operation and performance matrix;
- is the shortest route from the current blocker to custom models if a
  compatible archmodel already exists in accessible binaries.

Limitations:

- proprietary compiler components may be incomplete;
- public SDKs have version or platform skew;
- a compiler can emit syntactically valid but unsafe target programs.

### 5.3 Driver-first command-stream analysis — supporting route

Trace open, mmap, allocation, ioctl, submission, wait, and cleanup
behavior at the `/dev/galcore` boundary.

Advantages:

- proves whether a workload reaches hardware;
- reveals memory and submission behavior hidden by the NN SDK;
- supports dispatch, concurrency, and fault characterization.

Limitations:

- substantially more complex than graph-level work;
- command streams may contain relocated addresses and version-specific
  structures;
- replaying raw streams is unsafe until allocation and relocation rules
  are understood.

### 5.4 Chosen combination

The program is runtime-first, pulls in compiler recovery at the custom
add gate, and uses driver tracing throughout as an evidence channel.
The v3 audio loader is a bounded side probe, not a substitute for custom
model compilation.

## 6. Research Architecture

```text
                       HOST
  ┌───────────────────────────────────────────────┐
  │ Firmware manifests and binary analysis       │
  │ NBG parser / corpus database                  │
  │ Controlled graph generator                   │
  │ Compiler / archmodel experiments             │
  │ Golden-reference model inference             │
  │ Benchmark analysis and documentation         │
  └──────────────────────┬────────────────────────┘
                         │ hashed artifacts
                         ▼
                      DEVICE
  ┌───────────────────────────────────────────────┐
  │ /data/waved/npu-lab/                         │
  │   safe runner + exact test NBG               │
  │   runtime boundary tracer                    │
  │   ioctl/mmap observation shim                │
  │   operation and model probes                 │
  └──────────────────────┬────────────────────────┘
                         │
       libnnsdk → libovxlib → libGAL → /dev/galcore
                         │
                         ▼
                   Wave VIP NPU
```

The host owns compilation, parsing, corpus analysis, golden outputs,
and report generation. The device only receives bounded, hashed test
artifacts and executes one serialized experiment at a time.

## 7. Repository Structure

The implementation keeps research, device probes, and production
inference separate:

```text
docs/npu/
  README.md
  architecture.md
  abi.md
  nbg-format.md
  compiler.md
  driver.md
  operations.md
  benchmarks.md
  audio-models.md
  reproduction.md
  manifests/
  lab-notebook/

waved/crates/wave-npu-format/
  host-safe bounded NBG parsing and corpus inspection

waved/crates/waved-npu-probe/
  device runtime probes and controlled microbenchmarks

waved/crates/waved-npu/
  production inference abstraction and model lifecycle

waved/tools/npu-shims/
  small C/C++ tracing shims where Rust interposition is impractical

waved/scripts/npu/
  reproducible host build, stage, run, collect, and report commands

artifacts/npu/
  raw traces and large generated binaries; gitignored
```

Normalized manifests and compact JSON/CSV results are committed. Raw
command buffers, core files, and large compiler outputs stay outside
Git unless a small fixture is essential for a parser test.

The existing `waved-npu-probe` NN SDK code will be split into focused
modules as the lab grows. The production `waved-npu` API will not depend
on research-only tracing code.

### 7.1 Execution cadence

This document is the program-level design, not one monolithic
implementation plan. Each numbered PoC is executed as a separate,
reviewable unit:

1. write the PoC-specific implementation plan;
2. implement only that PoC's bounded deliverables;
3. run its host and device acceptance checks;
4. publish the lab-notebook entry and promote stable findings;
5. commit the complete PoC;
6. use its evidence to finalize the next PoC's plan.

PoCs 0 through 8 now have separate plans and evidence. Later PoCs receive a
new plan when a result materially changes their architecture; the overall
goals and safety boundary remain governed by this program design.

## 8. PoC 0 — Reproducible NPU Lab

### Objective

Make every later result attributable, repeatable, and safe.

### Deliverables

- host and device version manifest;
- SHA-256 inventory for firmware libraries and models;
- device pre/post snapshot command;
- serialized experiment runner with timeout;
- JSON result envelope;
- gitignored raw-artifact directory;
- lab-notebook template;
- explicit mutation and risk classification.

### Result envelope

Every experiment records:

```json
{
  "experiment_id": "poc-number/name",
  "timestamp_utc": "ISO-8601",
  "git_commit": "full SHA",
  "device_kernel": "string",
  "runtime_hashes": {},
  "model_hash": "SHA-256",
  "command": ["argv"],
  "environment_allowlist": {},
  "timeout_ms": 15000,
  "exit": {},
  "device_before": {},
  "device_after": {},
  "artifacts": []
}
```

Secrets and unrelated environment variables are never captured.

### Acceptance

- Two runs of the existing stock CTC probe produce equivalent result
  envelopes apart from timestamps, handles, and measured timing.
- The runner refuses an unknown model hash.
- Timeout and signal termination produce explicit failure classes.
- Post-run device health is captured even after probe failure.

## 9. PoC 1 — Runtime ABI and Call-Graph Map

### Objective

Map the complete graph lifecycle and all relevant implementation
branches in the exact Wave libraries.

### Static scope

- `libnnsdk.so`;
- `libovxlib.so`;
- `libOpenVX.so`;
- `libGAL.so`;
- `libVSC.so`;
- `libArchModelSw.so`;
- stock callers such as `libasr_reverb.so`;
- v3 `libmodel_infer.so` and its exact callers.

### Methods

- dynamic symbol and relocation inventory;
- strings-to-code cross references;
- Ghidra headless decompilation;
- objdump/Rizin disassembly for ABI-sensitive call sites;
- firmware-version binary diffs;
- public-header cross-checks;
- function table and backend selection recovery.

### Required maps

- `aml_module_create` configuration and backend dispatch;
- NBG file and memory parsers;
- product and version selection;
- tensor creation and binding;
- run, wait, output retrieval, and cleanup;
- profile and hardware-status routes;
- `vipunify_*` wrapper-to-OVXLIB calls;
- ADLA branch classification;
- error-to-stage taxonomy.

### Acceptance

- Every native function called by project code has a documented
  prototype, evidence source, size/offset tests, and firmware hash.
- The stock CTC execution sequence is represented as a call graph.
- Unverified assumptions are explicitly marked and excluded from FFI.
- A firmware-version matrix identifies ABI changes.

## 10. PoC 2 — NBG Corpus and Format Explorer

### Objective

Parse and compare NBG v1 and v2 safely without loading native code.

### Parser rules

- all offsets and sizes use checked arithmetic;
- every slice is bounded by the file length;
- counts have documented upper bounds;
- unknown sections are preserved as opaque regions;
- parsing never follows device pointers;
- malformed input returns structured errors, never panics;
- parser behavior is deterministic.

### Initial fields

- `VPMN` magic;
- NBG version;
- product ID;
- network identity where present;
- input/output count;
- tensor name, dimensions, precision, quantization, scale, zero point;
- section offsets and lengths;
- embedded checksums or hashes;
- constant and command-data regions;
- unknown-region entropy and similarity fingerprints.

### Corpus analysis

For each firmware NBG:

- file hash and size;
- header version/product;
- tensor metadata;
- associated model family and known topology;
- repeated section signatures;
- cross-language or split-model similarity;
- weight-region similarity;
- target and compiler-family clusters.

### Differential validation

Parser metadata is compared with:

- `aml_util_getInputTensorInfo`;
- `aml_util_getOutputTensorInfo`;
- TIM-VX's bundled NBG parser where its version supports the file;
- known source ONNX shapes where available.

### Acceptance

- Every stock NBG in the repository is classified.
- Supported versions parse without panic.
- Metadata matches the stock runtime for the test corpus.
- Truncation and corruption tests cover every parsed field.
- Unknown fields remain documented rather than guessed.

## 11. PoC 3 — Runtime and Driver Trace

### Objective

Observe a successful NBG transaction from NN SDK entry to galcore
completion.

### Instrumentation layers

1. Explicit boundary logging in `waved-npu-probe`.
2. Preload interposition for `open`, `close`, `ioctl`, `mmap`, `munmap`,
   `poll`, and synchronization functions.
3. Dynamic-loader observation for backend libraries and symbols.
4. Optional debugger breakpoints on selected exported functions.
5. Kernel log collection before and after the test.

If symbol interposition cannot see internal direct calls, the fallback
is debugger-assisted tracing or narrowly patched copies staged under
`/data`; system libraries are never overwritten.

### Trace data policy

- pointer values are normalized per run;
- payload capture is allowlisted by ioctl and bounded in size;
- audio/model contents are not captured by default;
- raw buffers are stored only in the gitignored artifact directory;
- reports contain structures, sizes, hashes, and relationships.

### Acceptance

- Trace a complete successful stock CTC create/run/retrieve/destroy.
- Identify driver open, allocation/mapping, submission, wait, and
  release boundaries.
- A second trace has the same normalized state sequence.
- The tracer itself does not change the output digest.
- Device parameters and service state remain unchanged.

## 12. PoC 4 — Firmware v3 Audio-Model Loader

### Objective

Determine what `libmodel_infer.so` loads and which backend executes it.

### Static work

- recover prototypes for generic and model-specific ANS/AES functions;
- recover configuration structures and callbacks;
- identify the exact caller initialization sequence;
- classify supported model containers;
- locate matching firmware model assets;
- map backend selection strings to executable branches.

### Device work

After static prototypes are confirmed:

- stage the v3 library and one matching model under
  `/data/waved/npu-lab/`;
- run it in a disposable process;
- feed deterministic silence, impulse, sine, and noise frames;
- trace `/dev/galcore`, OpenCL, CPU threads, and mappings;
- compare output with a host/reference path when available;
- destroy the context and verify repeatability.

### Interpretation

The loader is not labeled an NPU path merely because its API is neural.
It must produce galcore activity attributable to inference or expose
another verified hardware backend. A CPU or OpenCL result remains useful
but does not satisfy NPU execution.

### Acceptance

- Function prototypes are ABI-evidenced.
- Accepted container format and frame contract are documented.
- Backend is proven rather than inferred from strings.
- One deterministic run succeeds or the precise failure boundary is
  documented.
- No live-firmware upgrade is required.

## 13. PoC 5 — Wave-Targeted Compiler Gate

### Objective

Compile and execute the deterministic custom add graph for product
`0x1000000e`.

### Route A: TIM-VX simulator target recovery

- map `VSIMULATOR_CONFIG`, `CARCH_CONFIG_PATH`, and related lookups;
- recover the configuration-file grammar;
- inventory product profiles inside host compiler libraries;
- determine whether a VIP9000 profile exists but is not selected;
- compare generated headers and sections across selected products.

### Route B: Accessible VIP9000 SDK differential

- inventory the public eYs3D VIP9000 user stack and kernel sources;
- compare 6.4.18.x structures with Wave 6.4.19;
- isolate archmodel/compiler data from runtime-only components;
- determine whether compilation can run on host, in an emulated
  environment, or safely on-device without replacing galcore.

### Route C: Wave compiler-component recovery

- map `libVSC.so` and `libArchModelSw.so` compiler entry points;
- identify required shader/kernel source assets;
- trace any stock runtime-compilation path;
- classify whether missing assets can be recovered from public SDK
  material compatible with the same product.

### Route D: Vendor toolchain

If a legitimate Amlogic or VeriSilicon Acuity toolkit becomes available,
document its version, target configuration, model conversion, and
redistribution restrictions. Keep generated NBGs separable from
proprietary compiler binaries.

### Safety rule

Changing the four product bytes in an incompatible NBG is diagnostic
only and is never executed on hardware. Header acceptance cannot prove
instruction compatibility.

### Acceptance

- Compiler input is a source-controlled two-input UINT8 add graph.
- Output NBG identifies product `0x1000000e`.
- Runtime tensor metadata matches the source graph.
- Inputs `[1,2]` and `[3,4]` return exactly `[4,6]`.
- Ten iterations pass, graph destruction succeeds, and a second process
  repeats the result.
- Normalized driver trace is consistent with a stock graph.

This is the go/no-go gate for arbitrary custom NPU models.

## 14. PoC 6 — Operation and Hardware Characterization

### Objective

Build the Wave equivalent of an accelerator operation laboratory.

### Operation catalog

Initial families:

- identity, copy, reshape, transpose, concat, split;
- add, subtract, multiply, activation and clamp;
- 1x1, spatial, grouped, and depthwise convolution;
- matrix multiplication and fully connected;
- pooling and reduction;
- softmax and normalization;
- resize and interpolation;
- recurrent operations or their lowered primitive graphs;
- quantize, dequantize, cast, and mixed-precision boundaries.

Each case records:

- supported or rejected;
- supported layouts;
- minimum and maximum observed dimensions;
- alignment constraints;
- precision and quantization modes;
- numerical tolerance;
- compiler and runtime errors;
- latency and memory behavior.

### Performance experiments

- empty/minimal graph dispatch floor;
- tensor-size scaling;
- convolution versus matmul for equivalent work;
- graph-depth scaling;
- constant versus dynamic input behavior;
- FP32, FP16, INT8, UINT8, and mixed precision where supported;
- warm versus cold graph execution;
- memory-working-set sweep to identify cliffs;
- serialized versus bounded concurrent requests;
- context coexistence with the normal application;
- sustained execution and thermal stability.

### Measurement rules

- report median, p95, p99, minimum, maximum, and sample count;
- separate compile, create, first-run, warm-run, retrieve, and destroy;
- use read-only clock/thermal observations;
- pin the exact graph and input hashes;
- measure host overhead independently when possible;
- never change clock, power, recovery, or thermal settings.

### Acceptance

- A machine-readable operation matrix exists.
- At least one compute-bound graph and one dispatch-bound graph are
  identified.
- A memory-performance transition is bounded or explicitly reported as
  not observable with safe tests.
- Numerical results match host goldens within per-precision tolerances.
- Benchmark commands reproduce the published summaries.

## 15. PoC 7 — Neural Noise Suppression

### Objective

Run a custom real-time microphone denoising model on the NPU and
integrate the winning model behind the existing `NpuInfer` abstraction.

### Stage A: correctness model

The first model is a small causal spectral-mask or time-domain network
constructed only from operations already proven by PoC 6. Its purpose is
to validate:

- model state handling;
- 48 kHz / 10 ms framing;
- repeated real-time dispatch;
- output-to-host numerical agreement;
- reset and teardown;
- fallback behavior.

It is not required to beat the existing CPU DeepFilterNet in quality.

### Stage B: production candidate

Candidate architectures are selected after the operation matrix is
known. The evaluation may include:

- the existing low-latency DeepFilterNet model or a compatible export;
- a reduced recurrent spectral-mask model;
- a convolutional causal denoiser;
- a legally usable vendor-style VDF/VDCRN architecture recreated from
  public descriptions rather than copied proprietary weights.

The model is chosen on measured quality and deadline behavior, not name
recognition.

### Audio pipeline

```text
8-channel capture
  → beamforming
  → acoustic echo cancellation
  → mono 48 kHz / 10 ms frames
  → feature extraction if required
  → NPU denoising model
  → synthesis and wet/dry control
  → normal waved output
```

### Golden and quality validation

The evaluation corpus is frozen before comparing candidates:

- clean speech from multiple speakers;
- stationary HVAC/fan noise;
- keyboard and impulsive noise;
- diffuse room noise;
- reverberant speech;
- near-end speech with residual echo;
- speech-only preservation segments;
- silence.

Required measurements:

- exact or tolerance-bounded host/NPU output comparison;
- segmental SNR and SI-SDR improvement;
- STOI or an equivalent intelligibility metric;
- speech-only RMS preservation;
- transient behavior;
- real-time latency distribution;
- CPU delta and memory use;
- long-run deadline misses.

### Acceptance

Correctness PoC:

- 10,000 consecutive frames complete;
- no missing or reordered output;
- no state drift after a documented reset;
- NPU output matches the host golden within the chosen precision
  tolerance;
- inference p99 is below 5 ms, with total processing below the 10 ms
  frame deadline.

Production candidate:

- improves non-stationary-noise SI-SDR by at least 1 dB over Speex on the
  frozen corpus;
- does not reduce STOI by more than 0.01 relative to Speex;
- preserves speech-only RMS within 1 dB;
- has zero deadline misses in a sustained 30-minute run;
- CPU/Speex fallback activates cleanly when the NPU model is absent or
  rejected;
- subjective listening notes are recorded but do not replace objective
  measurements.

## 16. PoC 8 — Other Relevant Models

Each model is an independent PoC with its own manifest, golden output,
latency report, and production decision.

### Priority order

1. Voice activity detection.
2. Wake-word or command-spotting classifier.
3. Residual echo suppression / acoustic echo suppression.
4. Dereverberation.
5. Speaker embedding and diarization.
6. Audio-event classification.

VAD and wake word come first because small causal classifiers are likely
to use the simplest verified operation subset. Echo suppression and
dereverberation are stateful and require explicit reference/frame
contracts. Speaker embeddings can reuse knowledge from stock
diarization graphs.

### Acceptance template

- licensed and hashed source model;
- documented preprocessing and state;
- host golden;
- Wave-targeted NBG;
- correctness tolerance;
- p50/p95/p99 latency;
- memory and CPU overhead;
- sustained-run behavior;
- fallback semantics;
- model-specific accuracy metric;
- ship, research-only, or rejected decision.

## 17. PoC 9 — Production Runtime

### Objective

Turn research access into a bounded production backend without exposing
unsafe research controls to the main application.

### Components

- model manifest with target, NBG version, tensor contract, precision,
  hash, and expected runtime;
- safe NN SDK library wrapper;
- typed input/output tensor bindings;
- stateful model lifecycle;
- serialized or bounded queue;
- deadline and error metrics;
- deterministic cleanup;
- CPU/Speex fallback;
- health reporting in audio statistics;
- deployment script that writes only under `/data/waved`.

### Isolation

Production code may consume:

- the validated ABI wrapper;
- the safe NBG metadata library;
- model manifests;
- verified runtime configuration.

It may not consume:

- arbitrary ioctl replay;
- raw trace collection;
- malformed-NBG experiments;
- compiler internals;
- device mutation helpers.

### Acceptance

- selected noise model runs in the real audio pipeline;
- backend identity is externally visible;
- fallback is tested by missing, corrupt, and incompatible model cases;
- startup, hot operation, shutdown, and restart all release resources;
- no service or firmware changes are required;
- a full audio acceptance run passes without deadline misses or crash
  loops.

## 18. Documentation Standard

Every PoC produces:

1. a dated lab-notebook entry;
2. exact commands;
3. input and binary hashes;
4. expected and observed output;
5. negative tests;
6. device before/after state;
7. interpretation separated from direct evidence;
8. known limitations;
9. the next decision.

Stable knowledge is promoted from the notebook into:

- `architecture.md` for the software/hardware stack;
- `abi.md` for function and structure contracts;
- `nbg-format.md` for confirmed format fields;
- `compiler.md` for target generation;
- `driver.md` for normalized kernel interactions;
- `operations.md` for support constraints;
- `benchmarks.md` for methodology and results;
- `audio-models.md` for model-specific pipelines;
- `reproduction.md` for a clean-room rerun.

Claims use one of four labels:

- **Observed:** directly measured on the live device.
- **Static evidence:** present in a binary, header, or source file.
- **Inferred:** best explanation of multiple observations.
- **Unknown:** deliberately not claimed.

No inferred field is presented as confirmed format documentation.

## 19. Safety and Device Mutation Policy

### Allowed without a new gate

- host-side static analysis;
- parsing copied firmware and model files;
- cross-building research binaries;
- read-only ADB inspection;
- staging disposable artifacts under `/data/waved/npu-lab/`;
- executing allowlisted, hashed probes with timeouts;
- reading existing sysfs values;
- collecting bounded logs and traces;
- removing only disposable lab artifacts created by this program.

### Requires explicit approval later

- flashing or upgrading firmware;
- replacing a system library;
- loading or unloading a kernel module;
- stopping a normal service solely for an experiment;
- rebooting for recovery;
- writing sysfs clock, power, recovery, or debug parameters;
- raw command-stream replay;
- executing a graph whose target compatibility is not established;
- persistent boot or init-script changes.

### Prohibited in this program

- blind mutation fuzzing against `/dev/galcore`;
- executing product-ID-patched NBGs;
- overwriting stock models;
- destructive filesystem operations;
- automatic driver resets;
- collecting unrelated user audio or credentials.

## 20. Error Handling and Recovery

Failure classes:

- argument or manifest rejection;
- host parse error;
- unsupported NBG version;
- checksum mismatch;
- library load or symbol mismatch;
- graph parse or create rejection;
- tensor contract mismatch;
- input binding failure;
- execution timeout;
- output retrieval or numerical mismatch;
- cleanup failure;
- ADB loss;
- driver or device unresponsiveness.

Every device probe:

1. captures the pre-state;
2. validates all hashes;
3. enters one native stage at a time;
4. reports the last completed boundary;
5. attempts paired cleanup only for successfully created resources;
6. captures the post-state;
7. stops the program if device health changed.

The runner never automatically reboots or resets the driver. A wedged
device is reported as a blocking safety event.

## 21. Testing Strategy

### Host tests

- FFI size and offset assertions;
- bounded parser unit tests;
- full-corpus parser tests;
- truncation at every byte boundary;
- targeted field corruption;
- cross-version fixtures;
- model-manifest validation;
- benchmark-analysis tests;
- golden model inference;
- documentation link and command checks.

### Device tests

- context proof;
- stock graph control;
- custom add gate;
- trace equivalence;
- operation correctness;
- warm/cold benchmark separation;
- repeated process cleanup;
- concurrent-context tests only after serialized stability;
- model correctness;
- sustained real-time audio;
- fallback and restart.

### Regression rule

The stock CTC graph remains the control throughout the program. Any new
runtime, tracer, parser-assisted loader, or production wrapper must
continue to execute it with the known output digest unless the firmware
or model hash intentionally changes.

## 22. Program Gates

### Gate A — Lab trustworthy

PoC 0 passes. No later dynamic experiment runs outside the manifest and
snapshot harness.

### Gate B — Runtime understood

PoCs 1–3 pass. ABI and traces are sufficient to attribute failures.

### Gate C — Custom compilation

PoC 5 add graph passes. Until then, only stock Wave-targeted NBGs execute
on hardware.

### Gate D — Model feasibility

PoC 6 verifies the operations and precision needed by the selected audio
model.

### Gate E — Real-time audio

PoC 7 meets correctness, quality, deadline, stability, and fallback
criteria.

### Gate F — Production

PoC 9 passes the complete audio acceptance suite without privileged
system changes.

## 23. Risks and Mitigations

| Risk | Consequence | Mitigation |
|---|---|---|
| VIP9000 compiler remains proprietary | Custom NBG gate blocked | Pursue simulator config, public SDK differential, Wave components, and legitimate vendor tooling; preserve all evidence |
| NBG v2 differs substantially from public parser | Incorrect format claims | Build bounded parser incrementally and validate every field against stock utilities |
| Driver tracing perturbs timing | Invalid benchmarks | Benchmark with tracing disabled; use traces for control-flow evidence only |
| v3 audio loader is CPU-only | Does not unlock NPU | Prove backend with driver activity; retain result as format/audio research |
| Recurrent model ops unsupported | DFN3-style graph cannot compile | Start with verified causal convolutional/mask model and select production architecture from operation catalog |
| Runtime accepts graph but produces wrong values | Silent incompatibility | Require host goldens and full-output verification for every graph |
| Device wedge | Research interruption | Serialize tests, use strict allowlists/timeouts, stop on health change, require approval for recovery |
| Dirty audio worktree overlaps integration | Accidental data loss | Keep research files isolated and inspect ownership before PoC 9 changes |
| Proprietary binary/model licensing | Cannot redistribute artifacts | Commit hashes, metadata, and tooling; keep restricted binaries out of new distributable artifacts |

## 24. Completion Criteria

The program is complete when:

- the runtime and driver lifecycle are documented from app to hardware;
- NBG versions in the firmware corpus are safely classified;
- the compiler produces a legitimate `0x1000000e` custom add graph;
- supported operations, layouts, precisions, memory behavior, and
  performance are reproducibly characterized;
- a custom neural noise-suppression model runs in real time on the NPU;
- at least two additional relevant audio-model PoCs are evaluated;
- the winning noise model is integrated with safe fallback;
- every result is reproducible from committed commands and manifests;
- no production path depends on research-only mutation or tracing.

## 25. References

- [Inside the M4 Apple Neural Engine, Part 1: Reverse Engineering](https://maderix.substack.com/p/inside-the-m4-apple-neural-engine)
- [Maderix ANE research repository](https://github.com/maderix/ANE)
- [VeriSilicon TIM-VX](https://github.com/VeriSilicon/TIM-VX)
- [Amlogic NN SDK public examples](https://github.com/Amlogic-NN/AML_NN_SDK)
- [eYs3D VIP9000 root filesystem](https://github.com/eYs3D/eCV5546_rootfs)
- [eYs3D VIP9000 kernel sources](https://github.com/eYs3D/eCV5546_linux)
- `docs/REPORT_NPU.md`
- `docs/analysis_reports/NPU_Stack_Inventory.md`
- `waved/docs/COMPUTE_HARDWARE.md`
- `docs/superpowers/specs/2026-07-23-wave-npu-inference-poc-design.md`

## 26. Resolved Assumptions

- The program uses the balanced ordering approved on 2026-07-28.
- Custom compilation, not direct runtime access, is the primary blocker.
- Product ID `0x1000000e` is authoritative even where marketing names
  for the VIP variant differ.
- The current live unit remains on its existing firmware during the
  initial program.
- The v3 audio loader is studied as an optional side path and is not
  assumed to use the NPU.
- The current CPU DeepFilterNet implementation remains the quality and
  fallback baseline.
- Device experiments remain non-destructive until a later explicit
  approval changes the safety boundary.
