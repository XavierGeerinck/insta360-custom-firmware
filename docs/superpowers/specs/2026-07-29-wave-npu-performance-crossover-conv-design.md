# Wave NPU Performance Crossover and Spectral Conv Design

**Date:** 2026-07-29
**Status:** approved design; implementation not started
**Scope:** PoC 15A through PoC 15C
**Target:** Insta360 Wave, product/customer ID `0x1000000e`
**Primary question:** can the Wave NPU beat the Wave CPU without temporal
batching, first on a known substantial graph and then on a custom
single-audio-hop graph?

## 1. Decision

PoC 13G is not evidence that the NPU is generally slow. It compared a
256-byte CPU lookup table over 481 values with a synchronous accelerator
submission:

```text
CPU wall:       3.303 us
NPU Run:       68.330 us
NPU total:     86.338 us
```

The current NPU boundary cannot make that isolated lookup faster. Input and
output transfer alone averaged 14.841 us, before NPU submission.

PoC 15 will answer the broader question in three gated stages:

1. **PoC 15A:** compare a source/binary-paired stock CTC graph through stock
   ONNX Runtime on the Wave CPU and `libnnsdk` on the Wave NPU.
2. **PoC 15B:** measure a bounded no-temporal-batching Erf width/residency
   curve using only the already qualified operation family.
3. **PoC 15C:** try one statically justified FP16 spectral Conv signature,
   escalating its resident layer count only after the one-layer graph
   compiles and executes correctly.

PoC 15D, training and qualifying a production noise-suppression model, is a
separate design. It may start only if PoC 15C produces a numerical and
end-to-end performance pass.

## 2. Success definition

The user selected **single-hop latency**, not aggregate throughput obtained
by batching successive audio hops.

A custom-graph speed claim requires all of the following:

- one logical audio hop per invocation;
- no temporal batching;
- one graph/session construction before timing;
- one input transfer and one output transfer per invocation;
- NPU-resident intermediate tensors;
- no CPU implementation or fallback inside the NPU measurement;
- identical deterministic logical inputs;
- predeclared numerical agreement;
- NPU bridge-total ratio of means at most `0.90`;
- paired NPU/CPU ratio p50 at most `0.90`;
- paired NPU/CPU ratio p95 below `1.00`;
- NPU bridge-total p99 below the 10 ms audio-hop deadline;
- no device-health, kernel, holder, timeout, or canary failure.

“Bridge total” includes any FP32-to-FP16 conversion, NPU input submission,
graph execution, output retrieval, and FP16-to-FP32 conversion required to
present the same FP32 boundary as the CPU implementation.

NPU-native timing is also reported, but it cannot by itself establish a
hybrid-pipeline speedup.

PoC 15A uses a native 16-step CTC invocation and therefore does not claim a
10 ms denoiser-hop result. It establishes whether the NPU is worthwhile for
a substantial real graph. PoC 15C is the stage that can satisfy the
single-hop custom-model definition.

## 3. Approaches considered

### 3.1 Stock CTC source/binary pair

This is the strongest first proof because it compares a real dense model on
both compute engines without requiring the unavailable Acuity compiler.
Its limit is that the NBG is stock, not repository-generated.

### 3.2 Synthetic Erf scaling only

This safely measures submission and residency crossover with an operation
already generated and executed by repository code. Its limit is that
repeated lookup-table work is not a useful neural model and can be
algebraically composed by a CPU implementation.

### 3.3 Direct custom Conv first

This is closest to a useful denoiser, but the earlier FLOAT32 Conv probe
failed graph compilation. Starting here would confound accelerator
capability with compiler/kernel compatibility.

The selected order is 3.1, then a bounded 3.2 diagnostic, then 3.3.

## 4. Shared measurement rules

### 4.1 Timing

Native timing uses `CLOCK_MONOTONIC_RAW`. CPU-thread and process consumption
use `CLOCK_THREAD_CPUTIME_ID` and `CLOCK_PROCESS_CPUTIME_ID`.

Every paired measurement uses the same logical input. Execution order
alternates:

```text
even pair: CPU -> NPU
odd pair:  NPU -> CPU
```

Session creation, model parsing, graph construction, graph compilation,
initial allocation, and metadata discovery occur before cold timing. They
are recorded separately and excluded from steady-state inference.

All inference buffers are allocated before timing. Each path writes into
canary-guarded storage. Output hashing and cross-path comparison occur
outside the timed interval, after the complete output has been materialized.

