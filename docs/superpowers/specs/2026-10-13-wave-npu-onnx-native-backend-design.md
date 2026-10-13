# Wave NPU ONNX native backend

**State:** Approved by the owner with `go` on 2026-10-13. Implementation and
the four bounded device qualification controls below are authorized. The
ONNX backend is not implemented or qualified merely by this document.

## Goal

Accept ONNX files, extract their actual parameters, lower supported static
graphs to the exact Wave TIM-VX stack, export native NBGs and run them with
validated ONNX-shaped inputs and outputs. Unsupported graphs retain explicit
CPU fallback; strict NPU mode reports the exact unsupported boundary.

The first milestone covers two operator families and the shapes already
demonstrated by [PoC 16](../../npu/lab-notebook/2026-10-13-poc16-native-fp16-conv-u8-fc.md).
It must import weights from the supplied ONNX file rather than recognize a
fixture and silently dispatch to a canned model. Larger shapes, multi-layer
networks and a full ORT execution provider are subsequent milestones.

## Existing evidence and alternatives

- `waved/tools/onnx-hybrid` already provides bounded ONNX admission, graph
  signatures, CPU inference, tensor validation and deterministic evidence.
  Its default policy intentionally has no qualified NPU signatures.
- PoC 16 exported and executed FP16 FIR Conv and asymmetric-U8 FC through
  TIM-VX commit `702715d714277376d7ff466b7b72833ff57564c1` and exact device
  libraries. The rebuilt wrapper's SHA-256 is
  `c984e063e9ab4fe4d7e7ac85e783d88a330f1560268924ba4f449f5b258478dc`.
- The local ORT v1.22 VSI-NPU `qlinearmatmul_op_builder.h` creates
  `tim::vx::ops::Matmul`, which does not inherit the dedicated FC proof.
  Its build script also needs an absent TIM-VX installation and a new ORT
  build/dependency qualification. Preserve the locally modified upstream
  checkout; do not use that script as the initial backend.

Three approaches were considered:

| Approach | Benefit | Cost / boundary |
|---|---|---|
| **ONNX → small native lowering → NBG** | Reuses the working runtime; makes layout, constants and qualification explicit | Implements a restricted operator/shape subset first |
| Full ORT VSI-NPU provider build | Existing general frontend and graph partition machinery | New cross-build/runtime qualification; stock MatMul lowering needs changes; provider registration alone does not prove NN residency |
| ONNX signature → existing PoC 16 NBG registry | Fast verification of equivalent wrappers | Supports only pre-existing weights and cannot establish actual parameter import |

Use the first approach. Keep the registry as a qualification ledger for
newly imported and compiled artifacts, never as a substitute for lowering.

## First supported import profiles

Use the standard ONNX domain, model IR supported by the pinned host ONNX
stack, and opset 13. Both profiles have one compute node, one external
input and one output. All parameters and quantizers are inline initializers.
Reject unknown attributes, overridable initializers, duplicate names,
unsupported domains, hidden graphs/functions, external tensor data and
non-finite values. Resource limits remain enforced by the existing auditor.

### FP16 Conv

- NCW input/output `[1,1,16]`, weights `[1,1,3]`; or NCHW input/output
  `[1,1,1,16]`, weights `[1,1,1,3]`.
- Input, weights, output and optional ONNX bias are FLOAT16. Bias is `[1]`
  and may be absent, in which case insert zero.
- Group 1; stride 1; ONNX dilation 1; padding `SAME_UPPER`, or equivalent
  explicit symmetric padding. Reject conflicting `pads`/`auto_pad`.
- Decode coefficient bits from ONNX and preserve them exactly. Convert
  the finite constant FP16 bias to exact FP32 for the native Conv wrapper.
- Normalize both ONNX ranks to the proven native Conv2d shapes
  `[16,1,1,1]` and `[3,1,1,1]`. This profile has one channel and batch 1,
  so row-major ONNX payload order is preserved by the shape conversion.
- Map ONNX's undilated kernel to the proven native `[0,0]` convention.
  The pinned `vsi_nn_util.c` normalizes both zero dilation components to 1
  for padding calculations; PoC 16 independently proves this native graph.

