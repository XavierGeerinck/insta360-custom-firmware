# Wave NPU Learned Erf Noise Suppression — Implementation Plan

**Date:** 2026-07-29

**Design:**
`docs/superpowers/specs/2026-07-29-wave-npu-learned-erf-noise-suppression-design.md`

**Planning workflow:** Manual test-first fallback because the required
`writing-plans` skill is not installed in this session

**Execution order:** Complete and qualify host PoC 13A; perform exactly two
compile-only captures; qualify the sealed package; perform six numerical NPU
executions; produce one NPU-derived enhanced waveform; integrate and run the
10,000-frame qualification soak.

## 1. Objective

Deliver the exact milestone defined by the approved design:

```text
48 kHz noisy speech
  -> CPU causal 960-point STFT and noise estimate
  -> learned 481-bin affine transform
  -> UINT8 [481] Wave NPU Erf
  -> CPU mask application and inverse STFT
  -> measurably enhanced waveform
```

The implementation is complete only when:

- the learned model passes held-out quality and ablation gates;
- the exact NBG compiles deterministically for `0x1000000e`;
- six bounded NPU vectors match the independent CPU oracle;
- NPU-returned masks produce the enhanced waveform;
- strict placement, hybrid fallback, latency, and 10,000-frame soak evidence
  pass;
- every result is documented and hash-pinned.

Compilation alone, CPU-only enhancement, an analytic gate, or a graph that
returns nonzero bytes does not complete the plan.

## 2. Working-Tree and Tool Rules

The worktree contains substantial earlier NPU work.

Rules:

- preserve every unrelated modification and untracked file;
- edit local files with `apply_patch`;
- use formatting tools only for mechanical rewrites;
- never reset, clean, checkout, or overwrite unrelated work;
- stage or commit only an exact path after verifying the staged set;
- run every shell command through `lean-ctx -c '...'` or
  `lean-ctx raw '...'`;
- keep downloaded corpora and raw device runs below ignored
  `artifacts/npu/`;
- use exact temporary roots and refuse pre-existing roots;
- never delete a workspace root, broad `/tmp` path, glob, or unresolved
  environment-variable target;
- do not spawn subagents;
- do not call the device before the relevant host and policy gates pass.

## 3. Architectural Boundaries

### 3.1 Python host model

Extend the existing `wave-onnx-hybrid` project. It already pins NumPy, ONNX,
ONNX Runtime, pytest, and Ruff and already implements deterministic NPZ/JSON
artifacts and strict graph signatures.

New modules:

```text
waved/tools/onnx-hybrid/src/wave_onnx_hybrid/
  erf_mask_signal.py
  erf_mask_corpus.py
  erf_mask_train.py
  erf_mask_model.py
  erf_mask_metrics.py
  erf_mask_package.py
  erf_mask_cli.py
```

New tests:

```text
waved/tools/onnx-hybrid/tests/
  test_erf_mask_signal.py
  test_erf_mask_corpus.py
  test_erf_mask_train.py
  test_erf_mask_model.py
  test_erf_mask_metrics.py
  test_erf_mask_package.py
  test_erf_mask_cli.py
```

The host modules have no ADB, compiler, NN SDK, or device import.

### 3.2 Device compile and evidence harnesses

Reuse the fixed-source/fake-transport pattern from PoCs 5C, 7, 8, and 12B.
Every native executable has a closed argument set and fixed graph/tensor
contract.

### 3.3 Rust real-time integration

Extract the reusable NN SDK ABI from `waved-npu-probe` into a narrow runtime
crate. Keep signal/model/fallback logic in `waved-npu`. Keep period buffering
and noise-suppressor ownership in `waved-audio`.

Dependency direction:

```text
waved-npu-runtime
  -> libloading + exact NN SDK ABI only

waved-npu
  -> model math + STFT + executor traits
  -> optional waved-npu-runtime backend

waved-audio
  -> generic NpuInfer-backed period buffer
  -> opt-in backend selection
```

The audio layer never receives arbitrary graph bytes or NN SDK symbols.

## 4. Phase A — Host-Only PoC 13A

### Task 1 — Lock the host baseline

No files change in this task.

Run:

