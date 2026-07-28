# PoC 2 — NBG Corpus and Format Explorer

**Date:** 2026-07-28  
**Status:** Passed  
**Safety class:** Host-only parsing of immutable local files  
**Device operations:** None  
**Depends on:** PoC 0 live control and PoC 1 exact runtime ABI  
**Plan:** [`2026-07-28-wave-npu-poc2-nbg-corpus-format.md`](../../superpowers/plans/2026-07-28-wave-npu-poc2-nbg-corpus-format.md)

## Hypothesis

A bounded host parser can recover the Wave NBG envelope, section directory,
I/O tensor metadata, graph metadata, memory requirements, and integrity
records from the available stock corpus without executing native code or
inventing semantics for accelerator payloads.

The hypothesis is falsified if any of the following occurs:

- exact stock files cannot be described without unchecked offsets or
  unbounded allocations;
- the live-tested CTC metadata disagrees with the static parse;
- public v2 structures do not match the exact Wave byte layout;
- a configured stock file falls outside the parser's supported v2 grammar;
- stored section checksums cannot be reproduced consistently;
- malformed inputs escape the structured error boundary;
- generated evidence is nondeterministic;
- parsing requires ADB, a firmware library, or model execution.

## Safety gate

Allowed:

- read and hash local `.nb` files;
- inspect pinned public source;
- consult existing PoC 1 Ghidra evidence;
- generate malformed test-owned bytes in memory;
- create canonical JSON and Markdown;
- run Python and Rust host tests.

Excluded:

- ADB;
- `/dev/galcore`;
- `libnnsdk.so`, OpenVX, or any other native firmware execution;
- device staging;
- graph mutation on the device;
- product-ID patching;
- raw driver commands;
- firmware, service, sysfs, module, or boot changes.

No device command was issued during PoC 2.

## Inputs

### Exact corpus configuration

```text
waved/scripts/npu/nbg_corpus.json
SHA-256 2aac501653893d3fb96478dfa27039d5bd6fdc3086f9486ae4b08bb5c4db64db
```

Configured inputs:

```text
137 stock Wave .nb files
  3,782,869,224 bytes

1 foreign legacy add negative control
  2,944 bytes
  SHA-256 9bef0c0f2271ad534cb975b386be8428e973dc7ee4c71c04854c95eeac1fea5c

1 live-tested stock CTC reference
  2,810,984 bytes
  SHA-256 5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66
```

Combined:

```text
139 files
3,785,683,152 bytes
```

The extracted stock root is operator-provided and gitignored. Required file
count drift fails inventory construction.

### Exact implementation

```text
waved/scripts/npu/nbg_format.py
SHA-256 66b1b03a17dc95bb153664e17b26cf89b37e312f6f0611576e92cfc17c7eaa0a
2,894 lines

waved/scripts/npu/tests/test_nbg_format.py
SHA-256 283b84bc319651831c542c21529bfd7ed99acede2c74a921edd2c870a477503b
963 lines
```

### Pinned public sources

#### VIPLite 2.0.0 v2 format

```text
repository https://github.com/Ameba-AIoT/ameba-arduino-pro2
commit     b83b89f3842cf2938c8ce8adec9dee1864b7d72a
file       .../VIPLiteDrv_2.0.0/nbg_linker/inc/nbg_fmt_v2.h
SHA-256    6af06536a70b2534c96ba384e775a19d2e9ef73acfb6817bd7975d4fd9cdc35b
```

Supporting public header:

```text
file       .../VIPLiteDrv_2.0.0/include/nbg_linker/vip_lite.h
SHA-256    d0fd4ea3c98a032f7e8c3045d1d27972b6f1745c1b1e377aeffcc16cc9eb138f
```

Additional directly read dependencies:

