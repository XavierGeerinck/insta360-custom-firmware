# Wave CPU/NPU Benchmark and Serializer Expansion — Implementation Plan

**Date:** 2026-07-29

**Design:**
`docs/superpowers/specs/2026-07-29-wave-npu-cpu-benchmark-serializer-expansion-design.md`

**Planning workflow:** Manual test-first fallback because the required
`writing-plans` skill is not installed in this session.

## 1. Objective

Execute the approved sequence:

1. measure the exact U8 `[481]` Erf operation on both the Wave Cortex-A55
   CPU and Wave NPU;
2. recover and qualify the smallest useful affine-plus-Erf serializer path;
3. attempt a Conv-plus-affine-plus-Erf expansion only after every affine
   gate passes.

No performance ratio may combine Apple-host CPU timing with Wave NPU timing.
No new graph reaches the device because it parses or returns nonzero bytes.

## 2. Worktree and Execution Rules

- Preserve all unrelated tracked and untracked work.
- Use `apply_patch` for source and documentation edits.
- Use formatters only for mechanical formatting.
- Run every shell command through `lean-ctx -c '...'` or
  `lean-ctx raw '...'`.
- Stage and commit only exact reviewed paths.
- Keep raw device evidence under ignored `artifacts/npu/`.
- Use new exact remote roots and refuse pre-existing roots.
- Do not delete prior `/tmp/wave-npu-poc13*` evidence to replay a stage.
- Run host tests and static policy before staging a native binary.
- Do not use subagents.
- Do not invoke the quarantined CLC.
- Do not perform firmware, flash, boot, stock-library, service, module,
  sysfs-write, raw-ioctl, reset, clock, thermal, or recovery actions.

## 3. Phase A — PoC 13G Host Contracts

### Task 1 — Lock the regression baseline

Run without changing files:

```bash
cd waved/tools/onnx-hybrid
uv sync --frozen
uv run pytest -q
uv run ruff check .
uv run ruff format --check .

cd ../../..
uv --project waved/tools/onnx-hybrid run pytest -q waved/scripts/npu/tests
```

Record exact counts and any pre-existing failures. Do not weaken a test.

### Task 2 — Define the fixed benchmark manifest

Create:

```text
waved/scripts/npu/erf_cpu_npu_benchmark_manifest.json
waved/scripts/npu/npu_erf_cpu_npu_benchmark_lab.py
waved/scripts/npu/tests/test_npu_erf_cpu_npu_benchmark_lab.py
```

Test first:

- exact schema and unknown-key refusal;
- one allowed device identity and kernel;
- exact NBG, runtime, source, and native-binary hashes;
- exact CPU table SHA-256;
- exact xorshift seed and recurrence;
- exactly 1 cold, 100 warm-up, and 10,000 measured frames;
- one graph create, 10,101 runs, and one graph destroy;
- exactly 10,101 CPU evaluations;
- fixed result and raw-timing paths;
- remote-root nonexistence;
- staged/system library equality;
- absent quarantined CLC on every loader path;
- no holder before execution;
- fixed watchdog;
- wrong output, count, digest, canary, timer, percentile, health, and cleanup
  refusal;
- first-error abort and no retry;
- fake transport only in tests.

The host parser independently computes nearest-rank percentiles from raw
nanoseconds. It never trusts native summary fields without reproducing them.

### Task 3 — Define the raw timing format

Keep the format minimal and fixed:

```text
magic
schema version
record count
timer calibration count
records[10,000]:
  sequence
  order
  cpu_wall_ns
  cpu_thread_ns
  npu_copy_in_wall_ns
  npu_run_wall_ns
  npu_copy_out_wall_ns
  npu_total_wall_ns
  npu_thread_ns
```

The cold and warm-up summaries are separate fixed fields. The timing file:

- is little-endian;
- contains no addresses or tensor contents;
- is written once with exclusive creation;
- is hashed before pull;
- has an exact expected size;
- rejects truncation, extra bytes, duplicate sequence, wrong alternating
  order, zero duration, overflow, and nonmonotonic sequence.

## 4. Phase B — PoC 13G Native Benchmark

### Task 4 — Implement the fixed native runner

