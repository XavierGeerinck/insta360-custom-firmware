# Wave NPU Research Tools

This directory contains two deliberately separate security domains:

- `npu_lab.py` is the allowlisted device-experiment boundary;
- `abi_inventory.py`, `ghidra_abi.py`, `arch_profile.py`,
  `compiler_profile.py`, `compiler_acquisition.py`, `nbg_format.py`,
  `nbg_constant_diff.py`, `nbg_dyna_profile.py`, and
  `nbg_erf_mask_profile.py` are host-only evidence tools.
  `affine_compile_profile.py` correlates PoC 14B's one live rejection with
  the finite pinned elementwise type table.
  `compiler_profile.py` has one explicitly isolated x86 parser-only mode;
  `compiler_acquisition.py` is entirely static and never executes or imports
  an acquired component.

The static tools never invoke ADB or execute a firmware binary. The lab
harness is intentionally not a general ADB wrapper: it accepts no arbitrary
shell command, remote argument, environment override, remote path, or timeout
override. Its only executable definitions come from the adjacent tracked
[`experiments.json`](experiments.json).

The separately pinned host ONNX auditor and CPU fallback runner lives under
[`waved/tools/onnx-hybrid`](../../tools/onnx-hybrid/). It is a third,
host-only domain and never imports this device harness.

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

PoC 14B's host-only compiler profile can be regenerated with:

```bash
python3 waved/scripts/npu/affine_compile_profile.py
```

The historical A1 device capture is append-only and must not be replayed.

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

## Architecture profile recovery

[`arch_profile_targets.json`](arch_profile_targets.json) pins the exact Wave
and x86 simulator architecture-model/performance libraries.
`arch_profile.py` reuses the bounded Ghidra adapter, then reads the
statically demonstrated architecture tables as inert ELF bytes.

It recovers:

- the five-word chip/version/product/eco/customer lookup;
- the `0x0000fff0` revision-family fallback;
- the unique target-`0x1000000e` database rows;
- exact table geometry and complete row hashes;
- selected architecture ABI sizes and configuration offsets;
- a strict evidence profile with explicit unknowns.

Commands:

```bash
python3 waved/scripts/npu/arch_profile.py validate-targets
python3 waved/scripts/npu/arch_profile.py generate-ghidra
python3 waved/scripts/npu/arch_profile.py generate-profile
python3 waved/scripts/npu/arch_profile.py validate-profile
python3 waved/scripts/npu/arch_profile.py verify-generated
```

The expensive deterministic check reruns all four Ghidra targets:

```bash
python3 waved/scripts/npu/arch_profile.py \
  verify-generated --ghidra-only
```

Outputs:

```text
docs/npu/profile/arch-profile-ghidra-evidence.json
docs/npu/profile/wave-profile-evidence.json
docs/npu/profile/wave-profile.schema.json
```

The tool never loads an analyzed ELF, invokes a compiler, imports ADB, opens
`/dev/galcore`, or emits `VIV_GPU_FILE`. See
[`docs/npu/profile/README.md`](../../../docs/npu/profile/README.md).

## Configuration parser recovery

[`compiler_profile_corpus.json`](compiler_profile_corpus.json) pins exact
Wave, adjacent, x86 simulator, documentation, and PoC 9A profile inputs.
[`compiler_profile_targets.json`](compiler_profile_targets.json) limits
Ghidra to one reviewed x86 emulator and nine parser/catalog focus symbols.

`compiler_profile.py`:

- distinguishes source-documented `VIV_GPU_FILE` from the proven
  `VSIMULATOR_CONFIG` consumer;
- recovers five ordered uint32 assignments and their record offsets;
- extracts all 358 built-in 100-byte target records as inert ELF data;
- requires one exact customer-`0x1000000e` row;
- implements a bounded strict parser and canonical renderer;
- optionally runs 13 fixed parser cases in fresh isolated containers;
- emits a Wave candidate with an explicit compiler refusal.

Commands:

```bash
python3 waved/scripts/npu/compiler_profile.py validate-targets
python3 waved/scripts/npu/compiler_profile.py generate-ghidra
python3 waved/scripts/npu/compiler_profile.py probe-vendor-parser
python3 waved/scripts/npu/compiler_profile.py recover-grammar
python3 waved/scripts/npu/compiler_profile.py generate-candidate
python3 waved/scripts/npu/compiler_profile.py verify-generated
```

Outputs:

```text
docs/npu/profile/compiler-profile-ghidra-evidence.json
docs/npu/profile/vsim-parser-probe-results.json
docs/npu/profile/viv-gpu-file-grammar.json
docs/npu/profile/wave-viv-gpu-file.candidate.config
docs/npu/profile/wave-viv-gpu-file.candidate.manifest.json
```

The isolated mode loads only the pinned x86 `libEmulator.so` and copied,
rehash-checked `libvdtproxy.so`. It calls no constructor, compiler, OpenVX,
VSC, CLC, graph, device, or driver entrypoint. Each case uses a networkless,
read-only, no-new-privileges container with fixed CPU, memory, PID,
file-descriptor, and wall limits.

The candidate is not a compiler-qualified Acuity profile. The required
no-kernel SDK 6.4.22-or-newer graph-to-NBG `VIV_GPU_FILE` consumer is absent.

## Compiler acquisition and static qualification

[`compiler_acquisition_targets.json`](compiler_acquisition_targets.json)
pins PoC 9D's Radxa-linked Allwinner distribution, both nested ZIPs, the OCI
TAR and metadata, every selected tool, the exact Wave config, the Acuity
wheel, and its embedded simulator SDK.

`compiler_acquisition.py`:

- hash-checks the complete 13.5 GB nested acquisition set;
- rejects unsafe or ambiguous ZIP/TAR members;
- validates the OCI image identity without registering it;
- rehashes the manifest, config, and all 15 layer blobs;
- proves the standalone compilers are static x86-64 ELFs;
- byte-locks the recovered `VIV_GPU_FILE` parser function, format strings,
  call sites, and database lookup;
- parses the vendor-authored Wave profile through the strict PoC 9C grammar;
- scans the Acuity wrapper, wheel, and embedded SDK without importing them;
- emits an explicit gate ledger and refuses compiler execution.

Commands:

```bash
python3 -m unittest \
  waved/scripts/npu/tests/test_compiler_acquisition.py

python3 waved/scripts/npu/compiler_acquisition.py validate-targets
python3 waved/scripts/npu/compiler_acquisition.py inspect
python3 waved/scripts/npu/compiler_acquisition.py generate-evidence
python3 waved/scripts/npu/compiler_acquisition.py verify-generated
```

Output:

```text
docs/npu/profile/compiler-acquisition-evidence.json
```

The acquired package contains the exact vendor Wave config and genuine
`VIV_GPU_FILE` consumers, but they are `6.4.18:967862` standalone shader
compilers and do not emit NBG. Acuity's graph path uses
`VSIMULATOR_CONFIG` and an embedded `6.4.6:769531` simulator SDK. The
generated qualification status must remain `blocked`, with compiler,
container, and device call counts all zero.

See the
[`PoC 9D notebook`](../../../docs/npu/lab-notebook/2026-07-28-poc9d-compiler-acquisition-static-qualification.md)
for the archive chain, parser addresses, component matrix, and exact next
package requirements.

## ONNX admission and CPU fallback

The sibling [`onnx-hybrid`](../../tools/onnx-hybrid/) package provides PoC
9E/9F's general ONNX-facing boundary:

- strict policy loading bound to the PoC 9C compiler-refusal manifest;
- bounded parse, checker, shape inference, and recursive subgraph inventory;
- deterministic executable graph signatures;
- per-node placement and stable refusal reasons;
- deterministic ONNX Runtime CPU execution;
- `audit`, `cpu`, `hybrid`, and `strict` modes;
- zero NPU attempts in the current hybrid configuration;
- injectable one-attempt NPU-construction semantics for tests.

