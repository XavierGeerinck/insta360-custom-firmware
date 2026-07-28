# Wave NPU PoC 2 — NBG Corpus and Format Explorer Plan

**Date:** 2026-07-28  
**Status:** Approved program, PoC 2 execution plan  
**Depends on:** PoC 0 pass, PoC 1 pass at commit `f68708d`  
**Safety class:** Host-only binary parsing and comparison  
**Program design:** `docs/superpowers/specs/2026-07-28-wave-npu-reverse-engineering-program-design.md`

## 1. Goal

Build a strict, deterministic host parser and corpus inventory for the
VeriSilicon Network Binary Graph files available in the Wave research
workspace.

PoC 2 answers:

- Which NBG envelope versions and product targets are present?
- Which header fields and section locators are supported by direct public
  source, exact Wave runtime code, or both?
- Which input/output tensor descriptors can be recovered without loading
  native firmware code?
- Which section types, offsets, sizes, record widths, and counts occur in the
  Wave NBG v2 corpus?
- Which graph files are exact duplicates, near structural relatives, or
  unique?
- Which bytes are understood, bounded-but-opaque, or completely unknown?
- Does the host parser agree with the live-proven stock CTC metadata?
- Does every malformed boundary fail with a structured error rather than a
  panic, allocation spike, or partial result?

The main output must let a reviewer trace:

```text
corpus file and SHA-256
  -> NBG envelope and target
  -> bounded section directory
  -> input/output descriptor records
  -> normalized tensor metadata
  -> source and confidence for every interpreted field
  -> opaque-region fingerprints for everything else
```

## 2. Non-goals

PoC 2 does not:

- execute an NBG;
- send any model bytes to the device;
- modify an NBG in the corpus;
- patch a product ID;
- infer command-buffer semantics from entropy or similarity alone;
- disassemble embedded NPU instructions;
- construct or execute a custom graph;
- claim a section type’s semantic name without independent evidence;
- parse the private export-data format;
- attach to, trace, or call firmware libraries;
- invoke ADB;
- benchmark NPU performance.

Runtime and driver tracing belongs to PoC 3. A handcrafted export-data graph
or custom compiled NBG belongs to a separately reviewed execution PoC.

## 3. Safety Boundary

All PoC 2 work is host-only and read-only with respect to model inputs.

Allowed:

- read, hash, and parse local `.nb` files;
- inspect pinned public TIM-VX source;
- use exact PoC 1 Ghidra evidence for the Wave parser;
- synthesize malformed byte strings in host unit tests;
- generate deterministic JSON and Markdown in the repository;
- compute bounded hashes, entropy, and similarity fingerprints;
- compare parser output with already recorded PoC 0 metadata.

Excluded:

- ADB;
- `/dev/galcore`;
- any native firmware library execution;
- any device file read or write;
- service changes;
- sysfs;
- kernel-module operations;
- reboot;
- raw driver command replay;
- staging model files on the device;
- executing parser-mutated data.

The parser is not imported by the device probe in this PoC. A malformed-input
test corpus remains in memory or in test-owned temporary directories.

## 4. Evidence Sources

### 4.1 Exact Wave runtime

PoC 1 pins the Wave `libnnsdk.so` and focused Ghidra evidence. Relevant exact
functions are:

- `read_nbg_from_memory`;
- `parse_nb_head_info` at `FUN_00113810`;
- `parse_nb_io_info` at `FUN_00113a20`.

For NBG v2 these functions establish:

- magic at `+0x00`;
- version at `+0x04`;
- product target at `+0x08`;
- nine additional header words beginning at `+0x0c`;
- a section-directory locator word at `+0x18`;
- a directory preamble of offset, record size, and count;
- 44-byte section-directory records in the observed runtime;
- section type 1 as input metadata;
- section type 2 as output metadata;
- an IO record stride carried by directory word 8;
- normalization to 128-byte internal IO records;
- a final 512-byte connection region copied by the runtime.

Decompiler names and inferred types remain secondary to exact accesses and
corpus consistency.

### 4.2 Pinned public TIM-VX

Current checkout:

```text
waved/third_party/TIM-VX
commit 702715d714277376d7ff466b7b72833ff57564c1
```

Relevant files:

```text
include/tim/utils/nbg_parser/gc_vip_nbg_format.h
include/tim/utils/nbg_parser/nbg_parser_version.h
src/tim/utils/nbg_parser/nbg_parser.c
```

The public parser supports legacy fixed-layout NBG through
`NBG_FORMAT_VERSION=0x00010014`. The local foreign add fixture is version
`0x0001001e`, so fields retained beyond the public parser’s declared maximum
must be labeled as compatible-prefix observations rather than guaranteed
public schema.

