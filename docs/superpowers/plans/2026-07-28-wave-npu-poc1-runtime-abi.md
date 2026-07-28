# Wave NPU PoC 1 — Runtime ABI and Call-Graph Implementation Plan

**Date:** 2026-07-28  
**Status:** Approved program, PoC 1 execution plan  
**Depends on:** PoC 0 pass, commit `f2c4096`  
**Safety class:** Host-only static analysis  
**Program design:** `docs/superpowers/specs/2026-07-28-wave-npu-reverse-engineering-program-design.md`

## 1. Goal

Recover an evidence-backed map of the Wave NPU userspace ABI and stock graph
lifecycle from the exact firmware binaries.

PoC 1 answers:

- Which public and internal entry points exist in each known firmware?
- Which functions implement NBG parsing, product selection, tensor binding,
  execution, output retrieval, and cleanup?
- Which `libnnsdk` backend family is reached by the working stock CTC path?
- What does `vipunify_*` wrap?
- Which ADLA-looking paths are compiled, exported, dispatched, or merely
  present as strings?
- Which firmware v3 audio-model APIs exist, and which stock binaries call
  them?
- Which C prototypes and structure layouts are safe enough for project FFI?
- Which assumptions remain too weak to encode in FFI?

The output must let a reviewer trace every project-used native function from:

```text
project FFI declaration
  -> stock caller or public header evidence
  -> exact provider export
  -> implementation/backend branch
  -> cleanup and error path
  -> firmware hash
```

## 2. Non-goals

PoC 1 does not:

- execute a new graph;
- attach a debugger or tracer to the device;
- change a device file;
- infer the complete NBG binary schema;
- recover kernel ioctl payloads;
- compile a custom graph;
- claim `libmodel_infer.so` uses a particular backend based on strings alone;
- claim that a function name proves hardware is present;
- publish guessed prototypes as safe FFI.

NBG parsing belongs to PoC 2. Runtime/driver tracing belongs to PoC 3.
`libmodel_infer.so` dynamic backend validation belongs to PoC 4.

## 3. Safety Boundary

All PoC 1 work is host-only.

Allowed:

- hash and inspect extracted firmware files;
- read local source and public headers;
- run `rz-bin`, Rizin analysis, Ghidra headless, and standard host tools;
- generate deterministic JSON and Markdown under the repository;
- clone public reference repositories into a temporary directory;
- compare exact bytes across firmware versions.

Not needed and therefore excluded:

- ADB;
- device execution;
- `/dev/galcore` access;
- sysfs reads or writes;
- service changes;
- firmware changes;
- model changes;
- kernel-module operations;
- reboot;
- raw command replay.

If a static question cannot be answered, record it as unknown. Do not convert
PoC 1 into a dynamic experiment.

## 4. Evidence Hierarchy

Each recovered claim receives a confidence class:

| Class | Label | Required evidence |
|---|---|---|
| A | `live-proven` | PoC 0 or another allowlisted live experiment exercised the exact ABI successfully. |
| B | `binary-proven` | Exact hashed provider/caller bytes, relocation or xref, and disassembly/decompilation agree. |
| C | `header-cross-checked` | Public vendor header agrees with exact binary calling convention or caller layout. |
| D | `strong-inference` | Multiple static observations agree, but an argument, branch, or backend remains unresolved. |
| E | `name-only` | Export or string name exists without adequate code-path proof. |
| F | `unknown` | Evidence is insufficient or contradictory. |

Only A–C declarations may be marked `safe_for_project_ffi: true`.

Name-only ADLA and backend strings remain E until a call edge or dynamic test
supports them.

## 5. Exact Static Corpus

Firmware roots:

- `firmwares/fw_v1.0.13.6/extracted/rootfs`
- `firmwares/fw_v1.1.6.6/extracted/rootfs`
- `firmwares/fw_business_v1.0.2.6/extracted/rootfs`
- `firmwares/fw_v3.0.0.1/extracted/rootfs`

Provider libraries:

- `usr/lib/libnnsdk.so`
- `usr/lib/libovxlib.so`
- `usr/lib/libOpenVX.so`
- `usr/lib/libGAL.so`
- `usr/lib/libVSC.so`
- `usr/lib/libArchModelSw.so`
- `usr/lib/libmodel_infer.so` where present

Primary callers:

- `usr/lib/libasr_reverb.so`
- every ELF in `usr/lib`, `usr/bin`, and `usr/sbin` importing one of the
  relevant exported symbols;
- v3 callers of `model_infer_{ans,aes}_*`;
- project source in `waved/crates/waved-npu-probe/src/nnsdk.rs`.

The corpus configuration records required versus optional files. Missing
optional `libmodel_infer.so` in older firmware is a matrix fact, not a tool
failure.

## 6. Pinned Public References

Public references are cross-checks, never substitutes for exact firmware
evidence:

- Amlogic NN SDK commit
  `96f7846bbbac4def9ccc38b649a43e77a8194435`;
- local TIM-VX commit
  `702715d714277376d7ff466b7b72833ff57564c1`
  (`v1.2.22-9-g702715d`);
- eYs3D VIP9000 rootfs commit
  `f6d3c3b87667d333eaa0785d0a1a2078ca5bfdc8`;
- eYs3D VIP9000 kernel commit
  `946d2c841fd82b916bdd2168033906bfec89db61`.

Relevant header excerpts are documented by repository, commit, path, symbol,
and line. Do not vendor an entire external SDK into this repository for PoC
1.

## 7. Deliverables

Create:

- `waved/scripts/npu/abi_corpus.json`
- `waved/scripts/npu/abi_inventory.py`
- `waved/scripts/npu/tests/test_abi_inventory.py`
- `scripts/ghidra/ExportNpuAbi.java`
- `docs/npu/abi/README.md`
- `docs/npu/abi/runtime-abi-inventory.json`
- `docs/npu/abi/firmware-matrix.md`
- `docs/npu/abi/prototypes.md`
- `docs/npu/abi/stock-ctc-call-graph.md`
- `docs/npu/abi/backend-map.md`
- `docs/npu/abi/error-taxonomy.md`
- `docs/npu/abi/ghidra-evidence.json`
- `docs/npu/lab-notebook/2026-07-28-poc1-runtime-abi.md`

Modify:

- `docs/npu/README.md`
- `waved/docs/COMPUTE_HARDWARE.md`
- `waved/crates/waved-npu-probe/src/nnsdk.rs` only if PoC 1 finds an
  evidence-backed correction or can add a missing compile-time layout test
  without expanding the live ABI.

Raw Ghidra projects, complete decompilations, and temporary public repositories
remain outside Git.

## 8. Corpus Schema

`abi_corpus.json` uses strict schema version 1:

```json
{
  "schema_version": 1,
  "firmwares": {
    "fw_v1.1.6.6": {
      "root": "firmwares/fw_v1.1.6.6/extracted/rootfs",
      "live_match": true
    }
  },
  "binaries": {
    "libnnsdk.so": {
      "path": "usr/lib/libnnsdk.so",
      "required": true,
      "roles": ["provider", "runtime"]
    }
  },
  "symbol_families": {
    "project_ffi": ["aml_module_create"]
  }
}
```

Constraints:

- roots and binary paths are repository-relative;
- no path may escape the repository;
- schema and nested keys are exact;
- firmware IDs and binary IDs are stable;
- project FFI symbols are an explicit exact list;
- broader research families use anchored prefixes or exact names;
- optional files are represented as absent rather than omitted from results.

## 9. Deterministic ELF Inventory

`abi_inventory.py` uses the Python standard library and `rz-bin`.

For every corpus binary it records:

- repository-relative path;
- presence;
- bytes;
- SHA-256;
- ELF class, architecture, endianness, PIE, stripped status, and load base;
- linked libraries;
- imported relevant symbols;
- exported relevant symbols;
- relevant named relocations;
- function virtual address, physical file offset, and reported size;
- SHA-256 of exact function bytes when offset and size are valid;
- relevant diagnostic strings with bounded context metadata;
- tool name and version.

