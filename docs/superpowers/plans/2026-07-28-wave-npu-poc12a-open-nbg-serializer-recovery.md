# Wave NPU PoC 12A: Open NBG Serializer Recovery Implementation Plan

**Date:** 2026-07-28

**Status:** Ready for implementation

**Design:** `docs/superpowers/specs/2026-07-28-wave-npu-open-nbg-serializer-design.md`

**Design commit:** `431eec3`

**Safety class:** Host-only static analysis, bounded ONNX inspection, and
byte-identical re-emission of unchanged NBG files

## 1. Goal

Implement the first SDK-independent NBG compiler-recovery PoC without
creating, modifying, or executing a graph.

PoC 12A must produce four independently verifiable results:

1. bounded static evidence for the exact Wave serializer call graph;
2. a lossless VPMN v2 image whose ranges own every source byte exactly once;
3. byte-identical re-emission of five pinned exact-target NBG controls;
4. a bounded ONNX/NBG coefficient-correlation ledger with explicit
   confidence classes.

The implementation must fail closed. An unresolved coefficient transform is
an acceptable research result. A guessed transform, changed checksum, custom
NBG, native library call, simulator call, or device call is not.

## 2. Existing code to reuse

The implementation extends existing host-only infrastructure:

```text
waved/scripts/npu/nbg_format.py
  strict VPMN v1/v2 parsing and bounded range reads

waved/scripts/npu/ghidra_abi.py
  hash-pinned temporary-project Ghidra adapter

scripts/ghidra/ExportNpuAbi.java
  deterministic target/caller/callee/decompilation exporter

waved/scripts/npu/abi_inventory.py
  exported-function offset, size, and byte-hash recovery

waved/tools/onnx-hybrid/
  pinned Python 3.12, ONNX 1.22, and NumPy environment
```

Do not duplicate the existing VPMN decoder. `nbg_image.py` consumes
`nbg_format.parse_bytes()` and adds lossless byte ownership and unchanged
emission. Do not couple the generic NBG parser to ONNX.

## 3. Exact initial corpus

The initial manifest contains exactly these artifacts.

### 3.1 Firmware ELFs

```text
id: wave-openvx
path: firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libOpenVX.so
bytes: 6422744
sha256: f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb

id: wave-ovxlib
path: firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libovxlib.so
bytes: 5838160
sha256: 09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9
```

### 3.2 Exact-target NBG controls

```text
id: generated-erf
path: artifacts/npu/runs/poc5c-erf-compile-1-20260728/candidate-1.nbg
bytes: 5840
sha256: 6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee

id: ctc-en
path: models/ai-models/asr-models/model/asr_en/reverb_asr_online/ctc_online.nb
bytes: 5263360
sha256: 3a9294ea8595789cc23ddc9298ebceb4474738fed07a9a8b10e2442b67b0d8d5

id: ctc-de
path: models/ai-models/asr-models/model/asr_de/de_online/ctc_online.nb
bytes: 2810984
sha256: 5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66

id: ctc-fr
path: models/ai-models/asr-models/model/asr_fr/fr_online/ctc_online.nb
bytes: 2935144
sha256: db8a79e3e7b35eddd87a7369bd464101607083f489a75418d73073718ff6f079

id: ctc-ja
path: models/ai-models/asr-models/model/asr_ja/ja_online/ctc_online.nb
bytes: 2717544
sha256: 2c261beb8aedb1361c3a4832878ff09dec597bcae4681aee742069c6faa54b5d
```

### 3.3 ONNX source anchor

```text
id: ctc-en-onnx
path: models/ai-models/asr-models/model/asr_en/reverb_asr_online/ctc.onnx
bytes: 10258790
sha256: 8d6d3b6ce9da51f852981bb5bdd33e2818d47d3e545e9efe5d3700a37b367219
```

The manifest describes `ctc-en-onnx` and `ctc-en` as a correlation anchor,
not as proven same-build provenance.

## 4. Target file layout

Create:

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

Modify only where required:

```text
scripts/ghidra/ExportNpuAbi.java
waved/scripts/npu/README.md
docs/npu/format/README.md
docs/npu/format/field-map.md
docs/npu/lab-notebook/README.md
docs/npu/README.md
docs/npu/reproduction.md
waved/docs/COMPUTE_HARDWARE.md
```

Do not edit `nbg_format.py` unless a lossless-image test proves that a
strictly decoded offset or size is unavailable. Prefer adding a read-only
field to its parsed result over introducing emission behavior there.