### 4.3 Live-proven metadata

PoC 0 records the exact v2 stock CTC graph:

```text
SHA-256 5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66
size     2810984
version  0x00020000
product  0x1000000e
input    FP16 [256, 16, 1], 8192 bytes
output   FP32 [5200, 16, 1], 332800 bytes
```

Parser output for that hash must match the recorded runtime result.

## 5. Exact Corpus

The local stock model root is:

```text
models/ai-models/asr-models/model
```

At plan time it contains 137 `.nb` files totaling 3,782,869,224 bytes. Every
file has magic `VPMN`, version `0x00020000`, and product `0x1000000e`.

The ignored model root is an operator-supplied research corpus, not a Git
artifact. The generated inventory therefore records every exact relative
path, size, and SHA-256, while verification fails clearly if the corpus is
absent or drifts.

Two local reference fixtures are also included:

```text
waved/third_party/stock-npu-stack/sample-add.nb
  version 0x0001001e
  product 0x10000020
  role    foreign host-generated legacy-format negative control

waved/third_party/stock-npu-stack/sample-ctc.nb
  version 0x00020000
  product 0x1000000e
  role    duplicate of the live-proven stock CTC graph
```

The configuration must distinguish:

- `stock`: firmware/model-bundle files expected to target the Wave;
- `reference`: exact local duplicates used by existing probes;
- `negative_control`: known foreign-target graphs that must never be
  reclassified as Wave-compatible.

## 6. Deliverables

Create:

- `waved/scripts/npu/nbg_corpus.json`
- `waved/scripts/npu/nbg_format.py`
- `waved/scripts/npu/tests/test_nbg_format.py`
- `docs/npu/format/README.md`
- `docs/npu/format/stock-nbg-inventory.json`
- `docs/npu/format/corpus-summary.md`
- `docs/npu/format/field-map.md`
- `docs/npu/lab-notebook/2026-07-28-poc2-nbg-corpus-format.md`

Modify:

- `waved/scripts/npu/README.md`
- `docs/npu/README.md`
- `docs/npu/lab-notebook/README.md`
- `waved/docs/COMPUTE_HARDWARE.md` only for durable, verified format facts.

Do not add `.nb` binaries, mutated fixtures, full binary dumps, or extracted
command buffers to Git.

## 7. Configuration Schema

`nbg_corpus.json` uses strict schema version 1:

```json
{
  "schema_version": 1,
  "collections": [
    {
      "id": "wave-stock-models",
      "root": "models/ai-models/asr-models/model",
      "glob": "**/*.nb",
      "role": "stock",
      "required": true,
      "expected_count": 137
    }
  ],
  "files": [
    {
      "id": "foreign-sample-add",
      "path": "waved/third_party/stock-npu-stack/sample-add.nb",
      "role": "negative_control",
      "required": true,
      "expected_sha256": "9bef0c0f2271ad534cb975b386be8428e973dc7ee4c71c04854c95eeac1fea5c"
    }
  ]
}
```

Constraints:

- all keys and enum values are exact;
- IDs are unique and match a conservative token grammar;
- roots and paths are repository-relative and cannot escape it;
- symlinks are rejected;
- globs are fixed in configuration, never supplied by the operator;
- only regular files ending in `.nb` are accepted;
- a physical path may appear only once in the final scan;
- expected file count is mandatory for required collections;
- exact reference-file hashes are mandatory;
- duplicate JSON keys fail;
- absent optional inputs are recorded, not silently omitted;
- output ordering is collection ID then bytewise relative path.

## 8. Parser Result Model

Each successfully parsed file emits:

```json
{
  "path": "models/...",
  "role": "stock",
  "size": 2810984,
  "sha256": "...",
  "format_family": "v2-directory",
  "magic": "VPMN",
  "version": 131072,
  "version_hex": "0x00020000",
  "product": 268435470,
  "product_hex": "0x1000000e",
  "header": {},
  "sections": [],
  "inputs": [],
  "outputs": [],
  "connection_tail": {},
  "coverage": {},
  "warnings": []
}
```

Numbers used for bounds remain JSON integers. Hex strings are included only
for human review and must be derived from those integers.

No raw tensor data, weights, commands, connection bytes, or full names beyond
bounded UTF-8 metadata are emitted.

## 9. Structured Error Model

`NbgParseError` includes:

- stable code;
- stage;
- byte offset when known;
- bounded context;
- human message.

Stable initial codes:

