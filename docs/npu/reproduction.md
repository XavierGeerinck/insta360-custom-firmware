# Reproducing Wave NPU Controls

The first procedure reproduces PoC 0, proving that code we control can
execute a firmware-targeted NBG on the Insta360 Wave NPU. The PoC 3 section
then reproduces the bounded metadata-only runtime/driver trace.

It does **not** install a probe, alter firmware, stop services, write sysfs,
replace a model, or compile a custom model. The exact probe and stock graph
must already be present with the hashes in `experiments.json`.

## Expected result

A passing reproduction creates two local, gitignored run directories. Both
envelopes should report:

- classification `success`;
- remote exit `0`;
- status `PASS`;
- 3 iterations;
- 332,800 output bytes;
- output SHA-256
  `e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4`;
- cleanup `0`;
- responsive post-run ADB;
- unchanged NPU parameters and required hashes.

The comparator should print `equivalent`.

## Prerequisites

- Run from the repository root.
- Use Python 3.10 or newer.
- Install Android Platform Tools so `adb version` works.
- Connect exactly one authorized Wave by USB.
- Confirm the device is in a normal, stable boot state.
- Ensure no unrelated firmware update or model deployment is in progress.
- Keep the tracked `experiments.json` unmodified, or understand that a dirty
  definition produces a different definition digest.

No Python package installation is required.

## 1. Verify the host-only contracts

These commands do not contact the device:

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -v
python3 -m py_compile waved/scripts/npu/npu_lab.py
python3 waved/scripts/npu/npu_lab.py list
```

Expected list entry:

```text
stock-ctc	20000 ms	Firmware-matched CTC inference control
```

The test suite uses a fake ADB transport. It proves the refusal and envelope
logic independently of the current device.

## 2. Confirm ADB identity

```bash
adb devices -l
adb get-state
```

Expected state:

```text
device
```

If no device is shown, reconnect USB and restore the device's authorized ADB
mode. Do not work around this by changing firmware or boot configuration as
part of PoC 0.

If more than one device is attached, disconnect unrelated targets. The
harness intentionally has no arbitrary serial-selection feature in schema
version 1.

## 3. Take a read-only snapshot

Choose a new output path:

```bash
python3 waved/scripts/npu/npu_lab.py snapshot \
  --experiment stock-ctc \
  --output artifacts/npu/poc0-snapshot.json
```

The command refuses to overwrite an existing snapshot. Inspect the bounded
fields:

```bash
python3 - <<'PY'
import json
from pathlib import Path

payload = json.loads(
    Path("artifacts/npu/poc0-snapshot.json").read_text(encoding="utf-8")
)
snapshot = payload["snapshot"]
print("definition:", payload["definition_sha256"])
print("adb:", snapshot["adb_state"])
print("kernel:", snapshot["kernel"])
print("NPU parameters:", snapshot["npu_parameters"])
print("galcore:", snapshot["galcore_device"])
print("holders:", snapshot["galcore_holders"])
print("hashes:", snapshot["artifact_hashes"])
print("errors:", snapshot["errors"])
PY
```

Do not continue unless:

- ADB state is `device`;
- kernel identity is populated;
- `nanoqFreq` and `enableNN` are observed;
- `/dev/galcore` is observed;
- every artifact hash equals `experiments.json`;
- `errors` is empty or contains only a reviewed optional observation.

The live PoC 0 baseline observed:

```text
kernel: Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT
        Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux
nanoqFreq: 838860800
enableNN: 255
recovery: 1
```

Clock values can be firmware- and runtime-dependent. Hashes and definition
identity, rather than a hard-coded clock, are the execution gate.

## 4. Run the first control

The run ID must be new and must match
`[a-z0-9][a-z0-9-]{0,63}`:

```bash
python3 waved/scripts/npu/npu_lab.py run stock-ctc \
  --artifact-root artifacts/npu \
  --run-id poc0-first-control-20260728
```

Expected output begins:

```text
success:
```

The harness performs the hash preflight again. A standalone snapshot never
substitutes for the run's own preflight.

## 5. Inspect the first envelope

```bash
python3 - <<'PY'
import json
from pathlib import Path

