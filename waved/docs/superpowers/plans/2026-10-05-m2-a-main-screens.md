# Display Redesign — Milestone 2, Workstream A: Main Screens — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the revision-2 main screens on the milestone 1.1 kit: Home (A1), Recording (A2), After stop (A3), the resting screen (A4), Start-up and First start (A5), and the message copy and feed (A6). Each screen is a `kit::screen::Screen` in `waved-ui/src/screens_v2/`, tested on the host against the approved mockups. Nothing is wired into the runtime here; the integrator's runtime switch registers the screens at the end of milestone 2.

**Architecture:**
- **One shared module, `screens_v2/main_parts.rs`.** It holds what Home, Recording, After stop and the resting screen share: the ring looks from the dial table, `MainDial` (the dial, Quieter / Louder and the 1.5 s volume gauge, with stale-snapshot-safe stepping), and pure formats (timer, countdown, size, server host).
- **Pure views, thin widgets.** Every screen computes a `…View` struct from the `UiSnapshot` with a pure function (unit-tested against the README dial table), and applies it to LVGL only when it changed. Goldens check the look; view tests check the logic.
- **Messages.** `screens_v2/messages.rs` owns every message string (`copy`), tested to fit one status line, and a pure `MessageFeed` that turns `DeviceEvent`s and snapshot changes (battery thresholds, microphone fault) into `Message`s. Screens show their own action messages with `ctx.dispatch.show(..)`. The status slot of whatever screen is visible shows them, through a snapshot field (contract request 2), so a message survives the screen change that often follows it (Send → Home).
- **No navigation through the bus.** Screens send `Command`s for services and use the dispatcher for UI events. The base screen (Home / Recording / After stop) follows the meeting state through the existing `Router::sync`.

**Tech Stack:** Rust 2021 (stable 1.94), LVGL 9.2 via bindgen, the milestone 1 kit (`waved-ui/src/kit`), `tokio` broadcast + `std::mpsc` (dispatcher), `uuid` 1, `png` 0.17 goldens.

**Spec:**
- `../docs/mockups/index.html` and `../docs/mockups/png/{home,home-muted,home-volume,ambient,recording,recording-muted,hold-stop,paused,decision,boot,first-run}.png` (approved on the panel, commit `c244c9f`). **These are authoritative over `../TODO.md` wording**: Record has no "includes last 30 s" line (a pre-recording dot leads the status pill instead); Recording uses round Mark | Stop (hold) | Pause; After stop uses round Keep | Send | Delete in the same layout.
- `../docs/mockups/README.md` (dial states, timings, bottom actions, interaction model).
- `../TODO.md` workstream A (A1–A6) and decisions D1, D3, D4, D10.
- `crates/waved-ui/src/kit/README.md` (kit rules) and `docs/superpowers/plans/2026-10-05-display-relayout-kit.md` (geometry).

All code paths are relative to `waved/`. Paths starting with `../` point into the monorepo root.

---

## Global Constraints

Everything here is binding for every task.

