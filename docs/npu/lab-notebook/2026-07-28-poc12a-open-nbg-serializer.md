# PoC 12A — Exact Wave NBG Serializer Recovery

**Date:** 2026-07-28

**Status:** Completed host-only

**Design:** `docs/superpowers/specs/2026-07-28-wave-npu-open-nbg-serializer-design.md`

**Implementation plan:** `docs/superpowers/plans/2026-07-28-wave-npu-poc12a-open-nbg-serializer-recovery.md`

**Safety class:** Static ELF/Ghidra analysis, bounded ONNX inspection, and
byte-identical reconstruction of unchanged NBG inputs

## 1. Result

PoC 12A completed the first SDK-independent serializer recovery step.

The exact Wave firmware contains enough named static structure to establish:

```text
vsi_nn_GenerateNBG
  -> vxGenerateNBG
  -> graph processing / binary capture

vxoBinaryGraph_NetworkBinaryGraph
  -> SaveNetworkBinaryGraph export alias
  -> SaveBinaryEntrance implementation
  -> RelocateNetworkBinaryGraph

SaveNNTPOperation
  -> SaveNNTPCommand
  -> SaveNNTPInstruction
  -> SaveWeightAndBias
  -> SaveNNTPStates
```

The implementation also proves:

- all 21 selected serializer/load targets have exact ELF symbol bytes and
  SHA-256 identities;
- both Ghidra imports use one consistent `0x100000` PIE load bias;
- every selected target has complete, untruncated bounded decompilation;
- every byte in five selected exact-target VPMN v2 files has one exclusive
  owner;
- all five files reconstruct byte-for-byte and hash-for-hash;
- semantically changed images are refused;
- the English CTC weight and bias have no exact occurrence under 47/50
  reviewed finite transform candidates;
- plain FP16 is explicitly a no-match for both tensors;
- PoC 12B coefficient-packing recovery is therefore `not-ready`.

No custom NBG was emitted. No compiler, simulator, OpenVX runtime, native
extension, container, ADB command, driver, or Wave device was called.

## 2. Why this PoC exists

PoC 9D established that the obtainable public Acuity package contains the
exact Wave architecture identity but does not contain a qualified no-kernel
compiler path for the exact VPMN v2 target.

The project therefore cannot treat the proprietary compiler as a future
dependency. A custom compiler must be recovered from exact firmware behavior
in bounded steps:

```text
serializer structure
  -> coefficient packing
  -> command/patch/relocation emission
  -> fixed offline graph construction
  -> separately authorized device control
```

PoC 12A covers only the first line.

## 3. Exact input corpus

### 3.1 Firmware libraries

| ID | Path | Bytes | SHA-256 |
|---|---|---:|---|
| `wave-openvx` | `firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libOpenVX.so` | 6,422,744 | `f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb` |
| `wave-ovxlib` | `firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libovxlib.so` | 5,838,160 | `09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9` |

Both pass strict host-side ELF admission:

```text
class:       ELF64
endianness:  little
machine:     AArch64
PT_LOAD:     bounded inside the file
```

### 3.2 NBG controls

| ID | Bytes | SHA-256 | Role |
|---|---:|---|---|
| `generated-erf` | 5,840 | `6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee` | smallest exact-target generated control |
| `ctc-en` | 5,263,360 | `3a9294ea8595789cc23ddc9298ebceb4474738fed07a9a8b10e2442b67b0d8d5` | ONNX/NBG source anchor target |
| `ctc-de` | 2,810,984 | `5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66` | differential control |
| `ctc-fr` | 2,935,144 | `db8a79e3e7b35eddd87a7369bd464101607083f489a75418d73073718ff6f079` | differential control |
| `ctc-ja` | 2,717,544 | `2c261beb8aedb1361c3a4832878ff09dec597bcae4681aee742069c6faa54b5d` | differential control |

Every NBG is:

```text
magic:    VPMN
version:  0x00020000
target:   0x1000000e
```

### 3.3 ONNX anchor

```text
path:
  models/ai-models/asr-models/model/asr_en/reverb_asr_online/ctc.onnx

bytes:
  10258790

SHA-256:
  8d6d3b6ce9da51f852981bb5bdd33e2818d47d3e545e9efe5d3700a37b367219
```

Strict graph:

```text
hidden [1,16,512] FP32
  -> MatMul(weight [512,4999] FP32)
  -> Add(bias [4999] FP32)
  -> LogSoftmax
  -> probs [1,16,4999] FP32
```

