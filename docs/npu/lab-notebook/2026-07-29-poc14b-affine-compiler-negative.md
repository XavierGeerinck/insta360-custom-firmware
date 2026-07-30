# PoC 14B/C — Wave Affine Compiler Profile and Clean Negative

**Date:** 2026-07-29
**Device compile-only processes:** 1 of the maximum 6
**Graph executions:** 0
**Result:** clean negative at the TIM-VX type checker; PoC 14C remains blocked

## Question

Can the exact Wave TIM-VX stack compile the frozen PoC 14A
`Multiply -> Add -> Erf` graph so controlled weight and bias differentials
can reveal coefficient packing and executable-record ownership?

## Static precondition

The fixed native candidate used only public TIM-VX graph construction:

```text
tim::vx::ops::Multiply
tim::vx::ops::Add
tim::vx::ops::Erf
tim::vx::Graph::CompileToBinary
```

The exact `libtim-vx.so` disassembly maps these wrappers to built-in ovxlib
nodes:

| TIM-VX object | Constructor | Built-in opcode |
|---|---:|---:|
| `Add` | `0x000c3770` | 0 |
| `Multiply` | `0x000c3870` | 1 |
| `Erf` | `0x000c5460` | 151 |

`BuiltinOpImpl` reaches `vsi_nn_AddNode`. `GraphImpl::CompileToBinary`
performs graph setup and calls `vsi_nn_GenerateNBG`; the native candidate
contains no `Graph::Run`, tensor copy, NBG load, raw ioctl, direct galcore
open, or dynamic loader path.

The exact source tree provides two relevant static facts:

1. `op_check_mul` delegates its two input types to the ADD checker.
2. the ADD checker contains a finite table of 59 accepted input-type pairs.

The OpenVX backend registers both operations:

```text
add -> vxTensorAddNode
mul -> vxTensorMultiplyNode
```

These facts predicted a normal, non-CLC producer path but did not guarantee
that the selected per-channel quantization was admitted.

## Fixed candidate and call ledger

One executable exposed only:

```text
--a1 / --a2  identical frozen baseline
--b1 / --b2  weight[17] 127 -> 126
--c1 / --c2  bias[17] -4773 -> -4772
--describe    metadata only
```

Each candidate had a separate final and partial path below:

```text
/tmp/wave-npu-poc14b/output/
```

The compiler embedded the exact 6,321-byte PoC 14A constants blob and
validated its `AFC1` header and model-contract digest before graph creation.
The graph was fixed to:

```text
input:       UINT8 ASYMMETRIC [481], scale 0.0625, zp 128
weights:     INT8 SYMMETRIC_PER_CHANNEL [481], channel 0
product:     INT32 SYMMETRIC_PER_CHANNEL [481], transient
bias:        INT32 SYMMETRIC_PER_CHANNEL [481], constant
Erf input:   UINT8 ASYMMETRIC [481], scale 0.03125, zp 128
Erf output:  UINT8 ASYMMETRIC [481], scale 0.0078125, zp 128
```

The process had a 15-second in-process alarm, parent-death signal, exact
chip/customer checks, fixed clean environment, exclusive 0444 output
installation, and VPMN/version/product validation. The host added a
20-second ADB deadline, artifact hashes, output absence, CLC absence,
holder, kernel, and postflight gates.

## Reproducible native build

```text
binary:
  artifacts/npu/build/poc14b/wave-npu-compile-erf-affine-v1
  eab129f936cd3fc22142acd7abe55d9ecd276d1a2d7e76d0c647acf04af75fe9

source:
  waved/tools/npu-compiler/tim_vx_compile_erf_affine.cc
  70ac7317f5e84a864b1f757e1b139abc60927327648577386e42337a61cd1e31

constants:
  artifacts/npu/build/poc14b/affine-constants.bin
  a2b588be1a4e68a3179a18b51be016115782f923344521c667ca4310c9eeecb4

libtim-vx.so:
  d2c8fc7aaf0ad0b86f02fcd54bb1cdca9f04967b5cd9efa61a211565b453375b
```

Two independent builds produced the same binary digest. The policy checker
confirmed:

- AArch64 ELF64 PIE;
- full RELRO, immediate binding, non-executable stack;
- fixed `/tmp/wave-npu-poc14b/lib:/usr/lib` runpath;
- exactly one `Multiply`, one `Add`, and one `Erf`;
- exactly two `CompileToBinary` source calls;
- embedded constants start/end symbols;
- no execution or tensor-I/O imports.

## Live A1 observation

Run:

```text
artifacts/npu/runs/poc14b-a1-erf-affine-20260729
```

The process passed the exact identity and artifact gates. It created the
TIM-VX graph and stopped while checking the first node:

```text
OVXLIB_VERSION==1.2.15
Sort graph nodes.
Setup node id[0] uid[1] op[MULTIPLY]
Inputs/Outputs data type not support: ASYM UINT8, SYMM PC INT8
Check node[0] MULTIPLY fail
wave_npu_poc14b: size compile failed
```

Result:

```text
native exit:         5
host ADB timeout:    false
host duration:       74 ms
NBG final output:    absent
NBG partial output:  absent
CLC paths:           all absent
health differences:  none
kernel alerts:       none
galcore holders:     none
```

Evidence hashes:

| Evidence | SHA-256 |
|---|---|
| `run.json` | `f9ac0d50662d1c6658c2d0d608711dbe0306aa8f03ac0c2aeb533fca5155a574` |
| `stdout.txt` | `7f8e2b4058986af91472476eb8513b202fedba2124307fa22678efb50adbe575` |
| `preflight.json` | `02a6e39325a31327cb3e1124c3da58b6a3e624e11508b2b39079d445d4098f55` |
| `postflight.json` | `f6135d088784ed7c38f7d87b1fcdc16e95e0f240944b0b3e73381992b169bf10` |

The staged root was subsequently verified to contain only the three
hash-pinned payload files and an empty output directory, then only
`/tmp/wave-npu-poc14b` was removed. The root is now absent.

## Source/binary correlation

The device reported:

```text
function: op_check_add
line:     465
types:    ASYM UINT8, SYMM PC INT8
```

The pinned source's ADD checker spans lines 390–473; its diagnostic is line
466, a one-line source/build offset. Its 59-pair table contains no
per-channel quantization entry. In normalized form, the observed signature
is:

```text
D_U8|Q_ASYM, D_I8|Q_SYM_PC
```

That pair is absent. Because Multiply explicitly delegates to ADD, the
source predicts the exact observed rejection.

Machine-readable evidence:

```text
docs/npu/profile/affine-elementwise-compiler-profile.json
SHA-256:
  fd9472889a3db8160a2acf5283c03c8a8f602314e7beb0d6dd3a07fbd801cbfa
```

The generator records all 59 accepted pairs:

```bash
python3 waved/scripts/npu/affine_compile_profile.py
```

## Why A2/B/C were not run

A2 is the exact same graph and constants as A1. B1/B2 and C1/C2 alter only a
payload coefficient after constructing the same rejected tensor types.
They cannot change the operation checker result.

Running them would therefore be an exact retry or a known-equivalent retry,
not an independent controlled differential. The approved rule says a failed
pair is retained and not retried with an unreviewed graph mutation.

Call accounting:

| Candidate | Planned role | Result |
|---|---|---|
| A1 | baseline | consumed; clean negative |
| A2 | identical baseline | skipped; prohibited retry |
| B1/B2 | one weight differential | skipped; same rejected type signature |
| C1/C2 | one bias differential | skipped; same rejected type signature |

## PoC 14C consequence

No NBG exists, so there are no A/B/C byte ranges to correlate. The following
remain unqualified:

- affine coefficient packing;
- weight/bias alignment and padding;
- operation descriptor emission;
- command/state ownership;
- affine patch and relocation ownership;
- semantic emission;
- any affine execution artifact.

The open serializer must continue to refuse semantic affine emission.
PoCs 14D–14H remain blocked by their prerequisite ladder.

## What the negative does and does not prove

It proves:

- the selected useful PoC 14A quantizer is outside this TIM-VX elementwise
  checker's admitted input-type table;
- rejection occurs before backend selection, NBG generation, or execution;
- no compiler differential can be obtained for the frozen contract through
  this exact public path.

It does not prove:

- the VIP9000 hardware cannot multiply per-channel INT8 values;
- every private Acuity compiler rejects the model;
- all elementwise graphs are impossible;
- a U8/U8 lowering exports successfully.

The last distinction matters. U8/U8 is present in the type table, but PoC 5B
already selected OpenVX `add` for U8/U8 and then failed at
`vxGenerateNBG`. Type admission is not serializer support.

## ONNX and hybrid-runtime implication

This result does not change the general ONNX policy:

- ONNX Runtime CPU remains the general implementation;
- hybrid mode may offload only hash-pinned, independently qualified
  partitions;
- the already qualified U8 Erf partition remains valid and provides working
  NPU-assisted noise cancellation;
- `Multiply`, `Add`, Conv, and arbitrary ONNX graphs remain unapproved for
  Wave NPU placement.

The working model therefore remains:

```text
CPU feature extraction + learned affine
  -> Wave NPU Erf
  -> CPU mask application + selector + synthesis
```

PoC 13G also shows why this is a correctness/residency research result, not a
latency win: the Wave CPU lookup was about 20.69 times faster than NPU
`Run()` for the tiny Erf partition.

## Reproduction

The live A1 capture is append-only and must not be replayed.

Host-only checks:

```bash
waved/tools/npu-compiler/build-device-erf-affine.sh

waved/tools/npu-compiler/verify-device-erf-affine-policy.sh \
  artifacts/npu/build/poc14b/wave-npu-compile-erf-affine-v1 \
  waved/tools/npu-compiler/tim_vx_compile_erf_affine.cc \
  artifacts/npu/build/poc14b/affine-constants.bin

python3 waved/scripts/npu/affine_compile_profile.py

python3 -m unittest \
  waved.scripts.npu.tests.test_affine_compile_profile \
  waved.scripts.npu.tests.test_npu_erf_affine_compile_lab
```

Historical live sequence, for audit only:

```bash
waved/tools/npu-compiler/stage-device-erf-affine.sh

python3 waved/scripts/npu/npu_erf_affine_compile_lab.py run a1 \
  --run-id poc14b-a1-erf-affine-20260729

waved/tools/npu-compiler/cleanup-device-erf-affine.sh
```

Do not repeat that sequence without a new reviewed target, new remote root,
new call ledger, host quality evidence, and static evidence that addresses
both the type checker and the known Add exporter failure.

## Decision

PoC 14B is a clean negative for the frozen per-channel affine contract.
PoC 14C cannot start. The current safe endpoint is the already working
hybrid CPU-affine/NPU-Erf denoiser plus CPU fallback. A future affine attempt
must first qualify a new lowering on the host and explain why it reaches a
producer path that can actually export an NBG; changing only tensor types is
not enough.
