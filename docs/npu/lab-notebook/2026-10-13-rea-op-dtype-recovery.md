# Wave NPU operator and dtype recovery with REA

**Date:** 2026-10-13

**Status:** Static findings recovered; stock CTC execution passed; resulting FP16 Conv/U8 FC controls passed [PoC 16](2026-10-13-poc16-native-fp16-conv-u8-fc.md).

**Scope:** Exact Wave OpenVX target selection and Conv/FC dtype admission.

The Wave still runs its stock CTC model through the existing NPU control.
REA also recovered a concrete reason that the earlier FP32 Conv probe cannot
use the NN target: `vxnneIsNNSupportFormat` rejects a non-null FP32 weight
tensor when an output tensor is present. FP16 and quantized weights have
conditional admission paths. The subsequent approved PoC 16 compiled and
executed both fixed controls on the NN target with exact CPU-oracle output.

## Questions and results

| Question | Result |
|---|---|
| Does the known model still execute? | Passed: three CTC iterations match the July digest and leave captured state unchanged. |
| Why are FP32 weights a poor custom-Conv control? | Static proof: the exact NN format checker rejects weight type `0x00a`. The Conv initializer returns `-3` when that check fails. |
| Which replacement signatures are worth testing? | FP16/FP16 and asymmetric U8/U8 are present in both firmware Conv and FC admission tables; hardware/context gates still apply. |
| Did the replacement controls work? | Yes, in PoC 16: FP16 FIR Conv and asymmetric-U8 FC exported native NBGs and each passed one exact oracle execution. |
| Does this unlock general custom models? | Broader graph, shape and ONNX coverage remains unqualified. |

## Tool and artifact identity

REA CLI/MCP version: `6.4.0`. Analysis provider: Ghidra `12.1.4` on macOS
ARM64 with OpenJDK `21.0.12.1`. Ghidra was upgraded through Homebrew with the
owner's explicit authorization. REA used session-local environment settings:

```text
GHIDRA_INSTALL_DIR=/opt/homebrew/opt/ghidra/libexec
JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home
```

The analyzed `firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libOpenVX.so`
is 6,422,744 bytes, SHA-256
`f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb`.
A read-only `adb shell sha256sum /usr/lib/libOpenVX.so` returned the same
digest today. The companion `libovxlib.so` remains
`09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9`.

Complete REA Evidence is retained at
`artifacts/npu/rea-20261013/openvx-evidence-bundle.json`: 22 records,
1,525,209 bytes, SHA-256
`df18779be728c6bcc05110b388292087d3aec61b6f6367c9b8b87404f8cf8a7f`.
The three retained unknowns concern inferred private function signature
types. No reconstructed private API was called. The native session was
closed after export.

## NN format rejection and target selection

Ghidra rebases this ELF by `0x100000`. Addresses below are analysis
addresses; subtract that base for these file-backed ELF virtual addresses.

| Function | Ghidra address | Evidence ID |
|---|---|---|
| `vxnneIsNNSupportFormat` | `0x44c1b0` | `ev_a84bc68ce8e7f2b255fec669c8d4b46f9f0606e62dbc039adee310637877966e` |
| Assembly of the same checker | `0x44c1b0` | `ev_1f40c36ae9a10994efb8133ce7ab759deffb5824cd24a63523c15b4845a3c252` |
| `_vxnneConvolutionReluPoolingInitializer` | `0x3d0850` | `ev_2b27c6e7f53386bd325f1fe27cc0f0fb52ebcabe1745199c82db1e0930491a61` |
| `vxoNNFullyConnectedLayerInitializer` | `0x367a40` | `ev_834bc52ba6afb1eef45bf7b61238e3277e87bee7ff61b1ebc28fdf4787f27e88` |
| `vxnneIsTPSupportFormat` | `0x44c9b0` | `ev_b8e8724b2a9a19148d5ce2b81765cc1f8b7ffb47a5d6101dc474a572eb7cc9d1` |
| `vxnneGetOperatorTargetName` | `0x448dc0` | `ev_450837e4ccd021df836615adb47f15af0a8c624b772280ac68850f1dc3be1bb4` |
| `vxoNNBatchGemm_NN_Support` | `0x3cd060` | `ev_22d97570ffab83f7c0ccfe65b3ccbaa0a0a489d8411bdea08faf1e8c5858240b` |

The checker reads the input, weight and output types from tensor offset
`+0x200`; its callers pass the weight descriptor's tensor at `+0xbb8`.
Assembly independently shows the weight argument copied to `x19`, then
its type loaded into `w24` at `0x44c22c`. For non-null weight/output
tensors, `0x44c3d8` through `0x44c3f4` dispatch on that weight type:
`0x00a` reaches the `0x00f` comparison and branches to the false-return
path. This establishes the FP32-weight rejection without relying on the
decompiler's private C parameter types.

The Conv initializer calls this checker before allocating its operation
layer. A false result prints `hw not support this format` and returns
`-3`. Its admitted path initializes an NN operation with target value `2`.
The FC initializer considers TP and NN paths and also checks dimensions,
weight preparation and alignment. Its admitted NN branch likewise uses
target `2`. A TIM-VX log saying `Instance OPENVX node` alone does not
identify one of these hardware targets.

The target-name function establishes:

| Value | Runtime target name |
|---:|---|
| 1 | `VXNNE_OPERATION_TARGET_SH` |
| 2 | `VXNNE_OPERATION_TARGET_NN` |
| 3 | `VXNNE_OPERATION_TARGET_TP` |
| 4 | `VXNNE_OPERATION_TARGET_SW` |
| 5 | `VXNNE_OPERATION_TARGET_SC` |