The path, tensor dimensions, and output width make this a strong source/NBG
anchor. They do not prove that the files were produced by the same compiler
build.

## 4. Strict target manifest

The single source of PoC inputs and policy is:

```text
waved/scripts/npu/nbg_serializer_targets.json
```

It rejects:

- duplicate JSON keys;
- unknown or missing schema fields;
- invalid artifact IDs, roles, sizes, and hashes;
- absolute paths, `..`, repository escapes, and symlink components;
- missing or non-regular files;
- ELF class, endianness, machine, program-table, or load-range drift;
- non-VPMN, non-v2, or non-Wave NBG controls;
- wrong function-to-ELF assignment;
- missing, duplicate, or unapproved serializer functions;
- correlation limits or transforms outside the reviewed allowlist;
- output paths outside the four canonical evidence paths;
- any nonzero prohibited-action counter.

The real manifest validates:

```text
artifacts:  8
functions: 21
```

## 5. Static serializer recovery

### 5.1 Method

`nbg_serializer_recovery.py` performs three independent bindings:

1. parse the ELF64 dynamic symbol table directly without loading the ELF;
2. map each symbol virtual address through exactly one bounded `PT_LOAD`
   segment and hash its exact file bytes;
3. compare the result with Ghidra's function at one common per-ELF load
   bias.

Ghidra 12.0.4 analyzed one disposable import per ELF. The common inferred
load bias was:

```text
wave-openvx-serializer: 0x100000
wave-ovxlib-serializer: 0x100000
```

Several exported names also appear as PLT/import aliases inside Ghidra. The
adapter keeps those alternate contexts but selects exactly one function
whose normalized entry equals the ELF dynamic-symbol address.

The serializer evidence raises the existing default decompilation limit only
for this reviewed run:

```text
legacy/default limit:  24,000 characters
PoC 12A hard limit:   131,072 characters
```

The default behavior and existing evidence schema remain unchanged.

### 5.2 Exact selected functions

| Function | ELF vaddr | Bytes | SHA-256 | Kind |
|---|---:|---:|---|---|
| `vxGenerateNBG` | `0x1fb3f0` | 1088 | `37590c849f22f0b98a08c717924d1354f76d4bbca2dd56ac96e7c481d4af1273` | function |
| `vxoBinaryGraph_NetworkBinaryGraph` | `0x1fb3b0` | 56 | `36345047edc5c5c62a39063bce9cb7f35395069ec558052d723f2ce8892039fa` | function |
| `vxoBinaryGraph_GenerateSectionMemory` | `0x1d0ab0` | 248 | `fbd923f1b71a019daa6267e0b695bb33838fe2e73971f72b9b500fc3dbe1b493` | function |
| `vxoBinaryGraph_SaveBinaryEntrance` | `0x1f80d0` | 13004 | `de0e83178b72986508ac4452b5cbb1823c7fc2d4698a26ae4867ac2edfa99074` | function |
| `vxoBinaryGraph_SaveBinaryEntranceBefore` | `0x1f2f50` | 240 | `46472ab1197cc38145b506409dcad14041303e2f94142fc4db9baad3aac03398` | function |
| `vxoBinaryGraph_SaveNetworkBinaryGraph` | `0x1fb3a0` | 4 | `db59d4090bce5362a610d6b0007be32b52536409519c9eef51e3766532dbc0f2` | export thunk alias |
| `vxoBinaryGraph_SaveWeightAndBias` | `0x1ee750` | 464 | `9d3af1469af439d06c58e853b4cc93bd96968f997436c922c004c39cac794d47` | function |
| `vxoBinaryGraph_UpdateCoefHeaderCache` | `0x1ed360` | 192 | `c463148f0a8f18ed8452ce9c32dda9183a5ea437fefd29e618deac25105d15dc` | function |
| `vxoBinaryGraph_SaveNNTPCommand` | `0x1f32b0` | 1984 | `618d1730443df406bcf23bf81ec2eea423ef324449fd214caf6a6d61bbc16636` | function |
| `vxoBinaryGraph_SaveNNTPInstruction` | `0x1eea20` | 2780 | `02d67d8997ec944b82fb3c278b8b42480050a40c2195d5d2ec876dd5b516408e` | function |
| `vxoBinaryGraph_SaveNNTPOperation` | `0x1f3a70` | 348 | `e24353fa1b35c020ad5444c744a9bac79fde8749735a0a239d5a962a6e5c960e` | function |
| `vxoBinaryGraph_SaveNNTPStates` | `0x1ef8f0` | 2052 | `7b770c56553025054e1230c2a962a085045525df3816839abe5a7e9b8335e9eb` | function |
| `vxoBinaryGraph_AllocateBinaryLoadBuffer` | `0x1d1f30` | 1384 | `25cfabbf99067bffc6da66a498efe9d3f3884ea6890df524ab6d6d94641c5181` | function |
| `vxoBinaryGraph_LoadNBG` | `0x1d8080` | 2380 | `1e7f9ea3f9995af43c7b9eb6354562eb1c2ab20d33ddad29abd9d69322bec38e` | function |
| `vxoBinaryGraph_Patch32bitVa` | `0x1daf70` | 580 | `692c2bb1a03d8f9b4ed1cf144451275f2a44883f05956caf84ceac38778f5346` | function |
| `vxoBinaryGraph_Patch40bitVa` | `0x1dd620` | 1024 | `d74c77380593ecae79dbd84a88a556a86be176e93902cad78591b55f8c7de03a` | function |
| `vxoBinaryGraph_ReleaseNBG` | `0x1d5da0` | 596 | `ecea5bd96cfab493c7b1e130878255a2538267341d6bf49616a6f2373e4e28ea` | function |
| `vxoBinaryGraph_RelocateBinaryEntrance` | `0x1f3bd0` | 184 | `a0bf0c455d7ab37755abf4c05e39eb614963f400c0df8d346a23c7c4ed2f9c14` | function |
| `vxoBinaryGraph_RelocateNetworkBinaryGraph` | `0x1f3c90` | 108 | `28b75a924ada04183f28f35d42e9475ed1052f9a5625a4d5a0010ae4f5276a5d` | function |
| `vxoBinaryGraph_SetPatchDataInfo` | `0x1ecae0` | 472 | `92e6983652d5f90ec692dbf29bc971be09934e68c02794047951b7bbc965ac5b` | function |
| `vsi_nn_GenerateNBG` | `0x4b270` | 32 | `0a6d04a105dd7a5cd317028b3c631e577e16f1c2d1516ca8bdc619fe483089dd` | function |

