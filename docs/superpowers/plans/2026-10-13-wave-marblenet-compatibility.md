# Wave MarbleNet Compatibility Implementation Plan

> Execute the approved recommendation continuously. REA explicitly directs
> independent questions to separate workers. The controller alone owns all
> device calls; fixtures, static binary evidence and review have distinct
> scopes. The user's `go` supplies execution approval for this plan.

**Goal:** Isolate the rejected convolution signature and qualify an exact
compatible full-model lowering when the observed native routes support it.
**Architecture:** Frozen diagnostic ONNX variants reuse the current bridge;
REA establishes binary constraints; host-tested transformations enter native
qualification only after their exact contract/source/build review.
**Tech Stack:** Existing pinned Python3.12/ONNX1.22/ORT1.27/NumPy2.5/Torch2.5,
REA6.4/Ghidra, TIM-VX702715d and offline AArch64 cross image.
**Spec:** `docs/superpowers/specs/2026-10-13-wave-marblenet-compatibility-design.md`.

## Global constraints

- Checkpoint84bda37e925ac6fd740c2ced55642cb79f94f81348e1fa0db992ca50d4b09706.
- Runtimec984e063e9ab4fe4d7e7ac85e783d88a330f1560268924ba4f449f5b258478dc;
  imagea45f1308adfe3fc3b90e329e72d5efe9d743e6cc77017254d28bbba1c689586a.
- Exact18C source/activation/checkpoint identities remain unchanged; new
  models/packages/receipts stay under `artifacts/npu/poc19/`.
- At most8 compiles/16 export API calls/46 Runs, no retry; isolation initial2/4/2.
- Native/host deadlines15/20 compile and8/12 execution; no recovery or
  service/driver/library/firmware changes; CLC excluded.
- Finite0.02+0.02absref; full probability0.02/decision99%; preserve older
  exact-byte profiles and all working packages.

## Review focus

- Isolation K13 taps are a documented centered subset, not claimed as an
  equivalent full trained layer; K29 coefficients and downstream nodes stay exact.
- Only width/dilation/padding changes in the diagnostic family; fresh graph
  signatures and independent CPU reference outputs are required.
- Native source and opcode eligibility, not symbol names or algebra alone,
  determine whether a rewrite can run without missing shader compilation.
- No partial output can pass NaN/finite checks; do not reseal mutable receipts
  or claim CPU reference bytes as actual native output.
- Long host preparation must preserve overlapping kernel-history snapshots;
  any gap remains explicit instead of weakening the diagnostic gate.

## Task1 — Frozen isolation fixtures

**Create:** `waved/tools/onnx-hybrid/src/wave_onnx_hybrid/marblenet_compat.py`,
`tests/test_marblenet_compat.py`.
**Interface:** `prepare_isolation(prepared:Path, output:Path)->dict`.

- [x] Observe RED for missing implementation, changed18C/preparation/input,
  wrong pads/taps/downstream nodes, unsafe output alias and nonfinite values.
- [x] Copy validated18C ONNX and alter only first depthwise initializer and
  attributes. For K13, use `weights[...,8:21].copy()`; rates2/pads12.
  For K29, retain exact weights; rates1/pads14. Keep output width200.
- [x] Retain original FP16 activation input bytes, fresh ONNX/lowering
  identities and actual ORT/independent outputs. Compare with the existing
  finite policy; bind all files in an exclusive preparation receipt.
- [x] Run focused tests and old graph/source regressions; commit owned files.

## Task2 — REA rejection trace

**Read-only:** exact firmware `libOpenVX.so`, retained PoC18/REA evidence,
matching TIM-VX implementation. **Write:** private `poc19/rea/` report/evidence.

- [x] Reuse saved function Evidence for graph kernel initialization, Conv
  initializer, depthwise/dilation routes and hardware feature predicates.
- [x] Open only the unresolved native target via the existing REA workflow,
  inspect rejecting callees/assembly with complete Evidence saved to disk.
  Never load the ARM vendor library into a host process.
- [x] Report exact address/branch/data evidence, inferred conditions and
  boundaries; correlate with Task3 outcomes once available; close owned session.

## Task3 — Build, review and bounded isolation runs

**Reuse:** package importer/source generator/native bridge. **Artifacts:**
`poc19/packages/19a`, `19b`, frozen controls and continuous snapshot ledger.

- [x] Import/build both diagnostic packages offline, inspect ELF/hash/source
  equality and obtain independent review before device contact.
- [x] Capture initial read-only health and retained overlapping diagnostic
  snapshots during host work. Launch19a compile once; qualify once only if
  successful. Inspect exit/output/cleanup before any next device action.
- [x] Repeat for independent19b unless a fault/timeout/health/numerical gate
  stopped all native work. Record clean negatives without retry.

## Task4 — Evidence-selected exact compatible lowering

- [x] Select a supported route using Task2/3 facts; record exact graph/source
  contract and remaining compile/Run budget before implementation/launch.
- [x] Prove learned-tap accounting, fixed tensor geometry and original FP32/
  folded FP16/logit/probability parity on all four host clips. Add meaningful
  wrong-shift/pad/group/tap and unsupported-op refusal tests.
