# Wave NPU Learned Erf Noise-Suppression Design

**Date:** 2026-07-29

**Status:** Research PoCs 13A–13F implemented and qualified; production Rust
integration remains incomplete

**Parent designs:**

- `2026-07-28-wave-npu-reverse-engineering-program-design.md`
- `2026-07-28-wave-npu-compiler-profile-onnx-hybrid-design.md`
- `2026-07-28-wave-npu-open-nbg-serializer-design.md`

**Immediate outcome:** Achieved. A learned spectral-mask denoiser ran its
quantized Erf partition numerically on the Insta360 Wave NPU, reconstructed
an enhanced waveform from those NPU results, and retained an equivalent CPU
fallback.

**Honest result name:** `learned-npu-assisted-erf-mask-denoiser`

## Implementation Result Addendum

The executed PoCs changed two implementation details without weakening the
NPU-residency claim:

- the final quality-qualified architecture evaluates both a DeepFilterNet
  candidate and the learned Erf candidate, then uses a causal per-frame
  selector; the NPU-derived Erf candidate was selected for 803 of 1,201
  fixture frames;
- the offline overlap-add flush adds one final partition input, so PoC 13E
  executed 1,201 frames rather than the design estimate of 1,200.

PoC 13E reconstructed all 576,000 samples of the 12-second fixture from
Wave-returned tensors. All 577,681 NPU output bytes and all reconstructed
floating-point samples exactly matched the independent CPU path. The
waveform passed the noise, speech, intelligibility, silence, and numerical
gates. PoC 13F then completed 10,000 consecutive NPU executions with all ten
host-oracle digest checkpoints matching and no device-health change.

The current runtime boundary is Python/lab code plus fixed offline TIM-VX
device runners. It proves working NPU-assisted noise cancellation, but it is
not yet the production Rust `NpuInfer` backend described in sections 3.5,
8.4, and 11.6. Production modes, service scheduling, percentile latency,
memory, reset, and thermal qualification remain follow-on work. The
requirement-by-requirement status is recorded in
`docs/npu/models/erf-mask/completion-audit.md`.

## 1. Decision

The first real noise-suppression model will be a deliberately small hybrid
model built around the only custom operation already proven on the exact Wave
stack: quantized Erf.

The model is:

```text
mono 48 kHz audio
  -> CPU causal STFT and noise-power estimate
  -> CPU learned per-frequency affine transform
  -> quantized UINT8 logits
  -> Wave NPU Erf over all 481 frequency bins
  -> CPU learned-candidate mask scaling and application
  -> CPU DeepFilterNet candidate
  -> CPU causal DFN-or-Erf selector
  -> CPU inverse STFT
  -> enhanced mono audio
```

This is not the old fixed three-tap filter and it is not an analytic hard
gate. It has trained parameters, held-out quality evaluation, a canonical
model package, an independently evaluated CPU implementation, and an NPU
partition whose returned values directly alter the reconstructed waveform.

It is also not represented as a fully NPU-resident convolutional network.
Feature extraction, the learned affine transform, and synthesis remain on
the CPU. The NPU executes the quantized nonlinear mask partition. A later
serializer-first phase will expand NPU residency to Conv and other learned
operations without delaying this first numerical denoiser.

## 2. Why This Design Exists

### 2.1 Verified capabilities

The current evidence establishes all of the following:

- the Wave accepts VPMN v2 NBG files for product `0x1000000e`;
- exact Wave libraries generated and executed an external-input UINT8 Erf
  graph;
- the same producer generated deterministic constant-input Erf graphs;
- PoC 12C recovered the VPMN v2 file CRC and conditional
  section-20/section-26 DYNAIO mapping across the Wave v2 corpus; the
  external-input Erf graph subsequently proved that not every VPMN v2 file
  contains those records;
- a CPU DeepFilterNet implementation already exists behind `NpuInfer`;
- the ONNX control plane already implements audit, CPU, hybrid, and strict
  policies, but currently has zero approved NPU signatures.

### 2.2 Verified constraints

The current evidence also establishes:

- ordinary MatrixMul and Conv2d graph construction reaches OpenVX operator
  selection but fails graph verification and NBG export;
- the exact firmware does not contain the matching `libCLC.so`;
- the available adjacent `6.4.18` CLC library is ABI-incompatible,
  crash-producing, quarantined, and must not be invoked again;
- no compatible free no-kernel SDK has been recovered;
- the current hybrid ONNX implementation makes zero NPU attempts and cannot
  yet support an NPU claim;
- the PoC 8 three-tap filter never executed and was not a learned model.