path = Path(
    "artifacts/npu/poc0-first-control-20260728/envelope.json"
)
envelope = json.loads(path.read_text(encoding="utf-8"))
for key in (
    "definition_sha256",
    "preflight_hashes",
    "execution",
    "semantic_result",
    "health",
):
    print(f"{key}:")
    print(json.dumps(envelope[key], indent=2, sort_keys=True))
PY
```

Also inspect the bounded native logs:

```bash
sed -n '1,220p' \
  artifacts/npu/poc0-first-control-20260728/stdout.txt
sed -n '1,120p' \
  artifacts/npu/poc0-first-control-20260728/stderr.txt
```

Pointer addresses and timing may differ. The tensor dimensions, output length,
output digest, and cleanup result must not.

## 6. Run the second independent control

```bash
python3 waved/scripts/npu/npu_lab.py run stock-ctc \
  --artifact-root artifacts/npu \
  --run-id poc0-second-control-20260728
```

Use a different new run ID when the documented names already exist. The
harness will not overwrite them.

## 7. Compare semantic invariants

```bash
python3 waved/scripts/npu/npu_lab.py compare \
  artifacts/npu/poc0-first-control-20260728/envelope.json \
  artifacts/npu/poc0-second-control-20260728/envelope.json
```

Expected:

```text
equivalent
```

Exit status `0` means the selected invariants match. Exit status `1` prints
bounded field-by-field differences. Do not edit envelopes to make them match;
retain the negative result and explain it in the notebook.

## 8. Interpret timing correctly

The PoC reports native per-iteration time as descriptive evidence only.

For the 2026-07-28 baseline:

```text
control 1: 8.198, 2.241, 2.290 ms; mean 4.243 ms
control 2: 3.539, 2.155, 2.160 ms; mean 2.618 ms
six-sample median: 2.266 ms
range: 2.155–8.198 ms
```

This is not a production benchmark because:

- the graph is a stock CTC component, not a noise suppressor;
- input is deterministically zero-filled;
- there are only six samples;
- no warm-up policy, thermal control, CPU isolation, or scheduler control was
  applied;
- host-side ADB duration includes process and transport overhead.

PoC 6 owns formal operation and hardware characterization. PoC 7 owns audio
deadline and quality measurement.

## Failure handling

### `preflight-rejected`

At least one required fact did not match. Inspect
`device_before.artifact_hashes`, `required_artifacts`, and
`execution.reason`. Do not bypass the hash. Determine whether firmware,
probe, model, or runtime changed and create a new reviewed manifest if
appropriate.

### `adb-unavailable` or `adb-failure`

Check cable, authorization, and `adb devices -l`. Preserve the envelope. Do
not reboot or change boot configuration under the PoC 0 authority.

### `native-failure`

Inspect remote exit, stdout, stderr, pre/post state, and hashes. A native
failure is not permission to patch NBG product bytes, replace a library, or
replay driver commands.

### `result-parse-failure`

The native contract changed or output was incomplete. Compare the raw result
with the tracked parser. Update code and definition only after determining why
the contract changed.

### `native-timeout`

Stop dynamic work. The device rootfs has no `timeout` utility. The harness
kills only the local ADB client, records holder state when possible, and does
not perform remote recovery. Confirm manually that:

- ADB is responsive;
- no unexpected probe process remains;
- `/dev/galcore` holders are understood;
- NPU parameters are unchanged.

Any remote kill, service stop, driver reset, reboot, or sysfs write requires a
separate explicit decision.

### `post-snapshot-failure`

The native result passed, but post-run safety evidence did not. Do not call
the run successful. Restore observational access and determine what evidence
is missing.

### `internal-error`

Inspect `execution.reason` and retain the raw directory. Host bugs should be
reproduced with the fake transport before another live attempt.

## Raw artifact retention

`artifacts/npu/` is ignored by Git because native logs can be large and may
contain addresses or device-specific observations. The tracked notebook
records stable digests and conclusions.

Keep raw runs while:

- a PoC is under review;
- a semantic difference is unexplained;
- a later parser relies on the original evidence;
- a regression may need comparison.

Before deleting a run, verify its exact path and that the notebook contains
the necessary hashes. Use no wildcard and no variable expansion. For example,
to remove only a known disposable rerun:

```bash
test -f artifacts/npu/poc0-disposable-rerun/envelope.json
rm -r -- artifacts/npu/poc0-disposable-rerun
```

Never remove `artifacts/npu/` broadly during active research, and never point
cleanup at a workspace root, home directory, or unresolved variable.

## Reproducing PoC 3

PoC 3 executes the same stock graph and expected result as PoC 0. It adds a
fixed syscall trace and a metadata-only `ioctl` interposer.

It does not capture addresses in tracked output, read command payloads,
mutate/replay a request, or execute a new graph.

### 1. Build the AArch64 interposer

Docker must be available:

```bash
waved/tools/npu-trace/build.sh
```

Expected output digest:

```text
dccaedd13812c161949905efecf7aa19fe74712b75de8d80211924e286e68601
```

The build uses the pinned cross image in `build.sh`, targets GLIBC 2.17, and
is reproducible byte-for-byte on the documented host setup.

### 2. Stage the disposable files

The live unit had no available `/data` blocks, so PoC 3 uses a dedicated
tmpfs directory:

```bash
waved/tools/npu-trace/stage.sh
```

Expected read-back:

```text
dccaedd13812c161949905efecf7aa19fe74712b75de8d80211924e286e68601  /tmp/wave-npu-lab/libwave-gal-trace.so
7799626fcad221088a109740022e93ca5b7652d7589c9e79347ec92a1b1ac866  /tmp/wave-npu-lab/stock-ctc-trace
```

The script:

- validates the host files;
- creates only `/tmp/wave-npu-lab/`;
- skips an already-identical file;
- refuses to overwrite an unexpected file;
- sets 0444/0555 permissions;
- verifies remote hashes.

Reboot clears the directory. The experiment harness allows only the exact
wrapper path, not arbitrary `/tmp` executables.

### 3. Preflight all 13 dependencies

Use a new output path:

```bash
python3 waved/scripts/npu/npu_lab.py snapshot \
  --experiment stock-ctc-trace \
  --output artifacts/npu/poc3-local-preflight.json
