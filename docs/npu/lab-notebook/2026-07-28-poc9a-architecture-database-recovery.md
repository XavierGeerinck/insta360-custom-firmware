# PoC 9A — Wave Architecture Database Recovery

**Date:** 2026-07-28
**Status:** Passed host-only gate
**Device contact:** None
**Device graph/API calls:** None
**Compiler calls:** None
**Safety class:** Static analysis of hash-pinned local ELF files

## 1. Outcome

PoC 9A recovered the Wave's five-key architecture lookup, the unique
customer-`0x1000000e` rows in two vendor architecture-performance databases,
the exact masked-revision fallback, selected private structure bounds, and a
strict evidence profile.

The most important result is:

```text
live identity:
  chip ID       0x00009000
  chip revision 0x00009002
  product ID    0x05090001
  eco ID        0x04000000
  customer ID   0x1000000e

exact Wave architecture database row:
  chip ID       0x00009000
  chip revision 0x00009001
  product ID    0x05090001
  eco ID        0x04000000
  customer ID   0x1000000e
```

`CreateAPModel` performs an exact lookup first, then a family lookup whose
revision test is:

```text
(live_revision XOR row_revision) AND 0x0000fff0 == 0
```

`0x9002` and `0x9001` pass that mask. The initial Wave profile therefore
records:

- live silicon revision `0x9002`;
- static database-family revision `0x9001`;
- a candidate `masked-revision-family-fallback` match mode.

It does not collapse them into one number.

PoC 9A does **not** recover enough information to emit a compiler
configuration. It deliberately leaves physical core count, SRAM budgets,
alignment constraints, complete bug features, and version fields unknown.

## 2. Question and hypothesis

Question:

> What target identity, feature database, architecture configuration, and
> performance-model fields are embedded in the exact Wave architecture
> libraries, and which can be recovered without invoking them?

Hypothesis:

> The effective Acuity/Vivante target profile is partly encoded as a
> five-field lookup table inside `libNNArchPerf.so`. The exact Wave table can
> be located from decompiled lookup code, read as inert ELF data, and
> cross-checked against the x86 simulator table and prior live identity.

The hypothesis would have failed if:

- the exact ELF hash differed;
- the lookup function was absent or truncated;
- the table geometry could not be established;
- customer `0x1000000e` had zero or multiple rows;
- the row disagreed with the live identity outside a demonstrated fallback;
- the x86 comparison contradicted the key order;
- any useful conclusion required loading a target library.

None of those failures occurred.

## 3. Authorization and hard safety gate

The owner approved compiler/profile recovery and requested stepwise PoCs with
documentation. PoC 9A was explicitly scoped to host/static analysis.

Allowed:

- read local firmware and SDK files;
- hash and parse ELF metadata;
- run Ghidra in a temporary host project;
- decompile named functions and their bounded related records;
- read file-backed table bytes;
- generate documentation, tests, schema, and JSON evidence.

Prohibited:

- ADB;
- any Wave filesystem or process access;
- `/dev/galcore`;
- `dlopen`, `ctypes`, or direct calls into the AArch64 libraries;
- OpenVX, TIM-VX, or graph construction;
- compiler invocation;
- calling the quarantined 6.4.18 CLC;
- generating or testing `VIV_GPU_FILE`;
- device execution.

The fact that the physical device was available did not change this gate.

## 4. Inputs

### 4.1 Exact Wave libraries

| Artifact | Bytes | SHA-256 |
|---|---:|---|
| `firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libArchModelSw.so` | 457,080 | `2b3df0844a2ff582f71da6517113939b54acdc7c97bebcac771e6e58140df63f` |
| `firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libNNArchPerf.so` | 598,152 | `253167677e9839d09815de93a86423824f26f56f097c100d59c03170adecffa9` |

The exact device-SDK copies are byte-identical to these firmware files. They
were retained as provenance but not redundantly decompiled.

### 4.2 Differential x86 simulator libraries

| Artifact | Bytes | SHA-256 |
|---|---:|---|
| `waved/third_party/TIM-VX/prebuilt-sdk/x86_64_linux/lib/libArchModelSw.so` | 459,728 | `c4766ef80043ba38dfd5d8bf1ed06590395a995c8d489a093e7a68265f342f97` |
| `waved/third_party/TIM-VX/prebuilt-sdk/x86_64_linux/lib/libNNArchPerf.so` | 498,736 | `e5f58113287a803663f897784f97ed983a32e07c2755e23bd25b63fec3aff70d` |

