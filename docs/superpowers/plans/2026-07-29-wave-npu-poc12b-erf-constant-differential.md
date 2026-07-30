# PoC 12B Controlled Erf Constant Differential — Implementation Plan

**Date:** 2026-07-29

**Design:** `docs/superpowers/specs/2026-07-29-wave-npu-erf-constant-differential-design.md`

**Planning workflow:** Local test-first fallback because the required
`writing-plans` skill is not installed in this session

**Execution mode:** Implement and verify host contracts first; cross-build
once; review exact hashes and live preflight; then run the three approved
compile-only modes

## 1. Objective

Produce a controlled same-topology Wave NBG differential:

```text
A1 constant: [0,1,2,3]
A2 constant: [0,1,2,3]
B  constant: [0,1,2,4]
graph:        CONSTANT U8[4] -> Erf -> OUTPUT U8[4]
```

The implementation must decide:

1. whether the exact producer is byte-deterministic for A1/A2;
2. whether the one-byte A/B source change survives into the NBG;
3. which exact owned ranges change;
4. which changes are payload versus verified integrity collateral;
5. whether the result is sufficient for one richer constant differential.

No generated NBG is loaded or executed.

## 2. Working-tree constraints

The repository contains substantial uncommitted work from earlier PoCs.

Rules:

- preserve every unrelated modification and untracked file;
- use `apply_patch` for tracked workspace edits;
- do not reset, clean, checkout, or rewrite existing work;
- do not stage or commit implementation unless explicitly requested;
- the two design commits already made are the only required commits;
- use exact paths for temporary cleanup;
- never broaden a deletion to `/tmp`, the workspace root, or a glob;
- keep raw run files under ignored `artifacts/npu/`.

## 3. Shell and environment rules

Every repository, build, test, and device shell command runs through:

```text
lean-ctx -c '...'
```

or:

```text
lean-ctx raw '...'
```

The only approved container use is one pinned offline build invocation:

```text
image: existing immutable cross-rs AArch64 digest
network: none
purpose: compile and inspect the fixed probe
device container calls: zero
unapproved container calls: zero
```

If the image is not already available locally, stop. Do not pull it during
the experiment.

## 4. Planned files

### 4.1 Create

```text
waved/tools/npu-compiler/tim_vx_compile_erf_constant_diff.cc
waved/tools/npu-compiler/build-device-erf-constant-diff.sh
waved/tools/npu-compiler/verify-device-erf-constant-diff-policy.sh
waved/tools/npu-compiler/stage-device-erf-constant-diff.sh

waved/scripts/npu/erf_constant_diff_manifest.json
waved/scripts/npu/npu_erf_constant_diff_lab.py
waved/scripts/npu/nbg_constant_diff.py

waved/scripts/npu/tests/test_npu_erf_constant_diff_lab.py
waved/scripts/npu/tests/test_nbg_constant_diff.py

docs/npu/format/erf-constant-differential.json
docs/npu/lab-notebook/2026-07-29-poc12b-erf-constant-differential.md
```

### 4.2 Modify

```text
docs/npu/README.md
docs/npu/format/README.md
docs/npu/format/field-map.md
docs/npu/lab-notebook/README.md
docs/npu/reproduction.md
waved/scripts/npu/README.md
waved/docs/COMPUTE_HARDWARE.md
```

### 4.3 Reuse without semantic changes

```text
waved/scripts/npu/nbg_format.py
waved/scripts/npu/nbg_image.py
waved/scripts/npu/npu_compile_lab.py
waved/tools/npu-compiler/tim_vx_compile_erf.cc
waved/tools/npu-compiler/build-device-erf.sh
waved/tools/npu-compiler/verify-device-erf-policy.sh
```

## 5. Task 1 — Define strict host contracts first

### 5.1 Manifest test module

Create:

```text
waved/scripts/npu/tests/test_npu_erf_constant_diff_lab.py
```

Write failing tests for:

- schema version;
- exact device serial and kernel;
- exact remote root `/tmp/wave-npu-poc12b`;
- exact executable path;
- exact environment allowlist;
- exact candidate order `a1`, `a2`, `b`;
- fixed arguments `--a1`, `--a2`, `--b`;
- fixed output and partial-output paths;
- exactly three candidates;
- fixed constants and graph definition;
- exact target/version/size limits;
- required artifact hash validation;
- duplicate JSON key rejection;
- unknown/missing field rejection;
- path escape and symlink refusal;
- unexpected candidate or output refusal;
- simulator/interposition environment refusal;
- any nonzero prohibited-action counter refusal;
- exactly one approved offline build-container provenance record.

Use temporary manifests for negative cases. Tests must not invoke ADB.

### 5.2 Harness behavior tests

Implement a fake transport with explicit call recording.

Write failing tests for:

- preflight success;
- no device, wrong serial, or multiple devices;
- wrong kernel;
- wrong `/dev/galcore` major/minor;
- wrong required-artifact hash;
- unexpected holder;
- pre-existing remote root;
- inadequate temp space;
- A1 success/pull/parse;
- A1 failure blocks A2 and B;
- A2 failure blocks B;
- A1/A2 byte mismatch blocks B;
- B success only after exact determinism;
- malformed result line;
- duplicate result line;
- timeout;
- wrong target/version;
- output too small/large;
- pull hash mismatch;
- parser/checksum failure;
- new kernel alert;
- changed galcore parameter;
- cleanup success;
- cleanup failure;
- no kill/reset/reboot/service-stop command;
- no more than three native invocations;
- raw run directory exclusivity.

### 5.3 Differential test module

Create:

```text
waved/scripts/npu/tests/test_nbg_constant_diff.py
```

Reuse synthetic VPMN v2 fixture builders from the NBG parser/image tests.

Write failing tests for:

- A1/A2 exact match;
- A1/A2 first-difference reporting;
- direct U8 constant mapping;
- fixed block/lane mapping;
- identical A/B folded-or-elided classification;
- valid section CRC collateral;
- missing or wrong CRC collateral;
- header `crc_sum` treated as unresolved producer collateral;
- structural mismatch;
- section-directory mismatch;
- duplicate source occurrence ambiguity;
- multi-region unresolved classification;
- in-memory exact observed-B reconstruction;
- reconstruction mismatch;
- deterministic canonical evidence;
- no raw section payload in evidence;
- stale evidence refusal;
- output non-overwrite.

### 5.4 Run red tests

Expected initial failures are missing modules/files only:

```text
python3 -m unittest \
  waved.scripts.npu.tests.test_npu_erf_constant_diff_lab \
  waved.scripts.npu.tests.test_nbg_constant_diff
```

Do not start native source until the expected contract failures are clear.

## 6. Task 2 — Implement the strict manifest and host data types

### 6.1 Manifest schema

Create:

```text
waved/scripts/npu/erf_constant_diff_manifest.json
```

Root fields:

```text
schema_version
poc_id
device
environment
remote_root
remote_executable
timeout_ms
pull_timeout_ms
required_artifacts
candidates
graph
expected_nbg
policy
outputs
```

Candidate fields:

```text
id
argument
constant_u8
expected_oracle_u8
output
partial_output
```

Policy fields:

```text
allowed_operations
approved_build_provenance
prohibited_actions
```

The initial manifest may contain only already known firmware/runtime hashes.
The new probe hash must not use a sentinel or placeholder. Add it only after
the successful reviewed build.

### 6.2 Frozen Python contracts

In `npu_erf_constant_diff_lab.py`, define frozen dataclasses:

```text
CandidateSpec
GraphSpec
DeviceSpec
ExperimentPolicy
ExperimentManifest
NativeResult
CandidateCapture
ExperimentResult
```

All maps exposed publicly use immutable views or copied canonical
structures.

### 6.3 Strict loader

Implement:

```text
load_manifest(path)
```

Requirements:

- duplicate-key JSON hook;
- exact key sets at every level;
- strict integer-versus-bool checks;
- canonical lowercase SHA-256;
- repository-relative local paths;
- exact remote path prefix;
- control-character rejection;
- three exact candidates and constants;
- A1 equals A2;
- A versus B differs at exactly one byte;
- all expected oracle arrays equal `[0,1,1,1]`;
- all prohibited counters equal zero;
- one approved pinned offline build record;
- no simulator/interposition environment keys.

## 7. Task 3 — Implement the host-only differential analyzer

### 7.1 Data structures

In `nbg_constant_diff.py`, define frozen dataclasses:

```text
ByteDifference
DifferenceRun
SectionDifference
IntegrityCollateral
DeterminismResult
StructuralComparison
MappingCandidate
DifferentialResult
```

### 7.2 Source admission

The analyzer receives exact file specs rather than arbitrary directory
discovery.

For each file:

1. reject symlinks and nonregular files;
2. validate expected byte count and SHA-256;
3. parse with `nbg_format.parse_bytes`;
4. require VPMN v2 and target `0x1000000e`;
5. require stored section checksums to pass;
6. construct `NbgImage`;
7. prove total ownership;
8. prove unchanged exact round trip.

### 7.3 Determinism

Implement:

```text
compare_identical(a1, a2)
```

Record:

- sizes and hashes;
- exact-match boolean;
- first byte difference;
- structural signature equality;
- section descriptor/hash equality.

Never interpret B when this gate fails.

### 7.4 Structural A/B comparison

Compare all stable envelope, directory, tensor, network, and memory fields.

Do not normalize:

- total size;
- section offsets;
- header CRC;
- section checksum records.

Classify differences rather than hiding them.

### 7.5 Changed-run enumeration

Walk equal-length files byte by byte and merge adjacent offsets into maximal
runs.

Every run records:

```text
start/end/length
old/new edge hex, bounded
old/new SHA-256
owner and byte class
section index/type/name
section-relative offset
candidate interpretations
classification
```

If sizes differ, record tail insertion/removal as a bounded run and classify
structural mismatch unless an exact section-size rule is demonstrated.

### 7.6 Integrity explanation

For each changed payload section:

- compute A and B CRC-32;
- locate exact type-40 checksum records;
- confirm stored A/B values;
- mark the checksum run explained only when both values match.

Header `crc_sum` may be recorded but remains unresolved.

### 7.7 Finite constant mapping

Generate only:

```text
identity
four-byte fixed permutations already admitted in PoC 12A
zero-padding to original section alignment
```

Search admitted section ranges with exact byte matching.

Require uniqueness across the complete admitted range set.

### 7.8 Classification

Emit exactly one:

```text
direct-constant-mapping
deterministic-transformed-mapping
constant-folded-or-elided
serializer-nondeterministic
multi-region-unresolved
compile-blocked
device-health-failure
```

PoC 12C readiness is:

```text
ready-limited
not-ready
```

### 7.9 Canonical evidence

Create:

```text
build_evidence(...)
generate
verify-generated
```

Canonical JSON is UTF-8, sorted, two-space indented, final newline, and
written atomically without overwrite during raw-run capture.

## 8. Task 4 — Implement the fixed native probe

### 8.1 Source

Create:

```text
waved/tools/npu-compiler/tim_vx_compile_erf_constant_diff.cc
```

Adapt only the reviewed mechanisms from `tim_vx_compile_erf.cc`.

Fixed constants:

```text
remote root:       /tmp/wave-npu-poc12b
max NBG bytes:     64 MiB
watchdog:          15 seconds
chip model:        0x00009000
customer/target:   0x1000000e
version:           0x00020000
shape:             [4]
dtype:             UINT8
quantization:      asymmetric, scale 1, zero point 0
```

### 8.2 CLI

Accepted:

```text
--describe
--a1
--a2
--b
```

Every other argv returns 10 before context creation.

`--describe` prints the exact graph and constants and performs no native
runtime call.

### 8.3 Constant tensor

Use:

```text
TensorAttribute::CONSTANT
graph->CreateTensor(constant_spec, constant_bytes.data())
```

Bind exactly one input and one output to one `tim::vx::ops::Erf`.