Reports contain mean, minimum, p50, p95, p99, p99.9 when sample count
permits, maximum, standard deviation, ratio of means, paired-ratio
percentiles, cold latency, CPU time, wall time, and isolated throughput.
Percentiles use nearest rank, matching PoC 13G.

### 4.2 CPU baseline

The CPU path uses the device's stock
`/usr/lib/libonnxruntime.so.1.15.1`, whose extracted copy has SHA-256:

```text
457784da0d1c525171b15438cfbaf869f57e9317817e30691136b8ebe10d4639
```

The runner resolves the ONNX Runtime C API through `OrtGetApiBase` and
refuses any version or library digest mismatch. Only
`CPUExecutionProvider` is admitted. Graph optimization is enabled. Default
stock thread-pool behavior is retained and recorded, so the NPU is compared
with the practical four-Cortex-A55 baseline rather than a deliberately
weakened single-thread implementation.

Input and output tensors are preallocated and reused. Per-inference model
loading, session construction, heap allocation, file I/O, logging, hashing,
and fixture generation are forbidden inside the timed CPU interval.

### 4.3 NPU baseline

The NPU path uses the firmware-matched `libnnsdk` ABI already proven by the
stock control, or the fixed TIM-VX direct-graph ABI in PoCs 15B/15C. Each
runner validates:

- device identity and boot ID;
- `npu_version`, driver, product, and customer identity;
- exact DSO, model, runner, and manifest digests;
- input/output count, type, shape, and byte count;
- no unexpected galcore holder;
- fixed remote paths and clean output state.

Input conversion, input submission, NPU execution, output retrieval, output
conversion, and bridge total are measured as separate intervals.

### 4.4 Fail-closed behavior

Any malformed timing record, nonfinite duration, shape/type disagreement,
numerical failure, output canary change, unexpected output digest,
timeout, holder, kernel alert, health difference, or artifact mismatch makes
the run non-qualifying. A failure never triggers an automatic retry,
recovery, service restart, device reset, graph mutation, or broader
candidate search.

## 5. PoC 15A — stock CTC CPU versus NPU

### 5.1 Fixed source/binary anchor

The compared files are:

```text
ONNX:
  models/ai-models/asr-models/model/asr_en/reverb_asr_online/ctc.onnx
  SHA-256 8d6d3b6ce9da51f852981bb5bdd33e2818d47d3e545e9efe5d3700a37b367219

NBG:
  models/ai-models/asr-models/model/asr_en/reverb_asr_online/ctc_online.nb
  SHA-256 3a9294ea8595789cc23ddc9298ebceb4474738fed07a9a8b10e2442b67b0d8d5
```

The ONNX graph is:

```text
input:       FLOAT32 [1,16,512]
MatMul:      weights FLOAT32 [512,4999]
Add:         bias FLOAT32 [4999]
LogSoftmax
output:      FLOAT32 [1,16,4999]
opset:       14
matrix MACs: 40,951,808 per invocation
```

The NBG is VPMN v2 for `0x1000000e`:

```text
input:   FP16 [512,16,1], 16,384 bytes
output:  FP16 [4999,16,1], 159,968 bytes
file:    5,263,360 bytes
```

The layout adapter maps the two representations without reordering logical
`[time, feature]` or `[time, class]` elements. All generated inputs are
exactly representable as FP16. The CPU receives their exact FP32 expansion.

### 5.2 Numerical qualification

Before timing, one process evaluates four immutable fixtures:

1. all zeros;
2. one `0.5` impulse at logical `[time=0, feature=0]`;
3. a deterministic striped tensor using values
   `{-0.5, -0.25, 0, 0.25, 0.5}`;
4. xorshift32 seed `0x573315a1`, mapped to multiples of `1/256` in
   `[-0.5, 0.5]`.

NPU FP16 output is expanded to FP32. CPU output is additionally rounded to
FP16 and expanded, allowing output-storage quantization to be separated
from compute drift.

Each fixture must satisfy:

- exact element count and finite values;
- all 16 per-time-step top-1 class indices equal;
- mean absolute error against FP16-rounded CPU output at most `0.025`;
- maximum absolute error at most `0.25`;
- cosine similarity at least `0.999`;
- deterministic output digest across two untimed NPU qualification runs.

