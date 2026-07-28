# Wave NPU PoC 0 — Reproducible Lab Implementation Plan

> **Goal:** Build a safe, allowlisted host runner that records exact NPU
> experiment provenance, captures device state before and after native
> execution, refuses unknown artifacts, classifies failures, and proves
> semantic repeatability with two stock-CTC control runs.

**Program design:**  
`docs/superpowers/specs/2026-07-28-wave-npu-reverse-engineering-program-design.md`

**PoC:** 0 — Reproducible NPU Lab

**Tech stack:** Python 3 standard library, ADB, JSON, SHA-256, existing
Rust `waved-npu-probe`, Markdown.

## 1. Scope

PoC 0 establishes the experimental control plane. It does not reverse a
new ABI, parse additional NBG fields, compile a graph, benchmark the
accelerator, or change the audio application.

It must:

- run only source-controlled experiment definitions;
- reject an unknown experiment name;
- reject a mismatched executable, runtime library, or model hash;
- capture a bounded device snapshot before and after execution;
- preserve raw stdout/stderr outside Git;
- write a normalized JSON result envelope;
- classify success, native failure, timeout, ADB failure, and malformed
  output;
- compare two result envelopes semantically;
- execute the existing stock CTC control twice;
- publish an evidence-labeled lab-notebook entry.

## 2. Safety Boundary

Allowed device actions:

- read-only ADB commands;
- execute the already-staged
  `/data/waved/waved-npu-probe.poc`;
- write result artifacts on the host;
- create no new device files during acceptance.

Not allowed:

- firmware or system-library replacement;
- sysfs writes;
- service termination;
- reboot;
- kernel-module changes;
- arbitrary remote shell commands supplied by CLI users;
- cleanup outside the host `artifacts/npu/` tree.

The runner must attempt its post-state snapshot after native failure and
timeout. It must not attempt automatic device recovery.

## 3. Files

Create:

- `waved/scripts/npu/npu_lab.py`
- `waved/scripts/npu/experiments.json`
- `waved/scripts/npu/tests/test_npu_lab.py`
- `waved/scripts/npu/README.md`
- `docs/npu/README.md`
- `docs/npu/reproduction.md`
- `docs/npu/manifests/README.md`
- `docs/npu/manifests/wave-live-v1.1.6.6.json`
- `docs/npu/lab-notebook/README.md`
- `docs/npu/lab-notebook/000-template.md`
- `docs/npu/lab-notebook/2026-07-28-poc0-reproducible-lab.md`

Modify:

- `.gitignore`

Raw live-run files are written under `artifacts/npu/` and remain
untracked.

## 4. Experiment Definition Schema

`experiments.json` is the only source of executable experiment
definitions.

```json
{
  "schema_version": 1,
  "experiments": {
    "stock-ctc": {
      "description": "Firmware-matched CTC inference control",
      "remote_argv": [
        "/data/waved/waved-npu-probe.poc",
        "--run-stock-ctc"
      ],
      "environment": {
        "LD_LIBRARY_PATH": "/data/waved/lib:/usr/lib",
        "VIVANTE_SDK_DIR": "/usr"
      },
      "timeout_ms": 20000,
      "expected_remote_exit": 0,
      "required_artifacts": {
        "/data/waved/waved-npu-probe.poc": "fe8da5702032c397023acebd1a4bdf40ce086f6e86277a52c4f059f409552db0",
        "/usr/lib/libnnsdk.so": "36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe",
        "/usr/lib/libovxlib.so": "09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9",
        "/model/asr_de/de_online/ctc_online.nb": "5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66"
      },
      "expected_result": {
        "status": "PASS",
        "iterations": 3,
        "output_bytes": 332800,
        "output_sha256": "e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4",
        "cleanup": 0
      }
    }
  }
}
```

The implementation substitutes no values into `remote_argv` or
`environment`. CLI arguments select an experiment by exact key only.

## 5. Result Envelope Schema

Each run creates:

```text
artifacts/npu/RUN_ID/
  envelope.json
  stdout.txt
  stderr.txt
```

The JSON envelope contains:

```json
{
  "schema_version": 1,
  "experiment_id": "stock-ctc",
  "run_id": "UTC-name-commit",
  "timestamp_utc": "ISO-8601",
  "git_commit": "full SHA or explicit dirty marker",
  "host": {
    "platform": "string",
    "python": "string",
    "adb": "string"
  },
  "definition_sha256": "SHA-256",
  "required_artifacts": {},
  "preflight_hashes": {},
  "command": {
    "argv": [],
    "environment_allowlist": {},
    "timeout_ms": 20000
  },
  "device_before": {},
  "execution": {
    "classification": "success",
    "adb_exit": 0,
    "remote_exit": 0,
    "duration_ms": 0,
    "timed_out": false
  },
  "semantic_result": {},
  "device_after": {},
  "health": {},
  "artifacts": []
}
```

