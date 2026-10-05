# Display Redesign — Milestone 2, Workstream C: Recording and Audio Backend — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the milestone 2 screens a working backend. That means pause and resume, the pre-recording setting and its indicator, live recording settings, the "Send this recording?" timeout, audit-safe Delete, resending, the Files list, local playback, a fixed beam aim for the directional modes, real stereo over USB, the on-device mic level, the start-up microphone check, and the missing state producers the new screens read. It also fixes the two `recording_decision` integration tests that fail at the base commit.

**Architecture:**
- **Two lanes, two crates.** Everything lands in `crates/waved-meeting/**` (recording, decisions, Files, playback service) and `crates/waved-audio/**` (beamformer steering, stereo uplink, mic gain, mic check, speaker mixing). The two lanes touch different crates and can run in parallel. The one cross-lane dependency: Task 13 (player) needs Task 12 (`PlaybackService::play_local`).
- **Live inputs without touching `main.rs`.** `MeetingService` keeps its constructor and `run` signature. Two new builders, `with_live_inputs` and `with_files_publisher`, plug in the bus watches. Until the integrator wires them at the runtime switch, the service falls back to the boot config, which is today's behaviour. `AudioService` gets one builder, `set_host_listening_probe`. The player is a new task the integrator spawns.
- **Host-testable.** Pure logic is a free function or a small struct with its own unit tests: steering table, stereo rebuild, uplink interleave, mic-health monitor, status mapping, seconds-left, WAV track. Service behaviour is tested by driving the real service over its channels on a temp SQLite file. `waved-audio` tests run on macOS. `waved-meeting` tests run in the Linux container (sqlx).

**Tech Stack:** Rust 2021 (stable), tokio, sqlx 0.8 (SQLite), hound 3.5.1, chrono 0.4.44, nix 0.29 (statvfs), reqwest 0.12 (rustls), Docker/OrbStack for `scripts/linux-test.sh`.

**Spec:**
- `../TODO.md`: decisions D1–D10, workstream C items C1–C12, bugs BUG-15/16/17
- `../docs/mockups/README.md`: Dial states (Paused, Recording + "Speaker *n* talking", "Computer is using the mic"), Timings (auto-Keep 60 s), Interaction model ("Drag the pattern … Aim the beam")
- `docs/AUDIO_HARDWARE.md`: mic geometry, ALSA permutation, Mic Gain Pipeline, UAC2 routing, Playback DSP chain
- `../.superpowers/m2-planning/planning-brief.md`: M2 rules and ownership
- Format reference: `docs/superpowers/plans/2026-10-04-display-redesign-m1-foundation.md`

All code paths in this plan are relative to `waved/` (the Rust workspace).

---

## TODO coverage

| Item | Covered by | Notes |
|---|---|---|
| C1 Pause and resume | Tasks 5, 6 | BUG-16. Pause spans in metadata, timer frozen, diarizer timeline kept continuous |
| C2 Pre-recording setting | Task 3 | Toggle drops the buffer at once. **The length stays the compile-time 30 s** (`PREBUFFER_SECONDS`): D3 fixes the copy "includes last 30 s" and no screen sets a length, so a config key would be unused surface |
| C3 Fixed aim for directional modes | Task 14 | BUG-17. Follow talker is already `AudioMode::Steered`. Stereo is pinned to the device orientation |
| C4 Stereo over USB | Task 16 | BUG-15. The L/R pair is rebuilt around the echo-cancelled mid |
| C5 Mic level | Task 17 | `SetMicVolume` is clamped, applied and mirrored. The host-side UAC control is dormant on kernel 5.4, so the device control is the only runtime path |
| C6 Microphone health check | Task 18 | Needs contract request R1 |
| C7 Recording decisions | Tasks 8, 9, 10 | Auto-Keep with countdown, audit-safe Delete, resend |
| C8 Playback | Tasks 12, 13 | Speaker mixing in `waved-audio`, player in `waved-meeting` |
| C10 Apply the persisted beam aim | Task 15 | |
| C11 Live recording settings | Task 2 | |
| C12 Publish the pre-recording state | Task 4 | |
| Pre-existing test failures | Task 1 | `send_queues_upload_when_online`, `automatic_upload_emits_success_for_the_recording_id` |
| Support (no TODO ID) | Tasks 7, 11, 19 | Producers the M2 screens read but nothing writes today: `MeetingState.marks` / `current_speaker` (A2), `FilesState` (D1/D3 backend half), `AudioState.host_listening` / `doa_confidence` (A1) |
| **Excluded** | — | **C9 audio acceptance** (device sessions, milestone 4). Nothing from B, D, E or F is touched |

---

## Global Constraints

- **Ownership.** Edit only `crates/waved-meeting/**` and `crates/waved-audio/**`. Shared contracts, `main.rs`, `waved-ui` and docs outside these crates go through **Contract requests** or the **Integrator handoff** below. Legacy UI files are not touched.
- **Recording format.** WAV, 48 000 Hz, mono, 16-bit PCM (`RECORDING_SAMPLE_RATE`, `docs/AUDIO_HARDWARE.md` "Recording bit depth"). Metadata `schema_version` stays `1`. New fields are additive and `#[serde(default)]`.
- **Audio pipeline facts:**
  - ALSA period: 1024 frames (21.3 ms).
  - Mic capture: `hw:0,2`, 8 channels. ALSA→physical map `[5, 4, 7, 6, 1, 0, 3, 2]`. All channel masks in this plan use **ALSA order** (bit n = ALSA channel n).
  - Speaker: `hw:0,1`, 2 channels, S16, 48 kHz, through the SP#5 DSP chain. The AEC reference taps the post-gain signal.
  - USB uplink: `hw:1,0` playback, 2 channels, S16, 48 kHz.
  - Mic mute zero-fills the raw frame in `AudioService::run` **before** VU, DSP and recording. The microphone check must read the frame before that point.
- **Value rules:**
  - `mic_volume` is linear percent, 0..=800, 100 = unity. Always clamp with `waved_config::clamp_mic_volume`.
  - Beam aim wraps to [0, 360) with `waved_config::wrap_beam_aim`. A non-finite aim is ignored.
  - Aim and DoA azimuth are the same "device degrees": the kit dial draws `doa_horizontal` clockwise from the top of the screen without conversion (`kit/components/dial.rs`), so `beam_aim_deg` is used as the beamformer azimuth unchanged. The device pass confirms the sign (Device verification, step 6).
- **Timings:**

  | What | Value |
  |---|---|
  | Pre-roll | 30 s (`PREBUFFER_SECONDS`) |
  | "Send this recording?" auto-Keep | `SettingsState.ask_timeout_s` (default 60 s, the settings layer clamps 15..=600) |
  | Meeting tick (timer, countdown) | 1 s |
  | Files list refresh | every 5 s, plus right after any command that changes a recording |
  | Playback position on the bus | every 250 ms while playing, plus on every state change |
  | Player queue | 2 periods ahead (~43 ms) |
  | Microphone check | 3 s windows, at most 10 windows |

- **Privacy rules:**
  - D3 dot: `pre_recording_active` = setting on ∧ not recording ∧ not muted ∧ capture healthy.
  - Turning pre-recording off drops buffered audio at once.
  - The pre-roll is frozen while a take runs.
  - Nothing captured during a pause reaches the file.
  - Delete never brings a recording back after a power cut.
  - An unanswered decision is **kept** (D4), never sent or deleted.
- **Copy** (plain-language failure reasons in `FilesState`): sentence case and plain words. Say what happened and what to do next. No "…".
- **Commits:** conventional format `type(scope): lowercase summary` with a bullet-list body, ending in:
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
- **Verification:** always run with `set -o pipefail`, from `waved/`.
  - **Host (macOS):** `cargo test -p waved-audio --lib`. Integration tests: `cargo test -p waved-audio --test <name>`.
  - **Linux container:**
    - `scripts/linux-test.sh test -p waved-meeting --lib`
    - `scripts/linux-test.sh test -p waved-meeting --test recording_decision`
    - `scripts/linux-test.sh check -p waved-audio --features capture-alsa` (the `uac` module only builds with ALSA)
    - `scripts/linux-test.sh check -p waved --features ui,capture-alsa` (the binary still builds)
  - **Lint, `waved-audio`:** `cargo clippy -p waved-audio --all-targets`. The baseline at `374015f` is **18** warnings for `(lib)`, **37** for `(lib test)` and **1** for `(test "steered_beam_handoff")`. Counts must not grow, and no warning may point into a file this plan creates (`beam/aim.rs`, and the new code in `mic_check.rs`, `processed_frame.rs` and `playback.rs`).
  - **Lint, `waved-meeting`:** `scripts/linux-test.sh clippy -p waved-meeting --all-targets`. Task 1 records the baseline counts. Later tasks must not grow them, and no warning may point into `removal.rs`, `files.rs`, `player.rs` or `service_tests.rs`.

## Review Focus

These five inputs aren't exercised by the happy-path tests but are likely to hurt someone using the device. Each one has a test in the task named after it.

1. **Turning pre-recording off right before pressing Record.** The buffered audio must be gone at once and the next take must have no pre-roll. This is a privacy promise (D3). *(Task 3: `pre_roll_follows_the_setting`)*
2. **Talking during a pause, then stopping while still paused.** No paused audio may reach the WAV, the timer must stay frozen, and the take must still finalize with its pause span closed. A dropped frame during the pause must not destroy the take. *(Task 5: `paused_audio_never_reaches_the_file`, `stop_while_paused_closes_the_span`; Task 6: `pause_and_resume_cut_the_paused_audio_and_freeze_the_timer`, `a_dropped_frame_while_paused_keeps_the_take`)*
3. **Power cut in the middle of Delete.** Reconciliation imports any finalized directory without a row as local-only. A half-done delete must therefore finish at boot and must never resurrect the recording. *(Task 9: `interrupted_delete_finishes_on_boot`)*
4. **Answering "Send this recording?" just before the timeout.** Send at second 59 must win. The auto-Keep must not overwrite it a second later. *(Task 8: `answer_before_timeout_cancels_auto_keep`)*
5. **Aim precedence and conventions.**
   - The test override (`SetDoaOverride`, `scripts/test-beamforming.sh`) must still win.
   - Follow talker must ignore the aim, and Stereo must stay on the device orientation.
   - Focused, Narrow and Two sides must hold their aim while the talker moves.

   *(Task 14: `fixed_direction_follows_the_mode_table`, `cardioid_holds_the_aim_while_the_talker_moves`, `stereo_keeps_the_device_orientation`, `doa_override_still_wins_for_the_beamforming_test`)*

---

## Contract requests

### Land in M2.0, before any task runs (integrator)

**R1 — Microphone health in `AudioState` (C6).** File: `crates/waved-state/src/lib.rs`.

```rust
/// Start-up microphone check (C6). Masks use ALSA channel order:
/// bit n = ALSA channel n (see docs/AUDIO_HARDWARE.md "ALSA → Physical Permutation").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MicHealth {
    /// Not judged yet, or the room stayed too quiet to judge.
    #[default]
    Unknown,
    Ok,
    /// Every element answers, but these are much quieter than the rest.
    Degraded { low_mask: u8 },
    /// These elements are silent; the UI shows "Microphones not responding".
    NotResponding { dead_mask: u8 },
}
```

Add to `AudioState` (after `processed_frames_dropped`):

```rust
    /// Result of the start-up microphone check (C6).
    pub mic_health: MicHealth,
```

…and `mic_health: MicHealth::Unknown,` in `impl Default for AudioState`.

**Why:** A6 shows "Microphones not responding" from state, and no existing field carries the verdict (`capture_healthy` only reports a dead producer thread).

That is the only M2.0 request. Every other command and field this plan needs already exists at `374015f`:
- commands: `PauseRecording`, `ResumeRecording`, `SetPreRecording`, `SetUploadMode`, `SetBeamAim`, `DeleteRecording`, `PlayRecording`, `PausePlayback`, `ResumePlayback`, `SeekPlayback`, `StopPlayback`
- state: `MeetingState.{paused, marks, current_speaker, ask_deadline_secs, pre_recording_active}`, `FilesState`, `PlaybackState`, `SettingsState`, `AudioState.{beam_aim_deg, host_listening, doa_confidence}`

### Integrator handoff — wire at the end-of-M2 runtime switch (`crates/waved/src/main.rs`)

These call APIs that Tasks 2, 11, 13 and 19 create, so they can't land in M2.0. Until they land, every service keeps today's behaviour.

**H1 — Meeting service live inputs and Files list (Tasks 2, 11).** Replace the `Ok(meeting_svc) => {` arm of the "Meeting service" block with:

```rust
        Ok(meeting_svc) => {
            let meeting_svc = meeting_svc
                .with_live_inputs(waved_meeting::MeetingLiveInputs {
                    settings: bus.settings.subscribe(),
                    audio: bus.audio.subscribe(),
                })
                .with_files_publisher(Arc::clone(&bus.files));
            let meeting_cmd_rx = bus.commands.subscribe();
            let wifi_rx = bus.wifi.subscribe();
            tokio::spawn(async move { meeting_svc.run(meeting_cmd_rx, wifi_rx).await });
        }
```

**H2 — Recording player (Task 13).** In the `else if let Some(pb) = cfg.playback.clone()` branch, right after `let playback = Arc::new(playback);`:

```rust
            tokio::spawn(
                waved_meeting::player::RecordingPlayer::new(
                    PathBuf::from(RECORDINGS_DIR),
                    Arc::clone(&playback) as Arc<dyn waved_meeting::player::SpeakerOutput>,
                    Arc::clone(&bus.playback),
                )
                .run(bus.commands.subscribe(), bus.audio.subscribe(), bus.meeting.subscribe()),
            );
```

**H3 — "Computer is using the mic" (Task 19).** Inside `if uac_cfg.enabled {`, right after the `UacUpstreamSink::spawn(...)` / `std::mem::forget(upstream);` lines:

```rust
                let probe_stats = Arc::clone(&uac_stats);
                audio_svc.set_host_listening_probe(move || {
                    probe_stats
                        .upstream_active
                        .load(std::sync::atomic::Ordering::Relaxed)
                });
```

**Docs handoff (for the integrator or L1).** Facts to record in `docs/AUDIO_HARDWARE.md` once the device pass confirms them:
- **Fixed aim (Beamforming Modes).** Focused/Narrow/Two sides steer to `audio.beam_aim_deg` (device degrees, same frame as the DoA azimuth). Stereo steers to (0°, 0°). `SetDoaOverride` still overrides both.
- **Stereo uplink (UAC2 Routing).** L/R = processed mid ± side. The side is ducked by the mid's AEC/NS suppression ratio and ramped across each period.
- **Mic check (new section).** 3 s windows on raw pre-mute frames. An exactly-zero channel means not responding. Verdicts are relative to the median; the thresholds are those of `MicCheck`.
- **Playback DSP chain.** `play_local` input is mixed (saturating add) with the host stream before HPF/EQ/MBAGC/limiter, so speaker volume, the limiter and the AEC reference cover local playback.

### What other workstreams get from this plan

| Consumer | State / command | Behaviour |
|---|---|---|
| A2 Recording | `MeetingState.paused`, `marks`, `current_speaker` | `paused` follows Pause/Resume. `marks` counts saved marks of the current take (reset at Start, unchanged while paused). `current_speaker` is the open diarization segment's number, `None` when nobody talks, while paused, and after stop |
| A3 After stop | `MeetingState.ask_deadline_secs` | `Some(n)` counting down once a second while a decision is pending. `None` otherwise. At 0 the take is kept (`LocalOnly`) |
| A1 Home, A4 Resting | `MeetingState.pre_recording_active`, `AudioState.host_listening`, `doa_confidence` | See Global Constraints. `host_listening` needs H3 |
| A6 Messages | `AudioState.mic_health` | `NotResponding { .. }` → "Microphones not responding" |
| D1–D3 Files | `FilesState` | Newest first. Rows that are recording or being deleted are hidden. D sorts "attention first" itself. `low_space` = storage blocked or free space below `storage.minimum_free_mb` |
| D2 Recording detail | `PlaybackState`; `PlayRecording(id)`, `PausePlayback`, `ResumePlayback`, `SeekPlayback(ms)`, `StopPlayback`; `SendRecording(id)`, `RetryUpload(id)`, `DeleteRecording(id)` | `marks_ms` count from the start of the file (pre-roll included). `SendRecording(id)` queues the pending take *or* a kept/failed one. `RetryUpload(id)` queues a kept/failed one. Neither does anything for uploaded or damaged takes. `DeleteRecording(id)` is refused while that recording is uploading. Playback pauses by itself during a call (HID off-hook) or a recording |
| B4 Volume / Mic level | `SetMicVolume(u16)`, `AudioState.mic_volume` | Clamped to 0..=800 (100 = unity). How the arc maps onto it is B's choice |
| B2 Audio mode | `SetBeamAim(f32)`, `AudioState.beam_aim_deg` | Wrapped to [0, 360), applied on the next audio period, persisted by `waved-settings` |

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/waved-meeting/Cargo.toml` | Add `waved-settings` dependency (boot-config fallback) | Modify |
| `crates/waved-meeting/tests/recording_decision.rs` | Hermetic decision-flow tests against a local webhook receiver | Modify |
| `crates/waved-meeting/src/lib.rs` | Live inputs, stop policy, pause/resume, pre-recording, marks/speaker, decision timer, Delete, resend, Files publishing | Modify |
| `crates/waved-meeting/src/service_tests.rs` | Service-level test rig and tests for Tasks 2–11 | Create |
| `crates/waved-meeting/src/recorder.rs` | `PreBuffer` on/off; pause spans on `ActiveRecording` | Modify |
| `crates/waved-meeting/src/metadata.rs` | `recording.pauses` in metadata.json | Modify |
| `crates/waved-meeting/src/diarization.rs` | Pause/resume with a continuous timeline; `current_speaker` | Modify |
| `crates/waved-meeting/src/model.rs` | `MeetingStatus::Deleting` | Modify |
| `crates/waved-meeting/src/db.rs` | Stale decisions, removal claim, requeue, Files rows | Modify |
| `crates/waved-meeting/src/removal.rs` | Audit-safe removal and boot completion | Create |
| `crates/waved-meeting/src/files.rs` | `FilesState` snapshot: status mapping, plain reasons, free space | Create |
| `crates/waved-meeting/src/player.rs` | `SpeakerOutput`, `Track` (WAV reader), `RecordingPlayer` | Create |
| `crates/waved-audio/Cargo.toml` | Add `waved-config` dependency (clamp rules) | Modify |
| `crates/waved-audio/src/lib.rs` | `handle_command`, fixed steering, aim, stereo rebuild, mic clamp, mic check, host-listening probe, DoA confidence | Modify |
| `crates/waved-audio/src/beam/aim.rs` | Mode → fixed direction table | Create |
| `crates/waved-audio/src/beam/mod.rs` | `pub mod aim;`, `Beamformer::direction()` | Modify |
| `crates/waved-audio/src/processed_frame.rs` | `rebuild_stereo`, `side_gain`, `uplink_interleaved_i16` | Modify |
| `crates/waved-audio/src/uac/upstream.rs` | Send the frame's uplink pair | Modify |
| `crates/waved-audio/src/playback.rs` | Local input mixed into the speaker loop | Modify |
| `crates/waved-audio/src/mic_check.rs` | `MicHealthMonitor` | Modify |
| `crates/waved-audio/src/stats_collector.rs` | `mic_health`, `mic_dead_mask`, `mic_low_mask` in `/tmp/audio_stats.json` | Modify |

Task order:
- **Meeting lane:** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 13. Every task edits `lib.rs`, so they run in sequence.
- **Audio lane:** 12 → 14 → 15 → 16 → 17 → 18 → 19. Every task edits `lib.rs` or `playback.rs`, so they run in sequence.
- **Between lanes:** Task 13 starts after Task 12.

---

## Meeting lane

### Task 1: Diagnose and fix the failing `recording_decision` tests

**Diagnosis (done while planning, reproduce it first):**
- `tests/recording_decision.rs` says "Each test boots a `MeetingService` with the `simulate` feature so upload calls land in `./test_uploads/`". Nothing turns that feature on.
- Without it, the worker posts to `https://example.com/upload` over real HTTPS, so nothing ever appears in `./test_uploads/` and no `RecordingUploadSucceeded` event is sent.
- Measured at `374015f` in the container:
  - plain run: 7 passed, 2 failed (`send_queues_upload_when_online` panics at line 175, `automatic_upload_emits_success_for_the_recording_id` at line 249)
  - with `--features simulate`: 9 passed
- The fix makes the tests hermetic. They upload to a local receiver on 127.0.0.1, the same pattern as `tests/webhook_contract.rs`. They no longer depend on a feature flag, on the network, or on files left in the crate directory.

**Files:**
- Modify: `crates/waved-meeting/tests/recording_decision.rs`

**Interfaces:**
- Consumes: `waved_meeting::MeetingService::{new, run}`, `waved_config::WavedConfig`, the webhook contract (the `Idempotency-Key` header carries the meeting id; see `tests/webhook_contract.rs`)
- Produces: test-only `async fn webhook_receiver() -> (String, mpsc::UnboundedReceiver<String>)`

- [ ] **Step 1: Reproduce the failure and confirm the root cause**

```bash
set -o pipefail
scripts/linux-test.sh test -p waved-meeting --test recording_decision 2>&1 | grep -E '^test |test result'
scripts/linux-test.sh test -p waved-meeting --features simulate --test recording_decision 2>&1 | grep -E '^test |test result'
```

Expected:
- The first run shows `test result: FAILED. 7 passed; 2 failed`, with the two named tests failing.
- The second run shows `test result: ok. 9 passed`.
- If the second run also fails, the root cause is different. In that case stop and use superpowers:systematic-debugging before changing anything. Check, in order:
  1. Does `db::claim_next_upload` return the row (status `queued`, `attempt_cycle < retry_max`)?
  2. Does the worker wake (`upload_wake.notify_one()` after `SendRecording`)?
  3. Does `wifi_rx.borrow().connected` read `true`?

- [ ] **Step 2: Record the clippy baseline for this crate**

```bash
set -o pipefail
scripts/linux-test.sh clippy -p waved-meeting --all-targets 2>&1 | grep -E '^warning: `waved-meeting`' | tee /tmp/waved-meeting-clippy-baseline.txt
```

Expected: one line per target with its warning count (or no lines if it is clean). Copy those lines into this task's commit body as the baseline for later tasks.

- [ ] **Step 3: Rewrite the harness to use a local receiver**

Replace the top of `crates/waved-meeting/tests/recording_decision.rs` (module doc through the end of `boot_with_mode`) with:

```rust
//! Integration tests for the Save / Send / Discard recording flow.
//!
//! Uploads go to a local webhook receiver on 127.0.0.1, so these tests need
//! neither the network nor the `simulate` feature. With `simulate` on, the
//! worker copies files to ./test_uploads instead of using HTTP and never
//! reaches the receiver, so this file only builds without it.
#![cfg(not(feature = "simulate"))]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, watch};
use waved_audio::callbacks::AudioFrame;
use waved_audio::processed_frame::ProcessedAudioFrame;
use waved_config::{UploadMode, WavedConfig};
use waved_meeting::MeetingService;
use waved_state::{Command, DeviceEvent, MeetingState, WifiState};

fn test_config(endpoint: &str, upload_mode: UploadMode) -> WavedConfig {
    let mut config = WavedConfig::default();
    config.device.name = "test".into();
    config.api.endpoint = endpoint.into();
    config.api.allow_insecure_http = true;
    config.api.upload_mode = upload_mode;
    config.api.retry_max = 1;
    config.api.retry_backoff_ms = 10;
    config.storage.prune_uploaded_at_percent = 100;
    config.storage.minimum_free_mb = 0;
    config
}

/// A webhook receiver that answers every request with 204 and reports each
/// request's Idempotency-Key, which is the meeting id.
async fn webhook_receiver() -> (String, mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (keys_tx, keys_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let keys_tx = keys_tx.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0_u8; 16 * 1024];
                let header_end = loop {
                    let read = match socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => read,
                    };
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while request.len() < header_end + content_length {
                    match socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&buffer[..read]),
                    }
                }
                let key = headers.lines().find_map(|line| {
                    line.strip_prefix("idempotency-key:")
                        .map(|value| value.trim().to_string())
                });
                let _ = socket
                    .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                if let Some(key) = key {
                    let _ = keys_tx.send(key);
                }
            });
        }
    });
    (format!("http://{address}/webhook"), keys_rx)
}

fn fake_frame() -> ProcessedAudioFrame {
    let frame = AudioFrame::mic_only(vec![0i16; 480], 1, 48000, 0);
    let mic_mono = vec![0i16; 480];
    let ref_mono = vec![0i16; 480];
    ProcessedAudioFrame::from_raw(&frame, &mic_mono, &ref_mono)
}

struct Harness {
    cmd_tx: broadcast::Sender<Command>,
    meeting_rx: watch::Receiver<MeetingState>,
    wifi_tx: watch::Sender<WifiState>,
    audio_tx: mpsc::Sender<ProcessedAudioFrame>,
    recordings_dir: PathBuf,
    events_rx: broadcast::Receiver<DeviceEvent>,
    /// Meeting ids the receiver has accepted, in arrival order.
    uploads: mpsc::UnboundedReceiver<String>,
}

async fn boot(connected: bool) -> Harness {
    boot_with_mode(connected, UploadMode::Ask).await
}

async fn boot_with_mode(connected: bool, upload_mode: UploadMode) -> Harness {
    let (endpoint, uploads) = webhook_receiver().await;
    let dir = tempfile::tempdir().unwrap();
    let recordings_dir = dir.path().join("recordings");
    let db_path = dir.path().join("t.db").to_string_lossy().into_owned();
    Box::leak(Box::new(dir)); // outlive the test

    let (cmd_tx, _) = broadcast::channel::<Command>(16);
    let (meeting_tx, meeting_rx) = watch::channel(MeetingState::default());
    let (events_tx, events_rx) = broadcast::channel::<DeviceEvent>(16);
    let (wifi_tx, wifi_rx) = watch::channel(WifiState {
        connected,
        ..Default::default()
    });
    let (audio_tx, audio_rx) = mpsc::channel::<ProcessedAudioFrame>(16);
    let (doa_tx, _) = broadcast::channel(16);

    let svc = MeetingService::new(
        test_config(&endpoint, upload_mode),
        &db_path,
        recordings_dir.clone(),
        Arc::new(meeting_tx),
        events_tx,
        audio_rx,
        doa_tx.subscribe(),
    )
    .await
    .unwrap();

    let cmd_rx = cmd_tx.subscribe();
    tokio::spawn(async move { svc.run(cmd_rx, wifi_rx).await });

    Harness {
        cmd_tx,
        meeting_rx,
        wifi_tx,
        audio_tx,
        recordings_dir,
        events_rx,
        uploads,
    }
}
```

