# Waved Recording — Save / Send / Discard Confirmation Design

**Date:** 2026-04-30
**Depends on:** WAV recording (commit `9f4d89e`, 2026-04-30) — `MeetingService` writes a finalisable `audio.wav` per meeting and the recording screen exists.
**Status:** spec — implementation plan to be created via `writing-plans`.

---

## 1. Context

Today, tapping **Stop** on the recording screen sends `Command::StopRecording`, which causes `MeetingService::stop_recording` to immediately finalise the WAV, persist the meeting row, and — if `config.api.auto_upload && wifi.connected` — auto-upload to `config.api.endpoint`. The user has no chance to review or to *not* upload a take. There is no concept of a local-only save, no way to discard a recording the user didn't want, and the configured endpoint URL is invisible from the device — the user has to trust that the file went to the right place.

This design adds a confirmation step between Stop and the recording's terminal disposition. After Stop, the file is finalised but the user must explicitly choose what happens to it: keep locally, send to the configured endpoint (which also keeps locally), or discard.

The local path and endpoint URL are surfaced on the confirmation screen so the user can verify the destination before committing.

## 2. Decisions (locked in brainstorming)

1. **Three actions, always shown.** Save / Send / Discard. The `[api].auto_upload` config flag is retired — there is no silent-upload-on-stop path any more. Send is the explicit "queue for upload" action, Save is local-only, Discard is hard-delete.

2. **Stop finalises the WAV before showing the confirmation.** The recording loop closes the writer (`hound::WavWriter::finalize`), persists `end_time` + `duration_secs` to the db, and *then* publishes a new `MeetingState.pending_decision` field that drives the UI. Finalising up front means the file is readable for any future preview/playback feature and keeps the recorder loop's responsibilities clean (one place that closes the writer).

3. **Send is fire-and-forget.** Reuses the existing `try_upload_with_retry` machinery (`retry_max=5`, `retry_backoff_ms=1000`) and the `wifi_rx.changed()` reconnect-flush. The confirmation screen exits to Home immediately on tap. `MeetingState.pending_uploads` and the existing failure-toast surface upload state to the user.

4. **Discard is hard-delete.** The db row, all bookmarks for it, and the entire `<recordings_dir>/<uuid>/` directory are removed. No "discarded" tombstone — Discard means "this didn't happen". A future recordings-list feature would not benefit from preserved discard rows; if the user wants history they should pick Save.

5. **Layout: stacked text + button row at the bottom.** Three 64×64 icon buttons in a row above the bottom edge, with title + duration/size + path + endpoint stacked above. Mirrors the existing recording-screen pattern (90×90 stop, 76×76 mic, both `BOTTOM_MID`). Three-button row needs slightly smaller buttons (64) and a 16 px gap to fit inside the 380 px circular safe area.

6. **Modal semantics.** While `pending_decision.is_some()`, navigation buttons (Home, More, Settings shortcuts) are inert; only Save/Send/Discard close the screen. New `Command::StartRecording` is rejected — the previous take must be resolved first. `Command::Shutdown` is treated as implicit Save (preserve user data).

7. **New `upload_status = 'local_only'`.** Existing `get_pending_uploads` query already filters to `'pending' OR 'upload_failed'`, so `'local_only'` is naturally excluded from the wifi-flush.

8. **Path/endpoint truncation in the UI, not in the data model.** `MeetingState.pending_decision` carries the absolute `audio_path` and the full `endpoint` URL. The confirmation screen formats them on render — this keeps the formatting policy testable and out of the recorder.

## 3. Scope

**In scope:**

- New `Command` variants: `SaveRecording(Uuid)`, `SendRecording(Uuid)`, `DiscardRecording(Uuid)`.
- New `MeetingState.pending_decision: Option<PendingDecision>` carrying id, audio_path, duration_secs, size_bytes, endpoint.
- `MeetingService::stop_recording` rewritten to populate `pending_decision` instead of auto-uploading; new handlers for the three new commands.
- New `db::delete_meeting_cascade` (single transaction over `bookmarks` + `meetings`).
- Removal of `[api].auto_upload` from `WavedConfig` and `default.toml`. Old configs with the field accepted via `#[serde(default)]` for round-trip safety.
- New `crates/waved-ui/src/screens/confirmation.rs` plus `UiScreen::Confirmation` in `nav.rs`.
- Auto-navigation `Home/Recording ↔ Confirmation` driven by `pending_decision`.
- Three new icon assets (`btn_save.png`, `btn_send.png`, `btn_discard.png`, 36×36 white-on-tint to match the existing `bg_airecord_*` family).
- Path and endpoint formatter helpers with unit tests.

**Out of scope:**

