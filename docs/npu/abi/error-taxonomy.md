# Wave NPU Error Taxonomy

The stock runtime mixes null pointers, integer status codes, log-only errors,
and process termination. This taxonomy prevents those different signals from
being collapsed into “NPU failed.”

## Layers

```text
host harness
  -> ADB transport
    -> native probe/watchdog
      -> project FFI validation
        -> libnnsdk wrapper
          -> VIPUNIFY/OpenVX
            -> GAL / galcore driver
```

An error label should identify the lowest layer actually supported by
evidence. For example, `aml_module_create returned NULL` is a create-stage
failure; it is not automatically a driver failure.

## Project native exit codes

`waved-npu-probe` assigns:

| Exit | Stage | Meaning |
|---:|---|---|
| 10 | `driver` | Required device/runtime precondition is unavailable. |
| 11 | `model-read` | Model or tensor metadata could not be read safely. |
| 12 | `checksum` | Model hash/header differs from the pinned expectation. |
| 13 | `runtime-load` | Exact system DSO could not be loaded. |
| 14 | `symbol-resolution` | Required ABI symbol could not be resolved. |
| 15 | `graph-create` | Native create returned null or graph initialization failed. |
| 16 | `input-submit` | Input construction/submission failed. |
| 17 | `graph-execute` | Execution could not be initiated/completed. |
| 18 | `output-retrieve` | Output descriptor/count/pointer/size was invalid. |
| 19 | `output-verify` | Output content violated a semantic invariant. |
| 20 | `cleanup` | Tensor metadata or graph cleanup failed. |
| 21 | watchdog timeout | Native worker exceeded its internal deadline. |
| 22 | worker disconnected | Native worker channel ended without a result. |

These are probe errors, not vendor error numbers.

## Harness classifications

The host lab wraps native outcomes in:

| Classification | Boundary |
|---|---|
| `success` | Native result, expected semantics, and post-state all pass. |
| `preflight-rejected` | Required device fact or hash differs before native execution. |
| `native-failure` | Native exit/result differs from the definition. |
| `native-timeout` | Host ADB deadline expires. |
| `adb-unavailable` | Device is unavailable before the run. |
| `adb-failure` | Transport or remote-exit sentinel fails. |
| `result-parse-failure` | Native result line is absent, duplicate, or malformed. |
| `post-snapshot-failure` | Native result passes but safety evidence is incomplete. |
| `internal-error` | Unexpected harness failure after artifact-directory creation. |

The host timeout and native watchdog are deliberately separate. Killing the
local ADB client does not prove the remote process stopped.

## `libnnsdk` top-level statuses

### Create

`aml_module_create` returns:

| Result | Meaning |
|---|---|
| non-null | A 40-byte wrapper containing a real backend context. |
| null | Config, hardware selection, backend create, graph setup, or verification failed. |

It does not expose a structured create error. Diagnostics are the only
substage evidence.

Known create diagnostics include:

- input config is null;
- hardware selection failed;
- current platform has no supported NPU;
- non-TFLite model cannot use GPU/CPU fallback;
- dynamic VSI function loading failed;
- NBG is invalid;
- NBG size is invalid;
- hardware version is not in product list;
- context creation failed;
- graph creation failed;
- graph IO binding failed;
- NBG node creation failed;
- tensor creation failed;
- graph setup failed;
- graph verification failed.

The project maps a null result to native exit 15 and preserves native output
for diagnosis.

### Input

Top-level `aml_module_input_set` returns:

| Result | Meaning |
|---:|---|
| 4 | Wrapper or input pointer is null. |
| 1 | Backend value/context could not be unwrapped. |
| 0 | Backend input function was dispatched. |

For a valid context, the wrapper discards the backend function’s status and
returns zero. Therefore:

```text
input_set == 0
```

is necessary but not sufficient proof of a valid input. Subsequent execution
and output invariants are mandatory.

### Output/execute

`aml_module_output_get` has pointer return type:

| Condition | Observed return behavior |
|---|---|
| Null wrapper | null |
| Mode 1 execute | backend run dispatched; return normally ignored/null |
| Mode 2 retrieve success | backend-owned descriptor pointer |
| Retrieve backend failure | null |
| Invalid real context/backend | one path returns integer 1 as a pointer |

The small-pointer case is a vendor ABI defect. Safe callers should reject:

- null;
- unaligned values;
- obviously small addresses;
- invalid tensor count;
- null data pointer;
- zero/overflowing byte length;
- metadata/descriptor inconsistency.

The current project performs all content checks except an explicit
small-pointer threshold. That hardening is recommended before expanding
beyond the pinned control.

### Destroy

`aml_module_destroy` returns:

| Status | Meaning |
|---:|---|
| 0 | Backend destroyed and wrapper freed. |
| 1 | Backend value invalid or real context absent. |
| 4 | Wrapper pointer null. |
| other | Propagated backend destroy status. |

