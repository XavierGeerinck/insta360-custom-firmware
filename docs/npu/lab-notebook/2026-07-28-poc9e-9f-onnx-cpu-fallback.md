# PoC 9E/9F — General ONNX Audit and Hybrid CPU Fallback

**Date:** 2026-07-28
**Status:** Passed the host-only gate
**Device contact:** None
**Wave API, graph, driver, or ADB calls:** None
**Compiler calls:** None
**NPU provider calls:** None
**NPU signatures approved:** Zero

## 1. Outcome

PoC 9E/9F implemented a strict ONNX admission and capability auditor, a
deterministic ONNX Runtime CPU oracle, and a fail-closed hybrid session
interface.

The current decision path is:

```text
ONNX model
    -> bounded parse/check/infer/sign
    -> exact NPU signature approval lookup
    -> no approved signature
    -> all placement remains CPU
```

Four modes are now explicit:

```text
audit:   create no session
cpu:     create one CPU-only session
hybrid:  create one CPU-only session, make zero NPU attempts
strict:  refuse before any session
```

A fixed Conv/Sigmoid signal-mask control ran through both `cpu` and `hybrid`.
The two output archives were byte-identical:

```text
output NPZ SHA-256:
  b97eebd8c8d50e48fbc6fc0e43ecfd3b49a8b89eb8930af8b418dfe172398385

raw mask tensor SHA-256:
  788cab709bee63c8eb281065d49b90daec93d24ef945c8b21bf61350bf37fc40
```

The output also agreed with the independent NumPy Conv/Sigmoid calculation:

```text
maximum absolute difference: 5.960464477539063e-08
rtol: 1e-6
atol: 1e-6
```

That comparison is numerical, not byte equality. NumPy and ONNX Runtime
rounded several float32 sigmoid results one unit in the last place
differently. CPU and hybrid used the same pinned runtime and were exactly
byte-equal to each other.

The real repository Silero VAD model passed non-executing CPU admission. Its
top-level graph has five nodes, but the completed recursive inventory found
51 graphs and 689 total nodes. All placement remains one CPU partition.

This passes general host ONNX admission and CPU fallback. It does not prove a
Wave NPU compiler, provider, operator, partition, or inference path.

## 2. Question and falsifiable hypothesis

Question:

> Can the project accept and explain ordinary ONNX models now, provide a
> deterministic CPU oracle, and expose hybrid fallback semantics without
> implying that the unqualified Wave compiler can execute them on the NPU?

Hypothesis:

> A strict host tool can validate, shape-infer, sign, and place bounded ONNX
> models; keep every node CPU-resident while no exact NPU signature is
> approved; run a fixed Conv/Sigmoid graph through a CPU-only ONNX Runtime
> session; produce the same output in CPU and hybrid modes; and refuse strict
> mode before session construction.

The hypothesis would fail if any of these occurred:

- policy approval could bypass the PoC 9C compiler refusal;
- external tensor data or an oversized nested graph escaped admission limits;
- graph signatures changed with producer or documentation metadata;
- a constant or topology change did not alter the signature;
- any node became NPU-resident with an empty approval registry;
- audit mode created an inference session;
- current hybrid mode attempted an NPU construction or run;
- strict mode created a CPU session;
- a future construction failure retried the NPU;
- CPU and hybrid differed numerically or in serialized output;
- ONNX Runtime selected a provider other than CPU;
- the fixture generator was nondeterministic;
- the real Silero model could not be audited within the configured limits;
- implementation required ADB, `/dev/galcore`, a Wave process, or a compiler.

None occurred.

## 3. Why this PoC exists

PoCs 5 through 8 established a narrow positive path for one firmware-provided
Erf operation and clean negative boundaries for ordinary MatrixMul and
Conv2d graphs. PoCs 9A and 9C recovered the architecture identity and a
simulator configuration grammar, but the required no-kernel graph compiler
was absent from that corpus. PoC 9D later found the same grammar in a real
shader compiler while confirming that a compatible graph-to-NBG consumer
remains absent.

That leaves two independent problems:

1. compiler qualification for Wave-native NBG generation;
2. application-facing model admission, numerical reference execution, and
   fallback behavior.

The first remains externally blocked. The second can be solved safely on the
host. Doing so makes later NPU work measurable:

- an exact model and graph identity already exists;
- candidate partition boundaries are deterministic;
- the CPU result is a numerical oracle;
- input/output validation is fixed;
- fallback classifications are fixed;
- strict mode can prove that it did not silently execute CPU work.

## 4. Authorization and safety gate

The owner requested general ONNX support and hybrid CPU fallback, then
requested continued stepwise implementation.

Allowed:

- create and parse small local ONNX fixtures;
- read and audit the repository's Silero VAD ONNX file;
- install pinned host Python dependencies into the tool-local virtual
  environment;
- run ONNX checker and shape inference;
- run the fixed control through ONNX Runtime's CPU provider;
- inject fake CPU/NPU session factories in unit tests;
- write deterministic host evidence and documentation.

Explicitly prohibited:

- ADB or any other Wave transport;
- `/dev/galcore`;
- Wave filesystem, services, processes, or audio streams;
- TIM-VX/OpenVX graph creation;
- NBG generation, mutation, import, or execution;
- `libCLC.so`, `libVSC.so`, `libGAL.so`, or firmware library calls;
- an ONNX Runtime NPU provider;
- adding an NPU graph signature to the approval registry;
- treating Conv, Sigmoid, Identity, or any other operator name as proof of
  Wave NPU support.

There was no native watchdog because no device or native Wave probe ran.
Host commands were bounded by their ordinary test/process boundaries.

## 5. Worktree and implementation identity

The base commit during this host experiment was:

```text
7684c54abaae4cfd3c720da108064951404b9531
```

The worktree was intentionally dirty with the uncommitted, scoped NPU
research series and pre-existing report edits. No unrelated change was
discarded or overwritten.

Tool:

```text
waved/tools/onnx-hybrid/
```

Important source identities after the completed recursive-graph hardening:

| File | SHA-256 |
|---|---|
| `src/wave_onnx_hybrid/audit.py` | `1c54aa1ff1e3305bea22bd77e2aa2fd98c4da7d80b70fc3b79484b6c96df5a11` |
| `src/wave_onnx_hybrid/cli.py` | `018684d3e91210a489810c3adbb0b6ea0eee40c38b3268b08ec2e915980727c5` |
| `src/wave_onnx_hybrid/errors.py` | `342e6a63157f3686d9caba09a9366983aff8258f21699d74fb4e6723fa015a63` |
| `src/wave_onnx_hybrid/fixtures.py` | `65da6d4880ac9d4f54da21bd249e78d17f57ee5d0c82707e868a7d90f7707fa8` |
| `src/wave_onnx_hybrid/runtime.py` | `ce17f94a97523139ffa8428c8525d41ceab44925fa63c6de1b39e03f3f00ce5a` |
| `tests/test_hybrid.py` | `09c8585fe8bbd60766da99b10bea9cd97f004d0e831f132230a2a5d70cf6b260` |
| `pyproject.toml` | `afe4e0b75556c5bdec11bcbbf2358b42ff5893dc229b2f1bfd8455ed8d3a747a` |
| `uv.lock` | `635e0712489724fcad12b216a8d1a66b60696073ed2f1fc9ca0bccf0d7e96faf` |

## 6. Pinned host environment

The `uv` environment used:

| Component | Version |
|---|---|
| Python | 3.12.9 |
| uv | 0.9.26 |
| NumPy | 2.5.1 |
| ONNX | 1.22.0 |
| ONNX Runtime | 1.27.0 |
| pytest | 9.1.1 |
| Ruff | 0.15.22 |

The tracked project requires Python `>=3.12,<3.13`, pins ONNX and ONNX
Runtime exactly, constrains NumPy to major version 2, and resolves all
transitive versions in `uv.lock`.

This environment is a host research environment. It is not a claim about
packages available on the Wave root filesystem.

## 7. Compiler-refusal policy binding

Policy:

```text
docs/npu/onnx/hybrid-policy.json
SHA-256:
  41e9f9354608e9ca22aaed5721611260555ec9edf4fb5d334a386467c1c5cb46
```

Compiler-profile dependency:

```text
docs/npu/profile/wave-viv-gpu-file.candidate.manifest.json
SHA-256:
  45b3c44488e4b8ceb8840111ffed67c18762bc8214a7e1a629dc6fc0f8721ddc
```

The dependency says:

