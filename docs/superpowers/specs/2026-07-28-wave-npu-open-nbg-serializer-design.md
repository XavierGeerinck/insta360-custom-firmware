# Wave NPU Open NBG Serializer Recovery Design

**Date:** 2026-07-28

**PoC:** 12A

**Status:** Approved for specification; implementation requires review of
this written document

**Safety class:** host-only static analysis and byte-identical re-emission

## 1. Executive summary

The proprietary Vivante no-kernel SDK is not obtainable for this project.
PoC 12A therefore begins an SDK-independent compiler-recovery route by
recovering the Network Binary Graph serializer already present in the exact
Insta360 Wave firmware.

The exact Wave `libOpenVX.so` exports a large, named NBG generation surface,
including:

```text
vxGenerateNBG
vxoBinaryGraph_SaveNetworkBinaryGraph
vxoBinaryGraph_SaveBinaryEntrance
vxoBinaryGraph_SaveNNTPCommand
vxoBinaryGraph_SaveNNTPInstruction
vxoBinaryGraph_SaveWeightAndBias
vxoBinaryGraph_SetPatchDataInfo
vxoBinaryGraph_RelocateNetworkBinaryGraph
vxoBinaryGraph_LoadNBG
```

This provides a stronger primary specification than blind comparison of NBG
files. PoC 12A will recover the bounded generation call graph, extend the
strict NBG representation into a lossless image model, and prove
byte-identical parse/re-emit behavior on exact-target Wave artifacts.

The corpus also contains an unusually valuable source/binary anchor:

```text
ctc.onnx
  MatMul -> Add -> LogSoftmax

ctc_online.nb
  VPMN v2
  target 0x1000000e
  one FP16 [512,16,1] input
  one FP16 [4999,16,1] output
```

The ONNX weights and bias do not appear linearly as direct FP16 byte strings
inside the NBG. The coefficient path therefore includes genuine packing,
tiling, quantization, alignment, or metadata transformation. PoC 12A records
that boundary and builds the evidence needed for a later coefficient-packing
PoC; it does not guess or emit transformed coefficients.

No compiler, simulator, container, graph, driver, or device call is allowed
in PoC 12A.

## 2. Problem statement

Earlier work established:

- the Wave executes stock target-`0x1000000e` NBGs;
- repository code generated and executed one exact-target U8 Erf NBG;
- the NBG v1/v2 envelope, section directory, tensor metadata, memory
  metadata, and stored section CRC-32 records are strictly parsed;
- ordinary Add, MatrixMul, and Conv graphs cannot currently be exported by
  the exact device stack;
- the acquired public Acuity package contains the exact Wave profile, but its
  graph path uses an older simulator SDK;
- the required proprietary no-kernel graph compiler cannot be treated as a
  future dependency.

The current parser preserves opaque sections by fingerprint, but it does not:

- represent every file byte as an owned range;
- re-emit an unchanged NBG;
- decode serializer ownership and ordering;
- map command, coefficient, patch, state, and connection bytes back to exact
  generator functions;
- correlate source ONNX initializers with packed NBG coefficient bytes.

Without those contracts, custom NBG generation would be guesswork.

## 3. Goals

PoC 12A has five goals.

### 3.1 Recover the exact serializer call graph

Statically recover the generation path from `vsi_nn_GenerateNBG` through
`vxGenerateNBG` and the named `vxoBinaryGraph_*` serializer functions in the
exact firmware.

Every target function is bound to:

- source ELF path;
- ELF SHA-256 and byte count;
- virtual address;
- function byte count and SHA-256;
- direct callers and callees;
- referenced diagnostic and section strings;
- bounded decompilation evidence.

### 3.2 Model every byte

Represent every byte of a VPMN v2 NBG as exactly one of:

```text
decoded-record
alignment
padding
preserved-opaque
connection-tail
```

Overlapping, unowned, multiply owned, or out-of-bounds bytes are errors.

### 3.3 Prove lossless re-emission

Parse and re-emit unchanged NBG files with:

```text
output bytes == input bytes
output SHA-256 == input SHA-256
```

Semantic equivalence, equal parsed metadata, or equal section checksums is
not sufficient.

### 3.4 Establish coefficient-packing evidence

Compare the English CTC ONNX initializers with the exact Wave coefficient,
table, command, patch, and state sections. Record direct, transformed,
differential, inferred, and unresolved relationships without emitting a new
coefficient layout.

### 3.5 Define the next safe boundary

Produce enough evidence to decide whether PoC 12B may recover coefficient
packing as a deterministic, invertible transform. PoC 12A does not authorize
semantic reconstruction or device execution.

## 4. Non-goals

