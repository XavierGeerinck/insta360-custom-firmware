# PoC20 — trained SqueezeNet spatial compatibility

Date:2026-10-14. Owner authorized testing other real models and researching
strong noise reduction/echo cancellation, with VAD integration deferred.
The [design](../../superpowers/specs/2026-10-14-wave-squeezenet-compatibility-design.md)
and [plan](../../superpowers/plans/2026-10-14-wave-squeezenet-compatibility.md)
were committed before native work. Controller alone operated the device;
REA workers reviewed source/evidence and researched audio candidates.

Result:20a spatial Conv/Relu,20b MaxPool and20c Fire/Concat qualified on NN.
20d's exact classifier AVG13×13 kernel failed initialization during the
first size-only export API call. It returned5 with clean health/cleanup,
produced no NBG and performed no inference.20e and the conditional benchmark
were skipped. No native retry, precision-policy relaxation or CPU neural
fallback occurred. Actual budget use:4 compiler processes/7 public
CompileToBinary calls/3 NPU Runs; planned maximum6/12/30.

The [guide](../onnx/squeezenet.md) records numerical results, model provenance,
bounded operator claims and the failed initializer. The
[machine-readable ledger](../onnx/squeezenet-poc20-results.json) binds model,
source/ELF, NBG, run, reference and cleanup identities.

## Evidence retained

- `artifacts/npu/poc20/prepared/`:32 frozen files and exclusive receipt;
  original trained FP32, explicit FP16 exports, four images' inputs/references.
- `independent-oracle/`:eight separately retained NumPy outputs and actual
  ORT comparison; `core-verification/`: coefficient bits/source regeneration
  and unchanged older profile/source proof.
- `packages/20a`–`20e`:lowerings, exact compiler source/AArch64 ELF/build
  receipts;20a–20c have NBGs, executor and qualified evidence.20d has a
  retained clean negative;20e is host-built but never exported.
- `artifacts/npu/runs/rea-poc20{a,b,c,d}-*-compile-20261014-a1` and
  `rea-poc20{a,b,c}-*-execute-20261014-a1`:raw command, result, before/after,
  numerical and cleanup evidence. `classifier-negative-audit.md` preserves
  the independent exact-source/backend attribution.
- `packages/native-phase.json`:35 operation events plus one final read-only
  continuity check. Literal terminal state ACTIVE; experiment closed and
  no native call remained. No automatic reset or final cosmetic advance.
- `host-final-verification.json`:three qualified pure loads, twelve copied
  tamper refusals and two unqualified strict/missing-NBG refusals, with ADB
  and ORT-session throw guards. No actual CPU neural session or device call.
- `final-host-tests.txt`:381 passed,5 subtests passed. Conditional benchmark
  RED/host-development logs retained, but unused source removed and no
  benchmark compiled or executed.

## Health history

`host-preparation-health-0001.json` remains FAIL: the boot changed during
host fixture/source preparation, before any SqueezeNet native call, and
Wi-Fi diagnostics changed. Cause unknown. Temporary manual supplicant
disconnect used prior owner authorization and did not lower wlan0 or stop
services. The206.882-second quiet window and independently reviewed fresh
current-boot seed admitted phase `squeezenet-20261014-a1`.

All native local and continuous phase checks passed. Every owned root was
cleaned after successful execution or the clean rejection. The first final
raw health comparison remains FAIL because its temporary-root CLC key
differs from the seed's key. `final-native-health-0003.json` binds the retained raw hashes and records the
explicit identical SYSTEM_ABSENT projection and independent38-root absence
check; all other hashes/state/diagnostics remain checked. Seed→final,
seed→post-restoration and final→post-restoration comparisons pass with no
new alerts. This does not reclassify the earlier boot/Wi-Fi failure.

`wifi-restoration.json` verifies the original saved network, COMPLETED,
adminUP, assigned IP, ACK OK and no saved configuration. Private network
details stay in ignored before/after files. No native calls followed restore.

The policy reviewer generated a deterministic local20a compile-manifest
during host review, disclosed it and had the controller validate its bytes.
It performed no ADB/vendor operation. Reports do not claim zero peer host
artifact writes; source/evidence ownership remained separated.

## Subsequent work

Isolate the AVG13×13 backend eligibility condition and prove any equivalent
lowering before reconsidering full SqueezeNet. A clean negative is not an
allowance for unselected retries. The next audio compatibility work requires
new state/multiple-I/O/GRU/ConvTranspose/normalization controls; public ONNX
availability does not qualify the Wave. See the primary-sourced
[speech-enhancement candidates](../onnx/speech-enhancement-candidates.md).
DPDFNet2_48khz_hr is the stronger full-band denoising candidate; LocalVQE
v1.2 is a downloadable joint AEC/NS/dereverb candidate, DTLN-AEC128 an
established control, and GTCRN the smallest first recurrent workload.
AEC needs synchronized microphone/playback-reference pairs; received-speech
denoising is a separate stream/state. No audio-model inference or integration
was performed in PoC20. VAD integration remains deferred under C20.
