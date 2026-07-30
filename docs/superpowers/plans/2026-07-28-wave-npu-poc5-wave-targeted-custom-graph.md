# Wave NPU PoC 5 — Wave-Targeted Custom-Graph Compiler Plan

**Date:** 2026-07-28
**Status:** Closed — host simulator negative; exact-device sub-PoCs followed
**Depends on:** PoC 0, PoC 1, PoC 2, and PoC 3 passed
**Safety class:** Offline compiler and format analysis; no custom graph may execute on the Wave in this PoC
**Program design:** `docs/superpowers/specs/2026-07-28-wave-npu-reverse-engineering-program-design.md`

## 1. Goal

Recover a legitimate, reproducible route that compiles a deliberately tiny
TIM-VX graph for the exact Wave NPU target:

```text
NBG magic:    VPMN
NBG version:  expected to be compatible with the Wave v2 loader
product ID:   0x1000000e
graph:        UINT8 Add([2], [2]) -> [2]
semantics:    [1, 2] + [3, 4] = [4, 6]
```

The product ID and every target-dependent section must come from the
compiler's selected hardware description. Editing an NBG field after
generation is not compilation and is explicitly excluded.

PoC 5 answers:

- Which host compiler component chooses the simulator chip identity?
- Does the bundled TIM-VX x86 simulator contain a profile for the Wave
  product, or does it require an external hardware configuration?
- Can a version-adjacent VIP9000 SDK or an exact Wave userspace component
  supply that configuration without involving the live device?
- Is compilation deterministic for a fixed graph, toolchain, and target?
- Does the generated file pass the project's strict NBG parser and
  independent structural comparisons with the stock Wave corpus?

PoC 5 does **not**:

- execute any generated graph on `/dev/galcore`;
- patch a product ID, version, CRC, command stream, or feature field;
- invoke raw galcore ioctls;
- overwrite a stock model, library, executable, or configuration;
- flash, reboot, reset, stop services, alter modules, or write sysfs/debugfs;
- claim operator correctness from a syntactically valid NBG alone.

## 2. Evidence Baseline

The accepted stock corpus contains 138 NBG v2 files for product
`0x1000000e`. The stock CTC graph executes repeatedly with the accepted
output digest. The existing host Add graph is a useful negative control:

| Graph | Compiler/runtime | Version | Product | Status |
|---|---|---:|---:|---|
| Stock CTC | Wave firmware | `0x00020000` | `0x1000000e` | Live-proven |
| Existing Add | bundled x86 TIM-VX simulator | `0x0001001e` | `0x10000020` | Foreign; never execute |

The pinned TIM-VX checkout is:

```text
repository: https://github.com/VeriSilicon/TIM-VX
commit:     702715d714277376d7ff466b7b72833ff57564c1
x86 SDK:    6.4.15_CL690884A_D690855_A690484_R690194_T690259_O688896
```

The x86 simulator exposes `VSIMULATOR_CONFIG`. TIM-VX documentation also
describes `VIV_GPU_FILE` for newer no-kernel compiler SDKs. The local SDK
bundle includes `libGAL.so`, `libOpenVX.so`, `libVSC.so`,
`libArchModelSw.so`, and `libNNArchPerf.so`, but no external hardware
`.config` file has yet been found.

The exact Wave copies of the compiler-related DSOs are preserved under
`waved/third_party/device-sdk/lib/`. Version-adjacent, symbols-rich
VIP9000 6.4.18.5 copies are preserved under
`waved/third_party/vip9000-sdk/6.4.18.5/`.

## 3. Route Matrix

Routes are attempted in order. A route that cannot establish authentic
target selection is documented and stopped; it is never repaired by
editing output bytes.

### Route A — bundled x86 simulator identity

1. Build the fixed tiny Add compiler against the pinned x86 SDK.
2. Record the default identity and output.
3. Enumerate only documented or binary-evidenced `VSIMULATOR_CONFIG`
   identities.
