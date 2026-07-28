# Recovered Wave NN SDK Prototypes and Layouts

This file documents the ABI used by the exact live-reference
`libnnsdk.so`:

```text
path:   firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libnnsdk.so
bytes:  288448
sha256: 36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe
arch:   AArch64 little-endian
```

Names below are chosen for readability. Layout, width, offset, and calling
convention are the ABI facts; decompiler-generated source types are not.

## Project-safe function surface

The current project uses:

```c
void *aml_module_create(struct wave_aml_config *config);
int aml_module_input_set(void *context, struct wave_nn_input *input);
struct wave_nn_output *aml_module_output_get(
    void *context,
    struct wave_output_config config
);
int aml_module_destroy(void *context);

struct wave_tensor_info_list *aml_util_getInputTensorInfo(
    const char *nbg_bytes
);
struct wave_tensor_info_list *aml_util_getOutputTensorInfo(
    const char *nbg_bytes
);
int aml_util_freeTensorInfo(struct wave_tensor_info_list *list);
```

Confidence:

| Function | Confidence | Basis |
|---|---|---|
| `aml_module_create` | A/B/C | Live stock graph, exact stock callers, provider decompilation, public API name. |
| `aml_module_input_set` | A/B/C | Live stock graph, six stock call sites, provider dispatch. |
| `aml_module_output_get` | A/B/C | Live execute/retrieve, stock 56-byte aggregate call sites, provider dispatch. |
| `aml_module_destroy` | A/B/C | Live cleanup status, stock destructors, provider cleanup. |
| Tensor metadata getters | A/B/C | Live metadata, exact parser wrappers, public tensor layout. |
| Tensor metadata free | A/B | Exact provider returns zero; public header declares `void`. |

The declarations intentionally describe the firmware ABI, even where the
pinned public header differs.

## Create configuration

Wave layout:

```c
struct wave_aml_config {
    int32_t type_size;          // +0x000, must be 0x8e0
    int32_t length;             // +0x004, NBG/export byte length
    const char *path;           // +0x008
    const char *data;           // +0x010
    int32_t model_type;         // +0x018
    int32_t nbg_type;           // +0x01c
    uint8_t private_fields[2240]; // +0x020 .. +0x8df
};                              // sizeof = 0x8e0 / 2272
```

Verified offsets:

| Field | Offset | Evidence |
|---|---:|---|
| `type_size` | `0x000` | Stock caller writes `0x8e0`; Rust layout test. |
| `length` | `0x004` | Stock caller combines length with `0x8e0`; memory loader reads it. |
| `path` | `0x008` | File loader and hardware selector dereference it. |
| `data` | `0x010` | Memory NBG path passes it to `init_model_ctx`. |
| `model_type` | `0x018` | Stock caller writes ONNX value 4; backend stores it. |
| `nbg_type` | `0x01c` | Provider selects modes 0–3 from this field. |
| private mode | `0x878` | Create’s exceptional-null branch checks value 1. |
| `hw_flag` | `0x8d8` | Passed by address to `aml_get_hardware_type`. |

Zero initialization matters because the provider reads private fields far
beyond the public prefix. A smaller allocation is an out-of-bounds read even
if the first six fields look correct.

Known `model_type` values inherited from the public enum:

| Value | Model family |
|---:|---|
| 0 | Caffe |
| 1 | TensorFlow |
| 2 | TensorFlow Lite |
| 3 | Darknet |
| 4 | ONNX |
| 5 | Keras |
| 6 | PyTorch |

Known `nbg_type` values from exact provider branches:

| Value | Meaning |
|---:|---|
| 0 | Compiled NBG from `path`. |
| 1 | Compiled NBG from `data`/`length`. |
| 2 | Private export-data graph from file. |
| 3 | Private export-data graph from memory. |

The current Rust probe only constructs mode 1.

## Input descriptor

Wave layout:

```c
struct wave_input_info {
    int32_t valid;              // +0x00
    int32_t int16_type;         // +0x04
    int32_t preprocess_debug;   // +0x08
    float mean[3];              // +0x0c
    float scale;                // +0x18
    int32_t input_format;       // +0x1c
};                              // sizeof = 32

struct wave_nn_input {
    int32_t type_size;          // +0x00
    int32_t input_index;        // +0x04
    int32_t size;               // +0x08
    int32_t alignment_padding;  // +0x0c
    uint8_t *input;             // +0x10
    int32_t input_type;         // +0x18
    struct wave_input_info info;// +0x1c
    uint32_t tail_padding;      // implicit, sizeof = 64
};
```

