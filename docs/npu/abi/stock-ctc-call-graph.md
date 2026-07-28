# Stock CTC NPU Call Graph

This document connects the live PoC 0 result to exact static caller and
provider code.

## Identities

Caller:

```text
path:   firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libasr_reverb.so
sha256: 9097f2aaffcab1982c77ea0caa82aabc891db67a603283e0d03132a10085c392
role:   live-matched stock ASR caller
```

Provider:

```text
path:   firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libnnsdk.so
sha256: 36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe
role:   NN SDK wrapper and backend dispatch
```

NPU runtime:

```text
libovxlib.so
sha256: 09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9
```

Live control model:

```text
path:    /model/asr_de/de_online/ctc_online.nb
bytes:   2810984
sha256:  5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66
magic:   VPMN
version: 0x00020000
target:  0x1000000e
```

## Lifecycle overview

```text
wenet::AmlnnAsrModel::Read
  │
  ├─ read ctc_online.nb into ordinary process memory
  ├─ zero 0x8e0-byte create configuration
  ├─ set typeSize, length, data pointer, modelType=ONNX, nbgType=memory
  └─ aml_module_create
       │
       ├─ aml_get_hardware_type
       │    └─ /dev/galcore exists => hardware type 0
       │
       └─ NNsdk_module_create[0]
            └─ aml_vipunify_create_network
                 └─ nbgType 1 => vipunify_create_network_off
                      ├─ read_nbg_from_memory
                      ├─ parse NBG v2 header and IO table
                      ├─ aml_vipunify_create_context_ovxlib
                      ├─ vipunify_load_hw_config
                      ├─ aml_vipunify_create_graph_ovxlib
                      ├─ aml_vipunify_set_graph_inoutputs_ovxlib
                      ├─ aml_vipunify_new_node_ovxlib(op=0x44/NBG)
                      ├─ create IO tensor descriptors
                      ├─ aml_vipunify_setup_graph_ovxlib
                      └─ aml_vipunify_verify_graph_ovxlib

wenet::AmlnnAsrModel::ForwardEncoderFunc
  │
  ├─ aml_module_input_set(context, input descriptors)
  ├─ aml_util_flushTensorHandle(context, input)
  ├─ aml_module_output_get(context, perfMode=1)  # execute
  ├─ aml_util_flushTensorHandle(...)
  └─ aml_module_output_get(context, perfMode=2)  # retrieve
       └─ first output data pointer at returned object + 0x50

wenet::AmlnnAsrModel::Destroy / destructor
  ├─ free caller-owned staging buffers
  └─ aml_module_destroy
       └─ NNsdk_module_destroy[0]
            └─ aml_vipunify_destroy
                 └─ release graph/context/backend allocations
```

## Create call site

One recovered `Read` implementation is at caller entry `0x00408b30`. It is a
large C++ function, but the ABI-relevant sequence is unambiguous:

1. Allocate/read the model.
2. Zero a stack object of `0x8e0` bytes.
3. Place the model byte length in the high 32 bits of the first 64-bit stack
   word and `0x8e0` in the low 32 bits.
4. Store the in-memory model pointer at stack offset `0x10`.
5. Write `modelType=4` at `+0x18`.
6. Write `nbgType=1` at `+0x1c`.
7. Pass the object address to `aml_module_create`.
8. Store the returned wrapper in the ASR object.
9. Treat null as fatal in the stock process.
10. Free the original read buffer after create returns.

Step 10 establishes an important ownership property: backend creation fully
consumes or copies what it needs from the NBG synchronously. The project probe
conservatively keeps its model `Vec` alive for the graph lifetime anyway.

The diarization callers independently construct the same 2272-byte object:

- `diarization::VoiceModel::VoiceModel` at `0x0046e470`;
- `diarization::CAMPPlusModel::CAMPPlusModel` at `0x0046e7d0`.

Their agreement is why the layout is binary-proven rather than inferred from
one optimized function.

## Top-level create wrapper

Provider entry: `aml_module_create` at `0x00110ba0`.

Behavior:

1. Log SDK entry/version.
2. `calloc(0x28, 1)` for a wrapper.
3. Initialize wrapper hardware type to `-1`.
4. Reject a null config.
5. Pass:

   ```text
   destination hardware type = wrapper + 0x08
   hardware-selection flag   = config + 0x8d8
   model/path discriminator  = *(config + 0x08)
   ```

   to `aml_get_hardware_type`.

6. Index `NNsdk_module_create` by hardware type.
7. Save the backend context at wrapper `+0x00`.
8. Return the wrapper.

The wrapper accepts only backend values 0–3.

## Hardware selection

`aml_get_hardware_type` probes four device nodes:

| Probe | Selected path |
|---|---|
| `/dev/galcore` | VIPUNIFY backend 0 |
| `/dev/vipcore` | VIPLITE backend 1 |
| `/dev/adla0` | ADLA backend 2 |
| `/dev/mali0` without an NPU | TFLite GPU/CPU delegate backend 3 |

PoC 0 observed `/dev/galcore` on the connected Wave and executed a graph
whose product is accepted by the VIPUNIFY runtime. The ADLA branch is compiled
but not the live CTC path.

## NBG v2 parsing

`read_nbg_from_memory` branches at version `0x00020000`.

For v2:

```text
read_nbg_from_memory
  ├─ parse_nb_head_info (Ghidra name FUN_00113810)
  │    ├─ validate "VPMN"
  │    ├─ read version at +0x04
  │    ├─ read target at +0x08
  │    └─ locate/copy a 44-byte-entry descriptor table
  │
  └─ parse_nb_io_info (Ghidra name FUN_00113a20)
       ├─ find table type 1 => inputs
       ├─ find table type 2 => outputs
       ├─ allocate 128 bytes per normalized IO entry
       └─ copy dimensions, data/quantization fields, and names
```

The parser also copies the final 512 bytes of the NBG into a connection
buffer. The meaning of every table type and trailer byte is deferred to
PoC 2.

## Product check

`get_product_ID` returns a different global depending on whether NBG version
is below `0x00020000`.

The v2 stock graph yields `0x1000000e`. `vipunify_load_hw_config` recognizes
that value and installs the internal hardware label
`VIPNANOQI_PID0X1E` (the string’s `1E` spelling does not match the numeric
low byte `0E`; the numeric comparison is authoritative).

A host-generated add graph with target `0x10000020` reaches this code and is
rejected as not present in the product list. The target field therefore
participates in runtime hardware selection; it is not a cosmetic ID.

## Graph construction

The compiled-NBG path does not reconstruct every original neural operation.
It creates one OpenVX graph node with operation value `0x44`, assigns the NBG
source to that node, creates/binds the normalized IO tensors, and calls setup
and verify.

That boundary is:

```text
NBG contains compiled graph and command data
             │
             ▼
OpenVX NBG node (0x44)
             │
             ▼
libovxlib / libOpenVX / libGAL
             │
             ▼
/dev/galcore
```

This explains why a syntactically valid foreign NBG can parse but still fail:
the embedded compiled target must match the runtime hardware configuration.

## Input submission

Recovered stock calls include:

- four `wenet::AmlnnAsrModel::ForwardEncoderFunc` variants;
- `diarization::VoiceModel::Run`;
- `diarization::CAMPPlusModel::Run`.

All use the same descriptor offsets:

```text
input + 0x04  tensor index
input + 0x08  byte count
input + 0x10  data pointer
input + 0x18  input kind
```

The top-level provider:

```text
aml_module_input_set
  -> unwrap real context
  -> NNsdk_module_input_set[hardware_type]
  -> backend input setter
```

Important caveat: for a valid real context, the wrapper returns zero after
dispatch and does not propagate the backend’s native result. Output
validation remains required.

## Execute and retrieve

The stock caller builds a 56-byte zeroed output command. It sets:

```text
+0x00 typeSize = 56
+0x08 perfMode
```

Provider dispatch:

| `perfMode` | Function-table action | Return |
|---:|---|---|
| 0 | output-set, run, output-get | descriptor |
| 1 | run | null/ignored |
| 2 | output-get | descriptor |
| 3 | output-set | null |

The project deliberately uses separate mode-1 and mode-2 calls. This matches
the stock ASR lifecycle and makes execution timing distinct from output copy.

The first descriptor begins at returned object `+0x08`; its data pointer is
at descriptor `+0x48`, producing absolute returned-object offset `+0x50`.
Both the ASR and diarization code read that address.

## Live output

PoC 0 queried and then ran:

```text
input:
  name:   input/output[0]
  dtype:  FP16
  shape:  [256, 16, 1]
  bytes:  8192

output:
  name:   uid_20000_sub_uid_1_out_0
  dtype:  FP32
  shape:  [5200, 16, 1]
  bytes:  332800
```

Six native iterations across two process lifecycles produced the same output
digest:

```text
e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4
```

Those live results make the create/input/execute/retrieve/destroy path class A.
They do not make the zero-input output a model-quality test.

## Cleanup

`aml_module_destroy`:

1. validates the wrapper and backend value;
2. obtains the real context;
3. calls the backend destroy function;
4. frees the top-level wrapper only after backend success;
5. returns zero on success.

PoC 0 observed cleanup zero twice.

The project `Graph`:

- provides an explicit consuming `destroy`;
- replaces its stored pointer with null before the native call;
- has a best-effort `Drop` fallback for early Rust errors;
- never calls destroy twice for the same pointer.

## Concurrency notes

Static analysis found global state:

- a global VIPUNIFY “off/on” mode selector;
- global parsed NBG header and IO tables;
- global dynamically resolved function tables;
- a mutex around VIPUNIFY creation.

This suggests creation/parsing was designed around serialized global state.
It does not prove that already-created graph execution is globally exclusive,
but production code should initially serialize create/destroy and avoid
mixing NBG and export-data modes in one process until a concurrency PoC proves
otherwise.

## Failure edges

Create can fail at:

- config validation;
- hardware detection;
- dynamic backend function loading;
- NBG magic/version/size parsing;
- product selection;
- context creation;
- graph creation;
- IO binding;
- NBG node creation;
- tensor creation;
- graph setup;
- graph verification.

Retrieve can fail with a null descriptor or invalid descriptor content.
Destroy can fail before freeing the wrapper.

The categorized handling is in [`error-taxonomy.md`](error-taxonomy.md).

## What becomes safe after PoC 1

Safe:

- retain the current narrow project FFI;
- depend on the exact layouts in `prototypes.md`;
- parse NBG v2 headers and IO tables on the host;
- treat backend 0/VIPUNIFY as the live stock route;
- treat v3 AES/ANS as a CPU reference workload.

Not yet safe:

- execute a handcrafted NBG or export-data graph;
- patch NBG product fields;
- invoke graph-generation exports;
- run two backend modes concurrently;
- infer kernel ioctl layouts;
- change device clocks, power policy, modules, or services.
