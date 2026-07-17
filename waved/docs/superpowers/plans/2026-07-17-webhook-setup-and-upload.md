# Configurable recording webhooks — implementation plan

**Goal:** Configure Wave from the temporary setup network and reliably stream
finalized meeting WAVs plus versioned JSON metadata to a generic multipart
webhook.

**Design:** `docs/superpowers/specs/2026-07-17-webhook-setup-and-upload-design.md`

## Execution rules

- Implement test-first at each state-machine or parser boundary.
- Keep the existing Wi-Fi provisioning and recording flows usable after every
  commit.
- Never deploy a binary until host tests, strict clippy, ARM64 build, and glibc
  checks pass.
- Back up the live config, database, binary, and recording index before the
  first device migration.
- Do not delete or rewrite legacy recording artifacts during development.

## Task 1: Version and migrate `WavedConfig`

**Files:**

- Modify `config/default.toml`
- Modify `crates/waved-config/Cargo.toml`
- Modify `crates/waved-config/src/lib.rs`
- Add `crates/waved-config/src/migrate.rs`
- Add `crates/waved-config/src/validate.rs`

1. Add `config_version`, `UploadMode`, new API fields, and `StorageConfig`.
2. Give newly introduced fields serde defaults so version-zero files remain
   parseable after migration.
3. Parse into `toml::Value`, migrate version zero to one, then deserialize the
   typed structure.
4. Migrate `auto_upload` and obsolete `audio.mode = "noise_reduction"` exactly
   as specified.
5. Validate URLs, endpoint transport security, strings, enum/range values, NTP
   hosts, and storage settings.
6. Make startup try the last-known-valid backup before visible built-in
   defaults.
7. Add tests for current live TOML, legacy true/false auto-upload, unknown
   future versions, malformed fields, defaults, and backup fallback.
8. Run `cargo test -p waved-config` and strict clippy for the crate.

**Commit:** `feat(config): version and migrate device settings`

## Task 2: Add the typed form registry and atomic save

**Files:**

- Add `crates/waved-config/src/form.rs`
- Add `crates/waved-config/src/persist.rs`
- Modify `crates/waved-config/src/lib.rs`
- Modify `crates/waved-config/Cargo.toml`

1. Define `Section`, `FieldSpec`, `FieldKind`, option metadata, and submitted
   field values.
2. Cover Device, Audio, Webhook, Storage, Security, and NTP fields, including
   the conditional steered-mode azimuth.
3. Exclude only `config_version` as read-only schema metadata.
4. Implement write-only secret semantics: blank keeps, explicit clear removes.
5. Apply decoded values to a typed config and return path-keyed validation
   errors.
6. Write, sync, reparse, chmod `0600`, back up, and atomically replace the live
   TOML.
7. Fault-inject each persistence step and prove the old live file survives.
8. Add a registry-coverage test that fails when a supported typed path lacks
   form metadata.
9. Run config tests and strict clippy.

**Commit:** `feat(config): expose typed setup form schema`

## Task 3: Expand HTTP framing and setup routes

**Files:**

- Modify `crates/waved-provision/Cargo.toml`
- Modify `crates/waved-provision/src/http.rs`
- Add `crates/waved-provision/src/setup_form.rs`

1. Add the `waved-config` dependency.
2. Increase the bounded request body limit to 32 KiB without weakening header
   or connection timeouts.
3. Add explicit routes for:
   - `GET /` — complete setup form;
   - `POST /save` — save settings and reconnect;
   - `POST /save-connect` — save settings and connect to submitted Wi-Fi; and
   - existing captive probe redirects.
4. Render form sections from `FieldSpec`, current typed values, scanned SSIDs,
   and path-keyed errors.
5. Render secrets only as configured/not-configured indicators.
6. Preserve the current responsive visual design and safe-area behavior.
7. Add CSRF/session token fields without reflecting untrusted request data.
8. Test split reads, size bounds, route/method handling, full field decoding,
   lists, explicit secret clear, HTML escaping, error rendering, and the two
   final actions.