Do not call `CopyDataToTensor`, `Run`, `Compile`, graph execution, or tensor
readback.

### 8.4 Identity and environment

Before graph construction:

- exact `VIVANTE_SDK_DIR`;
- required `LD_LIBRARY_PATH`;
- reject `VSIMULATOR_CONFIG`, `LD_PRELOAD`, `LD_AUDIT`;
- set parent-death signal;
- arm watchdog;
- create context;
- call `gcoHAL_QueryChipIdentityEx`;
- require model/customer values.

### 8.5 Output

Map each fixed mode to exactly one final and partial path.

Use exclusive, no-follow creation; bounded write loop; `fchmod(0444)`;
`fsync`; close; atomic same-directory hard-link/rename pattern; no final
overwrite.

On failure, unlink only the current partial.

### 8.6 Result

Emit one strict line naming:

```text
status
mode=compile-only
candidate
bytes
version
product
output
```

## 9. Task 5 — Build and source/ELF policy

### 9.1 Build script

Create:

```text
waved/tools/npu-compiler/build-device-erf-constant-diff.sh
```

Use the same immutable cross-rs image as PoC 5C.

Before invoking:

```text
docker image inspect \
  ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c
```

Stop if absent.

Build requirements:

- `--network=none`;
- C++17;
- warnings as errors;
- PIE;
- stack protector;
- RELRO/NOW;
- no exec stack;
- exact runpath `/tmp/wave-npu-poc12b/lib:/usr/lib`;
- `-ltim-vx -lGAL -ldl -lpthread`;
- deterministic build metadata.

### 9.2 Static source policy

Create:

```text
waved/tools/npu-compiler/verify-device-erf-constant-diff-policy.sh
```

Assert:

- exact source/binary paths;
- exact three final and three partial output literals;
- exact A/A/B arrays;
- exactly one Erf operation;
- exactly two `CompileToBinary` call sites;
- constant tensor attribute;
- one input/output binding;
- no `Run`, `CopyDataToTensor`, `ioctl`, `/dev/galcore`, `dlopen`,
  `system`, `exec`, `fork`, or arbitrary file path;
- exact accepted modes;
- watchdog and 64 MiB bound.

### 9.3 ELF policy

Inside the pinned offline container, assert:

- ELF64 AArch64 DYN/PIE;
- required libraries only;
- exact runpath;
- BIND_NOW and RELRO;
- required Erf and chip-identity imports;
- no graph execution, ioctl, dlopen, or process-launch import;
- no reference to `libCLC.so`.

### 9.4 Capture exact hashes

After one successful build:

- hash source;
- hash binary;
- hash `libtim-vx.so`;
- hash the required SDK header;
- inspect `--describe` through an emulator only if already supported without
  loading vendor libraries; otherwise source-test the description;
- patch the strict manifest and staging script with the exact binary hash.

Rerun all policy and manifest tests after the hash patch.

## 10. Task 6 — Implement staging

Create:

```text
waved/tools/npu-compiler/stage-device-erf-constant-diff.sh
```

The script:

1. verifies local source/binary/runtime/header hashes;
2. requires exactly the approved serial;
3. refuses any existing remote root;
4. creates the exact directory tree;
5. pushes to hash-named incoming files;
6. verifies remote hashes;
7. assigns exact modes;
8. atomically hard-links/renames to final stage paths;
9. verifies final hashes and permissions;
10. verifies all six output/partial paths are absent;
11. prints the exact ready state.

It does not run the probe.

Implement a separate exact cleanup command in the Python harness rather than
a general shell `rm` interface.

## 11. Task 7 — Implement the live host orchestrator

### 11.1 Read-only preflight

Use the transport patterns in `npu_compile_lab.py`, but keep the new policy
isolated.

Capture:

- serial list;
- `uname -a`;
- `/dev/galcore` major/minor;
- required artifact hashes;
- NPU holders;
- selected galcore parameters;
- boot ID when available;
- uptime;
- bounded dmesg tail;
- remote root absence;
- `/tmp` available bytes.

### 11.2 Candidate run

For one candidate:

- build one fixed quoted remote command;
- allow only fixed environment;
- append a unique remote-exit sentinel;
- bound host output;
- parse exact native result;
- require expected exit;
- inspect output and partial states;
- pull to an exclusive host path;
- hash and parse immediately;
- capture post-candidate health.

### 11.3 Sequence

```text
preflight
stage verification
A1
A2
host exact A1/A2 comparison
if exact: B
postflight
cleanup
cleanup verification
```

If A1/A2 mismatch, skip B and continue only postflight/cleanup.

### 11.4 Run envelope

Write:

```text
definition.json
preflight.json
candidate files/logs/reports
postflight.json
cleanup.json
run-envelope.json
```

No file overwrite.

## 12. Task 8 — Host-only validation before device work

Run:

```text
python3 -m unittest \
  waved.scripts.npu.tests.test_npu_erf_constant_diff_lab \
  waved.scripts.npu.tests.test_nbg_constant_diff

python3 -m unittest \
  waved.scripts.npu.tests.test_npu_compile_lab \
  waved.scripts.npu.tests.test_nbg_format \
  waved.scripts.npu.tests.test_nbg_image \
  waved.scripts.npu.tests.test_nbg_serializer_recovery

python3 -m py_compile \
  waved/scripts/npu/npu_erf_constant_diff_lab.py \
  waved/scripts/npu/nbg_constant_diff.py

bash -n \
  waved/tools/npu-compiler/build-device-erf-constant-diff.sh \
  waved/tools/npu-compiler/verify-device-erf-constant-diff-policy.sh \
  waved/tools/npu-compiler/stage-device-erf-constant-diff.sh
```

Run Ruff in the existing pinned ONNX environment if available.

No ADB mutation occurs before every host gate passes.

## 13. Task 9 — Cross-build and hash freeze

### 13.1 Check builder availability

Read-only:

```text
docker image inspect \
  ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c
```

If absent, stop and report. Do not pull.

### 13.2 Build once

Run the build script exactly once.

Record:

- image digest;
- command policy;
- binary bytes and SHA-256;
- source SHA-256;
- dynamic dependencies;
- ELF security properties;
- policy result.

### 13.3 Freeze

Patch the manifest and staging script with the observed exact hash.

Rerun:

- policy;
- focused unit tests;
- manifest validation;
- local artifact hashing.

Do not rebuild after freezing. Any source edit invalidates the binary and
requires a new reviewed build.

## 14. Task 10 — Live read-only preflight

Run only the harness preflight command.

Manually review:

```text
one serial
exact kernel
exact firmware hashes
no holder
no stage root
no new alert
sufficient /tmp space
zero native calls
```

If any gate fails, stop.

## 15. Task 11 — Stage and verify

Run the fixed staging script.

Then run preflight again with staged-artifact expectations:

- all staged hashes exact;
- permissions exact;
- output/partial paths absent;
- no unexpected files;
- no NPU holder;
- no health change.

Capture the exact planned remote argv for A1/A2/B in the raw definition.

## 16. Task 12 — Live A1/A2/B sequence

### 16.1 A1

Run once.

Require:

- valid result;
- target/version correct;
- output bounds;
- no partial;
- successful pull;
- strict parser/checksum/ownership/roundtrip pass;
- health unchanged.

### 16.2 A2

Run once with the same requirements.

### 16.3 Determinism gate

Compare A1/A2 on the host.

If any difference:

- classify `serializer-nondeterministic`;
- do not run B;
- perform postflight and cleanup;
- retain evidence.

### 16.4 B

Only after A1/A2 exact equality, run B once with the same requirements.

### 16.5 Postflight and cleanup

Capture health first, then remove only:

```text
/tmp/wave-npu-poc12b
```

Confirm absence and capture final ADB/holder/alert state.

No output is re-staged.

## 17. Task 13 — Analyze and generate evidence

Bind the pulled A1/A2/B hashes into the raw run envelope.

Run:

```text
python3 waved/scripts/npu/nbg_constant_diff.py analyze \
  --run artifacts/npu/runs/poc12b-erf-constant-diff-20260729 \
  --output docs/npu/format/erf-constant-differential.json
```

Then:

```text
python3 waved/scripts/npu/nbg_constant_diff.py verify-generated
```

Inspect:

- classification;
- PoC 12C readiness;
- A1/A2 equality;
- structural comparison;
- changed sections/runs;
- direct/permuted constant matches;
- CRC collateral;
- header CRC behavior;
- reconstruction result;
- prohibited counters.

Do not broaden transforms after seeing the result.

## 18. Task 14 — Documentation

Create the complete notebook:

```text
docs/npu/lab-notebook/2026-07-29-poc12b-erf-constant-differential.md
```

Include:

- hypothesis;
- exact source/build/runtime/device identities;
- preflight;
- graph definitions and constant oracle;
- exact native commands and counts;
- A1/A2 determinism result;
- A/B structural and byte-run ledger;
- checksum explanation;
- classification;
- PoC 12C decision;
- cleanup and health;
- test totals;
- raw/tracked evidence locations;
- positive, negative, inferred, and unknown findings;
- non-claims;
- reproduction.

Update all seven authoritative indexes named in the design.

## 19. Task 15 — Final regression and audit

Run the complete NPU unit suite inside the pinned environment so NumPy-backed
correlation tests are admitted:

```text
uv run --project waved/tools/onnx-hybrid \
  python -m unittest discover \
  -s waved/scripts/npu/tests \
  -p 'test_*.py' -v
```

Also run:

```text
uv run --project waved/tools/onnx-hybrid \
  pytest -q waved/tools/onnx-hybrid/tests

uv run --project waved/tools/onnx-hybrid \
  ruff check \
    waved/scripts/npu/npu_erf_constant_diff_lab.py \
    waved/scripts/npu/nbg_constant_diff.py \
    waved/scripts/npu/tests/test_npu_erf_constant_diff_lab.py \
    waved/scripts/npu/tests/test_nbg_constant_diff.py

python3 waved/scripts/npu/nbg_constant_diff.py verify-generated
python3 waved/scripts/npu/nbg_image.py verify-generated
uv run --project waved/tools/onnx-hybrid \
  python waved/scripts/npu/nbg_correlate.py verify-generated

git diff --check
```

For new untracked files, separately verify:

- final newline;
- no trailing whitespace;
- no placeholders;
- local Markdown links resolve;
- JSON is canonical;
- safety counters are zero.

## 20. Completion checklist

- [ ] Approved design remains satisfied.
- [ ] Offline build-container policy is exact.
- [ ] Strict manifest passes.
- [ ] Native source and ELF policies pass.
- [ ] Probe hash is frozen before staging.
- [ ] All host tests pass before device mutation.
- [ ] Preflight passes.
- [ ] Stage root is exclusive.
- [ ] A1 runs at most once.
- [ ] A2 runs at most once.
- [ ] A1/A2 determinism is exact.
- [ ] B runs only after the determinism gate.
- [ ] No more than three NBGs are created.
- [ ] Every NBG passes strict parsing, checksums, ownership, and round trip.
- [ ] Postflight passes.
- [ ] Exact stage cleanup is confirmed.
- [ ] Differential classification is deterministic.
- [ ] Integrity collateral is explained or explicitly unresolved.
- [ ] PoC 12C readiness is explicit.
- [ ] No NBG is loaded or executed.
- [ ] Every prohibited-action counter is zero.
- [ ] Tracked evidence verifies as current.
- [ ] Complete regressions pass.
- [ ] Extensive documentation is published.

## 21. Stop conditions

Stop and report rather than improvise after:

- unavailable pinned builder image;
- build or policy failure;
- source edit after hash freeze;
- device ambiguity;
- firmware hash drift;
- holder or health anomaly;
- stage collision;
- timeout;
- malformed native result;
- invalid NBG;
- A1/A2 nondeterminism;
- postflight or cleanup failure.

An allowed clean negative result is not a reason to try PReLU, another
constant, another operation, a retry, or graph execution in this PoC.
