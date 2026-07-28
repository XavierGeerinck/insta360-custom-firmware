# Wave NBG Format Explorer — PoC 2

PoC 2 passed on 2026-07-28. The repository now has a strict, non-executing
parser for the legacy reference graph and the Wave's NBG v2 files, plus a
hash-pinned inventory of every available stock graph.

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
- tensor format `8`, previously anonymous in the corpus, is signed INT32;
- every v2 I/O object in this corpus is a tensor; the parser also knows the
  public image, array, and scalar codes;
- all command, coefficient, relocation, and state payloads remain bounded
  and fingerprinted rather than interpreted or executed.

This is a format and integrity result. It is not yet a custom-model unlock.
The stock runtime still needs either a compiler that emits target
`0x1000000e` or a fully reconstructed private export-data graph.

## Evidence chain

The v2 field map is supported by three independent sources:

1. The exact Wave `libnnsdk.so` parser analyzed in PoC 1 establishes the
   offsets, 44-byte section headers, 112-byte I/O records, section types 1
   and 2, and the 512-byte connection tail used by the live runtime.
2. The stock corpus supplies 138 v2 instances, including the exact graph
   executed in PoC 0.
3. A public Ameba Pro2 BSP contains VeriSilicon VIPLite 2.0.0 linker headers
   whose structures exactly match those bytes.

Pinned public sources:

- [`nbg_fmt_v2.h` at `b83b89f`](https://github.com/Ameba-AIoT/ameba-arduino-pro2/blob/b83b89f3842cf2938c8ce8adec9dee1864b7d72a/Arduino_package/hardware/system/project/realtek_amebapro2_v0_example/src/VIPLiteDrv_2.0.0/nbg_linker/inc/nbg_fmt_v2.h),
  SHA-256
  `6af06536a70b2534c96ba384e775a19d2e9ef73acfb6817bd7975d4fd9cdc35b`;
- [`vip_lite.h` at `b83b89f`](https://github.com/Ameba-AIoT/ameba-arduino-pro2/blob/b83b89f3842cf2938c8ce8adec9dee1864b7d72a/Arduino_package/hardware/system/project/realtek_amebapro2_v0_example/src/VIPLiteDrv_2.0.0/include/nbg_linker/vip_lite.h),
  SHA-256
  `d0fd4ea3c98a032f7e8c3045d1d27972b6f1745c1b1e377aeffcc16cc9eb138f`;
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
inventory SHA-256              48c19e2f47d82a65eaf16af6e5aec0ed3beb322e679a0cfa91d736894f97b49c
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
        typed, aligned section payloads
        final 512-byte runtime connection region
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

The header field named `crc_sum` by the public source has 136 unique values,
but standard CRC-32 over the tested whole-file exclusions does not reproduce
it. The parser records it as `header.crc32` for source-name fidelity but does
not claim or enforce its algorithm. This unresolved file-level field must
not be confused with the verified per-section checksums.

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

Public structure names establish the section roles. PoC 2 semantically
decodes I/O, network info, memory sizes, and checksum records. It does not
decode or emit NN/TP commands, coefficient bytes, patch records, dynamic-I/O
payloads, load states, or PPU instructions.

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
- the algorithm or coverage of the header `crc_sum`;
- a legal or available Wave-target compiler;
- correctness of any graph produced for another target;
- operation coverage for a denoiser;
- real-time neural noise suppression.

PoC 3 may now trace the unchanged, hash-pinned stock CTC lifecycle and
correlate runtime calls and allocations with these exact sections. It still
must not mutate graph bytes, replay raw driver commands, or treat a
product-ID edit as compilation.
