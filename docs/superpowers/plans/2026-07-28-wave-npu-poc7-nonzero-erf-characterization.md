# Wave NPU PoC 7: Nonzero NBG Data-Path Characterization

**Date:** 2026-07-28
**Status:** Executed once — PASS
**Authorization:** The device owner requested continuous, stepwise NPU PoCs
in this session
**Depends on:** PoC 6 executed the exact PoC 5C NBG once with zero input and
clean health gates
**Safety class:** One fixed execution of the already validated NBG; no
compiler, raw driver access, retry, or performance loop

## 1. Question

Does the generated Erf NBG move nonzero tensor data through the imported NBG
node and return the expected quantized nonlinear result?

PoC 6 established graph import, dispatch, and zero output. Zero is also the
default value of a newly allocated output, so PoC 7 deliberately uses values
whose expected result cannot be explained by an untouched output buffer.

## 2. Fixed Model and Numeric Contract

PoC 7 reuses only:

```text
artifacts/npu/runs/poc5c-erf-compile-1-20260728/candidate-1.nbg
bytes:  5840
sha256: 6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee
```

The NBG has one U8 `[4]` affine-asymmetric input and output, both with scale
`1.0` and zero point `0`. The single fixed vector is:

```text
encoded input:       [0, 1, 2, 255]
dequantized input:   [0, 1, 2, 255]
erf(input):          [0, 0.8427008, 0.9953223, approximately 1]
quantized output:    [0, 1, 1, 1]
```

The output buffer is initialized to `[165, 165, 165, 165]` before execution.
An exact `[0, 1, 1, 1]` result therefore proves both a write and nonzero
numeric behavior.

## 3. Isolation and Fixed Interface

The runtime is separate from PoC 6:

```text
/tmp/wave-npu-exec7/bin/wave-npu-execute-erf-nonzero-v1
/tmp/wave-npu-exec7/lib/libtim-vx.so
/tmp/wave-npu-exec7/model/wave-erf-openvx.nbg
```

Its library path is exactly:

```text
/tmp/wave-npu-exec7/lib:/usr/lib
```

`libCLC.so` must be absent from `/tmp/wave-npu-exec7/lib`, `/usr/lib`, and
`/lib`. The executable accepts only `--describe` and `--execute-once`.

## 4. Bounds

- one context, graph, NBG node, input tensor, and output tensor;
- one fixed tensor upload;
- one explicit `Compile`;
- one explicit `Run`;
- one tensor readback;
- one native process;
- 8-second native watchdog and 12-second host deadline;
- no retry, loop, benchmark, output file, dynamic model, arbitrary tensor,
  compiler call, raw ioctl, driver recovery, or service action.

## 5. Preflight and Postflight

Use the PoC 6 identity, hash, permission, holder, CLC-isolation, boot-ID,
parameter, process, and bounded-kernel-diagnostic gates, substituting only
the PoC 7 isolated root and executable hash.

Any nonzero exit, signal, timeout, malformed output, wrong tensor value,
holder, reboot, hash or parameter change, CLC visibility, or new kernel alert
stops the PoC without another execution.

## 6. Acceptance

PoC 7 passes only if the single native process prints:

```text
EXECUTION status=PASS mode=execute-once iterations=1 input=0,1,2,255 output=0,1,1,1 bytes=4
```

and all postflight health checks pass.

A pass proves nonzero data movement and the expected quantized Erf behavior.
It does not establish a multi-operator model, signed audio range, latency, or
noise suppression; those belong to PoC 8.

## 7. Result

```text
run:
  artifacts/npu/runs/poc7-erf-nonzero-1-20260728
manifest sha256:
  860af548e46cd198bdca10459f9f065ca206a98f03c3b52e95259d1412a07507
executor sha256:
  df354003e0bb89458d8d425718ca91db0ed344e1c7c792e1c7ea11fa2c779e58
execution attempts:
  1
native duration:
  86 ms
status:
  PASS
```

The exact result was:

```text
EXECUTION status=PASS mode=execute-once iterations=1 input=0,1,2,255 output=0,1,1,1 bytes=4
```

The output buffer began as `[165,165,165,165]`, so the observed output
cannot be an untouched allocation. The same boot ID, kernel, NPU parameters,
artifact hashes, and CLC-absence state were present before and after. No
holder, health difference, or kernel alert was observed.

The hypothesis passed: generated-NBG execution carries nonzero input data and
returns the expected quantized Erf result.