```text
cd waved/tools/onnx-hybrid
uv sync --frozen
uv run pytest
uv run ruff check .

cd ../../..
python3 -m unittest discover -s waved/scripts/npu/tests -p 'test_*.py'

cd waved
cargo test -p waved-npu -p waved-audio
```

Record:

- exact command;
- dependency lock hash;
- test count;
- pass/fail;
- existing unrelated failures, if any.

Do not weaken or skip an existing test to obtain a baseline.

### Task 2 — Implement the quantized Erf oracle first

Create failing tests in `test_erf_mask_signal.py` for:

- constants: 48 kHz, hop 480, FFT 960, 481 bins;
- exact input quantization endpoints;
- ties-to-even behavior at positive and negative half units;
- saturation outside `[-4, 3.96875]`;
- a 256-entry immutable Erf table;
- exact table endpoints and zero point;
- output range `0..255`;
- monotonic output over all 256 U8 inputs;
- mask conversion `q / 256`;
- `0.05` mask floor;
- NaN and infinity refusal;
- input arrays remain unmodified.

Implement in `erf_mask_signal.py`:

```text
ErfMaskConfig
build_erf_u8_table()
quantize_logits()
cpu_erf_u8()
mask_from_erf_u8()
```

Use `math.erf` only while constructing the 256-entry oracle. Store the
generated table in the model package and verify its SHA-256. Do not depend on
an accelerator math library for the golden.

Run:

```text
uv run pytest tests/test_erf_mask_signal.py -q
uv run ruff check src/wave_onnx_hybrid/erf_mask_signal.py \
  tests/test_erf_mask_signal.py
```

### Task 3 — Implement deterministic streaming STFT

Extend `test_erf_mask_signal.py` with failing tests for:

- periodic square-root Hann coefficients;
- the squared half-overlap sum;
- one-sided 481-bin shape;
- one-hop startup zero output;
- no future-sample dependence;
- exact 480-sample output length;
- identity-mask reconstruction after startup;
- impulse reconstruction;
- sine reconstruction;
- 10,000-hop identity run without state drift;
- reset returning to byte-equivalent initial state;
- wrong hop length refusal;
- non-finite input refusal;
- input immutability.

Implement isolated types:

```text
StreamingAnalysis
StreamingSynthesis
StreamingStft
```

Use NumPy `rfft`/`irfft` with explicit normalization and contiguous
`float32` public arrays. Keep internal accumulation in `float64` where it
reduces deterministic reconstruction error. Do not add a second framing
implementation.

Pass thresholds:

- identity reconstruction max absolute error below `2e-6` after startup;
- identity reconstruction RMS error below `1e-7`;
- no lost or duplicated output hop.

### Task 4 — Implement the exact causal noise estimator

Add failing tests for:

- first-frame initialization;
- `epsilon = 1e-12`;
- power smoother coefficients `0.8/0.2`;
- 100-frame per-bin minimum ring;
- `1.5` minimum bias;
- noise-dominant update `0.9/0.1`;
- speech-dominant update `0.995/0.005`;
- posterior threshold `3.0`;
- independent bins;
- deterministic reset;
- bounded positive output;
- 1,000-frame reference comparison against a slow scalar oracle.

Implement:

```text
CausalNoiseEstimator
posterior_log_snr_feature()
```

The ring state is fixed-size. No clean or noise-reference tensor appears in
the inference API.

### Task 5 — Implement deterministic procedural corpus generation

Create `test_erf_mask_corpus.py` with failing tests for:

- strict corpus configuration keys;
- duplicate JSON key refusal;
- disjoint train/validation/test seeds;
- exact SNR grid `[-10,-5,0,5,10]`;
- voiced, unvoiced, silence, and speech-only source categories;
- white, pink, narrow-band, 50 Hz hum, impulsive, and
  amplitude-modulated-noise categories;
- stable PCM and metadata hashes;
- measured mixture SNR within `0.05` dB of target;
- no clipping or non-finite samples;
- no global NumPy RNG use;
- output non-overwrite;
- canonical JSON and deterministic WAV output.

Implement:

```text
ProceduralCorpusConfig
CorpusItem
generate_clean_source()
generate_noise_source()
mix_at_snr()
generate_procedural_split()
```

