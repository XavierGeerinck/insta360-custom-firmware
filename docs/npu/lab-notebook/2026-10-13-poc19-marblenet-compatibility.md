# 2026-10-13 — PoC19: MarbleNet convolution compatibility

**Completed after owner reconnection on2026-10-14.** The complete trained
graph qualified on NN/TP, all800 frozen frame decisions match FP32, and the
paired prepared-feature benchmark measured10.399× faster boundary execution.
Wi-Fi and final device state are verified restored. The failed2026-10-13 and
first2026-10-14 intervals below remain failed. See the resumption section.

## Scope and identity

The owner authorized continuing the REA investigation, a compatible trained
model rewrite and staged native testing. The controller owns every device
operation. Source reconstruction, native static analysis and independent
review have separate scopes. The existing feature checkout and unrelated
changes are preserved. No production service deployment is implied.

This continues [PoC18](2026-10-13-poc18-trained-marblenet.md), preserving its
rejected K29/rate2 control and all earlier receipts. Checkpoint
`84bda37e925ac6fd740c2ced55642cb79f94f81348e1fa0db992ca50d4b09706`,
TIM-VX `702715d714277376d7ff466b7b72833ff57564c1`, runtime
`c984e063e9ab4fe4d7e7ac85e783d88a330f1560268924ba4f449f5b258478dc`
and the pinned offline cross image are unchanged. The current boot identity
was already different from PoC18 before this experiment began; no reboot
or recovery was performed by this experiment.

## Isolation results

| Control | Trained-layer change | NBG bytes | Actual native result |
|---|---|---:|---|
| 19a | centered13 original taps, rate2, pad12 | 24,608 | NN compile and one Run PASS; max error0.000244140625 |
| 19b | all29 original taps, rate1, pad14 | 26,656 | NN compile and one Run PASS; max error0.000244140625 |
| 19c | exact Pad28/28 then original K29/rate2 VALID | 32,168 | TP Pad + NN Conv; one Run PASS; max error0.000244140625 |
| 19d | original first K11/stride2 block, flat-height native2×2 | 28,960 | NN compile and one Run PASS; max error0.001953125 |
| 19e | original K15/K17 residual blocks | 79,376 | local NN compile PASS; surrounding health interval FAIL; no inference |
| 19f | full exact Pad graph | none | not launched |

19a/b reuse the exact frozen18C activation and downstream pointwise/bias/Relu.
They are diagnostic graphs, not equivalents of the complete trained layer.
The14-file preparation receipt is
`692d3e2499fa447651f7296c660cf3f0c8cacd4f6bb3a677e927e31f8030f204`.
Run names are `rea-poc19{a,b}-isolation-{compile,execute}-20261013-a1`.
All outputs are finite, with zero failures under the unchanged
`0.02+0.02*abs(reference)` element policy. CPU ORT, independent NumPy and
actual native outputs are retained separately. Health/alerts were empty
and cleanup was CLEAN. Neither isolated width nor isolated dilation explains
the prior combined K29/rate2 rejection.

## REA evidence and limits

An owned REA6.4.0/Ghidra12.1.4 session inspected the exact firmware OpenVX
library, SHA
`f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb`.
It was closed, its process/temp directory removed, and the target hash
remained unchanged. The retained113-record evidence bundle is3,473,580 bytes,
SHA `b3321361bee1281a3bd9b58ab9a71f9165eaa07a94f9d70f05e5aa64bc1afa75`.
Addresses below are REA/Ghidra default-space addresses, rebased by0x100000.
The decisive width load maps to measured file offset0x25d2c4, and the
status−1 instruction maps to0x27c648; raw instruction evidence is retained.

- `vxConvolutionLayer` at0x24b200 selects kernel0x1000/table0x70a0f0 and
  initializer0x35cfa0. Its trivial validator0x362500 is not that initializer.
- Generic depthwise admission depends on representable signed padding,
  batch/format/weight flags, multiplier, runtime-configured kernel bounds,
  dilation support, and either unit strides or a feature-gated2×2 stride.
  The width comparison at0x35d2c4 uses context+0x2e78e8. These live bounds
  come from `gcoVX_GetNNConfig`; their numeric values remain unknown.
- The specialized depthwise fallback has unconditional false NN/TP predicates;
  EVIS admission is conditional. Once an admitted backend's initializer
  fails, this dispatch returns that failure rather than trying another backend.