- A "recordings list" / browsing UI for previously saved takes. Save persists the file and metadata so a future feature can read them, but the listing UI itself is a separate workstream.
- Boot-time janitor that reconciles stray dirs in `<recordings_dir>` against the db. Adds resilience after a partial Discard failure but is not gating release.
- Playback / preview on the confirmation screen.
- Upload-progress UI. We rely on the existing `pending_uploads` counter and any toast/notification surface that already exists.
- Migration of existing in-flight pending uploads on upgrade — there is no install base of this firmware.

## 4. Non-goals

- We are *not* preserving the legacy "auto-upload on stop" behaviour behind a flag. Removing it is intentional: silent uploads are exactly the telemetry behaviour this firmware is meant to replace.
- We are *not* introducing a new "soft-delete" / trash bin. Discard means hard-delete.

## 5. Data model changes

### 5.1 `waved-state::MeetingState` + `Command`

```rust
// crates/waved-state/src/lib.rs

pub struct MeetingState {
    pub recording: bool,
    pub paused: bool,
    pub meeting_id: Option<Uuid>,
    pub duration_secs: u32,
    pub pending_uploads: u32,
    pub failed_uploads: u32,
    pub pending_decision: Option<PendingDecision>,   // NEW
}

#[derive(Clone, Debug)]
pub struct PendingDecision {
    pub id: Uuid,
    pub audio_path: PathBuf,
    pub duration_secs: u32,
    pub size_bytes: u64,
    /// Snapshot of `config.api.endpoint` at the moment of stop, so a
    /// concurrent config edit can't change what the user is shown.
    pub endpoint: String,
}

pub enum Command {
    // ... existing variants ...
    SaveRecording(Uuid),
    SendRecording(Uuid),
    DiscardRecording(Uuid),
}
```

`StopRecording` keeps its current variant — it still means "stop the take". Its handler changes; the variant doesn't.

### 5.2 `WavedConfig`

```rust
// crates/waved-config/src/lib.rs
pub struct ApiConfig {
    pub endpoint: String,
    pub auth_token: String,
    pub retry_max: u32,
    pub retry_backoff_ms: u64,
}
```

`auto_upload` removed. `#[serde(default)]` on the field is *not* added; instead the `ApiConfig` struct is left strict and any old config containing the field will trip the existing `reject_invalid_toml` path. Since old config is `~/.waved/config.toml` on a developer device or `/etc/waved/config.toml` on hardware, the upgrade path is "delete the line manually" — acceptable for open-source firmware where the user owns the device. **(Re-evaluated during self-review — see §9.)**

### 5.3 `default.toml`

```toml
[api]
endpoint = "https://localhost:8080/meetings/upload"
auth_token = ""
retry_max = 5
retry_backoff_ms = 1000
```

### 5.4 db schema (no migration needed)

`upload_status` is a TEXT column with no CHECK constraint. New value `'local_only'` is added by code only; existing rows keep their values. `get_pending_uploads` is unchanged — its `WHERE upload_status = 'pending' OR upload_status = 'upload_failed'` filter naturally excludes `'local_only'` and `'uploaded'`.

New helper:

```rust
// crates/waved-meeting/src/db.rs
pub async fn delete_meeting_cascade(pool: &SqlitePool, id: &Uuid) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM bookmarks WHERE meeting_id = ?")
        .bind(id.to_string()).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM meetings WHERE id = ?")
        .bind(id.to_string()).execute(&mut *tx).await?;
    tx.commit().await
}
```

## 6. Behaviour

### 6.1 Stop

`MeetingService::run`'s `Command::StopRecording` arm:

1. `rec.finalize()` — close WAV header. Required before `metadata().len()` returns the right size.
2. `db::finish_meeting(&pool, &id, duration).await`.
3. `events_tx.send(DeviceEvent::RecordingStopped(id))`.
4. Compute `size_bytes` via `tokio::fs::metadata(&audio_path).await.map(|m| m.len()).unwrap_or(0)`.
5. `meeting_tx.send_modify(|s| {
       s.recording = false;
       s.paused = false;
       s.meeting_id = None;
       s.duration_secs = duration;
       s.pending_decision = Some(PendingDecision { id, audio_path, duration_secs: duration, size_bytes, endpoint: self.config.api.endpoint.clone() });
   })`.
6. Drop `active`. *No upload attempted.*

### 6.2 Save

`Command::SaveRecording(id)` handler:

1. Read `meeting_tx.borrow().pending_decision`. If absent, or the carried id mismatches, warn-log and ignore.
2. `db::set_upload_status(&pool, &id, "local_only").await`.
3. `meeting_tx.send_modify(|s| s.pending_decision = None)`.

### 6.3 Send

`Command::SendRecording(id)` handler:

1. Validate id (as Save).
2. `db::set_upload_status(&pool, &id, "pending").await` (defensive — should already be 'pending' from `insert_meeting`).
3. If `wifi_rx.borrow().connected`, spawn `try_upload_with_retry(&id.to_string(), &audio_path).await` *without awaiting* (so the screen returns immediately). If offline, leave the row 'pending' — the existing `wifi_rx.changed()` reconnect-flush will pick it up.
4. `meeting_tx.send_modify(|s| s.pending_decision = None)`.

### 6.4 Discard

`Command::DiscardRecording(id)` handler:

1. Validate id.
2. `db::delete_meeting_cascade(&pool, &id).await`. On error, warn-log and continue — leaving a stray dir is preferable to leaving a half-deleted db.
3. `tokio::fs::remove_dir_all(&recording_dir).await`. NotFound is ignored. Other errors warn-logged.
4. `meeting_tx.send_modify(|s| s.pending_decision = None)`.

### 6.5 Shutdown while pending

`Command::Shutdown` handler, modified:

1. If `meeting_tx.borrow().pending_decision.is_some()`, treat as implicit Save: `set_upload_status(id, 'local_only')`, clear `pending_decision`. *Never auto-discard.*
2. Existing logic for stopping an active recording stays.
3. Return.

### 6.6 New StartRecording while pending

`Command::StartRecording` handler, gated:

```rust
if meeting_tx.borrow().pending_decision.is_some() {
    warn!("ignoring StartRecording: previous take awaits decision");
    continue;
}
```

## 7. UI

### 7.1 Screen and navigation

New `crates/waved-ui/src/screens/confirmation.rs` implementing the existing `Screen` trait. New `UiScreen::Confirmation` in `nav.rs`.

The main UI loop in `lib.rs` already has an auto-navigate block for Home ↔ Recording driven by `meeting.recording`. Extend it:

```rust
let snap = meeting_rx.borrow().clone();
let pending = snap.pending_decision.is_some();
let recording = snap.recording;

match (nav.current.clone(), recording, pending) {
    (UiScreen::Recording, false, true) | (UiScreen::Home, _, true) => {
        // pending_decision appeared → show Confirmation
        confirmation.set_decision(snap.pending_decision.clone().unwrap());
        let root = confirmation.create();
        unsafe { lv_screen_load(root); }
        nav.navigate_to(UiScreen::Confirmation);
        nav.set_root(root);
    }
    (UiScreen::Confirmation, _, false) => {
        // user resolved → back to Home
        let root = home.create();
        unsafe { lv_screen_load(root); }
        nav.navigate_to(UiScreen::Home);
        nav.set_root(root);
    }
    // existing Home ↔ Recording branches unchanged
    _ => {}
}
```

The `confirmation.set_decision(...)` call caches the `PendingDecision` on the screen struct so button callbacks emit the right uuid.

### 7.2 Layout (1:1 with mockup A; 400×304 frame, circle r=190 at (200,152))

```
y=38   "SAVE RECORDING"                       14 px CLR_WAVE_GREEN     (TOP_MID, y=38)
y=78   "duration 02:14 · 12.8 MB"             11 px CLR_TEXT_DIM       (TOP_MID, y=78)
y=110  "📁 4f3a8e2c…/audio.wav"               11 px CLR_TEXT_DIM       (TOP_MID, y=110)
y=132  "🌐 localhost:8080/meetings/upload"    11 px CLR_TEXT_DIM       (TOP_MID, y=132)
y=188  [💾]   [📤]   [🗑]                     three 64×64 buttons      (BOTTOM_MID, y_offset=-28, x_offsets=-80/0/+80)
```

Total horizontal span of the button row: 64×3 + 16×2 = 224 px, comfortably inside the 380 px circular chord at y≈252.

Button visuals: each is an `lv_image` with `bg_airecord_stop.png` as the tinted background (current stop button uses this). Foreground icon is centred with `lv_image` over the bg, sourcing one of the new PNGs (§7.3). Click event sends the corresponding `Command::*Recording(id)` with `id` from the cached `PendingDecision`.

### 7.3 Icon assets

New monochrome PNGs at `crates/waved-ui/assets/icon/btn/`:

- `btn_save.png` — floppy / down-arrow into tray, 36×36, white on transparent.
- `btn_send.png` — paper plane, 36×36, white on transparent.
- `btn_discard.png` — trash can, 36×36, white on transparent.

Asset deploy path follows the existing convention used by the recording screen (`b"A:/etc/pspapp/lvgl/icon/btn/btn_save.png\0"`). Asset bundle is built into the binary via the existing `build.rs` mechanism in `waved-ui`.

### 7.4 Formatters