`wait_for`, `record_and_stop` and the other tests stay as they are, except for these three changes.

In `stop_recording_populates_pending_decision`, replace `assert_eq!(d.endpoint, "https://example.com/upload");` with:

```rust
    assert!(
        d.endpoint.starts_with("http://127.0.0.1:"),
        "endpoint snapshot: {}",
        d.endpoint
    );
```

Replace the body of `save_clears_pending_and_marks_local_only` after the `assert!(dir.join("audio.wav").exists());` line with:

```rust
    // Even if Wi-Fi comes up, nothing is sent: the status is local_only.
    let _ = h.wifi_tx.send(WifiState {
        connected: true,
        ..Default::default()
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        h.uploads.try_recv().is_err(),
        "save must not trigger an upload"
    );
}
```

Replace `send_queues_upload_when_online` with:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_queues_upload_when_online() {
    let mut h = boot(true).await;
    let id = record_and_stop(&mut h).await;
    h.cmd_tx.send(Command::SendRecording(id)).unwrap();
    wait_for(&mut h.meeting_rx, |s| s.pending_decision.is_none()).await;
    let delivered = tokio::time::timeout(Duration::from_secs(5), h.uploads.recv())
        .await
        .expect("the receiver got no upload within 5 s")
        .expect("receiver stopped");
    assert_eq!(delivered, id.to_string());
}
```

`automatic_upload_emits_success_for_the_recording_id` needs no change: it now uploads to the receiver and gets the event.

- [ ] **Step 4: Run and expect a pass, with and without other features**

```bash
set -o pipefail
scripts/linux-test.sh test -p waved-meeting --test recording_decision 2>&1 | grep -E '^test |test result'
scripts/linux-test.sh test -p waved-meeting --features simulate --test recording_decision 2>&1 | grep -E 'test result|running'
scripts/linux-test.sh test -p waved-meeting --lib 2>&1 | grep -E 'test result'
```

Expected:
- The first run shows `test result: ok. 9 passed`.
- The second shows `running 0 tests` for this file, because it is compiled out under `simulate`.
- The third shows `--lib` all passing.

- [ ] **Step 5: Remove the stale local artifacts**

`crates/waved-meeting/test_uploads/` is gitignored output from earlier `simulate` runs. Delete it locally with `rm -rf crates/waved-meeting/test_uploads` (this changes nothing in git).

- [ ] **Step 6: Lint and commit**

Run the clippy command from Step 2 and confirm the counts did not grow.

```bash
git add crates/waved-meeting/tests/recording_decision.rs
git commit -m "test(meeting): run recording decision tests against a local webhook receiver

- send_queues_upload_when_online and automatic_upload_emits_success_for_the_recording_id
  relied on the simulate feature, which nothing enabled, so they posted to
  example.com and failed deterministically
- upload to a 127.0.0.1 receiver instead and assert on the Idempotency-Key
- compile the file out under simulate, where uploads never reach HTTP
- clippy baseline for waved-meeting: <paste lines from step 2>

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Live recording settings in `MeetingService` (C11) and the service test rig

**Files:**
- Modify: `crates/waved-meeting/Cargo.toml`
- Modify: `crates/waved-meeting/src/lib.rs`
- Create: `crates/waved-meeting/src/service_tests.rs`

**Interfaces:**
- Consumes:
  - `waved_settings::settings_from_config(&WavedConfig) -> SettingsState`
  - `waved_state::{SettingsState, AudioState, UploadMode}`
- Produces:
  - `pub struct MeetingLiveInputs { pub settings: watch::Receiver<SettingsState>, pub audio: watch::Receiver<AudioState> }`
  - `impl MeetingService { pub fn with_live_inputs(self, inputs: MeetingLiveInputs) -> Self }`
  - private `struct StopPolicy { upload_mode: UploadMode }` (Task 8 adds `ask_timeout_s`)
  - test rig `service_tests::{Rig, start_rig, start_rig_in, quiet_config, settings_for, frame_of}`

- [ ] **Step 1: Add the dependency**

In `crates/waved-meeting/Cargo.toml` under `[dependencies]`, after `waved-audio`:

```toml
waved-settings = { path = "../waved-settings" }
```

- [ ] **Step 2: Write the failing tests and the rig**

Add to `crates/waved-meeting/src/lib.rs` after the `pub mod worker;` line:

```rust
#[cfg(test)]
mod service_tests;
```

Create `crates/waved-meeting/src/service_tests.rs`:

```rust
//! Service-level tests for the milestone 2 recording backend. Each test boots
//! a real MeetingService on a temp SQLite file and drives it over its
//! channels, the same way main.rs wires it.

// The rig grows with the tasks: some fields only keep channels open and not
// every test uses every helper.
#![allow(dead_code)]

use super::*;
use std::time::Duration;
use waved_audio::integrity::{IntegritySignal, RecordingIntegrity};
use waved_state::{AudioMode, AudioState, SettingsState, UploadMode, WifiState};

pub(crate) fn quiet_config(upload_mode: UploadMode) -> WavedConfig {
    let mut config = WavedConfig::default();
    config.api.upload_mode = upload_mode;
    config.storage.prune_uploaded_at_percent = 100;
    config.storage.minimum_free_mb = 0;
    config
}

pub(crate) fn settings_for(upload_mode: UploadMode) -> SettingsState {
    SettingsState {
        upload_mode,
        ..SettingsState::default()
    }
}

/// One 10 ms processed frame (480 samples) holding `value`.
pub(crate) fn frame_of(value: f32) -> ProcessedAudioFrame {
    ProcessedAudioFrame {
        mic_mono_f32: vec![value; 480],
        mic_stereo_f32: None,
        reference_f32: vec![0.0; 480],
        sample_rate: 48_000,
        timestamp_ms: 0,
        raw_mic_i16: vec![0; 480],
        raw_mic_channels: 1,
        aec_metrics: Default::default(),
    }
}

pub(crate) struct Rig {
    pub meeting: Arc<watch::Sender<MeetingState>>,
    pub frames: mpsc::Sender<ProcessedAudioFrame>,
    pub settings: watch::Sender<SettingsState>,
    pub audio: watch::Sender<AudioState>,
    pub commands: broadcast::Sender<Command>,
    pub doa: broadcast::Sender<DoaFrame>,
    pub wifi: watch::Sender<WifiState>,
    pub integrity: Arc<RecordingIntegrity>,
    pub integrity_tx: mpsc::UnboundedSender<IntegritySignal>,
    pub db_path: PathBuf,
    pub recordings_dir: PathBuf,
    _tmp: tempfile::TempDir,
}

pub(crate) async fn start_rig(config: WavedConfig, settings: SettingsState) -> Rig {
    start_rig_in(tempfile::tempdir().unwrap(), config, settings).await
}

/// Boot in `tmp`, which a test may have seeded with `t.db` and `rec/` first.
pub(crate) async fn start_rig_in(
    tmp: tempfile::TempDir,
    config: WavedConfig,
    settings: SettingsState,
) -> Rig {
    let db_path = tmp.path().join("t.db");
    let recordings_dir = tmp.path().join("rec");
    std::fs::create_dir_all(&recordings_dir).unwrap();
    let meeting = Arc::new(watch::channel(MeetingState::default()).0);
    let (events, _) = broadcast::channel(64);
    let (frames, frames_rx) = mpsc::channel(512);
    let (doa, _) = broadcast::channel::<DoaFrame>(64);
    let (settings_tx, settings_rx) = watch::channel(settings);
    let (audio, audio_rx) = watch::channel(AudioState {
        mode: config.audio.mode.clone(),
        ..AudioState::default()
    });
    let integrity = Arc::new(RecordingIntegrity::default());
    let (integrity_tx, integrity_rx) = mpsc::unbounded_channel();
    let service = MeetingService::new_with_integrity(
        config,
        db_path.to_str().unwrap(),
        recordings_dir.clone(),
        Arc::clone(&meeting),
        events,
        MeetingPipelineInputs {
            audio_rx: frames_rx,
            doa_rx: doa.subscribe(),
            integrity: Arc::clone(&integrity),
            integrity_rx,
        },
    )
    .await
    .unwrap()
    .with_live_inputs(MeetingLiveInputs {
        settings: settings_rx,
        audio: audio_rx,
    });
    let (commands, commands_rx) = broadcast::channel(32);
    let (wifi, wifi_rx) = watch::channel(WifiState::default());
    tokio::spawn(service.run(commands_rx, wifi_rx));
    Rig {
        meeting,
        frames,
        settings: settings_tx,
        audio,
        commands,
        doa,
        wifi,
        integrity,
        integrity_tx,
        db_path,
        recordings_dir,
        _tmp: tmp,
    }
}

impl Rig {
    pub fn send(&self, command: Command) {
        self.commands.send(command).unwrap();
    }

    /// Queue `count` frames holding `value`, then give the service time to
    /// drain them before the next command (commands and frames arrive on
    /// different channels, and `select!` picks among ready branches at random).
    pub async fn feed(&self, count: usize, value: f32) {
        for _ in 0..count {
            self.frames.send(frame_of(value)).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    pub async fn wait_for(&self, what: &str, predicate: impl Fn(&MeetingState) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !predicate(&self.meeting.borrow()) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}: {:?}",
                *self.meeting.borrow()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub async fn start(&self) -> Uuid {
        self.send(Command::StartRecording);
        self.wait_for("recording", |s| s.recording).await;
        self.meeting.borrow().meeting_id.expect("recording has an id")
    }

    pub async fn stop(&self) {
        self.send(Command::StopRecording);
        self.wait_for("stopped", |s| !s.recording && s.meeting_id.is_none())
            .await;
    }

    pub async fn pool(&self) -> SqlitePool {
        db::init_pool(self.db_path.to_str().unwrap()).await.unwrap()
    }

    pub async fn status_in(pool: &SqlitePool, id: Uuid) -> Option<String> {
        sqlx::query_as::<_, (String,)>("SELECT upload_status FROM meetings WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await
            .unwrap()
            .map(|(status,)| status)
    }

    pub async fn status(&self, id: Uuid) -> Option<String> {
        Self::status_in(&self.pool().await, id).await
    }

    pub async fn wait_status(&self, id: Uuid, want: Option<&str>) {
        let pool = self.pool().await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let got = Self::status_in(&pool, id).await;
            if got.as_deref() == want {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "status of {id} is {got:?}, want {want:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub fn dir(&self, id: Uuid) -> PathBuf {
        self.recordings_dir.join(id.to_string())
    }

    pub fn metadata(&self, id: Uuid) -> metadata::WebhookMetadata {
        serde_json::from_slice(&std::fs::read(self.dir(id).join("metadata.json")).unwrap())
            .unwrap()
    }

    pub fn wav_samples(&self, id: Uuid) -> u32 {
        hound::WavReader::open(self.dir(id).join("audio.wav"))
            .unwrap()
            .duration()
    }
}

#[tokio::test]
async fn upload_mode_changed_during_a_take_applies_at_stop() {
    let rig = start_rig(
        quiet_config(UploadMode::Automatic),
        settings_for(UploadMode::Automatic),
    )
    .await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    // What SettingsService does when the user picks "Keep on Wave only".
    rig.settings.send_modify(|s| s.upload_mode = UploadMode::LocalOnly);
    rig.stop().await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
    assert!(rig.meeting.borrow().pending_decision.is_none());
}

#[tokio::test]
async fn metadata_records_the_live_audio_mode() {
    // The boot config says Omni; the user switched to Focused since.
    let rig = start_rig(
        quiet_config(UploadMode::LocalOnly),
        settings_for(UploadMode::LocalOnly),
    )
    .await;
    rig.audio.send_modify(|a| a.mode = AudioMode::Cardioid);
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    assert_eq!(rig.metadata(id).recording.audio_mode, "Cardioid");
}
```

- [ ] **Step 3: Run and expect a compile failure**

Run `scripts/linux-test.sh test -p waved-meeting --lib service_tests`.

Expected: build error, because `MeetingLiveInputs` and `with_live_inputs` don't exist yet.

- [ ] **Step 4: Implement**

In `crates/waved-meeting/src/lib.rs`:

1. Extend the imports:

```rust
use waved_state::{
    AudioState, Command, DeviceEvent, MeetingState, PendingDecision, RecordingIntegrityFault,
    SettingsState, WifiState,
};
```

2. Add three fields to `MeetingService` after `webhook_test_marker`:

```rust
    /// Live user settings (upload mode, pre-recording, ask timeout). Seeded
    /// from the boot config; `with_live_inputs` swaps in the bus watch.
    settings_rx: watch::Receiver<SettingsState>,
    /// Live audio state: the mode recorded in metadata, plus mute and capture
    /// health for the pre-recording indicator.
    audio_state_rx: watch::Receiver<AudioState>,
    /// Keeps the boot-config fallback channels open until live inputs replace them.
    _fallback: Option<(watch::Sender<SettingsState>, watch::Sender<AudioState>)>,
```

3. Add after `pub struct MeetingPipelineInputs { … }`:

```rust
/// Bus watches that make recording policy follow the device settings live
/// (C11). Without them the service uses the boot-time config.
pub struct MeetingLiveInputs {
    pub settings: watch::Receiver<SettingsState>,
    pub audio: watch::Receiver<AudioState>,
}

/// What happens to a take at Stop, read from the live settings at that moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StopPolicy {
    upload_mode: UploadMode,
}
```

4. In `new_with_integrity`, before `Ok(Self {`:

```rust
        let (settings_fallback, settings_rx) =
            watch::channel(waved_settings::settings_from_config(&config));
        let (audio_fallback, audio_state_rx) = watch::channel(AudioState {
            mode: config.audio.mode.clone(),
            ..AudioState::default()
        });
```

…and add to the struct literal:

```rust
            settings_rx,
            audio_state_rx,
            _fallback: Some((settings_fallback, audio_fallback)),
```

5. Add to `impl MeetingService`, after `new_with_integrity`:

```rust
    /// Follow the live settings and audio state instead of the boot config.
    pub fn with_live_inputs(mut self, inputs: MeetingLiveInputs) -> Self {
        self.settings_rx = inputs.settings;
        self.audio_state_rx = inputs.audio;
        self._fallback = None;
        self
    }

    fn stop_policy(&self) -> StopPolicy {
        StopPolicy {
            upload_mode: self.settings_rx.borrow().upload_mode,
        }
    }

    fn live_audio_mode(&self) -> String {
        format!("{:?}", self.audio_state_rx.borrow().mode)
    }
```

6. `RecordingFinalizer::stop_recording` takes the policy. Change the signature to `async fn stop_recording(self, rec: recorder::ActiveRecording, policy: StopPolicy)`. Replace `match self.config.api.upload_mode {` with `match policy.upload_mode {`, and `(self.config.api.upload_mode == UploadMode::Ask)` with `(policy.upload_mode == UploadMode::Ask)`.

7. `MeetingService::stop_recording` passes it through:

```rust
    fn stop_recording(
        &self,
        rec: recorder::ActiveRecording,
        policy: StopPolicy,
    ) -> impl Future<Output = ()> + Send + 'static {
        RecordingFinalizer::from(self).stop_recording(rec, policy)
    }
```

8. Both call sites (the `Command::StopRecording` and `Command::Shutdown` arms) become `let finalization = self.stop_recording(rec, self.stop_policy());`.

9. In the `Command::StartRecording` arm, read the mode once and use it for both the recorder and the row. Replace the two `&format!("{:?}", self.config.audio.mode)` expressions so the arm starts:

```rust
                                    let audio_mode = self.live_audio_mode();
                                    match recorder::ActiveRecording::start_with_integrity_epoch(
                                        &self.recordings_dir,
                                        &audio_mode,
                                        self.integrity.epoch(),
                                    )
                                    .await
                                    {
                                        Ok(rec) => {
                                            let id = rec.id;
                                            let _ = db::insert_meeting(
                                                &self.pool, &id,
                                                &audio_mode,
                                                &rec.dir.to_string_lossy(),
                                            ).await;
```

(Place `let audio_mode = …;` right before the `match`, after the `storage_blocked` check.)

- [ ] **Step 5: Run and expect a pass**

```bash
set -o pipefail
scripts/linux-test.sh test -p waved-meeting --lib 2>&1 | grep -E 'test result|FAILED|panicked'
scripts/linux-test.sh test -p waved-meeting --test recording_decision 2>&1 | grep -E 'test result'
```

Expected:
- `--lib` all pass, including both new tests.
- `recording_decision` shows `ok. 9 passed`. Those tests boot without live inputs, which proves the fallback still uses the config.

- [ ] **Step 6: Lint and commit**

Run `scripts/linux-test.sh clippy -p waved-meeting --all-targets` and confirm no growth over the baseline.

```bash
git add crates/waved-meeting/Cargo.toml crates/waved-meeting/src/lib.rs crates/waved-meeting/src/service_tests.rs
git commit -m "feat(meeting): follow live upload mode and audio mode

- MeetingLiveInputs + with_live_inputs plug the settings and audio watches in
- the upload mode is read at Stop and the audio mode at Start, so SetUploadMode
  and mode changes apply without a restart (C11)
- without live inputs the boot config still decides
- add the service test rig used by the milestone 2 backend tests

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Pre-recording setting (C2)

**Files:**
- Modify: `crates/waved-meeting/src/recorder.rs`
- Modify: `crates/waved-meeting/src/lib.rs`
- Modify: `crates/waved-meeting/src/service_tests.rs`

**Interfaces:**
- Consumes: `SettingsState.pre_recording` (live, from Task 2)
- Produces:
  - `PreBuffer::set_enabled(&mut self, enabled: bool)`
  - `PreBuffer::is_enabled(&self) -> bool`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/waved-meeting/src/recorder.rs`:

```rust
    #[test]
    fn prebuffer_disabled_drops_buffered_audio_and_ignores_pushes() {
        let mut pb = PreBuffer::new(1, 100, 1);
        pb.push(&[0.1; 50]);
        pb.set_enabled(false);
        assert!(!pb.is_enabled());
        assert_eq!(pb.len(), 0, "turning it off drops what was buffered");
        pb.push(&[0.2; 10]);
        assert_eq!(pb.len(), 0, "nothing is buffered while off");
        pb.set_enabled(true);
        pb.push(&[0.3; 10]);
        assert_eq!(pb.drain_all(), vec![0.3; 10]);
    }
```

Append to `crates/waved-meeting/src/service_tests.rs`:

```rust
#[tokio::test]
async fn pre_roll_follows_the_setting() {
    let rig = start_rig(
        quiet_config(UploadMode::LocalOnly),
        settings_for(UploadMode::LocalOnly),
    )
    .await;

    // On (the default): 50 idle frames (0.5 s) become the pre-roll.
    rig.feed(50, 0.5).await;
    let first = rig.start().await;
    rig.feed(20, 0.25).await;
    rig.stop().await;
    assert_eq!(rig.metadata(first).recording.pre_roll_duration_ms, 500);
    assert_eq!(rig.wav_samples(first), 70 * 480);

    // Off right before Record: the buffer is dropped and stays empty.
    rig.feed(50, 0.5).await;
    rig.settings.send_modify(|s| s.pre_recording = false);
    tokio::time::sleep(Duration::from_millis(50)).await;
    rig.feed(10, 0.5).await;
    let second = rig.start().await;
    rig.feed(20, 0.25).await;
    rig.stop().await;
    assert_eq!(rig.metadata(second).recording.pre_roll_duration_ms, 0);
    assert_eq!(rig.wav_samples(second), 20 * 480);
}
```

- [ ] **Step 2: Run and expect a failure**

Run `scripts/linux-test.sh test -p waved-meeting --lib prebuffer_disabled pre_roll_follows`.

Expected:
- build error: `set_enabled` doesn't exist
- after a stub, the second take's `pre_roll_duration_ms` is 1000, not 0

- [ ] **Step 3: Implement `PreBuffer` on/off**

In `crates/waved-meeting/src/recorder.rs`, add a field after `paused: bool,`:

```rust
    /// The user's pre-recording setting (D3). Off means nothing is buffered.
    enabled: bool,
```

Set `enabled: true,` in `PreBuffer::new`. Add after `resume`:

```rust
    /// Turn pre-recording on or off. Off drops everything buffered at once,
    /// because the setting is a privacy control, and frees the ring's memory.
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.samples = VecDeque::new();
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }
```

In `push`, change the guard to `if self.capacity == 0 || self.paused || !self.enabled {`.

- [ ] **Step 4: Follow the setting in the service**

In `MeetingService::run` in `crates/waved-meeting/src/lib.rs`:

1. Right after `let mut prebuffer = PreBuffer::new(…);`:

```rust
        prebuffer.set_enabled(self.settings_rx.borrow().pre_recording);
        let mut settings_rx = self.settings_rx.clone();
```

2. Add a `select!` branch before the `Ok(()) = wifi_rx.changed() =>` branch:

```rust
                Ok(()) = settings_rx.changed() => {
                    let pre_recording = settings_rx.borrow_and_update().pre_recording;
                    if pre_recording != prebuffer.is_enabled() {
                        prebuffer.set_enabled(pre_recording);
                        info!(
                            "pre-recording {}",
                            if pre_recording { "on" } else { "off; buffered audio dropped" }
                        );
                    }
                }
```

`clear()`/`resume()` after a take keep working: `resume()` only lifts the take freeze, and `push` still honours `enabled`.

- [ ] **Step 5: Run and expect a pass**

Run `scripts/linux-test.sh test -p waved-meeting --lib`.

Expected: all pass.

- [ ] **Step 6: Lint and commit**

Run clippy as in Task 2, Step 6.

```bash
git add crates/waved-meeting/src/recorder.rs crates/waved-meeting/src/lib.rs crates/waved-meeting/src/service_tests.rs
git commit -m "feat(meeting): honour the pre-recording setting

- PreBuffer::set_enabled drops buffered audio when turned off (D3)
- MeetingService follows SettingsState.pre_recording live (C2)
- the length stays the 30 s that the Record copy promises

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Publish the pre-recording state (C12)

**Files:**
- Modify: `crates/waved-meeting/src/lib.rs`
- Modify: `crates/waved-meeting/src/service_tests.rs`

**Interfaces:**
- Consumes: `SettingsState.pre_recording`, `AudioState.{mic_muted, capture_healthy}`
- Produces:
  - `pub(crate) fn pre_recording_active(setting_on: bool, recording: bool, audio: &AudioState) -> bool`
  - writes `MeetingState.pre_recording_active`

- [ ] **Step 1: Write the failing tests**

Append to `crates/waved-meeting/src/service_tests.rs`:

```rust
#[test]
fn pre_recording_indicator_rule() {
    let healthy = AudioState::default();
    assert!(pre_recording_active(true, false, &healthy));
    assert!(!pre_recording_active(false, false, &healthy), "setting off");
    assert!(!pre_recording_active(true, true, &healthy), "recording");
    let muted = AudioState {
        mic_muted: true,
        ..AudioState::default()
    };
    assert!(!pre_recording_active(true, false, &muted), "muted");
    let dead = AudioState {
        capture_healthy: false,
        ..AudioState::default()
    };
    assert!(!pre_recording_active(true, false, &dead), "no audio arriving");
}