The design therefore uses the narrow proven compiler/runtime route without
pretending that the general Conv compiler problem is solved.

## 3. Goals

### 3.1 Functional goal

Produce an enhanced waveform for a frozen noisy-speech fixture where every
spectral mask used in reconstruction came from a successful Wave NPU Erf
execution.

### 3.2 Learning goal

Train and freeze frequency-dependent affine parameters against clean/noise
references. The trained parameters must outperform their deterministic
initialization and an all-ones identity mask on held-out mixtures.

### 3.3 Numerical goal

For every qualification vector, compare all 481 NPU output elements with an
independent quantized-Erf CPU oracle. Integer results must be exact or differ
by no more than one output quantization unit.

### 3.4 Hybrid-runtime goal

Provide:

- strict mode, which fails unless the sealed NBG partition runs on the NPU;
- hybrid mode, which uses the NPU when qualified and falls back to the same
  quantized model on the CPU after one failure;
- CPU mode, which never opens the NPU runtime;
- audit mode, which produces placement and artifact evidence without
  inference.

### 3.5 Integration goal

Implement the model behind the existing `NpuInfer` abstraction at 48 kHz and
480 samples per call. The implementation must remain opt-in until numerical,
quality, latency, and soak gates pass.

### 3.6 Documentation goal

Every host and device stage must produce a lab notebook, a machine-readable
result, exact artifact hashes, reproduction commands, explicit non-claims,
and the next evidence gate.

## 4. Non-Goals

This phase does not:

- claim a complete Acuity or VeriSilicon compiler;
- claim general NPU support for arbitrary ONNX;
- claim that Conv, MatrixMul, FFT, recurrent, or complex-valued operators run
  on the NPU;
- claim that CPU feature extraction or synthesis is NPU-resident;
- replace the existing DeepFilterNet backend;
- beat DeepFilterNet as a production-quality benchmark;
- use clean reference audio or oracle noise at inference time;
- train on private recordings already present in the workspace;
- record or retain live microphone audio without separate user permission;
- flash firmware, replace stock libraries, alter boot configuration, reload
  kernel modules, write sysfs controls, replay raw ioctls, or reset the NPU;
- invoke the quarantined adjacent `libCLC.so`;
- execute a graph merely because its target ID or header parses;
- approve the general VSINPU compiler path from one sealed Erf graph.

## 5. Approaches Considered

### 5.1 Learned hybrid Erf spectral mask — selected

Train a frequency-dependent affine mask estimator on the CPU and execute its
elementwise Erf partition using one sealed Wave-targeted NBG.

Advantages:

- uses an operation already proven to compile and execute;
- yields a real learned denoiser rather than another compiler-only control;
- has a small, independently reproducible CPU oracle;
- isolates NPU numerical correctness from STFT and training correctness;
- integrates naturally with the existing hybrid and `NpuInfer` boundaries;
- creates meaningful waveform evidence before Conv serialization is solved.

Limitations:

- only the Erf partition is NPU-resident;
- the model is intentionally small and is not expected to match
  DeepFilterNet;
- one NPU dispatch per 10 ms frame may be dispatch-bound.

### 5.2 Serializer-first Conv/Sigmoid model — retained as the next expansion

Continue recovering coefficient packing, operation records, command/state
records, relocation behavior, and load-time patching until a fixed
Conv/Sigmoid spectral-mask graph can be serialized and run.

Advantages:

- moves materially more learned computation onto the NPU;
- is a better foundation for larger custom models;
- can reuse the ONNX capability and partitioning control plane.

Limitations:

- PoC 12C resolved DYNAIO and the file CRC, not the executable command stream
  or general coefficient serializer;
- a malformed command/state region has a higher device risk;
- completion time is substantially less predictable.

This route begins after the Erf denoiser produces verified audio, and it
continues independently if the first Erf shape or quantization compile is a
clean negative.

### 5.3 Recover or shim the proprietary compiler ABI — rejected for execution

A clean-room compatibility layer around available compiler components could
eventually provide broader operation support.

This is not the immediate route because the exact dependency is absent and
the adjacent CLC candidate already demonstrated an unsafe ABI mismatch. The
candidate remains available for static analysis only.

## 6. Signal and Model Architecture

### 6.1 Frame contract

The public inference contract remains:

```text
sample rate:       48,000 Hz
input hop:         480 mono f32 samples
output hop:        480 mono f32 samples
hop duration:      10 ms
analysis length:   960 samples
FFT length:        960
frequency bins:    481 real bins
window:            periodic square-root Hann
overlap:           50 percent
```