It excludes:

- complete binary strings;
- unrelated symbols;
- absolute host paths;
- timestamps;
- non-deterministic ordering;
- complete decompilation text.

The generated JSON is canonical: sorted keys, sorted arrays, no run timestamp,
and repository-relative paths.

### Relevant symbol families

Exact project FFI:

- `aml_module_create`
- `aml_module_input_set`
- `aml_module_output_get`
- `aml_module_destroy`
- `aml_util_getInputTensorInfo`
- `aml_util_getOutputTensorInfo`
- `aml_util_freeTensorInfo`

Runtime families:

- `aml_module_`
- `aml_util_`
- `read_nbg_`
- `get_product_`
- `get_hardware_`
- `aml_vipunify_`
- `vipunify_`
- `aml_viplite_`
- `aml_adla_`
- `Adla`
- `vsi_nn_`
- selected `vx` and `gco` lifecycle functions;
- `model_infer_ans_`
- `model_infer_aes_`.

String filters:

- NBG version/product/size/read failures;
- product-list rejection;
- driver and hardware version;
- backend selection;
- `/dev/galcore` and `/dev/adla0`;
- graph create/run/wait/output/cleanup;
- NNAPI, OpenCL, OpenGL ES, ONNX, VXC, and archmodel terms.

## 10. Function Byte Identity

Symbol presence alone is insufficient for a firmware matrix.

For each defined relevant function with a valid file offset and nonzero size,
the inventory hashes its exact bytes. The matrix classifies each symbol:

- `identical` — same function-byte hash;
- `changed` — present with different byte hash;
- `size-only` — present but bytes cannot be safely extracted;
- `added`;
- `removed`;
- `absent`;
- `unknown`.

Virtual addresses are recorded as evidence but are not treated as semantic
identity.

Tiny four- or eight-byte exports are flagged as probable thunks and must not
be interpreted as complete implementations without following their target.

## 11. Caller Discovery

The inventory scans ELF imports across bounded firmware directories to answer:

- Which binaries import each project FFI symbol?
- Does the stock caller use direct ELF relocation, PLT call, or `dlsym`?
- Which binaries import `model_infer_ans_*` or `model_infer_aes_*`?
- Are ADLA functions imported by a stock caller, internally defined, resolved
  with `dlsym`, or present only as strings?

The result is a provider/caller index:

```text
symbol
  provider firmware/hash/address/size/function hash
  callers firmware/hash/import kind
  evidence class
```

## 12. Focused Ghidra Export

`ExportNpuAbi.java` is a deterministic headless post-script.

Inputs:

- output JSON path;
- comma-separated exact target function names;
- optional comma-separated diagnostic string patterns.

For each found target it exports:

- program name and program SHA-256;
- Ghidra version/language/compiler spec;
- symbol name;
- entry address;
- function body size;
- called function names and call-site addresses;
- caller function names and reference addresses;
- bounded decompiler C text;
- decompiler success/failure;
- referenced diagnostic strings;
- evidence warnings for thunks, unresolved indirect calls, or duplicate names.

The script must:

- sort all output;
- normalize newlines;
- avoid absolute paths;
- cap decompiled text per function;
- emit valid JSON even when a target is absent;
- delete the temporary Ghidra project after export.

Focused evidence targets include:

- project FFI exports in `libnnsdk.so`;
- NBG memory/file parsers;
- product/version selectors;
- `aml_vipunify_*` graph/input/output/destroy routes;
- candidate ADLA dispatch;
- the stock caller functions referencing lifecycle imports;
- generic and model-specific `model_infer_{ans,aes}` load/run functions.

Complete library decompilation is not a deliverable.

## 13. Prototype Ledger

`docs/npu/abi/prototypes.md` is the human-reviewed ABI contract.

Each row includes:

- symbol;
- provider hash and firmware range;
- proposed C prototype;
- AAPCS64 special behavior;
- argument ownership and lifetime;
- return ownership and lifetime;
- structure size and relevant field offsets;
- cleanup pair;
- error convention;
- evidence IDs;
- confidence class;
- `safe for project FFI` yes/no.

Minimum project FFI ledger:

```c
void *aml_module_create(struct aml_config *config);
int aml_module_input_set(void *module, struct nn_input *input);
struct nn_output *aml_module_output_get(
    void *module,
    struct aml_output_config config_by_value
);
int aml_module_destroy(void *module);

struct tensor_info_list *aml_util_getInputTensorInfo(
    const char *nbg_memory
);
struct tensor_info_list *aml_util_getOutputTensorInfo(
    const char *nbg_memory
);
int aml_util_freeTensorInfo(struct tensor_info_list *list);
```

These signatures remain provisional in the plan. The ledger must support each
with exact evidence before marking it safe.

### Required structure checks

- `aml_config`: 2,272 bytes; offsets 0, 4, 8, 16, 24, 28;
- input info: 32 bytes;
- `nn_input`: 64 bytes; offsets 4, 8, 16, 24, 28;
- output config: 56 bytes; `perf_mode` offset 8; `format` offset 12;
- output buffer: 96 bytes; buffer offset 72; params offset 80;
- output descriptor: count offset 4; first buffer offset 8;
- tensor info entry: 116 bytes; name offset 52;
- tensor info list: 16 bytes; entries offset 8.

PoC 1 verifies the evidence for existing Rust tests and adds any missing
offset tests. A decompiler-assigned type is not sufficient by itself.

## 14. AAPCS64 Aggregate Check

`aml_module_output_get` takes a 56-byte output-config aggregate by value.
Under AAPCS64 this is passed indirectly as a copy, not as the Rust source-level
pointer type one might guess from decompiler output.

Acceptance requires:

- stock caller disassembly at the call site;
- provider entry disassembly or decompilation;
- public header cross-check if available;
- current Rust function type;
- compile-time size/offset tests.

This ABI detail receives a dedicated section because a wrong declaration may
appear to work until stack or register layout changes.

## 15. Stock CTC Call Graph

The call graph has four levels:

```text
waved-npu-probe
  -> libnnsdk public module API
    -> selected backend family
      -> OVXLIB/OpenVX/Vivante runtime
        -> libGAL -> /dev/galcore
```

For each lifecycle stage document:

1. tensor metadata parse;
2. module configuration parse;
3. NBG memory read;
4. NBG version and product selection;
5. backend context/network creation;
6. input tensor binding/copy;
7. run or wait;
8. output retrieval;
9. graph/network/context destruction;
10. tensor-metadata destruction.

Every edge is labeled:

- direct call;
- import relocation;
- thunk;
- function pointer;
- inferred dispatch;
- live-proven boundary.

Unknown indirect edges remain dashed and described.

## 16. Backend Map

`backend-map.md` distinguishes:

- public module layer;
- VIP unify layer;
- VIP lite path;
- OVXLIB/OpenVX path;
- ADLA path;
- model delegate path;
- firmware v3 audio-model wrapper;
- kernel-facing GAL path.

For each branch record:

- selection condition if recovered;
- create/input/run/output/destroy functions;
- provider library;
- direct dependencies;
- product/version checks;
- firmware presence;
- evidence class.

The ADLA branch must be split into:

- exported functions;
- internal defined functions;
- dlsym strings;
- device-node strings;
- actual stock callers;
- live device-node presence from the existing manifest.

The absence of `/dev/adla0` on the PoC 0 live device and the presence of ADLA
names in `libnnsdk.so` are both recorded without collapsing them into a
single conclusion.

## 17. Error Taxonomy

Map diagnostic strings and return paths to lifecycle stages:

- definition/config validation;
- file or memory NBG read;
- NBG magic/version/quantization;
- hardware product list;
- context/network creation;
- tensor allocation and binding;
- input submission;
- schedule/run/wait;
- output retrieval;
- cleanup;
- backend unavailable;
- driver mismatch.