Create:

```text
waved/tools/npu-compiler/wave_npu_benchmark_erf_cpu_vs_npu.cc
waved/tools/npu-compiler/build-device-erf-cpu-npu-benchmark.sh
waved/tools/npu-compiler/verify-device-erf-cpu-npu-benchmark-policy.sh
waved/tools/npu-compiler/stage-device-erf-cpu-npu-benchmark.sh
```

The binary accepts only:

```text
--describe
--benchmark
```

It contains:

- exact NBG and result paths;
- fixed frame counts and seed;
- immutable 256-byte CPU table;
- one TIM-VX graph;
- parent-death signal;
- fixed alarm;
- canary-protected CPU and NPU buffers;
- `CLOCK_MONOTONIC_RAW` wall timing;
- calibrated `CLOCK_THREAD_CPUTIME_ID`;
- complete-process `CLOCK_PROCESS_CPUTIME_ID`;
- alternating CPU-first/NPU-first measured order;
- rolling input, CPU-output, and NPU-output FNV-1a64 digests;
- exclusive result creation;
- no dynamic path, graph, tensor, count, runtime, or library argument.

The timed NPU total begins immediately before `CopyDataToTensor` and ends
immediately after `CopyDataFromTensor`. `graph->Run()` remains a separately
timed subset. CPU time covers exactly 481 table writes.

### Task 5 — Write the static native policy

Policy requires:

- AArch64 ELF;
- only the fixed two-mode CLI;
- exact constants and counts;
- exactly one graph construction site;
- one compile before measured frames;
- no CPU floating-point `erf`;
- exact embedded CPU-table hash;
- exact tensor shape, type, scales, and zero points;
- required copy/run/read calls;
- required clocks and parent-death/watchdog calls;
- output non-overwrite;
- no shell, ADB, system, ioctl, reset, firmware, module, sysfs-write,
  service, CLC, `dlopen`, arbitrary file, or retry behavior.

Test both source and binary policy. Forbidden strings in comments also fail.

### Task 6 — Cross-build deterministically

Reuse the pinned offline container and exact Wave sysroot. Build twice
locally and require identical SHA-256 before selecting one artifact. This
does not consume a device call.

Run:

```bash
bash -n waved/tools/npu-compiler/*erf-cpu-npu-benchmark*.sh

waved/tools/npu-compiler/verify-device-erf-cpu-npu-benchmark-policy.sh \
  artifacts/npu/build/poc13g/wave-npu-benchmark-erf-cpu-vs-npu-v1 \
  waved/tools/npu-compiler/wave_npu_benchmark_erf_cpu_vs_npu.cc
```

Update the manifest only after the binary and source hashes are final.

## 5. Phase C — PoC 13G Device Qualification

### Task 7 — Stage with read-only preflight

Before staging:

- full host regression passes;
- static policy passes;
- manifest canonicalization passes;
- one authorized device is present;
- serial, kernel, galcore identity, module parameters, and runtime hashes
  match;
- no galcore holder exists;
- exact remote root is absent;
- no CLC exists on an admitted loader path.

Stage only fixed regular files beneath the new root. Refuse symlinks and
existing outputs.

### Task 8 — Execute once

One authorized device process performs:

```text
1 graph create
1 cold NPU run
100 warm-up NPU runs
10,000 measured NPU runs
1 graph destroy
```

It also performs the same 10,101 CPU evaluations. There is no retry.

On success:

- pull the fixed result and raw timing file;
- verify remote and local hashes;
- independently recompute every percentile and ratio;
- compare every numerical and health invariant;
- retain all evidence and remote root.

On failure:

- record the clean negative;
- perform only known cleanup through normal process teardown;
- do not replay or reset.

### Task 9 — Publish PoC 13G

Create:

```text
docs/npu/performance/erf-wave-cpu-vs-npu.json
docs/npu/lab-notebook/2026-07-29-poc13g-wave-cpu-vs-npu.md
```

Update:

```text
docs/npu/README.md
docs/npu/reproduction.md
docs/REPORT_NPU.md
waved/docs/COMPUTE_HARDWARE.md
waved/scripts/npu/README.md
docs/npu/models/erf-mask/README.md
```

