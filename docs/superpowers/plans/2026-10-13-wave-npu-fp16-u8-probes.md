# Wave NPU FP16 Conv and U8 FC implementation plan

> **For agentic workers:** Implement the approved fixed probe design task by
> task. Independent runtime construction and probe source work may proceed
> in parallel under REA's investigation workflow; live operations are sequential.

**Goal:** Compile the fixed FP16 Conv and U8 FC controls and execute each
valid native NBG once against its oracle.

**Architecture:** Separate fixed native compiler processes reuse the
existing `npu_compile_lab` gates. A rebuilt, hash-bound TIM-VX wrapper links
only to the exact device libraries. Host parsing precedes separate NBG
execution; failures remain raw evidence.

**Tech stack:** C++17, TIM-VX/OpenVX, Python standard-library unittest,
existing NBG parser, ADB, immutable Docker cross-toolchain image.

**Spec:** `docs/superpowers/specs/2026-10-13-wave-npu-fp16-u8-probes-design.md`

## Global constraints

- TIM-VX commit `702715d714277376d7ff466b7b72833ff57564c1` and exact Wave SDK.
- Separate roots `/tmp/wave-npu-compile16a` and `/tmp/wave-npu-compile16b`.
- One size-query and one export per candidate; four export calls maximum.
- Native timeout 15 seconds; host timeout 20000 ms; no retry or recovery.
- Logging `VIV_VX_DEBUG_LEVEL=1`, `VIV_VX_ENABLE_PRINT_TARGET=1`.
- Native VPMN v2 target `0x1000000e`; one oracle comparison per valid NBG.
- A health fault ends all live work. Preserve unrelated workspace edits.

## Review focus

- Wrong tensor dtype or shape: host NBG contract rejects before execution.
- Wrong runtime/model hash: preflight rejects before native calls.
- A changed environment or candidate: fixed policy refuses it.
- A compiler failure/timeout: preserve raw logs and run no inference.
- Existing staged output or galcore holder: refuse rather than overwrite/run.

### Task 1: Rebuild and identify the wrapper

**Files:** build artifacts and provenance under `artifacts/npu/build/poc16/`;
new build script under `waved/tools/npu-compiler/`.

**Interface:** produce an AArch64 `libtim-vx.so` linked against
`waved/third_party/device-sdk/lib`, its SHA-256, dependency list and pinned
container/source provenance. Later native builds consume that exact path.

- [x] Verify the source commit and SDK hashes, inspect the immutable local
  cross image for `cmake`, `ninja` and `aarch64-linux-gnu-g++`.
- [x] Build with `TIM_VX_USE_EXTERNAL_OVXLIB=ON`, NBG parser enabled and
  tests/examples disabled; no model/compiler execution occurs on the host.
- [x] Inspect ELF architecture, DT_NEEDED and required libc versions; save
  the actual wrapper digest and complete build log.

### Task 2: Implement fixed compiler and host policies

**Files:** `tim_vx_compile_fp16_conv.cc`, `tim_vx_compile_u8_fc.cc`, their
build/policy helpers under `waved/tools/npu-compiler/`; fixed entry module,
two manifests and tests under `waved/scripts/npu/`.

**Interface:** subclass `CompilePolicy.expected_environment` to add only
the two fixed diagnostic variables. `POLICIES["fp16-conv"]` and
`POLICIES["u8-fc"]` each supply one `candidate-1`. Reuse
`load_manifest(policy=...)` and `run_candidate(...)` unchanged.

- [x] Write refusal tests first, including this fixed environment guard:

  ```python
  data = json.loads(policy.manifest_path.read_text())
  data["environment"]["VIV_VX_ENABLE_PRINT_TARGET"] = "0"
  with self.assertRaises(npu_compile_lab.ManifestError):
      npu_compile_lab.load_manifest(changed_path, policy=policy)
  ```

  Also test extra candidate rejection, wrong tensor metadata, bad checksums,
  and independent oracles `fp16_fir_oracle()` / `u8_fc_oracle()` against the
  spec's bytes. Run the focused unittest module and observe the missing
  implementation failure before adding the policies/oracles.
- [x] Adapt the existing Conv compiler lifecycle to FP16 input/weights/
  output and FP32 bias. Build FC with the specified sparse weight columns
  and quantized INT32 bias. Preserve fixed argument/path/identity/alarm/
  atomic-output behavior; no Run, NBG import or tensor readback API.
- [x] Build with `-Wall -Wextra -Werror`, inspect native imports and source
  policy, and write exact hashes into the two manifests.
- [x] Run focused new tests and the existing shared compile-harness tests.

### Task 3: Compile, qualify and execute

**Files:** fixed staging/cleanup helper; conditional NBG execution runner
and manifest; raw runs under `artifacts/npu/runs/rea-poc16*`; new notebook
under `docs/npu/lab-notebook/` and existing NPU status references.

**Interface:** each compile envelope provides a validated artifact path
and SHA-256, or a recorded clean negative. Only validated artifacts feed
the one-shot execution runner and fixed oracle.

- [x] Stage only absent fixed files; verify their hashes/modes. Capture
  preflight, compile FP16 Conv once and inspect raw result/postflight.
- [x] If health is unchanged, compile the independent U8 FC once and
  inspect raw result/postflight. Do not rerun rejected candidates.
- [x] For each exported NBG, validate target, dimensions, dtype, size and
  section CRCs with `nbg_format.parse_nbg`. Build/hash-bind a fixed NBG
  runner and initialize output to a canary. Execute once with its frozen
  input and compare every returned byte with the independent oracle.
- [x] Preserve envelopes and raw logs, clean only known staged artifacts,
  verify final state, update the notebook and supported conclusions.
- [x] Have an independent reviewer inspect probe and host boundaries;
  fix meaningful issues and rerun the affected host checks before completion.

## Execution record

Owner approved the full probe chain with `go`; no repeated approval is
needed for its already-described compile and qualified execution steps.
The design and implementation choices are reviewable in these files.

Completed 2026-10-13. Both models exported native VPMN v2 NBGs for
`0x1000000e` with explicit NN target logs, then executed exactly once with
all 32 FP16 or 4 U8 output bytes matching the independent CPU oracles.
All four staging roots were removed; captured boot, parameters, system
hashes, service identities and health remained unchanged. Implementation:
`e541da012a266f3f3670f31931749793e08bacb0`. Full evidence is in
[PoC 16](../../npu/lab-notebook/2026-10-13-poc16-native-fp16-conv-u8-fc.md).

Implementation adjustments supported by inspection and review:

- The pinned toolchain supplied Make and no Ninja; the runtime build used
  Unix Makefiles with source/SDK read-only and networking disabled.
- The shared compile harness required a small change to accept each fixed
  policy's diagnostic environment. The execution harness was generalized
  from four-byte output to the policy's exact output length, preserving old
  Erf behavior, and checks PoC 16 service identities during execution.
- Review tightened both offline runners to seed device output after Compile
  and retain the alarm through teardown before any live execution.
- Cleanup acquired a tested bounded diagnostic-window fallback after busy
  Wi-Fi logs rotated the short baseline window; it still rejects missing
  markers or new faults. Refused cleanup attempts performed no deletion or
  native run.
- Independent native/host boundary review passed after corrections; the
  relevant 94 host tests and both native policy verifiers passed.
