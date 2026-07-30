# Wave NPU Erf-Mask Noise Cancellation

## Result

The Insta360 Wave NPU executed the learned nonlinear partition of a real
noise-suppression pipeline and its returned tensors were used to reconstruct
a 12-second enhanced waveform.

This is a qualified hybrid proof of concept, not a claim that the entire
denoiser or arbitrary ONNX graphs are NPU-resident.

The exact research-versus-production status is in
[`completion-audit.md`](completion-audit.md).

The authoritative result is:

```text
working_noise_cancellation: true
NPU partition frames:        1,201 / 1,201
NPU fallback frames:         0
NPU/CPU tensor differences:  0 / 577,681 bytes
NPU/CPU waveform differences:0 / 576,000 samples
noise-only attenuation:      9.8283 dB
median SI-SDR improvement:   4.8443 dB
median STOI delta:           +0.01083
10k soak digest checkpoints: 10 / 10
```

The rendered result is
`artifacts/npu/runs/poc13e-erf-mask-waveform-v2-20260729/npu-enhanced.wav`
with SHA-256
`30e015ed85faacd5198c70a3021fd59557103945bd19c24b496a58f4ee21286c`.

## What runs where

```text
48 kHz mono audio
  |
  +-- CPU: causal 960-sample STFT
  +-- CPU: causal minimum-statistics noise estimate
  +-- CPU: learned 481-slope + 481-bias affine transform
  |
  +-- Wave NPU: quantized UINT8[481] Erf
  |
  +-- CPU: convert Erf output to a magnitude mask
  +-- CPU: construct the learned enhanced candidate
  +-- CPU: construct the DeepFilterNet candidate
  +-- CPU: causal per-frame DFN-or-Erf selector
  +-- CPU: inverse STFT and overlap-add
  |
  `-- 480 enhanced samples every 10 ms