#[tokio::test]
async fn pre_recording_dot_follows_setting_mute_and_recording() {
    let rig = start_rig(
        quiet_config(UploadMode::LocalOnly),
        settings_for(UploadMode::LocalOnly),
    )
    .await;
    rig.wait_for("dot on at idle", |s| s.pre_recording_active).await;
    rig.audio.send_modify(|a| a.mic_muted = true);
    rig.wait_for("dot off while muted", |s| !s.pre_recording_active).await;
    rig.audio.send_modify(|a| a.mic_muted = false);
    rig.wait_for("dot back after unmute", |s| s.pre_recording_active).await;
    rig.start().await;
    rig.wait_for("dot off while recording", |s| !s.pre_recording_active).await;
    rig.stop().await;
    rig.wait_for("dot back after stop", |s| s.pre_recording_active).await;
    rig.settings.send_modify(|s| s.pre_recording = false);
    rig.wait_for("dot off with the setting", |s| !s.pre_recording_active).await;
}
```

- [ ] **Step 2: Run and expect a failure**

Run `scripts/linux-test.sh test -p waved-meeting --lib pre_recording`.

Expected: build error, because `pre_recording_active` doesn't exist yet.

- [ ] **Step 3: Implement**

In `crates/waved-meeting/src/lib.rs`, add a free function after `struct StopPolicy`:

```rust
/// The pre-recording dot (D3, C12): the buffer is filling right now.
pub(crate) fn pre_recording_active(setting_on: bool, recording: bool, audio: &AudioState) -> bool {
    setting_on && !recording && !audio.mic_muted && audio.capture_healthy
}
```

Add to `impl MeetingService`:

```rust
    fn publish_pre_recording(&self, recording: bool) {
        let active = pre_recording_active(
            self.settings_rx.borrow().pre_recording,
            recording,
            &self.audio_state_rx.borrow(),
        );
        self.meeting_tx.send_if_modified(|state| {
            if state.pre_recording_active == active {
                return false;
            }
            state.pre_recording_active = active;
            true
        });
    }
```

In `run`:

1. After `let mut settings_rx = self.settings_rx.clone();` add:

```rust
        let mut audio_state_rx = self.audio_state_rx.clone();
        self.publish_pre_recording(false);
```

2. Add a `select!` branch next to the settings branch:

```rust
                Ok(()) = audio_state_rx.changed() => {
                    // Mute and capture health feed the dot; the bottom of
                    // the loop republishes it.
                    let _ = audio_state_rx.borrow_and_update();
                }
```

3. Right after the closing `}` of the `tokio::select!` block (still inside `loop`), add:

```rust
            self.publish_pre_recording(active.is_some());
```

The audio watch changes about 47 times per second (VU levels). The branch only marks the value seen, and `send_if_modified` only notifies on a real change.

- [ ] **Step 4: Run and expect a pass**

Run `scripts/linux-test.sh test -p waved-meeting --lib`.

Expected: all pass.

- [ ] **Step 5: Lint and commit**

```bash
git add crates/waved-meeting/src/lib.rs crates/waved-meeting/src/service_tests.rs
git commit -m "feat(meeting): publish the pre-recording indicator

- MeetingState.pre_recording_active = setting on, idle, unmuted, capture healthy (C12)
- republished after every event, notifying only on change

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Pause spans in the recorder and metadata (C1, part 1)

**Files:**
- Modify: `crates/waved-meeting/src/recorder.rs`
- Modify: `crates/waved-meeting/src/metadata.rs`

**Interfaces:**
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct PauseSpan { pub offset_ms: u64, pub duration_ms: u64 }`. `offset_ms` is measured on the recording timeline, which excludes the pre-roll (the same frame as bookmark offsets).
  - `ActiveRecording::{pause_at(&mut self, now: std::time::Instant) -> bool, resume_at(&mut self, now: std::time::Instant) -> bool, is_paused(&self) -> bool, pauses(&self) -> &[PauseSpan]}`
  - `FinalizedRecording.pauses: Vec<PauseSpan>`
  - `metadata::PauseMetadata { offset_ms: u64, duration_ms: u64 }`
  - `RecordingMetadata.pauses: Vec<PauseMetadata>` (`#[serde(default)]`)

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/waved-meeting/src/recorder.rs`:

```rust
    #[tokio::test]
    async fn paused_audio_never_reaches_the_file() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Omni").await.unwrap();
        let t0 = Instant::now();
        rec.append_samples(&vec![0.25; 48_000]).unwrap();
        assert!(rec.pause_at(t0));
        assert!(!rec.pause_at(t0), "a second pause is a no-op");
        assert!(rec.is_paused());
        assert_eq!(rec.append_samples(&vec![0.5; 48_000]).unwrap(), 0);
        assert_eq!(rec.duration_secs(), 1, "the timer is frozen while paused");
        assert!(rec.resume_at(t0 + Duration::from_secs(5)));
        assert!(!rec.resume_at(t0 + Duration::from_secs(6)), "not paused any more");
        rec.append_samples(&vec![0.75; 48_000]).unwrap();
        assert_eq!(rec.duration_secs(), 2);

        let finalized = rec.finalize().unwrap();
        assert_eq!(
            finalized.pauses,
            vec![PauseSpan { offset_ms: 1_000, duration_ms: 5_000 }]
        );
        let mut reader = hound::WavReader::open(&finalized.audio_path).unwrap();
        let samples: Vec<i16> = reader.samples::<i16>().map(Result::unwrap).collect();
        assert_eq!(samples.len(), 96_000);
        // 0.5 * 32768 = 0x4000: the paused value must not appear.
        assert!(samples.iter().all(|&s| s != 0x4000));
    }

    #[tokio::test]
    async fn stop_while_paused_closes_the_span() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Omni").await.unwrap();
        rec.append_pre_roll(&vec![0.1; 4_800]).unwrap();
        rec.append_samples(&vec![0.1; 24_000]).unwrap();
        rec.pause_at(std::time::Instant::now());
        let finalized = rec.finalize().unwrap();
        assert_eq!(finalized.pauses.len(), 1);
        assert_eq!(finalized.pauses[0].offset_ms, 500, "the offset excludes the pre-roll");
    }
```

Append to `mod tests` in `crates/waved-meeting/src/metadata.rs`:

```rust
    #[tokio::test]
    async fn pauses_are_written_and_old_metadata_still_parses() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let mut recording = ActiveRecording::start(dir.path(), "omni").await.unwrap();
        let t0 = Instant::now();
        recording.append_samples(&vec![0.0; 48_000]).unwrap();
        recording.pause_at(t0);
        recording.resume_at(t0 + Duration::from_secs(5));
        let finalized = recording.finalize().unwrap();
        let metadata = build_and_write(&finalized, &WavedConfig::default(), "wave-test", vec![], vec![])
            .await
            .unwrap();
        assert_eq!(metadata.recording.pauses.len(), 1);
        assert_eq!(metadata.recording.pauses[0].offset_ms, 1_000);
        assert_eq!(metadata.recording.pauses[0].duration_ms, 5_000);
        let json = std::fs::read_to_string(finalized.dir.join("metadata.json")).unwrap();
        assert!(json.contains("\"pauses\""));

        let old = r#"{"started_at":null,"clock_synchronized":false,"recording_duration_ms":1,
            "audio_duration_ms":1,"pre_roll_duration_ms":0,"audio_mode":"Omni"}"#;
        let parsed: RecordingMetadata = serde_json::from_str(old).unwrap();
        assert!(parsed.pauses.is_empty());
    }
```

Also add `assert!(metadata.recording.pauses.is_empty());` to the existing `complete_metadata_uses_samples_and_never_empty_device_id`.

- [ ] **Step 2: Run and expect a failure**

Run `scripts/linux-test.sh test -p waved-meeting --lib recorder metadata`.

Expected: build errors, because `pause_at`, `PauseSpan` and `pauses` don't exist yet.

- [ ] **Step 3: Implement the recorder side**

In `crates/waved-meeting/src/recorder.rs`:

```rust
/// One pause in a take. `offset_ms` is where the pause began on the
/// recording timeline (pre-roll excluded, like bookmark offsets);
/// `duration_ms` is how long it lasted on the wall clock. Paused audio is
/// not in the file, so the file is shorter than the wall-clock take by the
/// sum of the durations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PauseSpan {
    pub offset_ms: u64,
    pub duration_ms: u64,
}
```

Add fields to `ActiveRecording` after `pre_roll_samples`:

```rust
    pauses: Vec<PauseSpan>,
    /// Set while paused: when the pause began and its recording-timeline offset.
    paused_since: Option<(std::time::Instant, u64)>,
```

Initialise them with `pauses: Vec::new(), paused_since: None,` in `start_with_integrity_epoch`. Add `pub pauses: Vec<PauseSpan>,` to `FinalizedRecording`.

Add the methods to `impl ActiveRecording`:

```rust
    pub fn is_paused(&self) -> bool {
        self.paused_since.is_some()
    }

    pub fn pauses(&self) -> &[PauseSpan] {
        &self.pauses
    }

    /// Start a pause. Returns false when already paused.
    pub fn pause_at(&mut self, now: std::time::Instant) -> bool {
        if self.paused_since.is_some() {
            return false;
        }
        let offset_ms = samples_to_ms(self.total_samples.saturating_sub(self.pre_roll_samples));
        self.paused_since = Some((now, offset_ms));
        true
    }

    /// End the current pause. Returns false when not paused.
    pub fn resume_at(&mut self, now: std::time::Instant) -> bool {
        let Some((since, offset_ms)) = self.paused_since.take() else {
            return false;
        };
        let duration_ms = now.saturating_duration_since(since).as_millis() as u64;
        self.pauses.push(PauseSpan { offset_ms, duration_ms });
        true
    }
```

In `append_samples`, add as the first statement:

```rust
        if self.is_paused() {
            // Paused audio never reaches the file (C1).
            return Ok(0);
        }
```

In `finalize`, add as the first statement (before `let writer = …`):

```rust
        if self.is_paused() {
            // Stop while paused: close the span at the moment of Stop.
            self.resume_at(std::time::Instant::now());
        }
```

…and `pauses: std::mem::take(&mut self.pauses),` in the `FinalizedRecording` literal.

- [ ] **Step 4: Implement the metadata side**

In `crates/waved-meeting/src/metadata.rs`:

```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PauseMetadata {
    pub offset_ms: u64,
    pub duration_ms: u64,
}
```

Add to `RecordingMetadata` after `audio_mode`:

```rust
    /// Pauses in the take (C1). Offsets use the recording timeline, like
    /// bookmarks. Absent in files written before milestone 2.
    #[serde(default)]
    pub pauses: Vec<PauseMetadata>,
```

In `build_and_write`, add to the `RecordingMetadata` literal:

```rust
            pauses: recording
                .pauses
                .iter()
                .map(|pause| PauseMetadata {
                    offset_ms: pause.offset_ms,
                    duration_ms: pause.duration_ms,
                })
                .collect(),
```

- [ ] **Step 5: Run and expect a pass**

Run `scripts/linux-test.sh test -p waved-meeting --lib`.

Expected: all pass.

- [ ] **Step 6: Lint and commit**

```bash
git add crates/waved-meeting/src/recorder.rs crates/waved-meeting/src/metadata.rs
git commit -m "feat(meeting): record pause spans in takes and metadata

- ActiveRecording drops samples while paused and keeps PauseSpan offsets on
  the recording timeline
- Stop while paused closes the span
- metadata.json gains recording.pauses (additive, defaults to empty)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Pause and resume in the service (C1, part 2)

**Files:**
- Modify: `crates/waved-meeting/src/diarization.rs`
- Modify: `crates/waved-meeting/src/recorder.rs`
- Modify: `crates/waved-meeting/src/lib.rs`
- Modify: `crates/waved-meeting/src/service_tests.rs`

**Interfaces:**
- Consumes: `Command::{PauseRecording, ResumeRecording}`, `ActiveRecording::{pause_at, resume_at, is_paused}`
- Produces:
  - `Diarizer::pause(&mut self) -> Option<DiarizationSegment>`
  - `Diarizer::resume(&mut self)`
  - `ActiveRecording::rebase_integrity_epoch(&mut self, epoch: u64)`
  - `MeetingState.paused`
  - free fn `paused_take_tolerates(rec: &recorder::ActiveRecording, fault: RecordingIntegrityFault) -> bool`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/waved-meeting/src/diarization.rs`:

```rust
    #[test]
    fn pause_closes_the_segment_and_resume_keeps_the_timeline_continuous() {
        use waved_audio::doa::{DoaBackend, DoaFrame};
        let voiced = |ms: u64, az: f32| DoaFrame {
            frame_idx: 0,
            timestamp_ms: ms,
            azimuth_deg: az,
            elevation_deg: 0.0,
            confidence: 0.8,
            vad: true,
            backend: DoaBackend::GccPhat,
        };
        let mut d = Diarizer::new(uuid::Uuid::nil());
        for i in 0..10u64 {
            assert!(d.on_doa_frame(&voiced(i * 21, 45.0)).is_none());
        }
        let closed = d.pause().expect("10 voiced frames make a segment");
        assert_eq!((closed.start_ms, closed.end_ms, closed.speaker_id), (0, 189, 1));
        assert!(d.on_doa_frame(&voiced(1_000, 45.0)).is_none(), "paused frames are ignored");
        d.resume();
        let resumed_at = 189 + 5_000;
        for k in 0..12u64 {
            d.on_doa_frame(&voiced(resumed_at + k * 21, 180.0));
        }
        let seg = d.finalize().expect("12 voiced frames make a segment");
        assert_eq!(seg.speaker_id, 2);
        assert_eq!(seg.start_ms, 210, "the paused 5 s are not on the timeline");
    }
```

Append to `crates/waved-meeting/src/service_tests.rs`:

```rust
fn no_pre_roll(upload_mode: UploadMode) -> SettingsState {
    SettingsState {
        pre_recording: false,
        ..settings_for(upload_mode)
    }
}

#[tokio::test]
async fn pause_and_resume_cut_the_paused_audio_and_freeze_the_timer() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(100, 0.25).await; // 1 s
    rig.send(Command::PauseRecording);
    rig.wait_for("paused", |s| s.paused).await;
    rig.feed(100, 0.5).await; // talked over the pause: dropped
    tokio::time::sleep(Duration::from_millis(1_100)).await; // a tick passes
    assert_eq!(rig.meeting.borrow().duration_secs, 1);
    rig.send(Command::ResumeRecording);
    rig.wait_for("resumed", |s| !s.paused).await;
    rig.feed(100, 0.75).await;
    rig.stop().await;
    assert_eq!(rig.wav_samples(id), 200 * 480);
    let pauses = rig.metadata(id).recording.pauses;
    assert_eq!(pauses.len(), 1);
    assert_eq!(pauses[0].offset_ms, 1_000);
    assert!(pauses[0].duration_ms >= 1_100, "{pauses:?}");
}

#[tokio::test]
async fn a_dropped_frame_while_paused_keeps_the_take() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(10, 0.25).await;
    rig.send(Command::PauseRecording);
    rig.wait_for("paused", |s| s.paused).await;
    rig.integrity.record_fault(RecordingIntegrityFault::ProcessedFrameDropped);
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    rig.integrity_tx
        .send(IntegritySignal::with_acknowledgement(
            RecordingIntegrityFault::ProcessedFrameDropped,
            ack_tx,
        ))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), ack_rx).await.unwrap().unwrap();
    assert!(rig.meeting.borrow().recording, "nothing was being written; the take survives");
    rig.send(Command::ResumeRecording);
    rig.wait_for("resumed", |s| !s.paused).await;
    rig.feed(10, 0.25).await;
    rig.stop().await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
}

#[tokio::test]
async fn a_capture_failure_while_paused_still_ends_the_take() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.send(Command::PauseRecording);
    rig.wait_for("paused", |s| s.paused).await;
    rig.integrity.record_fault(RecordingIntegrityFault::CaptureEnded);
    rig.integrity_tx
        .send(IntegritySignal::new(RecordingIntegrityFault::CaptureEnded))
        .unwrap();
    rig.wait_for("take ended", |s| !s.recording).await;
    rig.wait_status(id, Some("artifact_error")).await;
}
```

- [ ] **Step 2: Run and expect a failure**

Run `scripts/linux-test.sh test -p waved-meeting --lib pause`.

Expected:
- build error: `Diarizer::pause` doesn't exist
- after stubs, `paused` never becomes true

- [ ] **Step 3: Implement the diarizer pause**

In `crates/waved-meeting/src/diarization.rs`, add fields to `Diarizer` after `primed`:

```rust
    /// Frames are ignored while paused.
    pub(crate) paused: bool,
    /// The next frame after a resume shifts the timeline by the paused gap.
    pub(crate) resume_pending: bool,
    /// Capture timestamp of the last frame fed, for the paused gap.
    pub(crate) last_frame_ms: u32,
```

Initialise them with `paused: false, resume_pending: false, last_frame_ms: 0,` in `new`.

Replace the start of `on_doa_frame` (through the `let rel_ms = …;` line) with:

```rust
    pub fn on_doa_frame(&mut self, frame: &DoaFrame) -> Option<DiarizationSegment> {
        if self.paused {
            return None;
        }
        let ts = frame.timestamp_ms as u32;
        if !self.primed {
            self.meeting_start_ms = ts;
            self.primed = true;
        } else if self.resume_pending {
            // Paused audio is not in the file, so segment times must not
            // count the pause either: move the origin by the gap.
            let gap = ts
                .saturating_sub(self.last_frame_ms)
                .saturating_sub(PERIOD_MS);
            self.meeting_start_ms = self.meeting_start_ms.saturating_add(gap);
        }
        self.resume_pending = false;
        self.last_frame_ms = ts;
        let rel_ms = ts.saturating_sub(self.meeting_start_ms);
```

Add after `finalize`:

```rust
    /// Pause the take: close the open segment and ignore frames until `resume`.
    pub fn pause(&mut self) -> Option<DiarizationSegment> {
        self.paused = true;
        self.close_active(0)
    }

    pub fn resume(&mut self) {
        if self.paused {
            self.paused = false;
            self.resume_pending = true;
        }
    }
```

- [ ] **Step 4: Implement the recorder epoch rebase**

In `crates/waved-meeting/src/recorder.rs`, add to `impl ActiveRecording`:

```rust
    /// Accept faults that happened while paused, when nothing was written.
    pub(crate) fn rebase_integrity_epoch(&mut self, epoch: u64) {
        self.integrity_epoch = epoch;
    }
```

- [ ] **Step 5: Implement the service commands**

In `crates/waved-meeting/src/lib.rs`, add a free function near `pre_recording_active`:

```rust
/// A fault while paused only ends the take when the pipeline itself is gone:
/// frame drops and xruns lose nothing, since nothing is written while paused.
fn paused_take_tolerates(rec: &recorder::ActiveRecording, fault: RecordingIntegrityFault) -> bool {
    rec.is_paused() && !fault.requires_restart()
}
```

Add two arms to the command `match` after `Command::StopRecording => { … }`:

```rust
                                Command::PauseRecording => {
                                    let Some(rec) = active.as_mut() else { continue };
                                    if rec.pause_at(std::time::Instant::now()) {
                                        if let Some(seg) = diarizer.as_mut().and_then(|d| d.pause()) {
                                            if let Err(e) = db::insert_diarization_segment(&self.pool, &seg).await {
                                                warn!("failed to insert diarization segment at pause: {e}");
                                            }
                                        }
                                        self.meeting_tx.send_modify(|s| {
                                            s.paused = true;
                                            s.current_speaker = None;
                                        });
                                        info!("recording paused at {}s", rec.duration_secs());
                                    }
                                }
                                Command::ResumeRecording => {
                                    let Some(rec) = active.as_mut() else { continue };
                                    if rec.resume_at(std::time::Instant::now()) {
                                        if let Some(d) = diarizer.as_mut() {
                                            d.resume();
                                        }
                                        self.meeting_tx.send_modify(|s| s.paused = false);
                                        info!("recording resumed at {}s", rec.duration_secs());
                                    }
                                }
```

In the `Some(signal) = self.integrity_rx.recv() =>` branch, replace everything from `let fault = signal.fault;` through the end of the `if affects_active { … } else { … }` block with:

```rust
                    let fault = signal.fault;
                    let epoch = self.integrity.epoch();
                    let moved = active
                        .as_ref()
                        .is_some_and(|rec| epoch != rec.integrity_epoch());
                    let tolerated = moved
                        && active
                            .as_ref()
                            .is_some_and(|rec| paused_take_tolerates(rec, fault));
                    if tolerated {
                        if let Some(rec) = active.as_mut() {
                            warn!(
                                "{} while paused; nothing was being written, the take continues",
                                fault.diagnostic()
                            );
                            rec.rebase_integrity_epoch(epoch);
                        }
                    } else if moved {
                        let rec = active.take().expect("active recording disappeared");
                        diarizer = None;
                        self.abort_recording(rec, fault).await;
                        prebuffer.clear();
                        prebuffer.resume();
                    } else {
                        self.publish_idle_fault(fault);
                    }
```

(Keep the acknowledgement lines that follow.)

In `drain_before_stop`, replace the signal loop and the epoch fallback with:

```rust
        while let Ok(signal) = self.integrity_rx.try_recv() {
            if self.integrity.epoch() != rec.integrity_epoch()
                && !paused_take_tolerates(rec, signal.fault)
            {
                fault.get_or_insert(signal.fault);
            }
            if let Some(acknowledgement) = signal.acknowledgement {
                acknowledgements.push(acknowledgement);
            }
        }
        if self.integrity.epoch() != rec.integrity_epoch() {
            fault = fault.or_else(|| {
                self.integrity
                    .last_fault()
                    .filter(|last| !paused_take_tolerates(rec, *last))
            });
        }
```

In the `StartRecording` success `send_modify`, also set `s.paused = false;`.

Frames that arrive while paused already go nowhere: the active branch calls `rec.append_samples`, which returns `Ok(0)`, and the pre-roll stays frozen.

- [ ] **Step 6: Run and expect a pass**

Run `scripts/linux-test.sh test -p waved-meeting --lib`.

Expected: all pass, including the existing `integrity_signal_aborts_take_before_ack_and_excludes_upload` and `stop_drains_queued_fault_instead_of_finalizing_and_storage_fault_blocks_restart`, since an unpaused take behaves as before.

- [ ] **Step 7: Lint and commit**

```bash
git add crates/waved-meeting/src/diarization.rs crates/waved-meeting/src/recorder.rs crates/waved-meeting/src/lib.rs crates/waved-meeting/src/service_tests.rs
git commit -m "feat(meeting): handle PauseRecording and ResumeRecording

- pausing stops writing, freezes the timer and sets MeetingState.paused (C1, BUG-16)
- the diarizer closes its segment and keeps segment times continuous
- frame drops or xruns during a pause no longer destroy the take;
  a dead capture pipeline still ends it

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Marks and current speaker for the Recording screen

**Files:**
- Modify: `crates/waved-meeting/src/diarization.rs`
- Modify: `crates/waved-meeting/src/lib.rs`
- Modify: `crates/waved-meeting/src/service_tests.rs`

**Interfaces:**
- Produces:
  - `Diarizer::current_speaker(&self) -> Option<u8>`
  - writes `MeetingState.marks` (count of saved marks in the current take) and `MeetingState.current_speaker`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/waved-meeting/src/diarization.rs`:

```rust
    #[test]
    fn current_speaker_is_the_open_segment() {
        use waved_audio::doa::{DoaBackend, DoaFrame};
        let frame = |ms: u64, vad: bool| DoaFrame {
            frame_idx: 0,
            timestamp_ms: ms,
            azimuth_deg: 45.0,
            elevation_deg: 0.0,
            confidence: if vad { 0.8 } else { 0.0 },
            vad,
            backend: DoaBackend::GccPhat,
        };
        let mut d = Diarizer::new(uuid::Uuid::nil());
        assert_eq!(d.current_speaker(), None);
        d.on_doa_frame(&frame(0, true));
        assert_eq!(d.current_speaker(), Some(1));
        d.on_doa_frame(&frame(600, false)); // > 500 ms of silence closes it
        assert_eq!(d.current_speaker(), None);
    }
```

Append to `crates/waved-meeting/src/service_tests.rs`:

```rust
fn voiced(ms: u64, az: f32) -> DoaFrame {
    DoaFrame {
        frame_idx: 0,
        timestamp_ms: ms,
        azimuth_deg: az,
        elevation_deg: 0.0,
        confidence: 0.8,
        vad: true,
        backend: waved_audio::doa::DoaBackend::GccPhat,
    }
}

#[tokio::test]
async fn marks_count_per_take_and_ignore_paused_marks() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    rig.start().await;
    rig.feed(10, 0.1).await;
    rig.send(Command::AddBookmark);
    rig.wait_for("one mark", |s| s.marks == 1).await;
    rig.send(Command::PauseRecording);
    rig.wait_for("paused", |s| s.paused).await;
    rig.send(Command::AddBookmark);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.meeting.borrow().marks, 1, "Mark is dimmed while paused");
    rig.send(Command::ResumeRecording);
    rig.wait_for("resumed", |s| !s.paused).await;
    rig.stop().await;
    rig.start().await;
    assert_eq!(rig.meeting.borrow().marks, 0, "a new take starts at zero");
}

#[tokio::test]
async fn current_speaker_follows_the_talker_and_clears_at_stop() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    rig.start().await;
    for i in 0..12u64 {
        rig.doa.send(voiced(i * 21, 45.0)).unwrap();
    }
    rig.wait_for("speaker 1", |s| s.current_speaker == Some(1)).await;
    rig.stop().await;
    assert_eq!(rig.meeting.borrow().current_speaker, None);
}
```

- [ ] **Step 2: Run and expect a failure**

Run `scripts/linux-test.sh test -p waved-meeting --lib current_speaker marks_count`.

Expected:
- build error: `current_speaker` doesn't exist
- after a stub, `marks` stays 0

- [ ] **Step 3: Implement**

In `crates/waved-meeting/src/diarization.rs`:

```rust
    /// The speaker of the open segment ("Speaker n talking"), if any.
    pub fn current_speaker(&self) -> Option<u8> {
        self.active
            .as_ref()
            .map(|segment| segment.speaker_id.min(u32::from(u8::MAX)) as u8)
    }
