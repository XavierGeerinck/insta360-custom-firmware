# Wave NPU Runtime ABI Evidence

PoC 1 recovers the userspace boundary used by Wave firmware to load and run
neural-network graphs. It is a host-only static-analysis result over exact
firmware files; it performs no ADB operation and no device mutation.

The central result is that the project’s existing stock-CTC control uses a
stable four-call Amlogic wrapper over the VeriSilicon `VIPUNIFY`/OpenVX
backend:

```text
Wave caller
  aml_module_create
    -> hardware dispatch: /dev/galcore => backend 0
    -> aml_vipunify_create_network
       -> NBG file/memory path for nbgType 0/1
       -> export-data graph path for nbgType 2/3
  aml_module_input_set
  aml_module_output_get(perfMode=1)  # execute
  aml_module_output_get(perfMode=2)  # retrieve
  aml_module_destroy
```

The exact project-used wrapper implementations are byte-identical in all four
firmware roots in the corpus. The public Amlogic SDK is useful for names and
enums, but several Wave structures are a private, expanded ABI and must not be
declared from the public header alone.

## Deliverables

| File | Purpose |
|---|---|
| [`runtime-abi-inventory.json`](runtime-abi-inventory.json) | Deterministic ELF identities, imports, exports, relocations, selected diagnostics, exact function bytes, and caller discovery. |
| [`firmware-matrix.md`](firmware-matrix.md) | Human-readable exact-function comparison across four firmware families. |
| [`ghidra-evidence.json`](ghidra-evidence.json) | Bounded decompilation, callers, callees, call sites, and diagnostics for 69 requested targets. |
| [`prototypes.md`](prototypes.md) | Recovered FFI declarations, layouts, ownership, confidence, and public-header deltas. |
| [`stock-ctc-call-graph.md`](stock-ctc-call-graph.md) | End-to-end lifecycle of the live-proven stock graph. |
| [`backend-map.md`](backend-map.md) | Hardware dispatch, VIPUNIFY modes, OpenVX bridge, and v3 audio-loader classification. |
| [`error-taxonomy.md`](error-taxonomy.md) | Native failure semantics and their mapping into project errors. |
| [PoC 1 notebook](../lab-notebook/2026-07-28-poc1-runtime-abi.md) | Hypothesis, execution record, findings, limits, and decision. |

The generators and configurations are:

```text
waved/scripts/npu/
  abi_corpus.json
  abi_inventory.py
  ghidra_targets.json
  ghidra_abi.py
  tests/test_abi_inventory.py
  tests/test_ghidra_abi.py

scripts/ghidra/
  ExportNpuAbi.java
```

## Evidence classes

This documentation uses the PoC 1 evidence hierarchy:

| Class | Label | Meaning |
|---|---|---|
| A | `live-proven` | The exact contract was exercised successfully by an allowlisted live experiment. |
| B | `binary-proven` | Exact hashed provider/caller bytes, call references, and disassembly/decompilation agree. |
| C | `header-cross-checked` | A pinned public vendor header agrees with the exact binary evidence. |
| D | `strong-inference` | Multiple observations agree, but a type, branch, or semantic detail remains unresolved. |
| E | `name-only` | An export or diagnostic exists without proof that the relevant device follows that path. |
| F | `unknown` | Evidence is insufficient or contradictory. |

Only A–C facts are used as safe project FFI. Decompiler-generated parameter
names and C types are never treated as declarations by themselves.

## Exact corpus

The corpus configuration SHA-256 is:

```text
2276074b6405657a657bb063391a172a57c41a44f10f8418a836bd692a1cc053
```

Firmware roots:

| Firmware ID | Role |
|---|---|
| `fw_v1.0.13.6` | Older consumer firmware. |
| `fw_v1.1.6.6` | Exact live-reference family used by PoC 0. |
| `fw_business_v1.0.2.6` | Business firmware with the generic audio model API. |
| `fw_v3.0.0.1` | Newer firmware with expanded AES/ANS loaders. |

The inventory checks eight logical binaries per firmware:

- `libnnsdk.so`;
- `libovxlib.so`;
- `libOpenVX.so`;
- `libGAL.so`;
- `libVSC.so`;
- `libArchModelSw.so`;
- `libasr_reverb.so`;
- optional `libmodel_infer.so`.

It also scans bounded ELF roots for actual importers. This found the live
`libasr_reverb.so` caller, the business and v3 audio signal-processing
callers, and `libnndemo.so`.

## Pinned public cross-checks

Public sources are reference material, not a substitute for firmware:

- [Amlogic `nn_sdk.h` at commit `96f7846…`](https://github.com/Amlogic-NN/AML_NN_SDK/blob/96f7846bbbac4def9ccc38b649a43e77a8194435/Linux/examples/include/nn_sdk.h),
  Git blob `22c0be4baeea5e1827b70cf8a5b611bcd288c002`;
- [TIM-VX at commit `702715d…`](https://github.com/VeriSilicon/TIM-VX/tree/702715d714277376d7ff466b7b72833ff57564c1);
- [TIM-VX NBG fixed-layout header](https://github.com/VeriSilicon/TIM-VX/blob/702715d714277376d7ff466b7b72833ff57564c1/include/tim/utils/nbg_parser/gc_vip_nbg_format.h);
- [eYs3D VIP9000 rootfs at `f6d3c3b…`](https://github.com/eYs3D/eCV5546_rootfs/tree/f6d3c3b87667d333eaa0785d0a1a2078ca5bfdc8);
- [eYs3D VIP9000 kernel at `946d2c8…`](https://github.com/eYs3D/eCV5546_linux/tree/946d2c841fd82b916bdd2168033906bfec89db61).

The local TIM-VX checkout is exactly
`702715d714277376d7ff466b7b72833ff57564c1`. Older project notes that call
the checkout “v1.2.22” describe an earlier point in the investigation; PoC 1
records the full current commit instead of relying on a version nickname.

## Reproduce the inventory

Requirements:

- Python 3.10 or newer;
- `rz-bin` 0.8.2 on `PATH`;
- the four extracted firmware roots at the configured paths.

Generate:

```bash
python3 waved/scripts/npu/abi_inventory.py inventory \
  --output docs/npu/abi/runtime-abi-inventory.json

python3 waved/scripts/npu/abi_inventory.py matrix \
  --inventory docs/npu/abi/runtime-abi-inventory.json \
  --output docs/npu/abi/firmware-matrix.md
```

Verify that tracked outputs are byte-for-byte current:

```bash
python3 waved/scripts/npu/abi_inventory.py verify-generated
```

The generated inventory and matrix SHA-256 values for this PoC are:

```text
0706ed93101d10457d4e3cbeea43fd2e9f0272f34b2071286df50807ca329b4d
3a3b6862e2ab1fc9390af81f94029175613cf15572b168ca272ba9d2526e9070
```

## Reproduce the focused Ghidra export

Requirements:

- Ghidra 12.0.4;
- Java 21;
- `analyzeHeadless` at a standard Homebrew path or named by
  `GHIDRA_ANALYZE_HEADLESS`;
- the same exact firmware corpus.

Generate:

```bash
python3 waved/scripts/npu/ghidra_abi.py generate \
  --output docs/npu/abi/ghidra-evidence.json
```

Verify:

```bash
python3 waved/scripts/npu/ghidra_abi.py verify-generated
```

The target configuration SHA-256 is:

```text
fdaef62504eefdb6df1d458f316af58920dd0bae3f62264b4a4492a956c71042
```

The generated evidence SHA-256 is:

```text
aa34531bb3245bca3cc89d32382ed9aa15e6e9c7e067f268020a1911c831739a
```

The exporter creates a temporary Ghidra project per target and validates the
ELF SHA-256 before analysis. It records:

- Ghidra version, language, compiler specification, and program digest;
- exact requested symbol;
- all matching function entries, including thunks;
- direct call references and call-site addresses;
- up to 64 local direct callers;
- bounded diagnostic string references;
- bounded decompiled C and truncation state.

It does not retain a Ghidra project or modify the input ELF.

## Coverage result

| Evidence target | Requested symbols | Missing | Exported function records | Provider decompile failures | Warnings |
|---|---:|---:|---:|---:|---:|
| Business model-infer | 8 | 0 | 8 | 0 | 0 |
| Live ASR caller | 7 | 0 | 57 | 0 | 0 |
| Live `libnnsdk` | 43 | 0 | 108 | 0 | 0 |
| Live `libovxlib` | 10 | 0 | 57 | 0 | 0 |
| v3 audio caller | 10 | 0 | 47 | 0 | 0 |
| v3 model-infer | 19 | 0 | 24 | 0 | 0 |

Imported external stubs in caller DSOs are intentionally not decompiled.
Their local direct callers are decompiled instead. Every non-external target
record decompiled successfully.

## High-confidence findings

### Stable project ABI

The exact bytes of these provider functions are identical across all four
firmware roots:

| Function | Bytes | Exact function SHA-256 prefix |
|---|---:|---|
| `aml_module_create` | 864 | `800a5d8d4eba` |
| `aml_module_destroy` | 624 | `31a0d8b74c37` |
| `aml_module_input_set` | 580 | `4ceeacc26be8` |
| `aml_module_output_get` | 556 | `1aa64d8f6ded` |
| `aml_util_freeTensorInfo` | 136 | `c7bc9725d074` |
| `aml_util_getInputTensorInfo` | 88 | `b5d7b0aa5141` |
| `aml_util_getOutputTensorInfo` | 132 | `4a723ca25521` |

These hashes come from exact physical function bytes, not symbol names or
virtual addresses.

### Private Wave structure fork

The stock caller zeroes a `0x8e0`-byte create configuration and writes:

```text
+0x000  typeSize = 0x8e0
+0x004  NBG byte length
+0x008  file path pointer
+0x010  in-memory NBG pointer
+0x018  model type
+0x01c  NBG source/type
...
+0x878  private mode flag read by create error handling
+0x8d8  private hardware-selection flag
```

The pinned public `aml_config` is much smaller and places `length` after both
pointers. Treating it as the Wave layout would pass the wrong fields.

The same issue exists for input metadata, output configuration, and output
descriptors. Tensor-info entries, by contrast, agree with the public header.
See [`prototypes.md`](prototypes.md).

### Backend dispatch

`aml_get_hardware_type` probes:

```text
/dev/galcore -> backend 0, VIPUNIFY
/dev/vipcore -> backend 1, VIPLITE
/dev/adla0   -> backend 2, ADLA
/dev/mali0   -> GPU/CPU delegate fallback for eligible TFLite models
```

The tested Wave exposes `/dev/galcore`, and the live-proven NBG path therefore
uses backend 0. Compiled ADLA functions do not imply that this unit has ADLA
hardware.

### Four VIPUNIFY model-source modes

`aml_vipunify_create_network` selects:

| `nbgType` | Source | VIPUNIFY branch |
|---:|---|---|
| 0 | NBG file | `vipunify_create_network_off` |
| 1 | NBG memory | `vipunify_create_network_off` |
| 2 | Export-data file | `vipunify_create_network_on` |
| 3 | Export-data memory | `vipunify_create_network_on` |

The “off” path parses a compiled `VPMN` NBG, creates an OpenVX graph, inserts
an NBG node (`0x44`), binds IO, then runs setup and verification.

The “on” path parses a private text/binary export whose visible section
markers include:

```text
[export.data.info]:
[node_info]:
[norm_tensor]:
[const_tensor]:
[virtual_tensor]:
[conn_info]:
[param_info]:
[graph_info]:
```

It constructs individual OpenVX nodes and tensors, then calls setup and
verification. This is a new research path, not yet a safe executable format:
PoC 1 proves the parser and branch exist, but no handcrafted export has been
executed.

### NBG v2 header parser

For NBG versions at or above `0x00020000`, the firmware parser reads:

```text
+0x00  "VPMN"
+0x04  version
+0x08  target/product
```

It then consumes a table described by later header fields. Each table record
is 44 bytes; record type 1 describes input data and type 2 describes output
data. The parser normalizes each IO record into a 128-byte internal entry.
This is sufficient to make PoC 2 a bounded parser exercise, but PoC 1 does
not claim the full NBG schema.

### Firmware v3 AES/ANS is not the NPU path

The v3 `libmodel_infer.so` exposes attractive APIs such as:

- `model_infer_aes_load_vdcrn_16k_0_07m_from_buf`;
- `model_infer_ans_load_vdf2_16k_0_25m_from_buf`;
- `model_infer_ans_run`;
- `model_infer_ans_get_mask`;
- `model_infer_ans_get_snr_db`.

Static evidence classifies this DSO as a CPU/MNN-style model runner:

- its only dynamic dependencies are `libc`, `libgcc_s`, `libm`, and
  `libstdc++`;
- it imports no `aml_*`, `vsi_nn_*`, OpenVX, VIP, GAL, or galcore API;
- it contains a complete CPU-backend implementation and diagnostics;
- the internal AES and ANS loaders create interpreter/session objects,
  resolve named tensors, and allocate host tensor wrappers;
- the v3 audio caller invokes these APIs from its signal-processing threads.

Therefore the v3 denoiser is valuable as a workload and quality reference,
but its API is not evidence of NPU acceleration. See
[`backend-map.md`](backend-map.md).

## Non-claims

PoC 1 does not claim:

- a complete NBG format;
- a Wave-targeting compiler;
- that a foreign NBG becomes valid if its product field is changed;
- that the private export-data path accepts arbitrary handcrafted graphs;
- that the v3 AES/ANS model format is fully recovered;
- that compiled ADLA or VIPLITE code is active on this hardware;
- safe raw access to `/dev/galcore`;
- production-ready neural noise suppression.

## Tests

Host tooling:

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -p 'test_*.py'
```

The PoC 1 completion run passed 49 tests.

Project FFI layouts:

```bash
cargo test --manifest-path waved/Cargo.toml -p waved-npu-probe
```

The completion run passed 15 tests, including exact AArch64 `repr(C)` sizes
and offsets for create, input, output, buffer, and tensor-info structures.

## Next decision

PoC 2 should implement a strict host parser for NBG v1/v2 headers and IO
records, inventory every stock NBG, and compare parser output with the
live-proven metadata API. It must remain non-executing for mutated data.

After that, a separate, explicitly reviewed PoC can test a minimal
`nbgType=3` export-data identity/add graph. That experiment must have a native
watchdog, exact hash allowlisting, pre/post state capture, and no fallback to
raw driver mutation.