Failure means the source/binary anchor is not numerically qualified. No
performance claim or PoC 15B/15C device work follows.

### 5.3 Performance run

The performance input sequence uses the same fixed xorshift mapping with a
distinct seed, `0x573315b1`.

One process performs:

```text
cold pairs:       1
warm-up pairs:   20
measured pairs: 200
```

The CPU interval is preallocated `OrtRun`. NPU-native total includes
`aml_module_input_set`, `PERF_MODE_RUN`, output retrieval, and copying the
reported output bytes into guarded storage. NPU bridge total additionally
includes FP32/FP16 boundary conversion in both directions.

PoC 15A passes semantically when qualification passes. It establishes an
NPU performance win only when the shared ratio gates pass for bridge total.
If semantics pass but speed does not, the result is retained as a clean
performance negative and later device stages stop.

## 6. PoC 15B — no-temporal-batching Erf crossover

PoC 15B begins only after a semantic and performance pass in PoC 15A.

### 6.1 Candidate grid

All candidates use the already qualified U8 Erf operation family. The first
layer retains the exact PoC 13 quantized contract:

```text
input:   UINT8, scale 0.03125, zero point 128
output:  UINT8, scale 0.0078125, zero point 128
```

For resident depth greater than one, every later layer consumes and emits
`UINT8` at scale `0.0078125`, zero point `128`. The CPU oracle therefore
uses the PoC 13 lookup for layer one and an independently generated
same-scale Erf lookup for later layers. It does not incorrectly reuse one
table across incompatible quantization contracts.

The seven unique fixed graphs are:

| Candidate | Spatial channels | Width | Resident Erf depth |
|---|---:|---:|---:|
| `w1-d1` | 1 | 481 | 1 |
| `w2-d1` | 2 | 962 | 1 |
| `w4-d1` | 4 | 1,924 | 1 |
| `w8-d1` | 8 | 3,848 | 1 |
| `w1-d4` | 1 | 481 | 4 |
| `w1-d16` | 1 | 481 | 16 |
| `w1-d32` | 1 | 481 | 32 |

Width represents independent same-hop channels. Eight is the physical Wave
microphone count. It never represents successive time hops.

Depth variants have one dynamic input, one dynamic output, and transient
NPU-resident intermediate tensors. The CPU comparator intentionally applies
the same runtime lookup once per graph node. These variants characterize
submission/residency behavior only; they are not a production-model claim,
because a CPU implementation could compose repeated lookup tables.

### 6.2 Compilation and execution caps

Each candidate has a fixed compile mode and distinct final/partial artifact
path. At most seven compile-only processes and seven execution processes
may run. A type, setup, backend, export, or health failure stops all
known-equivalent candidates in that family.

Every accepted artifact must pass the strict VPMN parser, exact target and
tensor contract, checksum, section, padding, and output-absence gates before
execution.

Each executed candidate performs:

```text
cold pairs:          1
warm-up pairs:     100
measured pairs:   2,000
```

All bytes must match the independently applied CPU LUT at every depth.
The first candidate to meet the shared bridge-total performance gates is
recorded as the measured synthetic crossover. All predeclared smaller
candidates remain in the curve; larger candidates need not run after a
stable crossover is established.

## 7. PoC 15C — custom FP16 spectral Conv

PoC 15C begins only after PoC 15A proves that substantial NPU execution
beats stock CPU ONNX Runtime. PoC 15B is diagnostic and does not gate Conv.

### 7.1 Static hypothesis

The pinned TIM-VX `CONV2D` checker explicitly admits:

```text
input FLOAT16, weights FLOAT16
```

The OpenVX backend constructs `vxConvolutionLayer`. Stock Wave NBGs prove
that FP16 neural graphs execute on this silicon. The earlier direct Conv
negative used a tiny FLOAT32 graph; it does not answer this FP16,
multi-channel, audio-shaped candidate.

Before device work, a machine-readable profile must bind the exact checker
entry, kernel selector, OpenVX construction path, TIM-VX source digest,
device DSO digests, and the candidate types and shapes. A static
contradiction blocks the live candidate.

### 7.2 Fixed graph family

Every layer uses TIM-VX WHCN layout:

```text
input/output: FLOAT16 [481,1,16,1]
weights:      FLOAT16 [3,1,16,16]
bias:         FLOAT16 [16]
padding:      SAME
stride:       [1,1]
dilation:     [1,1]
MACs/layer:   369,408
```