- **Panel and tap limit.** Framebuffer 400 × 304. Visible circle **r 198 around (200, 106)**, cut flat at row 0. Every tappable pixel within **r 190 of (200, 106)** and at **y ≥ 6**; targets ≥ 44 px, main actions ≥ 50 px. Every screen test calls `assert_targets_ok(root)`.
- **Geometry tokens used** (`kit::tokens`): `STATUS_PILL` (80, 6, 240, 28) with hit area `STATUS_PILL_HIT` (80, 6, 240, 44); `STATUS` (80, 8, 240, 20); `TITLE_Y` 38; dial `DIAL_CX` 200, `DIAL_CY` 121, `DIAL_R` 70 (stroke 3; tap circle r 71); Home stack `DIAL_ICON_X` 190, `DIAL_ICON_Y` 73, `DIAL_TITLE_Y` 95, `DIAL_LINE_Y` 127, `DIAL_HINT_Y` 153; Recording stack `REC_CHIP_Y` 76, `REC_TIMER_Y` 104 (the 48 px digits' box top: baseline y 138), `REC_LINE_Y` 140, `REC_HINT_Y` 161, `REC_DOT` 7, `REC_DOT_GAP` 5; side buttons `SIDE_LEFT` (26, 91, 60, 60) / `SIDE_RIGHT` (314, 91, 60, 60), labels at `SIDE_LABEL_Y` 155; bottom `BOTTOM_MAIN` (168, 209, 64, 64), `BOTTOM_LEFT` (85, 181, 56, 56), `BOTTOM_RIGHT` (259, 181, 56, 56); hold ring r 35 stroke 3; `MESSAGE_MAX_W` 240.
- **Screen-only positions** (used by one screen file) are named `const`s at the top of that file, each citing its mockup element (CSS `top` from `index.html`; a 48 px value placed like `REC_TIMER_Y`, i.e. CSS top + 11). Colours come only from `kit::tokens`.
- **Colours:** BG `000000`, SURFACE `141c1e`, RAISED `1f292b`, LINE `2a3639`, TEXT `f2f5f4`, TEXT_2 `9aa7a6`, TEXT_3 `5d6a6a`, LIVE `3fd6b1`, REC `ff4b3e`, WARN `ffb547`. Teal = live/selected/healthy, red = recording/muted/destructive, amber = paused/attention, everything else grey. Red is never a button fill.
- **Type** (`kit::fonts::Font`): Num48 (line 43, baseline 34 from top), Num26 (line 24, baseline 19), Num20 (line 23, baseline 19), Dial 26, Title 20, Body 15, Small 13, Micro 12. Never hard-code a line height; use `Font::line_height()` / `one_line()` / `centered_text()`.
- **Text inside the ring:** any line drawn inside the r 70 ring is at most **`RING_TEXT_W` = 124 px** wide (`main_parts`); the resting screen's lower line at most 112 px. Lines that can be longer fall back to Small, then truncate with "…" inside their width.
- **Timings:** messages 2 s (success/info) / 4 s (warning/error) (`Severity::duration_ms`); volume gauge `GAUGE_MS` 1 500 ms after the last step; Quieter/Louder step 10 % and repeat every 300 ms while held (kit `SideButton`); hold to stop `HOLD_TO_STOP_MS` 1 000 ms; "Send this recording?" auto-Keep 60 s (owned by the service, C7: the screen shows `MeetingState::ask_deadline_secs`); Record double-tap guard `RECORD_DEBOUNCE_MS` 2 000 ms; start-up ring `RING_MS` 1 200 ms.
- **Copy:** sentence case, plain words, no "…" placeholders; a button names its action and the message that follows uses the same verb (Keep → "Kept on Wave", Delete → "Recording deleted", Send → "Sending recording"); every message fits one status line (text ≤ `MESSAGE_MAX_W − 44` = 196 px in Small). D1: a short tap on Stop shows "Hold Stop to end the recording"; holding shows "Keep holding to stop" (hold-stop.png).
- **Kit rules** (`kit/README.md`): screens use `Obj`, `event::on`/`on_click` and components; no raw `crate::bindings` calls in screens (event-code and state constants are fine); user settings come from `snap.settings` / `snap.audio`, never `ctx.config`; screens never navigate through bus commands.
- **Ownership:** this workstream edits only `crates/waved-ui/src/screens_v2/{home,recording,decision,ambient,boot,first_run,messages,main_parts}.rs` and new goldens `crates/waved-ui/golden/{home,home_muted,home_volume,recording,recording_muted,recording_hold_stop,recording_paused,recording_hours,decision,decision_no_server,decision_delete,ambient,ambient_recording,boot,first_run}.png`. No legacy UI file, kit file, state, config or `main.rs` edits.
- **Commits:** `type(scope): lowercase summary`, bullet-list body, ending with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
- **Commands** (from `waved/`, always with `set -o pipefail`):
  - Tests: `cargo test -p waved-ui --lib screens_v2::<module>` per task; `cargo test -p waved-ui --lib` before each commit.
  - Goldens: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::<module>`, then compare `target/ui-preview/<name>.png` with `../docs/mockups/png/<mockup>.png` (880 × 880 at 2×; the framebuffer sits at x 40..840, y 228..836). A mismatch writes `target/golden-diff/<name>.actual.png`.
  - Lint gate (the legacy files have clippy findings; only kit and screens_v2 must be clean):
    `cargo clippy -p waved-ui --all-targets --message-format=short 2>&1 | grep -E '^crates/waved-ui/src/(kit|screens_v2)/' ; test $? -eq 1`
    Expected: no output, exit 0.

## Review Focus

Five inputs the happy-path goldens don't exercise but that would bite on the device. Each has a named test.

1. **Holding Louder while the snapshot lags.** The service applies `SetSpeakerVolume` a frame or more later, so a naive step from `snap.audio.speaker_volume` resends 70, 70, 70. Steps must climb from the last value sent (60 → 70 → 80 → 90 → 100, then nothing), and a volume change from the host must still show the gauge. *(Task 3: `holding_louder_climbs_from_the_local_target_not_the_stale_snapshot`, `the_gauge_shows_for_1_5_s_after_a_step_and_after_a_host_change`)*
2. **Double taps on one-shot actions.** Tapping Send then Delete (or Keep twice) on After stop must send exactly one command; a double tap on Record must start one recording. *(Task 6: `a_decision_is_sent_once_and_the_buttons_lock`; Task 4: `record_starts_once_and_explains_when_blocked`)*
3. **Text inside the ring.** The timer after 59:59 ("1:02:03" is 161 px in Num48, the ring leaves 124) and long dial lines ("Connected by Bluetooth", a long Bluetooth name) must stay inside the ring. *(Task 1: `every_timer_fits_inside_the_ring`; Task 4: `every_home_line_fits_inside_the_ring`; Task 5: `the_timer_past_an_hour_fits_inside_the_ring`; Task 7: `every_resting_line_fits_inside_the_ring`)*
4. **Webhook endpoints that are empty, odd or carry credentials.** "To …" must show only the host (never `user:secret@`, port, path or query), and without a server After stop offers only Keep and Delete. *(Task 1: `the_server_line_shows_only_the_host`; Task 6: `without_a_server_only_keep_and_delete_show`)*
5. **Battery state before the MCU reports, and levels flapping around 10 %.** `BatteryState::default()` is level 0, so a naive rule warns "Battery 0%" at every boot; a level bouncing 10 ↔ 11 must warn once. *(Task 2: `battery_messages_fire_once_per_threshold_and_not_before_a_reading`)*

---

## Contract requests

The integrator lands these in M2.0, before any task below runs. Tasks assume they exist.

1. **Module declarations** — `crates/waved-ui/src/screens_v2/mod.rs` (and `pub mod screens_v2;` in `lib.rs`), with an empty stub file for `main_parts.rs` next to the other stubs:
   ```rust
   pub mod ambient;
   pub mod boot;
   pub mod decision;
   pub mod first_run;
   pub mod home;
   pub mod main_parts; // workstream A: pieces shared by Home, Recording, After stop and Resting
   pub mod messages;
   pub mod recording;
   ```
   Why: Home, Recording, After stop and the resting screen share the dial, the gauge and the formats; one A-owned module avoids copying them into four files. `pub` so the items are reachable and clippy reports no dead code before the runtime switch.

2. **The visible message lives in the snapshot** — `crates/waved-ui/src/kit/snapshot.rs`, `kit/components/status.rs`, runtime:
   ```rust
   // kit/snapshot.rs
   use crate::kit::dispatch::Message;

   /// The message in the status slot. The runtime keeps it, so it survives a
   /// screen change (Send on After stop, then Home shows "Sending recording").
   #[derive(Clone, Debug, PartialEq, Eq)]
   pub struct ActiveMessage {
       /// Increments for every message shown, so the same text twice shows twice.
       pub seq: u64,
       pub message: Message,
       /// `now_ms` when the runtime accepted it. It expires at
       /// `since_ms + message.severity.duration_ms()`.
       pub since_ms: u64,
   }

   pub struct UiSnapshot {
       // ...existing fields...
       /// Set by the runtime from `UiEvent::Show`; `capture` leaves it `None`.
       pub message: Option<ActiveMessage>,
   }
   ```
   ```rust
   // kit/components/status.rs: new field `message_seq: Option<u64>` (None in build),
   // and at the top of `StatusArea::update`:
   if let Some(active) = &snap.message {
       if self.message_seq != Some(active.seq) {
           self.message_seq = Some(active.seq);
           if snap.now_ms < active.since_ms + active.message.severity.duration_ms() {
               self.show(&active.message, active.since_ms);
           }
       }
   }
   ```
   Runtime (at the switch): on `UiEvent::Show(m)`, `seq += 1; active = Some(ActiveMessage { seq, message: m, since_ms: now_ms })`, copied into every `UiSnapshot`. Why: `Screen` has no message hook, and screens are rebuilt on every navigation, so a message sent just before a base-screen change (After stop → Home, Stop → After stop) would otherwise be lost. Kit test to add: `a_snapshot_message_shows_on_a_new_status_area_until_it_expires`.

3. **Microphone fault flag** — `crates/waved-state/src/lib.rs`, `AudioState`:
   ```rust
   /// The start-up microphone check (C6) found a dead or missing element.
   pub mic_fault: bool, // Default: false
   ```
   Producer: workstream C (C6). A6 shows "Microphones not responding" when it turns true. If C's own contract request already adds an equivalent field, use that one and rename `mic_fault` in Task 2 accordingly.

4. **Speakers and marks on the pending decision** — `crates/waved-state/src/lib.rs`, `PendingDecision`:
   ```rust
   /// Distinct speakers the diarizer numbered in this recording (0 = unknown).
   pub speakers: u8,
   /// Bookmarks in this recording.
   pub marks: u32,
   ```
   The integrator sets both to `0` in the existing struct literal (`crates/waved-meeting/src/lib.rs:170`) and in `kit/router.rs` tests so everything compiles; workstream C fills them from the bookmark count and the diarizer. Why: decision.png reads "184 MB, 3 speakers, 4 marks". A zero part is left out of the line, so 0 is safe.

5. **Dial-line copy that fits the ring** — `crates/waved-ui/src/kit/snapshot.rs`, `UiSnapshot::connection_line`: return **"Computer is listening"** instead of "Computer is using the mic" (and update its test). Why: the Home status line sits inside the r 70 ring, which leaves 124 px; "Computer is using the mic" is 153 px in Body and 133 px in Small, so it would cross the ring. "Computer is listening" is 125 px in Body and fits in Small (~108 px). `docs/mockups/README.md`'s dial table should follow (user to confirm the wording).

6. **Runtime-switch wiring** (integrator, end of M2; no code needed from A before then):
   - Routes → screens: `Route::Boot` → `BootScreen::default()`, `Route::FirstRun` → `FirstRunScreen::default()`, `Route::Home` → `HomeScreen::default()`, `Route::Recording` → `RecordingScreen::default()`, `Route::Decision` → `DecisionScreen::default()`, `Route::Resting` → `AmbientScreen::default()` (all `pub` in their modules).
   - Messages: one `screens_v2::messages::MessageFeed::new()` in the UI loop. For each `DeviceEvent`: `if let Some(m) = feed.on_event(&event, &snap) { show(m) }`; once per loop: `for m in feed.on_snapshot(&snap) { show(m) }`, where `show` is the same path as `UiEvent::Show`.
   - `UiEvent::BootDone` is sent by `BootScreen` itself once its ring has drawn (`RING_MS`); the runtime only needs to route it to the router (it already does).

---

## TODO coverage

| Item | Covered here | Left out (owner) |
|---|---|---|
| A1 Home | Dial tap mutes; Quieter/Louder with hold-repeat and the 1.5 s gauge (also for host-side changes); status pill opens Menu; round Record (no "includes last 30 s": the kit pill's pre-recording dot replaces it); ring states incl. "In a call" (HID off-hook) and the computer using the mic (`host_listening`); Record refuses with a message when storage or integrity blocks it | Producing `pre_recording_active` (C12); swipe-down gesture (integrator, runtime switch) |
| A2 Recording | Same dial and sides; Mark / Stop (1 s hold, tap explains) / Pause⇄Resume; muted and paused states; h:mm:ss after 59:59 (Num26 so it fits the ring); "Marked at mm:ss" | Pause/resume handling in the service (C1) |
| A3 After stop | "Send this recording?" with Keep / Send / Delete; Delete confirms first (kit `confirm_dialog`, on this screen); countdown from `ask_deadline_secs`; no Send without a server | The 60 s auto-Keep itself (C7, service side) |
| A4 Resting screen | Clock, mic state, pre-recording chip; recording variant with ring and timer; nothing tappable | Backlight dim/off timers and "never fully off while recording" (E1); first-touch wake guard and `UiEvent::Rest`/`Wake` routing (integrator) |
| A5 Start-up, first start | Ring draws itself over 1.2 s, then `BootDone`; first start with "Set up Wi-Fi" (opens Wi-Fi setup) and "Use without Wi-Fi", both marking first start done | Starting the UI earlier in boot (integrator); the Wi-Fi setup screen (B7) |
| A6 Messages | Copy for every legacy message (uploads, integrity faults, recording unavailable, stop, bookmark) rewritten to the revision-2 rules, plus "Marked at", "Hold Stop to end the recording", "Keep holding to stop", battery 10 % / 5 % and "Microphones not responding"; `MessageFeed` | "Shutting down"/power-off copy (B6/E2 own the confirmation and its message); the battery 3 % shutdown (F2, milestone 3); low-storage message (D3); the legacy "Volume: n%" toast is replaced by the dial gauge on Home/Recording (B4's Volume screen shows its own value) |

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/waved-ui/src/screens_v2/main_parts.rs` | Ring looks, volume steps, formats (timer, countdown, size, host), `ring_font`, `MainDial`, test fixtures | Modify (stub from M2.0) |
| `crates/waved-ui/src/screens_v2/messages.rs` | `copy` (every message string), `MessageFeed` | Modify (stub) |
| `crates/waved-ui/src/screens_v2/home.rs` | `HomeScreen`, `HomeView`, `record_blocked` | Modify (stub) |
| `crates/waved-ui/src/screens_v2/recording.rs` | `RecordingScreen`, `RecordingView` | Modify (stub) |
| `crates/waved-ui/src/screens_v2/decision.rs` | `DecisionScreen`, `DecisionView`, `decision_detail`, `countdown_text` | Modify (stub) |
| `crates/waved-ui/src/screens_v2/ambient.rs` | `AmbientScreen`, `AmbientView`, `resting_clock` | Modify (stub) |
| `crates/waved-ui/src/screens_v2/boot.rs` | `BootScreen` | Modify (stub) |
| `crates/waved-ui/src/screens_v2/first_run.rs` | `FirstRunScreen` | Modify (stub) |
| `crates/waved-ui/golden/*.png` | The 15 goldens listed in Global Constraints | Create |

## Execution Rules

- One worktree, branch `m2-a-main-screens` from `feat/display-redesign` **after** the M2.0 contract commit. Tasks run in order (each consumes the previous ones' interfaces).
- Every task: failing test first, then implementation, then green tests, the lint gate, and a commit.
- Golden flow: the first run of a new golden test fails with "missing golden". Run once with `UPDATE_GOLDEN=1 UI_PREVIEW=1`, **open** `target/ui-preview/<name>.png` next to the named mockup PNG, fix until it matches (apart from the accepted differences listed in the task), regenerate, commit the PNG with the code.
- LVGL is a global singleton: every LVGL test runs inside `testkit::lvgl(|lv| …)`.

---

### Task 1: Shared ring looks, volume steps and formats

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/main_parts.rs`

**Interfaces:**
- Consumes: `kit::components::dial::RingStyle`, `kit::fonts::Font`, `kit::obj::text_width`, `kit::snapshot::UiSnapshot`, `kit::tokens::REC_TIMER_Y`, `kit::dispatch::{Dispatcher, Message, Severity, UiEvent}`, `kit::snapshot::ActiveMessage` (contract 2).
- Produces:
  - `pub const VOLUME_STEP: u16 = 10; pub const VOLUME_MAX: u16 = 100; pub const TIMER_HOURS_Y: i32 = 112; pub const RING_TEXT_W: i32 = 124;`
  - `pub struct RingLook { pub style: RingStyle, pub talker: Option<i16> }` (Copy, PartialEq, Debug)
  - `pub fn home_ring(snap: &UiSnapshot) -> RingLook`, `pub fn recording_ring(snap: &UiSnapshot) -> RingLook`
  - `pub fn step_volume(volume: u16, up: bool) -> u16`
  - `pub fn format_timer(secs: u32) -> String`, `pub fn timer_font(secs: u32) -> (Font, i32)`, `pub fn format_countdown(secs: u32) -> String`, `pub fn format_size(bytes: u64) -> String`, `pub fn server_host(endpoint: &str) -> Option<String>`, `pub fn ring_font(text: &str, preferred: Font) -> Font`
  - `#[cfg(test)] pub mod fixtures { pub fn ctx() -> (ScreenCtx, broadcast::Receiver<Command>, mpsc::Receiver<UiEvent>); pub fn snap() -> UiSnapshot; pub fn sent(&mut broadcast::Receiver<Command>) -> Vec<String>; pub fn events(&mpsc::Receiver<UiEvent>) -> Vec<UiEvent>; pub fn with_message(&mut UiSnapshot, Severity, &str) }`

- [ ] **Step 1: Write the failing tests**

Append to `main_parts.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::lvgl;

    #[test]
    fn volume_steps_land_on_tens_inside_0_to_100() {
        assert_eq!(step_volume(60, true), 70);
        assert_eq!(step_volume(65, true), 70);
        assert_eq!(step_volume(95, true), 100);
        assert_eq!(step_volume(100, true), 100);
        assert_eq!(step_volume(60, false), 50);
        assert_eq!(step_volume(65, false), 60);
        assert_eq!(step_volume(1, false), 0);
        assert_eq!(step_volume(0, false), 0);
        assert_eq!(step_volume(250, true), 100, "host values past 100 clamp");
        assert_eq!(step_volume(250, false), 90);
    }

    #[test]
    fn timers_switch_to_hours_after_59_59() {
        assert_eq!(format_timer(0), "00:00");
        assert_eq!(format_timer(2_537), "42:17");
        assert_eq!(format_timer(3_599), "59:59");
        assert_eq!(format_timer(3_600), "1:00:00");
        assert_eq!(format_timer(3_723), "1:02:03");
        assert_eq!(format_timer(360_000), "100:00:00");
        assert_eq!(timer_font(3_599), (Font::Num48, REC_TIMER_Y));
        assert_eq!(timer_font(3_600), (Font::Num26, TIMER_HOURS_Y));
        assert_eq!(format_countdown(52), "0:52");
        assert_eq!(format_countdown(60), "1:00");
    }

    #[test]
    fn every_timer_fits_inside_the_ring() {
        lvgl(|_| {
            for secs in [0, 2_537, 3_599, 3_600, 35_999, 359_999] {
                let (font, _) = timer_font(secs);
                let width = text_width(&format_timer(secs), font);
                assert!(width <= RING_TEXT_W, "{secs} s is {width} px");
            }
        });
    }

    #[test]
    fn sizes_read_in_decimal_megabytes_then_gigabytes() {
        assert_eq!(format_size(184_000_000), "184 MB");
        assert_eq!(format_size(1_400_000), "1 MB");
        assert_eq!(format_size(120_000), "Under 1 MB");
        assert_eq!(format_size(999_600_000), "1.0 GB");
        assert_eq!(format_size(1_250_000_000), "1.3 GB");
    }

    #[test]
    fn the_server_line_shows_only_the_host() {
        for (endpoint, host) in [
            ("https://notes.example.com/hook", Some("notes.example.com")),
            (
                "https://user:secret@notes.example.com:8443/hook?token=x",
                Some("notes.example.com"),
            ),
            ("http://[fd00::1]:8080/hook", Some("[fd00::1]")),
            ("notes.example.com/hook", Some("notes.example.com")),
            ("", None),
            ("   ", None),
            ("https://", None),
            ("https://user@/x", None),
        ] {
            assert_eq!(server_host(endpoint).as_deref(), host, "{endpoint:?}");
        }
    }

    #[test]
    fn ring_looks_follow_the_dial_table() {
        let mut snap = UiSnapshot::default();
        assert_eq!(
            home_ring(&snap),
            RingLook { style: RingStyle::Idle, talker: None }
        );
        snap.audio.doa_active = true;
        snap.audio.doa_horizontal = 306.6;
        assert_eq!(home_ring(&snap).talker, None, "no wedge while nothing listens");
        snap.audio.host_listening = true;
        assert_eq!(
            home_ring(&snap),
            RingLook { style: RingStyle::Live, talker: Some(307) }
        );
        snap.audio.host_listening = false;
        snap.audio.call_off_hook = true;
        assert_eq!(home_ring(&snap).style, RingStyle::Live, "in a call");
        snap.audio.mic_muted = true;
        assert_eq!(
            home_ring(&snap),
            RingLook { style: RingStyle::Muted, talker: None }
        );

        let mut rec = UiSnapshot::default();
        rec.meeting.recording = true;
        rec.audio.doa_active = true;
        rec.audio.doa_horizontal = 60.0;
        assert_eq!(
            recording_ring(&rec),
            RingLook { style: RingStyle::Recording, talker: Some(60) }
        );
        rec.audio.doa_active = false;
        assert_eq!(recording_ring(&rec).talker, None, "no wedge without voice");
        rec.meeting.paused = true;
        assert_eq!(
            recording_ring(&rec),
            RingLook { style: RingStyle::Paused, talker: None }
        );
        rec.audio.mic_muted = true;
        assert_eq!(recording_ring(&rec).style, RingStyle::RecordingMuted, "mute wins over pause");
    }

    #[test]
    fn long_ring_lines_drop_to_small_type() {
        lvgl(|_| {
            assert_eq!(ring_font("In a call", Font::Body), Font::Body);
            assert_eq!(ring_font("Connected by Bluetooth", Font::Body), Font::Small);
        });
    }
}
```

- [ ] **Step 2: Run and confirm the failure**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::main_parts 2>&1 | tail -20`
Expected: compile errors `cannot find function `step_volume`` (and the other new items).

- [ ] **Step 3: Implement**

Put this above the tests in `main_parts.rs`:

```rust
//! Pieces shared by Home, Recording, After stop and the resting screen: ring
//! looks from the dial table (docs/mockups/README.md, "Dial states"),
//! Quieter / Louder steps, and the formats for timers, sizes and the upload
//! server. `MainDial` (Task 3) puts the dial, the side buttons and the volume
//! gauge together.

use crate::kit::components::dial::RingStyle;
use crate::kit::fonts::Font;
use crate::kit::obj::text_width;
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::REC_TIMER_Y;

/// One Quieter / Louder step ("Interaction model": speaker −/+ 10 %).
pub const VOLUME_STEP: u16 = 10;
pub const VOLUME_MAX: u16 = 100;
/// From an hour on, the timer (h:mm:ss) is drawn in Num26: "1:02:03" is
/// 161 px in Num48 and would cross the ring. Num26 digits fill the top 19 px
/// of its 24 px box, so top 112 centres them on the dial's centre line (121).
pub const TIMER_HOURS_Y: i32 = 112;
/// Widest text that stays clear of the r 70 ring (stroke 3) on the rows of
/// the timer and the status line: the ring's inner chord there is ~130 px.
pub const RING_TEXT_W: i32 = 124;

/// What the dial ring shows: a style and, while someone talks, the wedge's
/// direction in whole device degrees (whole degrees keep redraws rare).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RingLook {
    pub style: RingStyle,
    pub talker: Option<i16>,
}

/// The wedge, while voice is detected and the dial style follows talkers.
fn talker_if(snap: &UiSnapshot, follows: bool) -> Option<i16> {
    (follows && snap.audio.doa_active).then(|| snap.audio.doa_horizontal.round() as i16)
}

/// Home's ring: red when muted, teal while a call or the computer uses the
/// microphone, grey otherwise.
pub fn home_ring(snap: &UiSnapshot) -> RingLook {
    let style = if snap.audio.mic_muted {
        RingStyle::Muted
    } else if snap.in_call() || snap.audio.host_listening {
        RingStyle::Live
    } else {
        RingStyle::Idle
    };
    RingLook {
        style,
        talker: talker_if(snap, style == RingStyle::Live),
    }
}

/// Recording's ring: solid red when muted (the recording holds silence),
/// dashed amber while paused, dim red with the talker wedge otherwise.
pub fn recording_ring(snap: &UiSnapshot) -> RingLook {
    let style = if snap.audio.mic_muted {
        RingStyle::RecordingMuted
    } else if snap.meeting.paused {
        RingStyle::Paused
    } else {
        RingStyle::Recording
    };
    RingLook {
        style,
        talker: talker_if(snap, style == RingStyle::Recording),
    }
}

/// The next speaker volume for Quieter (`up == false`) or Louder: the
/// neighbouring multiple of 10, within 0..=100.
pub fn step_volume(volume: u16, up: bool) -> u16 {
    let volume = volume.min(VOLUME_MAX);
    if up {
        ((volume / VOLUME_STEP + 1) * VOLUME_STEP).min(VOLUME_MAX)
    } else if volume == 0 {
        0
    } else {
        (volume - 1) / VOLUME_STEP * VOLUME_STEP
    }
}

/// "42:17", or "1:02:03" from an hour on.
pub fn format_timer(secs: u32) -> String {
    let (hours, minutes, seconds) = (secs / 3_600, secs / 60 % 60, secs % 60);
    if hours == 0 {
        format!("{minutes:02}:{seconds:02}")
    } else {
        format!("{hours}:{minutes:02}:{seconds:02}")
    }
}

/// The timer's font and label top: Num48 like the mockups, Num26 from an
/// hour on so h:mm:ss stays inside the ring.
pub fn timer_font(secs: u32) -> (Font, i32) {
    if secs < 3_600 {
        (Font::Num48, REC_TIMER_Y)
    } else {
        (Font::Num26, TIMER_HOURS_Y)
    }
}

/// "0:52": a countdown in minutes and seconds.
pub fn format_countdown(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// "184 MB", "1.3 GB", in decimal units like the file managers people use.
pub fn format_size(bytes: u64) -> String {
    const MB: u64 = 1_000_000;
    const GB: u64 = 1_000_000_000;
    let megabytes = (bytes + MB / 2) / MB;
    if megabytes == 0 {
        "Under 1 MB".into()
    } else if megabytes < 1_000 {
        format!("{megabytes} MB")
    } else {
        let tenths = (bytes + GB / 20) / (GB / 10);
        format!("{}.{} GB", tenths / 10, tenths % 10)
    }
}

/// The host of a webhook endpoint, for "To notes.example.com": no scheme,
/// credentials, port, path or query. `None` when there is no server.
pub fn server_host(endpoint: &str) -> Option<String> {
    let rest = endpoint.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, after)| after);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = if host_port.starts_with('[') {
        host_port.split_inclusive(']').next().unwrap_or(host_port)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// The font for a line inside the ring: `preferred` when it fits
/// `RING_TEXT_W`, else Small (the label truncates beyond that).
pub fn ring_font(text: &str, preferred: Font) -> Font {
    if text_width(text, preferred) <= RING_TEXT_W {
        preferred
    } else {
        Font::Small
    }
}

/// Shared fixtures for the screens_v2 tests: what the mockups show around
/// the dial (USB and Wi-Fi up, 14:32, battery 82 %).
#[cfg(test)]
pub mod fixtures {
    use crate::kit::dispatch::{Dispatcher, Message, Severity, UiEvent};
    use crate::kit::screen::ScreenCtx;
    use crate::kit::snapshot::{ActiveMessage, UiSnapshot};
    use std::sync::{mpsc, Arc};
    use tokio::sync::broadcast;
    use waved_config::WavedConfig;
    use waved_state::Command;

    pub fn ctx() -> (
        ScreenCtx,
        broadcast::Receiver<Command>,
        mpsc::Receiver<UiEvent>,
    ) {
        let (dispatch, bus, ui) = Dispatcher::test();
        let ctx = ScreenCtx {
            dispatch,
            config: Arc::new(WavedConfig::default()),
        };
        (ctx, bus, ui)
    }

    pub fn snap() -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.mcu.usb_connected = true;
        snap.wifi.connected = true;
        snap.clock = "14:32".into();
        snap.battery.level = 82;
        snap.battery.voltage = 3_900;
        snap.settings.first_run_done = true;
        snap.now_ms = 10_000;
        snap
    }

    /// Commands sent so far, as their Debug text ("SetSpeakerVolume(70)").
    pub fn sent(bus: &mut broadcast::Receiver<Command>) -> Vec<String> {
        std::iter::from_fn(|| bus.try_recv().ok())
            .map(|command| format!("{command:?}"))
            .collect()
    }

    pub fn events(ui: &mpsc::Receiver<UiEvent>) -> Vec<UiEvent> {
        ui.try_iter().collect()
    }

    /// What the runtime does with `UiEvent::Show` (contract request 2).
    pub fn with_message(snap: &mut UiSnapshot, severity: Severity, text: &str) {
        let seq = snap.message.as_ref().map_or(1, |active| active.seq + 1);
        snap.message = Some(ActiveMessage {
            seq,
            message: Message::new(severity, text),
            since_ms: snap.now_ms,
        });
    }
}
```

- [ ] **Step 4: Run and confirm green**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::main_parts 2>&1 | grep "test result"`
Expected: `ok. 7 passed`

- [ ] **Step 5: Lint**

Run the lint gate from Global Constraints. Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-ui/src/screens_v2/main_parts.rs
git commit -m "feat(ui): shared ring looks, volume steps and formats for the main screens

- home_ring / recording_ring follow the README dial table (mute wins, wedge only while voice is detected)
- step_volume moves to the neighbouring 10 % within 0..=100
- format_timer switches to h:mm:ss after 59:59, drawn in Num26 so it fits the ring
- server_host shows only the host of a webhook endpoint, never its credentials
- test fixtures shared by the screens_v2 tests

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Message copy and the message feed (A6)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/messages.rs`

**Interfaces:**
- Consumes: `kit::dispatch::{Message, Severity}`, `kit::snapshot::UiSnapshot`, `kit::tokens::MESSAGE_MAX_W`, `main_parts::format_timer`, `waved_state::{DeviceEvent, UploadMode, RecordingIntegrityFault}`, `AudioState::mic_fault` (contract 3).
- Produces:
  - `pub mod copy` with `pub const HOLD_STOP, KEEP_HOLDING, RECORDING_SENT, SENDING, KEPT, DELETED, NO_WIFI_UPLOAD_WAITING, UPLOAD_WILL_RETRY, UPLOAD_FAILED, RECORDING_INTERRUPTED, AUDIO_RESTARTING, RECORDING_UNAVAILABLE, STORAGE_FULL, MICS_NOT_RESPONDING: &str` and `pub fn marked_at(secs: u32) -> String`, `pub fn battery_low(level: u8) -> String`, `pub fn battery_critical(level: u8) -> String`, `pub fn after_stop(mode: UploadMode) -> Message`
  - `pub struct MessageFeed` (Default) with `pub fn new() -> Self`, `pub fn on_event(&mut self, event: &DeviceEvent, snap: &UiSnapshot) -> Option<Message>`, `pub fn on_snapshot(&mut self, snap: &UiSnapshot) -> Vec<Message>`

Copy table (replaces the legacy `toast.rs`, `upload_notice.rs`, `integrity_notice.rs` and `lib.rs` strings):

| Trigger | Severity | Text | Legacy text |
|---|---|---|---|
| Upload succeeded (recording stopped here) | Success | Recording sent | Recording uploaded |
| Upload deferred, Wi-Fi down (first time per recording) | Warning | No Wi-Fi, upload waiting | Saved - upload queued |
| Upload deferred, Wi-Fi up (first time per recording) | Warning | Server not reached, will retry | Saved - upload queued |
| Upload needs attention | Error | Upload failed, see Files | Saved - check Webhook |
| Integrity fault during a recording | Error | Recording interrupted | Recording interrupted |
| Integrity fault needing a restart | Warning | Audio restarting | Audio restarting... |
| Integrity fault blocking recording / Record while blocked | Error | Recording unavailable | Recording unavailable |
| Record while storage is full | Error | Storage full, can't record | Recording unavailable |
| Stop held, Ask mode | Info | Recording stopped | Saving recording... |
| Stop held, Automatic | Info | Recording saved, sending | Saving recording... |
| Stop held, Local only | Success | Recording saved on Wave | Saving recording... |
| Mark | Info | Marked at 42:17 | Bookmark added |
| Short tap on Stop | Info | Hold Stop to end the recording | — |
| Pressing Stop | Info | Keep holding to stop | — |
| After stop: Send / Keep / Delete | Info / Success / Success | Sending recording / Kept on Wave / Recording deleted | — |
| Battery ≤ 10 %, not charging | Warning | Battery 10%, plug in Wave | — |
| Battery ≤ 5 %, not charging | Error | Battery 5%, plug in Wave now | — |
| Microphone check failed | Error | Microphones not responding | — |

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fonts::Font;
    use crate::kit::obj::text_width;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::MESSAGE_MAX_W;
    use waved_state::{RecordingIntegrityFault, UploadMode};

    fn message(severity: Severity, text: &str) -> Message {
        Message::new(severity, text)
    }

    #[test]
    fn every_message_fits_one_line_of_the_status_slot() {
        // StatusArea::show leaves MESSAGE_MAX_W − 44 px for the text (icon and padding).
        let text_max = MESSAGE_MAX_W - 44;
        let mut texts: Vec<String> = [
            copy::HOLD_STOP,
            copy::KEEP_HOLDING,
            copy::RECORDING_SENT,
            copy::SENDING,
            copy::KEPT,
            copy::DELETED,
            copy::NO_WIFI_UPLOAD_WAITING,
            copy::UPLOAD_WILL_RETRY,
            copy::UPLOAD_FAILED,
            copy::RECORDING_INTERRUPTED,
            copy::AUDIO_RESTARTING,
            copy::RECORDING_UNAVAILABLE,
            copy::STORAGE_FULL,
            copy::MICS_NOT_RESPONDING,
        ]
        .map(String::from)
        .into();
        texts.push(copy::marked_at(359_999));
        texts.push(copy::battery_low(10));
        texts.push(copy::battery_critical(5));
        for mode in [UploadMode::Ask, UploadMode::Automatic, UploadMode::LocalOnly] {
            texts.push(copy::after_stop(mode).text);
        }
        lvgl(|_| {
            for text in &texts {
                let width = text_width(text, Font::Small);
                assert!(width <= text_max, "{text:?} is {width} px");
                assert!(!text.contains("..."), "{text:?}");
            }
        });
    }

    #[test]
    fn stop_messages_follow_the_upload_mode() {
        assert_eq!(copy::after_stop(UploadMode::Ask), message(Severity::Info, "Recording stopped"));
        assert_eq!(
            copy::after_stop(UploadMode::Automatic),
            message(Severity::Info, "Recording saved, sending")
        );
        assert_eq!(
            copy::after_stop(UploadMode::LocalOnly),
            message(Severity::Success, "Recording saved on Wave")
        );
        assert_eq!(copy::marked_at(2_537), "Marked at 42:17");
    }

    #[test]
    fn uploads_are_announced_only_for_recordings_stopped_here() {
        let mut feed = MessageFeed::new();
        let snap = UiSnapshot::default();
        let (ours, backlog) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(feed.on_event(&DeviceEvent::RecordingStopped(ours), &snap), None);
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadSucceeded(backlog), &snap),
            None
        );
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadNeedsAttention(ours), &snap),
            Some(message(Severity::Error, copy::UPLOAD_FAILED))
        );
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadSucceeded(ours), &snap),
            Some(message(Severity::Success, copy::RECORDING_SENT))
        );
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadSucceeded(ours), &snap),
            None,
            "announced once"
        );
    }

    #[test]
    fn a_deferral_is_announced_once_with_its_reason() {
        let mut feed = MessageFeed::new();
        let mut snap = UiSnapshot::default();
        let (first, second) = (Uuid::from_u128(1), Uuid::from_u128(2));
        feed.on_event(&DeviceEvent::RecordingStopped(first), &snap);
        feed.on_event(&DeviceEvent::RecordingStopped(second), &snap);
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadDeferred(first), &snap),
            Some(message(Severity::Warning, copy::NO_WIFI_UPLOAD_WAITING))
        );
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadDeferred(first), &snap),
            None,
            "retries stay quiet"
        );
        snap.wifi.connected = true;
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadDeferred(second), &snap),
            Some(message(Severity::Warning, copy::UPLOAD_WILL_RETRY))
        );
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadSucceeded(first), &snap),
            Some(message(Severity::Success, copy::RECORDING_SENT))
        );
    }

    #[test]
    fn integrity_faults_map_to_plain_messages() {
        let mut feed = MessageFeed::new();
        let snap = UiSnapshot::default();
        let event = |meeting_id, fault| DeviceEvent::RecordingIntegrityFailed { meeting_id, fault };
        assert_eq!(
            feed.on_event(&event(Some(Uuid::from_u128(1)), RecordingIntegrityFault::CaptureXrun), &snap),
            Some(message(Severity::Error, copy::RECORDING_INTERRUPTED))
        );
        assert_eq!(
            feed.on_event(&event(None, RecordingIntegrityFault::CaptureEnded), &snap),
            Some(message(Severity::Warning, copy::AUDIO_RESTARTING))
        );
        assert_eq!(
            feed.on_event(&event(None, RecordingIntegrityFault::WavWriteFailed), &snap),
            Some(message(Severity::Error, copy::RECORDING_UNAVAILABLE))
        );
        assert_eq!(
            feed.on_event(&event(None, RecordingIntegrityFault::CaptureXrun), &snap),
            None
        );
        assert_eq!(feed.on_event(&DeviceEvent::UsbConnected, &snap), None);
    }

    #[test]
    fn battery_messages_fire_once_per_threshold_and_not_before_a_reading() {
        let mut feed = MessageFeed::new();
        let mut snap = UiSnapshot::default(); // level 0, voltage 0: no reading yet
        assert!(feed.on_snapshot(&snap).is_empty(), "no warning before the MCU reports");
        snap.battery.voltage = 3_600;
        snap.battery.level = 11;
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.battery.level = 10;
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Warning, "Battery 10%, plug in Wave")]
        );
        snap.battery.level = 11;
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.battery.level = 10;
        assert!(feed.on_snapshot(&snap).is_empty(), "flapping around 10 % warns once");
        snap.battery.level = 5;
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Error, "Battery 5%, plug in Wave now")]
        );
        snap.battery.is_charging = true;
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.battery.is_charging = false;
        snap.battery.level = 9;
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Warning, "Battery 9%, plug in Wave")],
            "unplugging re-arms the warning"
        );
    }

    #[test]
    fn a_microphone_fault_is_announced_when_it_starts() {
        let mut feed = MessageFeed::new();
        let mut snap = UiSnapshot::default();
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.audio.mic_fault = true;
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Error, copy::MICS_NOT_RESPONDING)]
        );
        assert!(feed.on_snapshot(&snap).is_empty(), "once per fault");
    }

    #[test]
    fn tracking_keeps_the_16_newest_recordings() {
        let mut feed = MessageFeed::new();
        let snap = UiSnapshot::default();
        for n in 0..=16 {
            feed.on_event(&DeviceEvent::RecordingStopped(Uuid::from_u128(n)), &snap);
        }
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadSucceeded(Uuid::from_u128(0)), &snap),
            None,
            "the oldest fell out"
        );
        assert!(feed
            .on_event(&DeviceEvent::RecordingUploadSucceeded(Uuid::from_u128(16)), &snap)
            .is_some());
    }
}
```

- [ ] **Step 2: Run and confirm the failure**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::messages 2>&1 | tail -20`
Expected: compile errors `failed to resolve: use of undeclared crate or module `copy`` and `cannot find type `MessageFeed``.

- [ ] **Step 3: Implement**

```rust
//! Every message the main screens show (A6), and `MessageFeed`, which turns
//! service events and state changes into messages. Messages replace the
//! status pill for 2 s (success, info) or 4 s (warning, error), never take a
//! tap, and fit one line of at most 240 px.

use crate::kit::dispatch::{Message, Severity};
use crate::kit::snapshot::UiSnapshot;
use std::collections::VecDeque;
use uuid::Uuid;
use waved_state::DeviceEvent;

/// The words, in one place, so every screen says the same thing.
pub mod copy {
    use crate::kit::dispatch::{Message, Severity};
    use crate::screens_v2::main_parts::format_timer;
    use waved_state::UploadMode;

    pub const HOLD_STOP: &str = "Hold Stop to end the recording";
    pub const KEEP_HOLDING: &str = "Keep holding to stop";
    pub const RECORDING_SENT: &str = "Recording sent";
    pub const SENDING: &str = "Sending recording";
    pub const KEPT: &str = "Kept on Wave";
    pub const DELETED: &str = "Recording deleted";
    pub const NO_WIFI_UPLOAD_WAITING: &str = "No Wi-Fi, upload waiting";
    pub const UPLOAD_WILL_RETRY: &str = "Server not reached, will retry";
    pub const UPLOAD_FAILED: &str = "Upload failed, see Files";
    pub const RECORDING_INTERRUPTED: &str = "Recording interrupted";
    pub const AUDIO_RESTARTING: &str = "Audio restarting";
    pub const RECORDING_UNAVAILABLE: &str = "Recording unavailable";
    pub const STORAGE_FULL: &str = "Storage full, can't record";
    pub const MICS_NOT_RESPONDING: &str = "Microphones not responding";

    pub fn marked_at(secs: u32) -> String {
        format!("Marked at {}", format_timer(secs))
    }

    pub fn battery_low(level: u8) -> String {
        format!("Battery {level}%, plug in Wave")
    }

    pub fn battery_critical(level: u8) -> String {
        format!("Battery {level}%, plug in Wave now")
    }

    /// What Stop's completed hold says, by what happens to the recording next.
    pub fn after_stop(mode: UploadMode) -> Message {
        match mode {
            UploadMode::Ask => Message::new(Severity::Info, "Recording stopped"),
            UploadMode::Automatic => Message::new(Severity::Info, "Recording saved, sending"),
            UploadMode::LocalOnly => Message::new(Severity::Success, "Recording saved on Wave"),
        }
    }
}

/// Recordings remembered for upload messages (the legacy tracker's bound).
const TRACKED: usize = 16;
const BATTERY_LOW: u8 = 10;
const BATTERY_CRITICAL: u8 = 5;
/// Back at or above this (or charging) re-arms the battery messages, so a
/// level bouncing between 10 and 11 % warns once.
const BATTERY_REARM: u8 = 15;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum BatteryStage {
    #[default]
    Normal,
    Low,
    Critical,
}

/// Turns service events and state changes into messages. The runtime owns
/// one and shows what it returns (contract request 6).
#[derive(Debug, Default)]
pub struct MessageFeed {
    /// Recordings stopped while this UI ran, oldest first, with whether their
    /// first deferral was already announced. Uploads of older recordings (the
    /// retry backlog after a restart) stay quiet.
    stopped: VecDeque<(Uuid, bool)>,
    battery: BatteryStage,
    mic_fault: bool,
}

impl MessageFeed {
    pub fn new() -> Self {
        Self::default()
    }

    /// The message for one service event, if it has one.
    pub fn on_event(&mut self, event: &DeviceEvent, snap: &UiSnapshot) -> Option<Message> {
        match event {
            DeviceEvent::RecordingStopped(id) => {
                self.track(*id);
                None
            }
            DeviceEvent::RecordingUploadSucceeded(id) => {
                let at = self.position(id)?;
                self.stopped.remove(at);
                Some(Message::new(Severity::Success, copy::RECORDING_SENT))
            }
            DeviceEvent::RecordingUploadDeferred(id) => {
                let at = self.position(id)?;
                if std::mem::replace(&mut self.stopped[at].1, true) {
                    return None;
                }
                let text = if snap.wifi.connected {
                    copy::UPLOAD_WILL_RETRY
                } else {
                    copy::NO_WIFI_UPLOAD_WAITING
                };
                Some(Message::new(Severity::Warning, text))
            }
            DeviceEvent::RecordingUploadNeedsAttention(id) => {
                self.position(id)?;
                Some(Message::new(Severity::Error, copy::UPLOAD_FAILED))
            }
            DeviceEvent::RecordingIntegrityFailed { meeting_id, fault } => {
                if meeting_id.is_some() {
                    Some(Message::new(Severity::Error, copy::RECORDING_INTERRUPTED))
                } else if fault.requires_restart() {
                    Some(Message::new(Severity::Warning, copy::AUDIO_RESTARTING))
                } else if fault.blocks_recording() {
                    Some(Message::new(Severity::Error, copy::RECORDING_UNAVAILABLE))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Messages for what changed since the previous snapshot: battery
    /// thresholds and the microphone check.
    pub fn on_snapshot(&mut self, snap: &UiSnapshot) -> Vec<Message> {
        let mut messages = Vec::new();
        let battery = &snap.battery;
        // BatteryState::default() is level 0 with voltage 0: no reading yet.
        let stage = if battery.voltage == 0 || battery.is_charging {
            BatteryStage::Normal
        } else if battery.level <= BATTERY_CRITICAL {
            BatteryStage::Critical
        } else if battery.level <= BATTERY_LOW {
            BatteryStage::Low
        } else if battery.level >= BATTERY_REARM {
            BatteryStage::Normal
        } else {
            self.battery.min(BatteryStage::Low)
        };
        if stage > self.battery {
            match stage {
                BatteryStage::Low => messages.push(Message::new(
                    Severity::Warning,
                    copy::battery_low(battery.level),
                )),
                BatteryStage::Critical => messages.push(Message::new(
                    Severity::Error,
                    copy::battery_critical(battery.level),
                )),
                BatteryStage::Normal => {}
            }
        }
        self.battery = stage;
        if snap.audio.mic_fault && !self.mic_fault {
            messages.push(Message::new(Severity::Error, copy::MICS_NOT_RESPONDING));
        }
        self.mic_fault = snap.audio.mic_fault;
        messages
    }

    fn track(&mut self, id: Uuid) {
        if let Some(at) = self.position(&id) {
            self.stopped.remove(at);
        }
        if self.stopped.len() == TRACKED {
            self.stopped.pop_front();
        }
        self.stopped.push_back((id, false));
    }

    fn position(&self, id: &Uuid) -> Option<usize> {
        self.stopped.iter().position(|(tracked, _)| tracked == id)
    }
}
```

- [ ] **Step 4: Run and confirm green**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::messages 2>&1 | grep "test result"`
Expected: `ok. 8 passed`

- [ ] **Step 5: Lint** — lint gate. Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-ui/src/screens_v2/messages.rs
git commit -m "feat(ui): message copy and feed for the main screens

- one copy table for every message, each tested to fit one status line
- upload, integrity, battery and microphone-fault messages from events and state
- uploads only announced for recordings stopped while the UI ran; a deferral once
- battery warnings wait for a real reading and fire once per threshold

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `MainDial` — dial, Quieter / Louder and the volume gauge

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/main_parts.rs`

**Interfaces:**
- Consumes: `Dial::new(parent, cx, cy, r, width) -> Dial`, `.on_tap(..)`, `set_ring(&mut self, RingStyle)`, `set_talker(&self, Option<f32>)`, `set_gauge(&mut self, Option<u8>)`, `hit() -> Option<Obj>`, `talker_visible()`; `SideButton::new(parent, Side, icon, label, on_step)`, `.obj()`; `Dispatcher::command`; tokens `DIAL_CX/CY/R`, `GAUGE_MS`, `REC_TIMER_Y`, `SCREEN_W`, `TEXT`, `TEXT_2`; `Command::{ToggleMicMute, SetSpeakerVolume(u16)}`.
- Produces:
  - `pub const GAUGE_CAPTION_Y: i32 = 77;`
  - `pub struct MainDial` with `pub fn new(root: Obj, dispatch: &Dispatcher, snap: &UiSnapshot) -> Self` and `pub fn update(&mut self, snap: &UiSnapshot, look: RingLook) -> bool` (true while the gauge shows; the screen then hides its own text inside the dial)
  - test-only: `quieter() -> Obj`, `louder() -> Obj`, `hit() -> Obj`, `gauge_text() -> String`, `talker_visible() -> bool`

Gauge layout (home-volume.png, README "Dial content stack (gauge, progress, resting)"): caption "Speaker" Small TEXT_2 centred, top 77; value Num48 TEXT, digits' box top `REC_TIMER_Y` 104; "%" Num20 TEXT_2, 2 px after the digits, top 104 + 15 (shared baseline: Num48 baseline 34, Num20 baseline 19); the pair centred on x 200. The gauge arc is the kit's (−135°…135°, LINE track, LIVE fill, stroke 5).

- [ ] **Step 1: Write the failing tests**

Append to `main_parts.rs`:

```rust
#[cfg(test)]
mod dial_tests {
    use super::fixtures::{sent, snap};
    use super::*;
    use crate::bindings::{
        lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT, lv_event_code_t_LV_EVENT_PRESSED,
    };
    use crate::kit::dispatch::Dispatcher;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::GAUGE_MS;

    #[test]
    fn holding_louder_climbs_from_the_local_target_not_the_stale_snapshot() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, mut bus, _ui) = Dispatcher::test();
            let mut now = snap();
            now.audio.speaker_volume = 60;
            let mut dial = MainDial::new(root, &dispatch, &now);
            dial.update(&now, home_ring(&now));
            lv.send(dial.louder(), lv_event_code_t_LV_EVENT_PRESSED);
            for _ in 0..4 {
                now.now_ms += 300;
                dial.update(&now, home_ring(&now)); // the service has not caught up: still 60
                lv.advance(300);
                lv.send(dial.louder(), lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT);
            }
            assert_eq!(
                sent(&mut bus),
                [
                    "SetSpeakerVolume(70)",
                    "SetSpeakerVolume(80)",
                    "SetSpeakerVolume(90)",
                    "SetSpeakerVolume(100)"
                ]
            );
        });
    }

    #[test]
    fn the_gauge_shows_for_1_5_s_after_a_step_and_after_a_host_change() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let look = RingLook {
                style: RingStyle::Live,
                talker: Some(307),
            };
            let mut now = snap();
            now.audio.speaker_volume = 60;
            let mut dial = MainDial::new(root, &dispatch, &now);
            assert!(!dial.update(&now, look), "no gauge when the screen opens");
            assert!(dial.talker_visible());
            lv.click(dial.louder());
            assert!(dial.update(&now, look));
            assert_eq!(dial.gauge_text(), "70");
            assert!(!dial.talker_visible(), "the gauge replaces the ring");
            now.now_ms += GAUGE_MS - 1;
            now.audio.speaker_volume = 70; // the service caught up
            assert!(dial.update(&now, look));
            now.now_ms += 1;
            assert!(!dial.update(&now, look), "gone 1.5 s after the last step");
            assert!(dial.talker_visible(), "the ring and wedge return");
            now.now_ms += 500;
            now.audio.speaker_volume = 40; // the host moved it
            assert!(dial.update(&now, look));
            assert_eq!(dial.gauge_text(), "40");
            now.now_ms += GAUGE_MS;
            assert!(!dial.update(&now, look));
        });
    }

    #[test]
    fn quieter_steps_down_and_the_dial_toggles_mute() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, mut bus, _ui) = Dispatcher::test();
            let mut now = snap();
            now.audio.speaker_volume = 65;
            let mut dial = MainDial::new(root, &dispatch, &now);
            dial.update(&now, home_ring(&now));
            lv.click(dial.quieter());
            lv.click(dial.hit());
            assert_eq!(sent(&mut bus), ["SetSpeakerVolume(60)", "ToggleMicMute"]);
        });
    }
}
```

- [ ] **Step 2: Run and confirm the failure**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::main_parts::dial_tests 2>&1 | tail -20`
Expected: compile error `cannot find type `MainDial``.