From the repository root:

```bash
cd waved/tools/onnx-hybrid
uv sync --python 3.12 --dev
uv run wave-onnx-hybrid verify-fixtures fixtures
uv run pytest -q
```

The general PoC 9 approval registry remains empty. PoC 13's exact Erf
partition is governed by a separate sealed-NBG policy and does not mutate or
globally enable that registry. See
[`docs/npu/onnx/README.md`](../../../docs/npu/onnx/README.md) for commands,
evidence hashes, the fixed Conv/Sigmoid control, and the real Silero VAD
audit.

## PoC 13 Erf-mask qualification harnesses

PoC 13 uses four fixed, separately policy-checked device boundaries:

| Harness | Device action |
|---|---|
| `npu_erf_mask_compile_lab.py` | Exactly two independent compile-only processes |
| `npu_erf_mask_execution_lab.py` | One graph and six fixed qualification vectors |
| `npu_erf_mask_stream_execution_lab.py` | One graph and the sealed 1,201-frame fixture |
| `npu_erf_mask_soak_lab.py` | One graph and exactly 10,000 deterministic frames |
| `npu_erf_cpu_npu_benchmark_lab.py` | One graph and 10,101 same-frame Cortex-A55/NPU pairs |

Their adjacent JSON manifests bind the exact binary, source, NBG, tensor
contract, device identity, library hashes, fixed remote paths, call counts,
timeouts, and prohibited-action policy. Fake transports cover preflight,
wrong hashes, malformed output, call-count drift, first-error abort,
postflight, and no-retry behavior.

The fixed runners produced exact CPU-oracle results for six vectors, the
complete fixture stream, and all ten soak checkpoints. The authoritative
commands, artifact hashes, non-claims, and production gap are in the
[`PoC 13 result guide`](../../../docs/npu/models/erf-mask/README.md) and
[`completion audit`](../../../docs/npu/models/erf-mask/completion-audit.md).

The benchmark harness additionally parses a fixed 640,036-byte little-endian
timing file, independently recomputes nearest-rank latency and paired-ratio
distributions, captures CPU-frequency/thermal context, and distinguishes
resolution-limited CPU thread-clock zeroes from mandatory positive wall
durations. PoC 13G-R2 found the Cortex-A55 LUT 20.686 times faster than NPU
`Run()` and 26.137 times faster than NPU total by ratio of means. See the
[`benchmark notebook`](../../../docs/npu/lab-notebook/2026-07-29-poc13g-wave-cpu-vs-npu.md).

These harnesses are not arbitrary graph launchers. They accept no caller
selected NBG, operation, tensor shape, runtime library, remote path, or
iteration count, and they perform no automatic retry or recovery.

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
- validation of the v2 header CRC over section payloads in serializer order;
- decoding and strict geometry validation of paired section-20/26
  dynamic-I/O data and `(offset, size)` table records;
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
138 / 138 v2 header CRC-32 values match
8,939 / 8,939 DYNAIO table entries satisfy the recovered geometry
```

Format evidence and non-claims are documented in
[`docs/npu/format/README.md`](../../../docs/npu/format/README.md) and
[`docs/npu/format/field-map.md`](../../../docs/npu/format/field-map.md).

## Lossless NBG image and serializer recovery

PoC 12A is split into three host-only tools with one shared strict manifest:

```text
nbg_serializer_targets.json
  2 exact AArch64 ELFs
  5 exact VPMN v2 NBG controls
  1 exact English CTC ONNX anchor
  21 exact serializer/load targets

nbg_serializer_recovery.py
  manifest validation and bounded Ghidra evidence

nbg_image.py
  immutable byte ownership and unchanged exact re-emission

nbg_correlate.py
  finite ONNX-initializer/NBG-section candidate search
```

Validate and verify:

```bash
python3 waved/scripts/npu/nbg_serializer_recovery.py validate
python3 waved/scripts/npu/nbg_serializer_recovery.py verify-ghidra
python3 waved/scripts/npu/nbg_image.py verify-generated

