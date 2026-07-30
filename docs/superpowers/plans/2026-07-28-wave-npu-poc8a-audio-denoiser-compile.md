# Wave NPU PoC 8A: Minimal Audio Denoiser Compile

**Date:** 2026-07-28
**Status:** Executed once — clean negative
**Authorization:** The device owner requested continuous, stepwise NPU and
noise-model PoCs in this session
**Depends on:** PoC 5C proved exact-target NBG generation; PoC 6 and PoC 7
proved zero and nonzero NBG execution
**Safety class:** One fixed graph compilation through TIM-VX; no graph
execution, incompatible CLC compiler, raw driver access, or retry

## 1. Question

Can the Wave compile a useful signal-processing model, with embedded
parameters and one dynamic audio-frame input, into a target-valid NBG using
the stock OpenVX matrix-multiply backend?

This PoC compiles but does not execute the model.

## 2. Fixed Model

The model is a 16-sample linear denoising layer:

```text
input:
  FLOAT32 [16,1]
constant weights:
  FLOAT32 [16,16]
operation:
  Matmul(input, weights)
output:
  FLOAT32 [16,1]
```

The coefficient matrix implements the zero-padded three-tap filter:

```text
y[t] = 0.25*x[t-1] + 0.50*x[t] + 0.25*x[t+1]
```

for the 16-sample frame. Missing neighbors at the two boundaries contribute
zero. This is a deterministic linear denoiser: it preserves DC in the
interior and suppresses alternating sample-to-sample energy. The 256
coefficients are compiled as a constant tensor, leaving one runtime input and
one runtime output.

This is intentionally not presented as a production neural noise suppressor.
It is the smallest audio-relevant parameterized model that establishes the
model-loading, constant-weight, matrix arithmetic, and audio tensor path
needed by later learned spectral-mask or time-domain networks.

## 3. Why Matmul

The public TIM-VX source matching the Wave runtime registers `matrixmul` with
the recovered VX-first selector:

```text
SP 4 -> VX 3 -> EVIS 2 -> CL 1 -> CPU 0
```

PoC 5C proved that this VX-first path can produce a valid NBG without a
shader compiler. PoC 8A requires the live selector log to report exactly:

```text
Instance OPENVX node with kernel "matrixmul"
```

A different backend or missing selector line fails the PoC.

## 4. CLC Quarantine and Runtime Root

PoC 5F established that the available 6.4.18 `libCLC.so` relocates against the
Wave libraries but is compiler/program-object ABI-incompatible with the
Wave's 6.4.19 stack. It remains quarantined and must not be loaded or called.

PoC 8A therefore uses a new root:

```text
/tmp/wave-npu-compile8/bin/wave-npu-compile-audio-denoiser-v1
/tmp/wave-npu-compile8/lib/libtim-vx.so
/tmp/wave-npu-compile8/sdk/include/CL/cl_viv_vx_ext.h
/tmp/wave-npu-compile8/output/wave-audio-denoiser-candidate-1.nbg
```

The exact environment is:

```text
LD_LIBRARY_PATH=/tmp/wave-npu-compile8/lib:/usr/lib
VIVANTE_SDK_DIR=/tmp/wave-npu-compile8/sdk
VSI_NN_LOG_LEVEL=5
```

`libCLC.so` must be absent from the isolated library directory, `/usr/lib`,
and `/lib`. `/tmp/wave-npu-compile` is not on the loader path.

## 5. Fixed Compiler Behavior

The executable accepts only `--describe` and `--candidate-1`. It:

1. validates the exact Wave identity;
2. creates one input, one constant weight, and one output tensor;
3. creates exactly one `tim::vx::ops::Matmul`;
4. makes the two standard size/data `CompileToBinary` calls;
5. verifies VPMN version `0x00020000` and target `0x1000000e`;
6. writes one new mode-`0444` output atomically and without replacement;
7. exits.

There is no `Compile`, `Run`, tensor upload, result readback, model file,
operation selector, arbitrary shape, second candidate, loop, raw ioctl, or
driver recovery path.

## 6. Bounds and Health Gates

- one native compile process;
- 15-second native watchdog;
- 20-second host deadline;
- no retry after any result;
- exact device, kernel, NPU parameters, device node, executable, TIM-VX,
  header, and stock DSO hashes;
- no `/dev/galcore` holder before or after;
- no existing output or partial output;
- unchanged boot ID, parameters, and hashes;
- no new kernel warning, error, fault, timeout, hang, recovery, exception, or
  MMU diagnostic.

## 7. Acceptance

PoC 8A passes only if:

- the static source/ELF policy passes;
- the isolated CLC absence gate passes;
- the selector is exactly OpenVX `matrixmul`;
- the native process emits a target-valid NBG;
- the strict parser reports one FLOAT32 `[16,1]` input, one FLOAT32 `[16,1]`
  output, and no unexpected runtime weight input;
- every NBG section checksum validates;
- device health remains unchanged.

Only after those facts and the exact NBG SHA-256 are recorded may a separate
PoC 8B execution plan be written. A failure is documented without retry or
CLC substitution.

## 8. Result

PoC 8A ran once:

```text
run:
  artifacts/npu/runs/poc8a-audio-denoiser-compile-1-20260728
manifest sha256:
  25054a2dd78773055420dd65d49c334f0910fbcc932af344916399ff8c12b042
compiler sha256:
  e8704e8cf05afbda70c7cad0c89046cb197966aa106989dc98e733eed8f642ae
duration:
  74 ms
remote exit:
  5
status:
  FAIL (clean compile-only negative)
```

The exact graph was accepted and the required backend was selected:

```text
op[MATRIXMUL]
input:     f32 [16,1]
constant:  f32 [16,16]
output:    f32 [16,1]
Instance OPENVX node with kernel "matrixmul"
```

`CompileToBinary(nullptr, &size)` then returned false. No output or partial
NBG was created. This matches the earlier Add result: OpenVX node creation
does not imply that the Wave's `vxGenerateNBG` exporter can serialize that
built-in. Erf remains the only confirmed exportable custom graph.

The isolated CLC paths remained absent. The boot ID, NPU parameters, required
hashes, and device state were unchanged; there was no holder or kernel alert.
No retry or compiler substitution occurred.

## 9. Conclusion and Next Boundary

The NBG-export hypothesis failed, but the result narrows the boundary:

```text
OpenVX matrixmul node construction: PASS
OpenVX matrixmul NBG serialization: FAIL
device health:                     PASS
```

PoC 8B may not execute an NBG because none exists. A separately reviewed
fallback may execute the identical fixed Matmul graph directly through
TIM-VX `Compile`/`Run`, which tests the live graph path without claiming
portable NBG export.