PoC 12A does not:

- generate a custom NBG;
- modify an existing NBG;
- patch a target or product identifier;
- rewrite a checksum for changed data;
- execute a compiler or simulator;
- load an Acuity native extension;
- call `vxGenerateNBG` or any other firmware function;
- load an AArch64 library;
- contact ADB or the Wave;
- open `/dev/galcore`;
- replay a driver command;
- decode the complete NN, TP, shader, PPU, or DMA instruction set;
- claim that an ONNX operator maps to one NBG layer;
- claim that the CTC source and NBG were built by the same compiler version
  unless separate provenance proves it;
- implement Conv lowering;
- approve a graph signature for NPU execution.

## 5. Approaches considered

### 5.1 Exact serializer first — selected

Use named generation functions in the exact Wave `libOpenVX.so` as the
primary evidence source. Corroborate recovered structures with the generated
Erf NBG, the English ONNX/NBG pair, and selected stock controls.

Advantages:

- exact target and runtime family;
- named function boundaries rather than inferred offsets;
- serializer and loader can be compared;
- supports a strict, lossless representation before semantic mutation;
- no proprietary compiler execution is required.

Risks:

- AArch64 decompilation still requires manual validation;
- runtime structures may be private and version-specific;
- coefficient and instruction transforms may remain opaque after PoC 12A.

### 5.2 Corpus differential only — rejected as the primary route

Compare stock NBGs and infer changed ranges from topology, shape, language,
and model-size differences.

Advantages:

- broad exact-target corpus;
- no binary decompilation dependency;
- useful corroboration for repeated layouts.

Risks:

- most source ONNX files are unavailable;
- compiler and model changes are confounded;
- equal topology cannot be assumed from filenames;
- correlation alone cannot establish serializer ownership.

This remains a secondary evidence source.

### 5.3 Controlled simulator corpus first — deferred

Execute an isolated simulator compiler repeatedly with tiny graph variants.

Advantages:

- controlled topology, shape, type, and constant changes;
- potentially strong differential signal;
- useful for testing transform hypotheses.

Risks:

- the available simulator emits a foreign target and older NBG version;
- its compiler ABI has not been qualified for execution;
- target-dependent layouts may differ;
- it would broaden PoC 12A beyond static analysis.

This may become a later, separately reviewed control after its compiler
boundary is statically qualified.

### 5.4 Private export-data first — deferred

Recover the `nbgType` 2/3 graph representation accepted by `libnnsdk.so`.

It might bypass precompiled NBG input, but graph verification may still need
the missing kernel compiler. It is retained as a separate research branch,
not mixed into serializer recovery.

## 6. Input corpus

### 6.1 Exact firmware

Required exact-build inputs:

- `firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libOpenVX.so`;
- `firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libovxlib.so`;
- the corresponding generated ABI inventory;
- public TIM-VX/OpenVX headers already pinned in the repository.

The target manifest records exact hashes and rejects any replacement,
symlink escape, architecture mismatch, or version drift.

### 6.2 Minimal exact-target NBG

The generated Erf control:

```text
path:
  artifacts/npu/runs/poc5c-erf-compile-1-20260728/candidate-1.nbg

bytes:
  5840

SHA-256:
  6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee

target:
  0x1000000e
```

This is the smallest available VPMN v2 exact-target graph and the primary
round-trip control.

### 6.3 Source/binary CTC anchor

Source:

```text
models/ai-models/asr-models/model/asr_en/reverb_asr_online/ctc.onnx
SHA-256:
  8d6d3b6ce9da51f852981bb5bdd33e2818d47d3e545e9efe5d3700a37b367219
```

Graph:

```text
hidden [1,16,512] FP32
  -> MatMul(weight [512,4999] FP32)
  -> Add(bias [4999] FP32)
  -> LogSoftmax
  -> probs [1,16,4999] FP32
```

Compiled control:

```text
models/ai-models/asr-models/model/asr_en/reverb_asr_online/ctc_online.nb
SHA-256:
  3a9294ea8595789cc23ddc9298ebceb4474738fed07a9a8b10e2442b67b0d8d5
```

The source/binary relationship is strongly suggested by path, names, shapes,
and dimensions. PoC 12A calls it an anchor, not proven build provenance.

### 6.4 Differential CTC controls

Select a bounded set from German, French, and Japanese online CTC graphs.
The manifest fixes each path and hash. Selection favors:

- the smallest exact-target stock graphs;
- similar layer and absolute-operation counts;
- differing vocabulary/output dimensions;
- known byte-identical duplicate controls;
- no more than eight total NBG files in the initial analysis.