9. Run provision/config tests and strict clippy.

**Commit:** `feat(provision): render full device setup form`

## Task 4: Persist setup and coordinate reconnect/restart

**Files:**

- Modify `crates/waved-provision/src/lib.rs`
- Modify `crates/waved-state/src/lib.rs`
- Modify `crates/waved/src/main.rs`
- Modify `crates/waved-provision` tests

1. Generate a cryptographically random per-session CSRF token alongside the AP
   passphrase.
2. Keep the AP and form alive when validation or persistence fails.
3. On Save, atomically persist settings before tearing down the AP.
4. For reconnect-only, restore station processes and wait for an enabled saved
   profile without requiring a plaintext password.
5. For save-and-connect, reuse the existing secure `wpa::connect` path.
6. Add durable apply/test marker handling.
7. Reject setup start/save while a meeting is actively recording.
8. Add a clean restart command/coordinator: complete AP cleanup and station
   reconnection, shut down services, exit successfully, and let the watchdog
   start the new process.
9. Clear the marker only after the restarted process consumes it.
10. Test invalid-save retention, cancel cleanup, reconnect-only,
    save-and-connect, marker recovery, and restart ordering.

**Commit:** `feat(provision): apply settings across controlled restart`

## Task 5: Migrate meeting persistence to explicit lifecycle states

**Files:**

- Modify `crates/waved-meeting/src/db.rs`
- Add `crates/waved-meeting/src/model.rs`
- Modify `crates/waved-meeting/src/lib.rs`

1. Introduce `PRAGMA user_version` migrations without losing current rows.
2. Add typed `MeetingStatus` values from the approved design.
3. Add persisted attempt-cycle count, last error/status code, and the fields
   needed for retry/test/status reporting.
4. Insert new meetings as `recording`, not `pending`.
5. Increment attempt counters only when an HTTP attempt begins.
6. Add atomic claim/update queries so only one worker can upload a meeting.
7. Return deterministic rowid-ordered eligible work.
8. Test migration from the exact current schema and all allowed transitions.

**Commit:** `feat(meeting): persist explicit recording lifecycle`

## Task 6: Finalize durable WAV and complete metadata

**Files:**

- Modify `crates/waved-meeting/Cargo.toml`
- Modify `crates/waved-meeting/src/recorder.rs`
- Add `crates/waved-meeting/src/metadata.rs`
- Modify `crates/waved-meeting/src/db.rs`
- Modify `crates/waved-meeting/src/lib.rs`

1. Record S16 mono 48 kHz audio into `audio.wav.partial`.
2. Track exact live and pre-roll sample counts.
3. Finalize and sync the WAV, then atomically rename to `audio.wav`.
4. Query bookmarks and speaker segments from SQLite.
5. Obtain a stable non-empty device ID and software version.
6. Stream SHA-256 calculation and build the version-one nested metadata.
7. Use `started_at = null` with `clock_synchronized = false` until trustworthy
   time exists.
8. Atomically write and sync `metadata.json`.
9. Only then transition to `queued`, `awaiting_decision`, or `local_only`
   according to `upload_mode`.
10. Update automatic mode to skip confirmation, ask mode to preserve it, and
    local-only mode to retain immediately.
11. Test file names, S16 WAV headers, sample-derived durations, short
    prebuffers, metadata content, finalization failure, and mode transitions.

**Commit:** `feat(meeting): finalize durable webhook artifacts`

## Task 7: Reconcile boot state and legacy recordings

**Files:**

- Add `crates/waved-meeting/src/reconcile.rs`
- Modify `crates/waved-meeting/src/lib.rs`
- Modify `crates/waved-meeting/src/db.rs`

1. Inspect each database row and recording directory before starting uploads.
2. Requeue interrupted `uploading` rows only when both final artifacts exist.
3. Mark missing artifacts and Opus-only legacy rows without retrying them.
4. Import orphan finalized WAV/metadata pairs conservatively as local-only.
5. Preserve `.partial` recordings and report them as incomplete.
6. Make reconciliation idempotent and deletion-free.
7. Build a fixture mirroring the live mix of 47 rows, legacy Opus, WAV, and
   missing paths; verify no retry storm remains.

