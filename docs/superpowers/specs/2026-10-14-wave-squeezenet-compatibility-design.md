# Wave SqueezeNet compatibility and audio-model research

The owner's "go test" accepts the proposed next compatibility experiment,
SqueezeNet 1.1 FP16, followed by research into strong noise reduction and echo
cancellation models for microphone input and received playback audio. This
records execution authorization. VAD integration remains deferred under C20;
this task does not install an audio-processing production service.

## Frozen source and model

Use torchvision0.20.1 / Torch2.5.1 SqueezeNet1.1 in evaluation mode, with
IMAGENET1K_V1 checkpoint SHA
`b8a52dc049b60e4b6ab68ad0df457362afab8b6304b2febdc1650a5dab4d7e7b`
(4,958,839 bytes). The pinned source SHA is
`6a98c53c42399ebff8f776c042c476e39128af368c6175504aa75cbdec807f2d`.
Account for all1,235,496 parameters; each converted FP16 parameter bit must
be preserved by lowering. No retraining, pruning or omitted model stages.
Keep original FP32 and derived FP16 ONNXs, IR8/opset13, separate. Freeze all
actual graph/model/preprocessing/output hashes before native work.

Input is batch1 RGB224×224, resize256/bilinear/center crop and torchvision's
ImageNet mean/std. Four original images are frozen: PyTorch hub dog, torchvision
v0.20.1 dog1/dog2, and matplotlib v3.10.0 Grace Hopper. Original URL, content
hash, tensor bytes and reference class ordering remain explicit. The failed
initial Grace Hopper URL remains a host-only acquisition negative.

## Staged architecture and geometry

Add a separate bounded `fp16-squeezenet-v1` graph family. Existing profile
admission, generated source bytes and qualifications remain unchanged.
Support only static batch1 shapes and the observed standard Conv/Relu,
MaxPool, channel Concat, GlobalAveragePool and Flatten forms. Validate every
node's input/output shape, initializer use, attributes and DAG identity.
Metadata does not authorize unsupported graphs or imply device qualification.

Map NCHW to native WHCN, preserving flat value bytes; OIHW coefficient bytes
become native WHIO dimensions without an accidental transpose. Channel Concat
axis1 maps to native axis2, preserving expand1×1 then expand3×3 branch order.
Bind output Flatten to an explicit reviewed reshape. Use non-square control
inputs to expose H/W swaps.

Use four real-weight controls before the full graph, each once:

| ID | Contract | Trained source |
|---|---|---|
|20a|FP16[1,3,17,13]→[1,64,8,6]|first Conv3×3/stride2 +Relu on a cropped preprocessed image|
|20b|FP16[1,64,15,19]→[1,64,7,9]|first MaxPool3×3/stride2 on frozen first-convolution activation|
|20c|FP16[1,64,7,9]→[1,128,7,9]|first complete Fire block on frozen pool activation, both branches and Concat|
|20d|FP16[1,512,13,13]→[1,1000]|original classifier Conv/Relu/global average/Flatten on frozen final-feature activation|
|20e|FP16[1,3,224,224]→[1,1000]|complete unchanged trained evaluation graph|

Pool input extents in the full224 graph are111,55,27: ceiling and floor give
the same result. Admit only observed geometries for which that equivalence
holds; do not claim general ceiling-mode support. Freeze the actual native
global average operation and all source/build contracts before launch.

The largest expected activation is1,577,088 bytes, full input301,056 bytes,
and converted coefficients2,470,992 bytes. Bound every tensor at8MiB, graph at
96 nodes, initializer total8MiB and regular artifact at64MiB. Intermediate
controls deliberately have small outputs. Executors must keep stack usage
bounded, prime output with half NaNs, enforce finite/readback/guard checks,
and keep the alarm armed through teardown. NN and TP routes are candidates;
SH/EVIS/SW/CPU neural fallback does not count as full native compatibility.
The tensor-only20b Pool control may establish TP-only execution. Every
Conv-containing control and the full model still require at least one NN
route and exclusively NN/TP targets. Record the observed route faithfully.

## Numerical and performance contract

Independent NumPy evaluation accumulates convolutions/reductions in FP32 and
rounds each declared FP16 tensor. Retain actual host ORT and independent
outputs separately. Native-vs-FP16 element policy is finite
`abs(error)<=0.03+0.015*abs(reference)` for every output element. Original-FP32
vs-derived-FP16 logits use `0.05+0.02*abs(reference)`; both derived and native
classification results require maximum softmax-probability error<=0.01,
identical top1 and at least4/5 top5 overlap against the original FP32 result
for each of the four frozen inputs. Record reference class margins. These
are parity checks, not ImageNet accuracy measurement. Do not relax limits
after observing native output.
Independent NumPy step-rounded FP16 bytes are authoritative for native
qualification. Actual host ORT bytes must pass the same immutable element
policy against that oracle on every prepared control/full input before launch.

After full four-input qualification, one paired benchmark may repeat the first
image with4 warm-up and16 measured pairs, alternating FP32 CPU and FP16 native.
Use stock ORT1.15.1 / one thread, exact qualified native output and prepared
input copying/Run/output readback/softmax boundary timings. Exclude image
preprocessing, precision conversion, setup, validation and ADB. Calling-thread
CPU clocks do not measure aggregate CPU or energy. Native benchmark alarm30s,
host40s; no unbounded soak or repeated benchmark.

## Budget, device lifecycle and independence

This is PoC20, not an extension of the closed PoC19 budget. At most6 native
export processes/12 size-export API calls/30 NPU Runs. The selected five
graphs consume5 exports/10 API calls and4 partial +4 full Runs; the20 paired
benchmark Runs bring the planned total to28. Spare budget does not authorize
an extra graph. Compile15s/native20s/host, single execution8s/native12s/host.
No native control retries. A clean unsupported export skips dependencies and
permits only already-selected independent controls. Faults, timeouts,
ambiguous exits, numerical failures or changed health halt all native work.

Use the unchanged pinned Wave stack, TIM-VX/runtime/offline image and absent
CLC policy. New-profile calls require a separately reviewed explicit quiet
phase seed, continuity before staging and immediate preflight, and CLEAN
cleanup before anchor advance. The controller alone operates the device.
Independent source/native evidence, model fixtures and review have separate
scopes as directed by REA. Preserve old phases and incidents. No driver reset,
NPU sysfs write, firmware/library replacement or service stop. Previously
authorized temporary Wi-Fi disconnect may be used only for stabilization,
with same saved network/adminUP restoration verified at closure.

## Audio-model research and deliverables

Research exact publicly available trained models, checkpoints/ONNXs, sample
rates, state/latency, operators, quality evidence and deployment terms from
primary sources. Distinguish strong paper results from accessible pretrained
weights and from unknown Wave/NPU compatibility. Microphone AEC needs a
synchronized playback reference and double-talk preservation; playback noise
reduction is a separate received-speech use case. Record promising joint AEC,
noise suppression and dereverberation candidates without claiming inference
or integrating them into audio services.

Deliver frozen fixture and source/build receipts, actual native outcomes or
precise blockers, benchmark when prerequisites pass, tests and independent
review, sanitized notebook/results/model guide, and a sourced audio-model
shortlist. Use existing feature checkout; commit owned paths only. No push,
merge or production integration.
