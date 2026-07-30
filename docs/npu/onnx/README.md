# Wave ONNX Audit and CPU Fallback

**PoC:** 9E/9F
**Date:** 2026-07-28
**Status:** Host-only gate passed
**General ONNX NPU support:** None approved
**Separate sealed partition:** Quantized U8 `[481]` Erf approved by PoC 13
**Device, driver, compiler, or ADB calls:** None

This directory records the first general ONNX-facing control plane for the
Insta360 Wave work. It deliberately solves model admission, capability
reporting, deterministic CPU execution, and fallback semantics before it
claims any NPU execution.

The concrete result is:

```text
ONNX model
    |
    v
strict bounded audit
    |
    +-- exact approved NPU signature? -- no
    |
    +-- audit  -> manifest only, no session
    +-- cpu    -> one ONNX Runtime CPU session
    +-- hybrid -> one ONNX Runtime CPU session, zero NPU attempts
    `-- strict -> refuse before session construction
```

This is useful general ONNX support on the host and a testable hybrid
interface. It is not a Wave NPU backend.

## PoC 13 partition-first update

PoCs 13A–13F did not globally enable this PoC 9 policy or claim an arbitrary
ONNX importer. They qualified one exact partition through a separate
hash-bound policy:

```text
ONNX/model orchestration
    |
    +-- CPU: STFT, features, affine transform, DFN candidate, selector
    |
    +-- exact approved signature and NBG hash?
            |
            `-- Wave NPU: UINT8 [481] Erf
                    |
                    `-- validated UINT8 [481] result