## 5. Shared implementation rules

### 5.1 Test first

For each task:

1. add the smallest failing contract test;
2. run only that test and observe the expected failure;
3. implement the minimum behavior;
4. rerun the focused test;
5. run the task's neighboring regression suite;
6. inspect the diff before moving to the next task.

### 5.2 Deterministic evidence

All tracked JSON uses:

```python
json.dumps(
    document,
    allow_nan=False,
    ensure_ascii=False,
    indent=2,
    sort_keys=True,
) + "\n"
```

Arrays with semantic ordering preserve it. Set-like arrays are sorted before
serialization. Evidence contains no timestamps, temporary paths, hostnames,
absolute paths, Python object IDs, or nondeterministic Ghidra project names.

### 5.3 Stable errors

Expose one `SerializerError` base with a stable `code`, artifact ID, bounded
location, and safe message. Use the design's exact categories:

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

Do not put coefficient payloads or complete decompilation text in exception
messages.

### 5.4 Safety counters

Every top-level generated evidence file contains:

```json
{
  "safety": {
    "adb_calls": 0,
    "compiler_calls": 0,
    "container_registrations": 0,
    "container_runs": 0,
    "device_calls": 0,
    "generated_custom_nbg_files": 0,
    "mutated_nbg_files": 0,
    "native_extension_imports": 0,
    "simulator_calls": 0
  }
}
```

The schema accepts no nonzero value. These counters are assertions about the
implemented code path, not mutable telemetry supplied by a caller.

## 6. Task 1 — Strict target manifest and corpus freeze

### Files

```text
Create: waved/scripts/npu/nbg_serializer_targets.json
Create: waved/scripts/npu/nbg_serializer_recovery.py
Create: waved/scripts/npu/tests/test_nbg_serializer_recovery.py
```

### 6.1 Write failing manifest tests

Cover:

- duplicate JSON keys;
- missing or unknown root keys;
- unsupported schema version and PoC ID;
- invalid artifact IDs, kinds, roles, byte counts, and SHA-256 values;
- absolute paths, `..`, repository escapes, and symlink components;
- missing or non-regular artifacts;
- artifact size or hash drift;
- duplicate artifact roles within one artifact;
- duplicate correlation endpoints;
- unknown corpus IDs in an anchor or Ghidra batch;
- duplicate and unapproved function names;
- an ELF with the wrong class, endianness, machine, or load geometry;
- a policy with a nonzero prohibited-action counter;
- an output path outside `docs/npu/format`;
- mutation or execution operations in the allowed-operation list.

The success fixture uses tiny temporary ELF/NBG/ONNX placeholders and an
injectable artifact validator. Unit tests must not depend on the 6 MiB
firmware ELFs.

### 6.2 Implement strict immutable manifest types

Use frozen dataclasses:

```text
ArtifactSpec
GhidraBatch
CorrelationAnchor
OutputSpec
SerializerPolicy
SerializerManifest
```

`load_manifest(path, repository_root=...)` performs syntax and structural
validation. `validate_artifacts(manifest)` performs filesystem, hash, ELF,
and NBG-envelope validation.

ELF admission is read-only and checks:

```text
magic: ELF
class: ELF64
endianness: little
machine: AArch64
program-header table: bounded inside the file
every PT_LOAD file range: bounded inside the file
```

Do not import `pyelftools`; the existing codebase uses bounded standard
library parsing and external static tools. Implement only the ELF identity
fields required by this manifest.

### 6.3 Pin target batches

The initial exact target set is:

```text
entry:
  vsi_nn_GenerateNBG
  vxGenerateNBG
  vxoBinaryGraph_NetworkBinaryGraph

envelope-and-memory:
  vxoBinaryGraph_SaveNetworkBinaryGraph
  vxoBinaryGraph_SaveBinaryEntrance
  vxoBinaryGraph_SaveBinaryEntranceBefore
  vxoBinaryGraph_GenerateSectionMemory
  vxoBinaryGraph_SaveWeightAndBias
  vxoBinaryGraph_UpdateCoefHeaderCache

operation-and-command:
  vxoBinaryGraph_SaveNNTPOperation
  vxoBinaryGraph_SaveNNTPCommand
  vxoBinaryGraph_SaveNNTPInstruction
  vxoBinaryGraph_SaveNNTPStates

patch-relocation-and-load:
  vxoBinaryGraph_SetPatchDataInfo
  vxoBinaryGraph_Patch32bitVa
  vxoBinaryGraph_Patch40bitVa
  vxoBinaryGraph_RelocateBinaryEntrance
  vxoBinaryGraph_RelocateNetworkBinaryGraph
  vxoBinaryGraph_LoadNBG
  vxoBinaryGraph_AllocateBinaryLoadBuffer
  vxoBinaryGraph_ReleaseNBG
```

