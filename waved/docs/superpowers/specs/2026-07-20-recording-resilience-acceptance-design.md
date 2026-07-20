# Recording resilience acceptance design

## Goal

Prove that Waved can record for an extended period without silent audio loss,
classify interrupted recordings safely after reboot, and protect all
non-delivered artifacts under storage pressure.

This work completes the remaining P0 recording-integrity acceptance items. It
adds deterministic automated checks and a real-device harness, runs a shortened
proof immediately, and leaves the same harness ready for a literal one-hour
certification run.

## Scope

The implementation covers:

- measuring process memory, audio integrity counters, free space, WAV growth,
  finalized sample count, and duration during a configurable recording;
- rebooting during an active recording and interrupting known finalization
  phases;
- reconciling every interrupted artifact into a durable, non-uploadable or
  conservatively local-only state;
- exercising both configured storage thresholds on the device without filling
  its real filesystem;
- proving that retention deletes only recordings already marked `uploaded`;
- blocking a new recording when storage remains constrained and no delivered
  artifact can be pruned; and
- documenting a one-hour invocation after the shortened acceptance run passes.

Webhook failure behavior, stock audio-quality comparison, and literal
uncontrolled power removal remain separate roadmap work. An authorized reboot
provides the interruption mechanism for this acceptance pass.

## Safety boundaries

Existing recordings under `/mnt/udisk/recordings` must never be used as storage
pressure fixtures. Storage acceptance creates a unique temporary root and
SQLite database, records their paths in the test log, and removes only that
root after successful inspection.

The harness may restart or reboot the device. Before each disruptive action it
must capture the active meeting UUID, database status, artifact listing, binary
checksum, and process identity. After reconnecting, it must wait for one stable
application process and verify that the expected production or HIL binary is
running before evaluating reconciliation.

No test may create a large filler file on the real recording volume. Threshold
conditions are injected into the retention decision boundary while file
deletion and SQLite updates still execute against the on-device fixture.

## Architecture

### Retention decision boundary

Split storage inspection from retention enforcement:

- `DiskUsage` becomes a crate-visible value containing `used_percent` and
  `free_mb`.
- Production `enforce` obtains that value from `statvfs`.
- An internal `enforce_for_usage` path accepts an explicit value and performs
  the same candidate selection, deletion, database update, and
  `storage_blocked` state transition.

Production and acceptance therefore share one pruning implementation. Tests do
not mock candidate eligibility or deletion; they inject only the filesystem
capacity reading that would otherwise require filling the device.

The meeting service runs retention enforcement at startup, after successful
finalization, and immediately before each `StartRecording`. A start proceeds
only if that latest check clears `storage_blocked`.

### On-device fixture runner

Add a small feature-gated acceptance binary or subcommand that:

1. creates a dedicated fixture root and database;
2. inserts artifacts for every protected state: recording, queued, retrying,
   awaiting decision, local-only, artifact error, and incomplete;
3. inserts multiple uploaded artifacts with deterministic ages;
4. invokes the shared retention path first with a percentage threshold breach
   and then with a minimum-free-space breach;
5. proves oldest-first deletion of uploaded artifacts;
6. checks every protected directory and row byte-for-byte for survival; and
7. checks `storage_blocked` when no uploaded candidate remains.

The runner emits a concise machine-readable result so the shell harness can
fail on any missing assertion. It accepts only a caller-provided fixture root
under `/tmp` or another explicit safe prefix.

### Recording resilience harness

Add `scripts/test-recording-resilience.sh`. Its important inputs are:

- `ADB_SERIAL`;
- `DURATION_SECONDS`, with a short acceptance default;
- `SAMPLE_INTERVAL_SECONDS`;
- the expected binary checksum; and
- an explicit acknowledgement for disruptive reboot cases.

The endurance phase starts a real recording through the same device command
path used by the UI. At each interval it records:

- timestamp and application PID;
- `VmRSS` from `/proc/<pid>/status`;
- available recording-volume space;
- WAV partial-file size;
- capture frames, xruns, raw drops, and processed drops; and
- the active meeting UUID and database status.

After Stop, the harness waits for durable finalization, pulls the WAV and
metadata, and verifies:

- RIFF/WAVE PCM format, 48 kHz, 16-bit, mono;
- data-chunk byte count and corresponding sample count;
- audio duration derived from samples;
- metadata audio duration, file size, and SHA-256;
- absence of `audio.wav.partial`;
- zero new drop/xrun counters during the accepted take; and
- a valid terminal status for the configured upload mode.

The harness reports initial, peak, and final RSS. The shortened proof detects
gross leaks; the one-hour certification is the authoritative memory-stability
measurement. A bounded absolute and percentage growth policy is stated in the
script output and applied only after an initial warm-up window.

### Interruption checkpoints

Use feature-gated HIL checkpoints so finalization interruption is
deterministic. Checkpoints write a ready marker and pause at these boundaries:

1. active recording with only `audio.wav.partial`;
2. WAV finalized but metadata not yet committed; and
3. WAV and metadata committed but the meeting row still says `recording`.

The harness waits for the relevant marker and then reboots the device. The
checkpoint feature is absent from the production build.

After each reboot, normal boot reconciliation must produce:

| Interrupted state | Required result |
| --- | --- |
| Partial WAV only | `artifact_error`; partial retained; never uploadable |
| Final WAV without metadata | `missing_artifact`; WAV retained; never uploadable |
| Final WAV plus metadata, row still recording | `local_only`; pair retained; never uploaded automatically |

The final case is deliberately conservative. Reconciliation cannot infer the
upload choice that had not yet been committed, so it preserves the completed
pair without sending it.

Every reconciliation check is run twice to prove idempotency.

## Error handling

The shell harness stops immediately on:

- unexpected device disconnect outside a planned reboot;
- a changed binary checksum;
- more than one long-lived application process;
- any new integrity counter during the accepted endurance recording;
- a malformed or inconsistent WAV/metadata pair;
- an artifact becoming uploadable contrary to the matrix;
- deletion of a protected storage fixture; or
- failure to block a start under unrecoverable storage pressure.

Failure retains the fixture and pulled evidence and prints their paths. Success
may clean temporary device state after evidence has been copied locally.

The normal application remains fail-closed: an actual write failure retains a
partial artifact, records `artifact_error`, and keeps recording blocked until
restart and successful storage inspection.

## Tests

Host tests cover:

- both `DiskUsage` threshold comparisons;
- oldest uploaded selection and deletion;
- preservation of every non-uploaded status;
- transition to `storage_blocked` with no safe candidate;
- clearing the block after capacity becomes acceptable;
- pre-start storage re-evaluation; and
- all three reconciliation outcomes, including a second idempotent pass.

The shortened on-device acceptance covers:

- a real recording and final WAV/metadata validation;
- periodic RSS, integrity, file-growth, and free-space sampling;
- the isolated retention fixture for both thresholds;
- reboot during an active recording;
- reboot at both deterministic finalization checkpoints; and
- post-test restoration of the production binary and watchdog-managed process.

The literal certification command uses `DURATION_SECONDS=3600` with the same
script and assertions; only the duration and sampling count differ.

## Acceptance criteria

This work is complete when:

1. all new host tests and existing workspace tests pass;
2. the production ARM64 build passes;
3. the shortened on-device run passes without new xruns or dropped frames;
4. WAV sample count, duration, size, checksum, and metadata agree;
5. all interrupted artifacts match the reconciliation matrix after two passes;
6. both storage thresholds prune only uploaded fixtures;
7. a constrained fixture with no uploaded candidate blocks a new start;
8. real recording directories remain unchanged by storage testing;
9. the device is restored to the production binary and stable watchdog state;
   and
10. the one-hour command and expected evidence are documented in `TODO.md`.