```

In `crates/waved-meeting/src/lib.rs`:

1. Add to `impl MeetingService`:

```rust
    fn publish_speaker(&self, speaker: Option<u8>) {
        self.meeting_tx.send_if_modified(|state| {
            if state.current_speaker == speaker {
                return false;
            }
            state.current_speaker = speaker;
            true
        });
    }
```

2. In the DoA branch, inside `Ok(frame) => { … }`, after the `if let Some(diar) = diarizer.as_mut() { … }` block, add:

```rust
                            self.publish_speaker(diarizer.as_ref().and_then(|d| d.current_speaker()));
```

3. In the `StartRecording` success `send_modify`, also set `s.marks = 0;` and `s.current_speaker = None;`.

4. Replace the `Command::AddBookmark` arm with:

```rust
                                Command::AddBookmark => {
                                    if let Some(ref rec) = active {
                                        if rec.is_paused() {
                                            info!("bookmark ignored while paused");
                                            continue;
                                        }
                                        let ts = rec.duration_secs();
                                        match db::add_bookmark(&self.pool, &rec.id, ts).await {
                                            Ok(()) => {
                                                self.meeting_tx
                                                    .send_modify(|s| s.marks = s.marks.saturating_add(1));
                                                info!("bookmark at {}s", ts);
                                            }
                                            Err(error) => warn!("bookmark at {ts}s not saved: {error}"),
                                        }
                                    }
                                }
```

5. Add `state.current_speaker = None;` next to every `state.recording = false;` in this file. There are five: three in `RecordingFinalizer::stop_recording`, one in `RecordingFinalizer::mark_recording_fault` and one in `MeetingService::mark_recording_fault`.

- [ ] **Step 4: Run and expect a pass**

Run `scripts/linux-test.sh test -p waved-meeting --lib`.

Expected: all pass.

- [ ] **Step 5: Lint and commit**

```bash
git add crates/waved-meeting/src/diarization.rs crates/waved-meeting/src/lib.rs crates/waved-meeting/src/service_tests.rs
git commit -m "feat(meeting): publish marks and the current speaker

- MeetingState.marks counts saved marks in the take; marks are refused while paused
- MeetingState.current_speaker follows the open diarization segment and clears at stop

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Auto-Keep an unanswered "Send this recording?" (C7, part 1)

**Files:**
- Modify: `crates/waved-meeting/src/db.rs`
- Modify: `crates/waved-meeting/src/lib.rs`
- Modify: `crates/waved-meeting/src/service_tests.rs`

**Interfaces:**
- Consumes: `SettingsState.ask_timeout_s` (snapshot at Stop)
- Produces:
  - `StopPolicy.ask_timeout_s: u32`
  - `pub(crate) fn seconds_left(deadline: tokio::time::Instant, now: tokio::time::Instant) -> u32` (rounds up)
  - `pub async fn db::keep_stale_decisions(pool: &SqlitePool) -> Result<u64, sqlx::Error>`
  - writes `MeetingState.ask_deadline_secs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/waved-meeting/src/service_tests.rs`:

```rust
#[test]
fn seconds_left_rounds_up_and_stops_at_zero() {
    let now = tokio::time::Instant::now();
    assert_eq!(seconds_left(now + Duration::from_millis(59_001), now), 60);
    assert_eq!(seconds_left(now + Duration::from_secs(60), now), 60);
    assert_eq!(seconds_left(now + Duration::from_millis(1), now), 1);
    assert_eq!(seconds_left(now, now), 0);
    assert_eq!(seconds_left(now, now + Duration::from_secs(5)), 0);
}

fn ask_after(seconds: u32) -> SettingsState {
    SettingsState {
        ask_timeout_s: seconds,
        ..settings_for(UploadMode::Ask)
    }
}

#[tokio::test]
async fn unanswered_decision_is_kept_after_the_timeout() {
    let rig = start_rig(quiet_config(UploadMode::Ask), ask_after(1)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    assert_eq!(rig.meeting.borrow().ask_deadline_secs, Some(1));
    rig.wait_for("kept", |s| s.pending_decision.is_none()).await;
    assert_eq!(rig.meeting.borrow().ask_deadline_secs, None);
    rig.wait_status(id, Some("local_only")).await;
}

#[tokio::test]
async fn answer_before_timeout_cancels_auto_keep() {
    let rig = start_rig(quiet_config(UploadMode::Ask), ask_after(2)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    rig.send(Command::SendRecording(id));
    rig.wait_for("answered", |s| s.pending_decision.is_none()).await;
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("queued"), "Send must win");
    assert_eq!(rig.meeting.borrow().ask_deadline_secs, None);
}

#[tokio::test]
async fn a_decision_left_by_a_restart_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let id = Uuid::new_v4();
    let dir = tmp.path().join("rec").join(id.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("audio.wav"), b"RIFF").unwrap();
    std::fs::write(dir.join("metadata.json"), b"{}").unwrap();
    let pool = db::init_pool(tmp.path().join("t.db").to_str().unwrap()).await.unwrap();
    db::insert_meeting(&pool, &id, "Omni", &dir.to_string_lossy()).await.unwrap();
    db::finish_meeting(&pool, &id, 3, MeetingStatus::AwaitingDecision).await.unwrap();
    pool.close().await;

    let rig = start_rig_in(tmp, quiet_config(UploadMode::Ask), settings_for(UploadMode::Ask)).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
    assert!(rig.meeting.borrow().pending_decision.is_none());
}
```

- [ ] **Step 2: Run and expect a failure**

Run `scripts/linux-test.sh test -p waved-meeting --lib seconds_left decision`.

Expected:
- build error: `seconds_left` doesn't exist
- after a stub, the pending decision never clears by itself

- [ ] **Step 3: Implement the database part**

Append to `crates/waved-meeting/src/db.rs` (before `fn truncate_diagnostic`):

```rust
/// Recordings a restart left waiting for "Send this recording?". The
/// question is gone after a restart, so they are kept on Wave, the D4 default.
pub async fn keep_stale_decisions(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("UPDATE meetings SET upload_status = ? WHERE upload_status = ?")
        .bind(MeetingStatus::LocalOnly.as_str())
        .bind(MeetingStatus::AwaitingDecision.as_str())
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}
```

In `MeetingService::new_with_integrity`, right after the `reconcile::run` block:

```rust
        match db::keep_stale_decisions(&pool).await {
            Ok(0) => {}
            Ok(kept) => info!("kept {kept} recording(s) whose decision was interrupted by a restart"),
            Err(error) => warn!("failed to keep interrupted decisions: {error}"),
        }
```

- [ ] **Step 4: Implement the timer**

In `crates/waved-meeting/src/lib.rs`:

1. Extend `StopPolicy` with `ask_timeout_s: u32,` and `stop_policy()` with `ask_timeout_s: self.settings_rx.borrow().ask_timeout_s,`. Write it as one borrow:

```rust
    fn stop_policy(&self) -> StopPolicy {
        let settings = self.settings_rx.borrow();
        StopPolicy {
            upload_mode: settings.upload_mode,
            ask_timeout_s: settings.ask_timeout_s,
        }
    }
```

2. Add a free function:

```rust
/// Whole seconds left before `deadline`, rounded up, so the countdown shows
/// 60 for the whole first second.
pub(crate) fn seconds_left(deadline: tokio::time::Instant, now: tokio::time::Instant) -> u32 {
    let millis = deadline.saturating_duration_since(now).as_millis();
    millis.div_ceil(1_000).min(u128::from(u32::MAX)) as u32
}
```

3. In `RecordingFinalizer::stop_recording`, right after `let decision = …;`, add:

```rust
        let ask_deadline_secs = decision.as_ref().map(|_| policy.ask_timeout_s);
```

…and in the final `send_modify`, set `state.ask_deadline_secs = ask_deadline_secs;`.

4. Make `clear_pending` clear the countdown too:

```rust
    fn clear_pending(&self) {
        self.meeting_tx.send_modify(|s| {
            s.pending_decision = None;
            s.ask_deadline_secs = None;
        });
    }
```

5. Add to `impl MeetingService`:

```rust
    fn arm_ask_deadline(&self) -> Option<tokio::time::Instant> {
        let state = self.meeting_tx.borrow();
        state.pending_decision.as_ref()?;
        let seconds = state.ask_deadline_secs?;
        Some(tokio::time::Instant::now() + Duration::from_secs(u64::from(seconds)))
    }

    /// Nobody answered "Send this recording?": keep it on Wave (D4).
    async fn keep_unanswered(&self) {
        let Some(id) = self.meeting_tx.borrow().pending_decision.as_ref().map(|d| d.id) else {
            return;
        };
        info!("no answer to \"Send this recording?\" for {id}; keeping it on Wave");
        self.handle_save_recording(id).await;
    }
```

6. In `run`, declare `let mut ask_deadline: Option<tokio::time::Instant> = None;` before the loop. In the `StopRecording` arm, after `upload_wake.notify_one();` in the successful-finalization branch, add `ask_deadline = self.arm_ask_deadline();`.

7. Replace the tick branch with:

```rust
                _ = tick.tick(), if active.is_some() || ask_deadline.is_some() => {
                    if let Some(rec) = active.as_ref() {
                        let secs = rec.duration_secs();
                        self.meeting_tx.send_modify(|s| {
                            s.duration_secs = secs;
                        });
                    }
                    if ask_deadline.is_some() && self.meeting_tx.borrow().pending_decision.is_none() {
                        // Answered (Save / Send / Discard / Delete) since the last tick.
                        ask_deadline = None;
                    }
                    if let Some(deadline) = ask_deadline {
                        match seconds_left(deadline, tokio::time::Instant::now()) {
                            0 => {
                                ask_deadline = None;
                                self.keep_unanswered().await;
                            }
                            left => {
                                self.meeting_tx.send_if_modified(|s| {
                                    if s.ask_deadline_secs == Some(left) {
                                        return false;
                                    }
                                    s.ask_deadline_secs = Some(left);
                                    true
                                });
                            }
                        }
                    }
                }
```

- [ ] **Step 5: Run and expect a pass**

```bash
set -o pipefail
scripts/linux-test.sh test -p waved-meeting --lib 2>&1 | grep -E 'test result|FAILED|panicked'
scripts/linux-test.sh test -p waved-meeting --test recording_decision 2>&1 | grep -E 'test result'
```

Expected:
- `--lib` all pass.
- `recording_decision` shows `ok. 9 passed`. Its default `ask_timeout_s` is 60 s, longer than any of those tests.

- [ ] **Step 6: Lint and commit**

```bash
git add crates/waved-meeting/src/db.rs crates/waved-meeting/src/lib.rs crates/waved-meeting/src/service_tests.rs
git commit -m "feat(meeting): keep an unanswered recording after the ask timeout

- MeetingState.ask_deadline_secs counts down once a second (D4, C7)
- at zero the take is kept local-only; any answer before that wins
- decisions interrupted by a restart are kept at boot

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Audit-safe Delete (C7, part 2)

**Files:**
- Modify: `crates/waved-meeting/src/model.rs`
- Modify: `crates/waved-meeting/src/db.rs`
- Create: `crates/waved-meeting/src/removal.rs`
- Modify: `crates/waved-meeting/src/lib.rs`
- Modify: `crates/waved-meeting/src/service_tests.rs`

**Interfaces:**
- Consumes: `Command::{DeleteRecording, DiscardRecording}`
- Produces:
  - `MeetingStatus::Deleting` (`"deleting"`)
  - `pub enum db::RemovalClaim { Claimed { previous_status: String }, Uploading, NotFound }`
  - `pub async fn db::claim_for_removal(pool: &SqlitePool, id: &Uuid) -> Result<RemovalClaim, sqlx::Error>`
  - `pub async fn db::ids_with_status(pool: &SqlitePool, status: MeetingStatus) -> Result<Vec<Uuid>, sqlx::Error>`
  - `pub(crate) enum removal::RemoveOutcome { Removed { previous_status: String }, Uploading, NotFound }`
  - `pub(crate) async fn removal::remove_recording(pool: &SqlitePool, recordings_dir: &Path, id: Uuid) -> std::io::Result<RemoveOutcome>`
  - `pub(crate) async fn removal::finish_interrupted(pool: &SqlitePool, recordings_dir: &Path) -> u32`

- [ ] **Step 1: Write the failing tests**

Append to `crates/waved-meeting/src/service_tests.rs`:

```rust
async fn wait_gone(path: &std::path::Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while path.exists() {
        assert!(tokio::time::Instant::now() < deadline, "{} still exists", path.display());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn delete_removes_a_kept_recording() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::AddBookmark);
    rig.wait_for("mark", |s| s.marks == 1).await;
    rig.stop().await;
    rig.send(Command::DeleteRecording(id));
    wait_gone(&rig.dir(id)).await;
    rig.wait_status(id, None).await;
    let pool = rig.pool().await;
    let (bookmarks,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM bookmarks WHERE meeting_id = ?")
        .bind(id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(bookmarks, 0);
    let leftovers: Vec<_> = std::fs::read_dir(&rig.recordings_dir).unwrap().collect();
    assert!(leftovers.is_empty(), "no .deleted-* directory left behind");
}

#[tokio::test]
async fn delete_is_refused_while_uploading() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    db::set_status(&rig.pool().await, &id, MeetingStatus::Uploading).await.unwrap();
    rig.send(Command::DeleteRecording(id));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rig.dir(id).join("audio.wav").exists());
    assert_eq!(rig.status(id).await.as_deref(), Some("uploading"));
}

#[tokio::test]
async fn delete_of_the_pending_recording_discards_it() {
    let rig = start_rig(quiet_config(UploadMode::Ask), settings_for(UploadMode::Ask)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    rig.send(Command::DeleteRecording(id));
    rig.wait_for("cleared", |s| s.pending_decision.is_none() && s.ask_deadline_secs.is_none())
        .await;
    wait_gone(&rig.dir(id)).await;
}

#[tokio::test]
async fn delete_of_the_running_recording_is_ignored() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.send(Command::DeleteRecording(id));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rig.meeting.borrow().recording);
    assert!(rig.dir(id).exists());
}

#[tokio::test]
async fn interrupted_delete_finishes_on_boot() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = tmp.path().join("rec");
    let pool = db::init_pool(tmp.path().join("t.db").to_str().unwrap()).await.unwrap();
    // Power cut after the rename: the row still says local_only.
    let renamed = Uuid::new_v4();
    let trash = rec.join(format!("{}{renamed}", removal::DELETED_PREFIX));
    std::fs::create_dir_all(&trash).unwrap();
    std::fs::write(trash.join("audio.wav"), b"RIFF").unwrap();
    std::fs::write(trash.join("metadata.json"), b"{}").unwrap();
    db::insert_meeting(&pool, &renamed, "Omni", &rec.join(renamed.to_string()).to_string_lossy())
        .await
        .unwrap();
    db::set_status(&pool, &renamed, MeetingStatus::LocalOnly).await.unwrap();
    // Power cut right after the claim: the directory is intact.
    let claimed = Uuid::new_v4();
    let dir = rec.join(claimed.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("audio.wav"), b"RIFF").unwrap();
    std::fs::write(dir.join("metadata.json"), b"{}").unwrap();
    db::insert_meeting(&pool, &claimed, "Omni", &dir.to_string_lossy()).await.unwrap();
    db::set_status(&pool, &claimed, MeetingStatus::Deleting).await.unwrap();
    pool.close().await;

    let rig = start_rig_in(tmp, quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    assert!(!trash.exists());
    assert!(!rig.dir(renamed).exists(), "reconciliation must not bring it back");
    assert!(!rig.dir(claimed).exists());
    assert_eq!(rig.status(renamed).await, None);
    assert_eq!(rig.status(claimed).await, None);
}
```

- [ ] **Step 2: Run and expect a failure**

Run `scripts/linux-test.sh test -p waved-meeting --lib delete`.

Expected: build errors, because `removal` and `MeetingStatus::Deleting` don't exist yet.

- [ ] **Step 3: Add the status**

In `crates/waved-meeting/src/model.rs`:
- add `Deleting,` after `ArtifactError,`
- add `Self::Deleting => "deleting",` to `as_str`
- add `"deleting" => Self::Deleting,` to `parse`

- [ ] **Step 4: Add the database functions**

Append to `crates/waved-meeting/src/db.rs` (before `fn truncate_diagnostic`):

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum RemovalClaim {
    Claimed { previous_status: String },
    Uploading,
    NotFound,
}

/// Mark a recording `deleting` unless it is uploading right now. The worker
/// only claims queued and retry_wait rows, so a claimed row is never picked
/// up for delivery while its files go away.
pub async fn claim_for_removal(pool: &SqlitePool, id: &Uuid) -> Result<RemovalClaim, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row: Option<(String,)> = sqlx::query_as("SELECT upload_status FROM meetings WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
    let claim = match row {
        None => RemovalClaim::NotFound,
        Some((status,)) if status == MeetingStatus::Uploading.as_str() => RemovalClaim::Uploading,
        Some((previous_status,)) => {
            sqlx::query("UPDATE meetings SET upload_status = ? WHERE id = ?")
                .bind(MeetingStatus::Deleting.as_str())
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            RemovalClaim::Claimed { previous_status }
        }
    };
    tx.commit().await?;
    Ok(claim)
}

pub async fn ids_with_status(pool: &SqlitePool, status: MeetingStatus) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT id FROM meetings WHERE upload_status = ?")
        .bind(status.as_str())
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().filter_map(|(id,)| id.parse().ok()).collect())
}
```

- [ ] **Step 5: Create the removal module**

`crates/waved-meeting/src/removal.rs`:

```rust
//! Audit-safe removal of a recording (Files → Delete, After stop → Delete).
//!
//! The order guarantees that a power cut at any step never brings a deleted
//! recording back and never leaves one half deleted:
//! 1. claim the row (`deleting`). The upload worker never picks such a row,
//!    and rows that are uploading right now are refused;
//! 2. rename `<id>/` to `.deleted-<id>/` and sync the parent directory.
//!    Reconciliation never imports it because the name is not a UUID;
//! 3. delete the database rows (meeting, bookmarks, speaker segments);
//! 4. remove the renamed directory.
//!
//! `finish_interrupted` completes steps 2–4 at boot for anything left over.

use crate::db::{self, RemovalClaim};
use crate::model::MeetingStatus;
use sqlx::SqlitePool;
use std::path::Path;
use tracing::{info, warn};
use uuid::Uuid;

pub(crate) const DELETED_PREFIX: &str = ".deleted-";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoveOutcome {
    Removed { previous_status: String },
    Uploading,
    NotFound,
}

pub(crate) async fn remove_recording(
    pool: &SqlitePool,
    recordings_dir: &Path,
    id: Uuid,
) -> std::io::Result<RemoveOutcome> {
    let previous_status = match db::claim_for_removal(pool, &id)
        .await
        .map_err(std::io::Error::other)?
    {
        RemovalClaim::Claimed { previous_status } => previous_status,
        RemovalClaim::Uploading => return Ok(RemoveOutcome::Uploading),
        RemovalClaim::NotFound => return Ok(RemoveOutcome::NotFound),
    };
    finish_one(pool, recordings_dir, id).await?;
    Ok(RemoveOutcome::Removed { previous_status })
}

async fn finish_one(pool: &SqlitePool, recordings_dir: &Path, id: Uuid) -> std::io::Result<()> {
    let dir = recordings_dir.join(id.to_string());
    let trash = recordings_dir.join(format!("{DELETED_PREFIX}{id}"));
    match tokio::fs::rename(&dir, &trash).await {
        Ok(()) => std::fs::File::open(recordings_dir)?.sync_all()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    db::delete_meeting_cascade(pool, &id)
        .await
        .map_err(std::io::Error::other)?;
    match tokio::fs::remove_dir_all(&trash).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Finish deletes a power cut interrupted: rows still marked `deleting`
/// and `.deleted-<id>` directories. Runs before reconciliation.
pub(crate) async fn finish_interrupted(pool: &SqlitePool, recordings_dir: &Path) -> u32 {
    let mut ids = match db::ids_with_status(pool, MeetingStatus::Deleting).await {
        Ok(ids) => ids,
        Err(error) => {
            warn!("cannot list interrupted deletes: {error}");
            Vec::new()
        }
    };
    if let Ok(mut entries) = tokio::fs::read_dir(recordings_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(rest) = name.to_str().and_then(|name| name.strip_prefix(DELETED_PREFIX)) else {
                continue;
            };
            match rest.parse::<Uuid>() {
                Ok(id) if !ids.contains(&id) => ids.push(id),
                Ok(_) => {}
                Err(_) => warn!("leaving unexpected {} in place", entry.path().display()),
            }
        }
    }
    let mut finished = 0;
    for id in ids {
        match finish_one(pool, recordings_dir, id).await {
            Ok(()) => {
                finished += 1;
                info!("audit: finished the interrupted delete of recording {id}");
            }
            Err(error) => warn!("could not finish deleting recording {id}: {error}"),
        }
    }
    finished
}
```

Add `mod removal;` after `pub mod reconcile;` in `lib.rs`.

- [ ] **Step 6: Wire the service**

In `crates/waved-meeting/src/lib.rs`:

1. In `new_with_integrity`, between `let pool = db::init_pool(db_path).await?;` and the `reconcile::run` block:

```rust
        let finished = removal::finish_interrupted(&pool, &recordings_dir).await;
        if finished > 0 {
            info!("finished {finished} interrupted delete(s)");
        }
```

2. Add to `impl MeetingService`:

```rust
    async fn handle_delete_recording(&self, id: Uuid) {
        let was_pending = self
            .meeting_tx
            .borrow()
            .pending_decision
            .as_ref()
            .is_some_and(|decision| decision.id == id);
        match removal::remove_recording(&self.pool, &self.recordings_dir, id).await {
            Ok(removal::RemoveOutcome::Removed { previous_status }) => {
                info!("audit: recording {id} deleted on request (was {previous_status})");
                if was_pending {
                    self.clear_pending();
                }
            }
            Ok(removal::RemoveOutcome::Uploading) => {
                warn!("delete refused: recording {id} is uploading right now")
            }
            Ok(removal::RemoveOutcome::NotFound) => warn!("delete ignored: no recording {id}"),
            Err(error) => warn!("delete of recording {id} failed: {error}"),
        }
    }
```

3. Replace the body of `handle_discard_recording` with:

```rust
    async fn handle_discard_recording(&self, id: Uuid) {
        if self.validate_pending(&id).is_none() {
            return;
        }
        self.handle_delete_recording(id).await;
        // The question is answered even if the files could not be removed.
        self.clear_pending();
    }
```

4. Add a command arm after `Command::DiscardRecording(id) => { … }`:

```rust
                                Command::DeleteRecording(id) => {
                                    if active.as_ref().is_some_and(|rec| rec.id == id) {
                                        warn!("delete ignored: recording {id} is still running");
                                        continue;
                                    }
                                    self.handle_delete_recording(id).await;
                                }
```

`dir_for` is now unused. Delete it.

- [ ] **Step 7: Run and expect a pass**

```bash
set -o pipefail
scripts/linux-test.sh test -p waved-meeting --lib 2>&1 | grep -E 'test result|FAILED|panicked'
scripts/linux-test.sh test -p waved-meeting --test recording_decision 2>&1 | grep -E 'test result'
```

Expected: both all pass (`discard_removes_file_and_db_row` now goes through the safe path).

- [ ] **Step 8: Lint and commit**

```bash
git add crates/waved-meeting/src/model.rs crates/waved-meeting/src/db.rs crates/waved-meeting/src/removal.rs crates/waved-meeting/src/lib.rs crates/waved-meeting/src/service_tests.rs
git commit -m "feat(meeting): delete recordings safely from Files

- DeleteRecording removes any kept, failed or pending recording (C7)
- claim -> rename to .deleted-<id> -> drop rows -> remove, finished at boot
  after a power cut, so reconciliation never resurrects a deleted take
- refused while that recording uploads or is still running; audit log line per delete
- DiscardRecording uses the same path

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Resend kept or failed recordings (C7, part 3)

**Files:**
- Modify: `crates/waved-meeting/src/db.rs`
- Modify: `crates/waved-meeting/src/lib.rs`
- Modify: `crates/waved-meeting/src/service_tests.rs`

**Interfaces:**
- Consumes: `Command::{SendRecording, RetryUpload}`
- Produces: `pub async fn db::requeue_for_delivery(pool: &SqlitePool, id: &Uuid) -> Result<bool, sqlx::Error>`. It returns `true` when the row was `local_only`, `retry_wait` or `configuration_error` and is now `queued` with a fresh attempt cycle.

- [ ] **Step 1: Write the failing tests**

Append to `crates/waved-meeting/src/service_tests.rs`:

```rust
#[tokio::test]
async fn resend_only_requeues_kept_or_failed_recordings() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = db::init_pool(tmp.path().join("t.db").to_str().unwrap()).await.unwrap();
    use MeetingStatus as M;
    for status in [M::LocalOnly, M::RetryWait, M::ConfigurationError, M::Uploaded, M::ArtifactError, M::Uploading, M::AwaitingDecision] {
        let id = Uuid::new_v4();
        db::insert_meeting(&pool, &id, "Omni", "/nonexistent").await.unwrap();
        db::set_status(&pool, &id, status).await.unwrap();
        let requeued = db::requeue_for_delivery(&pool, &id).await.unwrap();
        let expected = matches!(status, M::LocalOnly | M::RetryWait | M::ConfigurationError);
        assert_eq!(requeued, expected, "{status:?}");
        let want = if expected { "queued" } else { status.as_str() };
        assert_eq!(Rig::status_in(&pool, id).await.as_deref(), Some(want), "{status:?}");
    }
}