State CPU/NPU ratios for:

- compute-only CPU versus NPU `Run()`;
- CPU total versus NPU copy/run/read total;
- steady-state p50/p95/p99/max;
- cold inference;
- process/thread CPU time;
- 10 ms hop-budget fraction.

## 6. Phase D — PoC 14A Host Model

### Task 10 — Define the affine deployment quantizer

Create host tests before implementation for a finite registry covering:

- feature U8 and signed-I8 candidates;
- per-tensor and per-channel weight scales;
- bias accumulator scale;
- ties-to-even rounding;
- saturation;
- zero points;
- exact dequantized affine equation;
- output requantization into the already qualified Erf input contract.

Extend the existing Erf-mask modules rather than fork STFT, estimator,
training, metrics, or synthesis.

Planned files:

```text
waved/tools/onnx-hybrid/src/wave_onnx_hybrid/erf_affine_model.py
waved/tools/onnx-hybrid/src/wave_onnx_hybrid/erf_affine_package.py
waved/tools/onnx-hybrid/tests/test_erf_affine_model.py
waved/tools/onnx-hybrid/tests/test_erf_affine_package.py
```

### Task 11 — Train/refit and qualify Target A

Freeze:

- one quantization candidate;
- exact 481 slopes and biases;
- independent integer CPU oracle;
- deterministic ONNX representation;
- six numerical vectors;
- complete device fixture inputs;
- model and evaluation manifests.

Pass the existing gates:

- deterministic regeneration;
- synthetic median SI-SDR improvement at least 3 dB;
- natural median SI-SDR improvement at least 1 dB;
- natural STOI delta at least `-0.02`;
- noise attenuation at least 6 dB;
- 477-item clean median absolute RMS delta at most 1 dB;
- no invalid items or private input.

If no deployment quantizer passes, stop PoC 14 before serializer or device
work and document the clean negative.

## 7. Phase E — PoC 14B/C Serializer Recovery

### Task 12 — Build the exact target/function ledger

Extend the existing serializer manifest and Ghidra scripts only with
reviewed targets for:

- constant conversion and packing;
- Multiply and Add operation emission;
- NN/TP command and state emission;
- patch generation;
- relocation generation;
- load-time patch application.

Every target records exact ELF hash, address, symbol/body hash, callers,
callees, completeness, and evidence class. Truncation blocks qualification.

### Task 13 — Define three compile-only pairs

Create one fixed native compiler with closed modes:

```text
--a1 / --a2  identical affine baseline
--b1 / --b2  one fixed weight delta
--c1 / --c2  one fixed bias delta
```

Do not build or stage it until static analysis predicts a safe producer path
that does not invoke CLC. Each pair gets separate paths and contexts. A
failed pair is not retried.

Create manifest, fake-transport tests, source policy, build, and stage
scripts following PoC 13B.

### Task 14 — Recover coefficient packing

Extend the lossless NBG image with a separate read-only correlation layer.
Do not enable semantic emission.

Require:

- byte-identical A1/A2, B1/B2, and C1/C2;
- bounded changed ranges;
- predicted value-to-byte mapping for two weights and one bias;
- exact inverse or fully specified quantizer;
- explained alignment and zero padding;
- patch/relocation ownership for every changed byte;
- reconstruction of all six producer artifacts.

No unbounded transform search is allowed. Add a transform only with a
documented static or controlled-differential hypothesis.

### Task 15 — Recover operation and executable records

Map the baseline/differential artifacts through:

- operation descriptor;
- tensor/constant references;
- NN/TP command/state records;
- patches;
- relocations;
- load-time changes.

Create corruption fixtures for every owned range. An unexplained edited
executable byte keeps the serializer `not-ready`.

Publish host-only evidence even if the gate remains blocked.

## 8. Phase F — PoC 14D Affine Partition Construction

### Task 16 — Add semantic emission behind an exact target

Only after Tasks 14 and 15 pass:

- introduce typed affine operation and constant records;
- preserve opaque unchanged ranges;
- calculate alignment and padding;
- emit patch and relocation records;
- calculate section checksums and VPMN file CRC;
- refuse every graph except the exact frozen Target A.