Use one explicit `numpy.random.Generator(PCG64(seed))` per item. Derive item
seeds from canonical item identity, not generation order.

Write raw generated corpus only below:

```text
artifacts/npu/corpora/erf-mask-procedural-v1/
```

### Task 6 — Implement safe SLR31 acquisition and selection

Add host dependencies through `uv` and update `pyproject.toml` and `uv.lock`:

- one FLAC reader;
- one deterministic polyphase resampler;
- one STOI implementation.

The lockfile, not this plan, supplies exact transitive versions. Do not add a
training framework.

Create failing tests for:

- exact allowed source hosts and paths;
- archive names and publisher MD5 values;
- streaming download byte limit;
- partial-download cleanup;
- MD5 and SHA-256 computation;
- checksum mismatch refusal;
- safe tar extraction;
- absolute path, `..`, symlink, hard-link, device, FIFO, and duplicate-member
  refusal;
- extraction below one exact corpus root;
- numeric speaker-ID parsing;
- train/dev speaker overlap refusal;
- deterministic validation/test split;
- sorted utterance selection;
- 2-to-20-second duration rule;
- deterministic 16-to-48-kHz resampling;
- source audio immutability;
- attribution and license evidence in the manifest.

Implement in `erf_mask_corpus.py`:

```text
Slr31ArchiveSpec
download_slr31_archive()
verify_slr31_archive()
extract_slr31_archive()
select_slr31_items()
resample_slr31_item()
```

Fixed archive identities:

```text
train-clean-5.tar.gz
  MD5 5df7d4e78065366204ca6845bb08f490

dev-clean-2.tar.gz
  MD5 6d7ab67ac6a1d2c993d050e16d61080d
```

Download and extraction root:

```text
artifacts/npu/corpora/slr31/
```

No private workspace recording is scanned or imported.

### Task 7 — Build deterministic training examples

Create failing tests for:

- square-root Wiener targets;
- target range `[0,1]`;
- exact feature/target shape `[frames,481]`;
- source/noise STFT alignment;
- balanced SNR and noise-family sampling;
- deterministic frame reservoir;
- no validation/test frame in training;
- exact maximum frame counts;
- deterministic memory-mapped array metadata;
- source item and derived tensor hashes.

Implement:

```text
TrainingExampleBuilder
build_feature_target_pair()
build_balanced_frame_store()
```

Fixed frame budgets:

```text
train:       65,536
validation:  16,384
test:        32,768
```

The procedural training split and `train-clean-5` may populate training.
`dev-clean-2` may populate only validation, test, and the device fixture.

### Task 8 — Implement the deterministic trainer

Create `test_erf_mask_train.py` with failing tests for:

- 481 `alpha` and 481 bias values;
- `a = softplus(alpha) + 1e-4`;
- initial deployed slope exactly `1.0` within `1e-7`;
- deployment quantizer in the forward pass;
- straight-through derivative only inside clipping range;
- exact loss and `1e-4` adjacent-weight regularizers;
- finite gradient;
- deterministic minibatch sequence;
- deterministic Adam state;
- early-checkpoint tie selects the earliest step;
- two complete tiny training runs produce byte-identical weights and history;
- trained tiny fixture loss beats initialization;
- NaN, overflow, wrong shape, and empty split refusal.

Implement:

```text
TrainingConfig
TrainingState
initialize_parameters()
loss_and_gradient()
adam_update()
train_erf_mask()
```

Default full-run training contract:

```text
seed:                 0x57335645
batch size:           512 frames
steps:                2,000
Adam learning rate:   0.01
Adam beta1:           0.9
Adam beta2:           0.999
Adam epsilon:         1e-8
validation interval:  50 steps
checkpoint rule:      lowest validation loss, earliest exact tie
```

Run training on CPU only. Record NumPy, Python, architecture, and thread
environment. Force single-threaded numerical-library settings in the
reproduction command.

### Task 9 — Export weights, ONNX, and canonical package inputs

Create `test_erf_mask_model.py` and `test_erf_mask_package.py` with failing
tests for:

- deterministic `a`/`b` NPZ bytes;
- exact names, dtypes, and shapes;
- exact complete ONNX topology;
- ONNX checker and shape inference pass;
- CPU ONNX output matches the quantized NumPy model;
- source ONNX hash and canonical executable graph signature;
- partition signature covers shape, quantization, operation, and boundary;
- model package rejects unknown/missing keys;
- model package rejects any weight, LUT, ONNX, config, or corpus hash change;
- sealed NBG fields are absent before PoC 13C;
- `execution_approved` cannot be true without numerical evidence;
- deterministic canonical manifest bytes.

Implement:

```text
build_erf_mask_onnx()
serialize_erf_mask_weights()
erf_partition_signature()
build_unsealed_model_manifest()
verify_model_package()
```

Do not set the existing general compiler policy to qualified. Add a separate
sealed-partition registry schema whose first version is empty.

### Task 10 — Implement objective metrics and fixed device fixture

Create `test_erf_mask_metrics.py` with failing tests for:

- SI-SDR identity, scale invariance, known improvement, and silence policy;
- RMS dB behavior;
- noise-only attenuation;
- STOI input validation and sample-rate handling;
- clipping/NaN counts;
- per-item and median aggregation;
- metric version and configuration evidence;
- exact 12-second fixture segment order;
- fixture and clean/noise-reference hashes.

Implement:

```text
si_sdr()
rms_db()
noise_attenuation_db()
stoi_score()
evaluate_audio_item()
aggregate_evaluation()
build_device_fixture()
```

The fixture is six consecutive two-second segments:

1. pink noise only, seed `1301001`;
2. clean held-out speech;
3. held-out speech plus stationary pink/hum noise at `0` dB SNR;
4. held-out speech plus amplitude-modulated noise at `-5` dB SNR;
5. held-out speech plus impulsive noise at `5` dB SNR;
6. silence.

Walk sorted test utterances until eight seconds of speech are available.
Never duplicate or loop an utterance. Fail if the held-out split cannot
supply the fixture.

### Task 11 — Add the host PoC CLI

Create `test_erf_mask_cli.py` before implementation.

Commands:

```text
wave-erf-mask generate-procedural
wave-erf-mask acquire-slr31
wave-erf-mask build-examples
wave-erf-mask train
wave-erf-mask evaluate
wave-erf-mask build-fixture
wave-erf-mask verify-package
wave-erf-mask qualify-host
```

Test:

- strict required arguments;
- no implicit default output outside `artifacts/npu/`;
- exclusive output creation;
- atomic temporary files;
- structured JSON errors;
- stable nonzero exit by error category;
- rerun refusal;
- no ADB/device imports;
- `qualify-host` fails if any metric or hash gate fails.

Register only:

```text
wave-erf-mask = "wave_onnx_hybrid.erf_mask_cli:main"
```

Keep the existing `wave-onnx-hybrid` CLI behavior unchanged.

### Task 12 — Execute and document PoC 13A

Run the focused and full host suites:

```text
cd waved/tools/onnx-hybrid
uv sync --frozen
uv run pytest tests/test_erf_mask_signal.py \
  tests/test_erf_mask_corpus.py \
  tests/test_erf_mask_train.py \
  tests/test_erf_mask_model.py \
  tests/test_erf_mask_metrics.py \
  tests/test_erf_mask_package.py \
  tests/test_erf_mask_cli.py
uv run pytest
uv run ruff check .
```

Run the deterministic host pipeline twice in separate empty roots with
single-threaded numerical settings. Compare every generated model, manifest,
fixture, and metric byte.

Required tracked outputs:

```text
docs/npu/models/erf-mask/README.md
docs/npu/models/erf-mask/model-manifest.json
docs/npu/models/erf-mask/training-manifest.json
docs/npu/models/erf-mask/evaluation-summary.json
docs/npu/models/erf-mask/partition-policy.json
docs/npu/lab-notebook/2026-07-29-poc13a-learned-erf-mask-host.md
```

Required pass gates:

- deterministic artifacts;
- synthetic median SI-SDR improvement at least 3 dB;
- natural held-out median SI-SDR improvement at least 1 dB;
- natural STOI no more than `0.02` below noisy input;
- clean-speech RMS within 1 dB;
- noise-only attenuation at least 6 dB;
- no clipping, non-finite sample, split overlap, or private input.