- [ ] **Step 3: Implement**

Add to the imports of `main_parts.rs`:

```rust
use crate::kit::components::buttons::{Side, SideButton};
use crate::kit::components::dial::Dial;
use crate::kit::dispatch::Dispatcher;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::tokens::{DIAL_CX, DIAL_CY, DIAL_R, GAUGE_MS, SCREEN_W, TEXT, TEXT_2};
use std::cell::Cell;
use std::rc::Rc;
use waved_state::Command;
```

(merge `REC_TIMER_Y` into the same `tokens` import.) Then add:

```rust
/// Top of the gauge caption ("Speaker"), README "Dial content stack (gauge,
/// progress, resting)": caption 13 px at top 77.
pub const GAUGE_CAPTION_Y: i32 = 77;
/// The 20 px "%" shares the 48 px digits' baseline: 34 − 19 px below their top.
const SUFFIX_DY: i32 = 15;
const SUFFIX_GAP: i32 = 2;

/// The dial with Quieter and Louder, shared by Home and Recording: tapping
/// the dial toggles mute, the side buttons step the speaker volume, and the
/// dial turns into a volume gauge for 1.5 s after any change.
#[cfg_attr(not(test), allow(dead_code))] // side buttons are read only by test accessors
pub struct MainDial {
    dial: Dial,
    quieter: SideButton,
    louder: SideButton,
    caption: Obj,
    value: Obj,
    percent: Obj,
    /// The volume the next step starts from: the last value sent while
    /// stepping, else the snapshot's. The service applies a step a frame or
    /// more later, so stepping from the snapshot would repeat the same value.
    target: Rc<Cell<u16>>,
    /// A side button stepped since the last update.
    stepped: Rc<Cell<bool>>,
    /// Until then `target` wins over the snapshot.
    local_until_ms: u64,
    gauge_until_ms: u64,
    last_volume: Option<u16>,
    /// The gauge on screen: (volume, speaker muted).
    shown_gauge: Option<(u16, bool)>,
    /// The ring on screen; `None` forces a redraw.
    look: Option<RingLook>,
}

impl MainDial {
    pub fn new(root: Obj, dispatch: &Dispatcher, snap: &UiSnapshot) -> Self {
        let mute = dispatch.clone();
        let dial = Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3)
            .on_tap(move || mute.command(Command::ToggleMicMute));
        let target = Rc::new(Cell::new(snap.audio.speaker_volume.min(VOLUME_MAX)));
        let stepped = Rc::new(Cell::new(false));
        let stepper = |up: bool| {
            let (target, stepped, dispatch) = (target.clone(), stepped.clone(), dispatch.clone());
            move || {
                let next = step_volume(target.get(), up);
                if next != target.get() {
                    target.set(next);
                    dispatch.command(Command::SetSpeakerVolume(next));
                }
                // At 0 or 100 the gauge still shows, as feedback for the press.
                stepped.set(true);
            }
        };
        let quieter = SideButton::new(root, Side::Left, icons::VOLUME_LOW, "Quieter", stepper(false));
        let louder = SideButton::new(root, Side::Right, icons::VOLUME_HIGH, "Louder", stepper(true));
        let caption = Obj::label(root, "Speaker", Font::Small, TEXT_2)
            .centered_text(SCREEN_W)
            .pos(0, GAUGE_CAPTION_Y)
            .hidden(true);
        let value = Obj::label(root, "", Font::Num48, TEXT).one_line().hidden(true);
        let percent = Obj::label(root, "%", Font::Num20, TEXT_2).one_line().hidden(true);
        Self {
            dial,
            quieter,
            louder,
            caption,
            value,
            percent,
            target,
            stepped,
            local_until_ms: 0,
            gauge_until_ms: 0,
            last_volume: None,
            shown_gauge: None,
            look: None,
        }
    }

    /// Follow the snapshot and draw `look` whenever the gauge is not showing.
    /// Returns whether the gauge shows.
    pub fn update(&mut self, snap: &UiSnapshot, look: RingLook) -> bool {
        let now = snap.now_ms;
        let volume = snap.audio.speaker_volume.min(VOLUME_MAX);
        if self.stepped.replace(false) {
            self.local_until_ms = now + GAUGE_MS;
            self.gauge_until_ms = now + GAUGE_MS;
        } else if now >= self.local_until_ms && self.last_volume.is_some_and(|last| last != volume) {
            // The host, a physical button or another screen changed it.
            self.gauge_until_ms = now + GAUGE_MS;
        }
        self.last_volume = Some(volume);
        if now >= self.local_until_ms {
            self.target.set(volume);
        }
        let gauge = (now < self.gauge_until_ms).then(|| {
            let shown = if now < self.local_until_ms {
                self.target.get()
            } else {
                volume
            };
            (shown, snap.audio.speaker_muted)
        });
        if gauge != self.shown_gauge {
            match gauge {
                Some((shown, muted)) => {
                    self.dial.set_gauge(Some(shown as u8));
                    self.show_gauge(shown, muted);
                }
                None => {
                    for obj in [self.caption, self.value, self.percent] {
                        obj.hidden(true);
                    }
                    // set_gauge(None) restores the ring; redraw the look on it.
                    self.dial.set_gauge(None);
                    self.look = None;
                }
            }
            self.shown_gauge = gauge;
        }
        if gauge.is_none() && self.look != Some(look) {
            self.dial.set_ring(look.style);
            self.dial.set_talker(look.talker.map(f32::from));
            self.look = Some(look);
        }
        gauge.is_some()
    }

    /// "Speaker" over "60%", centred on the dial.
    fn show_gauge(&self, volume: u16, muted: bool) {
        let digits = volume.to_string();
        // One spare pixel, so text that fits exactly never ends in "...".
        let digits_w = text_width(&digits, Font::Num48) + 1;
        let percent_w = text_width("%", Font::Num20) + 1;
        let x = (SCREEN_W - (digits_w + SUFFIX_GAP + percent_w)) / 2;
        self.value
            .width(digits_w)
            .pos(x, REC_TIMER_Y)
            .hidden(false)
            .set_text(&digits);
        self.percent
            .width(percent_w)
            .pos(x + digits_w + SUFFIX_GAP, REC_TIMER_Y + SUFFIX_DY)
            .hidden(false);
        self.caption
            .hidden(false)
            .set_text(if muted { "Speaker muted" } else { "Speaker" });
    }

    #[cfg(test)]
    pub fn quieter(&self) -> Obj {
        self.quieter.obj()
    }

    #[cfg(test)]
    pub fn louder(&self) -> Obj {
        self.louder.obj()
    }

    #[cfg(test)]
    pub fn hit(&self) -> Obj {
        self.dial.hit().expect("the main dial is tappable")
    }

    #[cfg(test)]
    pub fn gauge_text(&self) -> String {
        self.value.text()
    }

    #[cfg(test)]
    pub fn talker_visible(&self) -> bool {
        self.dial.talker_visible()
    }
}
```

