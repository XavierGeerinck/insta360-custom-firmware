# Recording stop and webhook feedback — implementation plan

**Goal:** Acknowledge Stop immediately and show a truthful toast when the same
recording is uploaded, deferred, or requires webhook attention.

**Design:** `docs/superpowers/specs/2026-07-20-recording-upload-toasts-design.md`

## Execution rules

- Keep SQLite and the webhook worker as the durable source of truth.
- Emit transient UI events only after the corresponding database write
  succeeds.
- Never show a backlog upload as the result of the recording just stopped.
- Keep toast correlation bounded and independent of LVGL for host testing.
- Deploy only after meeting/state/UI tests, strict changed-crate Clippy, an
  ARM64 build, and the GLIBC 2.17 check pass.

## Task 1: Add typed recording-upload events

**Files:**

- Modify `crates/waved-state/src/lib.rs`

1. Add succeeded, deferred, and needs-attention `DeviceEvent` variants carrying
   the meeting UUID.
2. Extend state tests to prove the variants round-trip through the broadcast
   bus.
3. Run `cargo test -p waved-state --lib` and strict Clippy.

## Task 2: Emit durable worker outcomes

**Files:**

- Modify `crates/waved-meeting/src/lib.rs`
- Modify `crates/waved-meeting/src/worker.rs`
- Modify or add `waved-meeting` worker tests

1. Pass the existing device-event sender into `UploadWorker`.
2. Emit success only after `mark_uploaded` succeeds.
3. Emit deferred only after `RetryWait` is persisted and attempts remain.
4. Emit needs-attention after a persisted permanent failure or exhausted
   transient retry cycle.
5. Do not emit a success event when the webhook accepts the request but SQLite
   cannot persist `uploaded`.
6. Run meeting unit, recording-decision, and webhook-contract tests plus strict
   Clippy.

## Task 3: Correlate current-session recordings and show toasts

**Files:**

- Modify `crates/waved-ui/src/lib.rs`
- Add `crates/waved-ui/src/upload_notice.rs`

1. Add a pure Rust bounded UUID tracker with capacity 16.
2. On `StopRecording`, capture the active meeting UUID, track it, and show
   `Saving recording…` for 1.5 seconds.
3. Subscribe the UI thread to `DeviceEvent`.
4. Map matching succeeded, deferred, and needs-attention events to the approved
   text.
5. Remove a UUID after success; retain it after deferred or attention outcomes.
6. Ignore unrelated backlog and lagged event notifications.
7. Add host tests for mapping, correlation, removal, retention, and eviction.
8. Run UI/state/meeting tests and strict Clippy for changed crates.

## Task 4: Verify, commit, deploy, and accept

1. Run `git diff --check`.
2. Cross-build `waved` with `ui,capture-alsa` in the CentOS target directory.
3. Strip the ARM64 binary, confirm the ELF architecture and maximum
   `GLIBC_2.17` symbol requirement, and checksum it.
4. Commit the implementation.
5. Back up and replace the live binary through `/tmp`, retaining an immediate
   rollback copy.
6. Stop every existing watchdog by actual PID, start exactly one supervisor,
   and verify UART/ALSA ownership, Wi-Fi, NTP, database state, and checksum.
7. Record a short Automatic-mode meeting:
   - Stop immediately shows `Saving recording…`;
   - the receiver gets exactly one multipart request;
   - SQLite becomes `uploaded`; and
   - the display shows `Recording uploaded`.

**Commit:** `feat(ui): show recording upload toasts`