#[tokio::test]
async fn send_from_files_queues_a_kept_recording() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
    rig.send(Command::SendRecording(id));
    rig.wait_status(id, Some("queued")).await;
}

#[tokio::test]
async fn retry_resets_a_failed_recording_and_ignores_others() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    let pool = rig.pool().await;
    db::set_status(&pool, &id, MeetingStatus::Uploaded).await.unwrap();
    rig.send(Command::RetryUpload(id));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("uploaded"), "already delivered");
    db::set_status(&pool, &id, MeetingStatus::RetryWait).await.unwrap();
    rig.send(Command::RetryUpload(id));
    rig.wait_status(id, Some("queued")).await;
}
```

- [ ] **Step 2: Run and expect a failure**

Run `scripts/linux-test.sh test -p waved-meeting --lib resend send_from_files retry_resets`.

Expected:
- build error: `requeue_for_delivery` doesn't exist
- after a stub, `retry_resets…` fails because `RetryUpload` re-queues the uploaded row

- [ ] **Step 3: Implement**

Append to `crates/waved-meeting/src/db.rs`:

```rust
/// Queue a kept or failed recording for delivery again ("Send" / "Send
/// again" in Files). Uploaded, damaged, pending and in-flight recordings are
/// left alone. Returns true when the row was queued.
pub async fn requeue_for_delivery(pool: &SqlitePool, id: &Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE meetings SET upload_status = ?, attempt_cycle = 0, \
         last_error = NULL, last_status_code = NULL, next_attempt_at_ms = NULL \
         WHERE id = ? AND upload_status IN ('local_only', 'retry_wait', 'configuration_error')",
    )
    .bind(MeetingStatus::Queued.as_str())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}
```

In `crates/waved-meeting/src/lib.rs`, add to `impl MeetingService`:

```rust
    async fn requeue(&self, id: Uuid, why: &str) -> bool {
        match db::requeue_for_delivery(&self.pool, &id).await {
            Ok(true) => {
                info!("{why}: {id} queued for delivery");
                true
            }
            Ok(false) => {
                warn!("{why} ignored: {id} is not a kept or failed recording");
                false
            }
            Err(error) => {
                warn!("{why}: db update failed for {id}: {error}");
                false
            }
        }
    }
```

Replace the `Command::SendRecording(id)` and `Command::RetryUpload(id)` arms with:

```rust
                                Command::SendRecording(id) => {
                                    let pending = self
                                        .meeting_tx
                                        .borrow()
                                        .pending_decision
                                        .as_ref()
                                        .is_some_and(|decision| decision.id == id);
                                    if pending {
                                        self.handle_send_recording(id).await;
                                        upload_wake.notify_one();
                                    } else if self.requeue(id, "send").await {
                                        upload_wake.notify_one();
                                    }
                                }
                                Command::RetryUpload(id) => {
                                    if self.requeue(id, "retry").await {
                                        upload_wake.notify_one();
                                    }
                                }
```

`db::reset_attempt_cycle` now has no caller. Leave it in `db.rs` (it is `pub` and harmless). If clippy reports it as dead code, delete it.

- [ ] **Step 4: Run and expect a pass**

```bash
set -o pipefail
scripts/linux-test.sh test -p waved-meeting --lib 2>&1 | grep -E 'test result|FAILED|panicked'
scripts/linux-test.sh test -p waved-meeting --test recording_decision 2>&1 | grep -E 'test result'
```

Expected: both all pass.

- [ ] **Step 5: Lint and commit**

```bash
git add crates/waved-meeting/src/db.rs crates/waved-meeting/src/lib.rs crates/waved-meeting/src/service_tests.rs
git commit -m "feat(meeting): resend kept or failed recordings

- SendRecording(id) queues the pending take or, from Files, a kept or failed one (C7)
- RetryUpload(id) only re-queues kept or failed recordings, never uploaded or damaged ones

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Publish the Files list (`FilesState`)

**Files:**
- Modify: `crates/waved-meeting/src/db.rs`
- Create: `crates/waved-meeting/src/files.rs`
- Modify: `crates/waved-meeting/src/lib.rs`
- Modify: `crates/waved-meeting/src/service_tests.rs`

**Interfaces:**
- Consumes: `waved_state::{FilesState, RecordingSummary, RecordingStatus}`, `StorageConfig.minimum_free_mb`, `MeetingState.storage_blocked`
- Produces:
  - `pub struct db::RecordingRow { pub id: Uuid, pub start_time: String, pub duration_secs: u32, pub status: String, pub attempt_cycle: u32, pub last_status_code: Option<i64>, pub dir: String, pub marks: u32 }`
  - `pub async fn db::recording_rows(pool: &SqlitePool) -> Result<Vec<RecordingRow>, sqlx::Error>` (newest first)
  - `files::{summary_status, plain_reason, started_at, snapshot}`
  - `impl MeetingService { pub fn with_files_publisher(self, files: Arc<watch::Sender<FilesState>>) -> Self }`

- [ ] **Step 1: Write the failing tests**

Create `crates/waved-meeting/src/files.rs` with only its tests for now:

```rust
//! The recordings list for Files (D1–D3).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MeetingStatus as M;
    use waved_state::RecordingStatus as R;

    #[test]
    fn statuses_map_to_what_files_shows() {
        assert_eq!(summary_status(M::Recording, 0, 5), None);
        assert_eq!(summary_status(M::Finalized, 0, 5), None);
        assert_eq!(summary_status(M::Deleting, 0, 5), None);
        assert_eq!(summary_status(M::AwaitingDecision, 0, 5), Some(R::AwaitingDecision));
        assert_eq!(summary_status(M::LocalOnly, 0, 5), Some(R::LocalOnly));
        assert_eq!(summary_status(M::Queued, 0, 5), Some(R::Waiting));
        assert_eq!(summary_status(M::Uploading, 1, 5), Some(R::Uploading));
        assert_eq!(summary_status(M::RetryWait, 4, 5), Some(R::Waiting));
        assert_eq!(summary_status(M::RetryWait, 5, 5), Some(R::Failed));
        assert_eq!(summary_status(M::ConfigurationError, 1, 5), Some(R::Failed));
        assert_eq!(summary_status(M::Uploaded, 1, 5), Some(R::Uploaded));
        for damaged in [M::MissingArtifact, M::LegacyUnsupported, M::ArtifactError] {
            assert_eq!(summary_status(damaged, 0, 5), Some(R::Damaged), "{damaged:?}");
        }
    }

    #[test]
    fn only_rows_that_need_attention_get_a_reason() {
        assert_eq!(plain_reason(M::LocalOnly, R::LocalOnly, None), None);
        assert_eq!(plain_reason(M::RetryWait, R::Waiting, None), None);
        assert_eq!(
            plain_reason(M::ConfigurationError, R::Failed, Some(401)).as_deref(),
            Some("The server refused it (HTTP 401). Check the upload settings.")
        );
        assert_eq!(
            plain_reason(M::RetryWait, R::Failed, None).as_deref(),
            Some("The server couldn't be reached. Send it again when the network is back.")
        );
        assert_eq!(
            plain_reason(M::ArtifactError, R::Damaged, None).as_deref(),
            Some("The recording was interrupted and is incomplete.")
        );
        for status in [M::ConfigurationError, M::RetryWait, M::MissingArtifact, M::LegacyUnsupported, M::ArtifactError] {
            let summary = summary_status(status, 9, 5).unwrap();
            let reason = plain_reason(status, summary, None).unwrap();
            assert!(!reason.contains('…') && reason.ends_with('.'), "{reason}");
        }
    }

    #[test]
    fn start_times_need_a_synchronized_clock() {
        assert_eq!(
            started_at("2026-10-05 14:02:00").as_deref(),
            Some("2026-10-05T14:02:00+00:00")
        );
        assert_eq!(started_at("1970-01-01 00:00:12"), None, "clock not set yet");
        assert_eq!(started_at("garbage"), None);
    }
}
```

Add `mod files;` after `pub mod diarization;` in `lib.rs`.

Append to `crates/waved-meeting/src/service_tests.rs`:

```rust
async fn wait_files(rig: &Rig, what: &str, predicate: impl Fn(&FilesState) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(7);
    while !predicate(&rig.files.borrow()) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}: {:?}",
            *rig.files.borrow()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn files_list_tracks_takes_marks_and_deletes() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(10, 0.1).await;
    rig.send(Command::AddBookmark);
    rig.wait_for("mark", |s| s.marks == 1).await;
    rig.stop().await;
    wait_files(&rig, "one recording", |f| f.recordings.len() == 1).await;
    let files = rig.files.borrow().clone();
    let entry = &files.recordings[0];
    assert_eq!(entry.id, id);
    assert_eq!(entry.status, waved_state::RecordingStatus::LocalOnly);
    assert_eq!(entry.marks, 1);
    assert!(entry.size_bytes > 44, "WAV header plus audio");
    assert!(entry.last_error.is_none());
    assert!(files.free_bytes > 0);
    assert!(!files.low_space);
    rig.send(Command::DeleteRecording(id));
    wait_files(&rig, "empty list", |f| f.recordings.is_empty()).await;
}
```

- [ ] **Step 2: Run and expect a failure**

Run `scripts/linux-test.sh test -p waved-meeting --lib files`.

Expected: build errors, because `summary_status` and `Rig::files` don't exist yet.

- [ ] **Step 3: Implement the rows query**

Append to `crates/waved-meeting/src/db.rs`:

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingRow {
    pub id: Uuid,
    pub start_time: String,
    pub duration_secs: u32,
    pub status: String,
    pub attempt_cycle: u32,
    pub last_status_code: Option<i64>,
    pub dir: String,
    pub marks: u32,
}

/// Every recording row with its bookmark count, newest first.
pub async fn recording_rows(pool: &SqlitePool) -> Result<Vec<RecordingRow>, sqlx::Error> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, String, i64, String, i64, Option<i64>, String, i64)> = sqlx::query_as(
        "SELECT m.id, m.start_time, COALESCE(m.duration_secs, 0), m.upload_status, \
         COALESCE(m.attempt_cycle, 0), m.last_status_code, m.file_path, \
         (SELECT COUNT(*) FROM bookmarks b WHERE b.meeting_id = m.id) \
         FROM meetings m ORDER BY m.rowid DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(
            |(id, start_time, duration, status, attempt_cycle, last_status_code, dir, marks)| {
                Some(RecordingRow {
                    id: id.parse().ok()?,
                    start_time,
                    duration_secs: duration.max(0) as u32,
                    status,
                    attempt_cycle: attempt_cycle.max(0) as u32,
                    last_status_code,
                    dir,
                    marks: marks.max(0) as u32,
                })
            },
        )
        .collect())
}
```

- [ ] **Step 4: Implement the snapshot**

Put the implementation above the tests in `crates/waved-meeting/src/files.rs`:

```rust
//! The recordings list for Files (D1–D3): every recording on Wave, newest
//! first, with its delivery state and, when it needs attention, the reason
//! in plain words; plus free space and whether it is low.

use crate::db;
use crate::model::MeetingStatus;
use nix::sys::statvfs::statvfs;
use sqlx::SqlitePool;
use std::path::Path;
use waved_config::StorageConfig;
use waved_state::{FilesState, RecordingStatus, RecordingSummary};

/// What Files shows for a database status. `None` hides the row: the
/// recording is still running, between WAV and decision, or being deleted.
pub(crate) fn summary_status(
    status: MeetingStatus,
    attempt_cycle: u32,
    retry_max: u32,
) -> Option<RecordingStatus> {
    use MeetingStatus as M;
    Some(match status {
        M::Recording | M::Finalized | M::Deleting => return None,
        M::AwaitingDecision => RecordingStatus::AwaitingDecision,
        M::LocalOnly => RecordingStatus::LocalOnly,
        M::Queued => RecordingStatus::Waiting,
        M::Uploading => RecordingStatus::Uploading,
        M::RetryWait if attempt_cycle < retry_max.max(1) => RecordingStatus::Waiting,
        M::RetryWait | M::ConfigurationError => RecordingStatus::Failed,
        M::Uploaded => RecordingStatus::Uploaded,
        M::MissingArtifact | M::LegacyUnsupported | M::ArtifactError => RecordingStatus::Damaged,
    })
}

/// The failure reason Recording detail shows, for rows that need attention.
pub(crate) fn plain_reason(
    status: MeetingStatus,
    summary: RecordingStatus,
    last_status_code: Option<i64>,
) -> Option<String> {
    use MeetingStatus as M;
    if !summary.needs_attention() {
        return None;
    }
    Some(match (status, last_status_code) {
        (M::ConfigurationError, Some(code)) => {
            format!("The server refused it (HTTP {code}). Check the upload settings.")
        }
        (M::ConfigurationError, None) => {
            "The upload settings are incomplete. Check them in Uploads.".into()
        }
        (M::RetryWait, _) => {
            "The server couldn't be reached. Send it again when the network is back.".into()
        }
        (M::MissingArtifact, _) => "The audio file is missing from Wave.".into(),
        (M::LegacyUnsupported, _) => "Made by an older firmware, so it can't be sent.".into(),
        (M::ArtifactError, _) => "The recording was interrupted and is incomplete.".into(),
        _ => return None,
    })
}

/// SQLite `datetime('now')` (UTC) as RFC 3339, or `None` before the clock
/// was synchronized (2020-01-01, the recorder's own threshold).
pub(crate) fn started_at(start_time: &str) -> Option<String> {
    let parsed = chrono::NaiveDateTime::parse_from_str(start_time, "%Y-%m-%d %H:%M:%S").ok()?;
    let utc = parsed.and_utc();
    (utc.timestamp() >= 1_577_836_800).then(|| utc.to_rfc3339())
}

fn audio_size(dir: &Path) -> u64 {
    ["audio.wav", "audio.wav.partial"]
        .iter()
        .find_map(|name| std::fs::metadata(dir.join(name)).ok())
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

/// Free bytes and free MiB on the recordings volume; zero when unknown.
fn free_space(dir: &Path) -> (u64, u64) {
    match statvfs(dir) {
        Ok(stats) => {
            let bytes = (stats.blocks_available() as u64)
                .saturating_mul((stats.fragment_size() as u64).max(1));
            (bytes, bytes / (1024 * 1024))
        }
        Err(_) => (0, 0),
    }
}

pub(crate) async fn snapshot(
    pool: &SqlitePool,
    recordings_dir: &Path,
    storage: &StorageConfig,
    retry_max: u32,
    storage_blocked: bool,
) -> Result<FilesState, sqlx::Error> {
    let rows = db::recording_rows(pool).await?;
    let mut recordings = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(status) = MeetingStatus::parse(&row.status) else {
            continue;
        };
        let Some(summary) = summary_status(status, row.attempt_cycle, retry_max) else {
            continue;
        };
        recordings.push(RecordingSummary {
            id: row.id,
            started_at: started_at(&row.start_time),
            duration_secs: row.duration_secs,
            size_bytes: audio_size(Path::new(&row.dir)),
            status: summary,
            marks: row.marks,
            last_error: plain_reason(status, summary, row.last_status_code),
        });
    }
    let (free_bytes, free_mb) = free_space(recordings_dir);
    Ok(FilesState {
        recordings,
        free_bytes,
        low_space: storage_blocked || free_mb < storage.minimum_free_mb,
    })
}
```

- [ ] **Step 5: Publish from the service**

In `crates/waved-meeting/src/lib.rs`:

1. Imports: add `FilesState` to the `waved_state::{…}` list.
2. Field after `_fallback`: `files_tx: Option<Arc<watch::Sender<FilesState>>>,` initialised as `files_tx: None,`.
3. Add to `impl MeetingService`:

```rust
    /// Publish the Files list on this watch (refreshed every 5 s and after
    /// any command that changes a recording).
    pub fn with_files_publisher(mut self, files: Arc<watch::Sender<FilesState>>) -> Self {
        self.files_tx = Some(files);
        self
    }

    async fn publish_files(&self) {
        let Some(files_tx) = self.files_tx.as_ref() else {
            return;
        };
        let storage_blocked = self.meeting_tx.borrow().storage_blocked;
        match files::snapshot(
            &self.pool,
            &self.recordings_dir,
            &self.config.storage,
            self.config.api.retry_max,
            storage_blocked,
        )
        .await
        {
            Ok(state) => {
                files_tx.send_if_modified(|current| {
                    if *current == state {
                        return false;
                    }
                    *current = state;
                    true
                });
            }
            Err(error) => warn!("failed to publish the recordings list: {error}"),
        }
    }
```

4. Add a free function:

```rust
/// Commands after which the Files list is republished right away.
fn changes_files(command: &Command) -> bool {
    matches!(
        command,
        Command::StopRecording
            | Command::SaveRecording(_)
            | Command::SendRecording(_)
            | Command::DiscardRecording(_)
            | Command::RetryUpload(_)
            | Command::RetryFailedUploads
            | Command::DeleteRecording(_)
    )
}
```

5. In `run`, before the loop:

```rust
        let mut files_tick = tokio::time::interval(Duration::from_secs(5));
        files_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
```

…and a `select!` branch (the first tick fires at once, giving the boot publish):

```rust
                _ = files_tick.tick(), if self.files_tx.is_some() => {
                    self.publish_files().await;
                }