`quieter` and `louder` are read only by the test accessors (the objects themselves belong to LVGL), hence the `cfg_attr` on the struct.

- [ ] **Step 4: Run and confirm green**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::main_parts 2>&1 | grep "test result"`
Expected: `ok. 10 passed`

- [ ] **Step 5: Lint** — lint gate. Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-ui/src/screens_v2/main_parts.rs
git commit -m "feat(ui): main dial with quieter, louder and the volume gauge

- tapping the dial toggles mute; side buttons step 10 % and repeat while held
- steps climb from the last value sent, not from a snapshot the service has not updated yet
- the dial shows the gauge for 1.5 s after a step or a host-side volume change
- the ring and talker wedge return when the gauge ends

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Home (A1)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/home.rs`
- Create: goldens `home.png`, `home_muted.png`, `home_volume.png`

**Interfaces:**
- Consumes: `MainDial`, `home_ring`, `ring_font`, `RING_TEXT_W` (Tasks 1, 3); `messages::copy::{STORAGE_FULL, RECORDING_UNAVAILABLE}` (Task 2); `StatusArea::pill(root, &dispatch)`, `.update`, `.cluster()`; `RoundButton::record(root, on_tap)`, `.obj()`; `UiSnapshot::connection_line()` (contract 5), `in_call()`; `Command::StartRecording`.
- Produces: `pub struct HomeScreen` (Default, `Screen`); `pub struct HomeView { pub icon: &'static str, pub icon_color: u32, pub title: &'static str, pub title_color: u32, pub line: String, pub line_color: u32, pub hint: &'static str }` with `pub fn from_snapshot(snap: &UiSnapshot) -> Self`; `pub fn record_blocked(snap: &UiSnapshot) -> Option<&'static str>`; `pub const RECORD_DEBOUNCE_MS: u64 = 2_000`.

Layout (home.png / home-muted.png / home-volume.png; all positions are kit tokens):

| Element | Font | Colour | Position |
|---|---|---|---|
| Status pill (pre-recording dot, USB, Wi-Fi, clock, battery, chevron) | kit | kit | `StatusArea::pill` |
| Dial | — | ring from `home_ring` | `MainDial` |
| Mic icon (MIC / MIC_OFF) | Icon20 | TEXT_2 / REC when muted | (`DIAL_ICON_X`, `DIAL_ICON_Y`) |
| "Mic on" / "Muted" | Dial | TEXT / REC | centred, top `DIAL_TITLE_Y` |
| Connection line (`connection_line()`) | Body, Small if > 124 px | LIVE in a call or while the computer listens, else TEXT_2 | 124 wide at x 138, top `DIAL_LINE_Y` |
| "Tap to mute" / "Tap to unmute" | Micro | TEXT_3 | centred, top `DIAL_HINT_Y` |
| Quieter / Louder | kit | kit | `MainDial` |
| Record | kit | red dot, Text 2 label | `RoundButton::record` (main spot) |

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::LV_STATE_PRESSED;
    use crate::kit::dispatch::{Message, UiEvent};
    use crate::kit::obj::text_width;
    use crate::kit::route::Route;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::main_parts::fixtures::{ctx, events, sent, snap, with_message};

    /// home.png: in a call, someone talking at 307°, pre-recording on.
    fn in_a_call() -> UiSnapshot {
        let mut snap = snap();
        snap.audio.call_off_hook = true;
        snap.audio.doa_active = true;
        snap.audio.doa_horizontal = 307.0;
        snap.meeting.pre_recording_active = true;
        snap
    }

    #[test]
    fn home_view_follows_the_dial_table() {
        let mut snap = snap();
        let view = HomeView::from_snapshot(&snap);
        assert_eq!(
            (view.title, view.line.as_str(), view.line_color, view.hint),
            ("Mic on", "Connected by USB", TEXT_2, "Tap to mute")
        );
        snap.audio.host_listening = true;
        let view = HomeView::from_snapshot(&snap);
        assert_eq!((view.line.as_str(), view.line_color), ("Computer is listening", LIVE));
        snap.audio.call_off_hook = true;
        assert_eq!(HomeView::from_snapshot(&snap).line, "In a call");
        snap.audio.mic_muted = true;
        let view = HomeView::from_snapshot(&snap);
        assert_eq!(
            (view.icon, view.icon_color, view.title, view.title_color),
            (icons::MIC_OFF, REC, "Muted", REC)
        );
        assert_eq!(
            (view.line.as_str(), view.line_color, view.hint),
            ("Call can't hear you", TEXT_2, "Tap to unmute")
        );
        let mut idle = snap_without_usb();
        assert_eq!(HomeView::from_snapshot(&idle).line, "Not connected");
        idle.audio.mic_muted = true;
        assert_eq!(HomeView::from_snapshot(&idle).line, "Mic is off");
    }

    fn snap_without_usb() -> UiSnapshot {
        let mut snap = snap();
        snap.mcu.usb_connected = false;
        snap
    }

    #[test]
    fn home_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            HomeScreen::default().build(root, &ctx, &in_a_call());
            assert_golden("home", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn home_muted_with_a_message_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = snap();
            snap.audio.call_off_hook = true;
            snap.audio.mic_muted = true;
            with_message(&mut snap, Severity::Success, "Recording sent");
            HomeScreen::default().build(root, &ctx, &snap);
            assert_golden("home_muted", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn home_volume_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, _ui) = ctx();
            let mut snap = in_a_call();
            snap.audio.speaker_volume = 50;
            let mut screen = HomeScreen::default();
            screen.build(root, &ctx, &snap);
            let louder = screen.parts.as_ref().unwrap().dial.louder();
            lv.click(louder);
            louder.set_state(LV_STATE_PRESSED, true);
            screen.update(&snap);
            assert_eq!(sent(&mut bus), ["SetSpeakerVolume(60)"]);
            let parts = screen.parts.as_ref().unwrap();
            assert!(parts.title.is_hidden(), "the gauge replaces the dial text");
            assert_golden("home_volume", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_dial_mutes_and_the_pill_opens_the_menu() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut screen = HomeScreen::default();
            screen.build(root, &ctx, &snap());
            let parts = screen.parts.as_ref().unwrap();
            lv.click(parts.dial.hit());
            lv.click(parts.status.cluster());
            assert_eq!(sent(&mut bus), ["ToggleMicMute"]);
            assert_eq!(events(&ui), [UiEvent::Navigate(Route::Menu)]);
        });
    }

    #[test]
    fn record_starts_once_and_explains_when_blocked() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut snap = snap();
            let mut screen = HomeScreen::default();
            screen.build(root, &ctx, &snap);
            let record = screen.parts.as_ref().unwrap().record.obj();
            lv.click(record);
            lv.click(record);
            assert_eq!(sent(&mut bus), ["StartRecording"], "a double tap starts once");
            snap.now_ms += RECORD_DEBOUNCE_MS;
            screen.update(&snap);
            lv.click(record);
            assert_eq!(sent(&mut bus), ["StartRecording"], "a later tap retries");
            snap.meeting.storage_blocked = true;
            screen.update(&snap);
            lv.click(record);
            assert!(sent(&mut bus).is_empty());
            assert_eq!(
                events(&ui),
                [UiEvent::Show(Message::new(Severity::Error, copy::STORAGE_FULL))]
            );
            snap.meeting.storage_blocked = false;
            snap.meeting.recording_blocked_reason =
                Some(waved_state::RecordingIntegrityFault::WavWriteFailed);
            assert_eq!(record_blocked(&snap), Some(copy::RECORDING_UNAVAILABLE));
        });
    }

    #[test]
    fn every_home_line_fits_inside_the_ring() {
        lvgl(|lv| {
            let mut cases = Vec::new();
            let mut s = snap();
            cases.push(s.clone()); // Connected by USB
            s.mcu.usb_connected = false;
            cases.push(s.clone()); // Not connected
            s.bt.connected = true;
            cases.push(s.clone()); // Connected by Bluetooth
            s.audio.host_listening = true;
            cases.push(s.clone()); // Computer is listening
            s.audio.call_off_hook = true;
            cases.push(s.clone()); // In a call
            s.audio.mic_muted = true;
            cases.push(s.clone()); // Call can't hear you
            for case in cases {
                let line = HomeView::from_snapshot(&case).line;
                let width = text_width(&line, ring_font(&line, Font::Body));
                assert!(width <= RING_TEXT_W, "{line:?} is {width} px");
            }
            // A long Bluetooth name stays one truncated line inside the ring.
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut long = snap_without_usb();
            long.bt.connected = true;
            long.bt.device_name = "Conference room phone on the third floor".into();
            let mut screen = HomeScreen::default();
            screen.build(root, &ctx, &long);
            let line = screen.parts.as_ref().unwrap().line;
            line.update_layout();
            let area = line.coords();
            assert_eq!(
                (area.x1, area.x2 - area.x1 + 1),
                (DIAL_CX - RING_TEXT_W / 2, RING_TEXT_W)
            );
            assert_eq!(area.y2 - area.y1 + 1, Font::Small.line_height(), "one line");
        });
    }
}
```

- [ ] **Step 2: Run and confirm the failure**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::home 2>&1 | tail -20`
Expected: compile error `cannot find type `HomeScreen``.