```text
nbg_fmt_descriptor.h
SHA-256 9845c8d2231a1c627928c231ad2bccd84b5bd38b4268bc0b85fb4125eff85ef3

nbg_fmt_util.h
SHA-256 35db9be3c41749c6bc8b493eee6056074bd177f427503b86bcf14bea29d9782c

nbg_fmt_v1.h
SHA-256 da033621a39d2dbc07ce19e2ffcb8c401bd96dfadcf063e2add3e8fd955d5289

vip_lite_common.h
SHA-256 9593b14d011f212a971dd1f0003d2a97cc1429e485cc62a9cb75841cd958275f

vip_lite_config.h
SHA-256 2466a0f3edf29187572bd92cccce0da63ffe4218b6226b1bbfb01d1b9272691f
```

#### TIM-VX public parser API

```text
repository https://github.com/VeriSilicon/TIM-VX
commit     702715d714277376d7ff466b7b72833ff57564c1
file       include/tim/utils/nbg_parser/nbg_parser.h
SHA-256    ef2e3eef354150f2643dad5ce0fa4c330dfb221389feba02c64eecc6cb81b5c8
```

The TIM-VX API independently supplies buffer-format, buffer-type, and
quantization enums. The newer VIPLite header adds sub-byte formats.

### Exact Wave runtime evidence

PoC 1 already pinned the Wave libraries and exported the relevant parser
functions. This notebook relies on:

- `parse_nb_head_info` / `FUN_00113810`;
- `parse_nb_io_info` / `FUN_00113a20`;
- `read_nbg_from_memory`;
- exact diagnostic strings naming the v2 section enum;
- the live CTC loader lifecycle.

See [`../abi/README.md`](../abi/README.md) and
[`2026-07-28-poc1-runtime-abi.md`](2026-07-28-poc1-runtime-abi.md).

## Implementation record

### 1. Strict corpus discovery

The configuration loader rejects:

- unknown or duplicate JSON keys;
- schema drift;
- duplicate IDs or paths;
- invalid roles;
- absolute or escaping paths;
- symlink components;
- non-`.nb` inputs;
- missing required collections;
- expected-count drift;
- reference hash mismatch.

Discovery ordering uses collection ID and bytewise relative path. It records
no timestamps, inodes, hostnames, or absolute workspace paths.

### 2. Checked reader and structured failures

All format-controlled offsets, counts, products, and ranges are validated
before a read or allocation. Expected failures become `NbgParseError` with a
stable code, stage, bounded context, and optional byte offset.

Limits:

```text
section headers       256
I/O records/direction 256
dimensions            6
tensor storage        1 TiB
connection tail       512 bytes
fingerprint chunk     64 KiB
edge fingerprint      4 KiB
```

The initial I/O limit was 64. The first full scan stopped safely at
`asr_ch/online_zipformer/zf_fixmean.nb`, whose input and output sections each
contain 75 records. The bound was reviewed and raised to 256; a 257-record
test now proves the refusal edge.

### 3. Legacy v1 compatible prefix

The foreign add graph parsed as:

```text
version       0x0001001e
target        0x10000020
network       synthetic add fixture
inputs        2 × UINT8 [2]
outputs       1 × UINT8 [2]
I/O stride    116 bytes
opaque gap    8 bytes
```

Its version exceeds the pinned public parser's declared maximum
`0x00010014`. The parser therefore decodes the compatible public prefix and
fingerprints the remaining fixed-header gap instead of assigning fields.

### 4. v2 layout correlation

Before the public v2 header was found, exact Wave accesses and corpus
relations established:

```text
48-byte header
12-byte directory preamble
44-byte directory records
112-byte I/O records
type 1 input
type 2 output
final 512-byte connection region
```

The commit-pinned VIPLite header then resolved the neutral words as:

```text
nbg_format_header_t
nbg_sub_header_t
nbg_section_header_t
nbg_section_input_output_t
nbg_section_network_info_t
nbg_section_memory_pool_t
nbg_section_vip_sram_t
nbg_section_checksum_t
```

Its sizes and offsets match the Wave files exactly. The full section enum
also matches names already present in the exact runtime strings.

### 5. Tensor enum correction

The first corpus pass exposed 42 I/O records with raw format 8. The pinned
TIM-VX and VIPLite enums identify 8 as signed INT32. After updating the
mapping:

```text
FP32   21 records
FP16   421 records
UINT8  3 records
INT32  42 records
```

All 487 records are public buffer type 0, tensor.

### 6. Network and memory metadata

Each v2 graph has one 220-byte network-info record. The parser now:

- decodes its bounded name and counts;
- rejects counts beyond limits;
- cross-checks input/output counts against section records;
- decodes the public attribute bitfield;
- summarizes vendor-reserved words without naming them;
- decodes memory-pool and SRAM requirements.

Corpus-wide:

```text
cores                 1 in all 138 graphs
address width         32 in all 138 graphs
NN command size       192 bytes in all 138 graphs
TP command size       192 bytes in all 138 graphs
VIP SRAM              262,144 bytes in all 138 graphs
memory pool minimum   161,280 bytes
memory pool maximum   57,445,376 bytes
memory pool values    34 unique
AXI SRAM section      absent
```

Every graph's declared hardware-run operation count is exactly its saved
type-4 operation-record count plus three.

### 7. Checksum validation

The public v2 structure names each checksum record as:

```text
u32 section type
u32 checksum value
```

For the live CTC graph, `zlib.crc32` exactly reproduced all five records.
The parser was then changed to calculate CRC-32 during its existing streaming
fingerprint pass and require:

- exactly one referenced section;
- exact CRC equality;
- no successful result on mismatch.

The complete corpus then passed:

```text
v2 graphs             138
checksums per graph   5
stored records        690
matching records      690
mismatches            0
```

Protected types are NN, TP, coefficient data, PPU instructions, and load
state.

The header `crc_sum` did not match tested standard variants over whole-file
ranges, including skipped versus zeroed checksum bytes and inclusion versus
exclusion of the 512-byte tail. It remains recorded but unenforced.

## Commands

Parser and all Python NPU tests:

```bash
python3 -m py_compile \
  waved/scripts/npu/nbg_format.py \
  waved/scripts/npu/tests/test_nbg_format.py

python3 -m unittest discover \
  -s waved/scripts/npu/tests \
  -p 'test_*.py' -v
```

Observed:

```text
72 tests passed
0 failed
```

The PoC 2 module contributes 23 test methods. Parameterized loops cover
additional malformed boundaries, truncations, byte flips, and seeded random
inputs.

Rust ABI regression:

```bash
cargo test --manifest-path waved/Cargo.toml -p waved-npu-probe
```

Observed:

```text
15 passed
0 failed
```

One exact parse:

```bash
python3 waved/scripts/npu/nbg_format.py parse \
  waved/third_party/stock-npu-stack/sample-ctc.nb
```

Corpus generation:

```bash
python3 waved/scripts/npu/nbg_format.py inventory
python3 waved/scripts/npu/nbg_format.py summary
```

Byte-for-byte verification:

```bash
python3 waved/scripts/npu/nbg_format.py verify-generated
```

## Generated outputs

```text
docs/npu/format/stock-nbg-inventory.json
lines      150,848
bytes      4,556,649
SHA-256    48c19e2f47d82a65eaf16af6e5aec0ed3beb322e679a0cfa91d736894f97b49c

docs/npu/format/corpus-summary.md
lines      281
bytes      9,403
SHA-256    75b81a80e01893296511a2cbe709e967c7ccbc4c29b5972f3031c5d9cb47dc30
```

The inventory embeds the generator SHA, corpus configuration SHA, source
reference commits and hashes, parser limits, file hashes, structural
signatures, decoded metadata, and section fingerprints.

## Corpus result

### Version and target

```text
138 files  version 0x00020000  target 0x1000000e
  1 file   version 0x0001001e  target 0x10000020
```

The foreign graph remains explicitly a negative control. No byte was changed
to make it resemble the Wave target.

### Directory variants

```text
18 section headers  27 graphs
20 section headers 111 graphs
```

The two additional sections are dynamic I/O data and its table.

Observed v2 type set:

```text
1, 2, 3, 4, 7, 8, 9, 10, 19, 20,
21, 23, 24, 25, 26, 27, 35, 37, 38, 40
```