`vsi_nn_GenerateNBG` belongs only to `wave-ovxlib`; every other function
belongs only to `wave-openvx`. Tests reject a target in the wrong ELF.

### 6.4 Focused verification

Run:

```text
python3 -m unittest \
  waved.scripts.npu.tests.test_nbg_serializer_recovery.ManifestTests
```

Then validate the real manifest twice and assert that its canonical
document is byte-identical.

### 6.5 Stop condition

Do not begin NBG parsing if any real artifact hash differs. Report the exact
artifact ID and observed hash without automatically updating the manifest.

## 7. Task 2 — Immutable lossless VPMN v2 image

### Files

```text
Create: waved/scripts/npu/nbg_image.py
Create: waved/scripts/npu/tests/test_nbg_image.py
Read/reuse: waved/scripts/npu/nbg_format.py
```

### 7.1 Write synthetic ownership tests

Reuse or adapt the existing `make_v2()` fixture helpers. Add synthetic files
that exercise:

- adjacent sections with no gap;
- a required alignment gap;
- extra padding beyond the alignment gap;
- nonzero padding bytes;
- an unknown section type;
- a zero-length section;
- a valid final-512-byte connection view;
- directory/header padding;
- duplicated nonempty section ranges;
- a final-512-byte view already covered by a section;
- one-byte truncation at every root structural boundary.

The tests first require the following public contracts:

```python
image = NbgImage.parse(payload, display_path="fixture.nbg")
image.ranges
image.assert_total_ownership()
image.to_bytes()
image.mark_changed("test").to_bytes()  # raises
image.ownership_document()
```

### 7.2 Implement immutable range types

Use frozen dataclasses:

```text
ByteClass
NbgOwnedRange
NbgImage
RoundtripResult
```

`ByteClass` has exactly:

```text
decoded-record
alignment
padding
preserved-opaque
connection-tail
```

Each `NbgOwnedRange` contains:

```text
start
end
owner
byte_class
schema
raw bytes
raw SHA-256
```

The public evidence document excludes `raw bytes`; the writer retains them
in memory.

### 7.3 Derive root ranges

For VPMN v2:

1. call `nbg_format.parse_bytes()` for existing strict semantic validation;
2. own bytes `[0, 48)` as the common/v2 header;
3. own the directory locator record and its section records as decoded
   records using parsed offsets and record sizes;
4. own decoded section payloads as record-sized ranges where the schema is
   proven;
5. own other section payloads as `preserved-opaque`;
6. expose the final validated 512 bytes as a connection view; own it as
   `connection-tail` only when no section already owns those bytes, otherwise
   label it `section-covered-view`;
7. classify all remaining source bytes deterministically as alignment or
   padding;
8. sort all ranges and prove exact coverage of `[0, file_size)`.

At minimum, decoded VPMN v2 payload schemas are:

```text
input/output IO records
network info
memory pool
VIP SRAM
AXI SRAM
checksum records
```

Unknown section types and known-but-undecoded payloads remain opaque.

### 7.4 Alignment and padding rule

For each gap before a section:

1. inspect that section's original alignment value;
2. if alignment is zero or one, classify the complete gap as padding;
3. otherwise require the section offset to satisfy the original alignment;
4. classify the minimum prefix needed to advance the prior cursor to its
   next aligned address as `alignment`;
5. classify any remaining bytes before the section as `padding`.

For root gaps without a following section alignment contract, use
`padding`. Preserve all bytes exactly, including nonzero values.

No byte may be represented by two conceptual classes. Alignment metadata on
a section is a decoded field; an alignment gap is a source-byte range.

### 7.5 Prove exclusive coverage

`assert_total_ownership()` walks ranges in ascending order:

```text
expected next start begins at 0
range.start must equal expected next start
range.end must be greater than range.start
range.end must not exceed source size
raw length must equal end - start
raw hash must match
final expected next start must equal source size
```

Map failures to `NBG_RANGE_GAP`, `NBG_RANGE_OVERLAP`,
`NBG_SECTION_GEOMETRY`, or `NBG_CONNECTION_TAIL_INVALID`.

### 7.6 Implement the unchanged writer

`to_bytes()`:

1. refuses if `changed` is true;
2. reruns ownership validation;
3. concatenates the ordered owned-range bytes;
4. verifies length and source SHA-256;
5. returns the reconstructed byte string.

It must not return a cached copy of the original payload directly. The
round-trip proof depends on reconstruction from owned ranges.

`mark_changed(reason)` returns a new frozen image with `changed=True`.
There is no field-editing, checksum-update, or section-replacement API.

### 7.7 Focused verification

Run:

```text
python3 -m unittest waved.scripts.npu.tests.test_nbg_image
python3 -m unittest waved.scripts.npu.tests.test_nbg_format
```

### 7.8 Stop condition

If current `nbg_format.py` does not expose enough geometry, add only the
missing source offset/length to its parsed dictionary plus a focused parser
test. Do not reimplement its semantic decoder in `nbg_image.py`.

## 8. Task 3 — Exact corpus round-trip and evidence

### Files

```text
Modify: waved/scripts/npu/nbg_image.py
Modify: waved/scripts/npu/tests/test_nbg_image.py
Generate: docs/npu/format/serializer-byte-ownership.json
Generate: docs/npu/format/serializer-roundtrip-evidence.json
```

### 8.1 Add round-trip APIs and CLI

Public functions:

```text
roundtrip_bytes(payload, artifact_id, display_path)
roundtrip_path(path, artifact_id, expected_size, expected_sha256)
build_ownership_evidence(manifest)
build_roundtrip_evidence(manifest)
verify_generated(manifest, tracked_paths)
write_identical_copy_exclusive(source, destination)
```

CLI commands:

```text
inspect ARTIFACT_ID
roundtrip ARTIFACT_ID [--copy-to PATH]
generate-evidence
verify-generated
```

The optional copy path:

- must resolve below the repository;
- must not exist;
- is opened exclusively;
- receives only verified byte-identical output;
- is removed if writing or post-write verification fails;
- is not a tracked evidence input.

### 8.2 Write real-corpus tests

For all five NBG IDs, assert:

```text
parsed target == 0x1000000e
parsed version == 0x00020000
output length == input length
output bytes == input bytes
output SHA-256 == expected SHA-256
ownership starts at 0 and ends at file size
every ownership range has a valid class and raw hash
```

Add a one-byte corruption test that updates neither manifest hash nor NBG
checksums and proves refusal before emission.

Add a semantic-change test:

```python
changed = image.mark_changed("target")
with self.assertRaisesRegex(..., "NBG_SEMANTIC_CHANGE_REFUSED"):
    changed.to_bytes()
```

### 8.3 Generate separate evidence documents

`serializer-byte-ownership.json` records each file and every owned range.

`serializer-roundtrip-evidence.json` records:

```text
input/output bytes and SHA-256
exact_match
first_difference: null
range count and class byte totals
semantic mutation policy: refused
copy written: false
safety counters
```

Generation does not write an NBG.

### 8.4 Verify determinism

Generate each document twice into separate temporary paths and compare bytes.
Then write the tracked paths atomically and run `verify-generated`.

### 8.5 Stop condition

The first byte mismatch blocks every later task. Record its file offset and
range owner, but do not normalize, repair, or rewrite it.

## 9. Task 4 — Exact serializer static-evidence recovery

### Files

```text
Modify: waved/scripts/npu/nbg_serializer_recovery.py
Modify: waved/scripts/npu/tests/test_nbg_serializer_recovery.py
Optional modify: scripts/ghidra/ExportNpuAbi.java
Generate: docs/npu/format/serializer-ghidra-evidence.json
```

### 9.1 Test the adapter with a fake executor

Build small canonical fake Ghidra results and test:

- exact configured target set;
- every target is found once;
- one nonexternal target function per requested export, with thunks accepted
  only when explicitly allowlisted as export aliases;
- successful, untruncated decompilation;
- no Ghidra warnings;
- direct caller/callee records are structurally valid and sorted;
- function virtual address, ELF symbol size, and byte hash are bound;
- a missing symbol, duplicate function, unapproved probable thunk, truncated
  function, missing hash, unexpected target, or changed function body fails;
- two fake runs produce byte-identical evidence;
- temporary project paths never enter the output.

### 9.2 Bind Ghidra evidence to ELF export bytes

Reuse the existing ABI inventory representation or its proven
virtual-address/file-offset logic. For each requested export, record:

```text
name
ELF virtual address
ELF file offset
ELF symbol byte count
ELF symbol-byte SHA-256
Ghidra entry address
Ghidra function-body byte count
thunk/external status
callers
callees
diagnostics
bounded decompiled C
```