- [ ] **Step 3: Implement**

```rust
//! Home (A1, docs/mockups/png/home.png): the dial tells the table what the
//! microphones do and mutes on a tap; Quieter / Louder at the sides; Record
//! in the main spot; the status pill opens the menu (its teal dot shows
//! pre-recording, which replaced Record's old "includes last 30 s" line).

use crate::kit::components::buttons::RoundButton;
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::Severity;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    DIAL_CX, DIAL_HINT_Y, DIAL_ICON_X, DIAL_ICON_Y, DIAL_LINE_Y, DIAL_TITLE_Y, LIVE, REC,
    SCREEN_W, TEXT, TEXT_2, TEXT_3,
};
use crate::screens_v2::main_parts::{home_ring, ring_font, MainDial, RING_TEXT_W};
use crate::screens_v2::messages::copy;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::Command;

/// A second Record tap within this window is ignored (the Recording screen
/// takes over within a frame or two once the service starts).
pub const RECORD_DEBOUNCE_MS: u64 = 2_000;

/// The text inside Home's dial ("Dial states" in docs/mockups/README.md).
#[derive(Clone, Debug, PartialEq)]
pub struct HomeView {
    pub icon: &'static str,
    pub icon_color: u32,
    pub title: &'static str,
    pub title_color: u32,
    pub line: String,
    pub line_color: u32,
    pub hint: &'static str,
}

impl HomeView {
    pub fn from_snapshot(snap: &UiSnapshot) -> Self {
        let muted = snap.audio.mic_muted;
        let live = !muted && (snap.in_call() || snap.audio.host_listening);
        Self {
            icon: if muted { icons::MIC_OFF } else { icons::MIC },
            icon_color: if muted { REC } else { TEXT_2 },
            title: if muted { "Muted" } else { "Mic on" },
            title_color: if muted { REC } else { TEXT },
            line: snap.connection_line(),
            line_color: if live { LIVE } else { TEXT_2 },
            hint: if muted { "Tap to unmute" } else { "Tap to mute" },
        }
    }
}

/// Why Record can't start now, as the message to show instead.
pub fn record_blocked(snap: &UiSnapshot) -> Option<&'static str> {
    if snap.meeting.storage_blocked || snap.files.low_space {
        Some(copy::STORAGE_FULL)
    } else if snap.meeting.recording_blocked_reason.is_some() {
        Some(copy::RECORDING_UNAVAILABLE)
    } else {
        None
    }
}

/// What Record's tap needs from the latest snapshot.
#[derive(Clone, Copy, Default)]
struct RecordGate {
    blocked: Option<&'static str>,
    now_ms: u64,
    started_ms: Option<u64>,
}

#[derive(Default)]
pub struct HomeScreen {
    parts: Option<Parts>,
}

#[cfg_attr(not(test), allow(dead_code))] // `record` is read only by tests
struct Parts {
    status: StatusArea,
    dial: MainDial,
    icon: Obj,
    title: Obj,
    line: Obj,
    hint: Obj,
    record: RoundButton,
    gate: Rc<Cell<RecordGate>>,
    shown: Option<HomeView>,
    stack_hidden: bool,
}

impl Screen for HomeScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        let status = StatusArea::pill(root, &ctx.dispatch);
        let dial = MainDial::new(root, &ctx.dispatch, snap);
        let icon = Obj::label(root, icons::MIC, Font::Icon20, TEXT_2).pos(DIAL_ICON_X, DIAL_ICON_Y);
        let title = Obj::label(root, "", Font::Dial, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, DIAL_TITLE_Y);
        let line = Obj::label(root, "", Font::Body, TEXT_2)
            .centered_text(RING_TEXT_W)
            .pos(DIAL_CX - RING_TEXT_W / 2, DIAL_LINE_Y);
        let hint = Obj::label(root, "", Font::Micro, TEXT_3)
            .centered_text(SCREEN_W)
            .pos(0, DIAL_HINT_Y);
        let gate = Rc::new(Cell::new(RecordGate::default()));
        let record = {
            let (gate, dispatch) = (gate.clone(), ctx.dispatch.clone());
            RoundButton::record(root, move || {
                let mut now = gate.get();
                if let Some(text) = now.blocked {
                    dispatch.show(Severity::Error, text);
                } else if now
                    .started_ms
                    .is_none_or(|at| now.now_ms >= at + RECORD_DEBOUNCE_MS)
                {
                    now.started_ms = Some(now.now_ms);
                    gate.set(now);
                    dispatch.command(Command::StartRecording);
                }
            })
        };
        self.parts = Some(Parts {
            status,
            dial,
            icon,
            title,
            line,
            hint,
            record,
            gate,
            shown: None,
            stack_hidden: false,
        });
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        p.status.update(snap);
        let mut gate = p.gate.get();
        gate.blocked = record_blocked(snap);
        gate.now_ms = snap.now_ms;
        p.gate.set(gate);
        let gauge = p.dial.update(snap, home_ring(snap));
        if gauge != p.stack_hidden {
            for obj in [p.icon, p.title, p.line, p.hint] {
                obj.hidden(gauge);
            }
            p.stack_hidden = gauge;
        }
        let view = HomeView::from_snapshot(snap);
        if p.shown.as_ref() == Some(&view) {
            return;
        }
        p.icon.text_color(view.icon_color).set_text(view.icon);
        p.title.text_color(view.title_color).set_text(view.title);
        p.line
            .font(ring_font(&view.line, Font::Body))
            .centered_text(RING_TEXT_W)
            .pos(DIAL_CX - RING_TEXT_W / 2, DIAL_LINE_Y)
            .text_color(view.line_color)
            .set_text(&view.line);
        p.hint.set_text(view.hint);
        p.shown = Some(view);
    }
}
```

- [ ] **Step 4: Run; logic passes, goldens are missing**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::home 2>&1 | grep -E "missing golden|test result"`
Expected: three "missing golden" panics (`home`, `home_muted`, `home_volume`); the other tests pass.

- [ ] **Step 5: Generate and review the goldens**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::home 2>&1 | grep "test result"`

Compare `target/ui-preview/home.png` with `../docs/mockups/png/home.png`, `home_muted.png` with `home-muted.png`, `home_volume.png` with `home-volume.png`: ring, wedge and dot at 307°, text stack, side buttons, Record, status pill (pre-recording dot leading), message in the status slot (home_muted), gauge 60 % with the "%" on the digits' baseline and Louder pressed (home_volume). Accepted differences: Tabler icon shapes; LVGL antialiasing. Anything else is a defect: fix and regenerate.

- [ ] **Step 6: Run everything**

Run: `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | grep "test result"`
Expected: `ok.` with 0 failed.

- [ ] **Step 7: Lint** — lint gate. Expected: no output.

- [ ] **Step 8: Commit**

```bash
git add crates/waved-ui/src/screens_v2/home.rs crates/waved-ui/golden/home.png crates/waved-ui/golden/home_muted.png crates/waved-ui/golden/home_volume.png
git commit -m "feat(ui): home screen on the revision-2 kit

- dial text follows the dial table: in a call, computer listening, connected, muted
- tapping the dial mutes, the status pill opens the menu, the gauge hides the dial text
- record starts once per double tap and explains why when storage or integrity blocks it
- connection lines switch to small type and truncate inside the ring
- goldens compared with home.png, home-muted.png and home-volume.png

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Recording (A2)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/recording.rs`
- Create: goldens `recording.png`, `recording_muted.png`, `recording_hold_stop.png`, `recording_paused.png`, `recording_hours.png`

**Interfaces:**
- Consumes: `MainDial`, `recording_ring`, `format_timer`, `timer_font`, `RING_TEXT_W` (Tasks 1, 3); `copy::{HOLD_STOP, KEEP_HOLDING, marked_at, after_stop}` (Task 2); `StatusArea::pill`; `RoundButton::new(root, BottomSpot::Left, icons::MARK, "Mark", Tone::Neutral, ..)`, `RoundButton::stop(root, on_tap, on_stop)`, `RoundButton::new(root, BottomSpot::Right, icons::PAUSE, "Pause", Tone::Neutral, ..)`, `set_enabled`, `set_icon`, `set_label`, `set_accent`, `obj()`; `kit::event::on` with `crate::bindings::lv_event_code_t_LV_EVENT_PRESSED`; `Command::{AddBookmark, PauseRecording, ResumeRecording, StopRecording}`.
- Produces: `pub struct RecordingScreen` (Default, `Screen`); `pub struct RecordingView { pub chip: &'static str, pub chip_color: u32, pub chip_dot: bool, pub timer: String, pub timer_font: Font, pub timer_y: i32, pub line: String, pub line_color: u32, pub hint: &'static str, pub paused: bool }` with `pub fn from_snapshot(snap: &UiSnapshot) -> Self`.

Layout (recording.png, recording-muted.png, hold-stop.png, paused.png):

| Element | Font | Colour | Position |
|---|---|---|---|
| Status pill | kit | kit | `StatusArea::pill` (messages replace it: hold-stop.png) |
| Chip: 7 px dot + "Recording" / "Paused" (no dot) | Small | REC / WARN | row centred across 400, top `REC_CHIP_Y`, gap `REC_DOT_GAP` |
| Timer `format_timer` | Num48 (Num26 from an hour) | TEXT | centred, top `REC_TIMER_Y` (`TIMER_HOURS_Y` from an hour) |
| Line: "Speaker n talking" / "Listening" / "Not recording" / "Muted, not recorded" | Small | TEXT_2 / REC when muted | 124 wide at x 138, top `REC_LINE_Y` |
| "Tap to mute" / "Tap to unmute" | Micro | TEXT_3 | centred, top `REC_HINT_Y` |
| Mark (left companion) | kit | Neutral; Text 3 while paused | `BottomSpot::Left` |
| Stop (main, 1 s hold) | kit | red square and label | `RoundButton::stop` |
| Pause ⇄ Resume (right companion) | kit | Neutral / WarnText with PLAY | `BottomSpot::Right` |

Behaviour: Mark sends `AddBookmark` and shows "Marked at mm:ss" (disabled while paused). Pause sends `PauseRecording`, Resume `ResumeRecording`. Pressing Stop shows "Keep holding to stop"; releasing early shows "Hold Stop to end the recording"; a full 1 s hold sends `StopRecording` and shows `copy::after_stop(snap.settings.upload_mode)`, which also replaces "Keep holding to stop" before the screen changes.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::{lv_event_code_t_LV_EVENT_PRESSED, LV_STATE_PRESSED};
    use crate::kit::components::dial::RingStyle;
    use crate::kit::dispatch::{Message, UiEvent};
    use crate::kit::obj::text_width;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::kit::tokens::HOLD_TO_STOP_MS;
    use crate::screens_v2::main_parts::fixtures::{ctx, events, sent, snap, with_message};

    /// recording.png: speaker 2 talking at 60°.
    fn recording(secs: u32) -> UiSnapshot {
        let mut snap = snap();
        snap.meeting.recording = true;
        snap.meeting.duration_secs = secs;
        snap.meeting.current_speaker = Some(2);
        snap.audio.doa_active = true;
        snap.audio.doa_horizontal = 60.0;
        snap
    }

    fn show(severity: Severity, text: &str) -> UiEvent {
        UiEvent::Show(Message::new(severity, text))
    }

    #[test]
    fn recording_view_follows_the_dial_table() {
        let mut snap = recording(2_537);
        let view = RecordingView::from_snapshot(&snap);
        assert_eq!(
            (view.chip, view.chip_color, view.chip_dot, view.timer.as_str()),
            ("Recording", REC, true, "42:17")
        );
        assert_eq!((view.line.as_str(), view.hint), ("Speaker 2 talking", "Tap to mute"));
        snap.meeting.current_speaker = None;
        assert_eq!(RecordingView::from_snapshot(&snap).line, "Listening");
        snap.meeting.paused = true;
        let view = RecordingView::from_snapshot(&snap);
        assert_eq!(
            (view.chip, view.chip_color, view.chip_dot, view.line.as_str()),
            ("Paused", WARN, false, "Not recording")
        );
        snap.audio.mic_muted = true;
        let view = RecordingView::from_snapshot(&snap);
        assert_eq!(
            (view.line.as_str(), view.line_color, view.hint),
            ("Muted, not recorded", REC, "Tap to unmute")
        );
        assert_eq!(recording_ring(&snap).style, RingStyle::RecordingMuted);
    }

    #[test]
    fn recording_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            RecordingScreen::default().build(root, &ctx, &recording(2_537));
            assert_golden("recording", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn recording_muted_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = recording(2_702);
            snap.clock = "14:35".into();
            snap.audio.mic_muted = true;
            RecordingScreen::default().build(root, &ctx, &snap);
            assert_golden("recording_muted", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn holding_stop_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, ui) = ctx();
            let mut snap = recording(2_538);
            let mut screen = RecordingScreen::default();
            screen.build(root, &ctx, &snap);
            let stop = screen.parts.as_ref().unwrap().stop.obj();
            stop.set_state(LV_STATE_PRESSED, true);
            lv.send(stop, lv_event_code_t_LV_EVENT_PRESSED);
            lv.advance(620);
            assert_eq!(events(&ui), [show(Severity::Info, copy::KEEP_HOLDING)]);
            with_message(&mut snap, Severity::Info, copy::KEEP_HOLDING);
            screen.update(&snap);
            assert_golden("recording_hold_stop", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn paused_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = recording(2_537);
            snap.clock = "14:40".into();
            snap.battery.level = 81;
            snap.meeting.paused = true;
            let mut screen = RecordingScreen::default();
            screen.build(root, &ctx, &snap);
            let parts = screen.parts.as_ref().unwrap();
            assert!(!parts.mark.obj().is_clickable(), "Mark is off while paused");
            assert_golden("recording_paused", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn mark_pause_and_stop_send_their_commands() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut snap = recording(2_537);
            let mut screen = RecordingScreen::default();
            screen.build(root, &ctx, &snap);
            let (mark, pause, stop) = {
                let p = screen.parts.as_ref().unwrap();
                (p.mark.obj(), p.pause.obj(), p.stop.obj())
            };
            lv.click(mark);
            lv.click(pause);
            lv.click(stop); // a short tap
            assert_eq!(sent(&mut bus), ["AddBookmark", "PauseRecording"]);
            assert_eq!(
                events(&ui),
                [
                    show(Severity::Info, "Marked at 42:17"),
                    show(Severity::Info, copy::KEEP_HOLDING),
                    show(Severity::Info, copy::HOLD_STOP),
                ]
            );
            snap.meeting.paused = true;
            screen.update(&snap);
            lv.click(mark);
            lv.click(pause);
            assert_eq!(sent(&mut bus), ["ResumeRecording"], "Mark does nothing while paused");
            lv.send(stop, lv_event_code_t_LV_EVENT_PRESSED);
            lv.advance(HOLD_TO_STOP_MS as u32 + 100);
            assert_eq!(sent(&mut bus), ["StopRecording"]);
            assert_eq!(
                events(&ui).last(),
                Some(&UiEvent::Show(copy::after_stop(UploadMode::Automatic)))
            );
        });
    }

    #[test]
    fn the_timer_past_an_hour_fits_inside_the_ring() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut screen = RecordingScreen::default();
            screen.build(root, &ctx, &recording(3_723));
            let timer = screen.parts.as_ref().unwrap().timer;
            assert_eq!(timer.text(), "1:02:03");
            assert!(text_width("1:02:03", Font::Num26) <= RING_TEXT_W);
            assert_golden("recording_hours", &lv.render());
            assert_targets_ok(root);
        });
    }
}
```

- [ ] **Step 2: Run and confirm the failure**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::recording 2>&1 | tail -20`
Expected: compile error `cannot find type `RecordingScreen``.

- [ ] **Step 3: Implement**

