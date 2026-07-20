# Non-blocking recording finalization implementation plan

**Design:** `docs/superpowers/specs/2026-07-20-nonblocking-recording-finalization-design.md`

## Task 1: Add the deterministic drain regression

**Files:**

- Modify `crates/waved-meeting/src/lib.rs`

1. Add a test that constructs a `MeetingService` with a deliberately tiny
   processed-audio channel.
2. Hold a synthetic finalization future pending while a producer sends more
   frames than the channel can hold.
3. Require the producer to receive an integrity-signal acknowledgement before
   allowing finalization to complete.
4. Assert the operation finishes under a timeout, the idle fault is
   published, and every expected sample reaches the pre-roll.
5. Run the focused test first and retain its initial compile/failure result as
   proof that production behavior is not yet present.

## Task 2: Isolate durable finalization from the meeting actor

**Files:**

- Modify `crates/waved-meeting/src/lib.rs`

1. Introduce an owned finalization context containing only cloned
   configuration, database, state/event sender, integrity, device, recording,
   and retention dependencies.
2. Move the existing durable `stop_recording` sequence onto that context
   without changing artifact ordering, database statuses, events, or UI
   state.
3. Run `ActiveRecording::finalize` through `tokio::task::spawn_blocking`.
4. Map both filesystem and blocking-task join failures onto the existing
   fail-closed WAV-finalization path.
5. Keep all feature-gated interruption checkpoints at their current durable
   boundaries.

## Task 3: Drain the live pipeline while finalization is pending

**Files:**

- Modify `crates/waved-meeting/src/lib.rs`

1. Add a focused `MeetingService` helper that pins an owned finalization
   future.
2. Use a biased `tokio::select!` to prioritize completion while receiving
   processed audio, integrity signals, and DoA frames.
3. Push post-Stop processed frames into the cleared and resumed pre-roll.
4. Publish and acknowledge integrity signals as idle faults.
5. Drain/discard DoA frames, preserving lag/closed diagnostics and preventing
   closed-channel busy loops inside the helper.
6. Do not receive commands in the helper, preserving serialized Stop/Start
   behavior.
7. Use the helper for both normal Stop and active-recording Shutdown.

## Task 4: Verify backend behavior

1. Run the new focused regression.
2. Run `cargo fmt --check` and `git diff --check`.
3. Run all `waved-meeting` tests with default features.
4. Run meeting tests with `recording-integrity-test-hooks`.
5. Run strict clippy for `waved-meeting`.
6. Run the broader workspace tests needed by changed public behavior.
7. Inspect the diff for artifact-state, shutdown, and checkpoint regressions.

## Task 5: Commit, cross-build, and deploy

1. Commit the implementation independently from this plan.
2. Cross-build the normal ARM64 production binary with the existing
   `ui,capture-alsa` feature set.
3. Strip and inspect architecture, GLIBC requirements, and checksum.
4. Deploy atomically while retaining the current device binary as a temporary
   rollback artifact.
5. Confirm one production `waved` process, saved Wi-Fi association, DHCP, and
   database/recording paths.

## Task 6: Run device acceptance

1. Deploy the HIL build only if integrity counters are not available in the
   production build.
2. Record enough audio to exercise non-trivial finalization.
3. Snapshot integrity counters immediately before Stop.
4. Stop and wait for durable WAV, metadata, database state, and completion
   toast.
5. Assert no counter increases through finalization.
6. Validate WAV format, size, metadata checksum, and database status.
7. Start a second recording after completion and validate that its pre-roll
   includes the finalization interval.
8. Restore the verified production binary and re-check process and Wi-Fi
   state.