Reject disagreement between the ELF virtual address and Ghidra entry.
Reject an export without a positive symbol size or `hashed` byte status.

Do not execute `nm`, `readelf`, or `rz-bin` as a source of truth during
tracked evidence verification unless the exact tool and version are included
in the evidence. Prefer reading the already tracked ABI inventory after
verifying its source hash, or reuse its deterministic parser path.

### 9.3 Translate target batches to the existing Ghidra adapter

`nbg_serializer_recovery.py` constructs an in-memory
`ghidra_abi.GhidraConfig` from the strict serializer manifest and calls
`ghidra_abi.build_evidence()`.

Use separate target batch IDs so the output stays reviewable:

```text
wave-ovxlib-entry
wave-openvx-entry
wave-openvx-envelope-memory
wave-openvx-operation-command
wave-openvx-patch-relocation-load
```

The same hash-pinned `libOpenVX.so` may appear in four bounded analysis
batches. Batch identity and configured target names are part of the evidence.

### 9.4 Handle decompilation bounds without changing old evidence

Run the existing exporter first. If one selected function is marked
truncated solely by the current 24,000-character limit, extend
`ExportNpuAbi.java` with an optional maximum-decompilation argument:

- default behavior and schema remain byte-identical for all existing users;
- the serializer adapter passes a reviewed hard maximum no greater than
  128 KiB per function;
- output still fails if the function exceeds that bound;
- add a Java-script argument validation test through the Python fake/command
  layer;
- rerun `ghidra_abi.py verify-generated` to prove old evidence did not drift.

Do not silently remove decompiled code or accept a truncated target.

### 9.5 Build an ownership/call relationship ledger

For each serializer family, summarize:

```text
entry and direct invocation chain
file/stream write calls
allocation and section-size calls
named NBG section constants or diagnostics
calls into coefficient, command, state, patch, or relocation helpers
load-side inverse or validation functions
unresolved indirect edges
```

Every relationship is `static-verified`, `inferred`, or `unresolved`.
Function names alone do not establish a decoded field.

### 9.6 Run real bounded Ghidra analysis

Run:

```text
python3 waved/scripts/npu/nbg_serializer_recovery.py generate-ghidra
python3 waved/scripts/npu/nbg_serializer_recovery.py verify-ghidra
```

Generate twice and compare canonical bytes. Inspect warnings, truncation,
duplicate entries, unresolved calls, and unexpected thunks before accepting
the tracked evidence.

### 9.7 Stop condition

Any missing priority family or truncated priority function blocks the PoC.
Unresolved indirect callees are recorded and may remain unresolved if every
configured target itself is complete.

## 10. Task 5 — Bounded coefficient-correlation engine

### Files

```text
Create: waved/scripts/npu/nbg_correlate.py
Create: waved/scripts/npu/tests/test_nbg_correlate.py
Read/reuse: waved/tools/onnx-hybrid/
Read/reuse: waved/scripts/npu/nbg_image.py
```

### 10.1 Use the pinned ONNX environment

Run the correlator and its tests through:

```text
uv run --project waved/tools/onnx-hybrid \
  python waved/scripts/npu/nbg_correlate.py ...
```

The correlator imports ONNX and NumPy only. It does not import ONNX Runtime,
create an inference session, load a native Acuity extension, or import a
firmware library.

### 10.2 Write transform-fixture tests

Create tiny in-memory tensors and NBG section payloads for:

- one unique direct match;
- the same bytes at two offsets, producing ambiguity;
- FP32-to-FP16 match;
- FP32-to-BF16 round-to-nearest-even match;
- 2D transpose then FP16 match;
- fixed block permutation match;
- byte-lane and halfword-lane permutation match;
- unrelated negative control;
- transform candidate-count limit;
- source tensor byte-size limit;
- destination section byte-size limit;
- provenance and inverse metadata;
- deterministic relationship ordering.

Tests must assert exact destination section ID, offset, length, source tensor
name, transform ID, parameters, source/output hashes, occurrence count, and
evidence class.

### 10.3 Admit the ONNX anchor strictly

Before extracting initializers:

- verify exact model byte count and SHA-256;
- enforce the existing hybrid policy's model, tensor, rank, dimension, and
  initializer limits;
- reject external data, sparse initializers, nested graph constants, and
  malformed payloads for this PoC;
- run `onnx.checker.check_model`;
- require the expected top-level chain
  `MatMul -> Add -> LogSoftmax`;
