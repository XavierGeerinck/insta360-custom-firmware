# Recording pipeline integrity implementation plan

**Design:** `docs/superpowers/specs/2026-07-20-recording-pipeline-integrity-design.md`

## Task 1: Add the shared integrity model

**Files:**

- Add `crates/waved-audio/src/integrity.rs`
- Modify `crates/waved-audio/src/lib.rs`
- Modify `crates/waved-state/src/lib.rs`

1. Add the typed recording fault and capture/audio exit enums.
2. Add `RecordingIntegrity` with atomic epoch, last fault, and drop/xrun
   counters.
3. Add `IntegritySignal` with an optional one-shot acknowledgement.
4. Extend `AudioState`, `MeetingState`, and `DeviceEvent` with sanitized
   integrity state.
5. Test epoch ordering, counters, fixed messages, defaults, and event UUID
   round-trips.

## Task 2: Make capture exits and losses explicit

**Files:**

- Modify `crates/waved-audio/src/capture/thread.rs`
- Modify capture-thread callers and tests

1. Return `CaptureExit` from `run_capture_loop`.
2. Distinguish normal stop, EOF, raw-channel closure, and fatal read exit.
3. Record and signal recovered xruns and raw-channel drops before continuing.
4. Preserve existing rate-limited diagnostics.
5. Test exact exits, counter changes, and signal-before-exit behavior.

## Task 3: Supervise the DSP-to-meeting boundary

**Files:**

- Modify `crates/waved-audio/src/lib.rs`
- Modify audio-service tests

1. Remove the retained raw sender from `AudioService`.
2. Return a typed `AudioServiceExit` on normal shutdown or channel closure.
3. Handle processed meeting-channel `Full` as a counted integrity fault.
4. Handle processed meeting-channel `Closed` as a fatal service exit.
5. Update constructors and tests without changing UAC/stats behavior.

## Task 4: Make recording writes fallible and abortable

**Files:**

- Modify `crates/waved-meeting/src/recorder.rs`
- Modify `crates/waved-meeting/src/db.rs`
- Modify recorder/database tests

1. Snapshot the shared integrity epoch at recording start.
2. Return results from pre-roll and live sample appends.
3. Add bounded writer-failure injection for tests.
4. Add explicit best-effort `abort()` that preserves only
   `audio.wav.partial`.
5. Add a database helper that persists `artifact_error`, end time,
   sample-derived duration, and a sanitized `last_error`.
6. Test writer/pre-roll failure, partial preservation, and durable diagnostics.

## Task 5: Serialize faults in `MeetingService`

**Files:**

- Modify `crates/waved-meeting/src/lib.rs`
- Modify `crates/waved-meeting/src/reconcile.rs` if verification exposes a gap
- Modify meeting integration tests

1. Receive integrity signals in the meeting actor.
2. Abort an active take through one idempotent helper.
3. Acknowledge fatal faults only after persistence/abort completes.
4. Drain immediately queued frames/signals and compare epochs before Stop
   finalization.
5. Block new recordings only after storage/write failure.
6. Emit the typed UI event and update continuous state.
7. Pin Stop/fault races, upload exclusion, repeated signals, and idle-fault
   behavior.

## Task 6: Add the unified fatal-pipeline supervisor

**Files:**

- Modify `crates/waved/src/main.rs`
- Add focused helper tests where practical

1. Retain capture-thread and audio-task terminal results.
2. Route the first unexpected terminal result through the meeting
   acknowledgement path.
3. Wait at most two seconds, then notify the top-level lifecycle.
4. Reuse normal shutdown sequencing so the watchdog performs the restart.
5. Ignore normal capture/audio exits during requested shutdown.
6. Test acknowledgement success, timeout, and duplicate fatal exits.

## Task 7: Surface safe on-device feedback

**Files:**

- Modify `crates/waved-ui/src/lib.rs`
- Add or modify a pure UI notice helper
- Modify `crates/waved-ui/src/screens/webhook.rs`

1. Map active fault, idle restart, and blocked-start states to approved ASCII
   copy.
2. Show only fixed sanitized reasons.
3. Include the persistent recording block in status rendering.
4. Test exact copy, UUID correlation, unrelated events, and sanitization.

## Task 8: Add non-production HIL fault injection

**Files:**

- Modify relevant `Cargo.toml` feature declarations
- Modify `crates/waved/src/main.rs`
- Add `scripts/test-recording-integrity.sh`

1. Add non-default `recording-integrity-test-hooks` feature propagation.
2. Accept typed fault injection only in that build.
3. Cover idle/active capture failure, frame drop, and writer failure.
4. Make the script back up state, clean every hook, and refuse destructive
   storage operations.

## Task 9: Verify, commit, build, and deploy

1. Run formatting and `git diff --check`.
2. Run focused state, audio, meeting, UI, and application tests.
3. Run strict clippy on changed backend crates; identify pre-existing UI
   warnings separately.
4. Cross-build and strip the normal ARM64 `ui,capture-alsa` binary.
5. Verify architecture, checksum, and maximum GLIBC requirement.
6. Commit the implementation.
7. Deploy atomically with the current device binary retained in `/tmp`.
8. Confirm one watchdog/app process, UART, ALSA, Wi-Fi, database, and receiver.
9. Run a clean recording/upload smoke test.
10. Build/deploy the HIL variant only for fault acceptance, then restore the
    normal production binary and re-run the clean smoke test.