The analyzer keeps the previous 480 samples and appends the current hop to
form one trailing 960-sample frame. It never reads future audio. Weighted
overlap-add emits one 480-sample hop per call after deterministic startup
delay. Window normalization is precomputed and verified by an identity-mask
perfect-reconstruction test.

The startup policy emits zeroes until one complete analysis frame exists.
Reset clears the analysis history, overlap-add state, noise estimator, and
mask history. Reset behavior is part of the 10,000-frame qualification.

### 6.2 Causal noise-power estimate

For each frame and frequency bin:

```text
Y[t,f] = one-sided complex STFT coefficient
P[t,f] = |Y[t,f]|^2
```

The CPU maintains this deterministic causal noise-power estimate:

```text
epsilon        = 1e-12
power smooth   = 0.8 * S[t-1,f] + 0.2 * P[t,f]
minimum window = the last 100 smoothed-power frames
candidate      = max(1.5 * min_window(S)[t,f], epsilon)

if P[t,f] <= 3.0 * N[t-1,f]:
    N[t,f] = 0.9   * N[t-1,f] + 0.1   * candidate
else:
    N[t,f] = 0.995 * N[t-1,f] + 0.005 * candidate
```

The first frame initializes both `S` and `N` to `max(P, epsilon)`. The
100-frame minimum is maintained independently for every bin in a fixed-size
ring. Reset clears the ring and restores first-frame initialization.

These coefficients, initialization, minimum floor, and state version are
stored in the model manifest and covered by the model-package digest.

The estimator is trained and evaluated without clean-reference inputs at
inference time. Clean and noise components are available only to construct
training targets and objective metrics.

The feature passed to the learned affine layer is:

```text
x[t,f] = clip(
    ln(P[t,f] + epsilon) - ln(N[t,f] + epsilon),
    -8,
     8
)
```

`epsilon` is a fixed positive power floor recorded in the manifest.

### 6.3 Learned affine layer

The model has one slope and one bias for every real FFT bin:

```text
z[t,f] = a[f] * x[t,f] + b[f]
```

There are 962 learned scalar parameters:

```text
a: FLOAT32 [481]
b: FLOAT32 [481]
```

Training parameterizes each slope as:

```text
a[f] = softplus(alpha[f]) + 1e-4
```

`alpha` is initialized so that the deployed slope starts at exactly `1.0`.
This ensures that increasing posterior SNR cannot lower the raw mask. The
stored deployment values are the final nonnegative slopes, not optimizer
variables.

The clean-reference target is the clipped square-root Wiener magnitude mask:

```text
target[t,f] = sqrt(
    P_clean[t,f] /
    (P_clean[t,f] + P_noise[t,f] + epsilon)
)
```

The loss evaluates the deployment quantizer in the forward path and uses a
straight-through derivative inside the clipping range:

```text
loss =
    mean((mask_predicted - target)^2)
    + 1e-4 * mean((a[1:] - a[:-1])^2)
    + 1e-4 * mean((b[1:] - b[:-1])^2)
```

The training manifest records the optimizer, step count, batch order,
initialization, seed, loss history, selected validation checkpoint, and the
exact floating-point environment.

### 6.4 Fixed NPU quantization contract

The affine output is clipped and quantized on the CPU:

```text
z_clipped = clip(z, -4.0, 3.96875)
q_in      = clip(round_ties_to_even(z_clipped * 32.0) + 128, 0, 255)
```

The NPU graph contract is:

```text
INPUT UINT8 [481]
  affine asymmetric scale:      0.03125
  affine asymmetric zero point: 128
       |
       v
      Erf
       |
       v
OUTPUT UINT8 [481]
  affine asymmetric scale:      0.0078125
  affine asymmetric zero point: 128
```

The independent CPU oracle is:

```text
z_q       = (q_in - 128) / 32
q_erf     = clip(round_ties_to_even(erf(z_q) * 128) + 128, 0, 255)
mask_raw  = q_erf / 256
mask      = max(mask_raw, 0.05)
```

The `0.05` floor bounds maximum attenuation and is part of the package
signature. The exact producer may implement a different documented integer
rounding point. A candidate remains eligible only when all qualification
outputs are exact or within one U8 unit of this independent oracle.

### 6.5 Synthesis

The CPU multiplies each complex noisy spectrum by the NPU-derived real mask
to form the learned candidate. The final qualified architecture also
constructs a DeepFilterNet candidate and selects it when its retained frame
power is within 1 dB of the noisy frame; otherwise it selects the learned
Erf candidate. The selected candidate is overlap-added into the output. This
causal selector is part of the frozen model policy and does not inspect a
clean reference.

The implementation must:

- reject non-finite input before NPU submission;
- reject a returned tensor with the wrong byte count;
- reject canary corruption;
- reject non-monotonic qualification output;
- clip final floating output to `[-1, 1]` only at the public boundary;
- report clipping counts rather than silently hiding them;
- preserve frame order and exact output length.

## 7. Canonical Model Package

One deployable model is a directory with a canonical manifest covering:

```text
model schema version
model name and semantic version
sample rate, hop, analysis length, FFT length, and window identity
noise-estimator algorithm, coefficients, state version, and epsilon
learned a and b tensor hashes
training and evaluation corpus manifests
full hybrid ONNX SHA-256 and executable graph signature
NPU partition signature
NBG SHA-256, byte size, target, version, tensor contract, and file CRC
input and output quantization
mask floor and rounding rule
CPU oracle implementation version
runtime library and device identity allowlist
quality, numerical, latency, and soak evidence hashes
```

The reference ONNX graph describes the complete learned Erf candidate. The
causal DFN-or-Erf selector remains an explicit CPU orchestration step:

```text
feature FLOAT [1,481]
  -> Mul(a)                 CPU
  -> Add(b)                 CPU
  -> Clip                   CPU
  -> QuantizeLinear         CPU boundary
  -> DequantizeLinear
  -> Erf                    sealed NBG partition
  -> QuantizeLinear         sealed NBG partition
  -> DequantizeLinear       CPU boundary
  -> mask scaling/floor     CPU
```

The sealed-NBG approval is separate from the existing general VSINPU
compiler qualification flag. Passing this design adds one exact approved
partition signature and one exact NBG hash. It does not change
`compiler_qualified` to true and does not authorize any other graph,
dimension, type, quantization, or operation.

A package is invalid if any covered artifact changes without a new package
version and a complete requalification.

## 8. Runtime Components

### 8.1 Host trainer and evaluator

Responsibilities:

- generate or ingest hash-pinned corpus items;
- construct deterministic mixtures;
- train the 962 affine parameters;
- export deterministic weights and ONNX;
- evaluate identity, initialization, learned CPU, and quantized CPU paths;
- emit quality metrics and reproducible audio fixtures.

It never accesses the device.

### 8.2 Sealed NBG worker

Responsibilities:

- verify package and NBG hashes before loading;
- verify live runtime and device identity;
- create exactly one graph from the approved NBG;
- validate runtime-reported input and output metadata;
- accept only one contiguous 481-byte input tensor per call;
- return exactly one contiguous 481-byte output tensor;
- enforce sequence numbers, deadlines, canaries, and cleanup;
- expose structured attempts, successes, failures, and latency.

It does not accept arbitrary model paths, graph bytes, tensor shapes, or
runtime-library paths from production callers.

### 8.3 CPU implementation

The CPU implementation evaluates the same deployed quantized model. It uses
the exact stored affine weights, quantizer, 256-entry Erf oracle table, mask
floor, estimator, and STFT state.

The CPU implementation serves three roles:

- independent host golden;
- same-model hybrid fallback;
- explicit CPU execution mode.

DeepFilterNet also supplies the second candidate in the final
quality-qualified selector. The same-model CPU Erf path remains the
independent numerical oracle and same-frame NPU-failure fallback. Speex and
the existing service-level DeepFilterNet selection remain higher-level
fallback choices when the entire hybrid model cannot initialize.

### 8.4 `NpuInfer` integration

The new backend implements the existing contract:

```text
run(input: &[f32]) -> Result<Vec<f32>>  # input length must equal 480
provider() -> stable backend label
sr() -> 48000
hop_size() -> 480
```

The audio layer is generalized only as far as required to own a boxed
`NpuInfer` implementation. Unrelated denoiser or AEC refactoring is excluded.

Expected provider labels are:

```text
wave_erf_mask_npu_strict
wave_erf_mask_npu_hybrid
wave_erf_mask_cpu
deep_filter_cpu
```

Runtime statistics distinguish requested mode, active backend, NPU attempts,
NPU successes, CPU fallback frames, deadline failures, and the first stable
error code.

## 9. Modes and Failure Semantics

### 9.1 Audit

- verifies the model package and placement;
- constructs no inference session;
- opens no device library or device node;
- reports whether the exact NBG signature is approved.

### 9.2 CPU

- constructs only the CPU implementation;
- makes zero NPU attempts;
- is the reference used for waveform comparison.

### 9.3 Strict

- requires the exact approved package, device identity, runtime hashes, NBG
  hash, tensor metadata, and partition signature;
- treats any create, bind, run, readback, deadline, canary, or health failure
  as an error;
- never reports an enhanced frame produced by CPU fallback as NPU output.

All qualification and completion claims use strict mode.

