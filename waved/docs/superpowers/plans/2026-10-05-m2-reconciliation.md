# Milestone 2 — Reconciliation and M2.0 Contracts

This document overrides the six milestone-2 plans wherever they disagree. Every implementer reads it together with their own plan.

Plans:
- A `2026-10-05-m2-a-main-screens.md`
- B `2026-10-05-m2-b-settings-screens.md`
- C `2026-10-05-m2-c-recording-audio.md`
- D `2026-10-05-m2-d-files.md`
- E `2026-10-05-m2-e-system-services.md`
- Z `2026-10-05-m2-z-runtime-switch.md` (M2.0 scaffolding S1–S2 and the end-of-M2 runtime switch Z1–Z10)

## Order

1. **M2.0 (integrator, serial):** Z's S1 and S2, plus every contract in "M2.0 contracts" below. Lands on `feat/display-redesign` before any workstream starts.
2. **Workstreams in parallel**, one worktree each, branched from the M2.0 head: A, B, C-meeting (C Tasks 1–11, 13), C-audio (C Tasks 12, 14–19; C13 waits for C12 — run C as one worktree with two sequential sub-lanes if simpler), D, E.
3. **Runtime switch (integrator):** Z1–Z9 after A, B, D, E and C merge; then Z10, the milestone-2 device pass, with the user.

## Rulings on conflicts between plans

