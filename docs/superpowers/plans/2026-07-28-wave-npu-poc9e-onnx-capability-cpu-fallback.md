# Wave NPU PoC 9E/9F: ONNX Audit and CPU Fallback

**Date:** 2026-07-28
**Status:** Completed host-only
**Authorization:** The owner requested general ONNX support with hybrid CPU
fallback and continued stepwise implementation
**Design:** `docs/superpowers/specs/2026-07-28-wave-npu-compiler-profile-onnx-hybrid-design.md`
**Depends on:** PoC 9A profile and PoC 9C compiler-refusal manifest
**Safety class:** Host-side ONNX parsing, generation, audit, and CPU inference

## 1. Goal

Build the frontend and failure contract now, even though the Wave NPU
compiler is not qualified.

The PoC must:

- admit well-formed ONNX models under strict size and external-data policy;
- infer and report shapes, types, opsets, attributes, constants, and graph
  topology;
- calculate a deterministic graph signature;
- emit per-node placement and exact rejection reasons;
- run accepted models with ONNX Runtime's CPU Execution Provider;
- implement `audit`, `cpu`, `hybrid`, and `strict` modes;
- guarantee that current `hybrid` mode performs zero NPU attempts and creates
  one CPU-only session;
- guarantee that current `strict` mode refuses before inference;
- provide a small noise-mask-relevant Conv/Sigmoid control;
- leave an exact interface for future approved NPU signatures.

The PoC does not compile or execute an NPU graph.

## 2. Why this can proceed before the compiler

PoC 9C's compiler manifest says:

```text
consumer_status:
  unverified-missing-no-kernel-sdk

compiler_execution:
  false
```

That blocks NPU compilation, not ONNX model validation or CPU execution.

A useful hybrid stack should fail closed:

```text
model
  -> audit
  -> no approved NPU signatures
  -> one CPU-only ONNX Runtime session
  -> output labeled cpu-preflight-fallback
```

This lets application APIs, telemetry, model storage, input validation,
numerical oracles, and audio continuity mature independently.

## 3. Tool package

Create a self-contained Python 3.12 `uv` project:

```text
waved/tools/onnx-hybrid/
  pyproject.toml
  uv.lock
  src/wave_onnx_hybrid/
    __init__.py
    audit.py
    cli.py
    errors.py
    fixtures.py
    runtime.py
  tests/
  fixtures/
```

Pin:

- `onnx`;
- `onnxruntime`;
- `numpy`;
- `pytest`.

The lockfile is part of the evidence. Production device packaging remains a
separate task because the Wave currently carries a different CPU-only ONNX
Runtime build.

## 4. Strict model admission

Default limits:

```text
model bytes:                 64 MiB
graph nodes:                 4,096
graph inputs:                64
graph outputs:               64
initializers:                4,096
single initializer bytes:    64 MiB
total initializer bytes:     256 MiB
tensor rank:                 8
known dimension:             1..2^20
estimated tensor bytes:      256 MiB
attribute nesting/depth:     bounded by ONNX checker and local counters
```

The node and constant-tensor limits apply across the entire graph/subgraph
tree, not only the top-level graph. The audit reports top-level and nested
counts separately.

Reject:

- protobuf parse failure;
- ONNX checker failure;
- missing graph;
- external tensor data;
- sparse external data;
- absolute or relative external paths;
- unsupported custom domain for NPU eligibility;
- duplicate nonempty node names;
- duplicate graph value definitions;
- unbounded tensor size for NPU eligibility;
- invalid or overflowing dimensions;
- unsupported element type for NPU eligibility;
- malformed initializer payload;
- graph functions or training metadata that alter the execution contract
  without explicit support.

CPU admission and NPU eligibility are separate. A model can be valid for CPU
while every node remains ineligible for NPU placement.

## 5. Canonical graph signature

The signature document includes:

- IR version;
- sorted opset imports;
- ordered graph inputs and outputs with element types and dimensions;
- ordered nodes with stable index, domain, op type, input/output names, and
  canonical attributes;