- The FP16 shader getter includes width/dilation branches. A null shader at
  0x37b2d4 can reach status−1 at0x37c648; reshape failures can also return−1.
  The historical18C log cannot identify which branch actually fired.
- Static public TIM-VX Pad reaches native NN/TP routes. One-input static Slice
  can reach StridedSlice/view/copy routes; two-input Slice has different routes.
  This source eligibility does not qualify any new fixed graph.

Missing depthwise shader resources were observed in the staged SDK and
matching searched firmware/SDK trees. Their absence and CLC quarantine do
not establish the cause of18C. No arbitrary15-tap limit is inferred.

## Selected exact rewrite

The first candidate explicitly pads `[1,64,200]` by positive-zero28 samples
on each temporal side, producing `[1,64,256]`. The unchanged original
K29/rate2 depthwise filter then uses VALID pads0 and produces200 frames;
pointwise/bias/Relu remain unchanged. Effective extent57 gives256−57+1=200.
Every original initializer bit is retained.19c directly established native
TP padding plus NN convolution and passed finite numerical qualification.

A separate `fp16-marblenet-pad-v1` profile binds structural constants,
canonical Pad geometry and a flat-height depthwise stride policy. For native
height1/kernel-height1/height-pad0 only, temporal stride2 uses native2×2
strides with the same height1 output.19d qualified that policy independently.
Full-model admission reverses the deterministic
rewrite to the exact original FP16 model hash; shape alone is insufficient.

## Health and temporary Wi-Fi change

Three real atbm Wi-Fi WARN/tx-flush incidents occurred before any new
model/NPU call, and the original failed snapshots are retained. A separate
false-positive matching `hang` inside `changed` was fixed with regressions;
actual warning/timeout detection remains intact. The owner explicitly
authorized temporarily disabling Wi-Fi and restoring it afterward.

The controller captured original state privately, requested supplicant
disconnect at2026-10-13T13:12:04.475Z and lowered wlan0. The running waved
poller raised the interface again, while manual supplicant DISCONNECTED
persisted. Credentials/configuration, stock services/watchdog, NPU settings,
driver, firmware and system libraries were unchanged. A separately reviewed
quiet71-second interval established the new eligible measurement phase.
The prior failing intervals are not reclassified as healthy. Final Wi-Fi
restoration and closure evidence remain required.

## USB incident and2026-10-13 stopping point

The literal `WARNING: Drop the transfer event` appeared at uptime4992.636165,
37.14 seconds after19d CLEAN cleanup at4955.50. It was already in19e's
stage-before at5008.78 and preflight at5016.07.19e local before/after PASS
therefore means no further change after its own baseline; it does not clear
the genuine failed interval from the previous accepted state. Snapshot0015
has exact diagnostic overlap, unchanged static state and this new alert.
Native launches stopped when the encompassing interval check found it.