If backend destroy fails, the top-level wrapper is not freed by the normal
path. Retrying blindly risks double-destroying partial backend state; record
and stop.

### Tensor metadata free

The pinned public header declares `void`. The exact firmware wrapper invokes
the backend free function and returns zero. The project checks that observed
zero. A different firmware/SDK must be re-audited before relying on the
return.

## Parser termination hazards

Several parsing errors call `exit(0)`, including:

- null NBG/export-data pointer;
- invalid NBG magic on legacy paths;
- invalid required export-data section labels.

Exit status zero is therefore not proof of success when directly invoking the
vendor parser. The lab harness also requires exactly one machine-readable
result line and expected semantic fields; an early vendor `exit(0)` becomes a
`result-parse-failure`, not success.

Future probes should isolate parser calls in a child process when processing
anything other than an exact pinned stock model.

## NBG failure stages

Use these labels in notebooks:

| Stage | Evidence signal | Typical interpretation |
|---|---|---|
| `nbg-envelope` | Bad magic, truncated header, impossible table range. | File is not safely parseable. |
| `nbg-version` | Unsupported version diagnostic. | Parser/runtime version skew. |
| `nbg-target` | Product absent from hardware list. | Compiled for different silicon or target database. |
| `nbg-io` | Input/output table or graph binding fails. | Malformed/incompatible metadata. |
| `context` | OpenVX/VSI context null or poisoned. | Userspace/driver mismatch or resource failure. |
| `graph-create` | Graph/null node/tensor failure. | ABI, memory, target, or operation incompatibility. |
| `graph-setup` | Setup failure. | Kernel selection/compilation or graph validity failure. |
| `graph-verify` | Verification failure. | Unsupported shape/op/target contract. |
| `input` | Descriptor rejected or later output invalid. | Layout, dtype, shape, size, or cache issue. |
| `execute` | Run failure/timeout. | Runtime, driver, contention, or graph fault. |
| `retrieve` | Null/invalid descriptor. | Run did not produce valid outputs or ABI mismatch. |
| `semantic` | Valid buffer but wrong/nonfinite/unstable data. | Model/input or computation correctness failure. |
| `cleanup` | Nonzero destroy or holder persists. | Resource lifecycle failure. |

Do not skip directly from a create null to `driver-crash`; the evidence must
show the lower layer.

## Known product-mismatch signature

The foreign add NBG produced:

```text
hardware version is not in product list
set_graph_inoutputs fail
aml_module_create returned NULL
```

Classification:

```text
primary:   graph-create / nbg-target
not:       input, execute, or output failure
decision:  do not patch product bytes; obtain a real Wave target
```

## Timeouts

### Native watchdog

The stock-control probe bounds worker execution to 15 seconds. On timeout it
returns exit 21 and does not reset the driver.

### Host timeout

The allowlisted host experiment bounds ADB to 20 seconds. On timeout it:

- stops only the local ADB client;
- records the timeout;
- attempts the bounded post-snapshot;
- performs no remote kill;
- performs no sysfs write;
- performs no service stop;
- performs no driver reset;
- performs no reboot.

A timeout is a manual-review stop condition. It is not permission to rerun
with a longer arbitrary timeout.

## Cleanup invariants

A successful graph experiment requires all of:

- native destroy status zero;
- one valid result line;
- expected output count and bytes;
- semantic output invariant;
- ADB responsive after the run;
- required file hashes unchanged;
- captured read-only NPU parameters unchanged;
- no new persistent holder in the bounded scan.

Absence of a holder in one scan is a point-in-time observation, not proof
that stock services never use the NPU.

## Recovery policy

Permitted automatically:

- preserve logs/envelope;
- take bounded read-only post-state;
- stop the experiment sequence;
- analyze host artifacts.

Requires explicit approval for the specific action:

- kill a remote process;
- stop a service;
- write an NPU sysfs parameter;
- unload/reload galcore;
- replace a library or model;
- reboot;
- flash firmware.

Never:

- blind `/dev/galcore` mutation fuzzing;
- product-ID patch-and-run;
- overwrite a stock graph;
- automatically reset the driver after a hang.

## Noise-suppression acceptance errors

Once a custom denoiser exists, add model-level classes:

| Class | Examples |
|---|---|
| `audio-contract` | Wrong frame length, sample rate, channel count, or state shape. |
| `numeric` | NaN/Inf, exploding recurrent state, clipping, denormals. |
| `deadline` | Missed real-time frame deadline or unbounded jitter. |
| `quality` | Worse objective/subjective score than baseline or speech damage. |
| `continuity` | Click, discontinuity, state reset artifact, dropped frame. |
| `fallback` | CPU fallback unavailable, late, or state-incompatible. |
| `resource` | Memory growth, context leak, persistent holder, thermal runaway. |

An NPU model is not production-ready merely because a single inference
returns bytes.
