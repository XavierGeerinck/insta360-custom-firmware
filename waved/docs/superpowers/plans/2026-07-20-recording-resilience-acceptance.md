# Recording resilience acceptance implementation plan

**Design:** `docs/superpowers/specs/2026-07-20-recording-resilience-acceptance-design.md`

## Task 1: Make retention deterministic and reusable

**Files:**

- Modify `crates/waved-meeting/src/retention.rs`
- Modify `crates/waved-meeting/src/db.rs` if fixture setup needs a focused helper
- Modify `crates/waved-meeting/src/lib.rs`

1. Expose a crate-visible `DiskUsage` value and keep production `statvfs`
   inspection behind `enforce`.
2. Add `enforce_for_usage` so host tests and the device fixture runner execute
   the real pruning loop with injected capacity readings.
3. Re-run production retention immediately before every recording start.
4. Refuse the start when the fresh check leaves `storage_blocked` set.
5. Test threshold boundaries, clearing a prior block, oldest-first uploaded
   pruning, preservation of all protected statuses, and no-candidate blocking.

## Task 2: Pin interrupted reconciliation outcomes

**Files:**

- Modify `crates/waved-meeting/src/reconcile.rs`
- Modify reconciliation integration tests

1. Add fixtures for partial-only, final-WAV-only, and finalized-pair states
   whose database row still says `recording`.
2. Require `artifact_error`, `missing_artifact`, and `local_only`
   respectively.
3. Assert that none of those rows becomes upload-claimable.
4. Run reconciliation twice and assert stable rows and artifacts.
5. Fix any ordering or classification gap exposed by these tests.

## Task 3: Add deterministic finalization checkpoints

**Files:**

- Modify `crates/waved/Cargo.toml`
- Modify `crates/waved/src/main.rs`
- Modify `crates/waved-meeting/Cargo.toml`
- Modify `crates/waved-meeting/src/lib.rs`
- Modify `crates/waved-meeting/src/recorder.rs` or `metadata.rs` only where the
  transaction boundary requires it

1. Extend the existing non-default recording test-hook feature.
2. Add inactive-by-default checkpoints after active partial creation, after
   WAV publication, and after metadata publication but before terminal
   database state.
3. Write a ready marker and pause only when an explicit on-device hook requests
   that checkpoint.
4. Keep Start/Stop control on the existing HIL command path.
5. Ensure production builds contain no checkpoint behavior.
6. Add focused tests proving inactive hooks do not alter finalization.

## Task 4: Add the isolated on-device retention runner

**Files:**

- Add `crates/waved-meeting/src/bin/retention-acceptance.rs`
- Modify `crates/waved-meeting/Cargo.toml`

1. Gate the runner behind the recording acceptance feature.
2. Require an explicit fixture root under a safe temporary prefix.
3. Create its own SQLite database, meeting state channel, and artifact tree.
4. Populate protected and uploaded statuses with deterministic ordering.
5. Exercise percentage-used and minimum-free-space threshold breaches through
   `enforce_for_usage`.
6. Verify oldest-first uploaded deletion, protected row/file survival, and
   final `storage_blocked` state.
7. Emit one concise PASS/FAIL result and preserve the fixture on failure.

## Task 5: Build the recording resilience shell harness

**Files:**

- Add `scripts/test-recording-resilience.sh`

1. Validate required tools, ADB target, expected checksum, single-process
   state, and disruptive-test acknowledgement.
2. Snapshot all real recording directory names before fixture work.
3. Run a configurable recording through the existing HIL Start/Stop actions.
4. Periodically capture PID, RSS, free space, partial size, meeting row, and
   audio integrity counters into a local evidence directory.
5. Pull and validate the finalized WAV and metadata, including PCM properties,
   sample-derived duration, byte size, SHA-256, and absence of a partial.
6. Invoke the isolated retention runner for both thresholds and verify that the
   real recording snapshot is unchanged.
7. Reboot at the active and two finalization checkpoints; wait for ADB,
   watchdog, and application recovery after each reboot.
8. Run reconciliation assertions twice for every interrupted artifact.
9. Restore the production binary and stable watchdog process on every exit
   path where the device is reachable.
10. Print retained evidence paths and the literal one-hour invocation.

## Task 6: Update roadmap and operator documentation

**Files:**

- Modify `TODO.md`
- Modify script comments or adjacent device documentation if required

1. Record deterministic host coverage and the shortened device proof.
2. Leave the P0 endurance item open until the literal one-hour command passes,
   while marking its implemented subchecks precisely.
3. Mark storage-pressure safety complete only after both fixture thresholds
   pass on-device.
4. Document the one-hour command, expected evidence, reboot effects, and
   production restoration check.

## Task 7: Verify, commit, cross-build, and run on-device acceptance

1. Run `git diff --check` and focused formatting on changed Rust files only.
2. Run `waved-meeting`, application, configuration, and affected UI/state
   tests.
3. Run strict clippy on changed crates and separate pre-existing warnings.
4. Cross-build and strip the production ARM64 binary.
5. Cross-build the feature-gated HIL application and retention runner.
6. Verify architecture, checksum, and maximum GLIBC requirement.
7. Commit the implementation before deployment.
8. Deploy the HIL artifacts and run the shortened resilience harness.
9. Preserve and summarize evidence from recording, storage, and reboot phases.
10. Restore the production binary atomically and verify one application
    process, one watchdog, ALSA progress, Wi-Fi, and binary checksum.