The stock ASR and diarization callers place:

- tensor index at `+0x04`;
- byte count at `+0x08`;
- buffer pointer at `+0x10`;
- input kind at `+0x18`.

The working project probe follows the stock caller and leaves `type_size`
zero. It uses input kind 3, binary raw data.

Known input kinds:

| Value | Meaning |
|---:|---|
| 0 | RGB24 raw |
| 1 | Tensor raw |
| 2 | Quantized tensor raw |
| 3 | Binary raw |
| 4 | DMA data |

## Output command aggregate

Wave layout:

```c
struct wave_output_config {
    int32_t type_size;          // +0x00, 56
    int32_t module_type;        // +0x04
    int32_t perf_mode;          // +0x08
    int32_t format;             // +0x0c
    int32_t order;              // +0x10
    uint8_t private_fields[36]; // +0x14
};                              // sizeof = 56
```

This aggregate is passed **by value** at the C API. Under AAPCS64 a
56-byte aggregate is materialized in caller memory and passed indirectly.
The Rust `extern "C"` declaration must keep the by-value type so the compiler
performs this lowering.

`perf_mode` controls the operation:

| Value | Provider behavior | Project use |
|---:|---|---|
| 0 | Set outputs, run, then retrieve. | Not used. |
| 1 | Run graph only. | `Graph::execute`. |
| 2 | Retrieve outputs. | `Graph::output` / `raw_output`. |
| 3 | Set output buffers only. | Not used. |

The provider reads `perf_mode` at `+0x08`. The stock caller zeroes the full
56 bytes, writes `type_size=56`, then writes modes 1 or 2.

## Output descriptors

Wave layout:

```c
struct wave_buffer_params {
    uint32_t num_of_dims;       // +0x00
    uint32_t sizes[4];          // +0x04
    int32_t data_format;        // +0x14
    int32_t quant_format;       // +0x18
    uint8_t quant_data[8];      // +0x1c
};                              // sizeof = 36

struct wave_out_buffer {
    uint32_t size;              // +0x00
    char name[64];              // +0x04
    uint32_t alignment_padding; // +0x44
    uint8_t *buffer;            // +0x48
    struct wave_buffer_params *params; // +0x50
    int32_t output_format;      // +0x58
    uint32_t tail_padding;      // +0x5c
};                              // sizeof = 96

struct wave_nn_output {
    int32_t type_size;          // +0x00
    uint32_t num;               // +0x04
    struct wave_out_buffer outputs[32]; // +0x08
};                              // sizeof = 3080
```

The strongest caller anchor is the first output buffer pointer:

```text
wave_nn_output + 0x08  first descriptor
descriptor      + 0x48 buffer
absolute        + 0x50
```

Independent live ASR and diarization callers read the returned data pointer
at output-object offset `0x50`.

The returned descriptor and its buffers are runtime-owned. Project code
copies or hashes the data before another output operation or graph
destruction. It does not free the descriptor.

## Tensor metadata

This portion matches the pinned public Amlogic header:

```c
struct wave_tensor_info_entry {
    uint32_t dimension_count;        // +0x00
    uint32_t dimensions[6];          // +0x04
    uint32_t data_format;            // +0x1c
    uint32_t data_type;              // +0x20
    uint32_t quantization_format;    // +0x24
    int32_t fixed_point_position;    // +0x28
    float tf_scale;                  // +0x2c
    int32_t tf_zero_point;           // +0x30
    char name[64];                   // +0x34
};                                   // sizeof = 116

struct wave_tensor_info_list {
    uint32_t valid;                  // +0x00
    uint32_t count;                  // +0x04
    struct wave_tensor_info_entry *entries; // +0x08
};                                   // sizeof = 16
```

Data formats:

| Value | Element |
|---:|---|
| 0 | FP32 |
| 1 | FP16 |
| 2 | UINT8 |
| 3 | INT8 |
| 4 | UINT16 |
| 5 | INT16 |

The current project bounds metadata counts to 1–16 and dimensions to 1–6
before dereferencing entries.