### 5.3 Export alias clarification

`vxoBinaryGraph_SaveNetworkBinaryGraph` is not a missing implementation. Its
complete four-byte symbol body is:

```text
AArch64 unconditional branch
  -> vxoBinaryGraph_SaveBinaryEntrance@plt
```

This is the only selected probable thunk. It is accepted as an exact,
explicitly named export alias. Every other selected target must resolve to a
full non-thunk body.

### 5.4 Entry path

Static-verified:

```text
vsi_nn_GenerateNBG(graph)
  -> vxGenerateNBG(graph->...)
```

The exact `vxGenerateNBG` body:

- refuses a null graph;
- refuses a graph that already contains binary content;
- verifies and processes the graph;
- supports memory-buffer and file-oriented output;
- uses static OS open, seek, write, flush, and close calls;
- reports the produced NBG buffer and size.

No call was executed in this PoC.

### 5.5 Network serialization and relocation

Static-verified:

```text
vxoBinaryGraph_NetworkBinaryGraph
  -> vxoBinaryGraph_SaveNetworkBinaryGraph
       -> SaveBinaryEntrance implementation through the export alias
  -> vxoBinaryGraph_RelocateNetworkBinaryGraph
       -> vxoBinaryGraph_RelocateBinaryEntrance
```

`SaveBinaryEntrance` is the largest selected body at 13,004 bytes. It has 216
direct call sites in the bounded Ghidra function evidence. The call surface
includes:

- graph/node and memory inspection;
- command-size discovery;
- environment-controlled output behavior;
- chip identity and feature queries;
- allocation and zeroing;
- multiple internal serialization helpers;
- error-path cleanup.

PoC 12A records those edges. It does not assign unresolved internal
`FUN_*` functions semantic names without separate evidence.

### 5.6 NN/TP command chain

Static-verified:

```text
SaveNNTPOperation
  -> SaveNNTPCommand
       -> SaveNNTPInstruction
            -> SaveWeightAndBias
  -> SaveNNTPStates
```

Observed selected-body sizes:

```text
SaveNNTPOperation:    348 bytes
SaveNNTPCommand:    1,984 bytes
SaveNNTPInstruction: 2,780 bytes
SaveNNTPStates:     2,052 bytes
SaveWeightAndBias:    464 bytes
```

`SaveNNTPInstruction` also calls:

- command-type and GPU-count helpers;
- TP/NN input/output serialization;
- address checking;
- trace-zone and debug-dump paths;
- command buffer copies;
- weight/bias specialization helpers.

This proves serializer ownership and ordering. It does not decode the NN/TP
instruction set.

### 5.7 Load, patch, and relocation side

Static-verified target families:

```text
LoadNBG
AllocateBinaryLoadBuffer
ReleaseNBG

SetPatchDataInfo
Patch32bitVa
Patch40bitVa

RelocateBinaryEntrance
RelocateNetworkBinaryGraph
```

`LoadNBG` has explicit cleanup edges to `ReleaseNBG`. Both 32-bit and 40-bit
patch paths are present. Their existence supports a later relocation model;
it does not authorize patch emission in PoC 12A.

## 6. Lossless NBG image

### 6.1 Representation

`nbg_image.py` builds a frozen image from the strict existing
`nbg_format.py` parse:

```text
NbgImage.parse(source bytes)
  -> immutable header/directory/section views
  -> exact owned byte ranges
  -> total-coverage proof

NbgImage.to_bytes()
  -> concatenate owned ranges
  -> compare length
  -> compare every byte
  -> compare SHA-256
```

The writer does not return the cached source payload as its implementation.
It reconstructs the output from the ordered owned ranges.

Any `mark_changed(reason)` result is refused with:

```text
NBG_SEMANTIC_CHANGE_REFUSED
```

There is no field-editing, section-replacement, checksum-update, or custom
graph API.

### 6.2 Exclusive range classes

Every source byte is exactly one of:

```text
decoded-record
alignment
padding
preserved-opaque
connection-tail
```

Known decoded payloads:

- v2 header;
- v2 directory sub-header and 44-byte section records;
- input and output records;
- network info;
- memory-pool records;
- VIP/AXI SRAM records;
- section checksum records.

Known but undecoded command, coefficient, patch, state, shader, and operation
payloads remain `preserved-opaque`.

### 6.3 Connection-tail correction

The earlier generic parser fingerprints the last 512 bytes of every NBG as a
possible connection region.

Exact VPMN v2 evidence shows that all five selected files have section
directory entries extending to EOF. Therefore the final 512-byte view
overlaps ordinary section payloads:

```text
generated Erf:
  final coefficient/load-state sections end at EOF

stock CTC:
  final load-state section ends at EOF
```

Treating the last 512 bytes as a second owner would violate the central
exclusive-ownership invariant and would incorrectly remove coefficient/state
bytes from their sections.

PoC 12A records:

```text
connection_tail_mode: section-covered-view
connection-tail owned bytes: 0
```

For a synthetic VPMN v2 fixture where 512 bytes genuinely follow every
section, the image classifies them as an independent connection tail. The
generic fingerprint remains useful as a view, but it is not automatically a
structural owner.

### 6.4 Alignment rule

For a gap before a section with alignment greater than one:

1. the section offset must satisfy the original alignment;
2. the minimum bytes advancing the prior cursor to the next aligned address
   are `alignment`;
3. any remaining original gap bytes are `padding`;
4. every original value, including nonzero bytes, is preserved.

No alignment or padding bytes are synthesized.

## 7. Round-trip evidence

| Artifact | Decoded | Alignment | Padding | Opaque | Exact |
|---|---:|---:|---:|---:|---|
| `ctc-de` | 1,348 | 156 | 0 | 2,809,480 | yes |
| `ctc-en` | 1,348 | 244 | 0 | 5,261,768 | yes |
| `ctc-fr` | 1,348 | 220 | 0 | 2,933,576 | yes |
| `ctc-ja` | 1,348 | 92 | 0 | 2,716,104 | yes |
| `generated-erf` | 1,104 | 0 | 0 | 4,736 | yes |

For every file:

```text
output bytes == input bytes
output SHA-256 == input SHA-256
first_difference == null
```

No output NBG copy was requested or written.

Tracked evidence:

```text
docs/npu/format/serializer-byte-ownership.json
docs/npu/format/serializer-roundtrip-evidence.json
```

## 8. Coefficient correlation

### 8.1 Strict ONNX admission

The existing PoC 9E admission policy:

- validated the 10,258,790-byte model;
- rejected external data and unsupported graph features;
- ran the ONNX checker and strict shape inference;
- produced graph signature:

```text
89281ef23aec1c0aa4ef3662d84f3f6873d644167be3a99989d73d94d15257ed
```

The correlator then required exact node connectivity and exact initializer
names, types, and shapes.

Initializer identities:

| Name | Shape | Canonical bytes | SHA-256 |
|---|---|---:|---|
| `ctc_lo.bias` | `[4999]` | 19,996 | `08e3d98aaa31b261103ab307f03c776f4f79fccda1644d247afaa91cc9bc022c` |
| `onnx::MatMul_7` | `[512,4999]` | 10,237,952 | `516a6e577b687129fb890d3b2413ac356743ab5ddb2c257734af847985704fbc` |

### 8.2 Search boundary

Only nine English NBG section ranges were searched:

```text
OPERATIONS
NN
TP
SH
PATCH
COEF_DATA_TABLE
LOADSTATE_TABLE
COEF_DATA
LOADSTATE
```

Total admitted bytes per candidate:

```text
5,260,176
```

Headers, IO metadata, checksums, alignment gaps, padding, and the overlapping
last-512-byte view were not searched as coefficient data.

### 8.3 Finite transform registry

Named transform families:

```text
identity FP32 little-endian
FP16 nearest-even little-endian
BF16 nearest-even little-endian
2D transpose in FP32
2D transpose then FP16
2D transpose then BF16
fixed FP32 block permutations
FP16 byte-lane permutations
FP16 halfword-lane permutations
```

Finite widths:

```text
FP32 blocks:       2, 4, 8, 16, 32 elements
byte lanes:        2, 4, 8 bytes
halfword lanes:    2, 4, 8 halfwords
```

Finite patterns:

```text
reverse
even-odd
odd-even
adjacent-pair-swap
```

No factorial permutation generation or heuristic search exists.

### 8.4 Exact search results

Bias:

```text
candidates:                  47
section bytes per candidate: 5,260,176
comparison-byte budget used: 247,228,272
plain FP16 occurrences:      0
all exact occurrences:       0
relationship:                unresolved
```

Weight:

```text
candidates:                  50
section bytes per candidate: 5,260,176
comparison-byte budget used: 263,008,800
plain FP16 occurrences:      0
all exact occurrences:       0
relationship:                unresolved
```

The weight's plain FP16 candidate identity is:

```text
bytes:
  5118976

SHA-256:
  70665e038d7d1a2c8f2886c4d4ee9b8cb2ceb5d32c94577ceaff25569bc8f551
```

The bias's plain FP16 candidate identity is:

```text
bytes:
  9998

SHA-256:
  bc66307c1ca53f8e0c60d1e0e20f4c4d04df67c2e2214530f7a89e7e5d6274ee
```

These no-match results confirm that the NBG coefficient path is not a direct
linear FP16 dump under the reviewed layouts.

### 8.5 Differential controls

| Artifact | Output dimensions | `COEF_DATA` bytes |
|---|---|---:|
| `ctc-en` | `[4999,16,1]` | 5,257,856 |
| `ctc-de` | `[5200,16,1]` | 2,804,480 |
| `ctc-fr` | `[5432,16,1]` | 2,928,576 |
| `ctc-ja` | `[5132,16,1]` | 2,711,104 |

Vocabulary width and coefficient size are not monotonic across these
controls. Language, topology, training artifact, and compiler differences
remain confounded.

The evidence includes:

- section presence, offset, size, stride, and SHA-256;
- exact full-section equality;
- bounded common prefix and suffix;
- bounded 32/64/128-byte block-hash samples;
- size deltas.

These comparisons are `differentially-supported` observations. They do not
promote any packing hypothesis.

### 8.6 PoC 12B decision

```text
not-ready
```

Reason:

> No deterministic invertible coefficient transform is corroborated across
> two tensors or graphs.

This is a successful PoC 12A result. It prevents a guessed serializer from
being mislabeled as compiler progress.

Tracked evidence:

```text
docs/npu/format/onnx-nbg-correlation.json
```

## 9. Evidence files

| File | Purpose |
|---|---|
| `docs/npu/format/serializer-ghidra-evidence.json` | Exact function bytes, hashes, target contexts, callers, callees, diagnostics, and bounded decompilation |
| `docs/npu/format/serializer-byte-ownership.json` | Every owned source range and class total for five NBGs |
| `docs/npu/format/serializer-roundtrip-evidence.json` | Exact input/output size, hash, and first-difference result |
| `docs/npu/format/onnx-nbg-correlation.json` | ONNX admission, finite transforms, exact searches, differential controls, and PoC 12B decision |