Weights are deterministic FP16 constants. For weight
`W[kx, 0, input_channel, output_channel]`, with channels modulo 16:

```text
W[1,0,c,c]          =  0.5
W[0,0,c,c]          =  0.125
W[2,0,c,c]          =  0.125
W[1,0,c,(c+1)%16]   =  0.0625
W[1,0,c,(c+15)%16]  = -0.03125
all other weights   =  0
all biases          =  0
```

These powers of two are exactly representable in FP16. The fixed formula
bounds activation growth, makes the CPU oracle stable, and prevents
all-zero or identity-only work. A generated manifest records every
coefficient digest and the equivalent FLOAT32 ONNX model.

The closed family is:

| Candidate | Resident layers | MACs |
|---|---:|---:|
| `c1` | 1 | 369,408 |
| `c4` | 4 | 1,477,632 |
| `c8` | 8 | 2,955,264 |

Only `c1` may be attempted initially. If `c1` fails graph compilation,
verification, execution, or numerical qualification, `c4` and `c8` are
known-equivalent backend retries and are prohibited.

If `c1` passes numerically but does not beat CPU bridge total, `c4` may run.
If `c4` also passes numerically but does not beat CPU, `c8` may run. The
first performance pass stops escalation.

### 7.3 Execution architecture

The NPU runner constructs one direct in-memory TIM-VX graph at process
startup, calls the ordinary graph compile/verification path once, and then
reuses the graph. It does not require NBG export. All `c4`/`c8` intermediate
tensors remain within the graph.

The CPU path uses a generated FLOAT32 ONNX graph with identical logical
layout, weights, padding, and layer count through stock ONNX Runtime.

The logical input is one 481-bin, 16-feature tensor for one audio hop. No
time axis is batched. The performance runner uses:

```text
cold pairs:          1
warm-up pairs:      50
measured pairs:   2,000
```

### 7.4 Numerical gates

Inputs use bounded FP16-exact deterministic values in `[-0.5, 0.5]`. NPU
FP16 output is compared with the FLOAT32 CPU oracle.

For depth `d`, qualification requires:

- exact shape and finite output;
- mean absolute error at most `0.002 * d`;
- maximum absolute error at most `0.02 * d`;
- cosine similarity at least `0.999`;
- identical NPU output across two untimed executions;
- input/output canaries intact.

Performance qualification additionally requires the shared single-hop
bridge-total gates. Only a candidate passing both numerical and performance
gates proves that a custom Wave NPU graph beats the Wave CPU.

An untimed qualification execution must also show the ordinary VIP
submission sequence through the existing metadata-only runtime tracer and
must contain no CPU-fallback registration or execution marker. The tracer
may record only the already approved driver envelope and command labels; it
must not capture addresses, command unions, buffers, model data, weights, or
tensors. A graph that returns numerically correct output without proving
VIP submission is not an NPU performance pass.

## 8. Device safety and call ledger

All live work uses new fixed roots:

```text
/tmp/wave-npu-poc15a
/tmp/wave-npu-poc15b
/tmp/wave-npu-poc15c
```

Staging refuses an existing root. Payloads are hash-pinned, non-overwriting,
and read-only after installation. Cleanup may remove only its exact verified
root after:

- process and galcore-holder absence;
- expected directory membership;
- final/partial artifact accounting;
- evidence capture;
- postflight health comparison.

Allowed:

- normal `libnnsdk`, ONNX Runtime, TIM-VX, OpenVX, and ovxlib APIs;
- read-only identity, process, holder, temperature, clock, and kernel-log
  observations;
- writes below the three exact temporary roots;
- bounded compile, graph verification, inference, and cleanup described
  here.

Forbidden:

- firmware, rootfs, `/model`, system-library, or stock-model mutation;
- service stop/start/restart;
- module unload/reload;
- sysfs writes, governor/clock changes, reset, reboot, or recovery;
- raw ioctl construction or replay;
- direct command-buffer, NN/TP, patch, relocation, or register mutation;
- loading or calling the quarantined version-adjacent `libCLC.so`;
- unbounded shape, datatype, layer, or retry search;
- fallback inside a measured NPU interval.

Maximum new live processes:

| Stage | Compile-only processes | Execution/direct-graph processes |
|---|---:|---:|
| PoC 15A | 0 | 1 |
| PoC 15B | 7 | 7 |
| PoC 15C | 0 | 3 |