The normative padding, type and bias contract comes from
[ONNX Conv v11](https://onnx.ai/onnx/operators/onnx__Conv.html#conv-11),
which is the applicable Conv definition for opset 13.

### U8 QLinearMatMul with constant right operand

- Input `A` is UINT8 `[1,16]`; initializer `B` is UINT8 `[16,4]`;
  output is UINT8 `[1,4]`.
- Input, weight and output quantizers are scalar FLOAT scale `1.0` and
  scalar UINT8 zero point `0`. Reject vectors, other scales/zero points,
  dynamic quantizers and signed types in this first profile.
- Lower to `FullyConnected(axis=0, units=4)`, with native input `[16,1]`,
  output `[4,1]` and INT32 zero bias of scale `1`.
- Repack ONNX row-major `B[k,j]` into contiguous native output columns:
  byte offset `j*16+k`. Merely reversing the dimensions is insufficient.
- Accumulation is bounded for 16 UINT8 products. The CPU oracle applies
  the ONNX saturation rule; initial live controls avoid saturation.

This is a specific lowering of a constant, rank-2 matrix product; it is not
approval for dynamic MatMul or BatchGemm. The normative behavior comes from
[ONNX QLinearMatMul v10](https://onnx.ai/onnx/operators/onnx__QLinearMatMul.html#qlinearmatmul-10),
the applicable definition for opset 13.

## Architecture and artifacts

### Host importer

A focused module in `wave_onnx_hybrid` reuses admission and shape inference,
then produces an immutable lowering record: source-model SHA, executable
graph signature, importer/profile version, ONNX/native tensor contracts,
normalized attributes, coefficient bytes/digests and bias conversion.

The importer has no device calls. An importable graph is a compile candidate;
it becomes runnable only after an exact NBG qualification record exists.
Human-readable model/node names are never inserted into generated C++ or
shell text. Generated source uses only validated numeric bit patterns,
fixed templates and generated identifiers.

### Native compile package

Generate a fixed native constructor from the lowering record and immutable
templates derived from PoC 16. Cross-build in the pinned offline image,
link to the exact wrapper/device SDK, inspect ELF dependencies and retain
source/build hashes. Each package has fixed paths, one argument, one model,
one size-query and one export, with the existing bounded lifecycle.

The package ledger binds ONNX model/signature, lowering record, generated
source, compiler ELF, wrapper, environment and compile manifest. A weight
change must alter the lowering/source/package identities and must not reuse
an old qualified artifact.

### Qualification and session adapter

Validate the exported NBG using `nbg_format`: native VPMN v2, target
`0x1000000e`, exact native I/O dimensions/types/quantization and all CRCs.
Retain explicit NN target selection from compilation and the bounded
numerical/health result. A qualification ledger binds all evidence to the
exact source graph and artifact hash.

Provide a session adapter compatible with the existing `RunnableSession`
interface. It converts only the documented ONNX/native shapes, preserves
payload dtype and validates the returned bytes before rebuilding the ONNX
array. Strict mode never creates a CPU session on refusal. Hybrid mode
falls back to one CPU-only session when the graph lacks qualification; a
runtime fault latches CPU fallback without another NPU attempt.

Keep the PoC 9 default refusal policy and its historic evidence intact.
Add a separate native-backend policy/ledger; explicit backend selection
uses it. Eligibility requires a supported import profile **and** exact
qualified model/NBG evidence. An operator-name allowlist cannot promote
unqualified models. Audit output distinguishes importable, compiled,
qualified and CPU-only states.

The existing `_node_reasons` rejects every quantization operator and
`execute_cpu_model` deliberately passes no NPU factory. Add native-profile
eligibility and a separate backend execution entry point; do not try to
enable this route merely by editing the old signature list. Reuse shared
admission/tensor/CPU primitives while retaining their original defaults.

### Input and output boundary

The reusable runner receives a host-validated, hash-bound input payload at
a fixed path. Read only an exact-size regular file. Import the fixed NBG,
compile once, seed device output with `0xa5`, run once and read back into
`0xa5` host storage with `0x5a` guards. Save only an exact-size output to a
new regular file; the host verifies every byte against its current oracle
during qualification. Model, input, runner and output are all in the run
record. This supports different input tensors without recompiling weights.

The public command boundary extends the existing package with separate
host-only import/build operations and explicit native compile/run
operations. CPU mode remains usable for all ordinarily admitted ONNX files.
No parsing or compiler setup occurs inside the `waved` service.

Proposed CLI surface:

```text
wave-onnx-hybrid import-native MODEL --package-dir NEW_DIR
wave-onnx-hybrid build-native PACKAGE_DIR
wave-onnx-hybrid compile-native PACKAGE_DIR --run-id NEW_ID
wave-onnx-hybrid qualify-native PACKAGE_DIR --inputs INPUTS.npz --run-id NEW_ID
wave-onnx-hybrid run MODEL --backend wave-native --package PACKAGE_DIR \
  --inputs INPUTS.npz --mode strict|hybrid --outputs OUTPUTS.npz \
  --evidence EVIDENCE.json --run-id NEW_ID
```

Import/build never contact the device. Compile/qualify own stage, preflight,
bounded native operation, evidence capture and admitted cleanup. Qualification
uses the frozen oracle and promotes the exact package only on numerical and
health success. Future `run` calls do not recompile the graph; they load its
qualified NBG and transport the validated input. This task invokes only the
four qualification executions below. Ordinary CPU run syntax is unchanged.

## Four frozen live qualification controls

Generate deterministic ONNX/NPZ fixtures with the pinned host dependencies,
then record their actual hashes before any device call:

| Control | ONNX parameters | Frozen input | What it checks |
|---|---|---|---|
| 17A1 | Rank-3 Conv `[0.25,0.5,0.25]`, absent bias | Alternating `+1,-1`, FLOAT16 | Import/layout equivalent to the proven FIR |
| 17A2 | Rank-4 Conv `[0.5,0.25,-0.25]`, FLOAT16 bias `0.125` | `x[k]=(k-8)/8`, FLOAT16 | Actual coefficient import, tap order, bias and alternate ONNX rank |
| 17B1 | QLinearMatMul; `B[k,j]=1` only at `k=[0,5,10,15][j]` | UINT8 `0..15` | Constant-matrix → FC weight layout |
| 17B2 | QLinearMatMul; `B[k,j]=1` when `(k+2*j)%5==0`, else `0` | UINT8 `0..15` | Changed, non-symmetric weight data and independent columns |

Compute expected tensors through ONNX Runtime CPU and an independent NumPy
oracle. Require their agreement before staging. FP16 controls use exact
dyadic values; all U8 control accumulations are below 255. Compare every
returned byte. Do not accept a tolerance to hide a layout/import error.

Each control gets one compile invocation (one size-query and one export)
and at most one qualified execution. Maximum: eight export API calls and
four inference calls. Clean compiler rejection is retained without retry.
A native fault, ambiguous remote exit, timeout or health change ends all
remaining live work. Subsequent input/shape/operator qualification is outside
this four-control live budget.

## Device invariants

Reuse the exact Wave kernel, identity and system hashes from PoC 16. Compile
and execution stay in new disposable PoC 17 roots, one control at a time.
All staged artifacts have verified hashes/modes; no existing path is
overwritten. Keep native/host compile deadlines at 15/20 seconds and
execution deadlines at 8/12 seconds. Keep native alarms armed through
teardown, parent-death handling, remote exit sentinels, holder scans,
service-identity checks and the tested bounded diagnostic-window fallback.

CLC stays excluded. Do not stop services, change sysfs or firmware, replace
device libraries/drivers, replay ioctls or recover a fault. Remove only
hash-verified owned files after successful cleanup admission, and retain
raw logs/NBGs/envelopes locally.

## Verification and completion

- Host importer tests prove exact tensor/attribute rules, NCW/NCHW shape
  normalization, FC coefficient transpose, FP16 bias conversion and
  fingerprint changes from altered parameters.
- Refusal tests cover invalid/external/nested/dynamic models, unknown
  attributes, wrong opset/types/shapes/quantizers and missing qualification
  before any ADB/native call.
- Session tests cover model/NBG/input hash mismatch, malformed output,
  canary/guard failures, strict refusal and one-attempt latched fallback.
- Existing ONNX CPU/hybrid tests and affected native harness/parser tests
  remain green. Run package lint/format checks and native source/ELF policy
  checks before live work.
- Live results must identify the source ONNX and extracted parameter hashes,
  prove NN export and exact CPU-oracle outputs, and leave captured health
  unchanged. Failed controls are documented as unsupported.
- Document actual operator/shape coverage and reproduction commands in the
  ONNX guide, a PoC 17 notebook and durable NPU status. Do not claim Silero,
  DeepFilterNet, full-model residency or performance from these controls.

Completion means ONNX files for the qualified profiles reach a real native
backend, changed initializer data drives newly compiled artifacts, exact
input/output shape contracts are honored, and other admitted graphs retain
explicit CPU behavior. It does not mean arbitrary ONNX runs on the NPU.
