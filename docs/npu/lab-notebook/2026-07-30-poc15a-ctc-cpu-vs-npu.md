# PoC 15A — Stock CTC CPU versus NPU

**Date:** 2026-07-30
**Device:** Insta360 Wave `PSP_0000000000000000`
**Status:** corrected PoC 15A-R2 passed semantic qualification and completed
the paired benchmark; performance classification is `cpu-faster`.

## 1. Question

PoC 13G showed that the Wave NPU loses badly to a Cortex-A55 for a tiny
481-element lookup operation. PoC 15A asks the complementary question:

> Does the same NPU beat the Wave's stock four-core CPU stack when both run
> a substantial, real, firmware-shipped neural-network partition?

The comparison uses the English Reverb-ASR CTC projection because the
firmware contains both its source ONNX and its compiled NBG:

| Artifact | Contract | SHA-256 |
|---|---|---|
| `ctc.onnx` | FP32 `[1,16,512] -> [1,16,4999]` | `8d6d3b6ce9da51f852981bb5bdd33e2818d47d3e545e9efe5d3700a37b367219` |
| `ctc_online.nb` | FP16 `[512,16,1] -> [4999,16,1]` | `3a9294ea8595789cc23ddc9298ebceb4474738fed07a9a8b10e2442b67b0d8d5` |

The ONNX graph is `MatMul -> Add -> LogSoftmax` and performs 40,951,808
matrix MACs per invocation. Linear input and output memory order is the
same in the ONNX and NBG representations.

## 2. Runtime boundaries

The CPU path is the stock:

```text
/usr/lib/libonnxruntime.so.1.15.1
SHA-256 457784da0d1c525171b15438cfbaf869f57e9317817e30691136b8ebe10d4639
provider CPUExecutionProvider only
graph optimization ORT_ENABLE_ALL
default stock thread pool
preallocated input and output OrtValue storage
```

The NPU path is the firmware-matched:

```text
/usr/lib/libnnsdk.so
SHA-256 36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe
aml_module_input_set
aml_module_output_get(PERF_MODE_RUN)
aml_module_output_get(PERF_MODE_OUTPUT_GET)
```

Two NPU timing boundaries are required:

- **native total:** FP16 input submission, execution, output retrieval, and
  copying all 159,968 output bytes into guarded local memory;
- **FP32 bridge total:** native total plus exact FP32-to-FP16 input and
  FP16-to-FP32 output conversion.

The performance claim is based on FP32 bridge total, not the vendor's run
call alone.

## 3. Approved qualification and performance protocol

Semantic qualification consists of exactly four immutable fixtures:

1. all zeros;
2. a `0.5` impulse at linear index zero, logical `[time=0, feature=0]`;
3. the repeating stripe `{-0.5,-0.25,0,0.25,0.5}`;
4. xorshift32 seed `0x573315a1`, mapped by
   `((state & 255) - 128) / 256`.

For each fixture, the NPU FP16 output is compared with the CPU output
rounded to FP16 and expanded back to FP32. A fixture passes only when:

- all 79,984 values from both paths are finite;
- all 16 per-time-step top-1 classes match;
- mean absolute error is no more than `0.025`;
- maximum absolute error is no more than `0.25`;
- cosine similarity is at least `0.999`;
- two NPU executions produce byte-identical FP16 output.

Only these four fixtures decide semantic qualification.

After qualification, the performance sequence has one recorded initial
pair, 20 unrecorded warm-up pairs, and 200 recorded pairs. Execution order
alternates CPU-first and NPU-first. Performance inputs use seed
`0x573315b1` with the same fixed `[-0.5,0.5)` mapping. Performance pairs
must materialize finite, canary-safe outputs, but they do not reapply the
top-1 qualification gate.

The NPU wins only if all FP32-bridge conditions hold:

```text
ratio of means <= 0.90
paired-ratio p50 <= 0.90
paired-ratio p95 < 1.00
```

## 4. First implementation

The first runner was committed as:

```text
commit 6a1c65a8dc61339d2b0e2e2697b89d5c87c09ce3
source SHA-256 18435fc05207005cf0d553741109cd11fcb775fa9c9aa80982507756d92bdae2
binary SHA-256 0cd9e4413bc545d4f0fefe9746a00c4cc334ea4e0888d74e44813437c9694635
manifest SHA-256 7deb6686e130cb3c7754eed78c9c8baefcc97ff288b81773206ba5fde0cad490
```

It passed:

- two byte-identical AArch64 builds;
- AArch64 ELF64 PIE, full RELRO, immediate binding, and non-executable
  stack checks;
- the closed ORT and `libnnsdk` source/import policy;
- 19 focused profile/controller tests;
- 93 existing generic, compile-lab, stream, and PoC 13G regression tests;
- Ruff, shell syntax, ShellCheck, and profile regeneration.

## 5. First live execution

Only one native process was authorized and executed:

```text
run ID: poc15a-ctc-crossover-20260730-a1
started: 2026-07-30T05:37:32.368Z
finished: 2026-07-30T05:37:39.735Z
native execution duration reported by controller: 818 ms
native exit: 19
native failure stage: pair-numerical-gate
```

Raw host evidence is retained below:

```text
artifacts/npu/runs/poc15a-ctc-crossover-20260730-a1/
  preflight.json
  postflight.json
  stdout.txt
  stderr.txt
  run.json
```

The observed live identity was:

```text
chip_model      0x00009000
chip_revision   0x00009002
chip_product_id 0x05090001
chip_eco_id     0x04000000
customer_id     0x1000000e
flags           0x0000000000000000
```

All four native qualification records passed:

| Fixture | Top-1 | Finite | Mean abs. error | Max abs. error | Cosine | Repeat |
|---|---:|---:|---:|---:|---:|---|
| zero | 16/16 | 79,984 | 0.000117211 | 0.0078125 | 0.999999994047 | exact |
| impulse | 16/16 | 79,984 | 0.000117748 | 0.0078125 | 0.999999994007 | exact |
| stripe | 16/16 | 79,984 | 0.000948688 | 0.015625 | 0.999999976145 | exact |
| xorshift | 16/16 | 79,984 | 0.002705152 | 0.03125 | 0.999999957457 | exact |

These results strongly support that the ONNX/NBG source-binary anchor is
numerically related. They are not a valid final qualification record,
however, because the first implementation did not generate two fixtures
exactly as specified.

The runner stopped later at `pair-numerical-gate`. It published neither
the final timing file nor a partial timing file. Therefore:

- there is no CPU-versus-NPU performance result from this attempt;
- native-process wall time must not be repurposed as inference latency;
- PoC 15B and PoC 15C remain gated;
- no automatic retry is allowed.

## 6. Root cause: protocol implementation, not runtime failure

Post-run source review found four deviations:

1. The strict top-1/MAE/cosine qualification gate was incorrectly applied
   to every performance pair. The approved design applies it only to the
   four fixtures. Closely separated CTC logits can change top-1 under FP16
   rounding even when aggregate numerical agreement is excellent.
2. Random values were generated with nine low bits:
   `((state & 511) - 256) / 256`, giving `[-1,1)`, instead of the approved
   eight-bit `[-0.5,0.5)` mapping.
3. The impulse was placed at the center of the flat tensor instead of
   logical `[time=0, feature=0]`.
4. The recorded initial pair ran before qualification instead of after the
   approved qualification boundary.

The host native-result parser also contained stale non-Wave revision,
product, and ECO expectations. This did not cause the live failure because
the native process stopped before result parsing, but it is corrected in
R2.

The failure is consequently classified as:

```text
invalid-run / harness-protocol-defect
```

It is not classified as `semantic-negative`, `cpu-faster`, or
`npu-faster`.

## 7. Safety and postflight

The postflight recorded:

```text
galcore holders: none
benchmark processes: none
partial timing output: absent
final timing output: absent
kernel alerts: none
health differences: none
temperature: 38.4 C (38.1 C preflight)
CPU governor: schedutil
CPU frequency observed postflight: 1.5 GHz
```

No firmware, stock model, system DSO, service, module, sysfs value, clock,
governor, thermal control, driver state, or root filesystem file was
modified. The fixed staging root `/tmp/wave-npu-poc15a` was removed after
the evidence was pulled and audited.

## 8. Corrected PoC 15A-R2

R2 changes only the protocol defects:

- remote root: `/tmp/wave-npu-poc15a-r2`;
- binary: `wave-npu-benchmark-ctc-cpu-vs-npu-v2`;
- timing schema: `CCN2-v2`;
- qualification precedes the recorded performance sequence;
- impulse index is exactly zero;
- random mapping is exactly `((state & 255) - 128) / 256`;
- qualification gates apply only to qualification fixtures;
- performance pairs still require complete finite output materialization
  and intact canaries;