```rust
// crates/waved-ui/src/screens/confirmation.rs

/// Truncate the immediate parent dir name to 8 chars + ellipsis, keep the filename.
/// /mnt/udisk/recordings/4f3a8e2c-1234-…/audio.wav → "4f3a8e2c…/audio.wav"
pub fn format_path(audio_path: &Path) -> String { ... }

/// Strip http(s):// scheme; if remaining length > 28 chars, ellipsize the
/// path part keeping host:port and the last path segment.
/// "https://localhost:8080/meetings/upload" → "localhost:8080/meetings/upload"
/// "https://meetings.example.com/api/v2/very/long/path/upload" → "meetings.example.com/…/upload"
pub fn format_endpoint(url: &str) -> String { ... }
```

Both are pure functions, unit-tested.

### 7.5 Pill (Dynamic Island)

Hidden during `UiScreen::Confirmation`. The recording pill in the status bar is driven by `meeting.recording`; once stop is processed `recording == false`, so the pill naturally disappears. No code change needed beyond confirming this in the smoke test.

## 8. Testing

**`waved-config`** — `default.toml` updated; the existing `default_config_parses` test continues to pass. New test: `legacy_config_with_auto_upload_field_rejected` (asserts the strict-parsing decision in §5.2). If self-review (§9) flips this to `serde(default)`, replace with `legacy_config_with_auto_upload_field_accepted`.

**`waved-meeting/src/db.rs`** — `delete_meeting_cascade_removes_meeting_and_bookmarks`: insert a meeting + 2 bookmarks, run cascade, assert both tables empty. Use existing tempfile harness.

**`waved-meeting/src/lib.rs`** integration tests against the `simulate` feature path:

1. `stop_recording_populates_pending_decision` — start, drain N frames, send Stop. Assert `pending_decision.is_some()` with right id, the audio file exists at `audio_path`, db row `end_time` set and `upload_status='pending'`, no upload was attempted.
2. `save_clears_pending_and_marks_local_only` — after stop, send `SaveRecording(id)`, assert `pending_decision` cleared, db row `'local_only'`, file present, no upload was attempted even after a `wifi_changed(connected=true)` event.
3. `send_queues_upload_when_online` — after stop, send `SendRecording(id)` with simulated wifi connected, assert `simulate::upload_recording` was called with the right path + endpoint, db row eventually `'uploaded'`.
4. `send_defers_when_offline_and_flushes_on_reconnect` — after stop, send `SendRecording(id)` with wifi disconnected, assert no upload attempt, then flip wifi connected, assert upload runs and row goes `'uploaded'`.
5. `discard_removes_file_and_db_row` — after stop, send `DiscardRecording(id)`, assert dir gone, meeting + bookmark rows gone, `pending_decision` cleared.
6. `shutdown_during_pending_decision_implicit_saves` — start, stop, send Shutdown, assert `'local_only'`, file preserved, service exits.
7. `start_during_pending_decision_is_ignored` — after stop, send `StartRecording`, assert no new recording started and `pending_decision` unchanged.

**`waved-ui/src/screens/confirmation.rs`** — pure formatter unit tests:

- `format_path_short_uuid_truncates_parent_dir`
- `format_path_short_filename_kept_intact`
- `format_endpoint_strips_scheme`
- `format_endpoint_ellipsises_long_path`
- `format_endpoint_short_url_unchanged`

**No new on-device script.** Manual smoke pass after deploy:
- Start → Stop → confirmation appears with correct path/endpoint/duration/size.
- Tap Discard → file dir + db row gone (verify via `ls /mnt/udisk/recordings` + `sqlite3 /data/waved/meetings.db 'SELECT id FROM meetings'`).
- Tap Save → file present, db row `'local_only'`.
- Tap Send → file present, db row eventually `'uploaded'` (or `'upload_failed'` if the configured endpoint is unreachable, then `'uploaded'` after the next wifi event flushes).

## 9. Self-review

- **Placeholders:** none.
- **Internal consistency:** §5.2 says drop `auto_upload` strictly; §8 hedges with "if self-review flips this". On reflection, strict drop is right — this is single-developer firmware, the user is also the only deployer, and `#[serde(default)]` would carry an ignored field forever. Confirmed strict; the §8 hedge is removed by writing the test as `legacy_config_with_auto_upload_field_rejected`.
- **Scope:** focused on a single subsystem (recording terminal disposition) with one UI surface and one service. Suitable for a single implementation plan.
- **Ambiguity:** §6.3's "without awaiting" is the key phrase — the implementation must spawn a `tokio::task` for the upload so the command-loop iteration completes immediately. Otherwise the modal lingers. Made explicit.
- **Race:** `pending_decision` is set on Stop and cleared on each terminal command. If two button taps arrive (e.g., Save then Send before the first state propagates to the UI), the second handler sees `pending_decision = None` (already cleared) and warn-logs. Validated by handlers checking the id at the top.

## 10. Open questions

None. All design forks resolved during brainstorming.
