# PoC 12C — DYNAIO and Serializer File-CRC Profile

- **Date:** 2026-07-29
- **Mode:** host-only static analysis and corpus validation
- **Device calls:** 0
- **NBG loads/executions:** 0 / 0
- **Result:** `dyna-io-read-and-file-crc-qualified`

## Outcome

PoC 12C resolved both unknown ranges left by the controlled PoC 12B capture:

- section 20 is a 64-byte-granular DYNAIO data region;
- section 26 is an array of 8-byte `(offset, size)` records into that region;
- the controlled graph has one table entry `(0,4)`;
- those four bytes are exactly A `[0,1,2,3]` and B `[0,1,2,4]`;
- the header `crc_sum` is reflected IEEE CRC-32 over section payloads in
  directory order, starting at zero and excluding alignment gaps;
- type-40 record type words participate in that CRC, but their checksum-value
  words are treated as zero because the producer fills them afterward.

Starting only from A, the host profiler:

1. replaced the mapped four-byte DYNAIO entry with B;
2. recomputed the recovered header checksum;
3. made no type-40 change;
4. compared the in-memory result with the captured B file.

The reconstruction matched every byte and had B's exact SHA-256:

```text
c9e3e18414f5bc674d4204921966ba53b08918ddcef73112003b065b9599f227
```

The profiler did not write the reconstructed NBG to disk.

## Hypotheses

### H1 — Header checksum

The four-byte header change is the output of a standard incremental CRC
function, but the serializer hashes a selected stream rather than the whole
final file.

### H2 — DYNAIO ownership

The section-26 record identifies the meaningful byte range in section 20,
and the controlled constant occupies that range directly.

### H3 — Corpus generality

If these are real format contracts rather than a one-file coincidence, every
stock Wave v2 graph should reproduce its stored header checksum, and every
stock DYNAIO table should satisfy the same geometry.

All three hypotheses passed.

## Exact inputs

| Input | SHA-256 |
|---|---|
| Wave `libOpenVX.so` | `f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb` |
| PoC 12B A1/A2 | `4a84faf4da4604429522dfa6f0a8eb9887287eecb0f97d697843e0beb78c8a8b` |
| PoC 12B B | `c9e3e18414f5bc674d4204921966ba53b08918ddcef73112003b065b9599f227` |
| save-routine Ghidra export | `177e1a78191bdde750dec8b21e2cf260fde94b7d0232377642fdece2b601f3e9` |
| CRC-routine Ghidra export | `b1e80fd54ea3be3c183336fccbd78b7e81bd4ba12ec77717b227396609fa420f` |
| regenerated corpus inventory | `7103775e6659117bea2eafef353aece3ff75c5046a7fcb198a8a50e8d3dc1c6c` |

The tracked machine evidence is
[`dyna-io-crc-profile.json`](../format/dyna-io-crc-profile.json), SHA-256
`734151668237b7b3387c29f23e52375eaf1b2926060c9220dbf75716b0a1ddeb`.

## Public format corroboration

The pinned VIPLite 2.0.0 headers independently provide:

- section ID 20 as `NBG_SECTION_DYNAIO`;
- section ID 26 as `NBG_SECTION_DYNAIO_TABLE`;
- an 8-byte 32-bit-class data-table record containing offset and size;
- a descriptor with `dyna_io`, `dyna_io_table`, and `n_dyna_io`;
- dynamic generic/input/output patch-source categories;
- a header field named `nbg_crc_sum`.

Primary sources:

- [`nbg_fmt_v2.h` at the pinned Ameba commit](https://github.com/Ameba-AIoT/ameba-arduino-pro2/blob/b83b89f3842cf2938c8ce8adec9dee1864b7d72a/Arduino_package/hardware/system/project/realtek_amebapro2_v0_example/src/VIPLiteDrv_2.0.0/nbg_linker/inc/nbg_fmt_v2.h);
- [`nbg_fmt_descriptor.h` at the same commit](https://github.com/Ameba-AIoT/ameba-arduino-pro2/blob/b83b89f3842cf2938c8ce8adec9dee1864b7d72a/Arduino_package/hardware/system/project/realtek_amebapro2_v0_example/src/VIPLiteDrv_2.0.0/nbg_linker/inc/nbg_fmt_descriptor.h);
- [`nbg_fmt_util.h` at the same commit](https://github.com/Ameba-AIoT/ameba-arduino-pro2/blob/b83b89f3842cf2938c8ce8adec9dee1864b7d72a/Arduino_package/hardware/system/project/realtek_amebapro2_v0_example/src/VIPLiteDrv_2.0.0/nbg_linker/inc/nbg_fmt_util.h).

The public header names and the exact Wave runtime/corpus behavior are
independent evidence. The public source alone does not prove the Wave
producer's temporal checksum order.

## Static runtime recovery

Exact-string cross-references found the save routine at Ghidra entry
`002d2f20`. Its bounded decompilation shows this sequence:

```text
global file CRC = 0
for each section descriptor in serializer order:
    write any alignment bytes without updating the file CRC
    write the section payload
    update global file CRC with the payload
store global CRC into header offset 12
write the updated header
populate final type-40 per-section checksum values
rewrite the type-40 checksum section
```

The update primitive is the 160-byte function at Ghidra entry `002c8160`.
Its recovered properties are:

```text
polynomial     0xedb88320
reflected      yes
input          prior CRC plus byte buffer
precondition   complement prior CRC
postcondition  complement result
empty update   returns prior CRC
```

This is the same incremental convention exposed by Python's
`zlib.crc32(buffer, prior_crc)`.

The important temporal detail is type 40. Its final serialized records are:

```text
u32 section_type
u32 checksum_value
```

The type words already exist when the global CRC is accumulated. The value
words are still zero and are populated only after `crc_sum` is stored.
Computing over the final type-40 value words therefore produces the wrong
answer.

## Recovered file-checksum algorithm

For each section in directory order:

```text
if section type != 40:
    crc = zlib.crc32(section_payload, crc)
else:
    for each 8-byte record:
        crc = zlib.crc32(section_type_word, crc)
        crc = zlib.crc32(b"\x00\x00\x00\x00", crc)
```

The stream excludes:

- the 48-byte header;
- the 12-byte sub-header;
- the section directory;
- bytes inserted only for physical alignment between sections;
- any other bytes not owned by a section payload.

It includes every section payload, not just the five sections that have
type-40 integrity records.

Controlled results:

| File | Recorded | Computed | Match |
|---|---:|---:|---|
| A1 | `0xd346207e` | `0xd346207e` | yes |
| A2 | `0xd346207e` | `0xd346207e` | yes |
| B | `0x67768f6f` | `0x67768f6f` | yes |

The A/B XOR differential also independently located exactly 4,332 processed
bytes after the changed source byte. That equals the remaining section-20
bytes plus sections 23, 27, 26, 24, and 21, while excluding the 60-byte
alignment gap before coefficient data.

## Full-corpus file-CRC gate

The strict parser regenerated all 139 configured records:

```text
configured files             139
Wave v2 files                138
legacy v1 negative controls    1
v2 file CRC matches          138
v2 file CRC mismatches         0
covered section bytes         3,785,534,000
```

This validation includes 137 stock model files and the exact live CTC
reference. The foreign v1 graph is intentionally outside this v2 algorithm.

The parser now rejects a v2 NBG when its recorded header `crc_sum` does not
equal the recovered computation.

## DYNAIO table qualification

Across the 138 Wave v2 files:

```text
files with both sections 20 and 26  111
files with neither                    27
files with only one                    0
table entries                       8,939
invalid or out-of-bounds entries        0
overlapping entries                     0
unaligned entry offsets                 0
zero-sized entries                      0
```

Every entry offset is 64-byte aligned. Every entry is positive-sized,
ordered, non-overlapping, and bounded by its section-20 region.

Aggregate configured-v2 geometry:

```text
DYNAIO region bytes    201,683,328
mapped payload bytes   201,600,304
padding bytes               83,024
nonzero padding bytes             0
```

Observed entry counts per graph:

| Entries | Files |
|---:|---:|
| 50 | 6 |
| 51 | 66 |
| 64 | 4 |
| 80 | 20 |
| 96 | 2 |
| 97 | 2 |
| 101 | 6 |
| 300 | 1 |
| 361 | 1 |
| 588 | 3 |

The controlled A and B graphs add one entry each:

```text
section 20 offset  1508
section 20 size      64
section 26 offset  1596
entry 0             (offset=0, size=4)
A entry bytes       00 01 02 03
B entry bytes       00 01 02 04
changed file byte   1511
```

## Exact controlled reconstruction

The host-only reconstruction used no observed B bytes except for the final
comparison:

```text
start with exact A
replace section20[0:4] with [0,1,2,4]
recompute header crc_sum
leave the type-40 records unchanged
compare against exact B
```

Result:

```text
exact match              true
first difference         none
reconstructed SHA-256    c9e3e18414f5bc674d4204921966ba53b08918ddcef73112003b065b9599f227
written to disk          false
```

This upgrades the controlled source mapping from an inference to an exact
serialization proof.

## Parser and tool contracts

[`nbg_format.py`](../../../waved/scripts/npu/nbg_format.py) now enforces:

- file-level CRC reconstruction;
- type-40 temporal normalization;
- paired section-20/26 presence;
- 8-byte table records;
- nonempty 64-byte-aligned data regions;
- positive entry sizes;
- 64-byte-aligned entry offsets;
- ordered, non-overlapping, in-bounds ranges;
- zero bytes outside mapped entry ranges.

[`nbg_dyna_profile.py`](../../../waved/scripts/npu/nbg_dyna_profile.py)
adds:

- exact A1/A2 determinism;
- unique direct entry mapping;
- exact in-memory A-to-B reconstruction;
- generated-inventory aggregation;
- exact-runtime Ghidra provenance checks;
- canonical, payload-free evidence generation.

Malformed fixtures cover header mismatch, payload mismatch, final type-40
values, missing section pairs, bad table stride, out-of-bounds entries,
overlap, alignment, zero-sized entries, and nonzero padding.

## What this unlocks

PoC 12C permits host tooling to:

- validate the Wave serializer's v2 header checksum;
- enumerate the byte ranges in DYNAIO;
- identify the exact controlled constant buffer;
- reconstruct this one controlled NBG in memory;
- reason about DYNAIO ownership without treating the full region as opaque.

This is useful for serializer recovery and later constant/tensor
correlations. It is not enough to compile a denoiser.

For a noise-cancellation model, the remaining hard parts are still:

1. operation selection and graph lowering;
2. target-native NN/TP command generation;
3. coefficient packing and quantization;
4. patch/load-state generation;
5. runtime-safe mutation rules;
6. execution and numerical acceptance on a separately approved graph.

The existing ONNX CPU/hybrid path remains the safe general-model route while
those NPU stages are unresolved.

## Findings

### Positive

- Exact serializer file CRC recovered from the Wave runtime.
- 138/138 stock/reference v2 files validate.
- Exact DYNAIO table geometry recovered and enforced.
- 8,939/8,939 configured-v2 entries pass.
- Controlled constant mapping is direct.
- In-memory reconstruction is byte-exact.

### Negative

- The result does not identify semantic roles for all 8,939 stock entries.
- It does not prove that an arbitrary entry may be changed safely.
- It does not recover command or coefficient encoders.

### Inferred

- Many DYNAIO entries likely represent aligned mutable or runtime-provided
  buffers, consistent with the public dynamic source categories. Individual
  input/output/generic roles still require patch-source correlation.

### Unknown

- entry-to-tensor/node identity in general graphs;
- lifetime and mutability rules at NBG load time;
- whether constants of other types use DYNAIO, coefficient data, or both;
- how the private Acuity/export-data serializer chooses between those
  regions.

## Reproduction

Focused tests:

```bash
python3 -m unittest \
  waved.scripts.npu.tests.test_nbg_format \
  waved.scripts.npu.tests.test_nbg_dyna_profile \
  waved.scripts.npu.tests.test_nbg_constant_diff \
  waved.scripts.npu.tests.test_nbg_image
```

Regenerate and verify corpus evidence:

```bash
python3 waved/scripts/npu/nbg_format.py inventory
python3 waved/scripts/npu/nbg_format.py summary
python3 waved/scripts/npu/nbg_format.py verify-generated
```

Regenerate and verify PoC 12C evidence:

```bash
python3 waved/scripts/npu/nbg_dyna_profile.py generate
python3 waved/scripts/npu/nbg_dyna_profile.py verify-generated
```

All commands above are host-only.

## Non-claims

PoC 12C does not claim:

- general ONNX-to-NBG support;
- an Acuity compiler replacement;
- recovered NN/TP command generation;
- recovered coefficient packing;
- a writable general-purpose NBG serializer;
- safe arbitrary graph mutation;
- NPU execution of a noise-cancellation model;
- production authorization.
