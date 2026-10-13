# 2026-10-13 — PoC 17: ONNX-derived native Conv and MatMul

## Record

| Field | Value |
|---|---|
| PoC | `17A1`, `17A2`, `17B1`, `17B2` |
| Experiment ID | `wave-npu-poc17-onnx-native` |
| Operator | REA controller with independent source/evidence review |
| Device alias | `wave-lab-unit-a` |
| First native start UTC | `2026-10-13T08:44:04.565Z` |
| Last native finish UTC | `2026-10-13T08:49:46.631Z` |
| Controller commit | `027e97f664432e89d1b530df9734f27f306489aa` |
| Worktree state | Reviewed native sources committed; documentation and host integration test additions in progress during the final controls |
| Frozen definition SHA-256 | `f9682686a3b6a4a12df21b202f737a7ee6065ea26e1f24f10ebe5b9732a45b99` |
| Outcome | **PASS: four NN exports and four byte-exact inferences** |

## Falsifiable hypothesis

Four real ONNX files, including changed Conv weights/bias and a non-symmetric
MatMul coefficient pattern, can be lowered from their actual initializers,
compiled on the Wave NN engine, and executed once each with all output bytes
matching ONNX Runtime CPU and an independent NumPy oracle, while captured
health remains unchanged and all owned staging files are removed.

PoC 16 proved the native dtype routes. This PoC connects ordinary ONNX
protobufs to those routes through an importer, generated native sources,
hash-bound package qualification and explicit session/CLI selection.

## Scope and authorization

The owner approved the design and implementation with `go`. The frozen
sequence allowed four compiler invocations, one size query plus one public
export per compiler (eight export API calls), and at most four inferences.
All eight native invocations completed successfully, with no retry. Native
calls stayed sequential and controller-owned.

Only disposable files under package-derived `/tmp/wave-npu-onnx17-*` roots
were staged. Services, firmware, drivers, system libraries and NPU settings
were unchanged. CLC remained excluded; the incompatible adjacent CLC was
not used. Compile alarms were 15 seconds native/20 seconds host; inference
alarms were 8/12 seconds. Native alarms stayed armed through teardown.
Timeout, fault, ambiguous exit or health change would stop remaining live
work without recovery actions.

## Identity and prerequisites

```text
kernel: Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux
/dev/galcore: character device 199:0
enableNN: 255
nanoqFreq: 838860800
recovery: 1
NBG: VPMN v2, product 0x1000000e
TIM-VX: 702715d714277376d7ff466b7b72833ff57564c1
runtime SHA-256: c984e063e9ab4fe4d7e7ac85e783d88a330f1560268924ba4f449f5b258478dc
runtime provenance SHA-256: 58601a21a3ac1ff0435833d5d30409a43332a892c77047428cb9a54d658efade
image: sha256:a45f1308adfe3fc3b90e329e72d5efe9d743e6cc77017254d28bbba1c689586a
```

The runtime provenance binds the exact SDK and TIM-VX input inventories.
Offline cross-builds used read-only source/SDK mounts and no network. ELF
checks verified AArch64, dependencies, symbol-version floors, protections
and the exact package runtime path before device contact.

The [tracked result ledger](../onnx/native-poc17-results.json) retains full
model, graph, lowering, package, compiler, executor, input, output, NBG,
manifest, CPU-evidence, run, stdout and cleanup hashes. The four ONNX/NPZ
fixtures and their deterministic manifest are tracked under
`waved/tools/onnx-hybrid/fixtures/native-poc17/`.

## Method

1. Admit only standard-domain opset-13 single-node fixed-shape Conv or
   QLinearMatMul, with inline immutable constants. Reject unsupported
   attributes, shapes, quantizers and hidden/overridable constants.
2. Preserve FP16 coefficient bits, convert optional FP16 Conv bias exactly
   to FP32, and transpose ONNX MatMul B `[16,4]` into contiguous native FC
   columns. Generate native source from these actual bytes.