The complete 137-file stock corpus remains available for later validation,
but it is not the first Ghidra/correlation working set.

## 7. Architecture

```text
                 EXACT WAVE FIRMWARE

  libovxlib.so                         libOpenVX.so
  vsi_nn_GenerateNBG                  vxGenerateNBG
          |                                 |
          +---------------+-----------------+
                          |
                          v
             bounded serializer call graph
                          |
                          v
               function/evidence ledger


                  IMMUTABLE NBG CORPUS

  generated Erf   English CTC pair   multilingual controls
          \             |                  /
           +------------+-----------------+
                        |
                        v
              strict section decoder
                        |
                        v
               lossless NBG image model
                        |
             +----------+-----------+
             |                      |
             v                      v
    byte-identical writer    coefficient correlation
             |                      |
             v                      v
      round-trip evidence     relationship ledger
```

### 7.1 Target manifest

`nbg_serializer_targets.json` is the only source of analyzed artifacts and
function targets. It contains:

- schema version;
- PoC ID and safety policy;
- exact ELF and model paths;
- byte counts and SHA-256 values;
- artifact roles;
- permitted static-analysis operations;
- prohibited action counters;
- bounded Ghidra targets;
- selected NBG corpus IDs;
- source/binary anchor metadata;
- canonical output paths.

Unknown keys, duplicate keys, duplicate roles, uncontained paths, mutable
globs, missing hashes, and unreviewed artifact kinds fail.

### 7.2 Serializer evidence generator

`nbg_serializer_recovery.py`:

- validates the target manifest;
- validates ELF64 AArch64 identity and load geometry;
- invokes the existing bounded Ghidra adapter on only the named targets;
- binds function bodies to exact hashes;
- records callers, callees, referenced functions, and diagnostics;
- rejects missing, duplicate, thunk-only, truncated, or overlapping
  functions;
- emits canonical static evidence;
- never imports or executes an analyzed artifact.

Priority target families:

1. generation entry:
   `vsi_nn_GenerateNBG`, `vxGenerateNBG`;
2. serializer orchestration:
   `SaveNetworkBinaryGraph`, `SaveBinaryEntrance`,
   `SaveBinaryEntranceBefore`;
3. section and memory:
   `GenerateSectionMemory`, `SaveWeightAndBias`,
   `UpdateCoefHeaderCache`;
4. command and operation:
   `SaveNNTPOperation`, `SaveNNTPCommand`,
   `SaveNNTPInstruction`, `SaveNNTPStates`;
5. patch and relocation:
   `SetPatchDataInfo`, `Patch32bitVa`, `Patch40bitVa`,
   `RelocateNetworkBinaryGraph`;
6. load-side differential:
   `LoadNBG`, `AllocateBinaryLoadBuffer`, `ReleaseNBG`.

The first implementation plan may divide these into bounded target batches
so one evidence file does not become an unreviewable decompilation dump.

### 7.3 Lossless NBG image

`nbg_image.py` defines an immutable representation with:

- original file identity;
- header and directory;
- ordered section descriptors;
- decoded record views;
- raw bytes for every range;
- gap and ownership ledger;
- connection-tail range;
- original alignment and padding;
- source offsets and lengths;
- change state.

The representation separates observation from emission:

```text
NbgImage.parse(bytes) -> immutable image
NbgImage.to_bytes()    -> allowed only when semantically unchanged
```

Any API that changes a decoded field marks the image changed. PoC 12A's
writer refuses changed images. It is not a general serializer API.

### 7.4 Round-trip verifier

The verifier:

1. hashes the original;
2. parses it under strict bounds;
3. proves complete, exclusive byte ownership;
4. serializes the unchanged image;
5. compares byte length;
6. compares every byte;
7. reports the first disagreement if any;
8. hashes the re-emitted bytes;
9. writes only evidence by default.

An optional output-copy argument may write the identical result to a new,
exclusive host path for manual comparison. It refuses overwrite and removes
an incomplete output after an internal failure.

### 7.5 Correlation analyzer

`nbg_correlate.py` reads:

- bounded ONNX graph and initializer metadata;
- selected NBG image sections;
- section fingerprints;
- related CTC controls.

It may test deterministic, named transforms such as:

- original FP32 bytes;
- FP16 conversion;
- BF16 conversion;
- tensor transpose;
- dimension-major permutations;
- fixed block sizes;
- fixed alignment padding;
- byte/halfword lane permutations.

Every tested transform is bounded by tensor size and operation count.
Combinatorial permutation search is forbidden.

## 8. Evidence classes

Each relationship uses one class:

`direct-match`
: Exact source bytes occur at one unambiguous NBG range.