No adjacent 6.4.18 `libArchModelSw.so` or `libNNArchPerf.so` exists in the
current corpus. The differential analysis therefore uses exact AArch64 and
x86 simulator variants only. The 6.4.18 compiler/CLC remains unrelated and
quarantined.

### 4.3 Prior live evidence imported, not rerun

- `docs/npu/lab-notebook/2026-07-28-poc5-custom-compiler.md`;
- `docs/npu/lab-notebook/2026-07-28-poc6-7-custom-nbg-execution.md`;
- `docs/npu/manifests/wave-live-v1.1.6.6.json`.

These provide the reviewed capability record, runtime version, exact live
identity, target/NBG corroboration, and artifact provenance. Their current
hashes and sizes are embedded in the generated profile.

## 5. Tools and generated artifacts

Tools:

- Python 3.14.6;
- Ghidra 12.0.4, recorded inside each generated analysis;
- `ExportNpuAbi.java`, the existing bounded exporter;
- `ghidra_abi.py`, the existing strict temporary-project adapter;
- `rz-bin` 0.8.2 for independent section/symbol inspection;
- `arch_profile.py`, added by this PoC.

Generated:

```text
docs/npu/profile/arch-profile-ghidra-evidence.json
docs/npu/profile/wave-profile-evidence.json
docs/npu/profile/wave-profile.schema.json
```

The Ghidra evidence is 749,534 bytes and has SHA-256:

```text
b66aee19692523ed38730baee2de1cbda5fa0ad8d2ad7ed0ca7d290a93407814
```

Its target-manifest digest is:

```text
51f4c2e2a658450c2b16874306d6a168773a827167f33d97afb0aafcf401fd15
```

Final generated profile SHA-256:

```text
0e465f66f80c19747d4f0a73ab7db0fc9ba4a3b873fb9253a544b6dc4c3235e8
```

Profile-schema SHA-256:

```text
6375353f984c9137213fe3768f6d418da5ea9ce734107c0598f0e1dd48258fd1
```

## 6. Static-analysis target

### 6.1 `libArchModelSw.so`

The focused set was:

```text
archCalculateArchPerf
archFillOpInfo
archIsFeatureAvailable
archSetManualFeatures
archShowConfigurations
archSwLibInit
archSwLibInitFromDriver
updateCustomizedCArchConfigs
```

All eight exact Wave targets were found. Seven corresponding x86 targets
were found; that build does not export `archShowConfigurations` under the
same requested set, so the x86 target manifest contains only its verified
exports.

### 6.2 `libNNArchPerf.so`

The focused set was:

```text
APMDeriveHWFeatures
APMGetBugFeatures
APMSetManualFeatures
CreateAPModel
DestroyAPModel
amInitialize
amSetFeaturesSetting
amSetState
```

All eight exact Wave targets were found.

The x86 simulator has the five C-style APM entrypoints but lacks the three
requested C++ `am*` exports. The absence is recorded as an x86 build
difference. It is not interpreted as a missing Wave hardware feature.

### 6.3 Extraction quality

```text
target                         requested  found  Ghidra warnings
wave-archmodel-aarch64              8       8          0
wave-nnarchperf-aarch64             8       8          0
x86-sim-archmodel                   7       7          0
x86-sim-nnarchperf                  8       5          0
```

Every function used for a profile claim has successful, non-truncated
decompilation.

## 7. Identity lookup recovery

### 7.1 Exact Wave control flow

The exact `CreateAPModel` decompilation is at Ghidra entry `0x00116ad8`.
It demonstrates:

```text
table base in Ghidra image:  DAT_00161450
ELF virtual address:         0x00061450
record step:                 0x90 uint32 words
record bytes:                0x240 / 576
loop bound:                  0x1c9 / 457 records
exact keys:                  input[0..4] == row[0..4]
exact-row condition:         row[8] != 0
revision-family mask:        0x0000fff0
failure:                     five-key diagnostic + __assert_fail
```

Why `0x90` means 576 bytes:

```text
0x90 uint32 words × 4 bytes = 0x240 bytes
```

Ghidra rebased the AArch64 shared object at `0x00100000`. The table's
file-backed ELF virtual address is therefore `0x00161450 - 0x00100000`,
or `0x00061450`. The ELF program headers map that address to file offset
`0x00051450`.