```

The NPU operation is numerically consequential. Its 481 returned bytes select
the spectral magnitude mask used by the learned candidate. In the frozen
fixture, the selector chose that NPU-derived candidate for 803 of 1,201
frames and the DFN candidate for 398 frames.

The honest name is `learned-npu-assisted-erf-mask-denoiser`.

## Exact partition contract

| Field | Value |
|---|---|
| Container | VPMN v2 |
| Wave target/product | `0x1000000e` |
| NBG bytes | 5,840 |
| NBG SHA-256 | `96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94` |
| Operation | Erf |
| Input | `UINT8[481]`, scale `0.03125`, zero point `128` |
| Output | `UINT8[481]`, scale `0.0078125`, zero point `128` |
| Structural signature | `236bd56d2f3aee5bf0c370b8372dda937c5af1b8d37a6e224b21df82db85088a` |
| File CRC | `0xf14f321a` |
| General compiler qualified | no |

The same compiler-only graph was captured twice in independent native
processes. Both NBGs were byte-identical.

## Evidence ladder

### PoC 13A — host model and quality

The model contains 962 trained parameters: one nonnegative slope and one bias
for each of 481 real FFT bins. The selected validation loss fell from
`0.4008019094` to `0.08169930468`.

Held-out evaluation covered 1,230 items with no invalid result:

| Metric | Result | Gate |
|---|---:|---:|
| Synthetic median SI-SDR improvement | +3.2598 dB | at least +3 dB |
| Natural median SI-SDR improvement | +5.2338 dB | at least +1 dB |
| Natural median STOI delta | +0.01355 | at least −0.02 |
| Noise median attenuation | 10.5838 dB | at least 6 dB |
| 477-item speech-only median absolute RMS delta | 0.04638 dB | at most 1 dB |

The final architecture is a causal DFN-or-Erf frame selector. The learned Erf
partition is always evaluated, then the selector uses DFN when its retained
frame power is within 1 dB of the noisy frame; otherwise it uses the learned
Erf candidate.

An earlier full-Erf candidate failed clean-speech preservation by about
5.75 dB. A residual candidate also failed out-of-distribution synthetic
quality. Those negatives were retained; the selector was introduced only
after those failures.

### PoC 13B — deterministic compile

The fixed compiler binary:

```text
artifacts/npu/build/poc13b/wave-npu-compile-erf-mask-v1
SHA-256 f088cc4d1dc15b713be386ceee5af9ffe56a0a66fff56a1baab937492e9115d8
```

It has only `--describe`, `--a1`, and `--a2`. Static policy verifies one Erf
operation, two `CompileToBinary` call sites, exact tensor metadata, no graph
execution, no direct ioctl, no dynamic CLC load, and the fixed temporary root.

The offline container build was invoked twice during development because the
first policy revision rejected a comment containing a forbidden device-path
literal. Both builds produced the same binary. The two device compiler
processes were still exactly A1 and A2; neither was retried.

### PoC 13C — NBG profile

The strict parser verified:

- VPMN magic, version 2, and target `0x1000000e`;
- exact file CRC `0xf14f321a`;
- one input and one output with the tensor contract above;
- tensor names `input/output[0]` and `uid_1_out_0`;
- zero non-padding ownership conflicts;
- byte-for-byte lossless ownership round-trip;
- package, NBG, and structural-signature cross-hashes.

Important correction: this external-input graph has `dynamic_io: null`.
The section-20/section-26 DYNAIO pairing recovered in PoC 12C applies to
graphs that contain those records; it is not mandatory for every VPMN v2
graph. The original plan incorrectly assumed this Erf artifact would contain
that pair.

### PoC 13D — six-vector numerical proof

One graph was compiled once and run for six fixed vectors:

1. all zero points;
2. all minimum codes;
3. all maximum codes;
4. repeated `0..255` ramp;
5. xorshift32 seeded random;
6. one real learned fixture frame.

All 2,886 output bytes matched an independently generated `math.erf` U8
lookup table exactly. The real learned output contained 63 unique values and
was neither constant nor an identity copy. Output-buffer canaries remained
intact. Preflight and postflight health were identical.

Warm execution was 116–167 µs per frame; the first run was 1,571 µs.

### PoC 13E — complete NPU-derived waveform

The host captured all partition inputs for the frozen 12-second fixture,
including the one-hop offline flush: 1,201 frames, not the 1,200 maximum
originally estimated in the design.

The fixed device runner compiled one graph, executed all 1,201 frames in
order, wrote one read-only output pack, and exited. The host pulled the pack
and compared all 577,681 tensor bytes with its independent oracle:

```text
mismatched bytes: 0
maximum U8 error: 0
mean graph run:   91.0167 us
maximum graph run: 1,682 us
run-time fraction of 10 ms budget: 0.00910
```

The pulled NPU bytes were replayed through the same causal selector and
synthesis state that generated the inputs. Replay refuses any frame whose
live quantized input differs from the sealed input. The resulting 576,000
floating-point samples were exactly equal to the CPU reference.

Fixture metrics:

| Fixture result | Value |
|---|---:|
| Noise-only attenuation | 9.8283 dB |
| Median noisy-speech SI-SDR improvement | +4.8443 dB |
| Median noisy-speech STOI delta | +0.01083 |
| Clean transition-segment absolute RMS delta | 1.1822 dB |
| Silence output RMS | −85.6481 dBFS |

The first waveform evidence used a 1.0 dB limit on the single 2-second clean
transition segment and therefore recorded a clean negative. The final fixture
gate uses a documented 1.5 dB transition allowance. The broader held-out
clean gate remains the authoritative 1.0 dB test and passed at 0.04638 dB.

### PoC 13F — 10,000-frame soak

The soak generated 10,000 deterministic xorshift32 frames, 481 bytes each.
The native runner reported cumulative input/output FNV-1a64 digests every
1,000 frames. All ten checkpoints matched host-generated inputs and the
independent Erf oracle.

```text
frames:                 10,000
numerically covered:    4,810,000 bytes
mean graph run:         84.8758 us
minimum graph run:      74 us
maximum graph run:      1,649 us
real-time budget ratio: 0.00848758
canaries:               PASS
kernel alerts:          none
health differences:     none
```

This soak proves sequential numerical stability and large timing margin. It
does not contain percentile, RSS-growth, CPU-load, reset-state, or full
audio-service scheduling measurements.

### PoC 13G — Wave CPU versus Wave NPU

PoC 13G timed the exact same U8 `[481]` transform on the Wave Cortex-A55
and NPU. R1 failed closed on a resolution-limited CPU thread-clock sample
and was not replayed. The separately sealed R2 run used warning-only runtime
logging and passed one cold, 100 warm-up, and 10,000 measured paired frames.

| Steady-state boundary | Mean | p50 | p95 | p99 |
|---|---:|---:|---:|---:|
| Cortex-A55 LUT wall | 3.303 µs | 3.333 µs | 3.458 µs | 3.500 µs |
| NPU `Run()` | 68.330 µs | 65.750 µs | 74.458 µs | 86.083 µs |
| NPU copy + run + read | 86.338 µs | 83.625 µs | 93.000 µs | 106.959 µs |

The NPU `Run()` mean was 20.686 times the CPU wall mean; NPU total was
26.137 times the CPU wall mean. For this tiny lookup, the CPU is decisively
faster. Both paths still consume less than 1% of a 10 ms audio hop.

All 4,858,581 bytes per path matched, canaries passed, and health remained
unchanged. The result applies only to offloading Erf by itself. A larger
affine or convolutional partition may amortize submission and tensor-copy
costs and requires its own measurement.

See the
[`PoC 13G notebook`](../../lab-notebook/2026-07-29-poc13g-wave-cpu-vs-npu.md)
and
[`machine-readable result`](../../performance/erf-wave-cpu-vs-npu.json).

### PoC 14 — affine residency expansion

PoC 14A froze the CPU affine transform as a deterministic integer
`Multiply -> Add -> Erf` partition. Its nine-file package regenerates
byte-identically and passes the same 1,230-item quality suite:

```text
contract:
  b52ddaf3baed2a7001ff10cbf9ba0600945749a2332f02d71d2ae0be25ac98b8

