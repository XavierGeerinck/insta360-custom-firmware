# Wave NPU Research Tools

This directory contains two deliberately separate security domains:

- `npu_lab.py` is the allowlisted device-experiment boundary;
- `abi_inventory.py`, `ghidra_abi.py`, and `nbg_format.py` are host-only,
  deterministic static evidence tools.

The static tools never invoke ADB or execute a firmware binary. The lab
harness is intentionally not a general ADB wrapper: it accepts no arbitrary
shell command, remote argument, environment override, remote path, or timeout
override. Its only executable definitions come from the adjacent tracked
[`experiments.json`](experiments.json).

## Requirements

- Python 3.10 or newer;
- `adb` on the host `PATH`;
- one authorized Wave visible as `device`;
- the exact probe and firmware artifacts named by the selected definition;
- no need for third-party Python packages.

Unit tests use a fake transport and do not require ADB:

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -v
python3 -m py_compile waved/scripts/npu/npu_lab.py
```

## Device-lab commands

List definitions without touching ADB:

```bash
python3 waved/scripts/npu/npu_lab.py list
```

Capture the bounded preflight state without executing a graph:

```bash
python3 waved/scripts/npu/npu_lab.py snapshot \
  --experiment stock-ctc \
  --output artifacts/npu/poc0-snapshot.json
```

Run one exact definition:

```bash
python3 waved/scripts/npu/npu_lab.py run stock-ctc \
  --artifact-root artifacts/npu \
  --run-id poc0-first-control-20260728
```

PoC 3 adds a separately built and staged trace definition:

```bash
waved/tools/npu-trace/build.sh
waved/tools/npu-trace/stage.sh

python3 waved/scripts/npu/npu_lab.py run stock-ctc-trace \
  --artifact-root artifacts/npu/runs \
  --run-id poc3-local-trace-1
```

Compare semantic invariants from two runs:

```bash
python3 waved/scripts/npu/npu_lab.py compare \
  artifacts/npu/poc0-first-control-20260728/envelope.json \
  artifacts/npu/poc0-second-control-20260728/envelope.json
```

## What a run does

1. Loads and strictly validates schema version 1.
2. Resolves an experiment by exact key.
3. Creates a new run directory and refuses to reuse one.
4. Captures bounded pre-run state.
5. Compares every required artifact SHA-256 with the tracked definition.
6. Refuses before native execution if any required fact is missing or
   different.
7. Builds a fixed, quoted remote wrapper from validated tokens.
8. Executes through `adb` with a host timeout.
9. Recovers the remote exit status from a unique internal sentinel.
10. Parses exactly one stock result line.
11. Checks the semantic result against the definition.
12. Captures post-run state even after native failure or timeout.
13. Writes bounded stdout, stderr, and an atomic JSON envelope.

No harness step writes the device. The existing PoC 0 probe and model are
preconditions and are not installed by the harness. A separate, explicitly
invoked staging script owns PoC 3's disposable tmpfs files.

## Bounded snapshot

The snapshot contains only:

- `adb get-state`;
- `uname -a`;
- `/proc/uptime`;
- the read-only galcore parameters `nanoqFreq`, `enableNN`, and `recovery`;
- `/dev/galcore` metadata;
- at most 32 selected process lines for `waved`, `audioserver`, or `pspapp`;
- at most 32 `/dev/galcore` file-descriptor holders;
- hashes for required artifacts.

It does not collect environment variables, arbitrary process command lines,
audio data, full kernel logs, unrelated files, or secrets. ANSI color
sequences are removed from normalized snapshot values.

## Output envelope

Each run produces:

```text
artifacts/npu/RUN_ID/
  envelope.json
  stdout.txt
  stderr.txt