```

Do not continue unless:

- ADB state is `device`;
- `/dev/galcore` is present;
- all 13 hashes match;
- NPU parameters are observed;
- errors are empty;
- holder state is understood.

### 4. Capture two runs

Use unique run IDs:

```bash
python3 waved/scripts/npu/npu_lab.py run stock-ctc-trace \
  --artifact-root artifacts/npu/runs \
  --run-id poc3-local-trace-1

python3 waved/scripts/npu/npu_lab.py run stock-ctc-trace \
  --artifact-root artifacts/npu/runs \
  --run-id poc3-local-trace-2
```

Both must classify as `success`. Stop after the first timeout, state drift,
hash drift, output mismatch, nonzero cleanup, or post-snapshot failure.

### 5. Normalize without pointers

```bash
python3 waved/scripts/npu/trace_normalize.py normalize \
  --run-directory artifacts/npu/runs/poc3-local-trace-1 \
  --output artifacts/npu/poc3-local-normalized-1.json

python3 waved/scripts/npu/trace_normalize.py normalize \
  --run-directory artifacts/npu/runs/poc3-local-trace-2 \
  --output artifacts/npu/poc3-local-normalized-2.json
```

The normalizer accepts only successful `stock-ctc-trace` envelopes and exact
trace framing. It fails on malformed metadata, unknown trace forms, pointer
retention, nonzero ioctl/HAL status, sequence gaps, phase mismatches, or
strace/GAL count disagreement.

### 6. Compare

```bash
python3 waved/scripts/npu/trace_normalize.py compare \
  --left artifacts/npu/poc3-local-normalized-1.json \
  --right artifacts/npu/poc3-local-normalized-2.json
```

Expected JSON includes:

```json
{
  "equivalent": true
}
```

The 2026-07-28 accepted baseline contains:

```text
73 GAL calls
73 syscall results zero
73 returned HAL statuses zero
376 interface bytes
33b93229efcf7258253d2124129ac24f107de63b1742d6666db2e674cbb1c226
  normalized sequence SHA-256