USB_STATE=DISCONNECTED follows the warning by25µs, with gadget reconfiguration
nearby. The public Khadas Amlogic5.4
[CRG UDC source](https://raw.githubusercontent.com/khadas/linux/khadas-vims-5.4.y/drivers/amlogic/usb/crg/crg_udc.c)
contains the same text when a transfer event refers to a dequeued USB request.
This supports a USB-associated explanation, but it is not an exact binary
callsite proof for this Wave kernel and does not reclassify the failed interval.

The controller attempted the already-authorized Wi-Fi restoration, but ADB
reported the selected device absent before the first link command succeeded.
No reassociation command was issued and restoration is unverified. Host ADB
inventories continue to show no device. The owner was asked to reconnect
with debugging enabled and clarify whether USB was changed. No driver reset,
USB configuration mutation, daemon restart or new native model call followed.

Independent review3 identifies a real workflow gap: continuity from the last
accepted cleanup was not checked before19e staging, allowing the new warning
into its local baseline. Commit0843080 adds a persistent host phase guard for
the new Pad profile. It checks the accepted anchor before staging mutations,
after staging and on immediate native preflight; warnings/lost overlap/state
changes latch HALTED. Only verified CLEAN cleanup advances the anchor.
A narrowly validated clean compiler negative advances then rethrows its
negative result; faults/numerical/artifact failures remain halted. Local
NoRetry and missing-NBG requests refuse before phase activation. No production
phase or seed has been created. This supports the sole controller workflow;
its owner UUID is not an interprocess lock or compare-and-swap mechanism.

## Status at the2026-10-13 stop

The bounded plan permits at most8 compiler processes,16 size/export API
calls and46 NPU Runs, without retrying a control. Actual use is5 compiler
processes,10 size/export API calls and4 NPU Runs. No control was retried.
Compile deadlines15s native/20s host and execution8s/12s cover teardown.
Faults, timeouts, numerical failures, ambiguous exits or health changes
stop further native work; a clean compiler negative can guide another
independent, reviewed candidate within the remaining budget.

The padded block and first trained stride2 block passed native qualification.
Remaining K15/K17 inference, complete graph/four clips and same-device
benchmark were not launched after the incident. Every completed caller's
local health/cleanup checks passed; the encompassing interval failed and
final target state cannot be verified while ADB is absent. No full-model
native success or speedup is claimed. The
[sanitized ledger](../onnx/marblenet-poc19-results.json) retains these distinctions.

The frozen55-file Pad preparation is
`92019faefcd00d93faee1d28b5d2cf9c8b1ba63d8c13bc71ac00048ac79df76d`.
Full candidate SHA
`4929d1c702ed471bc6a9c50fae46bd23561892457405b4fd02fffb372ac10186`
retains all34 folded initializer tensors,90,418 elements and180,836 FP16
bytes. Four-clip folded FP32 and FP16 logits are byte-exact to the originals;
all800 decisions match the original unfused FP32 reference, probability
error0.0007275641. Independent FP16 comparisons pass with max error0.00390625.

Source commits are core589fe0b, nativebce86be/16d0a4b and admissionb427b78.
Root harness427 tests passed with2 optional skips; relevant non-ERF host
tests430 passed. The broad host collection retained a negative for9 unrelated
ERF modules needing SciPy/pystoi absent from the isolated Torch environment.
Pure-host full admission accepted the exact rewrite and refused an altered
weight even after resealing its digest, with zero CPU sessions/transport.
The real unqualified full frontend refused before features/session/ADB.
Actual qualified19c/d and valid copied packages load without CPU/transport;
26 altered-artifact cases refuse under CPU-session/ADB guards. The original
model, source, output and evidence files were not modified by these tests.
The guard's full root harness passed470 tests, with2 optional skips and329
subtests. The last uncompiled-qualification ordering correction has a separate
RED→GREEN refusal regression, with zero phase/oracle/transport/attempt marker.
Final host verification rechecks all123 prepared files and14 packages,
including10 qualified ones, with CPU-session/ADB guards. The sole-controller
guard passed independent review; this does not authorize live continuation
or replace the outstanding Wi-Fi restoration and final device checks.

## 2026-10-14 resumption and complete native qualification

The owner explicitly reconnected the unit. Boot identity is now
`eff903fb-00b5-45ab-bcad-30f36195cef5`; the pinned serial, kernel, NPU settings,
system libraries and model/build/preparation hashes match. No agent reboot,
driver reset, service restart or USB configuration change was used. Wi-Fi
was already COMPLETED on the original saved network at reconnection.

The first explicitly reviewed new phase, a1, then HALTED before staging or
any vendor call: a fresh atbm `wsm_write_mib` WARNING at uptime363.952494 and
lost200-line diagnostic overlap were detected. Its exact HALTED document,
SHA `ace9b39d0a235121a47a33b38a87dc51f4375b6a139016a76209d36c67a88dbc`,
and seed remain retained. The rejected raw snapshot was not saved by the old
write ordering; it was not reconstructed. Commit446a85d moves its exclusive
host write before the phase check and device mutation. An exact-warning
regression proves retention and zero device mutations;45 focused tests and
5 subtests pass, with Ruff clean.

Under the owner's existing temporary-disable authorization, Wi-Fi was again
manually disconnected without saving configuration. A separate525.396-second
quiet interval had exact overlap, no new alerts and unchanged system state.
Reviewed snapshot SHA
`18b56028320f38207ce4c85551f014d04f408698f8baf25582e685d90fe0b172`
explicitly seeded b1 after byte-exact a1 archival. No automatic reseed occurred.

19e's blocked host marker remains untouched. A new host evidence directory,
`19e-wifi-quiet`, copies26 immutable files from the original unqualified
candidate, inventory SHA
`0134a8115f38a5be9b615a419da262dbb72e75da6292116399c2d1102bca92a8`.
This supports19e's first actual native Run; it is neither a native retry nor
a repeated export. It passed with max activation error0.00048828125 and
zero mismatches. Its old surrounding export interval remains failed.

19f's first native export passed and produced232,384 bytes, NBG SHA
`69eddc1ac45e2016f5adee2f53de6587fcc564eaaf90c4720e316b1d7e2b53f9`.
Raw parsing verifies CRCs and fixed FP16 I/O. Stock routing is24NN +1TP,
with zero SH/CPU routes. The actual-size executor source/ELF passed independent
review before the first full qualification. That speech-feature Run and
three subsequent strict WAV frontend Runs passed. The frontend calls were
wrapped in an ORT neural-session throw guard and each recorded one native
call, zero CPU sessions and CLEAN cleanup.

| Clip | Speech frames | FP32 decision agreement | Max speech-probability error |
|---|---:|---:|---:|
| Public speech |175/200|100%|0.000814493|
| Silence |0/200|100%|0.000023429|
| Seeded noise |0/200|100%|0.000030908|
| Derived noisy speech |175/200|100%|0.000168334|

All800 decisions agree with the original source-matched FP32 reconstruction;
max native activation error is0.00390625 under the unchanged element policy.
These are fixed-clip parity results, not a labeled VAD accuracy evaluation.
The real speech clip uses exact frozen features for initial qualification;
the other three additionally exercise the actual strict file frontend.

## Paired benchmark and limits

One process ran36 CPU/NN pairs:4 warm-up and32 measured, alternating order.
CPU is the original folded FP32 graph on stock ORT1.15.1 with one thread;
native is the exact qualified FP16 NBG. Every native iteration reproduces
the qualified output bytes. The CPU reference matches the earlier successful
Wave baseline. Source, actual ELF, inputs, both references and preparation
receipt were independently reviewed before the single benchmark attempt.

| Prepared-feature measurement | CPU median / p95 ms | Native median / p95 ms |
|---|---:|---:|
| Run |8.577104 /9.252773|0.690792 /0.814966|
| Prepared-feature boundary |8.664375 /9.331614|0.833188 /0.993871|
| Calling-thread CPU time |8.545312 /8.760459|0.387897 /0.461373|

Boundary median speedup is10.399069837×; calling-thread CPU time falls95.46071%.
Native boundary includes input and NaN-canary copies, Run, output readback
and softmax. CPU boundary includes input copy, Run and softmax; CPU output
priming and both paths' result validation are outside it. Feature extraction,
FP32→FP16 feature conversion, session/graph setup and file/ADB orchestration
are excluded. Caller-thread clocks do not measure total system CPU or energy.
Every pair repeats the same frozen speech window. Each window is3.99seconds;
the20ms output interval still requires about
1.46seconds of future context and does not establish causal streaming latency.

A host-only build attempt initially used relative Docker volume paths and
was rejected before compiler/device execution. Its log is preserved; a new
host directory with absolute paths produced byte-identical source/data.
This consumed no native budget or retry. The final run count is6 compiler
processes,12 export API calls and45 NPU Runs, within8/16/46 ceilings.

## Final closure

Independent review reconstructs all35 b1 phase events and every cleanup
advance from the reviewed seed through the benchmark. Every operation has
empty health differences/alerts and CLEAN cleanup. Final direct seed-to-state
diagnostic comparison also passes; kernel, boot, service identities, hashes,
NPU settings and compiler absence are unchanged. All28 unique staging roots
are absent and no galcore holders/native lab processes remain.

After closing the measurement phase, wlan0 was raised and reassociation
acknowledged OK. At2026-10-14T10:01:43.929Z the original saved SSID/network ID
were verified privately, supplicant COMPLETED, adminUP and an IP assigned.
Credentials/configuration were not saved or changed. A separate post-restoration
snapshot passes with no new kernel alerts or state deltas. The earlier blocked
restoration attempts and all failed intervals remain retained.

Before the benchmark,13 altered artifacts of actual qualified19f copies
refused with zero CPU sessions/transport and unchanged originals. After the
benchmark, a separate pure-host check revalidates all123 prepared files and
15 packages, including12 qualified packages, again with zero CPU/transport.
No production VAD service was installed. Further live work requires its own
reviewed phase and bounded contract; no native retries are authorized by
this completed experiment. Raw resumed evidence is retained under
`artifacts/npu/poc19/resume-20261014/`; the tracked sanitized ledger contains
the exact qualification, clip and benchmark receipts.
