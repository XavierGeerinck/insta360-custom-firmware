# Wave NPU PoC 12B — Controlled Erf Constant Differential

**Date:** 2026-07-29

**Status:** Approved design; implementation not started

**Parent result:** PoC 12A exact NBG serializer recovery

**Safety class:** Bounded compile-only device experiment followed by
host-only binary differential analysis

## 1. Purpose

PoC 12A established:

- exact identities for 21 Wave serializer/load targets;
- total byte ownership for five exact VPMN v2 graphs;
- byte-identical unchanged reconstruction;
- explicit semantic-mutation refusal;
- a finite ONNX/NBG coefficient search with no exact weight or bias
  occurrence.

The missing input is a controlled producer differential:

```text
same topology
same target
same compiler/runtime build
same tensor shape and quantization
one known constant change
two exact output NBGs
```

PoC 12B will obtain that input from the exact connected Wave using the
already proven firmware Erf export path. It will compile two identical
controls and one graph whose constant differs by one byte. It will not load
or execute any generated graph.

The objective is not to build a mutable serializer. The objective is to
determine whether one known source-constant change has a unique,
deterministic representation in a Wave-targeted NBG and to explain every
collateral byte change.

## 2. Connected-device baseline

Read-only discovery on 2026-07-29 established:

```text
ADB state:        device
ADB serial:       PSP_0000000000000000
ADB product:      occam
ADB model:        Nexus_4
OS:               Buildroot
machine:          aarch64
kernel:           5.4.242-ab575
/dev/galcore:     character device, major 199
galcore holders:  none observed
```

The Android-looking ADB identity is not treated as hardware identity. The
probe must independently query the exact Vivante chip/customer identity
through the already recovered `gcoHAL_QueryChipIdentityEx` ABI before graph
construction.

The live device baseline is an implementation precondition, not a value to
silently update. Any divergence from the existing Wave manifests blocks the
native phase until reviewed.

## 3. Hypothesis and falsifiers

### 3.1 Primary hypothesis

For a constant-input U8 Erf graph, changing one constant byte while holding
the entire graph definition and exact producer stack fixed will cause a
deterministic, bounded NBG difference attributable to the serialized
constant and its integrity metadata.

### 3.2 Determinism hypothesis

Two independent compile invocations using the same graph and constant will
emit byte-identical NBG files.

### 3.3 Falsifiers

The useful-pair hypothesis is falsified or remains unresolved if:

- either identical control does not compile;
- the two identical controls differ by any byte;
- the changed-constant graph does not compile;
- the A and B outputs are identical because the source constant is folded or
  elided;
- topology, section directory, I/O metadata, operation count, layer count,
  target, or version changes;
- changed payload ranges cannot be uniquely separated from nondeterministic
  producer state;
- the changed bytes occur only in unresolved command/state regions with no
  source-constant correspondence;
- a device health invariant changes;
- any output or dependency hash is not exact.

A clean falsifier is a valid PoC result. It does not authorize a larger
operation sweep.

## 4. Scope

### 4.1 Included

- one fixed repository-controlled AArch64 probe;
- one already proven operation, U8 Erf;
- exactly three compile-only invocations;
- two identical A controls and one one-byte B change;
- at most three NBG output files;
- strict host/device manifests and hash checks;
- preflight and postflight device-health evidence;
- exact VPMN v2 parsing and checksum validation;
- A1/A2 determinism comparison;
- A1/B range and section differential;
- finite source-byte and expected-output correlation;
- in-memory reconstruction of the already observed B bytes from A when all
  changes are fully explained;
- machine-readable evidence and an extensive lab notebook.

### 4.2 Excluded

- graph execution;
- tensor upload or readback;
- NBG import;
- direct `/dev/galcore` open or ioctl by the probe;
- raw command capture or replay;
- hand-written command, patch, or relocation data;
- persistent semantic NBG mutation;
- a general NBG serializer;
- PReLU, Multiply, Add, Conv, MatrixMul, or any operation sweep;
- the quarantined version-adjacent `libCLC.so`;
- simulator or container execution;
- replacing firmware libraries;
- changing kernel modules or galcore parameters;
- reset, reboot, service stop, or privileged recovery;
- noise-cancellation inference.

