# 2026-10-13 — PoC18: trained MarbleNet blocks and full-model CPU fallback

## Record

| Field | Value |
|---|---|
| PoC | `18` |
| Experiment | `wave-npu-poc18-trained-marblenet` |
| Operator | REA controller; independent source/evidence reviewers |
| Device alias | `wave-lab-unit-a` |
| Start UTC | `2026-10-13T10:46:39.564Z` |
| Branch | existing `feat/m3`; unrelated shared-checkout work preserved |
| Source commits | reference/import `ce4a075`; graph sources `47b2e12`; NaN prime `188b636`; bridge `2b2a33d`; frontend `80a0ab0`; admission `c831b91`; CPU source `6a3ca7c`; CPU harness `ae5bf01` |
| Outcome | two trained NPU blocks passed; dilated control rejected; full NPU graph skipped; complete CPU fallback tested |
| Detailed ledger | [sanitized results](../onnx/marblenet-poc18-results.json) |

## Falsifiable hypothesis

The pinned trained MarbleNet FP16 graph can export on the exact Wave NN
stack, execute with finite bounded activation errors, reproduce the original
FP32 speech decisions, and leave captured device health unchanged.

The complete-model NPU hypothesis did not pass. The trained dilated
depthwise control failed kernel initialization before an NBG existed.
Its dependent full-model compile and numerical/performance runs were skipped.

## Safety gate

The owner's `go` authorized the recommendation and staged experiment.
The controller alone operated the device. Actions used exclusive disposable
`/tmp/wave-npu-onnx18-<lowering16>-compile` or `-execute` inventories.
Stock services/watchdog stayed running. No service stop, driver/sysfs write,
system-library replacement, firmware change, reboot, recovery or raw ioctl
replay occurred. CLC remained absent; the incompatible compiler stayed
quarantined. Checkpoint/audio/SDK/raw device logs remain private artifacts.

Maximum planned budget was4 compiles,8 size/export API calls and43 NPU
Runs including benchmark warmups. Actual native use stopped at3 compiler
processes,5 `CompileToBinary` calls and2 Runs. The15s compiler/20s host and
8s executor/12s host deadlines covered teardown. No rejected control was
retried. An independent CPU-only baseline adds no NPU calls.

## Identity and prerequisites

```text
kernel: Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux
galcore: 199:0
enableNN: 255
nanoqFreq: 838860800
recovery: 1
TIM-VX: 702715d714277376d7ff466b7b72833ff57564c1
libtim-vx.so: c984e063e9ab4fe4d7e7ac85e783d88a330f1560268924ba4f449f5b258478dc
runtime provenance: 58601a21a3ac1ff0435833d5d30409a43332a892c77047428cb9a54d658efade
offline cross image: sha256:a45f1308adfe3fc3b90e329e72d5efe9d743e6cc77017254d28bbba1c689586a
```

All 54 prepared files match the frozen receipt. Source-matched unfused Torch,
folded Torch and ORT FP32 agree within `1e-4 + 1e-5*abs(reference)`;
maximum host error is 1.9073486e-6. Every state tensor's shape/dtype/value/hash
is checked, BN epsilon is0.001, and there is no invented activation between
depthwise and pointwise Conv. ONNX exports are actually **IR8**, opset 13.
The future benchmark's IR compatibility helper preserves an already-IR8
file byte-for-byte; an earlier IR10 assumption was corrected before use.

The first host cross-build failed because OrbStack was stopped. Later the
cached cross image disappeared; its exact official registry digest was
restored and the resulting pinned image ID/architecture checked. Both
negative logs remain retained. Native builds used no network, pinned
toolchain and read-only source/SDK mounts.

## Method and observations

The design/plan are under `docs/superpowers/`. REA source and native audits
ran in independent scopes; root source/build/numeric admission was reviewed
before native launch. Actual ONNX constants lower to temporal-fast native
coefficients without an OIK transpose. Depthwise multiplier is1, native
dilation is the rate, and explicit pads preserve the trained graph.

| Control | Compile run / execution run suffix | NBG bytes | Actual NN result |
|---|---|---:|---|
| 18A | `rea-poc18a-marblenet-{compile,execute}-20261013-a1` | 25,544 | both PASS, numeric max 0.001953125, cleanup CLEAN |
| 18B | `rea-poc18b-marblenet-{compile,execute}-20261013-a1` | 57,856 | both PASS, numeric max 0.00048828125, cleanup CLEAN |
| 18C | `rea-poc18c-marblenet-compile-20261013-a1` | none | FAIL exit 5, no timeout/health delta/kernel alert, cleanup CLEAN |
| 18D | no native run | none | skipped |