```text
decision: parser-shaped-candidate-not-compiler-qualified
authorization.compiler_execution: false
```

The ONNX policy correspondingly says:

```text
compiler_qualified: false
approved_npu_signatures: []
```

The strict loader:

- rejects duplicate and unknown JSON keys;
- requires repository-contained policy and compiler-manifest paths;
- rejects absolute paths and `..`;
- verifies the exact compiler-manifest digest;
- requires sorted, unique, lowercase SHA-256 signatures;
- refuses approved signatures while `compiler_qualified` is false;
- refuses compiler qualification while the dependency manifest denies
  compiler execution.

This is the central fail-closed link between frontend capability reporting
and the unresolved compiler.

## 8. ONNX admission implementation

Default limits:

| Resource | Limit |
|---|---:|
| Model bytes | 67,108,864 |
| Nodes across the graph tree | 4,096 |
| Inputs per graph | 64 |
| Outputs per graph | 64 |
| Constant tensors across the graph tree | 4,096 |
| One encoded constant tensor | 67,108,864 |
| Total encoded constant tensor bytes | 268,435,456 |
| Rank | 8 |
| Known dimension | 1,048,576 |
| Estimated tensor bytes | 268,435,456 |

The sequence is:

1. read at most `max_model_bytes + 1`;
2. reject an empty or oversized payload;
3. parse the protobuf;
4. require a graph;
5. reject external tensor data recursively;
6. reject functions and training metadata;
7. inventory and bound all graph-valued attributes recursively;
8. run `onnx.checker.check_model(..., full_check=True)`;
9. run strict type-checking shape inference;
10. normalize inputs, outputs, constants, nodes, and attributes;
11. compute the graph signature;
12. calculate exact per-node refusal reasons;
13. form deterministic top-level partitions and boundary tensors.

### 8.1 Recursive graph correction

An initial review before evidence freeze found that the first implementation
bounded only the top-level node list. This was safe for the tiny fixtures but
would have reported Silero as a five-node graph even though its `If`
attributes hold the real network.

The implementation was corrected before completion to:

- walk all `GRAPH` and `GRAPHS` attributes in deterministic depth-first
  order;
- apply the node limit to the full tree;
- check per-graph input/output and duplicate-name rules;
- count initializers and tensor-valued attributes across the tree;
- apply individual and total constant-byte limits;
- reject external data inside nested and tensor-valued attributes;
- report graph count, depth, top-level nodes, nested nodes, and constant
  counts.

Three focused tests were added:

- nested graph inventory and total-node bound;
- external tensor data inside a nested graph;
- nested documentation metadata excluded from the graph signature.

The focused suite increased from 32 to 35 passing tests.

## 9. Executable graph signature

The signature document contains:

- schema and ONNX IR versions;
- sorted opset imports;
- ordered inferred input and output contracts;
- sorted initializer records with encoded payload hashes;
- ordered top-level nodes, wiring, domains, operators, and normalized
  attributes;
- normalized hashes for graph-, tensor-, and sparse-tensor-valued
  attributes.

It excludes:

- source filename;
- source artifact SHA-256;
- producer name/version;
- model, graph, node, value, tensor, and attribute documentation strings.

The source artifact digest remains separately recorded in `source_model`.

Tests proved:

- repeated canonicalization is stable;
- a weight change changes both model and graph hashes;
- a topology change changes the graph hash;
- producer and top-level documentation changes do not change the graph hash;
- nested graph and node documentation changes do not change the graph hash.

The fixed spectral-mask graph signature is:

```text
13c8633af1974817c34ff1437676f047d789ee7c2110c91d43ee434a03afeaed
```

The real Silero signature after normalized nested-graph coverage is:

```text
035071de3d208b626e52c63533651a3316dd55df337cdc59bad49d5d2304cb87
```

## 10. Placement contract

The future-candidate operator list is diagnostic only. Each top-level node
is evaluated for:

- standard versus custom domain;
- operator family;
- quantization involvement;
- tensor versus non-tensor values;
- candidate data type;
- fully static shape;
- exact graph-signature approval;
- compiler qualification.

All current nodes receive:

```text
placement: cpu
```

Even the fixed `Conv` and `Sigmoid` nodes carry:

```text
COMPILER_DEPENDENCY_MISMATCH
PARTITION_NOT_PREFLIGHTED
```