## 5. Approaches considered

### 5.1 Constant-input Erf differential — selected

Advantages:

- Erf already emitted a valid target-`0x1000000e` NBG through the exact Wave
  stack;
- the experiment changes one source byte;
- no new shader compiler is required by the proven path;
- three outputs establish both determinism and differential behavior;
- compile-only behavior is already supported by a reviewed harness pattern.

Risk:

- the producer may constant-fold or elide the source tensor.

That risk is measurable and yields the explicit
`constant-folded-or-elided` classification.

### 5.2 Runtime input plus constant PReLU — rejected for this PoC

PReLU would resemble a learned-weight operation more closely. Its available
implementations include source-format EVIS kernels, however, and may enter
the known missing-CLC path. Introducing another operation would confound
producer-path recovery with coefficient recovery.

### 5.3 Static-only serializer analysis — retained as supporting evidence

The exact `SaveWeightAndBias` and coefficient-cache functions remain useful
for interpreting a differential. Static analysis alone cannot uniquely
locate a source constant without a controlled pair, so it is not the primary
PoC.

## 6. Fixed graph definitions

Every native mode constructs this exact graph:

```text
CONSTANT UINT8 [4]
  quantization: affine asymmetric
  scale:        1.0
  zero point:   0
        |
        v
       Erf
        |
        v
OUTPUT UINT8 [4]
  quantization: affine asymmetric
  scale:        1.0
  zero point:   0
```

There is no runtime input tensor.

### 6.1 A1

```text
mode:      --a1
constant:  [0, 1, 2, 3]
```

### 6.2 A2

```text
mode:      --a2
constant:  [0, 1, 2, 3]
```

A2 is a separate process invocation and separate graph/context
construction. It is not a copy of A1.

### 6.3 B

```text
mode:      --b
constant:  [0, 1, 2, 4]
```

Only the fourth source byte changes:

```text
offset: 3
old:    0x03
new:    0x04
```

### 6.4 Folded-output control

At scale 1 and zero point 0, both constants have the same expected
quantized Erf output:

```text
A expected output: [0, 1, 1, 1]
B expected output: [0, 1, 1, 1]
```

This is intentional. If A and B NBGs differ, the difference cannot be
explained merely by a different folded numerical result. If the compiler
fully folds and elides the source constant, identical A/B NBGs are an
expected clean negative.

The host analyzer computes this oracle independently and records its exact
rounding rule. The native probe does not execute the graph to confirm the
oracle.

## 7. Architecture

PoC 12B has four isolated components.

### 7.1 Fixed native probe

Proposed source:

```text
waved/tools/npu-compiler/tim_vx_compile_erf_constant_diff.cc
```

Responsibilities:

- expose `--describe`, `--a1`, `--a2`, and `--b` only;
- reject every other argv shape;
- reject simulator and loader-interposition environment variables;
- require one exact `VIVANTE_SDK_DIR`;
- establish a native watchdog;
- set `PR_SET_PDEATHSIG`;
- query and validate chip identity;
- construct exactly one fixed graph;
- call `CompileToBinary` for size and bytes;
- validate magic, version, target, and bounds;
- write exactly one exclusive output;
- emit one machine-readable result line;
- never execute, import, or load the graph.

### 7.2 Reproducible build and staging policy

Proposed files:

```text
waved/tools/npu-compiler/build-device-erf-constant-diff.sh
waved/tools/npu-compiler/verify-device-erf-constant-diff-policy.sh
waved/tools/npu-compiler/stage-device-erf-constant-diff.sh
```

The build reuses the pinned AArch64 toolchain and exact external ovxlib
configuration already proven for the Erf compile path.

The policy checker binds:

- source SHA-256;
- compiler identity;
- produced ELF SHA-256;
- ELF class, machine, and linkage;
- allowed imports;
- forbidden imports and strings;
- accepted argv literals;
- fixed output paths;
- watchdog and output-size constants;
- absence of execution APIs and direct driver APIs.

