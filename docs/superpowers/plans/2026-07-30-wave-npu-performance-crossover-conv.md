# Wave NPU Performance Crossover and Spectral Conv — Implementation Plan

**Date:** 2026-07-30

**Design:**
`docs/superpowers/specs/2026-07-29-wave-npu-performance-crossover-conv-design.md`

**Planning workflow:** manual test-first fallback because the design
workflow's required `writing-plans` skill is unavailable in this session.

## 1. Objective

Execute the approved prerequisite ladder:

1. compare the exact English CTC source/binary anchor through stock ONNX
   Runtime on the Wave CPU and `libnnsdk` on the Wave NPU;
2. only after semantic and performance success, measure the closed
   no-temporal-batching Erf residency curve;
3. only after PoC 15A success and a static Conv profile, try the fixed FP16
   single-hop Conv family;
4. stop at the first clean prerequisite failure and document it.

The end goal is not merely high aggregate throughput. A custom pass requires
the NPU's complete FP32-boundary bridge to beat stock CPU ONNX Runtime for
one logical audio hop, with p99 below 10 ms and no hidden CPU fallback.

## 2. Worktree and execution rules

- Preserve every unrelated tracked and untracked change.
- Use `apply_patch` for source and documentation edits.
- Use formatting tools only for mechanical formatting.
- Run every shell command through `lean-ctx -c '...'` or
  `lean-ctx raw '...'`.
- Stage and commit only exact reviewed paths.
- Keep raw device evidence below ignored `artifacts/npu/`.
- Refuse existing local run directories and existing remote roots.
- Run host tests, deterministic builds, and static policy before ADB.
- Do not use subagents.
- Do not invoke the quarantined CLC.
- Do not mutate firmware, rootfs, `/model`, system libraries, services,
  modules, sysfs, clocks, governors, thermal controls, driver state, or
  stock models.
- Do not construct raw ioctls, command buffers, NN/TP records, patches,
  relocations, or register writes.
- Never retry a failed candidate automatically.

## 3. Phase A — PoC 15A host contracts

### Task 1 — lock the source/binary anchor

Create a strict host profile that independently validates:

```text
ctc.onnx:
  SHA-256 8d6d3b6ce9da51f852981bb5bdd33e2818d47d3e545e9efe5d3700a37b367219
  FLOAT32 [1,16,512] -> FLOAT32 [1,16,4999]
  MatMul [512,4999] -> Add [4999] -> LogSoftmax
  opset 14

ctc_online.nb:
  SHA-256 3a9294ea8595789cc23ddc9298ebceb4474738fed07a9a8b10e2442b67b0d8d5
  VPMN v2, target 0x1000000e
  FP16 [512,16,1] -> FP16 [4999,16,1]
```

Add:

```text
waved/scripts/npu/ctc_cpu_npu_benchmark_profile.py
waved/scripts/npu/tests/test_ctc_cpu_npu_benchmark_profile.py
docs/npu/profile/ctc-cpu-npu-benchmark-profile.json
```

Tests cover model hashes, ONNX nodes/opset, initializers, NBG header,
input/output types and sizes, memory-order equivalence, MAC count, unknown
schema keys, and deterministic regeneration.

### Task 2 — define the fixed benchmark manifest and state machine

Add:

```text
waved/scripts/npu/ctc_cpu_npu_benchmark_manifest.json
waved/scripts/npu/npu_ctc_cpu_npu_benchmark_lab.py
waved/scripts/npu/tests/test_npu_ctc_cpu_npu_benchmark_lab.py
```

Test first:

- exact authorized device, kernel, driver, DSO, model, binary, and source
  hashes;
- fixed remote root `/tmp/wave-npu-poc15a`;
- root-nonexistence and no-symlink staging;
- fixed runner CLI and one authorized benchmark process;
- fixed four-fixture qualification;
- exactly 1 cold, 20 warm-up, and 200 measured pairs;
- alternating CPU/NPU order;
- exact binary-record size and field order;
- output/canary/digest/numerical/timing refusal;
- NPU-native and FP32-bridge timing boundaries;
- no fallback, holder, CLC, health difference, kernel alert, timeout, or
  retry;
- preflight, stage, run, collect, analyze, cleanup-audit, and describe
  operations;
- fake transport only in tests.

The controller independently recomputes all metrics and never trusts a
native summary without matching raw records.

### Task 3 — define the binary evidence format

Use a fixed little-endian format:

```text
header:
  magic/version
  fixture count
  measured record count
  tensor element/byte counts
  fixed seeds and iteration counts

qualification[4]:
  fixture ID
  CPU FP32 digest
  CPU-rounded-FP16 digest
  NPU FP16 digest
  top-1 matches
  finite count
  sum/max absolute error
  cosine dot product and both squared norms

records[200]:
  sequence/order
  CPU OrtRun wall/thread
  NPU FP32->FP16 conversion wall/thread
  NPU input-set wall/thread
  NPU run wall/thread
  NPU output-get-and-copy wall/thread
  NPU FP16->FP32 conversion wall/thread
  NPU native total wall/thread
  NPU bridge total wall/thread
```

The file contains no addresses, raw model data, tensors, weights, or audio.
It is installed atomically with exclusive creation and an exact byte count.
Each qualification record is 88 bytes; the host derives mean absolute error
and cosine similarity from the stored aggregates.

## 4. Phase B — PoC 15A native runner

### Task 4 — acquire and pin the ORT 1.15.1 C header

Use only the official ONNX Runtime `v1.15.1` source header. Record its URL,
tag, SHA-256, and license. Do not download or execute a binary.

Place the exact header below an isolated compiler include directory or use
the existing build cache if byte-identical. The runner must call only the
documented C API rooted at `OrtGetApiBase`.

### Task 5 — implement the fixed CTC runner

Create:

```text
waved/tools/npu-compiler/wave_npu_benchmark_ctc_cpu_vs_npu.cc
waved/tools/npu-compiler/build-device-ctc-cpu-npu-benchmark.sh
waved/tools/npu-compiler/verify-device-ctc-cpu-npu-benchmark-policy.sh
waved/tools/npu-compiler/stage-device-ctc-cpu-npu-benchmark.sh
waved/tools/npu-compiler/cleanup-device-ctc-cpu-npu-benchmark.sh
```

The binary accepts only:

```text
--describe
--benchmark
```

It contains:

- fixed staged ONNX/NBG and result paths;
- exact model and library digests;
- fixed tensor shapes and byte counts;
- firmware-matched `libnnsdk` ABI layouts with compile-time size/offset
  assertions;
- stock ORT CPU provider only;
- preallocated ORT input/output tensors;
- preallocated canary-guarded FP16 and FP32 NPU buffers;
- deterministic FP16-exact fixtures and measured inputs;
- one ORT session and one NPU module;
- exact IEEE-754 binary16 conversion;
- the four numerical gates;
- one cold pair, 20 warm-up pairs, and 200 measured pairs;
- alternating execution order;
- wall/thread/process clocks;
- parent-death signal and a fixed native watchdog;
- atomic non-overwriting timing/result output;
- graph/module cleanup and exact result framing.

The runner must materialize the complete NPU output in guarded local memory.
It may not time a pointer return without copying the reported bytes.

### Task 6 — write the native static policy

Require:

- AArch64 ELF64 PIE, full RELRO, immediate binding, non-executable stack;
- fixed RPATH/library roots only;
- exactly two fixed modes;
- exact paths, hashes, shapes, seeds, and counts;
- `OrtGetApiBase` and the closed `libnnsdk` symbol set;
- no alternate ORT provider registration;
- no arbitrary model/path/shape/count argument;
- required clocks, watchdog, parent-death, canaries, copy, and cleanup;
- no CPU CTC implementation outside stock ORT;
- no shell, ADB, service, module, sysfs-write, raw ioctl, reset, CLC,
  firmware, stock-file write, retry, or recovery surface.

Policy tests inspect both source and binary. Comments do not exempt a
forbidden token.

### Task 7 — deterministic cross-build and host gate

Build twice from the same source, header, sysroot, and toolchain. Require
identical SHA-256.

Before staging:

- all new unit tests pass;
- existing PoC 13G and generic lab tests pass;
- Ruff, formatting, shell syntax, and `git diff --check` pass;
- profile regeneration is byte-identical;
- policy verification passes;
- the final manifest contains exact source, binary, model, header, and DSO
  hashes;
- a dry-run ledger proves exactly one eligible PoC 15A device process.

## 5. Phase C — PoC 15A Wave qualification

### Task 8 — preflight and stage

Verify:

- one authorized Wave;
- expected boot, kernel, driver, NPU, product, and customer identities;
- exact stock `/usr/lib/libonnxruntime.so.1.15.1`, `libnnsdk.so`, and
  dependent runtime hashes;
- no galcore holder;
- no admitted CLC path;
- no prior root or output;
- no process matching any PoC 15 runner.

