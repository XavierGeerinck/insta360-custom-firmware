# Wave NPU PoC 9C: `VIV_GPU_FILE` Parser Recovery

**Date:** 2026-07-28
**Status:** Completed host-only 2026-07-28
**Authorization:** The owner approved compiler/profile recovery, hybrid CPU
fallback, and continued stepwise implementation
**Design:** `docs/superpowers/specs/2026-07-28-wave-npu-compiler-profile-onnx-hybrid-design.md`
**Depends on:** PoC 9A strict Wave architecture profile
**Safety class:** Host-side source, ELF, string, symbol, relocation,
decompilation, and isolated parser testing only

## 1. Question

What does the available VeriSilicon/TIM-VX stack mean by a hardware
configuration file, how does it select a target, and can a deterministic
Wave candidate be rendered using only facts proven in PoC 9A?

The public TIM-VX platform sample names `VIV_GPU_FILE`, but the local binary
corpus must determine whether the available stack accepts:

- a filename whose stem names a built-in target;
- a file containing five identity assignments;
- a wider key/value configuration;
- a serialized private structure;
- or a different contract in each SDK variant.

PoC 9C does not compile a graph and does not execute anything on the Wave.

## 2. Immediate evidence

The local TIM-VX sample says:

```text
VIV_GPU_FILE=/path/to/VIP9000NANOQ_PLUS_PID0X100000XX.config
```

That is a family example with a placeholder, not proof of the Wave name or
file contents.

The x86 `libEmulator.so` contains:

```text
StateParser::chooseTargetByConfigFile(char const*)
StateParser::getConfig(_VSimulatorConfig*)
vsimConfigs
gcQueryFeatureDB(...)
```

It also contains a built-in target name for customer `0x1000000e`:

```text
VIP9000NANOS_PID0X1000000E
```

This differs from the sample's generic `NANOQ_PLUS` spelling and must be
resolved before a candidate filename is emitted.

## 3. Inputs

Hash-pin:

- `waved/third_party/TIM-VX/prebuilt-sdk/x86_64_linux/lib/libEmulator.so`;
- x86 `libGAL.so`, `libOpenVX.so.1.3.0`, `libVSC.so`, and `libCLC.so` where
  they participate in configuration discovery;
- exact Wave `libGAL.so`, `libOpenVX.so`, `libVSC.so`, and
  `libVSC_Lite.so` for negative/differential string and call evidence;
- adjacent 6.4.18 `libGAL.so`, `libOpenVX.so`, `libVSC.so`, and
  quarantined `libCLC.so` for static comparison only;
- local TIM-VX platform and multi-device sample READMEs;
- PoC 9A `wave-profile-evidence.json`.

The quarantined CLC may be hashed, inventoried, and decompiled. Its compiler
entrypoints remain forbidden.

## 4. Focus symbols and diagnostics

Primary x86 symbols:

```text
StateParser::chooseTargetByConfigFile(char const*)
StateParser::getConfig()
StateParser::getConfig(_VSimulatorConfig*)
StateParser::StateParser(...)
StateParser::~StateParser(...)
gcQueryFeatureDB(...)
vsimConfigs
StateParser::m_Config
StateParser::m_Config_byUser
StateParser::m_hwConfigID
```

Search all relevant artifacts for:

```text
VIV_GPU_FILE
VSIMULATOR_CONFIG
chipModel
chipRevision
productID
ecoID
customerID
QueryFeatureDB
not found GPU configuration
Invalid config
fopen
fscanf
sscanf
ifstream
getline
```

## 5. Deliverables

Create:

```text
waved/scripts/npu/compiler_profile_targets.json
waved/scripts/npu/compiler_profile.py
waved/scripts/npu/tests/test_compiler_profile.py
docs/npu/profile/compiler-profile-ghidra-evidence.json
docs/npu/profile/viv-gpu-file-grammar.json
docs/npu/profile/wave-viv-gpu-file.candidate.config
docs/npu/profile/wave-viv-gpu-file.candidate.manifest.json
docs/npu/lab-notebook/2026-07-28-poc9c-viv-gpu-file-parser-recovery.md
```

Update:

```text
docs/npu/profile/README.md
docs/npu/README.md
docs/npu/lab-notebook/README.md
waved/scripts/npu/README.md
```

If the corpus does not prove a renderable file grammar, do not create the
candidate config. Create a refusal manifest explaining the missing contract
instead.

## 6. Evidence classes

Every grammar claim must be one of:

- `source-documented`: exact local upstream source or sample;
- `static-verified`: demonstrated by a hash-pinned binary;
- `differentially-verified`: same behavior in independent variants;
- `isolated-observed`: returned by a bounded host subprocess;
- `inferred`;
- `unknown`.

A filename seen in a string table proves only that the name exists. A format
string proves only a possible display/parse pattern. Neither alone proves a
required field or default.

## 7. Implementation tasks

### Task 1: Strict corpus manifest

Tests first:

- duplicate and unknown keys fail;
- paths must remain below the repository;
- lowercase SHA-256 is mandatory;
- every configured file must match;
- ELF targets must be ELF;
- text targets must be valid UTF-8;
- roles are exact and unique;
- the quarantined CLC role cannot be marked executable.

The loader never imports or executes a configured ELF.

### Task 2: Focused Ghidra evidence

Extend the existing bounded Ghidra adapter through a dedicated target file.
Do not modify PoC 1 or PoC 9A evidence.

Acceptance:

- the full `chooseTargetByConfigFile` body is successful and untruncated;
- direct callers identify how a path reaches it;
- file and string APIs are captured;
- `getConfig` and `gcQueryFeatureDB` show the output contract;
- zero Ghidra warnings;
- a second run is byte-identical.

### Task 3: Recover discovery semantics

Determine:

- which component reads `VIV_GPU_FILE`;
- whether the path or environment variable is handled by the currently
  available SDK or only a no-kernel SDK named by the sample;
- how `VSIMULATOR_CONFIG` differs;
- whether extensions and basename are significant;
- whether a missing file falls back;
- whether an unknown config name falls back;
- whether fallback can be disabled.

Acceptance:

- each discovery step names its exact artifact/function;
- absent functionality in the local SDK is recorded, not invented;
- `VIV_GPU_FILE` and `VSIMULATOR_CONFIG` are never conflated.

### Task 4: Recover file grammar

Determine:

- lexical encoding and line endings;
- comment syntax;
- whitespace rules;
- assignment separator and terminator;
- numeric formats and widths;
- case sensitivity;
- exact key spellings;
- required versus optional keys;
- duplicate-key behavior;
- unknown-key behavior;
- range and cross-field validation;
- default selection.

Candidate minimal identity grammar to test only after static evidence:

```text
chipModel    = 0x00009000;
chipRevision = 0x00009002;
productID    = 0x05090001;
ecoID        = 0x04000000;
customerID   = 0x1000000e;
```

The spacing and spelling above are hypotheses until the parser proves them.

### Task 5: Recover built-in configuration catalog

Read `vsimConfigs` as inert data using symbol/section bounds.

Acceptance:

- exact entry count and record width;
- exact list digest;
- one unique customer-`0x1000000e` name;
- nearby target names for differential context;
- no inference from table position alone;
- the Wave name is separated from the public sample placeholder.

### Task 6: Isolated parser behavior

Only if constructor, object size, method ABI, ownership, and dependencies are
statically proven:

- build a tiny x86 host harness;
- run it in a fresh temporary directory and child process;
- use a fixed empty environment;
- set CPU and wall-clock limits;
- pass only test files created for this PoC;
- capture bounded stdout/stderr and exit/signal;
- never invoke OpenVX, CLC, VSC, graph, or compilation symbols.

Test:

- exact built-in Wave name;
- exact minimal identity file;
- missing file;
- empty file;
- duplicate key;
- unknown key;
- malformed number;
- overlong line;
- wrong customer ID.

If the ABI is not sufficiently proven, do not guess an object layout. Use a
pure reimplementation test oracle and record vendor-parser dynamic
equivalence as unresolved.

### Task 7: Strict parser and renderer

Implement a host-only parser for exactly the recovered grammar.

Requirements:

- duplicate-key rejection;
- unknown-key rejection;
- bounded file and line sizes;
- ASCII or UTF-8 contract as recovered;
- exact integer syntax and range;
- exact required-key set;
- canonical key order;
- lowercase or uppercase rendering exactly as proven;
- atomic output;
- source profile hash in the manifest;
- refusal if any emitted field is unknown or inferred below the allowed
  confidence gate.

No generic user-defined key/value passthrough is allowed.

### Task 8: Candidate generation

Generate only if every emitted field has:

- a recovered parser key;
- a proven type and width;
- a PoC 9A value;
- a source reference;
- no unresolved default dependency.

The candidate filename must come from parser behavior, not merely the sample
README.

The manifest records:

```text
candidate SHA-256
grammar evidence SHA-256
Wave profile SHA-256
source artifact hashes
emitted fields and sources
non-claims
```

