# Wave ONNX Native Backend Implementation Plan

> **For agentic workers:** Implement this approved plan task by task with
> independent review. The owner approved execution with `go`; do not repeat
> the design approval or ask for an execution-method choice. The controller
> owns device operations. Source work with disjoint files may proceed in
> parallel under the REA investigation workflow's independent-scope rule.

**Goal:** Import real ONNX parameters, compile the two native profiles and
execute four exact ONNX controls against CPU oracles.

**Architecture:** Host admission lowers ONNX into a deterministic record;
fixed native source templates compile its extracted constants through the
proven TIM-VX wrapper. Exact package/NBG/run evidence controls eligibility
of a file-oriented session adapter; unsupported models retain CPU fallback.

**Tech Stack:** Python 3.12, pinned ONNX/ORT/NumPy, C++17 TIM-VX, immutable
offline AArch64 Docker toolchain, existing NBG parser and ADB lab controls.

**Spec:** `docs/superpowers/specs/2026-10-13-wave-npu-onnx-native-backend-design.md`

## Global Constraints

- TIM-VX `702715d714277376d7ff466b7b72833ff57564c1`; wrapper
  `c984e063e9ab4fe4d7e7ac85e783d88a330f1560268924ba4f449f5b258478dc`.
- Image `sha256:a45f1308adfe3fc3b90e329e72d5efe9d743e6cc77017254d28bbba1c689586a`;
  offline build, read-only source/SDK, no third-party source mutation.
- ONNX opset 13; only the exact single-node Conv and QLinearMatMul profiles
  in the spec. Importable is distinct from device-qualified.
- Four frozen controls; one size query + one export each, eight export API
  calls maximum; at most one inference per valid control, four total.
- Native/host compile deadlines 15/20 seconds; inference 8/12 seconds.
- VPMN v2, target `0x1000000e`, exact tensor contract and all CRCs required.
- CLC excluded; no service stops, sysfs/driver/library/firmware changes,
  recovery actions or retries. Any native fault stops remaining live work.
- Keep the historic default ONNX CPU/refusal policy intact. New backend
  selection must be explicit and hash-bound to a qualified package.

## Review Focus

- Unknown attributes/opset/domain, overridable or hidden constants must
  refuse before any device call.
- Imported FC coefficients must be transposed from `[K,N]` row-major to
  contiguous native output columns; non-symmetric controls catch mistakes.
- Changing model/constants/source/build/NBG/input must invalidate stale
  package evidence before staging.
- A session's failed execution must latch CPU fallback and make no second
  NPU call; strict mode must make no CPU session on failure.
- Cleanup must validate owned files, remote exit, diagnostics and service
  identities, with alarms active through native teardown.

## Task 1: ONNX lowering, fixtures and independent CPU oracle

**Files:** create `waved/tools/onnx-hybrid/src/wave_onnx_hybrid/native_import.py`,
`native_fixtures.py`, `tests/test_native_import.py`, and deterministic controls
under `fixtures/native-poc17/`.

**Interfaces:** `import_native_model(path: Path) -> dict` returns a record
with `schema_version`, `profile_id` (`fp16-conv-v1` or `u8-fc-v1`),
`source_model_sha256`, `graph_signature`, `onnx_input`, `onnx_output`,
`native_input`, `native_output`, `weights`, `bias`, `operation`, and
`lowering_sha256`. Tensor records use `name` where applicable, `dtype`
(`FLOAT16`, `UINT8`, `FLOAT32`, `INT32`), `shape` and `byte_size`.
Constants add `payload_hex` and `sha256`. `lowering_sha256` hashes canonical
JSON excluding itself. `independent_oracle(record, inputs: dict) -> dict`
returns arrays in the ONNX output contract. `generate_native_fixtures(path)`
writes four controls from the spec and a deterministic manifest.

- [ ] Write the layout and changed-weight tests before implementation:

  ```python
  record = import_native_model(fixtures / "17b2.onnx")
  weights = np.frombuffer(bytes.fromhex(record["weights"]["payload_hex"]),
                          dtype=np.uint8).reshape(4, 16)
  expected = np.array([[int((k + 2*j) % 5 == 0) for k in range(16)]
                       for j in range(4)], dtype=np.uint8)
  np.testing.assert_array_equal(weights, expected)
  ```

  Add refusal cases for dynamic shapes, wrong dtypes/quantizers, unknown
  attributes, overridable/external/nested constants, opsets/domains and
  changed model signature. Observe the missing-module failure.
- [ ] Reuse the existing auditor; validate exact profile contracts and
  convert/repack actual initializer bytes. Preserve half bits and exact
  FLOAT16→FLOAT32 bias conversion; emit immutable canonical identities.
- [ ] Generate all four fixtures with IR 10/opset 13 and compare ORT CPU
  outputs to independent NumPy outputs bit for bit. Track fixture hashes.