### 9.4 Hybrid

- makes one NPU session-construction attempt;
- uses NPU output while the worker is healthy;
- on the first NPU failure, computes the affected frame with the same CPU
  model and opens a session-level circuit breaker;
- makes no automatic NPU retry for the rest of that session;
- records the transition and all subsequent CPU frames.

This avoids repeated driver pressure and prevents intermittent NPU results
from being mislabeled.

### 9.5 Error taxonomy

The implementation reuses existing stable errors where applicable and adds
model-specific distinctions:

```text
MODEL_PACKAGE_INVALID
MODEL_WEIGHT_HASH_MISMATCH
MODEL_STATE_VERSION_MISMATCH
NBG_SIGNATURE_NOT_APPROVED
NBG_TENSOR_CONTRACT_MISMATCH
NPU_INPUT_QUANTIZATION_MISMATCH
NPU_OUTPUT_QUANTIZATION_MISMATCH
NPU_OUTPUT_NONMONOTONIC
NPU_OUTPUT_NUMERICAL_MISMATCH
AUDIO_RECONSTRUCTION_MISMATCH
AUDIO_QUALITY_GATE_FAILURE
AUDIO_REALTIME_GATE_FAILURE
CPU_FALLBACK_FAILURE
```

Errors are reported with stage, frame sequence, artifact identities, and
whether an NPU attempt occurred. Audio samples are not embedded in normal
logs.

## 10. Frozen Evaluation Corpus

### 10.1 Training tier

The first training tier is deterministic and procedural. It contains:

- voiced harmonic sources with changing fundamental frequency and envelope;
- unvoiced band-limited sources;
- silence and speech-only preservation intervals;
- white, pink, narrow-band, mains-hum, impulsive, and
  amplitude-modulated-noise families;
- mixture SNRs spanning `-10`, `-5`, `0`, `5`, and `10` dB.

Training, validation, and test seeds are disjoint. Test-only noise parameter
ranges are recorded before training and cannot be moved into training after
metrics are observed.

### 10.2 Natural-speech qualification tier