| # | Topic | Ruling |
|---|---|---|
| R1 | Time zone / local clock (A, B, D, Z, E all asked) | E's shape wins: `waved_state::SystemState { utc_offset_min: i32, about: AboutInfo }` on `StateBus.system`; `UiSnapshot.system: SystemState`; `UiSnapshot::local_unix_s()`; `capture()` formats the status clock with `system.utc_offset_min` and shows `"--:--"` before `CLOCK_VALID_AFTER_UNIX_S`. B and D read `snap.system.utc_offset_min` / `snap.local_unix_s()` instead of `UiSnapshot.utc_offset_min`. Z1's tz-clock change is satisfied by this. |
| R2 | Device serial (B `DeviceInfo`, E `AboutInfo.serial`) | E's `AboutInfo` (in `SystemState.about`). B's About screen reads `snap.system.about`. No `DeviceInfo`. |
| R3 | Microphone fault (A `AudioState.mic_fault: bool`, C `MicHealth`) | C's `MicHealth` enum on `AudioState.mic_health`. A's messages use `matches!(snap.audio.mic_health, MicHealth::NotResponding { .. })` ("Microphones not responding") and may show `Degraded` as a warning. No `mic_fault`. |
| R4 | Message copy and device-event messages (A6 `messages.rs` + `MessageFeed`, Z2 `runtime/notices.rs`) | A owns all message copy and the event→message mapping in `screens_v2/messages.rs` (`MessageFeed::on_event`, `on_snapshot`). A adds Z's copy: "Upload failed, trying again", "Upload failed, see Uploads", "Recording failed, see Files", "Audio restarting", "Can't record, check storage", and D's "Can't play while recording", "Can't play during a call", "Storage almost full". Z2 calls A's `MessageFeed` from the runtime and keeps only the sound side effects; Z does not define message strings. |
| R5 | Swipe gestures (B `kit::event::{Swipe, on_swipe}` + `Lvgl::swipe`, Z3 runtime gestures, Z S2 `keep_gestures`) | M2.0 lands B's `Swipe`/`on_swipe` + `Lvgl::swipe` and Z's `Obj::keep_gestures()` together. Z3 builds the runtime's global swipe-down (Menu) / swipe-right (Back) and the wake guard on top of the same recogniser; screen-level `on_swipe` (e.g. Volume ⇄ Mic level swipe-left) takes precedence over the runtime's swipe-right only for left swipes. |
| R6 | Deleting a recording | After stop (A): inline `confirm_dialog` + `DiscardRecording` (pending decision). Files detail (D): navigates to B's `Route::Confirm(ConfirmKind::DeleteRecording(id))`. B's confirmation screen handles kept files only (`DeleteRecording`), and drops its "discard pending" branch. |
| R7 | Files backend (D proposed C13; C Task 11) | C Task 11 produces `FilesState` and must also fill `RecordingSummary.marks_ms` (see M2.0 contracts) and use `storage.minimum_free_mb` for `low_space`. D's amber free-space line follows `FilesState.low_space`. |
| R8 | `PendingDecision { speakers, marks }` (A) | Lands in M2.0 with the integrator setting 0 at `waved-meeting/src/lib.rs:170`; C fills real values (add to C's Task 7, which produces marks and speaker). |
| R9 | Screen type names (Z S1 table vs A/B/D plans) | Z S1's README table is the convention; where a workstream plan names a type differently, the workstream uses the S1 name. Z6 reconciles anything left. |
| R10 | Shutdown vs MCU power-off frame (E risk 2) | Z8 makes `main.rs` wait for the MCU service's power-off acknowledgement (or at least 3.5 s) after `Command::Shutdown` before exiting. Device-verified in Z10. |
| R11 | E and C integration hooks (E's Integration requests; C's H1–H3) | Z8 lands them in `main.rs` and the UI loop, using the exact code in the E and C plans. |
| R12 | `components/mod.rs` module ownership | S1 declares and stubs: `mode_picker` (B), `wifi` (B), `file_row` (D), `scrubber` (D). `screens_v2/mod.rs` also declares `main_parts` (A) and `messages` (A). |
| R13 | PIN in M2 | The runtime treats `pin_enabled` as false in M2 (Z); B's Menu lock icon still follows `snap.settings.pin_enabled` for display only. |
| R14 | `connection_line` copy | "Computer is listening" (needs user confirmation — see open questions). |

## M2.0 contracts (integrator lands these, in this order)

1. Z S1 scaffolding (screens_v2 + component stubs + README table), with R12's module list.
2. Z S2 `Obj::keep_gestures()`; B's `kit::event::{Swipe, on_swipe}` + `Lvgl::swipe` test helper (R5).
3. `waved-state`:
   - E CR1 `PowerKeyPress { Short, Long }`, `DeviceEvent::PowerKey(PowerKeyPress)`.
   - E CR2 `CLOCK_VALID_AFTER_UNIX_S`, `AboutInfo`, `SystemState`, `StateBus.system`.
   - C R1 `MicHealth` + `AudioState.mic_health`.
   - A `PendingDecision { speakers: u8, marks: u32 }` (+ 0 at `waved-meeting/src/lib.rs:170`).
   - D `RecordingSummary.marks_ms: Vec<u32>`.
4. `kit/snapshot.rs`: E CR3 (`UiSnapshot.system`, offset clock, `"--:--"`, `local_unix_s()`); A's `ActiveMessage { seq, message, since_ms }` + `UiSnapshot.message`, and `StatusArea::update` showing it when `seq` changes and it hasn't expired (Z1 relies on the same shape); `connection_line` copy per R14.
5. `kit/route.rs`: B's `ConfirmKind::ChangeNetwork`, `Route::TimeZone`, `Route::DimAfter`, with `guarded_by` (TimeZone, DimAfter → System; ChangeNetwork → Wifi).
6. `kit/tokens.rs`: D's `SUBTITLE_Y = 62`, `DETAIL_SIDE_LEFT/RIGHT = Rect(26|314, 104, 60, 60)`, labels `(11|299, 168)`, `DETAIL_NOTE_Y = 218`.
7. `kit/components`: D's `Tone::LiveIcon` and `RoundButton::at(parent, rect, label_pos, icon, text, accent, on_tap)` (non-repeating round button at an arbitrary rect); `list::fade` `pub(super)`; B's `RadioList::scroll_to(index)`.
8. `kit/testkit.rs`: tap-target check clips each object to its ancestors' boxes, as LVGL hit-testing does (B, D).
9. Icons `BELL`, `CALENDAR` (B): add to `fonts/icons.py`, regenerate with the pinned generator (output must stay reproducible).

Each M2.0 item gets tests in the owning crate; goldens must stay byte-identical except where the snapshot clock changes ("--:--" fixtures: set `now_unix_s` past 2020 in fixtures so existing goldens keep their clock).

## User decisions (2026-10-05)

- R14 copy: **"Computer is listening"**.
- Stereo over USB: **echo-free stereo** (L/R rebuilt around the echo-cancelled mid with a ducked side signal), as C plans.
- Pre-recording: **fixed 30 s**, on/off only.
- Mic level on the device: **0–200 %** (100 % in the middle); the host can still set up to 800 %.
- Execution approved: M2.0 contracts, then A–E in parallel, then the runtime switch and a device pass.

## Landed names (M2.0-1, commits 0721549..df466b8; M2.0-2, commits 1dfb53f..7fc75be) — use these, not the plan text

| Plan | Plan text | Use instead |
|---|---|---|
| B, D | `snap.utc_offset_min` | `snap.system.utc_offset_min`, or `snap.local_unix_s()` |
| B | `snap.device.serial` | `snap.system.about.serial` |
| A | `snap.audio.mic_fault` / `mic_fault: bool` | `matches!(snap.audio.mic_health, MicHealth::NotResponding { .. })` (`waved_state::MicHealth`) |
| Z1 | `ActiveMessage.id` in `kit::dispatch`; 4-argument `capture(.., utc_offset)` | `ActiveMessage.seq` in `kit::snapshot` (with `until_ms()`); `capture` keeps 3 arguments and reads the offset from `bus.system` |
| B | `on_swipe` via `LV_EVENT_GESTURE` + `lv_indev_get_gesture_dir` | Same `on_swipe(obj, Swipe, FnMut())` signature, delivered on a kit event code by `kit::event::send_swipe(obj, dir) -> bool`; LVGL's `LV_EVENT_GESTURE` is not used. Tests: `lv.swipe(obj, dir)` |
| Z3 | Own `SWIPE_MIN_PX`, `SWIPE_MAX_MS` and direction rule in `runtime/gesture.rs` | Reuse `kit::event::{Swipe, Swipe::recognise, SwipeTracker, SWIPE_MIN_PX, SWIPE_MAX_MS}` (60 px along one axis, at most half across, within 700 ms, once per press). Hand every swipe the runtime does not take itself to `send_swipe(object under the press start, dir)` |
| Z3 | — | The runtime blocks swipes that start on a `keep_gestures` object before recognising anything |
| Z3 | — | If `send_swipe` returns true, cancel the press (`lv_indev_wait_release`) exactly as for the runtime's own gestures, so a swipe never also fires a tap |
| B | `volume::LevelScreen::speaker()` / `::mic()` | `volume::VolumeScreen::speaker()` / `::mic()` (R9; README "Registering a screen") |
| B, D | — | `Obj::parent() -> Option<Obj>` exists. `RadioList` shows whole rows only (three from `RADIO_TOP`, y 68–231) and snaps to rows; `scroll_to(index)` puts that row at the top, or shows the last rows when it is near the end. It does not centre the row |

Note for B (Date and time): `local_unix_s()` has no validity guard; before the clock is valid (`now_unix_s < CLOCK_VALID_AFTER_UNIX_S`) show the date as "--" like the clock.

## Runtime switch addendum (after lanes A–E merged)

The Z plan predates the merged workstreams. Where it disagrees with this section, this section wins. Read the merged code before the plan text. The plan's tests stay unless a row below removes the code they test.

| # | Z task | Ruling |
|---|---|---|
| RS1 | Z1 | Mostly landed in M2.0: `ActiveMessage { seq, message, since_ms }` in `kit::snapshot`, `UiSnapshot.message`, the offset clock, `"--:--"`. Z1 shrinks to what the runtime still needs: posting a `Dispatcher::show` message (or a `MessageFeed` message) as the next `ActiveMessage` with a new `seq`, and dropping it once it expires. Don't re-add `ActiveMessage` to `kit::dispatch`. |
| RS2 | Z2 | No `runtime/notices.rs` copy (R4). The runtime calls `screens_v2::messages::MessageFeed`: `on_event(&event, &snap)` for every `DeviceEvent`, `on_command(&cmd)` for every command a screen sends, and `on_snapshot(&snap)` on every loop iteration from boot. Don't wire D's low-space notice, which was deleted; `on_snapshot` covers "Storage almost full". `feedback.rs` loses the mute, volume and shutdown sounds (E IR4: not in the spec, and `aplay` can't open `hw:0,1` while `PlaybackService` holds it). Recording start and stop tones come from `waved_system::tones` (IR2). If nothing is left in `feedback.rs`, don't create it. |
| RS3 | Z3 | Gestures reuse `kit::event::{Swipe, Swipe::recognise, SwipeTracker, SWIPE_MIN_PX, SWIPE_MAX_MS, send_swipe}` (Landed names). The `keep_gestures` block checks the clickable object LVGL pressed (`lv_indev_get_active_obj`), or walks up from the hit object and skips non-clickable ones (B: ModePicker aim area). A swipe the runtime does not take goes to `send_swipe(pressed object, dir)`; if that returns true, cancel the press. Leaving Wi-Fi setup with swipe Back or ToBase while provisioning sends `CancelWifiProvisioning` (B). Swipe Back from File detail while playing sends `StopPlayback` (D). The wake gate is E's `DisplayPower::on_touch(pressed, now_ms) -> TouchOutcome { disposition, transition }`: deliver the press to LVGL only when `disposition == TouchDisposition::Deliver`. No separate `WakeGate`. |
| RS4 | Z4 | E's `waved_system::display::DisplayPower` (`on_touch`, `on_power_key`, `tick -> Option<Transition>`, `backlight_pct(brightness_pct)`) and `waved_system::backlight::Backlight::{open(BACKLIGHT_DIR), set_percent}` replace Z4's `PowerPolicy` / `Backlight` traits and `PowerDriver`. Drive them exactly as E IR3 shows. If the runtime needs a seam for host tests, use one small trait over `set_percent` only. `keep_lit = snap.meeting.recording \|\| snap.in_call()`. |
| RS5 | Z5 | The runtime refreshes `snap.now_ms` every tick (A). Only the top screen gets `update()` (D). PIN masked (R13). Before routing, keep the M3-routes-go-back rule. |
| RS6 | Z6 | Register the merged types: `home::HomeScreen`, `recording::RecordingScreen`, `decision::DecisionScreen`, `ambient::AmbientScreen`, `boot::BootScreen`, `first_run::FirstRunScreen`, `menu::MenuScreen`, `audio_mode::AudioModeScreen`, `noise::NoiseScreen`, `volume::VolumeScreen::{speaker, mic}`, `system::SystemScreen`, `display::{DisplayScreen, DimAfterScreen}`, `datetime::{DateTimeScreen, TimeZoneScreen}`, `wifi::WifiScreen`, `wifi_setup::WifiSetupScreen`, `uploads::UploadsScreen`, `upload_mode::UploadModeScreen`, `confirm::ConfirmScreen::new(kind)`, `about::AboutScreen`, `files::FilesScreen`, `file_detail::FileDetailScreen::new(id)`. Check each constructor in the source. The shared date helper is already `kit::time` (M2-INT-1). |
| RS7 | Z7–Z8 | The power key reaches the UI as `DeviceEvent::PowerKey` on `bus.events`, from E's `power_key::start` (not `UiInputs`). `main.rs` calls `power_key::start` only when the env var `WAVED_POWER_KEY=1` is set, until Z10 confirms that the `vad_keypad` node is the physical key (no presses from speech or claps). Then Z10 makes it the default. Z8 also lands E IR1–IR3 and IR5 as written, C's H1–H3 as written, and R10 (after `Command::Shutdown`, wait for the MCU power-off acknowledgement or at least 3.5 s). It adds `WAVED_BUILD_ID` to `Cross.toml` `[build.env] passthrough` and sets it to the git short sha in the build command (About shows it). Z8 starts only after lane C has merged; rebase onto it first. |
| RS8 | Z9 | Keep `OpenWifiProvisioning` / `CancelWifiProvisioning`. `LV_FONT_DEFAULT` = `font_body_15`. |
| RS9 | Z10 | Device pass checklist = A plan "Device verification" 1–8; B lane list (in the ledger); D plan checks plus knob drag/flick, tap during a status change, the touch point after the 180° flip, row rebuild cost; `crates/waved-system/README.md` 12 steps; C plan "Device verification". Deploy rules: `project_device_deploy_gotchas`. |