```rust
//! Recording (A2, docs/mockups/png/recording.png): Home's layout, so mute and
//! volume stay where they were; the bottom becomes Mark, Stop and Pause on
//! the arc. Stop takes Record's spot and needs a 1 s hold (D1).

use crate::bindings::lv_event_code_t_LV_EVENT_PRESSED;
use crate::kit::components::buttons::{BottomSpot, RoundButton, Tone};
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::Severity;
use crate::kit::event::on;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    DIAL_CX, REC, REC_CHIP_Y, REC_DOT, REC_DOT_GAP, REC_HINT_Y, REC_LINE_Y, REC_TIMER_Y,
    SCREEN_W, TEXT, TEXT_2, TEXT_3, WARN,
};
use crate::screens_v2::main_parts::{
    format_timer, recording_ring, timer_font, MainDial, RING_TEXT_W,
};
use crate::screens_v2::messages::copy;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::{Command, UploadMode};

/// The text inside Recording's dial ("Dial states": Recording, Recording
/// muted, Paused).
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingView {
    pub chip: &'static str,
    pub chip_color: u32,
    pub chip_dot: bool,
    pub timer: String,
    pub timer_font: Font,
    pub timer_y: i32,
    pub line: String,
    pub line_color: u32,
    pub hint: &'static str,
    pub paused: bool,
}

impl RecordingView {
    pub fn from_snapshot(snap: &UiSnapshot) -> Self {
        let (audio, meeting) = (&snap.audio, &snap.meeting);
        let (timer_font, timer_y) = timer_font(meeting.duration_secs);
        let (line, line_color) = if audio.mic_muted {
            ("Muted, not recorded".to_string(), REC)
        } else if meeting.paused {
            ("Not recording".to_string(), TEXT_2)
        } else if let Some(speaker) = meeting.current_speaker {
            (format!("Speaker {speaker} talking"), TEXT_2)
        } else {
            ("Listening".to_string(), TEXT_2)
        };
        Self {
            chip: if meeting.paused { "Paused" } else { "Recording" },
            chip_color: if meeting.paused { WARN } else { REC },
            chip_dot: !meeting.paused,
            timer: format_timer(meeting.duration_secs),
            timer_font,
            timer_y,
            line,
            line_color,
            hint: if audio.mic_muted { "Tap to unmute" } else { "Tap to mute" },
            paused: meeting.paused,
        }
    }
}

/// What the button handlers need from the latest snapshot.
#[derive(Clone, Copy, Default)]
struct Live {
    paused: bool,
    duration_secs: u32,
    upload_mode: UploadMode,
}

#[derive(Default)]
pub struct RecordingScreen {
    parts: Option<Parts>,
}

#[cfg_attr(not(test), allow(dead_code))] // `stop` is read only by tests
struct Parts {
    status: StatusArea,
    dial: MainDial,
    chip: Obj,
    chip_dot: Obj,
    chip_text: Obj,
    timer: Obj,
    line: Obj,
    hint: Obj,
    mark: RoundButton,
    stop: RoundButton,
    pause: RoundButton,
    live: Rc<Cell<Live>>,
    shown: Option<RecordingView>,
    stack_hidden: bool,
}

impl Screen for RecordingScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        let status = StatusArea::pill(root, &ctx.dispatch);
        let dial = MainDial::new(root, &ctx.dispatch, snap);
        let chip = Obj::plain(root)
            .pos(0, REC_CHIP_Y)
            .size(SCREEN_W, Font::Small.line_height())
            .row(REC_DOT_GAP);
        let chip_dot = Obj::plain(chip).size(REC_DOT, REC_DOT).round().bg(REC);
        let chip_text = Obj::label(chip, "Recording", Font::Small, REC);
        let timer = Obj::label(root, "", Font::Num48, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, REC_TIMER_Y);
        let line = Obj::label(root, "", Font::Small, TEXT_2)
            .centered_text(RING_TEXT_W)
            .pos(DIAL_CX - RING_TEXT_W / 2, REC_LINE_Y);
        let hint = Obj::label(root, "", Font::Micro, TEXT_3)
            .centered_text(SCREEN_W)
            .pos(0, REC_HINT_Y);
        let live = Rc::new(Cell::new(Live::default()));

        let mark = {
            let (dispatch, live) = (ctx.dispatch.clone(), live.clone());
            RoundButton::new(root, BottomSpot::Left, icons::MARK, "Mark", Tone::Neutral, move || {
                let now = live.get();
                if !now.paused {
                    dispatch.command(Command::AddBookmark);
                    dispatch.show(Severity::Info, copy::marked_at(now.duration_secs));
                }
            })
        };
        let stop = {
            let (tap, done, live) = (ctx.dispatch.clone(), ctx.dispatch.clone(), live.clone());
            RoundButton::stop(
                root,
                move || tap.show(Severity::Info, copy::HOLD_STOP),
                move || {
                    done.command(Command::StopRecording);
                    // Also replaces "Keep holding to stop" before the screen changes.
                    let message = copy::after_stop(live.get().upload_mode);
                    done.show(message.severity, message.text);
                },
            )
        };
        {
            let (dispatch, obj) = (ctx.dispatch.clone(), stop.obj());
            on(obj, lv_event_code_t_LV_EVENT_PRESSED, move |_| {
                if obj.is_clickable() {
                    dispatch.show(Severity::Info, copy::KEEP_HOLDING);
                }
            });
        }
        let pause = {
            let (dispatch, live) = (ctx.dispatch.clone(), live.clone());
            RoundButton::new(root, BottomSpot::Right, icons::PAUSE, "Pause", Tone::Neutral, move || {
                dispatch.command(if live.get().paused {
                    Command::ResumeRecording
                } else {
                    Command::PauseRecording
                });
            })
        };
        self.parts = Some(Parts {
            status,
            dial,
            chip,
            chip_dot,
            chip_text,
            timer,
            line,
            hint,
            mark,
            stop,
            pause,
            live,
            shown: None,
            stack_hidden: false,
        });
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        p.status.update(snap);
        p.live.set(Live {
            paused: snap.meeting.paused,
            duration_secs: snap.meeting.duration_secs,
            upload_mode: snap.settings.upload_mode,
        });
        let gauge = p.dial.update(snap, recording_ring(snap));
        if gauge != p.stack_hidden {
            for obj in [p.chip, p.timer, p.line, p.hint] {
                obj.hidden(gauge);
            }
            p.stack_hidden = gauge;
        }
        let view = RecordingView::from_snapshot(snap);
        if p.shown.as_ref() == Some(&view) {
            return;
        }
        let paused_changed = !p
            .shown
            .as_ref()
            .is_some_and(|shown| shown.paused == view.paused);
        p.chip_dot.hidden(!view.chip_dot);
        p.chip_text.text_color(view.chip_color).set_text(view.chip);
        p.timer
            .font(view.timer_font)
            .centered_text(SCREEN_W)
            .pos(0, view.timer_y)
            .set_text(&view.timer);
        p.line.text_color(view.line_color).set_text(&view.line);
        p.hint.set_text(view.hint);
        if paused_changed {
            p.mark.set_enabled(!view.paused);
            if view.paused {
                p.pause.set_icon(icons::PLAY);
                p.pause.set_label("Resume");
                p.pause.set_accent(Tone::WarnText);
            } else {
                p.pause.set_icon(icons::PAUSE);
                p.pause.set_label("Pause");
                p.pause.set_accent(Tone::Neutral);
            }
        }
        p.shown = Some(view);
    }
}
```

- [ ] **Step 4: Run; logic passes, goldens are missing**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::recording 2>&1 | grep -E "missing golden|test result"`
Expected: five "missing golden" panics; `recording_view_follows_the_dial_table` and `mark_pause_and_stop_send_their_commands` pass.

- [ ] **Step 5: Generate and review the goldens**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::recording 2>&1 | grep "test result"`

Compare `recording.png` ↔ `../docs/mockups/png/recording.png`, `recording_muted.png` ↔ `recording-muted.png`, `recording_hold_stop.png` ↔ `hold-stop.png` (message in the status slot, red hold ring ~223°, circle Raised, wedge still drawn), `recording_paused.png` ↔ `paused.png` (dashed amber ring, amber "Paused" without dot, Mark in Text 3, amber Resume). `recording_hours.png` has no mockup: check the Num26 "1:02:03" sits centred on y 121 with clear space to the ring and the chip. Accepted differences: Tabler icons; antialiasing.

- [ ] **Step 6: Run everything** — `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | grep "test result"`. Expected: 0 failed.

- [ ] **Step 7: Lint** — lint gate. Expected: no output.

- [ ] **Step 8: Commit**

```bash
git add crates/waved-ui/src/screens_v2/recording.rs crates/waved-ui/golden/recording*.png
git commit -m "feat(ui): recording screen with mark, hold-to-stop and pause

- same dial, sides and gauge as home; red, muted and paused ring states
- mark bookmarks and confirms with marked at mm:ss; off while paused
- stop explains the hold on a tap, says keep holding while pressed, stops after 1 s
- pause and resume swap in place; the timer switches to h:mm:ss in num26 after an hour
- goldens compared with recording, recording-muted, hold-stop and paused mockups

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: After stop (A3)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/decision.rs`
- Create: goldens `decision.png`, `decision_no_server.png`, `decision_delete.png`

**Interfaces:**
- Consumes: `format_timer`, `format_countdown`, `format_size`, `server_host` (Task 1); `copy::{KEPT, SENDING, DELETED}` (Task 2); `StatusArea::plain`; `RoundButton::new` at `BottomSpot::{Left, Main, Right}`, `set_enabled`, `obj()`; `confirm_dialog(root, &ConfirmSpec, on_confirm, on_cancel) -> ButtonRow`, `ButtonRow::obj(i)`; `PendingDecision { id, duration_secs, size_bytes, endpoint, speakers, marks }` (contract 4); `MeetingState::ask_deadline_secs`; `Command::{SaveRecording, SendRecording, DiscardRecording}(Uuid)`.
- Produces: `pub struct DecisionScreen` (Default, `Screen`); `pub struct DecisionView { pub title: &'static str, pub duration: String, pub detail: String, pub server: Option<String> }`; `pub fn decision_view(decision: &PendingDecision) -> DecisionView`; `pub fn decision_detail(size_bytes: u64, speakers: u8, marks: u32) -> String`; `pub fn countdown_text(secs: Option<u32>) -> Option<String>`.

Layout (decision.png; the companions and main spot match Recording's, so after Stop the choices appear where Mark, Stop and Pause were):

| Element | Font | Colour | Position |
|---|---|---|---|
| Status cluster | kit | kit | `StatusArea::plain` |
| "Send this recording?" / "Keep this recording?" (no server) | Title | TEXT | centred, `TITLE_Y` |
| Duration `format_timer` | Num48 | TEXT | centred, top 73 (CSS 62 + 11) |
| "184 MB, 3 speakers, 4 marks" | Small | TEXT_2 | 280 wide at x 60, top 116 |
| "To notes.example.com" (only with a server) | Small | TEXT_3 | 280 wide at x 60, top 134 |
| "Keeps it on Wave in 0:52" (only while `ask_deadline_secs` is set) | Micro | TEXT_3 | 280 wide at x 60, top 152 (134 without a server) |
| Keep (left), DEVICE icon | kit | Neutral | `BottomSpot::Left` |
| Send (main), SEND icon (only with a server) | kit | Live | `BottomSpot::Main` |
| Delete (right), TRASH icon | kit | RecText | `BottomSpot::Right` |
| Delete confirmation (replaces the content) | kit | `Tone::RecText` | `confirm_dialog`: TRASH, "Delete recording?", "The 42:17 recording is erased from Wave and can't be recovered.", Cancel / Delete |

Behaviour: the first choice wins; later taps do nothing and the buttons lock (Text 3) on the next update. Keep → `SaveRecording(id)` + "Kept on Wave"; Send → `SendRecording(id)` + "Sending recording"; Delete → the confirmation on this screen, then `DiscardRecording(id)` + "Recording deleted" (Cancel returns). The confirmation stays on this screen rather than `Route::Confirm(DeleteRecording)` because that route is PIN-guarded under Files (D10 never locks the meeting flow) and its command is `DeleteRecording`, while the pending decision is cleared by `DiscardRecording`. The base screen leaves on its own when the service clears `pending_decision`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::{Message, UiEvent};
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::main_parts::fixtures::{ctx, events, sent, snap};
    use uuid::Uuid;

    const ID: u128 = 42;

    /// decision.png: 42:17, 184 MB, 3 speakers, 4 marks, 52 s left.
    fn pending(endpoint: &str) -> UiSnapshot {
        let mut snap = snap();
        snap.clock = "15:14".into();
        snap.battery.level = 79;
        snap.meeting.pending_decision = Some(PendingDecision {
            id: Uuid::from_u128(ID),
            audio_path: "/mnt/udisk/recordings/x/audio.wav".into(),
            duration_secs: 2_537,
            size_bytes: 184_000_000,
            endpoint: endpoint.into(),
            speakers: 3,
            marks: 4,
        });
        snap.meeting.ask_deadline_secs = Some(52);
        snap
    }

    #[test]
    fn decision_view_reads_the_pending_recording() {
        let snap = pending("https://user:secret@notes.example.com/hook");
        assert_eq!(
            decision_view(snap.meeting.pending_decision.as_ref().unwrap()),
            DecisionView {
                title: "Send this recording?",
                duration: "42:17".into(),
                detail: "184 MB, 3 speakers, 4 marks".into(),
                server: Some("To notes.example.com".into()),
            }
        );
        assert_eq!(decision_detail(2_400_000, 1, 0), "2 MB, 1 speaker");
        assert_eq!(decision_detail(2_400_000, 0, 1), "2 MB, 1 mark");
        assert_eq!(decision_detail(2_400_000, 0, 0), "2 MB");
        assert_eq!(
            countdown_text(Some(52)).as_deref(),
            Some("Keeps it on Wave in 0:52")
        );
        assert_eq!(countdown_text(None), None);
        let local = pending("");
        let view = decision_view(local.meeting.pending_decision.as_ref().unwrap());
        assert_eq!((view.title, view.server), ("Keep this recording?", None));
    }

    #[test]
    fn decision_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            DecisionScreen::default().build(root, &ctx, &pending("https://notes.example.com/hook"));
            assert_golden("decision", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn without_a_server_only_keep_and_delete_show() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut screen = DecisionScreen::default();
            screen.build(root, &ctx, &pending(""));
            let p = screen.parts.as_ref().unwrap();
            assert!(p.send.is_none() && p.keep.is_some() && p.delete.is_some());
            assert_eq!(p.title.text(), "Keep this recording?");
            p.countdown.update_layout();
            assert_eq!(p.countdown.coords().y1, SERVER_Y, "the countdown moves up");
            assert_golden("decision_no_server", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn a_decision_is_sent_once_and_the_buttons_lock() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let snap = pending("https://notes.example.com/hook");
            let mut screen = DecisionScreen::default();
            screen.build(root, &ctx, &snap);
            let (keep, send, delete) = {
                let p = screen.parts.as_ref().unwrap();
                (
                    p.keep.as_ref().unwrap().obj(),
                    p.send.as_ref().unwrap().obj(),
                    p.delete.as_ref().unwrap().obj(),
                )
            };
            lv.click(send);
            lv.click(keep);
            lv.click(delete);
            let id = Uuid::from_u128(ID);
            assert_eq!(sent(&mut bus), [format!("SendRecording({id:?})")]);
            assert_eq!(
                events(&ui),
                [UiEvent::Show(Message::new(Severity::Info, copy::SENDING))]
            );
            assert!(screen.parts.as_ref().unwrap().confirm.is_hidden(), "Delete no longer asks");
            screen.update(&snap);
            for button in [keep, send, delete] {
                assert!(!button.is_clickable(), "locked after the choice");
            }
        });
    }

    #[test]
    fn delete_asks_first_and_cancel_returns() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut screen = DecisionScreen::default();
            screen.build(root, &ctx, &pending("https://notes.example.com/hook"));
            let p = screen.parts.as_ref().unwrap();
            let (main, confirm, delete) = (p.main, p.confirm, p.delete.as_ref().unwrap().obj());
            let row = p.confirm_row.as_ref().unwrap();
            let (cancel, confirm_delete) = (row.obj(0), row.obj(1));
            lv.click(delete);
            assert!(main.is_hidden() && !confirm.is_hidden());
            assert_golden("decision_delete", &lv.render());
            assert_targets_ok(root);
            lv.click(cancel);
            assert!(!main.is_hidden() && confirm.is_hidden());
            assert!(sent(&mut bus).is_empty(), "cancel deletes nothing");
            lv.click(delete);
            lv.click(confirm_delete);
            let id = Uuid::from_u128(ID);
            assert_eq!(sent(&mut bus), [format!("DiscardRecording({id:?})")]);
            assert_eq!(
                events(&ui),
                [UiEvent::Show(Message::new(Severity::Success, copy::DELETED))]
            );
        });
    }

    #[test]
    fn the_countdown_follows_the_service_and_hides_without_one() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = pending("https://notes.example.com/hook");
            let mut screen = DecisionScreen::default();
            screen.build(root, &ctx, &snap);
            let countdown = screen.parts.as_ref().unwrap().countdown;
            assert_eq!(countdown.text(), "Keeps it on Wave in 0:52");
            snap.meeting.ask_deadline_secs = Some(51);
            screen.update(&snap);
            assert_eq!(countdown.text(), "Keeps it on Wave in 0:51");
            snap.meeting.ask_deadline_secs = None;
            screen.update(&snap);
            assert!(countdown.is_hidden());
        });
    }

    #[test]
    fn a_screen_without_a_pending_decision_has_no_actions() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut screen = DecisionScreen::default();
            screen.build(root, &ctx, &snap());
            let p = screen.parts.as_ref().unwrap();
            assert!(p.keep.is_none() && p.send.is_none() && p.delete.is_none());
            assert_targets_ok(root);
        });
    }
}
```

- [ ] **Step 2: Run and confirm the failure**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::decision 2>&1 | tail -20`
Expected: compile error `cannot find type `DecisionScreen``.

- [ ] **Step 3: Implement**

