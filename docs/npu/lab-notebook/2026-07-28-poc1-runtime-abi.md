# PoC 1 — Runtime ABI and Call Graph

## Record

| Field | Value |
|---|---|
| Date | 2026-07-28 |
| PoC | 1 |
| Safety class | Host-only static analysis |
| Device used | No |
| Hypothesis | Exact firmware providers and callers are sufficient to recover a narrow, stable graph lifecycle ABI and classify the active backend without guessing from names. |
| Result | **Passed** |
| Plan | `docs/superpowers/plans/2026-07-28-wave-npu-poc1-runtime-abi.md` |

## Exit gate

The gate required:

- a strict, hash-pinned four-firmware corpus;
- deterministic ELF inventory and exact function-byte comparisons;
- focused decompiler/call-graph evidence for project FFI and backend code;
- public-header cross-check;
- executable `repr(C)` layout tests;
- documented prototypes, lifecycle, backend map, errors, limits, and next
  decision.

All items passed.

## Safety

Allowed:

- read extracted firmware files;
- hash and statically analyze ELF files;
- run Rizin and Ghidra on the host;
- read pinned public sources;
- compile and run host tests;
- generate tracked JSON/Markdown.

Excluded:

- ADB;
- device execution;
- `/dev/galcore`;
- sysfs;
- service changes;
- model or firmware changes;
- kernel-module changes;
- reboot;
- raw command replay.

No device state was read or changed during PoC 1.

## Exact corpus

Corpus definition:

```text
path:   waved/scripts/npu/abi_corpus.json
sha256: 2276074b6405657a657bb063391a172a57c41a44f10f8418a836bd692a1cc053
```

Firmware IDs:

```text
fw_v1.0.13.6
fw_v1.1.6.6        live-reference family
fw_business_v1.0.2.6
fw_v3.0.0.1
```

Logical binary families:

```text
libnnsdk.so
libovxlib.so
libOpenVX.so
libGAL.so
libVSC.so
libArchModelSw.so
libasr_reverb.so
libmodel_infer.so (optional)
```

The scanner also discovered actual importers in bounded firmware ELF roots.

## Tools

```text
Python:  host Python 3
rz-bin:  0.8.2, Homebrew darwin-arm64
Ghidra:  12.0.4
Ghidra language: AARCH64:LE:64:v8A
Ghidra compiler spec: default
```

Relevant implementation commits:

```text
781fa6e  PoC 1 implementation plan
b06b465  deterministic runtime ABI inventory and matrix
0738c0e  deterministic focused Ghidra export
cba7bbb  expanded NBG/export/audio internals and Rust layout locks
```

## Public references

Pinned:

```text
Amlogic AML_NN_SDK:
  commit 96f7846bbbac4def9ccc38b649a43e77a8194435
  Linux/examples/include/nn_sdk.h
  git blob 22c0be4baeea5e1827b70cf8a5b611bcd288c002

TIM-VX:
  commit 702715d714277376d7ff466b7b72833ff57564c1

eYs3D rootfs:
  commit f6d3c3b87667d333eaa0785d0a1a2078ca5bfdc8

eYs3D kernel:
  commit 946d2c841fd82b916bdd2168033906bfec89db61
```

Public headers were used only to cross-check names, enums, and compatible
substructures. Exact firmware caller/provider behavior remained primary.

## Inventory execution

Commands:

```bash
python3 waved/scripts/npu/abi_inventory.py inventory \
  --output docs/npu/abi/runtime-abi-inventory.json

python3 waved/scripts/npu/abi_inventory.py matrix \
  --inventory docs/npu/abi/runtime-abi-inventory.json \
  --output docs/npu/abi/firmware-matrix.md

python3 waved/scripts/npu/abi_inventory.py verify-generated
```

Outputs:

```text
runtime-abi-inventory.json
  sha256 0706ed93101d10457d4e3cbeea43fd2e9f0272f34b2071286df50807ca329b4d

firmware-matrix.md
  sha256 3a3b6862e2ab1fc9390af81f94029175613cf15572b168ca272ba9d2526e9070
```

The inventory records exact physical function bytes. During development an
apparent set of `vipunify_*.c` “exports” was traced to ELF `FILE` records.
The generator was corrected to retain only relevant `FUNC`/`NOTYPE` symbols,
and a regression test was added. This negative finding matters because names
without symbol type would have overstated the ABI surface.

## Ghidra execution

Target definition:

```text
path:   waved/scripts/npu/ghidra_targets.json
sha256: fdaef62504eefdb6df1d458f316af58920dd0bae3f62264b4a4492a956c71042
```

Command:

```bash
python3 waved/scripts/npu/ghidra_abi.py generate \
  --output docs/npu/abi/ghidra-evidence.json
```

Output:

```text
sha256: aa34531bb3245bca3cc89d32382ed9aa15e6e9c7e067f268020a1911c831739a
```

Coverage:

| Target | Requested | Missing | Function records | Non-external target failures | Warnings |
|---|---:|---:|---:|---:|---:|
| Business model-infer | 8 | 0 | 8 | 0 | 0 |
| Live ASR caller | 7 | 0 | 57 | 0 | 0 |
| Live `libnnsdk` | 43 | 0 | 108 | 0 | 0 |
| Live `libovxlib` | 10 | 0 | 57 | 0 | 0 |
| v3 audio caller | 10 | 0 | 47 | 0 | 0 |
| v3 model-infer | 19 | 0 | 24 | 0 | 0 |

Imported external stubs were expected not to decompile. Their local direct
callers were exported and decompiled.

## Host tests

