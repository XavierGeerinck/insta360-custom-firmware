# PoC 8 — Minimal Audio Denoiser and Model-Portability Boundary

**Date:** 2026-07-28
**Status:** Complete — four clean negative experiments; no inference reached
**Evidence class:** Two compile-only and two separately reviewed one-shot
direct-graph experiments
**Purpose:** Test the smallest audio-relevant learned-model primitives after
PoCs 6/7 proved custom NBG execution

## Executive result

The Wave can execute the PoC 5C custom Erf NBG, but the tested public TIM-VX
path cannot yet turn ordinary MatrixMul or Conv2d graphs into executable
custom models:

```text
                         node selected   graph verified   NBG exported   ran
Erf LUT/user kernel           yes              yes             yes        yes
Add built-in                  yes              n/a             no         no
MatrixMul built-in            yes              no              no         no
Conv2d built-in               yes              no              no         no
```

PoC 8 deliberately used a 16-sample, three-tap smoothing filter. It is a
minimal linear denoiser reference, not a learned speech-enhancement model:

```text
weights: [0.25, 0.50, 0.25]
input:   [1,-1,1,-1,...,1,-1]
ideal output with zero/SAME boundary:
         [0.25,0,0,0,0,0,0,0,0,0,0,0,0,0,0,-0.25]

input energy:   16.0
output energy:  0.125
energy ratio:   0.0078125
attenuation:    about 21.07 dB for this alternating component
```

None of the four PoC 8 processes performed an inference. Both direct runners
stopped at `Graph::Compile()` before `Graph::Run()`. The attenuation above is
the independent expected result used by the acceptance gate, not an observed
NPU result.

## Why these two representations

A noise-suppression network usually needs one or both of:

- affine transforms, commonly represented as MatMul/GEMM;
- convolutional feature extraction and temporal filtering.

The same fixed FIR filter was expressed in two ways:

1. a constant `[16,16]` MatrixMul matrix;
2. a constant `[3,1,1,1]` Conv2d kernel with SAME padding.

This separated a model-shape issue from an operation-family issue while
keeping tensor values, expected energy, execution count, and health policy
fixed.

## PoC 8A — MatrixMul NBG export

### Graph

```text
input:    FLOAT32 [16,1]
weights:  FLOAT32 [16,16], constant
op:       BatchGemm / OpenVX matrixmul
output:   FLOAT32 [16,1]
```

The constant matrix contains the three-tap `0.25, 0.50, 0.25` filter with
zero-boundary behavior.

### Identities and result

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
```

Observed:

```text
Instance OPENVX node with kernel "matrixmul"
wave_npu_compile_audio_denoiser: size-only compile failed
```

The first `CompileToBinary(nullptr, &size)` returned false. No NBG or partial
file was created. Health remained unchanged, and no retry or compiler
substitution occurred.

## PoC 8B — MatrixMul direct graph

PoC 8B was not a retry of export. It used a new source, binary, isolated
runtime, manifest, native interface, and reviewed question: can the identical
fixed graph pass ordinary TIM-VX compile/verification and run without an NBG
export?

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
```

Observed:

```text
Instance OPENVX node with kernel "matrixmul"
wave_npu_run_audio_denoiser: graph compile failed
```

`Graph::Run()` and output readback were not reached. Preflight and postflight
had no health difference or new kernel alert.

## PoC 8C — Conv2d NBG export

### Graph

```text
input:    FLOAT32 [16,1,1,1]
weights:  FLOAT32 [3,1,1,1] = [0.25,0.50,0.25], constant
bias:     FLOAT32 [1] = [0], constant
op:       Conv2d
padding:  SAME
stride:   [1,1]
dilation: [0,0]
output:   FLOAT32 [16,1,1,1]
```

### Identities and result

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
```

Observed:

```text
Instance OPENVX node with kernel "conv2d"
wave_npu_compile_conv_audio_denoiser: size-only compile failed
```

Again, the first size-only export call returned false. No output existed, and
health remained clean.

## PoC 8D — Conv2d direct graph

PoC 8D used a fourth isolated root and a separately reviewed execution plan:

```text
run:
  artifacts/npu/runs/poc8d-conv-audio-denoiser-direct-1-20260728

manifest sha256:
  e598463b990074e5c94b7fef7f760e48078201f710ebd9c7da928b24e99cd897

runner sha256:
  bf3e4716588055ed7c774135ed15a77ad068fddab52b362c0c0c03dfb05f2c9f

execution attempts:
  1

duration:
  158 ms

remote exit:
  6
