# Wave SqueezeNet mean compatibility repair

The owner asks why the classifier was rejected and whether full inference can
be fixed, renewing the compatibility work after closed PoC20. This continues
the existing REA/host-fixture/native qualification workflow. No production or
VAD/audio integration is requested. PoC20 evidence/packages remain immutable.

REA binds the actual PoolingLayer2 registration and initializer to the pinned
libOpenVX SHA f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb.
Its ordinary NN support callback refuses average mode and also restricts its
MaxPool geometry. Overall failure still depends on remaining SH/SW eligibility;
we do not claim a universal average-pooling hardware limit. Changing stride13
to1 cannot make AVG pass that NN callback. Public ReduceMean/separable AVG
remain alternative routes with unqualified dispatch/rounding/CLC behavior.

Selected repair: keep the original FP16 ONNX GlobalAveragePool and all trained
coefficient bits, but define a separate bounded native lowering to fixed
13x13 depthwise Conv, stride1, pad0, rate1, multiplier1, zero FP32 bias. Every
channel has169 weights with binary16 bits0x1e0f, nearest1/169. This coefficient
is1551/262144:169alpha=1-25/262144, relativeerror -0.0095367431640625% before
accumulation/rounding. It is a numerically qualified approximation to the mean,
not a claim of exact real or bitwise equivalence. NN execution must establish
its actual eligibility and output error.

New profile fp16-squeezenet-mean-v1, namespace/persistence/evidence PoC21 only.
Classifier/full model derivation changes native-profile metadata only; reversing
that marker must recover the pinned original20d or20e full model SHA exactly.
Native policies bind mean bits/shape/group/bias and original-model identity.
A fixed21a pool-only diagnostic accepts [1,1,13,13]→[1,1,1,1], GlobalAveragePool
only, no trained constants. Its input is a frozen nonzero real classifier-map
crop derived from the original trained checkpoint and first frozen dog input.
21b uses original20d classifier,21c uses original20e complete65-node graph.

The authoritative native oracle remains the independent original NumPy mean;
actual ORT CPU outputs remain separate. All output values finite, same frozen
native tolerance0.03+0.015absref. OriginalFP32 classifier/full policy unchanged:
0.05+0.02absref logits, maxprob0.01, identicaltop1, top5overlap≥4/5 on each of
four original prepared inputs. No tolerance relaxation after observation.
Host model/parameters/source/ELF/reference/canary bounds match PoC20, including
8MiB tensor/initializer,96nodes,64MiB artifact,48MiB source,128KiB stack limits;
new derived native constant payloads count against emission bounds.

New budget4 compiler processes/8 publicCompileToBinary calls/8 NPU Runs;
selected sequence3 exports/6 API calls/6 Runs:21a first,21b if21a qualifies,
21c full ifclassifierqualifies, then four full inputs. No benchmark or unselected
alternatives in this repair. No native retry. A canonical clean exit5 skips
dependents; any fault, timeout, numerical/health failure stops all native work.
Fresh current-boot reviewed quiet phase, continuous/immediate preflight,
unchanged pinned stack/services, compile15/20s and inference8/12s, cleanup and
original Wi-Fi restoration remain mandatory. Earlier temporary-disconnect
owner consent persists; controller alone does every device/vendor operation.
Do not reseed or advance the closed PoC20 phase.

Audio is independent: primary matched-checkpoint/quality/runtime research plus
host-only state probes select the next candidate. No audio Wave deployment or
NPU inference under this budget. Identical-feed reset discrepancies remain
unresolved until diagnosed; finite outputs alone do not establish equivalence.