If the learned architecture cannot pass after correcting implementation
bugs, document the clean negative and revise the design before changing
model architecture or quality thresholds.

## 5. Phase B — PoC 13B Compile-Only Capture

No Phase B task starts unless PoC 13A passes.

### Task 13 — Define compile manifest and fake-transport tests

Create:

```text
waved/scripts/npu/erf_mask_compile_manifest.json
waved/scripts/npu/npu_erf_mask_compile_lab.py
waved/scripts/npu/tests/test_npu_erf_mask_compile_lab.py
```

Write failing tests for:

- exact Wave serial, kernel, galcore device, and runtime identities;
- exact remote root `/tmp/wave-npu-poc13b`;
- exactly two candidates `a1`, `a2`;
- two separate process invocations;
- exact UINT8 `[481]` input/output graph;
- exact scales and zero points;
- exact output names and size bounds;
- required compiler and library hashes;
- source and ELF policy hashes;
- CLC absence and quarantine;
- pre-existing root refusal;
- wrong device or multiple devices;
- unexpected holder;
- A1 failure blocks A2;
- A1/A2 mismatch;
- timeout and partial output;
- parse/CRC/target/tensor mismatch;
- health and cleanup mismatch;
- no graph load, run, retry, reboot, reset, service stop, or raw ioctl;
- no more than two native processes.

Use the existing fake ADB transport and strict JSON loader patterns. Unit
tests make no device call.

### Task 14 — Implement the fixed native compiler

Create:

```text
waved/tools/npu-compiler/tim_vx_compile_erf_mask.cc
waved/tools/npu-compiler/build-device-erf-mask.sh
waved/tools/npu-compiler/verify-device-erf-mask-policy.sh
waved/tools/npu-compiler/stage-device-erf-mask.sh
```

The executable accepts only:

```text
--describe
--a1
--a2
```

Both compile modes construct the exact graph from design section 6.4. The
source has:

- no arbitrary dimensions, type, quantization, model, output, or library
  argument;
- no graph `Compile`, `Run`, tensor upload, or tensor readback;
- two fixed `CompileToBinary` calls per process;
- target/version/header checks before file creation;
- atomic exclusive output below the fixed remote root;
- mode `0444` final files;
- one machine-readable result line.

The policy verifier inspects source and AArch64 ELF imports, symbols, strings,
and control flow. It refuses runtime execution symbols and the quarantined
CLC.

### Task 15 — Cross-build once and bind exact hashes

Reuse only the already approved offline AArch64 build environment. Network is
disabled. If the immutable image is absent, stop rather than pulling an
unreviewed image.

Run:

```text
build-device-erf-mask.sh
verify-device-erf-mask-policy.sh
```

Add the exact source, binary, TIM-VX, SDK header, and build-provenance hashes
to the manifest. Re-run all manifest and policy tests after inserting real
hashes. Sentinel or placeholder hashes are prohibited.

### Task 16 — Perform the two compile-only calls

Preflight:

- one expected ADB device;
- exact serial, kernel, `/dev/galcore`, NPU parameters, and library hashes;
- no galcore holder;
- exact remote root absent;
- sufficient free space;
- no quarantined CLC on the loader path.

Run A1 once. Pull, hash, parse, and validate it before starting A2. Run A2
once. Pull, hash, parse, and compare byte-for-byte.

There is no third compile candidate and no retry.

Document:

```text
docs/npu/lab-notebook/2026-07-29-poc13b-erf-mask-compile.md
```

If compilation, quantization metadata, determinism, or health fails, stop
device work and return to host serializer/compiler analysis.

## 6. Phase C — PoC 13C Sealed Package Qualification

### Task 17 — Qualify the NBG without execution

Create:

```text
waved/scripts/npu/nbg_erf_mask_profile.py
waved/scripts/npu/tests/test_nbg_erf_mask_profile.py
docs/npu/format/erf-mask-nbg-profile.json
docs/npu/lab-notebook/2026-07-29-poc13c-erf-mask-package.md
```

Write failing tests for:

- exact A1/A2 identity;
- VPMN v2 target and file CRC;
- section-20/26 pairing and geometry;
- input/output count, dimensions, type, and quantization;
- zero padding;
- unchanged byte-identical reconstruction;
- truncation and every section corruption;
- wrong target, shape, type, scale, zero point, and graph hash;
- package/artifact cross-hash mismatch;
- execution approval remaining false;
- output non-overwrite and canonical evidence.

