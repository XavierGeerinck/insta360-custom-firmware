# Wave MarbleNet NPU Implementation Plan

> **For agentic workers:** Execute the approved design continuously in the
> existing `feat/m3` checkout. Use independent scopes under REA's explicit
> parallel-investigation rule, then source/evidence review. The controller
> owns every device call. The owner already approved the proposed execution
> with `go`; preserve that authorization and do not ask again between tasks.

**Goal:** Deploy a real pretrained MarbleNet lab package, qualify its trained
FP16 graph on the Wave, compare speech decisions and measure same-device cost.

**Architecture:** Source-matched checkpoint reconstruction provides an
unfused FP32 reference, folded FP32 and FP16 ONNX graphs and actual activation
fixtures. A bounded Conv/Add/Relu importer and immutable source generator
feed the existing package/health bridge with explicit numerical qualification.
Audio preprocessing, transpose, softmax and VAD decisions remain CPU-owned.

**Tech Stack:** Python3.12, pinned Torch2.5.1/ONNX1.22.0/ORT1.27.0/NumPy2.5.1,
TIM-VX C++17, exact device SDK and offline AArch64 Docker toolchain.

**Spec:** `docs/superpowers/specs/2026-10-13-wave-marblenet-npu-design.md`.

## Global constraints

- Checkpoint `84bda37e925ac6fd740c2ced55642cb79f94f81348e1fa0db992ca50d4b09706`,
  HF revision `9d25eb877d9edab5101ccc843edd04787a6c72c4`.
- NeMo source `ceeafa4f2b8cece8a0fd47b29051ad659e55b76c`; BN epsilon `1e-3`.
- Audio63,840 samples at16kHz; features `[1,80,400]`; logits `[1,2,200]`.
- TIM-VX `702715d714277376d7ff466b7b72833ff57564c1`; runtime
  `c984e063e9ab4fe4d7e7ac85e783d88a330f1560268924ba4f449f5b258478dc`;
  provenance `58601a21a3ac1ff0435833d5d30409a43332a892c77047428cb9a54d658efade`.
- Docker image
  `sha256:a45f1308adfe3fc3b90e329e72d5efe9d743e6cc77017254d28bbba1c689586a`;
  no network, read-only source/SDK, inspected ELF.
- Four compiles/eight query-export API calls maximum. Seven qualification
  Runs maximum, plus4warm/32measured qualified full-graph Runs:43maximum.
- Compile15/20seconds native/host; inference8/12seconds. Stop on fault,
  timeout, ambiguous exit, failed numerical gate or health change; no retry.
- No service/driver/system-library/firmware/NPU-setting writes or recovery;
  CLC excluded. Use exact new inventories and remove staging promptly.
- Numerical tolerance only for the new profile: every element finite and
  `abs(error)<=0.02+0.02*abs(reference)`; full speech-probability error<=0.02
  and decisions agree>=99%. Old PoC17 remains byte-exact.
- Raw checkpoint/audio/SDK/device logs stay ignored under `artifacts/npu/poc18/`.

## Review focus

- A different BN epsilon, invented activation or incomplete checkpoint
  mapping must fail host parity before a native build.
- ONNX C-order `[O,I,K]` already matches native temporal-fast coefficient
  memory; a transpose or wrong depthwise multiplier must fail layout tests.
- Native dilation is a rate, not rate-minus-one; the trained dilated control
  must distinguish rate2 from1 before full-model execution.
- Numeric qualification must bind actual native and actual CPU bytes
  independently. Changing either must refuse without CPU session or ADB.
- A20ms output interval is not causal latency: retain model context and
  preprocessing/copy/orchestration cost in all deployment/timing claims.

## Shared record interface

The new profile ID is `fp16-marblenet-v1`; schema_version1. It retains the
existing source_model_sha256, graph_signature, onnx_input/output,
native_input/output and lowering_sha256 fields. Native tensors are
`[T,1,C,1]`; names remain host-only. Add `constants` mapping initializer
names to `{name,dtype,shape,byte_size,payload_hex,sha256}` in ONNX order,
and `nodes` in topological order:

```python
conv = {
    "type": "Conv", "inputs": ["x"], "output": "y",
    "input_shape": [1, 80, 200], "output_shape": [1, 128, 200],
    "weights": "w", "bias": "b", "kernel": 1, "stride": 1,
    "dilation": 1, "pads": [0, 0], "group": 1,
}
relu = {
    "type": "Relu", "inputs": ["y"], "output": "z",
    "input_shape": [1, 128, 200], "output_shape": [1, 128, 200],
}
addition = {
    "type": "Add", "inputs": ["main", "skip"], "output": "sum",
    "input_shapes": [[1, 64, 200], [1, 64, 200]],
    "output_shape": [1, 64, 200],
}
```