An intrinsically unsupported operator such as fixture `Sin` additionally
carries:

```text
ONNX_OPERATOR_UNSUPPORTED
```

A dynamic `Identity` fixture additionally carries:

```text
ONNX_SHAPE_UNSUPPORTED
```

No audit can produce an NPU partition while the approval registry is empty.

## 11. Runtime and fallback contract

CPU construction uses exactly:

```text
providers                 ["CPUExecutionProvider"]
execution mode            ORT_SEQUENTIAL
graph optimization        ORT_ENABLE_EXTENDED
intra-op threads           1
inter-op threads           1
```

The observed provider list is checked after construction.

The tensor boundary rejects:

- missing or extra names;
- wrong dtype;
- wrong rank or known dimension;
- non-contiguous arrays;
- oversized arrays;
- NaN or infinity;
- wrong output name, type, rank, dimension, size, or finiteness.

Session-construction tests use counting fake factories. They proved:

| Case | NPU attempts | CPU sessions | Result |
|---|---:|---:|---|
| Current hybrid, no eligible partition | 0 | 1 | `cpu-preflight-fallback` |
| Current strict, no eligible partition | 0 | 0 | `PARTITION_NOT_NPU_RESIDENT` |
| Future eligible hybrid, NPU constructor fails | 1 | 1 | `cpu-session-construction-fallback` |
| Future eligible strict, NPU constructor fails | 1 | 0 | `PARTITION_COMPILE_FAILURE` |

No retry loop exists.

## 12. Deterministic fixtures

Fixture manifest:

```text
waved/tools/onnx-hybrid/fixtures/manifest.json
SHA-256:
  687e40e1f8094ab254abb81e683ee0319fb1aa94fe8c58cdf87ed400927ae67f
```

Models:

| Fixture | Bytes | Purpose | SHA-256 |
|---|---:|---|---|
| `dynamic_identity.onnx` | 272 | Dynamic-shape CPU admission/NPU refusal | `677ab415db19fcdfdef60504408a68cdfd8f5feb3d0d586297722df525c2991e` |
| `mixed_sin.onnx` | 403 | Unsupported future-NPU boundary | `043ec66203b5a73844b38f2b388c1da97e2cd334e3e2fcdcb51c23f78e1ea58f` |
| `spectral_mask_conv.onnx` | 429 | Conv/Sigmoid numerical control | `175b5ca1f7d13778cc4d1e4fc7670aecf850c5f0f11a977e52bbbaacd81af165` |

Tensor archives:

| Fixture | Bytes | SHA-256 |
|---|---:|---|
| `spectral_mask_input.npz` | 282 | `78d19010a09ee4ed042965ac3094594a977ba682308fc574631e05797c3a47e1` |
| `spectral_mask_expected.npz` | 274 | `ce36553c40d2b84d6715af0a85f3a464f5344d0741ec9efb8f51230010e6b2a7` |

`verify-fixtures` regenerates all five artifacts in memory and compares every
byte and manifest record.

## 13. Spectral-mask experiment

Graph:

```text
features float32 [1, 1, 8]
    -> Conv1D
       weight [1, 1, 3] = [0.25, 0.50, 0.25]
       bias   [1]       = [0.0]
       pads             = [1, 1]
    -> Sigmoid
    -> mask float32 [1, 1, 8]
```

This is a fixed tensor control. It does not consume PCM, estimate noise,
maintain recurrent state, or establish perceptual denoising quality.

Input:

```text
raw tensor bytes: 32
raw tensor SHA-256:
  4f44161a6bec4e4a987439877ce4518b0822bbf66bec96acc0f05b863a4cf1d3
```

CPU evidence:

```text
classification: cpu-explicit
NPU attempts:   0
CPU sessions:  1
provider:       CPUExecutionProvider
```

Hybrid evidence:

```text
classification: cpu-preflight-fallback
NPU attempts:   0
CPU sessions:  1
provider:       CPUExecutionProvider
```

The complete result artifacts are:

| Artifact | SHA-256 |
|---|---|
| `spectral-mask-audit.json` | `5a9346d50b0d2e52abe0c0d1d81dafc8c71bc1e0bab47fc8f990576ebdb41020` |
| `spectral-mask-cpu-run.json` | `45e673ad87c132502a2dddc13037671534e7285b695fd37bcda51dec977bbacb` |
| `spectral-mask-hybrid-run.json` | `cdfdacc2af6689d03b04610ecddfc2ce770ee05547c29a74795b18afd7d7d392` |
| `spectral-mask-cpu-output.npz` | `b97eebd8c8d50e48fbc6fc0e43ecfd3b49a8b89eb8930af8b418dfe172398385` |
| `spectral-mask-hybrid-output.npz` | `b97eebd8c8d50e48fbc6fc0e43ecfd3b49a8b89eb8930af8b418dfe172398385` |

## 14. Real Silero VAD audit

Source:

```text
models/ai-models/asr-models/model/vad/silero_vad_v5.onnx
bytes:
  2,327,524
SHA-256:
  2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f
```

Audit evidence:

```text
docs/npu/onnx/silero-vad-audit.json
SHA-256:
  cbd8466865621acf5a2f98924f52f20388c829226f9ba1d7467ff2f6294048cc
```

Inputs:

```text
input  FLOAT [dynamic, dynamic]
state  FLOAT [2, dynamic, 128]
sr     INT64 scalar
```

Outputs:

```text
output FLOAT [dynamic, 1]
stateN FLOAT [dynamic, dynamic, dynamic]
```

Recursive inventory:

```text
graphs:                    51
maximum depth:             4
top-level nodes:           5
nested nodes:              684
total nodes:               689
graph initializer tensors: 0
attribute tensors:         345
encoded constant bytes:    2,186,503
```

Top-level operators:

```text
Constant
Equal
If
Identity
Identity
```

All five are one CPU partition. `Constant`, `Equal`, and `If` have intrinsic
operator/type blockers; `If` and both `Identity` outputs have dynamic-shape
blockers. All five also have compiler and preflight blockers.

The placement manifest enumerates top-level nodes. The normalized signature
cryptographically covers both graph-valued `If` branches, and the graph
inventory counts every recursively nested node. The unsupported `If` keeps
the entire control-flow boundary on CPU; nested nodes are not individually
advertised as NPU candidates.

No Silero inference ran. Therefore this evidence does not establish VAD
accuracy, state semantics, accepted frame lengths, sample-rate behavior,
latency, memory use, or Wave deployment compatibility.

## 15. Determinism procedure

Two fresh output directories were used:

```text
/tmp/wave-onnx-poc9ef.kySmAv
/tmp/wave-onnx-poc9ef-repeat.wuqJuX
```

Each pass independently produced:

- spectral audit;
- CPU output and execution evidence;
- hybrid output and execution evidence;
- Silero audit.

Every corresponding file compared byte-for-byte equal across the two passes.
Within each pass, CPU and hybrid output NPZ files also compared byte-for-byte
equal.

The tracked evidence was copied from the first verified pass only after the
repeat comparison succeeded.

Fixture regeneration separately reported:

```text
verified 3 models and 2 tensor sets
```

### 15.1 Corrected output-path mistake

During the earlier evidence-generation iteration, five generated ONNX files
were initially written one directory above the repository because a relative
path contained one extra `..`.

The exact five newly generated files were moved into `docs/npu/onnx/`; no
pre-existing file was overwritten. The resulting empty stray
`Projects/docs/npu/onnx`, `Projects/docs/npu`, and `Projects/docs`
directories were removed. No other file outside the repository was changed.

This was a host path correction, not a model, compiler, device, or runtime
event.

## 16. Validation

Focused tool checks:

```text
ruff check:         pass
ruff format check:  pass
pytest:             35 passed
```

Test coverage includes:

- deterministic fixtures;
- model, node, tensor, rank, dimension, and initializer bounds;
- top-level and nested external-data refusal;
- duplicate names and invalid ONNX;
- policy schema, path, hash, and authorization refusal;
- stable signatures and metadata exclusion;
- weight and topology sensitivity;
- dynamic-shape and unsupported-operator reasons;
- CPU, hybrid, strict, and injected-construction modes;
- one-attempt NPU construction;
- input and output type/shape/finite checks;
- numerical CPU oracle comparison;
- CPU/hybrid equality;
- exclusive output behavior;
- CLI generation, verification, audit, run, and strict refusal.

Existing NPU regression:

```text
198 tests passed
```

No test or evidence command contacted ADB, a Wave process, `/dev/galcore`, a
compiler, an NPU provider, or a device library.

