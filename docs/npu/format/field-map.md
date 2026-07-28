# NBG Field Map and Confidence Ledger

This document maps the NBG fields used by PoC 2. It separates exact
structure names from corpus relations and unresolved semantics.

## Confidence codes

| Code | Meaning |
|---|---|
| `W` | Exact access or diagnostic in the hash-pinned Wave runtime |
| `P` | Named structure or enum in a commit-pinned public VeriSilicon source |
| `C` | Relation holds across the configured corpus |
| `L` | Agrees with the exact graph observed through the live stock runtime |
| `I` | Inference; plausible but not yet uniquely established |
| `U` | Unknown |

An interpretation supported by `W+P+C` is treated as high confidence.
Corpus invariants are not automatically universal format requirements.

## Common envelope

| Offset | Width | Parser key | Interpretation | Evidence | Failure |
|---:|---:|---|---|---|---|
| `0x00` | 4 | `magic` | ASCII `VPMN` | `W+P+C` | `bad-magic` |
| `0x04` | 4 | `version` | Little-endian NBG version | `W+P+C` | unsupported family |
| `0x08` | 4 | `target`, compatibility alias `product` | Hardware target/CID | `W+P+C+L` | recorded; execution compatibility is a later gate |

The public v2 source calls the third word `target`. Existing project and
runtime diagnostics call it product or CID. The parser emits both names with
the same integer and does not normalize target families.

## v2 `nbg_format_header_t`

The structure is 48 bytes.

| Offset | Width | Public field | Parser key | Corpus invariant | Evidence |
|---:|---:|---|---|---|---|
| `0x00` | 4 | `magic[4]` | `magic` | `VPMN` | `W+P+C` |
| `0x04` | 4 | `version` | `version` | `0x00020000` | `W+P+C` |
| `0x08` | 4 | `target` | `target` / `product` | `0x1000000e` | `W+P+C+L` |
| `0x0c` | 4 | `crc_sum` | `header.crc32` | 136 unique values | `P+C`; algorithm `U` |
| `0x10` | 4 | `total_size_low` | `declared_file_size_low` | equals file size low | `P+C` |
| `0x14` | 4 | `total_size_high` | `declared_file_size_high` | zero | `P+C` |
| `0x18` | 4 | `sub_header_offset` | `sub_header_offset` | 48 | `W+P+C` |
| `0x1c` | 4 | `sub_header_size` | `sub_header_size` | 12 | `P+C` |
| `0x20` | 4 | `sub_header_count` | `sub_header_count` | 1 | `P+C` |
| `0x24` bit 0 | 1 bit | `class` | `class` | 0 | `W+P+C` |
| `0x24` bits 1–31 | 31 bits | `reserved1` | `reserved.class_word_reserved_bits` | zero | `P+C` |
| `0x28` | 4 | `reserved2` | `reserved.reserved2` | zero | `P+C` |
| `0x2c` | 4 | `reserved3` | `reserved.reserved3` | zero | `P+C` |

`total_size_low/high` is composed as an unsigned 64-bit integer before being
compared with the actual file size.

The public comment describes `crc_sum` as CRC-32 excluding its own field.
The tested standard CRC-32 variants over the whole file, with and without
the final 512 bytes and with skipped/zeroed checksum bytes, do not reproduce
the stock value. PoC 2 therefore preserves the source name but does not
validate it. Alternatives include different coverage, ordering, seed, or a
producer-specific calculation.

## v2 `nbg_sub_header_t`

All Wave v2 files place one 12-byte sub-header at `0x30`.

| Relative | Width | Public field | Parser key | Corpus invariant | Evidence |
|---:|---:|---|---|---|---|
| `+0x00` | 4 | `sh_offset` | `section_headers_offset` | `0x3c` | `W+P+C` |
| `+0x04` | 4 | `sh_size` | `section_header_size` | 44 | `W+P+C` |
| `+0x08` | 4 | `sh_count` | `section_header_count` | 18 or 20 | `W+P+C` |

The parser supports exactly one sub-header because it returns one graph
description. A count other than one fails instead of silently ignoring
additional graphs.

## v2 `nbg_section_header_t`

Each record is 44 bytes/11 little-endian words.