```

See [`trace/README.md`](trace/README.md) for the complete interpretation and
[`trace/repeatability.md`](trace/repeatability.md) for the retained negative
PTY-framing pilot.

## Reviewing PoCs 5 through 8

These PoCs include one-time compiler and execution observations. Reproducing
their host contracts is routine; repeating their live graph actions is not.

### Host-only verification

```bash
python3 -m unittest discover \
  -s waved/scripts/npu/tests \
  -p 'test_*.py'

python3 waved/scripts/npu/npu_erf_execution_lab.py manifest
python3 waved/scripts/npu/npu_erf_nonzero_execution_lab.py manifest
python3 waved/scripts/npu/npu_audio_denoiser_lab.py manifest
python3 waved/scripts/npu/npu_conv_audio_denoiser_lab.py manifest

bash -n waved/tools/npu-compiler/*.sh
git diff --check
```

The accepted generated NBG can be parsed without a device:

```bash
python3 waved/scripts/npu/nbg_format.py parse \
  artifacts/npu/runs/poc5c-erf-compile-1-20260728/candidate-1.nbg
```

Verify:

```text
bytes:     5840
sha256:    6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee
version:   0x00020000
target:    0x1000000e
checksums: 3 / 3
```

Accepted live evidence directories:

```text
artifacts/npu/runs/poc5c-erf-compile-1-20260728/
artifacts/npu/runs/poc5d-hardware-caps-20260728/
artifacts/npu/runs/poc5f-a-clc-loader-20260728/
artifacts/npu/runs/poc6-erf-execution-1-20260728/
artifacts/npu/runs/poc7-erf-nonzero-1-20260728/
artifacts/npu/runs/poc8a-audio-denoiser-compile-1-20260728/
artifacts/npu/runs/poc8b-audio-denoiser-direct-1-20260728/
artifacts/npu/runs/poc8c-conv-audio-denoiser-compile-1-20260728/
artifacts/npu/runs/poc8d-conv-audio-denoiser-direct-1-20260728/
```

Inspect `run.json`, then the bounded stdout and pre/post snapshots. A clean
negative has a nonzero native exit and no health differences or kernel
alerts; it must not be relabeled as a pass.

### Live repetition boundary

Do not rerun an old `run` command merely to check that it still works.
The harness refuses the existing run directory, and choosing a new run ID
would be a new live execution. A legitimate repeat needs:

- the exact reviewed graph and unchanged binary/model/runtime hashes;
- a stated determinism or regression question;
- current device/firmware identity;
- a new exclusive evidence directory;
- the same one-attempt and no-recovery rule.

The PoC 5F `6.4.18:886827` CLC is an exception in the opposite direction:
its compiler entry points are permanently quarantined after the observed ABI
failure and must not be called again.

Detailed results:

- [`PoC 5 compiler notebook`](lab-notebook/2026-07-28-poc5-custom-compiler.md)
- [`PoC 6/7 execution notebook`](lab-notebook/2026-07-28-poc6-7-custom-nbg-execution.md)
- [`PoC 8 audio notebook`](lab-notebook/2026-07-28-poc8-audio-denoiser-boundary.md)

## Reviewing PoCs 9A, 9C, 9D, 9E, and 9F

These gates are host-only. The commands below must not contact ADB, the Wave,
`/dev/galcore`, or a compiler.

Verify the generated architecture and configuration-parser evidence:

```bash
python3 waved/scripts/npu/arch_profile.py \
  verify-generated --ghidra-only

python3 waved/scripts/npu/compiler_profile.py \
  verify-generated
```

The PoC 9C candidate must retain:

```text
decision:
  parser-shaped-candidate-not-compiler-qualified

authorization.compiler_execution:
  false
```

Do not pass the candidate config to an available compiler. PoC 9D later
found genuine `VIV_GPU_FILE` consumers, but only in version-below-gate
standalone shader compilers—not in a compatible graph-to-NBG backend.

Verify PoC 9D's acquired archive, exact Wave profile, parser, and refusal
ledger without loading the OCI image or executing a vendor component:

```bash
python3 -m unittest \
  waved/scripts/npu/tests/test_compiler_acquisition.py

python3 waved/scripts/npu/compiler_acquisition.py \
  validate-targets

python3 waved/scripts/npu/compiler_acquisition.py \
  verify-generated
```

`verify-generated` streams and rehashes approximately 13.5 GB of pinned
outer, nested, OCI, and selected artifacts, plus every OCI layer. Expected
qualification:

```text
status:                   blocked
compiler calls:           0
container registrations:  0
container runs:           0
device calls:             0
generated NBG files:      0
```

The gate must continue to report:

```text
exact Wave vendor profile:  pass
VIV_GPU_FILE parser:         pass-limited
SDK >= 6.4.22:               fail
no-kernel NBG consumer:      fail
compiler I/O ownership ABI:  fail
fixed Conv compile:          blocked
```

Do not use `docker load`, run the image, import the Acuity extensions,
execute `vcompiler`/`vcCompiler`, or invoke the adjacent quarantined CLC.

Set up and verify the isolated ONNX tool:

```bash
cd waved/tools/onnx-hybrid
uv sync --python 3.12 --dev
uv run wave-onnx-hybrid verify-fixtures fixtures
uv run pytest -q
uv run ruff check .
uv run ruff format --check .
```

Create fresh host evidence:

```bash
uv run wave-onnx-hybrid audit \
  fixtures/spectral_mask_conv.onnx \
  --output /tmp/wave-spectral-audit.json

uv run wave-onnx-hybrid run \
  fixtures/spectral_mask_conv.onnx \
  --inputs fixtures/spectral_mask_input.npz \
  --mode cpu \
  --outputs /tmp/wave-spectral-cpu.npz \
  --evidence /tmp/wave-spectral-cpu.json

uv run wave-onnx-hybrid run \
  fixtures/spectral_mask_conv.onnx \
  --inputs fixtures/spectral_mask_input.npz \
  --mode hybrid \
  --outputs /tmp/wave-spectral-hybrid.npz \
  --evidence /tmp/wave-spectral-hybrid.json

cmp /tmp/wave-spectral-cpu.npz /tmp/wave-spectral-hybrid.npz
```

Verify these invariants:

```text
graph signature:
  13c8633af1974817c34ff1437676f047d789ee7c2110c91d43ee434a03afeaed

CPU classification:
  cpu-explicit

hybrid classification:
  cpu-preflight-fallback

NPU attempts in both:
  0

CPU sessions in both:
  1

raw output tensor SHA-256:
  788cab709bee63c8eb281065d49b90daec93d24ef945c8b21bf61350bf37fc40
```

`strict` mode must exit nonzero with `PARTITION_NOT_NPU_RESIDENT` before it
creates an output:

```bash
uv run wave-onnx-hybrid run \
  fixtures/spectral_mask_conv.onnx \
  --inputs fixtures/spectral_mask_input.npz \
  --mode strict \
  --outputs /tmp/wave-spectral-strict.npz \
  --evidence /tmp/wave-spectral-strict.json
```

Audit the real Silero model without inference:

```bash
uv run wave-onnx-hybrid audit \
  ../../../models/ai-models/asr-models/model/vad/silero_vad_v5.onnx \
  --output /tmp/wave-silero-audit.json
```

Verify:

```text
source SHA-256:
  2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f
graphs:
  51
total nodes:
  689
NPU partitions:
  0
```

Output paths are exclusive. Choose new explicit temporary names or remove
only your own prior `/tmp/wave-*` outputs before repeating a command.

Detailed results:

- [`PoC 9A architecture notebook`](lab-notebook/2026-07-28-poc9a-architecture-database-recovery.md)
- [`PoC 9C parser notebook`](lab-notebook/2026-07-28-poc9c-viv-gpu-file-parser-recovery.md)
- [`PoC 9D compiler-acquisition notebook`](lab-notebook/2026-07-28-poc9d-compiler-acquisition-static-qualification.md)
- [`PoC 9E/9F ONNX notebook`](lab-notebook/2026-07-28-poc9e-9f-onnx-cpu-fallback.md)
- [`ONNX evidence guide`](onnx/README.md)

## Reviewing PoC 12A

PoC 12A is host-only. These commands must not contact ADB, load a firmware
ELF, invoke OpenVX, run a compiler or simulator, or execute an NBG.

Validate the exact eight-artifact and 21-function policy:

```bash
python3 waved/scripts/npu/nbg_serializer_recovery.py validate
```

Expected summary:

```text
serializer manifest valid: artifacts=8 functions=21
```

Verify the tracked static evidence against a fresh bounded Ghidra analysis:

```bash
python3 waved/scripts/npu/nbg_serializer_recovery.py verify-ghidra
```

This requires Ghidra 12.0.4 and Java 21. It creates only a temporary Ghidra
project, analyzes the two exact AArch64 ELFs, compares canonical evidence,
and removes the project. A different ELF hash, address, symbol size,
function-body hash, call record, diagnostic, truncation state, or output
byte blocks verification.

Verify total byte ownership and unchanged re-emission:

```bash
python3 waved/scripts/npu/nbg_image.py verify-generated
```

The five controls must each report one exclusive owner for every source
byte, no first difference, and equal input/output SHA-256. The final
512-byte runtime connection fingerprint is a non-owning
`section-covered-view` in all five controls because a declared section
already reaches EOF. No changed NBG is emitted.

Verify the ONNX/NBG coefficient ledger inside its pinned environment:

```bash
uv run --project waved/tools/onnx-hybrid \
  python waved/scripts/npu/nbg_correlate.py verify-generated
```

Expected decision:

```text
poc12b: not-ready
reason: no deterministic invertible coefficient transform is corroborated
        across two tensors or graphs
```

Both the `[512,4999]` weight and `[4999]` bias must retain zero exact
occurrences across the recorded finite transform registry and admitted NBG
section ranges. Do not change the registry until a separately reviewed
hypothesis narrows the next search.

Run the focused contracts:

```bash
python3 -m unittest \
  waved.scripts.npu.tests.test_nbg_serializer_recovery \
  waved.scripts.npu.tests.test_nbg_image \
  waved.scripts.npu.tests.test_nbg_format

uv run --project waved/tools/onnx-hybrid \
  python -m unittest waved.scripts.npu.tests.test_nbg_correlate
```

The complete exact function table, range totals, transform definitions,
multilingual controls, safety counters, and non-claims are in the
[`PoC 12A notebook`](lab-notebook/2026-07-28-poc12a-open-nbg-serializer.md).

## Reviewing sealed PoC 12B

The live capture is complete; reviewing it is host-only. Do not reconnect to
ADB or rerun the compiler merely to verify the tracked interpretation. The
ignored capture directory must contain the exact regular files
`a1.nbg`, `a2.nbg`, and `b.nbg`.

Verify their frozen identities and the canonical evidence:

```bash
python3 waved/scripts/npu/nbg_constant_diff.py verify-generated
```

The verifier admits only the captured files, checks their lengths and
SHA-256 values, reruns the original bounded finite-registry analysis in
memory, and requires byte-identical
[`erf-constant-differential.json`](format/erf-constant-differential.json).
Expected top-level results:

```text
A1/A2 exact match:       true
A1/A2 bytes:             5904
A SHA-256:               4a84faf4...78c8a8b
B SHA-256:               c9e3e184...99f227
classification:          multi-region-unresolved
PoC 12C readiness:       not-ready
```

The last two values are intentionally historical. The PoC 12B search
registry excluded section 20 and did not know the header checksum. Later
PoC 12C evidence is kept separate instead of retroactively broadening the
reviewed live interpretation.

Run the frozen analyzer contracts:

```bash
python3 -m unittest \
  waved.scripts.npu.tests.test_nbg_constant_diff -v
```

See the
[`PoC 12B notebook`](lab-notebook/2026-07-29-poc12b-erf-constant-differential.md)
for the three remote exit codes, durations, artifact and source hashes,
prohibited-action counters, post-state comparison, and exact cleanup
record.

## Reviewing PoC 12C

PoC 12C is entirely host-only. It must not write an NBG, import a vendor
runtime, contact ADB, open `/dev/galcore`, or load a graph.

Verify the regenerated NBG inventory and the derived qualification:

```bash
python3 waved/scripts/npu/nbg_format.py verify-generated
python3 waved/scripts/npu/nbg_dyna_profile.py verify-generated
```

The first command reparses the configured 3.79 GB corpus and can take
several minutes. The second verifies the hash-pinned inventory, controlled
A1/A2/B files, Wave `libOpenVX.so`, and two bounded Ghidra exports before
rebuilding
[`dyna-io-crc-profile.json`](format/dyna-io-crc-profile.json) in memory.
Expected gates:

```text
v2 file CRC matches:       138 / 138
covered section bytes:     3,785,534,000
paired DYNAIO files:       111
DYNAIO entries:            8,939
nonzero padding bytes:     0
controlled classification: direct-dyna-io-entry-mapping
in-memory reconstruction:  exact
written to disk:           false
```

Run the focused parser and qualifier contracts:

```bash
python3 -m unittest \
  waved.scripts.npu.tests.test_nbg_format \
  waved.scripts.npu.tests.test_nbg_dyna_profile -v
```

A successful result qualifies only read/validation of the section-20/26
table, the v2 file checksum, and the one exact controlled reconstruction.
It does not approve arbitrary NBG emission or execution. See the
[`PoC 12C notebook`](lab-notebook/2026-07-29-poc12c-dyna-io-crc-profile.md).

## Reviewing PoCs 13A–13G

The PoC 13 result is a working learned NPU-assisted denoiser, not a general
ONNX compiler. Start with the consolidated
[`result guide`](models/erf-mask/README.md) and
[`completion audit`](models/erf-mask/completion-audit.md).

Run the host qualification and fallback tests:

```bash
cd waved/tools/onnx-hybrid
uv sync --frozen
uv run pytest -q
uv run ruff check .
cd ../../..
uv --project waved/tools/onnx-hybrid run pytest -q waved/scripts/npu/tests
```

Verify the fixed native artifacts against their source policies:

```bash
waved/tools/npu-compiler/verify-device-erf-mask-policy.sh \
  artifacts/npu/build/poc13b/wave-npu-compile-erf-mask-v1 \
  waved/tools/npu-compiler/tim_vx_compile_erf_mask.cc

waved/tools/npu-compiler/verify-device-erf-mask-qualification-policy.sh \
  artifacts/npu/build/poc13d/wave-npu-run-erf-mask-qualification-v1 \
  waved/tools/npu-compiler/wave_npu_run_erf_mask_qualification.cc

waved/tools/npu-compiler/verify-device-erf-mask-stream-policy.sh \
  artifacts/npu/build/poc13e/wave-npu-run-erf-mask-stream-v1 \
  waved/tools/npu-compiler/wave_npu_run_erf_mask_stream.cc

waved/tools/npu-compiler/verify-device-erf-mask-soak-policy.sh \
  artifacts/npu/build/poc13f/wave-npu-soak-erf-mask-v1 \
  waved/tools/npu-compiler/wave_npu_soak_erf_mask.cc

waved/tools/npu-compiler/verify-device-erf-cpu-npu-benchmark-policy.sh \
  artifacts/npu/build/poc13g/wave-npu-benchmark-erf-cpu-vs-npu-v2 \
  waved/tools/npu-compiler/wave_npu_benchmark_erf_cpu_vs_npu.cc
```

The authoritative live results are:

```text
NBG SHA-256:             96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94
six-vector run SHA-256:  f8ffbca89fb96275e1c404ea780c1ccb62de3deda07820ec7308ccf6aaab7fac
stream run SHA-256:      6079c34599386f2024ac7787deacf01413231b5bf312f75b030292f2cb87bda2
waveform SHA-256:        6589c517483fbf120e70176c46267c867f96dc2d0286d57521920dab2d93d09b
10k soak SHA-256:        e4f3fc3d0de3c833dadf912563585c3737d7dc30c7338f2e26d9936077ee6f8d
13G timing SHA-256:      7998b0df5000c0140b5c3229b244dbed8d158bbfc42a025fb741ff478dc0eb32
13G result:              CPU 20.686x faster than NPU Run by ratio of means
```

PoC 13C's profile must report `dynamic_io: null`; section 20/26 records are
conditional and are not expected in this external-input Erf artifact.

The device scripts use fixed `/tmp/wave-npu-poc13*` roots, refuse an existing
root, and have no retry or recovery path. Review the pulled evidence in
place. Do not delete device evidence just to rerun a stage, and never invoke
the quarantined adjacent `libCLC.so`.

The PoC 13G R1 negative is retained and must not be replayed. R2 used a
different binary and `/tmp/wave-npu-poc13g-r2`; it is also non-replayable.
Review its raw evidence through the tracked
[`notebook`](lab-notebook/2026-07-29-poc13g-wave-cpu-vs-npu.md) and
[`JSON result`](performance/erf-wave-cpu-vs-npu.json). Do not compare the
earlier Apple-host CPU measurements with Wave NPU timing.

## Reviewing PoC 14A/14B

PoC 14A is host-only and reproducible. Verify its strict package and focused
contracts:

```bash
uv run --project waved/tools/onnx-hybrid wave-erf-affine verify-host \
  --package artifacts/npu/runs/poc14a-erf-affine-host-20260729

uv run --project waved/tools/onnx-hybrid pytest -q \
  waved/tools/onnx-hybrid/tests/test_erf_affine_model.py \
  waved/tools/onnx-hybrid/tests/test_erf_affine_package.py \
  waved/tools/onnx-hybrid/tests/test_erf_affine_qualification.py \
  waved/tools/onnx-hybrid/tests/test_erf_affine_cli.py
```

Expected package identity:

```text
model contract:
  b52ddaf3baed2a7001ff10cbf9ba0600945749a2332f02d71d2ae0be25ac98b8
model manifest:
  99cccb924ab807171ce93b5cba40b3d7ed52219294c0f5d3104b153aa0294263
native constants:
  a2b588be1a4e68a3179a18b51be016115782f923344521c667ca4310c9eeecb4
compiler_qualified: false
execution_approved: false
```

PoC 14B's one live A1 capture is a retained negative and must not be
replayed. Review it through the tracked profile:

```bash
python3 waved/scripts/npu/affine_compile_profile.py

python3 -m unittest \
  waved.scripts.npu.tests.test_affine_compile_profile \
  waved.scripts.npu.tests.test_npu_erf_affine_compile_lab

waved/tools/npu-compiler/verify-device-erf-affine-policy.sh \
  artifacts/npu/build/poc14b/wave-npu-compile-erf-affine-v1 \
  waved/tools/npu-compiler/tim_vx_compile_erf_affine.cc \
  artifacts/npu/build/poc14b/affine-constants.bin
```

Expected interpretation:

```text
status:                       clean-negative
first blocking stage:         TIM-VX operation type check
observed input pair:          ASYM UINT8, SYMM PC INT8
accepted ADD/Multiply pairs:  59
per-channel accepted pairs:   0
NBG emitted:                  false
health differences:           none
remaining compiler calls:     skipped, not replayed
semantic emission:            refused
```

The live `/tmp/wave-npu-poc14b` staging root was removed after exact-content
and holder checks. Do not recreate it merely to reproduce the negative.
See the
[`PoC 14A notebook`](lab-notebook/2026-07-29-poc14a-affine-host.md),
[`PoC 14B notebook`](lab-notebook/2026-07-29-poc14b-affine-compiler-negative.md),
and
[`compiler profile`](profile/affine-elementwise-compiler-profile.json).

## Creating a new allowlisted experiment

The runner has no dynamic command feature by design. A new experiment
requires:

1. an approved PoC design and safety gate;
2. a native probe with internal bounded execution and cleanup;
3. explicit review before staging under `/data/waved/npu-lab/`; an ephemeral
   fallback must be an exact executable allowlist entry, not a broad `/tmp`
   root;
4. exact hashes for the probe, runtimes, and models;
5. a strict `experiments.json` entry;
6. fake-transport tests for every relevant classification;
7. a read-only snapshot;
8. two repeated controls and semantic comparison;
9. a notebook documenting positive, negative, and unknown results.

See [`waved/scripts/npu/README.md`](../../waved/scripts/npu/README.md) for
schema details.