**Commit:** `fix(meeting): reconcile legacy and interrupted artifacts`

## Task 8: Stream the generic multipart webhook

**Files:**

- Modify `crates/waved-meeting/Cargo.toml`
- Replace `crates/waved-meeting/src/upload.rs`
- Add `crates/waved-meeting/tests/webhook_contract.rs`

1. Reuse one configured `reqwest::Client` with redirects disabled.
2. Stream the WAV with a known content length and bounded buffers.
3. Send `meeting_id`, `metadata`, and `audio` with the documented filenames and
   content types.
4. Add Bearer, idempotency, device ID, and user-agent headers.
5. Omit Authorization when the token is empty.
6. Classify success, transient, and configuration failures precisely.
7. Bound and sanitize response diagnostic bodies.
8. Implement the small valid silent-WAV `webhook.test` artifact.
9. Use a local mock receiver to assert the exact multipart and headers,
   redirects, error classes, duplicate keys, and test event.
10. Add a large sparse/fixture WAV test proving upload code does not call a
    whole-file read.

**Commit:** `feat(webhook): stream generic recording multipart`

## Task 9: Add the durable upload worker and retry policy

**Files:**

- Add `crates/waved-meeting/src/worker.rs`
- Modify `crates/waved-meeting/src/lib.rs`
- Modify `crates/waved-meeting/src/db.rs`
- Modify `crates/waved-state/src/lib.rs`
- Modify `crates/waved/src/main.rs`

1. Move all HTTP delivery out of the meeting command loop.
2. Wake the worker for finalization, Wi-Fi reconnection, retry deadlines,
   relevant configuration changes, manual retry, and pending setup test.
3. Process one claimed recording at a time in deterministic order.
4. Add capped exponential backoff with bounded jitter and attempt cycles.
5. Keep configuration failures visible until endpoint/security settings change
   or the user retries them.
6. Reuse meeting UUIDs as idempotency keys on every attempt.
7. Publish queued/uploading/failed counts plus last test/success state.
8. Test simultaneous wakeups, disconnect during upload, retry cap, reboot
   recovery, endpoint changes, manual retry, and no command-loop blocking.

**Commit:** `feat(webhook): run durable background upload worker`

## Task 10: Implement safe uploaded-recording pruning

**Files:**

- Add `crates/waved-meeting/src/retention.rs`
- Modify `crates/waved-meeting/src/db.rs`
- Modify `crates/waved-meeting/src/lib.rs`
- Modify `crates/waved-state/src/lib.rs`

1. Read filesystem used percentage and free bytes for the recording volume.
2. Trigger after upload, finalization, and boot reconciliation.
3. Select oldest uploaded rows by insertion order.
4. Delete each artifact and its relational data consistently, then recheck
   both thresholds.
5. Never select unknown, unsent, failed, local-only, active, or incomplete
   artifacts.
6. Publish a storage-blocked state when pressure remains and no safe candidate
   exists; reject new recording starts without affecting existing data.
7. Test threshold boundaries, deletion order, partial deletion failures, no
   candidate, and recovery after space becomes available.

**Commit:** `feat(meeting): prune only delivered recordings`

## Task 11: Replace the broken external NTP invocation

**Files:**

- Add `crates/waved-wifi/src/ntp.rs`
- Modify `crates/waved-wifi/src/lib.rs`
- Modify `crates/waved-wifi/Cargo.toml`
- Modify `crates/waved-state/src/lib.rs`

1. Implement a bounded SNTP request/response path or use a verified available
   device primitive; do not call the absent `ntpd`.
2. Validate packet mode, origin timestamp, stratum, and response size.
3. Apply time only when running with the required device privilege.
4. Publish whether the clock is trustworthy for meeting metadata and UI.
5. Fail over through configured servers with timeouts.
6. Unit-test packet parsing and invalid responses; integration-test against a
   controlled server where practical.

