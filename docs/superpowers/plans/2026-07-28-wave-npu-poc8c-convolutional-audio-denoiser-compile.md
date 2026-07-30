# Wave NPU PoC 8C: Convolutional Audio Denoiser Compile

**Date:** 2026-07-28
**Status:** Executed once — clean negative
**Authorization:** The device owner requested continuous, stepwise NPU and
noise-model PoCs in this session
**Depends on:** PoC 8A/8B established that OpenVX BatchGemm construction
succeeds but graph verification and NBG export fail
**Safety class:** One fixed Conv2d graph compilation; no graph execution,
CLC compiler, raw driver access, retry, or recovery

## 1. Question

Can the Wave's stock `vxConvolutionLayer` path compile the same three-tap
audio denoiser into a valid NBG?

This is not a Matmul retry. It uses the native convolution primitive that
underpins the convolution-heavy NPU model corpus and directly represents the
filter in its intended form.

## 2. Fixed Graph

TIM-VX uses WHCN tensor layout and WHIcOc kernel layout:

```text
input:
  FLOAT32 [16,1,1,1]
constant weights:
  FLOAT32 [3,1,1,1] = [0.25,0.50,0.25]
constant bias:
  FLOAT32 [1] = [0.0]
operation:
  Conv2d
padding:
  SAME
stride:
  [1,1]
dilation:
  [0,0] (TIM-VX representation of no dilation)
output:
  FLOAT32 [16,1,1,1]
```

The graph implements:

```text
y[t] = 0.25*x[t-1] + 0.50*x[t] + 0.25*x[t+1]
```

with zero padding at the frame boundaries. Weights and bias are compile-time
constants, so the exported graph must have one runtime input and one runtime
output.

## 3. Backend Rationale

The matching TIM-VX source maps `conv2d` to the stock OpenVX
`vxConvolutionLayer`. Unlike EVIS/CL source shaders, this path does not
require `libCLC.so`. The harness requires exactly:

```text
Instance OPENVX node with kernel "conv2d"
```

No EVIS, CL, CPU, SP, or ambiguous selection is accepted.

## 4. Isolated Compile Root

```text
/tmp/wave-npu-compile8c/bin/wave-npu-compile-conv-audio-denoiser-v1
/tmp/wave-npu-compile8c/lib/libtim-vx.so
/tmp/wave-npu-compile8c/sdk/include/CL/cl_viv_vx_ext.h
/tmp/wave-npu-compile8c/output/wave-conv-audio-denoiser-candidate-1.nbg
```

Environment:

```text
LD_LIBRARY_PATH=/tmp/wave-npu-compile8c/lib:/usr/lib
VIVANTE_SDK_DIR=/tmp/wave-npu-compile8c/sdk
VSI_NN_LOG_LEVEL=5
```

The incompatible PoC 5F CLC is outside this path. `libCLC.so` must also be
absent from the isolated directory, `/usr/lib`, and `/lib`.

## 5. Fixed Compiler and Bounds

The executable accepts only `--describe` and `--candidate-1`. It contains
one Conv2d, three constant coefficients, one zero bias, and exactly two
`CompileToBinary` calls. It never calls `Compile`, `Run`, or a tensor-copy
method.

Bounds:

- one native process;
- one graph and one output candidate;
- 15-second native watchdog;
- 20-second host deadline;
- no retry or determinism candidate;
- non-overwriting mode-`0444` output;
- no raw ioctl, device-node open, service action, module action, reboot, or
  recovery action.

Preflight and postflight apply the exact device, kernel, boot-ID, NPU
parameter, DSO hash, CLC-absence, holder, process, output-absence, and kernel
diagnostic gates used by PoC 8A.

## 6. Acceptance

PoC 8C passes only if:

- static source and ELF policy checks pass;
- OpenVX `conv2d` is selected exactly once;
- a version-2 target-`0x1000000e` NBG is emitted;
- the strict parser finds one FP32 `[16,1,1,1]` input, one matching output,
  and valid section checksums;
- no constant weight or bias appears as a dynamic runtime input;
- device health remains unchanged.

Only a passing, hash-recorded NBG may receive a separate PoC 8D execution
plan. If graph export fails, stop and document it without direct execution or
another datatype/shape attempt.

## 7. Result

```text
run:
  artifacts/npu/runs/poc8c-conv-audio-denoiser-compile-1-20260728
manifest sha256:
  0dd893389e24abf3e05037390f217495d863ff2188e1bbda2e82338ede3574c9
compiler sha256:
  7ee79cb52426e9fe8658ea2636d652f0df44d2c9f9c89f9a59a28fb99643dca9
duration:
  80 ms
remote exit:
  5
status:
  FAIL (clean compile-only negative)
```

The intended graph and backend were accepted:

```text
input:     f32 [16,1,1,1]
weights:   f32 [3,1,1,1], constant
bias:      f32 [1], constant
output:    f32 [16,1,1,1]
selector:  OPENVX conv2d
```

The first size-only `CompileToBinary` call returned false. No NBG or partial
file was created, and no execution occurred. Postflight was clean: unchanged
boot ID, parameters, hashes, and CLC absence; no holder or kernel alert.

## 8. Conclusion

The NBG export hypothesis failed. Together with Add and MatrixMul, this shows
that the exposed `vxGenerateNBG` path is not a general serializer for stock
OpenVX built-ins:

```text
Erf LUT user kernel export:   PASS
Add built-in export:          FAIL
BatchGemm built-in export:    FAIL
Convolution built-in export:  FAIL
```

PoC 8C stops here as planned. A later, separately bounded experiment may
evaluate ordinary Conv2d graph verification and execution, but it must not be
described as NBG export or as a continuation/retry of this compile process.