4. Compile twice per accepted identity in fresh output directories.
5. Strict-parse each result and record version, product, sections, CRCs,
   I/O metadata, and file hash.

This route passes only if the simulator itself emits an unmodified product
`0x1000000e` graph.

### Route B — external hardware configuration

1. Search pinned public BSPs and SDKs for a genuine VIP9000NanoQ hardware
   configuration matching product `0x1000000e`.
2. Preserve repository URL, commit, path, license, and SHA-256.
3. Validate the file's declared identity independently.
4. Supply it through the documented `VIV_GPU_FILE` mechanism to a compatible
   no-kernel compiler.
5. Compile the identical graph twice and apply the same parser gate.

No configuration is synthesized by copying values from a stock NBG.

### Route C — exact Wave compiler components under host emulation

If Routes A and B cannot produce the target:

1. Map exports/imports and loader requirements of the exact AArch64 Wave
   `libVSC`, `libArchModelSw`, `libNNArchPerf`, `libOpenVX`, and
   `libovxlib` artifacts.
2. Build an isolated AArch64 root containing only copied userspace files.
3. Run only under host user-mode emulation, with no `/dev/galcore`, no
   device bind mounts, and no network.
4. Invoke read-only identity/configuration APIs before any graph compiler
   entry.
5. Attempt graph export only if all dependencies and arguments are
   reconstructed from exact headers or call sites.

An on-device compiler harness is outside PoC 5. It would require a separate
plan even if it did not execute a graph.

### Route D — private export-data grammar

In parallel with compiler-component analysis, reconstruct the `nbgType=2/3`
sectioned export-data grammar from the exact `libnnsdk.so`:

```text
[export.data.info]:
[node_info]:
[norm_tensor]:
[const_tensor]:
[virtual_tensor]:
[conn_info]:
[param_info]:
[graph_info]:
```

The parser and graph-builder can provide an independent semantic
description of a tiny graph. This route is considered host-valid only when:

- every emitted field has an exact parser/call-site basis;
- a strict host parser accepts the complete document;
- tensor sizes, types, quantization, connections, and operation parameters
  are self-consistent;
- malformed and incomplete variants are rejected in subprocess isolation.

It does not become executable merely because it parses.

## 4. Fixed Graph Definition

The first graph is intentionally smaller than an audio model:

```text
input A:  UINT8, asymmetric scale 1.0, zero point 0, shape [2]
input B:  UINT8, asymmetric scale 1.0, zero point 0, shape [2]
operation: Add
output C: UINT8, asymmetric scale 1.0, zero point 0, shape [2]
vectors:
  A = [1, 2]
  B = [3, 4]
  C = [4, 6]
```

The source, build flags, linked-library hashes, environment, stdout, stderr,
exit status, and output hash are captured for every compiler invocation.

## 5. Host Experiment Design

### Phase A — inventory and provenance

- hash every compiler-related DSO;
- record ELF architecture, SONAME, build ID, version strings, imports, and
  exports;
- record TIM-VX and BSP commits;
- locate documented environment selectors and their consumers;
- classify each artifact as exact Wave, version-adjacent public, or generic
  simulator.

### Phase B — reproducible compiler harness

- build inside a pinned Linux/amd64 container;
- copy the compiler executable and its runtime dependency closure to a
  content-addressed output directory;
- add `--describe` and fixed graph-export modes;
- reject unknown options and pre-existing output paths;
- create output atomically inside a dedicated host artifact directory;
- run with a fixed locale and a minimal environment;
- record all identities before parsing the NBG.

### Phase C — selector differential

For each candidate selector:

1. compile the same graph from a clean process;
2. compile it a second time from another clean process;
3. parse both files with `nbg_inspect.py --strict`;
4. compare full hashes and normalized section maps;
5. compare against the foreign default control and representative stock v2
   files;
6. reject any result whose product, version, checksums, table bounds, or I/O
   metadata are inconsistent.