3. Freeze all four model/package/compiler/input/oracle identities after
   host tests and independent source review passed.
4. Compile each once through TIM-VX; require explicit
   `VXNNE_OPERATION_TARGET_NN`, exact VPMN v2 tensors/target and valid CRCs.
   Validate and remove the compiler staging root before inference.
5. Compare ORT CPU and the independent oracle, then build the fixed NBG
   file runner. Compile/Run once, prime device output after Compile, retrieve
   guarded host readback, atomically retain actual output and teardown before
   PASS. Check every byte and remove execution staging.
6. Reload all four real qualified packages with CPU execution and ADB
   patched to fail. Mutate ten artifacts in copied 17A1 packages; require
   refusal before CPU construction or transport. No extra NPU call.
7. Capture final read-only health and all eight staging-root absences.

Exact commands, supported profiles and the future session interface are in
[the native backend guide](../onnx/native-backend.md). The four controls used
these run IDs:

| Control | Compile run ID | Inference run ID |
|---|---|---|
| 17A1 | `rea-poc17a1-onnx-compile-20261013-a1` | `rea-poc17a1-onnx-execute-20261013-a1` |
| 17A2 | `rea-poc17a2-onnx-compile-20261013-a1` | `rea-poc17a2-onnx-execute-20261013-a1` |
| 17B1 | `rea-poc17b1-onnx-compile-20261013-a1` | `rea-poc17b1-onnx-execute-20261013-a1` |
| 17B2 | `rea-poc17b2-onnx-compile-20261013-a1` | `rea-poc17b2-onnx-execute-20261013-a1` |

## Observations

| Control | ONNX parameters/input | NBG bytes | Exact native result |
|---|---|---:|---|
| 17A1 | FP16 rank-3 Conv `[.25,.5,.25]`, no bias, alternating ±1 | 1,800 | 32/32 bytes agree; endpoints +.25/−.25, interior zero |
| 17A2 | FP16 rank-4 Conv `[.5,.25,-.25]`, bias .125, input `(k-8)/8` | 1,800 | 32/32 bytes agree, including the bias and boundary padding |
| 17B1 | U8 MatMul selecting input indices 0,5,10,15; input 0..15 | 2,056 | `[0,5,10,15]`, all 4 bytes agree |
| 17B2 | U8 B[k,j]=1 iff `(k+2*j)%5==0`; input 0..15 | 2,056 | `[30,24,18,27]`, all 4 bytes agree |

All compile and inference transports returned remote exit zero without
timeouts. Every raw envelope reports PASS, empty errors/health differences/
kernel alerts and one inference iteration. All cleanup records report
CLEAN. Compiler logs select the NN engine; FC is optimized to a native
convolution by the vendor stack.

| Control | NBG SHA-256 | Returned output hex |
|---|---|---|
| 17A1 | `4e2b1df127c7f723faea050bbaff40db618eacd21e9efa1d18bff52151c7af4d` | `00340000000000000000000000000000000000000000000000000000000000b4` |
| 17A2 | `e22bbe15103b5cbe73cc75c3d3975ff278524ba387969a5e0ec5cd2d2dc1cfbf` | `002e80b680b580b400b300b100ae00a80028002e00310033803480358036c039` |
| 17B1 | `00521883a0e69d27f3ce2092f2cfff1932a208fad7bf2665648537d1f52a4a1e` | `00050a0f` |
| 17B2 | `8e034418a12761f3dc24ffd5efd632e1c3ba8108bee372a7ef73aa2e820884ee` | `1e18121b` |

17A1 and 17B1 reproduce the matching native PoC 16 NBGs. Changed ONNX
coefficients/bias in 17A2 and 17B2 produce different NBG hashes and different,
correct readbacks. The importer uses actual model parameters rather than
routing fixture hashes to canned binaries.

### Timing — descriptive

One first-run graph observation per control was recorded:
17A1 2068 µs, 17A2 1722 µs, 17B1 1730 µs, 17B2 1680 µs.
These single observations include no benchmark warm-up or statistics and
make no throughput or production latency claim.

