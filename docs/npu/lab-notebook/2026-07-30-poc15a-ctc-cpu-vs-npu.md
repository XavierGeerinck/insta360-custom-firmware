# PoC 15A — Stock CTC CPU versus NPU

**Date:** 2026-07-30
**Device:** Insta360 Wave `PSP_0000000000000000`
**Status:** first live attempt invalidated by a host-runner protocol defect;
corrected PoC 15A-R2 is host-qualified but has not been executed.

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

## 9. Authorization boundary

The approved PoC 15A plan capped this stage at one live process and
forbade automatic retries. That process has been consumed. R2 is therefore
host-qualified but deliberately not staged or run.

A second native process is justified technically because the first process
did not implement the approved protocol, but it is still a second live
process. It requires explicit authorization before:

```text
waved/tools/npu-compiler/stage-device-ctc-cpu-npu-benchmark.sh
python3 waved/scripts/npu/npu_ctc_cpu_npu_benchmark_lab.py \
  run --run-id <new-r2-run-id>
```

Until then, no CPU-versus-NPU speed claim is made and no downstream device
candidate is eligible.