Staging creates one exact temporary root and refuses pre-existing content.

### 7.3 Strict host harness

Proposed files:

```text
waved/scripts/npu/erf_constant_diff_manifest.json
waved/scripts/npu/npu_erf_constant_diff_lab.py
```

Responsibilities:

- reject duplicate or unknown manifest fields;
- hash every local and remote dependency;
- verify connected-device identity and health;
- require an empty exclusive staging root;
- stage only manifest-listed files;
- run exactly A1, A2, and B, once each and in that order;
- stop on the first failure;
- pull and hash each result;
- run the strict NBG parser before accepting an artifact;
- record stdout, stderr, duration, exit, and health evidence;
- remove only the exact temporary staging root after successful evidence
  capture or a bounded failure cleanup;
- never accept dynamic native argv or arbitrary remote commands.

### 7.4 Host differential analyzer

Proposed files:

```text
waved/scripts/npu/nbg_constant_diff.py
waved/scripts/npu/tests/test_nbg_constant_diff.py
```

The analyzer imports the existing strict NBG parser and immutable image. It
does not import a vendor library and does not call ADB.

Responsibilities:

- validate the three raw-artifact hashes from one successful run;
- prove A1/A2 byte identity;
- compare structural signatures;
- enumerate every A1/B changed byte and contiguous run;
- associate runs with exclusive byte owners and section descriptors;
- explain verified section-checksum changes;
- search only finite exact representations of the known source constants;
- classify the result;
- optionally reconstruct the exact observed B bytes in memory;
- emit deterministic, payload-free JSON evidence.

## 8. Data flow

```text
reviewed C++ source
        |
        v
pinned AArch64 build
        |
        v
source/ELF policy gate
        |
        v
exact temporary staging root
        |
        +--> compile A1 --> validate/pull/hash
        |
        +--> compile A2 --> validate/pull/hash
        |
        +--> A1 == A2 gate
        |
        +--> compile B  --> validate/pull/hash
        |
        v
postflight + exact cleanup
        |
        v
strict host VPMN parsing
        |
        v
owned-range A1/B differential
        |
        v
classification + evidence + notebook
```

B may be compiled before the host A1/A2 comparison only if the fixed device
harness cannot interleave pull/compare safely. In that case, B remains
uninterpreted unless the later A1/A2 gate passes. The preferred sequence
pulls and checks A1/A2 before B.

## 9. Device paths and output limits

Dedicated root:

```text
/tmp/wave-npu-poc12b/
```

Proposed layout:

```text
/tmp/wave-npu-poc12b/
  bin/wave-npu-compile-erf-constant-diff
  sdk/
  output/
    erf-constant-a1.nbg
    erf-constant-a2.nbg
    erf-constant-b.nbg
```

Each output:

- is created through a same-directory partial file;
- uses `O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW`;
- is bounded to 64 MiB;
- is flushed and fsynced;
- receives mode `0444`;
- is atomically renamed to the final path;
- is never overwritten.

The probe may unlink only its current partial file after its own interrupted
write. It may not unlink a final output.

Total native output count is capped at three. No diagnostic mode may produce
another NBG.

## 10. Preflight

The host refuses native work unless all required checks pass.

### 10.1 Transport and host inputs

- exactly one authorized ADB device;
- expected serial;
- host source, probe, builder, manifest, and runtime hashes;
- no symlink or path escape;
- no pre-existing raw run directory;
- no mutable artifact selection.

### 10.2 Device identity

- Buildroot/aarch64 baseline;
- exact kernel identity;
- `/dev/galcore` type and major;
- exact required firmware-library hashes;
- exact probe hash after staging;
- expected chip model `0x00009000`;
- expected customer ID `0x1000000e`;
- captured revision and ECO values agree with the existing manifest.

### 10.3 Health

- ADB responsive;
- no unexpected `/dev/galcore` holder;
- no existing PoC staging root;
- sufficient `/tmp` space for staging plus three maximum bounded outputs;
- captured galcore parameters readable;
- no new kernel alert in the bounded preflight window.