## Host validation and review

- 108 affected bridge, compile, execution, dtype/staging and NBG-parser
  tests passed; 345 ONNX package tests passed. Lint/format checks passed.
- Two actual-package integrity integration tests passed: all four real strict sessions construct
  without CPU execution or transport, and ten changed artifacts refuse.
- Independent reviews closed cleanup-continuity, diagnostic-window,
  service-identity, output-canary, strict CPU-session and run-ID issues
  before the frozen launch. Final evidence review passed and is retained locally; its SHA-256 is
  recorded in the tracked result ledger.
- Strict native loading/execution computes the independent NumPy oracle
  without constructing an ORT CPU session. Actual ORT provider/version and
  model/input/output evidence is retained from explicit qualification and
  bound by its complete file hash.
- Existing default PoC 9 CPU/refusal behavior is preserved. Injected-session
  tests cover strict refusal, native success, unsupported CPU fallback,
  malformed readback and failed-execution latching without a retry.

The integration check is host-only:

```bash
WAVE_ONNX_QUALIFIED_PACKAGES=artifacts/npu/poc17/packages \
  waved/tools/onnx-hybrid/.venv/bin/python -m unittest discover \
  -s waved/scripts/npu/tests -p test_npu_onnx_qualified_integration.py -v
```

## Safety postconditions

| Invariant | Observed result |
|---|---|
| ADB and exact kernel | Responsive, unchanged |
| Boot identity | Unchanged |
| NPU settings | `255`, `838860800`, `1`, unchanged |
| `/dev/galcore` | Present, unchanged 199:0 |
| Required system hashes | All 17 unchanged |
| Service process identities | Unchanged |
| Captured galcore holders | Empty before/after |
| Kernel diagnostics | No new relevant alert; marker continuity retained |
| Native teardown | All four runners exited zero after teardown |
| Temporary roots | All eight absent |
| Native ONNX processes | None remaining |
| System/service/firmware changes | None |

The first cross-control final comparison included different package-specific
absence keys: A1's temporary `libCLC.so` path versus B2's. Its sole reported
difference was that key set, with both paths absent. The original snapshot
and comparison are retained. The validated comparison projects the same
three global CLC paths on both sides and independently verifies all eight
whole staging roots absent; all other health fields are compared unchanged.
This requires no new device operation and excludes no global health change.

## Raw evidence and retention

Raw artifacts remain ignored under:

```text
artifacts/npu/poc17/frozen-controls.json
artifacts/npu/poc17/packages/17{a1,a2,b1,b2}/
artifacts/npu/runs/rea-poc17*-onnx-{compile,execute}-20261013-a1/
artifacts/npu/poc17/final-health.json
artifacts/npu/poc17/final-health-validated.json
artifacts/npu/poc17/review-{1,2,3,4,5}.md
```

The tracked [sanitized ledger](../onnx/native-poc17-results.json) binds the
raw evidence without publishing serials, boot IDs, complete process lists
or device diagnostic dumps. Full packages remain required for actual native
eligibility; a summary ledger alone cannot authorize execution.

## Evidence classification and gate decision

**Observed live:** four NN NBG exports, four one-iteration byte-exact
readbacks, clean teardown/cleanup and unchanged captured health.

**Static verified:** ONNX parameter extraction/layout conversion, bounded
native lifecycle, model/source/build/NBG/evidence binding, VPMN checksums,
and strict/hybrid refusal/fallback semantics.

**Unqualified:** broader shapes/dtypes/operators, multi-node models,
arbitrary ONNX, full Silero/DeepFilterNet and production inference latency,
thermal or long-run stability. Native template/bridge resources require
an editable repository installation; standalone-wheel operation is not
implemented.

The PoC 17 gate passes for these four exact model packages. The explicit
backend is available for their qualified contracts; any new model requires
separate qualification. Production noise reduction remains parked on the
NPU under the earlier end-to-end cost decision.