Implement by composing `nbg_format.py` and `nbg_dyna_profile.py`. Do not add a
second general NBG parser. Do not mutate any unresolved section.

On pass, populate the unapproved sealed-NBG fields in the model manifest.

## 7. Phase D — PoC 13D Numerical NPU Qualification

No execution source or binary is staged before PoC 13C passes.

### Task 18 — Define the six-vector execution policy

Create:

```text
waved/scripts/npu/erf_mask_execution_manifest.json
waved/scripts/npu/npu_erf_mask_execution_lab.py
waved/scripts/npu/tests/test_npu_erf_mask_execution_lab.py
```

Write fake-transport and oracle tests for:

- exact approved NBG hash and package hash;
- one graph create, six executions, one destroy;
- six exact vector IDs and hashes;
- all-zero-point, minimum, maximum, repeated ramp, seeded random, and learned
  vectors;
- 481 bytes in and out per execution;
- exact or one-U8-unit oracle tolerance;
- ramp monotonicity;
- learned output nonconstant and nonidentity;
- canaries;
- sequence ordering;
- one runtime trace;
- any failure blocks later vectors;
- no retry or CPU fallback in strict mode;
- timeout, process death, malformed result, wrong output, canary, health, and
  cleanup failures.

### Task 19 — Implement the fixed six-vector runner

Prefer extracting and reusing the proven `waved-npu-probe` NN SDK wrapper.
Create a dedicated binary whose input vectors are compiled into the artifact
or hash-bound in a fixed read-only input file.

Planned files:

```text
waved/crates/waved-npu-erf-mask-probe/Cargo.toml
waved/crates/waved-npu-erf-mask-probe/src/main.rs
waved/tools/npu-compiler/build-device-erf-mask-probe.sh
waved/tools/npu-compiler/verify-device-erf-mask-probe-policy.sh
waved/tools/npu-compiler/stage-device-erf-mask-probe.sh
```

The runner:

- verifies the exact NBG before loading;
- validates runtime tensor metadata;
- allocates canary-protected 481-byte buffers;
- executes the six vectors in fixed order;
- records per-run monotonic timing;
- emits output payload hashes and one bounded result document;
- destroys the graph exactly once on the success path;
- reports cleanup separately after a failure;
- accepts no arbitrary graph, tensor, library, or iteration argument.

### Task 20 — Execute and approve the sealed partition

Run the static policy, preflight, six executions, trace normalization,
postflight, and cleanup checks once.

On pass:

- add the exact NBG and partition signature to the sealed registry;
- set `execution_approved: true` only for this exact package;
- leave general `compiler_qualified: false`;
- write
  `docs/npu/lab-notebook/2026-07-29-poc13d-erf-mask-numerical.md`;
- run all host package and policy tests again.

## 8. Phase E — PoC 13E NPU-Derived Enhanced Waveform

### Task 21 — Define a bounded batch transport

Extend the dedicated runner with one fixed `--fixture` mode or create a
separate fixed binary if keeping call-count proof clearer.

The fixture transport has:

```text
magic
schema version
frame count, 1..1200
bytes per frame, exactly 481
payload SHA-256 in the host manifest
```

The runner performs:

- one graph create;
- exactly `frame_count` sequential graph executions;
- one graph destroy;
- ordered output write to a new fixed path.

It does not decode audio or compute features on the Wave. It never accepts
more than 1,200 frames.

Add tests for:

- malformed header;
- wrong frame count or width;
- trailing bytes;
- input/output path and symlink attacks;
- output non-overwrite;
- per-frame ordering;
- first-error abort;
- exact call count;
- no retry;
- output hash and cleanup.

### Task 22 — Run the fixture and reconstruct audio

Host steps:

1. load the exact frozen 12-second fixture and model package;
2. run causal CPU feature extraction and learned affine transform;
3. serialize up to 1,200 UINT8 vectors;
4. compute the full CPU-mask and CPU-waveform reference;
5. run the approved fixture mode once;
6. pull and validate all returned masks;
7. reconstruct audio from only NPU-returned masks;
8. compute numerical and audio metrics;
9. render deterministic signed-16-bit WAV evidence.

