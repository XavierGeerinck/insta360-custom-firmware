# Wave NPU PoC 9A: Architecture Database Recovery

**Date:** 2026-07-28
**Status:** Completed and verified host-only
**Authorization:** The device owner approved the compiler/profile recovery
design and requested implementation to continue
**Design:** `docs/superpowers/specs/2026-07-28-wave-npu-compiler-profile-onnx-hybrid-design.md`
**Depends on:** PoC 5D exact capability query and PoC 8 compiler boundary
**Safety class:** Static ELF, strings, symbol, relocation, and decompilation
analysis only; no AArch64 library invocation, ADB, graph construction,
compilation, `/dev/galcore`, or device execution

## 1. Question

What target identity, feature database, architecture configuration, and
performance-model fields are embedded in the exact Wave
`libArchModelSw.so` and `libNNArchPerf.so`, and which of them can be
recovered with field-level evidence without calling either target library?

PoC 9A does not generate `VIV_GPU_FILE`. It produces the evidence profile
needed to recover that grammar in PoC 9C.

## 2. Inputs

Hash-pin and analyze:

- exact Wave AArch64 `libArchModelSw.so`;
- exact Wave AArch64 `libNNArchPerf.so`;
- the matching repository copy in `waved/third_party/device-sdk`;
- the TIM-VX x86 simulator versions of both libraries;
- the version-adjacent 6.4.18 AArch64 versions when present.

The duplicate exact firmware/device-SDK copy is recorded as a provenance
cross-check but analyzed only once.

Focus functions:

```text
libArchModelSw:
  archShowConfigurations
  archIsFeatureAvailable
  archSwLibInit
  archSwLibInitFromDriver
  archSetManualFeatures
  updateCustomizedCArchConfigs
  archCalculateArchPerf
  archFillOpInfo

libNNArchPerf:
  APMDeriveHWFeatures
  APMGetBugFeatures
  APMSetManualFeatures
  CreateAPModel
  DestroyAPModel
  amInitialize
  amSetFeaturesSetting
  amSetState
```

Focus diagnostics:

```text
ChipID
ChipVersion
ProductID
EcoID
CustomerID
featureDB
NumNNCores
NumUsedNNCores
VIP SRAM
AXI SRAM
VIP7Version
ArchModelVersion
SWTilingVersion
```

## 3. Deliverables

Create:

```text
waved/scripts/npu/arch_profile_targets.json
waved/scripts/npu/arch_profile.py
waved/scripts/npu/tests/test_arch_profile.py
docs/npu/profile/arch-profile-ghidra-evidence.json
docs/npu/profile/wave-profile.schema.json
docs/npu/profile/wave-profile-evidence.json
docs/npu/profile/README.md
docs/npu/lab-notebook/2026-07-28-poc9a-architecture-database-recovery.md
```

Update:

```text
waved/scripts/npu/README.md
docs/npu/README.md
docs/npu/lab-notebook/README.md
```

Large deterministic Ghidra evidence may be tracked because it is the direct
source for field-level claims. It must contain only local binary analysis,
not device-derived secrets or mutable temporary paths.

## 4. Evidence Schema

`wave-profile-evidence.json` has a strict schema and rejects duplicate or
unknown keys. Its root contains:

```text
schema_version
profile_id
target
inputs
identity
features
resources
versions
unknowns
non_claims
```

Every recovered field contains:

```text
value
representation
evidence_class
confidence
sources[]
validation_state
notes
```

Allowed evidence classes:

```text
observed-live
static-verified
differentially-verified
inferred
unknown
```

Allowed validation states:

```text
corroborated
single-source
candidate
unresolved
```

The generator must not promote a field from `inferred` or `unknown` merely
because a nearby SDK uses a plausible value.

## 5. Implementation Tasks

### Task 1: Add strict target and profile schemas

Files:

- create `waved/scripts/npu/arch_profile_targets.json`;
- create `waved/scripts/npu/arch_profile.py`;
- create `waved/scripts/npu/tests/test_arch_profile.py`.