The serializer Ghidra evidence was generated twice and compared
byte-for-byte. The other evidence generators are also deterministic and
have explicit `verify-generated` paths.

## 10. Tests

Focused results at completion:

```text
serializer manifest/static tests: 20
lossless image tests:              17
strict NBG parser + image tests:   40 combined
correlation tests:                 14
complete NPU discovery in uv env:  261
ONNX-hybrid package tests:         35
Ruff checks:                       pass
```

Contracts include:

- duplicate-key, schema, path, symlink, hash, and ELF refusal;
- wrong function/ELF role and target-set refusal;
- exact export hash and load-bias binding;
- missing, truncated, duplicate, address-drift, and unexpected-thunk
  refusal;
- gaps, overlaps, invalid range hashes, truncation, and v1 refusal;
- nonzero padding and independent-tail preservation;
- section-covered last-512-byte non-overlap;
- semantic-change refusal;
- five exact real round trips;
- direct, ambiguous, FP16, BF16, transpose, block, byte-lane, and
  halfword-lane fixtures;
- transform, section, candidate, and byte-budget limits;
- deterministic real correlation.

Fresh completion verification also rebuilt PoC 12A's complete Ghidra
evidence and matched the tracked canonical bytes. The shared PoC 1 exporter
compatibility check was split into its six configured targets to stay below
the shell capture ceiling; every regenerated target record matched
`docs/npu/abi/ghidra-evidence.json` exactly with the unchanged default
decompilation limit.

## 11. Reproduction

Validate the exact manifest:

```bash
python3 waved/scripts/npu/nbg_serializer_recovery.py validate
```

Regenerate and verify exact Ghidra evidence:

```bash
python3 waved/scripts/npu/nbg_serializer_recovery.py generate-ghidra
python3 waved/scripts/npu/nbg_serializer_recovery.py verify-ghidra
```

Regenerate and verify ownership/round-trip evidence:

```bash
python3 waved/scripts/npu/nbg_image.py generate-evidence
python3 waved/scripts/npu/nbg_image.py verify-generated
```

Regenerate and verify correlation in the pinned ONNX environment:

```bash
uv run --project waved/tools/onnx-hybrid \
  python waved/scripts/npu/nbg_correlate.py generate

uv run --project waved/tools/onnx-hybrid \
  python waved/scripts/npu/nbg_correlate.py verify-generated
```

Run focused tests:

```bash
python3 -m unittest \
  waved.scripts.npu.tests.test_nbg_serializer_recovery \
  waved.scripts.npu.tests.test_nbg_image \
  waved.scripts.npu.tests.test_nbg_format

uv run --project waved/tools/onnx-hybrid \
  python -m unittest waved.scripts.npu.tests.test_nbg_correlate
```

None of these commands contacts the Wave.

## 12. Safety accounting

Every generated evidence envelope asserts:

```text
compiler_calls:             0
simulator_calls:            0
native_extension_imports:   0
container_registrations:    0
container_runs:             0
adb_calls:                  0
device_calls:               0
generated_custom_nbg_files: 0
mutated_nbg_files:          0
```

Static Ghidra imports analyze ELF bytes in disposable host projects. They do
not execute or dynamically load those libraries.

## 13. What is now proven

Proven:

- exact Wave serializer entry and major generation/load families;
- exact function identities and bounded call/decompile evidence;
- exact VPMN v2 byte ownership for five controls;
- exact unchanged reconstruction;
- explicit mutation refusal;
- strict finite coefficient-search coverage;
- explicit absence of reviewed direct/transformed coefficient matches.

Not proven:

- coefficient packing;
- a decoded command or instruction ISA;
- semantic NBG mutation;
- command, patch, or relocation emission;
- ONNX-to-NBG compilation;
- a fixed Conv graph;
- NPU noise cancellation.

## 14. Next bounded PoC

PoC 12B should not begin as a general packing search.

The next useful input must provide at least one controlled constant
differential, for example:

```text
same topology
same target
same compiler build
same tensor shape
one known constant change
two exact output NBGs
```

Without that controlled pair, corpus differences remain confounded and the
correct decision stays `not-ready`.