Missing Conv bias is `None` and lowered to FP32zero bias. Operation is
`{"type":"ConvGraph","node_count":len(nodes)}` and numerical_policy is
`{"atol":0.02,"rtol":0.02}`. Lowering hashes use existing canonical_json
excluding lowering_sha256. Admit opset13, static one-input/output FP16,
at most48nodes, channels in `{2,64,80,128}`, temporal widths in `{200,400}`,
kernels in `{1,11,13,15,17,29}`, stride/dilation1or2, explicit symmetric
pads, group1or depthwise(group=Cin,Cout=Cin). Validate all constants/edges,
reject unused/overridable/external constants and all other operations.
Eligibility still requires exact package qualification, not this profile ID.

## Task1 — Checkpoint reference, audio fixtures, export and importer

**Create:** `waved/tools/onnx-hybrid/src/wave_onnx_hybrid/marblenet_model.py`,
`marblenet_graph.py`, `tests/test_marblenet_model.py`, `test_marblenet_graph.py`.
**Modify only dispatch:** `native_import.py`.

**Interfaces:**
`prepare_marblenet(checkpoint:Path, output_dir:Path, speech_wav:Path)->dict`;
`import_graph(path:Path)->dict`; `validate_graph_record(record:dict)->None`;
`independent_graph_oracle(record:dict, inputs:dict)->dict`;
`validate_graph_output(record:dict, expected:bytes, observed:bytes)->dict`.

- [ ] Write missing-module/refusal/layout tests, observe RED, then implement
  exact checkpoint hash/member/state validation and the source-matched
  unfused Torch reference. Load weights_only and account for all84tensors.
- [ ] Verify BNfold and classifier-to-Conv transformation against unfused
  FP32, then ORTFP32 and ORTFP16. Reuse serialized window/filterbank; test
  waveform63840→400frames, epsilon0.001 and no DW/PW intermediate activation.
- [ ] Generate deterministic silence, low-amplitude seeded noise, public
  speech and noisy-speech windows. Freeze wave/feature/probability hashes.
  Emit original/folded FP32 and folded FP16 ONNX and18A/B/Csubgraphs with
  actual activation inputs, outputs and provenance under the exclusive
  output directory. Public speech source is NVIDIA's model-card example:
  `https://dldata-public.s3.us-east-2.amazonaws.com/2086-149220-0033.wav`.
- [ ] Implement the exact record interface and NumPy oracle. Test actual
  coefficient order/depthwise groups, wrongdilation, broadcasting, changed
  parameters, graph wiring, nonfinite/output-bound and stale identities.
  Add profile dispatch without broadening either existing PoC17 profile.
- [ ] Run meaningful focused tests, old importer/regression checks and lint;
  commit owned code. Record full reference/export/fixture receipts.

## Task2 — Native graph source generation

**Create:** `marblenet_sources.py`, `test_marblenet_sources.py`,
`waved/tools/npu-compiler/marblenet/compile_fp16_graph.cc.in`.
**Modify only dispatch:** `native_sources.py`.

**Interfaces:** same generate/validate_compile_source and execute_source
signatures as native_sources, consuming the shared graph record.

- [ ] Write source-policy tests showing wrong root, edited template,
  altered constants, extra native call and premature alarm cancellation
  refusal; observeRED before implementation.
- [ ] Derive fixed compiler lifecycle from immutable PoC17 template; emit
  numeric identifiers and bit-pattern arrays only. Conv nodes use actual
  FP16weights and exactly converted FP32bias; depthwise multiplier1,
  dense multiplier0, explicit pads `{L,R,0,0}`, stride `{s,1}`, dilation
  `{d,1}`. Bind Add/Relu with exact same-shape tensors. Retain operations and
  tensors until teardown, then release all before PASS.
- [ ] Validate graph records before generation. New remote roots are
  `/tmp/wave-npu-onnx18-<loweringhex16>-compile`/`-execute`; executables keep
  versioned filenames. Reuse pinned input-file executor lifecycle with
  generated tensor sizes/shapes and operation `fp16-marblenet`.
- [ ] Source equality/template hashes control admission. Verify exactly
  one size query/export and one Compile/Run, guarded canary after Compile,
  atomic output and alarm through teardown. Build only offline on host.