```text
not-regular-file
symlink-rejected
file-too-small
bad-magic
unsupported-version-family
truncated-header
invalid-header-field
invalid-directory-locator
invalid-directory-record-size
directory-count-limit
directory-range-overflow
section-range-overflow
section-overlap
invalid-io-record-size
io-count-limit
invalid-dimension-count
invalid-dimension
tensor-size-overflow
invalid-name
truncated-connection-tail
corpus-count-mismatch
corpus-hash-mismatch
generated-output-drift
```

Errors contain no model payload. CLI errors are deterministic and do not
include tracebacks for expected malformed inputs.

## 10. Checked Reader

Implement a small immutable byte reader with:

- `u32_le(offset, field)`;
- `bytes(offset, size, field)`;
- `cstring(offset, size, field)`;
- `checked_range(offset, size, field)`;
- `checked_product(count, stride, field)`.

Rules:

- all additions and multiplications are checked against file size and explicit
  limits before slicing;
- no negative values enter range calculations;
- no count controls an allocation before its bounded byte range is validated;
- maximum directory records: 256;
- maximum IO records per direction: 64;
- maximum dimensions per tensor: 6;
- maximum emitted name bytes: 64;
- minimum NBG size for runtime-compatible files: 513 bytes;
- parsing never uses an offset from the file before validating it;
- a parser failure returns no partial “success” object.

Python integers do not overflow, so explicit upper bounds and checked-range
helpers are still required to model the native risk.

## 11. Legacy v1 Parser

Family selection:

```text
0x00010000 <= version < 0x00020000
```

Supported evidence-backed prefix:

- common magic/version/product;
- 64-byte network-name field;
- layer, operation, input, and output counts;
- 256-byte feature database for version `>=0x00010003`;
- pool and SRAM words;
- public fixed-section entry pairs through the version-supported prefix;
- input and output table descriptors;
- 116-byte v1 IO entries for version `>=0x0001000b`;
- 120-byte transitional IO entries for versions `0x00010004..0x0001000a`;
- 56-byte oldest IO entries for earlier versions.

PoC 2 must not invent semantics for newer fixed-header words in
`0x0001001e`. It records:

- the public-prefix coverage end;
- the first referenced section offset;
- the gap as `opaque_fixed_extension`;
- its size, SHA-256, entropy, and zero-byte ratio.

The foreign add fixture must parse as two inputs and one output while
remaining explicitly classified as product `0x10000020`.

## 12. Wave v2 Directory Parser

Family selection:

```text
version >= 0x00020000
```

Initial header map:

| Offset | Width | Initial label | Evidence |
|---:|---:|---|---|
| `0x00` | 4 | magic | exact runtime |
| `0x04` | 4 | version | exact runtime |
| `0x08` | 4 | product target | exact runtime |
| `0x0c` | 4 | header word 0 / possible checksum | unknown semantic |
| `0x10` | 4 | declared file size | corpus relation to validate |
| `0x14` | 4 | header word 2 | unknown |
| `0x18` | 4 | directory locator | exact runtime access |
| `0x1c` | 4 | header word 4 | unknown |
| `0x20` | 4 | header word 5 | unknown |
| `0x24` | 4 | flags | exact bit extraction, semantic partial |
| `0x28` | 4 | header word 7 | unknown |
| `0x2c` | 4 | header word 8 | unknown |

Directory preamble at the validated locator:

```text
+0x00 directory-record array offset
+0x04 directory-record size
+0x08 directory-record count
```

Observed directory records are 44 bytes/11 little-endian words. Preserve all
words with neutral labels:

```text
type
data_offset_low
data_offset_high
data_size_low
data_size_high
word_5
word_6
word_7
record_stride
word_9
word_10
```

Combine high/low offset or size words only where exact code performs that
operation. A corpus relation is not enough to rename a field.

For every directory record:

- validate the record itself;
- validate its referenced data range;
- record type, range, stride, record count where divisible;
- reject impossible overlaps between the header/directory and referenced
  section data;
- permit repeated zero-sized ranges but label them;
- fingerprint payloads without emitting them.

Section types 1 and 2 are parsed as IO. All other types remain
`opaque_section_type_N` until independent code/source evidence supplies a
name.

## 13. v2 IO Records

The exact Wave parser and stock corpus establish an observed 112-byte IO
record:

```text
+0x00 u32 dimension_count
+0x04 u32 dimensions[6]
+0x1c u32 data_format
+0x20 u32 data_type
+0x24 u32 quantization_format
+0x28 u32 quantization_word_0
+0x2c u32 quantization_word_1
+0x30 char name[64]
```

The runtime normalizes this into 128 bytes. PoC 2 emits the raw two
quantization words plus a conservative interpretation:

- quantization format 1: word 0 is fixed position; word 1 remains preserved;
- other observed formats: word 0 is IEEE-754 scale and word 1 is signed zero
  point, matching the exact runtime copy pattern;
- unknown formats remain raw.

Tensor byte length is derived only for known data formats and positive
dimensions. Unknown dtypes produce `element_bytes: null`, not a guessed size.

## 14. Opaque-Region Fingerprints

For each bounded opaque section or gap emit:

- offset and size;
- SHA-256;
- SHA-256 of the first and last at most 4096 bytes;
- zero-byte ratio to six decimal places;
- Shannon entropy in bits/byte to six decimal places;
- 64 KiB chunk-hash Merkle-style summary;
- optional normalized structural key based only on type, size, stride, and
  count.

Do not emit:

- raw bytes;
- printable-string dumps;
- NPU command words;
- weights;
- connection-tail contents.

Fingerprint calculations stream in bounded chunks. No whole multi-hundred-MB
section copy is allocated.

## 15. Corpus Analysis

The generated summary includes:

- total files and bytes;
- format/version/product clusters;
- parse success/error counts;
- unique full-file hashes and exact duplicate groups;
- directory record sizes and counts;
- observed section-type set;
- per-type size/stride/count distributions;
- input/output count and shape distributions;
- data format, data type, and quantization distributions;
- network-name presence;
- declared-size agreement;
- connection-tail uniqueness;
- model-family groups derived from path only;
- structural signatures that intentionally exclude payload bytes.

Similarity claims must distinguish:

- exact duplicate: same full SHA-256;
- structure duplicate: same parsed structural signature;
- payload-related: matching bounded chunk hashes;
- path-family inference: filename/directory relationship only.

No similarity score is treated as proof of operation identity.

## 16. Deterministic CLI

Subcommands:

```bash
python3 waved/scripts/npu/nbg_format.py parse PATH
python3 waved/scripts/npu/nbg_format.py inventory
python3 waved/scripts/npu/nbg_format.py summary
python3 waved/scripts/npu/nbg_format.py verify-generated
```

`parse` accepts only a local path and emits one bounded JSON document. It
never modifies the file.

`inventory` writes the canonical generated JSON path by default, with an
optional repository-contained `--output` for development.

`summary` reads a validated inventory and writes the canonical Markdown
summary.

`verify-generated` regenerates both outputs in memory or temporary files and
compares bytes with the tracked files. It accepts no path override so CI
cannot accidentally verify a different corpus.

JSON uses:

- UTF-8;
- sorted keys;
- two-space indentation;
- one trailing newline;
- deterministic float formatting;
- no timestamps, hostnames, absolute paths, inode numbers, or mtimes.

## 17. Test Matrix

### Configuration

- valid strict configuration;
- duplicate JSON key;
- unknown top-level and nested key;
- duplicate ID;
- invalid role;
- absolute path;
- `..` escape;
- symlink;
- wrong extension;
- missing required collection;
- expected-count mismatch;
- duplicate physical path;
- reference hash mismatch.

### Common envelope

- exact minimum valid header;
- every truncation from byte 0 through byte 12;
- wrong magic;
- unsupported version family;
- product and version little-endian behavior;
- 512-byte-tail minimum.

### v1

- supported public-prefix fixture;
- `0x0001001e` compatible-prefix fixture;
- truncation at every parsed header word;
- oversized header counts;
- bad input/output table range;
- table size not divisible by record size;
- dimension count 0 and 7;
- zero dimension;
- unterminated 64-byte name;
- tensor-size overflow limit;
- opaque fixed-extension preservation;
- real foreign add fixture metadata.

### v2

- valid synthetic directory;
- directory locator at each boundary;
- record size 0, 43, 44, and oversized;
- count 0, 1, 256, and 257;
- multiplication/range overflow;
- truncated directory;
- section outside file;
- section overlapping header/directory;
- duplicate zero-sized section;
- input/output stride 0, 111, 112, and 113;
- section size not divisible by stride;
- all dimension counts;
- fixed and affine quantization interpretation;
- invalid name encoding behavior;
- final 512-byte tail present/truncated;
- real stock CTC metadata.

### Determinism and fuzz-like sweeps

- parse each prefix truncation of small synthetic fixtures;
- flip each byte in the common and directory header of a tiny fixture;
- seeded random byte strings across bounded sizes;
- repeated parse produces identical canonical JSON;
- parser never raises an exception other than `NbgParseError` for expected
  malformed input;
- generated inventory and summary verification.

This is host parser robustness testing, not device fuzzing.

## 18. Differential Validation

Required:

1. Parse the live-proven stock CTC hash.
2. Assert version, product, input/output count, shapes, formats, and derived
   byte sizes against PoC 0.
3. Parse the foreign add fixture and assert version/product remain distinct.
4. Compare legacy fields with pinned TIM-VX source.
5. Compare v2 field accesses with exact PoC 1 Ghidra evidence.
6. Confirm all 137 stock models classify without parser errors.

Optional only where version-compatible:

- compile the public TIM-VX parser as a host reference;
- compare its legacy fixture output.

The public parser’s rejection of newer versions is a version-boundary fact,
not evidence that the Wave file is malformed.

## 19. Documentation

`docs/npu/format/README.md` explains:

- result and non-claims;
- exact corpus;
- how to reproduce;
- coverage and parser limitations;
- relationship to PoCs 0 and 1;
- next decision.

`field-map.md` records every interpreted field with:

- family;
- offset/width;
- neutral name;
- semantic interpretation;
- evidence source;
- confidence;
- corpus invariants;
- failure behavior;
- unresolved alternatives.

`corpus-summary.md` is generated and contains no hand-edited conclusions.

The notebook records:

- exact commands;
- input/config/output hashes;
- test counts;
- corpus counts and bytes;
- observed distributions;
- negative findings;
- parser coverage;
- unknowns;
- whether the exit gate passed.

## 20. Implementation Sequence

### Commit A — plan and strict corpus configuration

1. Add this plan.
2. Add failing configuration tests.
3. Implement strict config loading and corpus discovery.
4. Pin reference hashes and expected stock count.
5. Commit once tests pass.

### Commit B — common and v1 parser

1. Add checked reader and structured errors.
2. Add envelope tests.
3. Add legacy fixed-prefix parser.
4. Add IO parser and tensor-size derivation.
5. Validate the foreign add fixture.
6. Commit once malformed-input tests pass.

### Commit C — v2 directory and IO parser

1. Add synthetic v2 tests.
2. Implement header/directory bounds.
3. Preserve all directory words.
4. Parse type 1/2 IO records.
5. Fingerprint opaque sections and connection tail.
6. Validate the stock CTC against PoC 0.
7. Commit once all parser tests pass.

### Commit D — corpus inventory and generated summary

1. Scan all configured files.
2. Generate canonical inventory.
3. Generate Markdown summary.
4. Add byte-for-byte verifier.
5. Add generated-output regression tests.
6. Commit generated artifacts and tooling.

### Commit E — field map, notebook, and project indexes

1. Write the evidence-backed field map.
2. Write PoC 2 README and notebook.
3. Update program and harness indexes.
4. Graduate durable facts to compute-hardware docs.
5. Run the complete verification suite.
6. Commit the PoC 2 result.

## 21. Verification Commands

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -p 'test_*.py' -v
python3 -m py_compile \
  waved/scripts/npu/nbg_format.py \
  waved/scripts/npu/tests/test_nbg_format.py

python3 waved/scripts/npu/nbg_format.py parse \
  waved/third_party/stock-npu-stack/sample-ctc.nb

python3 waved/scripts/npu/nbg_format.py verify-generated

cargo test --manifest-path waved/Cargo.toml -p waved-npu-probe
git diff --check
```

The Rust suite is retained as a cross-check for the live ABI and recorded
stock metadata; PoC 2 does not move the host parser into Rust.

## 22. Exit Gate

PoC 2 passes only if:

- every configured stock NBG is classified;
- required corpus count and reference hashes match;
- v1 and v2 supported families parse without unhandled exceptions;
- every parsed range and count is bounded;
- truncation/corruption tests cover each parsed field family;
- all unknown sections are preserved and fingerprinted rather than guessed;
- the stock CTC metadata matches PoC 0 exactly;
- the foreign add target remains explicitly incompatible;
- generated JSON and Markdown verify byte-for-byte;
- documentation separates proven, inferred, opaque, and unknown fields;
- no device operation occurred.

PoC 2 failure does not authorize a native parser call, product patch, or
device experiment.

## 23. Decision After PoC 2

If the gate passes:

- PoC 3 may trace the exact stock CTC lifecycle using the existing unchanged
  graph and compare runtime phases with parsed sections.
- Compiler research may use the structural signatures to reject obviously
  foreign outputs before device execution.
- Export-data recovery may proceed independently with its own strict grammar.

PoC 2 does not by itself authorize:

- executing a mutated NBG;
- executing a handcrafted export-data graph;
- raw galcore tracing or command replay;
- compiling or running a neural denoiser.

The next graph-execution gate still requires a genuine Wave target or a fully
reconstructed and independently reviewed export-data representation.
