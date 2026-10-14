# Trained MarbleNet VAD on the Wave

The Wave runs the complete trained MarbleNet model on its NPU through an
exact explicit-padding rewrite. The full graph qualified on 2026-10-14:
all 800 frame decisions across speech, silence, noise and noisy speech match
the original FP32 reference. The maximum speech-probability difference is
0.000815. Strict WAV execution uses the native backend with zero CPU neural
sessions. This is a fixed-window model qualification; streaming integration
requires separate work.

This is [NVIDIA Frame-VAD Multilingual MarbleNet v2.0](https://huggingface.co/nvidia/Frame_VAD_Multilingual_MarbleNet_v2.0),
with a source-matched reconstruction of the serialized NeMo checkpoint.
The complete NeMo framework was not executed. All 84 state tensors and
91,378 learned parameters are accounted for; batch normalization uses the
checkpoint's epsilon 0.001. Its folded neural graph contains 21 Conv,
9 Relu and3 Add nodes, with the classifier represented by an equivalent
1×1 convolution.

## What was measured

| Control | Native result | Numerical result |
|---|---|---|
| 18A: trained pointwise Conv, folded BN, Relu | NN export and one Run passed | max activation error 0.001953125 |
| 18B: first trained residual block | NN export and one Run passed | max activation error 0.00048828125 |
| 18C: trained depthwise K29, rate 2, pointwise, Relu | size-query export rejected, exit 5 | no native output |
| 18D: complete trained graph | skipped after 18C | no full-model NPU output |

PoC19 isolated the rejection and qualified a compatible trained layer:

| Control | Native result | Numerical result |
|---|---|---|
| 19a: diagnostic centered K13, rate2 | NN export and one Run passed | max error0.000244140625 |
| 19b: diagnostic K29, rate1 | NN export and one Run passed | max error0.000244140625 |
| 19c: exact zero Pad28, original K29/rate2 VALID, pointwise/Relu | TP padding + NN convolution export and one Run passed | max error0.000244140625 |
| 19d: original first trained K11/stride2 block | NN export and one Run passed | max error0.001953125 |
| 19e: original remaining K15/K17 residual blocks | NN inference passed in the fresh 2026-10-14 phase; old export interval remains failed | max error 0.00048828125 |
| 19f: complete padded graph | one native export and four clip Runs passed; 24 NN + 1 TP routes | max activation error 0.00390625; 800/800 FP32 decisions agree |

19a/b are diagnostic changes, not full-layer equivalents.19c preserves every
trained coefficient: `[1,64,200]` receives28 positive-zero samples on each
side, then the original29-tap/rate2 filter uses VALID pads0 to return200
frames. Its padding runs on TP and its convolutions on NN.19d uses equivalent
native2×2 strides only for height1/kernel-height1/height-pad0 depthwise Conv.
These changes have their own bounded `fp16-marblenet-pad-v1` profile.

The candidate full graph has34 nodes, adding one Pad. All34 folded initializer
tensors/90,418 elements retain their exact180,836 FP16 bytes. Its CPU FP32
and FP16 outputs match the original folded exports byte-for-byte on all four
clips. Full frontend admission reverses the deterministic rewrite to the
pinned original full-model hash; a same-shape graph or resealed weight receipt
cannot pass. Actual full-model native inference subsequently passed all four
frozen clips, independently of that host equivalence proof. The qualified
NBG is 232,384 bytes, SHA
`69eddc1ac45e2016f5adee2f53de6587fcc564eaaf90c4720e316b1d7e2b53f9`.

The new USB warning occurred after19d cleanup and before19e staging.19e's
local before/after check began after it, so local PASS did not clear the failed
inter-run interval. Further native calls stopped when that gap was detected.
The selected device subsequently disappeared from ADB; that Wi-Fi
restoration attempt could not reach it. The owner reconnected it on 2026-10-14,
beginning a new boot with the same pinned libraries/kernel/NPU configuration.
A new Wi-Fi warning stopped the first host attempt before device staging or
any NPU Run. Its HALTED phase is retained unchanged. A separately reviewed
525-second disconnected quiet window admitted the successful new phase;
neither earlier failure was reclassified. See the
[PoC19 ledger](marblenet-poc19-results.json) and
[compatibility notebook](../lab-notebook/2026-10-13-poc19-marblenet-compatibility.md).

Final seed-to-device diagnostics and pinned state checks passed, with all 28
temporary staging roots absent. Wi-Fi was restored to the original saved
network, verified COMPLETED/adminUP with an assigned IP; its separate
post-restoration snapshot also passed with no new alerts.

New Pad-profile launches now require a reviewed persistent host phase under
the package family. The guard checks inter-run continuity before staging
and immediately before native execution, and advances only from verified
CLEAN cleanup. No production phase has been seeded. This guard assumes one
controller; it does not provide interprocess locking or automatic recovery.

Every native activation element must be finite and satisfy
`abs(error) <= 0.02 + 0.02 * abs(reference)`. Actual NPU, actual ORT CPU and
independent NumPy outputs have separate byte/hash receipts. The older
PoC17 profiles retain their byte-exact policy. New FP16 output buffers are
primed with NaN before Run, so unwritten elements fail the numerical gate.

18C's raw log identifies node 0 `depthwise_conv2d`, followed by failure to
initialize `vivante.nn.convolution.layer` with status -1. No NBG was produced.
Captured device health stayed unchanged and the staging root was removed.
Vendor teardown emitted force-release warnings. This establishes rejection
of this exact signature, rather than a universal kernel-size or dilation
limit. Native dilation is a **rate**: TIM-VX `{2,1}` represents temporal
dilation 2; the OpenVX gap is 1.

The full host hybrid frontend ran speech, silence, seeded noise and derived
noisy speech. All 800 decisions at threshold 0.5 agreed with the original
source-matched FP32 reference; maximum speech-probability error was0.000728.
These are CPU parity results, not full-model NPU results or labeled VAD
accuracy. Only the public speech sample is real audio; the other clips are
synthetic or derived.

The independent Wave CPU-only baseline passed 36 calls (4 warm,32 measured)
using installed ORT1.15.1, one thread and the full FP32 graph. Its output
matched the host folded FP32 reference byte-for-byte; all 200 speech
decisions agreed with the original reconstruction. Median ORT Run was
**8.55ms**; input-copy/Run/softmax boundary was **8.63ms** (p95 **8.82ms**)
for each 3.99-second window. The baseline and timings are recorded in the
[result ledger](marblenet-poc18-results.json) and
[PoC18 notebook](../lab-notebook/2026-10-13-poc18-trained-marblenet.md).
Its prepared-feature timings include input copying, ORT Run and softmax;
they exclude output priming, result validation, feature extraction and
file/ADB orchestration. The later paired PoC19 benchmark is recorded separately.

The completed PoC19 paired benchmark ran 36 CPU/native pairs in one process,
with 4 warm-up and 32 measured pairs, alternating order. CPU uses the original
folded FP32 graph and one-thread stock ORT1.15.1; native uses the qualified
FP16 graph with exact output bytes on every iteration. All pairs repeat the
same frozen 3.99-second speech window; the four-clip tests establish parity.

| Measurement | CPU median / p95 ms | Native median / p95 ms |
|---|---:|---:|
| Run |8.577104 /9.252773|0.690792 /0.814966|
| Prepared-feature boundary |8.664375 /9.331614|0.833188 /0.993871|
| Calling-thread CPU time |8.545312 /8.760459|0.387897 /0.461373|

The boundary is **10.399× faster**, with **95.46% less calling-thread CPU
time**. It includes input copy, Run and softmax; native also includes NaN
canary copy and output readback. It excludes feature extraction, FP32→FP16
input conversion, session/graph setup, result validation and file/ADB
orchestration. CPU output priming is outside its boundary. This is prepared
neural inference performance, not end-to-end audio latency, energy or total
system CPU usage. See the [PoC19 results](marblenet-poc19-results.json) for all
samples' aggregate metrics and exact receipts.

For the completed PoC18 experiment, every individual native/CPU call passed
its captured health checks, all eight derived staging roots were absent,
and final hashes, services, boot
identity and parameters match the initial state. Whole-experiment kernel
history remains inconclusive: the first compile-to-inference gap lacks
retained overlap, and its marker had aged out by final collection.
Retained snapshots establish
diagnostic continuity from the rejected control through CPU execution and
final cleanup. The original failed global comparison is preserved.

## Run a WAV through the qualified NPU package

Use the retained isolated host environment, or create one with Python3.12
and Torch2.5.1, NumPy2.5.1, ONNX1.22.0, ORT1.27.0, PyYAML6.0.3 and
soundfile0.13.1. Torch is optional for the core ONNX/native tools and needed
here for the checkpoint's source-matched feature extraction. No NeMo
installation or new device library is required.

From the repository root:

```bash
PYTHONPATH=waved/tools/onnx-hybrid/src \
artifacts/npu/poc18/hostenv/bin/python -m wave_onnx_hybrid.marblenet_cli run \
  --checkpoint artifacts/npu/poc18/checkpoint/frame_vad_multilingual_marblenet_v2.0.nemo \
  --package artifacts/npu/poc19/packages/19f \
  --audio recording-16k-mono.wav --offset-samples 0 \
  --run-id marblenet-file-unique-001 --mode strict \
  --outputs artifacts/npu/marblenet-file-001.npz \
  --evidence artifacts/npu/marblenet-file-001.json
```

Both output paths must be new and outside the package. The input must be
a finite mono16kHz WAV with at least63,840 samples after the selected
offset. The command selects exactly 63,840 samples (3.99seconds), produces
400 mel frames and200 two-class output frames, and writes `logits`,
`speech_probability` and boolean `speech` arrays. There is no implicit
resampling or short-window padding. Evidence records the audio/selection,
checkpoint, features, model, outputs, actual provider and separate host
timing components.

The retained 18D package has no native qualification. With that package,
`--mode hybrid` uses
`CPUExecutionProvider` with zero native attempts and one CPU session.
`--mode strict` refuses it before feature preparation or inference.
Neither mode treats a partial trained block as the complete model.
The 19f package is now qualified. Its strict frontend performs one native Run
and zero CPU neural sessions. Live launches also require the reviewed
single-controller phase to remain READY with continuous clean diagnostics;
a saved qualification receipt does not authorize bypassing a HALTED phase.
Use new run IDs and output paths. This lab command stages a temporary native
executor and cleans it afterward; it does not install a production VAD service.

## Context and reproducibility

The 20ms output interval is not causal20ms latency. The network's receptive
field is291 mel frames and requires145 future frames (1.45seconds), plus
STFT context. Use it as an offline window test with about 1.46seconds of
lookahead; real streaming integration needs a separate design.

Features use the serialized Hann window and80-band mel filterbank,
preemphasis0.97, centered reflect STFT512/hop160, and the source log guard.
At exactly4seconds, the original preprocessing produces401 valid mel
frames, padded to402; cropping them to400 would change this test contract.

| Artifact | Exact identity |
|---|---|
| HF revision | `9d25eb877d9edab5101ccc843edd04787a6c72c4` |
| NeMo source revision | `ceeafa4f2b8cece8a0fd47b29051ad659e55b76c` |
| Checkpoint | `84bda37e925ac6fd740c2ced55642cb79f94f81348e1fa0db992ca50d4b09706` |
| Full FP16 ONNX | `f5d1cdfe2c1f3ae8b3feb3ffa7568403839a7c59e8ff363846e3fde46cfad46a` |
| Qualified full Pad rewrite | `4929d1c702ed471bc6a9c50fae46bd23561892457405b4fd02fffb372ac10186` |
| Folded FP32 ONNX | `354de9ff0d0b3f75b9c197d322db37ef2a8e38e4f671905024cffa7c8926b96f` |
| Preparation receipt | `4fcc00ccc0f1983f0a43c88053586da7ec3b5154ff13e4829f55972912db38cb` |

Raw weights, audio, builds and device receipts remain ignored under
`artifacts/npu/poc18/` and `artifacts/npu/poc19/`. Tracked source generators
and templates require this
repository, exact SDK/runtime inputs and qualified evidence; the native
adapter is not a portable general-purpose ONNX execution provider.
An algebraically equivalent zero-expanded57-tap kernel could remove this
dilation, but its accumulation accuracy and Wave admission remain untested.
It is outside this completed experiment's native profile and budget.