| Relative | Width | Public field | Parser key | Observed | Evidence |
|---:|---:|---|---|---|---|
| `+0x00` | 4 | `sd_type` | `type`, `type_name` | public IDs listed below | `W+P+C` |
| `+0x04` | 4 | `sd_offset_low` | low half of `offset` | in-file | `W+P+C` |
| `+0x08` | 4 | `sd_offset_high` | high half of `offset` | zero | `P+C` |
| `+0x0c` | 4 | `sd_size_low` | low half of `size` | section-specific | `W+P+C` |
| `+0x10` | 4 | `sd_size_high` | high half of `size` | zero | `P+C` |
| `+0x14` | 4 | `sd_addr_low` | low half of `load_address` | zero | `P+C` |
| `+0x18` | 4 | `sd_addr_high` | high half of `load_address` | zero | `P+C` |
| `+0x1c` | 4 | `sd_alignsize` | `alignment` | 0, 64, or 256 | `P+C` |
| `+0x20` | 4 | `sd_entsize` | `entry_size`, compatibility alias `stride` | section-specific | `W+P+C` |
| `+0x24` | 4 | `reserved1` | `reserved_words[0]` | zero | `P+C` |
| `+0x28` | 4 | `reserved2` | `reserved_words[1]` | zero | `P+C` |

The public header comment says a section with a load address should be
loaded into address space when the NBG is executable. Serialized Wave files
leave both address words zero; PoC 2 does not infer the runtime address
assignment mechanism from that fact.

## Public v2 section enum

The enum is append-only by its own comment. PoC 2 accepts IDs 0 through 40
for exact v2 version `0x00020000`; it does not accept an unknown ID and then
pretend to understand it.

| ID | Public name | In Wave corpus | PoC 2 handling |
|---:|---|---|---|
| 0 | `NBG_SECTION_NONE` | no | named only |
| 1 | `NBG_SECTION_INPUT` | yes | decoded I/O records |
| 2 | `NBG_SECTION_OUTPUT` | yes | decoded I/O records |
| 3 | `NBG_SECTION_LAYER` | yes | named, fingerprinted |
| 4 | `NBG_SECTION_OPERATIONS` | yes | named, fingerprinted |
| 5 | `NBG_SECTION_COEF_HEADER_TABLE` | no | named only |
| 6 | `NBG_SECTION_COEF_HEADER` | no | named only |
| 7 | `NBG_SECTION_NN` | yes | named, CRC-verified, fingerprinted |
| 8 | `NBG_SECTION_TP` | yes | named, CRC-verified, fingerprinted |
| 9 | `NBG_SECTION_SH` | yes | named, fingerprinted |
| 10 | `NBG_SECTION_PATCH` | yes | named, fingerprinted |
| 11 | `NBG_SECTION_LAYER_PARAM` | no | named only |
| 12 | `NBG_SECTION_SW` | no | named only |
| 13 | `NBG_SECTION_HWINITOPERATIONS` | no | named only |
| 14 | `NBG_SECTION_ICDT` | no | named only |
| 15 | `NBG_SECTION_ICD` | no | named only |
| 16 | `NBG_SECTION_PPU_PARAM` | no | named only |
| 17 | `NBG_SECTION_OPSYNC` | no | named only |
| 18 | `NBG_SECTION_DMA_SBI_INFO` | no | named only |
| 19 | `NBG_SECTION_PPU_INSTR` | yes | named, CRC-verified, fingerprinted |
| 20 | `NBG_SECTION_DYNAIO` | 111 files | named, fingerprinted |
| 21 | `NBG_SECTION_LOADSTATE` | yes | named, CRC-verified, fingerprinted |
| 22 | `NBG_SECTION_DMA_SBI_ADDITION` | no | named only |
| 23 | `NBG_SECTION_COEF_DATA_TABLE` | yes | named, fingerprinted |
| 24 | `NBG_SECTION_COEF_DATA` | yes | named, CRC-verified, fingerprinted |
| 25 | `NBG_SECTION_PPU_INSTR_TABLE` | yes | named, fingerprinted |
| 26 | `NBG_SECTION_DYNAIO_TABLE` | 111 files | named, fingerprinted |
| 27 | `NBG_SECTION_LOADSTATE_TABLE` | yes | named, fingerprinted |
| 28 | `NBG_SECTION_LAYER_DUMP_OPERATION` | no | named only |
| 29 | `NBG_SECTION_LAYER_DUMP_OUTPUTS` | no | named only |
| 30 | `NBG_SECTION_LAYER_DUMP_PATCH_DATA` | no | named only |
| 31 | `NBG_SECTION_LAYER_DUMP_INSTR_TABLE` | no | named only |
| 32 | `NBG_SECTION_LAYER_DUMP_INSTR` | no | named only |
| 33 | `NBG_SECTION_LAYER_DUMP_LOADSTATE_TABLE` | no | named only |
| 34 | `NBG_SECTION_LAYER_DUMP_LOADSTATE` | no | named only |
| 35 | `NBG_SECTION_NETWORK_INFO` | yes | decoded |
| 36 | `NBG_SECTION_CUSTOMER_DATA` | no | named only |
| 37 | `NBG_SECTION_MEMORY_POOL` | yes | decoded |
| 38 | `NBG_SECTION_VIP_SRAM` | yes | decoded |
| 39 | `NBG_SECTION_AXI_SRAM` | no | decoder implemented |
| 40 | `NBG_SECTION_CHECKSUM` | yes | decoded and enforced |

