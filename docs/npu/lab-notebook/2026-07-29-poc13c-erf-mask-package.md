# PoC 13C — Erf-Mask NBG Package Qualification

## Question

Can the two compiler-produced NBGs be parsed, ownership-mapped, reconstructed,
and bound to the model package without executing them?

## Method

The strict VPMN v2 parser checked:

- target, version, size, and file CRC;
- type table and tensor metadata;
- exact input/output names, shape, type, scale, and zero point;
- section ownership and padding;
- lossless unchanged reconstruction;
- A1/A2 identity;
- package, NBG, and structural-signature cross-hashes;
- corruption and mismatch negatives.

## Corrected assumption

The plan expected section-20/section-26 DYNAIO records because PoC 12C
recovered that relationship in other Wave v2 graphs. The external-input Erf
artifact instead has `dynamic_io: null`.

This is a format finding, not a parser waiver: both independent captures have
the same structure, all bytes have lossless ownership, the tensor contract is
present elsewhere in the container, and the CRC validates. DYNAIO is
conditional, not universal.

## Result

```text
status: PASS
magic/version/product: VPMN / 2 / 0x1000000e
bytes: 5,840
file CRC: 0xf14f321a
NBG SHA-256: 96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94
structural signature: 236bd56d2f3aee5bf0c370b8372dda937c5af1b8d37a6e224b21df82db85088a
padding nonzero bytes: 0
lossless round trip: exact
device calls: 0
general compiler qualified: false
```

Tensor names:

```text
input/output[0]
uid_1_out_0
```

## Evidence

The tracked profile is
`docs/npu/format/erf-mask-nbg-profile.json`, SHA-256
`9e5300363c5eca8bd9dae6db820128d4e9534f2aa6007a69fe10bf7b9bc9cab7`.

## Verification

```bash
python3 waved/scripts/npu/nbg_erf_mask_profile.py \
  --a1 artifacts/npu/runs/poc13b-erf-mask-a1-20260729/a1.nbg \
  --a2 artifacts/npu/runs/poc13b-erf-mask-a2-20260729/a2.nbg \
  --model artifacts/npu/runs/poc13a-erf-mask-host-20260729-model-v4-selector
```

The profile writer is exclusive; use a new output during independent
reproduction.

## Interpretation

The exact NBG is package-qualified and safe to advance to bounded numerical
execution. This does not approve arbitrary byte mutation or general NBG
serialization.

## Next gate

Run six fixed numerical vectors once and compare all 481 elements per vector
with an independent CPU oracle.