uv run --project waved/tools/onnx-hybrid \
  python waved/scripts/npu/nbg_correlate.py verify-generated
```

Intentional regeneration uses `generate-ghidra`,
`nbg_image.py generate-evidence`, and `nbg_correlate.py generate`.
Regeneration never updates input hashes automatically.

`NbgImage` reconstructs only from original raw owned ranges. It refuses
emission after `mark_changed()`, rejects overlapping owners, and records
analytical views separately. In all five real controls, the final 512 bytes
are already covered by a declared section, so the connection fingerprint is
a non-owning `section-covered-view`.

Ghidra evidence is bound independently to ELF virtual address, PT_LOAD file
offset, symbol size, and symbol-byte SHA-256. Exactly one target,
`vxoBinaryGraph_SaveNetworkBinaryGraph`, is an allowlisted four-byte AArch64
export alias. An unexpected thunk, target drift, truncation, warning, or
changed body fails verification.

The correlation registry is finite and deterministic: direct FP32, FP16,
BF16, transpose variants, fixed block patterns, and fixed byte/halfword lane
patterns. Searches are limited to explicitly admitted English CTC section
types and record every candidate hash, comparison count, and provenance.
There is no fuzzy or nearest-neighbor result. Current weight and bias
results are both unresolved, so PoC 12A did not authorize coefficient
repacking.

Canonical outputs:

```text
docs/npu/format/serializer-ghidra-evidence.json
docs/npu/format/serializer-byte-ownership.json
docs/npu/format/serializer-roundtrip-evidence.json
docs/npu/format/onnx-nbg-correlation.json
```

See the
[`PoC 12A notebook`](../../../docs/npu/lab-notebook/2026-07-28-poc12a-open-nbg-serializer.md)
for the exact results and safety accounting.

## Controlled constant differential and DYNAIO/CRC profile

PoC 12B and PoC 12C deliberately use separate analyzers:

```text
nbg_constant_diff.py
  verifies the sealed live A1/A2/B interpretation with its original finite
  registry; it does not emit or execute NBG bytes

nbg_dyna_profile.py
  combines the captured bytes, strict inventory, exact Wave libOpenVX hash,
  and bounded Ghidra exports to qualify the file CRC and section-20/26 table