The probe repeats chip identity internally so a host parsing error cannot
authorize graph construction.

## 11. Native execution contract

Each of the three invocations:

- runs in a fresh process;
- receives one fixed mode;
- receives only the allowlisted environment;
- has a 15-second native alarm;
- has a tracked host timeout;
- performs exactly two `CompileToBinary` calls: size query and byte output;
- creates at most one final NBG;
- never retries;
- returns one result classification.

Expected success line:

```text
RESULT status=PASS mode=compile-only candidate=A1 \
bytes=N version=0x00020000 product=0x1000000e \
output=/tmp/wave-npu-poc12b/output/erf-constant-a1.nbg
```

Equivalent fixed candidate/path pairs exist for A2 and B.

The result parser rejects:

- missing or duplicate result lines;
- unknown fields;
- wrong mode or candidate;
- unexpected path;
- invalid integer;
- output outside bounds;
- wrong version or product;
- success with nonzero native exit;
- a partial file remaining after success.

## 12. Watchdog and failure behavior

### 12.1 Native watchdog

The native alarm:

- covers context creation, identity query, graph construction, compilation,
  validation, and output write;
- removes only the active partial output;
- emits a bounded timeout result;
- exits with code 124;
- cannot reset the driver or kill another process.

### 12.2 Parent loss

`PR_SET_PDEATHSIG` is set to `SIGKILL`. The probe checks that its parent has
not already become PID 1.

### 12.3 Host timeout

If the host boundary expires:

- the local ADB client is terminated;
- no remote kill or driver reset is attempted;
- the run is classified `native-timeout`;
- a bounded holder and health snapshot is attempted;
- no later candidate runs;
- cleanup is attempted only if no active holder is observed.

### 12.4 Stop-on-failure

The ladder stops immediately after:

- a preflight failure;
- A1 failure;
- A2 failure;
- A1/A2 nondeterminism;
- B failure;
- malformed or foreign-target output;
- postflight health failure;
- cleanup failure.

There is no automatic alternate operation or relaxed retry.

## 13. Safety policy

### 13.1 Allowed operations

```text
hash reviewed files
build one fixed AArch64 probe
read device identity and health
create one exact temporary staging directory
push exact manifest-listed files
invoke the fixed probe three times
compile fixed graphs to bounded NBG bytes
pull exact output files
parse and hash NBG files on the host
remove the exact temporary staging directory
write ignored raw evidence
write tracked JSON and Markdown evidence
```

### 13.2 Prohibited-action counters

Every evidence envelope records:

```text
graph_execution_calls:       0
nbg_load_calls:              0
tensor_upload_calls:         0
tensor_readback_calls:       0
direct_galcore_opens:        0
raw_ioctl_calls:             0
driver_resets:               0
device_reboots:              0
service_stop_calls:          0
compiler_retries:            0
alternate_operation_calls:   0
quarantined_clc_calls:       0
simulator_calls:             0
container_runs:              0
persistent_mutated_nbgs:     0
```

Any nonzero prohibited-action counter invalidates the PoC.

### 13.3 Environment allowlist

Only the exact runtime variables already required by the proven compile path
may be set. At minimum:

```text
LD_LIBRARY_PATH
VIVANTE_SDK_DIR
```

The following must be absent:

```text
VSIMULATOR_CONFIG
LD_PRELOAD
LD_AUDIT
```

No host or complete device environment is captured in evidence.

## 14. Raw evidence envelope

Raw run outputs remain ignored under:

```text
artifacts/npu/runs/POC12B_RUN_ID/
```

Proposed contents:

```text
definition.json
preflight.json
a1.stdout
a1.stderr
a2.stdout
a2.stderr
b.stdout
b.stderr
postflight.json
cleanup.json
erf-constant-a1.nbg
erf-constant-a2.nbg
erf-constant-b.nbg
run-envelope.json
```

Each file has a recorded byte count and SHA-256. NBG payloads are not copied
into tracked JSON or Markdown.

Raw evidence directories are exclusive and never overwritten.

## 15. Structural admission

Each generated NBG must pass:

- regular-file and size bounds;
- `VPMN` magic;
- exact version `0x00020000`;
- exact target `0x1000000e`;
- declared-size equality;
- strict sub-header and section-directory checks;
- nonoverlapping sections;
- public entry-width rules;
- all stored per-section checksum checks;
- complete exclusive byte ownership;
- unchanged byte-identical round trip.

The expected I/O profile is:

```text
inputs:   0
outputs:  1 x UINT8 [4], scale 1, zero point 0
```

If the producer exposes a constant tensor as an input descriptor, the
structural result may instead contain one input. That is recorded as a
producer result, but A1/A2/B must all agree exactly. The implementation must
not hard-code an unsafe parser exception merely to force the expected count.

## 16. Differential algorithm

### 16.1 Load and bind

The analyzer accepts one strict manifest that binds:

- the exact raw run envelope;
- A1, A2, and B paths;
- byte counts and SHA-256 values;
- graph definitions and source constants;
- expected target and version;
- admitted section types;
- output evidence path;
- policy and prohibited-action counters.

Unknown keys, duplicate keys, path escapes, symlinks, missing files, or hash
drift fail.

### 16.2 Determinism gate

Compare A1 and A2:

1. byte length;
2. SHA-256;
3. every byte;
4. first disagreement;
5. parsed structural signature;
6. every section descriptor and section hash.

Acceptance requires exact full-file equality. Structural equality alone is
not sufficient.

### 16.3 Structural A/B gate

Before interpreting payload differences, require equality of:

- magic, version, target, and format class;
- total file size unless an exact coefficient-size rule explains it;
- section count and section type order;
- section header widths;
- section entry widths;
- section alignments;
- input/output metadata;
- network name policy;
- layer and operation counts;
- core and address-width metadata;
- memory and SRAM requirements, unless the constant representation itself
  deterministically changes one requirement.

Any permitted exception must be evidence-classed and cannot be silently
normalized.

### 16.4 Byte-run enumeration

Enumerate:

- every differing byte offset;
- old and new byte;
- maximal contiguous differing runs;
- owning range;
- section index, type, relative offset, and entry index when applicable;
- original range SHA-256 values;
- whether the run lies in decoded metadata, alignment, padding, opaque
  payload, or a non-owning view.

The analyzer never prints complete opaque sections.

### 16.5 Integrity collateral

For each changed section:

- recompute standard section CRC-32;
- identify the exact checksum record;
- verify whether its stored value changed from the A section CRC to the B
  section CRC;
- classify those four checksum bytes as `explained-integrity-collateral`.

The unresolved header `crc_sum` is recorded separately. Its change may be
classified as `producer-integrity-collateral`, but it is not declared
decoded unless this pair uniquely establishes and a second control
corroborates its algorithm.

### 16.6 Constant candidates

Finite source representations:

```text
raw A bytes: 00 01 02 03
raw B bytes: 00 01 02 04
changed byte: 03 -> 04
```

Finite folded-output representation:

```text
A output: 00 01 01 01
B output: 00 01 01 01
```

The analyzer may additionally test exact fixed representations already
admitted by PoC 12A only when meaningful for U8:

- direct bytes;
- fixed block permutations of four bytes;
- byte-lane permutations of four bytes;
- exact zero padding to a recorded alignment.

It does not use fuzzy search, compression guessing, XOR-key search, arbitrary
permutations, or unbounded transforms.

### 16.7 In-memory observed-B reconstruction

If every A/B changed run is classified, the analyzer may:

1. copy A into memory;
2. apply only the exact observed source-mapping changes;
3. recompute only fully proved integrity fields;
4. compare the resulting bytes with the already observed B;
5. discard the buffer.

It must not:

- write the reconstructed buffer as an NBG;
- invent a novel constant;
- use B's opaque changed bytes as a generic patch recipe;
- describe equality with B as a general serializer.

## 17. Result classifications

Exactly one top-level classification is emitted.

### 17.1 `direct-constant-mapping`

Requirements:

- A1 equals A2;
- source delta occurs directly and uniquely in an admitted constant or
  coefficient range;
