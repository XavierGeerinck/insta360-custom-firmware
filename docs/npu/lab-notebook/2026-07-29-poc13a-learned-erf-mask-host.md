# PoC 13A — Learned Erf-Mask Host Qualification

## Question

Can a deterministic learned model built around quantized Erf improve noisy
speech while preserving clean speech, before any new Wave device call?

## Falsifiers

- training does not improve validation loss;
- model artifacts do not reproduce;
- synthetic median SI-SDR improvement is below 3 dB;
- natural median SI-SDR improvement is below 1 dB;
- natural STOI delta is below −0.02;
- noise attenuation is below 6 dB;
- 477-item clean-speech median absolute RMS delta exceeds 1 dB;
- any invalid, non-finite, or clipped evaluation item;
- any device call.

## Model

The host computes a causal 960-point STFT, causal noise estimate, and 481
learned slopes plus 481 biases. Quantized logits use U8 scale `0.03125` and
zero point `128`. The independent Erf oracle returns U8 scale `0.0078125`
and zero point `128`.

The final denoiser evaluates a learned Erf candidate and a DeepFilterNet CPU
candidate every frame. It selects DFN when retained frame power is at least
`0.7943282347` of noisy power; otherwise it selects the learned candidate.

## Negative iterations retained

- The first full learned-Erf waveform lost about 5.75 dB on clean speech.
- A weak residual Erf postfilter did not satisfy out-of-distribution
  synthetic quality.
- The causal DFN-or-Erf selector was the first candidate to satisfy every
  held-out gate.

## Result

```text
status: PASS
items: 1,230
invalid: 0
synthetic median SI-SDR improvement: +3.2597667132 dB
natural median SI-SDR improvement:   +5.2338097012 dB
natural median STOI delta:           +0.0135532935
noise median attenuation:            10.5837640380 dB
clean median absolute RMS delta:      0.0463758547 dB
partition calls:                      740,742
Erf-selected frames:                  450,140
DFN-selected frames:                  290,602
device calls:                         0
```

## Evidence

| Artifact | SHA-256 |
|---|---|
| Model manifest | `bd7ffc58d253855e3ddc546def711f91812da8cb9ecd88dc658b56aada40ce5e` |
| Weights | `ab722366943c9e090f4f97fa5916bb3bf0da0d42bd04a924e1f30b5b256c030b` |
| ONNX | `effebd8e351d857fcc284558b7c551869eb191ba0eb4f572bbde6e45f7168539` |
| Evaluation | `32d7b224253e2a05f230555db7cfdf4bb6094c81d832d5793851a582a376ee2d` |
| Host qualification | `ad286a54ca1c3311cb1a44818c777de2dd62dca1b8d155b715e076f03710693d` |

## Verification

```bash
cd waved/tools/onnx-hybrid
uv run pytest -q
uv run ruff check .
```

Final regression result after PoC 13F: 187 tests passed; Ruff passed.

## Interpretation

The learned host architecture and quantization boundary are qualified. This
does not prove compilation, NPU execution, NPU residency, or general ONNX
support.

## Next gate

Compile the one fixed `Erf UINT8[481] -> UINT8[481]` graph twice without
executing it.