The envelope records only the allowlisted environment. It never captures
the host or device process environment.

## 6. Device Snapshot

The bounded snapshot records:

- `adb get-state`;
- `uname -a`;
- `/proc/uptime`;
- `/sys/module/galcore/parameters/nanoqFreq`;
- `/sys/module/galcore/parameters/enableNN`;
- `/sys/module/galcore/parameters/recovery`;
- `/dev/galcore` metadata;
- selected process lines containing `waved`, `audioserver`, or
  `pspapp`;
- best-effort `/dev/galcore` file-descriptor holders;
- SHA-256 for the experiment's required artifacts.

The snapshot does not capture:

- arbitrary process command lines;
- environment variables;
- audio buffers;
- the full kernel log;
- unrelated filesystem contents.

Volatile fields such as uptime and PID are retained in the raw envelope
but ignored by semantic comparison unless the experiment defines an
invariant for them.

## 7. Semantic Result Parser

The stock CTC parser extracts only the final machine-relevant line:

```text
RESULT: PASS — NPU executed 3 firmware-graph iterations;
output_bytes=332800,
output_sha256=SHA256_HEX,
average=LATENCY_MS ms,
cleanup=0.
```

It records:

- status;
- iteration count;
- output byte count;
- output SHA-256;
- cleanup status.

Average latency remains in raw stdout and may be recorded, but it is not
an equivalence key because timing naturally varies.

Malformed, missing, or duplicate result lines are failures.

## 8. Failure Classification

The runner uses these exact classifications:

- `success`
- `preflight-rejected`
- `native-failure`
- `native-timeout`
- `adb-unavailable`
- `adb-failure`
- `result-parse-failure`
- `post-snapshot-failure`
- `internal-error`

The envelope is written for every classification after an artifact
directory has been created.

An expected native failure may be added in a later experiment
definition, but PoC 0's `stock-ctc` definition expects remote exit zero.

## 9. Testability Architecture

`npu_lab.py` separates:

- immutable experiment/schema loading;
- an `AdbTransport` boundary;
- snapshot collection;
- hash preflight;
- remote execution;
- result parsing;
- envelope writing;
- semantic comparison;
- CLI rendering.

Unit tests use a fake transport. No unit test requires ADB or an NPU.

The production transport uses `subprocess.run` with:

- argv lists, never `shell=True`;
- explicit timeout;
- bounded captured output;
- UTF-8 replacement for invalid bytes;
- no inherited stdin.

The remote command uses only tokens loaded from the source-controlled
definition. A unique exit sentinel is appended by a fixed wrapper so the
remote exit status does not depend on the host `adb` exit behavior.

## 10. Task 1 — Add Failing Schema and Refusal Tests

**Files:**

- Create: `waved/scripts/npu/tests/test_npu_lab.py`
- Create: `waved/scripts/npu/experiments.json`

Tests:

- valid experiment definition loads;
- unknown schema version is rejected;
- unknown experiment name is rejected;
- environment keys outside the allowlist are rejected;
- absolute executable path outside `/data/waved/npu-lab/` and the
  grandfathered probe path is rejected;
- empty artifact list is rejected;
- invalid SHA-256 is rejected;
- nonpositive timeout is rejected;
- mutable CLI command arguments are not accepted.
- optional run IDs must match `[a-z0-9][a-z0-9-]{0,63}` and an existing
  artifact directory is never overwritten.

Run:

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -v
```

Expected before implementation: import or assertion failures.

## 11. Task 2 — Implement Definition Loading

**Files:**

- Create: `waved/scripts/npu/npu_lab.py`

Implement:

- typed dataclasses for experiment definitions;
- strict JSON key validation;
- SHA-256 validation;
- environment allowlist validation;
- fixed remote-path policy;
- canonical JSON hashing;
- `list` CLI command.

Run:

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -v
python3 waved/scripts/npu/npu_lab.py list
```

Expected: schema/refusal tests pass; `stock-ctc` is listed without
touching ADB.

## 12. Task 3 — Add Snapshot and Preflight Tests

**Files:**

- Modify: `waved/scripts/npu/tests/test_npu_lab.py`
- Modify: `waved/scripts/npu/npu_lab.py`

Tests:

- snapshot parses all bounded fields;
- unavailable optional sysfs field is represented explicitly;
- artifact hash response is parsed by exact path;
- a missing artifact is rejected;
- a mismatched artifact hash is rejected;
- no remote execution occurs after preflight rejection;
- holder scan output is bounded and normalized.

Implement `AdbTransport` and `FakeAdbTransport` boundaries after tests
fail.

## 13. Task 4 — Add Execution and Envelope Tests

**Files:**