```rust
//! After stop (A3, docs/mockups/png/decision.png): "Send this recording?"
//! with Keep, Send and Delete on the arc where Mark, Stop and Pause were.
//! Shown while `MeetingState::pending_decision` is set ("Ask me each time").
//! The service keeps the file when nobody answers (D4, `ask_deadline_secs`,
//! C7); this screen only shows the countdown. Delete asks first, here.

use crate::kit::components::buttons::{BottomSpot, RoundButton, Tone};
use crate::kit::components::row::{confirm_dialog, ButtonRow, ConfirmSpec};
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::{Dispatcher, Severity};
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, SCREEN_H, SCREEN_W, TEXT, TEXT_2, TEXT_3, TITLE_Y};
use crate::screens_v2::main_parts::{format_countdown, format_size, format_timer, server_host};
use crate::screens_v2::messages::copy;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::{Command, PendingDecision};

/// docs/mockups/index.html #decision. The 48 px duration (CSS top 62) is
/// placed by its digits like `REC_TIMER_Y` (CSS top + 11).
const DURATION_Y: i32 = 73;
const DETAIL_Y: i32 = 116;
const SERVER_Y: i32 = 134;
const COUNTDOWN_Y: i32 = 152;
/// The detail lines can carry a long host: 280 px, truncated, centred.
const LINE_X: i32 = 60;
const LINE_W: i32 = 280;

#[derive(Clone, Debug, PartialEq)]
pub struct DecisionView {
    pub title: &'static str,
    pub duration: String,
    pub detail: String,
    /// "To notes.example.com", or `None` without a server.
    pub server: Option<String>,
}

pub fn decision_view(decision: &PendingDecision) -> DecisionView {
    let server = server_host(&decision.endpoint);
    DecisionView {
        title: if server.is_some() {
            "Send this recording?"
        } else {
            "Keep this recording?"
        },
        duration: format_timer(decision.duration_secs),
        detail: decision_detail(decision.size_bytes, decision.speakers, decision.marks),
        server: server.map(|host| format!("To {host}")),
    }
}

/// "184 MB, 3 speakers, 4 marks"; unknown (zero) parts are left out.
pub fn decision_detail(size_bytes: u64, speakers: u8, marks: u32) -> String {
    let mut parts = vec![format_size(size_bytes)];
    match speakers {
        0 => {}
        1 => parts.push("1 speaker".into()),
        n => parts.push(format!("{n} speakers")),
    }
    match marks {
        0 => {}
        1 => parts.push("1 mark".into()),
        n => parts.push(format!("{n} marks")),
    }
    parts.join(", ")
}

pub fn countdown_text(secs: Option<u32>) -> Option<String> {
    secs.map(|secs| format!("Keeps it on Wave in {}", format_countdown(secs)))
}

/// Keep, Send or the confirmed Delete: the first choice wins.
fn choice(
    dispatch: &Dispatcher,
    decided: &Rc<Cell<bool>>,
    command: Command,
    severity: Severity,
    text: &'static str,
) -> impl FnMut() + 'static {
    let (dispatch, decided) = (dispatch.clone(), decided.clone());
    move || {
        if !decided.replace(true) {
            dispatch.command(command.clone());
            dispatch.show(severity, text);
        }
    }
}

#[derive(Default)]
pub struct DecisionScreen {
    parts: Option<Parts>,
}

#[cfg_attr(not(test), allow(dead_code))] // layers, title and the confirm row are read only by tests
struct Parts {
    status: StatusArea,
    title: Obj,
    /// The question and its buttons, and the delete confirmation: two
    /// full-screen layers, one visible at a time.
    main: Obj,
    confirm: Obj,
    countdown: Obj,
    keep: Option<RoundButton>,
    send: Option<RoundButton>,
    delete: Option<RoundButton>,
    confirm_row: Option<ButtonRow>,
    decided: Rc<Cell<bool>>,
    locked: bool,
    shown_countdown: Option<Option<u32>>,
}

impl Screen for DecisionScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        // Round buttons need the root or a full-screen container without a layout.
        let layer = |hidden: bool| {
            Obj::plain(root)
                .rect(Rect::new(0, 0, SCREEN_W, SCREEN_H))
                .hidden(hidden)
        };
        let (main, confirm) = (layer(false), layer(true));
        let decision = snap.meeting.pending_decision.clone();
        let view = decision.as_ref().map(decision_view);
        let has_server = view.as_ref().is_some_and(|view| view.server.is_some());
        let title = Obj::label(
            main,
            view.as_ref().map_or("Send this recording?", |view| view.title),
            Font::Title,
            TEXT,
        )
        .centered_text(SCREEN_W)
        .pos(0, TITLE_Y);
        let countdown = Obj::label(main, "", Font::Micro, TEXT_3)
            .centered_text(LINE_W)
            .pos(LINE_X, if has_server { COUNTDOWN_Y } else { SERVER_Y })
            .hidden(true);
        let decided = Rc::new(Cell::new(false));
        let (mut keep, mut send, mut delete, mut confirm_row) = (None, None, None, None);
        if let (Some(decision), Some(view)) = (&decision, &view) {
            Obj::label(main, &view.duration, Font::Num48, TEXT)
                .centered_text(SCREEN_W)
                .pos(0, DURATION_Y);
            Obj::label(main, &view.detail, Font::Small, TEXT_2)
                .centered_text(LINE_W)
                .pos(LINE_X, DETAIL_Y);
            if let Some(server) = &view.server {
                Obj::label(main, server, Font::Small, TEXT_3)
                    .centered_text(LINE_W)
                    .pos(LINE_X, SERVER_Y);
            }
            let id = decision.id;
            keep = Some(RoundButton::new(
                main,
                BottomSpot::Left,
                icons::DEVICE,
                "Keep",
                Tone::Neutral,
                choice(&ctx.dispatch, &decided, Command::SaveRecording(id), Severity::Success, copy::KEPT),
            ));
            if has_server {
                send = Some(RoundButton::new(
                    main,
                    BottomSpot::Main,
                    icons::SEND,
                    "Send",
                    Tone::Live,
                    choice(&ctx.dispatch, &decided, Command::SendRecording(id), Severity::Info, copy::SENDING),
                ));
            }
            delete = Some({
                let decided = decided.clone();
                RoundButton::new(main, BottomSpot::Right, icons::TRASH, "Delete", Tone::RecText, move || {
                    if !decided.get() {
                        main.hidden(true);
                        confirm.hidden(false);
                    }
                })
            });
            let spec = ConfirmSpec {
                icon: icons::TRASH,
                title: "Delete recording?".into(),
                body: format!(
                    "The {} recording is erased from Wave and can't be recovered.",
                    view.duration
                ),
                confirm: "Delete".into(),
                tone: Tone::RecText,
            };
            confirm_row = Some(confirm_dialog(
                confirm,
                &spec,
                choice(&ctx.dispatch, &decided, Command::DiscardRecording(id), Severity::Success, copy::DELETED),
                move || {
                    confirm.hidden(true);
                    main.hidden(false);
                },
            ));
        }
        // Last, so its messages draw over both layers.
        let status = StatusArea::plain(root);
        self.parts = Some(Parts {
            status,
            title,
            main,
            confirm,
            countdown,
            keep,
            send,
            delete,
            confirm_row,
            decided,
            locked: false,
            shown_countdown: None,
        });
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        p.status.update(snap);
        let secs = snap.meeting.ask_deadline_secs;
        if p.shown_countdown != Some(secs) {
            match countdown_text(secs) {
                Some(text) => p.countdown.hidden(false).set_text(&text),
                None => {
                    p.countdown.hidden(true);
                }
            }
            p.shown_countdown = Some(secs);
        }
        if p.decided.get() && !p.locked {
            for button in [&p.keep, &p.send, &p.delete].into_iter().flatten() {
                button.set_enabled(false);
            }
            p.locked = true;
        }
    }
}
```

- [ ] **Step 4: Run; logic passes, goldens are missing**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::decision 2>&1 | grep -E "missing golden|test result"`
Expected: three "missing golden" panics; the other tests pass.

- [ ] **Step 5: Generate and review the goldens**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::decision 2>&1 | grep "test result"`

Compare `decision.png` ↔ `../docs/mockups/png/decision.png` (title, 42:17, three detail lines, Keep / Send / Delete on the arc). `decision_no_server.png`: same layout without the server line and Send, title "Keep this recording?", countdown at 134. `decision_delete.png` ↔ `../docs/mockups/png/confirm.png` layout (red-tinted disc with the trash icon, "Delete recording?", two-line body, Cancel / red Delete pills). Accepted differences: Keep's icon is Text (kit `Tone::Neutral`), the mockup's is Text 2; Tabler icons; antialiasing.

- [ ] **Step 6: Run everything** — `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | grep "test result"`. Expected: 0 failed.

- [ ] **Step 7: Lint** — lint gate. Expected: no output.

- [ ] **Step 8: Commit**

```bash
git add crates/waved-ui/src/screens_v2/decision.rs crates/waved-ui/golden/decision*.png
git commit -m "feat(ui): after-stop screen with keep, send and delete

- keep, send and delete on the arc where mark, stop and pause were
- the first choice wins and the buttons lock; delete confirms on this screen
- the countdown follows the service's ask deadline and hides without one
- without a server only keep and delete show, under keep this recording?
- goldens compared with decision.png and confirm.png

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Resting screen (A4)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/ambient.rs`
- Create: goldens `ambient.png`, `ambient_recording.png`

**Interfaces:**
- Consumes: `Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 2)`, `set_ring`; `format_timer`, `timer_font` (Task 1); tokens `REC_TIMER_Y`, `REC_DOT`, `REC_DOT_GAP`, `LIVE`, `REC`, `WARN`, `TEXT`, `TEXT_2`.
- Produces: `pub struct AmbientScreen` (Default, `Screen`); `pub struct AmbientView { pub ring: RingStyle, pub value: String, pub value_font: Font, pub value_y: i32, pub line: &'static str, pub line_color: u32, pub pre_recording: bool }`; `pub fn ambient_view(snap: &UiSnapshot) -> AmbientView`; `pub fn resting_clock(clock: &str) -> &str`.

Layout (ambient.png; the mockup's 45 % `.dimmer` layer stands for the dimmed backlight, which E1 sets, so the screen draws at full colour):

| Element | Font | Colour | Position |
|---|---|---|---|
| Ring, stroke 2 | — | Idle LINE / Live LIVE 40 % / Muted REC / Recording REC 35 % / RecordingMuted REC / Paused dashed WARN | (200, 121) r 70 |
| Clock (`snap.clock` without " am"/" pm") or the recording timer | Num48 (Num26 from an hour) | TEXT | centred, top `REC_TIMER_Y` (`TIMER_HOURS_Y`) |
| Mic line | Small | see view | 112 wide at x 144, top 145 |
| Pre-recording chip: 7 px dot + "Pre-recording" | Micro | LIVE | row centred across 400, top 236, gap `REC_DOT_GAP` |
| No status, no buttons; nothing is tappable (the runtime's wake guard takes the first touch) | | | |

Mic line: muted → "Mic off" REC; in a call → "Mic on, in a call" LIVE; computer listening → "Computer is listening" LIVE; otherwise "Mic on" TEXT_2. Recording: "Recording" REC, "Muted, not recorded" REC, or "Paused" WARN. The 12-hour suffix is dropped because "12:32 pm" (~151 px) would cross the ring.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::obj::text_width;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::main_parts::fixtures::{ctx, snap};

    fn clickable(obj: Obj) -> u32 {
        (0..obj.child_count() as i32)
            .map(|i| {
                let child = obj.child(i);
                u32::from(child.is_clickable()) + clickable(child)
            })
            .sum()
    }

    #[test]
    fn ambient_view_shows_the_mic_state_and_the_recording() {
        let mut snap = snap();
        snap.meeting.pre_recording_active = true;
        let view = ambient_view(&snap);
        assert_eq!(
            (view.ring, view.value.as_str(), view.line, view.line_color, view.pre_recording),
            (RingStyle::Idle, "14:32", "Mic on", TEXT_2, true)
        );
        snap.audio.call_off_hook = true;
        let view = ambient_view(&snap);
        assert_eq!((view.ring, view.line, view.line_color), (RingStyle::Live, "Mic on, in a call", LIVE));
        snap.audio.mic_muted = true;
        assert_eq!((ambient_view(&snap).ring, ambient_view(&snap).line), (RingStyle::Muted, "Mic off"));
        let mut rec = recording();
        let view = ambient_view(&rec);
        assert_eq!(
            (view.ring, view.value.as_str(), view.line, view.pre_recording),
            (RingStyle::Recording, "42:17", "Recording", false)
        );
        rec.meeting.paused = true;
        assert_eq!((ambient_view(&rec).ring, ambient_view(&rec).line), (RingStyle::Paused, "Paused"));
        rec.audio.mic_muted = true;
        assert_eq!(ambient_view(&rec).line, "Muted, not recorded");
        assert_eq!(resting_clock("2:32 pm"), "2:32");
        assert_eq!(resting_clock("12:05 am"), "12:05");
        assert_eq!(resting_clock("14:32"), "14:32");
    }

    fn recording() -> UiSnapshot {
        let mut snap = snap();
        snap.meeting.recording = true;
        snap.meeting.duration_secs = 2_537;
        snap
    }

    #[test]
    fn every_resting_line_fits_inside_the_ring() {
        lvgl(|_| {
            for line in [
                "Mic on",
                "Mic off",
                "Mic on, in a call",
                "Computer is listening",
                "Recording",
                "Muted, not recorded",
                "Paused",
            ] {
                let width = text_width(line, Font::Small);
                assert!(width <= LINE_W, "{line:?} is {width} px");
            }
            assert!(text_width("12:59", Font::Num48) <= crate::screens_v2::main_parts::RING_TEXT_W);
        });
    }

    #[test]
    fn ambient_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = snap();
            snap.audio.call_off_hook = true;
            snap.meeting.pre_recording_active = true;
            AmbientScreen::default().build(root, &ctx, &snap);
            assert_golden("ambient", &lv.render());
            assert_targets_ok(root);
            assert_eq!(clickable(root), 0, "the first touch only wakes");
        });
    }

    #[test]
    fn the_recording_variant_shows_the_ring_and_timer() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut screen = AmbientScreen::default();
            screen.build(root, &ctx, &recording());
            let parts = screen.parts.as_ref().unwrap();
            assert_eq!(parts.value.text(), "42:17");
            assert!(parts.chip.is_hidden());
            assert_golden("ambient_recording", &lv.render());
            assert_targets_ok(root);
            assert_eq!(clickable(root), 0);
        });
    }
}
```

- [ ] **Step 2: Run and confirm the failure**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::ambient 2>&1 | tail -20`
Expected: compile error `cannot find type `AmbientScreen``.

- [ ] **Step 3: Implement**

```rust
//! Resting screen (A4, docs/mockups/png/ambient.png): after "Dim after"
//! without a touch, Home shrinks to a clock, the mic state and the
//! pre-recording chip; while recording it shows the ring and the timer. The
//! backlight level and the off timer are E1's; the first-touch wake guard is
//! the runtime's, so nothing here is tappable.

use crate::kit::components::dial::{Dial, RingStyle};
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    DIAL_CX, DIAL_CY, DIAL_R, LIVE, REC, REC_DOT, REC_DOT_GAP, REC_TIMER_Y, SCREEN_W, TEXT,
    TEXT_2, WARN,
};
use crate::screens_v2::main_parts::{format_timer, timer_font};

/// ambient.png: the ring is drawn at stroke 2.
const RING_WIDTH: i32 = 2;
/// The mic line, CSS top 145; the ring's inner chord there is ~115 px.
const LINE_Y: i32 = 145;
const LINE_W: i32 = 112;
/// The pre-recording chip, CSS top 236.
const CHIP_Y: i32 = 236;

#[derive(Clone, Debug, PartialEq)]
pub struct AmbientView {
    pub ring: RingStyle,
    pub value: String,
    pub value_font: Font,
    pub value_y: i32,
    pub line: &'static str,
    pub line_color: u32,
    pub pre_recording: bool,
}

pub fn ambient_view(snap: &UiSnapshot) -> AmbientView {
    let (audio, meeting) = (&snap.audio, &snap.meeting);
    if meeting.recording {
        let (value_font, value_y) = timer_font(meeting.duration_secs);
        let (ring, line, line_color) = if audio.mic_muted {
            (RingStyle::RecordingMuted, "Muted, not recorded", REC)
        } else if meeting.paused {
            (RingStyle::Paused, "Paused", WARN)
        } else {
            (RingStyle::Recording, "Recording", REC)
        };
        AmbientView {
            ring,
            value: format_timer(meeting.duration_secs),
            value_font,
            value_y,
            line,
            line_color,
            pre_recording: false,
        }
    } else {
        let (ring, line, line_color) = if audio.mic_muted {
            (RingStyle::Muted, "Mic off", REC)
        } else if snap.in_call() {
            (RingStyle::Live, "Mic on, in a call", LIVE)
        } else if audio.host_listening {
            (RingStyle::Live, "Computer is listening", LIVE)
        } else {
            (RingStyle::Idle, "Mic on", TEXT_2)
        };
        AmbientView {
            ring,
            value: resting_clock(&snap.clock).to_string(),
            value_font: Font::Num48,
            value_y: REC_TIMER_Y,
            line,
            line_color,
            pre_recording: meeting.pre_recording_active,
        }
    }
}

/// The clock without its 12-hour suffix: "12:32 pm" would cross the ring.
pub fn resting_clock(clock: &str) -> &str {
    clock
        .strip_suffix(" am")
        .or_else(|| clock.strip_suffix(" pm"))
        .unwrap_or(clock)
}

#[derive(Default)]
pub struct AmbientScreen {
    parts: Option<Parts>,
}

struct Parts {
    dial: Dial,
    value: Obj,
    line: Obj,
    chip: Obj,
    shown: Option<AmbientView>,
}

impl Screen for AmbientScreen {
    fn build(&mut self, root: Obj, _ctx: &ScreenCtx, snap: &UiSnapshot) {
        let dial = Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, RING_WIDTH);
        let value = Obj::label(root, "", Font::Num48, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, REC_TIMER_Y);
        let line = Obj::label(root, "", Font::Small, TEXT_2)
            .centered_text(LINE_W)
            .pos(DIAL_CX - LINE_W / 2, LINE_Y);
        let chip = Obj::plain(root)
            .pos(0, CHIP_Y)
            .size(SCREEN_W, Font::Micro.line_height())
            .row(REC_DOT_GAP)
            .hidden(true);
        Obj::plain(chip).size(REC_DOT, REC_DOT).round().bg(LIVE);
        Obj::label(chip, "Pre-recording", Font::Micro, LIVE);
        self.parts = Some(Parts {
            dial,
            value,
            line,
            chip,
            shown: None,
        });
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        let view = ambient_view(snap);
        if p.shown.as_ref() == Some(&view) {
            return;
        }
        if p.shown.as_ref().is_none_or(|shown| shown.ring != view.ring) {
            p.dial.set_ring(view.ring);
        }
        p.value
            .font(view.value_font)
            .centered_text(SCREEN_W)
            .pos(0, view.value_y)
            .set_text(&view.value);
        p.line.text_color(view.line_color).set_text(view.line);
        p.chip.hidden(!view.pre_recording);
        p.shown = Some(view);
    }
}
```

