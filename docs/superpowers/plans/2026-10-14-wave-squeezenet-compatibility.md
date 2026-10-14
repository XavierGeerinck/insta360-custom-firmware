# Wave SqueezeNet Compatibility Implementation Plan

> Execute continuously with the executing-plans skill. REA directs independent
> source/evidence and research questions to separate workers; the controller
> owns every device operation. The owner's "go test" authorizes the proposed
> SqueezeNet compatibility experiment; no repeated permission request is needed.

**Goal:** Qualify a second complete trained model on Wave spatial/pooling/Concat
routes and identify deployable strong noise/echo enhancement candidates.
**Architecture:** Freeze pretrained FP32/FP16 fixtures, extend one bounded graph
family, qualify four small-output real-weight controls, then full four-image
inference and one conditional paired benchmark. Independent audio research
does not depend on native outcomes or modify production processing.
**Tech Stack:** Py3.12.9, Torch2.5.1/torchvision0.20.1, NumPy2.5.1/ONNX1.22/
ORT1.27, unchanged TIM-VX/runtime/image; stock CPU ORT1.15.1.
**Spec:** `docs/superpowers/specs/2026-10-14-wave-squeezenet-compatibility-design.md`.

## Global constraints

- Checkpoint b8a52dc049b60e4b6ab68ad0df457362afab8b6304b2febdc1650a5dab4d7e7b.
- New profile fp16-squeezenet-v1, source/evidence under artifacts/npu/poc20.
- New budget6 exports/12 API calls/30 Runs; selected sequence5/10/28 maximum.
- Immutable finite/native0.03+0.015absref, FP32-FP160.05+0.02absref,
  softmax0.01/top1identical/top5overlap4/5 on four frozen inputs.
- Compile15/20s, qualification8/12s, paired benchmark30/40s through teardown.
- Fresh reviewed persistent phase, single controller, no native retry/recovery,
  no service stops or stock libraries/settings changes; restore temporary Wi-Fi.
- Tensor8MiB/96nodes/initializer8MiB/regular artifact64MiB; only fixed observed
  spatial profiles. No inherited general ceil/depthwise/INT8 claims.

## Review focus

- NCHW/WHCN and OIHW/WHIO mapping: non-square control and distinct values detect
  axis swaps; Concat keeps exact channel order.
- Model semantic fidelity: no Dropout in eval export, exact coefficient
  accounting, explicit pool/global-average/Flatten source and dimensions.
- Manifest/source/ELF/NBG/input/readback identities and finite/canary guards;
  bound total emitted hex and stack buffers independently from model tensors.
- Separate host references/native output, FP16/logit/probability/top-k metrics,
  real source images and preprocessing/class ordering.
- Continuous encompassing health and no silent reseed; final Wi-Fi/root/system
  verification remains separate from benchmark success.

## Task1 — Frozen SqueezeNet fixtures

**Create:** `waved/tools/onnx-hybrid/src/wave_onnx_hybrid/squeezenet_model.py`
and `tests/test_squeezenet_model.py`; artifacts under poc20/prepared.
**Interface:** `prepare_squeezenet(assets:Path, output:Path)->dict` emits five
FP16 models, original FP32 model, four full-input references and control inputs.

- [ ] Establish RED for wrong checkpoint/preprocessing/parameter count, eval
  Dropout, source-mutation/linked paths and unsafe existing output.
- [ ] Load trusted checkpoint with weights_only=True. Use torchvision0.20.1
  eval model, original CPU outputs and exact explicit FP16 conversions.
- [ ] Export standard IR8/opset13, identify actual node/shape inventory, slice
  frozen original image/activations for the four specified controls.
- [ ] Freeze every acquired/derived byte, expected outputs, all parameter bits,
  top-k/class margins and environment in exclusive preparation receipt.
- [ ] Verify ORT against Torch FP32 and independent FP16 after Task2; commit
  owned fixture source/tests when all immutable references pass.

## Task2 — Bounded spatial graph family and native sources