```

The directory is created exclusively. A run ID must match
`[a-z0-9][a-z0-9-]{0,63}`. An existing directory is never overwritten.

The envelope preserves:

- timestamp and git identity, including a dirty marker;
- host platform, Python version, and first ADB version line;
- canonical experiment-definition digest;
- expected and observed artifact hashes;
- the exact fixed remote argv and allowlisted environment;
- pre- and post-device snapshots;
- host ADB exit, remote exit, timeout, and classification;
- parsed semantic result;
- post-run health invariants;
- hashes and byte sizes of the bounded raw logs.

Only `LD_LIBRARY_PATH` and `VIVANTE_SDK_DIR` may appear in an experiment
environment. The host and device environments are never captured.

## Failure classifications

| Classification | Meaning |
|---|---|
| `success` | Native exit, parsed result, expected result, and post-state all passed. |
| `preflight-rejected` | Device state or an artifact hash differed before execution. |
| `native-failure` | The probe exited unexpectedly or returned a valid but unexpected result. |
| `native-timeout` | The host ADB boundary exceeded the tracked timeout. |
| `adb-unavailable` | No device was available for the pre-run snapshot. |
| `adb-failure` | ADB transport failed or the remote exit sentinel was not recovered. |
| `result-parse-failure` | Native exit was expected but the machine result was missing, duplicate, or malformed. |
| `post-snapshot-failure` | Native work passed but required post-run evidence was incomplete. |
| `internal-error` | An unexpected host-harness error occurred after the run directory was created. |

Every classification after directory creation gets an envelope. A failing
classification is evidence, not permission to retry with fewer checks.

## Timeout limitation

The tested Wave rootfs has no `timeout` command. The known PoC probe has its
own 15-second worker watchdog. The tracked host timeout is 20 seconds for the
stock control and 30 seconds for the bounded tracing wrapper.

If the host timeout fires, the harness:

- terminates only its local `adb` client;
- records `native-timeout`;
- attempts the bounded post-state and holder scan;
- does not send `kill`, reset the driver, write sysfs, stop a service, or
  reboot.

Terminating a host ADB client does not prove a remote process has stopped.
Therefore a timeout is a stop condition for manual evidence review. New
native probes must provide their own bounded-execution behavior before being
allowlisted.

## Definition schema and policy

Schema version 1 requires these exact keys:

```json
{
  "description": "human purpose",
  "remote_argv": ["/data/waved/npu-lab/example", "--fixed-mode"],
  "environment": {
    "LD_LIBRARY_PATH": "/data/waved/lib:/usr/lib",
    "VIVANTE_SDK_DIR": "/usr"
  },
  "timeout_ms": 20000,
  "expected_remote_exit": 0,
  "required_artifacts": {
    "/data/waved/npu-lab/example": "lowercase-sha256"
  },
  "expected_result": {
    "status": "PASS",
    "iterations": 3,
    "output_bytes": 1234,
    "output_sha256": "lowercase-sha256",
    "cleanup": 0
  }
}
```

Executable policy:

- the original `/data/waved/waved-npu-probe.poc` is grandfathered for the
  stock control;
- new executables must be under `/data/waved/npu-lab/`;
- because `/data` was full during PoC 3, exactly
  `/tmp/wave-npu-lab/stock-ctc-trace` is also accepted; sibling `/tmp`
  executables remain rejected;
- `..`, control characters, empty arrays, unknown keys, duplicate JSON keys,
  uppercase/invalid digests, nonpositive timeouts, and exit codes outside
  `0..255` are rejected.

Artifact paths can name firmware files outside the disposable lab directory
because they are read-only preconditions. An executable cannot.

## Adding an experiment

Adding an experiment is a code-reviewed change, not an operator CLI action:

1. Write a bounded native probe with an internal watchdog and deterministic
   cleanup.
2. Test its parser and failure behavior on the host.
3. Stage it under `/data/waved/npu-lab/` only after that device mutation has
   been explicitly reviewed. An ephemeral path needs an equally narrow
   executable-policy change; do not allowlist `/tmp` as a directory.
4. Record its exact live SHA-256 and all runtime/model dependencies.
5. Add a complete definition to `experiments.json`.
6. Add fake-transport tests for success, refusal, timeout, parse failure, and
   cleanup.
7. Run `list`, then `snapshot`.
8. Review preflight evidence before the first native execution.
9. Repeat and compare.
10. Publish a notebook with both positive and negative findings.

Never make the CLI accept “temporary” arguments. A new behavior belongs in
the source-controlled probe and definition so that its hash and intent remain
reviewable.

## Semantic comparison

Comparison includes:

- experiment ID and definition digest;
- expected and preflight artifact hashes;
- kernel identity;
- pre- and post-NPU parameter values;
- native classification and remote exit;
- status, iteration count, output bytes, output digest, and cleanup;
- post-run ADB and state-health invariants.

Comparison ignores:

- timestamp and run ID;
- uptime and PIDs;
- graph pointer values in raw stdout;
- host/remote duration and average native latency;
- raw artifact host paths.

## Runtime ABI inventory

[`abi_corpus.json`](abi_corpus.json) pins four extracted firmware roots and
the logical NPU/runtime libraries to inspect. `abi_inventory.py` validates the
configuration strictly, hashes every selected ELF, extracts imports, exports,
relocations, diagnostics, exact function bytes, and bounded caller
discoveries, then emits:

```text
docs/npu/abi/runtime-abi-inventory.json
docs/npu/abi/firmware-matrix.md
```

Generate or verify:

```bash
python3 waved/scripts/npu/abi_inventory.py inventory \
  --output docs/npu/abi/runtime-abi-inventory.json

