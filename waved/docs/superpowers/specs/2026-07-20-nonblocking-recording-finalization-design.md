# Non-blocking recording finalization design

**Date:** 2026-07-20

## Context

The one-hour recording-resilience run completed its active capture with no
XRUNs, raw-frame drops, or processed-frame drops. The finalized 48 kHz mono
PCM WAV and its metadata matched byte-for-byte. During Stop, however,
`processed_frames_dropped` increased from 0 to 183 while `MeetingService`
calculated SHA-256 over the 349 MB WAV.

`MeetingService::run` owns the processed-audio receiver and awaits
`stop_recording` directly. `stop_recording` closes and syncs the WAV, reads the
entire finalized file for SHA-256, writes metadata, and commits the database
state. The service cannot receive processed audio during that await, so the
bounded channel eventually fills. `AudioService` then correctly reports
processed-frame drops.

The stopped take is complete before these drops occur, so its WAV is not
damaged. The drops still matter: they generate an idle integrity fault and
remove audio that should be available as pre-roll for the next take.

## Goals

- Keep receiving processed audio throughout WAV finalization, hashing,
  metadata persistence, and final database persistence.
- Store audio received after Stop in a fresh pre-roll buffer for the next
  recording.
- Keep `StartRecording` blocked until the previous take is durably finalized.
- Preserve existing durable-artifact ordering, upload modes, UI events,
  integrity behavior, and interruption-test checkpoints.
- Move synchronous WAV close, sync, and rename work off the async runtime
  worker.
- Prove with deterministic tests that finalization cannot back up the
  processed-audio channel.

## Non-goals

- Allowing overlapping recordings.
- Allowing a new recording while an earlier take is still finalizing.
- Changing the WAV or webhook metadata formats.
- Changing upload, retry, retention, or recording-decision behavior.
- Masking real capture, DSP, or channel-integrity faults.
- Treating a larger channel as a substitute for continuous draining.

## Considered approaches

### Finalization-aware drain loop

After Stop, `MeetingService` resumes a cleared pre-roll buffer and awaits the
finalization future inside a small `tokio::select!` loop. The loop receives
processed audio into that buffer and handles integrity and DoA input until
finalization completes. Normal command handling remains suspended, preserving
the current rule that a new recording cannot start during finalization.

This is the selected approach. It fixes the bounded-channel failure without
introducing another long-lived service or changing user-visible sequencing.

### Detached finalizer worker

A dedicated worker could own stopped recordings and send completion messages
back to `MeetingService`. This would keep the main loop responsive to every
command, but it would require new lifecycle state, completion channels,
shutdown coordination, and explicit handling for commands received while a
take is finalizing. That complexity is unnecessary while starts are
deliberately blocked.

### Larger processed-audio channel

Increasing channel capacity would absorb some finalization delay but would
still fail for sufficiently large recordings or slower storage. It would also
consume more memory and leave the architectural blockage intact.

## Design

### Finalization job

The durable work currently performed by `MeetingService::stop_recording` will
be represented as one future with owned or cloned dependencies:

- the stopped `ActiveRecording`;
- cloned configuration and database pool;
- device ID;
- meeting/event state senders;
- integrity state;
- recording and retention settings.

Closing, syncing, and atomically renaming the WAV are synchronous filesystem
operations. The job runs `ActiveRecording::finalize` with
`tokio::task::spawn_blocking`. The remaining database reads, SHA-256 stream,
atomic metadata write, database transition, state publication, event
publication, and retention enforcement retain their current order and
semantics.

The existing HIL checkpoints remain at the same durable boundaries:

1. active partial WAV;
2. finalized WAV before metadata publication;
3. finalized metadata before the database becomes upload-eligible.

### Draining during finalization

When Stop is accepted:

1. drain already queued processed frames and integrity signals into the active
   take, as today;
2. finalize the diarizer;
3. clear and resume the pre-roll buffer immediately;
4. create and pin the finalization job;
5. select between finalization completion and pipeline inputs.

While the job is pending:

- processed frames are pushed into the pre-roll buffer;
- integrity signals are treated as idle-pipeline faults, published normally,
  and acknowledged;
- DoA frames are drained and discarded because there is no active diarizer;
- a closed DoA stream retains the existing warning and closed-state behavior.

The finalization branch is polled before lower-priority ready branches so a
continuously active audio stream cannot starve durable completion.

Commands are not consumed inside this loop. This preserves the current
serialized behavior: Stop does not complete from the service’s perspective,
and no Start or recording decision can overtake finalization. Wi-Fi is a watch
channel, so a connectivity change observed after finalization still exposes
the latest state and wakes upload delivery.

### State invariants

- `active` is `None` as soon as Stop takes ownership of the recording.
- `MeetingState.recording` remains true until durable finalization succeeds or
  reaches a terminal artifact error, matching current UI behavior.
- The pre-roll buffer contains only audio received after the accepted Stop.
- `StartRecording` cannot be processed until finalization exits.
- `RecordingStopped` is published only at the same durable point as today.
- Ask-mode `pending_decision` is created only after both artifacts and the
  final database state are durable.
- Automatic-mode upload wake occurs after finalization returns to the command
  branch.

### Shutdown

Because commands remain serialized, a Shutdown command queued after Stop is
processed only after finalization completes. A Shutdown received while a
recording is still active follows the existing implicit-stop path. Its
finalization also uses the drain-aware path before the service performs the
existing implicit save for an Ask-mode pending decision and exits.

### Failure handling

- WAV finalization failure records `WavWriteFailed`, marks the meeting
  `artifact_error`, clears active UI state, and does not create uploadable
  metadata.
- Metadata/hash failure keeps the finalized WAV, marks `artifact_error`, and
  never makes the take upload-eligible.
- Database final-state failure retains the existing stopped event and UI
  cleanup behavior.
- A processed-audio or DoA channel closure retains the main run loop's
  existing closed-channel behavior and cannot create a busy receive loop.
- Genuine processed-frame drops, raw drops, capture errors, and XRUNs retain
  their existing counters and diagnostics. The design removes the service's
  self-inflicted backpressure; it does not suppress detection.
- A `spawn_blocking` join failure is handled as a WAV-finalization failure and
  follows the same fail-closed path.

## Testing

### Unit-level regression

Extract the await-and-drain behavior behind a focused helper or test seam. A
test holds a finalization future pending while sending more processed frames
than the bounded channel can hold. It asserts:

- the sender does not become blocked or fail because the receiver is drained;
- finalization still completes;
- the pre-roll buffer contains the expected newest samples;
- an integrity signal sent during the wait is acknowledged and published;
- no command is processed before completion.

### Existing meeting tests

Run the full `waved-meeting` test suite to confirm:

- durable artifact and metadata generation;
- Ask/automatic/local-only transitions;
- pending-decision behavior;
- shutdown implicit save;
- pipeline-integrity failure ordering;
- HIL checkpoint feature compilation.

### Device verification

Build and deploy the production binary, then:

1. record a take long enough that finalization performs a meaningful file
   hash;
2. snapshot all audio integrity counters immediately before Stop;
3. Stop and wait for durable finalization plus the completion toast;
4. verify all drop and XRUN counters remain unchanged;
5. verify the WAV, metadata size/hash, and database state;
6. start a second recording and confirm its pre-roll contains audio captured
   during the first take's finalization interval;
7. restore and verify normal production process and Wi-Fi state.

The previous 349 MB artifact may be used for targeted hashing benchmarks, but
acceptance requires a newly recorded device artifact so the live processed
channel is exercised.

## Acceptance criteria

- No `processed_frames_dropped` increase during or after Stop finalization.
- No raw-frame drops or XRUNs introduced by the change.
- Finalized WAV and metadata hashes match.
- Frames received after Stop are retained as the next take’s pre-roll.
- Start remains blocked until durable finalization completes.
- Existing recording, upload-mode, interruption, and shutdown tests pass.
- The production device reconnects to its saved Wi-Fi and runs exactly one
  verified `waved` process after deployment.