## 17. Evidence classification

### Host-observed

- ONNX Runtime created only `CPUExecutionProvider`.
- CPU and hybrid each created one CPU session.
- Hybrid made zero NPU attempts.
- CPU and hybrid output archives were byte-identical.
- The raw output tensor agreed with the NumPy reference within `1e-6`.
- Two complete evidence passes were byte-identical.
- 35 focused and 198 existing NPU tests passed.

### Static verified

- The policy is hash-bound to the PoC 9C refusal manifest.
- The approval registry is empty.
- Model and recursive graph limits are encoded in policy and implementation.
- External data is rejected throughout the graph tree.
- The Silero model has the recorded source digest.
- Silero's normalized graph tree contains the recorded counts.
- All reported placement is CPU.

### Inferred

- The same audit/fallback schema can be retained when an exact NPU compiler
  becomes available; only a reviewed compiler binding, signature approval,
  partition compiler, and session implementation should need promotion.
- The fixed Conv/Sigmoid control is structurally relevant to mask-producing
  audio networks, but it says nothing about denoising quality.

### Negative findings

- General Wave NPU ONNX compilation remains unavailable.
- No ONNX operator is approved for Wave NPU execution.
- The available PoC 9C config grammar is still not qualified for a compatible
  graph-to-NBG compiler. PoC 9D later corroborated it in a version-below-gate
  shader compiler only.
- Silero's control flow and dynamic state are not NPU-eligible.
- Hybrid currently means a classified CPU preflight fallback, not mixed
  CPU/NPU execution.

### Unknown

- The missing no-kernel graph-to-NBG SDK's actual `VIV_GPU_FILE` consumer
  behavior. PoC 9D recovered the separate shader-compiler consumer.
- Acuity profile schema and version accepted by a compatible NBG compiler.
- Exact ONNX-to-VSINPU operator lowering.
- Quantization, layout, memory, alignment, and partition constraints.
- Whether a useful denoiser can meet real-time latency and quality on the
  Wave NPU.
- Production device packaging for this host tool and ONNX Runtime version.

## 18. Safety postconditions

| Invariant | Before | After | Pass |
|---|---|---|---|
| ADB contacted | no | no | yes |
| `/dev/galcore` contacted | no | no | yes |
| Wave compiler loaded | no | no | yes |
| NPU provider loaded | no | no | yes |
| Wave graph created | no | no | yes |
| Device filesystem changed | no | no | yes |
| Services or firmware changed | no | no | yes |
| Approval registry empty | yes | yes | yes |
| PoC 9C compiler refusal hash matches | yes | yes | yes |

## 19. Conclusion

The hypothesis passed.

Proven:

- general bounded ONNX CPU admission;
- deterministic executable graph identity;
- exact all-CPU capability reporting;
- deterministic CPU reference execution;
- fail-closed hybrid and strict behavior;
- recursive control-flow graph accounting;
- real-model audit at Silero scale;
- stable policy binding to the unresolved compiler gate.

Disproven:

- that a five-node top-level Silero report is a sufficient model inventory;
- that an operator-name allowlist is enough to claim NPU support;
- that hybrid mode must wait for the NPU compiler before its API and failure
  contract can be implemented.

Still unknown:

- the exact compiler/Acuity profile;
- which exact graph signatures can compile for the Wave;
- useful mixed CPU/NPU partition behavior;
- noise-cancellation quality, latency, and production integration.

## 20. Gate decision

```text
Current PoC gate:
  pass — host ONNX audit and CPU fallback

Newly allowed:
  audit bounded ONNX models on the host
  run admitted models through explicit CPU mode
  use current hybrid mode as a classified CPU fallback
  use strict mode to prove absence of an approved NPU partition
  compare later NPU results against the fixed CPU oracle

Still prohibited:
  approving an NPU graph signature
  invoking an unqualified compiler
  claiming Conv, Sigmoid, Silero, or a denoiser runs on the Wave NPU
  treating the simulator config as the missing compiler profile

Next positive NPU gate:
  acquire and hash-pin the no-kernel compiler consumer
  prove one exact fixed graph compiles for the PoC 9A Wave profile
  validate the untouched artifact
  execute it under a separately approved bounded device PoC
  compare it with this CPU oracle
```