- initializer names, data types, dimensions, byte sizes, and payload hashes;
- graph topology references;

Volatile producer metadata and documentation strings are excluded.
The source model SHA-256 is recorded in the audit envelope, outside the
executable graph signature. This lets byte-different models share a signature
only when the normalized executable graph agrees.

The signature is:

```text
sha256(canonical JSON signature document)
```

Two byte-different models with the same executable graph may share a graph
signature only when their canonical executable fields and constant payload
hashes agree.

## 6. Capability and placement policy

There are currently no approved NPU graph signatures:

```text
approved_npu_signatures = {}
```

Every node therefore receives placement `cpu`.

The auditor still reports intrinsic blockers:

- `ONNX_SHAPE_UNSUPPORTED`;
- `ONNX_TYPE_UNSUPPORTED`;
- `ONNX_OPERATOR_UNSUPPORTED`;
- `ONNX_QUANTIZATION_UNSUPPORTED`;
- `PARTITION_NOT_PREFLIGHTED`;
- `COMPILER_DEPENDENCY_MISMATCH`.

No operator name by itself is sufficient for NPU placement. Even a Conv node
remains CPU until its exact graph signature, shapes, types, attributes,
constants, profile, compiler, and output artifact pass later gates.

Consecutive CPU nodes may be grouped into a CPU partition. Boundary tensors
and estimated transfer bytes are still calculated so a future approved
signature can be compared without changing the manifest schema.

## 7. Modes

`audit`
: Parse, validate, infer, sign, and emit a manifest. Create no inference
  session.

`cpu`
: Require a CPU-admissible model, create exactly one ONNX Runtime session
  with `providers=["CPUExecutionProvider"]`, validate inputs, run once, and
  emit output/evidence hashes.

`hybrid`
: Audit first. With zero approved NPU partitions, create exactly one
  CPU-only session, make zero NPU factory/compile/run calls, and label the
  result `cpu-preflight-fallback`.

`strict`
: Require at least one designated, approved, NPU-resident partition. In the
  current unqualified state, fail with `PARTITION_NOT_NPU_RESIDENT` before
  creating any inference session.

No mode silently changes to another mode.

## 8. Session-construction fallback contract

Provide an injectable session planner for tests.

If a future approved NPU factory is selected and session construction fails:

1. capture one stable failure;
2. discard incomplete state;
3. make no second NPU attempt;
4. in `hybrid`, create one CPU-only session;
5. in `strict`, propagate failure without CPU inference;
6. report `cpu-session-construction-fallback` only in the hybrid case.

The current production configuration never reaches this branch because no
NPU signature is approved.

## 9. CPU runtime contract

Inputs and outputs use `.npz`:

- `allow_pickle=False`;
- exact input-name set;
- element type agreement;
- rank and known-dimension agreement;
- contiguous arrays;
- finite floating-point input by default;
- per-tensor byte bound;
- no implicit dtype conversion.

Session options:

- graph optimization explicitly recorded;
- intra/inter-op thread counts explicitly recorded;
- only `CPUExecutionProvider`;
- provider list checked after session creation.

Outputs must:

- exactly match the model output-name set;
- match declared type/rank/known dimensions;
- remain within byte bounds;
- be finite for floating-point outputs;
- be serialized deterministically enough for per-tensor payload hashes.

## 10. Fixtures

Generate deterministic ONNX fixtures:

### 10.1 `spectral_mask_conv.onnx`

```text
input:   float32 [1, 1, 8]
Conv:    fixed [1, 1, 3] smoothing weights, pad 1
Sigmoid
output:  float32 [1, 1, 8]
```

This is a tensor-level spectral-mask control, not a trained denoiser.

### 10.2 `mixed_sin.onnx`

```text
Conv -> Sin
```

It provides an explicitly unsupported future-NPU operator boundary.

### 10.3 `dynamic_identity.onnx`

```text
float32 [1, 1, dynamic] -> Identity
```

It remains CPU-admissible but receives
`ONNX_SHAPE_UNSUPPORTED` for NPU eligibility.

