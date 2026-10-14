# wave-onnx-hybrid

ONNX admission, deterministic CPU inference, and an explicit native backend
for the Insta360 Wave NPU research program.

The general PoC 9 policy approves no NPU signatures. PoC 13 separately
qualifies one exact U8 `[481]` Erf NBG through a sealed partition policy.
The default backend retains that policy. The separate `wave-native` backend
imports single-node FP16 Conv or U8 QLinearMatMul graphs and delegates
hash-bound compilation and execution to the repository's bounded ADB lab
bridge. Each exact model needs its own successful device qualification.

The bounded `fp16-marblenet-v1` graph profile also imports actual trained
Conv/Add/Relu graphs. PoC18 qualified MarbleNet's pointwise and first
residual blocks; its K29/dilation2 depthwise control was rejected. The
original full package uses the explicit file-oriented hybrid CPU frontend,
and strict mode refuses that unqualified package. PoC19's separate
`fp16-marblenet-pad-v1` exact rewrite now qualifies the complete trained model
on NN/TP. All800 frozen frame decisions match original FP32; the paired
prepared-feature boundary is10.399× faster than one-thread Wave CPU inference.
The qualified19f WAV frontend runs strictly on native with zero CPU neural
sessions; it retains the fixed3.99-second window and future-context limits. See the
[MarbleNet guide](../../../docs/npu/onnx/marblenet.md) for the command,
optional Torch feature environment, fixed window and evidence.

Use the [native backend guide](../../../docs/npu/onnx/native-backend.md) for
the supported tensor contracts, package lifecycle and device commands.

The complete result, evidence hashes, limitations, and promotion gate are in
[`docs/npu/onnx/README.md`](../../../docs/npu/onnx/README.md).

## Learned Erf-mask result

This package now also contains the host side of the
`learned-npu-assisted-erf-mask-denoiser`:

```text
48 kHz audio
  -> causal STFT and noise estimator
  -> learned 481-slope + 481-bias logits
  -> sealed UINT8 [481] Erf partition
  -> learned candidate + DeepFilterNet candidate
  -> causal selector and inverse STFT
```

For the frozen device fixture, the package built and hash-bound 1,201 input
frames, replayed 1,201 Wave-returned tensors, refused any input-stream drift,
and reconstructed 576,000 output samples. NPU/CPU tensor and waveform
differences were both zero. The NPU-derived candidate was selected for 803
frames.

`HybridErfPartition` validates the exact U8 `[481]` result. On the first
exception or contract violation it recomputes the affected frame through the
CPU Erf oracle, permanently latches CPU fallback, and makes no retry.

Use the command inventory:

```bash
uv run wave-erf-mask --help
```

The full commands, evidence hashes, metrics, and production non-claims are
in the
[`Erf-mask result guide`](../../../docs/npu/models/erf-mask/README.md) and
[`completion audit`](../../../docs/npu/models/erf-mask/completion-audit.md).

## Setup

```bash
cd waved/tools/onnx-hybrid
uv sync --python 3.12 --dev
```

Use the tracked `uv.lock` and the repository editable installation. Native
commands also need `waved/scripts/npu/`, the checked-in C++ templates, the
exact local SDK/runtime evidence and the pinned Docker image. A standalone
wheel does not include these resources. Host dependencies stay in this
environment.

## Commands

Verify all generated fixture bytes:

```bash
uv run wave-onnx-hybrid verify-fixtures fixtures
```

Audit without creating a runtime session:

```bash
uv run wave-onnx-hybrid audit \
  fixtures/spectral_mask_conv.onnx \
  --output /tmp/wave-spectral-audit.json
```

Run explicit CPU inference:

```bash
uv run wave-onnx-hybrid run \
  fixtures/spectral_mask_conv.onnx \
  --inputs fixtures/spectral_mask_input.npz \
  --mode cpu \
  --outputs /tmp/wave-spectral-cpu.npz \
  --evidence /tmp/wave-spectral-cpu.json
```

Run current fail-closed hybrid mode:

```bash
uv run wave-onnx-hybrid run \
  fixtures/spectral_mask_conv.onnx \
  --inputs fixtures/spectral_mask_input.npz \
  --mode hybrid \
  --outputs /tmp/wave-spectral-hybrid.npz \
  --evidence /tmp/wave-spectral-hybrid.json
```

Default-backend hybrid behavior is exactly one CPU session and zero NPU
attempts. Explicit `--backend wave-native` uses a qualified package or
reports CPU fallback.

Default-backend strict mode must fail before session construction:

```bash
uv run wave-onnx-hybrid run \
  fixtures/spectral_mask_conv.onnx \
  --inputs fixtures/spectral_mask_input.npz \
  --mode strict \
  --outputs /tmp/wave-spectral-strict.npz \
  --evidence /tmp/wave-spectral-strict.json
```

All output paths are exclusive.

## Package layout

```text
src/wave_onnx_hybrid/
  audit.py       policy, admission, signing, placement, partition evidence
  runtime.py     CPU session and one-attempt fallback semantics
  fixtures.py    deterministic ONNX and NPZ controls
  cli.py         exclusive file-oriented command boundary
  native_import.py   exact ONNX profiles and independent numerical oracle
  native_sources.py  immutable native compile and execution templates
  native_backend.py qualification-bound session and latched fallback
  native_cli.py     import/build/compile/qualify command boundary
  native_fixtures.py four ONNX-derived native controls
  errors.py      stable machine error categories
  erf_mask_signal.py         framing, estimator, quantization, synthesis
  erf_mask_corpus.py         deterministic procedural/natural corpus
  erf_mask_train.py          deterministic 962-parameter trainer
  erf_mask_model.py          learned model and ONNX export
  erf_mask_inference.py      CPU and latched hybrid Erf partition
  erf_mask_device_stream.py  sealed input/output stream contract
  erf_mask_qualification.py  held-out quality gates
  erf_mask_cli.py            host training/evidence command boundary

fixtures/
  manifest.json
  dynamic_identity.onnx
  mixed_sin.onnx
  spectral_mask_conv.onnx
  spectral_mask_input.npz
  spectral_mask_expected.npz

tests/
  test_hybrid.py
  test_erf_mask_*.py
```

## Design invariants

- CPU admission and NPU eligibility are separate decisions.
- The compiler-refusal manifest is hash-pinned by policy.
- An operator name never grants NPU placement.
- The graph signature describes executable model identity; policy separately
  binds compiler/profile authorization.
- Model limits include nested control-flow graphs and tensor-valued
  attributes.
- External tensor data is always rejected.
- Audit mode creates no session.
- Default-backend hybrid creates one CPU-only session and makes no NPU call.
- Native eligibility binds the supplied ONNX, lowering, source, ELF, NBG,
  CPU qualification evidence, actual readback and cleanup evidence.
- Native strict loading creates no CPU session. Hybrid execution failure
  latches CPU fallback without a second device dispatch.
- Strict mode never silently changes into CPU mode.
- A future NPU constructor gets at most one attempt.
- ONNX Runtime must report only `CPUExecutionProvider`.
- Input and output names, dtypes, ranks, dimensions, sizes, contiguity, and
  finiteness are validated without implicit conversion.
- Evidence and NPZ output are deterministic and never overwrite an existing
  path.

## Tests

```bash
uv run pytest -q
uv run ruff check .
uv run ruff format --check .
```

The focused suite covers admission limits, nested graphs, external data,
signatures, policy binding, placement, session construction, runtime tensor
validation, numerical output, deterministic serialization, and CLI refusal
behavior.

## Future NPU integration

Do not add a signature to `approved_npu_signatures` merely because a graph
uses a familiar operator. Promotion requires:

1. a compiler manifest that independently authorizes execution;
2. an exact Wave-targeted artifact for that graph signature;
3. static NBG target, structure, tensor, and checksum validation;
4. a separately reviewed bounded device execution;
5. numerical agreement with this CPU oracle;
6. repeatability, cleanup, device-health, and fallback evidence.

Until then, all placement in the general PoC 9 policy remains CPU. The
separate PoC 13 approval applies only to its exact sealed Erf partition and
does not imply general operator placement.