```

6. In the command branch, change `Ok(cmd) => { match cmd {` to:

```rust
                        Ok(cmd) => {
                            let refresh_files = changes_files(&cmd);
                            match cmd {
```

…and after that `match cmd { … }` closes (still inside `Ok(cmd) => { … }`), add:

```rust
                            if refresh_files {
                                self.publish_files().await;
                            }
```

7. In the tick branch, after `self.keep_unanswered().await;`, add `self.publish_files().await;`.

In `crates/waved-meeting/src/service_tests.rs`:
- add `pub files: Arc<watch::Sender<FilesState>>,` to `Rig`
- in `start_rig_in`, create `let files = Arc::new(watch::channel(FilesState::default()).0);`, chain `.with_files_publisher(Arc::clone(&files))` after `.with_live_inputs(…)`, and set `files,` in the literal
- import `FilesState` with `use waved_state::FilesState;`

- [ ] **Step 6: Run and expect a pass**

```bash
set -o pipefail
scripts/linux-test.sh test -p waved-meeting --lib 2>&1 | grep -E 'test result|FAILED|panicked'
scripts/linux-test.sh test -p waved-meeting --test recording_decision 2>&1 | grep -E 'test result'
```

Expected: both all pass.

- [ ] **Step 7: Lint and commit**

```bash
git add crates/waved-meeting/src/db.rs crates/waved-meeting/src/files.rs crates/waved-meeting/src/lib.rs crates/waved-meeting/src/service_tests.rs
git commit -m "feat(meeting): publish the recordings list for Files

- FilesState: newest first, delivery state, marks, size, plain failure reason
- free space and low_space from statvfs and the storage threshold
- refreshed every 5 s and right after commands that change a recording

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Local playback of recordings (C8, part 2)

Runs after Task 11 (same crate) and Task 12 (`PlaybackService::play_local`).

**Files:**
- Create: `crates/waved-meeting/src/player.rs`
- Modify: `crates/waved-meeting/src/lib.rs` (`pub mod player;`)

**Interfaces:**
- Consumes:
  - `waved_audio::playback::PlaybackService::{play_local, local_queue_depth, channels, period_frames}` (Task 12)
  - `metadata::WebhookMetadata`
  - `Command::{PlayRecording, PausePlayback, ResumePlayback, SeekPlayback, StopPlayback, DeleteRecording, DiscardRecording, Shutdown}`
  - `AudioState.call_off_hook`, `MeetingState.recording`
- Produces:
  - `pub trait SpeakerOutput: Send + Sync { fn submit_local(&self, interleaved: Vec<i16>) -> bool; fn local_queued(&self) -> usize; fn output_channels(&self) -> usize; fn frames_per_period(&self) -> usize; }`, implemented for `PlaybackService`
  - `pub struct Track` with `open(recordings_dir: &Path, id: Uuid) -> std::io::Result<Self>`, `id() -> Uuid`, `duration_ms() -> u32`, `position_ms() -> u32`, `marks_ms() -> &[u32]`, `at_end() -> bool`, `seek_ms(u32) -> std::io::Result<()>`, `next_period(frames: usize, channels: usize) -> std::io::Result<Option<Vec<i16>>>`
  - `pub fn marks_from_metadata(path: &Path) -> Vec<u32>`
  - `pub struct RecordingPlayer` with `new(recordings_dir: PathBuf, output: Arc<dyn SpeakerOutput>, state_tx: Arc<watch::Sender<PlaybackState>>) -> Self`, `run(self, commands: broadcast::Receiver<Command>, audio: watch::Receiver<AudioState>, meeting: watch::Receiver<MeetingState>)` (async), `handle(&mut self, &Command)`, `set_blocked(&mut self, bool)`, `pump(&mut self)`

- [ ] **Step 1: Write the failing tests**

Create `crates/waved-meeting/src/player.rs` with the test module first:

```rust
//! Local playback of recordings through the speaker (C8).

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct FakeOutput {
        submitted: Mutex<Vec<Vec<i16>>>,
        queued: AtomicUsize,
        frames: usize,
    }

    impl FakeOutput {
        fn new(frames: usize) -> Arc<Self> {
            Arc::new(Self {
                submitted: Mutex::new(Vec::new()),
                queued: AtomicUsize::new(0),
                frames,
            })
        }
        /// The speaker played everything queued.
        fn drain(&self) {
            self.queued.store(0, Ordering::SeqCst);
        }
        fn submitted(&self) -> Vec<Vec<i16>> {
            self.submitted.lock().unwrap().clone()
        }
    }

    impl SpeakerOutput for FakeOutput {
        fn submit_local(&self, interleaved: Vec<i16>) -> bool {
            self.submitted.lock().unwrap().push(interleaved);
            self.queued.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn local_queued(&self) -> usize {
            self.queued.load(Ordering::SeqCst)
        }
        fn output_channels(&self) -> usize {
            2
        }
        fn frames_per_period(&self) -> usize {
            self.frames
        }
    }

    fn ramp(n: usize) -> Vec<i16> {
        (0..n).map(|i| (i % 30_000) as i16).collect()
    }

    fn fixture(dir: &Path, samples: &[i16], pre_roll_ms: u64, bookmarks_ms: &[u64]) -> Uuid {
        let id = Uuid::new_v4();
        let rec = dir.join(id.to_string());
        std::fs::create_dir_all(&rec).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(rec.join("audio.wav"), spec).unwrap();
        for &sample in samples {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let metadata = serde_json::json!({
            "schema_version": 1,
            "event": "meeting.recording.completed",
            "meeting_id": id.to_string(),
            "device": {"id": "wave-test", "name": "Wave", "software_version": "0"},
            "recording": {
                "started_at": null, "clock_synchronized": false,
                "recording_duration_ms": 0, "audio_duration_ms": 0,
                "pre_roll_duration_ms": pre_roll_ms, "audio_mode": "Omni"
            },
            "audio": {
                "filename": "x.wav", "content_type": "audio/wav", "codec": "pcm_s16le",
                "sample_rate_hz": 48000, "channels": 1, "bit_depth": 16,
                "size_bytes": 0, "sha256": ""
            },
            "bookmarks": bookmarks_ms.iter().map(|ms| serde_json::json!({"offset_ms": ms})).collect::<Vec<_>>(),
            "speaker_segments": []
        });
        std::fs::write(rec.join("metadata.json"), serde_json::to_vec(&metadata).unwrap()).unwrap();
        id
    }

    fn player(dir: &Path, output: &Arc<FakeOutput>) -> (RecordingPlayer, watch::Receiver<PlaybackState>) {
        let (tx, rx) = watch::channel(PlaybackState::default());
        let output: Arc<dyn SpeakerOutput> = Arc::clone(output) as Arc<dyn SpeakerOutput>;
        (RecordingPlayer::new(dir.to_path_buf(), output, Arc::new(tx)), rx)
    }

    #[test]
    fn play_streams_mono_to_both_channels_two_periods_ahead() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(1_000), 0, &[]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        let periods = out.submitted();
        assert_eq!(periods.len(), 2, "two periods queued ahead");
        let expected: Vec<i16> = (0..100).flat_map(|i| [i as i16, i as i16]).collect();
        assert_eq!(periods[0], expected);
        let s = state.borrow().clone();
        assert_eq!(s.recording, Some(id));
        assert!(s.playing);
        assert_eq!(s.duration_ms, 20);
    }

    #[test]
    fn end_of_file_parks_at_the_end_and_play_restarts() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(250), 0, &[]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        out.drain();
        p.pump();
        let periods = out.submitted();
        assert_eq!(periods.len(), 3);
        assert_eq!(periods[2][..2], [200, 200]);
        assert!(periods[2][100..].iter().all(|&s| s == 0), "the last period is padded");
        let s = state.borrow().clone();
        assert!(!s.playing);
        assert_eq!(s.position_ms, s.duration_ms);
        out.drain();
        p.handle(&Command::PlayRecording(id));
        assert_eq!(out.submitted()[3][..2], [0, 0], "Play at the end starts over");
    }

    #[test]
    fn seek_moves_the_next_period_and_clamps_past_the_end() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(48_000), 0, &[]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        p.handle(&Command::SeekPlayback(500));
        assert_eq!(state.borrow().position_ms, 500);
        out.drain();
        p.pump();
        assert_eq!(out.submitted()[2][..2], [24_000, 24_000]);
        p.handle(&Command::SeekPlayback(10_000));
        assert_eq!(state.borrow().position_ms, state.borrow().duration_ms);
    }

    #[test]
    fn marks_count_from_the_start_of_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(1_000), 1_000, &[500, 2_000]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        assert_eq!(state.borrow().marks_ms, vec![1_500, 3_000]);
        assert!(marks_from_metadata(&tmp.path().join("missing.json")).is_empty());
    }

    #[test]
    fn a_call_or_recording_pauses_and_blocks_playback() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(48_000), 0, &[]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        p.set_blocked(true);
        assert!(!state.borrow().playing);
        p.handle(&Command::ResumePlayback);
        assert!(!state.borrow().playing, "blocked while the call lasts");
        p.set_blocked(false);
        p.handle(&Command::ResumePlayback);
        assert!(state.borrow().playing);
    }

    #[test]
    fn deleting_the_playing_recording_stops_playback() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(48_000), 0, &[]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        p.handle(&Command::DeleteRecording(Uuid::new_v4()));
        assert!(state.borrow().playing, "another recording's delete changes nothing");
        p.handle(&Command::DeleteRecording(id));
        assert_eq!(*state.borrow(), PlaybackState::default());
    }

    #[test]
    fn a_missing_recording_clears_the_state() {
        let tmp = tempfile::tempdir().unwrap();
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(Uuid::new_v4()));
        assert_eq!(*state.borrow(), PlaybackState::default());
        assert!(out.submitted().is_empty());
    }

    #[tokio::test]
    async fn run_pauses_when_a_call_starts() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(480_000), 0, &[]);
        let out = FakeOutput::new(1_024);
        let (p, mut state) = player(tmp.path(), &out);
        let (commands, commands_rx) = broadcast::channel(8);
        let (audio, audio_rx) = watch::channel(AudioState::default());
        let (_meeting, meeting_rx) = watch::channel(MeetingState::default());
        let task = tokio::spawn(p.run(commands_rx, audio_rx, meeting_rx));
        commands.send(Command::PlayRecording(id)).unwrap();
        tokio::time::timeout(Duration::from_secs(1), state.wait_for(|s| s.playing))
            .await
            .unwrap()
            .unwrap();
        audio.send_modify(|a| a.call_off_hook = true);
        tokio::time::timeout(Duration::from_secs(1), state.wait_for(|s| !s.playing))
            .await
            .unwrap()
            .unwrap();
        commands.send(Command::Shutdown).unwrap();
        task.await.unwrap();
    }
}
```

Add `pub mod player;` after `pub mod model;` in `lib.rs`.

- [ ] **Step 2: Run and expect a failure**

Run `scripts/linux-test.sh test -p waved-meeting --lib player`.

Expected: build errors, because `RecordingPlayer`, `SpeakerOutput` and `Track` don't exist yet.

- [ ] **Step 3: Implement**

Put the implementation above the tests in `crates/waved-meeting/src/player.rs`:

```rust
//! Local playback of recordings through the speaker (C8): play, pause, seek
//! and marks for Recording detail. It pauses by itself while a call is active
//! or a recording runs, so the speaker never talks over either.
//!
//! Periods go to `PlaybackService::play_local`, which mixes them with any
//! host audio before the speaker DSP, so volume, the limiter and the AEC
//! reference cover playback too. `position_ms` is the next period to queue:
//! at most two periods (~43 ms) ahead of what is audible.

use crate::metadata::WebhookMetadata;
use crate::recorder::RECORDING_SAMPLE_RATE;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tracing::{info, warn};
use uuid::Uuid;
use waved_state::{AudioState, Command, MeetingState, PlaybackState};

/// Periods kept queued ahead of the speaker: enough to ride out a late tick,
/// few enough that Pause and Seek feel immediate.
const QUEUE_AHEAD: usize = 2;
/// Position updates on the bus at most this often while playing.
const PUBLISH_STEP_MS: u32 = 250;

/// Where decoded periods go. The speaker's PlaybackService in production,
/// a fake in tests.
pub trait SpeakerOutput: Send + Sync {
    /// Queue one interleaved period; false when the speaker is gone or full.
    fn submit_local(&self, interleaved: Vec<i16>) -> bool;
    /// Periods queued and not played yet.
    fn local_queued(&self) -> usize;
    fn output_channels(&self) -> usize;
    fn frames_per_period(&self) -> usize;
}

impl SpeakerOutput for waved_audio::playback::PlaybackService {
    fn submit_local(&self, interleaved: Vec<i16>) -> bool {
        self.play_local(interleaved).is_ok()
    }
    fn local_queued(&self) -> usize {
        self.local_queue_depth()
    }
    fn output_channels(&self) -> usize {
        self.channels() as usize
    }
    fn frames_per_period(&self) -> usize {
        self.period_frames()
    }
}

/// One recording opened for playback.
pub struct Track {
    id: Uuid,
    reader: hound::WavReader<BufReader<std::fs::File>>,
    position: u32,
    total: u32,
    marks_ms: Vec<u32>,
}

impl Track {
    pub fn open(recordings_dir: &Path, id: Uuid) -> std::io::Result<Self> {
        let dir = recordings_dir.join(id.to_string());
        let reader = hound::WavReader::open(dir.join("audio.wav")).map_err(to_io)?;
        let spec = reader.spec();
        if spec.channels != 1 || spec.sample_rate != RECORDING_SAMPLE_RATE || spec.bits_per_sample != 16 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported recording format {spec:?}"),
            ));
        }
        let total = reader.duration();
        Ok(Self {
            id,
            reader,
            position: 0,
            total,
            marks_ms: marks_from_metadata(&dir.join("metadata.json")),
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn duration_ms(&self) -> u32 {
        frames_to_ms(self.total)
    }

    pub fn position_ms(&self) -> u32 {
        frames_to_ms(self.position)
    }

    pub fn marks_ms(&self) -> &[u32] {
        &self.marks_ms
    }

    pub fn at_end(&self) -> bool {
        self.position >= self.total
    }

    /// Move the next period to `ms` from the start, clamped to the end.
    pub fn seek_ms(&mut self, ms: u32) -> std::io::Result<()> {
        let frame = ms_to_frames(ms).min(self.total);
        self.reader.seek(frame)?;
        self.position = frame;
        Ok(())
    }

    /// The next period with the mono sample copied to every channel; the
    /// last period is zero-padded. `None` once at the end.
    pub fn next_period(&mut self, frames: usize, channels: usize) -> std::io::Result<Option<Vec<i16>>> {
        if self.at_end() {
            return Ok(None);
        }
        let mut out = vec![0i16; frames * channels];
        let mut read = 0usize;
        for (index, sample) in self.reader.samples::<i16>().take(frames).enumerate() {
            let sample = sample.map_err(to_io)?;
            out[index * channels..(index + 1) * channels].fill(sample);
            read += 1;
        }
        self.position = if read < frames {
            self.total
        } else {
            (self.position + read as u32).min(self.total)
        };
        Ok(Some(out))
    }
}

fn frames_to_ms(frames: u32) -> u32 {
    (u64::from(frames) * 1_000 / u64::from(RECORDING_SAMPLE_RATE)) as u32
}

fn ms_to_frames(ms: u32) -> u32 {
    (u64::from(ms) * u64::from(RECORDING_SAMPLE_RATE) / 1_000).min(u64::from(u32::MAX)) as u32
}

fn to_io(error: hound::Error) -> std::io::Error {
    match error {
        hound::Error::IoError(io) => io,
        other => std::io::Error::new(std::io::ErrorKind::InvalidData, other),
    }
}

/// Bookmark offsets from the start of the file. Metadata counts them from
/// the end of the pre-roll. Empty when metadata is missing or unreadable.
pub fn marks_from_metadata(path: &Path) -> Vec<u32> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(metadata) = serde_json::from_slice::<WebhookMetadata>(&bytes) else {
        return Vec::new();
    };
    let pre_roll = metadata.recording.pre_roll_duration_ms;
    metadata
        .bookmarks
        .iter()
        .map(|bookmark| (pre_roll + bookmark.offset_ms).min(u64::from(u32::MAX)) as u32)
        .collect()
}

fn is_blocked(audio: &AudioState, meeting: &MeetingState) -> bool {
    audio.call_off_hook || meeting.recording
}

pub struct RecordingPlayer {
    recordings_dir: PathBuf,
    output: Arc<dyn SpeakerOutput>,
    state_tx: Arc<watch::Sender<PlaybackState>>,
    track: Option<Track>,
    playing: bool,
    blocked: bool,
    published_ms: u32,
}

impl RecordingPlayer {
    pub fn new(
        recordings_dir: PathBuf,
        output: Arc<dyn SpeakerOutput>,
        state_tx: Arc<watch::Sender<PlaybackState>>,
    ) -> Self {
        Self {
            recordings_dir,
            output,
            state_tx,
            track: None,
            playing: false,
            blocked: false,
            published_ms: 0,
        }
    }

    pub async fn run(
        mut self,
        mut commands: broadcast::Receiver<Command>,
        mut audio: watch::Receiver<AudioState>,
        mut meeting: watch::Receiver<MeetingState>,
    ) {
        let period = Duration::from_micros(
            self.output.frames_per_period() as u64 * 1_000_000 / u64::from(RECORDING_SAMPLE_RATE),
        );
        let mut tick = tokio::time::interval(period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let blocked = is_blocked(&audio.borrow(), &meeting.borrow());
        self.set_blocked(blocked);
        info!("recording player ready");
        loop {
            tokio::select! {
                received = commands.recv() => match received {
                    Ok(Command::Shutdown) | Err(broadcast::error::RecvError::Closed) => return,
                    Ok(command) => self.handle(&command),
                    Err(broadcast::error::RecvError::Lagged(n)) => warn!("player lagged {n} commands"),
                },
                Ok(()) = audio.changed() => {
                    let blocked = is_blocked(&audio.borrow_and_update(), &meeting.borrow());
                    self.set_blocked(blocked);
                }
                Ok(()) = meeting.changed() => {
                    let blocked = is_blocked(&audio.borrow(), &meeting.borrow_and_update());
                    self.set_blocked(blocked);
                }
                _ = tick.tick(), if self.playing => self.pump(),
            }
        }
    }

    /// A call or a recording started (true) or ended (false). Playback
    /// pauses and stays paused; the user resumes it.
    pub fn set_blocked(&mut self, blocked: bool) {
        if blocked && !self.blocked && self.playing {
            info!("playback paused: a call or recording started");
            self.playing = false;
            self.publish(true);
        }
        self.blocked = blocked;
    }

    pub fn handle(&mut self, command: &Command) {
        match command {
            Command::PlayRecording(id) => self.play(*id),
            Command::PausePlayback => {
                if self.playing {
                    self.playing = false;
                    self.publish(true);
                }
            }
            Command::ResumePlayback => {
                if let Some(id) = self.track.as_ref().map(Track::id) {
                    self.play(id);
                }
            }
            Command::SeekPlayback(ms) => self.seek(*ms),
            Command::StopPlayback => self.stop(),
            Command::DeleteRecording(id) | Command::DiscardRecording(id)
                if self.track.as_ref().is_some_and(|track| track.id() == *id) =>
            {
                self.stop()
            }
            _ => {}
        }
    }

    fn play(&mut self, id: Uuid) {
        if self.blocked {
            warn!("play ignored: a call or recording is active");
            return;
        }
        if self.track.as_ref().map(Track::id) != Some(id) {
            match Track::open(&self.recordings_dir, id) {
                Ok(track) => self.track = Some(track),
                Err(error) => {
                    warn!("cannot play recording {id}: {error}");
                    self.stop();
                    return;
                }
            }
        }
        let track = self.track.as_mut().expect("track loaded above");
        if track.at_end() {
            if let Err(error) = track.seek_ms(0) {
                warn!("cannot rewind recording {id}: {error}");
                self.stop();
                return;
            }
        }
        self.playing = true;
        self.publish(true);
        self.pump();
    }

    fn seek(&mut self, ms: u32) {
        let Some(track) = self.track.as_mut() else {
            return;
        };
        if let Err(error) = track.seek_ms(ms) {
            warn!("seek to {ms} ms failed: {error}");
            return;
        }
        self.publish(true);
    }

    fn stop(&mut self) {
        self.track = None;
        self.playing = false;
        self.publish(true);
    }

    /// Keep `QUEUE_AHEAD` periods queued while playing.
    pub fn pump(&mut self) {
        if !self.playing {
            return;
        }
        let frames = self.output.frames_per_period();
        let channels = self.output.output_channels();
        while self.output.local_queued() < QUEUE_AHEAD {
            let Some(track) = self.track.as_mut() else {
                self.playing = false;
                break;
            };
            match track.next_period(frames, channels) {
                Ok(Some(period)) => {
                    if !self.output.submit_local(period) {
                        warn!("the speaker refused playback; stopping");
                        self.playing = false;
                        break;
                    }
                }
                Ok(None) => {
                    self.playing = false;
                    break;
                }
                Err(error) => {
                    warn!("playback read failed: {error}");
                    self.playing = false;
                    break;
                }
            }
        }
        let finished = !self.playing;
        self.publish(finished);
    }

    fn publish(&mut self, force: bool) {
        let state = match self.track.as_ref() {
            Some(track) => PlaybackState {
                recording: Some(track.id()),
                playing: self.playing,
                position_ms: track.position_ms(),
                duration_ms: track.duration_ms(),
                marks_ms: track.marks_ms().to_vec(),
            },
            None => PlaybackState::default(),
        };
        if !force && state.position_ms.abs_diff(self.published_ms) < PUBLISH_STEP_MS {
            return;
        }
        self.published_ms = state.position_ms;
        self.state_tx.send_if_modified(|current| {
            if *current == state {
                return false;
            }
            *current = state;
            true
        });
    }
}
```

- [ ] **Step 4: Run and expect a pass**

```bash
set -o pipefail
scripts/linux-test.sh test -p waved-meeting --lib 2>&1 | grep -E 'test result|FAILED|panicked'
scripts/linux-test.sh check -p waved --features ui,capture-alsa 2>&1 | tail -3
```

Expected:
- all tests pass
- the binary still builds (nothing calls the player yet; H2 wires it)

- [ ] **Step 5: Lint and commit**

```bash
git add crates/waved-meeting/src/player.rs crates/waved-meeting/src/lib.rs
git commit -m "feat(meeting): play recordings through the speaker

- RecordingPlayer handles Play/Pause/Resume/Seek/Stop and publishes PlaybackState (C8)
- marks are file offsets (pre-roll + bookmark); seek clamps to the end
- pauses by itself during a call or a recording; stops when its recording is deleted
- SpeakerOutput is implemented for PlaybackService::play_local

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Audio lane

### Task 12: A local input on the speaker (C8, part 1)

**Files:**
- Modify: `crates/waved-audio/src/playback.rs`

**Interfaces:**
- Produces:
  - `PlaybackService::play_local(&self, samples: Vec<i16>) -> Result<(), PlaybackError>`
  - `PlaybackService::local_queue_depth(&self) -> usize`
  - `pub fn mix_into(dst: &mut [i16], src: &[i16])`
  - `run_playback_loop` gains a `local_rx: mpsc::Receiver<Vec<i16>>` parameter right after `cmd_rx`

- [ ] **Step 1: Write the failing tests**

In `mod tests` of `crates/waved-audio/src/playback.rs`:

1. Change the helper signature to `fn run_with_mock(stop_after: usize, fail_once: bool, prefill: Vec<Vec<i16>>, local_prefill: Vec<Vec<i16>>, period_samples: usize, channels: u32)`.

2. After the `for b in prefill { … }` loop, add:

```rust
        let (local_tx, local_rx) = mpsc::channel::<Vec<i16>>(8);
        for b in local_prefill {
            local_tx.try_send(b).unwrap();
        }
```

3. Pass `local_rx` to `run_playback_loop` right after `cmd_rx`, and add `drop(local_tx);` next to `drop(cmd_tx);`.

4. Update the four existing callers to pass `vec![]` as the new fourth argument:
   - `run_with_mock(3, false, vec![], vec![], 8, 2)`
   - `run_with_mock(2, false, vec![payload.clone()], vec![], 8, 2)`
   - `run_with_mock(1, false, vec![vec![99, 99]], vec![], 8, 2)`
   - `run_with_mock(3, true, vec![], vec![], 4, 1)`

Append:

```rust
    #[test]
    fn mix_into_adds_and_saturates() {
        let mut dst = vec![1_000i16, -1_000, 30_000, -30_000];
        mix_into(&mut dst, &[500, -500, 10_000, -10_000]);
        assert_eq!(dst, vec![1_500, -1_500, i16::MAX, i16::MIN]);
        let mut short = vec![1i16, 2, 3];
        mix_into(&mut short, &[1]);
        assert_eq!(short, vec![2, 2, 3], "a short local batch leaves the rest alone");
    }

    #[test]
    fn local_batch_plays_without_host_audio() {
        let (batches, stats, ref_batches) =
            run_with_mock(2, false, vec![], vec![vec![100; 8]], 8, 2);
        assert_eq!(batches.len(), 2);
        assert!(
            stats.frames_written.load(Ordering::Relaxed) > 0,
            "local audio counts as written, not silence"
        );
        assert!(!ref_batches.is_empty(), "the AEC reference sees local playback");
    }
```

In `queue_depth_reports_pending_batches`, add `let (local_tx, _local_rx) = mpsc::channel::<Vec<i16>>(4);` and `local_tx: local_tx.clone(),` to the `PlaybackService` literal. Then append:

```rust
        assert_eq!(svc.local_queue_depth(), 0);
        svc.play_local(vec![0; 4]).unwrap();
        assert_eq!(svc.local_queue_depth(), 1);
```

- [ ] **Step 2: Run and expect a failure**

Run `cargo test -p waved-audio --lib playback`.

Expected: build errors, because `mix_into`, `local_tx` and the extra parameter don't exist yet.

- [ ] **Step 3: Implement**

In `crates/waved-audio/src/playback.rs`:

1. Add a field to `PlaybackService` after `cmd_tx`:

```rust
    /// Local playback (recordings), mixed with the host stream each period.
    local_tx: mpsc::Sender<Vec<i16>>,
```

2. Add methods after `play_batch`:

```rust
    /// Submit one period of local playback (Files → Play). It is mixed with
    /// whatever the host sends, so neither stream interrupts the other.
    pub fn play_local(&self, samples: Vec<i16>) -> Result<(), PlaybackError> {
        self.local_tx
            .try_send(samples)
            .map_err(|_| PlaybackError::Stopped)
    }

    /// Local periods queued and not played yet.
    pub fn local_queue_depth(&self) -> usize {
        self.local_tx.max_capacity() - self.local_tx.capacity()
    }
```

3. Add the free function near `WriteError`:

```rust
/// Add `src` into `dst` sample by sample, saturating at the i16 limits.
/// A shorter `src` leaves the tail of `dst` unchanged.
pub fn mix_into(dst: &mut [i16], src: &[i16]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d = d.saturating_add(*s);
    }
}
```

4. In `run_playback_loop`, add the parameter `mut local_rx: mpsc::Receiver<Vec<i16>>,` after `mut cmd_rx: mpsc::Receiver<Vec<i16>>,`. Change `let (batch, is_silence) = match cmd_rx.try_recv() {` to `let (batch, mut is_silence) = match cmd_rx.try_recv() {`. Then, right before `chain.process_i16(&mut normalized);`, insert:

```rust
        // Local playback rides on the same period, before the DSP chain, so
        // volume, limiting and the AEC reference cover it too.
        if let Ok(local) = local_rx.try_recv() {
            mix_into(&mut normalized, &local);
            is_silence = false;
        }
```

5. In `spawn_playback`, add `let (local_tx, local_rx) = mpsc::channel::<Vec<i16>>(8);` after the `cmd_tx` channel. Pass `local_rx` after `cmd_rx` in the `run_playback_loop(…)` call, and add `local_tx,` to the returned `PlaybackService`.

- [ ] **Step 4: Run and expect a pass**

```bash
set -o pipefail
cargo test -p waved-audio --lib 2>&1 | grep -E 'test result|FAILED|panicked'
scripts/linux-test.sh check -p waved-audio --features capture-alsa 2>&1 | tail -3
```

Expected:
- 244 + 2 tests pass
- the ALSA build compiles

- [ ] **Step 5: Lint and commit**

Run `cargo clippy -p waved-audio --all-targets`. The counts stay at 18 / 37 / 1; the existing `too_many_arguments` warning on `run_playback_loop` is the same warning, still one.

```bash
git add crates/waved-audio/src/playback.rs
git commit -m "feat(audio): mix a local playback input into the speaker loop

- PlaybackService::play_local queues periods that are mixed (saturating) with the
  host stream before the DSP chain, so both play and the AEC reference sees both
- local_queue_depth lets the recording player pace itself

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: A fixed aim for the directional modes (C3)

**Files:**
- Create: `crates/waved-audio/src/beam/aim.rs`
- Modify: `crates/waved-audio/src/beam/mod.rs`
- Modify: `crates/waved-audio/src/lib.rs`

**Interfaces:**
- Consumes: `AudioState.{mode, beam_aim_deg}` (seeded by `waved_settings::seed_state` before `AudioService::new`)
- Produces:
  - `pub fn beam::aim::fixed_direction(mode: &AudioMode, beam_aim_deg: f32, test_override: Option<(f32, f32)>) -> Option<(f32, f32)>`
  - `pub const beam::aim::STEREO_FORWARD: (f32, f32) = (0.0, 0.0)`
  - `pub fn Beamformer::direction(&self) -> (f32, f32)`
  - private `fn AudioService::handle_command(&mut self, command: Command) -> Option<AudioServiceExit>`
  - private fields `AudioService.{mode: AudioMode, beam_aim_deg: f32}`

- [ ] **Step 1: Write the failing tests**

Create `crates/waved-audio/src/beam/aim.rs` with only the tests for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_direction_follows_the_mode_table() {
        use AudioMode::*;
        assert_eq!(fixed_direction(&Omni, 90.0, None), None);
        assert_eq!(fixed_direction(&Steered { initial_az_deg: 30.0 }, 90.0, None), None);
        assert_eq!(
            fixed_direction(&Steered { initial_az_deg: 30.0 }, 90.0, Some((10.0, 0.0))),
            None,
            "Follow talker has its own controller"
        );
        for mode in [Cardioid, Supercardioid, Figure8] {
            assert_eq!(fixed_direction(&mode, 270.0, None), Some((270.0, 0.0)), "{mode:?}");
            assert_eq!(
                fixed_direction(&mode, 270.0, Some((45.0, 10.0))),
                Some((45.0, 10.0)),
                "the test override wins for {mode:?}"
            );
        }
        assert_eq!(fixed_direction(&Stereo, 270.0, None), Some(STEREO_FORWARD));
        assert_eq!(fixed_direction(&Stereo, 270.0, Some((45.0, 0.0))), Some((45.0, 0.0)));
    }
}
```

Add `pub mod aim;` to `crates/waved-audio/src/beam/mod.rs` (next to `pub mod delays;`).

Append to `mod tests` in `crates/waved-audio/src/lib.rs`:

```rust
    /// A 1 kHz plane wave arriving from `az_deg` (device degrees).
    fn plane_wave_8ch(az_deg: f32, frame_size: usize) -> AudioFrame {
        let geom = crate::beam::MicGeometry::default();
        let sr = 48_000.0_f32;
        let omega = 2.0 * std::f32::consts::PI * 1_000.0 / sr;
        let (cos, sin) = (az_deg.to_radians().cos(), az_deg.to_radians().sin());
        let mut samples = vec![0i16; frame_size * 8];
        for ch in 0..8 {
            let p = geom.position_for_alsa(ch);
            let tau = -((p.x * cos + p.y * sin) * 1e-3) / 343.0 * sr;
            for i in 0..frame_size {
                samples[i * 8 + ch] =
                    (((i as f32 - tau) * omega).sin() * 0.3 * i16::MAX as f32) as i16;
            }
        }
        AudioFrame::mic_only(samples, 8, 48_000, 0)
    }

    /// A service whose state was seeded with `mode` and `aim`, as main.rs does.
    fn service_with(
        mode: waved_state::AudioMode,
        aim: f32,
    ) -> (AudioService, Arc<watch::Sender<AudioState>>) {
        let state = AudioState {
            mode,
            beam_aim_deg: aim,
            nr_level: NrLevel::Off,
            ..AudioState::default()
        };
        let audio_tx = Arc::new(watch::channel(state).0);
        let (frame_tx, frame_rx) = mpsc::channel(4);
        let (meeting_tx, _meeting_rx) = mpsc::channel(4);
        let svc = AudioService::new(
            Arc::clone(&audio_tx),
            frame_tx,
            frame_rx,
            meeting_tx,
            480,
            48_000,
            crate::beam::MicGeometry::default(),
        );
        (svc, audio_tx)
    }

    #[test]
    fn cardioid_holds_the_aim_while_the_talker_moves() {
        let (mut svc, _state) = service_with(waved_state::AudioMode::Cardioid, 90.0);
        for az in [0.0, 180.0, 270.0] {
            svc.process_frame(&plane_wave_8ch(az, 480));
            assert_eq!(svc.beamformer.direction(), (90.0, 0.0), "talker at {az}°");
        }
    }

    #[test]
    fn stereo_keeps_the_device_orientation() {
        let (mut svc, _state) = service_with(waved_state::AudioMode::Stereo, 200.0);
        svc.process_frame(&plane_wave_8ch(90.0, 480));
        assert_eq!(svc.beamformer.direction(), (0.0, 0.0));
    }

    #[test]
    fn doa_override_still_wins_for_the_beamforming_test() {
        let (mut svc, _state) = service_with(waved_state::AudioMode::Cardioid, 90.0);
        svc.handle_command(Command::SetDoaOverride(Some((45.0, 10.0))));
        svc.process_frame(&plane_wave_8ch(0.0, 480));
        assert_eq!(svc.beamformer.direction(), (45.0, 10.0));
        svc.handle_command(Command::SetDoaOverride(None));
        svc.process_frame(&plane_wave_8ch(0.0, 480));
        assert_eq!(svc.beamformer.direction(), (90.0, 0.0), "clearing it restores the aim");
    }

    #[test]
    fn mode_switch_changes_the_steering() {
        let (mut svc, state) = service_with(waved_state::AudioMode::Cardioid, 90.0);
        assert!(svc.handle_command(Command::SetAudioMode(waved_state::AudioMode::Stereo)).is_none());
        assert_eq!(state.borrow().mode, waved_state::AudioMode::Stereo);
        svc.process_frame(&plane_wave_8ch(90.0, 480));
        assert_eq!(svc.beamformer.direction(), (0.0, 0.0));
        assert_eq!(svc.handle_command(Command::Shutdown), Some(AudioServiceExit::StopRequested));
    }
```

- [ ] **Step 2: Run and expect a failure**

Run `cargo test -p waved-audio --lib aim cardioid_holds stereo_keeps doa_override_still mode_switch`.

Expected: build errors, because `fixed_direction`, `direction` and `handle_command` don't exist yet.

- [ ] **Step 3: Implement the table and the getter**

Put above the tests in `crates/waved-audio/src/beam/aim.rs`:

```rust
//! Where the beamformer points in each audio mode (D2, C3).
//!
//! Directions are "device degrees", the same frame as the DoA azimuth. The UI
//! draws `doa_horizontal` clockwise from the top of the screen with no
//! conversion (`waved-ui` `kit/components/dial.rs`), so an aim set by
//! dragging on the Audio mode screen is used as is.

use waved_state::AudioMode;

/// Stereo keeps the device orientation: its two lobes sit at ±45° around the
/// front of the device and never follow talkers.
pub const STEREO_FORWARD: (f32, f32) = (0.0, 0.0);

/// The fixed (azimuth, elevation) for `mode`, or `None` when the mode has no
/// fixed direction. Whole room (Omni) has none, and Follow talker (Steered)
/// is driven by its own controller. `test_override` is the `SetDoaOverride`
/// hook used by scripts/test-beamforming.sh; it wins over aim and orientation.
pub fn fixed_direction(
    mode: &AudioMode,
    beam_aim_deg: f32,
    test_override: Option<(f32, f32)>,
) -> Option<(f32, f32)> {
    match mode {
        AudioMode::Omni | AudioMode::Steered { .. } => None,
        AudioMode::Stereo => Some(test_override.unwrap_or(STEREO_FORWARD)),
        AudioMode::Cardioid | AudioMode::Supercardioid | AudioMode::Figure8 => {
            Some(test_override.unwrap_or((beam_aim_deg, 0.0)))
        }
    }
}
```

In `crates/waved-audio/src/beam/mod.rs`, add to `impl Beamformer` after `set_doa`:

```rust
    /// Where the pattern points now (azimuth, elevation), in degrees.
    pub fn direction(&self) -> (f32, f32) {
        (self.doa_az_deg, self.doa_el_deg)
    }
```

- [ ] **Step 4: Implement the service changes**

In `crates/waved-audio/src/lib.rs`:

1. Add fields to `AudioService` after `steered`:

```rust
    /// Current audio mode, for steering.
    mode: waved_state::AudioMode,
    /// Fixed aim for Focused / Narrow / Two sides (device degrees). Seeded
    /// from AudioState, which main.rs seeds from config before this service
    /// is built.
    beam_aim_deg: f32,
```

2. In `AudioService::new`, read the aim with the other initial values:

```rust
        let (initial_nr, initial_mode, initial_aim) = {
            let s = audio_tx.borrow();
            (s.nr_level.clone(), s.mode.clone(), s.beam_aim_deg)
        };
```

…and add `mode: initial_mode, beam_aim_deg: initial_aim,` at the end of the `Self { … }` literal. Move `initial_mode` there; the earlier uses already clone it.

3. In `process_frame`, replace the `else { let (steer_az, steer_el) = … }` block after `if let Some(beam) = self.steered.as_mut() { … }` with:

```rust
            } else if let Some((az, el)) =
                crate::beam::aim::fixed_direction(&self.mode, self.beam_aim_deg, self.doa_override)
            {
                // Directional modes hold their aim and Stereo its orientation
                // instead of chasing the raw DoA every frame (BUG-17).
                self.beamformer.set_doa(az, el);
            }
```

4. Move the body of the `cmd = cmd_rx.recv() =>` branch into a method:

```rust
    /// Apply one bus command. Returns the exit when the command stops the service.
    fn handle_command(&mut self, command: Command) -> Option<AudioServiceExit> {
        match command {
            Command::SetAudioMode(mode) => {
                info!("audio mode → {mode:?}");
                self.steered = match &mode {
                    waved_state::AudioMode::Steered { initial_az_deg } => {
                        Some(SteeredBeam::new(*initial_az_deg))
                    }
                    _ => None,
                };
                self.beamformer.set_mode(mode.clone());
                self.beamformer.set_omni_mix(0.0); // reset crossfade for non-steered modes
                self.mode = mode.clone();
                self.audio_tx.send_modify(|s| s.mode = mode);
            }
            Command::SetDoaOverride(ov) => {
                match ov {
                    Some((az, el)) => info!("DOA override → ({az:.1}°, {el:.1}°)"),
                    None => info!("DOA override cleared — back to the mode's steering"),
                }
                self.doa_override = ov;
            }
            Command::SetNrLevel(level) => {
                info!("NR level -> {level:?}; rebuilding DSP chain");
                self.dsp_chain = build_dsp_chain(
                    self.frame_size,
                    self.sample_rate,
                    level.clone(),
                    self.aec_config.clone(),
                );
                self.audio_tx.send_modify(|s| s.nr_level = level);
            }
            Command::SetPlaybackPreset(p) => {
                info!("playback preset → {p:?}");
                self.audio_tx.send_modify(|s| s.playback_preset = p);
            }
            Command::ToggleMicMute => {
                self.audio_tx.send_modify(|s| s.mic_muted = !s.mic_muted);
            }
            Command::ToggleSpeakerMute => {
                let new_muted = !self.audio_tx.borrow().speaker_muted;
                self.audio_tx.send_modify(|s| {
                    s.speaker_muted = new_muted;
                    if !new_muted && s.speaker_volume < 10 {
                        s.speaker_volume = 10;
                    }
                });
                // SP#6: UacControl mirrors the new state to hw:1
                // PCM Playback Switch on the next state-watch tick.
            }
            Command::SetSpeakerVolume(v) => {
                self.audio_tx.send_modify(|s| s.speaker_volume = v);
                // SP#6: UacControl mirrors to hw:1 PCM Playback Volume.
            }
            Command::SetMicVolume(v) => {
                self.audio_tx.send_modify(|s| s.mic_volume = v);
            }
            Command::Shutdown => return Some(AudioServiceExit::StopRequested),
            _ => {}
        }
        None
    }
```

…and make the branch in `run`:

```rust
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Ok(command) => {
                            if let Some(exit) = self.handle_command(command) {
                                return exit;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return AudioServiceExit::StopRequested;
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("audio_srv lagged {n} commands");
                        }
                    }
                }
```

- [ ] **Step 5: Run and expect a pass**

```bash
set -o pipefail
cargo test -p waved-audio --lib 2>&1 | grep -E 'test result|FAILED|panicked'
cargo test -p waved-audio --test steered_beam_handoff --test mute_end_to_end 2>&1 | grep -E 'test result'
```

Expected: all pass. `audio_service_applies_beamformer_before_aec` still passes: Cardioid with aim 0 points at the 0° source.

- [ ] **Step 6: Lint and commit**

Run `cargo clippy -p waved-audio --all-targets`: no growth, and no warning in `beam/aim.rs`.

```bash
git add crates/waved-audio/src/beam/aim.rs crates/waved-audio/src/beam/mod.rs crates/waved-audio/src/lib.rs
git commit -m "feat(audio): hold a fixed aim in the directional modes

- Focused, Narrow and Two sides steer to beam_aim_deg instead of the raw DoA
  each frame (C3, BUG-17); Stereo stays on the device orientation
- SetDoaOverride still wins, so test-beamforming.sh is unchanged
- command handling moves into AudioService::handle_command

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: Apply `SetBeamAim` (C10)

**Files:**
- Modify: `crates/waved-audio/Cargo.toml`
- Modify: `crates/waved-audio/src/lib.rs`

**Interfaces:**
- Consumes: `Command::SetBeamAim(f32)`, `waved_config::wrap_beam_aim(f32) -> Option<f32>`
- Produces: `AudioState.beam_aim_deg` follows the command. The next period steers to it.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/waved-audio/src/lib.rs`:

```rust
    #[test]
    fn set_beam_aim_moves_the_fixed_beam_and_the_state() {
        let (mut svc, state) = service_with(waved_state::AudioMode::Supercardioid, 0.0);
        svc.handle_command(Command::SetBeamAim(-60.0));
        assert_eq!(state.borrow().beam_aim_deg, 300.0);
        svc.process_frame(&plane_wave_8ch(0.0, 480));
        assert_eq!(svc.beamformer.direction(), (300.0, 0.0));
        svc.handle_command(Command::SetBeamAim(f32::NAN));
        assert_eq!(state.borrow().beam_aim_deg, 300.0, "NaN is ignored");
        svc.handle_command(Command::SetBeamAim(720.0));
        assert_eq!(state.borrow().beam_aim_deg, 0.0);
    }
```

- [ ] **Step 2: Run and expect a failure**

Run `cargo test -p waved-audio --lib set_beam_aim`.

Expected: FAIL. `beam_aim_deg` stays 0.0, because no handler exists.

- [ ] **Step 3: Implement**

In `crates/waved-audio/Cargo.toml` under `[dependencies]`, after `waved-state`:

```toml
waved-config = { path = "../waved-config" }
```

In `handle_command`, add before `Command::Shutdown`:

```rust
            Command::SetBeamAim(degrees) => match waved_config::wrap_beam_aim(degrees) {
                Some(aim) => {
                    info!("beam aim → {aim:.1}°");
                    self.beam_aim_deg = aim;
                    self.audio_tx.send_modify(|s| s.beam_aim_deg = aim);
                }
                None => warn!("beam aim ignored: {degrees} is not a direction"),
            },
```

- [ ] **Step 4: Run and expect a pass**

Run `cargo test -p waved-audio --lib`.

Expected: all pass.

- [ ] **Step 5: Lint and commit**

```bash
git add crates/waved-audio/Cargo.toml crates/waved-audio/src/lib.rs
git commit -m "feat(audio): apply SetBeamAim to the directional beam

- AudioService wraps the aim like waved-settings does, updates
  AudioState.beam_aim_deg and steers to it on the next period (C10)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 16: Stereo over USB (C4)

**Files:**
- Modify: `crates/waved-audio/src/processed_frame.rs`
- Modify: `crates/waved-audio/src/lib.rs`
- Modify: `crates/waved-audio/src/uac/upstream.rs`

**Design:** The beamformer's L/R pair is raw: it runs before AEC and noise suppression. Sending it as is would send the far end its own echo. Instead the pair is rebuilt around the processed mono:
- L = mid′ + g·side and R = mid′ − g·side, where mid′ is the AEC/NS output and side = (L − R)/2 from the beam.
- g is the mid's own suppression ratio, rms(mid′) / rms(raw mid), clamped to [0, 1]. It ramps linearly across each period from the previous value.
- (L + R)/2 equals the processed mono exactly, so recordings and the mono path don't change. The side is ducked whenever AEC or NS removes energy.

**Interfaces:**
- Produces:
  - `pub fn side_gain(raw_mid_rms: f32, processed_mid_rms: f32) -> f32`
  - `pub fn rebuild_stereo(raw_l: &[f32], raw_r: &[f32], processed_mid: &[f32], prev_side_gain: f32) -> (Vec<f32>, Vec<f32>, f32)`
  - `ProcessedAudioFrame::uplink_interleaved_i16(&self, out: &mut Vec<i16>)`
  - private field `AudioService.stereo_side_gain: f32`

- [ ] **Step 1: Write the failing tests**

Append to the tests module of `crates/waved-audio/src/processed_frame.rs` (create `#[cfg(test)] mod tests { use super::*; … }` at the end of the file if it doesn't exist):

```rust
    #[test]
    fn untouched_mid_gives_back_the_beam_pair() {
        let l = vec![0.4, 0.2, -0.1, 0.0];
        let r = vec![0.0, 0.2, 0.3, -0.2];
        let mid: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
        let (sl, sr, gain) = rebuild_stereo(&l, &r, &mid, 1.0);
        assert!((gain - 1.0).abs() < 1e-6);
        for i in 0..4 {
            assert!((sl[i] - l[i]).abs() < 1e-6 && (sr[i] - r[i]).abs() < 1e-6, "sample {i}");
        }
    }

    #[test]
    fn rebuilt_pair_averages_to_the_processed_mid() {
        let l = vec![0.5; 480];
        let r = vec![-0.1; 480];
        let processed = vec![0.05; 480]; // AEC/NS took the mid from 0.2 to 0.05
        let (sl, sr, gain) = rebuild_stereo(&l, &r, &processed, 0.25);
        assert!((gain - 0.25).abs() < 1e-6);
        for i in 0..480 {
            assert!((0.5 * (sl[i] + sr[i]) - processed[i]).abs() < 1e-6);
        }
        assert!((sl[0] - sr[0] - 0.6 * 0.25).abs() < 1e-5, "side ducked like the mid");
    }

    #[test]
    fn side_gain_ramps_across_the_period() {
        let l = vec![0.5; 100];
        let r = vec![-0.5; 100];
        let processed = vec![0.0; 100];
        // Raw mid is 0: no side at all, whatever came before.
        let (sl, _, gain) = rebuild_stereo(&l, &r, &processed, 1.0);
        assert_eq!(gain, 0.0);
        assert!(sl[0] > sl[99], "ramps down from the previous gain");
        assert!(sl[99].abs() < 1e-6);
    }

    #[test]
    fn side_gain_is_the_mid_suppression_ratio() {
        assert_eq!(side_gain(0.0, 0.1), 0.0);
        assert!((side_gain(0.2, 0.05) - 0.25).abs() < 1e-6);
        assert_eq!(side_gain(0.1, 0.5), 1.0, "never boosts");
    }

    #[test]
    fn uplink_sends_the_pair_in_stereo_and_mono_twice_otherwise() {
        let frame = AudioFrame::mic_only(vec![0; 3], 1, 48_000, 0);
        let mono = ProcessedAudioFrame::from_raw(&frame, &[0, 16_384, -16_384], &[0, 0, 0]);
        let mut out = Vec::new();
        mono.uplink_interleaved_i16(&mut out);
        assert_eq!(out, vec![0, 0, 16_383, 16_383, -16_383, -16_383]);
        let stereo = mono.with_stereo(vec![1.5, 0.0, -1.5], vec![-0.5, 0.5, 0.0]);
        stereo.uplink_interleaved_i16(&mut out);
        assert_eq!(out, vec![32_767, -16_383, 0, 16_383, -32_767, 0]);
    }
```

(`16_384 / 32768 = 0.5` and `0.5 × 32767 = 16383.5`, which truncates to 16 383.)

Append to `mod tests` in `crates/waved-audio/src/lib.rs`:

```rust
    #[test]
    fn stereo_uplink_pair_averages_to_the_mono_uplink() {
        let (mut svc, _state) = service_with(waved_state::AudioMode::Stereo, 0.0);
        let mut last = None;
        for _ in 0..4 {
            last = Some(svc.process_frame(&plane_wave_8ch(60.0, 480)));
        }
        let frame = last.unwrap();
        let (l, r) = frame.mic_stereo_f32.as_ref().expect("Stereo mode carries a pair");
        for i in 0..frame.mic_mono_f32.len() {
            let mid = 0.5 * (l[i] + r[i]);
            assert!((mid - frame.mic_mono_f32[i]).abs() < 1e-3, "sample {i}: {mid} vs {}", frame.mic_mono_f32[i]);
        }
    }
```

- [ ] **Step 2: Run and expect a failure**

Run `cargo test -p waved-audio --lib rebuild side_gain uplink stereo_uplink`.

Expected: build errors, because `rebuild_stereo` and `uplink_interleaved_i16` don't exist yet.

- [ ] **Step 3: Implement the pure parts**

Append to `crates/waved-audio/src/processed_frame.rs` (above the tests):

```rust
fn rms_f32(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// How much of the beam's side signal to keep: the fraction of the mid that
/// survived AEC and noise suppression, clamped to [0, 1]. Zero when the beam
/// was silent.
pub fn side_gain(raw_mid_rms: f32, processed_mid_rms: f32) -> f32 {
    if raw_mid_rms <= 1e-6 {
        return 0.0;
    }
    (processed_mid_rms / raw_mid_rms).clamp(0.0, 1.0)
}

/// Rebuild the Stereo-mode pair around the processed mid (C4). The raw beam
/// pair comes before AEC and NS, so it is not sent as is:
/// `L = mid' + g·side`, `R = mid' − g·side`, with `side = (L − R)/2` from
/// the beam and `g` the mid's suppression ratio, ramped linearly from
/// `prev_side_gain` across the period so it never clicks. `(L + R)/2` is
/// exactly the processed mid. Returns the pair and the gain reached.
pub fn rebuild_stereo(
    raw_l: &[f32],
    raw_r: &[f32],
    processed_mid: &[f32],
    prev_side_gain: f32,
) -> (Vec<f32>, Vec<f32>, f32) {
    let n = processed_mid.len().min(raw_l.len()).min(raw_r.len());
    let raw_mid: Vec<f32> = raw_l[..n]
        .iter()
        .zip(&raw_r[..n])
        .map(|(l, r)| 0.5 * (l + r))
        .collect();
    let target = side_gain(rms_f32(&raw_mid), rms_f32(&processed_mid[..n]));
    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    for i in 0..n {
        let gain = prev_side_gain + (target - prev_side_gain) * (i + 1) as f32 / n as f32;
        let side = 0.5 * (raw_l[i] - raw_r[i]) * gain;
        left.push(processed_mid[i] + side);
        right.push(processed_mid[i] - side);
    }
    (left, right, target)
}

fn to_uplink_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}
```

Add to `impl ProcessedAudioFrame`:

```rust
    /// Interleaved S16 for the 2-channel USB microphone: the Stereo pair
    /// when present, otherwise the mono signal on both channels.
    pub fn uplink_interleaved_i16(&self, out: &mut Vec<i16>) {
        out.clear();
        out.reserve(self.mic_mono_f32.len() * 2);
        match &self.mic_stereo_f32 {
            Some((left, right)) => {
                for (l, r) in left.iter().zip(right) {
                    out.push(to_uplink_i16(*l));
                    out.push(to_uplink_i16(*r));
                }
            }
            None => {
                for &m in &self.mic_mono_f32 {
                    let s = to_uplink_i16(m);
                    out.push(s);
                    out.push(s);
                }
            }
        }
    }
```

- [ ] **Step 4: Use them in the service and the uplink**

In `crates/waved-audio/src/lib.rs`:
- add the field `stereo_side_gain: f32,` to `AudioService` (initialised to `0.0` in `new`)
- in `handle_command`'s `SetAudioMode` arm, add `self.stereo_side_gain = 0.0;` (Stereo then fades its side in)
- in `process_frame`, replace

```rust
            if let Some((l, r)) = beam_out.stereo {
                p = p.with_stereo(l, r);
            }
```

with

```rust
            if let Some((l, r)) = beam_out.stereo {
                let (left, right, gain) = crate::processed_frame::rebuild_stereo(
                    &l,
                    &r,
                    &p.mic_mono_f32,
                    self.stereo_side_gain,
                );
                self.stereo_side_gain = gain;
                p = p.with_stereo(left, right);
            }
```

In `crates/waved-audio/src/uac/upstream.rs`:
- replace `mono_to_stereo_i16(&frame.mic_mono_f32, &mut stereo);` with `frame.uplink_interleaved_i16(&mut stereo);`
- delete `fn mono_to_stereo_i16` and its test module, which `uplink_sends_the_pair_in_stereo_and_mono_twice_otherwise` now covers
- update the module doc's second sentence to: "Writes the frame's uplink pair (`ProcessedAudioFrame::uplink_interleaved_i16`): the rebuilt Stereo pair in Stereo mode, the mono signal on both channels otherwise."

- [ ] **Step 5: Run and expect a pass**

```bash
set -o pipefail
cargo test -p waved-audio --lib 2>&1 | grep -E 'test result|FAILED|panicked'
scripts/linux-test.sh check -p waved-audio --features capture-alsa 2>&1 | tail -3
scripts/linux-test.sh test -p waved-audio --features capture-alsa --lib uac 2>&1 | grep -E 'test result'
```

Expected:
- all host tests pass
- the ALSA build compiles
- the `uac` tests pass

- [ ] **Step 6: Lint and commit**

```bash
git add crates/waved-audio/src/processed_frame.rs crates/waved-audio/src/lib.rs crates/waved-audio/src/uac/upstream.rs
git commit -m "feat(audio): send real stereo to the computer in Stereo mode

- rebuild L/R around the echo-cancelled mid with a side ducked by the mid's
  AEC/NS suppression and ramped per period (C4, BUG-15)
- the USB uplink writes the pair; mono modes still send mono on both channels
- (L+R)/2 equals the processed mono, so recordings are unchanged

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 17: On-device mic level (C5)

**Files:**
- Modify: `crates/waved-audio/src/lib.rs`

**Interfaces:**
- Consumes: `Command::SetMicVolume(u16)`, `waved_config::clamp_mic_volume(u16) -> u16`
- Produces: `AudioState.mic_volume` is always within 0..=800. The gain stage (`apply_gain`) already reads it every period and `UacControl` mirrors it to `hw:1`.

**Facts:**
- The host-side `PCM Playback Volume` control is not surfaced by kernel 5.4 `f_uac2` (see the `uac/control.rs` comments), so the device control (B4 Mic level, `SetMicVolume`) is the only runtime path.
- `waved-settings` persists the value.
- What's left for C5 is clamping in `AudioService` and an end-to-end test of the command path.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/waved-audio/src/lib.rs`:

```rust
    #[test]
    fn set_mic_volume_is_clamped_into_state() {
        let (mut svc, state) = service_with(waved_state::AudioMode::Omni, 0.0);
        svc.handle_command(Command::SetMicVolume(5_000));
        assert_eq!(state.borrow().mic_volume, 800);
        svc.handle_command(Command::SetMicVolume(0));
        assert_eq!(state.borrow().mic_volume, 0);
        svc.handle_command(Command::SetMicVolume(250));
        assert_eq!(state.borrow().mic_volume, 250);
    }

    #[tokio::test]
    async fn the_device_mic_level_reaches_the_gain_stage() {
        let state = Arc::new(watch::channel(AudioState { nr_level: NrLevel::Off, ..AudioState::default() }).0);
        let (frame_tx, frame_rx) = mpsc::channel(8);
        let (meeting_tx, mut meeting_rx) = mpsc::channel(8);
        let svc = AudioService::new(
            Arc::clone(&state),
            frame_tx.clone(),
            frame_rx,
            meeting_tx,
            256,
            48_000,
            crate::beam::MicGeometry::default(),
        );
        let (cmd_tx, cmd_rx) = broadcast::channel(8);
        let handle = tokio::spawn(svc.run(cmd_rx));
        cmd_tx.send(Command::SetMicVolume(0)).unwrap();
        let mut seen = state.subscribe();
        tokio::time::timeout(std::time::Duration::from_secs(1), seen.wait_for(|s| s.mic_volume == 0))
            .await
            .unwrap()
            .unwrap();
        frame_tx
            .send(AudioFrame::mic_only(vec![8_000; 256], 1, 48_000, 0))
            .await
            .unwrap();
        let out = tokio::time::timeout(std::time::Duration::from_secs(1), meeting_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(out.mic_mono_f32.iter().all(|&s| s == 0.0), "level 0 silences the uplink");
        cmd_tx.send(Command::Shutdown).unwrap();
        assert_eq!(handle.await.unwrap(), AudioServiceExit::StopRequested);
    }
```

- [ ] **Step 2: Run and expect a failure**

Run `cargo test -p waved-audio --lib set_mic_volume the_device_mic_level`.

Expected: `set_mic_volume_is_clamped_into_state` fails with `5000 != 800`.

- [ ] **Step 3: Implement**

In `handle_command`, replace the `SetMicVolume` arm with:

```rust
            Command::SetMicVolume(v) => {
                // The UI's Mic level and the host both land here; keep
                // AudioState in the documented 0..=800 range.
                let v = waved_config::clamp_mic_volume(v);
                self.audio_tx.send_modify(|s| s.mic_volume = v);
            }
```

- [ ] **Step 4: Run and expect a pass**

Run `cargo test -p waved-audio --lib`.

Expected: all pass.

- [ ] **Step 5: Lint and commit**

```bash
git add crates/waved-audio/src/lib.rs
git commit -m "feat(audio): clamp the on-device mic level

- SetMicVolume is clamped to 0..=800 before it reaches AudioState (C5)
- test the device path end to end: command -> state -> gain stage

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 18: Start-up microphone check (C6)

Needs contract request R1 (`waved_state::MicHealth`, `AudioState.mic_health`).

**Files:**
- Modify: `crates/waved-audio/src/mic_check.rs`
- Modify: `crates/waved-audio/src/lib.rs`
- Modify: `crates/waved-audio/src/stats_collector.rs`

**Interfaces:**
- Consumes: `MicCheck` (existing), `waved_state::MicHealth`
- Produces:
  - `pub struct MicHealthMonitor` with `WINDOW_MS: u64 = 3_000`, `MAX_ATTEMPTS: u32 = 10`, `new(channels: usize, sample_rate: u32) -> Self`, `is_done(&self) -> bool`, `push_i16(&mut self, interleaved: &[i16], channels: usize) -> Option<MicHealth>`
  - private `AudioService::check_mics(&mut self, frame: &AudioFrame)`
  - `AudioStats.{mic_health: String, mic_dead_mask: u8, mic_low_mask: u8}`

**Verdict rules (per 3 s window):**
- Any channel whose RMS is **exactly zero** gets `NotResponding`, with that channel in `dead_mask`. Real MEMS elements never produce digital silence; the quiet-desk floor is about 9 LSB RMS (`docs/AUDIO_HARDWARE.md`). This catches a whole array or PDM line delivering zeros, which `MicCheck` alone would call "inconclusive".
- Otherwise `MicCheck::verdict()` decides:
  - inconclusive: a quiet room, so retry the next window, at most 10 windows, after which the health stays `Unknown`
  - any `Dead` channel: `NotResponding`
  - any `Low` channel: `Degraded`
  - otherwise: `Ok`
- The verdict is final once given.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/waved-audio/src/mic_check.rs`:

```rust
    use waved_state::MicHealth;

    /// `frames` of interleaved i16 with channel c at ±amps[c] (RMS = |amp|).
    fn frames_i16(amps: &[i16], frames: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(amps.len() * frames);
        for f in 0..frames {
            let sign = if f % 2 == 0 { 1 } else { -1 };
            out.extend(amps.iter().map(|&a| a * sign));
        }
        out
    }

    #[test]
    fn healthy_array_is_ok_after_one_window() {
        let mut m = MicHealthMonitor::new(8, 1_000); // 3 000-frame window
        assert_eq!(m.push_i16(&frames_i16(&[3_000; 8], 2_000), 8), None);
        assert_eq!(m.push_i16(&frames_i16(&[3_000; 8], 1_000), 8), Some(MicHealth::Ok));
        assert!(m.is_done());
        assert_eq!(m.push_i16(&frames_i16(&[0; 8], 3_000), 8), None, "the verdict is final");
    }

    #[test]
    fn a_dead_element_is_not_responding() {
        let mut amps = [3_000i16; 8];
        amps[3] = 0;
        let mut m = MicHealthMonitor::new(8, 1_000);
        assert_eq!(
            m.push_i16(&frames_i16(&amps, 3_000), 8),
            Some(MicHealth::NotResponding { dead_mask: 0b0000_1000 })
        );
    }

    #[test]
    fn a_quiet_element_is_degraded() {
        let mut amps = [3_000i16; 8];
        amps[5] = 750; // 0.25 × median
        let mut m = MicHealthMonitor::new(8, 1_000);
        assert_eq!(
            m.push_i16(&frames_i16(&amps, 3_000), 8),
            Some(MicHealth::Degraded { low_mask: 0b0010_0000 })
        );
    }

    #[test]
    fn an_all_zero_capture_is_not_responding() {
        let mut m = MicHealthMonitor::new(8, 1_000);
        assert_eq!(
            m.push_i16(&frames_i16(&[0; 8], 3_000), 8),
            Some(MicHealth::NotResponding { dead_mask: 0xFF })
        );
    }

    #[test]
    fn a_quiet_room_retries_then_gives_up() {
        let mut m = MicHealthMonitor::new(8, 1_000);
        for _ in 0..MicHealthMonitor::MAX_ATTEMPTS {
            assert!(!m.is_done());
            assert_eq!(m.push_i16(&frames_i16(&[1; 8], 3_000), 8), None);
        }
        assert!(m.is_done(), "stays Unknown after the last attempt");
    }

    #[test]
    fn frames_with_another_channel_count_are_ignored() {
        let mut m = MicHealthMonitor::new(8, 1_000);
        assert_eq!(m.push_i16(&frames_i16(&[0; 2], 3_000), 2), None);
        assert!(!m.is_done());
    }
```

Append to `mod tests` in `crates/waved-audio/src/lib.rs`:

```rust
    #[tokio::test]
    async fn mic_check_hears_the_microphones_before_mute() {
        let state = Arc::new(watch::channel(AudioState {
            mic_muted: true, // mute zero-fills frames after the check
            nr_level: NrLevel::Off,
            ..AudioState::default()
        }).0);
        let (frame_tx, frame_rx) = mpsc::channel(8);
        let (meeting_tx, _meeting_rx) = mpsc::channel(8);
        let svc = AudioService::new(
            Arc::clone(&state),
            frame_tx.clone(),
            frame_rx,
            meeting_tx,
            256,
            48_000,
            crate::beam::MicGeometry::default(),
        );
        let (cmd_tx, cmd_rx) = broadcast::channel(8);
        let handle = tokio::spawn(svc.run(cmd_rx));
        // 2 channels, channel 1 digitally silent: 2 × 72 000 frames = one 3 s window.
        for _ in 0..2 {
            let data: Vec<i16> = (0..72_000).flat_map(|i| [if i % 2 == 0 { 900 } else { -900 }, 0]).collect();
            frame_tx.send(AudioFrame::mic_only(data, 2, 48_000, 0)).await.unwrap();
        }
        let mut seen = state.subscribe();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            seen.wait_for(|s| s.mic_health == waved_state::MicHealth::NotResponding { dead_mask: 0b10 }),
        )
        .await
        .expect("verdict published")
        .unwrap();
        cmd_tx.send(Command::Shutdown).unwrap();
        handle.await.unwrap();
    }
```

Append to the tests in `crates/waved-audio/src/stats_collector.rs`:

```rust
    #[test]
    fn mic_health_appears_in_stats() {
        let frame = frame_from(silence_8ch(1)[..8 * 256].to_vec());
        let cs = CaptureStats::new();
        let state = AudioState {
            mic_health: waved_state::MicHealth::NotResponding { dead_mask: 0b1000_0001 },
            ..Default::default()
        };
        let s = compute_stats_with_processed(
            &frame, &cs, ChannelMapSource::File, None, "none", None, None, &state, None,
            #[cfg(feature = "capture-alsa")]
            None,
            None,
        );
        assert_eq!(s.mic_health, "not_responding");
        assert_eq!(s.mic_dead_mask, 0b1000_0001);
        assert_eq!(s.mic_low_mask, 0);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"mic_health\":\"not_responding\""));
    }
```

- [ ] **Step 2: Run and expect a failure**

Run `cargo test -p waved-audio --lib mic`.

Expected: build errors, because `MicHealthMonitor` and `AudioStats.mic_health` don't exist yet.

- [ ] **Step 3: Implement the monitor**

Append to `crates/waved-audio/src/mic_check.rs` (above the tests):

```rust
use waved_state::MicHealth;

/// Runs `MicCheck` over the first seconds of capture and turns it into one
/// `MicHealth` verdict (C6). Feed raw frames before mic mute.
pub struct MicHealthMonitor {
    check: MicCheck,
    channels: usize,
    window_frames: u64,
    attempts_left: u32,
    scratch: Vec<f32>,
    done: bool,
}

impl MicHealthMonitor {
    pub const WINDOW_MS: u64 = 3_000;
    /// Quiet-room windows before giving up (30 s with the default window).
    pub const MAX_ATTEMPTS: u32 = 10;

    pub fn new(channels: usize, sample_rate: u32) -> Self {
        Self {
            check: MicCheck::new(channels.max(1)),
            channels,
            window_frames: u64::from(sample_rate) * Self::WINDOW_MS / 1_000,
            attempts_left: Self::MAX_ATTEMPTS,
            scratch: Vec::new(),
            done: channels == 0,
        }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Accumulate interleaved samples; returns the verdict once, when a
    /// window is conclusive.
    pub fn push_i16(&mut self, interleaved: &[i16], channels: usize) -> Option<MicHealth> {
        if self.done || channels != self.channels {
            return None;
        }
        self.scratch.clear();
        self.scratch
            .extend(interleaved.iter().map(|&s| f32::from(s) / 32_768.0));
        self.check.push_interleaved(&self.scratch);
        if self.check.samples_per_channel() < self.window_frames {
            return None;
        }
        let verdict = self.check.verdict();
        self.check.reset();
        let zero = mask(verdict.rms.iter().map(|&rms| rms == 0.0));
        let dead = mask(verdict.per_channel.iter().map(|&s| s == MicStatus::Dead));
        let low = mask(verdict.per_channel.iter().map(|&s| s == MicStatus::Low));
        if zero != 0 {
            self.done = true;
            return Some(MicHealth::NotResponding { dead_mask: zero | dead });
        }
        if verdict.inconclusive {
            self.attempts_left = self.attempts_left.saturating_sub(1);
            self.done = self.attempts_left == 0;
            return None;
        }
        self.done = true;
        Some(if dead != 0 {
            MicHealth::NotResponding { dead_mask: dead }
        } else if low != 0 {
            MicHealth::Degraded { low_mask: low }
        } else {
            MicHealth::Ok
        })
    }
}

/// Bit n set when the n-th flag is true (first 8 channels).
fn mask(flags: impl Iterator<Item = bool>) -> u8 {
    flags
        .take(8)
        .enumerate()
        .fold(0u8, |m, (i, set)| if set { m | (1 << i) } else { m })
}
```

- [ ] **Step 4: Run it in the service and expose it in stats**

In `crates/waved-audio/src/lib.rs`, add two fields to `AudioService`:

```rust
    /// Start-up microphone check; `None` before the first frame and after the verdict.
    mic_health: Option<crate::mic_check::MicHealthMonitor>,
    mic_check_done: bool,
```

Initialise them with `mic_health: None, mic_check_done: false,`. Add the method:

```rust
    /// Feed the start-up microphone check (C6). Called before mic mute so a
    /// muted boot is still checked.
    fn check_mics(&mut self, frame: &AudioFrame) {
        if self.mic_check_done {
            return;
        }
        let channels = frame.channels as usize;
        let monitor = self.mic_health.get_or_insert_with(|| {
            crate::mic_check::MicHealthMonitor::new(channels, frame.sample_rate)
        });
        if let Some(health) = monitor.push_i16(&frame.data, channels) {
            match health {
                waved_state::MicHealth::NotResponding { dead_mask } => {
                    warn!("microphone check: ALSA channels {dead_mask:#010b} are not responding")
                }
                waved_state::MicHealth::Degraded { low_mask } => {
                    warn!("microphone check: ALSA channels {low_mask:#010b} are much quieter than the rest")
                }
                other => info!("microphone check: {other:?}"),
            }
            self.audio_tx.send_modify(|s| s.mic_health = health);
        }
        if monitor.is_done() {
            if self.audio_tx.borrow().mic_health == waved_state::MicHealth::Unknown {
                info!("microphone check: the room stayed too quiet to judge");
            }
            self.mic_health = None;
            self.mic_check_done = true;
        }
    }
```

In `run`, right after the `let Some(mut frame) = frame else { … };` statement and **before** `let muted = …; apply_mic_mute(…)`, add:

```rust
                    self.check_mics(&frame);
```

In `crates/waved-audio/src/stats_collector.rs`, add to `AudioStats` after `recording_limiter_reduction_db`:

```rust
    /// Start-up microphone check: unknown / ok / degraded / not_responding.
    pub mic_health: String,
    /// ALSA channels that are silent (bit n = ALSA channel n).
    pub mic_dead_mask: u8,
    /// ALSA channels that are much quieter than the rest.
    pub mic_low_mask: u8,
```

Add a helper:

```rust
fn mic_health_fields(health: waved_state::MicHealth) -> (String, u8, u8) {
    use waved_state::MicHealth;
    match health {
        MicHealth::Unknown => ("unknown".into(), 0, 0),
        MicHealth::Ok => ("ok".into(), 0, 0),
        MicHealth::Degraded { low_mask } => ("degraded".into(), 0, low_mask),
        MicHealth::NotResponding { dead_mask } => ("not_responding".into(), dead_mask, 0),
    }
}
```

In `compute_stats_with_processed`, before `AudioStats {`, add `let (mic_health, mic_dead_mask, mic_low_mask) = mic_health_fields(audio_state.mic_health);` and add the three fields to the literal. Add `mic_health: "unknown".into(), mic_dead_mask: 0, mic_low_mask: 0,` to `sample_audio_stats()` in the tests.

- [ ] **Step 5: Run and expect a pass**

```bash
set -o pipefail
cargo test -p waved-audio --lib 2>&1 | grep -E 'test result|FAILED|panicked'
cargo test -p waved-audio --test mute_end_to_end --test steered_beam_handoff 2>&1 | grep -E 'test result'
scripts/linux-test.sh check -p waved-audio --features capture-alsa 2>&1 | tail -3
```

Expected:
- all pass
- the ALSA build compiles (`sample_audio_stats` is shared by both cfgs)

- [ ] **Step 6: Lint and commit**

```bash
git add crates/waved-audio/src/mic_check.rs crates/waved-audio/src/lib.rs crates/waved-audio/src/stats_collector.rs
git commit -m "feat(audio): check the microphones at start-up

- MicHealthMonitor runs MicCheck over 3 s windows of raw, pre-mute frames (C6)
- exactly-silent channels are not responding; a quiet room retries, then stays unknown
- the verdict lands in AudioState.mic_health and /tmp/audio_stats.json

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 19: "Computer is using the mic" and DoA confidence

**Files:**
- Modify: `crates/waved-audio/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub fn AudioService::set_host_listening_probe(&mut self, probe: impl Fn() -> bool + Send + 'static)`
  - writes `AudioState.host_listening` once per frame when a probe is set, and `AudioState.doa_confidence` once per 8-channel frame

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/waved-audio/src/lib.rs`:

```rust
    #[tokio::test]
    async fn host_listening_follows_the_probe() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let state = Arc::new(watch::channel(AudioState::default()).0);
        let (frame_tx, frame_rx) = mpsc::channel(8);
        let (meeting_tx, mut meeting_rx) = mpsc::channel(8);
        let mut svc = AudioService::new(
            Arc::clone(&state),
            frame_tx.clone(),
            frame_rx,
            meeting_tx,
            16,
            48_000,
            crate::beam::MicGeometry::default(),
        );
        let open = Arc::new(AtomicBool::new(true));
        let probe = Arc::clone(&open);
        svc.set_host_listening_probe(move || probe.load(Ordering::Relaxed));
        let (cmd_tx, cmd_rx) = broadcast::channel(4);
        let handle = tokio::spawn(svc.run(cmd_rx));
        frame_tx.send(AudioFrame::mic_only(vec![0; 16], 1, 48_000, 0)).await.unwrap();
        meeting_rx.recv().await.unwrap();
        assert!(state.borrow().host_listening);
        open.store(false, Ordering::Relaxed);
        frame_tx.send(AudioFrame::mic_only(vec![0; 16], 1, 48_000, 1)).await.unwrap();
        meeting_rx.recv().await.unwrap();
        assert!(!state.borrow().host_listening);
        cmd_tx.send(Command::Shutdown).unwrap();
        handle.await.unwrap();
    }

    #[test]
    fn doa_confidence_is_published_with_the_direction() {
        let (mut svc, state) = service_with(waved_state::AudioMode::Omni, 0.0);
        let mut doa = svc.subscribe_doa();
        svc.process_frame(&plane_wave_8ch(90.0, 480));
        let frame = doa.try_recv().unwrap();
        assert_eq!(state.borrow().doa_confidence, frame.confidence);
    }
```

- [ ] **Step 2: Run and expect a failure**

Run `cargo test -p waved-audio --lib host_listening doa_confidence`.

Expected:
- build error: `set_host_listening_probe` doesn't exist
- after a stub, `host_listening` stays false

- [ ] **Step 3: Implement**

In `crates/waved-audio/src/lib.rs`, add a field to `AudioService` (initialised to `None`):

```rust
    /// Reads whether the host has the USB microphone open (UAC "PCM Playback
    /// State"). main.rs installs it when UAC routing is on.
    host_listening_probe: Option<Box<dyn Fn() -> bool + Send>>,
```

Add the method:

```rust
    /// Publish `AudioState.host_listening` from this probe on every frame
    /// ("Computer is using the mic" on Home).
    pub fn set_host_listening_probe(&mut self, probe: impl Fn() -> bool + Send + 'static) {
        self.host_listening_probe = Some(Box::new(probe));
    }
```

In `run`, before `let integrity = self.integrity.snapshot();`, add:

```rust
                    let host_listening = self.host_listening_probe.as_ref().map(|probe| probe());
```

…and inside the `self.audio_tx.send_modify(|s| { … })` that follows, add:

```rust
                        if let Some(listening) = host_listening {
                            s.host_listening = listening;
                        }
```

In `process_frame`, extend the DoA `send_modify` with `s.doa_confidence = doa.confidence;`.

- [ ] **Step 4: Run and expect a pass**

Run `cargo test -p waved-audio --lib`.

Expected: all pass.

- [ ] **Step 5: Lint and commit**

```bash
git add crates/waved-audio/src/lib.rs
git commit -m "feat(audio): publish host listening and DoA confidence

- AudioService::set_host_listening_probe drives AudioState.host_listening
  (\"Computer is using the mic\"); main.rs wires it to UacStats.upstream_active
- AudioState.doa_confidence follows the latest DoA estimate

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## End-of-lane check (both lanes done)

- [ ] Run the full verification set and confirm every line passes:

```bash
set -o pipefail
cargo test -p waved-audio --lib
cargo test -p waved-audio --test mute_end_to_end --test steered_beam_handoff --test biquad_regression
scripts/linux-test.sh test -p waved-meeting --lib
scripts/linux-test.sh test -p waved-meeting --test recording_decision --test webhook_contract
scripts/linux-test.sh check -p waved-audio --features capture-alsa
scripts/linux-test.sh check -p waved --features ui,capture-alsa
cargo clippy -p waved-audio --all-targets 2>&1 | grep -E "^warning: .*generated"
scripts/linux-test.sh clippy -p waved-meeting --all-targets 2>&1 | grep -E '^warning: `waved-meeting`'
```

Expected:
- every test command passes and both `check` commands build
- the clippy counts are at or below the baselines in Global Constraints (`waved-audio`) and the Task 1 commit (`waved-meeting`)

## Device verification (end-of-M2 device pass, after H1–H3 are wired)

Deploy as in `waved/CLAUDE.md`. Unless noted, read `/tmp/audio_stats.json`, `/tmp/waved.log` and `/mnt/udisk/recordings/<id>/metadata.json` over adb.

1. **Pause and resume.**
   - Record 20 s, Pause, talk for 10 s, Resume, record 10 s, Stop.
   - `metadata.json` has one `recording.pauses` entry with `offset_ms` ≈ 20 000 and `duration_ms` ≈ 10 000.
   - The WAV is about 30 s plus the pre-roll, and the paused speech is absent when you listen.
   - The dial timer froze during the pause.
2. **Pre-recording.**
   - The Home status pill shows the teal dot at idle. Mute the mic: the dot disappears.
   - Turn System → Pre-recording off, then Record: `recording.pre_roll_duration_ms` is 0.
   - Turn it on, wait 40 s, Record: the pre-roll is about 30 000 ms.
3. **"Send this recording?".**
   - With Uploads set to Ask, record and stop, then answer nothing.
   - The countdown runs, and after 60 s Files shows the take as kept on Wave.
   - Repeat and tap Send at about 55 s: the take is sent, not kept.
4. **Delete.**
   - Delete a kept take from Files. Afterwards `ls -a /mnt/udisk/recordings` shows no `<id>` and no `.deleted-*`, and `/tmp/waved.log` has the `audit:` line.
   - Delete during an upload is refused and the log says so.
5. **Resend.** Turn Wi-Fi off, record in Automatic mode until the take fails, then turn Wi-Fi on and use "Send again". The receiver gets the take once (same Idempotency-Key).
6. **Fixed aim — confirm the sign convention.**
   - Pick Focused and drag the aim to 90° (the screen's right).
   - A talker on the screen's right must be clearly louder in a test recording than the same talker on the left.
   - **If it is the other way round**, flip the convention in `beam/aim.rs`: use `(360.0 - beam_aim_deg).rem_euclid(360.0)` in the directional arm and add a table test row. Record the result in `docs/AUDIO_HARDWARE.md` (docs handoff).
   - Also confirm the beam no longer jumps between two alternating talkers (BUG-17), and that `scripts/test-beamforming.sh` still passes (override path).
7. **Stereo over USB.**
   - Select Stereo and record from the Mac in Audacity, in stereo.
   - A talker at the screen's left is louder in L. If not, swap `az_l`/`az_r` in `beam/mod.rs` and record it.
   - During a call with far-end audio, the far end hears no more echo than in Whole room mode.
   - Compare `aec.effective_echo_suppression_db` in stats between Omni and Stereo.
8. **Mic level.**
   - Move Volume → Mic level. `mic_gain_linear` in stats follows (level / 100), and recordings get louder or quieter.
   - The value survives a restart (`audio.mic_volume` in `config.toml`).
9. **Mic check.** The boot log shows `microphone check: Ok` within about 3 s, and stats show `"mic_health":"ok"`. A covered mic is not a fault (it is still above the dead threshold).
10. **Playback.**
    - Files → detail → Play: the speaker plays and the scrubber moves (250 ms steps). Seeking to a mark jumps there.
    - With Mac music playing, both are heard (mixed).
    - Start a call (HID off-hook): playback pauses.
    - Start a recording: playback pauses.
11. **"Computer is using the mic".** Open the Wave mic in a Mac app: the Home ring turns teal with "Computer is using the mic". Close it: the ring returns to grey.
12. **CPU.** Compare total waved CPU (`/proc/<pid>/stat` over 15 s, as in `docs/AUDIO_HARDWARE.md`) idle vs Stereo vs playing. Stereo rebuild and playback should add under 1 % of one core.

## Self-review

- **Spec coverage.**
  - C1 → 5/6, C2 → 3, C3 → 14, C4 → 16, C5 → 17, C6 → 18, C7 → 8/9/10, C8 → 12/13, C10 → 15, C11 → 2, C12 → 4.
  - BUG-15/16/17 → 16/6/14. The two failing tests → 1.
  - C9 is excluded and said so.
  - State that the M2 screens read but nobody produced (marks, current_speaker, FilesState, PlaybackState, host_listening, doa_confidence) has a producer task.
- **Types checked against `374015f`:**
  - `MeetingState.{paused, marks: u32, current_speaker: Option<u8>, ask_deadline_secs: Option<u32>, pre_recording_active}`
  - `SettingsState.{pre_recording, ask_timeout_s: u32, upload_mode}`
  - `FilesState { recordings, free_bytes: u64, low_space }`
  - `RecordingSummary { id, started_at: Option<String>, duration_secs: u32, size_bytes: u64, status, marks: u32, last_error: Option<String> }`
  - `RecordingStatus::{AwaitingDecision, LocalOnly, Waiting, Uploading, Uploaded, Failed, Damaged}` with `needs_attention()`
  - `PlaybackState { recording, playing, position_ms: u32, duration_ms: u32, marks_ms: Vec<u32> }`
  - `Command::{SeekPlayback(u32), SetBeamAim(f32), SetMicVolume(u16)}`
  - `waved_config::{wrap_beam_aim, clamp_mic_volume}`, `waved_settings::settings_from_config`
  - `hound::WavReader::seek(u32)` (3.5.1), `NaiveDateTime::and_utc` (chrono 0.4.44)
- **Placeholders.** No step says "TBD" or "similar to". The Task 1 commit body asks for the measured baseline, which only exists after Step 2.
- **Review focus.** Each of the five has a named test in the task listed.