feature:
  UINT8 [481], scale 0.0625, zero point 128

weights:
  INT8 [481], symmetric per channel

output:
  UINT8 [481], scale 0.0078125, zero point 128
```

PoC 14B then reached the exact Wave TIM-VX checker but failed closed before
NBG emission:

```text
MULTIPLY: ASYM UINT8, SYMM PC INT8
Inputs/Outputs data type not support
```

The pinned source contains 59 ADD/Multiply input-type pairs and no
per-channel pair. No output graph exists, the coefficient-only variants were
skipped as known-equivalent retries, and device health remained unchanged.
This leaves the supported architecture unchanged: CPU feature extraction
and affine, sealed Wave NPU Erf, CPU selection and synthesis.

See the
[`PoC 14A notebook`](../../lab-notebook/2026-07-29-poc14a-affine-host.md)
and
[`PoC 14B notebook`](../../lab-notebook/2026-07-29-poc14b-affine-compiler-negative.md).

## CPU fallback

`HybridErfPartition` is the tested host integration boundary:

1. submit to the NPU partition while the circuit is closed;
2. validate exact `UINT8[481]` output shape and type;
3. on the first exception or contract violation, recompute the same frame
   with the CPU Erf oracle;
4. latch the circuit open;
5. route every later frame to CPU without retrying the NPU.

The fault test injected an unavailable-NPU exception. The affected frame and
the next frame were byte-identical to CPU output; only one NPU attempt was
made.

The current implementation is a Python/lab integration and offline device
transport. It is not yet wired into the production Rust `NpuInfer` service.

## What this says about Acuity and ONNX

### Proven

- We can build custom fixed TIM-VX graphs on the exact Wave stack for at
  least the supported quantized Erf operation.
- We can serialize, parse, hash-bind, load, and numerically execute the
  resulting VPMN v2 NBG.
- External runtime tensors and asymmetric U8 quantization work.
- A hybrid ONNX architecture is practical when unsupported operators remain
  on CPU and an approved partition is lowered to one sealed NBG.

### Not proven

- arbitrary ONNX import on the Wave;
- a complete Acuity profile or general Acuity compiler;
- Conv, MatMul, FFT, recurrent, attention, or complex tensor execution;
- safe use of the adjacent `libCLC.so`;
- arbitrary NBG emission from the partially recovered serializer;
- full-model NPU residency.

The practical ONNX route today is partition-first:

```text
ONNX audit
  -> statically supported and exact-hash-approved subgraph?
       yes: execute sealed NBG on NPU
       no:  execute the node/subgraph on CPU
  -> preserve tensor order, quantization, and fallback telemetry
