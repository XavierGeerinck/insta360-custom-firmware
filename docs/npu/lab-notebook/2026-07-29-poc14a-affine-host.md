# PoC 14A — Learned Affine-plus-Erf Host Contract

**Date:** 2026-07-29
**Device calls:** 0
**Result:** PASS for the host model; deliberately unqualified for compilation
or execution

## Question

Can the CPU-resident learned affine stage from PoC 13 be frozen as a finite,
integer-defined `Multiply -> Add -> Erf` partition without losing the noise
suppression quality gates?

PoC 14A does not ask whether the Wave compiler accepts the partition. It
freezes the mathematical target, its quantizer, its test vectors, and its
package before a native compiler exists.

## Frozen partition

```text
feature: UINT8 [481], scale 0.0625, zero point 128
  -> per-bin Multiply by INT8 [481]
  -> Add INT32 bias [481]
  -> requantize to UINT8, scale 0.03125, zero point 128
  -> Erf
  -> UINT8 [481], scale 0.0078125, zero point 128
```

The selected quantization candidate is:

```text
name:               u8-per-channel
feature:            UINT8 affine-asymmetric
weights:            INT8 symmetric-per-channel
bias:               INT32, feature_scale * weight_scale
rounding:           nearest, ties to even
saturation:         explicit for feature and Erf input
contract SHA-256:   b52ddaf3baed2a7001ff10cbf9ba0600945749a2332f02d71d2ae0be25ac98b8
target product:     0x1000000e
```

The exact integer oracle centers the feature, multiplies by the quantized
weight, adds bias in accumulator units, requantizes with frozen float32
multipliers, and indexes the already qualified 256-entry U8 Erf table.

## Finite candidate registry

The host compared exactly four candidates:

```text
u8-per-tensor
u8-per-channel
i8-per-tensor
i8-per-channel
```

All use feature scale `0.0625`; U8 uses zero point 128 and I8 uses zero point
0. Weights are always INT8. The registry contains no dynamic type, scale,
zero-point, rounding, or saturation choice at inference time.

Validation used 16,384 held-out 481-bin frames. Selection was minimum MSE,
then maximum error, then registry order:

| Candidate | Exact fraction | Max U8 error | MAE | MSE |
|---|---:|---:|---:|---:|
| U8 per tensor | 0.9799457003 | 5 | 0.05120444 | 0.18419382 |
| U8 per channel | 0.9814606665 | 5 | 0.04615527 | 0.16474721 |
| I8 per tensor | 0.9799457003 | 5 | 0.05120444 | 0.18419382 |
| I8 per channel | 0.9814606665 | 5 | 0.04615527 | 0.16474721 |

The independent 32,768-frame test split was confirmation-only:

| Candidate | Exact fraction | Max U8 error | MAE | MSE |
|---|---:|---:|---:|---:|
| U8 per tensor | 0.9802865328 | 5 | 0.05047354 | 0.18160852 |
| U8 per channel | 0.9818325241 | 5 | 0.04532660 | 0.16185984 |
| I8 per tensor | 0.9802865328 | 5 | 0.05047354 | 0.18160852 |
| I8 per channel | 0.9818325241 | 5 | 0.04532660 | 0.16185984 |

U8 per-channel wins by registry order after the U8/I8 numerical tie.

## Six frozen vectors

The qualification bundle contains exactly:

1. zero point;
2. minimum;
3. maximum;
4. repeated ramp;
5. seeded random, seed `346387253`;
6. learned validation frame 5,513.

The binary bundle contains six inputs and six independent oracle outputs.
Its manifest is bound to the model contract and records each tensor digest.

```text
affine-vectors.bin SHA-256:
  fca16527e4efec7f726ceecad1e5d704dff1dd6658ef5062c71900a4948981dd

affine-vectors.json SHA-256:
  5c8b6184c6b2acba4cc6dd82e13df93eecab573de27e915739c7fccb20a4653b
```

## Full held-out evaluation

The new feature-boundary partition was injected into the existing causal
DFN-or-Erf selector. No STFT, noise estimator, synthesis, DFN output, corpus,
metric, or selector implementation was forked.

The run evaluated 1,230 items and rejected none:

| Gate | Result | Threshold | Status |
|---|---:|---:|---|
| Synthetic median SI-SDR improvement | +3.256976 dB | at least +3 dB | PASS |
| Natural median SI-SDR improvement | +5.233915 dB | at least +1 dB | PASS |
| Natural median STOI delta | +0.013537 | at least -0.02 | PASS |
| Noise-only median attenuation | 10.583481 dB | at least 6 dB | PASS |
| Clean-speech median absolute RMS delta | 0.046372 dB | at most 1 dB | PASS |

The evaluation made 740,742 partition calls. The selector chose the learned
Erf candidate 450,140 times and the CPU DeepFilterNet candidate 290,602
times.

The complete evaluation is retained at:

```text
artifacts/npu/runs/poc14a-erf-affine-host-20260729/evaluation.json
SHA-256:
  ee1c5cedbf0a1ab63f311ff07ca3b29b842673d395a90b5d0b9df26a0ec94647
```

## Deterministic package

Two independent builds produced byte-identical copies of all nine files:

```text
artifacts/npu/runs/poc14a-erf-affine-host-20260729
artifacts/npu/runs/poc14a-erf-affine-host-20260729-regen
```

The first build used the independently reproduced DFN `v3-a` evidence; the
second used `v3-b`. Both DFN payloads were already byte-identical. Package
hashes:

| Artifact | Bytes | SHA-256 |
|---|---:|---|
| `affine-config.json` | 1,543 | `a8cdec21a3bee0b2c29b70bcd4adfe0f2fc5beffd9fdd3002b3af12d6800a536` |
| `affine-model.onnx` | 5,983 | `3e8658d482626289443085d8cbeabc0f1a7130b1d5f00cbf8884ee6fb2478a0a` |
| `affine-vectors.bin` | 5,824 | `fca16527e4efec7f726ceecad1e5d704dff1dd6658ef5062c71900a4948981dd` |
| `affine-vectors.json` | 2,469 | `5c8b6184c6b2acba4cc6dd82e13df93eecab573de27e915739c7fccb20a4653b` |
| `affine-weights.npz` | 7,211 | `9e26df0a85570f99add70268a3e7a76dafd49bac263377602de4be490414855a` |
| `erf-u8-lut.bin` | 256 | `c9525963e022d4a4aea0814845119b37488f3727ec8d96684f52e1e2ad776296` |
| `evaluation.json` | 2,042,774 | `ee1c5cedbf0a1ab63f311ff07ca3b29b842673d395a90b5d0b9df26a0ec94647` |
| `source-model.json` | 2,938 | `15096a6ffb680bae6285badc840791be14279b5c50448ba4278a8427ffe5507a` |
| `model-manifest.json` | 3,666 | `99cccb924ab807171ce93b5cba40b3d7ed52219294c0f5d3104b153aa0294263` |

Strict reload reconstructs the quantized model and requires canonical
byte-for-byte reserialization. The ONNX model passes checker validation and
ONNX Runtime agrees with its exact reference graph.

The manifest intentionally remains:

```json
{
  "compiler_qualified": false,
  "execution_approved": false,
  "nbg": null,
  "numerical_evidence_sha256": null
}
```

Host quality is not compiler or NPU evidence.

## Native constants boundary

The selected model also has one fixed little-endian native representation:

```text
magic:               AFC1
schema:              1
width:               481
input type:          UINT8
weight granularity:  per-channel
header bytes:        68
payload layout:
  INT8 weights             481 bytes
  float32 weight scales  1,924 bytes
  INT32 biases           1,924 bytes
  float32 accumulator    1,924 bytes
total:                 6,321 bytes
```

The header embeds the 32-byte model-contract digest. The parser validates
every scalar, vector shape and type, reconstructs the model, recalculates the
contract, and requires canonical byte identity.

```text
artifacts/npu/build/poc14b/affine-constants.bin
SHA-256:
  a2b588be1a4e68a3179a18b51be016115782f923344521c667ca4310c9eeecb4
```

## Reproduction

From the repository root:

```bash
uv run --project waved/tools/onnx-hybrid wave-erf-affine build-host \
  --model artifacts/npu/runs/poc13a-erf-mask-host-20260729-model-v4-selector \
  --examples artifacts/npu/runs/poc13a-erf-mask-host-20260729-examples-v2 \
  --source-evaluation artifacts/npu/runs/poc13a-erf-mask-host-20260729-evaluation-v5-selector-final/evaluation-summary.json \
  --slr31 artifacts/npu/corpora/slr31-peak-v1 \
  --stream artifacts/npu/runs/poc13a-erf-mask-host-20260729-evaluation-stream-v3/evaluation-input.f32 \
  --dfn-output artifacts/npu/runs/poc13a-erf-mask-host-20260729-dfn-output-v3-a.f32 \
  --dfn-run artifacts/npu/runs/poc13a-erf-mask-host-20260729-dfn-run-v3-a.json \
  --output artifacts/npu/runs/poc14a-erf-affine-host-20260729

uv run --project waved/tools/onnx-hybrid wave-erf-affine verify-host \
  --package artifacts/npu/runs/poc14a-erf-affine-host-20260729

uv run --project waved/tools/onnx-hybrid wave-erf-affine \
  export-native-constants \
  --package artifacts/npu/runs/poc14a-erf-affine-host-20260729 \
  --output artifacts/npu/build/poc14b/affine-constants.bin
```

The output paths are non-overwriting. Use a new path for regeneration.

## Decision

PoC 14A passed. It proves a useful, deterministic host partition and CPU
fallback contract. It does not approve a graph for the Wave. PoC 14B must
independently show that the exact Wave producer accepts the frozen tensor
types before any NBG differential can be interpreted.