Python:

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -p 'test_*.py'
```

Result:

```text
49 passed
```

Rust:

```bash
cargo test --manifest-path waved/Cargo.toml -p waved-npu-probe
```

Result:

```text
15 passed
```

The Rust suite locks all relied-upon create/input/output/tensor structure
sizes and offsets.

## Observed static findings

### Stable wrapper ABI

Exact project-facing function bytes were identical across all four firmware
families:

```text
aml_module_create
aml_module_destroy
aml_module_input_set
aml_module_output_get
aml_util_getInputTensorInfo
aml_util_getOutputTensorInfo
aml_util_freeTensorInfo
```

This is binary identity, not just equal symbol names.

### Stock caller layout

Three independent caller families build a 2272-byte create config with:

```text
+0x00 typeSize
+0x04 model bytes
+0x08 path
+0x10 model data
+0x18 model type
+0x1c source type
```

The provider reads private fields at `+0x878` and `+0x8d8`. The public header
is not layout-compatible.

The stock output command is a 56-byte by-value aggregate. The first output
data pointer is at returned-object offset `0x50`.

### Hardware dispatch

The exact selector maps:

```text
/dev/galcore -> VIPUNIFY / backend 0
/dev/vipcore -> VIPLITE / backend 1
/dev/adla0   -> ADLA / backend 2
/dev/mali0   -> eligible TFLite GPU/CPU delegate fallback
```

PoC 0’s `/dev/galcore` and successful Wave-targeted NBG establish VIPUNIFY as
the stock CTC route.

### Compiled NBG modes

`nbgType` 0/1 selects the compiled NBG path. It:

1. parses NBG metadata;
2. selects hardware product;
3. creates an OpenVX graph;
4. adds operation `0x44` as an NBG node;
5. binds IO tensors;
6. sets up and verifies the graph.

### NBG v2 header

The exact parser confirms:

```text
+0x00 "VPMN"
+0x04 version
+0x08 target/product
```

It reads a 44-byte-entry descriptor table. Types 1 and 2 locate inputs and
outputs; each IO item is normalized to 128 bytes. This is enough to begin
PoC 2 without claiming the full schema.

### Private export-data modes

`nbgType` 2/3 selects a second graph path. The memory parser recognizes:

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

It parses individual operations and parameters, then builds an OpenVX graph
node by node. No complete example was present in the extracted firmware. The
branch is real; a safe custom graph is not yet proved.

### v3 AES/ANS backend

The v3 `libmodel_infer.so` exposes multiple AES/ANS loaders, but:

- links only standard CPU-side libraries;
- imports no NPU/OpenVX/VSI/GAL API;
- embeds a CPU backend;
- creates interpreter sessions and host tensor wrappers;
- is invoked by v3 signal-processing CPU threads.

Conclusion: this DSO is a CPU model runner, not the NPU route.

The v3 caller uses ANS masks inside a broader spectral/beamforming pipeline.
That is a useful workload shape for a future custom NPU model.

## Negative findings

- Public `aml_config`, `input_info`, output command, and output buffer layouts
  do not match Wave firmware.
- Function names alone previously overstated some ELF records; type filtering
  fixed the inventory.
- The v3 AES/ANS API does not reach the NPU.
- ADLA code presence does not prove ADLA hardware.
- The exact foreign add NBG remains a target mismatch, not a usable custom
  graph.
- No complete private export-data model was found in the extracted roots.

## Strong inferences

- Project FFI can remain narrow and stable across the four firmware families
  studied.
- Graph creation/parsing should be serialized initially because the provider
  uses global parse/backend state and creation mutexes.
- A spectral-mask estimator is a better first noise-suppression accelerator
  than porting an entire multichannel DSP pipeline.
- The private export-data path may offer a compiler-bypass route for simple
  graphs, but setup/verification could still require unavailable
  target-specific assets.

## Unknowns

- Complete NBG v2 section semantics and relocation/command tables.
- All export-data grammar tokens and operation enum mappings.
- Whether a handcrafted export-data identity graph verifies on this Wave.
- Whether current TIM-VX commit `702715d…` can construct or compile a graph
  with the exact stock external runtime.
- Kernel ioctl payloads and runtime command stream.
- v3 embedded model serialization and legal/reproducible extraction path.
- Operation/dtype/state support for a practical denoiser.
- Real-time quality and deadline behavior for any custom model.

## Errors discovered

- `aml_module_input_set` returns zero after dispatch without propagating the
  backend status.
- one invalid-context `aml_module_output_get` path returns integer 1 in a
  pointer return.
- several vendor parser failures call `exit(0)`.
- early create failures appear to leak some wrapper allocations.

The harness’s required result line prevents vendor `exit(0)` from being
misclassified as success.

## Conclusion

The hypothesis passed.

The runtime boundary is now explicit enough to:

- keep the stock control FFI;
- review every relied-upon layout;
- parse the stock NBG corpus safely on the host;
- separate actual NPU execution from CPU audio-model APIs;
- investigate the private export-data graph path as its own bounded PoC.

PoC 1 does not authorize any new device graph.

## Decision

Proceed to PoC 2:

1. implement a strict, non-executing NBG parser;
2. inventory all stock NBGs across the firmware corpus;
3. cross-check parser metadata with the live-proven CTC metadata;
4. identify representative audio-shaped graphs and required operations;
5. publish a format map with bounded unknown sections.

After PoC 2, prepare a separate safety-reviewed experiment for either:

- a minimal private export-data identity/add graph, or
- a graph generated with the current pinned TIM-VX stack and an authentic
  Wave target.

Still prohibited:

- product-field patch-and-run;
- blind driver mutation;
- stock model overwrite;
- service stop, sysfs write, module replacement, reboot, or flash without
  explicit approval.