Invalid selectors are bounded negative tests. A crash, hang, or ambiguous
fallback stops enumeration until its cause is understood.

### Phase D — target authenticity review

A candidate `0x1000000e` result receives an independent review:

- confirm the compiler reported the same identity before export;
- confirm byte offset `0x08` was emitted by the compiler, not changed later;
- compare all target-bearing header/table fields, not only offset `0x08`;
- require valid CRC coverage for every covered section;
- verify no output post-processor has write access to the NBG;
- reproduce the same normalized graph twice;
- document any nondeterministic bytes and prove they are metadata only.

## 6. Acceptance Gate

PoC 5 passes only if all of these are true:

- the graph is emitted by an identified compiler route without byte patching;
- strict parsing reports `VPMN` and target `0x1000000e`;
- the NBG version is accepted by the exact Wave loader's statically recovered
  version gate;
- every table, region, and CRC checked by the project parser is valid;
- I/O descriptors match the fixed graph definition;
- two clean builds have identical normalized structure and either identical
  bytes or a fully explained bounded metadata difference;
- compiler artifacts, target configuration, source, environment, and output
  hashes are recorded;
- the candidate passes a manual review that no stock graph bytes or product
  field were transplanted;
- no live-device action occurred.

If no available compiler can legitimately emit `0x1000000e`, PoC 5 closes
with a precise blocker and the evidence needed to obtain the missing vendor
hardware profile. A foreign graph is never promoted.

## 7. First Custom-Graph Device Gate

Passing PoC 5 still does not authorize execution. A follow-on PoC must define
and review:

- an exact graph SHA-256 and parser report;
- a no-argument, hash-pinned device probe;
- independent expected output and bounded iterations;
- pre/post health snapshots and the PoC 3 metadata-only trace;
- graph-create, execute, retrieve, and destroy timeouts;
- stop conditions for create rejection, cleanup failure, ADB loss, kernel
  diagnostics, or changed device state;
- no retry, reset, reboot, service action, or stock-file write.

Only that separately accepted graph may be staged and run.

## 8. Deliverables

```text
waved/tools/npu-compiler/
  README.md
  build.sh
  tim_vx_compile_add.cc
  compile-matrix.sh
  inspect-sdk.sh

waved/scripts/npu/
  compiler_matrix.py
  tests/test_compiler_matrix.py

docs/npu/compiler/
  README.md
  sdk-inventory.json
  target-selection.md
  compile-matrix.json
  wave-add.strict.json              # only after the gate passes

docs/npu/lab-notebook/
  2026-07-28-poc5-wave-targeted-compiler.md
```

Generated binaries, raw compiler logs, and NBG outputs remain under the
gitignored `artifacts/npu/compiler/` tree. Compact normalized evidence is
tracked.

## 9. Stop Conditions

Stop the current route on:

- a compiler timeout or crash;
- an undocumented selector that silently falls back to the default target;
- inconsistent product identities across diagnostics and output;
- invalid table bounds, sizes, alignments, or CRCs;
- an output write after the compiler exits;
- a requirement to patch target-bearing bytes;
- a requirement for live `/dev/galcore` access;
- unclear provenance or redistribution rights for a target configuration;
- evidence that the exact Wave runtime and compiler ABI generations are
  incompatible.

The failure and its hashes are preserved. The next route starts from a clean
process and never mutates the failed output.

## 10. Result

The bundled x86 simulator remained a foreign-target control:

```text
existing host Add NBG:
  magic:    VPMN
  version:  0x0001001e
  target:   0x10000020

required Wave target:
  0x1000000e
```

Static analysis found a Wave identity profile, but its selector is applied
after the simulator HAL has materialized its default identity. No authentic
external Wave hardware configuration was available. The host route was
closed without patching target bytes or executing its output.

This clean negative justified the separately reviewed PoC 5B exact-device
compile-only route. Later PoC 5C produced a genuine Wave-targeted Erf NBG;
that narrow device result does not retroactively make the host simulator a
Wave compiler.