### 7.2 Exact search

The first loop compares all five words exactly. It accepts only a row whose
word 8 is nonzero.

No exact `0x9002` Wave row exists.

### 7.3 Revision-family search

The second loop requires exact chip, product, eco, and customer values but
masks the revision comparison:

```text
(input[1] XOR row[1]) AND 0x0000fff0 == 0
```

It selects a row whose word 8 is zero. The matching Wave row satisfies that
generic-row condition.

This behavior explains why a table can contain `0x9001` while the connected
Wave reports `0x9002`. It does not prove every low-nibble revision is
interchangeable for all compiler outputs; it proves how this particular
performance-model lookup groups revisions.

### 7.4 Key order

The exact comparison proves five consecutive words. Its failure diagnostic
names them:

```text
ChipID
ChipVersion
ProductID
EcoID
CustomerID
```

The x86 function independently uses the same order and mask.

### 7.5 Unique Wave row

The generator reads every table record as inert little-endian bytes and
selects rows whose fifth word is customer `0x1000000e`.

Exact Wave:

```text
table file offset:   0x00051450
records:             457
record bytes:        576
matching rows:       1
matching row index:  320
row marker word 8:   0
row SHA-256:         85b860a40fbbc3663ea2c2b13dd4f81ed84ed0643d3e09fa3cee52ea7177f831
```

x86 simulator:

```text
table symbol/VA:     gChipInfo / 0x00241120
table file offset:   0x00041120
records:             387
record bytes:        552
matching rows:       1
matching row index:  290
row marker word 8:   0
row SHA-256:         c8483d5250b4c6fb0ab55a4d4620cdd19862121cdf6d89b93ecd626a47e68882
```

Both rows begin:

```text
word 0  0x00009000
word 1  0x00009001
word 2  0x05090001
word 3  0x04000000
word 4  0x1000000e
word 5  0x00000000
word 6  build-specific word; semantics not assigned in PoC 9A
word 7  0x00000000
word 8  0x00000000
```

### 7.6 Differential prefix

The first 114 words of the Wave and x86 rows agree except word 6. PoC 9A
classifies word 6 only as build-specific and does not promote its plausible
address-like values to a named field. The first wider semantic/layout
divergence appears at word 114.

This gives strong differential support to the identity and early
architecture fields, while also proving that the full records cannot be
treated as the same C structure.

The generator asserts the 114-word boundary. A future binary change will
fail generation instead of silently producing a new profile.

## 8. Architecture-model ABI recovery

### 8.1 Exact Wave `archSwLibInit`

At entry `0x00117298`, the initializer copies:

```text
source                         bytes
arch_nn_config                 0x1a0 / 416
arch_drv_option                0x2088 / 8,328
archHAL_CHIPIDENTITY           0x20 / 32
archNN_DATABASE_FEATURE        0x160 / 352
```

At exact `CreateAPModel`, internal objects are:

```text
APM input copy                 0x128 / 296
APM hardware-info allocation  0x0e8 / 232
```

These are bounds for these exact libraries. They do not establish public ABI
stability.

### 8.2 x86 simulator difference

The x86 initializer copies:

```text
source                         bytes
arch_nn_config                 0x198 / 408
arch_drv_option                0x268 / 616
archHAL_CHIPIDENTITY           0x20 / 32
archNN_DATABASE_FEATURE        0x148 / 328
```

Its `CreateAPModel` uses:

```text
APM input copy                 0x0f0 / 240
APM hardware-info allocation  0x0b0 / 176
```

The `arch_drv_option` difference is especially large. It rules out a plan
that allocates the public/x86 struct and passes it to the exact Wave library.

### 8.3 `archHAL_CHIPIDENTITY` display layout

`archShowConfigurations` prints:

| Offset | Field |
|---:|---|
| `0x00` | `chipModel` |
| `0x04` | `chipRevision` |
| `0x08` | `productID` |
| `0x0c` | `customerID` |
| `0x10` | `ecoID` |

This is not the same eco/customer order as the APM lookup tuple. The profile
stores both contracts separately.

### 8.4 Selected `arch_nn_config` offsets

The following mappings have direct, unambiguous argument flow into the
configuration print:

| Offset | Label |
|---:|---|
| `0x68` | `NumNNCores` |
| `0x6c` | `NumNNCoresInt8` |
| `0x70` | `NumNNCoresInt16` |
| `0x74` | `NumNNCoresUint16` |
| `0x78` | `NumNNCoresFloat16` |
| `0x80` | base term used by printed `VIPSRAMSize` |
| `0x84` | `AXISRAMSize` |
| `0xac` | `SWTiling` |
| `0xb0` | `DDRLatency` |
| `0xc8` | `VectorPrune` |
| `0xd0` | `AXISramLatency` |
| `0xd4` | `USCCacheSize` |

The printed VIP SRAM integer is derived as:

```text
config[0x20] + (config[0x1a] + 8) * 0x400
```

That is recorded as an expression, not bytes. The format label does not
establish the unit or reserved-partition semantics.

### 8.5 Configuration label inventory

`archShowConfigurations` exposes these groups:

Identity and execution resources:

```text
chipModel
chipRevision
productID
customerID
ecoID
NumNNCores
NumNNCoresInt8
NumNNCoresInt16
NumNNCoresUint16
NumNNCoresFloat16
NumTPCores
NumTPLiteCores
MadPerCore
VIP7Version
NumShaderCores
VIP_CORE_COUNT
```

Data paths and memories:

```text
InBuffDepth
AccumBufferDepth
DPAmount
XYDPX
XYDPY
ZDP
ZDP3Enable
ZDP6Enable
AXISRAMSize
VIPSRAMSize
L2CacheWidth
USCCacheSize
USCCacheControllers
EQUIVALENT_VIP_SRAM_WIDTH_IN_BYTE
SINGLE_PORT_VIPSRAM
VIPSRAM_ASYNC_FIFO
AXI_SRAM_ONLY_SW_TILING
```

Tiling and operation controls:

```text
SWTiling
SWTilingPhase1
SWTilingPhase3
SmallBatchEnable
TPWithFCLayer
TPCircularBufferSupport
DEPTH_WISE_SUPPORT
NN_WRITE_WITHOUT_USC
LanesPerConv
MaxTileSize
FLOAT_XYDP_X
FLOAT_XYDP_Y
FLOAT_ZDP
```

Bandwidth, latency, and clocks:

```text
DDRLatency
AXISramLatency
DDRReadBandWidthLimit
DDRWriteBandWidthLimit
DDRTotalBandWidthLimit
AXISRAMReadBandWidthLimit
AXISRAMWriteBandWidthLimit
AXISRAMTotalBandWidthLimit
AXIBusReadBandWidthLimit
AXIBusWriteBandWidthLimit
AXIBusTotalBandWidthLimit
FreqInMHZ
AxiClockFreqInMHZ
OutstandingTransfer
InternalWriteBWLimit
NN_DDR_MAX_BURST_SIZE
DDRKernelBurstSize
NN_DDR_BURST_SIZE
NN_LARGE_BURST_SIZE
```

Feature and errata labels:

```text
KERNEL_HEADER_NOT_CACHED_FIX
PER_3D_TILE_BUBBLE_FIX
TP_LOCALIZATION_REORDER_DISABLED_Fix
AsyncCopyPerfFix
ZDP3NoCompressFix
ZXDP3KernelReadConflictFix
xyOffsetLimitationFix
IMAGE_PARTIAL_CACHE_FIX
SLOW_NN_REQ_ARBITRATION_FIX
NN_COEF_COMPRESSION_ENHANCEMENT
TP_COMPRESSION_ENHANCEMENT
COEF_DELTA_CORD_OVER_FLOW_ZRL_8BIT_FIX
KERNEL_PER_CORE_LESS_THAN_THIRD_COEF_BUFF_DEPTH_FIX
LOW_EFFICIENCY_OF_ID_WRITE_IMGBUF_FIX
NN_KERNEL_SIZE_WASTE_IN_PARTIAL_MODE_FIX
KERNEL_VIP_SRAM_READ_BW_LIMITATION_FIX
IMG_POP_PIPELINE_PAUSE_FIX
FULL_CACHE_INTERVAL_FIX
DR_JD_Diff_For_Cacheline_Mode_Fix
TP_ACCESS_VIPSRAM_OT_IS_ONE_FIX
readReturnArbiterBubbleFix
nerghborImageDataTransferNotEfficientFix
tpVipSramOt1Fix
```

This inventory proves that the architecture model carries the classes of
information needed by a compiler. It does not assign a Wave value to every
label.

## 9. Existing live facts retained

