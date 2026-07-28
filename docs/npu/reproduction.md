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