- Modify: `waved/scripts/npu/tests/test_npu_lab.py`
- Modify: `waved/scripts/npu/npu_lab.py`

Tests:

- remote exit sentinel is parsed;
- sentinel is removed from stdout;
- stock PASS line is parsed;
- missing/duplicate/malformed result lines fail;
- native nonzero exit is classified;
- subprocess timeout is classified;
- post-snapshot is attempted after native failure;
- bounded stdout/stderr files are written;
- envelope contains no non-allowlisted environment;
- definition hash and artifact hashes are retained.

Implement:

- fixed remote wrapper;
- timeout handling;
- semantic parser;
- atomic JSON write;
- stdout/stderr artifact write;
- `run` CLI command.

## 14. Task 5 — Add Semantic Comparison

**Files:**

- Modify: `waved/scripts/npu/tests/test_npu_lab.py`
- Modify: `waved/scripts/npu/npu_lab.py`

Equivalence keys:

- experiment ID;
- definition hash;
- required/preflight hashes;
- device kernel;
- NPU read-only parameters;
- remote exit and classification;
- semantic status/iterations/output bytes/output digest/cleanup;
- post-run ADB health.

Ignored:

- timestamps;
- run IDs;
- pointer values;
- PIDs unless explicitly invariant;
- uptime;
- duration and average latency;
- artifact host paths.

Add:

```bash
python3 waved/scripts/npu/npu_lab.py compare \
  artifacts/npu/run-a/envelope.json \
  artifacts/npu/run-b/envelope.json
```

Exit zero means semantically equivalent; nonzero prints a bounded
field-by-field difference.

## 15. Task 6 — Add Documentation and Manifest

**Files:**

- Create the documentation files listed in section 3.
- Modify: `.gitignore`

The manifest records:

- live kernel and galcore identity;
- runtime/model/probe paths, sizes, and hashes;
- NBG version/product;
- known semantic output;
- evidence date and collection command;
- which fields are observed, static evidence, inferred, or unknown.

The documentation explains:

- installation requirements;
- safety policy;
- how to list and run an experiment;
- how to inspect an envelope;
- how to compare two runs;
- how to add a new allowlisted experiment;
- how raw artifacts are retained or removed safely;
- how stable findings graduate from notebooks.

## 16. Task 7 — Live Acceptance

Preconditions:

- `adb get-state` returns `device`;
- all required hashes match;
- the current probe context path passes;
- no mutation outside the approved safety boundary is needed.

Commands:

```bash
python3 waved/scripts/npu/npu_lab.py snapshot \
  --experiment stock-ctc \
  --output artifacts/npu/poc0-snapshot.json

python3 waved/scripts/npu/npu_lab.py run stock-ctc \
  --artifact-root artifacts/npu \
  --run-id poc0-first-control-20260728

python3 waved/scripts/npu/npu_lab.py run stock-ctc \
  --artifact-root artifacts/npu \
  --run-id poc0-second-control-20260728

python3 waved/scripts/npu/npu_lab.py compare \
  artifacts/npu/poc0-first-control-20260728/envelope.json \
  artifacts/npu/poc0-second-control-20260728/envelope.json
```

Record:

- both run IDs;
- exact definition and binary hashes;
- semantic output digest;
- p50-style descriptive timing only, not a benchmark claim;
- cleanup status;
- device health and unchanged parameters;
- any holder/PID differences and why they are non-semantic.

Publish the result in
`docs/npu/lab-notebook/2026-07-28-poc0-reproducible-lab.md`.

## 17. Task 8 — Final Verification

Run:

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -v
python3 -m py_compile waved/scripts/npu/npu_lab.py
python3 waved/scripts/npu/npu_lab.py list
git diff --check -- \
  .gitignore \
  waved/scripts/npu \
  docs/npu
```

Inspect the scoped diff and exclude unrelated work.

## 18. PoC 0 Acceptance Criteria

PoC 0 passes only when:

- source-controlled definitions are the sole command source;
- unknown commands and bad hashes are refused before execution;
- unit tests cover all required classifications;
- snapshots are bounded and contain no environment or audio data;
- every live run writes raw output and a JSON envelope;
- both stock CTC runs pass with the known output digest;
- semantic comparison returns equivalent;
- graph cleanup is zero;
- ADB remains responsive;
- NPU parameters remain unchanged;
- no service, firmware, model, system library, sysfs value, or boot
  configuration is changed;
- documentation and the lab notebook reproduce the result.

## 19. Commit Sequence

1. `docs: plan Wave NPU PoC 0 reproducible lab`
2. `test(npu-lab): define safe experiment runner contracts`
3. `feat(npu-lab): add allowlisted ADB experiment harness`
4. `docs(npu): publish PoC 0 reproducibility evidence`

Small implementation and test commits may be combined when splitting
would leave the branch in a non-working state, but unrelated audio files
must never be staged.