- [ ] Qualify the rewritten problematic block and remaining untested trained
  signatures in dependency order, once each, with reviewed sources/ELFs.
- [ ] Qualify the complete one-NBG graph and four frozen clips if prerequisites
  pass; then perform the reviewed4warm/32measured same-device benchmark.
  Preserve the existing explicit CPU fallback and strict eligibility rules.

## Task5 — Review, evidence and closure

- [ ] Actual qualified-load/tamper regression with no CPU session/transport;
  final global state, diagnostic chain, owned-root and process checks.
- [ ] Independent source/evidence/numerical/performance review; fix material
  findings with focused regressions.
- [ ] Record actual matrix/limits/compatible graph and timings, or precise
  unresolved native blocker, in a notebook/JSON ledger and model guide.
  Mark steps answered/partial/skipped honestly; commit only owned paths.

Adaptive Task4 details are selected from actual evidence, not invented before
the two isolation results exist. A negative remains an outcome; unresolved
questions do not justify unsafe target execution or false qualification.

### Approved Wi-Fi pre-launch stabilization

- [x] Preserve all three genuine pre-native Wi-Fi WARN intervals and the alert
  detector's separate `changed` false-positive regression. No NPU calls occurred.
- [x] Owner approved temporary Wi-Fi disable and restoration. Capture
  original state; disconnect supplicant without configuration save, lower
  interface; leave services running.
- [x] Verify manual DISCONNECTED state and quiet new baseline, then
  launch only reviewed/frozen controls under the unchanged native stop gate.
- [ ] Restore original UP/association state and verify at final closure.

### Task4 selected contract — explicit temporal padding

Both isolation controls compiled on NN and passed one native Run: K13/rate2
and K29/rate1 each had maximum element error0.000244140625. This leaves the
combined K29/rate2 signature, including implicit pad28/effective extent57,
unresolved. REA found a signed Conv padding eligibility guard and a native
NN/TP static Pad route; it did not prove the historical rejection branch.

The next independent candidate19c preserves every original initializer bit:
FP16 NCW `[1,64,200]` → constant zero temporal Pad28 on each side →
`[1,64,256]` → original K29/rate2 depthwise Conv with explicit pads0/0 →
`[1,64,200]` → unchanged pointwise/bias/Relu. No learned filter replacement,
extra multiply-accumulate, clipping, or changed activation is permitted.

Admit this through a separate `fp16-marblenet-pad-v1` profile. It permits only
the exact static zero Pad28/28 form with inline INT64 pads and optional
FP16 positive-zero scalar, plus existing bounded Conv/Add/Relu contracts.
The structural constants and canonical Pad fields must be bound to the
original ONNX and lowering signatures. Width256 is intermediate only.
The old profile, generated sources and frozen package semantics stay exact.
TIM-VX Pad uses native width-first four-axis pads `{28,0,0,0}` before/after.

Within the new profile only, flat height1 depthwise temporal stride2 uses
native strides2/2 instead of2/1. This preserves the height1 output exactly
and addresses the generic depthwise predicate requiring symmetric strides.
19d must qualify the original first trained K11/stride2 block under this
policy before any full model launch. The policy must be explicit in the
canonical lowering, source validation and meaningful geometry tests.

Remaining ceiling after isolation:6 compiler processes,12 export API calls,
44 Runs. Planned sequence:19c padded problematic block;19d first block;
19e remaining K15/K17 trained residual signatures;19f full padded graph.
Use one qualification Run per partial graph and four frozen full clips,
then4 warm/32 measured Runs only after every gate passes. Any alternate
after a clean negative requires its own evidence-selected written contract
within this ceiling. Fault/timeout/health/numeric failures stop native work.

Host proof must compare full original FP32 and folded FP16 against this
rewrite on all four original clips, preserve tap/checkpoint identities, and
bind a deterministic derivation receipt. File frontend and benchmark
admission must explicitly verify that receipt rather than weaken the old
full-model hash gate. Independent source/ELF review precedes live launches.

### Task5 continuity correction after the USB incident

Snapshot0015 records a real transfer-event WARNING between19d cleanup and
19e staging.19e's local baseline already contained it, so its local compile
PASS does not clear the failed encompassing interval. Stop native work and
retain this incident, the successful local compile and missing19e/full/bench
qualification separately. Do not relabel a quiet snapshot as recovery.

Correct the inter-run gap with a persistent host phase guard for the new Pad
profile. A reviewed explicit phase seed binds the selected unit and accepted
health snapshot. Check continuity from that accepted anchor before staging
mutations and again on the native runner's immediate preflight, before the
vendor command. Missing/invalid anchors, lost overlap, health deltas and
new warnings block launch and leave the phase HALTED; no automatic reseed.
Advance only from a verified CLEAN cleanup. Preserve previous receipts and
older profile source/qualification semantics. Add meaningful replay tests
for this exact warning present in a new local baseline, plus missing-anchor,
history-gap, changed state, immediate-preflight and failed-cleanup cases.

This is a host guard correction; it authorizes no new device experiment,
driver reset, USB configuration change, or service action. Starting a new
eligible phase requires a documented incident disposition and authorization.
Attempt the already-authorized Wi-Fi restoration; if ADB is absent, retain
the failed restoration and request reconnection while completing host work.