Required gates:

- every expected frame returned once in order;
- zero CPU fallback frames;
- every mask byte within one U8 unit;
- waveform max error at most `1/128`;
- waveform error RMS below `-50 dBFS`;
- noisy-speech SI-SDR improvement at least 3 dB;
- noise-only attenuation at least 6 dB;
- clean-speech RMS within 1 dB;
- no missing samples, clipping, NaN, or segment discontinuity.

Write:

```text
docs/npu/lab-notebook/2026-07-29-poc13e-erf-mask-waveform.md
```

Passing this task is the first evidence that learned noise suppression uses
numerical NPU output. Continue to Phase F before claiming the full milestone.

## 9. Phase F — PoC 13F Rust Hybrid Integration and Soak

### Task 23 — Extract the reusable NN SDK runtime

Create:

```text
waved/crates/waved-npu-runtime/
  Cargo.toml
  src/lib.rs
  src/abi.rs
  src/graph.rs
```

Move, do not casually duplicate, the reusable ABI definitions and graph
lifecycle logic from `waved-npu-probe/src/nnsdk.rs`. Keep stock CTC/add probe
constants and CLI policy in the probe crate.

Write host tests using an injected fake symbol table for:

- ABI structure sizes and offsets;
- missing symbol;
- graph create failure;
- input metadata mismatch;
- input write failure;
- execution failure;
- output metadata and length mismatch;
- output read failure;
- destroy on success;
- cleanup after every failure boundary;
- one graph owner and no double destroy;
- non-Send/non-Sync ownership unless proven safe.

Re-run all existing `waved-npu-probe` behavior tests or its describe/static
policy after the extraction.

### Task 24 — Implement shared Rust model math

Add:

```text
waved/crates/waved-npu/src/erf_mask/
  mod.rs
  config.rs
  signal.rs
  model.rs
  executor.rs
  package.rs
```

Add only required dependencies:

- `rustfft`;
- `serde`/`serde_json`;
- `sha2`;
- optional local `waved-npu-runtime`.

Define:

```text
ErfExecutor
CpuErfExecutor
WaveErfExecutor
HybridErfExecutor
ErfMaskModel
ErfMaskInfer<E>
ErfMaskStats
```

Port every Python golden into Rust test fixtures:

- window coefficients;
- identity reconstruction;
- noise-estimator trace;
- feature tensor;
- quantized logits;
- Erf bytes;
- masks;
- output waveform;
- reset state.

Require Rust and Python hashes to agree. Avoid a second model
interpretation.

### Task 25 — Implement strict and hybrid executors

Strict executor:

- verifies exact package and runtime identities;
- creates one sealed graph;
- validates UINT8 `[481]` metadata;
- submits one frame;
- returns exactly 481 bytes;
- exposes attempt/success/failure timing;
- never calls CPU fallback.

Hybrid executor:

- creates one strict executor attempt;
- holds one CPU executor;
- on first strict failure, recomputes the affected vector on CPU;
- opens a permanent session circuit breaker;
- makes no later NPU attempt;
- exposes transition sequence and stable error.

Write tests for create failure, first-frame failure, mid-session failure,
wrong output, deadline, circuit-breaker permanence, same-frame CPU
recomputation, reset, and stats.

### Task 26 — Generalize audio buffering minimally

Modify:

```text
waved/crates/waved-audio/src/aec/deep_filter.rs
waved/crates/waved-audio/src/lib.rs
```

Extract the period-to-hop adapter into a generic
`NeuralNoiseSuppressor<Box<dyn NpuInfer>>`. Keep `DeepFilterNs` as a
compatibility constructor or type wrapper.

Tests:

- existing DeepFilter tests remain unchanged in behavior;
- arbitrary ALSA period sizes;
- one-hop and multi-hop buffering;
- startup behavior;
- inference error policy;
- provider stats;
- Wave Erf backend selection only at 48 kHz;
- initialization failure returns to DeepFilterNet, then Speex;
- default configuration remains DeepFilterNet;
- no NPU path is selected without the opt-in model package and feature.

Do not change unrelated AEC or beamforming logic.

