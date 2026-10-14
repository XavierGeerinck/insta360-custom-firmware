# PoC21 — SqueezeNet mean repair and audio selection

Date: 2026-10-14. The owner requested an explanation of the rejected 13×13
classifier, a full-model inference fix, and the best next audio candidate.
The [design](../../superpowers/specs/2026-10-14-wave-squeezenet-mean-design.md)
and [plan](../../superpowers/plans/2026-10-14-wave-squeezenet-mean.md) define
a separate profile, budget and phase. The controller operated the device;
REA workers independently reviewed reconstruction, sources and evidence.
VAD integration remains deferred under root backlog C20.

Result: the scalar mean, trained 1,000-channel classifier and complete
trained SqueezeNet1.1 exported on NN and qualified. Full inference passed
all four original frozen images: 4,000 finite logits, zero policy mismatches,
maximum error 0.03125 against the independent original FP16 reference. Each
image preserves original FP32 top-1 and all five top-5 classes. There was no
benchmark, fallback or native retry. Actual use was 3 compiler processes,
6 public CompileToBinary calls and 6 NPU Runs, within the 4/8/8 budget.

The [guide](../onnx/squeezenet.md) explains the exact numerical scope; the
[machine-readable ledger](../onnx/squeezenet-poc21-results.json) binds model,
source/ELF, NBG, run, input/output, numerical and cleanup identities.

## Rejection evidence and repair

REA opened the firmware `libOpenVX.so`, whose SHA
`f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb`
matches the device. The initial unconfigured-provider refusal is retained;
existing Ghidra 12.1.4/OpenJDK were then supplied only in the child environment.
No system installation or global environment change occurred. The REA
session was explicitly closed.

At Ghidra address 0x398d50 (ELF0x298d50), the ordinary Pooling NN support
predicate clears its support flag for non-MAX modes. AVG is therefore not
admitted by that route. The verification foot at 0x4405e0 copies the flag:
`ldr w1,[x4]; mov w0,#0; str w1,[x3,#8]; ret`. It does not override the
decision. The exact failed remaining fallback branch is unobserved; the
evidence does not establish a universal kernel-size limit. PoC20's actual
AVG13×13/stride 13 initializer failure and closed packages remain unchanged.

The new `fp16-squeezenet-mean-v1` changes only ONNX profile metadata;
inverse derivation must recover the exact original models and lowerings.
Original GlobalAveragePool and learned bits stay intact. Native emission
uses fixed depthwise Conv2d with `[13,13,C,1]` WHIcOc weights, multiplier 1,
stride 1, dilation 1, no padding and positive FP32 zero bias. Each of 169
weights per channel has half bits 0x1e0f (`1551/262144`). Its gain is
`1 - 25/262144`, so this is an explicitly qualified approximation,
not an exact mathematical or bitwise mean identity. Synthesized constants
are included in all source/constant bounds. The full graph emits 2,820,880
constant bytes and has a 25,840-byte stack payload bound.

The authoritative oracle remains original NumPy FP32 accumulation with
declared FP16 tensor rounding. The unchanged native tolerance is
`0.03 + 0.015 * abs(reference)`. Original FP32 classification uses
`0.05 + 0.02 * abs(reference)` logits, maximum probability error 0.01,
identical top-1 and top-5 overlap at least 4/5. No tolerance was relaxed.

## Actual native sequence

21a uses a real classifier-map crop from the original trained checkpoint,
channel258, with original mean 31.359375. Zero output cannot pass the frozen
policy. Its NN export is 2,248 bytes and its one Run agrees byte-for-byte.
21b uses the original classifier input and all its trained coefficients;
its 1,492,320-byte NN export and one Run pass all 1,000 finite logits,
maximum error 0.015625. The full21c NBG is 3,278,944 bytes, SHA
`a965e230a5fe0aac63f89ccb8ce6618a46113c4d0e8c9c97120bed984596ece5`.