```

The exact NBG SHA-256 is
`96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94`.
It passed deterministic compilation, strict VPMN profiling, six-vector
numerical qualification, a complete 1,201-frame waveform run, and a
10,000-frame numerical soak.

This proves that partition-first hybrid ONNX support is practical: keep
unsupported nodes on CPU and dispatch only an exact, independently approved
subgraph to a sealed NBG. It does not approve Conv, MatMul, FFT, recurrent,
attention, arbitrary Erf shapes, or arbitrary graph signatures.

See the [`Erf-mask result`](../models/erf-mask/README.md), exact
[`partition policy`](../models/erf-mask/partition-policy.json), and
[`completion audit`](../models/erf-mask/completion-audit.md).

## What passed

- ONNX protobuf parsing, full checker validation, and strict shape inference;
- fixed resource limits and rejection of external tensor data;
- recursive accounting across ONNX control-flow subgraphs;
- stable executable graph signatures;
- exact per-node CPU/NPU placement reasons;
- deterministic CPU inference through only `CPUExecutionProvider`;
- fail-closed `audit`, `cpu`, `hybrid`, and `strict` behavior;
- one-attempt future NPU session-construction semantics;
- deterministic model, input, output, audit, and runtime evidence;
- a fixed Conv/Sigmoid spectral-mask control;
- a non-executing audit of the repository's real Silero VAD model;
- 35 focused tests and the existing 198-test NPU regression suite.

The current policy has zero approved NPU graph signatures. Therefore every
admitted node remains CPU-resident.

## Evidence map

| File | Purpose | SHA-256 |
|---|---|---|
| [`hybrid-policy.json`](hybrid-policy.json) | Limits, compiler-refusal binding, and empty approval registry | `41e9f935…b46` |
| [`spectral-mask-audit.json`](spectral-mask-audit.json) | Conv/Sigmoid admission and placement | `5a9346d5…1020` |
| [`spectral-mask-cpu-run.json`](spectral-mask-cpu-run.json) | Explicit CPU execution envelope | `45e673ad…acb` |
| [`spectral-mask-hybrid-run.json`](spectral-mask-hybrid-run.json) | Hybrid preflight fallback envelope | `cdfdacc2…d392` |
| [`spectral-mask-cpu-output.npz`](spectral-mask-cpu-output.npz) | Deterministic CPU output archive | `b97eebd8…9385` |
| [`spectral-mask-hybrid-output.npz`](spectral-mask-hybrid-output.npz) | Deterministic hybrid output archive | `b97eebd8…9385` |
| [`silero-vad-audit.json`](silero-vad-audit.json) | Real-model bounded audit, without inference | `cbd84668…48cc` |

The full digests remain in the files, fixture manifest, and
[`PoC 9E/9F notebook`](../lab-notebook/2026-07-28-poc9e-9f-onnx-cpu-fallback.md).

## Policy and compiler binding

The policy is not a free-standing allowlist. It is hash-bound to:

```text
docs/npu/profile/wave-viv-gpu-file.candidate.manifest.json
SHA-256 45b3c44488e4b8ceb8840111ffed67c18762bc8214a7e1a629dc6fc0f8721ddc
```

That PoC 9C manifest says `compiler_execution: false`. The ONNX policy must
therefore keep:

```json
{
  "compiler_qualified": false,
  "approved_npu_signatures": []
}
```

PoC 9D subsequently found the exact vendor Wave config and recovered the
same five-field grammar from a genuine `VIV_GPU_FILE` shader consumer. It
did not qualify a graph-to-NBG backend: the consumer is version-below-gate
and Acuity's NBG path uses an older simulator SDK. The PoC 9C refusal
manifest therefore remains the correct active policy binding. See the
[`PoC 9D qualification`](../lab-notebook/2026-07-28-poc9d-compiler-acquisition-static-qualification.md).

The loader rejects unknown or duplicate policy keys, path traversal,
noncanonical digests, a compiler-manifest hash mismatch, an approval under an
unqualified compiler, or an attempt to qualify a manifest that still refuses
compiler execution.

This binding prevents a frontend operator list from accidentally becoming an
NPU authorization.

## Admission boundary

Default limits are:

| Resource | Limit |
|---|---:|
| Model bytes | 64 MiB |
| Nodes across all graphs/subgraphs | 4,096 |
| Inputs per graph | 64 |
| Outputs per graph | 64 |
| Constant tensors across the graph tree | 4,096 |
| One encoded constant tensor | 64 MiB |
| All encoded constant tensors | 256 MiB |
| Tensor rank | 8 |
| One known dimension | 1,048,576 |
| Estimated tensor bytes | 256 MiB |

The auditor reads at most the model limit plus one byte before parsing. It
then rejects external tensor data anywhere in:

- top-level or nested graph initializers;
- sparse initializer values or indices;
- tensor-valued node attributes;
- sparse tensor-valued node attributes.

The recursive graph inventory prevents an `If`, `Loop`, or other graph-valued
attribute from hiding work behind a small top-level node count. Every graph
is counted, each graph's input/output limits are checked, and duplicate
nonempty node names are rejected within their owning graph.

Local ONNX functions and training metadata remain outside this PoC.

## Graph signature

The signature is:

```text
sha256(canonical JSON executable-graph document)
```

It covers:

- IR and opset versions;
- ordered model inputs and outputs;
- inferred tensor types and dimensions;
- ordered top-level nodes, wiring, domains, operators, and attributes;
- initializer types, shapes, encoded sizes, and payload hashes;
- normalized hashes for graph- and tensor-valued attributes.

Producer metadata and documentation strings are excluded, including
documentation nested inside graph-valued attributes. Constant payload or
topology changes alter the signature.

The byte-level source-model SHA-256 remains in the audit envelope, outside the
graph signature. This separates artifact identity from normalized executable
identity.

For control-flow models, nested graphs are counted and their normalized
protobuf is cryptographically covered by the owning top-level attribute. The
current placement manifest intentionally enumerates only top-level nodes.
Control-flow operators such as `If` are intrinsically unsupported for future
NPU placement, so the entire control-flow boundary remains one CPU partition.
No nested node is independently presented as NPU-capable.

## Placement is fail-closed

The auditor can identify operators that may be interesting in a future
compiler experiment. That is not support.

Every node needs both:

1. an exact graph signature in `approved_npu_signatures`; and
2. a compiler manifest that independently authorizes compilation.

Without both, reasons include:

- `PARTITION_NOT_PREFLIGHTED`;
- `COMPILER_DEPENDENCY_MISMATCH`;
- `ONNX_OPERATOR_UNSUPPORTED`;
- `ONNX_SHAPE_UNSUPPORTED`;
- `ONNX_TYPE_UNSUPPORTED`;
- `ONNX_QUANTIZATION_UNSUPPORTED`.

An operator name alone never yields `placement: "npu"`.

## Runtime modes

| Mode | Session behavior | Current classification |
|---|---|---|
| `audit` | No inference session | Audit manifest only |
| `cpu` | Exactly one CPU-only session | `cpu-explicit` |
| `hybrid` | Audit first; currently zero NPU attempts and one CPU session | `cpu-preflight-fallback` |
| `strict` | Requires an approved NPU partition | `PARTITION_NOT_NPU_RESIDENT` before any session |

The CPU session is constructed with:

```text
providers                 = ["CPUExecutionProvider"]
execution_mode            = ORT_SEQUENTIAL
graph_optimization_level  = ORT_ENABLE_EXTENDED
intra_op_num_threads      = 1
inter_op_num_threads      = 1
```

The provider list is checked after construction. An unexpected provider is a
failure, not a successful fallback.

The future construction interface is also test-locked:

- make at most one NPU construction attempt;
- discard incomplete NPU state after failure;
- in `hybrid`, create one CPU session and report
  `cpu-session-construction-fallback`;
- in `strict`, return the NPU construction error and create no CPU session.

The production PoC cannot enter that future branch because no signature is
approved and no NPU factory is configured.

## Tensor contract

Inputs and outputs are NPZ archives with pickle disabled. Runtime validation
requires:

- the exact tensor-name set;
- exact NumPy dtype;
- exact rank and every known dimension;
- C-contiguous arrays;
- finite floating-point values;
- the configured per-tensor byte bound.

No implicit dtype conversion, broadcasting, missing input, extra input, NaN,
or infinity is accepted at the interface.

Output archives use fixed ZIP metadata and deterministic member ordering.
Evidence records each tensor's dtype, shape, raw byte count, and raw payload
SHA-256.

## Spectral-mask control

The deterministic fixture is:

```text
features float32 [1, 1, 8]
    -> Conv1D(weights=[0.25, 0.50, 0.25], pad=1)
    -> Sigmoid
    -> mask float32 [1, 1, 8]