- [ ] Run source/layout/refusal tests and oldsource regressions; commit owned
  changes and retain a report. No device calls from this task.

## Task3 — Package bridge and numerical qualification

**Modify:** `npu_onnx_native_lab.py`, its focused tests, minimal shared
`npu_compile_lab.py`/`npu_erf_execution_lab.py` and `native_backend.py`.

- [ ] Write RED tests for new-profile byte-size bounds, actualoutput parsing,
  numeric failure, preserved oldexact policy and noCPU/ADB on changed evidence.
- [ ] Derive PoC18 roots/policies from profile, keep oldpackage manifests
  byte-identical and extend service/diagnostic gates to18. Bound file reads
  by exact tensor contracts up to8MiB rather than32bytes for newprofile.
- [ ] Preserve native output hex/file bytes in envelope; compare them to
  independent oracle with fixed policy. Retain actualORTCPU output bytes,
  CPU evidence, independent reference and native bytes with separate hashes.
  Require successful numerical/cleanup evidence in qualified loading.
- [ ] Strict native sessions use NumPy only and the pinned comparison;
  hybrid latches CPU fallback after onefailed dispatch. Oldprofiles remain
  byte-exact and all four existing qualified packages reload unchanged.
- [ ] Run affected bridge/parser/execution and ONNX regression checks. Build
  inspected compiler packages for18A–D. Independent source review must pass
  before any new live call; controller executes device operations only.

## Task4 — Four bounded live controls and full-model VAD comparison

**Create artifacts:** `artifacts/npu/poc18/frozen-controls.json`, packages,
run receipts, actualreadbacks, numerical metrics and finalhealth.

- [ ] Freeze all identities after hostparity/source/build checks. Compile
  18Aonce and qualify once; repeat18B and18C only if prerequisites passed.
- [ ] Compile18Donce and qualify against its frozen reference. If qualified,
  execute the remaining three frozen audio inputs once each. No retry.
- [ ] Compare native softmax probabilities with source-matched original
  FP32; requiremaxerror<=0.02 and decisionagreement>=99%. Report perclip
  speech statistics and disagreements; label synthetic inputs clearly.
- [ ] Validate/remove each owned root promptly. Nativefault or unhealthy
  state endsremaininglive work; clean negative skips dependent controls.
  Retain actual negative and continue useful host preparation/documentation.

## Task5 — Same-device benchmark and deployment command

**Create:** dedicated pinned benchmark source/harness and numerical/timing
tests; document exact binary/API identities before execution.

- [ ] Build reviewed CPUrunner for existing WaveORT and foldedFP32graph;
  no host dependencies or new libraries installed on device. Verify CPU
  readback against hostORT within predeclared floating tolerance.
- [ ] Only after fullgraph numerical qualification, build fixed NBG runner
  with4warm/32measured iterations and bounded input/canary/output validation.
  Keep8second alarm through allnative/teardown; onegraphCompile only.
- [ ] RetainCPU/NPU raw samples, median/p95, input/output copytime, Run time
  and fullboundary cost. Separate process/ADB startup from graph timing and
  state preprocessing/context costs. Report slowdown as readily as speedup.
- [ ] Provide explicit hostfile-oriented command for qualified MarbleNet
  VAD, usingexisting nativebackend and CPUpre/postprocessing; no production
  wavedservice integration. Test model/input/qualification tamper refusal.

## Task6 — Final review, durable evidence and completion

**Create:** PoC18notebook, MarbleNet guide and sanitized result ledger.
**Update:** NPU/ONNX README and COMPUTE_HARDWARE with observed coverage only.

- [ ] Actualqualified strictload/changed-artifact checks create noCPU/ADB.
  Finalglobalhealth compares identical systempaths; inspect allderivedroots
  absent and no nativeprocess remains. Preserve rawdiagnostics privately.
- [ ] Independently review source and actualnative/evidence/timing claims;
  resolve material findings with targeted regression tests.
- [ ] Record exactversions/checkpoint/model/build/NBG/output/evidence hashes,
  tolerances, lookahead and explicit failures. Keep futuremodel expansion
  separate from exactqualification. Mark plan steps honestly and commitowned
  changes without push, merge or service deployment.

## Execution record

Work remains in theexisting `feat/m3` checkout to retain exactSDK/runtime
evidence. The user's `go` approves the preceding recommendation and design;
source discovery refines fixed preprocessing/operator details. Rawprogress
and scoped rulings are recorded at `artifacts/npu/poc18/progress.md`.