`transformed-match`
: A deterministic named transform produces an exact NBG range. The transform,
parameters, source tensor, output range, and hashes are recorded.

`differentially-supported`
: Multiple related graphs change or preserve the predicted ranges in a way
consistent with the hypothesis, but an exact reversible transform is not
yet proven.

`inferred`
: Static generator code or corpus behavior supports the relationship, but
another explanation remains plausible.

`unresolved`
: No adequate relationship has been established.

A transformed match requires:

- no unbounded search;
- exact output bytes;
- unique or explicitly enumerated occurrence count;
- complete source and destination provenance;
- corroboration with another tensor, graph, or serializer function;
- an inverse or an explicit explanation of why the transform loses
  information.

## 9. Byte-ownership contract

For an NBG of length `N`, the ownership ledger must cover exactly `[0, N)`.

Each range has:

```text
start
end
length
class
section index or root owner
decoded schema, when any
raw SHA-256
notes
```

The verifier rejects:

- gaps;
- overlaps;
- negative or wrapped ranges;
- a range beyond EOF;
- duplicate section indices;
- duplicate nonempty section ranges;
- misalignment that violates the original directory;
- a connection tail not exactly where the validated format places it;
- decoded-record geometry that does not divide the section;
- a checksum entry that names an absent or duplicate section;
- bytes synthesized without an original source range.

Padding and alignment remain distinct. Padding is an explicit original byte
range. Alignment is a placement relationship. Neither is silently replaced
with zeroes.

## 10. Error model

Stable error categories:

```text
TARGET_MANIFEST_INVALID
TARGET_HASH_MISMATCH
ELF_IDENTITY_MISMATCH
GHIDRA_TARGET_MISSING
GHIDRA_FUNCTION_DRIFT
NBG_PARSE_FAILURE
NBG_RANGE_GAP
NBG_RANGE_OVERLAP
NBG_SECTION_GEOMETRY
NBG_CHECKSUM_MISMATCH
NBG_CONNECTION_TAIL_INVALID
NBG_SEMANTIC_CHANGE_REFUSED
NBG_ROUNDTRIP_LENGTH_MISMATCH
NBG_ROUNDTRIP_BYTE_MISMATCH
ONNX_ADMISSION_FAILURE
CORRELATION_LIMIT_EXCEEDED
CORRELATION_AMBIGUOUS
EVIDENCE_STALE
OUTPUT_EXISTS
INTERNAL_ERROR
```

Every failure names the input artifact and bounded location. Binary data,
complete coefficient payloads, and complete decompilation dumps are not
printed to the terminal.

## 11. Safety policy

The policy requires zero counts for:

```text
compiler_calls
simulator_calls
native_extension_imports
container_registrations
container_runs
adb_calls
device_calls
generated_custom_nbg_files
mutated_nbg_files
```

Allowed:

- read/hash exact files;
- parse ELF and NBG bytes;
- run Ghidra headlessly against copied or read-only inputs;
- parse ONNX through the pinned host environment without inference;
- compute bounded deterministic byte transforms in memory;
- write canonical JSON/Markdown evidence;
- optionally write an unchanged byte-identical NBG copy to a new host path.

Forbidden:

- execute or load analyzed firmware code;
- call a compiler, simulator, OpenVX, TIM-VX, GAL, or NBG generator;
- use Docker or another runtime to register or run the acquired OCI image;
- contact the Wave;
- edit an NBG semantic field;
- emit a custom graph;
- recompute integrity data for changed payloads;
- stage or execute any result.

## 12. Testing strategy

### 12.1 Manifest and artifact tests

- duplicate JSON keys;
- missing and unknown keys;
- invalid IDs and enums;
- path and symlink escapes;
- missing files;
- byte-count and SHA-256 drift;
- duplicate roles;
- wrong ELF class, endianness, machine, or program geometry;
- unapproved function targets.

### 12.2 NBG image tests

- exact Erf round trip;
- exact English CTC round trip;
- all selected multilingual control round trips;
- deterministic parse/emission;
- truncation at every structural boundary;
- overlapping and duplicate sections;
- section outside EOF;
- integer overflow;
- invalid record stride;
- unknown section preservation;
- nonzero padding preservation;
- connection-tail preservation;
- one-byte corruption detection;
- semantic-change refusal;
- output non-overwrite.

### 12.3 Serializer evidence tests

- exact target set;
- exact function hashes;
- caller/callee reference validation;
- missing function refusal;
- probable thunk distinction;
- canonical evidence generation;
- stale evidence rejection;
- temporary Ghidra project cleanup.

### 12.4 Correlation tests