- require exactly the named bias `[4999]` FP32 and weight `[512,4999]`
  FP32 initializers;
- normalize tensor bytes explicitly to little-endian contiguous order.

If the initializer names differ, report the observed canonical inventory and
stop. Do not select tensors only by byte size.

### 10.4 Define a finite transform registry

Each transform is a named pure function with a declared inverse property and
hard parameter allowlist:

```text
identity-fp32-le
cast-fp16-rne-le
cast-bf16-rne-le
transpose-2d
transpose-2d+cast-fp16-rne-le
transpose-2d+cast-bf16-rne-le
fixed-block-permute
byte-lane-permute
halfword-lane-permute
```

Initial block widths are finite and source-controlled:

```text
elements: 2, 4, 8, 16, 32
byte lanes: 2, 4, 8
halfword lanes: 2, 4, 8
```

Only explicitly listed permutations are tested: identity, reversal, even/odd
deinterleave, odd/even deinterleave, and adjacent-pair swap where defined.
No factorial permutation generation or heuristic search is allowed.

Cap:

```text
source initializer bytes: 64 MiB
one generated candidate: 64 MiB
candidate transforms per tensor: 128
searched NBG sections per graph: 16
total exact-search bytes per tensor: 256 MiB
```

### 10.5 Limit destination sections

Search only original bytes owned by these section families:

```text
NBG_SECTION_OPERATIONS
NBG_SECTION_COEF_HEADER_TABLE
NBG_SECTION_COEF_HEADER
NBG_SECTION_NN
NBG_SECTION_TP
NBG_SECTION_SH
NBG_SECTION_PATCH
NBG_SECTION_LOADSTATE
NBG_SECTION_COEF_DATA_TABLE
NBG_SECTION_COEF_DATA
NBG_SECTION_LOADSTATE_TABLE
```

Record absent section families. Never search header, IO metadata, padding,
alignment gaps, connection tails, or checksum records as coefficient data.

### 10.6 Classify matches

Use:

```text
direct-match
transformed-match
differentially-supported
inferred
unresolved
```

A transformed match requires exact bytes, complete provenance, finite
parameters, occurrence count, and inverse metadata. More than one occurrence
is reported as `CORRELATION_AMBIGUOUS`; it is not promoted to a transformed
match.

No match yields `unresolved`, not a failing process, as long as all bounded
candidates were executed and recorded.

### 10.7 Focused verification

Run:

```text
uv run --project waved/tools/onnx-hybrid \
  python -m unittest waved.scripts.npu.tests.test_nbg_correlate
```

Also rerun the ONNX hybrid unit suite to ensure admission behavior did not
change.

## 11. Task 6 — Real CTC correlation and differential controls

### Files

```text
Modify: waved/scripts/npu/nbg_correlate.py
Modify: waved/scripts/npu/tests/test_nbg_correlate.py
Generate: docs/npu/format/onnx-nbg-correlation.json
```

### 11.1 Analyze the English anchor

For the weight and bias:

1. record original tensor metadata and canonical source hash;
2. run every finite transform candidate;
3. search only admitted NBG section ranges;
4. record every candidate transform ID and result summary;
5. preserve the existing observation that plain FP16 has no direct linear
   match if the new run confirms it;
6. classify the best supported relationship without guessing.

The output must be small enough to review. Record candidate hashes and match
locations, not complete candidate or coefficient bytes.

### 11.2 Compare multilingual controls

For each selected section type and graph, record:

```text
presence
offset
size
entry size
record count
raw SHA-256
class byte totals
```

Calculate bounded pairwise comparisons against English:

- identical full sections;
- equal-size but changed sections;
- size deltas;
- common prefix/suffix length capped at 1 MiB;
- repeated 32/64/128-byte block hashes;
- exact duplicate graphs or sections.

These comparisons may support a hypothesis but cannot create a transformed
match without exact source reconstruction.

### 11.3 Generate and verify

Run:

```text
uv run --project waved/tools/onnx-hybrid \
  python waved/scripts/npu/nbg_correlate.py generate

uv run --project waved/tools/onnx-hybrid \
  python waved/scripts/npu/nbg_correlate.py verify-generated
```

Generate twice into temporary paths and compare bytes before updating the
tracked file.

### 11.4 Exit decision

Emit one explicit PoC 12B decision:

```text
ready
  at least one deterministic invertible coefficient transform is exact and
  corroborated across two tensors or graphs

not-ready
  evidence is useful but no transform meets the above bar
```

`not-ready` is a successful PoC 12A outcome and does not weaken the
round-trip result.

