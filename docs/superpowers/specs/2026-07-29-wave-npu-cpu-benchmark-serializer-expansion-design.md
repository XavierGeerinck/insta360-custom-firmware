# Wave CPU/NPU Benchmark and Serializer Expansion Design

**Date:** 2026-07-29

**Status:** PoC 13G and 14A passed; PoC 14B closed clean negative; PoCs
14C–14H blocked by the prerequisite ladder

**Parent designs:**

- `2026-07-28-wave-npu-reverse-engineering-program-design.md`
- `2026-07-28-wave-npu-open-nbg-serializer-design.md`
- `2026-07-29-wave-npu-learned-erf-noise-suppression-design.md`

**Selected sequence:**

1. PoC 13G measures the same quantized Erf inference on the Wave
   Cortex-A55 CPU and Wave NPU.
2. PoC 14 expands NPU residency through a serializer-first affine ladder,
   then attempts a minimal learned spectral Conv partition.

## Implementation result

PoC 13G passed but was performance-negative: on 10,000 same-device paired
frames, the Wave Cortex-A55 lookup was 20.686 times faster than NPU
`Run()` and 26.137 times faster than the complete NPU boundary by ratios of
means.

PoC 14A then froze and quality-qualified a useful U8-feature,
per-channel-INT8 affine-plus-Erf partition. Its model contract is
`b52ddaf3baed2a7001ff10cbf9ba0600945749a2332f02d71d2ae0be25ac98b8`.

PoC 14B consumed one of the six permitted compile-only processes. Baseline
A1 failed cleanly while checking its first `MULTIPLY` node:

```text
Inputs/Outputs data type not support: ASYM UINT8, SYMM PC INT8
```

The pinned source explains the result: `op_check_mul` delegates to ADD's
finite 59-pair input-type registry, which contains no per-channel pair. No
NBG was emitted, health remained unchanged, and the exact stage root was
removed. A2/B1/B2/C1/C2 were skipped because changing a coefficient cannot
change the rejected tensor signature and would be a known-equivalent retry.

PoC 14C therefore has no coefficient differential to analyze. Semantic
emission, affine execution, performance qualification, and Conv expansion
remain blocked. The existing CPU-affine/NPU-Erf hybrid denoiser remains the
working endpoint.

## 1. Decision

The current Erf result proves custom learned NPU execution, but it does not
prove a performance win. Its NPU timing measures only `graph->Run()`, while
the CPU fallback has not been timed on the Wave CPU. PoC 13G closes that
measurement gap before production integration or broader performance claims.

PoC 14 then targets the smallest useful increase in learned NPU residency:

```text
stage A:
  quantized feature [481]
    -> learned per-bin affine
    -> Erf
    -> quantized mask [481]

stage B:
  quantized feature [481]
    -> learned spectral Conv1D
    -> learned per-bin affine
    -> Erf
    -> quantized mask [481]
```

The affine stage comes first because it reuses the already qualified model
and exercises constant packing, elementwise operation records, command/state
ownership, patching, and relocation with less structural risk than Conv.
Conv work starts only after those serializer mechanisms are independently
proven.

The current Python/lab denoiser remains the quality oracle. Production Rust
`NpuInfer` integration is deferred until the same-device evidence shows that
a candidate partition has a useful latency or CPU-load advantage.

## 2. Current Evidence

The following facts are already proven:

- the Wave is an Amlogic A5/A113X2-class platform with Cortex-A55 CPU cores
  and a VeriSilicon VIP-class NPU;
- the exact Wave runtime generated a deterministic 5,840-byte VPMN v2 NBG
  for target `0x1000000e`;
- the approved NBG implements U8 `[481]` Erf and has SHA-256
  `96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94`;
- six fixed vectors matched an independent CPU oracle for all 2,886 bytes;
- the complete 1,201-frame fixture matched for all 577,681 bytes;
- a 10,000-frame NPU soak matched all host-oracle checkpoints;
- the 10,000-frame run reported 84.8758 microseconds mean, 74 microseconds
  minimum, and 1,649 microseconds maximum for `graph->Run()` only;