The filename includes `.candidate` so it cannot be mistaken for a qualified
compiler input.

### Task 9: Documentation

The notebook records:

- exact hashes and tool versions;
- discovery path;
- decompiled parser pseudocode;
- built-in name catalog result;
- grammar with positive and negative examples;
- defaults and fallback behavior;
- isolated subprocess envelope, if authorized by proven ABI;
- candidate or refusal decision;
- implications for PoC 9D;
- explicit statement that no graph was compiled.

## 8. Tests

Unit:

- strict manifest loading;
- bounded config parsing;
- canonical rendering;
- every malformed grammar case;
- duplicate and unknown keys;
- integer overflow and sign;
- required fields;
- target mismatch;
- source-profile provenance;
- deterministic candidate and manifest;
- atomic writes;
- candidate refusal on an unknown fact.

Integration:

- real artifact hashes;
- focused evidence completeness;
- built-in table extraction;
- unique Wave config name;
- isolated vendor parser cases, only if ABI-proven;
- second generation byte identity;
- full existing NPU suite.

Commands:

```bash
python3 -m unittest \
  waved.scripts.npu.tests.test_compiler_profile

python3 waved/scripts/npu/compiler_profile.py validate-targets
python3 waved/scripts/npu/compiler_profile.py generate-ghidra
python3 waved/scripts/npu/compiler_profile.py recover-grammar
python3 waved/scripts/npu/compiler_profile.py generate-candidate
python3 waved/scripts/npu/compiler_profile.py verify-generated

python3 -m unittest discover \
  -s waved/scripts/npu/tests \
  -p "test_*.py"

python3 -m py_compile \
  waved/scripts/npu/compiler_profile.py

git diff --check
```

## 9. Stop conditions

Stop without invoking a parser or compiler if:

- any input hash differs;
- the relevant function is missing or truncated;
- file grammar depends on a guessed structure;
- the local library only supports named built-ins, not file contents;
- a candidate needs an unknown PoC 9A field;
- the parser silently defaults an unknown Wave target;
- a dynamic harness would run constructors or code outside the parser
  boundary;
- a test would reach OpenVX, VSC, CLC, graph construction, or compilation;
- an action would touch ADB or `/dev/galcore`.

A stop yields a refusal manifest and the next recovery route.

## 10. Completion gate

PoC 9C passes when:

- config discovery and parser ownership are identified;
- built-in target naming is recovered;
- the accepted grammar, types, defaults, and validation are documented;
- the strict host parser rejects malformed files;
- a deterministic Wave candidate is generated only if fully evidenced;
- all emitted fields map to PoC 9A or a proven compiler default;
- all tests and deterministic evidence checks pass;
- no compiler or device call occurred.

Passing PoC 9C authorizes planning PoC 9D's isolated fixed-Conv compiler
compatibility test. It does not authorize that compiler call by itself.

## 11. Completion record

Completed deliverables:

```text
waved/scripts/npu/compiler_profile_corpus.json
waved/scripts/npu/compiler_profile_targets.json
waved/scripts/npu/compiler_profile.py
waved/scripts/npu/tests/test_compiler_profile.py
waved/tools/npu-profile/vsim_config_parser_probe.cc
docs/npu/profile/compiler-profile-ghidra-evidence.json
docs/npu/profile/vsim-parser-probe-results.json
docs/npu/profile/viv-gpu-file-grammar.json
docs/npu/profile/wave-viv-gpu-file.candidate.config
docs/npu/profile/wave-viv-gpu-file.candidate.manifest.json
docs/npu/lab-notebook/2026-07-28-poc9c-viv-gpu-file-parser-recovery.md
```

Recovered:

- five sequential `%x` assignments at config offsets `0x50..0x60`;
- a 100-byte config record with an 80-byte name;
- the 358-entry `vsimConfigs` table at ELF file offset `0x27c760`;
- unique Wave-family catalog record
  `VIP9000NANOS_PID0X1000000E`;
- simulator target selection for decimal ID, built-in name, and path;
- EOF-as-success, unbounded-line, unchecked-trailing-content, and
  void/no-status parser weaknesses;
- live revision `0x9002` acceptance in an isolated fixed case;
- deterministic rendering from high-confidence PoC 9A facts.

The actual no-kernel `VIV_GPU_FILE` consumer is not in the local corpus. The
candidate manifest therefore refuses compiler, graph, and device use. This is
a passed parser-recovery gate and a clean compiler-qualification blocker, not
a compiler unlock.
