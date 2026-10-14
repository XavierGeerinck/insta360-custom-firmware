# Wave SqueezeNet Mean Repair Implementation Plan

> Continue the owner's requested fix through the established inline controller
> workflow. REA assigns independent source/reconstruction/evidence questions;
> only the controller operates the Wave. No repeated go confirmation is needed.

**Goal:** Qualify a bounded mean lowering and complete SqueezeNet inference if
its prerequisite controls pass, then select the best next audio model test.
**Architecture:** Original ONNX/weights remain intact; separate native profile
emits fixed depthwise mean coefficients. Reuse frozen four-image/reference data
and lifecycle with PoC21 roots/phase. Host audio investigation stays independent.
**Tech Stack:** unchanged TIMVX/runtime/offlinetoolchain, PoC20 isolated Python.
**Spec:** docs/superpowers/specs/2026-10-14-wave-squeezenet-mean-design.md.

## Global constraints and review focus

- Budget4/8/8 processes/publicAPI/Run; selected3/6/6, no benchmark/retries.
- Mean169half weights0x1e0f perchannel, multiplier1, stride1,pad0,rate1, F32zero bias.
- Native0.03+0.015absref; four-inputFP32 logits0.05+0.02absref/prob0.01/top1same/top5≥4.
- Bind inverse metadata-only derivation to exact originals; no learned bit changes.
- Include synthesized native payloads in source/constant/stack bounds.
- Preserve old sources/profiles/evidence; new persistent phase and closed oldbudget.
- Final health/system/roots/boot/holders/Wi-Fi checks independent of native success.
- Audio statefulness/reset/correctwindows differ from finite smoke success.

## Task1 — Mean profile and exact source

Owned reconstruction unit: new wave_onnx_hybrid/vision_mean.py,
tests/test_vision_mean.py, explicitnative_import/native_sources dispatch;
vision_sources.py conditional native mean emission. Leave vision_graph.py
and old compiler templates unchanged. Interfaces derive_mean_model(bytes)->bytes,
make_mean_control()->bytes, import_graph(Path)->dict, validate_graph_record(dict),
independent_graph_oracle(record,inputs)->dict, compare_output(record,ref,actual).
Mean-only21a record fixed I/O; classifier/full keep original control20d/20e IDs
while distinct profile maps them into PoC21. Immutable new native policy binds
source and synthesized coefficients. Original oracle remains authoritative.

- [ ] RED: unsafe metadata/parent reconstruction, changed mean policy/weights/group,
  scalar diagnostic contract, resealed record; legacy bytes unchanged.
- [ ] Implement marked derivation and validator/oracle wrappers, source emission
  with NN Conv2d depthwise WHIcOc weights[13,13,C,1], multiplier1 and promoted zero bias.
- [ ] Generate actual three sources/reference bytes, verify learnedbits, host
  approximation/classification policies and previous profile/source tests; commit.

## Task2 — Controller and frozen packages

Controller owns npu_onnx_native_lab.py, npu_compile_lab.py and
scripts/npu/tests/test_npu_vision_mean_lab.py. New profile in graph/phase sets,
root21/poc21 dispatch, reference/output policy, explicitmean operation sentinel.

- [ ] RED: distinctroot/profile, mandatoryphase, uncompiled refusal beforephase,
  old bounds/policy and byte sources unchanged; then GREEN smallestbridge changes.
- [ ] Freeze21a actualcheckpoint classifier-map crop; preserve original prepared
  references; derive21b/c metadata-only models, import/build offline threepackages.
- [ ] Independently review exact source/ELF/model/reference/mean arithmetic.

## Task3 — Native qualification and closure

- [ ] Capture pinnedunit/read-only baseline and reviewed freshquietcurrentboot
  seed; temporaryWiFi disconnect/restoration as already authorized, no servicesstop.
- [ ] Export/qualify21a once, conditional21b once, conditional21c export and
  four original frozen inputRuns once. Strictfresh/preflight/cleanup/health gates.
- [ ] Retain canonicalnegative/fault outcomes; skipdependents or stop as specified.
- [ ] Purequalified-load/tamper/refusal checks, fixture/package integrity,
  finaldevice/system/roots/diagnostic and originalWiFi restorechecks.
- [ ] Update exact cause/repair/compatibility guide/ledger/notebook/hardware;
  independentfinalreview and ownedcommits. No performanceclaimwithoutbenchmark.

## Task4 — Audio candidate decision

- [ ] Reconcile matchedtraining/checkpoint quality, actualstate/operators,
  runtime/library eligibility, and AEC DSP/reference/export limitations.
- [ ] Hostfinite/state probes on retainedGTCRN/ULUNAS: originalfailures/reset
  discrepancies retained, correctanalysis/synthesiswindows required for parity.
- [ ] Select best immediately reviewable nexttest; publish primarysourced
  recommendation and actualhostevidence without Wave speed/quality promises.