- the same remote process took 1,180 ms for the full 10,000-frame command,
  including input generation, tensor copies, graph lifecycle, digesting,
  and output reporting;
- the CPU implementation is a fixed 256-entry lookup, but its Wave CPU
  latency has not been measured;
- section-20/section-26 DYNAIO and the VPMN v2 file CRC are recovered;
- unchanged selected NBGs have complete byte ownership and round-trip
  exactly;
- coefficient packing, operation emission, executable command/state
  generation, patch records, and relocation construction remain incomplete;
- ordinary Add, MatrixMul, and Conv graph construction previously failed
  export or graph verification;
- the exact firmware lacks its matching `libCLC.so`, and the adjacent CLC
  remains permanently quarantined.

## 3. Goals

### 3.1 PoC 13G performance goal

Produce the first same-device, same-input, same-output comparison of:

- an exact CPU U8 Erf lookup on the Wave Cortex-A55;
- the exact approved Wave NPU Erf graph;
- NPU copy-in, `Run()`, copy-out, and total inference boundaries;
- cold-start, warm steady-state, and complete-process overhead.

The result is a measurement, not a predetermined NPU-win gate.

### 3.2 PoC 14 residency goal

Move the existing learned affine parameters, and then one minimal learned
spectral Conv operation, into independently qualified NPU partitions without
weakening the CPU oracle, numerical checks, fallback behavior, or device
safety policy.

### 3.3 Quality goal

Each expanded partition must reproduce its frozen quantized CPU model within
one U8 unit and preserve the existing held-out and waveform quality gates.
Clean-reference audio is never an inference input.

### 3.4 Performance goal

For every executable PoC 14 partition, compare the identical model on the
Wave CPU and NPU using the PoC 13G timing boundary. Report latency and CPU
load even when the CPU wins.

### 3.5 Reverse-engineering goal

Recover only the serializer mechanisms required by the next exact
partition. Every recovered transform needs independent controls and an
inverse or reconstruction proof. An unexplained executable byte blocks
device execution.

## 4. Non-Goals

This design does not:

- claim arbitrary ONNX import or general Acuity support;
- claim a complete VeriSilicon ISA or command-stream decoder;
- enable arbitrary NBG paths, shapes, operations, or coefficients;
- mutate and execute a stock production graph;
- patch a product ID or foreign target;
- invoke the quarantined `libCLC.so`;
- use raw ioctl replay, register access, NPU reset, or firmware changes;
- change CPU governors, CPU affinity, NPU clocks, thermal policy, sysfs,
  kernel modules, services, boot state, or stock libraries;
- promise that the NPU will beat the CPU for the small Erf or affine
  partitions;
- integrate the result into the running audio service before performance and
  service-level qualification.

## 5. Approaches Considered

### 5.1 Benchmark first, then serializer expansion — selected

This provides the missing Wave CPU/NPU baseline and a dispatch-cost floor
before investing in larger partitions.

Advantages:

- produces an apples-to-apples result immediately;
- identifies copy and dispatch overhead separately;
- gives PoC 14 a measurable break-even target;
- avoids shipping an NPU path that is slower than its CPU fallback.

### 5.2 Production Rust integration first — deferred

Porting the current tiny Erf partition behind `NpuInfer` would prove service
plumbing, but it could optimize around an offload that provides no speed or
CPU-load benefit. The integration remains a follow-on after PoC 14 chooses a
useful partition.

### 5.3 Direct Conv mutation and execution — rejected

Editing a stock graph or opaque command region would skip coefficient,
operation, patch, relocation, and command ownership. Structural validity and
a corrected CRC would not prove executable correctness. No such artifact may
reach the device.

## 6. PoC 13G — Wave CPU Versus Wave NPU

### 6.1 Fixed workload

PoC 13G uses the already approved quantized contract:

```text
input:   UINT8 [481], scale 0.03125, zero point 128
op:      Erf
output:  UINT8 [481], scale 0.0078125, zero point 128
```

The input sequence is generated inside the fixed native binary using
xorshift32 with the same seed and recurrence as PoC 13F. The binary produces
exactly:

```text
1 cold frame
100 warm-up frames
10,000 measured frames
```

CPU and NPU consume the same bytes for every sequence index. The 100 warm-up
frames are checked numerically but excluded from steady-state statistics.
The cold frame is reported separately.

### 6.2 CPU implementation

The CPU implementation contains the same immutable 256-byte lookup table as
the host oracle:

```text
SHA-256 c9525963e022d4a4aea0814845119b37488f3727ec8d96684f52e1e2ad776296
```

For every frame it:

1. validates the fixed 481-byte boundary internally;
2. reads each U8 input exactly once;
3. writes each lookup result into a canary-protected output;
4. updates a rolling FNV-1a64 digest so the compiler cannot remove work;
5. verifies canaries outside the timed compute region.

No vectorized approximation or floating-point `erf()` implementation is
substituted.

### 6.3 NPU implementation

The NPU path uses only the exact approved NBG hash. One graph is created and
compiled before measured steady-state execution. Each frame records:

```text
copy-in:
  CopyDataToTensor

run:
  graph->Run

copy-out:
  CopyDataFromTensor

total:
  immediately before copy-in through immediately after copy-out
```

The result is written into a canary-protected buffer and compared
byte-for-byte with the CPU output before the next frame.

### 6.4 Timing method

The benchmark uses `CLOCK_MONOTONIC_RAW` when available and fails closed if
the required monotonic clock cannot be read. Before inference it records
100,000 back-to-back clock pairs to characterize timer-call overhead.

The measured loop alternates order to limit thermal and scheduling bias:

```text
even frame: CPU then NPU
odd frame:  NPU then CPU
```

Raw nanosecond durations are written to one fixed, non-overwriting result
file under the dedicated temporary root. The host independently recomputes:

- count;
- minimum;
- arithmetic mean;
- p50;
- p90;
- p95;
- p99;
- p99.9;
- maximum;
- frames per second;
- fraction of the 10 ms audio-hop budget;
- NPU/CPU ratio for `Run()` and for total copy/run/read.

Percentiles use a documented nearest-rank definition. Raw measurements are
retained so summary statistics are not trusted without reproduction.

### 6.5 CPU-use and system context

The paired loop records calibrated `CLOCK_THREAD_CPUTIME_ID` measurements
around each CPU lookup and each NPU total boundary. It also records
`CLOCK_PROCESS_CPUTIME_ID` around the complete benchmark. The host harness
records, read-only:

- exact kernel and device identity;
- online CPU set and reported CPU model;
- available CPU-frequency policy values;
- thermal-zone type and temperature values;
- `/proc/stat` before and after;
- galcore parameters and holders;
- relevant kernel alerts;
- exact runtime-library and NBG hashes.

No governor, affinity, priority, clock, or thermal setting is changed.
Results are descriptive for the naturally scheduled Wave system.

### 6.6 PoC 13G pass criteria

PoC 13G passes when:

1. the exact CPU table and NBG hashes validate before execution;
2. one graph is created, exactly 10,101 NPU runs occur, and one graph is
   destroyed;
3. exactly 10,101 CPU frames are evaluated;
4. every CPU and NPU output byte matches;
5. all canaries remain intact;
6. all raw timing arrays have the exact expected count and valid monotonic
   durations;
7. the host independently reproduces every reported statistic;
8. CPU time, frequency context, and thermal context are captured;
9. preflight and postflight device health agree;
10. there is no retry, timeout, kernel alert, holder leak, or cleanup error.

The performance classification is one of:

```text
cpu-faster
npu-run-faster-but-total-slower
npu-total-faster
within-measurement-noise
```

No classification is converted into a failure merely because the NPU is
slower.

### 6.7 PoC 13G artifacts

Tracked:

```text
docs/npu/performance/erf-wave-cpu-vs-npu.json
docs/npu/lab-notebook/2026-07-29-poc13g-wave-cpu-vs-npu.md
```

### 6.8 PoC 13G-R1 negative and R2 measurement correction

