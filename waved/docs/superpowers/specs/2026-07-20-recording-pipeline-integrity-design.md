# Recording pipeline integrity design

## Goal

Make every failure between ALSA capture and the finalized meeting WAV explicit,
durable, and visible. Waved must never label or upload a take as healthy after
capture stopped, audio frames were dropped, or the WAV writer failed.

The feature covers:

- unexpected capture EOF or fatal read failure;
- ALSA xruns while a recording is active;
- raw-frame channel saturation;
- processed-frame channel saturation or closure;
- WAV pre-roll/live write failure;
- races between a fault and Stop/finalization;
- on-device fault notification; and
- safe recovery through the existing process watchdog.

Long-session, power-loss, storage-pressure, webhook-failure, and stock audio
comparison remain separate roadmap items. This work provides the integrity
signals those tests need.

## Current failure modes

1. `AudioService` retains `_frame_tx`, so an exited capture producer cannot
   close `frame_rx`.
2. The capture thread's `JoinHandle` is discarded. EOF or four consecutive
   fatal read errors only stop that thread.
3. Capture backpressure and xruns are counted, but an active meeting is not
   told that audio was lost.
4. `AudioService` ignores `meeting_tx.try_send` failures.
5. `ActiveRecording::append_samples` logs a writer error and returns no fault
   to `MeetingService`.
6. A take can therefore finalize, receive completed metadata, and enter the
   upload queue after a silent integrity failure.

## Policy

The implementation is strict and fail-closed:

- a confirmed integrity fault during a take makes that take
  `artifact_error`;
- its inspectable partial WAV is preserved;
- it never receives completed metadata or enters an uploadable state;
- prior recordings and database rows are untouched;
- unexpected capture termination restarts the full application through the
  existing watchdog;
- storage/write failure blocks new recordings without causing a watchdog
  restart loop; and
- recoverable faults occurring only while idle remain observable counters and
  do not poison the next take.

In-process ALSA reconstruction is intentionally excluded. Reopening a source
inside an active process cannot guarantee continuous timestamps, AEC state,
pre-roll ownership, or sample order.

## Integrity model

### Shared epoch and counters

Add a small `RecordingIntegrity` object shared by the capture thread,
`AudioService`, and `MeetingService`. It contains atomic:

- integrity epoch;
- last typed fault kind;
- xrun count;
- raw-frame drop count; and
- processed-frame drop count.

Its `record_fault(kind)` operation stores the fixed fault kind and increments
the epoch before publishing a signal. An `ActiveRecording` snapshots the epoch
at start. Before a take can finalize, `MeetingService` compares the current
epoch with the take's start epoch. If the detailed signal is delayed, the
stored kind supplies the sanitized database reason.

This closes the Stop/fault race: even if Stop wins the asynchronous select and
the detailed fault message is still queued, the already-incremented epoch
prevents successful finalization.

An idle fault is naturally excluded because the next take snapshots the newer
epoch.

### Typed faults

Add typed fault and exit reasons rather than passing arbitrary producer strings:

```rust
enum RecordingIntegrityFault {
    CaptureXrun,
    RawFrameDropped,
    ProcessedFrameDropped,
    ProcessedChannelClosed,
    WavWriteFailed,
}

enum CaptureExit {
    StopRequested,
    EndOfStream,
    FatalRead,
}
```

The UI and database map these types to fixed, sanitized messages. Raw ALSA,
filesystem, and endpoint strings remain in logs only.

Fault producers send `IntegritySignal` values over a dedicated unbounded
channel to `MeetingService`. The channel is control-plane only; audio remains
on bounded channels.

### Unified fatal supervisor

`run_capture_loop` returns `CaptureExit`, and `AudioService::run` returns a
typed audio-service exit. The named capture thread and the Tokio audio task
send unexpected terminal results to one async fatal-pipeline supervisor.

For capture `EndOfStream`/`FatalRead`, raw-channel closure, or processed-channel
closure, the supervisor:

1. records the typed fault in the shared integrity object;
2. sends the typed integrity signal with a one-shot acknowledgement;
3. waits up to two seconds for `MeetingService` to persist/abort;
4. requests application shutdown whether acknowledgement succeeded or timed
   out.

`StopRequested` during normal shutdown does not create a fault or restart
cycle. The supervisor acts only on the first fatal exit, so the capture thread
and audio task closing in response to one failure cannot initiate duplicate
shutdown sequences.

The top-level shutdown sequence remains responsible for sending
`Command::Shutdown`, setting the capture stop flag, and allowing services to
flush before process exit. The existing `S60waved` watchdog then starts a
fresh ALSA/DSP pipeline.

## Capture and DSP behavior

### Capture thread

`run_capture_loop` gains the shared integrity handle and signal sender.

- A recovered ALSA xrun increments the xrun counter and integrity epoch, then
  emits `CaptureXrun`.
- Raw-channel `Full` increments the raw-drop counter and epoch, then emits
  `RawFrameDropped`.
- Raw-channel `Closed` returns `EndOfStream` unless normal stop was requested.
- Source EOF returns `EndOfStream`.
- More than three consecutive non-xrun read failures returns `FatalRead`.
- A requested stop returns `StopRequested`.

Rate-limited logging remains, but counters and typed signals are the
authoritative machine-readable result.

### Audio service

Remove the retained `_frame_tx`. Production owns the only raw-frame sender in
the capture thread; tests keep their sender explicitly.

