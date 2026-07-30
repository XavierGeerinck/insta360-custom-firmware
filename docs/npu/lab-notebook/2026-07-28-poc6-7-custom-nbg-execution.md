# PoCs 6 and 7 — First Custom Wave NBG Execution

**Date:** 2026-07-28
**Status:** Complete — zero and nonzero executions passed
**Evidence class:** Two separately reviewed, exactly-once live executions
**Input artifact:** PoC 5C Erf NBG
**Device:** `PSP_0000000000000000`

## Executive result

A graph defined by repository-controlled TIM-VX source, compiled on the Wave
by the exact userspace stack, and never byte-patched was imported and
executed successfully:

```text
custom TIM-VX Erf source
        |
        v
Wave-generated VPMN v2 NBG
        |
        v
TIM-VX NBG import
        |
        v
OpenVX / GAL / galcore
        |
        v
Wave NPU result readback
```

PoC 6 established the lifecycle with an all-zero control. PoC 7 used nonzero
input and a poisoned output buffer to prove that data was uploaded,
processed, and copied back rather than merely leaving an allocation
unchanged.

## Fixed model artifact

```text
path:
  artifacts/npu/runs/poc5c-erf-compile-1-20260728/candidate-1.nbg

sha256:
  6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee

bytes:
  5840

format:
  VPMN v2

target:
  0x1000000e

graph:
  one U8 [4] input
  one Erf operation
  one U8 [4] output
  affine-asymmetric scale 1, zero point 0
```

The strict host parser validated all three stored section checksums and
reported one operation, one layer, one core, and 43,520 bytes of serialized
VIP SRAM requirement.

## Containment shared by both PoCs

Each PoC used a different isolated runtime root and a different runner
binary. Both runners:

- accept only `--describe` or `--execute-once`;
- pin the NBG location, tensor shape, type, values, and expected result;
- query and validate the Wave chip/customer identity;
- contain one NBG operation, one graph compile, one graph run, one input
  upload, and one output readback;
- install an 8-second `SIGALRM` watchdog;
- request `SIGKILL` on parent death;
- run under a 12-second host ADB deadline;
- reject loader-interposition and simulator variables;
- require `libCLC.so` to be absent from the loader path;
- expose no arbitrary model, operation, tensor, input, iteration, benchmark,
  retry, reset, or recovery option.

Preflight and postflight pinned the device, kernel, boot ID, `/dev/galcore`
identity, NPU parameters, executable/NBG/runtime/system-library hashes,
staged modes, holder set, and bounded kernel diagnostics.

## PoC 6 — zero-input lifecycle

### Fixed input and output

```text
input:     [0,0,0,0]
expected:  [0,0,0,0]
```

### Identities

```text
run:
  artifacts/npu/runs/poc6-erf-execution-1-20260728

manifest sha256:
  9de7357b327d624eac302a6bd0c232d587cf07a9b913f26dfdc9d4209ec45377

runner sha256:
  ed6fa998e20e51cb785b928d1a2bb398f39770a7c95190a857627bee5d16e630

runtime:
  /tmp/wave-npu-exec
```

### Result

```text
execution attempts:   1
host-observed native duration: 127 ms
remote exit:          0
timeout:              no
health differences:   none
kernel alerts:        none
status:               PASS
```

Native result:

```text
IDENTITY chip_model=0x00009000 chip_revision=0x00009002 chip_product_id=0x05090001 chip_eco_id=0x04000000 customer_id=0x1000000e reserved=0x00000000 flags=0x0000000000000000
EXECUTION status=PASS mode=execute-once iterations=1 input=0,0,0,0 output=0,0,0,0 bytes=4
```

TIM-VX reported one NBG node with the fixed input and output metadata. The
control proved create/compile/run/readback/cleanup for this exact generated
artifact, but zero output alone was not enough to prove useful arithmetic.

## PoC 7 — nonzero characterization

### Fixed input, poisoned output, and expected result

```text
input:                  [0,1,2,255]
output before readback: [165,165,165,165]
expected output:        [0,1,1,1]
```

With scale 1 and zero point 0, the U8 output quantization rounds or saturates
the mathematical Erf output into this small integer range. This is a test of
the exact compiled graph, not a recommendation to represent audio or a full
activation pipeline as U8 scale 1.

### Identities

```text
run:
  artifacts/npu/runs/poc7-erf-nonzero-1-20260728

manifest sha256:
  860af548e46cd198bdca10459f9f065ca206a98f03c3b52e95259d1412a07507

runner sha256:
  df354003e0bb89458d8d425718ca91db0ed344e1c7c792e1c7ea11fa2c779e58

runtime:
  /tmp/wave-npu-exec7
```

### Result

```text
execution attempts:   1
host-observed native duration: 86 ms
remote exit:          0
timeout:              no
health differences:   none
kernel alerts:        none
status:               PASS
```

Native result:

```text
EXECUTION status=PASS mode=execute-once iterations=1 input=0,1,2,255 output=0,1,1,1 bytes=4
```

Because all four output bytes differed from the initial `0xa5` fill, the
result is not explained by an untouched output allocation. It also differs
by input element, so it is not a constant all-zero success.

## What is now proven

### Observed live

- A repository-defined graph can be compiled to an exact Wave-targeted NBG.
- That NBG can be imported by repository-controlled code.
- One graph compile and one graph execution complete normally.
- Nonzero host input reaches the graph.
- Four output bytes are written back with the expected quantized Erf values.
- Context, graph, tensor, and runtime cleanup leave no observed holder.
- The device remains responsive with unchanged captured health state.

### Static and parser-backed

- The executed bytes are exactly the PoC 5C artifact.
- The artifact is VPMN v2 for target `0x1000000e`.
- Its tensor metadata matches the runners.
- All stored checksums validated before execution.
- The runners do not import raw ioctl, loader injection, export, retry, or
  unreviewed operation interfaces.

## What remains unproven

- General ONNX or TFLite model import.
- General TIM-VX built-in graph verification or NBG export.
- EVIS/OpenCL custom-source compilation.
- Signed or floating-point custom NBG execution.
- More than one operation in a generated graph.
- Streaming, concurrency, latency distribution, sustained load, power, or
  thermals.
- Learned speech enhancement or subjective noise-cancellation quality.
- A production-safe scheduler or automatic fallback.

The 86 ms and 127 ms values cover host-observed native process work and
logging. They are descriptive one-shot durations, not accelerator kernel
benchmarks.

## Reproduction boundary

The exact build and stage tools are:

```text
waved/tools/npu-compiler/build-device-erf-executor.sh
waved/tools/npu-compiler/stage-device-erf-executor.sh
waved/tools/npu-compiler/build-device-erf-nonzero-executor.sh
waved/tools/npu-compiler/stage-device-erf-nonzero-executor.sh
```

The fixed host entry points are:

```text
waved/scripts/npu/npu_erf_execution_lab.py
waved/scripts/npu/npu_erf_nonzero_execution_lab.py
```

The run command refuses an existing evidence directory. Repeating a live
execution is not a unit test and must receive a new reviewed run ID and
purpose. Host-only tests may be run freely.

## Decision

The custom-model question has moved from “can our process access the NPU?” to
“which compilation paths and operation families can produce a graph the
exact Wave stack accepts?” PoC 8 therefore tested an audio-relevant linear
denoiser without assuming that Erf generalizes to MatrixMul or Conv2d.