Naming a section does not decode its payload. The inventory classification
distinguishes decoded metadata from bounded opaque content.

## v2 `nbg_section_input_output_t`

The structure is 112 bytes.

| Relative | Width | Public field | Parser key | Evidence |
|---:|---:|---|---|---|
| `+0x00` | 4 | `dim_count` | `dimension_count` | `W+P+C+L` |
| `+0x04` | 24 | `dim_size[6]` | `dimensions` | `W+P+C+L` |
| `+0x1c` | 4 | `data_format` | `data_format`, `data_format_name` | `W+P+C+L` |
| `+0x20` | 4 | `data_type` | `data_type`, `data_type_name` | `W+P+C` |
| `+0x24` | 4 | `quant_format` | `quantization.format` | `W+P+C` |
| `+0x28` | 8 | `quant_data` union | raw words plus selected interpretation | `W+P+C` |
| `+0x30` | 64 | `name` | `name` | `W+P+C+L` |

Rules:

- dimension count must be 1 through 6;
- each selected dimension must be positive;
- unknown formats retain a null derived size;
- known sub-byte formats use ceiling division for total storage;
- quantization 0 exposes no semantic scale or zero point;
- quantization 1 interprets word 0 as signed fixed-point position;
- quantization 2 interprets word 0 as IEEE-754 scale and word 1 as signed
  zero point;
- raw quantization words are preserved in every case.

### Public buffer formats

| ID | Name | Element bits | Observed |
|---:|---|---:|---|
| 0 | FP32 | 32 | yes |
| 1 | FP16 | 16 | yes |
| 2 | UINT8 | 8 | v1 negative control |
| 3 | INT8 | 8 | no |
| 4 | UINT16 | 16 | no |
| 5 | INT16 | 16 | no |
| 6 | CHAR | 8 | no |
| 7 | BFP16 | 16 | no |
| 8 | INT32 | 32 | yes |
| 9 | UINT32 | 32 | no |
| 10 | INT64 | 64 | no |
| 11 | UINT64 | 64 | no |
| 12 | FP64 | 64 | no |
| 13 | INT4 | 4 | no |
| 14 | UINT4 | 4 | no |
| 15 | unassigned in pinned header | — | no |
| 16 | BOOL8 | 8 | no |

TIM-VX's public parser API at the pinned commit declares formats 0 through
12. The newer VIPLite header at its pinned commit additionally declares
INT4, UINT4, and BOOL8.

### Public buffer data types

| ID | Name | Observed |
|---:|---|---|
| 0 | TENSOR | all 487 records |
| 1 | IMAGE | no |
| 2 | ARRAY | no |
| 3 | SCALAR | no |

### Public quantization formats

| ID | Name | Observed |
|---:|---|---|
| 0 | NONE | 484 v2 records |
| 1 | DYNAMIC_FIXED_POINT | no |
| 2 | AFFINE_ASYMMETRIC | 3 v1 records |

I/O metadata does not expose the internal precision of every operation or
coefficient. In particular, the lack of INT8 graph I/O is not evidence that
the accelerator or compiler lacks INT8 kernels.

## v2 `nbg_section_network_info_t`

The structure is 220 bytes.