- [ ] Run focused tests and affected existing ONNX tests, lint/format,
  record results and commit only owned files for independent review.

## Task 2: Generated native package and bounded build

**Files:** create `native_sources.py` and tests in the ONNX package;
native runner template under `waved/tools/npu-compiler/`; package/lab bridge
`waved/scripts/npu/npu_onnx_native_lab.py` and its tests.

**Interfaces:** `generate_compile_source(record, remote_root) -> str`
and `generate_execute_source(record, remote_root, nbg_bytes) -> str` emit
fixed constructor/runner sources. The bridge exposes `import_package`,
`build_package`, `compile_package`, `qualify_package`, `load_package` and
`execute_package` to Task 3. Package data retains the supplied ONNX bytes
and lowering record. `execute_package` returns a dict containing raw output
bytes and a run envelope; it never runs an unqualified NBG.

- [ ] Write host refusal tests for modified constants/source/hash, unsafe
  remote paths, extra native calls, early alarm cancellation and output
  canary ordering. Use the existing source-policy testing pattern.
- [ ] Generate compiler constants from numeric byte patterns only. Reuse
  the PoC 16 identity/env/atomic export/alarm lifecycle, with package-derived
  fixed roots and metadata instead of copied fixture constants.
- [ ] Implement reusable input-file runners: exact regular-file length,
  fixed NBG size/v2/target, one Compile/Run, device canary, guarded readback,
  atomic exclusive output, teardown before PASS and no alarm cancellation.
- [ ] Cross-build in the pinned image with `--network=none`; inspect
  AArch64 ELF/dependencies/version floors and retain full build provenance.
  No vendor compiler/model binary executes on the Mac.
- [ ] Use subclassed existing compile policies, exact system hashes and
  package pins. Stage only new files, capture health and explicit NN target
  logs, qualify NBG metadata/CRCs, and clean exact owned inventories.
- [ ] Run host mocked stage/refusal/health tests plus affected lab/parser
  controls, and independently review before any live native operation.

## Task 3: Explicit backend session, policy and CLI

**Files:** create `native_backend.py`, `native_cli.py` and focused tests in
`wave_onnx_hybrid`; modify existing `cli.py` only to expose new commands and
optional explicit backend selection.

**Interfaces:** native eligibility combines Task 1 profile/signature with
Task 2's validated qualification ledger. A `NativeSession` implements
`run(inputs)` and `providers`, using an injected device call for tests.
Actual calls invoke the bridge's `execute_package`, with exact package/model
and input evidence. Retain CPU defaults and deterministic output NPZ.

- [ ] Write tests for unqualified/changed packages before native calls,
  unsupported models returning CPU fallback, strict refusal without CPU,
  malformed outputs and failed-run latching:

  ```python
  session = NativeSession(..., executor=failing_executor)
  first = session.run(inputs)
  second = session.run(inputs)
  assert native_calls == 1
  np.testing.assert_array_equal(first[output_name], cpu_output)
  np.testing.assert_array_equal(second[output_name], cpu_output)
  ```

- [ ] Add `import-native`, `build-native`, `compile-native`, `qualify-native`
  commands and `run --backend wave-native --package ... --run-id ...`.
  Run/input/output paths are exclusive and validated before device contact.
- [ ] Return actual backend/call/fallback/qualification evidence; never
  label CPU outputs as NPU. Native strict success requires qualified NBG
  hashes and exact output contract; hybrid failure makes no NPU retry.
- [ ] Run native backend/CLI tests and existing ONNX CPU/hybrid regression,
  lint/format, commit and independently review.

## Task 4: Four live controls and durable evidence

**Files:** raw artifacts under `artifacts/npu/poc17/` and `runs/`; new
PoC 17 notebook; ONNX README, package README and durable NPU status updates.

- [ ] Freeze model, package, binary, input and CPU-oracle hashes for each
  of 17A1/17A2/17B1/17B2. All host/source/build reviews pass first.
- [ ] Sequentially compile each once; validate exact native NBG and NN
  target logs; execute each qualifying artifact once against its oracle.
  Record any clean negative; do not retry a candidate or continue on a fault.
- [ ] Clean each staging root promptly and verify final hashes, boot,
  NPU settings, service identities, holder state and diagnostic markers.
- [ ] Record actual operator/shape coverage and exact reproduction commands,
  limitations, failures, model/build/NBG/output digests and test evidence.
- [ ] Complete final independent source/evidence review; commit owned
  changes without push, merge, service deployment or unrelated edits.

## Execution record

Owner approved the design, implementation and bounded live sequence with
`go`. Work stays on the existing `feat/m3` feature workspace so ignored
exact-device SDK/runtime evidence remains available. Preserve other edits.
Progress and reviews are recorded under `artifacts/npu/poc17/progress.md`.