python3 waved/scripts/npu/abi_inventory.py matrix \
  --inventory docs/npu/abi/runtime-abi-inventory.json \
  --output docs/npu/abi/firmware-matrix.md

python3 waved/scripts/npu/abi_inventory.py verify-generated
```

Requirements are Python 3.10 or newer, `rz-bin` 0.8.2, and the exact extracted
firmware roots. Verification fails on any byte drift.

## Focused Ghidra evidence

[`ghidra_targets.json`](ghidra_targets.json) is a bounded list of exact ELF
targets and symbols. `ghidra_abi.py` validates each input hash, creates a
temporary Ghidra project, runs
[`scripts/ghidra/ExportNpuAbi.java`](../../../scripts/ghidra/ExportNpuAbi.java),
and writes deterministic call, caller, diagnostic, and decompilation evidence
to `docs/npu/abi/ghidra-evidence.json`.

Generate or verify:

```bash
python3 waved/scripts/npu/ghidra_abi.py generate \
  --output docs/npu/abi/ghidra-evidence.json

python3 waved/scripts/npu/ghidra_abi.py verify-generated
```

Requirements are Ghidra 12.0.4 and Java 21. Set
`GHIDRA_ANALYZE_HEADLESS` only when the executable is not at a detected
Homebrew path. Inputs are read-only and temporary projects are removed.

Static evidence is documented in
[`docs/npu/abi/README.md`](../../../docs/npu/abi/README.md). Decompiled types
are hypotheses until exact callers, provider bytes, public headers, and
executable layout tests agree.

This makes repeated experiments comparable without treating natural timing
or allocator variation as a failure.

## Strict NBG format explorer

[`nbg_corpus.json`](nbg_corpus.json) pins the available 137-file Wave model
corpus, the foreign-target legacy add negative control, and the exact
live-tested CTC reference. `nbg_format.py` is a host-only parser: it does not
invoke ADB, load a firmware library, or execute a graph.

Commands:

```bash
python3 waved/scripts/npu/nbg_format.py parse \
  waved/third_party/stock-npu-stack/sample-ctc.nb

