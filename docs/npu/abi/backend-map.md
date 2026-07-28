# Wave Neural-Model Backend Map

The Wave firmware contains several neural-looking APIs. They are not all the
same accelerator path. This map separates dispatch that is proved by call
edges from names that merely coexist in a binary.

## System map

```text
                                  ┌────────────────────────────┐
                                  │ v3 libmodel_infer.so       │
                                  │ AES / ANS APIs             │
                                  │ embedded CPU/MNN runtime   │
                                  └─────────────┬──────────────┘
                                                │ CPU tensors
                                                ▼
                                          Arm Cortex-A CPU

┌──────────────────────────────┐
│ stock ASR / project probe    │
│ aml_module_*                 │
└──────────────┬───────────────┘
               │
               ▼
┌──────────────────────────────┐
│ libnnsdk hardware dispatch   │
├──────────────────────────────┤
│ 0 VIPUNIFY   /dev/galcore ───┼──► libovxlib / OpenVX / GAL ─► NPU
│ 1 VIPLITE    /dev/vipcore    │
│ 2 ADLA       /dev/adla0      │
│ 3 delegate   GPU/CPU TFLite  │
└──────────────────────────────┘
```

The connected Wave’s live stock-CTC control follows the horizontal NPU path.
The v3 AES/ANS API follows the upper CPU path.

## `libnnsdk` dispatch

Exact provider:

```text
sha256: 36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe
```

`aml_module_create` calls `aml_get_hardware_type`, then indexes a family of
parallel function tables for create, input, run, output, and destroy.

| Index | Family | Detection | Static state | Live Wave state |
|---:|---|---|---|---|
| 0 | VIPUNIFY | `/dev/galcore` | Full create/input/run/output/destroy implementation. | **Used by stock CTC.** |
| 1 | VIPLITE | `/dev/vipcore` | Full implementation present. | Device path not observed in PoC 0. |
| 2 | ADLA | `/dev/adla0` | Full implementation and dynamic `libadla.so` API present. | Device path not observed. |
| 3 | Delegate | TFLite GPU/CPU fallback logic, `/dev/mali0` | Delegate exports present. | Not used by NBG stock CTC. |

If no NPU node exists, the selector permits only eligible TFLite models to
fall back to GPU/CPU. It rejects non-TFLite models rather than pretending
they ran on another backend.

The existence of ADLA symbols is not proof of an ADLA block in this unit.
The device-node selection and live graph result establish VIPUNIFY instead.

## VIPUNIFY

VIPUNIFY is the firmware’s wrapper over the VeriSilicon OpenVX stack.

### Compiled graph: “off” branch

Selected by `nbgType` 0 or 1:

```text
NBG file or memory
  -> NBG header/IO parser
  -> hardware target selection
  -> OpenVX context
  -> graph and IO tensors
  -> one NBG node, operation 0x44
  -> setup
  -> verify
  -> run / output
```

This is the live-proven route.

### Reconstructed graph: “on” branch

Selected by `nbgType` 2 or 3:

```text
private export-data file or memory
  -> sectioned export-data parser
  -> per-node parameter conversion
  -> norm/const/virtual tensor creation
  -> connection map
  -> individual OpenVX nodes
  -> setup
  -> verify
  -> run / output
```

The parser recognizes section markers:

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

The parser allocates records with these exact internal strides:

| Record family | Bytes per record |
|---|---:|
| Node | `0x1f0` / 496 |
| Normal tensor | `0x34` / 52 |
| Constant tensor | `0x3c` / 60 |
| Virtual tensor | `0x1c` / 28 |
| Parameter | `0x11c` / 284 |
| Graph connection | `0x10` / 16 |

Its switch covers many operation parameter types and then
`vipunify_create_network_on_ovx` constructs the graph through the same exact
stock OpenVX bridge.

This is the most interesting newly recovered custom-graph route. It is not
yet approved for device execution:

- no complete example export was found in the extracted firmware;
- the entire grammar and all enum mappings are not yet encoded;
- setup may still require target-specific compiler assets;
- malformed input reaches parser paths that call `exit(0)`;
- the provider uses global parse state.

A minimal identity/add export must be generated and validated entirely on the
host before a bounded device PoC is proposed.

## OpenVX bridge

`libnnsdk` dynamically resolves a table of wrapper calls such as:

- context create/release;
- graph create/release;
- graph input/output binding;
- tensor creation;
- node creation;
- setup and verification;
- run;
- output copy.

Provider wrappers such as `aml_vipunify_create_context_ovxlib` and
`aml_vipunify_new_node_ovxlib` are very small indirect calls through that
table. The actual graph engine is in `libovxlib.so`.

The exact `libovxlib.so`, `libOpenVX.so`, `libGAL.so`, `libVSC.so`, and
`libArchModelSw.so` are byte-identical across the four firmware roots in this
corpus. That stability is useful, but it does not supply a host compiler for
the Wave product.

## Compiled NBG compatibility

Known graph headers:

| Graph | NBG version | Product | Runtime result |
|---|---:|---:|---|
| Stock CTC | `0x00020000` | `0x1000000e` | Executes and retrieves output. |
| Host TIM-VX add | `0x0001001e` | `0x10000020` | Rejected before execution. |

`vipunify_load_hw_config` accepts the Wave product and rejects the foreign
target. Patching the field is not compilation and remains prohibited.

The local TIM-VX tree is now at commit `702715d…`, newer than the checkout
used by early experiments. A later compiler PoC should retest this exact
commit against the external stock `libovxlib`, but PoC 1 does not assume that
new source alone contains the necessary VIP9000 target assets.