**Commit:** `fix(wifi): synchronize meeting timestamps without ntpd`

## Task 12: Show apply, webhook, and storage status on-device

**Files:**

- Modify `crates/waved-state/src/lib.rs`
- Modify `crates/waved-ui/src/nav.rs`
- Modify `crates/waved-ui/src/lib.rs`
- Modify `crates/waved-ui/src/screens/mod.rs`
- Modify `crates/waved-ui/src/screens/system.rs`
- Modify `crates/waved-ui/src/screens/wifi_provisioning.rs`
- Add `crates/waved-ui/src/screens/webhook.rs`
- Modify `crates/waved-ui/src/i18n.rs`

1. Extend provisioning rendering with Saving, Reconnecting, Restarting, and
   Testing Webhook states.
2. Add a System → Webhook screen showing endpoint host, auth configured,
   upload mode, last test, queue counts, and last success.
3. Add Retry Failed without exposing endpoint credentials.
4. Surface configuration faults and storage-full recording blocks.
5. Ensure circular-screen controls remain reachable and avoid per-tick widget
   rebuilds.
6. Test pure render models, navigation/commands, status sanitization, and
   connected/disconnected transitions.

**Commit:** `feat(ui): show webhook delivery and setup status`

## Task 13: Add integration tooling and documentation

**Files:**

- Add `scripts/test-webhook-setup.sh`
- Add a bounded local test receiver under `crates/` or `scripts/`
- Modify `scripts/test-wifi-provisioning.sh`
- Add `docs/WEBHOOKS.md`
- Modify `docs/WIFI.md`
- Modify `TODO.md`

1. Run a host receiver that records request headers, multipart metadata, and
   streamed WAV properties without retaining secrets.
2. Extend provisioning acceptance for configuration fields, secrets, both save
   actions, restart, and test result.
3. Add end-to-end automatic, ask, and local-only recording checks.
4. Exercise transient retry, permanent `400`, redirect rejection, endpoint
   change, Wi-Fi loss, restart during upload, and duplicate idempotency.
5. Exercise reconciliation with copied fixture data and retention under an
   isolated constrained filesystem.
6. Document the receiver contract, examples, authentication, retry semantics,
   metadata schema, setup flow, and insecure-HTTP warning.

**Commit:** `test(webhook): cover setup and delivery end to end`

## Task 14: Full verification and device deployment

**Files:**

- Modify only issues found by verification.

1. Run formatting, `git diff --check`, relevant crate tests, workspace tests
   where feature compatibility permits, and strict clippy for every changed
   backend crate.
2. Run simulated UI/app tests and build.
3. Cross-build ARM64 release with `ui,capture-alsa`.
4. Verify ELF architecture and maximum required glibc version `<= 2.31`.
5. Back up:
   - `/data/waved/waved`;
   - `/data/waved/config.toml`;
   - `/data/waved/meetings.db`; and
   - a recording-directory inventory under `/mnt/udisk/debug`.
6. Install the binary atomically and reboot through the watchdog.
7. Confirm configuration migration, mode `0600`, normal boot display, audio,
   Wi-Fi auto-connect, and no stale upload storm.
8. Complete the device smoke sequence from the design using an HTTPS receiver.
9. Record a long-enough fixture to compare RSS during recording and upload.
10. Force reboot/disconnect/failure/retention scenarios and verify no unsent
    recording is deleted.
11. Preserve final logs, receiver evidence, hashes, and rollback artifacts
    under `/mnt/udisk/debug`.

**Commit:** `docs(webhook): record verified device workflow`

## Rollback

Before deployment, preserve the verified binary, config, database, and
recording inventory. If migration, UI, recording, or connectivity fails:

1. stop the watchdog-launched `waved`;
2. restore the previous binary and configuration atomically;
3. restore the database only if its migration did not complete safely;
4. leave all recording directories untouched;
5. reboot; and
6. verify display, Wi-Fi, audio, and current recording access before further
   changes.