The first sealed implementation, commit `7b08f61`, was executed once from
`/tmp/wave-npu-poc13g` and failed closed at paired frame 4,833 with native
exit 7. It produced one valid cold pair, no timing artifact, no partial
artifact, no health difference, no kernel alert, and no holder leak. The
retained stdout contains 4,833 normal post-cold TIM-VX run markers before
the failure. Frame 4,833 was NPU-first, and no TIM-VX error was emitted.
This supports a resolution-limited zero tick from the sub-microsecond CPU
`CLOCK_THREAD_CPUTIME_ID` interval as the failure mechanism; R1 did not
identify individual clock/copy/run stages, so the inference is not promoted
to a directly observed fact.

R1 is not rerun, removed, or overwritten. PoC 13G-R2 is a distinct sealed
artifact under `/tmp/wave-npu-poc13g-r2` with these reviewed corrections:

- `VSI_NN_LOG_LEVEL=2` retains warnings and errors but removes the
  per-inference debug print that would contaminate timing;
- CPU thread-clock equality is encoded as a resolution-limited zero rather
  than a runtime failure;
- CPU wall time and every NPU wall/thread interval must still be positive;
- the raw file and native summary both report the CPU thread-zero count and
  the host requires equality;
- every clock, copy, run, and duration failure has a stage-specific native
  diagnostic;
- all numerical, canary, digest, output, health, and no-retry gates remain
  unchanged.

The device-call ledger is amended for observed R1 work plus one R2 attempt:

| Variant | Graph executions | Disposition |
|---|---:|---|
| 13G-R1 | at most 4,834 | consumed; failed closed; never replayed |
| 13G-R2 | at most 10,101 | one separately sealed attempt |
| 13G family | at most 14,935 | hard cumulative ceiling |

This amendment does not authorize any reset, recovery, cleanup replay,
runtime replacement, clock change, or broader graph.

Raw:

```text
artifacts/npu/runs/poc13g-wave-cpu-vs-npu-*/
```

## 7. PoC 14 — Serializer-First Learned Expansion

PoC 14 is a ladder. A later rung cannot compensate for missing evidence on an
earlier rung.

### 7.1 PoC 14A — host model and finite operation target

Device calls: zero.

The host first freezes two mathematical targets.

Target A, required:

```text
quantized feature [481]
  -> learned per-bin Multiply(slopes [481])
  -> Add(biases [481])
  -> requantize
  -> Erf
  -> U8 mask [481]
```

Target B, conditional on Target A:

```text
quantized feature [1,1,1,481]
  -> learned Conv2D kernel [1,1,1,5], same padding
  -> learned per-bin affine
  -> requantize
  -> Erf
  -> U8 mask [481]
```

The Conv is a spectral Conv1D represented as a one-row Conv2D because that is
the smallest bounded learned neighborhood operation relevant to the current
mask model.

The host stage:

- defines a finite candidate registry for tensor types, per-tensor or
  per-channel scales, zero points, rounding, saturation, and padding;
- trains or refits parameters through the deployment quantizer;
- rejects candidates that lose the PoC 13A quality gates;
- freezes one CPU oracle, ONNX representation, tensor contract, and six
  numerical vectors before any new executable NBG exists;
- records ablations against the current Erf selector and CPU DeepFilterNet.

An inference contract is not chosen merely because it is easier to
serialize. It must remain useful for noise suppression.

### 7.2 PoC 14B — bounded compiler and static evidence

Device execution calls: zero.

Host static work maps:

- public TIM-VX operation objects to exact Wave OpenVX/ovxlib paths;
- constant ingestion, conversion, alignment, cache, and release functions;
- operation record generation;
- NN/TP command and state generation;
- patch and relocation creation;
- load-time patch application;
- tensor metadata and quantization serialization.

No compiler call occurs until a specific static hypothesis predicts:

- the exact operation family;
- the expected result class;
- the sections allowed to change;
- a bounded coefficient transformation;
- why the call does not require the quarantined CLC.

The stage permits at most six compile-only native processes, allocated as
three independently reviewed pairs:

```text
A1/A2: identical baseline
B1/B2: one fixed weight differential
C1/C2: one fixed bias differential
```

Each positive pair must be byte-identical. A failed pair is retained and is
not retried with an unreviewed graph mutation.

### 7.3 PoC 14C — coefficient and executable-record recovery

Device calls: zero.

Coefficient packing is qualified only when:

1. at least two independently varied values map as predicted;
2. both a weight and a bias or two independent tensors corroborate the
   transform;
3. the transform has an exact inverse or a fully specified lossy quantizer;
4. alignment and padding are explained and zero-checked;
5. every patch and relocation touching the region is owned;
6. regenerated compiler controls match byte-for-byte.

Operation and command ownership is qualified only when:

- the operation descriptor fields are bounded by compiler controls or exact
  static producer evidence;
- every emitted command/state byte is produced by an understood routine or
  copied unchanged from the same exact producer control;
- address-like values have explicit relocation ownership;
- load-time changes are predicted and independently checked;
- no opaque executable region is edited.

The open serializer continues to refuse semantic emission until all of these
conditions pass.

### 7.4 PoC 14D — offline partition construction

Device calls: zero.

For each eligible target:

1. serialize the exact frozen graph;
2. parse it with the strict VPMN v2 parser;
3. verify target, version, section directory, tensor metadata, quantization,
   file CRC, section checksums, padding, ownership, and structural signature;
4. reconstruct it byte-for-byte from its owned representation;
5. reject truncation and single-byte corruption in every owned region;
6. bind the graph, model, weights, oracle, runtime, and device allowlist in
   one sealed package;
7. keep `execution_approved: false`.

Structural validity alone is insufficient. An executable candidate becomes
eligible only if it is:

- emitted by an exact producer path whose relevant behavior is bounded; or
- reconstructed from compiler-produced controls with complete coefficient,
  operation, command/state, patch, and relocation evidence.

If neither condition is met, PoC 14 ends as a documented host-only reverse-
engineering result.

### 7.5 PoC 14E — six-vector numerical qualification

This stage is separately authorized for Target A first. Target B receives a
new fixed runner and manifest only after Target A completes.

Each target gets:

- one graph create;
- six fixed executions;
- one graph destroy;
- exact or one-U8-unit CPU-oracle tolerance;
- input/output canaries;
- monotonic ordering and fixed sequence;
- zero fallback in strict evidence;
- preflight, postflight, holder, kernel-alert, and cleanup checks.

The vectors cover:

1. zero point;
2. minimum;
3. maximum;
4. repeated ramp;
5. seeded random;
6. one learned fixture frame.

A nonzero output, successful graph creation, or plausible audio is not a
numerical pass.

### 7.6 PoC 14F — NPU-derived waveform

After a target passes PoC 14E, the host captures all 1,201 frozen fixture
inputs for that exact partition. One fixed stream runner executes them in
order. The host reconstructs the waveform only from the returned NPU
tensors.

Required gates:

- 1,201 ordered NPU successes and zero fallback;
- every output byte within one U8 unit of the frozen CPU oracle;
- CPU/NPU waveform maximum error at most `1 / 128`;
- CPU/NPU waveform error RMS below `-50 dBFS`;
- median noisy-speech SI-SDR improvement at least 3 dB;
- noise-only attenuation at least 6 dB;
- held-out clean-speech median absolute RMS delta at most 1 dB;
- natural-speech STOI delta no worse than `-0.02`;
- no missing samples, NaNs, clipping, or unexplained discontinuities.

The fixture's transition-segment allowance remains separately documented and
does not weaken the held-out clean gate.

### 7.7 PoC 14G — same-device performance qualification

Each waveform-qualified partition is benchmarked through the PoC 13G
boundary:

- the exact same quantized model on the Wave CPU;
- the exact same graph on the Wave NPU;
- cold, warm, copy-in, run, copy-out, total, and process CPU time;
- p50/p95/p99/p99.9/max;
- output equality and health invariants.

Promotion is based on total inference, not `graph->Run()` alone.

A partition is performance-useful if it demonstrates at least one:

- lower NPU total p99 than Wave CPU total p99;
- lower audio-service CPU consumption without missing the 10 ms deadline;
- a material increase in learned NPU residency with no material latency,
  quality, or stability regression.

The third classification can justify continued research but not default
production enablement.

### 7.8 PoC 14H — Conv expansion

Target B starts only when Target A has proven:

- deterministic constant packing;
- operation emission;
- command/state ownership;
- patch and relocation behavior;
- bounded numerical execution;
- a complete waveform;
- same-device performance.

Conv then repeats PoCs 14B through 14G with a new package, signature, fixed
runner, device-call ledger, and notebooks. Affine evidence is reused only
where hashes and exact producer paths match; no global serializer
qualification is inferred.

The four Conv compile-only calls are fixed as:

```text
A1/A2: identical Conv baseline
B1/B2: one fixed kernel-weight differential
```

Bias and affine evidence may be reused only from the exact already-qualified
Target A serializer path.

## 8. CPU Fallback and Failure Semantics

Every PoC 14 model has one CPU implementation of the exact deployed
quantized graph. Hybrid behavior remains:

1. attempt only the exact approved NPU partition;
2. validate output type, shape, sequence, deadline, and canaries;
3. on the first failure, recompute the same frame through the CPU model;
4. permanently latch CPU fallback for the session;
5. perform no automatic retry or reset.

Strict qualification never relabels a CPU result as NPU output. Audit mode
opens no runtime. CPU mode opens no NPU runtime.

## 9. Device Safety and Call Caps

All device paths are fixed below new dedicated
`/tmp/wave-npu-poc13g-*` or `/tmp/wave-npu-poc14-*` roots. Every stage
refuses a pre-existing remote root and non-regular or symlinked inputs.

| Stage | Compile-only processes | Graph creates | Graph executions | Graph destroys |
|---|---:|---:|---:|---:|
| 13G | 0 | 1 | 10,101 | 1 |
| 14A | 0 | 0 | 0 | 0 |
| 14B | 6 maximum | 0 | 0 | 0 |
| 14C | 0 | 0 | 0 | 0 |
| 14D | 0 | 0 | 0 | 0 |
| 14E Target A | 0 | 1 | 6 | 1 |
| 14F Target A | 0 | 1 | 1,201 | 1 |
| 14G Target A | 0 | 1 | 10,101 | 1 |
| 14H Conv compile capture | 4 maximum | 0 | 0 | 0 |
| 14H Conv numerical | 0 | 1 | 6 | 1 |
| 14H Conv waveform | 0 | 1 | 1,201 | 1 |
| 14H Conv benchmark | 0 | 1 | 10,101 | 1 |

Each row after PoC 13G requires all preceding host and numerical gates for
that exact target. Caps are not a reservation or permission to consume calls
without a reviewed manifest.

Always prohibited:

- firmware, flash, boot, root-filesystem, or stock-library writes;
- service stop, reboot, module reload, reset, or clock/thermal changes;
- raw register access or ioctl replay;
- target-ID patching followed by execution;
- malformed graph execution or fuzzing;
- dynamic runtime, graph, tensor, shape, or path arguments;
- the quarantined CLC;
- automatic retry, recovery, or NPU reset;
- concurrent graphs.

An error records evidence, cleans up through the known API path, and returns
to host analysis. It does not broaden the experiment.

## 10. Validation Strategy

### 10.1 Host tests

- deterministic input generator and exact sequence hash;
- immutable CPU lookup and independent oracle;
- timer schema, count, monotonicity, and percentile definitions;
- fake clock and timer-overhead handling;
- raw timing file truncation and corruption;
- CPU/NPU digest disagreement;
- canary and output-size failures;
- serializer ownership, inverse transform, and reconstruction;
- wrong target, type, shape, scale, zero point, CRC, and signature;
- model-package cross-hash failures;
- same-frame fallback and circuit-breaker permanence.

### 10.2 Fake-transport tests

