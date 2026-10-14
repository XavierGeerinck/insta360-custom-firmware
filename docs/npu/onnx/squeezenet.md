# SqueezeNet1.1 compatibility on the Wave

Three real trained SqueezeNet controls qualified on the Wave NPU on
2026-10-14: spatial convolution, MaxPool and a Fire block with channel
concatenation. The classifier graph failed during initialization of its
13×13 average-pooling kernel. The complete model and paired benchmark were
therefore skipped. SqueezeNet is **partially compatible, not fully qualified**.

## Actual native results

| Control | Trained graph | Result | Independent reference agreement |
|---|---|---|---|
| 20a | First Conv3/stride2 + Relu; non-square input `[1,3,17,13]` | NN export and one Run passed | 3,072 finite elements, zero mismatches; maximum error 0.001953125 |
| 20b | MaxPool3/stride2, `[1,64,15,19]` → `[1,64,7,9]` | NN export and one Run passed | 4,032 elements, byte-for-byte agreement |
| 20c | First Fire block: squeeze Conv, parallel expand branches, Relu, channel Concat | NN export and one Run passed | 8,064 finite elements, zero mismatches; maximum error 0.001953125 |
| 20d | Classifier Conv1, Relu, GlobalAveragePool, Flatten | Size-only export failed, remote exit5 | No NBG or native output |
| 20e | Complete trained 65-node graph, four frozen images | Skipped after20d | No full-model native parity or timing measurement |

Every qualified element must satisfy the policy frozen before device work:
`abs(error) <= 0.03 + 0.015 * abs(reference)`, with finite outputs and NaN
canaries. The authoritative reference is independent NumPy FP32 accumulation
with declared FP16 tensor rounding. Actual host ONNX Runtime outputs passed
the same policy separately; their bytes are retained independently.

20a proves the tested NCHW/WHCN mapping using distinct non-square dimensions.
20c preserves both expand branches' order when ONNX channel Concat axis1
becomes native axis2. All successful export logs select NN, with no CPU/SH
neural fallback. The pool-only control could admit TP, but its actual route
was NN. These results qualify the exact signatures, not arbitrary geometries.

The selected ceiling-mode MaxPool geometries give the same output as floor
mode. This experiment does **not** qualify general ceiling behavior. The
classifier lowers GlobalAveragePool to public TIM-VX AVG Pool2d, kernel and
stride13×13, zero padding, FLOOR, followed by an explicit reshape. Its input
is FP16 `[13,13,1000,1]`; pooling output is `[1,1,1000,1]`.

The raw20d log reports initialization failure for
`vivante.nn.pooling.layer2`, status−1, then a size-only compile failure. The
first `CompileToBinary(nullptr, &size)` returned false, so the data-export
call never occurred. Vendor teardown also emitted two force-release
reference diagnostics. No NBG was produced, no inference ran, captured
health stayed unchanged, and cleanup removed the staging root. This does
not identify a universal average-pooling limit or prove which size, stride,
channel count or backend parameter caused the rejection.

See the [result ledger](squeezenet-poc20-results.json),
[experiment notebook](../lab-notebook/2026-10-14-poc20-squeezenet.md) and
[frozen design](../../superpowers/specs/2026-10-14-wave-squeezenet-compatibility-design.md).

## Trained model and reproducibility

The source is [torchvision0.20.1 SqueezeNet1.1](https://github.com/pytorch/vision/blob/v0.20.1/torchvision/models/squeezenet.py),
with its published ImageNet checkpoint. All52 state tensors and1,235,496
parameters match the exported original FP32 bits; FP16 conversion is explicit.
The eval graph has26 Conv,26 Relu,8 Concat,3 MaxPool,1 GlobalAveragePool
and1 Flatten, with no Dropout. Batch1 input is `[1,3,224,224]`; output is
`[1,1000]`. Inline FP16 parameters occupy2,470,992 bytes.

Four real images and preprocessing are frozen: RGB, resize256 with bilinear
antialiasing, center crop224, ImageNet normalization, Pillow11.3. Original
FP32, actual ORT FP16 and independent NumPy outputs pass the predeclared
logit/probability/top-k policy on all four images **on the host**. None of
these four full inputs ran on the NPU.

| Artifact | SHA-256 |
|---|---|
| Published checkpoint | `b8a52dc049b60e4b6ab68ad0df457362afab8b6304b2febdc1650a5dab4d7e7b` |
| Full FP16 ONNX | `f5ac6b08b8304372c06fb897c2cad63e0c30191e9bc866680b1777e5bb62e081` |
| Original FP32 ONNX | `76d825183eba140d2462759837ccb7c3802b9da807399ba7138cdad90c3af541` |
| Frozen32-file preparation | `848c42d1c0bfd704d4f19ab40d573ea04379f26b7fc73ba5c3a532c474367f3c` |

The bounded importer is `fp16-squeezenet-v1`. It rejects unsupported
operators, attributes, shapes and unconsumed coefficients. Importability
does not confer native qualification. The retained20d and20e packages
refuse strict native execution and uncompiled qualification locally, before
phase activation, a CPU neural session or device contact.20a–20c reload as
qualified packages without CPU sessions or ADB; twelve copied-artifact
tamper cases refuse. Original packages and earlier source bytes remain intact.

## Device continuity and closure

The new budget was6 compiler processes,12 public export API calls and30 NPU
Runs. Actual use was **4 processes,7 API calls and3 Runs**, producing3 NBGs.
There was no native retry. Full-model and benchmark allocations were unused;
unfinished conditional benchmark source was removed. No system installation
update or production integration was required.

A boot change and Wi-Fi warning during host preparation occurred before any
SqueezeNet vendor call. That failed health comparison remains retained; the
cause of the boot change is unknown. A separately reviewed206.882-second
disconnected quiet interval admitted the current boot's phase. Every staging,
immediate preflight, native operation and CLEAN cleanup has continuous
diagnostic evidence. The final read-only phase check added event36 and left
its literal state ACTIVE; no native operation remained and the experiment
closed without silently reseeding or advancing it.

The first final raw comparison flagged different temporary-root CLC-absence
keys between the seed and final snapshots. Its FAIL remains preserved. A
separate assessment compares the identical three system CLC paths and
checks temporary roots independently: all38 roots are absent, pinned hashes,
boot, services and parameters match, no lab processes/galcore holders remain,
and seed-to-final/post-restoration diagnostics have no new alerts.

Wi-Fi is verified restored to the original saved network, COMPLETED/adminUP
with an assigned IP; no configuration was saved.381 host tests and5 subtests
passed. Raw artifacts remain under `artifacts/npu/poc20/` and
`artifacts/npu/runs/rea-poc20*`.

The next spatial question is the exact rejected AVG signature and a proven
equivalent lowering, under a new bounded experiment. For audio, the
[speech-enhancement shortlist](speech-enhancement-candidates.md) recommends
DPDFNet for full-band noise reduction and LocalVQE/DTLN-AEC for echo
cancellation, with GTCRN as the smaller recurrent compatibility control.
Their Wave performance and native compatibility remain unmeasured. VAD
integration stays deferred under root backlog C20.