| Relative | Width | Public field | Parser key | Corpus |
|---:|---:|---|---|---|
| `+0x00` | 64 | `name` | `network_info.name` | 24 distinct names |
| `+0x40` | 4 | `input_count` | `input_count` | 1–75 |
| `+0x44` | 4 | `output_count` | `output_count` | 1–75 |
| `+0x48` | 4 | `layer_count` | `layer_count` | 4–2,460 |
| `+0x4c` | 4 | `abs_op_count` | `absolute_operation_count` | 4–2,736 |
| `+0x50` | 4 | `operation_count` | `operation_count` | 8–97,789 |
| `+0x54` | 4 | `core_count` | `core_count` | always 1 |
| `+0x58` | 4 | `address_width` | `address_width` | always 32 |
| `+0x5c` | 4 | attribute bitfield | `attribute` | always raw `0x0000000a` |
| `+0x60` | 124 | `vsi_reserved[31]` | count and SHA-256 only | 1 or 2 nonzero words |

Attribute bit allocation:

| Bits | Public meaning | Corpus |
|---:|---|---|
| 0 | change PPU parameter | 0 |
| 1–2 | NN command size code | 1, meaning 192-byte command |
| 3–4 | TP command size code | 1, meaning 192-byte command |
| 5–6 | DMA support code | 0 |
| 7–31 | reserved | 0 |

`vsi_reserved` is vendor-reserved rather than format-reserved. The parser
does not require it to be zero and does not invent names for its contents.

The parser cross-checks network input/output counts against the actual
section records. A mismatch is `metadata-count-mismatch`.

## Memory metadata

| Section | Layout | Parser key | Corpus |
|---|---|---|---|
| `MEMORY_POOL` | low/high u32 used size | `memory_pool_bytes` | 161,280–57,445,376 |
| `VIP_SRAM` | u32 used size | `vip_sram_bytes` | always 262,144 |
| `AXI_SRAM` | u32 used size | `axi_sram_bytes` | absent |

The sizes are graph requirements recorded by the producer, not live
allocation measurements.

## Section checksum records

Each record is:

```text
+0x00 u32 sd_type
+0x04 u32 checksum_value
```

All v2 files contain exactly one checksum for each of section types 7, 8, 19,
21, and 24. All 690 match standard CRC-32 of the exact referenced section
range. A missing, duplicate, or mismatching target section fails parsing.

The parser emits both stored and computed values plus `matches: true`.
It never emits a successful result with `matches: false`.

## v1 compatible-prefix map

The sole v1 file is the foreign target `0x10000020`, version `0x0001001e`.
Pinned TIM-VX source publicly supports only through `0x00010014`, so PoC 2
labels newer bytes conservatively.

Decoded common prefix:

| Offset | Width | Interpretation | Evidence |
|---:|---:|---|---|
| `0x00` | 4 | magic | public + fixture |
| `0x04` | 4 | version | public + fixture |
| `0x08` | 4 | target | public + fixture |
| `0x0c` | 64 | network name | public + fixture |
| `0x4c` | 4 | layer count | public + fixture |
| `0x50` | 4 | operation count | public + fixture |
| `0x54` | 4 | input count | public + fixture |
| `0x58` | 4 | output count | public + fixture |

The parser walks version-gated public section pairs and semantically decodes
only modern six-dimension named I/O records with public/private strides
116/128. Transitional 108/120 and older 44/56 widths are recognized for
bounds but not promoted to the modern semantic layout.

For version `0x0001001e`, the eight bytes between the public prefix end and
the first referenced section remain `opaque_fixed_extension`. They are
fingerprinted without invented fields.

## Final 512-byte connection region

Exact Wave runtime code copies the final 512 bytes into its internal model
object. PoC 2 therefore fingerprints that fixed tail separately. There are
30 unique tail SHA-256 values across the 139 configured files.

The contents and grammar remain unknown. The parser does not dump strings,
interpret pointers, or assume this region is target-independent.

## Unresolved fields and hypotheses

| Item | Current state | What would resolve it |
|---|---|---|
| Header `crc_sum` coverage/algorithm | public name, stock values preserved, tested standard variants mismatch | producer source or exact runtime validation routine |
| `vsi_reserved[31]` words | 1–2 nonzero words per graph, fingerprinted | linker/runtime field accesses |
| Three implicit run operations | declared run count is saved operation count +3 in every v2 graph | lifecycle trace and operation-table decoding |
| Patch source/destination semantics | public record structure known, payload opaque | bounded decoder plus runtime relocation trace |
| NN/TP instruction words | section and command widths known | hardware documentation or trace-assisted disassembly |
| Dynamic I/O data | section names and presence known | runtime consumer map |
| Load-state and PPU instruction grammar | section names, sizes, CRC known | exact runtime/driver trace |
| Connection tail | fixed length and fingerprints known | parser field accesses |

No unresolved item authorizes native mutation fuzzing or execution of a
hand-edited graph.