18A is the first trained pointwise Conv/foldedBN/Relu. 18B is the first
trained residual block (8 ONNX nodes,5 Conv), and its compiler target lines
are NN. 18C's input is FP16 `[1,64,200]`, depthwise weights `[64,1,29]`,
group 64, dilation 2 and symmetric pad 28. The exact source uses rate `{2,1}`,
not `{1,1}`. Raw stdout selects `depthwise_conv2d`, then reports
`vivante.nn.convolution.layer` initialization status -1 and size-only compile
failure. Vendor teardown also emits force-release warnings. No successful
size query, export or inference occurred for18C.

A mistaken C qualification host invocation refused because no compiled NBG
exists; its host marker is retained and is not a native Run. A separate CLI
usage typo also failed before device contact. Neither was retried natively.

New-profile comparisons retain actual CPU, independent oracle and actual
native bytes separately. Every native FP16 output element must be finite
and satisfy `abs(error)<=0.02+0.02*abs(reference)`. NaN output primes close
the unwritten-output tolerance loophole found in review. PoC17 remains
byte-exact and its four actual qualified packages still load unchanged.

The full host frontend used actual ORT FP16 CPU inference on the original
speech window and three frozen FLOAT WAV fixtures. All 800 threshold 0.5
decisions agree with the original FP32 reconstruction; maximum probability
error 0.0007275641. Speech/noisy-speech each flag 175/200 frames, silence/noise
each 0. Actual evidence reports CPUExecutionProvider, one CPU session and
zero native calls/attempts. An ADB guard was never called. Real strict
frontend refusal precedes feature preparation/session/transport.

## CPU-only device baseline

The separate runner uses the installed ORT1.15.1/API15 library, full folded
FP32 graph and frozen speech features, one thread, sequential execution and
all graph optimizations. Its ELF/source contain no TIM-VX, GAL, OpenVX,
chip identity or NPU submission path. Each of 4 warm and 32 measured CPU
iterations has guarded input/output, NaN priming, finite tight-reference
validation and CPU softmax. The8s alarm covers ORT teardown; the12s host
deadline and exclusive inventory cleanup apply.

Run `rea-poc18-cpu-benchmark-20261013-a1` passed with one process and 36 CPU
calls, zero NPU calls, clean captured health and cleanup CLEAN. Its actual
1,600-byte output is byte-identical to the host folded FP32 reference
`6449f6f3f605a689477e9b38a1bc266cdb601fa3d391d308076a4d1038f8819d`.
All 200 speech decisions match the original FP32 model; probability error
is 1.5557388e-7. The reviewed preparation receipt is
`5e3ce4b99dea1bed9c745274cb35c1e55771336f8c5392245ffa4222e144dcf8`.

| CPU component | Median ms | p95 ms |
|---|---:|---:|
| ORT Run | 8.547625 | 8.738842 |
| Input copy + Run + softmax boundary | 8.630792 | 8.824513 |
| Input copy | 0.051854 | 0.055283 |
| Softmax | 0.017916 | 0.018250 |
| Calling thread CPU time | 8.536126 | 8.700998 |

All 32 raw samples and exact output/build hashes are in the sanitized ledger.
Prepared-feature boundaries include input copying, ORT Run and softmax.
They exclude output NaN priming, result validation, STFT/mel extraction,
startup/staging and host orchestration. Per-thread CPU time is not system-wide CPU usage.
The full-model NPU benchmark has no measured samples or speedup claim.

## Review and limits

Independent review identified and resolved the finite-canary loophole,
mutable future-benchmark receipt binding, incomplete extra-clip raw-evidence
validation and the incorrect IR10 assumption. Actual18A/B strict loading
and13 changed-artifact cases refuse without creating a CPU session or ADB
transport. Broad old/new host regression receipts remain private artifacts.

This covers exact trained blocks, CPU parity and a device CPU baseline.
It does not measure labeled VAD accuracy, full-model NPU performance or
production streaming integration. The400→200 frame graph needs about
1.46s future context despite a 20ms output interval. A zero-expanded57-tap
alternative for the dilated layer is only a future algebraic proposal;
hardware admission and numerical accumulation remain unproven.

Final comparison verifies original-to-final system hashes, service process
identities, boot/kernel identity and NPU parameters, all eight roots absent
and no lab process. The original global comparison failed only diagnostic
alignment: the initial marker aged out of the kernel ring. That receipt is
preserved; whole-experiment kernel continuity is **inconclusive**, rather
than silently marked PASS. A retained overlapping chain from 18C cleanup
through pre-CPU and final snapshots passes with no kernel alerts. Every
individual native/CPU run's captured health gate passed. The final ledger
reports `global_health.status=PARTIAL` with this precise unresolved history.
See the [file deployment command](../onnx/marblenet.md).
