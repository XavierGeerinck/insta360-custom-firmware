# Wave ONNX native backend

The explicit `wave-native` backend imports ONNX initializers, compiles them
through the exact Wave TIM-VX/OpenVX stack, and runs the resulting NBG on
the NN engine. A package is eligible only after its exact model, compiler,
NBG and execution have passed device qualification. The default PoC 9
backend still uses its existing CPU/refusal policy.

PoC 17 qualified four real ONNX controls on 2026-10-13:

| Control | Operator/variation | Native output agreement |
|---|---|---|
| 17A1 | FP16 rank-3 Conv, no bias | 32/32 bytes |
| 17A2 | FP16 rank-4 Conv, changed weights and bias | 32/32 bytes |
| 17B1 | U8 QLinearMatMul, sparse index selection | 4/4 bytes |
| 17B2 | U8 QLinearMatMul, non-symmetric changed weights | 4/4 bytes |

Each compiled once on the NN engine and ran once. Both CPU references
agreed with actual native readback. All eight temporary roots were removed
and captured device health stayed unchanged. Full hashes and receipts are
in the [result ledger](native-poc17-results.json) and
[PoC 17 notebook](../lab-notebook/2026-10-13-poc17-onnx-native-backend.md).

## Supported import profiles

Both profiles require ONNX opset 13, the standard domain, one compute node,
inline immutable initializers and fixed input/output shapes.

| ONNX operator | ONNX contract | Native lowering |
|---|---|---|
| `Conv` | FP16 NCW `[1,1,16]`, weights `[1,1,3]`, or NCHW `[1,1,1,16]`, weights `[1,1,1,3]`; same output shape | FP16 Conv2d input/output `[16,1,1,1]`, weights `[3,1,1,1]` |
| `QLinearMatMul` | U8 A `[1,16]`, constant B `[16,4]`, Y `[1,4]`; all scalar F32 scales `1.0`, U8 zero points `0` | U8 FullyConnected `16→4`, B transposed into contiguous native output columns, zero INT32 bias |

Conv requires group 1, unit strides and dilations, and `SAME_UPPER` or
equivalent symmetric explicit padding. An optional FP16 bias `[1]` is
converted exactly to native FP32. Imported half coefficients retain their
bits. Other attributes, dtypes, quantizers, dynamic shapes, external or
overridable constants and multi-node graphs refuse native admission.

Importable does not mean qualified. A different supported model needs a
new package and its own bounded qualification. Full Silero, DeepFilterNet,
arbitrary Conv/MatMul shapes and general ONNX graphs remain outside this
backend's qualified coverage.

## Repository setup

From the repository root:

```bash
cd waved/tools/onnx-hybrid
uv sync --python 3.12 --dev
cd ../../..
```

Native commands use this editable repository installation, including
`waved/scripts/npu/` and `waved/tools/npu-compiler/onnx-native/`. These
resources are not bundled in a standalone wheel. The exact device SDK,
runtime/provenance under `artifacts/npu/build/poc16/`, clean pinned TIM-VX
checkout, running Docker and the pinned offline toolchain image must be
available. They are checked before building or staging. Device commands
also require the exact lab Wave to be connected through ADB.

| Build identity | Value |
|---|---|
| TIM-VX | `702715d714277376d7ff466b7b72833ff57564c1` |
| Runtime SHA-256 | `c984e063e9ab4fe4d7e7ac85e783d88a330f1560268924ba4f449f5b258478dc` |
| Runtime provenance SHA-256 | `58601a21a3ac1ff0435833d5d30409a43332a892c77047428cb9a54d658efade` |
| Docker image | `sha256:a45f1308adfe3fc3b90e329e72d5efe9d743e6cc77017254d28bbba1c689586a` |

Cross-builds use no network and read-only source/SDK mounts. The host checks
the AArch64 ELF, dependencies, symbol versions, protections and exact
runtime path. No vendor binary executes on the Mac, and no host Python
dependency is installed on the Wave.

## Package lifecycle