```

It is a tensor-level signal-processing control, not a trained denoiser and
not an audio-quality claim.

Results:

| Field | CPU | Hybrid |
|---|---:|---:|
| Graph signature | `13c8633a…eaed` | `13c8633a…eaed` |
| NPU attempts | 0 | 0 |
| CPU sessions | 1 | 1 |
| Provider | CPU | CPU |
| `mask` payload SHA-256 | `788cab70…fc40` | `788cab70…fc40` |
| Output NPZ SHA-256 | `b97eebd8…9385` | `b97eebd8…9385` |

The two output archives compare byte-for-byte equal.

The graph's Conv and Sigmoid nodes still report CPU placement with
`PARTITION_NOT_PREFLIGHTED` and `COMPILER_DEPENDENCY_MISMATCH`.

## Real Silero VAD audit

The repository model:

```text
models/ai-models/asr-models/model/vad/silero_vad_v5.onnx
bytes    2,327,524
SHA-256 2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f
```

passed CPU admission without inference.

Its complete recursive inventory is:

| Field | Value |
|---|---:|
| Graphs | 51 |
| Maximum subgraph depth | 4 |
| Top-level nodes | 5 |
| Nested nodes | 684 |
| Total nodes | 689 |
| Tensor-valued attributes | 345 |
| Encoded constant bytes | 2,186,503 |

The five top-level nodes are `Constant`, `Equal`, `If`, and two `Identity`
nodes. All five are in one CPU partition. The `If` branches contain the
actual recurrent network and remain behind that unsupported CPU control-flow
boundary. The audit does not claim that only five operations execute, and it
does not claim NPU support for any nested operation.

This PoC did not run Silero. A real streaming VAD or denoiser integration
still needs explicit state management, frame sizing, latency, numerical,
audio-quality, and device packaging work.

## Reproduction

From the repository root:

```bash
cd waved/tools/onnx-hybrid
uv sync --python 3.12 --dev
uv run wave-onnx-hybrid verify-fixtures fixtures
uv run pytest -q
uv run ruff check .
uv run ruff format --check .
```

Audit the fixed control without creating a session:

```bash
uv run wave-onnx-hybrid audit \
  fixtures/spectral_mask_conv.onnx \
  --output /tmp/wave-spectral-audit.json