- all other changes are explained integrity metadata;
- exact in-memory observed-B reconstruction passes.

### 17.2 `deterministic-transformed-mapping`

Requirements:

- A1 equals A2;
- one finite, named, invertible transform uniquely maps A and B constants;
- the transform is bounded and recorded;
- all collateral changes are explained;
- exact in-memory observed-B reconstruction passes.

This result remains preliminary until a later differential with a richer
constant corroborates the transform.

### 17.3 `constant-folded-or-elided`

Requirements:

- A1 equals A2;
- A and B graph definitions differ only in the source constant;
- A and B NBGs are identical, or no source-dependent range is present;
- expected folded numerical outputs are identical.

### 17.4 `serializer-nondeterministic`

A1 and A2 differ by any byte.

No A/B coefficient inference is permitted.

### 17.5 `multi-region-unresolved`

A1 equals A2, A differs from B, but changed runs cannot be uniquely
attributed under the finite registry.

### 17.6 `compile-blocked`

One fixed graph cannot pass the compile-only boundary. The exact stage and
diagnostic are retained.

### 17.7 `device-health-failure`

Native work occurred but a required postflight or cleanup invariant failed.
No further native work is permitted until manual review.

## 18. PoC 12C readiness

PoC 12C is `ready-limited` only when:

- the result is `direct-constant-mapping` or
  `deterministic-transformed-mapping`;
- A1/A2 are exactly deterministic;
- one source-dependent range is unique;
- section-integrity collateral is completely explained;
- no topology, command, patch, relocation, or load-state ambiguity remains
  for this exact change;
- every device health and cleanup gate passes;
- every prohibited-action counter is zero.

Even `ready-limited` authorizes only a second, richer constant differential.
It does not authorize a novel executable graph.

PoC 12C remains `not-ready` for all other classifications.

## 19. Testing

### 19.1 Native source-policy tests

- exact accepted argv;
- unknown/missing/extra argv refusal;
- exact output paths;
- fixed constants;
- graph-execution API absence;
- direct-driver API absence;
- watchdog and parent-death behavior present;
- output-exclusive flags present;
- 64 MiB bound present;
- simulator/interposition rejection present;
- no quarantined CLC reference.

### 19.2 Manifest tests

- exact schema;
- duplicate and unknown key rejection;
- role and candidate uniqueness;
- path and symlink escape refusal;
- hash and byte-count drift;
- exact three-output cap;
- nonzero prohibited-action refusal;
- immutable graph definition;
- fixed run order.

### 19.3 Harness tests with fake transport

- preflight success;
- device unavailable;
- wrong serial;
- wrong kernel or firmware hash;
- unexpected holder;
- existing stage root;
- push hash mismatch;
- A1 success;
- A1 failure stops A2/B;
- A2 failure stops B;
- A1/A2 mismatch stops B;
- B failure;
- timeout;
- malformed result line;
- malformed NBG;
- pull hash mismatch;
- postflight health failure;
- cleanup success and failure;
- no arbitrary remote argv;
- exactly three maximum native calls.

### 19.4 Differential unit tests

- direct one-byte constant match;
- named fixed permutation match;
- identical A/B folded classification;
- A1/A2 nondeterminism;
- changed section with correct checksum collateral;
- wrong checksum change;
- header-only difference;
- changed topology;
- changed section directory;
- ambiguous duplicate constant occurrence;
- multiple unexplained payload regions;
- alignment and padding preservation;
- unknown section ownership;
- exact in-memory observed-B reconstruction;
- reconstruction mismatch;
- payload-free canonical evidence;
- stale evidence rejection.

### 19.5 Regression tests

- PoC 12A serializer-manifest tests;
- NBG parser tests;
- immutable image tests;
- correlation tests;
- existing compiler harness tests;
- generated-evidence verification;
- Python syntax checks;
- C++ build and policy checks;
- shell syntax checks;
- `git diff --check`.

## 20. Evidence outputs

Tracked:

```text
docs/npu/format/erf-constant-differential.json
docs/npu/lab-notebook/2026-07-29-poc12b-erf-constant-differential.md
```