The following shows the commands used for 17A1. The retained package is
already qualified; existing package, run and output paths are exclusive.
Do not rerun its compile or qualification. For a separately planned
qualification, choose a new package and unique run IDs.

```bash
waved/tools/onnx-hybrid/.venv/bin/python -m wave_onnx_hybrid import-native \
  waved/tools/onnx-hybrid/fixtures/native-poc17/17a1.onnx \
  --package-dir artifacts/npu/poc17/packages/17a1

waved/tools/onnx-hybrid/.venv/bin/python -m wave_onnx_hybrid build-native \
  artifacts/npu/poc17/packages/17a1

waved/tools/onnx-hybrid/.venv/bin/python -m wave_onnx_hybrid compile-native \
  artifacts/npu/poc17/packages/17a1 \
  --run-id rea-poc17a1-onnx-compile-20261013-a1

waved/tools/onnx-hybrid/.venv/bin/python -m wave_onnx_hybrid qualify-native \
  artifacts/npu/poc17/packages/17a1 \
  --inputs waved/tools/onnx-hybrid/fixtures/native-poc17/17a1_input.npz \
  --run-id rea-poc17a1-onnx-execute-20261013-a1
```

Import and build are host-only. Compile stages a generated compiler,
exports once, validates VPMN v2 target `0x1000000e`, tensor contracts and
all CRCs, requires explicit NN selection, then removes its staging root.
Qualification compares ONNX Runtime CPU with an independent NumPy oracle,
builds the fixed file runner, executes once, checks every returned byte,
and removes the execution root. The retained package includes source/build
provenance, the NBG, actual CPU evidence, output readback and raw run/health/
cleanup evidence. Successful lifecycle commands return exit zero; their
receipts are `compile-build.json`, `compiled.json` and `qualification.json`.

Read-only inspection creates no inference session or device call:

```bash
waved/tools/onnx-hybrid/.venv/bin/python waved/scripts/npu/npu_onnx_native_lab.py \
  inspect artifacts/npu/poc17/packages/17a1
```

## Inference interface

For a future execution of an already qualified package, use a new run ID
and new output paths:

```bash
waved/tools/onnx-hybrid/.venv/bin/python -m wave_onnx_hybrid run \
  waved/tools/onnx-hybrid/fixtures/native-poc17/17a1.onnx \
  --backend wave-native --package artifacts/npu/poc17/packages/17a1 \
  --run-id onnx17a1-new-input-001 \
  --inputs waved/tools/onnx-hybrid/fixtures/native-poc17/17a1_input.npz \
  --mode strict \
  --outputs /tmp/wave-onnx17a1-output.npz \
  --evidence /tmp/wave-onnx17a1-run.json
```

This command performs an additional device inference; it was not run as
part of the four-control qualification. Run IDs contain 1–64 lowercase
letters, digits or hyphens and start with a letter or digit. Inputs require
exact names, dtype, shape, contiguous storage and finite values. An oracle
output that is non-finite or equals the entire output canary refuses before
device contact.

Strict mode creates no CPU session and never falls back. Hybrid mode uses
CPU when admission or qualification fails; an execution failure latches CPU
fallback for that session with no second native dispatch. Evidence reports
the actual provider, dispatch count, confirmed native calls, CPU sessions,
fallback reason and model/package/NBG/input/output identities. A failed
dispatch reports an unknown confirmed-call count rather than claiming that
no hardware call occurred. This is a repository session adapter, not an
ONNX Runtime execution provider.

Every execution reloads qualification evidence and computes the independent
oracle for the supplied input without creating an ORT CPU session. It stages
only disposable files, uses one native Compile/Run, primes the device output
after Compile, checks guarded host readback, and keeps the native alarm
armed through teardown. Compile deadlines are 15 seconds native/20 seconds
host; inference deadlines are 8/12 seconds. Cleanup requires unchanged
system hashes, boot, service identities, holder state, NPU settings and
kernel diagnostics. No services, drivers, firmware or system libraries are
changed. Faults stop the live sequence without retries or recovery actions.