python3 waved/scripts/npu/nbg_format.py inventory
python3 waved/scripts/npu/nbg_format.py summary
python3 waved/scripts/npu/nbg_format.py verify-generated
```

Canonical outputs:

```text
docs/npu/format/stock-nbg-inventory.json
docs/npu/format/corpus-summary.md
```

The parser supports:

- the evidence-backed compatible prefix of legacy fixed-layout v1 NBG;
- exact Wave v2 version `0x00020000`;
- public v2 headers, sub-headers, and section headers;
- input/output tensor metadata and public format/type/quantization enums;
- network-info and memory metadata;
- public section IDs 0 through 40;
- streaming SHA-256, CRC-32, entropy, edge, and chunk fingerprints;
- validation of the five stored section checksums in every Wave v2 graph;
- a separately fingerprinted final 512-byte runtime connection region.

It deliberately does not emit command, coefficient, state, patch, or
connection payloads. Unknown or unsupported structure fails with a stable
`NbgParseError`; it is not accepted as a best-effort partial result.

Current corpus gate:

```text
139 configured files
138 Wave v2 files at target 0x1000000e
1 foreign v1 negative control at target 0x10000020
690 / 690 stored section CRC-32 values match
```

Format evidence and non-claims are documented in
[`docs/npu/format/README.md`](../../../docs/npu/format/README.md) and
[`docs/npu/format/field-map.md`](../../../docs/npu/format/field-map.md).

## Metadata-only runtime trace

PoC 3 uses:

```text
waved/tools/npu-trace/gal_trace.c
waved/tools/npu-trace/build.sh
waved/tools/npu-trace/stage.sh
waved/tools/npu-trace/stock-ctc-trace.sh
waved/scripts/npu/trace_normalize.py
```

Build and stage:

```bash
waved/tools/npu-trace/build.sh
waved/tools/npu-trace/stage.sh
```

The build script uses the repository's pinned AArch64 cross image. The stage
script requires exact local hashes, uses only `/tmp/wave-npu-lab/`, skips an
identical target, refuses an unexpected existing target, and verifies remote
read-back hashes.

Run and normalize:

```bash
python3 waved/scripts/npu/npu_lab.py run stock-ctc-trace \
  --artifact-root artifacts/npu/runs \
  --run-id poc3-local-trace-1

python3 waved/scripts/npu/trace_normalize.py normalize \
  --run-directory artifacts/npu/runs/poc3-local-trace-1 \
  --output artifacts/npu/poc3-local-normalized-1.json
```

Compare two normalized reports:

```bash
python3 waved/scripts/npu/trace_normalize.py compare \
  --left artifacts/npu/poc3-local-normalized-1.json \
  --right artifacts/npu/poc3-local-normalized-2.json
```

The interposer forwards request 30000 unchanged and records only:

- command;
- hardware type;
- core index;
- interface byte size;
- syscall result and errno;
- returned HAL status.

It never logs the 32-byte envelope's pointers or reads the command-specific
union. The normalizer rejects pointer-shaped output, invalid records,
nonzero results/statuses, count mismatch, phase mismatch, sequence gaps, and
unknown framing.

Canonical results and limits are in
[`docs/npu/trace/README.md`](../../../docs/npu/trace/README.md).

## Test contracts

The test suite covers:

- strict schema and duplicate/unknown-key refusal;
- environment and executable policy;
- hash, timeout, and run-ID validation;
- no mutable CLI remote arguments;
- bounded snapshots and unavailable optional fields;
- exact preflight refusal with no native call;
- holder normalization;
- exit-sentinel and result parsing;
- all nine failure/success classifications;
- post-snapshot after native failure and timeout;
- bounded raw outputs;
- environment redaction;
- non-overwrite behavior;
- semantic comparison and volatile-field exclusion.

The format tests additionally cover strict corpus discovery, v1/v2 bounds,
all decoded metadata records, sub-byte tensor sizing, checksum mismatch,
header and section corruption, prefix truncation, seeded random inputs,
source-relative deterministic output, and exact live-CTC metadata.

The trace tests additionally cover framing, volatile-value redaction,
pointer-token rejection, metadata bounds, command/status pairing, phase
correlation, the narrow final-thread exception, strace/GAL count agreement,
repeatability, and exact ephemeral executable policy.

Do not weaken a contract merely to make a new probe run. Update the design and
evidence model first.