FP16 input additionally depends on a nonzero context flag at
`+0x2e7954`; the remaining output/weight checks consult HAL features.
Asymmetric U8 input consults context feature `0x1b`. HAL feature `0x2ac`,
when enabled, rejects U8 weights early. These live feature values were not
queried directly. PoC 16's live target logs establish admission for its two
fixed graphs; other shapes and signatures remain conditional. BatchGemm has
another feature gate `0x2ca` and shape
checks; it is not equivalent to the dedicated FC path.

## Firmware dtype admission

The exact ovxlib tables were correlated byte for byte with the existing
TIM-VX checkout `702715d714277376d7ff466b7b72833ff57564c1`. These are
input/weight pairs; they do not validate output or bias types.
Raw tables, relocations, source identities and the recovery commands are
retained in `artifacts/npu/rea-20261013/ovxlib-dtype-tables.json`,
60,456 bytes, SHA-256
`cac1205c374b8d1858d3741c3f362e4e82535410d802c4ab044b4b665bcd12d7`.

| Table | ELF VA/file offset | Rows | Table SHA-256 |
|---|---|---:|---|
| Conv2d | `0x4e1890` | 76 | `e9d4cf806b62e0719b26b1e2bd6ec128887324bc8e6bef2ad61c899e1412b790` |
| FC | `0x4e44f0` | 69 | `d1149e36ddc8e57d61eb70c50085cacc40b81ee376157646cac2395715e8f11e` |

Each row contains two little-endian 32-bit signatures. The shared
validator packs each as `vx_type | (qnt_type << 8)`. Both tables contain
FP16/FP16 (`0x000f, 0x000f`), asymmetric U8/U8
(`0x0203, 0x0203`) and asymmetric U8/per-channel symmetric I8
(`0x0203, 0x0302`). Both also admit FP32/FP32 at this earlier layer,
which explains why node creation did not expose the later NN rejection.
The checkers run `vsi_nn_QuantCheck` before their dtype table checks.

The TIM-VX Conv and FC wrappers convert a constant FP16 bias to FP32.
The FP16 control therefore declares its zero bias as FP32; an all-FP16
description including bound bias is inaccurate.
TIM-VX enum values also differ from the OpenVX values: TIM-VX FLOAT16 is
`8`, OpenVX FLOAT16 is `15`; TIM-VX UINT8 is `2`, OpenVX UINT8 is `3`.

## Live stock control

Exactly one runner invocation was executed:

```bash
python3 waved/scripts/npu/npu_lab.py run stock-ctc \
  --artifact-root artifacts/npu/runs --run-id rea-stock-ctc-20261013
python3 waved/scripts/npu/npu_lab.py compare \
  artifacts/npu/poc0-first-control-20260728/envelope.json \
  artifacts/npu/runs/rea-stock-ctc-20261013/envelope.json
```

The runner returned `success`, `PASS`, exit `0`, three iterations and
cleanup `0`. Each output contained 332,800 bytes with SHA-256
`e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4`.
The comparator returned `equivalent`. Iteration times were
3.554, 2.382 and 2.203 ms, mean 2.713 ms; these three observations are
a control result, not a new performance benchmark.

The raw envelope is
`artifacts/npu/runs/rea-stock-ctc-20261013/envelope.json`, SHA-256
`e597105084794e1464791cf03cf1a759eb525c3cc7004cf54bb18ab4033205a3`.
ADB remained responsive; artifact hashes and NPU parameters were unchanged;
holder scans were empty before and after. `waved` and its watchdog stayed
running. Empty scans describe those instants, not continuous exclusivity.

## Approved follow-up completed

The owner approved the fixed compile, qualification and execution chain
with `go`. Its [design](../../superpowers/specs/2026-10-13-wave-npu-fp16-u8-probes-design.md)
and [plan](../../superpowers/plans/2026-10-13-wave-npu-fp16-u8-probes.md)
fixed these two independent candidates:

| Candidate | Tensors and constants | Numerical control |
|---|---|---|
| FP16 FIR Conv | Input/output `[16,1,1,1]`; FP16 weights `[3,1,1,1] = [0.25,0.50,0.25]`; FP32 zero bias `[1]`; SAME padding and stride `[1,1]`. | Alternating `+1,-1` input gives `[0.25,0,...,0,-0.25]`; all values are exactly representable in FP16. |
| U8 FC | Input `[16,1]`, weights `[16,4]`, output `[4,1]`; per-tensor asymmetric scale `1`, zero point `0`; each output selects one input at indices `0,5,10,15`; INT32 zero bias `[4]` with scale `1`. | Input `[0,1,...,15]` gives `[0,5,10,15]`, without rounding or saturation. |

PoC 16 used separate bounded processes, fixed paths and hash-bound manifests
with the exact device runtime dependencies. One size-query and one export
per candidate produced 1,800-byte and 2,056-byte native VPMN v2 NBGs for
`0x1000000e`; both target logs explicitly identify the NN engine. All tensor
contracts and CRCs passed host qualification. Each NBG then ran exactly
once with a device-output canary and guarded readback; all 32 FP16 bytes
and all four U8 bytes matched their independent CPU oracles.

The historical `waved/target/tim-vx/libtim-vx.so` was absent, so the approved
work built and identified a new wrapper against the exact Wave SDK. The
adjacent SDK runtime was not substituted. The comment in
`waved/scripts/build-tim-vx.sh` claiming CLC is folded into OpenCL is
superseded by the July missing-CLC evidence. The quarantined CLC remained
excluded. No device health fault was observed, and all four new staging
roots were removed. Exact model/runtime/binary/manifest identities and raw
run digests are recorded in the [PoC 16 notebook](2026-10-13-poc16-native-fp16-conv-u8-fc.md).