Each error entry records:

- exact bounded string;
- provider hash;
- string address;
- code xref function;
- likely stage;
- confidence;
- current probe exit class where applicable.

Strings without code xrefs are labeled `name-only`.

## 18. Firmware Matrix

The matrix compares all four firmware roots:

- file presence, hash, and size;
- relevant imports/exports;
- project FFI function-byte identity;
- internal NBG/product/backend functions;
- `libmodel_infer.so` introduction;
- caller changes;
- structure/ABI evidence changes;
- runtime dependency changes.

The live v1.1.6.6-compatible hash set is the reference column.

An exported symbol retaining its name while its function bytes change is
explicitly marked changed. ABI stability is not inferred solely from the
name.

## 19. Tests

`test_abi_inventory.py` uses synthetic files and fake `rz-bin` JSON. No
firmware or external tool is required for unit tests.

Tests:

- valid corpus loads;
- unknown schema/key fails;
- escaping paths fail;
- missing required binary fails;
- missing optional binary is represented;
- SHA-256 and byte size are exact;
- malformed tool JSON fails clearly;
- nonzero tool exit fails clearly;
- irrelevant symbols and strings are excluded;
- project FFI symbols are retained exactly;
- exports and imports are distinguished;
- function bytes use physical offset, not virtual address;
- invalid offset/size is represented and never read out of bounds;
- function byte hashes are deterministic;
- thunk-sized functions are flagged;
- arrays and keys are sorted;
- output contains no absolute workspace path;
- firmware matrix classifies identical, changed, added, removed, and absent;
- caller/provider index is deterministic;
- generated output is byte-for-byte stable across two runs.

## 20. Task 1 — Pin Corpus and Failing Tests

Create `abi_corpus.json` and failing tests for schema, path safety, relevant
symbol filtering, function-byte hashes, and firmware comparison.

Run:

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -v
```

Expected before implementation: import or assertion failure for the ABI tool.

## 21. Task 2 — Implement ELF Inventory

Implement:

- strict dataclasses and corpus loader;
- repository boundary validation;
- command runner boundary;
- `rz-bin` JSON parsing;
- binary and function hashing;
- relevant symbol/string filtering;
- deterministic canonical output;
- `inventory` CLI command.

Run:

```bash
python3 waved/scripts/npu/abi_inventory.py inventory \
  --output docs/npu/abi/runtime-abi-inventory.json
```

The command is host-only.

## 22. Task 3 — Generate Firmware Matrix and Caller Index

Add:

- symbol-state comparison;
- function-byte identity;
- caller/provider index;
- deterministic Markdown rendering;
- matrix tests.

Commands:

```bash
python3 waved/scripts/npu/abi_inventory.py matrix \
  --inventory docs/npu/abi/runtime-abi-inventory.json \
  --output docs/npu/abi/firmware-matrix.md