- [ ] **Step 4: Run; logic passes, goldens are missing**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::ambient 2>&1 | grep -E "missing golden|test result"`
Expected: two "missing golden" panics; the other tests pass.

- [ ] **Step 5: Generate and review the goldens**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::ambient 2>&1 | grep "test result"`

Compare `ambient.png` ↔ `../docs/mockups/png/ambient.png`. Accepted differences: the golden is brighter (no `.dimmer`: the backlight dims on the device); the Live ring is 40 % opaque (kit `RingStyle::Live`) where the mockup draws 50 %. `ambient_recording.png` (no mockup): dim red ring, 42:17, red "Recording", no chip.

- [ ] **Step 6: Run everything** — `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | grep "test result"`. Expected: 0 failed.

- [ ] **Step 7: Lint** — lint gate. Expected: no output.

- [ ] **Step 8: Commit**

```bash
git add crates/waved-ui/src/screens_v2/ambient.rs crates/waved-ui/golden/ambient*.png
git commit -m "feat(ui): resting screen with clock, mic state and pre-recording chip

- clock, ring and mic line from the snapshot; the 12-hour suffix is dropped to fit the ring
- while recording: the recording ring and timer, muted and paused variants
- nothing is tappable, so the runtime's wake guard owns the first touch
- golden compared with ambient.png (no dimmer layer: the backlight dims)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Start-up (A5)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/boot.rs`
- Create: golden `boot.png`

**Interfaces:**
- Consumes: `Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3)`, `set_progress(&mut self, Option<f32>)`; `Dispatcher::ui(UiEvent::BootDone)`.
- Produces: `pub struct BootScreen` with `impl Default` (version `env!("CARGO_PKG_VERSION")`) and `pub fn with_version(version: &str) -> Self`, implementing `Screen`; `pub const RING_MS: u64 = 1_200`.

Layout (boot.png): progress ring on the dial circle (LINE track, LIVE progress clockwise from the top); "Starting" Title TEXT centred at top 99; "waved 0.9.3" Micro TEXT_3 centred at top 129. No status, nothing tappable. The ring fills over `RING_MS` from the screen's first snapshot; at full the screen sends `UiEvent::BootDone` once (the router then opens First start or the meeting base). The UI thread starts after the services in `main.rs`, so the ring is a short visual handover, not a progress measure.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::main_parts::fixtures::{ctx, events, snap};

    #[test]
    fn the_ring_draws_itself_then_boot_is_done_once() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, ui) = ctx();
            let mut now = snap();
            let mut boot = BootScreen::with_version("0.9.3");
            boot.build(root, &ctx, &now);
            now.now_ms += RING_MS - 1;
            boot.update(&now);
            assert!(events(&ui).is_empty(), "not before the ring is full");
            now.now_ms += 1;
            boot.update(&now);
            now.now_ms += 500;
            boot.update(&now);
            assert_eq!(events(&ui), [UiEvent::BootDone]);
        });
    }

    #[test]
    fn the_default_shows_this_build_s_version() {
        assert_eq!(BootScreen::default().version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn boot_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut now = snap();
            now.now_ms = 0;
            let mut boot = BootScreen::with_version("0.9.3");
            boot.build(root, &ctx, &now);
            now.now_ms = 733; // 61 %: the mockup's 220° arc
            boot.update(&now);
            assert_golden("boot", &lv.render());
            assert_targets_ok(root);
        });
    }
}
```

- [ ] **Step 2: Run and confirm the failure**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::boot 2>&1 | tail -20`
Expected: compile error `cannot find type `BootScreen``.

- [ ] **Step 3: Implement**

```rust
//! Start-up (A5, docs/mockups/png/boot.png): the ring draws itself, then the
//! screen hands over to First start or Home with `UiEvent::BootDone`. The
//! only animation that plays without a touch.

use crate::kit::components::dial::Dial;
use crate::kit::dispatch::{Dispatcher, UiEvent};
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{DIAL_CX, DIAL_CY, DIAL_R, SCREEN_W, TEXT, TEXT_3};

/// How long the ring takes to draw before start-up is done.
pub const RING_MS: u64 = 1_200;
/// boot.png: "Starting" (title, CSS top 99) and the version (micro, top 129).
const STARTING_Y: i32 = 99;
const VERSION_Y: i32 = 129;

pub struct BootScreen {
    version: String,
    parts: Option<Parts>,
}

struct Parts {
    dial: Dial,
    dispatch: Dispatcher,
    started_ms: u64,
    shown_permille: Option<u64>,
    done: bool,
}

impl Default for BootScreen {
    fn default() -> Self {
        Self::with_version(env!("CARGO_PKG_VERSION"))
    }
}

impl BootScreen {
    /// A start-up screen showing `version` (tests pin it so goldens stay put).
    pub fn with_version(version: &str) -> Self {
        Self {
            version: version.into(),
            parts: None,
        }
    }
}

impl Screen for BootScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        let dial = Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3);
        Obj::label(root, "Starting", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, STARTING_Y);
        Obj::label(root, &format!("waved {}", self.version), Font::Micro, TEXT_3)
            .centered_text(SCREEN_W)
            .pos(0, VERSION_Y);
        self.parts = Some(Parts {
            dial,
            dispatch: ctx.dispatch.clone(),
            started_ms: snap.now_ms,
            shown_permille: None,
            done: false,
        });
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        let elapsed = snap.now_ms.saturating_sub(p.started_ms);
        let permille = elapsed.min(RING_MS) * 1_000 / RING_MS;
        if p.shown_permille != Some(permille) {
            p.dial.set_progress(Some(permille as f32 / 1_000.0));
            p.shown_permille = Some(permille);
        }
        if elapsed >= RING_MS && !p.done {
            p.done = true;
            p.dispatch.ui(UiEvent::BootDone);
        }
    }
}
```

- [ ] **Step 4: Run; logic passes, the golden is missing**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::boot 2>&1 | grep -E "missing golden|test result"`
Expected: one "missing golden" panic; the other two pass.

- [ ] **Step 5: Generate and review the golden**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::boot 2>&1 | grep "test result"`

Compare `boot.png` ↔ `../docs/mockups/png/boot.png` (teal arc from the top to ~220°, grey track, "Starting", "waved 0.9.3"). Accepted difference: the kit's progress ring and track are stroke 6 (`Dial::set_progress`, shared with the update screen), the boot mockup draws stroke 3.

- [ ] **Step 6: Run everything** — `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | grep "test result"`. Expected: 0 failed.

- [ ] **Step 7: Lint** — lint gate. Expected: no output.

- [ ] **Step 8: Commit**

```bash
git add crates/waved-ui/src/screens_v2/boot.rs crates/waved-ui/golden/boot.png
git commit -m "feat(ui): start-up screen whose ring draws itself

- the progress ring fills over 1.2 s, then the screen sends boot done once
- shows this build's waved version; tests pin it so the golden stays stable
- golden compared with boot.png (kit progress stroke 6 vs the mockup's 3)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: First start (A5)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/first_run.rs`
- Create: golden `first_run.png`

**Interfaces:**
- Consumes: `StatusArea::plain`; `text_button(parent, rect, text, on_tap) -> Obj`; `RoundButton::new(root, BottomSpot::Main, icons::WIFI, "Set up Wi-Fi", Tone::Live, ..)`; `Command::MarkFirstRunDone`; `Dispatcher::navigate(Route::WifiSetup)`.
- Produces: `pub struct FirstRunScreen` (Default, `Screen`).

Layout (first-run.png):

| Element | Font | Colour | Position |
|---|---|---|---|
| Status cluster (clock and battery; no connections yet) | kit | kit | `StatusArea::plain` |
| "Set up Wave" | Dial | TEXT | centred, top 40 |
| "Wave works as a USB speakerphone right away. Connect Wi-Fi to send recordings to your own server." | Body | TEXT_2 | wrapped, centred, 270 wide at x 65, top 78, at most 3 lines |
| "Use without Wi-Fi" content pill | Body | TEXT_2 words on SURFACE | 176 × 44 at (112, 146), radius 22 |
| Set up Wi-Fi (main) | kit | Live (teal Wi-Fi icon and label) | `BottomSpot::Main` |

Behaviour: both choices send `MarkFirstRunDone` once; "Set up Wi-Fi" also opens `Route::WifiSetup` (B7). The router then swaps the FirstRun base for Home when `first_run_done` arrives, so Back from Wi-Fi setup lands on Home (backing out of Wi-Fi setup counts as skipping; Wi-Fi stays in the menu).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::main_parts::fixtures::{ctx, events, sent, snap};

    /// first-run.png: nothing connected yet, 09:02.
    fn fresh() -> UiSnapshot {
        let mut snap = snap();
        snap.mcu.usb_connected = false;
        snap.wifi.connected = false;
        snap.clock = "09:02".into();
        snap.settings.first_run_done = false;
        snap
    }

    #[test]
    fn first_run_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            FirstRunScreen::default().build(root, &ctx, &fresh());
            assert_golden("first_run", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn skipping_marks_first_start_done_once() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut screen = FirstRunScreen::default();
            screen.build(root, &ctx, &fresh());
            let skip = screen.parts.as_ref().unwrap().skip;
            lv.click(skip);
            lv.click(skip);
            assert_eq!(sent(&mut bus), ["MarkFirstRunDone"]);
            assert!(events(&ui).is_empty(), "the router moves on by itself");
        });
    }

    #[test]
    fn set_up_wifi_marks_done_and_opens_wifi_setup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut screen = FirstRunScreen::default();
            screen.build(root, &ctx, &fresh());
            let (setup, skip) = {
                let p = screen.parts.as_ref().unwrap();
                (p.setup.obj(), p.skip)
            };
            lv.click(setup);
            lv.click(skip);
            assert_eq!(sent(&mut bus), ["MarkFirstRunDone"]);
            assert_eq!(events(&ui), [UiEvent::Navigate(Route::WifiSetup)]);
        });
    }
}
```

- [ ] **Step 2: Run and confirm the failure**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::first_run 2>&1 | tail -20`
Expected: compile error `cannot find type `FirstRunScreen``.

- [ ] **Step 3: Implement**

```rust
//! First start (A5, docs/mockups/png/first-run.png): shown once, until Wi-Fi
//! is set up or skipped. Skipping is a full answer: Wave is a speakerphone
//! and local recorder without any network.

use crate::kit::components::buttons::{text_button, BottomSpot, RoundButton, Tone};
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, SCREEN_W, TEXT, TEXT_2};
use std::cell::Cell;
use std::rc::Rc;
use waved_state::Command;

/// first-run.png: the dial-size title (CSS top 40), the body (x 65, w 270,
/// top 78) and the "Use without Wi-Fi" content pill.
const SETUP_TITLE_Y: i32 = 40;
const BODY_X: i32 = 65;
const BODY_Y: i32 = 78;
const BODY_W: i32 = 270;
const SKIP: Rect = Rect::new(112, 146, 176, 44);
const BODY: &str =
    "Wave works as a USB speakerphone right away. Connect Wi-Fi to send recordings to your own server.";

#[derive(Default)]
pub struct FirstRunScreen {
    parts: Option<Parts>,
}

#[cfg_attr(not(test), allow(dead_code))] // `skip` and `setup` are read only by tests
struct Parts {
    status: StatusArea,
    skip: Obj,
    setup: RoundButton,
}

impl Screen for FirstRunScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        let status = StatusArea::plain(root);
        Obj::label(root, "Set up Wave", Font::Dial, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, SETUP_TITLE_Y);
        Obj::label(root, BODY, Font::Body, TEXT_2)
            .wrapped_text(BODY_W)
            .max_lines(3)
            .pos(BODY_X, BODY_Y);
        let done = Rc::new(Cell::new(false));
        let skip = {
            let (done, dispatch) = (done.clone(), ctx.dispatch.clone());
            text_button(root, SKIP, "Use without Wi-Fi", move || {
                if !done.replace(true) {
                    dispatch.command(Command::MarkFirstRunDone);
                }
            })
        };
        // Skipping is the quieter choice: grey words.
        skip.child(0).text_color(TEXT_2);
        let setup = {
            let dispatch = ctx.dispatch.clone();
            RoundButton::new(root, BottomSpot::Main, icons::WIFI, "Set up Wi-Fi", Tone::Live, move || {
                if !done.replace(true) {
                    dispatch.command(Command::MarkFirstRunDone);
                    dispatch.navigate(Route::WifiSetup);
                }
            })
        };
        self.parts = Some(Parts { status, skip, setup });
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(p) = self.parts.as_mut() {
            p.status.update(snap);
        }
    }
}
```

- [ ] **Step 4: Run; logic passes, the golden is missing**

Run: `set -o pipefail; cargo test -p waved-ui --lib screens_v2::first_run 2>&1 | grep -E "missing golden|test result"`
Expected: one "missing golden" panic; the other two pass.

- [ ] **Step 5: Generate and review the golden**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::first_run 2>&1 | grep "test result"`

Compare `first_run.png` ↔ `../docs/mockups/png/first-run.png`. Accepted differences: the pill's words are Body Medium (the mockup uses semibold 15); Tabler icons.

- [ ] **Step 6: Run everything** — `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | grep "test result"`. Expected: 0 failed.

- [ ] **Step 7: Lint** — lint gate. Expected: no output.

- [ ] **Step 8: Commit**

```bash
git add crates/waved-ui/src/screens_v2/first_run.rs crates/waved-ui/golden/first_run.png
git commit -m "feat(ui): first start screen with set up wi-fi and use without wi-fi

- both choices mark first start done once; set up wi-fi also opens wi-fi setup
- the router swaps the first-start base for home, so back from setup lands on home
- golden compared with first-run.png

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Device verification (end-of-M2 device pass, after the runtime switch)

Not a task here: these need the integrator's runtime switch and the C/E backends. Run them in the milestone 2 device pass.

1. **Boot → First start → Home:** after a factory reset, power on: the ring draws, "First start" appears; "Use without Wi-Fi" lands on Home and survives a reboot; on a second reset "Set up Wi-Fi" opens Wi-Fi setup and Back lands on Home.
2. **Home ring states:** USB connected, start a Teams/Zoom call (ring teal, "In a call", wedge follows a talker), open the mic without a call (QuickTime) for "Computer is listening", tap the dial (ring red, host shows muted), unplug USB ("Not connected").
3. **Volume:** tap and hold Louder/Quieter (steps every 300 ms, gauge for 1.5 s, no repeated value in the waved log); press Mac F11/F12 or move the host slider and see the gauge.
4. **Record → Recording:** Record starts once on a double tap; the timer runs; Mark shows "Marked at …" and adds a bookmark; Pause/Resume (needs C1) shows the dashed amber ring and greys Mark; a short Stop tap explains, a 1 s hold stops; mute mid-recording shows "Muted, not recorded"; a recording past one hour shows h:mm:ss inside the ring (or use a debug duration).
5. **After stop:** with "Ask me each time", the screen shows size, speakers, marks (needs contract 4 filled by C), host, and the countdown (needs C7); Send uploads and shows "Recording sent" on Home; Delete confirms; with an empty endpoint only Keep and Delete show.
6. **Resting:** after the dim timeout (E1) the resting screen shows clock, mic line and the pre-recording chip (needs C12); while recording it shows the ring and timer; the first touch only wakes.
7. **Messages:** pull Wi-Fi during an upload ("No Wi-Fi, upload waiting"), let it succeed ("Recording sent"), and, if the hardware allows, a battery below 10 % and the C6 microphone fault. Check that no message covers a button and each fits on one line on the panel.
8. **Text against the ring on the real panel:** look at the longest lines ("Connected by Bluetooth" in Small, "Computer is listening", "1:02:03") straight on and at 45°.

---

## Self-review

- **Spec coverage:** A1–A6 items map to Tasks 4, 5, 6, 7, 8–9 and 2 (see TODO coverage); every main-screen mockup (home, home-muted, home-volume, ambient, recording, recording-muted, hold-stop, paused, decision, boot, first-run) has a golden; the confirm layout is reused for Delete. Approved-design changes over TODO wording are applied: no "includes last 30 s", round Mark | Stop | Pause and Keep | Send | Delete, "Keep holding to stop" while pressed.
- **Types against the real APIs:** `Dial::{new, on_tap, set_ring, set_talker, set_gauge, set_progress, hit, talker_visible}`, `SideButton::{new, obj}`, `RoundButton::{new, record, stop, set_icon, set_label, set_accent, set_enabled, obj}`, `StatusArea::{plain, pill, update, cluster}`, `text_button`, `confirm_dialog`/`ConfirmSpec`/`ButtonRow::obj`, `Obj::{label, plain, pos, size, rect, row, round, bg, centered_text, one_line, wrapped_text, max_lines, width, font, text_color, hidden, set_text, text, child, child_count, coords, update_layout, set_state, is_clickable, is_hidden}`, `text_width`, `Dispatcher::{command, ui, navigate, show, test}`, `UiEvent::{Navigate, Show, BootDone}`, `Severity`, `Message::new`, `Route::{Menu, WifiSetup}`, and the `waved_state` fields used (`speaker_volume: u16`, `duration_secs: u32`, `current_speaker: Option<u8>`, `ask_deadline_secs: Option<u32>`, `pre_recording_active`, `storage_blocked`, `recording_blocked_reason`, `files.low_space`, `battery.{level, voltage, is_charging}`, `settings.upload_mode`) were checked against the code at `374015f`. `Command` has no `PartialEq`, so tests compare its Debug text. New fields come only from the contract requests (`UiSnapshot::message`/`ActiveMessage`, `AudioState::mic_fault`, `PendingDecision::{speakers, marks}`).
- **Placeholders:** none; every logic step has complete code, every layout element names its font, colour and position, and every golden names its mockup and accepted differences.
- **Review focus:** each of the five inputs has a test named in its task.