The PoC 5D capability record remains:

```text
evis1                         0
evis2                         1
subGroupSize                  4
coreCount capability field   0
supportVA40                   0
supportStreamProcessor       0
streamProcessorExecCount     0
streamProcessorVectorSize    0
ecoID                         0x04000000
customerID                    0x1000000e
```

The zero `coreCount capability field` is still not interpreted as zero
physical cores. A generated graph and stock graphs have executed on the NPU;
the field belongs to a different capability contract.

The exact live identity remains:

```text
chip_model       0x00009000
chip_revision    0x00009002
chip_product_id  0x05090001
chip_eco_id      0x04000000
customer_id      0x1000000e
```

The runtime remains:

```text
6.4.19.5.946407
```

PoC 9A adds static corroboration. It does not create new live observations.

## 10. Strict profile

The generated profile has:

```text
profile ID     wave-fw-v1.1.6.6-0x1000000e
target         0x1000000e
input files    8
identity facts 13
feature facts  6
resource facts 11
version facts  6
unknown facts  10
```

Every fact contains a value, representation, evidence class, confidence,
source list, validation state, and notes.

The Python validator and JSON Schema both reject extra keys. The Python
validator additionally enforces:

- duplicate-key rejection during JSON loading;
- exactly target `0x1000000e`;
- repository-contained source paths;
- lowercase SHA-256 values;
- unique artifact IDs;
- source references to known artifacts;
- no duplicate source pairs;
- evidence and state enums;
- non-null facts have evidence;
- unknown facts are null, source-free, confidence `none`, and unresolved;
- the root unknown list exactly equals the unknown fact set;
- canonical deterministic serialization.

## 11. Unknowns and why they stay unknown

### 11.1 Complete bug vector

The database tail contains dense bit patterns and the focused functions expose
feature/bug APIs. PoC 9A does not yet have a complete, collision-free mapping
from each row bit to each printed feature name. Raw bits are not promoted to
named booleans.

Next route:

- statically follow `APMGetBugFeatures`, feature initialization, and manual
  override tables;
- compare named index operations across exact and x86 code;
- add per-bit tests before any value enters the profile.

### 11.2 Physical and per-precision cores

The labels and config offsets are known. Their exact live values are not.
Several repeated `4` values in the row are suggestive but not adequately
mapped to physical versus usable core semantics.

Next route:

- recover the exact initializer that maps the database row into
  `arch_nn_config`;
- only then compare with a separately reviewed API-rendered report in PoC 9B.

### 11.3 VIP and AXI SRAM

The display expression and relevant offsets are known, but the unit,
partitioning, reserved regions, and compiler-available budget are not.

Next route:

- map row-to-config assignments;
- recover parser keys and unit conversion in PoC 9C;
- cross-check against unmodified stock NBG memory metadata;
- do not use a stock graph's allocated SRAM as total hardware capacity.

### 11.4 Cache and alignment

Several cache, burst, and alignment labels exist. No complete rule connects
them to compiler tensor constraints.

Next route:

- recover the compiler parser and validation diagnostics;
- audit public TIM-VX constraints;
- test fixed host-only parser inputs before any compilation.

### 11.5 Version fields

The binary contains labels for architecture-model, software-tiling, and VIP7
versions, but no value has been immutably tied to the exact Wave configuration.

Next route:

- recover `archCommonInitVersion` and its exact Wave equivalent;
- identify static source storage or a proven output structure;
- avoid treating a filename or neighboring SDK banner as the exact value.

## 12. Negative findings

- No adjacent 6.4.18 architecture/performance database is present.
- The x86 simulator ABI is structurally different from the Wave ABI.
- Three x86 `am*` functions requested for comparison are absent.
- No standalone Wave `VIV_GPU_FILE` was found or generated.
- No complete name-to-bit bug-feature mapping was recovered.
- No exact Wave physical core count was established.
- No exact compiler memory/alignment budget was established.
- No general ONNX-to-NPU path was unlocked by this PoC.

These are recorded constraints, not reasons to fill gaps with simulator
defaults.

## 13. Reproduction

Focused tests:

```bash
python3 -m unittest waved.scripts.npu.tests.test_arch_profile
```

Manifest validation:

```bash
python3 waved/scripts/npu/arch_profile.py validate-targets
```

Generate Ghidra evidence:

```bash
python3 waved/scripts/npu/arch_profile.py generate-ghidra
```