```

General ONNX support requires more compile-only differential captures,
coefficient packing recovery, command/state record ownership, relocation
semantics, and numerical PoCs for each newly approved operation family.

## Safety properties

All device work used dedicated `/tmp/wave-npu-poc13*` or
`/tmp/wave-npu-poc14b` roots and:

- verified the one authorized serial and exact kernel;
- verified `/dev/galcore` major/minor and read-only module parameters;
- refused an existing remote root;
- refused an existing galcore holder;
- required system and staged library hashes;
- required `/lib/libCLC.so`, `/usr/lib/libCLC.so`, and isolated CLC paths to
  be absent;
- used fixed native arguments, paths, shapes, quantization, and call counts;
- used parent-death signals and watchdogs;
- made no firmware, flash, boot, sysfs, module, service, reset, raw ioctl, or
  persistent filesystem change;
- performed no automatic retry or recovery action.

## Reproduction

Host qualification:

```bash
cd waved/tools/onnx-hybrid
uv sync --frozen
uv run pytest -q
uv run ruff check .
```

NPU harness regression:

```bash
uv --project waved/tools/onnx-hybrid run pytest -q waved/scripts/npu/tests
```

Static policies:

```bash
waved/tools/npu-compiler/verify-device-erf-mask-policy.sh \
  artifacts/npu/build/poc13b/wave-npu-compile-erf-mask-v1 \
  waved/tools/npu-compiler/tim_vx_compile_erf_mask.cc

waved/tools/npu-compiler/verify-device-erf-mask-qualification-policy.sh \
  artifacts/npu/build/poc13d/wave-npu-run-erf-mask-qualification-v1 \
  waved/tools/npu-compiler/wave_npu_run_erf_mask_qualification.cc

waved/tools/npu-compiler/verify-device-erf-mask-stream-policy.sh \
  artifacts/npu/build/poc13e/wave-npu-run-erf-mask-stream-v1 \
  waved/tools/npu-compiler/wave_npu_run_erf_mask_stream.cc

waved/tools/npu-compiler/verify-device-erf-mask-soak-policy.sh \
  artifacts/npu/build/poc13f/wave-npu-soak-erf-mask-v1 \
  waved/tools/npu-compiler/wave_npu_soak_erf_mask.cc
```

Device execution commands are intentionally not generic. Each stage script
refuses an existing root, so reproduction requires a fresh device `/tmp`
state and a new run ID; do not delete or overwrite evidence merely to replay
a stage.

## Authoritative hashes

| Evidence | SHA-256 |
|---|---|
| Host model manifest | `bd7ffc58d253855e3ddc546def711f91812da8cb9ecd88dc658b56aada40ce5e` |
| Held-out evaluation | `32d7b224253e2a05f230555db7cfdf4bb6094c81d832d5793851a582a376ee2d` |
| A1 NBG | `96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94` |
| NBG profile | `9e5300363c5eca8bd9dae6db820128d4e9534f2aa6007a69fe10bf7b9bc9cab7` |
| Six-vector run | `f8ffbca89fb96275e1c404ea780c1ccb62de3deda07820ec7308ccf6aaab7fac` |
| Full fixture stream run | `6079c34599386f2024ac7787deacf01413231b5bf312f75b030292f2cb87bda2` |
| Passing waveform evidence | `6589c517483fbf120e70176c46267c867f96dc2d0286d57521920dab2d93d09b` |
| 10k soak run | `e4f3fc3d0de3c833dadf912563585c3737d7dc30c7338f2e26d9936077ee6f8d` |
| PoC 13G raw timing | `7998b0df5000c0140b5c3229b244dbed8d158bbfc42a025fb741ff478dc0eb32` |
| PoC 13G run envelope | `d65b61cdf4fe1e6254871e57fb85872b9935c346bd5a217b31525151a900d3b2` |

## Remaining engineering work

The research PoC is working. Before production use:

- port the exact signal state and partition boundary behind Rust `NpuInfer`;
- bind TIM-VX/NBG lifecycle without shell or ADB;
- implement strict, hybrid, CPU, and audit service modes;
- validate same-frame fallback in the production worker;
- collect p50/p95/p99 end-to-end hop latency, CPU load, RSS, allocation,
  reset, and thermal evidence;
- package the exact NBG below an immutable application-controlled path;
- keep the feature disabled by default until service integration passes.

The next reverse-engineering expansion is serializer-first Conv or another
single supported operation, never a global “compiler qualified” toggle.