All load-address and section-reserved words are zero. Coefficient data is
64-byte aligned and PPU instructions are 256-byte aligned.

### I/O population

```text
v2 inputs   262
v2 outputs  222
all inputs  264
all outputs 223
```

Graph I/O count distribution:

```text
1 / 1    103 files
2 / 1     27 files
2 / 2      2 files
4 / 1      2 files
5 / 3      4 files
75 / 75    1 file
```

### Duplicates

There are 137 unique full hashes among 139 records:

1. Japanese `ctc_online.nb` and `ja_online.nb` are byte-identical.
2. Stock German online CTC and the local live reference are byte-identical.

There are 135 structural signatures. A structural match covers envelope,
section layout, and I/O metadata only; it is not operation or weight
identity.

### Connection tail

The fixed 512-byte connection region has 30 unique SHA-256 values. Its
content remains opaque.

## Live differential result

For exact model hash
`5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66`,
the parser reports:

```text
version       0x00020000
target        0x1000000e
network       de_online_ctc_NCHW
input         FP16 [256, 16, 1]
input bytes   8,192
output        FP32 [5200, 16, 1]
output bytes  332,800
```

The static I/O metadata exactly matches the PoC 0 live result. The parser
also adds source-backed metadata not exposed by the original probe:

```text
layers                 5
absolute operations    5
hardware-run ops       9
saved operation records 6
address width          32
memory pool            167,680 bytes
VIP SRAM               262,144 bytes
```

## Positive findings

- The public VIPLite v2 schema is an exact structural match for the Wave
  graph envelope.
- The parser classifies all configured files without native execution.
- Every interpreted range is bounded and every opaque section is
  fingerprinted.
- The exact live graph agrees with the static tensor description.
- All 690 per-section checksum values are standard CRC-32 and match.
- INT32 state/control tensors are now correctly distinguished from unknown
  formats.
- Network, command-width, address-width, and memory metadata are recovered.
- Generated evidence is canonical and source-identifying.

## Negative findings

- The available foreign add graph remains target `0x10000020`.
- No Wave-target compiler was found or produced.
- The header `crc_sum` algorithm/coverage was not reproduced.
- No graph has AXI-SRAM metadata in this corpus.
- No v2 graph exposes quantized I/O; internal quantization remains separate.
- Commands, coefficients, patches, dynamic I/O, load states, PPU
  instructions, and the connection tail are not semantically decoded.
- PoC 2 provides no evidence that editing a section and recomputing its CRC
  would produce a valid or safe graph.

## Inferences

- The identical command-width attribute and section entry sizes indicate a
  single compiler/runtime profile across the corpus.
- The consistent three-operation delta likely represents implicit lifecycle
  or synchronization operations, but its meaning is not yet established.
- The per-section checksum table is probably used as an integrity mechanism
  by the linker or runtime. PoC 2 proves the encoding, not the runtime
  enforcement point.

## Unknowns

- NPU instruction encoding;
- exact relocation application order;
- dynamic-I/O grammar;
- connection-tail grammar;
- header checksum algorithm;
- whether a usable target compiler exists in an obtainable BSP;
- full private export-data grammar;
- operation support for a streaming denoiser;
- performance and numerical behavior of custom audio graphs.

## Exit-gate decision

PoC 2 passes:

- 139 configured files classified;
- 137/137 stock count gate satisfied;
- reference hashes exact;
- v1 and v2 supported families bounded;
- malformed tests remain inside structured errors;
- exact CTC static/live metadata agrees;
- 690/690 section checksums agree;
- generated outputs are deterministic;
- no device or native runtime operation occurred.

PoC 3 is now allowed to trace the unchanged, hash-pinned stock CTC graph and
correlate runtime phases with named sections.

PoC 2 does not authorize:

- execution of a mutated graph;
- execution of a foreign graph with patched target bytes;
- handcrafted export-data execution;
- raw `/dev/galcore` command replay;
- driver reset or service replacement;
- a noise-cancellation integration claim.
