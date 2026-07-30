# Wave NPU PoC 8B: Direct Audio Denoiser Execution

**Date:** 2026-07-28
**Status:** Executed once — clean negative
**Authorization:** The device owner requested continuous, stepwise NPU and
noise-model PoCs in this session
**Depends on:** PoC 8A proved that the identical graph constructs an OpenVX
`matrixmul` node but cannot be serialized by `vxGenerateNBG`
**Safety class:** One fixed graph execution through public TIM-VX; no NBG
import/export, CLC compiler, raw driver access, retry, or benchmark

## 1. Question

Can the Wave execute the fixed 16-sample, constant-weight denoising Matmul
directly even though its OpenVX NBG exporter cannot serialize that node?

This is a new execution boundary. PoC 8A's failure does not authorize an NBG
execution because no NBG exists; PoC 8B instead uses the ordinary TIM-VX
`Compile`/`Run` path for the already inspected graph.

## 2. Identical Fixed Model

```text
input:
  FLOAT32 [16,1]
constant weights:
  FLOAT32 [16,16]
operation:
  Matmul(input, weights)
output:
  FLOAT32 [16,1]
filter:
  y[t] = 0.25*x[t-1] + 0.50*x[t] + 0.25*x[t+1]
boundary:
  zero padding
```

The single input frame is the highest-frequency alternating sequence:

```text
[1,-1,1,-1,1,-1,1,-1,1,-1,1,-1,1,-1,1,-1]
```

The exact expected output is:

```text
[0.25,0,0,0,0,0,0,0,0,0,0,0,0,0,0,-0.25]
```

All coefficients and values are exactly representable in FLOAT32. The probe
allows a maximum absolute output error of `1e-6`.

The energy contract is:

```text
input sum of squares:   16.0
output sum of squares:  0.125
energy ratio:           0.0078125
attenuation:            approximately 21.07 dB
```

This proves a useful audio property: the model strongly attenuates
sample-to-sample alternating noise. It is a linear reference denoiser, not a
production speech model.

## 3. Fixed Runtime

```text
/tmp/wave-npu-audio8b/bin/wave-npu-run-audio-denoiser-v1
/tmp/wave-npu-audio8b/lib/libtim-vx.so
```

Environment:

```text
LD_LIBRARY_PATH=/tmp/wave-npu-audio8b/lib:/usr/lib
VIVANTE_SDK_DIR=/usr
VSI_NN_LOG_LEVEL=5
```

`libCLC.so` must be absent from the isolated directory, `/usr/lib`, and
`/lib`. The quarantined `/tmp/wave-npu-compile/lib/libCLC.so` is outside the
loader path.

## 4. Fixed Native Behavior

The executable accepts only `--describe` and `--execute-once`. The execution
path:

1. validates the environment, CLC isolation, and exact Wave identity;
2. installs an 8-second watchdog and parent-death signal;
3. creates one context and graph;
4. creates one input, one constant weight, and one output tensor;
5. creates one Matmul with no transpose or adjoint flags;
6. uploads the one fixed input;
7. calls `Graph::Compile` once;
8. calls `Graph::Run` once;
9. copies the 16 outputs;
10. validates each element, the two energy values, and maximum error;
11. releases scoped graph objects;
12. prints one exact PASS line and exits.

No model file, tensor argument, coefficient argument, iteration count,
output file, async execution, `CompileToBinary`, raw ioctl, retry, recovery,
thread, or loop is exposed.

## 5. Backend and Health Gates

The host harness requires exactly one runtime selector line:

```text
Instance OPENVX node with kernel "matrixmul"
```

Preflight and postflight require the same exact serial, kernel, boot ID,
`/dev/galcore` `199:0`, NPU parameters, artifact hashes, no holders, bounded
kernel diagnostics, CLC absence, and no competing PoC 8B executor.

The native deadline is 8 seconds and the host deadline is 12 seconds. Any
failure stops without retry, reset, service action, module action, reboot, or
recovery.

## 6. Acceptance

PoC 8B passes only when its single attempt prints:

```text
AUDIO status=PASS mode=direct-graph iterations=1 input_energy=16.000000 output_energy=0.125000 max_error=0.000000
```

and:

- OpenVX `matrixmul` was selected exactly once;
- the remote exit is zero;
- device health is unchanged;
- there are no kernel alerts or lingering holders.

A pass establishes a custom, parameterized, audio-relevant model executing
through the Wave accelerator stack. It does not establish real-time
streaming, speech-quality enhancement, latency, or NBG portability.

## 7. Result

```text
run:
  artifacts/npu/runs/poc8b-audio-denoiser-direct-1-20260728
manifest sha256:
  c5b0248790fe8bf2eae769862b92b0594863bc4615e063a33b112a4b23d0a21d
runner sha256:
  7b0919b4576c5c4feb34bac71347a65765a111ad3d52ca5c2755bc266030adfc
execution attempts:
  1
duration:
  127 ms
remote exit:
  6
status:
  FAIL (clean graph-verification negative)
```

The graph again constructed the intended tensors and selected:

```text
Instance OPENVX node with kernel "matrixmul"
```

`Graph::Compile` then returned false, before `Graph::Run` or output readback.
The single attempt therefore performed no inference. The same matrixmul
boundary affects both paths:

```text
TIM-VX setup / OpenVX node creation: PASS
OpenVX graph verification:          FAIL
OpenVX NBG serialization:           FAIL (PoC 8A)
inference:                           NOT REACHED
```

All CLC paths remained absent, and postflight found the same boot ID, NPU
parameters, required hashes, no holders, and no kernel alerts. No retry was
made.

## 8. Conclusion

The hypothesis failed. `vxBatchGemmNode` can be instantiated by the Wave
stack for this graph but does not pass graph verification. A further model
attempt must use a different stock OpenVX primitive, not a changed Matmul
shape disguised as a retry. Convolution is the next justified candidate:
the Wave ASR corpus relies heavily on convolutional networks, TIM-VX exposes
`vxConvolutionLayer`, and convolution directly expresses the same three-tap
audio filter.