- full live identity parsing uses the observed Wave identity.

Frozen R2 host artifacts:

```text
source SHA-256 6d924c2779f2636e50aab49656d015165dab487a058c73691b857c14692b7d27
binary SHA-256 d5316b2b45d49edbbc83daf368f32776388580a4e7db3667d0fe4ac88ebccf52
build script SHA-256 06e2fb0852c6043459988d6e6889663e22080a08a3002f6ef3292b407c37fbf5
policy SHA-256 728eff19e5ce08432118766eba58db71e6ba834bd2751cf94a882d7317f3ef57
manifest SHA-256 ca4470dd272ea44e8461bae10b1cdf232f84c435ec471f0dffdf27d4c388026b
```

R2 again passes deterministic build, static policy, 19 focused tests, 93
regression tests, Ruff, shell syntax, and ShellCheck.

## 9. Corrected live execution

The user explicitly authorized one corrected R2 process. The staged bundle
and independent preflight both passed before execution. Exactly one native
process then ran:

```text
run ID:              poc15a-r2-ctc-crossover-20260730-a1
controller start:    2026-07-30T06:20:56.412Z
controller finish:   2026-07-30T06:21:13.529Z
native transport:    9.617 s
native exit:         0
execution attempts:  1
result status:       PASS
decision:            cpu-faster
source commit:       19eac419c40a7f84fef437d952337df49f8e569a
```

The raw evidence is retained below the ignored device-evidence root:

```text
artifacts/npu/runs/poc15a-r2-ctc-crossover-20260730-a1/
  preflight.json
  postflight.json
  stdout.txt
  stderr.txt
  pull.stdout.txt
  pull.stderr.txt
  ctc-cpu-npu-timings-v2.bin
  run.json
  report.md
```

The timing artifact is 27,752 bytes, contains four qualification records,
one cold record, and 200 measured records, and has:

```text
schema:          CCN2-v2
SHA-256:         e40ce01c1ad148c2ce9ea5ab19365317751d63623be58d0d587a5c2afc5430f8
FNV-1a64:        0x5d3d86b9b9547b57
measured input:  0xeea0b3af5a10188e
CPU output:      0x3a55b11149de1589
NPU output:      0x60f5d3463b3f609d
CPU run count:   225
NPU run count:   229
```

The host controller validated the file size, schema, record counts, FNV
digest, all fixed header values, and the native result framing. A separate
post-run `analyze` invocation over the pulled binary reproduced every
reported percentile, ratio, gate, and decision.

## 10. Corrected semantic qualification

All four corrected fixtures passed every gate:

| Fixture | Top-1 | Finite | Mean abs. error | Max abs. error | Cosine | Repeat |
|---|---:|---:|---:|---:|---:|---|
| zero | 16/16 | 79,984 | 0.000117211 | 0.0078125 | 0.999999994047 | exact |
| impulse at index 0 | 16/16 | 79,984 | 0.000117748 | 0.0078125 | 0.999999994007 | exact |
| stripe | 16/16 | 79,984 | 0.000948688 | 0.015625 | 0.999999976145 | exact |
| xorshift, 8-bit mapping | 16/16 | 79,984 | 0.000733863 | 0.015625 | 0.999999979828 | exact |

This is the valid qualification result. It proves that the fixed ONNX and
NBG implement the same useful CTC projection to within the approved FP16
tolerances for all four fixtures. It does not prove arbitrary ONNX support
or a general ONNX-to-NBG compiler.

## 11. Paired CPU-versus-NPU result

The 200 steady-state pairs produced:

| Boundary | Mean (ms) | p50 (ms) | p95 (ms) | p99 (ms) | Mean throughput |
|---|---:|---:|---:|---:|---:|
| Stock ORT CPU | 4.658214 | 4.646291 | 4.742667 | 4.788416 | 214.675/s |
| NPU native FP16 | 4.492492 | 4.481583 | 4.560542 | 4.810750 | 222.594/s |
| NPU FP32 bridge | 5.154930 | 5.142208 | 5.224917 | 5.474583 | 193.989/s |

The bridge performance gate was:

| Gate | Limit | Observed | Pass |
|---|---:|---:|---|
| Ratio of means | `<= 0.90` | 1.106632 | no |
| Paired-ratio p50 | `<= 0.90` | 1.106377 | no |
| Paired-ratio p95 | `< 1.00` | 1.128194 | no |