Tests must prove:

- compiler-control byte identity;
- deterministic repeated emission;
- strict parse;
- total ownership;
- no unresolved emitted byte;
- wrong shape/type/scale/hash refusal;
- corruption refusal;
- output non-overwrite.

The new package remains `execution_approved: false`.

### Task 17 — Independent host qualification

Use:

- the existing strict parser;
- the typed serializer;
- a second structural checker;
- the independent integer CPU oracle;
- exact model/package cross-hashes.

If there is no eligible exact producer or complete reconstruction chain, do
not create an execution runner.

## 9. Phase G — PoC 14E–G Affine Execution

### Task 18 — Six-vector qualification

Create a new fixed runner and harness only after Task 17 passes.

Device cap:

```text
1 graph create
6 graph runs
1 graph destroy
```

Require exact or one-U8-unit oracle agreement, canaries, nonconstant learned
output, zero fallback, and unchanged health.

### Task 19 — Complete waveform

Capture exactly 1,201 Target A inputs. One fixed runner executes one graph
for all frames. Reconstruct only from returned NPU tensors.

Pass numerical, SI-SDR, STOI, attenuation, clean-preservation, silence,
length, clipping, and discontinuity gates from the approved design.

### Task 20 — Same-device Target A benchmark

Extend the PoC 13G benchmark through a new exact manifest and fixed Target A
binary. Do not turn the binary into a generic benchmark.

Compare identical CPU and NPU quantized affine-plus-Erf over:

```text
1 cold
100 warm-up
10,000 measured
```

Publish copy/run/read/total and CPU-use evidence. Performance-negative is a
valid result.

## 10. Phase H — PoC 14H Conv Expansion

### Task 21 — Host Conv model

Only after Target A completes, add the frozen one-row 1x5 Conv model and
train through its deployment quantizer. Retain the Target A affine and Erf
contracts.

Pass deterministic, ablation, held-out, quality, and fixture gates before
compiler or serializer work.

### Task 22 — Four bounded Conv compile captures

Use fixed modes:

```text
--a1 / --a2  identical Conv baseline
--b1 / --b2  one fixed kernel-weight delta
```

Maximum four compile-only processes. No execution. Do not reuse affine
evidence unless exact hashes and producer paths match.

### Task 23 — Repeat serializer qualification

Recover Conv coefficient packing, operation records, command/state, patches,
relocations, and load-time behavior. Require exact producer reconstruction
and zero unexplained edited executable bytes.

### Task 24 — Repeat numerical, waveform, and benchmark gates

Use separate:

- sealed package and signature;
- six-vector runner;
- 1,201-frame stream runner;
- 10,101-frame CPU/NPU benchmark;
- notebooks and machine evidence.

Any failed host gate ends device work for Conv.

## 11. Verification Matrix

Python:

```bash
cd waved/tools/onnx-hybrid
uv run pytest -q
uv run ruff check .
uv run ruff format --check .
```

NPU harnesses:

```bash
uv --project waved/tools/onnx-hybrid run pytest -q waved/scripts/npu/tests
```

Native policy:

```bash
bash -n waved/tools/npu-compiler/*.sh
```

For every new binary, run its exact source/ELF policy before staging.

Artifact and documentation checks:

- canonical JSON parse and regeneration;
- exact SHA-256 verification;
- NBG full parse, CRC, ownership, and reconstruction;
- raw timing independent percentile recomputation;
- WAV format and sample count;
- Markdown local-link check;
- trailing-whitespace check for untracked files;
- `git diff --check`;
- exact dirty-worktree and staged-set review.

## 12. Documentation and Completion Audit

Each PoC gets:

- hypothesis and falsifiers;
- exact inputs and identities;
- safety envelope and call counts;
- reproduction commands;
- observed output and timing;
- machine-result path and SHA-256;
- explicit non-claims;
- next gate.

Final audit classifications:

```text
proven
contradicted
incomplete
missing
```

The final report must distinguish:

- current Erf CPU versus NPU;
- affine-plus-Erf CPU versus NPU;
- Conv-plus-affine-plus-Erf CPU versus NPU;
- partition latency versus complete audio-service latency;
- research qualification versus production integration.