`AudioService` handles `frame_rx.recv() == None` as a pipeline closure and
returns a typed fatal exit instead of waiting or spinning.

For each processed frame:

- successful `meeting_tx.try_send` is unchanged;
- `Full` increments the processed-drop counter and epoch, then emits
  `ProcessedFrameDropped`; and
- `Closed` ends the audio service with `ProcessedChannelClosed`; the unified
  supervisor records, acknowledges, and shuts down that fatal path.

UAC and stats broadcast behavior is unchanged.

## Meeting and artifact behavior

### Recording writes

`append_pre_roll` and `append_samples` return `Result<u64, RecordingWriteError>`.
The first failed write is terminal for that active take.

Add an explicit `ActiveRecording::abort()` path that:

1. finalizes the WAV header best-effort;
2. syncs the partial file and recording directory best-effort;
3. keeps the name `audio.wav.partial`; and
4. never creates `audio.wav` or `metadata.json`.

Drop remains a last-resort safety net, but normal fault handling calls
`abort()` explicitly.

### Single-writer fault serialization

`MeetingService` remains the sole owner of `ActiveRecording` and the sole
authority for terminal meeting state.

On a fault while active it:

1. removes the active recorder;
2. finalizes any open diarization segment as incomplete context only;
3. aborts the partial WAV;
4. atomically writes `upload_status = 'artifact_error'`, `end_time`,
   sample-derived duration, and a bounded `last_error`;
5. clears recording, pause, meeting ID, and pending-decision state;
6. sets the recording-block reason only for storage/write faults;
7. emits `RecordingIntegrityFailed`; and
8. acknowledges the fault supervisor when an acknowledgement is present.

On Stop, `MeetingService` drains immediately queued audio/control messages and
checks the shared epoch before exposing the final WAV. A changed epoch uses the
same abort path. Only an unchanged take may rename the WAV, generate metadata,
and transition to automatic/ask/local-only.

Repeated or late signals are idempotent. Once no active recorder exists, they
update counters/health but cannot rewrite a completed meeting's terminal
state.

### Durable diagnostics and upload exclusion

Add a database helper dedicated to artifact faults. It stores only a fixed
sanitized reason, truncated by the existing diagnostic limit.

`artifact_error` already appears in failed counts and is already excluded from
upload claiming and safe-retention candidates. Tests will pin those
properties.

Reconciliation continues preserving partial artifacts as `artifact_error`.

## State and UI

Extend continuous state with:

- capture health;
- recording blocked reason;
- last recording integrity error;
- xrun/raw-drop/processed-drop counters.

Add a `DeviceEvent::RecordingIntegrityFailed` event carrying an optional
meeting UUID and a typed, copy-safe reason.

UI mapping:

- active take aborted: **Recording interrupted**;
- idle fatal capture failure: **Audio restarting...**;
- new Start while storage/write-blocked: **Recording unavailable**.

The Webhook/System status view may show the fixed sanitized reason. It never
renders raw filesystem paths, ALSA device strings, credentials, or arbitrary
error text.

After watchdog recovery capture health starts clean. A persistent
storage/write block clears only after restart and successful recording-storage
inspection.

## Failure boundaries

- One xrun or dropped audio frame during a take is sufficient to mark it
  damaged. A later clean frame does not restore the take.
- Faults while idle do not block a future take unless the fault is a storage
  write failure.
- A fatal capture exit always leads to process exit, even if meeting
  acknowledgement times out.
- A writer fault never triggers automatic restart, avoiding a disk-failure
  restart loop.
- No failure path deletes a partial, local-only, queued, failed, or uploaded
  artifact.
- Fault handling must remain bounded; it may not wait indefinitely for the
  database, UI, or another task.

## Tests

### Unit and integration

- capture loop returns typed normal, EOF, xrun, backpressure, and fatal exits;
- the shared epoch increments before each signal;
- removing the final raw sender closes `AudioService`;
- raw and processed channel saturation update counters and signal faults;
- injected WAV writer failure returns an error and preserves only the partial;
- pre-roll failure follows the same terminal path as live-write failure;
- fault versus Stop produces exactly one terminal state;
- epoch mismatch prevents finalization even when its signal is delayed;
- artifact fault persistence records a sanitized reason;
- `artifact_error` cannot be claimed for upload or retention;
- UI copy mapping is exact ASCII and ignores unrelated events; and
- unified-supervisor acknowledgement success and timeout both reach the intended
  shutdown outcome.

### Device acceptance

Build a fault-injection variant behind a non-default
`recording-integrity-test-hooks` feature. The hook is absent from production
builds and lets the HIL script inject typed capture, drop, and writer faults.

Acceptance sequence:

1. back up the binary, config, database, and recording inventory;
2. deploy the test-hook build;
3. inject each fault while idle and during a short take;
4. verify the toast, database status/reason, partial file, upload exclusion,
   and watchdog behavior;
5. deploy the normal production build;
6. record and upload a clean take;
7. run a longer recording and compare sample-derived duration, WAV header,
   database duration, and all integrity counters; and
8. confirm all pre-existing recordings are unchanged.

## Non-goals

- repairing or uploading a damaged partial take automatically;
- concealing small gaps with inserted silence;
- reopening ALSA inside the same process;
- redesigning the webhook contract;
- changing audio DSP algorithms; or
- implementing the separate one-hour/power-loss/storage-pressure roadmap
  suites in this change.