## 12. Task 7 — Integrated evidence verification

### Files

```text
Modify: waved/scripts/npu/nbg_serializer_recovery.py
Modify: waved/scripts/npu/tests/test_nbg_serializer_recovery.py
```

### 12.1 Verification command boundary

The initial plan proposed one orchestration CLI exposing:

```text
validate
generate-ghidra
generate-roundtrip
generate-correlation
generate-all
verify-ghidra
verify-roundtrip
verify-correlation
verify-generated
```

The as-built implementation deliberately keeps the three evidence domains
behind their owning CLIs:

```text
nbg_serializer_recovery.py  validate / generate-ghidra / verify-ghidra
nbg_image.py                generate-evidence / verify-generated
nbg_correlate.py            generate / verify-generated
```

This avoids importing ONNX and NumPy into the base static-analysis process.
The correlation commands run through the pinned `onnx-hybrid` uv
environment. No command invokes a compiler, simulator, container runtime,
ADB, OpenVX tool, or device utility.

### 12.2 Add stale-evidence tests

For each tracked output, change one safe scalar in a temporary copy and prove
the owning verifier rejects the stale evidence.

Also test:

- missing evidence;
- duplicate JSON key;
- wrong generator path;
- wrong manifest hash;
- wrong source-artifact hash;
- nonzero safety counter;
- changed Ghidra function hash;
- changed round-trip result;
- changed correlation transform registry.

### 12.3 End-to-end host-only run

From repository root:

```text
python3 waved/scripts/npu/nbg_serializer_recovery.py validate
python3 waved/scripts/npu/nbg_serializer_recovery.py verify-ghidra
python3 waved/scripts/npu/nbg_image.py verify-generated
uv run --project waved/tools/onnx-hybrid \
  python waved/scripts/npu/nbg_correlate.py verify-generated
```

Capture tool versions used for static evidence, but no host-specific paths.

### 12.4 Required evidence assertions

The integrated verifier asserts:

```text
all eight source artifacts hash-match
all selected functions are present and untruncated
all five NBG files have total exclusive ownership
all five NBG files round-trip exactly
all five output hashes equal input hashes
semantic mutation refusal test is represented
correlation registry and limits match the manifest
plain FP16 result is explicit
PoC 12B readiness has one valid value
all prohibited-action counters are zero
```

## 13. Task 8 — Documentation and regression closure

### Files

```text
Create: docs/npu/lab-notebook/2026-07-28-poc12a-open-nbg-serializer.md
Modify: docs/npu/format/README.md
Modify: docs/npu/format/field-map.md
Modify: docs/npu/lab-notebook/README.md
Modify: docs/npu/README.md
Modify: docs/npu/reproduction.md
Modify: waved/scripts/npu/README.md
Modify: waved/docs/COMPUTE_HARDWARE.md
```

### 13.1 Write the lab notebook

Document:

- why serializer-first replaced reliance on the unavailable no-kernel SDK;
- exact corpus and hashes;
- exact function batches;
- static call-graph findings with evidence classes;
- the byte-ownership algorithm;
- per-file class totals and round-trip hashes;
- the ONNX initializer inventory;
- every transform family tested;
- direct/transformed/differential/unresolved findings;
- safety counters;
- test and reproduction commands;
- known limitations;
- the PoC 12B exit decision.

Avoid claiming ONNX-to-NBG support, a decoded ISA, safe coefficient mutation,
or device execution.

### 13.2 Update the field map

Add a separate lossless-ownership column or subsection that distinguishes:

```text
decoded semantic field
decoded raw record
opaque original payload
alignment gap
padding gap
connection tail
```

Do not promote inferred serializer relationships to proven field layouts.

### 13.3 Add reproduction commands

Document the host requirements:

```text
Python 3.12
uv
Ghidra version captured in evidence
existing onnx-hybrid lockfile
exact firmware/model artifacts
```

Provide commands for:

```text
manifest validation
focused unit tests
Ghidra regeneration
round-trip regeneration
correlation regeneration
full generated-evidence verification
```

State that none of these commands contacts the Wave.

### 13.4 Run focused suites

```text
python3 -m unittest waved.scripts.npu.tests.test_nbg_serializer_recovery
python3 -m unittest waved.scripts.npu.tests.test_nbg_image
python3 -m unittest waved.scripts.npu.tests.test_nbg_format

uv run --project waved/tools/onnx-hybrid \
  python -m unittest waved.scripts.npu.tests.test_nbg_correlate
```