Generate and validate the profile:

```bash
python3 waved/scripts/npu/arch_profile.py generate-profile
python3 waved/scripts/npu/arch_profile.py validate-profile
python3 waved/scripts/npu/arch_profile.py verify-generated
```

Rerun Ghidra and require byte identity:

```bash
python3 waved/scripts/npu/arch_profile.py \
  verify-generated --ghidra-only
```

Full host suite:

```bash
python3 -m unittest discover \
  -s waved/scripts/npu/tests \
  -p "test_*.py"

python3 -m py_compile waved/scripts/npu/arch_profile.py

python3 -m json.tool \
  waved/scripts/npu/arch_profile_targets.json >/dev/null

python3 -m json.tool \
  docs/npu/profile/arch-profile-ghidra-evidence.json >/dev/null

python3 -m json.tool \
  docs/npu/profile/wave-profile.schema.json >/dev/null

python3 -m json.tool \
  docs/npu/profile/wave-profile-evidence.json >/dev/null
```

## 14. Determinism and failure behavior

The profile generator refuses to proceed if:

- a target path leaves the repository;
- a binary hash differs;
- a file is not ELF64 little-endian;
- a table range is not backed by an ELF load segment;
- the Ghidra target-manifest digest differs;
- any reviewed analysis contains warnings;
- `CreateAPModel` no longer includes the table, stride, count, mask, and
  diagnostic tokens;
- customer `0x1000000e` has anything other than one row;
- the five identity words change;
- row word 8 no longer marks the generic row;
- the two tables disagree on identity;
- their checked common prefix changes;
- a profile source disappears or changes;
- strict profile validation fails.

Writes are atomic. Canonical JSON uses sorted keys, stable indentation, UTF-8,
and a final newline.

## 15. Implication for compiler/profile recovery

PoC 9A proves the compiler target is more than a marketing string:

```text
Wave profile selection
  = chip family
  + masked chip revision family
  + product ID
  + eco ID
  + customer ID
  + architecture configuration
  + feature/errata database
  + memory and tiling constraints
```

A file named:

```text
VIP9000NANOQ_PLUS_PID0X1000000E.config
```

would still be unsafe and incomplete if it contains guessed core, SRAM,
cache, or errata values. Filename compatibility is not profile correctness.

The recovered evidence is sufficient to drive the next static questions:

- which exported API produces or consumes each private structure;
- which compiler parser accepts which key spelling and type;
- how database values become compiler configuration fields;
- which fields have defaults versus must be exact.

It is not sufficient to compile a Conv graph today.

## 16. Implication for ONNX support

General ONNX support remains a frontend/runtime property:

```text
ONNX model
  -> shape/type/operator audit
  -> eligible fixed partitions to VSINPU/TIM-VX
  -> unsupported partitions to ONNX Runtime CPU
```

PoC 9A supplies part of the target profile beneath TIM-VX. It does not change
the known operation result:

- one exact Erf graph compiled and executed;
- Add, MatrixMul, and Conv2d export/verification remain blocked;
- a noise-cancelling spectral-mask partition still needs fixed Conv support.

The approved hybrid behavior remains:

- decide predictable CPU placement before VSINPU claims a partition;
- if unexpected NPU session creation fails, discard it and create one
  CPU-only session;
- never label fallback as NPU success;
- never retry NPU compilation automatically;
- strict mode must prove the designated mask partition is actually NPU
  resident.

## 17. Decision

PoC 9A passes.

It establishes the first versioned, machine-readable Wave profile with
field-level provenance and explicit unknowns.

It authorizes no device action.

The next preferred work is PoC 9C's host/static parser recovery because it
can reduce compiler-grammar uncertainty without touching the Wave. PoC 9B
remains a separate ABI gate: no architecture API should be called until its
exact input/output storage and lifecycle have been statically proven and the
individual call plan has been reviewed.

The broader sequence remains:

```text
9A  architecture database evidence         passed
9B  architecture API ABI                   separate gated plan
9C  VIV_GPU_FILE/parser grammar            next host/static recovery
9D  isolated fixed Conv compiler control   after profile completeness
9E  ONNX capability auditor                host-only
9F  hardened VSINPU                        host-only/failure injection
10A fixed Conv execution                   separate device plan
10B mixed CPU/NPU ONNX                     separate device plan
11  tiny spectral-mask audio model         after all earlier gates
```