```

Run explicit CPU mode:

```bash
uv run wave-onnx-hybrid run \
  fixtures/spectral_mask_conv.onnx \
  --inputs fixtures/spectral_mask_input.npz \
  --mode cpu \
  --outputs /tmp/wave-spectral-cpu.npz \
  --evidence /tmp/wave-spectral-cpu.json
```

Run current hybrid mode:

```bash
uv run wave-onnx-hybrid run \
  fixtures/spectral_mask_conv.onnx \
  --inputs fixtures/spectral_mask_input.npz \
  --mode hybrid \
  --outputs /tmp/wave-spectral-hybrid.npz \
  --evidence /tmp/wave-spectral-hybrid.json

cmp /tmp/wave-spectral-cpu.npz /tmp/wave-spectral-hybrid.npz
```

Confirm strict refusal:

```bash
uv run wave-onnx-hybrid run \
  fixtures/spectral_mask_conv.onnx \
  --inputs fixtures/spectral_mask_input.npz \
  --mode strict \
  --outputs /tmp/wave-spectral-strict.npz \
  --evidence /tmp/wave-spectral-strict.json
```

The final command exits nonzero with
`PARTITION_NOT_NPU_RESIDENT` and creates neither requested output.

Audit Silero without inference:

```bash
uv run wave-onnx-hybrid audit \
  ../../../models/ai-models/asr-models/model/vad/silero_vad_v5.onnx \
  --output /tmp/wave-silero-audit.json
```

All output paths are exclusive. Existing files are refused rather than
overwritten.

## Exact host environment

The tracked `uv.lock` SHA-256 is:

```text
635e0712489724fcad12b216a8d1a66b60696073ed2f1fc9ca0bccf0d7e96faf
```

The verified environment used:

| Component | Version |
|---|---|
| Python | 3.12.9 |
| uv | 0.9.26 |
| NumPy | 2.5.1 |
| ONNX | 1.22.0 |
| ONNX Runtime | 1.27.0 |
| pytest | 9.1.1 |
| Ruff | 0.15.22 |

These are host-PoC identities. They do not describe the Wave firmware's
packaged Python or ONNX Runtime environment.

## What is still required for NPU execution

No general ONNX node may move to the Wave NPU until a later PoC supplies all
of:

1. the missing no-kernel SDK or another exact compiler consumer;
2. a hash-pinned compiler/runtime/profile dependency set;
3. one untouched Wave-targeted artifact for a fixed graph;
4. static validation of its target and section integrity;
5. a bounded execution against a CPU numerical oracle;
6. repeatability, cleanup, health, and fallback evidence;
7. an exact graph signature added through a reviewed policy update.

Only then can the partition schema be exercised with an NPU-resident
partition. Broad operator support, arbitrary ONNX compilation, denoising
quality, real-time scheduling, and production deployment remain separate
claims.