Dog qualifies21c once. The remaining dog1, dog2 and grace_hopper inputs run
strictly against that qualified NBG under an ORT InferenceSession throw
guard, with no actual CPU ORT session. Every next call depends on prior
numerical/classification PASS, CLEAN cleanup and fresh continuity/preflight.
Actual source/ELF and actual-size executor gates were independently reviewed.
Target logs select NN; their repeated compiler/internal-route occurrences
do not count ONNX nodes. There are 65 nodes in the original eval graph.

All four runs preserve top-1 classes 258/273/235/457 respectively and all
five top-5 classes. Maximum probability error is0.0019452088010574742.
First-call transport durations are retained as correctness evidence,
not reported as model performance. The experiment does not measure
ImageNet accuracy, energy or end-to-end camera processing.

## Continuity and closure

The initial temporary Wi-Fi disconnect returned remote254 rather than an
acknowledged success. Actual state became DISCONNECTED/adminUP; a driver
`wsm_vif_flush_tx` warning and MIB diagnostics appeared before any PoC21
vendor operation. `disconnect-refusal-health.json` remains FAIL. No blind
disconnect retry, driver reset, service stop or historical phase reseed occurred.

A separately reviewed 62.1362136-second read-only quiet interval on the same
current boot admitted the first PoC21 phase, `squeezenet-mean-20261014-a1`.
This is a distinct first seed; it does not erase the preparation failure or
change the closed PoC20 phase. All 45 operation events passed. The final
read-only check adds event 46 and leaves the literal state ACTIVE; the
budget is closed, no native work remains and there is no cosmetic advance.

Final health compares identical SYSTEM_ABSENT keys and independently checks
all 44 historical/current staging roots: all are absent. Boot, pinned hashes,
services and parameters match, no lab processes or galcore holders remain,
and seed→final, final→restored and seed→restored diagnostics have no new alerts.
Wi-Fi is verified restored to the original saved network, COMPLETED/adminUP
with an IP and ACK OK; no configuration was saved and no native call followed.
The earlier failed Wi-Fi interval remains failed.

Pure loads of all three qualified packages pass under ADB and ORT-session
throw guards. Twelve copied-artifact tamper cases refuse; the original
20d/20e still refuse uncompiled/strict native execution before phase activity.
All 32 original and 7 new prepared files remain hash-valid, and the old PoC20
phase is unchanged. Thirty legacy generated compiler/executor sources remain
byte-identical. The host suite passes 415 tests and 5 subtests; Ruff passes.

## Retained artifacts and next audio test

Raw local evidence is under `artifacts/npu/poc21/` and
`artifacts/npu/runs/rea-poc21*`. The host arithmetic witness in
`host-native-models/` explicitly substitutes a mean convolution to test the
approximation on the host; those witness models are not the admitted ONNX
packages. Actual admitted models retain GlobalAveragePool. Private network
details and large binary/disassembly data remain in ignored artifacts.

The [audio guide](../onnx/speech-enhancement-candidates.md) selects GTCRN as
the first device baseline. Mac ORT1.27 smoke tests ran 128 consecutive real
speech spectra with explicit state; both GTCRN and UL-UNAS produced finite
outputs. GTCRN reproduces enhanced spectra for 16 reset frames byte-for-byte.
UL-UNAS returns different spectra/caches with identical fresh feed bytes;
optimization/arena/prepacking changes and the original non-simplified author
export did not resolve it. Cause unknown. The ONNX NumPy reference evaluator
could not complete bidirectional GRU, so no independent full audio reference
PASS is claimed. Probe output overwrites in the earlier exploration are
disclosed; probe0003 copies have unique paths and validated hash bindings.

UL-UNAS remains a stronger lightweight quality hypothesis from matched DNS3
paper results. DPDFNet2_48khz_hr is the stronger full-band candidate;
LocalVQE v1.2 and DTLN-AEC128 are concrete joint/standalone AEC candidates.
AEC needs paired microphone and synchronized actual playback-reference audio.
No audio model ran on the Wave, and host smoke tests do not establish quality
or latency. The next step is source-matched GTCRN state/waveform references,
then stock Wave CPU processing, followed by bounded native controls for its
new GRU/ConvTranspose/normalization contracts. Production integration waits.