Stage only the fixed runner, ONNX, NBG, manifest, and required private
libraries below `/tmp/wave-npu-poc15a`. Make payload files read-only.

### Task 9 — execute exactly once

Run one fixed process under host and native deadlines.

The process:

1. creates both sessions;
2. qualifies four fixtures;
3. stops immediately on semantic failure;
4. otherwise performs the recorded initial/warm/measured pairs;
5. writes one timing artifact and one result;
6. destroys both sessions and exits.

No retry is permitted. On any failure, capture stdout/stderr, preflight,
postflight, output absence/presence, holders, kernel deltas, and health.

### Task 10 — independent analysis and decision

The host:

- validates exact timing bytes and hashes;
- recomputes numerical gates, percentiles, and paired ratios;
- distinguishes NPU run, native total, and FP32 bridge total;
- reports CPU wall/thread/process consumption;
- records cold versus steady-state behavior;
- records temperature and observed CPU/NPU clock state without changing it;
- assigns `semantic-negative`, `cpu-faster`, or `npu-faster`.

PoC 15A must pass semantics and the bridge-total performance thresholds
before any PoC 15B/15C device work.

### Live-attempt addendum — 2026-07-30

The first live process, run
`poc15a-ctc-crossover-20260730-a1`, is an invalid run rather than a
candidate result. All four emitted qualification summaries passed, but
source review found that the runner:

- reapplied the strict qualification gate to performance inputs;
- used a nine-bit `[-1,1)` random mapping instead of the approved eight-bit
  `[-0.5,0.5)` mapping;
- placed the impulse at the flat-tensor center instead of index zero;
- recorded its initial pair before qualification.

It exited 19 before publishing any timing artifact. No retry was attempted,
postflight was clean, and the fixed remote root was removed after audit.

PoC 15A-R2 corrects those four deviations, uses remote root
`/tmp/wave-npu-poc15a-r2` and timing schema `CCN2-v2`, and is
host-qualified. The user explicitly authorized one corrected process, run
`poc15a-r2-ctc-crossover-20260730-a1`.

R2 completed with native exit zero, one execution attempt, valid semantic
qualification, a 27,752-byte timing artifact, and a clean postflight. The
200-pair steady-state means were:

```text
stock ORT CPU:   4.658214 ms
NPU native FP16: 4.492492 ms
NPU FP32 bridge: 5.154930 ms
```

The bridge/CPU ratio of means was `1.106632`, paired-ratio p50 was
`1.106377`, and paired-ratio p95 was `1.128194`. All three precommitted
NPU-win gates failed, so the valid decision is `cpu-faster`. Independent
analysis of the pulled `CCN2-v2` binary reproduced the controller result.

The accelerator `run` interval alone averaged 2.788182 ms and the native
FP16 boundary was 3.558% faster than CPU by ratio of means. The complete
FP32 bridge was nevertheless 10.663% slower because full output retrieval
and FP16-to-FP32 conversion erased that gain. Those narrower results do not
override the application-boundary gate.

No PoC 15B or PoC 15C process is eligible under this plan. The fixed R2
cleanup command was attempted after evidence collection, but ADB listed no
device and its serial guard exited before deletion. Cleanup of only
`/tmp/wave-npu-poc15a-r2` remains pending; the benchmark is non-replayable.

## 6. Phase D — PoC 15B Erf residency curve

**Outcome:** stopped before implementation and device execution because PoC
15A-R2 did not pass its complete FP32-bridge performance prerequisite.
Tasks 11–13 remain design records only. Zero PoC 15B compile or execution
processes were used.

### Task 11 — implement the closed graph generator

Only after PoC 15A passes, add fixed compiler modes for:

```text
w1-d1  w2-d1  w4-d1  w8-d1  w1-d4  w1-d16  w1-d32
```

The first layer uses the PoC 13 quantization. Later resident layers consume
and emit scale `0.0078125`, zero point `128`. Create independent CPU LUTs
from the mathematical quantization contract.

Add compile/execute manifests, controller tests, native policies, build,
stage, and cleanup scripts. There is no generic width/depth CLI.

### Task 12 — host/static qualification

For every still-eligible candidate:

- compile twice identically;
- parse VPMN strictly;
- verify target, tensor contract, section/checksum integrity, and output
  absence;
- prove one dynamic input/output and transient intermediates;
- verify exact CPU oracle for every depth;
- freeze separate artifact and execution manifests.

At most seven compile-only processes are authorized.

### Task 13 — bounded Wave curve

Execute only strict accepted artifacts, in table order, with:

```text
1 cold
100 warm-up
2,000 measured pairs
```

At most seven execution processes are authorized. Stop an equivalent family
after compiler, execution, numerical, or health failure. Stop larger
candidates after the first stable crossover.

Publish the complete attempted curve and clearly label it synthetic.

## 7. Phase E — PoC 15C custom FP16 Conv

**Outcome:** stopped before implementation and device execution because PoC
15A-R2 did not pass its complete FP32-bridge performance prerequisite.
Tasks 14–17 remain design records only. Zero PoC 15C device processes were
used.

### Task 14 — generate the static Conv profile

Only after PoC 15A passes, create:

```text
waved/scripts/npu/conv2d_fp16_profile.py
waved/scripts/npu/tests/test_conv2d_fp16_profile.py
docs/npu/profile/conv2d-fp16-compiler-profile.json
```

Bind:

- exact TIM-VX `CONV2D` checker entry admitting F16/F16;
- exact `vxConvolutionLayer` construction path;
- source and device DSO hashes;
- WHCN shapes, SAME padding, stride/dilation, weights, and bias;
- prior FLOAT32 negative and why this candidate is distinct;
- direct-graph execution only, with no serializer claim.

A static contradiction blocks the live family.

### Task 15 — generate equivalent fixed models

Generate deterministic:

```text
c1: 1 layer, 369,408 MACs
c4: 4 layers, 1,477,632 MACs
c8: 8 layers, 2,955,264 MACs
```

For each depth:

- emit a FLOAT32 ONNX CPU model;
- emit a native FP16 constant blob;
- verify coefficients against the design formula;
- verify a host FP16 graph oracle against ONNX;
- freeze model/constant/contract hashes.

### Task 16 — implement the direct-graph benchmark

Create a fixed TIM-VX runner and host controller with only `c1`, `c4`, and
`c8` modes. Each process constructs and compiles one graph once, runs
untimed numerical qualification, proves VIP submission with the approved
metadata-only tracer, then benchmarks:

```text
1 cold
50 warm-up
2,000 measured pairs
```

CPU uses stock ORT FLOAT32. NPU uses FP16 with bridge conversions included.
No output passes without numerical, canary, driver-submission, fallback,
timing, and health gates.

### Task 17 — enforce candidate escalation

- Run only `c1` first.
- If `c1` cannot compile, verify, execute, or qualify numerically, stop.
- If `c1` passes but is slower, permit `c4`.
- If `c4` passes but is slower, permit `c8`.
- Stop at the first bridge-total performance pass.
- Never exceed three direct-graph processes.

## 8. Verification matrix

| Boundary | Required proof |
|---|---|
| Source/binary identity | exact hashes, ONNX graph, strict NBG profile |
| Logical equivalence | fixed layout map, four fixtures, predeclared metrics |
| CPU identity | stock ORT hash, CPU provider only, preallocated tensors |
| NPU identity | product/driver/DSO hashes, VIP submission, no CPU fallback |
| Timing integrity | raw fixed records, independent host recomputation |
| Practical comparison | FP32 bridge total, paired order, default CPU threads |
| Single-hop custom result | no temporal batching, p99 < 10 ms |
| Device safety | deadlines, holder/health/kernel/CLC gates, no retry |
| Reproducibility | deterministic builds, manifests, append-only evidence |

## 9. Documentation and commits

Use small exact commits:

1. implementation plan;
2. PoC 15A host profile/controller/tests;
3. PoC 15A native runner/policy/build;
4. PoC 15A Wave evidence and documentation;
5. PoC 15B implementation/evidence only if eligible;
6. PoC 15C implementation/evidence only if eligible;
7. reports, reproduction guide, and completion audit.

Update:

```text
docs/npu/README.md
docs/npu/reproduction.md
docs/npu/lab-notebook/README.md
docs/npu/performance/
docs/npu/profile/
docs/REPORT_NPU.md
docs/REPORT_AI_MODELS.md
waved/docs/COMPUTE_HARDWARE.md
waved/scripts/npu/README.md
```

Each final notebook records hypotheses, exact commands, hashes, call ledger,
raw and derived metrics, health evidence, limits, and the next gated
decision. A clean negative is a completed result, not permission to mutate
the graph or retry.

The final PoC 15A evidence is recorded in:

```text
docs/npu/lab-notebook/2026-07-30-poc15a-ctc-cpu-vs-npu.md
docs/npu/performance/ctc-wave-cpu-vs-npu.json
artifacts/npu/runs/poc15a-r2-ctc-crossover-20260730-a1/
```