The maximum is a cap, not a target. Prerequisite failures skip downstream
calls.

## 9. Host implementation boundaries

The implementation is split into independently testable units:

1. **Artifact/manifest generator**
   - validates model hashes and tensor contracts;
   - creates fixed deterministic inputs and Conv constants;
   - emits canonical JSON manifests.
2. **Native runners**
   - contain only fixed modes and paths;
   - implement ABI, timing, canary, watchdog, and result framing;
   - cannot select arbitrary models, shapes, layer counts, or iterations.
3. **Host lab controller**
   - stages exact payloads;
   - performs preflight/postflight and process deadlines;
   - parses fixed binary timing records;
   - never retries.
4. **Analyzer**
   - recomputes all statistics and numerical gates;
   - emits machine-readable JSON and Markdown tables;
   - rejects unknown, missing, duplicate, or inconsistent records.
5. **Static policy verifier**
   - checks ELF hardening, imports, paths, operation counts, fixed constants,
     and prohibited execution/fallback surfaces before any stage.

The existing PoC 13G binary record format and host percentile implementation
are reused where their semantics match. Stock `waved-npu-probe` ABI layouts
are reused rather than independently guessed.

## 10. Testing

Host tests must cover:

- model/hash/layout admission;
- FP16-exact input generation;
- ONNX/NBG dimension mapping;
- numerical metrics and every threshold boundary;
- nearest-rank percentiles and paired ratios;
- timing-record truncation, duplication, overflow, zero, and nonfinite
  rejection;
- candidate/prerequisite state transitions;
- call-cap enforcement and no-retry behavior;
- canary, output-length, digest, and tensor-contract failures;
- process, health, CLC, holder, and kernel-alert gates;
- Conv coefficient determinism and ONNX/TIM-VX equivalence;
- static source/ELF policy;
- non-overwriting artifact and run-directory behavior.

Before device work:

- all focused host tests pass;
- existing PoC 13G and generic NPU lab regressions pass;
- lint, formatting, shell syntax, and `git diff --check` pass;
- native builds reproduce bit-for-bit;
- all runner and payload hashes are frozen in reviewed manifests;
- a dry-run ledger enumerates every still-eligible device process.

## 11. Evidence and documentation

Each stage writes an append-only run directory below:

```text
artifacts/npu/runs/
```

The directory includes the frozen manifest, stdout/stderr, binary timing
records, parsed results, numerical comparison, preflight/postflight,
artifact hashes, process ledger, and final decision.

Tracked outputs include:

```text
docs/npu/performance/ctc-wave-cpu-vs-npu.json
docs/npu/performance/erf-residency-crossover.json
docs/npu/performance/conv-wave-cpu-vs-npu.json
docs/npu/profile/conv2d-fp16-compiler-profile.json
docs/npu/lab-notebook/2026-07-29-poc15a-stock-ctc-cpu-vs-npu.md
docs/npu/lab-notebook/2026-07-29-poc15b-erf-residency-crossover.md
docs/npu/lab-notebook/2026-07-29-poc15c-fp16-spectral-conv.md
```

Documentation must distinguish:

- stock-graph hardware performance;
- synthetic supported-operation crossover;
- custom direct-graph performance;
- NPU-native versus hybrid bridge timing;
- mean throughput versus p99 single-hop latency;
- compiler compatibility from silicon capability;
- a performance carrier from a trained noise-cancellation model.

## 12. Exit decisions

Possible terminal conclusions are explicit:

1. **CTC semantic negative:** the presumed ONNX/NBG pair is not numerically
   equivalent; no CPU/NPU speed claim follows.
2. **CTC performance negative:** equivalent substantial graph, but NPU bridge
   does not beat stock CPU ORT; later live stages stop.
3. **Hardware performance pass only:** stock CTC wins, but custom FP16 Conv
   cannot compile or execute.
4. **Synthetic crossover only:** supported Erf graphs cross over, but that
   does not qualify a useful model.
5. **Custom Conv numerical pass, performance negative:** custom NPU compute
   works, but this boundary is not worth offloading.
6. **Custom Conv performance pass:** one single-hop direct graph beats stock
   CPU ORT under the predeclared bridge, percentile, numerical, and health
   gates.

Only decision 6 authorizes a separate PoC 15D design for a trained fused
noise-suppression model. No outcome globally enables arbitrary ONNX or
arbitrary NPU graphs.