Tests first:

- reject duplicate JSON keys;
- reject unknown root, target, and fact keys;
- reject paths outside the repository;
- reject a missing or uppercase SHA-256;
- reject a mismatched artifact hash;
- reject a non-ELF target;
- reject duplicate source references;
- reject an unknown evidence class or validation state;
- require `0x1000000e` as the Wave profile target;
- serialize deterministically.

Implementation:

- use immutable dataclasses for loaded config;
- resolve every path beneath the repository;
- hash every binary before analysis;
- expose `validate-targets`, `generate-ghidra`, `generate-profile`, and
  `verify-generated` subcommands;
- write outputs atomically;
- never import or execute analyzed ELF files.

Verification:

```bash
python3 -m unittest waved.scripts.npu.tests.test_arch_profile
python3 waved/scripts/npu/arch_profile.py validate-targets
```

### Task 2: Generate focused Ghidra evidence

Reuse:

- `waved/scripts/npu/ghidra_abi.py`;
- `scripts/ghidra/ExportNpuAbi.java`.

The profile tool loads the dedicated target config and calls the existing
hash-checked Ghidra adapter. It does not modify the earlier PoC 1 target list
or evidence file.

Requirements:

- analyze only the named functions and their direct callers;
- capture callers, callees, referenced diagnostics, and bounded decompiled C;
- record Ghidra version, language, compiler spec, file size, and SHA-256;
- run from a temporary Ghidra project and delete it;
- preserve the existing 30-minute per-target timeout;
- generate canonical JSON.

Verification:

```bash
python3 waved/scripts/npu/arch_profile.py generate-ghidra
python3 waved/scripts/npu/arch_profile.py verify-generated --ghidra-only
```

### Task 3: Recover the identity lookup contract

From the exact AArch64 decompilation:

- locate the feature-database lookup call;
- recover the order and width of chip ID, chip version, product ID, eco ID,
  and customer ID;
- distinguish arguments read from the driver identity from manual/default
  fields;
- recover failure behavior and fallback/default selection;
- map the exact diagnostic strings to call sites.

Acceptance:

- the five-key lookup tuple is statically demonstrated;
- recovered widths and offsets cite function entry and decompiled expression;
- any uncertain offset remains unresolved;
- no profile value is inferred from a format string alone.

### Task 4: Recover configuration and resource fields

From `archShowConfigurations`, initialization, and feature derivation:

- enumerate printed configuration field names in output order;
- map structure offsets when decompilation supports them;
- recover core-count fields by precision;
- recover SRAM/cache fields and their units;
- recover EVIS/VIP generation and instruction-feature flags;
- recover architecture-model, software-tiling, and VIP7 version strings;
- distinguish configuration, driver option, and feature-database structs.

Acceptance:

- each mapped field has at least one static source;
- critical target identity fields have an independent live/NBG source;
- fields with the same human-readable label but different structs remain
  separate;
- units are recorded only when demonstrated.

### Task 5: Differential comparison

Compare exact AArch64, adjacent 6.4.18 AArch64, and x86 simulator evidence:

- exported symbol sets;
- function body sizes and call relationships;
- diagnostic/configuration field sets;
- version strings;
- lookup signatures;
- feature derivation and bug-feature branches.

Acceptance:

- exact matches are labeled `differentially-verified`;
- version-only or architecture-only differences are recorded;
- absence in one build is not interpreted as absence in hardware;
- the adjacent compiler is never loaded.

### Task 6: Generate the initial Wave evidence profile

Populate only facts supported by current evidence.

Minimum identity section:

```text
product/customer target: 0x1000000e
eco ID:                  0x04000000
EVIS1:                   false
EVIS2:                   true
subgroup size:           4
VA40:                    false
stream processor:        false
```

These live values are existing PoC 5D observations. PoC 9A adds static
architecture-library sources where found; it does not rerun the query.

Minimum unknown section:

- physical NPU core count;
- per-precision usable core count;
- exact VIP SRAM partitioning;
- AXI SRAM compiler budget;
- cache-line and tensor-alignment constraints;
- complete errata/bug-feature vector;
- all `VIV_GPU_FILE` spellings and required defaults.

Acceptance:

- schema validation passes;
- every fact cites immutable artifact hashes and locations;
- every unresolved item has a reason and the next recovery route;
- no candidate config file is emitted in PoC 9A.

### Task 7: Documentation and reproduction

The notebook records:

- the hypothesis;
- exact input hashes and tool versions;
- focused functions and diagnostics;
- recovered structs and lookup flow;
- exact versus adjacent differences;
- field-level confidence;
- unknowns and non-claims;
- the PoC 9B ABI gate;
- the PoC 9C configuration-parser gate.

Update indexes and reproduction commands without changing prior PoC results.

## 6. Test Matrix

Unit tests:

- strict JSON parsing;
- path confinement;
- hash verification;
- ELF verification;
- evidence-class validation;
- source-reference validation;
- target identity validation;
- deterministic canonical JSON;
- atomic output behavior;
- disagreement handling;
- unknown-field preservation.

Integration tests:

- fake Ghidra executor produces deterministic evidence;
- malformed Ghidra evidence is rejected;
- real target hashes validate;
- generated profile validates against its schema;
- a second generation is byte-identical;
- existing NPU test suite remains green.

Commands:

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -p "test_*.py"
python3 -m py_compile waved/scripts/npu/arch_profile.py
python3 -m json.tool waved/scripts/npu/arch_profile_targets.json
python3 -m json.tool docs/npu/profile/arch-profile-ghidra-evidence.json
python3 -m json.tool docs/npu/profile/wave-profile.schema.json
python3 -m json.tool docs/npu/profile/wave-profile-evidence.json
git diff --check
```

## 7. Stop Conditions

Stop PoC 9A without expanding scope if:

- an input hash differs;
- the exact library is not ELF;
- Ghidra cannot resolve a requested function;
- decompilation disagrees across deterministic reruns;
- a field cannot be separated from a similarly named field;
- a source would require invoking AArch64 code;
- a conclusion depends only on the adjacent 6.4.18 SDK;
- the work would require ADB or `/dev/galcore`.

A stop produces a documented unknown, not a guessed profile value.

## 8. Completion

PoC 9A passes when:

- all input artifacts are hash-pinned;
- focused Ghidra evidence is deterministic;
- the five-key architecture lookup contract is recovered;
- configuration/resource fields are inventoried with source locations;
- the strict Wave evidence profile validates;
- unknowns remain explicit;
- all NPU tests pass;
- the notebook explains what PoC 9A proves and does not prove.

PoC 9A does not authorize PoC 9B device calls or PoC 9C compiler parsing.
Those receive separate plans after this result is reviewed.

## 9. Completion Record

Completed on 2026-07-28 without contacting the Wave.

Verified results:

- all four input ELFs matched their configured SHA-256;
- Ghidra 12.0.4 recovered every requested exact-Wave target with zero
  warnings;
- a second four-target Ghidra run was byte-identical to the tracked evidence;
- the exact five-word lookup and `0x0000fff0` revision mask were recovered;
- the unique Wave and x86 customer-`0x1000000e` rows were hash-bound;
- the live `0x9002` and database `0x9001` revisions remain distinct;
- the strict generated profile contains 36 facts and 10 explicit unknowns;
- the profile passed its Python contract and Draft 2020-12 JSON Schema;
- 184 host NPU tests passed;
- JSON syntax, Python compilation, deterministic profile regeneration, and
  `git diff --check` passed.

Result documentation:

- `docs/npu/profile/README.md`;
- `docs/npu/profile/wave-profile-evidence.json`;
- `docs/npu/lab-notebook/2026-07-28-poc9a-architecture-database-recovery.md`.

No `VIV_GPU_FILE`, compiler output, graph, ADB command, driver call, or
device-side artifact was produced.