```

The exact graph and intended backend were visible:

```text
op[CONV2D]
input:   f32 [16,1,1,1]
weights: f32 [3,1,1,1], constant
bias:    f32 [1], constant
output:  f32 [16,1,1,1]
Instance OPENVX node with kernel "conv2d"
```

The next line was:

```text
wave_npu_run_conv_audio_denoiser: graph compile failed
```

The static source/ELF policy guarantees that the `Graph::Run()` call is
after the failed check, so no inference or tensor readback occurred. The
one attempt took 158 ms, did not time out, left no holder or process, and
introduced no health difference or kernel alert.

## Interpretation

### What the failures mean

The graph front end recognizes these operations and constructs OpenVX nodes,
but the exact Wave stack rejects the new graphs at verification and refuses
to serialize them through `vxGenerateNBG`.

This is a software/compiler boundary above raw driver submission. It is not
evidence that the silicon lacks matrix or convolution hardware. The stock
Wave NBG corpus contains neural ASR and speaker models built from such
operation families, and stock graphs execute.

The most likely missing piece is a compatible target compiler/program
package that converts higher-level ops and firmware-provided sources into
the exact kernel objects, commands, constants, patches, and memory plan
expected by `6.4.19:946407`.

### What the failures do not mean

- They do not prove all MatrixMul or Conv2d shapes/dtypes fail.
- They do not justify trying arbitrary shapes until one passes.
- They do not make product-field patching safe.
- They do not justify raw ioctl replay or mutation.
- They do not prove that a stock compiled convolution cannot run.
- They do not prove a noise model has executed.

The fixed graphs answered their predeclared questions. Shape and datatype
enumeration would be a new compatibility-matrix PoC with its own bounded
candidate list, not an automatic retry loop.

## Noise-model portability map

| Model component | Wave evidence | Port status |
|---|---|---|
| Precompiled target-native NBG import | PoCs 0, 6, 7 pass | Proven for pinned artifacts |
| Nonzero U8 tensor upload/readback | PoC 7 pass | Proven |
| Erf LUT/user-kernel export | PoC 5C pass | Proven narrowly |
| Add built-in export | PoC 5B fail | Blocked |
| MatrixMul built-in verification/export | PoCs 8A/8B fail | Blocked |
| Conv2d built-in verification/export | PoCs 8C/8D fail | Blocked |
| Signal framing shader compilation | PoC 5E fail at missing CLC | Blocked |
| General EVIS/CL source compilation | PoC 5F ABI failure | Blocked |
| FP16 custom generated NBG | No successful candidate | Unknown |
| Recurrent state and multi-op generated graph | Not tested | Unknown |
| Streaming audio deadline | No inference candidate | Unknown |
| Objective/subjective speech quality | No learned model ran | Unknown |

## Relevant model ladder

The next useful model should minimize compiler surface before attempting a
large network:

1. **Fixed FIR/Conv control.** This PoC defines the numerical oracle. It
   becomes executable only after a compatible compiler route exists.
2. **Tiny spectral mask MLP.** CPU STFT/iSTFT with a small NPU network
   producing one gain per frequency bin. Required primitives: affine/conv,
   activation, elementwise mask, and optionally explicit state.
3. **Small depthwise-separable convolutional mask network.** More
   convolution-centric and likely closer to the hardware's intended graph
   style, but still needs reliable Conv, activation, reshape, and elementwise
   support.
4. **RNNoise/DTLN-class recurrent model.** Useful quality/size tradeoffs, but
   recurrent state, gating, normalization, and tensor reshaping materially
   expand the operator surface.
5. **DeepFilterNet-class model.** Keep the existing CPU implementation as the
   quality reference and fallback. A full NPU port should wait until the
   compiler and compatibility matrix cover its convolution, recurrent,
   normalization, masking, state, and complex-filtering needs.

This order is about evidence risk, not a claim that the smallest network will
produce the best audio.

## Recommended architecture once compilation is unlocked

```text
PCM capture
   |
   +--> CPU framing / feature normalization
   |
   +--> NPU mask or filter-coefficient graph
   |       explicit input state
   |       explicit output state
   |
   +--> CPU mask/filter application and overlap-add
   |
   +--> deadline/quality monitor
           |
           +--> CPU DeepFilterNet or existing backend fallback
```

Keeping framing, orchestration, and fallback on the CPU avoids making the
audio thread dependent on a private graph compiler and limits the first NPU
model to dense convolutional work.

Production acceptance must include:

- a fixed audio corpus with clean references and diverse noises;
- SI-SDR/SNR improvement and speech-distortion metrics;
- a perceptual metric appropriate to the available licensing and corpus;
- real-time factor and tail-latency measurements under stock services;
- recurrent-state reset and discontinuity tests;
- hundreds or thousands of bounded frames without holder or memory growth;
- CPU fallback on any compile/run/deadline/output-validation failure;
- no service replacement, sysfs tuning, module action, or automatic NPU
  reset.

## Exact next blocker

The next high-value artifact is one of:

1. exact AArch64 `libCLC.so` from the Wave's
   `6.4.19:946407` userspace release;
2. a matching offline shader/VXC compiler package;
3. a genuine Acuity/Vivante compiler profile that emits VPMN v2 target
   `0x1000000e`;
4. an already compiled, legally redistributable tiny audio graph for the
   exact target, with known source and numerical oracle.

As of 2026-07-28, no exact package was found in the local corpus or targeted
public-repository searches. Absence from those searches is not evidence that
the package does not exist.

Any candidate must first pass relocation/version inspection and a compile-only
graph. It must not replace stock libraries or models.

## Evidence locations

```text
artifacts/npu/runs/poc8a-audio-denoiser-compile-1-20260728/
artifacts/npu/runs/poc8b-audio-denoiser-direct-1-20260728/
artifacts/npu/runs/poc8c-conv-audio-denoiser-compile-1-20260728/
artifacts/npu/runs/poc8d-conv-audio-denoiser-direct-1-20260728/
```

Tracked plans:

```text
docs/superpowers/plans/2026-07-28-wave-npu-poc8a-audio-denoiser-compile.md
docs/superpowers/plans/2026-07-28-wave-npu-poc8b-audio-denoiser-direct-execution.md
docs/superpowers/plans/2026-07-28-wave-npu-poc8c-convolutional-audio-denoiser-compile.md
docs/superpowers/plans/2026-07-28-wave-npu-poc8d-convolutional-audio-denoiser-execution.md
```

## Decision

PoC 8 closes without a running denoiser. The custom NBG execution mechanism
is real, but useful model portability is blocked at the compatible compiler
and graph-verification layer. The device remained healthy throughout.

The repository should continue to describe noise suppression as a target,
with CPU DeepFilterNet as the reference/fallback—not as an NPU feature that
has already been delivered.
