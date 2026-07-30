# Wave NBG Format Explorer — PoC 2

PoC 2 passed on 2026-07-28. PoC 12A–12C subsequently extended its
serializer evidence without changing the non-executing safety boundary.
The repository now has a strict parser for the legacy reference graph and
the Wave's NBG v2 files, plus a hash-pinned inventory of every available
stock graph.

The result materially narrows the custom-model problem:

- all 137 stock files use NBG version `0x00020000` and target
  `0x1000000e`;
- the live-tested CTC duplicate parses to exactly the input and output
  metadata observed through `libnnsdk.so`;
- a pinned public VIPLite 2.0.0 header resolves the v2 header, sub-header,
  section-header, tensor, network, memory, and checksum structures;
- every v2 file has one network-info section, one memory-pool section, one
  VIP-SRAM section, and five section-checksum records;
- all 690 stored section checksums equal standard CRC-32 of the referenced
  section bytes;
- all 138 v2 header `crc_sum` values equal incremental reflected IEEE
  CRC-32 over section payloads in directory order, with checksum-value words
  in section type 40 treated as zero;
- all 111 v2 files with dynamic-I/O data pair section 20 with an eight-byte
  `(offset, size)` table in section 26; all 8,939 observed entries are
  positive, ordered, non-overlapping, 64-byte aligned, and in bounds;
- tensor format `8`, previously anonymous in the corpus, is signed INT32;
- every v2 I/O object in this corpus is a tensor; the parser also knows the
  public image, array, and scalar codes;
- all command, coefficient, relocation, and state payloads remain bounded
  and fingerprinted rather than interpreted or executed.

This is a format, integrity, and one controlled serializer-field result. It
is not yet a custom-model unlock. The stock runtime still needs either a
compiler that emits target `0x1000000e` or a fully reconstructed private
export-data graph.

## Evidence chain

The v2 field map is supported by three independent sources:

1. The exact Wave `libnnsdk.so` parser analyzed in PoC 1 establishes the
   offsets, 44-byte section headers, 112-byte I/O records, section types 1
   and 2, and the 512-byte connection tail used by the live runtime.
2. The stock corpus supplies 138 v2 instances, including the exact graph
   executed in PoC 0.
3. A public Ameba Pro2 BSP contains VeriSilicon VIPLite 2.0.0 linker headers
   whose structures exactly match those bytes.
4. Bounded analysis of the hash-pinned Wave `libOpenVX.so` save path, a
   controlled A1/A2/B serializer differential, and the complete v2 corpus
   establish the file-level CRC order and the section-20/26 relationship.

Pinned public sources:

- [`nbg_fmt_v2.h` at `b83b89f`](https://github.com/Ameba-AIoT/ameba-arduino-pro2/blob/b83b89f3842cf2938c8ce8adec9dee1864b7d72a/Arduino_package/hardware/system/project/realtek_amebapro2_v0_example/src/VIPLiteDrv_2.0.0/nbg_linker/inc/nbg_fmt_v2.h),
  SHA-256
  `6af06536a70b2534c96ba384e775a19d2e9ef73acfb6817bd7975d4fd9cdc35b`;
- [`vip_lite.h` at `b83b89f`](https://github.com/Ameba-AIoT/ameba-arduino-pro2/blob/b83b89f3842cf2938c8ce8adec9dee1864b7d72a/Arduino_package/hardware/system/project/realtek_amebapro2_v0_example/src/VIPLiteDrv_2.0.0/include/nbg_linker/vip_lite.h),
  SHA-256
  `d0fd4ea3c98a032f7e8c3045d1d27972b6f1745c1b1e377aeffcc16cc9eb138f`;
- [`nbg_fmt_descriptor.h` at `b83b89f`](https://github.com/Ameba-AIoT/ameba-arduino-pro2/blob/b83b89f3842cf2938c8ce8adec9dee1864b7d72a/Arduino_package/hardware/system/project/realtek_amebapro2_v0_example/src/VIPLiteDrv_2.0.0/nbg_linker/inc/nbg_fmt_descriptor.h);
- [`nbg_fmt_util.h` at `b83b89f`](https://github.com/Ameba-AIoT/ameba-arduino-pro2/blob/b83b89f3842cf2938c8ce8adec9dee1864b7d72a/Arduino_package/hardware/system/project/realtek_amebapro2_v0_example/src/VIPLiteDrv_2.0.0/nbg_linker/inc/nbg_fmt_util.h);
- [TIM-VX `nbg_parser.h` at `702715d`](https://github.com/VeriSilicon/TIM-VX/blob/702715d714277376d7ff466b7b72833ff57564c1/include/tim/utils/nbg_parser/nbg_parser.h),
  SHA-256
  `ef2e3eef354150f2643dad5ce0fa4c330dfb221389feba02c64eecc6cb81b5c8`.

The public availability of a header does not settle its redistribution
license. This repository records derived field facts and hashes; it does not
copy the vendor header.

Exact Wave evidence is indexed under [`../abi/README.md`](../abi/README.md).
The byte-level interpretation and confidence for each field is in
[`field-map.md`](field-map.md).

## Corpus identity

The strict configuration is
[`waved/scripts/npu/nbg_corpus.json`](../../../waved/scripts/npu/nbg_corpus.json).
It resolves:

| Role | Files | Meaning |
|---|---:|---|
| `stock` | 137 | Extracted Wave model bundle under `models/` |
| `reference` | 1 | Exact local duplicate of the live-tested German CTC |
| `negative_control` | 1 | Foreign-target legacy add graph |

Combined identity:

```text
files                          139
bytes                          3,785,683,152
unique full-file hashes        137
unique structural signatures  135
config SHA-256                 2aac501653893d3fb96478dfa27039d5bd6fdc3086f9486ae4b08bb5c4db64db
inventory SHA-256              7103775e6659117bea2eafef353aece3ff75c5046a7fcb198a8a50e8d3dc1c6c
```

Version/target split:

| Files | Version | Target | Interpretation |
|---:|---:|---:|---|
| 138 | `0x00020000` | `0x1000000e` | Wave v2 graphs, including the reference duplicate |
| 1 | `0x0001001e` | `0x10000020` | Foreign legacy negative control |

The generated human summary is
[`corpus-summary.md`](corpus-summary.md). The complete machine evidence is
[`stock-nbg-inventory.json`](stock-nbg-inventory.json).

## v2 envelope

Every Wave-format file has this outer layout:

```text
0x0000  48-byte nbg_format_header
0x0030  12-byte nbg_sub_header
0x003c  18 or 20 × 44-byte nbg_section_header
        typed, aligned section payloads through declared EOF
        runtime also reads the final 512 bytes as a connection-region view
```

Across all 138 v2 files:

- header class is 0;
- total-size high word is 0 and the combined declared size equals the file
  size;
- sub-header offset is 48, size is 12, and count is 1;
- all header reserved fields are zero;
- section-header count is 18 in 27 files and 20 in 111 files;
- the 20-section variant adds `NBG_SECTION_DYNAIO` and
  `NBG_SECTION_DYNAIO_TABLE`;
- all section load-address and reserved words are zero in the serialized
  files;
- all network address widths are 32 bits;
- the coefficient-data section requests 64-byte alignment;
- the PPU-instruction section requests 256-byte alignment.

The header field named `crc_sum` by the public source has 136 unique values.
PoC 12C recovered its exact producer order:

1. initialize the CRC accumulator to zero;
2. update it with each section payload in directory order;
3. exclude the header, sub-header, section directory, and inter-section
   alignment gaps;
4. include each type-40 checksum record's section-type word, but substitute
   zero for its checksum-value word;
5. write the resulting reflected IEEE CRC-32 to header offset `0x0c`;
6. fill the type-40 checksum values afterward.

The parser enforces this file-level checksum independently of the five
per-section checksums. All 138 Wave v2 graphs match, covering
3,785,534,000 section bytes. The legacy v1 negative control is not assigned
the v2 rule.

## Observed section profile

Twenty v2 section types occur:

| Type | Public name | Files | Entry size |
|---:|---|---:|---:|
| 1 | `NBG_SECTION_INPUT` | 138 | 112 |
| 2 | `NBG_SECTION_OUTPUT` | 138 | 112 |
| 3 | `NBG_SECTION_LAYER` | 138 | 76 |
| 4 | `NBG_SECTION_OPERATIONS` | 138 | 32 |
| 7 | `NBG_SECTION_NN` | 138 | 192 |
| 8 | `NBG_SECTION_TP` | 138 | 192 |
| 9 | `NBG_SECTION_SH` | 138 | 8 |
| 10 | `NBG_SECTION_PATCH` | 138 | 28 |
| 19 | `NBG_SECTION_PPU_INSTR` | 138 | 1 |
| 20 | `NBG_SECTION_DYNAIO` | 111 | 1 |
| 21 | `NBG_SECTION_LOADSTATE` | 138 | 1 |
| 23 | `NBG_SECTION_COEF_DATA_TABLE` | 138 | 8 |
| 24 | `NBG_SECTION_COEF_DATA` | 138 | 1 |
| 25 | `NBG_SECTION_PPU_INSTR_TABLE` | 138 | 8 |
| 26 | `NBG_SECTION_DYNAIO_TABLE` | 111 | 8 |
| 27 | `NBG_SECTION_LOADSTATE_TABLE` | 138 | 8 |
| 35 | `NBG_SECTION_NETWORK_INFO` | 138 | 220 |
| 37 | `NBG_SECTION_MEMORY_POOL` | 138 | 8 |
| 38 | `NBG_SECTION_VIP_SRAM` | 138 | 4 |
| 40 | `NBG_SECTION_CHECKSUM` | 138 | 8 |

Public structure names establish the section roles. The parser semantically
decodes I/O, network info, memory sizes, checksum records, and the geometry
of the dynamic-I/O data/table pair. It does not assign per-entry tensor or
operation roles, and it does not decode or emit NN/TP commands, coefficient
bytes, patch records, load states, or PPU instructions.

The network attribute word is identical in all v2 files:

```text
change_ppu_param    0
NN command code     1 = 192 bytes
TP command code     1 = 192 bytes
DMA support code    0
reserved bits       0
core count          1
VIP SRAM            262,144 bytes
```

The serialized `NBG_SECTION_OPERATIONS` record count is always exactly three
less than `network_info.operation_count`. The public comment says the latter
is a hardware-run instruction count that may exceed the saved command count.
The constant delta of three is a corpus invariant; PoC 2 does not yet assign
semantics to those three implicit operations.

Memory-pool requirements range from 161,280 to 57,445,376 bytes across 34
unique values. No v2 graph contains an AXI-SRAM section.

## Section integrity

Each v2 graph contains five checksum records, covering:

- `NBG_SECTION_NN`;
- `NBG_SECTION_TP`;
- `NBG_SECTION_COEF_DATA`;
- `NBG_SECTION_PPU_INSTR`;
- `NBG_SECTION_LOADSTATE`.

For all 690 records, the stored 32-bit value equals
`zlib.crc32(section_bytes) & 0xffffffff`. The parser computes this while
streaming each section and rejects a mismatch before returning a successful
result.

This protects the analyzed section bytes against unnoticed corpus drift. It
does not authenticate a graph, establish who produced it, or make a foreign
target safe to execute.

## Tensor surface

The v2 corpus contains 262 inputs and 222 outputs. The foreign v1 control
adds two UINT8 inputs and one UINT8 output.

Observed formats across all configured files:

| Raw | Public format | Records |
|---:|---|---:|
| 0 | FP32 | 21 |
| 1 | FP16 | 421 |
| 2 | UINT8 | 3 |
| 8 | INT32 | 42 |

All 487 I/O records have public data type 0, `TENSOR`. The parser also maps
the published image, array, and scalar codes without claiming they occur in
the Wave corpus.

All 484 v2 records use quantization format 0 (`NONE`). The three v1
negative-control records use affine-asymmetric quantization. No int8 or
dynamic-fixed-point I/O record appears in this corpus; that says nothing
about internal coefficient or activation formats.

Graph I/O counts are:

| Inputs / outputs | Files |
|---|---:|
| 1 / 1 | 103 |
| 2 / 1 | 27 |
| 2 / 2 | 2 |
| 4 / 1 | 2 |
| 5 / 3 | 4 |
| 75 / 75 | 1 |

The 75/75 case is the online Zipformer graph. It falsified the parser's
initial 64-record safety assumption; the reviewed bound is now 256 records
per direction.

## Live differential check

The graph with SHA-256
`5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66`
is both in the stock corpus and stored as the local reference. PoC 0 executed
that exact graph. PoC 2 statically recovers:

```text
network       de_online_ctc_NCHW
input         input/output[0], FP16 [256, 16, 1], 8,192 bytes
output        uid_20000_sub_uid_1_out_0, FP32 [5200, 16, 1], 332,800 bytes
layers        5
abs ops       5
run ops       9
cores         1
address width 32
memory pool   167,680 bytes
VIP SRAM      262,144 bytes
```

The I/O names, shapes, formats, and storage lengths match the live runtime
metadata. That agreement is the differential validation gate; timing and
numerical output are PoC 0 evidence, not parser output.

## PoC 12A lossless image and serializer evidence

PoC 12A builds a serializer-first layer above this parser without claiming
that arbitrary fields can be changed. Five exact v2 controls are
hash-pinned: the generated Erf graph plus the English, German, French, and
Japanese online CTC graphs.

For every control, `NbgImage`:

- retains the original bytes for each decoded, aligned, padded, and opaque
  range;
- proves exclusive ownership of every byte from offset zero through EOF;
- reconstructs output only by concatenating those ordered owned ranges;
- refuses emission after any semantic-change marker;
- compares length, every byte, and SHA-256 on unchanged re-emission.

All five controls round-trip exactly:

| Control | Bytes | Decoded | Alignment | Opaque | Result |
|---|---:|---:|---:|---:|---|
| generated Erf | 5,840 | 1,104 | 0 | 4,736 | exact |
| English CTC | 5,263,360 | 1,348 | 244 | 5,261,768 | exact |
| German CTC | 2,810,984 | 1,348 | 156 | 2,809,480 | exact |
| French CTC | 2,935,144 | 1,348 | 220 | 2,933,576 | exact |
| Japanese CTC | 2,717,544 | 1,348 | 92 | 2,716,104 | exact |

The runtime's final 512-byte connection region is still fingerprinted, but
all five selected files have a declared section that reaches EOF. The
connection region is therefore an overlapping analytical
`section-covered-view`; it is not assigned a second owner.

Bounded Ghidra analysis binds 21 named serializer/load exports to the exact
Wave ELFs and their symbol-byte hashes. Twenty are full bodies.
`vxoBinaryGraph_SaveNetworkBinaryGraph` is the sole four-byte manifest-
approved AArch64 export alias into the `SaveBinaryEntrance` path. Static
relationships include generation, section-memory, coefficient,
NN/TP-command, patch, relocation, load, and release families.

The adjacent English CTC ONNX initializers were searched only through a
finite, recorded transform registry and admitted NBG section set. Neither
the `[512,4999]` weight nor `[4999]` bias has an exact candidate occurrence.
PoC 12B coefficient packing is consequently `not-ready`; the non-match is
evidence, not permission to broaden into an unbounded search.

Machine evidence:

- [`serializer-ghidra-evidence.json`](serializer-ghidra-evidence.json)
- [`serializer-byte-ownership.json`](serializer-byte-ownership.json)
- [`serializer-roundtrip-evidence.json`](serializer-roundtrip-evidence.json)
- [`onnx-nbg-correlation.json`](onnx-nbg-correlation.json)

The complete method, exact function ledger, transform registry, safety
accounting, and reproduction commands are in the
[`PoC 12A notebook`](../lab-notebook/2026-07-28-poc12a-open-nbg-serializer.md).

## PoC 12B capture and PoC 12C DYNAIO/CRC qualification

PoC 12B compiled one fixed constant-input U8 Erf graph three times through
the Wave's stock serializer: A1 and A2 used bytes `[0, 1, 2, 3]`; B used
`[0, 1, 2, 4]`. A1 and A2 are byte-identical. B has the same size and
directory descriptors, but differs at header `crc_sum` and at one byte in
section 20.

The sealed PoC 12B analyzer intentionally retains its original finite search
registry. That registry did not admit section 20, so its historical
top-level result remains `multi-region-unresolved`. PoC 12C is a separate,
host-only qualification; it does not retroactively broaden or rewrite that
captured result.

PoC 12C proves the following bounded relationship:

```text
section 26 entry 0 = (offset 0, size 4)
section 20          = 64-byte data region
changed source byte = index 3
changed file byte   = section 20 + 3 = file offset 1511
A header crc_sum    = 0xd346207e
B header crc_sum    = 0x67768f6f
```

Replacing that single mapped byte in an in-memory copy of A and recomputing
the recovered header checksum yields B exactly, including SHA-256
`c9e3e18414f5bc674d4204921966ba53b08918ddcef73112003b065b9599f227`.
No mutated NBG was written, loaded, or executed.

The full corpus independently supports the table contract:

| Property | Result |
|---|---:|
| Files with both section 20 and section 26 | 111 |
| Files with exactly one of the pair | 0 |
| Table entries | 8,939 |
| Region bytes | 201,683,328 |
| Mapped payload bytes | 201,600,304 |
| Padding bytes | 83,024 |
| Nonzero padding bytes | 0 |

The PoC 12C serializer capability at that stage was deliberately narrow:
read and validate the table, compute the header CRC, and exactly reconstruct
the one controlled constant differential in memory. It does not yet permit
arbitrary section-20 writes, identify every entry's semantic owner, emit
operations or coefficients, compile general ONNX, or run a denoiser.

Machine evidence and notebooks:

- [`erf-constant-differential.json`](erf-constant-differential.json)
- [`dyna-io-crc-profile.json`](dyna-io-crc-profile.json)
- [`PoC 12B notebook`](../lab-notebook/2026-07-29-poc12b-erf-constant-differential.md)
- [`PoC 12C notebook`](../lab-notebook/2026-07-29-poc12c-dyna-io-crc-profile.md)

## PoC 13C external-input Erf profile

The learned denoiser's fixed U8 `[481]` Erf graph adds an important
qualification to the PoC 12C result. Its exact profile has:

```text
bytes:                 5,840
target:                0x1000000e
file CRC:              0xf14f321a
input/output:          UINT8 [481] / UINT8 [481]
dynamic_io:            null
structural signature:  236bd56d2f3aee5bf0c370b8372dda937c5af1b8d37a6e224b21df82db85088a
```

Therefore section 20 and section 26 form a required pair when either one is
present, but their absence is valid. DYNAIO is conditional, not a universal
VPMN v2 requirement. The strict profile still proves total byte ownership,
zero ownership conflicts, file CRC, exact tensor quantization, and a
byte-identical unchanged reconstruction.

Machine evidence:

- [`erf-mask-nbg-profile.json`](erf-mask-nbg-profile.json)
- [`PoC 13C notebook`](../lab-notebook/2026-07-29-poc13c-erf-mask-package.md)
- [`Erf-mask result`](../models/erf-mask/README.md)

## Parser safety model

[`nbg_format.py`](../../../waved/scripts/npu/nbg_format.py) never imports a
vendor runtime, invokes ADB, maps `/dev/galcore`, or executes model bytes.

Its important contracts are:

- regular repository-contained `.nb` files only;
- symlink rejection;
- exact configuration schema and duplicate-key rejection;
- checked little-endian reads and 64-bit range composition;
- maximum 256 section headers;
- maximum 256 inputs and 256 outputs;
- maximum six dimensions;
- maximum derived tensor storage of 1 TiB;
- exact v2 header, sub-header, section-header, and I/O widths;
- exact public v2 section-type range;
- mandatory v2 file-level CRC match over the recovered serializer scope;
- paired section-20/26 presence when either section exists, eight-byte table
  records, positive sizes, 64-byte-aligned ordered ranges, in-bounds
  non-overlap, and zero padding;
- no allocation driven by an unbounded on-disk count;
- overlap rejection;
- strict UTF-8, NUL-terminated bounded names;
- structured failures with no partial success result;
- streaming SHA-256, CRC-32, entropy, edge, and chunk fingerprints;
- no raw commands, coefficients, weights, states, or connection bytes in
  generated JSON.

The generated inventory contains metadata and fingerprints. It is large
because it preserves per-section evidence for 3.79 GB of source graphs, not
because it embeds their payloads.

## Reproduce

Host-only:

```bash
python3 -m unittest discover \
  -s waved/scripts/npu/tests \
  -p 'test_*.py' -v

python3 -m py_compile \
  waved/scripts/npu/nbg_format.py \
  waved/scripts/npu/tests/test_nbg_format.py

python3 waved/scripts/npu/nbg_format.py parse \
  waved/third_party/stock-npu-stack/sample-ctc.nb

python3 waved/scripts/npu/nbg_format.py verify-generated
python3 waved/scripts/npu/nbg_constant_diff.py verify-generated
python3 waved/scripts/npu/nbg_dyna_profile.py verify-generated
```

Regenerate intentionally:

```bash
python3 waved/scripts/npu/nbg_format.py inventory
python3 waved/scripts/npu/nbg_format.py summary
```

The ignored extracted corpus must be present and match the required count.
Reference files must match their configured hashes. Verification rebuilds
the inventory in memory and compares canonical bytes.

## Limits and next gate

PoC 2 does not establish:

- the instruction encoding inside NN, TP, PPU, or load-state sections;
- patch-record semantics beyond the public structure;
- how the runtime turns sections into `/dev/galcore` requests;
- whether section payload checksums are checked by the stock runtime;
- the semantic owner and runtime mutability rules for each dynamic-I/O
  entry;
- a legal or available Wave-target compiler;
- correctness of any graph produced for another target;
- operation coverage for a denoiser;
- real-time neural noise suppression.

PoC 3 may now trace the unchanged, hash-pinned stock CTC lifecycle and
correlate runtime calls and allocations with these exact sections. It still
must not mutate graph bytes, replay raw driver commands, or treat a
product-ID edit as compilation.

PoC 12A later proved byte-identical unchanged re-emission and statically
bounded the serializer families. PoC 12B supplied the deterministic
constant differential, and PoC 12C recovered the file checksum plus the
section-20/26 table geometry. Together they still do not establish
coefficient packing, operation emission, relocation construction, general
custom-model generation, or ONNX-to-NBG compilation.

PoC 14A later supplied a deterministic, quality-qualified affine-plus-Erf
host contract. PoC 14B could not supply the required producer controls: the
exact TIM-VX checker rejected its per-channel Multiply signature before NBG
emission. Consequently there are no affine A/B/C files to correlate and the
semantic serializer remains `not-ready`. See the
[`PoC 14B notebook`](../lab-notebook/2026-07-29-poc14b-affine-compiler-negative.md).