```

Verify both tracked results without contacting the device:

```bash
python3 waved/scripts/npu/nbg_constant_diff.py verify-generated
python3 waved/scripts/npu/nbg_dyna_profile.py verify-generated
```

Canonical outputs:

```text
docs/npu/format/erf-constant-differential.json
docs/npu/format/dyna-io-crc-profile.json
```

The PoC 12B evidence intentionally remains
`multi-region-unresolved`: its reviewed search registry excludes section 20
and treats the header field as unresolved producer collateral. PoC 12C does
not rewrite that record. It separately proves:

- reflected IEEE CRC-32 with initial value zero over section payloads in
  directory order;
- zero substitution for section-40 checksum-value words because the
  producer fills those values after writing the header CRC;
- 138/138 matching v2 file checksums;
- paired section-20/26 presence in 111 files;
- 8,939 positive, 64-byte-aligned, ordered, non-overlapping, in-bounds table
  entries and 83,024 zero padding bytes;
- one exact in-memory A-to-B reconstruction at section-20-relative offset 3.

`nbg_dyna_profile.py` writes only canonical metadata evidence. Its
controlled reconstruction is a transient `bytearray`; the result records
`written_to_disk: false`. This is not an arbitrary section writer, operation
encoder, coefficient packer, ONNX compiler, or graph runner.

See the
[`PoC 12B notebook`](../../../docs/npu/lab-notebook/2026-07-29-poc12b-erf-constant-differential.md)
and
[`PoC 12C notebook`](../../../docs/npu/lab-notebook/2026-07-29-poc12c-dyna-io-crc-profile.md).

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

## Fixed custom-compiler and execution labs

PoCs 5 through 8 use dedicated harnesses rather than adding arbitrary graph
arguments to `npu_lab.py`. Each harness has a source-controlled policy and
manifest:

| Harness | PoC | Live action |
|---|---:|---|
| `npu_compile_lab.py` | 5B | Compile-only Add |
| `npu_compile_erf_lab.py` | 5C | Compile-only Erf |
| `npu_caps_lab.py` | 5D | Identity/capability query only |
| `npu_compile_signal_frame_lab.py` | 5E | Compile-only SignalFrame |
| `npu_clc_loader_lab.py` | 5F-A | CLC relocation only; calls no compiler symbol |
| `npu_compile_signal_frame_clc_lab.py` | 5F-B | One compile-only CLC compatibility observation |
| `npu_erf_execution_lab.py` | 6 | One zero-input generated-NBG execution |
| `npu_erf_nonzero_execution_lab.py` | 7 | One nonzero generated-NBG execution |
| `npu_compile_audio_denoiser_lab.py` | 8A | Compile-only fixed MatrixMul |
| `npu_audio_denoiser_lab.py` | 8B | One direct fixed MatrixMul attempt |
| `npu_compile_conv_audio_denoiser_lab.py` | 8C | Compile-only fixed Conv2d |
| `npu_conv_audio_denoiser_lab.py` | 8D | One direct fixed Conv2d attempt |

The wrapper entry points reuse policy-tested implementations. They do not
accept operation, model, tensor, input, output, environment, timeout,
iteration, retry, benchmark, or recovery overrides.

### Manifest inspection

The `manifest` subcommand is host-only:

```bash
python3 waved/scripts/npu/npu_erf_execution_lab.py manifest
python3 waved/scripts/npu/npu_erf_nonzero_execution_lab.py manifest
python3 waved/scripts/npu/npu_audio_denoiser_lab.py manifest
python3 waved/scripts/npu/npu_conv_audio_denoiser_lab.py manifest
```

Compile-only wrappers expose the analogous manifest command. Loading a
manifest validates:

- exact device and kernel identity;
- exact remote executable and isolated root;
- fixed minimal environment and deadline;
- every required artifact SHA-256;
- required CLC absence or the one PoC 5F candidate;
- fixed graph/tensor/result expectations;
- no unknown or duplicate JSON keys.

### Live run shape

A reviewed one-shot invocation has the form:

```bash
python3 waved/scripts/npu/npu_erf_execution_lab.py run \
  --run-id poc6-erf-execution-1-20260728
```

Do not copy this command with an old run ID: run directories are exclusive,
and a repeated live action needs an explicit purpose and review. Unit tests
do not require a device.

For every live run, the harness:

1. creates the evidence directory exclusively;
2. captures and writes `preflight.json`;
3. refuses before native launch on any identity/hash/mode/holder/CLC/health
   mismatch;
4. invokes the one fixed executable once;
5. recovers the legacy Wave shell's native status from an unpredictable
   exactly-once sentinel;
6. captures bounded stdout and stderr;
7. captures `postflight.json` even after native failure;
8. compares boot, kernel, device node, NPU parameters, hashes, file states,
   holders, processes, and bounded kernel diagnostics;
9. writes `run.json` with the attempt count and exact classification.

An execution failure is evidence, not permission to retry.

### Legacy ADB status contract

This Wave's legacy ADB shell may return host status zero even when the remote
shell exits nonzero. The harness therefore treats:

- nonzero ADB status as a transport failure;
- the unpredictable exactly-once sentinel as the remote status;
- a missing, duplicate, malformed, or out-of-range sentinel as a result
  failure.

The remote command is built from validated tokens and an empty environment.
No user-provided shell fragment is accepted.

### Run artifact layout

```text
artifacts/npu/runs/RUN_ID/
  preflight.json
  stdout.txt
  stderr.txt
  postflight.json
  run.json
  candidate-1.nbg       # only when a compile-only result passes
  candidate-1.report.json
