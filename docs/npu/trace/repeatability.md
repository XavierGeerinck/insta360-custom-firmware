# PoC 3 Repeatability and Acceptance

## Accepted runs

| Field | Run 1 | Run 2 |
|---|---|---|
| Run ID | `poc3-trace-framed-1-20260728` | `poc3-trace-framed-2-20260728` |
| UTC | `2026-07-28T11:22:35.395Z` | `2026-07-28T11:24:34.530Z` |
| Definition SHA-256 | `dcc6efe…19992` | `dcc6efe…19992` |
| Classification | success | success |
| Remote exit | 0 | 0 |
| Host ADB duration | 347 ms | 337 ms |
| Native mean, descriptive | 3.562 ms | 3.474 ms |
| Output | 332,800 bytes | 332,800 bytes |
| Output SHA-256 | `e621b213…52d4` | `e621b213…52d4` |
| Cleanup | 0 | 0 |
| Raw stdout SHA-256 | `72ed5125…db79` | `03d8f835…ffeac` |
| Raw stdout bytes | 83,657 | 83,657 |
| Raw stderr SHA-256 | empty-file digest | empty-file digest |
| GAL call count | 73 | 73 |
| Completed status pairs | 73 | 73 |
| Sequence SHA-256 | `33b93229…1c226` | `33b93229…1c226` |

The Wave ADB shell presents a PTY, so the framed trace is captured in
`stdout.txt`; `stderr.txt` is empty. The wrapper makes this deterministic by
emitting the clean native result first, followed by named GAL and strace
frames.

## Machine comparison

[`repeatability.json`](repeatability.json) reports all checks true:

```text
control_result_equal
lifecycle_equal
path_observations_equal
driver_events_equal
gal_sequence_equal
gal_sequence_sha256_equal
gal_completion_equal
syscall_counts_equal
health_equal
```

The required equivalence gate excludes raw timing and address data. In these
two runs even the non-required syscall and completion checks matched.

## Normalization

The strict normalizer removes:

- PIDs and TIDs;
- file descriptor numbers;
- timestamps and syscall durations;
- mapping and ioctl argument addresses;
- probe timing text;
- host run-directory names.

It retains:

- semantic output and cleanup;
- selected health fields;
- required artifact identities;
- lifecycle phase descriptions and trace order;
- normalized successful path observations;
- driver open/close/missing events;
- selected syscall counts;
- command, adjacent name, hardware, core, interface size, and phase;
- aggregate ioctl completion evidence.

The serializer rejects any pointer-shaped hexadecimal value. Unknown trace
syntax, malformed GAL fields, duplicate fields, invalid sizes, sequence
gaps, mismatched enter/exit metadata, nonzero status, ioctl count drift, and
failed/unresolved ioctl results all fail normalization.

## Failed framing pilot

The first instrumented run is retained as:

```text
artifacts/npu/runs/poc3-trace-1-20260728/
```

It used definition SHA-256:

```text
406273cae098d403d1404887b0ed2762852723a94036a594edaa34f45211c6b8
```

Observed:

| Field | Pilot |
|---|---|
| Remote exit | 0 |
| Harness classification | `result-parse-failure` |
| Reason | No line began with exactly `RESULT:` |
| Native result visible inside raw stream | PASS, expected bytes/digest/cleanup |
| GAL ioctls visible | 73, all syscall results zero |
| Post-run ADB | responsive |
| NPU parameters | unchanged |
| Required hashes | unchanged |
| Raw stdout | 85,305 bytes, SHA `3212a123…f640` |

Cause:

```text
strace stderr ─┐
probe stdout  ─┼─ same remote PTY → byte interleaving inside strace lines
GAL stderr    ─┘
```

The native `RESULT` bytes appeared in the middle of a traced `write` line,
so the harness correctly refused to reinterpret it.

The fix did not weaken parsing. The wrapper instead:

1. writes strace and GAL diagnostics to two exact mode-0600 tmpfs files;
2. lets the stock result remain a normal standalone stdout line;
3. appends both traces between exact framing markers;
4. deletes both temporary files;
5. returns the stock probe exit status.

A new wrapper hash and experiment-definition hash were preflighted before the
accepted runs.

## Final-thread observation

The pilot also revealed a teardown race in the probe:

1. the worker can start a final unlock ioctl;
2. the kernel syscall returns zero;
3. the main thread can call `exit_group`;
4. process exit may preempt the interposer's post-call log.

The framed acceptance runs captured all 73 exit records, so no exception was
needed in the published evidence. The normalizer nevertheless test-locks a
narrow fallback:

- only the final contiguous call may lack an exit record;
- the strace ioctl count must equal the GAL entry count;
- every strace ioctl result must be zero;
- no returned HAL status is invented.

This variability is why completion equality is reported separately from the
stable command sequence.

## Safety comparison

For both accepted runs:

```text
adb_responsive:                        true
post_snapshot_complete:                true
npu_parameters_unchanged:              true
required_artifacts_unchanged:          true
galcore_present:                       true
timeout_requires_manual_holder_review: false
```

Before and after:

```text
nanoqFreq = 838860800
enableNN  = 255
recovery  = 1
```

Both wrapper-owned raw tmpfs files were absent after each run. No service,
stock library, stock model, sysfs value, module, boot setting, or firmware
was modified.

## Gate assessment

| Acceptance item | Result |
|---|---|
| Complete successful stock create/run/retrieve/destroy | pass |
| Driver reachability and runtime open identified | pass |
| Allocation/lock boundaries identified | pass |
| Submit/event/synchronization family identified | pass |
| Unlock/release boundary identified | pass |
| Same normalized state sequence twice | pass |
| Output digest unchanged by tracer | pass |
| Device state and hashes unchanged | pass |
| No raw mutation/replay/payload capture | pass |

PoC 3 therefore passes. The next gate may compare a legitimate tiny
Wave-targeted graph against this control. Captured request replay and
unverified graph execution remain outside the result.