- exact command and remote-path allowlists;
- pre-existing root refusal;
- wrong device or runtime hash;
- compiler and graph call-count drift;
- timeout and process death;
- first-error abort and no retry;
- malformed native results;
- health, holder, kernel-alert, and cleanup changes;
- output non-overwrite and symlink refusal.

### 10.3 Device evidence

- exact binary, source, manifest, runtime, NBG, and device hashes;
- static native-policy pass;
- preflight and postflight envelopes;
- native stdout, stderr, result, raw timing, and exit status;
- independently generated numerical oracle and statistics;
- explicit call ledger and prohibited-action counters.

## 11. Documentation Layout

Design and plan:

```text
docs/superpowers/specs/
  2026-07-29-wave-npu-cpu-benchmark-serializer-expansion-design.md

docs/superpowers/plans/
  2026-07-29-wave-npu-cpu-benchmark-serializer-expansion.md
```

Tracked evidence:

```text
docs/npu/performance/
  erf-wave-cpu-vs-npu.json

docs/npu/format/
  affine-serializer-profile.json
  conv-serializer-profile.json

docs/npu/models/
  erf-affine/
  erf-conv/

docs/npu/lab-notebook/
  2026-07-29-poc13g-wave-cpu-vs-npu.md
  2026-07-29-poc14a-affine-host.md
  2026-07-29-poc14b-serializer-recovery.md
  2026-07-29-poc14e-affine-numerical.md
  2026-07-29-poc14f-affine-waveform.md
  2026-07-29-poc14g-affine-performance.md
  2026-07-29-poc14h-conv-expansion.md
```

Large inputs, NBGs, raw timing arrays, waveforms, and native logs remain
under `artifacts/npu/`.

## 12. Completion Criteria

### 12.1 PoC 13G complete

PoC 13G is complete when:

1. CPU and NPU execute the same 10,101 frames on the Wave;
2. every output byte matches;
3. copy, run, total, cold, steady-state, and process-CPU boundaries exist;
4. raw timings reproduce p50/p95/p99/p99.9/max;
5. timer overhead and system context are recorded;
6. device health and cleanup pass;
7. the result states which path is faster without extrapolating from the
   Apple host.

### 12.2 PoC 14 Target A complete

The affine expansion is complete only when:

1. a useful host model and quantization contract are frozen;
2. coefficient packing is independently corroborated;
3. operation, command/state, patch, and relocation ownership is complete;
4. deterministic producer controls reconstruct exactly;
5. a sealed affine-plus-Erf NBG passes strict host validation;
6. six NPU vectors match the CPU oracle;
7. a complete NPU-derived waveform passes quality gates;
8. Wave CPU and NPU execute the identical partition under PoC 13G timing;
9. fallback and all documentation gates pass.

### 12.3 PoC 14 Conv complete

Conv expansion is complete only when Target A is complete and the same nine
classes of evidence are repeated for the Conv-plus-affine-plus-Erf package.

### 12.4 Explicit partial results

The following are valid but incomplete outcomes:

- `cpu-faster` for PoC 13G;
- compiler clean negative;
- coefficient transform unresolved;
- operation or command ownership incomplete;
- structurally valid but execution-ineligible NBG;
- numerically correct but quality-negative partition;
- quality-positive but performance-negative partition.

No partial result is described as general ONNX, an open compiler, or
production-ready NPU acceleration.

## 13. Follow-On Decision

After PoC 13G:

- if CPU wins the tiny Erf partition, retain Erf as the hardware and
  serializer regression control;
- if NPU total wins, keep the exact partition eligible for production
  integration;
- in both cases, use the measured dispatch floor as PoC 14's break-even
  target.

After PoC 14:

- integrate an NPU partition behind Rust `NpuInfer` only when total latency
  or measured audio-service CPU load justifies it; otherwise retain the
  qualified graph as a lab regression and keep CPU production inference;
- collect end-to-end audio-service p50/p95/p99, CPU, RSS, allocation,
  reset, thermal, and deadline evidence;
- keep CPU fallback and exact-hash placement;
- expand to VAD, residual echo suppression, or keyword spotting only through
  new operation-specific qualification.