```

## 23. Task 4 — Focused Ghidra Evidence

Write and test the headless post-script against:

1. live-matched `libnnsdk.so`;
2. live-matched `libasr_reverb.so`;
3. v3 `libmodel_infer.so`;
4. the discovered v3 stock callers;
5. `libovxlib.so` only for selected wrapper targets.

Store normalized focused output in `ghidra-evidence.json`. Record Ghidra
12.0.4 and the Java runtime.

If a target times out or fails decompilation, record the failure and retain
disassembly evidence. Do not invent pseudocode.

## 24. Task 5 — Public Header Cross-check

Clone public references to a temporary directory at the pinned commits.

For every project FFI declaration:

- find an exact or closest public declaration;
- record repository/commit/path/line;
- compare argument types, aggregate passing, and struct layout;
- explain vendor/version differences;
- prefer the exact Wave caller when references conflict.

No external repository is committed.

## 25. Task 6 — Human-Review Maps

Write:

- `prototypes.md`;
- `stock-ctc-call-graph.md`;
- `backend-map.md`;
- `error-taxonomy.md`;
- ABI README with reproduction commands.

Every assertion links to an inventory evidence ID, Ghidra evidence ID,
project layout test, or public reference.

## 26. Task 7 — Project FFI Audit

Compare the ledger with `waved-npu-probe/src/nnsdk.rs`.

For every `unsafe extern "C"` function:

- verify prototype;
- verify ownership/lifetime note;
- verify cleanup pair;
- verify relevant structures;
- verify firmware hash range;
- verify test coverage.

Add compile-time layout assertions only when evidence warrants them.

Run:

```bash
cargo test --manifest-path waved/Cargo.toml -p waved-npu-probe --lib
```

Do not expand the runtime API in PoC 1.

## 27. Task 8 — Notebook and Status

Publish:

- exact corpus hashes;
- tool versions;
- call graph;
- confirmed and unresolved prototypes;
- backend classification;
- firmware changes;
- error taxonomy;
- negative findings;
- acceptance checklist.

Update `docs/npu/README.md` and the authoritative compute document.

## 28. Final Verification

Run:

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -v
python3 -m py_compile \
  waved/scripts/npu/abi_inventory.py \
  waved/scripts/npu/tests/test_abi_inventory.py
python3 -m json.tool waved/scripts/npu/abi_corpus.json >/dev/null
python3 -m json.tool docs/npu/abi/runtime-abi-inventory.json >/dev/null
python3 -m json.tool docs/npu/abi/ghidra-evidence.json >/dev/null
python3 waved/scripts/npu/abi_inventory.py verify-generated
cargo test --manifest-path waved/Cargo.toml -p waved-npu-probe --lib
git diff --check -- \
  waved/scripts/npu \
  scripts/ghidra \
  docs/npu \
  waved/docs/COMPUTE_HARDWARE.md \
  waved/crates/waved-npu-probe/src/nnsdk.rs
```

Inspect the scoped diff and keep raw firmware and Ghidra projects untracked.

## 29. Acceptance Criteria

PoC 1 passes only when:

- all exact corpus binaries are hashed and indexed;
- the live-matched firmware is clearly identified;
- every project-called native function has a proposed prototype, exact
  provider evidence, caller/header cross-check, structure tests where
  relevant, ownership note, cleanup pair, and confidence class;
- unsafe or unresolved prototypes are marked not safe for FFI;
- stock CTC lifecycle is represented as a labeled call graph;
- NBG memory/file parsers and product/version checks are located;
- `vipunify_*` routes are mapped as far as static evidence permits;
- ADLA exports, branches, strings, callers, and live absence are not
  conflated;
- v3 audio loader exports and exact callers are inventoried;
- firmware changes are based on function bytes, not symbol names alone;
- errors are mapped to lifecycle stages with xref confidence;
- generated artifacts are deterministic;
- unit and existing Rust layout tests pass;
- the notebook distinguishes observed, binary-proven, header-cross-checked,
  inferred, name-only, and unknown claims;
- no device operation occurred.

## 30. Stop Conditions

Stop and document rather than guess when:

- a provider function is only reachable through an unresolved function
  pointer;
- Ghidra and disassembly disagree;
- public header and exact caller disagree;
- symbol size is zero or clearly a thunk and the target is unresolved;
- structure layout lacks both caller and header support;
- a diagnostic string has no code xref;
- a firmware file is missing or its hash changes during analysis;
- a tool emits nondeterministic or malformed evidence.

These conditions may lower confidence or defer a claim. They do not fail the
whole PoC if the limitation is explicit and no unsafe FFI depends on it.

## 31. Commit Sequence

1. `docs: plan Wave NPU PoC 1 runtime ABI`
2. `test(npu-abi): define static inventory contracts`
3. `feat(npu-abi): add deterministic firmware ABI inventory`
4. `feat(npu-abi): export focused Ghidra call evidence`
5. `docs(npu): publish PoC 1 runtime ABI map`

Implementation and tests may be combined if an intermediate commit would be
non-working. Generated evidence and the generator must be committed together.

