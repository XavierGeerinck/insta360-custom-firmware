# Wave NPU Inference Proof of Concept

**Date:** 2026-07-23

**Status:** Approved for implementation

## Objective

Prove that a process we control can load a compiled neural graph, submit
input tensors to the Insta360 Wave's VeriSilicon NPU through the stock
userspace runtime, execute the graph, and retrieve the expected output.

The proof must go beyond opening `/dev/galcore` or creating an OpenVX
context. Success requires a deterministic inference result.

## Current Device State

The connected Wave is reachable through root ADB. The following required
surfaces are present:

- `/dev/galcore`
- `/usr/lib/libnnsdk.so`
- `/usr/lib/libovxlib.so`
- `/usr/lib/libOpenVX.so`
- `/data/waved/waved-npu-probe`
- `/data/waved/lib/add_graph.nb`

The deployed `add_graph.nb` is byte-identical to
`waved/third_party/stock-npu-stack/sample-add.nb`:

```text
sha256 9bef0c0f2271ad534cb975b386be8428e973dc7ee4c71c04854c95eeac1fea5c
```

The graph accepts two `UINT8[2]` tensors, adds them element by element,
and returns one `UINT8[2]` tensor.

## Approaches Considered

### 1. Stock `libnnsdk.so` runtime — selected

Extend `waved-npu-probe` to call the exact NPU runtime shipped with the
device. Load `add_graph.nb`, submit `[1, 2]` and `[3, 4]`, execute the
graph, and verify `[4, 6]`.

This is the shortest path that uses the device-matched runtime and
kernel driver while producing an independently checkable result.

### 2. TIM-VX NBG runner

Use the existing cross-built `libtim-vx.so` and `tim_vx_run_nbg`.
Previous experiments encountered runtime ABI and NBG parser
incompatibilities. This remains a useful diagnostic fallback but is not
the primary path.

### 3. Production CAM++ or ASR graph

Load one of the stock diarization or ASR graphs. This would prove a
production workload, but it requires model-specific tensor shapes,
feature extraction, and multi-graph orchestration. It is deliberately
deferred until the minimal graph path works.

## Architecture

The existing `waved-npu-probe` binary gains a `--run-add` mode. The
default proof-of-life behavior remains unchanged.

```text
waved-npu-probe --run-add
    |
    +-- validate /dev/galcore and model checksum
    +-- load /usr/lib/libnnsdk.so
    +-- create graph from add_graph.nb
    +-- query or validate tensor metadata
    +-- set two UINT8[2] inputs
    +-- execute graph
    +-- retrieve one UINT8[2] output
    +-- verify [4, 6]
    +-- repeat and report latency
    +-- destroy graph and release runtime resources
```

## Components

### Runtime symbol wrapper

A small isolated wrapper owns `libnnsdk.so` and the graph handle. It
resolves the direct exported runtime functions required for create,
input, run, output, and destroy.

Before any call is implemented, each function signature must be
confirmed from the device-matched binary in Ghidra and cross-checked
against compatible SDK headers when available. The implementation must
not call a guessed ABI.

The wrapper uses RAII cleanup so a successfully created graph is
destroyed on every return path.

### Add-graph runner

The runner:

1. Validates the model checksum.
2. Loads the compiled graph through the stock runtime.
3. Confirms the expected input/output count and tensor sizes when the
   runtime exposes that metadata.
4. Supplies `[1, 2]` and `[3, 4]`.
5. Executes ten measured iterations.
6. Reads and verifies `[4, 6]` after every iteration.

### Diagnostics

The probe prints one concise line per boundary:

- galcore access
- runtime load
- symbol resolution
- model creation
- tensor metadata
- input submission
- graph execution
- output retrieval
- cleanup

It also records the current NPU clock, enable mask, hardware status when
available, per-iteration latency, and the final output.

## Safety

The PoC is read-only with respect to firmware and persistent device
configuration.

It must not:

- flash or replace firmware;
- change the NPU clock or any sysfs parameter;
- stop `waved`, `pspapp`, or another service automatically;
- overwrite a stock model;
- modify boot scripts or persistent configuration;
- reboot the device.

The only device write permitted is replacing the disposable probe
binary under `/data/waved/` if a rebuilt binary must be deployed. The
existing `add_graph.nb` is reused after its checksum is validated.

If `/dev/galcore` is busy or the runtime reports a conflict, the probe
stops and reports the owning process where discoverable. It does not
kill that process.

## Error Handling

Each runtime boundary maps to a distinct nonzero exit code. Errors must
include the operation name and native status code.

If graph creation succeeds, graph destruction is attempted even when a
later operation fails. Output buffers are initialized to a sentinel
value so a missing write cannot accidentally pass verification.

A timeout guards graph execution. A timeout is reported as a failure;
the probe does not reset the driver or alter recovery parameters.

## Verification

### Host checks

- Existing `waved-npu-probe` behavior remains covered.
- CLI parsing distinguishes the default context probe from
  `--run-add`.
- Tensor/output verification is unit tested independently of device
  FFI.
- The aarch64 release binary cross-compiles successfully.

### Device checks

1. Run the existing context proof.
2. Run `--run-add` once with full diagnostics.
3. Run ten inference iterations in the same process.
4. Run the complete command a second time to prove cleanup and
   repeatability.
5. Confirm that the device remains responsive and that the normal
   `waved` process is still running.

## Acceptance Criteria

The PoC passes only when all of the following are true:

- `/dev/galcore` opens successfully.
- The device-matched `libnnsdk.so` loads.
- The compiled add graph is accepted by the runtime.
- Both inputs are submitted successfully.
- Ten graph executions complete without timeout or native error.
- Every retrieved output is exactly `[4, 6]`.
- Graph/runtime cleanup succeeds.
- A second complete invocation also passes.
- No firmware, clock, sysfs, service, or boot configuration is changed.

Creating an OpenVX context without executing the graph is useful
diagnostic evidence but does not satisfy acceptance.

## Follow-up

Once this PoC passes, the next independent design can target arbitrary
models: first establish a reproducible ONNX-to-NBG compiler path, then
run a small custom audio classifier before attempting CAM++ or ASR.