- direct unique match;
- duplicate-match ambiguity;
- deterministic FP16/BF16 conversion;
- transpose and fixed permutation fixtures;
- transform-size and candidate-count limits;
- false-positive rejection;
- source tensor and destination range provenance;
- negative control with unrelated bytes;
- canonical relationship ledger.

### 12.5 Regression tests

- existing NBG parser tests;
- complete configured NBG corpus verification;
- architecture/profile tests;
- compiler acquisition tests;
- ONNX admission and CPU fallback tests;
- `git diff --check`;
- Python syntax checks;
- generated-evidence verification.

## 13. Outputs

Planned tracked outputs:

```text
waved/scripts/npu/nbg_serializer_targets.json
waved/scripts/npu/nbg_serializer_recovery.py
waved/scripts/npu/nbg_image.py
waved/scripts/npu/nbg_correlate.py
waved/scripts/npu/tests/test_nbg_serializer_recovery.py
waved/scripts/npu/tests/test_nbg_image.py
waved/scripts/npu/tests/test_nbg_correlate.py

docs/npu/format/serializer-ghidra-evidence.json
docs/npu/format/serializer-byte-ownership.json
docs/npu/format/serializer-roundtrip-evidence.json
docs/npu/format/onnx-nbg-correlation.json
docs/npu/lab-notebook/2026-07-28-poc12a-open-nbg-serializer.md
```

Existing documents updated by the implementation:

```text
docs/npu/format/README.md
docs/npu/format/field-map.md
docs/npu/lab-notebook/README.md
docs/npu/README.md
docs/npu/reproduction.md
waved/scripts/npu/README.md
waved/docs/COMPUTE_HARDWARE.md
```

Large generated or copied NBG data remains under `artifacts/npu/` and is not
tracked.

## 14. Acceptance criteria

PoC 12A passes only when:

1. all inputs are exact and hash-pinned;
2. serializer entry, orchestration, coefficient, command, patch, relocation,
   and load-side functions have bounded static evidence;
3. every byte of every selected NBG has exactly one ownership
   classification;
4. the generated Erf NBG round-trips byte-for-byte;
5. the English CTC NBG round-trips byte-for-byte;
6. every selected multilingual control round-trips byte-for-byte;
7. all output hashes equal their input hashes;
8. semantic changes are test-proven to be refused;
9. the ONNX/NBG coefficient relationship ledger is deterministic and
   evidence-classed;
10. direct plain-FP16 non-matches remain recorded rather than normalized
    away;
11. every prohibited-action counter is zero;
12. focused and existing regression suites pass;
13. generated evidence verifies as current;
14. documentation clearly separates lossless re-emission from custom graph
    generation.

A coefficient transform need not be fully recovered for PoC 12A to pass.
Failure to recover it is a valid `unresolved` result provided all bounded
tests and evidence are retained.

## 15. Exit decision and later PoCs

PoC 12A establishes:

```text
exact serializer call graph
+ total byte ownership
+ byte-identical unchanged re-emission
+ bounded coefficient evidence
```

It does not establish:

```text
custom NBG generation
safe semantic mutation
instruction generation
Conv lowering
device authorization
```

Later steps remain independently gated:

### PoC 12B — Coefficient packing

Recover deterministic transforms for constants, coefficient tables, cache
alignment, and relevant patch records. Require exact reconstruction across
multiple tensors or graphs.

### PoC 12C — Operation, command, and relocation emission

Recover one minimal operation family and its command/patch generation. Start
with an operation for which exact device generation already exists, not
Conv.

### PoC 12D — Fixed Conv static construction

Construct one immutable fixed Conv candidate entirely offline, validate all
sections and integrity data, and compare its invariants with exact stock
graphs. Do not execute it.

### PoC 12E — Fixed Conv device control

Only under a separately approved one-shot plan, stage the exact candidate,
execute canary-protected tensors, compare with the CPU oracle, and collect
unchanged health evidence.

### PoC 13 — Hybrid noise-cancellation island

Approve one fixed ONNX graph signature, retain CPU fallback, and qualify the
small spectral-mask partition before evaluating latency, quality, thermal,
and soak behavior.

## 16. Non-claims

The project must not describe a PoC 12A pass as:

- an open Wave compiler;
- ONNX-to-NBG support;
- a decoded NPU ISA;
- a reusable Conv kernel;
- permission to change coefficients;
- proof that the CTC NBG came from the adjacent ONNX file;
- a simulator-to-Wave translation;
- NPU noise cancellation.

The precise claim is:

> Exact Wave NBG serializer behavior has been statically bounded, and
> unchanged selected Wave NBGs can be represented and re-emitted
> byte-identically with total byte ownership and no native execution.