### Task 27 — Cross-build and static production policy

Cross-build the opt-in Wave backend once in the approved offline environment.
Verify:

- exact dependency and binary hashes;
- no unexpected DT_NEEDED entries;
- no compiler, CLC, arbitrary model loader, shell, ADB, ioctl replay, reset,
  or firmware-write strings;
- package and NBG paths are fixed below `/data/waved/npu/erf-mask/`;
- stock libraries are loaded read-only from exact paths;
- feature-off production behavior is unchanged.

Stage only under the dedicated `/data/waved/npu/erf-mask/` lab root. Do not
replace the running service or enable the backend by default.

### Task 28 — Run fault injection and exact 10,000-frame soak

Host-only fault injection first:

- pre-execution strict failure;
- post-construction worker failure;
- same-frame CPU output;
- one circuit-breaker transition;
- zero retry.

Device qualification:

- one graph create;
- exactly 10,000 sequential strict executions;
- one graph destroy;
- frozen frame sequence;
- pre/post health and normalized trace;
- p50/p95/p99/max inference and total-hop timing;
- CPU, RSS, allocation, and deadline statistics;
- reset comparison at a fixed sequence boundary;
- no concurrent NPU graph.

Pass:

- all 10,000 frames returned once and in order;
- max output numerical error within the approved tolerance;
- NPU inference p99 below 5 ms;
- total-hop p99 below 10 ms;
- zero deadline misses;
- no monotonic memory growth;
- no kernel alert, health change, holder leak, or cleanup error;
- CPU fallback below one-hop p99 and real-time factor `0.8`.

Write:

```text
docs/npu/lab-notebook/2026-07-29-poc13f-erf-mask-soak.md
```

## 10. Documentation Updates

After each passed or clean-negative PoC, update only relevant indexes:

```text
docs/npu/README.md
docs/npu/lab-notebook/README.md
docs/npu/onnx/README.md
docs/npu/reproduction.md
docs/REPORT_NPU.md
docs/REPORT_AI_MODELS.md
waved/docs/COMPUTE_HARDWARE.md
waved/scripts/npu/README.md
waved/tools/onnx-hybrid/README.md
```

Each claim links to a machine result and exact artifact hash. Keep these
labels distinct:

```text
host-trained
compile-qualified
package-qualified
numerically-npu-qualified
npu-waveform-qualified
hybrid-soak-qualified
```

Do not retroactively change PoC 12B's top-level
`multi-region-unresolved` result.

## 11. Verification Matrix

### Python

```text
cd waved/tools/onnx-hybrid
uv sync --frozen
uv run pytest
uv run ruff check .
```

### NBG and device harness

```text
python3 -m unittest discover -s waved/scripts/npu/tests -p 'test_*.py'
```

Run focused new test modules before the full suite.

### Rust

```text
cd waved
cargo fmt --check
cargo clippy -p waved-npu-runtime -p waved-npu -p waved-audio \
  --all-targets --all-features -- -D warnings
cargo test -p waved-npu-runtime -p waved-npu -p waved-audio
cargo test -p waved-npu-probe
```

If the new runtime crate is target-gated, host tests use the fake symbol
table and the cross-build verifies the real target module.

### Policy and artifacts

- shell syntax checks for every new script;
- native source/ELF static policy;
- exact manifest validation;
- NBG full parse and CRC;
- canonical JSON regeneration;
- deterministic model/package regeneration;
- documentation link checks;
- `git diff --check`;
- exact dirty-worktree review.

## 12. Completion Audit

Before claiming completion, build a requirement-to-evidence table for all 13
design completion criteria.

For each criterion record:

```text
requirement
authoritative artifact
artifact SHA-256
reproduction command
observed result
classification: proven / contradicted / incomplete / missing
```

Inspect current artifacts rather than relying on prior commentary. Verify:

- learned model and ablation;
- deterministic compile;
- strict NBG/package validation;
- six-vector numerical execution;
- hardware residency evidence;
- NPU-derived waveform and objective improvement;
- Rust `NpuInfer` integration;
- strict/hybrid/CPU/audit semantics;
- fault injection;
- exact 10,000-frame latency and health soak;
- documentation and hash agreement.

The goal remains active if any classification is not `proven`.