**Create:** `vision_graph.py`, `vision_sources.py`, focused tests and separate
`waved/tools/npu-compiler/vision/` compile template.
**Modify:** `native_import.py`, `native_sources.py` only for explicit new-profile
dispatch. Controller owns `npu_onnx_native_lab.py` and phase/shared health hooks.
**Interface:** import/validate/oracle/output comparison and exact compile/execute
source functions follow existing package contracts with the new profile ID.

- [ ] RED tests for asymmetric shapes/weights, channel ordering, unconsumed
  initializers, invalid pool geometry, unknown attrs/operator, unsafe constant
  payload/metadata and resealed incompatible DAGs.
- [ ] Add bounded batch1 Conv/Relu/MaxPool/Concat/GAP/Flatten importer and NumPy
  oracle, fixed numeric policy and explicit native spatial source generation.
- [ ] Preserve all earlier profile/source bytes; add mandatory continuity phase
  and new-profile reference/readback/metadata validation in the controller bridge.
- [ ] Run focused and previous profile/source tests, actual source regeneration,
  pure qualified-load/tamper checks, and independent source review before launch.
- [ ] Commit only assigned source/test/template files.

## Task3 — Native control and full-model qualification

**Reuse:** current package lifecycle and pinned offline build.
**Artifacts:** poc20/packages20a–20e, exact sources/ELFs, raw run receipts and
continuous snapshot ledger. No vendor libraries are loaded into host processes.

- [ ] Import/build all five frozen candidates offline; inspect source/binary
  identities, dependencies/ELF protections and native operation eligibility.
- [ ] Capture selected-unit baseline and a new60-second quiet eligible interval;
  independently review explicit new phase seed and each live control source.
- [ ] Export each already-selected independent control once, qualify successful
  controls once and stop on fault/health/numerical gates. Clean unsupported
  outcomes skip dependent full stages and remain explicit negatives.
- [ ] After all prerequisite signatures qualify, export20e once, qualify first
  full image once, run the other three fixed inputs once, enforce full parity.

## Task4 — Conditional paired benchmark and closure

**Create:** bounded vision benchmark generator/template and controller script,
meaningful host parser/source-policy tests, SqueezeNet guide/notebook/JSON.

- [ ] If full four-image gate passes, prepare one paired program with4 warm-up
  and16 measured fixed-image pairs, CPU FP32/stock one-thread ORT and exact
  qualified FP16 reference. Review exact source/ELF/receipt before one launch.
- [ ] Independently verify actual output, raw timing samples/statistics, stock
  target routes, complete phase chain, numeric/cleanup and used budget.
- [ ] Verify final pinned state/services/boot/parameters, diagnostic overlap,
  all staging roots absent and no lab processes/galcore holders. Restore and
  verify original Wi-Fi when temporarily disconnected.
- [ ] Retain actual qualified-load/tamper refusal evidence without CPU/ADB;
  final fixture/package integrity and tests; update observed compatibility
  docs with success or exact native blocker, not proposed operator claims.
- [ ] Final whole-branch review; fix material findings; commit only owned paths.

## Task5 — Noise reduction and echo cancellation model shortlist

**Research:** primary author/source/model repositories and exact public weights;
private report poc20/audio-model-research.md and tracked sourced model guide.

- [ ] Compare quality-focused and lightweight trained noise reduction candidates
  with the measured Wave DFN3 CPU baseline, sample rates/latency/state/operators.
- [ ] Find accessible AEC/joint enhancement checkpoints with microphone and
  far-end-reference inputs; distinguish NS-only DeepVQE ports from genuine AEC.
- [ ] Record input/uplink versus received-output speech roles, double-talk and
  reference-alignment requirements, trained weight/source/license evidence,
  unknown Wave compatibility and next bounded test priority. No integration.

## Decisions and initial negatives

- The initial14-Run draft was too small; corrected before any native call to
  30 with exact planned28. No budget borrowed from PoC19.
- Full224 pool ceil/floor coincidence cannot qualify general ceiling behavior;
  the profile admits only observed equivalent geometries.
- First Grace Hopper download URL404 remains retained; replacement comes from
  pinned matplotlibv3.10.0 with full hash. No native retry is involved.