The natural-speech source is the official
[OpenSLR SLR31 Mini LibriSpeech corpus](https://openslr.org/31/), published
for regression testing under CC BY 4.0.

The two required archives and their publisher-provided MD5 values are:

```text
train-clean-5.tar.gz  5df7d4e78065366204ca6845bb08f490
dev-clean-2.tar.gz    6d7ab67ac6a1d2c993d050e16d61080d
```

The downloaded bytes also receive locally computed SHA-256 hashes. The
publisher MD5 file, license statement, attribution, download URL, archive
hashes, and selected-file hashes are stored in the corpus manifest.

`train-clean-5` is the only natural-speech training pool. `dev-clean-2` is
held out from parameter updates and supplies validation, test, and the device
fixture. The selection algorithm sorts numeric speaker IDs, rejects any
speaker overlap, assigns the first half of `dev-clean-2` speakers to
validation and the second half to test, then sorts utterance paths within
each speaker. It uses every qualifying utterance between 2 and 20 seconds;
no item is selected or removed after metrics are observed.

All source audio is deterministically resampled from 16 kHz to 48 kHz before
mixture construction. The manifest records the resampler identity and
parameters, clean/noise levels, mixture seeds, and generated hashes. Private
workspace recordings are never substituted.

### 10.3 Device fixture

One 12-second fixture is frozen before NPU audio execution. It concatenates:

- noise only;
- clean speech;
- stationary-noise speech;
- nonstationary-noise speech;
- impulsive-noise speech;
- silence.

The device receives only derived UINT8 mask logits, not clean reference
audio. Clean references remain host-side for metrics.

## 11. Stepwise PoCs

Every PoC has a separate manifest and pass/fail result. A clean negative does
not authorize broader graph mutation or an unsafe compiler substitution.

### 11.1 PoC 13A — learned host model

**Device calls:** zero.

Work:

1. implement deterministic framing, STFT, estimator, quantizer, oracle, and
   synthesis;
2. prove identity-mask reconstruction;
3. generate the frozen procedural corpus;
4. acquire and verify the exact SLR31 archives and attribution;
5. train the 962 parameters using only the procedural training split and
   `train-clean-5`;
6. export deterministic weights and ONNX;
7. evaluate initialization, identity, floating model, and quantized model;
8. evaluate `dev-clean-2` without updating parameters;
9. freeze the 12-second device fixture and expected tensors.

Pass:

- two identical runs produce byte-identical model artifacts;
- no train/validation/test seed overlaps;
- trained held-out loss is below deterministic initialization;
- the quantized model improves median SI-SDR by at least 3 dB on the fixed
  synthetic-noise corpus;
- clean-speech RMS changes by no more than 1 dB;
- noise-only attenuation is at least 6 dB;
- held-out natural-speech SI-SDR improves by at least 1 dB;
- held-out natural-speech STOI is no more than `0.02` below the noisy-input
  baseline;
- identity-mask reconstruction has no frame loss and stays within the
  documented floating-point tolerance;
- no non-finite or clipped sample appears in the qualification fixture.

PoC 13A must pass before a new graph is compiled on the Wave.

### 11.2 PoC 13B — bounded compile-only Erf shape

**Device actions:** exactly two independent compile-only processes.

Both processes construct the exact external-input graph from section 6.4.
They use separate contexts, paths, and output files. They do not load an NBG
or call graph inference.

Pass:

- both compiles exit cleanly;
- both output files are byte-identical;
- target is exactly `0x1000000e`;
- VPMN version, file CRC, section table, and type-40 checks validate;
- runtime tensor metadata is UINT8 `[481]` in and UINT8 `[481]` out;
- input scale/zero point are exactly `0.03125` and `128`;
- output scale/zero point are exactly `0.0078125` and `128`;
- preflight and postflight health envelopes match;
- no process, holder, or temporary device file remains.

Any mismatch stops this graph from execution. It does not authorize header or
target patching.

### 11.3 PoC 13C — host serializer and package qualification

**Device calls:** zero.

Work:

- parse both PoC 13B artifacts with the strict VPMN v2 parser;
- prove byte identity and deterministic section ownership;
- verify section-20/section-26 DYNAIO entries when present, and verify zero
  padding; absence is valid for an external-input graph whose profile proves
  `dynamic_io: null`;
- reconstruct the unchanged graph byte-for-byte;
- create the initial sealed model package;
- derive the exact partition signature;
- run corruption, truncation, wrong-shape, wrong-quantization, and wrong-hash
  negative tests.

Pass:

- the strict parser and an independent checker agree;
- unchanged reconstruction is byte-identical;
- every negative fixture is rejected before a runtime call;
- no unresolved byte is mutated;
- the package remains marked `execution_approved: false`.

### 11.4 PoC 13D — bounded numerical NPU qualification

**Device actions:** one graph create, six graph executions, one graph destroy.

The six fixed vectors are:

1. all input zero points;
2. all minimum U8 values;
3. all maximum U8 values;
4. a repeated monotonic `0..255` ramp;
5. one seeded pseudorandom vector;
6. one learned logit frame from the frozen device fixture.

Pass:

- graph create and destroy return success;
- all six executions return exactly 481 bytes;
- canaries remain unchanged;
- output is monotonic for the ramp;
- every element is exact or within one U8 unit of the independent oracle;
- the learned vector output is neither constant nor an identity copy;
- a normalized runtime/driver trace attributes the run to the known
  `libnnsdk -> libovxlib -> libGAL -> /dev/galcore` path;
- strict evidence reports six NPU attempts, six NPU successes, and zero CPU
  fallback frames;
- no health, kernel-alert, holder, or cleanup invariant changes.

Only after this pass may the exact NBG hash be added to the sealed-partition
approval registry.

### 11.5 PoC 13E — NPU-derived enhanced waveform

**Device actions:** one graph create, exactly 1,201 sequential graph
executions including the offline flush, one graph destroy.

The host computes causal features and affine logits for the frozen 12-second
fixture. The Wave processes every 481-byte mask vector. The host reconstructs
audio exclusively from returned NPU mask tensors.

Pass:

- every expected frame has one ordered NPU result;
- strict mode records zero CPU fallback frames;
- the NPU and quantized-CPU mask tensors differ by at most one U8 unit;
- the NPU and quantized-CPU floating waveforms differ by no more than
  `1 / 128` at any sample and have error RMS below `-50 dBFS`;
- NPU-derived output improves SI-SDR by at least 3 dB on noisy-speech
  segments;
- noise-only attenuation is at least 6 dB;
- the fixture's single 2-second clean transition segment stays within
  1.5 dB, while the broader 477-item held-out clean-speech qualification
  remains within 1 dB;
- there are no missing samples, clipping, NaNs, or discontinuities at segment
  boundaries;
- input, CPU output, NPU output, masks, metrics, and logs all have exact
  hashes.

Passing this stage proves actual noise cancellation using numerical NPU
outputs. It does not yet prove sustained real-time readiness.

### 11.6 PoC 13F — hybrid integration and soak

**Device actions:** one approved graph, sequential execution only, exactly
10,000 consecutive qualification frames.

Work:

- integrate the model behind `NpuInfer`;
- exercise strict, hybrid, CPU, and audit modes;
- inject one pre-execution NPU failure and one post-construction worker
  failure using host-controlled fault hooks;
- compare frame ordering and output continuity;
- run the 100-second qualification soak;
- collect p50, p95, p99, maximum, CPU load, memory, and health evidence.

Pass:

- 10,000 consecutive strict-mode NPU frames complete;
- no frame is missing, duplicated, or reordered;
- strict NPU output matches the same CPU model within numerical tolerance;
- NPU inference p99 is below 5 ms;
- total per-hop processing p99 is below the 10 ms hop duration;
- reset reproduces the documented initial state with no drift;
- memory does not grow monotonically;
- there are no deadline misses, kernel alerts, holder leaks, or health
  changes;
- hybrid fault injection computes the affected frame on CPU, opens the
  circuit breaker, and performs no NPU retry;
- CPU fallback independently stays below one-hop p99 and a `0.8` real-time
  factor.

The NPU backend remains opt-in after this pass. Enabling it by default is a
separate production decision.

**Observed implementation status:** the Python/lab hybrid boundary passed
same-frame CPU fallback and permanent circuit-breaker tests. A fixed native
runner completed 10,000 consecutive NPU executions with exact rolling
oracle digests, canaries, and unchanged pre/post device health. The planned
Rust `NpuInfer` integration and the full service-level p50/p95/p99, CPU,
RSS, reset, and thermal qualification were not implemented in this research
session.

### 11.7 PoC 14 — serializer-first learned-operation expansion

After PoC 13E proves an NPU-derived enhanced waveform, continue controlled
host analysis toward a fixed Conv/Sigmoid or Conv/Erf mask network:

1. correlate stock ONNX weights with coefficient sections;
2. obtain single-variable compile-only differentials where the exact
   producer supports them;
3. recover coefficient packing and patch/relocation ownership;
4. require byte-identical reconstruction of compiler-produced controls;
5. create a separately reviewed numerical execution plan for the first Conv
   artifact.

PoC 14 does not weaken or retroactively relabel the hybrid Erf result.

## 12. Device Safety Policy

### 12.1 Always allowed

- host source and binary inspection;
- host model training and CPU inference;
- host NBG parsing and in-memory reconstruction;
- read-only ADB identity and health collection already covered by the lab
  policy;
- creation of local manifests and documentation.

### 12.2 Fixed device-call caps

| PoC | Compile calls | Graph creates | Graph executions | Graph destroys |
|---|---:|---:|---:|---:|
| 13B | 2 | 0 | 0 | 0 |
| 13D | 0 | 1 | 6 | 1 |
| 13E | 0 | 1 | 1,201 exactly | 1 |
| 13F | 0 | 1 | 10,000 exactly | 1 |

No stage starts until the preceding stage passes. There is no automatic
retry, parallel graph execution, or escalation to a different library.

### 12.3 Prohibited

- firmware, boot, flash, root-filesystem, stock-model, or stock-library
  writes;
- service stop, reboot, module reload, clock or thermal changes;
- raw register access or raw ioctl replay;
- target-ID patching followed by execution;
- malformed graph execution or fuzzing;
- invoking the quarantined CLC;
- automatic NPU reset;
- using an output path outside the dedicated temporary lab root.

### 12.4 Abort conditions

The active stage aborts immediately on:

- device identity or library-hash mismatch;
- unexpected graph metadata;
- compiler nondeterminism;
- parser or CRC failure;
- runtime status failure;
- timeout, canary corruption, or output-size mismatch;
- numerical error above one U8 unit;
- new kernel alert, health-envelope difference, holder leak, or cleanup
  failure.

An abort records evidence and returns to host analysis. It does not broaden
device access.

## 13. Validation Contract

### 13.1 Learned-model evidence

- weights are generated by the documented training run;
- training is deterministic from the pinned environment and seed;
- model hashes reproduce;
- held-out data are disjoint;
- the learned model beats initialization and identity ablations;
- inference uses no clean-reference tensor.

### 13.2 NPU residency evidence

- the exact NBG target and runtime tensor metadata validate;
- a graph is created through the exact Wave runtime;
- a normalized trace reaches `/dev/galcore`;
- returned bytes satisfy an independent numerical oracle;
- strict mode reports NPU attempts and successes with zero fallback;
- the NPU-returned masks are the masks used for waveform reconstruction.

Compilation, graph creation alone, operator names, library strings, and a
nonzero output do not satisfy this contract.

### 13.3 Audio-quality evidence

The stricter existing parent requirement remains authoritative:

- median SI-SDR improvement is at least 3 dB on the fixed synthetic-noise
  corpus;
- natural-speech STOI is no more than `0.02` below the noisy-input baseline;
- noise-only attenuation is at least 6 dB;
- clean-speech RMS remains within 1 dB;
- CPU and NPU reconstructions remain within the numerical tolerance.

The previously discussed 2 dB exploratory floor is not used as the final
completion bar because the parent hybrid design already requires 3 dB.

### 13.4 Stability evidence

- at least 10,000 consecutive frames;
- NPU inference p99 below 5 ms;
- total processing p99 below 10 ms;
- zero deadline misses in the qualification soak;
- deterministic reset;
- bounded memory;
- unchanged device health.

### 13.5 Fallback evidence

- explicit CPU mode makes no NPU call;
- hybrid mode attempts only the approved graph;
- the first injected NPU failure falls back for the same frame;
- a circuit breaker prevents automatic NPU retries;
- fallback output uses the same learned quantized model;
- initialization failure can still select the existing DeepFilterNet or
  Speex backend at the audio-service level.

## 14. Evidence and Documentation Layout

Tracked design and results:

```text
docs/superpowers/specs/
  2026-07-29-wave-npu-learned-erf-noise-suppression-design.md

docs/superpowers/plans/
  2026-07-29-wave-npu-learned-erf-noise-suppression.md

docs/npu/models/erf-mask/
  README.md
  completion-audit.md
  model-manifest.json
  training-manifest.json
  evaluation-summary.json
  partition-policy.json

docs/npu/lab-notebook/
  2026-07-29-poc13a-learned-erf-mask-host.md
  2026-07-29-poc13b-erf-mask-compile.md
  2026-07-29-poc13c-erf-mask-package.md
  2026-07-29-poc13d-erf-mask-numerical.md
  2026-07-29-poc13e-erf-mask-waveform.md
  2026-07-29-poc13f-erf-mask-soak.md
```

Large or device-specific evidence remains under the gitignored run root:

```text
artifacts/npu/runs/poc13*/
```

Every notebook contains:

- hypothesis and falsifiers;
- exact inputs and hashes;
- safety envelope and call counts;
- reproduction command;
- observed stdout, stderr, timing, and return codes;
- machine-result path and hash;
- interpretation;
- explicit non-claims;
- next gate.

## 15. Completion Criteria

The original production milestone required all of the following:

1. A deterministic learned 962-parameter model package exists.
2. The package passes held-out ablation and quality gates.
3. The exact UINT8 `[481]` Erf NBG compiles twice identically for
   `0x1000000e`.
4. The NBG and package pass strict parser, CRC, metadata, signature, and
   corruption tests.
5. Six bounded NPU vectors match the independent CPU oracle within one U8
   unit.
6. Strict evidence proves the numerical calls reached the NPU with no CPU
   fallback.
7. A complete enhanced waveform is reconstructed from NPU-returned masks.
8. That waveform passes SI-SDR, attenuation, speech-preservation, and
   CPU/NPU agreement gates.
9. The implementation runs behind `NpuInfer`.
10. Strict, hybrid, CPU, and audit modes behave as specified.
11. Fault injection proves same-frame CPU fallback and no automatic retry.
12. At least 10,000 consecutive frames meet latency, ordering, memory, health,
    and cleanup gates.
13. All required manifests, lab notebooks, hashes, and reproduction commands
    exist and agree with current artifacts.

Criteria 1–8 are proven. Criterion 11 is proven at the Python/lab integration
boundary. Criterion 12 is proven for 10,000-frame numerical ordering and
device health, but not for the full service-level percentile, memory, reset,
and thermal requirements. Criteria 9 and 10 remain incomplete because the
production Rust `NpuInfer` integration was not implemented. Criterion 13 is
proven by the final documentation audit.

This supports the narrower, evidence-backed statement “working
NPU-assisted noise cancellation research PoC”: a complete enhanced waveform
was reconstructed from numerical NPU outputs and passed the stated quality
gates. It does not support “production NPU backend complete.” Compilation,
one nonzero tensor, a CPU-only waveform, or an analytic Erf gate alone would
still be insufficient.

## 16. Follow-On Completion

After this milestone:

- continue PoC 14 until a learned Conv partition runs numerically;
- add that partition to the sealed allowlist without globally qualifying the
  compiler;
- compare dispatch and quality against the Erf model;
- investigate batched masks only after single-frame streaming is stable;
- evaluate residual echo suppression, VAD, and other models as separate
  hash-pinned PoCs.

The hybrid Erf model remains a supported fallback and a numerical regression
fixture for all later compiler work.