The result is therefore unambiguously `cpu-faster`: with equivalent FP32
application boundaries, the NPU path was 10.663% slower by ratio of means.
This is far outside all three precommitted NPU-win thresholds.

The native FP16 boundary tells a more nuanced story. Its ratio of means was
0.964424, with paired p50 0.964474 and paired p95 0.986071. It was 3.558%
faster on average, but not the required 10%, and its p99 was slightly worse.
The accelerator's `run` interval alone averaged 2.788182 ms, 40.145% below
the CPU wall mean. That interval is not an application latency result
because it excludes input submission and complete output retrieval.

## 12. Where the gain was lost

The NPU bridge mean decomposes as:

| Component | Mean (ms) | Share of 5.155 ms bridge |
|---|---:|---:|
| FP32 to FP16 input conversion | 0.085435 | 1.7% |
| NPU input submission | 0.079704 | 1.5% |
| NPU execution | 2.788182 | 54.1% |
| NPU output retrieval and 159,968-byte copy | 1.621567 | 31.5% |
| FP16 to FP32 output conversion | 0.569299 | 11.0% |

Small timer-bracketing differences account for the remaining fraction. The
dominant avoidable costs are the complete 79,984-element output transfer and
the output conversion, not input conversion. The model performs about 41
million matrix MACs but emits almost 160 KiB every invocation.

The measured calling-thread time was 4.574888 ms for the CPU call and
2.553058 ms for the NPU bridge, a 44.194% reduction at that narrow thread
boundary. The runner did not isolate whole-process CPU consumption per
engine, power, energy, or concurrent pipeline throughput, so this must not
be promoted to a total CPU-utilization or efficiency claim.

Cold measurements do not change the conclusion:

| Boundary | Cold wall (ms) |
|---|---:|
| Stock ORT CPU | 4.734375 |
| NPU native | 4.530333 |
| NPU FP32 bridge | 5.181208 |

## 13. Device state and safety

Preflight and postflight used the same boot ID
`10d25963-b224-4ad7-86a3-c1b1ed323741`. The observed environment was:

```text
kernel:       Linux 5.4.242-ab575 #2, aarch64
CPU online:   0-3
governor:     schedutil
CPU current:  1.404 GHz preflight, 1.500 GHz postflight
CPU limits:   100 MHz to 2.016 GHz
NPU setting:  nanoqFreq=838860800
temperature:  38.0 C preflight, 40.9 C postflight
```

Postflight found no benchmark process, no `/dev/galcore` holder, no partial
timing file, no kernel alert, no boot/driver/library health difference, and
no controller error. The final timing file was read-only and matched the
pulled SHA-256.

No firmware, stock model, system DSO, service, module, sysfs value, clock,
governor, thermal control, driver state, or root-filesystem file was
modified.

The guarded cleanup command was attempted only after all evidence was
pulled and independently parsed. By then ADB listed no connected device, so
the serial guard exited before deletion. The exact staged root
`/tmp/wave-npu-poc15a-r2` is consequently **cleanup pending** rather than
claimed removed. It may be removed only with the fixed cleanup script after
the exact Wave serial reconnects; the native benchmark must not be replayed.

## 14. Decision and downstream gate

PoC 15A-R2 establishes all of the following:

- the substantial stock CTC NBG is numerically related to its adjacent ONNX;
- NPU compute can beat the Wave CPU for a large matrix-heavy graph;
- the complete native FP16 boundary is only marginally faster;
- the equivalent FP32 application boundary is slower because output
  transfer and conversion erase the compute gain;
- the NPU remains useful for resident FP16 pipelines, concurrency, or
  higher arithmetic-intensity graphs, but those benefits are not proven by
  this latency experiment.

The precommitted plan requires an FP32-bridge win before PoC 15B or PoC 15C
device work. That prerequisite failed cleanly. Both downstream live ladders
are therefore stopped with zero PoC 15B/15C device processes. Continuing
them would be changing the experiment after seeing the result.

The next performance design must attack the measured boundary rather than
repeat this candidate. Defensible options are:

1. keep adjacent operators and tensors resident in FP16 so the bridge is
   paid once per larger pipeline;
2. retrieve a compact downstream result instead of all 79,984 logits;
3. choose a fused denoising or encoder graph with a much higher
   compute-to-output ratio;
4. separately measure concurrent CPU availability and energy, neither of
   which this benchmark resolves.

Any such design is a new, separately reviewed PoC. It cannot reuse this
run ID, timing output, staged root, or one-process authorization.