```

Output and partial-output states are part of both snapshots. A compile-only
pass pulls an NBG only after native success and unchanged health, then runs
the strict host parser. A failed compile leaves no synthetic result file.

### Current results

```text
PoC 5C: Erf NBG export                  PASS
PoC 6:  zero-input Erf NBG execution   PASS
PoC 7:  nonzero Erf NBG execution      PASS

PoC 5B: Add NBG export                  FAIL
PoC 8A: MatrixMul NBG export            FAIL
PoC 8B: MatrixMul graph verification    FAIL
PoC 8C: Conv2d NBG export               FAIL
PoC 8D: Conv2d graph verification       FAIL
```

The successful NBG is 5,840 bytes with SHA-256:

```text
6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee
```

The PoC 5F CLC candidate is quarantined after a compiler/program-object ABI
failure. No harness or documentation command authorizes another call to its
compiler entry points.

Native sources, pinned builders, source/ELF policies, staging behavior, and
binary hashes are documented in
[`waved/tools/npu-compiler/README.md`](../../tools/npu-compiler/README.md).
Long-form results are indexed by the
[`NPU notebook`](../../../docs/npu/lab-notebook/README.md).

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
header and section corruption, recovered file-CRC scope/order,
section-20/26 pairing and table geometry, zero-padding enforcement, prefix
truncation, seeded random inputs, source-relative deterministic output, and
exact live-CTC metadata.

The PoC 12A tests additionally cover strict manifest and ELF policy,
function-byte/Ghidra binding, the one approved alias thunk, complete
exclusive ownership, connection-view handling, alignment and opaque-byte
preservation, semantic-change refusal, exact five-file round trips, finite
transform generation, ambiguous and negative correlation controls, and
stale generated-evidence rejection.

The PoC 12B/12C tests additionally preserve the sealed finite-registry
classification, reject stale capture evidence, prove deterministic
four-byte DYNAIO mapping, verify in-memory-only exact reconstruction,
exercise section-40 CRC normalization, aggregate the full-corpus geometry,
and reject nondeterministic or multi-section controlled changes.

The trace tests additionally cover framing, volatile-value redaction,
pointer-token rejection, metadata bounds, command/status pairing, phase
correlation, the narrow final-thread exception, strace/GAL count agreement,
repeatability, and exact ephemeral executable policy.

The compiler/execution tests additionally cover policy-specific manifest
pinning, CLC isolation, staged modes, absent and pre-existing outputs,
hardware identity parsing, selector parsing, exact result lines, legacy ADB
remote status, one-attempt behavior, preflight refusal, postflight after
failure, process/holder detection, and new kernel-alert classification.

The architecture-profile tests additionally cover exact four-role target
selection, duplicate-key and path-escape refusal, hash and ELF validation,
unique table-row recovery, differential row invariants, evidence provenance,
strict fact/source enums, explicit unknown accounting, and deterministic
profile generation.

The compiler-profile tests additionally cover strict 17-artifact policy,
quarantine and isolated-dependency enforcement, duplicate and unknown keys,
path and symlink escape, ELF/text/hash validation, exact five-field parsing,
numeric and size bounds, catalog geometry and unique Wave-row recovery,
complete static parser evidence, live-profile provenance, deterministic
candidate generation, and refusal below the confidence gate.

The compiler-acquisition tests additionally cover strict literal handling,
ambiguous and escaping archive paths, ZIP traversal/duplicates/symlinks, TAR
traversal/special files/escaping links, cross-buffer archive scanning,
static ELF proof, dynamic and wrong-machine refusal, relative-call decoding,
the no-kernel SDK version gate, and canonical evidence serialization. The
full evidence verifier separately rehashes every acquired byte and OCI blob.

The separate ONNX tool tests cover fixture determinism, recursive graph and
constant bounds, nested external-data refusal, graph-signature stability,
policy/compiler binding, all-CPU placement reasons, CPU/hybrid equality,
strict refusal, one-attempt construction fallback, exact tensor validation,
and exclusive CLI outputs.

Do not weaken a contract merely to make a new probe run. Update the design and
evidence model first.