## Firmware v3 audio API

### Surface

Business and v3 firmware contain `libmodel_infer.so`. v3 exports:

AES-labelled loaders:

- `model_infer_aes_load_vdcrn_16k_0_07m_*`;
- `model_infer_aes_load_vdcrn_16k_0_1m_*`;
- `model_infer_aes_load_vdcrn_32k_0_1m_*`;
- generic load, run, destroy, delay, and version APIs.

ANS-labelled loaders:

- `model_infer_ans_load_vdf2_16k_0_25m_*`;
- `model_infer_ans_load_vdf2_16k_0_35m_*`;
- several 32 kHz VDF2 variants;
- 32 kHz VAIBF variants;
- a 16 kHz multi-channel fusion variant;
- generic load, run, mask, SNR, strength, post-filter, delay, version, and
  destroy APIs.

The suffixes are firmware labels. PoC 1 does not reinterpret every numeric
suffix as a measured parameter count.

### Dependency proof

v3 `libmodel_infer.so`:

```text
bytes:  2681888
sha256: 78b9ddba32f608dd6771b3300fd4f7ce5f2808a1a52b05cdf618348db608bbe5
needed:
  libc.so.6
  libgcc_s.so.1
  libm.so.6
  libstdc++.so.6
```

It has no dynamic dependency on:

- `libnnsdk`;
- `libovxlib`;
- `libOpenVX`;
- `libGAL`;
- `libVSC`.

It imports no relevant Amlogic, VSI-NN, OpenVX, VIP, or GAL symbol.

### Internal proof

The AES internal loader:

1. creates an interpreter from a buffer or file;
2. creates a session;
3. resolves a set of named input tensors;
4. resolves named output tensors;
5. allocates host tensor wrappers;
6. installs an AES function table.

The ANS internal loader performs the same pattern, sometimes for three model
parts, and optionally creates CPU worker threads.

The binary contains a full CPU backend and diagnostics such as tensor-copy,
session-resize, and CPU operator failures. This is behavior, not just the
word “CPU” in an unused string table.

### Caller proof

The v3 signal-processing DSO:

```text
path:   usr/lib/libspai_mic_sigp.so
bytes:  30330096
sha256: 8045da803d71ec3ea3e1c058340745ba2bd878eb28557300eba5cf65d8e5af1c
```

loads several ANS models and one AES model, launches audio worker threads, and
calls:

```text
model_infer_aes_run(context, spectral_input, auxiliary_output, mask)
model_infer_ans_run(context, spectral_input, output)
model_infer_ans_get_mask(context, mask)
model_infer_ans_get_snr_db(context)
```

One recovered path copies a 513-float (`0x804`-byte) ANS mask and applies it
inside a larger classical DSP/beamforming pipeline. This is a useful design
reference for an NPU model: accelerate the neural mask estimator while
keeping STFT, beamforming, smoothing, and failover explicit.

### Classification

| Claim | Result |
|---|---|
| v3 exposes useful denoising-model APIs | Proved. |
| stock audio calls those APIs | Proved. |
| the library runs a CPU interpreter/backend | Proved statically. |
| those AES/ANS calls use the Wave NPU | **Refuted for this DSO.** |
| the embedded models can be converted to Wave NBG | Unknown. |
| the v3 pipeline is a useful quality/reference workload | Strong inference; dynamic reference PoC still needed. |

## Noise-suppression implications

The quickest credible NPU target is not “run the v3 API on the NPU.” It is:

1. use the v3 ANS behavior to understand frame, feature, mask, delay, and
   post-filter contracts;
2. choose or train an openly reproducible small mask estimator;
3. establish a Wave-targeting graph route;
4. prove its operations and recurrent state;
5. compare against the existing CPU DeepFilterNet path and, if legally and
   operationally usable, the v3 reference.

Candidate architecture constraints now visible from stock code:

- streaming operation;
- bounded frames;
- explicit state/delay;
- spectral-mask output is practical;
- 16 kHz and 32 kHz variants are relevant;
- multi-channel fusion exists in stock CPU software;
- CPU DSP around the neural core remains substantial.

## Custom-model route matrix

| Route | Backend | Current evidence | Next PoC |
|---|---|---|---|
| Stock Wave NBG through `libnnsdk` | NPU | Live-proven. | Parse full stock corpus. |
| Foreign host NBG through `libnnsdk` | NPU intended | Product mismatch; safely rejected. | Retest a genuinely Wave-targeted compiler only. |
| Private export-data mode 3 | NPU/OpenVX intended | Parser and graph-builder binary-proven. | Reconstruct grammar; bounded identity graph. |
| Direct stock `libovxlib` via current TIM-VX | NPU intended | Context reachability known; current commit not yet retested end-to-end. | Minimal graph with exact stock runtime. |
| v3 `libmodel_infer` AES/ANS | CPU | Static backend proved. | Optional CPU reference/quality characterization. |
| Raw `/dev/galcore` replay/fuzzing | NPU | No safe contract. | Not part of this program. |

## Production cautions

- Serialize graph create/destroy until global-state behavior is tested.
- Do not mix export-data and NBG modes concurrently.
- Treat model input as trusted data; several parser errors call `exit`.
- Keep a CPU audio path alive and independently testable.
- Never make driver reset, service stop, or reboot an automatic recovery
  policy.
- Require exact library/model hashes for every device experiment.