Fixture generation must be byte-deterministic and verified against a strict
hash manifest.

## 11. Tests

Admission:

- model/file size limits;
- external data refusal;
- checker failure;
- duplicate node-name refusal;
- initializer bounds;
- dimension/rank overflow;
- custom-domain reporting;
- dynamic-shape CPU admission and NPU refusal.

Signature:

- fixture hashes stable;
- graph signature stable;
- doc strings and producer metadata do not affect graph signature;
- weight change does affect graph signature;
- node order/topology change does affect graph signature.

Placement:

- every current node is CPU;
- every rejected node has at least one exact reason;
- no manifest can report NPU residency with an empty approval registry;
- boundary tensors and bytes are deterministic;
- mixed graph partitions are deterministic.

Runtime:

- CPU Conv/Sigmoid output matches a NumPy reference;
- `hybrid` output equals `cpu`;
- `hybrid` records zero NPU attempts;
- `strict` refuses before creating a session;
- injected NPU construction failure is attempted once;
- hybrid construction failure creates one CPU session;
- strict construction failure creates no CPU session;
- dtype, shape, missing/extra input, NaN, and oversize input refusal;
- output shape/type/finite checks.

CLI:

- unknown keys and modes fail;
- output paths are explicit and atomically written;
- audit creates no output tensors;
- CPU and hybrid manifests identify the CPU provider;
- existing outputs are not silently overwritten unless an explicit
  deterministic verification command is used.

## 12. Generated evidence

Create:

```text
docs/npu/onnx/README.md
docs/npu/onnx/hybrid-policy.json
docs/npu/onnx/spectral-mask-audit.json
docs/npu/onnx/spectral-mask-cpu-run.json
docs/npu/onnx/spectral-mask-hybrid-run.json
docs/npu/lab-notebook/2026-07-28-poc9e-9f-onnx-cpu-fallback.md
```

Update NPU and tool indexes.

## 13. Stop conditions

Stop without an NPU action if:

- the PoC 9C manifest no longer refuses compiler execution;
- a dependency or fixture hash changes;
- ONNX Runtime exposes a non-CPU provider unexpectedly;
- model validation needs external data;
- a fixture is nondeterministic;
- strict mode creates a CPU session;
- hybrid mode attempts an NPU action without an approved signature;
- CPU and hybrid outputs differ;
- any command would contact ADB, `/dev/galcore`, or a Wave process.

## 14. Completion gate

PoC 9E/9F host fallback passes when:

- fixture generation and graph signatures are deterministic;
- model placement is deterministic and fully explained;
- all current placement is CPU;
- CPU and hybrid outputs are numerically identical;
- hybrid proves zero NPU attempts;
- strict refuses before inference;
- failure-injection tests prove one-attempt semantics;
- the real Silero VAD model can be audited within limits without inference;
- all tool and existing NPU tests pass;
- no compiler, graph, device, driver, or ADB call occurred.

Passing this gate establishes general ONNX CPU admission and a fail-closed
hybrid control plane. It does not establish any NPU operator support.

## 15. Completed result

The host gate passed on 2026-07-28:

- 35 focused tests passed;
- the three fixture models and two tensor archives regenerated byte-for-byte;
- two complete audit/run passes were byte-identical;
- `cpu` and `hybrid` produced byte-identical deterministic NPZ archives;
- both modes produced the same `mask` tensor payload SHA-256,
  `788cab709bee63c8eb281065d49b90daec93d24ef945c8b21bf61350bf37fc40`;
- `hybrid` recorded zero NPU attempts and one CPU session;
- `strict` refused before session construction;
- the real Silero VAD model was admitted for CPU audit with 51 graphs,
  689 total nodes, 345 tensor-valued constants, and zero NPU partitions;
- nested graph limits and nested external-data rejection are test-locked;
- the existing 198-test NPU suite remained green;
- no compiler, NPU provider, driver, device, or ADB call occurred.

The next positive NPU gate remains an exact, hash-pinned compiler result for
one fixed graph and architecture profile. Until then,
`approved_npu_signatures` remains empty.