Existing documents updated:

```text
docs/npu/README.md
docs/npu/format/README.md
docs/npu/format/field-map.md
docs/npu/lab-notebook/README.md
docs/npu/reproduction.md
waved/scripts/npu/README.md
waved/docs/COMPUTE_HARDWARE.md
```

Raw ignored evidence:

```text
artifacts/npu/runs/POC12B_RUN_ID/
```

The tracked JSON contains:

- source and implementation identities;
- raw artifact hashes and sizes;
- device pre/post/cleanup evidence summaries;
- parsed graph metadata;
- A1/A2 exact comparison;
- A1/B changed-run ledger;
- integrity-collateral ledger;
- finite transform registry;
- classification and PoC 12C readiness;
- prohibited-action counters.

It does not contain NBG payload bytes, firmware-library bytes, device
addresses, tensor contents beyond the four designed source/oracle bytes, or
unbounded logs.

## 21. Acceptance criteria

PoC 12B is complete when:

1. the fixed probe and harness policies are test-proven;
2. every local and remote input is hash-pinned;
3. preflight passes before native work;
4. no more than three fixed compile-only invocations occur;
5. every produced file passes strict VPMN validation;
6. A1/A2 determinism is decided exactly;
7. B is interpreted only if A1/A2 are identical;
8. every A/B change is recorded with byte ownership and provenance;
9. every verified section-checksum change is explained;
10. one valid top-level classification is emitted;
11. PoC 12C readiness is explicit;
12. postflight and cleanup are complete;
13. every prohibited-action counter is zero;
14. generated evidence verifies as current;
15. focused and neighboring regression suites pass;
16. documentation separates constant mapping from a general serializer;
17. no generated graph is loaded or executed;
18. no persistent semantically mutated NBG is written.

The PoC may pass with a negative classification. A useful coefficient
mapping is not required for the experiment itself to be valid.

## 22. Stop conditions requiring manual review

Stop without another native attempt if:

- ADB identity is ambiguous;
- more than one device is attached;
- exact firmware hashes drift;
- the NPU is unexpectedly held;
- the stage root already exists;
- a native watchdog or host timeout fires;
- a probe exits without a valid result;
- a partial file cannot be removed safely;
- A1 and A2 differ;
- a produced graph targets anything other than `0x1000000e`;
- parser or checksum validation fails;
- the kernel reports a new NPU/IOMMU/fault/reset alert;
- a galcore parameter changes;
- cleanup cannot be confirmed.

Recovery is observation and cleanup only. No reset, reboot, service stop,
kill of an unrelated process, or driver mutation is allowed.

## 23. Non-claims

PoC 12B must not be described as:

- an open Acuity compiler;
- a general Wave NBG serializer;
- arbitrary constant mutation;
- decoded coefficient packing for Conv or MatrixMul;
- a decoded NN/TP instruction set;
- command or relocation emission;
- ONNX-to-NBG support;
- an executable custom model;
- NPU noise cancellation;
- proof that all operations are deterministic.

The maximum positive claim is:

> For one fixed U8 constant-input Erf graph produced by one exact Wave
> firmware stack, a one-byte source-constant change has a deterministic and
> fully evidence-classed NBG representation.

That sentence is permitted only if the result satisfies the corresponding
classification gates.

## 24. Implementation sequence

The implementation plan must preserve this order:

1. strict graph/manifest/differential unit contracts;
2. fixed native probe;
3. build and static source/ELF policy;
4. fake-transport host harness;
5. host-only analyzer and fixtures;
6. all host regressions;
7. read-only live preflight;
8. explicit review of hashes and staged argv;
9. A1 compile/pull/validate;
10. A2 compile/pull/validate;
11. exact A1/A2 gate;
12. B compile/pull/validate;
13. postflight and exact cleanup;
14. host differential analysis;
15. evidence generation and verification;
16. extensive notebook and authoritative-document updates.

No implementation convenience may reorder B before the determinism gate
unless the raw outputs remain uninterpreted and the reason is documented.