### 13.5 Run neighboring regressions

```text
python3 -m unittest waved.scripts.npu.tests.test_ghidra_abi
python3 -m unittest waved.scripts.npu.tests.test_abi_inventory
python3 -m unittest waved.scripts.npu.tests.test_arch_profile
python3 -m unittest waved.scripts.npu.tests.test_compiler_profile
python3 -m unittest waved.scripts.npu.tests.test_compiler_acquisition

uv run --project waved/tools/onnx-hybrid pytest \
  waved/tools/onnx-hybrid/tests
```

### 13.6 Run static and generated checks

```text
python3 -m py_compile \
  waved/scripts/npu/nbg_serializer_recovery.py \
  waved/scripts/npu/nbg_image.py \
  waved/scripts/npu/nbg_correlate.py

python3 waved/scripts/npu/nbg_serializer_recovery.py verify-ghidra
python3 waved/scripts/npu/nbg_image.py verify-generated
uv run --project waved/tools/onnx-hybrid \
  python waved/scripts/npu/nbg_correlate.py verify-generated
python3 waved/scripts/npu/ghidra_abi.py verify-generated
git diff --check
```

If Ruff is already available in the pinned environment, run it on the three
new Python modules and tests. Do not change unrelated legacy formatting.

## 14. Final acceptance checklist

PoC 12A is complete only if every item is true:

- [x] The manifest is strict, exact, and hash-pinned.
- [x] Every configured ELF is ELF64 little-endian AArch64.
- [x] All 21 configured serializer targets are present in the correct ELF.
- [x] Every target has a positive symbol size and exact function-byte hash.
- [x] Every target has successful, untruncated bounded decompilation.
- [x] The serializer relationship ledger names evidence class and source.
- [x] Every byte in all five VPMN v2 controls has exactly one owner.
- [x] Every control is reconstructed from owned ranges byte-identically.
- [x] Every reconstructed SHA-256 equals the pinned input SHA-256.
- [x] A semantically changed image is refused.
- [x] No tracked or generated custom NBG exists.
- [x] The English ONNX model passes strict bounded admission.
- [x] Weight and bias correlation results are deterministic and classified.
- [x] Plain FP16 non-match remains explicit if confirmed.
- [x] Multilingual differential controls are bounded and provenance-linked.
- [x] PoC 12B receives an explicit `ready` or `not-ready` decision.
- [x] All four evidence documents verify as current.
- [x] All prohibited-action counters are zero.
- [x] Focused and neighboring regression suites pass.
- [x] Documentation distinguishes unchanged re-emission from compilation.
- [x] `git diff --check` passes for PoC 12A files.

### 14.1 As-built verification record

Completion results:

```text
focused serializer/image/format unit tests    60 passed
correlation unit tests                        14 passed
neighboring static-analysis unit tests        60 passed
complete NPU discovery in pinned uv env      261 passed
ONNX-hybrid package tests                     35 passed
Ruff                                           passed
Python byte compilation                        passed
PoC 12A Ghidra canonical comparison            current
PoC 1 six-target default-limit comparison      6 / 6 current
ownership/round-trip evidence                   current
ONNX/NBG correlation evidence                  current
git diff and new-file whitespace checks        passed
```

Base Python intentionally lacks the ONNX environment's NumPy dependency.
Plain base-Python discovery therefore passed 247 tests and reported only the
expected import error for `test_nbg_correlate`; the authoritative complete
discovery ran inside the pinned `onnx-hybrid` environment and passed all
261 tests.

## 15. Commit discipline

The worktree already contains unrelated and earlier PoC changes. Do not stage
the whole repository. If commits are requested, commit only exact PoC 12A
paths after reviewing `git diff --cached --name-status`.

Suggested logical commit boundaries:

```text
test(npu): define serializer manifest and lossless image contracts
feat(npu): add byte-identical NBG image and evidence
feat(npu): recover exact Wave NBG serializer evidence
feat(npu): add bounded ONNX NBG correlation
docs(npu): document serializer-first PoC 12A
```

Do not rewrite, reset, clean, or discard any pre-existing worktree change.

## 16. Explicitly deferred work

The following require later specifications and are not implementation
shortcuts for this plan:

```text
PoC 12B: coefficient packing reconstruction
PoC 12C: command, operation, patch, and relocation emission
PoC 12D: fixed offline Conv construction
PoC 12E: one-shot device execution
PoC 13: hybrid noise-cancellation NPU island
```

PoC 12A ends with static evidence and lossless unchanged re-emission.