## Public-header delta

Cross-check source:
[Amlogic `nn_sdk.h` at pinned commit](https://github.com/Amlogic-NN/AML_NN_SDK/blob/96f7846bbbac4def9ccc38b649a43e77a8194435/Linux/examples/include/nn_sdk.h).

| Type/API | Public header | Wave firmware | Consequence |
|---|---|---|---|
| `aml_config` | Small public struct; `length` follows both pointers. | 2272 bytes; `length` at `+4`; private flags through `+0x8d8`. | Public struct is unsafe on Wave. |
| `input_info` | 28 bytes; `mean` immediately follows `valid`. | 32 bytes; `int16_type` and `preprocess_debug` precede `mean`. | Public nested layout shifts fields. |
| `nn_input` | Public nested layout produces a smaller object. | 64 bytes with stock caller offsets above. | Use Wave layout. |
| `aml_output_config_t` | Four 32-bit fields, 16 bytes. | 56-byte by-value aggregate. | Public declaration breaks AAPCS64 call lowering. |
| `outBuf_t` | Buffer and params precede the name. | Name precedes pointers; data pointer at `+0x48`. | Public output dereference reads the wrong address. |
| `tensor_info` | 116-byte entries and 16-byte list. | Same observed layout. | Safe when paired with exact ownership rules. |
| `aml_util_freeTensorInfo` | Declared `void`. | Exact wrapper returns zero in `x0`. | Project models the observed return for cleanup checking; do not generalize to other SDKs. |

The delta is why public headers are a cross-check, not the primary ABI.

## Context object and hardware dispatch

`aml_module_create` allocates a 40-byte wrapper:

```text
+0x00 real backend context
+0x08 hardware/backend type (32-bit value, initialized -1)
+0x10 model type/state used by timing branches
```

It calls `aml_get_hardware_type` and indexes backend function tables by the
result:

| Backend | Meaning |
|---:|---|
| 0 | VIPUNIFY, selected by `/dev/galcore`. |
| 1 | VIPLITE, selected by `/dev/vipcore`. |
| 2 | ADLA, selected by `/dev/adla0`. |
| 3 | Delegate path used for eligible GPU/CPU TFLite fallback. |

Only 0–3 are accepted. The live Wave control exercises backend 0.

## Return-value caveats

### `aml_module_create`

- returns null for null config;
- returns null when hardware selection fails;
- returns null when the backend create function fails, except one private
  `nbgType=3` mode condition;
- leaks some allocations on early error paths in the decompiled wrapper.

### `aml_module_input_set`

For a valid wrapper and real context, the top-level function calls the backend
input setter and returns zero without propagating its return value. Therefore
zero proves wrapper dispatch completed; it does not necessarily prove the
backend accepted every byte.

### `aml_module_output_get`

- returns null for a null wrapper;
- returns a backend-owned descriptor for mode 2;
- returns null for mode 1 after dispatching execution;
- one invalid-real-context path returns integer value 1 in the pointer return
  register. Callers must not dereference small/noncanonical results.

The current project validates null and descriptor content. A future hardening
change should also reject obviously invalid small pointer values.

### `aml_module_destroy`

- 0: success;
- 4: null wrapper;
- 1: invalid backend or missing real context;
- otherwise: backend destroy status.

Successful destruction frees the 40-byte wrapper.

## Rust enforcement

`waved/crates/waved-npu-probe/src/nnsdk.rs` uses `#[repr(C)]`. Unit tests
assert:

- every total size above;
- every field offset used by firmware;
- first output data pointer absolute offset `0x50`;
- tensor metadata sizes and offsets.

Run:

```bash
cargo test --manifest-path waved/Cargo.toml -p waved-npu-probe
```

These tests validate the host compiler’s AArch64-independent C layout rules
for the field set. The actual live call remains pinned to an AArch64 probe
binary and exact firmware hashes by the PoC 0 harness.

## Unsafe extensions not yet approved

Do not add FFI for these merely because symbols exist:

- private export-data model creation;
- power, frequency, profiling, or auto-suspend setters;
- external DMA buffer switching;
- ADLA calls;
- raw OpenVX node creation;
- NBG generation;
- driver ioctls.

Each needs its own caller evidence, ownership analysis, bounded probe, and
safety review.
