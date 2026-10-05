# Display Redesign — Milestone 2: Scaffolding and the End-of-M2 Runtime Switch (integrator) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two integrator jobs bracket milestone 2. **M2.0** (before any workstream task) pre-creates every `screens_v2` module and the two workstream-owned kit component files, so workstreams A, B and D only ever edit their own files, and lands the one kit convention they need (controls that keep their drags from screen gestures). **The runtime switch** (after A, B, D and E are merged) replaces the legacy `waved-ui` loop with one `Router`, a `screens_v2` registry and a `Dispatcher`, fed by a `UiSnapshot` per iteration; turns device events into status-slot messages; adds swipe-down / swipe-right gestures and the wake guard; drives workstream E's display power controller; deletes the legacy UI, the `Open*`/`CloseSettings` commands (BUG-11, BUG-13), the PNG assets and Montserrat; and runs the milestone-2 device pass (D6).

**Architecture:**
- A new `waved-ui/src/runtime/` module holds everything the legacy `lib.rs` loop did, split into host-testable pieces: `Runtime` (router + registry + dispatcher + messages, one `step(snapshot, events)` per iteration), `notices` (device events → `Message`), `feedback` (command → sound), `touch` (`TouchSample`, wake gate), `gesture` (swipe recognizer and the drag-owner hit test), `power` (a `PowerDriver` over workstream E's timers), `device` (LVGL display and pointer registration) and `run` (the UI thread loop).
- Messages travel in the snapshot: the runtime keeps one `ActiveMessage` and puts it in `UiSnapshot::message`; every screen's `StatusArea::update` shows it. A message survives navigation and expires 2 s / 4 s after it was posted.
- Gestures and the wake guard work on raw touch samples before LVGL sees them, so they do not depend on which object is under the finger. A press that starts on an object with `LV_OBJ_FLAG_GESTURE_BUBBLE` cleared (`Obj::keep_gestures()`) never becomes a swipe.
- The PIN screen is milestone 3 (K2), so the runtime masks `settings.pin_enabled` before routing; nothing locks in M2.

**Tech Stack:** Rust 2021, LVGL 9.2 via bindgen, tokio broadcast/watch (state bus), `std::sync::mpsc` (UI channel), the kit testkit (headless LVGL, goldens), `cross` + `adb` for the device pass, `scripts/linux-test.sh` for the `waved` binary.

**Spec:**
- `../docs/mockups/README.md` (Interaction model, Timings, Components: Message; approved commit c244c9f) and `../docs/mockups/index.html` (Messages section)
- `../TODO.md`: P0.1 (Open* removal), P0.5, A6, E1/E2 (UI side), BUG-3, BUG-5, BUG-11, BUG-13, L3, decisions D1–D10 (D6 device pass, D10 PIN)
- `docs/superpowers/plans/2026-10-04-display-redesign-m1-foundation.md` ("After milestone 1", Task 22 device-check commands)
- `docs/superpowers/plans/2026-10-05-display-relayout-kit.md` (geometry)
- `crates/waved-ui/src/kit/README.md`, `docs/DISPLAY_HARDWARE.md`, `CLAUDE.md`
- Shared brief: `../.superpowers/m2-planning/planning-brief.md`

All paths are relative to `waved/` unless they start with `../`.

---

## Global Constraints

Binding values this plan relies on (copied from the spec and the M1 / M1.1 plans):

- Framebuffer 400 × 304. Visible area: circle **r 198 centred (200, 106)**, flat top at y 0. Tap limit: every tappable pixel within **r 190 of (200, 106)** and at **y ≥ 6**; targets ≥ 44 px.
- Status zone **y 6–34** ("Status | 6–34 | Status cluster or status pill … Messages replace it for a few seconds."). The swipe-down gesture must start in this zone (`TOP_EDGE_Y` = 34).
- Messages: "Message | h 28, max w 240, centred, top 6, radius 14 | top layer, not clickable, hides the status for its duration". Rule 5: "Messages replace the status pill. One line, ≤ 240 px, never clickable, never over a button." `StatusArea` caps the text at `MESSAGE_MAX_W - 44` = **196 px** (icon and padding take the rest).
- Timings: "Success and info messages | 2 s"; "Warning and error messages | 4 s"; "Resting screen ("Dim after") | 2 min without a touch, configurable"; "Screen off | 10 min without a touch, unless recording or in a call (then resting only)".
- Interaction model rows this plan implements:
  - "Tap the status pill, or swipe down from the top edge | Home, Recording | Open Menu"
  - "Tap Back, or swipe right | Any sub-screen | Go back one level"
  - "First tap on the resting screen | Resting | Wake only, nothing else happens"
  - "Power key, short press | Anywhere | Screen to resting/off, or wake"
  - "Power key, hold 2 s | Anywhere | Power-off confirmation"
- Copy: sentence case and plain words; a button names its action and the message that follows uses the same verb ("Send" → "Recording sent"); errors say what happened and what to do next; no "…" or "..." in finished text; ASCII only.
- D10: the PIN locks Files, Wi-Fi, Bluetooth, Uploads and System — but the PIN screen and enforcement are milestone 3 (K2), so M2 locks nothing.
- Touch hardware facts (from `memory/project_touch_input.md`, `src/input.rs`): ILITEK_TDDI on `/dev/input/event1` reports **native pixels** (x 0–399, y 0–303), not 0–2047; the panel is mounted upside down, so touch needs the same **180° flip** as `display.rs` (`x = 399 - raw_x`, `y = 303 - raw_y`); `lv_layer_top()` must not be CLICKABLE or it swallows every tap.
- Raw `crate::bindings` calls belong only in `kit/obj.rs`, `kit/event.rs`, `kit/testkit.rs`, `kit/components/*` and, from this plan, `runtime/device.rs`, `runtime/gesture.rs` (`drag_owner_at`) and `runtime/mod.rs` (`load`). Screens never call them.
- Ownership (brief): the integrator owns `waved-state`, `waved-config`, `waved-settings`, `src/main.rs`, `waved-ui/src/lib.rs`, `screens_v2/mod.rs` and the kit core (tokens, route, dispatch, snapshot, router, screen; plus `obj.rs`, `components/mod.rs`, `components/status.rs`, `components/controls.rs`, which no workstream owns). Never edit a workstream's `screens_v2/<name>.rs`, `kit/components/mode_picker.rs`, `kit/components/file_row.rs`, `crates/waved-system/**`, `crates/waved-meeting/**` or `crates/waved-audio/**`; ask the owner instead.
- Commits: `type(scope): lowercase summary`, bullet-list body, ending in
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
- Verification (always `set -o pipefail`):
  - host: `cargo test -p waved-ui --lib` (also `-p waved-state`, `-p waved-provision`, `-p waved-settings` when touched)
  - `waved` binary (sqlx): `scripts/linux-test.sh check -p waved --features ui,capture-alsa` and `scripts/linux-test.sh check -p waved --features ui,simulate`
  - waved-ui lint gate until Task Z9: `cargo clippy -p waved-ui --all-targets 2>&1 | grep -E -- "--> crates/waved-ui/src/(kit|screens_v2|runtime)/"` prints nothing. From Task Z9 on: `cargo clippy -p waved-ui --all-targets -- -D warnings`.
  - goldens: `UPDATE_GOLDEN=1` only when a task says so; this plan changes no existing golden.

## Review Focus

Five inputs the happy path does not exercise that would hurt someone at the device. Each has a test in the named task.

1. **A drag that moves right on a control** (the Volume arc knob across its top, the audio-mode aim, the playback scrubber). It must change the value, not go Back. *(S2: `controls_can_keep_their_drags_from_screen_gestures`, `value_arc_keeps_its_drags`; Z3: `a_press_on_a_drag_control_never_swipes`, `drag_owner_at_finds_controls_that_keep_gestures`)*
2. **The first touch on a dimmed or dark screen lands on Record or Stop.** It must only wake the screen; LVGL must see nothing until the finger lifts. *(Z3: `a_press_that_wakes_never_reaches_lvgl`; Z4: `touch_and_keys_wake_and_the_router_hears_it`)*
3. **Event floods.** The MCU sends `LowBattery` on every battery frame below 10 %, and the upload worker reports backlog recordings the user never touched. One message per new low level; outcomes only for recordings the user just stopped or sent. *(Z2: `low_battery_is_announced_once_per_new_low`, `upload_outcomes_only_for_recordings_the_user_touched`)*
4. **The meeting changes under a sub-screen, or a route has no screen yet.** Recording starting while Volume is open, a pending decision arriving on Menu, a password set on the setup page (PIN is M3), an M3 route such as Bluetooth. The user must never be stranded or locked out. *(Z5: `meeting_state_moves_the_base_from_any_screen`, `back_returns_to_the_screen_that_opened_it`, `a_route_without_a_screen_goes_back`, `a_device_password_locks_nothing_until_the_pin_screen_lands`)*
5. **Touch coordinates and the top layer.** Native-pixel ILITEK values with the 180° flip, and a top layer that never catches a tap (BUG-3). *(Z7: `touch_axes_flip_native_pixels`, `the_top_layer_never_takes_taps`; Z6: `every_m2_route_builds_inside_the_tap_limit`)*

## Contract requests

This plan is the integrator's own; it requests nothing from other workstreams through the contract channel. Two kit conventions must exist before any workstream task runs, so they land in M2.0 (Tasks S1 and S2 below) together with the other workstreams' contract requests:

1. **Screen registration convention** (Task S1, `kit/README.md`): each `screens_v2` module exports one type implementing `kit::screen::Screen` with the name and constructor in the table "Registering a screen" (e.g. `home::HomeScreen::default()`, `file_detail::FileDetailScreen::new(id: Uuid)`, `confirm::ConfirmScreen::new(kind: ConfirmKind)`, `volume::VolumeScreen::speaker()` / `::mic()`).
2. **`Obj::keep_gestures(self) -> Self`** (Task S2, `kit/obj.rs`): clears `LV_OBJ_FLAG_GESTURE_BUBBLE`. Any control that takes drags calls it on its tappable object; `ValueArc` does it already. Why: the runtime turns swipes into Back / Menu and must not steal a drag (Review Focus 1).

### Interfaces consumed from workstream E (`crates/waved-system`)

E's plan is written in parallel, so this table is the assumed API. Task Z8 step 1 reconciles it with what E merged: if a name or signature differs, only the adapter code in `runtime/power.rs`, `runtime/run.rs` and `src/main.rs` changes; the `PowerPolicy` / `Backlight` traits, `PowerKey` and every test from Task Z4 stay.

| Piece | Assumed path and signature |
|---|---|
| Display timers (E1) | `waved_system::display::DisplayPower::new(now_ms: u64) -> DisplayPower`; `fn touch(&mut self, now_ms: u64)` (activity; wakes from Dim/Off); `fn sleep(&mut self, now_ms: u64)`; `fn tick(&mut self, now_ms: u64, settings: &waved_state::SettingsState, keep_lit: bool) -> ScreenState`; `fn backlight_pct(&self, settings: &SettingsState) -> u8` (0 when off) |
| Screen state | `waved_system::display::ScreenState { On, Dim, Off }` |
| Backlight | `waved_system::display::SysfsBacklight::device() -> SysfsBacklight`; `fn set_pct(&mut self, pct: u8) -> std::io::Result<()>` |
| Power key (E2) | `waved_system::power_key::spawn_reader(device: &str, on_press: impl FnMut(KeyPress) + Send + 'static) -> std::io::Result<std::thread::JoinHandle<()>>`; `KeyPress { Short, Long }` |
| Time zone (E3) | `waved_system::time::utc_offset_minutes(zone: &str, unix_s: i64) -> i32` |

E's other services (alert tones E5, factory reset E6, about info E7, RTC) are wired by their own contract requests, not by this plan. This plan's `feedback` keeps today's mute / volume / shutdown sounds and leaves the recording start and stop tones to E5.

### Interfaces consumed from workstreams A, B and D

The types in the S1 convention table. Task Z6 step 1 reads every `screens_v2` module; if a workstream exported a different name or constructor, only `runtime/registry.rs` changes. `screens_v2/messages.rs` (A6) is not a screen: it holds the copy screens send with `ctx.dispatch.show(..)`. Device-event messages live in `runtime/notices.rs` (this plan), so there is one mapping per source.

## TODO coverage

- **Covered:** P0.5 (navigation stack and Back via `Router`, gestures, wake guard, meeting routing from every screen); P0.1's last bullet (the UI-only `Open*` / `CloseSettings` commands are deleted); A6's device-event half (upload and integrity notices, low battery); E1/E2 UI integration (Rest/Wake, backlight, power key); BUG-3 (toast and status bar swallow taps: deleted), BUG-5 (blank PNG buttons: deleted), BUG-11, BUG-13; BUG-9's UI half (clock in the configured zone, refreshed every iteration); L3 (`control_center.rs` deleted); the milestone-2 device pass (D6).
- **Left out:** the screens themselves (A, B, D); E's services and their `main.rs` wiring other than the power key, backlight and time offset; K2 PIN screen and enforcement (masked, milestone 3); M3 routes (Mic height B5/F3, Bluetooth I, USB storage G, updates H, incoming call I) — the registry returns no screen and the runtime goes back; E4 languages (`i18n.rs` stays, unused); L1/L2 docs beyond the `lv_conf.h` table in `DISPLAY_HARDWARE.md`; `WakeScreen` / `SleepScreen` bus commands from P0.1 (the power key reaches the UI directly through `UiInputs`, so no command is needed).
- `OpenWifiProvisioning` and `CancelWifiProvisioning` are **kept**: `waved-provision` consumes them (`crates/waved-provision/src/lib.rs:51`, `:89`), so they are service commands, not navigation. Only their doc comments change.

## File Structure

| File | Responsibility | Task | Action |
|---|---|---|---|
| `crates/waved-ui/src/screens_v2/mod.rs` | Declares every M2 screen module | S1 | Create |
| `crates/waved-ui/src/screens_v2/{home,recording,decision,ambient,boot,first_run,messages}.rs` | Workstream A stubs | S1 | Create (one doc line) |
| `crates/waved-ui/src/screens_v2/{menu,audio_mode,noise,volume,system,display,datetime,wifi,wifi_setup,uploads,upload_mode,confirm,about}.rs` | Workstream B stubs | S1 | Create (one doc line) |
| `crates/waved-ui/src/screens_v2/{files,file_detail}.rs` | Workstream D stubs | S1 | Create (one doc line) |
| `crates/waved-ui/src/kit/components/{mode_picker,file_row}.rs` | B and D component stubs | S1 | Create (one doc line) |
| `crates/waved-ui/src/kit/components/mod.rs` | Declare the two components | S1 | Modify |
| `crates/waved-ui/src/lib.rs` | `pub mod screens_v2;` (S1), `pub mod runtime;` (Z2), new `UiService` (Z8), legacy mods removed (Z9) | S1, Z2, Z8, Z9 | Modify |
| `crates/waved-ui/src/kit/README.md` | Registering a screen; drags and gestures; legacy note | S1, S2, Z9 | Modify |
| `crates/waved-ui/src/kit/obj.rs` | `Obj::keep_gestures` | S2 | Modify |
| `crates/waved-ui/src/kit/components/controls.rs` | `ValueArc` keeps its drags | S2 | Modify |
| `crates/waved-ui/src/kit/dispatch.rs` | `ActiveMessage` | Z1 | Modify |
| `crates/waved-ui/src/kit/snapshot.rs` | `UiSnapshot::message`, time-zone-aware `capture` | Z1 | Modify |
| `crates/waved-ui/src/kit/components/status.rs` | `StatusArea` follows `snap.message` | Z1 | Modify |
| `crates/waved-ui/src/runtime/mod.rs` | `Runtime` core | Z2 (module root), Z5 | Create |
| `crates/waved-ui/src/runtime/notices.rs` | Device events → messages | Z2 | Create |
| `crates/waved-ui/src/runtime/feedback.rs` | Commands → sounds | Z2 | Create |
| `crates/waved-ui/src/runtime/touch.rs` | `TouchSample`, `WakeGate` | Z3 | Create |
| `crates/waved-ui/src/runtime/gesture.rs` | `GestureTracker`, `gesture_event`, `drag_owner_at` | Z3 | Create |
| `crates/waved-ui/src/runtime/power.rs` | `PowerDriver`, traits (Z4); E adapter (Z8) | Z4, Z8 | Create |
| `crates/waved-ui/src/runtime/registry.rs` | `Route` → screen | Z6 | Create |
| `crates/waved-ui/src/input.rs` | Touch poll returns `TouchSample`; flip helpers | Z7 | Modify |
| `crates/waved-ui/src/runtime/device.rs` | LVGL display + pointer, flush/read callbacks, top layer | Z7 | Create |
| `crates/waved-ui/src/runtime/run.rs` | The UI thread loop | Z8 | Create |
| `crates/waved-ui/Cargo.toml` | `waved-system` dependency | Z8 | Modify |
| `crates/waved/Cargo.toml`, `crates/waved/src/main.rs` | `waved-system` dependency, power key → `UiInputs` | Z8 | Modify |
| `crates/waved-ui/src/{nav,theme,toast,status_bar,upload_notice,integrity_notice}.rs`, `src/screens/`, `assets/` | Legacy UI | Z8, Z9 | Delete |
| `crates/waved-ui/src/display.rs` | Clippy-clean `init` | Z9 | Modify |
| `crates/waved-ui/lv_conf.h` | No Montserrat, no PNG/FS, default font `font_body_15` | Z9 | Modify |
| `crates/waved-state/src/lib.rs` | Delete 8 UI-only commands, re-document 2 provisioning commands | Z9 | Modify |
| `docs/DISPLAY_HARDWARE.md` | LVGL config table; touch gestures (after the device pass) | Z9, Z10 | Modify |
| `../TODO.md` | Tick P0.5, L3; mark BUG-3/5/11/13 fixed | Z9 | Modify |

## Execution

- **M2.0** (Tasks S1, S2) runs on `feat/display-redesign` right after the M2.0 contract task and before any workstream branches off.
- **Runtime switch** (Tasks Z1–Z10) runs in one worktree branch `m2-runtime-switch` from `feat/display-redesign` **after A, B, D and E are merged**, in order. Z2–Z5 and Z7 touch only new runtime files and integrator files, so they may start earlier on that branch if the integrator has time; Z6 and Z8–Z10 need the merged screens and `waved-system`.
- Every task ends green and lint-clean, then commits. The legacy UI keeps working until Z8 switches `UiService` over.

---

# M2.0 scaffolding

### Task S1: Pre-create the `screens_v2` modules and the component stubs

**Files:**
- Create: `crates/waved-ui/src/screens_v2/mod.rs` and the 22 module files listed in it
- Create: `crates/waved-ui/src/kit/components/mode_picker.rs`, `crates/waved-ui/src/kit/components/file_row.rs`
- Modify: `crates/waved-ui/src/lib.rs`, `crates/waved-ui/src/kit/components/mod.rs`, `crates/waved-ui/src/kit/README.md`

**Interfaces:**
- Consumes: nothing.
- Produces: modules `crate::screens_v2::{ambient, boot, decision, first_run, home, messages, recording, about, audio_mode, confirm, datetime, display, menu, noise, system, upload_mode, uploads, volume, wifi, wifi_setup, file_detail, files}` and `crate::kit::components::{mode_picker, file_row}`, each empty except a doc line; the registration convention in `kit/README.md`.

- [ ] **Step 1: Declare the modules (the failing step).** Write `crates/waved-ui/src/screens_v2/mod.rs`:

```rust
//! Revision-2 screens (milestone 2). Each module implements
//! `kit::screen::Screen`; `runtime::registry` maps routes to them at the
//! runtime switch. The type and constructor each module exports are in
//! `kit/README.md`, "Registering a screen". Only the owning workstream edits
//! a module; this file belongs to the integrator.

// Workstream A: main screens.
pub mod ambient;
pub mod boot;
pub mod decision;
pub mod first_run;
pub mod home;
pub mod messages;
pub mod recording;

// Workstream B: settings screens.
pub mod about;
pub mod audio_mode;
pub mod confirm;
pub mod datetime;
pub mod display;
pub mod menu;
pub mod noise;
pub mod system;
pub mod upload_mode;
pub mod uploads;
pub mod volume;
pub mod wifi;
pub mod wifi_setup;

// Workstream D: Files.
pub mod file_detail;
pub mod files;
```

In `crates/waved-ui/src/lib.rs`, add after `pub mod screens;` (line 8):

```rust
pub mod screens_v2;
```

In `crates/waved-ui/src/kit/components/mod.rs`, replace the module list with:

```rust
pub mod buttons;
pub mod controls;
pub mod dial;
/// Files list rows (workstream D, D1).
pub mod file_row;
pub mod list;
/// Radial audio-mode picker (workstream B, B2).
pub mod mode_picker;
pub mod row;
pub mod status;
```

(keep the `#[cfg(test)] mod compositions;` line below it).

- [ ] **Step 2: Run and see it fail**

Run: `cargo check -p waved-ui 2>&1 | grep -m3 "file not found for module"`
Expected: `error[E0583]: file not found for module \`ambient\`` (and the other modules).

- [ ] **Step 3: Create the stubs.** Each file holds exactly one doc line naming its owner:

```bash
cd crates/waved-ui/src
stub() { printf '//! %s Owned by workstream %s; see kit/README.md, "Registering a screen".\n' "$2" "$3" > "$1"; }
stub screens_v2/ambient.rs     "Resting screen (A4)."             A
stub screens_v2/boot.rs        "Start-up screen (A5)."            A
stub screens_v2/decision.rs    "After stop: Send this recording? (A3)." A
stub screens_v2/first_run.rs   "First start (A5)."                A
stub screens_v2/home.rs        "Home (A1)."                       A
stub screens_v2/messages.rs    "Message copy screens send (A6); not a screen." A
stub screens_v2/recording.rs   "Recording (A2)."                  A
stub screens_v2/about.rs       "About (E7 data, B6 screen)."      B
stub screens_v2/audio_mode.rs  "Audio mode (B2)."                 B
stub screens_v2/confirm.rs     "Confirmation dialog (B6)."        B
stub screens_v2/datetime.rs    "Date and time (B6)."              B
stub screens_v2/display.rs     "Display (B6)."                    B
stub screens_v2/menu.rs        "Menu (B1)."                       B
stub screens_v2/noise.rs       "Noise reduction (B3)."            B
stub screens_v2/system.rs      "System (B6)."                     B
stub screens_v2/upload_mode.rs "After a recording (B8)."          B
stub screens_v2/uploads.rs     "Uploads (B8)."                    B
stub screens_v2/volume.rs      "Volume and Mic level (B4)."       B
stub screens_v2/wifi.rs        "Wi-Fi (B7)."                      B
stub screens_v2/wifi_setup.rs  "Wi-Fi setup (B7)."                B
stub screens_v2/file_detail.rs "Recording detail (D2)."           D
stub screens_v2/files.rs       "Files (D1)."                      D
printf '//! Radial audio-mode picker (B2). Owned by workstream B.\n' > kit/components/mode_picker.rs
printf '//! Files list row (D1). Owned by workstream D.\n' > kit/components/file_row.rs
```

- [ ] **Step 4: Publish the registration convention.** In `crates/waved-ui/src/kit/README.md`, insert after the "## A screen" section (before "## Components"):

```markdown
## Registering a screen

The runtime builds screens from `Route`s through `runtime::registry` at the end of
milestone 2. Each module in `src/screens_v2/` exports exactly this type and
constructor (the struct may hold anything; `build` gets the rest from
`ScreenCtx` and the snapshot):

| Route | Module | Type and constructor |
|---|---|---|
| `Boot` | `boot` | `BootScreen::default()` |
| `FirstRun` | `first_run` | `FirstRunScreen::default()` |
| `Home` | `home` | `HomeScreen::default()` |
| `Recording` | `recording` | `RecordingScreen::default()` |
| `Decision` | `decision` | `DecisionScreen::default()` |
| `Resting` | `ambient` | `AmbientScreen::default()` |
| `Menu` | `menu` | `MenuScreen::default()` |
| `AudioMode` | `audio_mode` | `AudioModeScreen::default()` |
| `NoiseReduction` | `noise` | `NoiseScreen::default()` |
| `Volume` / `MicLevel` | `volume` | `VolumeScreen::speaker()` / `VolumeScreen::mic()` |
| `Files` | `files` | `FilesScreen::default()` |
| `FileDetail(id)` | `file_detail` | `FileDetailScreen::new(id: Uuid)` |
| `Wifi` | `wifi` | `WifiScreen::default()` |
| `WifiSetup` | `wifi_setup` | `WifiSetupScreen::default()` |
| `Uploads` | `uploads` | `UploadsScreen::default()` |
| `UploadMode` | `upload_mode` | `UploadModeScreen::default()` |
| `System` | `system` | `SystemScreen::default()` |
| `Display` | `display` | `DisplayScreen::default()` |
| `DateTime` | `datetime` | `DateTimeScreen::default()` |
| `About` | `about` | `AboutScreen::default()` |
| `Confirm(kind)` | `confirm` | `ConfirmScreen::new(kind: ConfirmKind)` |

`messages.rs` is not a screen; it holds the message copy screens send. Mic height,
Bluetooth, PIN, USB storage, Update and Incoming call are milestone 3 and have no
module yet. A screen must cope with any snapshot (for example `FileDetail` for an
id no longer in `snap.files`, or `Decision` while `pending_decision` is `None` for a
frame) without panicking. Every screen with a status area calls
`status.update(snap)` from `update`: that is how runtime messages appear.
```

- [ ] **Step 5: Run and see it pass**

Run: `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | tail -3`
Expected: `test result: ok.` with the same test count as before.

Run: `cargo clippy -p waved-ui --all-targets 2>&1 | grep -E -- "--> crates/waved-ui/src/(kit|screens_v2)/"`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-ui/src/screens_v2 crates/waved-ui/src/kit/components crates/waved-ui/src/lib.rs crates/waved-ui/src/kit/README.md
git commit -m "feat(ui): pre-create the milestone 2 screen modules

- screens_v2/mod.rs declares every A, B and D screen module
- one-line stubs so each workstream edits only its own files
- kit components mode_picker (B) and file_row (D) declared
- kit README: the type and constructor the registry expects per route

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task S2: Controls that keep their drags from screen gestures

**Files:**
- Modify: `crates/waved-ui/src/kit/obj.rs`, `crates/waved-ui/src/kit/components/controls.rs`, `crates/waved-ui/src/kit/README.md`

**Interfaces:**
- Consumes: `Obj::flag`, `Obj::has_flag`.
- Produces: `pub fn keep_gestures(self) -> Self` on `Obj`; `ValueArc::new` returns an arc whose `obj()` has `LV_OBJ_FLAG_GESTURE_BUBBLE` cleared.

- [ ] **Step 1: Write the failing tests.** In `obj.rs`'s `mod tests`:

```rust
    #[test]
    fn controls_can_keep_their_drags_from_screen_gestures() {
        lvgl(|lv| {
            let root = lv.screen();
            let plain = Obj::plain(root);
            assert!(
                plain.has_flag(lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE),
                "children pass gestures up to the screen by default"
            );
            let knob = Obj::plain(root).clickable(true).keep_gestures();
            assert!(!knob.has_flag(lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE));
        });
    }
```

In `controls.rs`'s `mod tests`:

```rust
    #[test]
    fn value_arc_keeps_its_drags() {
        lvgl(|lv| {
            let root = lv.screen();
            let arc = ValueArc::new(root, 200, 132, 88, |_| {});
            assert!(
                !arc.obj().has_flag(lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE),
                "dragging the knob right must not read as swipe-right-for-Back"
            );
        });
    }
```

- [ ] **Step 2: Run and see it fail**

Run: `cargo test -p waved-ui --lib keep 2>&1 | tail -5`
Expected: compile error `no method named \`keep_gestures\` found for struct \`Obj\``.

- [ ] **Step 3: Implement.** In `obj.rs`, after `pub fn ext_click`:

```rust
    /// This object handles its own drags (a slider knob, an aim handle, a
    /// scrubber): a press that starts on it never becomes a screen swipe
    /// (swipe right for Back, swipe down for Menu). Clears
    /// `LV_OBJ_FLAG_GESTURE_BUBBLE`, which the runtime's gesture check reads.
    pub fn keep_gestures(self) -> Self {
        self.flag(lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE, false)
    }
```

In `controls.rs`, `ValueArc::new`, change the arc builder to:

```rust
        let arc = Obj::arc(parent)
            .pos(cx - size / 2, cy - size / 2)
            .size(size, size)
            .clickable(true)
            .keep_gestures();
```

In `kit/README.md`, add after the "Registering a screen" section:

```markdown
## Drags and the screen gestures

The runtime turns a swipe right into Back (on sub-screens) and a swipe down from
the top edge (y ≤ 34) into Menu (Home, Recording). A control that takes drags —
the value arc, the audio-mode aim, the playback scrubber — calls
`.keep_gestures()` on its tappable object, so a drag that starts on it is never
read as a swipe. `ValueArc` already does. Screens never handle these two swipes
themselves.
```

- [ ] **Step 4: Run and see it pass**

Run: `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | tail -3`
Expected: `test result: ok.`; no golden changes (`git status --short crates/waved-ui/golden` prints nothing).

Run: `cargo clippy -p waved-ui --all-targets 2>&1 | grep -E -- "--> crates/waved-ui/src/(kit|screens_v2)/"`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-ui/src/kit
git commit -m "feat(ui): let controls keep their drags from screen gestures

- Obj::keep_gestures clears LV_OBJ_FLAG_GESTURE_BUBBLE
- ValueArc keeps its drags, so moving the knob never goes Back
- kit README: which swipes the runtime owns and how controls opt out

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

# Runtime switch (end of milestone 2)

### Task Z1: Messages travel in the snapshot; the clock follows the time zone

**Files:**
- Modify: `crates/waved-ui/src/kit/dispatch.rs`, `crates/waved-ui/src/kit/snapshot.rs`, `crates/waved-ui/src/kit/components/status.rs`

**Interfaces:**
- Consumes: `Message`, `Severity::duration_ms`, `StatusArea::show`, `format_clock`.
- Produces:
  - `pub struct ActiveMessage { pub id: u64, pub message: Message, pub since_ms: u64 }` with `pub fn until_ms(&self) -> u64` (in `kit::dispatch`)
  - `UiSnapshot::message: Option<ActiveMessage>`
  - `pub fn UiSnapshot::capture(bus: &StateBus, now_unix_s: i64, now_ms: u64, utc_offset: fn(&str, i64) -> i32) -> UiSnapshot`
  - `StatusArea::update` shows `snap.message` once per `id`, timed from `since_ms`.

- [ ] **Step 1: Write the failing tests.**

In `dispatch.rs` tests:

```rust
    #[test]
    fn active_messages_end_after_their_severity() {
        let sent = ActiveMessage {
            id: 1,
            message: Message::new(Severity::Success, "Recording sent"),
            since_ms: 1_000,
        };
        assert_eq!(sent.until_ms(), 3_000);
        let failed = ActiveMessage {
            id: 2,
            message: Message::new(Severity::Error, "Upload failed, see Uploads"),
            since_ms: 1_000,
        };
        assert_eq!(failed.until_ms(), 5_000);
    }
```

In `snapshot.rs` tests, replace `capture_copies_the_bus_and_formats_the_clock` and add a zone test:

```rust
    #[test]
    fn capture_copies_the_bus_and_formats_the_clock() {
        let bus = StateBus::new();
        let _keep = bus.settings.subscribe();
        bus.settings.send_modify(|s| s.clock_24h = false);
        let snap = UiSnapshot::capture(&bus, 1_759_588_320, 42, |_, _| 0);
        assert!(!snap.settings.clock_24h);
        assert_eq!(snap.clock, "2:32 pm");
        assert_eq!(snap.now_ms, 42);
        assert!(snap.message.is_none(), "the runtime adds the message");
    }

    #[test]
    fn capture_shows_the_clock_in_the_configured_zone() {
        fn brussels(zone: &str, _unix_s: i64) -> i32 {
            if zone == "Europe/Brussels" {
                120
            } else {
                0
            }
        }
        let bus = StateBus::new();
        let _keep = bus.settings.subscribe();
        bus.settings
            .send_modify(|s| s.time_zone = "Europe/Brussels".into());
        let snap = UiSnapshot::capture(&bus, 1_759_588_320, 0, brussels);
        assert_eq!(snap.clock, "16:32");
    }
```

In `status.rs` tests (add `use crate::kit::dispatch::ActiveMessage;` to the test imports):

```rust
    #[test]
    fn runtime_messages_arrive_through_the_snapshot() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut status = StatusArea::plain(root);
            let mut now = snap();
            now.now_ms = 1_500;
            now.message = Some(ActiveMessage {
                id: 1,
                message: Message::new(Severity::Success, "Recording sent"),
                since_ms: 1_000,
            });
            status.update(&now);
            assert!(!status.message().is_hidden() && status.cluster().is_hidden());
            now.now_ms = 3_000;
            status.update(&now);
            assert!(
                status.message().is_hidden(),
                "gone 2 s after it was posted, not after it arrived"
            );
            now.now_ms = 3_100;
            now.message = Some(ActiveMessage {
                id: 2,
                message: Message::new(Severity::Success, "Recording sent"),
                since_ms: 3_100,
            });
            status.update(&now);
            assert!(
                !status.message().is_hidden(),
                "the same text again is a new message"
            );
        });
    }

    #[test]
    fn a_screen_built_mid_message_shows_only_the_rest() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut now = snap();
            now.now_ms = 4_500;
            now.message = Some(ActiveMessage {
                id: 7,
                message: Message::new(Severity::Error, "Upload failed, see Uploads"),
                since_ms: 1_000,
            });
            let mut status = StatusArea::plain(root);
            status.update(&now);
            assert!(!status.message().is_hidden());
            now.now_ms = 5_000;
            status.update(&now);
            assert!(status.message().is_hidden());
        });
    }
```

- [ ] **Step 2: Run and see it fail**

Run: `cargo test -p waved-ui --lib 2>&1 | grep -m3 "error\["`
Expected: `cannot find struct, variant or union type \`ActiveMessage\`` and `this function takes 3 arguments but 4 arguments were supplied`.

- [ ] **Step 3: Implement.**

`dispatch.rs`, after `impl Message`:

```rust
/// The message the runtime shows in the status slot, and since when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveMessage {
    /// New for every message, so the same text twice shows twice.
    pub id: u64,
    pub message: Message,
    /// Monotonic milliseconds (`UiSnapshot::now_ms`) when it was posted.
    pub since_ms: u64,
}

impl ActiveMessage {
    /// When the message leaves the status slot.
    pub fn until_ms(&self) -> u64 {
        self.since_ms + self.message.severity.duration_ms()
    }
}
```

`snapshot.rs`: import `use crate::kit::dispatch::ActiveMessage;`, add the field after `clock`:

```rust
    /// The status-slot message the runtime is showing; `StatusArea::update`
    /// displays it. `None` in tests unless a test sets it.
    pub message: Option<ActiveMessage>,
```

and replace `capture`:

```rust
    /// One consistent copy of the bus. `utc_offset(zone, unix_s)` is the
    /// zone's offset in minutes at that moment (on the device:
    /// `waved_system::time::utc_offset_minutes`).
    pub fn capture(
        bus: &StateBus,
        now_unix_s: i64,
        now_ms: u64,
        utc_offset: fn(&str, i64) -> i32,
    ) -> Self {
        let settings = bus.settings.borrow().clone();
        let offset = utc_offset(&settings.time_zone, now_unix_s);
        let clock = format_clock(now_unix_s, offset, settings.clock_24h);
        Self {
            audio: bus.audio.borrow().clone(),
            meeting: bus.meeting.borrow().clone(),
            battery: bus.battery.borrow().clone(),
            bt: bus.bt.borrow().clone(),
            wifi: bus.wifi.borrow().clone(),
            provisioning: bus.provisioning.borrow().clone(),
            mcu: bus.mcu.borrow().clone(),
            settings,
            files: bus.files.borrow().clone(),
            playback: bus.playback.borrow().clone(),
            now_unix_s,
            now_ms,
            clock,
            message: None,
        }
    }
```

`status.rs`: import `ActiveMessage` is not needed in the module body; add the field `message_id: Option<u64>,` to `StatusArea` (after `message_until_ms`), initialise it `message_id: None,` in `build`, and put this at the top of `update` (before the expiry check):

```rust
        // Runtime messages arrive in the snapshot; show each one once, timed
        // from when it was posted, so a screen built mid-message shows the rest.
        if let Some(active) = &snap.message {
            if self.message_id != Some(active.id) {
                self.message_id = Some(active.id);
                self.show(&active.message, active.since_ms);
            }
        }
```

- [ ] **Step 4: Run and see it pass**

Run: `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | tail -3`
Expected: `test result: ok.`; `git status --short crates/waved-ui/golden` prints nothing.

Run: `cargo clippy -p waved-ui --all-targets 2>&1 | grep -E -- "--> crates/waved-ui/src/(kit|screens_v2|runtime)/"`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-ui/src/kit
git commit -m "feat(ui): carry the status message and time zone in the snapshot

- ActiveMessage: one message with an id and its post time
- UiSnapshot::message; StatusArea shows it once per id, timed from posting
- capture takes the zone offset function, so the clock follows time_zone

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task Z2: Device events become messages; commands keep their sounds

**Files:**
- Create: `crates/waved-ui/src/runtime/mod.rs` (module root only), `crates/waved-ui/src/runtime/notices.rs`, `crates/waved-ui/src/runtime/feedback.rs`
- Modify: `crates/waved-ui/src/lib.rs` (`pub mod runtime;`)

**Interfaces:**
- Consumes: `DeviceEvent`, `Command`, `UiSnapshot`, `Message`, `Severity`, `audio_fx::SFX_*`.
- Produces:
  - `pub struct Notices` with `pub fn on_command(&mut self, command: &Command)`, `pub fn observe(&mut self, snap: &UiSnapshot)`, `pub fn handle(&mut self, event: &DeviceEvent, snap: &UiSnapshot) -> Option<Message>`
  - copy constants `SENT`, `UPLOAD_WAITING_NO_WIFI`, `UPLOAD_RETRYING`, `UPLOAD_FAILED`, `RECORDING_FAILED`, `AUDIO_RESTARTING`, `CANT_RECORD`, `pub fn low_battery(level: u8) -> String`
  - `pub fn feedback::sound_for(command: &Command, snap: &UiSnapshot) -> Option<&'static str>`

- [ ] **Step 1: Module root.** Create `crates/waved-ui/src/runtime/mod.rs`:

```rust
//! The revision-2 UI runtime: one router, the screens_v2 registry and the
//! dispatcher, fed by one snapshot per loop iteration.

pub mod feedback;
pub mod notices;
```

and add `pub mod runtime;` to `lib.rs` after `pub mod nav;`.

- [ ] **Step 2: Write the failing tests.** `notices.rs` starts with the tests module (implementation follows in Step 4):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fonts::Font;
    use crate::kit::obj::text_width;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::MESSAGE_MAX_W;
    use waved_state::RecordingIntegrityFault;

    fn online() -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.wifi.connected = true;
        snap.battery.level = 80;
        snap
    }

    fn text(message: Option<Message>) -> Option<(Severity, String)> {
        message.map(|m| (m.severity, m.text))
    }

    #[test]
    fn upload_outcomes_only_for_recordings_the_user_touched() {
        let mut notices = Notices::default();
        let snap = online();
        let stopped = Uuid::new_v4();
        let backlog = Uuid::new_v4();
        assert_eq!(notices.handle(&DeviceEvent::RecordingStopped(stopped), &snap), None);
        assert_eq!(
            notices.handle(&DeviceEvent::RecordingUploadSucceeded(backlog), &snap),
            None,
            "older backlog stays quiet"
        );
        assert_eq!(
            text(notices.handle(&DeviceEvent::RecordingUploadSucceeded(stopped), &snap)),
            Some((Severity::Success, SENT.into()))
        );
        assert_eq!(
            notices.handle(&DeviceEvent::RecordingUploadSucceeded(stopped), &snap),
            None,
            "announced once"
        );
        let resent = Uuid::new_v4();
        notices.on_command(&Command::SendRecording(resent));
        assert_eq!(
            text(notices.handle(&DeviceEvent::RecordingUploadNeedsAttention(resent), &snap)),
            Some((Severity::Error, UPLOAD_FAILED.into()))
        );
        let retried = Uuid::new_v4();
        notices.on_command(&Command::RetryUpload(retried));
        assert!(notices
            .handle(&DeviceEvent::RecordingUploadSucceeded(retried), &snap)
            .is_some());
    }

    #[test]
    fn a_deferred_upload_says_why_it_waits() {
        let mut notices = Notices::default();
        let id = Uuid::new_v4();
        notices.on_command(&Command::SendRecording(id));
        let mut snap = online();
        assert_eq!(
            text(notices.handle(&DeviceEvent::RecordingUploadDeferred(id), &snap)),
            Some((Severity::Warning, UPLOAD_RETRYING.into()))
        );
        snap.wifi.connected = false;
        assert_eq!(
            text(notices.handle(&DeviceEvent::RecordingUploadDeferred(id), &snap)),
            Some((Severity::Warning, UPLOAD_WAITING_NO_WIFI.into()))
        );
    }

    #[test]
    fn integrity_faults_map_to_plain_messages() {
        let mut notices = Notices::default();
        let snap = online();
        let during = DeviceEvent::RecordingIntegrityFailed {
            meeting_id: Some(Uuid::new_v4()),
            fault: RecordingIntegrityFault::WavWriteFailed,
        };
        assert_eq!(
            text(notices.handle(&during, &snap)),
            Some((Severity::Error, RECORDING_FAILED.into()))
        );
        let restart = DeviceEvent::RecordingIntegrityFailed {
            meeting_id: None,
            fault: RecordingIntegrityFault::CaptureReadFailed,
        };
        assert_eq!(
            text(notices.handle(&restart, &snap)),
            Some((Severity::Warning, AUDIO_RESTARTING.into()))
        );
        let storage = DeviceEvent::RecordingIntegrityFailed {
            meeting_id: None,
            fault: RecordingIntegrityFault::WavWriteFailed,
        };
        assert_eq!(
            text(notices.handle(&storage, &snap)),
            Some((Severity::Error, CANT_RECORD.into()))
        );
        let harmless = DeviceEvent::RecordingIntegrityFailed {
            meeting_id: None,
            fault: RecordingIntegrityFault::CaptureXrun,
        };
        assert_eq!(notices.handle(&harmless, &snap), None);
    }

    #[test]
    fn low_battery_is_announced_once_per_new_low() {
        let mut notices = Notices::default();
        let mut snap = online();
        snap.battery.level = 9;
        notices.observe(&snap);
        assert_eq!(
            text(notices.handle(&DeviceEvent::LowBattery(9), &snap)),
            Some((Severity::Warning, "Battery 9%, plug in Wave".into()))
        );
        assert_eq!(notices.handle(&DeviceEvent::LowBattery(9), &snap), None, "every frame repeats it");
        assert_eq!(notices.handle(&DeviceEvent::LowBattery(9), &snap), None);
        assert!(notices.handle(&DeviceEvent::LowBattery(8), &snap).is_some(), "a new low");
        assert_eq!(notices.handle(&DeviceEvent::LowBattery(9), &snap), None, "jitter back up is quiet");
        snap.battery.is_charging = true;
        notices.observe(&snap);
        assert_eq!(notices.handle(&DeviceEvent::LowBattery(8), &snap), None, "charging");
        snap.battery.is_charging = false;
        snap.battery.level = 9;
        notices.observe(&snap);
        assert!(
            notices.handle(&DeviceEvent::LowBattery(9), &snap).is_some(),
            "after a charge the next low is news again"
        );
    }

    #[test]
    fn tracking_is_bounded_and_ignores_duplicates() {
        let mut notices = Notices::default();
        let snap = online();
        let first = Uuid::new_v4();
        notices.on_command(&Command::SendRecording(first));
        notices.on_command(&Command::SendRecording(first));
        assert_eq!(notices.tracked.len(), 1);
        for _ in 1..MAX_TRACKED {
            notices.on_command(&Command::SendRecording(Uuid::new_v4()));
        }
        let newest = Uuid::new_v4();
        notices.on_command(&Command::SendRecording(newest));
        assert_eq!(notices.handle(&DeviceEvent::RecordingUploadSucceeded(first), &snap), None);
        assert!(notices
            .handle(&DeviceEvent::RecordingUploadSucceeded(newest), &snap)
            .is_some());
    }

    #[test]
    fn every_notice_fits_the_message_slot() {
        lvgl(|_lv| {
            let battery = low_battery(100);
            for copy in [
                SENT,
                UPLOAD_WAITING_NO_WIFI,
                UPLOAD_RETRYING,
                UPLOAD_FAILED,
                RECORDING_FAILED,
                AUDIO_RESTARTING,
                CANT_RECORD,
                battery.as_str(),
            ] {
                let width = text_width(copy, Font::Small) + 1;
                assert!(width <= MESSAGE_MAX_W - 44, "{copy:?} is {width} px");
                assert!(copy.is_ascii() && !copy.contains("..."), "{copy:?}");
            }
        });
    }
}
```

`feedback.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_plays_the_sound_for_the_state_it_toggles_to() {
        let mut snap = UiSnapshot::default();
        assert_eq!(sound_for(&Command::ToggleMicMute, &snap), Some(SFX_MIC_MUTE));
        snap.audio.mic_muted = true;
        assert_eq!(sound_for(&Command::ToggleMicMute, &snap), Some(SFX_MIC_UNMUTE));
    }

    #[test]
    fn volume_and_shutdown_have_sounds_recording_does_not() {
        let snap = UiSnapshot::default();
        assert_eq!(sound_for(&Command::SetSpeakerVolume(40), &snap), Some(SFX_SPK_VOL));
        assert_eq!(sound_for(&Command::ToggleSpeakerMute, &snap), Some(SFX_SPK_VOL));
        assert_eq!(sound_for(&Command::Shutdown, &snap), Some(SFX_SHUTDOWN));
        assert_eq!(
            sound_for(&Command::StartRecording, &snap),
            None,
            "recording tones belong to E5"
        );
        assert_eq!(sound_for(&Command::AddBookmark, &snap), None);
    }

    #[test]
    fn the_alert_tones_switch_silences_everything() {
        let mut snap = UiSnapshot::default();
        snap.settings.tones_enabled = false;
        assert_eq!(sound_for(&Command::ToggleMicMute, &snap), None);
        assert_eq!(sound_for(&Command::Shutdown, &snap), None);
    }
}
```

- [ ] **Step 3: Run and see it fail**

Run: `cargo test -p waved-ui --lib runtime:: 2>&1 | grep -m3 "error"`
Expected: unresolved names (`Notices`, `sound_for`, `SENT` …).

- [ ] **Step 4: Implement.** Top of `notices.rs` (above the tests):

```rust
//! Device events the user should hear about, as status-slot messages.
//! Replaces the legacy upload and integrity toasts (`upload_notice.rs`,
//! `integrity_notice.rs`). Screens send their own messages through
//! `ctx.dispatch.show(..)`; this is the one place device events become text.

use crate::kit::dispatch::{Message, Severity};
use crate::kit::snapshot::UiSnapshot;
use std::collections::VecDeque;
use uuid::Uuid;
use waved_state::{Command, DeviceEvent};

/// Recordings whose upload outcome is announced; older backlog stays quiet.
const MAX_TRACKED: usize = 16;
/// The MCU reports `LowBattery` below this level (`waved-mcu`).
const LOW_BATTERY_LEVEL: u8 = 10;

pub const SENT: &str = "Recording sent";
pub const UPLOAD_WAITING_NO_WIFI: &str = "No Wi-Fi, upload waiting";
pub const UPLOAD_RETRYING: &str = "Upload failed, trying again";
pub const UPLOAD_FAILED: &str = "Upload failed, see Uploads";
pub const RECORDING_FAILED: &str = "Recording failed, see Files";
pub const AUDIO_RESTARTING: &str = "Audio restarting";
pub const CANT_RECORD: &str = "Can't record, check storage";

pub fn low_battery(level: u8) -> String {
    format!("Battery {level}%, plug in Wave")
}

#[derive(Debug, Default)]
pub struct Notices {
    tracked: VecDeque<Uuid>,
    /// The lowest level announced since the battery was last charging or
    /// above the low threshold.
    battery_announced: Option<u8>,
}

impl Notices {
    /// Recordings the user sends from a screen get their outcome announced.
    pub fn on_command(&mut self, command: &Command) {
        if let Command::SendRecording(id) | Command::RetryUpload(id) = command {
            self.track(*id);
        }
    }

    /// Called every iteration: a charge or a recovered level re-arms the
    /// low-battery message.
    pub fn observe(&mut self, snap: &UiSnapshot) {
        if snap.battery.is_charging || snap.battery.level >= LOW_BATTERY_LEVEL {
            self.battery_announced = None;
        }
    }

    pub fn handle(&mut self, event: &DeviceEvent, snap: &UiSnapshot) -> Option<Message> {
        match event {
            DeviceEvent::RecordingStopped(id) => {
                self.track(*id);
                None
            }
            DeviceEvent::RecordingUploadSucceeded(id) => {
                let at = self.position(id)?;
                self.tracked.remove(at);
                Some(Message::new(Severity::Success, SENT))
            }
            DeviceEvent::RecordingUploadDeferred(id) => {
                self.position(id)?;
                let text = if snap.wifi.connected {
                    UPLOAD_RETRYING
                } else {
                    UPLOAD_WAITING_NO_WIFI
                };
                Some(Message::new(Severity::Warning, text))
            }
            DeviceEvent::RecordingUploadNeedsAttention(id) => {
                self.position(id)?;
                Some(Message::new(Severity::Error, UPLOAD_FAILED))
            }
            DeviceEvent::RecordingIntegrityFailed { meeting_id, fault } => {
                if meeting_id.is_some() {
                    Some(Message::new(Severity::Error, RECORDING_FAILED))
                } else if fault.requires_restart() {
                    Some(Message::new(Severity::Warning, AUDIO_RESTARTING))
                } else if fault.blocks_recording() {
                    Some(Message::new(Severity::Error, CANT_RECORD))
                } else {
                    None
                }
            }
            DeviceEvent::LowBattery(level) => {
                if snap.battery.is_charging
                    || self.battery_announced.is_some_and(|shown| *level >= shown)
                {
                    return None;
                }
                self.battery_announced = Some(*level);
                Some(Message::new(Severity::Warning, low_battery(*level)))
            }
            _ => None,
        }
    }

    fn track(&mut self, id: Uuid) {
        if let Some(at) = self.position(&id) {
            self.tracked.remove(at);
        }
        if self.tracked.len() == MAX_TRACKED {
            self.tracked.pop_front();
        }
        self.tracked.push_back(id);
    }

    fn position(&self, id: &Uuid) -> Option<usize> {
        self.tracked.iter().position(|tracked| tracked == id)
    }
}
```

Top of `feedback.rs`:

```rust
//! Sounds the device plays when a command goes out (the stock pspapp WAVs
//! that its business daemon used to play). Recording start and stop tones
//! belong to workstream E (E5); the Alert tones switch silences all of them.

use crate::audio_fx::{SFX_MIC_MUTE, SFX_MIC_UNMUTE, SFX_SHUTDOWN, SFX_SPK_VOL};
use crate::kit::snapshot::UiSnapshot;
use waved_state::Command;

pub fn sound_for(command: &Command, snap: &UiSnapshot) -> Option<&'static str> {
    if !snap.settings.tones_enabled {
        return None;
    }
    match command {
        // The snapshot still holds the state before the toggle.
        Command::ToggleMicMute => Some(if snap.audio.mic_muted {
            SFX_MIC_UNMUTE
        } else {
            SFX_MIC_MUTE
        }),
        Command::SetSpeakerVolume(_) | Command::ToggleSpeakerMute => Some(SFX_SPK_VOL),
        Command::Shutdown => Some(SFX_SHUTDOWN),
        _ => None,
    }
}
```

- [ ] **Step 5: Run and see it pass**

Run: `set -o pipefail; cargo test -p waved-ui --lib runtime:: 2>&1 | tail -3`
Expected: `test result: ok.` (9 tests). If `every_notice_fits_the_message_slot` reports a width, shorten that constant's copy (keep the verb and the next step) and rerun; do not widen the slot.

Run: `cargo clippy -p waved-ui --all-targets 2>&1 | grep -E -- "--> crates/waved-ui/src/(kit|screens_v2|runtime)/"`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-ui/src/runtime crates/waved-ui/src/lib.rs
git commit -m "feat(ui): map device events to status messages and commands to sounds

- runtime::notices: upload, integrity and low-battery events as Messages
- upload outcomes only for recordings just stopped, sent or retried
- one low-battery message per new low until the next charge
- runtime::feedback: mute, volume and shutdown sounds, silenced by Alert tones

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task Z3: Gestures and the wake gate

**Files:**
- Create: `crates/waved-ui/src/runtime/touch.rs`, `crates/waved-ui/src/runtime/gesture.rs`
- Modify: `crates/waved-ui/src/runtime/mod.rs` (`pub mod gesture; pub mod touch;`)

**Interfaces:**
- Consumes: `Obj`, `Route`, `UiEvent`, `Obj::keep_gestures` (S2).
- Produces:
  - `pub struct TouchSample { pub x: i32, pub y: i32, pub pressed: bool }` (`Clone, Copy, Debug, Default, PartialEq, Eq`)
  - `pub struct WakeGate` with `pub fn filter(&mut self, raw: TouchSample, asleep: bool) -> TouchSample`
  - `pub enum Gesture { SwipeDownFromTop, SwipeRight }`; consts `TOP_EDGE_Y = 34`, `SWIPE_MIN_PX = 60`, `SWIPE_MAX_MS = 700`
  - `pub struct GestureTracker` with `pub fn sample(&mut self, s: TouchSample, now_ms: u64, blocked_at: impl FnOnce(i32, i32) -> bool) -> Option<Gesture>`
  - `pub fn gesture_event(gesture: Gesture, current: &Route, depth: usize) -> Option<UiEvent>`
  - `pub fn drag_owner_at(root: Obj, x: i32, y: i32) -> bool`

- [ ] **Step 1: Write the failing tests.**

`touch.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: i32, y: i32, pressed: bool) -> TouchSample {
        TouchSample { x, y, pressed }
    }

    #[test]
    fn a_press_that_wakes_never_reaches_lvgl() {
        let mut gate = WakeGate::default();
        // The press lands on Record while the screen rests.
        assert_eq!(gate.filter(at(200, 241, true), true), at(200, 241, false));
        // The screen is awake now, but the same press stays hidden.
        assert_eq!(gate.filter(at(201, 240, true), false), at(201, 240, false));
        assert_eq!(gate.filter(at(201, 240, false), false), at(201, 240, false));
        // The next press is a normal tap.
        assert_eq!(gate.filter(at(200, 241, true), false), at(200, 241, true));
    }

    #[test]
    fn presses_while_awake_pass_through() {
        let mut gate = WakeGate::default();
        for sample in [at(56, 121, true), at(58, 122, true), at(58, 122, false)] {
            assert_eq!(gate.filter(sample, false), sample);
        }
    }

    #[test]
    fn a_press_held_into_sleep_is_not_cut_off() {
        let mut gate = WakeGate::default();
        assert_eq!(gate.filter(at(10, 10, true), false), at(10, 10, true));
        assert_eq!(gate.filter(at(10, 10, true), true), at(10, 10, true), "only a new press wakes");
    }
}
```

`gesture.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::route::ConfirmKind;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::Rect;

    fn down(x: i32, y: i32) -> TouchSample {
        TouchSample { x, y, pressed: true }
    }

    fn open() -> impl FnOnce(i32, i32) -> bool {
        |_, _| false
    }

    #[test]
    fn swipe_down_from_the_top_edge() {
        let mut g = GestureTracker::default();
        assert_eq!(g.sample(down(200, 12), 0, open()), None);
        assert_eq!(g.sample(down(202, 50), 80, open()), None, "38 px is not yet a swipe");
        assert_eq!(g.sample(down(206, 75), 160, open()), Some(Gesture::SwipeDownFromTop));
        assert_eq!(g.sample(down(206, 140), 240, open()), None, "once per press");
        assert_eq!(g.sample(TouchSample::default(), 300, open()), None);
    }

    #[test]
    fn a_swipe_down_that_starts_below_the_edge_is_ignored() {
        let mut g = GestureTracker::default();
        g.sample(down(200, 60), 0, open());
        assert_eq!(g.sample(down(200, 160), 200, open()), None, "that is a drag on the dial");
    }

    #[test]
    fn swipe_right_anywhere() {
        let mut g = GestureTracker::default();
        g.sample(down(100, 150), 0, open());
        assert_eq!(g.sample(down(170, 160), 150, open()), Some(Gesture::SwipeRight));
    }

    #[test]
    fn too_slow_too_diagonal_or_leftwards_is_not_a_swipe() {
        for (end, at_ms) in [((170, 200), 150), ((170, 150), 800), ((30, 150), 150)] {
            let mut g = GestureTracker::default();
            g.sample(down(100, 150), 0, open());
            assert_eq!(g.sample(down(end.0, end.1), at_ms, open()), None, "{end:?} at {at_ms} ms");
        }
    }

    #[test]
    fn a_press_on_a_drag_control_never_swipes() {
        let mut g = GestureTracker::default();
        let mut asked = 0;
        g.sample(down(120, 60), 0, |_, _| {
            asked += 1;
            true
        });
        assert_eq!(g.sample(down(200, 62), 100, |_, _| panic!("asked once per press")), None);
        assert_eq!(asked, 1);
    }

    #[test]
    fn gestures_map_to_menu_on_bases_and_back_on_sub_screens() {
        use Gesture::*;
        assert_eq!(gesture_event(SwipeDownFromTop, &Route::Home, 1), Some(UiEvent::Navigate(Route::Menu)));
        assert_eq!(gesture_event(SwipeDownFromTop, &Route::Recording, 1), Some(UiEvent::Navigate(Route::Menu)));
        assert_eq!(gesture_event(SwipeDownFromTop, &Route::Menu, 2), None);
        assert_eq!(gesture_event(SwipeDownFromTop, &Route::Decision, 1), None);
        assert_eq!(gesture_event(SwipeRight, &Route::Volume, 3), Some(UiEvent::Back));
        assert_eq!(
            gesture_event(SwipeRight, &Route::Confirm(ConfirmKind::PowerOff), 2),
            Some(UiEvent::Back),
            "swiping a dialog away cancels it"
        );
        assert_eq!(gesture_event(SwipeRight, &Route::Home, 1), None);
        assert_eq!(gesture_event(SwipeRight, &Route::Resting, 2), None);
    }

    #[test]
    fn drag_owner_at_finds_controls_that_keep_gestures() {
        lvgl(|lv| {
            let root = lv.screen();
            Obj::plain(root).rect(Rect::new(40, 60, 100, 60)).clickable(true);
            Obj::plain(root)
                .rect(Rect::new(220, 60, 100, 60))
                .clickable(true)
                .keep_gestures();
            let owner = Obj::plain(root).rect(Rect::new(120, 160, 160, 60)).keep_gestures();
            Obj::plain(owner).rect(Rect::new(10, 10, 60, 40)).clickable(true);
            root.update_layout();
            assert!(!drag_owner_at(root, 90, 90), "a plain button lets swipes through");
            assert!(drag_owner_at(root, 270, 90), "a control that keeps its drags");
            assert!(drag_owner_at(root, 150, 190), "inside a container that keeps them");
            assert!(!drag_owner_at(root, 380, 280), "empty space");
            // SAFETY: a null handle is what LVGL reports before the first screen loads.
            assert!(!drag_owner_at(unsafe { Obj::from_raw(std::ptr::null_mut()) }, 90, 90));
        });
    }
}
```

- [ ] **Step 2: Run and see it fail**

Run: `cargo test -p waved-ui --lib runtime:: 2>&1 | grep -m3 error`
Expected: unresolved `WakeGate`, `GestureTracker`, `gesture_event`, `drag_owner_at`.

- [ ] **Step 3: Implement.** `touch.rs` (above the tests):

```rust
//! Touch samples as the loop reads them, and the wake guard (P0.5).

/// One touch reading in LVGL coordinates (already flipped; see `input.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TouchSample {
    pub x: i32,
    pub y: i32,
    pub pressed: bool,
}

/// "First tap on the resting screen: wake only, nothing else happens."
/// A press that begins while the screen is asleep (resting or off) only
/// wakes it: LVGL sees the finger released until it lifts.
#[derive(Debug, Default)]
pub struct WakeGate {
    swallowing: bool,
    was_pressed: bool,
}

impl WakeGate {
    pub fn filter(&mut self, raw: TouchSample, asleep: bool) -> TouchSample {
        let press_starts = raw.pressed && !self.was_pressed;
        self.was_pressed = raw.pressed;
        if press_starts && asleep {
            self.swallowing = true;
        }
        if !raw.pressed {
            self.swallowing = false;
        }
        if self.swallowing {
            TouchSample {
                pressed: false,
                ..raw
            }
        } else {
            raw
        }
    }
}
```

`gesture.rs` (above the tests):

```rust
//! Swipe down from the top edge (Menu) and swipe right (Back), recognised on
//! raw samples before LVGL sees them (P0.5, mockups "Interaction model").

use crate::bindings::{
    lv_indev_search_obj, lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE, lv_obj_get_parent,
    lv_obj_has_flag, lv_point_t,
};
use crate::kit::dispatch::UiEvent;
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::runtime::touch::TouchSample;

/// Bottom of the status zone (y 6–34): a swipe down must start above it.
pub const TOP_EDGE_Y: i32 = 34;
/// Travel along the swipe direction before it counts.
pub const SWIPE_MIN_PX: i32 = 60;
/// A slower movement is a drag, not a swipe.
pub const SWIPE_MAX_MS: u64 = 700;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    SwipeDownFromTop,
    SwipeRight,
}

#[derive(Clone, Copy, Debug)]
struct Press {
    x: i32,
    y: i32,
    at_ms: u64,
    blocked: bool,
    done: bool,
}

#[derive(Debug, Default)]
pub struct GestureTracker {
    press: Option<Press>,
}

impl GestureTracker {
    /// Feed one sample per loop. `blocked_at(x, y)` is asked once, when a
    /// press starts: true if it starts on a control that keeps its drags.
    /// Returns a gesture at most once per press. The movement must stay
    /// within about 27° of its direction (cross travel ≤ half the travel).
    pub fn sample(
        &mut self,
        s: TouchSample,
        now_ms: u64,
        blocked_at: impl FnOnce(i32, i32) -> bool,
    ) -> Option<Gesture> {
        if !s.pressed {
            self.press = None;
            return None;
        }
        if self.press.is_none() {
            self.press = Some(Press {
                x: s.x,
                y: s.y,
                at_ms: now_ms,
                blocked: blocked_at(s.x, s.y),
                done: false,
            });
            return None;
        }
        let press = self.press.as_mut().expect("set above");
        if press.blocked || press.done || now_ms.saturating_sub(press.at_ms) > SWIPE_MAX_MS {
            return None;
        }
        let (dx, dy) = (s.x - press.x, s.y - press.y);
        let gesture = if press.y <= TOP_EDGE_Y && dy >= SWIPE_MIN_PX && dx.abs() * 2 <= dy {
            Some(Gesture::SwipeDownFromTop)
        } else if dx >= SWIPE_MIN_PX && dy.abs() * 2 <= dx {
            Some(Gesture::SwipeRight)
        } else {
            None
        };
        if gesture.is_some() {
            press.done = true;
        }
        gesture
    }
}

/// What a gesture does on the current screen: swipe down opens Menu from
/// Home and Recording; swipe right goes back on any sub-screen.
pub fn gesture_event(gesture: Gesture, current: &Route, depth: usize) -> Option<UiEvent> {
    match gesture {
        Gesture::SwipeDownFromTop if matches!(current, Route::Home | Route::Recording) => {
            Some(UiEvent::Navigate(Route::Menu))
        }
        Gesture::SwipeRight if depth > 1 && *current != Route::Resting => Some(UiEvent::Back),
        _ => None,
    }
}

/// True when the topmost tappable object at (x, y), or a container of it
/// below `root`, keeps its drags (`Obj::keep_gestures`).
pub fn drag_owner_at(root: Obj, x: i32, y: i32) -> bool {
    if root.is_null() {
        return false;
    }
    let mut point = lv_point_t { x, y };
    // SAFETY: `root` is the live active screen; LVGL only reads the point.
    let mut obj = unsafe { lv_indev_search_obj(root.raw(), &mut point) };
    while !obj.is_null() && obj != root.raw() {
        // SAFETY: `obj` is a live descendant of `root`.
        unsafe {
            if !lv_obj_has_flag(obj, lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE) {
                return true;
            }
            obj = lv_obj_get_parent(obj);
        }
    }
    false
}
```

Add `pub mod gesture;` and `pub mod touch;` to `runtime/mod.rs`.

- [ ] **Step 4: Run and see it pass**

Run: `set -o pipefail; cargo test -p waved-ui --lib runtime:: 2>&1 | tail -3`
Expected: `test result: ok.`

Run: `cargo clippy -p waved-ui --all-targets 2>&1 | grep -E -- "--> crates/waved-ui/src/(kit|screens_v2|runtime)/"`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-ui/src/runtime
git commit -m "feat(ui): recognise menu and back swipes and guard the waking tap

- WakeGate: a press that starts while asleep only wakes the screen
- GestureTracker: swipe down from the status zone, swipe right, once per press
- gesture_event: Menu from Home/Recording, Back on sub-screens
- drag_owner_at: presses on controls that keep their drags never swipe

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task Z4: The display power driver

**Files:**
- Create: `crates/waved-ui/src/runtime/power.rs`
- Modify: `crates/waved-ui/src/runtime/mod.rs` (`pub mod power;`)

**Interfaces:**
- Consumes: `UiSnapshot` (`settings`, `meeting.recording`, `in_call()`), `UiEvent`, `Route::Confirm(ConfirmKind::PowerOff)`.
- Produces:
  - `pub enum Power { On, Resting, Off }`, `pub enum PowerKey { Short, Long }`
  - `pub trait PowerPolicy { fn activity(&mut self, now_ms: u64); fn sleep(&mut self, now_ms: u64); fn tick(&mut self, now_ms: u64, settings: &SettingsState, keep_lit: bool) -> Power; fn backlight_pct(&self, settings: &SettingsState) -> u8; }`
  - `pub trait Backlight { fn set_pct(&mut self, pct: u8); }`
  - `pub struct PowerDriver` with `new(Box<dyn PowerPolicy>, Box<dyn Backlight>)`, `asleep(&self) -> bool`, `touch(&mut self, now_ms)`, `key(&mut self, key: PowerKey, now_ms) -> Option<UiEvent>`, `step(&mut self, now_ms, snap: &UiSnapshot) -> Option<UiEvent>`

- [ ] **Step 1: Write the failing tests** (bottom of `power.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// Timers the test drives by hand.
    struct Fake {
        state: Rc<Cell<Power>>,
        log: Rc<RefCell<Vec<String>>>,
    }

    impl PowerPolicy for Fake {
        fn activity(&mut self, _now_ms: u64) {
            self.log.borrow_mut().push("activity".into());
            self.state.set(Power::On);
        }
        fn sleep(&mut self, _now_ms: u64) {
            self.log.borrow_mut().push("sleep".into());
            self.state.set(Power::Resting);
        }
        fn tick(&mut self, _now_ms: u64, _settings: &SettingsState, keep_lit: bool) -> Power {
            self.log.borrow_mut().push(format!("tick keep_lit={keep_lit}"));
            self.state.get()
        }
        fn backlight_pct(&self, settings: &SettingsState) -> u8 {
            match self.state.get() {
                Power::On => settings.brightness_pct,
                Power::Resting => 20,
                Power::Off => 0,
            }
        }
    }

    struct Panel(Rc<RefCell<Vec<u8>>>);

    impl Backlight for Panel {
        fn set_pct(&mut self, pct: u8) {
            self.0.borrow_mut().push(pct);
        }
    }

    struct Rig {
        driver: PowerDriver,
        state: Rc<Cell<Power>>,
        log: Rc<RefCell<Vec<String>>>,
        writes: Rc<RefCell<Vec<u8>>>,
    }

    fn rig() -> Rig {
        let state = Rc::new(Cell::new(Power::On));
        let log = Rc::new(RefCell::new(Vec::new()));
        let writes = Rc::new(RefCell::new(Vec::new()));
        let driver = PowerDriver::new(
            Box::new(Fake { state: state.clone(), log: log.clone() }),
            Box::new(Panel(writes.clone())),
        );
        Rig { driver, state, log, writes }
    }

    #[test]
    fn timers_become_rest_and_wake_for_the_router() {
        let mut r = rig();
        let snap = UiSnapshot::default();
        assert_eq!(r.driver.step(0, &snap), None);
        assert!(!r.driver.asleep());
        r.state.set(Power::Resting);
        assert_eq!(r.driver.step(120_000, &snap), Some(UiEvent::Rest));
        assert!(r.driver.asleep());
        r.state.set(Power::Off);
        assert_eq!(r.driver.step(600_000, &snap), None, "already resting");
        r.state.set(Power::On);
        assert_eq!(r.driver.step(600_010, &snap), Some(UiEvent::Wake));
        assert_eq!(*r.writes.borrow(), [80, 20, 0, 80]);
    }

    #[test]
    fn the_backlight_is_written_only_when_it_changes() {
        let mut r = rig();
        let mut snap = UiSnapshot::default();
        r.driver.step(0, &snap);
        r.driver.step(10, &snap);
        snap.settings.brightness_pct = 50;
        r.driver.step(20, &snap);
        assert_eq!(*r.writes.borrow(), [80, 50], "brightness changes apply at once");
    }

    #[test]
    fn touch_and_keys_wake_and_the_router_hears_it() {
        let mut r = rig();
        let snap = UiSnapshot::default();
        r.state.set(Power::Off);
        r.driver.step(0, &snap);
        r.driver.touch(10);
        assert_eq!(r.driver.step(10, &snap), Some(UiEvent::Wake));
        assert_eq!(r.driver.key(PowerKey::Short, 20), None);
        assert_eq!(r.driver.step(20, &snap), Some(UiEvent::Rest), "short press while on sleeps");
        assert_eq!(r.driver.key(PowerKey::Short, 30), None);
        assert_eq!(r.driver.step(30, &snap), Some(UiEvent::Wake), "short press while asleep wakes");
        assert_eq!(
            r.driver.key(PowerKey::Long, 40),
            Some(UiEvent::Navigate(Route::Confirm(ConfirmKind::PowerOff)))
        );
        let log = r.log.borrow();
        assert!(log.contains(&"sleep".to_string()) && log.iter().filter(|l| *l == "activity").count() == 3);
    }

    #[test]
    fn recording_or_a_call_keeps_the_screen_lit() {
        let mut r = rig();
        let mut snap = UiSnapshot::default();
        r.driver.step(0, &snap);
        snap.meeting.recording = true;
        r.driver.step(1, &snap);
        snap.meeting.recording = false;
        snap.audio.call_off_hook = true;
        r.driver.step(2, &snap);
        assert_eq!(
            *r.log.borrow(),
            ["tick keep_lit=false", "tick keep_lit=true", "tick keep_lit=true"]
        );
    }
}
```

- [ ] **Step 2: Run and see it fail**

Run: `cargo test -p waved-ui --lib runtime::power 2>&1 | grep -m3 error`
Expected: unresolved `PowerDriver`, `PowerPolicy`, `Backlight`, `Power`, `PowerKey`.

- [ ] **Step 3: Implement** (top of `power.rs`):

```rust
//! Screen power for the UI: workstream E's dim/off timers (E1) drive the
//! router's Rest/Wake and the backlight; touches and the power key (E2) feed
//! the timers. The timers themselves live in `waved-system`; this file only
//! connects them (the adapter is added at the runtime switch, Task Z8).

use crate::kit::dispatch::UiEvent;
use crate::kit::route::{ConfirmKind, Route};
use crate::kit::snapshot::UiSnapshot;
use waved_state::SettingsState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    On,
    /// Dimmed: the resting screen shows.
    Resting,
    /// Backlight off.
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerKey {
    Short,
    /// Held for 2 s.
    Long,
}

/// The dim/off timers.
pub trait PowerPolicy {
    /// A touch or a key: restart the timers and wake.
    fn activity(&mut self, now_ms: u64);
    /// The power key asked for sleep.
    fn sleep(&mut self, now_ms: u64);
    /// The state now. `keep_lit`: recording or in a call, so never fully off.
    fn tick(&mut self, now_ms: u64, settings: &SettingsState, keep_lit: bool) -> Power;
    /// Backlight percentage for the current state (0 when off).
    fn backlight_pct(&self, settings: &SettingsState) -> u8;
}

pub trait Backlight {
    fn set_pct(&mut self, pct: u8);
}

pub struct PowerDriver {
    policy: Box<dyn PowerPolicy>,
    backlight: Box<dyn Backlight>,
    state: Power,
    applied: Option<u8>,
}

impl PowerDriver {
    pub fn new(policy: Box<dyn PowerPolicy>, backlight: Box<dyn Backlight>) -> Self {
        Self {
            policy,
            backlight,
            state: Power::On,
            applied: None,
        }
    }

    /// Resting or off: the next new press only wakes (`WakeGate`).
    pub fn asleep(&self) -> bool {
        self.state != Power::On
    }

    pub fn touch(&mut self, now_ms: u64) {
        self.policy.activity(now_ms);
    }

    /// Short press: sleep when on, wake when asleep. Long press: wake and
    /// ask to power off (power off never asks for the PIN, D10).
    pub fn key(&mut self, key: PowerKey, now_ms: u64) -> Option<UiEvent> {
        match key {
            PowerKey::Short if self.state == Power::On => {
                self.policy.sleep(now_ms);
                None
            }
            PowerKey::Short => {
                self.policy.activity(now_ms);
                None
            }
            PowerKey::Long => {
                self.policy.activity(now_ms);
                Some(UiEvent::Navigate(Route::Confirm(ConfirmKind::PowerOff)))
            }
        }
    }

    /// Advance the timers; returns Rest or Wake when the screen changes state,
    /// and writes the backlight whenever its level changes.
    pub fn step(&mut self, now_ms: u64, snap: &UiSnapshot) -> Option<UiEvent> {
        let keep_lit = snap.meeting.recording || snap.in_call();
        let next = self.policy.tick(now_ms, &snap.settings, keep_lit);
        let event = match (self.state, next) {
            (Power::On, Power::Resting | Power::Off) => Some(UiEvent::Rest),
            (Power::Resting | Power::Off, Power::On) => Some(UiEvent::Wake),
            _ => None,
        };
        self.state = next;
        let pct = self.policy.backlight_pct(&snap.settings);
        if self.applied != Some(pct) {
            self.backlight.set_pct(pct);
            self.applied = Some(pct);
        }
        event
    }
}
```

Add `pub mod power;` to `runtime/mod.rs`.

- [ ] **Step 4: Run and see it pass**

Run: `set -o pipefail; cargo test -p waved-ui --lib runtime::power 2>&1 | tail -3`
Expected: `test result: ok. 4 passed`.

Run: `cargo clippy -p waved-ui --all-targets 2>&1 | grep -E -- "--> crates/waved-ui/src/(kit|screens_v2|runtime)/"`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-ui/src/runtime
git commit -m "feat(ui): drive rest, wake and the backlight from the power timers

- PowerDriver turns timer states into router Rest/Wake events
- touches and the power key feed the timers; long press asks to power off
- recording or a call keeps the screen lit; backlight written on change only

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task Z5: The runtime core

**Files:**
- Modify: `crates/waved-ui/src/runtime/mod.rs`

**Interfaces:**
- Consumes: `Router::{new, handle, sync, current, depth}`, `Dispatcher::new`, `ScreenCtx`, `Screen`, `Obj::screen`, `Notices` (Z2), `ActiveMessage` / `UiSnapshot::message` (Z1).
- Produces:
  - `pub const BOOT_MIN_MS: u64 = 1_500`
  - `pub type Registry = Box<dyn Fn(&Route) -> Option<Box<dyn Screen>>>`
  - `pub struct Runtime` with `new(bus: broadcast::Sender<Command>, config: Arc<WavedConfig>, registry: Registry) -> Runtime`, `dispatcher(&self) -> Dispatcher`, `current(&self) -> &Route`, `depth(&self) -> usize`, `root(&self) -> Option<Obj>`, `shown_route(&self) -> Option<&Route>`, `observe_command(&mut self, &Command)`, `step(&mut self, snap: UiSnapshot, events: &[DeviceEvent])`; test-only `close(self)`
  - test helper `pub(crate) fn shows_text(root: Obj, text: &str) -> bool` (in `runtime::testing`, `#[cfg(test)]`)

- [ ] **Step 1: Write the failing tests** (bottom of `runtime/mod.rs`):

```rust
#[cfg(test)]
pub(crate) mod testing {
    use crate::bindings::{lv_label_class, lv_obj_check_type};
    use crate::kit::obj::Obj;

    /// A visible label under `root` reads exactly `text`.
    pub(crate) fn shows_text(root: Obj, text: &str) -> bool {
        if root.is_hidden() {
            return false;
        }
        // SAFETY: live object; `lv_label_class` is a static class descriptor.
        let label = unsafe { lv_obj_check_type(root.raw(), &raw const lv_label_class) };
        if label && root.text() == text {
            return true;
        }
        (0..root.child_count()).any(|i| shows_text(root.child(i as i32), text))
    }
}

#[cfg(test)]
mod tests {
    use super::testing::shows_text;
    use super::*;
    use crate::bindings::{lv_obj_is_valid, lv_screen_active};
    use crate::kit::components::status::StatusArea;
    use crate::kit::dispatch::Severity;
    use crate::kit::fonts::Font;
    use crate::kit::testkit::{lvgl, Lvgl};
    use crate::kit::tokens::TEXT;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use uuid::Uuid;
    use waved_state::PendingDecision;

    thread_local! {
        static BUILDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn builds() -> Vec<String> {
        BUILDS.with(|b| b.borrow().clone())
    }

    /// A screen that names its route and has a status area.
    struct Probe {
        name: String,
        status: Option<StatusArea>,
    }

    impl Screen for Probe {
        fn build(&mut self, root: Obj, _ctx: &ScreenCtx, snap: &UiSnapshot) {
            BUILDS.with(|b| b.borrow_mut().push(self.name.clone()));
            self.status = Some(StatusArea::plain(root));
            Obj::label(root, &self.name, Font::Body, TEXT).pos(100, 140);
            self.update(snap);
        }

        fn update(&mut self, snap: &UiSnapshot) {
            if let Some(status) = self.status.as_mut() {
                status.update(snap);
            }
        }
    }

    fn probes(route: &Route) -> Option<Box<dyn Screen>> {
        match route {
            Route::Bluetooth | Route::MicHeight | Route::Pin(_) => None,
            other => Some(Box::new(Probe {
                name: format!("{other:?}"),
                status: None,
            })),
        }
    }

    fn ready(now_ms: u64) -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.settings.first_run_done = true;
        snap.now_ms = now_ms;
        snap
    }

    fn runtime() -> Runtime {
        BUILDS.with(|b| b.borrow_mut().clear());
        let (bus, _) = broadcast::channel(16);
        Runtime::new(bus, Arc::new(WavedConfig::default()), Box::new(probes))
    }

    fn booted(lv: &mut Lvgl) -> Runtime {
        lv.screen();
        let mut rt = runtime();
        rt.step(ready(0), &[]);
        rt.step(ready(BOOT_MIN_MS), &[]);
        rt
    }

    fn send(rt: &mut Runtime, event: UiEvent, now_ms: u64) -> Option<Route> {
        rt.dispatcher().ui(event);
        rt.step(ready(now_ms), &[]);
        rt.shown_route().cloned()
    }

    fn decision() -> PendingDecision {
        PendingDecision {
            id: Uuid::new_v4(),
            audio_path: PathBuf::from("/mnt/udisk/recordings/x/audio.wav"),
            duration_secs: 2537,
            size_bytes: 184_000_000,
            endpoint: "https://notes.example.com/hook".into(),
        }
    }

    #[test]
    fn the_start_up_screen_gives_way_after_its_ring_or_when_it_says_so() {
        lvgl(|lv| {
            lv.screen();
            let mut rt = runtime();
            rt.step(ready(0), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Boot));
            rt.step(ready(BOOT_MIN_MS - 1), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Boot));
            rt.step(ready(BOOT_MIN_MS), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Home));
            rt.close();

            lv.screen();
            let mut early = runtime();
            early.step(ready(0), &[]);
            early.dispatcher().ui(UiEvent::BootDone);
            early.step(ready(400), &[]);
            assert_eq!(early.shown_route(), Some(&Route::Home));
            early.step(ready(BOOT_MIN_MS), &[]);
            assert_eq!(early.shown_route(), Some(&Route::Home), "the late BootDone changes nothing");
            early.close();
        });
    }

    #[test]
    fn navigation_builds_the_routed_screen_and_deletes_the_last_one() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            assert_eq!(rt.shown_route(), Some(&Route::Home));
            let home = rt.root().unwrap();
            assert_eq!(send(&mut rt, UiEvent::Navigate(Route::Menu), 2_000), Some(Route::Menu));
            // SAFETY: only compares pointers against live objects.
            unsafe {
                assert!(!lv_obj_is_valid(home.raw()), "the old screen and its handlers are gone");
                assert_eq!(lv_screen_active(), rt.root().unwrap().raw());
            }
            assert_eq!(builds(), ["Boot", "Home", "Menu"]);
            rt.step(ready(2_100), &[]);
            assert_eq!(builds().len(), 3, "an unchanged route is not rebuilt");
            rt.close();
        });
    }

    #[test]
    fn back_returns_to_the_screen_that_opened_it() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            assert_eq!(send(&mut rt, UiEvent::Navigate(Route::Menu), 2_000), Some(Route::Menu));
            assert_eq!(send(&mut rt, UiEvent::Navigate(Route::AudioMode), 2_100), Some(Route::AudioMode));
            assert_eq!(send(&mut rt, UiEvent::Back, 2_200), Some(Route::Menu), "BUG-11");
            assert_eq!(send(&mut rt, UiEvent::Navigate(Route::System), 2_300), Some(Route::System));
            assert_eq!(send(&mut rt, UiEvent::Navigate(Route::Display), 2_400), Some(Route::Display));
            assert_eq!(send(&mut rt, UiEvent::Back, 2_500), Some(Route::System));
            assert_eq!(send(&mut rt, UiEvent::Back, 2_600), Some(Route::Menu));
            assert_eq!(send(&mut rt, UiEvent::Back, 2_700), Some(Route::Home));
            assert_eq!(send(&mut rt, UiEvent::Back, 2_800), Some(Route::Home), "Back stops at the base");
            rt.close();
        });
    }

    #[test]
    fn meeting_state_moves_the_base_from_any_screen() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            send(&mut rt, UiEvent::Navigate(Route::Menu), 2_000);
            send(&mut rt, UiEvent::Navigate(Route::Volume), 2_100);
            let mut snap = ready(2_200);
            snap.meeting.recording = true;
            rt.step(snap.clone(), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Volume), "the open screen stays");
            rt.dispatcher().back();
            rt.dispatcher().back();
            snap.now_ms = 2_300;
            rt.step(snap.clone(), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Recording), "BUG-13: the base followed the meeting");
            rt.dispatcher().navigate(Route::Menu);
            snap.now_ms = 2_400;
            rt.step(snap.clone(), &[]);
            snap.meeting.recording = false;
            snap.meeting.pending_decision = Some(decision());
            snap.now_ms = 2_500;
            rt.step(snap, &[]);
            assert_eq!(
                (rt.shown_route(), rt.depth()),
                (Some(&Route::Decision), 1),
                "the decision takes over at once"
            );
            rt.close();
        });
    }

    #[test]
    fn a_route_without_a_screen_goes_back() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            send(&mut rt, UiEvent::Navigate(Route::Menu), 2_000);
            assert_eq!(send(&mut rt, UiEvent::Navigate(Route::Bluetooth), 2_100), Some(Route::Menu));
            assert_eq!(rt.depth(), 2);
            assert_eq!(builds(), ["Boot", "Home", "Menu"], "Menu was not rebuilt");
            rt.close();
        });
    }

    #[test]
    fn a_device_password_locks_nothing_until_the_pin_screen_lands() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            let mut snap = ready(2_000);
            snap.settings.pin_enabled = true;
            rt.dispatcher().navigate(Route::Files);
            rt.step(snap, &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Files));
            rt.close();
        });
    }

    #[test]
    fn messages_reach_the_status_slot_and_outlive_navigation() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            let id = Uuid::new_v4();
            rt.step(ready(2_000), &[DeviceEvent::RecordingStopped(id)]);
            rt.step(ready(2_100), &[DeviceEvent::RecordingUploadSucceeded(id)]);
            assert!(shows_text(rt.root().unwrap(), "Recording sent"));
            rt.dispatcher().navigate(Route::Menu);
            rt.step(ready(3_000), &[]);
            assert!(shows_text(rt.root().unwrap(), "Recording sent"), "the next screen keeps it");
            rt.step(ready(4_100), &[]);
            assert!(!shows_text(rt.root().unwrap(), "Recording sent"), "gone 2 s after it was posted");
            rt.dispatcher().show(Severity::Warning, "Hold Stop to end the recording");
            rt.step(ready(4_200), &[]);
            assert!(shows_text(rt.root().unwrap(), "Hold Stop to end the recording"));
            rt.close();
        });
    }

    #[test]
    fn a_resent_recording_is_announced() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            let id = Uuid::new_v4();
            rt.observe_command(&Command::SendRecording(id));
            rt.step(ready(2_000), &[DeviceEvent::RecordingUploadSucceeded(id)]);
            assert!(shows_text(rt.root().unwrap(), "Recording sent"));
            rt.close();
        });
    }
}
```

- [ ] **Step 2: Run and see it fail**

Run: `cargo test -p waved-ui --lib runtime::tests 2>&1 | grep -m3 error`
Expected: `cannot find type \`Runtime\`` and `cannot find value \`BOOT_MIN_MS\``.

- [ ] **Step 3: Implement.** Replace the top of `runtime/mod.rs` (keep the `pub mod` lines):

```rust
//! The revision-2 UI runtime: one router, the screens_v2 registry and the
//! dispatcher, fed by one snapshot per loop iteration. `step` is
//! host-testable; `run` (the UI thread) adds the device around it.

pub mod feedback;
pub mod gesture;
pub mod notices;
pub mod power;
pub mod touch;

use crate::kit::dispatch::{ActiveMessage, Dispatcher, Message, UiEvent};
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::router::Router;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use notices::Notices;
use std::sync::{mpsc, Arc};
use tokio::sync::broadcast;
use tracing::warn;
use waved_config::WavedConfig;
use waved_state::{Command, DeviceEvent};

/// The start-up screen shows at least this long (A5: the ring draws while
/// services start; `main.rs` starts the UI after the services). The boot
/// screen may leave earlier with `UiEvent::BootDone`; a second BootDone is
/// ignored by the router.
pub const BOOT_MIN_MS: u64 = 1_500;

/// Builds the screen for a route; `None` for routes with no screen yet.
pub type Registry = Box<dyn Fn(&Route) -> Option<Box<dyn Screen>>>;

/// Back-outs per step when routes have no screen (a chain of M3 routes).
const MAX_BACK_OUTS: usize = 4;

struct Shown {
    route: Route,
    root: Obj,
    screen: Box<dyn Screen>,
}

pub struct Runtime {
    router: Router,
    registry: Registry,
    ctx: ScreenCtx,
    ui_rx: mpsc::Receiver<UiEvent>,
    notices: Notices,
    message: Option<ActiveMessage>,
    next_message_id: u64,
    boot_done: bool,
    shown: Option<Shown>,
}

impl Runtime {
    pub fn new(
        bus: broadcast::Sender<Command>,
        config: Arc<WavedConfig>,
        registry: Registry,
    ) -> Self {
        let (ui_tx, ui_rx) = mpsc::channel();
        Self {
            router: Router::new(),
            registry,
            ctx: ScreenCtx {
                dispatch: Dispatcher::new(bus, ui_tx),
                config,
            },
            ui_rx,
            notices: Notices::default(),
            message: None,
            next_message_id: 0,
            boot_done: false,
            shown: None,
        }
    }

    pub fn dispatcher(&self) -> Dispatcher {
        self.ctx.dispatch.clone()
    }

    pub fn current(&self) -> &Route {
        self.router.current()
    }

    pub fn depth(&self) -> usize {
        self.router.depth()
    }

    /// The root of the screen on display.
    pub fn root(&self) -> Option<Obj> {
        self.shown.as_ref().map(|shown| shown.root)
    }

    /// The route of the screen on display.
    pub fn shown_route(&self) -> Option<&Route> {
        self.shown.as_ref().map(|shown| &shown.route)
    }

    /// Commands seen on the bus (to announce uploads the user started).
    pub fn observe_command(&mut self, command: &Command) {
        self.notices.on_command(command);
    }

    /// One UI loop iteration, before `lv_timer_handler`.
    pub fn step(&mut self, mut snap: UiSnapshot, events: &[DeviceEvent]) {
        // No PIN screen until milestone 3 (K2): nothing locks yet, even with
        // a device password set on the setup page.
        snap.settings.pin_enabled = false;
        self.notices.observe(&snap);
        for event in events {
            if let Some(message) = self.notices.handle(event, &snap) {
                self.post(message, snap.now_ms);
            }
        }
        if !self.boot_done && snap.now_ms >= BOOT_MIN_MS {
            self.boot_done = true;
            self.router.handle(&UiEvent::BootDone, &snap);
        }
        while let Ok(event) = self.ui_rx.try_recv() {
            match event {
                UiEvent::Show(message) => self.post(message, snap.now_ms),
                other => self.router.handle(&other, &snap),
            }
        }
        self.router.sync(&snap);
        if self
            .message
            .as_ref()
            .is_some_and(|active| snap.now_ms >= active.until_ms())
        {
            self.message = None;
        }
        snap.message = self.message.clone();
        self.show_current(&snap);
        if let Some(shown) = self.shown.as_mut() {
            shown.screen.update(&snap);
        }
    }

    fn post(&mut self, message: Message, now_ms: u64) {
        self.next_message_id += 1;
        self.message = Some(ActiveMessage {
            id: self.next_message_id,
            message,
            since_ms: now_ms,
        });
    }

    /// Build the routed screen if it is not the one on display, load it,
    /// then delete the old root with every handler.
    fn show_current(&mut self, snap: &UiSnapshot) {
        for _ in 0..MAX_BACK_OUTS {
            let route = self.router.current().clone();
            if self.shown.as_ref().is_some_and(|shown| shown.route == route) {
                return;
            }
            let Some(mut screen) = (self.registry)(&route) else {
                warn!("no screen for {route:?} yet; going back");
                if self.router.depth() > 1 {
                    self.router.handle(&UiEvent::Back, snap);
                    continue;
                }
                return;
            };
            let root = Obj::screen();
            screen.build(root, &self.ctx, snap);
            load(root);
            if let Some(old) = self.shown.take() {
                old.root.delete();
            }
            self.shown = Some(Shown { route, root, screen });
            return;
        }
    }

    /// Delete the screen on display (tests only; the device never stops).
    #[cfg(test)]
    pub fn close(mut self) {
        if let Some(shown) = self.shown.take() {
            shown.root.delete();
        }
    }
}

fn load(root: Obj) {
    // SAFETY: `root` is a live screen created by `Obj::screen`.
    unsafe { crate::bindings::lv_screen_load(root.raw()) };
}
```

`registry`, `device` and `run` are declared by the tasks that create them (Z6, Z7, Z8).

- [ ] **Step 4: Run and see it pass**

Run: `set -o pipefail; cargo test -p waved-ui --lib runtime:: 2>&1 | tail -3`
Expected: `test result: ok.` (the eight new runtime tests plus Z2–Z4's).

Run: `cargo clippy -p waved-ui --all-targets 2>&1 | grep -E -- "--> crates/waved-ui/src/(kit|screens_v2|runtime)/"`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-ui/src/runtime
git commit -m "feat(ui): add the runtime that routes, builds and messages

- Runtime::step: router, registry, dispatcher and one active message
- fresh screen per navigation; the old root and its handlers are deleted
- Back returns to the opener (BUG-11); the meeting moves the base anywhere (BUG-13)
- routes without a screen back out; no PIN locking until K2
- leaves the start-up screen after 1.5 s or on BootDone

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task Z6: The screen registry

**Files:**
- Create: `crates/waved-ui/src/runtime/registry.rs`
- Modify: `crates/waved-ui/src/runtime/mod.rs` (`pub mod registry;`)

**Interfaces:**
- Consumes: every `screens_v2` type from the S1 convention table (merged A, B, D), `Route`, `ConfirmKind`.
- Produces: `pub fn screen_for(route: &Route) -> Option<Box<dyn Screen>>`.

- [ ] **Step 1: Read the merged screens.** For each module in `crates/waved-ui/src/screens_v2/`, find the type that implements `Screen` and its constructor:

```bash
grep -n "impl Screen for\|pub fn new\|pub fn speaker\|pub fn mic\|derive(.*Default" crates/waved-ui/src/screens_v2/*.rs
```

Expected: every row of the S1 table has a match. Where a workstream used another name or constructor, write that name in Step 3 instead of the table's; never edit the screen file. If a screen needs an argument the table does not give, stop and raise it with the workstream.

- [ ] **Step 2: Write the failing tests** (bottom of `registry.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::{ActiveMessage, Dispatcher, Message, Severity};
    use crate::kit::route::ConfirmKind;
    use crate::kit::screen::ScreenCtx;
    use crate::kit::snapshot::UiSnapshot;
    use crate::kit::testkit::{assert_targets_ok, lvgl};
    use crate::runtime::testing::shows_text;
    use std::path::PathBuf;
    use std::sync::Arc;
    use uuid::Uuid;
    use waved_config::WavedConfig;
    use waved_state::{PendingDecision, RecordingStatus, RecordingSummary};

    const FILE: Uuid = Uuid::from_u128(0x5eed);

    fn m2_routes() -> Vec<Route> {
        vec![
            Route::Boot,
            Route::FirstRun,
            Route::Home,
            Route::Recording,
            Route::Decision,
            Route::Resting,
            Route::Menu,
            Route::AudioMode,
            Route::NoiseReduction,
            Route::Volume,
            Route::MicLevel,
            Route::Files,
            Route::FileDetail(FILE),
            Route::Wifi,
            Route::WifiSetup,
            Route::Uploads,
            Route::UploadMode,
            Route::System,
            Route::Display,
            Route::DateTime,
            Route::About,
            Route::Confirm(ConfirmKind::PowerOff),
            Route::Confirm(ConfirmKind::FactoryReset),
            Route::Confirm(ConfirmKind::DeleteRecording(FILE)),
            Route::Confirm(ConfirmKind::StopUsbStorage),
        ]
    }

    fn fixture() -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.settings.first_run_done = true;
        snap.clock = "14:32".into();
        snap.battery.level = 64;
        snap.mcu.usb_connected = true;
        snap.now_ms = 10_000;
        snap.now_unix_s = 1_759_588_320;
        snap.meeting.pending_decision = Some(PendingDecision {
            id: FILE,
            audio_path: PathBuf::from("/mnt/udisk/recordings/x/audio.wav"),
            duration_secs: 2537,
            size_bytes: 184_000_000,
            endpoint: "https://notes.example.com/hook".into(),
        });
        snap.files.recordings = vec![RecordingSummary {
            id: FILE,
            started_at: Some("2026-10-05T14:32:00Z".into()),
            duration_secs: 2537,
            size_bytes: 184_000_000,
            status: RecordingStatus::Uploaded,
            marks: 2,
            last_error: None,
        }];
        snap.files.free_bytes = 12_000_000_000;
        snap
    }

    fn build(route: &Route, snap: &UiSnapshot, check: impl FnOnce(crate::kit::obj::Obj)) {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let ctx = ScreenCtx {
                dispatch,
                config: Arc::new(WavedConfig::default()),
            };
            let mut screen = screen_for(route).unwrap_or_else(|| panic!("no screen for {route:?}"));
            screen.build(root, &ctx, snap);
            screen.update(snap);
            check(root);
        });
    }

    #[test]
    fn every_m2_route_builds_inside_the_tap_limit() {
        let snap = fixture();
        for route in m2_routes() {
            build(&route, &snap, |root| assert_targets_ok(root));
        }
    }

    #[test]
    fn screens_cope_with_missing_data() {
        // The runtime can show a screen one frame before or after its data.
        let snap = UiSnapshot::default();
        for route in m2_routes() {
            build(&route, &snap, |_| {});
        }
    }

    #[test]
    fn every_screen_with_a_status_shows_runtime_messages() {
        let mut snap = fixture();
        snap.message = Some(ActiveMessage {
            id: 1,
            message: Message::new(Severity::Info, "Probe message"),
            since_ms: snap.now_ms,
        });
        for route in m2_routes() {
            // The start-up, resting and confirmation screens have no status slot.
            if matches!(route, Route::Boot | Route::Resting | Route::Confirm(_)) {
                continue;
            }
            build(&route, &snap, |root| {
                assert!(shows_text(root, "Probe message"), "{route:?} hides runtime messages");
            });
        }
    }

    #[test]
    fn milestone_3_routes_have_no_screen_yet() {
        for route in [
            Route::MicHeight,
            Route::Bluetooth,
            Route::Pin(Box::new(Route::Files)),
            Route::UsbStorage,
            Route::Update,
            Route::IncomingCall,
        ] {
            assert!(screen_for(&route).is_none(), "{route:?}");
        }
    }
}
```

- [ ] **Step 3: Run and see it fail**

Run: `cargo test -p waved-ui --lib runtime::registry 2>&1 | grep -m3 error`
Expected: `cannot find function \`screen_for\``.

- [ ] **Step 4: Implement** (top of `registry.rs`; adjust names per Step 1 only):

```rust
//! Route → screen. The runtime builds a fresh screen from here on every
//! navigation (`kit/README.md`, "Registering a screen").

use crate::kit::route::Route;
use crate::kit::screen::Screen;
use crate::screens_v2::{
    about, ambient, audio_mode, boot, confirm, datetime, decision, display, file_detail, files,
    first_run, home, menu, noise, recording, system, upload_mode, uploads, volume, wifi,
    wifi_setup,
};

pub fn screen_for(route: &Route) -> Option<Box<dyn Screen>> {
    let screen: Box<dyn Screen> = match route {
        Route::Boot => Box::new(boot::BootScreen::default()),
        Route::FirstRun => Box::new(first_run::FirstRunScreen::default()),
        Route::Home => Box::new(home::HomeScreen::default()),
        Route::Recording => Box::new(recording::RecordingScreen::default()),
        Route::Decision => Box::new(decision::DecisionScreen::default()),
        Route::Resting => Box::new(ambient::AmbientScreen::default()),
        Route::Menu => Box::new(menu::MenuScreen::default()),
        Route::AudioMode => Box::new(audio_mode::AudioModeScreen::default()),
        Route::NoiseReduction => Box::new(noise::NoiseScreen::default()),
        Route::Volume => Box::new(volume::VolumeScreen::speaker()),
        Route::MicLevel => Box::new(volume::VolumeScreen::mic()),
        Route::Files => Box::new(files::FilesScreen::default()),
        Route::FileDetail(id) => Box::new(file_detail::FileDetailScreen::new(*id)),
        Route::Wifi => Box::new(wifi::WifiScreen::default()),
        Route::WifiSetup => Box::new(wifi_setup::WifiSetupScreen::default()),
        Route::Uploads => Box::new(uploads::UploadsScreen::default()),
        Route::UploadMode => Box::new(upload_mode::UploadModeScreen::default()),
        Route::System => Box::new(system::SystemScreen::default()),
        Route::Display => Box::new(display::DisplayScreen::default()),
        Route::DateTime => Box::new(datetime::DateTimeScreen::default()),
        Route::About => Box::new(about::AboutScreen::default()),
        Route::Confirm(kind) => Box::new(confirm::ConfirmScreen::new(*kind)),
        // Milestone 3: Mic height (B5/F3), Bluetooth (I), PIN (K2),
        // USB storage (G), firmware update (H), incoming call (I).
        Route::MicHeight
        | Route::Bluetooth
        | Route::Pin(_)
        | Route::UsbStorage
        | Route::Update
        | Route::IncomingCall => return None,
    };
    Some(screen)
}
```

The body never names `ConfirmKind` (the `kind` binding carries it), so the module imports only `Route`; add `use crate::kit::route::ConfirmKind;` to the test module's imports (Step 2).

Add `pub mod registry;` to `runtime/mod.rs`.

- [ ] **Step 5: Run and see it pass**

Run: `set -o pipefail; cargo test -p waved-ui --lib runtime::registry 2>&1 | tail -3`
Expected: `test result: ok. 4 passed`.

A failure here is an integration finding, not a registry bug: `assert_targets_ok` or a panic names the route; a missing message names the screen that does not call `status.update(snap)`. Raise it with the owning workstream (A, B or D) and wait for their fix on `feat/display-redesign`; do not patch their file.

Run: `cargo clippy -p waved-ui --all-targets 2>&1 | grep -E -- "--> crates/waved-ui/src/(kit|screens_v2|runtime)/"`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-ui/src/runtime
git commit -m "feat(ui): register the milestone 2 screens by route

- runtime::registry maps every M2 route to its screens_v2 screen
- milestone 3 routes have no screen; the runtime backs out of them
- every screen builds inside the tap limit, copes with missing data
  and shows runtime messages

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task Z7: Touch samples and the LVGL device layer

**Files:**
- Modify: `crates/waved-ui/src/input.rs`
- Create: `crates/waved-ui/src/runtime/device.rs`
- Modify: `crates/waved-ui/src/runtime/mod.rs` (`pub mod device;`)

**Interfaces:**
- Consumes: `FramebufferDisplay` (`display.rs`), `TouchSample` (Z3).
- Produces:
  - `pub fn input::flip_x(raw: i32) -> i32`, `pub fn input::flip_y(raw: i32) -> i32`; `TouchInput::poll(&mut self) -> TouchSample`
  - `pub struct Device` with `pub fn init(fb: FramebufferDisplay) -> Device`, `report(&self, TouchSample)`, `cancel_press(&self)`, `active_screen(&self) -> Obj`, `tick(&self, ms: u32)`, `run_timers(&self)`
  - `pub(crate) fn release_top_layer()`

- [ ] **Step 1: Write the failing tests.**

`input.rs`, new test module at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_axes_flip_native_pixels() {
        // ILITEK_TDDI reports native pixels (0..399, 0..303), not 0..2047,
        // and the panel is mounted upside down like the display.
        assert_eq!((flip_x(0), flip_x(399)), (399, 0));
        assert_eq!((flip_y(0), flip_y(303)), (303, 0));
        assert_eq!((flip_x(200), flip_y(152)), (199, 151));
        assert_eq!((flip_x(2047), flip_y(-4)), (0, 303), "out-of-range values clamp");
    }
}
```

`runtime/device.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::lvgl;

    #[test]
    fn the_top_layer_never_takes_taps() {
        lvgl(|_lv| {
            // SAFETY: the headless display's top layer.
            let top = unsafe { Obj::from_raw(lv_layer_top()) };
            top.clickable(true);
            release_top_layer();
            assert!(!top.is_clickable(), "a clickable top layer swallows every tap (BUG-3)");
        });
    }
}
```

- [ ] **Step 2: Run and see it fail**

Run: `cargo test -p waved-ui --lib -- touch_axes top_layer 2>&1 | grep -m3 error`
Expected: unresolved `flip_x`, `flip_y`, `release_top_layer`.

- [ ] **Step 3: Implement.**

`input.rs`: replace the struct, `init` and `poll` (keep `InputEvent`, the constants and `Drop`):

```rust
use crate::runtime::touch::TouchSample;

/// ILITEK_TDDI reports native screen pixels (x 0–399), not 0–2047, and the
/// touch panel is mounted upside down like the display (`display.rs` flips
/// 180° in `flush`).
pub fn flip_x(raw: i32) -> i32 {
    399 - raw.clamp(0, 399)
}

/// See `flip_x`; y runs 0–303.
pub fn flip_y(raw: i32) -> i32 {
    303 - raw.clamp(0, 303)
}

pub struct TouchInput {
    #[cfg(not(feature = "simulate"))]
    fd: std::os::unix::io::RawFd,
    sample: TouchSample,
}

impl TouchInput {
    #[cfg(feature = "simulate")]
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        info!("[touch] SIMULATE MODE — no real touch input");
        Ok(Self {
            sample: TouchSample::default(),
        })
    }

    #[cfg(not(feature = "simulate"))]
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        // (existing body unchanged up to the EVIOCGRAB block, then:)
        Ok(Self {
            fd,
            sample: TouchSample::default(),
        })
    }

    /// Read every pending event and return the latest touch in LVGL
    /// coordinates. Call once per loop iteration.
    pub fn poll(&mut self) -> TouchSample {
        #[cfg(not(feature = "simulate"))]
        // SAFETY: `fd` is the open, non-blocking evdev file; each read fills
        // at most one `InputEvent`.
        unsafe {
            let mut ev = std::mem::zeroed::<InputEvent>();
            let sz = std::mem::size_of::<InputEvent>();
            loop {
                let n = libc::read(self.fd, &mut ev as *mut _ as *mut libc::c_void, sz);
                if n < sz as isize {
                    break;
                }
                match ev.r#type {
                    EV_KEY if ev.code == BTN_TOUCH => self.sample.pressed = ev.value != 0,
                    EV_ABS if ev.code == ABS_MT_POSITION_X => self.sample.x = flip_x(ev.value),
                    EV_ABS if ev.code == ABS_MT_POSITION_Y => self.sample.y = flip_y(ev.value),
                    _ => {}
                }
            }
        }
        self.sample
    }
}
```

The non-simulate `init` keeps the existing open / EVIOCGNAME / EVIOCGRAB code verbatim (lines 36–80 today); only its final `return Ok(Self { fd, x: 0, y: 0, pressed: false });` becomes the tail expression shown above, which also removes clippy's `needless_return`.

`runtime/device.rs` (above the tests):

```rust
//! LVGL display and pointer registration for the Wave panel. With
//! `runtime/gesture.rs` and `runtime::load`, the only runtime code that calls
//! LVGL directly.

use crate::bindings::*;
use crate::display::{self, FramebufferDisplay};
use crate::kit::obj::Obj;
use crate::runtime::touch::TouchSample;
use std::cell::Cell;

pub struct Device {
    indev: *mut lv_indev_t,
    touch: &'static Cell<TouchSample>,
}

impl Device {
    /// `lv_init`, the framebuffer display and the touch pointer. Call once,
    /// on the UI thread.
    pub fn init(fb: FramebufferDisplay) -> Self {
        // SAFETY: called once before any other LVGL call on this thread; the
        // framebuffer and the touch cell are leaked, so LVGL's user-data
        // pointers stay valid for the life of the process.
        unsafe {
            lv_init();
            let disp = lv_display_create(display::SCREEN_W as i32, display::SCREEN_H as i32);
            lv_display_set_color_format(disp, lv_color_format_t_LV_COLOR_FORMAT_RGB565);
            lv_display_set_flush_cb(disp, Some(flush_cb));
            let buf1 = fb.buf1.as_ptr() as *mut _;
            let buf2 = fb.buf2.as_ptr() as *mut _;
            let len = fb.buf1.len() as u32;
            lv_display_set_user_data(disp, Box::into_raw(Box::new(fb)) as *mut _);
            lv_display_set_buffers(
                disp,
                buf1,
                buf2,
                len,
                lv_display_render_mode_t_LV_DISPLAY_RENDER_MODE_PARTIAL,
            );
            let touch: &'static Cell<TouchSample> =
                Box::leak(Box::new(Cell::new(TouchSample::default())));
            let indev = lv_indev_create();
            lv_indev_set_type(indev, lv_indev_type_t_LV_INDEV_TYPE_POINTER);
            lv_indev_set_read_cb(indev, Some(touch_read_cb));
            lv_indev_set_user_data(indev, touch as *const Cell<TouchSample> as *mut _);
            release_top_layer();
            Self { indev, touch }
        }
    }

    /// What LVGL reads at its next pointer poll.
    pub fn report(&self, sample: TouchSample) {
        self.touch.set(sample);
    }

    /// A gesture took this press: LVGL sends PRESS_LOST on release, no click.
    pub fn cancel_press(&self) {
        // SAFETY: the pointer device created in `init`.
        unsafe { lv_indev_wait_release(self.indev) };
    }

    /// The loaded screen (null before the first one).
    pub fn active_screen(&self) -> Obj {
        // SAFETY: LVGL returns the live active screen or null.
        unsafe { Obj::from_raw(lv_screen_active()) }
    }

    pub fn tick(&self, ms: u32) {
        // SAFETY: plain LVGL call on the UI thread.
        unsafe { lv_tick_inc(ms) };
    }

    pub fn run_timers(&self) {
        // SAFETY: plain LVGL call on the UI thread; handlers only push to channels.
        unsafe { lv_timer_handler() };
    }
}

/// `lv_layer_top()` must never catch taps, or nothing below it is reachable
/// (LVGL 9.2 already clears the flag; this keeps it cleared).
pub(crate) fn release_top_layer() {
    // SAFETY: the default display's top layer is live once a display exists.
    unsafe { lv_obj_remove_flag(lv_layer_top(), lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE) };
}

unsafe extern "C" fn flush_cb(disp: *mut lv_display_t, area: *const lv_area_t, px_map: *mut u8) {
    let fb = lv_display_get_user_data(disp) as *mut FramebufferDisplay;
    if !fb.is_null() {
        (*fb).flush(&*area, px_map as *const u8);
    }
    lv_display_flush_ready(disp);
}

unsafe extern "C" fn touch_read_cb(indev: *mut lv_indev_t, data: *mut lv_indev_data_t) {
    let touch = lv_indev_get_user_data(indev) as *const Cell<TouchSample>;
    let sample = if touch.is_null() {
        TouchSample::default()
    } else {
        (*touch).get()
    };
    (*data).point.x = sample.x;
    (*data).point.y = sample.y;
    (*data).state = if sample.pressed {
        lv_indev_state_t_LV_INDEV_STATE_PRESSED
    } else {
        lv_indev_state_t_LV_INDEV_STATE_RELEASED
    };
}
```

Add `pub mod device;` to `runtime/mod.rs`. The legacy `lib.rs` still calls `(*touch_ptr).poll()` and reads `touch.x/.y/.pressed` in its own `touch_read_cb`; change those two places to use the returned sample so the legacy loop keeps compiling until Z8:

```rust
        unsafe {
            (*touch_ptr).poll();
        }
```
becomes `let _ = unsafe { (*touch_ptr).poll() };`, and the legacy `touch_read_cb` body becomes:

```rust
    let touch = lv_indev_get_user_data(indev) as *mut input::TouchInput;
    if touch.is_null() {
        (*data).state = lv_indev_state_t_LV_INDEV_STATE_RELEASED;
        return;
    }
    let sample = (*touch).poll();
    (*data).point.x = sample.x;
    (*data).point.y = sample.y;
    (*data).state = if sample.pressed {
        lv_indev_state_t_LV_INDEV_STATE_PRESSED
    } else {
        lv_indev_state_t_LV_INDEV_STATE_RELEASED
    };
```

(`TouchInput` keeps the latest sample, so polling twice per iteration is harmless; Z8 deletes this code.)

- [ ] **Step 4: Run and see it pass**

Run: `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | tail -3`
Expected: `test result: ok.`

Run: `cargo check -p waved-ui --features simulate`
Expected: `Finished`.

Run: `cargo clippy -p waved-ui --all-targets 2>&1 | grep -E -- "--> crates/waved-ui/src/(kit|screens_v2|runtime|input)"`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-ui/src/input.rs crates/waved-ui/src/runtime crates/waved-ui/src/lib.rs
git commit -m "feat(ui): add the runtime's device layer and pinned touch mapping

- TouchInput::poll returns a TouchSample; flip_x/flip_y pin native pixels
  and the 180 degree flip
- runtime::device registers the framebuffer and pointer, reports gated
  samples, cancels presses taken by gestures, keeps the top layer unclickable

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task Z8: Switch `UiService` to the runtime and wire the power key

**Files:**
- Create: `crates/waved-ui/src/runtime/run.rs`
- Modify: `crates/waved-ui/src/runtime/power.rs` (E adapter), `crates/waved-ui/src/runtime/mod.rs` (`pub mod run;`), `crates/waved-ui/src/lib.rs`, `crates/waved-ui/Cargo.toml`
- Modify: `crates/waved/Cargo.toml`, `crates/waved/src/main.rs`
- Delete: `crates/waved-ui/src/upload_notice.rs`, `crates/waved-ui/src/integrity_notice.rs`

**Interfaces:**
- Consumes: everything from Z1–Z7; workstream E's API (table above).
- Produces:
  - `pub struct UiInputs { pub power_keys: Option<std::sync::mpsc::Receiver<PowerKey>> }` (`Default`)
  - `UiService::new(bus: Arc<StateBus>, config: WavedConfig, inputs: UiInputs) -> UiService`
  - `pub fn run::run(bus: Arc<StateBus>, config: WavedConfig, inputs: UiInputs) -> Result<(), Box<dyn std::error::Error>>`
  - `impl PowerPolicy for waved_system::display::DisplayPower`, `impl Backlight for waved_system::display::SysfsBacklight`, `pub fn power::device_driver(now_ms: u64) -> PowerDriver`

- [ ] **Step 1: Reconcile with workstream E.** Read `crates/waved-system/src/` (`display`, `power_key`, `time` or their equivalents) and compare with "Interfaces consumed from workstream E". Write down each difference; the code in Steps 3–5 uses the table's names, so substitute E's real names there. If E has no `sleep` or no keep-lit input, stop and raise it with E's owner; do not add timers to `waved-ui`.

Also check whether E's own contract request already added `waved-system` to `crates/waved/Cargo.toml` or spawns the power key reader in `main.rs`; if it does, reuse that reader's receiver instead of spawning a second one in Step 5.

- [ ] **Step 2: Write the failing adapter test** (append to `power.rs` tests):

```rust
    #[test]
    fn workstream_e_timers_follow_the_spec_through_the_adapter() {
        use waved_system::display::DisplayPower;
        let settings = SettingsState::default(); // dim 120 s, off 600 s
        let mut timers = DisplayPower::new(0);
        assert_eq!(PowerPolicy::tick(&mut timers, 0, &settings, false), Power::On);
        assert_eq!(PowerPolicy::tick(&mut timers, 120_000, &settings, false), Power::Resting);
        assert_eq!(PowerPolicy::tick(&mut timers, 600_000, &settings, true), Power::Resting, "recording: resting only");
        assert_eq!(PowerPolicy::tick(&mut timers, 600_000, &settings, false), Power::Off);
        assert_eq!(PowerPolicy::backlight_pct(&timers, &settings), 0);
        PowerPolicy::activity(&mut timers, 600_010);
        assert_eq!(PowerPolicy::tick(&mut timers, 600_010, &settings, false), Power::On);
    }
```

Run: `cargo test -p waved-ui --lib workstream_e 2>&1 | grep -m2 error`
Expected: `use of undeclared crate or module \`waved_system\``.

- [ ] **Step 3: Implement the adapter.** `crates/waved-ui/Cargo.toml` `[dependencies]`: add `waved-system = { path = "../waved-system" }`. Append to `power.rs` (above the tests):

```rust
use waved_system::display::{DisplayPower, ScreenState, SysfsBacklight};

impl PowerPolicy for DisplayPower {
    fn activity(&mut self, now_ms: u64) {
        DisplayPower::touch(self, now_ms);
    }

    fn sleep(&mut self, now_ms: u64) {
        DisplayPower::sleep(self, now_ms);
    }

    fn tick(&mut self, now_ms: u64, settings: &SettingsState, keep_lit: bool) -> Power {
        match DisplayPower::tick(self, now_ms, settings, keep_lit) {
            ScreenState::On => Power::On,
            ScreenState::Dim => Power::Resting,
            ScreenState::Off => Power::Off,
        }
    }

    fn backlight_pct(&self, settings: &SettingsState) -> u8 {
        DisplayPower::backlight_pct(self, settings)
    }
}

impl Backlight for SysfsBacklight {
    fn set_pct(&mut self, pct: u8) {
        if let Err(error) = SysfsBacklight::set_pct(self, pct) {
            tracing::warn!("backlight {pct}%: {error}");
        }
    }
}

/// The device's driver: workstream E's timers and the sysfs backlight.
pub fn device_driver(now_ms: u64) -> PowerDriver {
    PowerDriver::new(
        Box::new(DisplayPower::new(now_ms)),
        Box::new(SysfsBacklight::device()),
    )
}
```

Run: `set -o pipefail; cargo test -p waved-ui --lib runtime::power 2>&1 | tail -2`
Expected: `test result: ok. 5 passed`. A failure means E's timers disagree with the spec timings (2 min rest, 10 min off, resting only while recording): raise it with E.

- [ ] **Step 4: The UI thread loop.** Create `runtime/run.rs`:

```rust
//! The UI thread: device setup and the loop that feeds the runtime.

use crate::audio_fx;
use crate::display::FramebufferDisplay;
use crate::input::TouchInput;
use crate::kit::snapshot::UiSnapshot;
use crate::runtime::device::Device;
use crate::runtime::feedback;
use crate::runtime::gesture::{drag_owner_at, gesture_event, GestureTracker};
use crate::runtime::power;
use crate::runtime::touch::WakeGate;
use crate::runtime::{registry, Runtime};
use crate::UiInputs;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast::error::TryRecvError;
use tracing::{info, warn};
use waved_config::WavedConfig;
use waved_state::StateBus;

const LOOP_SLEEP: Duration = Duration::from_millis(10);

pub fn run(
    bus: Arc<StateBus>,
    config: WavedConfig,
    inputs: UiInputs,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("UI thread starting");
    let device = Device::init(FramebufferDisplay::init()?);
    let mut touch = TouchInput::init()?;
    let mut events_rx = bus.events.subscribe();
    let mut commands_rx = bus.commands.subscribe();
    let mut runtime = Runtime::new(
        bus.commands.clone(),
        Arc::new(config),
        Box::new(registry::screen_for),
    );
    let dispatch = runtime.dispatcher();
    let mut power = power::device_driver(0);
    let mut gate = WakeGate::default();
    let mut gestures = GestureTracker::default();
    let start = Instant::now();
    let mut last_tick = start;

    loop {
        device.tick(last_tick.elapsed().as_millis() as u32);
        last_tick = Instant::now();
        let now_ms = start.elapsed().as_millis() as u64;
        let snap = UiSnapshot::capture(
            &bus,
            unix_now(),
            now_ms,
            waved_system::time::utc_offset_minutes,
        );

        // Touch: wake guard first, then gestures, then LVGL.
        let raw = touch.poll();
        let sample = gate.filter(raw, power.asleep());
        if raw.pressed {
            power.touch(now_ms);
        }
        let root = device.active_screen();
        if let Some(gesture) = gestures.sample(sample, now_ms, |x, y| drag_owner_at(root, x, y)) {
            if let Some(event) = gesture_event(gesture, runtime.current(), runtime.depth()) {
                device.cancel_press();
                dispatch.ui(event);
            }
        }
        device.report(sample);

        // Power key and timers.
        if let Some(keys) = &inputs.power_keys {
            while let Ok(key) = keys.try_recv() {
                if let Some(event) = power.key(key, now_ms) {
                    dispatch.ui(event);
                }
            }
        }
        if let Some(event) = power.step(now_ms, &snap) {
            dispatch.ui(event);
        }

        // Bus traffic.
        loop {
            match commands_rx.try_recv() {
                Ok(command) => {
                    runtime.observe_command(&command);
                    if let Some(sound) = feedback::sound_for(&command, &snap) {
                        audio_fx::play(sound);
                    }
                }
                Err(TryRecvError::Lagged(missed)) => warn!("UI missed {missed} commands"),
                Err(_) => break,
            }
        }
        let mut events = Vec::new();
        loop {
            match events_rx.try_recv() {
                Ok(event) => events.push(event),
                Err(TryRecvError::Lagged(missed)) => warn!("UI missed {missed} device events"),
                Err(_) => break,
            }
        }

        runtime.step(snap, &events);
        device.run_timers();
        std::thread::sleep(LOOP_SLEEP);
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
```

Add `pub mod run;` to `runtime/mod.rs`.

- [ ] **Step 5: Replace `lib.rs` and wire `main.rs`.** New `crates/waved-ui/src/lib.rs`:

```rust
pub mod audio_fx;
pub mod bindings;
pub mod display;
pub mod i18n;
pub mod input;
pub mod kit;
pub mod runtime;
pub mod screens_v2;

// Legacy UI, no longer run; deleted in the next commit.
pub mod nav;
pub mod screens;
pub mod status_bar;
pub mod theme;
pub mod toast;

use runtime::power::PowerKey;
use std::sync::{mpsc, Arc};
use tracing::error;
use waved_config::WavedConfig;
use waved_state::StateBus;

/// What the UI reads besides the state bus.
#[derive(Default)]
pub struct UiInputs {
    /// Power key presses (workstream E's reader, wired in `main.rs`).
    pub power_keys: Option<mpsc::Receiver<PowerKey>>,
}

pub struct UiService {
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl UiService {
    pub fn new(bus: Arc<StateBus>, config: WavedConfig, inputs: UiInputs) -> Self {
        let handle = std::thread::Builder::new()
            .name("ui-lvgl".into())
            .spawn(move || {
                if let Err(e) = runtime::run::run(bus, config, inputs) {
                    error!("UI thread error: {e}");
                }
            })
            .expect("failed to spawn ui-lvgl thread");
        Self {
            thread_handle: Some(handle),
        }
    }

    pub fn join(mut self) {
        if let Some(h) = self.thread_handle.take() {
            let _ = h.join();
        }
    }
}
```

Delete `crates/waved-ui/src/upload_notice.rs` and `crates/waved-ui/src/integrity_notice.rs` (replaced by `runtime::notices`; they were private modules, so they would turn into dead code).

`crates/waved/Cargo.toml` `[dependencies]`: add `waved-system = { path = "../waved-system" }` if Step 1 found it missing.

`crates/waved/src/main.rs`: next to `CONFIG_PATH` add

```rust
/// Power key (E2): `vad_keypad`, which the stock firmware never read.
#[cfg(all(feature = "ui", not(feature = "simulate")))]
const POWER_KEY_DEVICE: &str = "/dev/input/event0";
```

and replace the `#[cfg(feature = "ui")]` block (lines 681–688 today) with:

```rust
    #[cfg(feature = "ui")]
    {
        use waved_ui::runtime::power::PowerKey;
        use waved_ui::{UiInputs, UiService};
        info!("Starting UI service");
        let (key_tx, key_rx) = std::sync::mpsc::channel::<PowerKey>();
        #[cfg(not(feature = "simulate"))]
        {
            use waved_system::power_key::{spawn_reader, KeyPress};
            if let Err(e) = spawn_reader(POWER_KEY_DEVICE, move |press| {
                let key = match press {
                    KeyPress::Short => PowerKey::Short,
                    KeyPress::Long => PowerKey::Long,
                };
                let _ = key_tx.send(key);
            }) {
                warn!("power key reader not started: {e}");
            }
        }
        #[cfg(feature = "simulate")]
        drop(key_tx);
        let ui = UiService::new(
            Arc::clone(&bus),
            config.clone(),
            UiInputs {
                power_keys: Some(key_rx),
            },
        );
        // Drop ui without joining — the thread runs until process exit.
        std::mem::forget(ui);
    }
```

- [ ] **Step 6: Run and see it pass**

Run: `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | tail -3`
Expected: `test result: ok.`

Run: `cargo check -p waved-ui --features simulate`
Expected: `Finished`.

Run: `set -o pipefail; scripts/linux-test.sh check -p waved --features ui,capture-alsa 2>&1 | tail -2 && scripts/linux-test.sh check -p waved --features ui,simulate 2>&1 | tail -2`
Expected: `Finished` twice.

Run: `cargo clippy -p waved-ui --all-targets 2>&1 | grep -E -- "--> crates/waved-ui/src/(kit|screens_v2|runtime|input|lib)"`
Expected: no output.

- [ ] **Step 7: Commit**

```bash
git add -A crates/waved-ui crates/waved
git commit -m "feat(ui): run the revision-2 runtime instead of the legacy loop

- UiService spawns runtime::run: snapshot, wake guard, gestures, power,
  sounds, device-event messages, then Runtime::step each iteration
- workstream E's display timers and backlight behind PowerDriver
- main.rs passes power key presses to the UI through UiInputs
- the legacy upload and integrity toasts are replaced by runtime::notices

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task Z9: Delete the legacy UI, the navigation commands, PNG assets and Montserrat

**Files:**
- Delete: `crates/waved-ui/src/screens/` (all, including `control_center.rs`), `crates/waved-ui/src/{nav,theme,toast,status_bar}.rs`, `crates/waved-ui/assets/`
- Modify: `crates/waved-ui/src/lib.rs`, `crates/waved-ui/src/display.rs`, `crates/waved-ui/lv_conf.h`, `crates/waved-ui/src/kit/README.md`, `crates/waved-ui/src/runtime/device.rs` (test), `crates/waved-state/src/lib.rs`, `docs/DISPLAY_HARDWARE.md`, `../TODO.md`

**Interfaces:**
- Consumes: nothing new.
- Produces: `Command` without `OpenSettings`, `CloseSettings`, `OpenAudioMode`, `OpenNoiseReduction`, `OpenSystemSettings`, `OpenWebhookStatus`, `OpenVolume`, `OpenWifiSettings`; LVGL's default font is `font_body_15`.

- [ ] **Step 1: Write the failing test** (append to `runtime/device.rs` tests):

```rust
    #[test]
    fn the_default_font_is_the_kit_body_font() {
        lvgl(|_lv| {
            // SAFETY: returns a static font descriptor.
            let default = unsafe { lv_font_default() };
            assert_eq!(default, crate::kit::fonts::Font::Body.ptr(), "no Montserrat left");
        });
    }
```

Run: `cargo test -p waved-ui --lib default_font 2>&1 | tail -3`
Expected: FAIL (`left` is `lv_font_montserrat_16`).

- [ ] **Step 2: Delete the legacy files**

```bash
git rm -r crates/waved-ui/src/screens crates/waved-ui/assets
git rm crates/waved-ui/src/nav.rs crates/waved-ui/src/theme.rs crates/waved-ui/src/toast.rs crates/waved-ui/src/status_bar.rs
```

In `lib.rs`, delete the five legacy `pub mod` lines and their comment.

- [ ] **Step 3: `lv_conf.h`.** Replace lines 63–68 (the Montserrat block and `LV_FONT_DEFAULT`) with:

```c
/* Kit fonts (src/kit/fonts, built by fonts/build_fonts.sh). Every kit label
 * sets its font; the default only covers LVGL's own fallbacks. */
#define LV_FONT_CUSTOM_DECLARE LV_FONT_DECLARE(font_body_15)
#define LV_FONT_DEFAULT &font_body_15
```

Replace the PNG / file-system block (`#define LV_USE_LODEPNG 1` through the `#endif` of `LV_USE_FS_POSIX`) with:

```c
#define LV_USE_LODEPNG 0  /* no PNG assets: icons are the kit icon font */
#define LV_USE_QRCODE  1  /* Wi-Fi setup QR code */
#define LV_USE_FS_POSIX 0 /* no asset files are read */
```

- [ ] **Step 4: `display.rs` clippy fixes.** Delete `const STRIDE` (unused). Split `init` into two cfg'd functions so neither needs `return`:

```rust
    #[cfg(feature = "simulate")]
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        let (buf1, buf2) = Self::draw_buffers();
        Ok(Self { buf1, buf2 })
    }

    #[cfg(not(feature = "simulate"))]
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        let (buf1, buf2) = Self::draw_buffers();
        // Kick the fb_ili79400 SPI refresh using dd — the only method
        // proven to reliably wake the display at boot.
        let _ = std::process::Command::new("dd")
            .args(["if=/dev/zero", "of=/dev/fb0", "bs=4096", "count=60"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/fb0")?;
        let fd = std::os::unix::io::IntoRawFd::into_raw_fd(file);
        // Backlight on until the power driver applies the brightness setting.
        let _ = std::fs::write("/sys/class/leds/lcd-backlight/brightness", "255");
        Ok(Self { fd, buf1, buf2 })
    }

    /// Two half-screen RGB565 draw buffers.
    fn draw_buffers() -> (Box<[u8]>, Box<[u8]>) {
        let size = (SCREEN_W * SCREEN_H / 2) as usize * BYTES_PER_PIXEL;
        (
            vec![0u8; size].into_boxed_slice(),
            vec![0u8; size].into_boxed_slice(),
        )
    }
```

- [ ] **Step 5: Delete the navigation commands.** In `crates/waved-state/src/lib.rs`, delete the variants `OpenSettings`, `CloseSettings`, `OpenAudioMode`, `OpenNoiseReduction`, `OpenSystemSettings`, `OpenWebhookStatus`, `OpenVolume`, `OpenWifiSettings` with their doc comments, and re-document the two provisioning commands that `waved-provision` consumes:

```rust
    /// Start Wi-Fi provisioning: scan, bring up the setup access point and
    /// serve the setup page (waved-provision). Sent by the Wi-Fi setup screen.
    OpenWifiProvisioning,
```

```rust
    /// Abort provisioning and return to station mode (waved-provision).
    CancelWifiProvisioning,
```

Run: `grep -rnE "OpenSettings|CloseSettings|OpenAudioMode|OpenNoiseReduction|OpenSystemSettings|OpenWebhookStatus|OpenVolume|OpenWifiSettings" crates scripts`
Expected: no output.

- [ ] **Step 6: Docs.**
- `kit/README.md` line 4–5: replace "The legacy screens in `src/screens/` are deleted when the runtime switches over." with "The runtime (`src/runtime/`) builds them by route; the legacy screens are gone."
- `docs/DISPLAY_HARDWARE.md`, "LVGL Configuration" table: replace the `LV_USE_LODEPNG`, `LV_USE_FS_POSIX` and `LV_FONT_POSIX_LETTER` rows with
  `| \`LV_USE_LODEPNG\`, \`LV_USE_FS_POSIX\` | \`0\` | No PNG assets since the milestone 2 runtime switch: icons come from the kit icon font |` and
  `| \`LV_FONT_DEFAULT\` | \`&font_body_15\` | The kit body font; Montserrat is not built |`,
  and delete the line "Call `lv_fs_posix_init()` immediately after `lv_init()`." Above "## Asset Paths on Device" add: "These stock pspapp assets are no longer used by waved (milestone 2); the table stays as a record of the stock firmware."
- `../TODO.md`: `- [ ] **P0.5 Navigation**` → `- [x] **P0.5 Navigation**`; `- [ ] **L3 Dead code**` → `- [x] **L3 Dead code**`; in the Known bugs table append ` — fixed by the M2 runtime switch` to the "Where" cell of BUG-3, BUG-5, BUG-11 and BUG-13.

- [ ] **Step 7: Run and see it pass**

Run: `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | tail -3`
Expected: `test result: ok.` and `git status --short crates/waved-ui/golden` prints nothing (a changed golden means a kit label relied on the default font: give that label its font explicitly, never update the golden).

Run: `set -o pipefail; for c in waved-state waved-provision waved-settings; do cargo test -p $c --lib 2>&1 | tail -1; done`
Expected: three `test result: ok.` lines.

Run: `cargo clippy -p waved-ui --all-targets -- -D warnings && cargo clippy -p waved-state --all-targets -- -D warnings`
Expected: `Finished` with no warnings (the crate-wide gate holds from now on).

Run: `cargo check -p waved-ui --features simulate`
Expected: `Finished`.

Run: `set -o pipefail; scripts/linux-test.sh check --workspace 2>&1 | tail -2 && scripts/linux-test.sh check -p waved --features ui,capture-alsa 2>&1 | tail -2 && scripts/linux-test.sh test -p waved 2>&1 | tail -2`
Expected: `Finished`, `Finished`, `test result: ok.`

- [ ] **Step 8: Commit**

```bash
git add -A crates/waved-ui crates/waved-state docs/DISPLAY_HARDWARE.md ../TODO.md
git commit -m "refactor(ui): delete the legacy UI and the navigation commands

- remove src/screens (incl. control_center), nav, theme, toast, status_bar
- remove the eight UI-only Open*/CloseSettings commands (BUG-11, BUG-13)
- keep OpenWifiProvisioning/CancelWifiProvisioning, which waved-provision uses
- no PNG assets, LodePNG or POSIX fs; Montserrat replaced by font_body_15
- clippy clean across waved-ui

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task Z10: Milestone 2 device pass (D6)

The one integration pass on the device for milestone 2 (D6), done with the user, who has the device. Covers TODO's milestone-2 row ("Home, Recording, menus; pause/resume; brightness; power key") plus everything this plan added.

**Files:**
- Modify: `docs/DISPLAY_HARDWARE.md` (touch gestures section), `../TODO.md` (findings)
- Possibly modify: `crates/waved-ui/src/runtime/gesture.rs` (threshold constants only, with the user)

- [ ] **Step 1: Build and deploy the test-hook build** (from `waved/`; the hooks inject recordings and faults):

```bash
cross build --release --target aarch64-unknown-linux-gnu \
  --features "ui,capture-alsa,recording-integrity-test-hooks" -p waved
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "killall waved 2>/dev/null; sleep 2; (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &)"
inject() { adb shell "printf '%s\n' '$1' > /tmp/waved-integrity-fault; kill -HUP \$(pidof waved)"; }
```

Expected: the start-up ring, then Home (or First start on a fresh config) within about 2 s; `adb shell grep -c "UI thread starting" /tmp/waved.log` prints `1`; `adb shell grep -i panic /tmp/waved.log` prints nothing.

- [ ] **Step 2: Touch reaches the right place.** Tap Quieter (left side button) and Louder (right): the dial gauge goes down / up. Tap the status pill: Menu. On Menu, scroll to the last row and tap it; tap Back at the bottom centre.
Expected: every tap lands on what is under the finger, including the bottom main button and the Back pill on the bottom curve.

- [ ] **Step 3: Gestures.**
  - Home: swipe down starting on the status pill → Menu, and the pill does not also fire (Menu opens once).
  - Recording (`inject start`): swipe down from the top → Menu.
  - Home: swipe down starting in the middle of the dial → nothing, and the mic does not mute.
  - Menu: swipe right → Home. Home: swipe right → nothing.
  - Volume: drag the arc knob over the top from left to right → the value changes and the screen stays.
  - Audio mode (Focused): drag the aim to the right → the aim moves and the screen stays.
  Expected: as listed. If swipes are hard to trigger or fire by accident, tune `SWIPE_MIN_PX` / `SWIPE_MAX_MS` / `TOP_EDGE_Y` with the user, rebuild, and rerun this step.

- [ ] **Step 4: Messages.**
  - While recording, tap Stop briefly → "Hold Stop to end the recording" in the status slot; while it shows, tap Mark → the mark is added (messages never take taps); it disappears after 4 s if a warning, 2 s if info.
  - Idle: `inject wav-write-failed` → "Can't record, check storage" in red for 4 s.
  - With a webhook configured and Wi-Fi up: record, stop, Send → "Recording sent". With Wi-Fi off: Send → "No Wi-Fi, upload waiting".
  Expected: as listed; each message also stays when navigating to another screen until its time is up.

- [ ] **Step 5: Navigation follows the meeting (BUG-11, BUG-13).**
  - Menu → Audio mode → Back → Menu (BUG-11).
  - `inject start`; open Menu → Volume; Back twice → Recording.
  - Upload mode "Ask me each time": with Volume open during a recording, `inject stop` → "Send this recording?" appears at once (BUG-13). Upload mode "Send automatically": Back from Volume lands on Home.
  - Pause and Resume on Recording: the Paused state (amber) and back (C1).
  Expected: as listed.

- [ ] **Step 6: Resting, off and the wake guard.**

```bash
adb shell "sed -i 's/^dim_after_s = .*/dim_after_s = 15/; s/^off_after_s = .*/off_after_s = 60/' /data/waved/config.toml"
adb shell "killall waved"   # the S60waved watchdog restarts it within ~3 s
```

  - Wait 15 s without touching → the resting screen, backlight dimmer (`adb shell cat /sys/class/leds/lcd-backlight/brightness` lower than at Home).
  - Tap exactly where Record is → Home comes back and no recording starts.
  - Wait 60 s → backlight `0`. Tap → Home, nothing else happens.
  - `inject start`, wait 70 s → resting, never `0` (recording keeps it lit). `inject stop`.
  Then restore: `adb shell "sed -i 's/^dim_after_s = .*/dim_after_s = 120/; s/^off_after_s = .*/off_after_s = 600/' /data/waved/config.toml"`.

- [ ] **Step 7: Brightness and the power key.**
  - Display screen: change brightness → the backlight value follows within a moment.
  - Power key short press → resting/off; short again → wakes; hold 2 s → Power off confirmation; Cancel → back where it was; swipe right on the confirmation also cancels.
  Expected: as listed.

- [ ] **Step 8: No PIN in M2, sounds.**
  - `adb shell "sed -i 's/^device_password = .*/device_password = \"4821\"/' /data/waved/config.toml; killall waved"` → Files, Wi-Fi, Uploads and System open without a PIN. Restore `device_password = ""` and restart.
  - Tap the dial: mute / unmute sounds play. Turn Alert tones off in System: no sound. Turn them back on.

- [ ] **Step 9: Record and restore.** Add a "Touch gestures" subsection to `docs/DISPLAY_HARDWARE.md` under the touch notes with the final `TOP_EDGE_Y`, `SWIPE_MIN_PX`, `SWIPE_MAX_MS` values and what was observed (for example how fast a natural swipe is on this panel). Write every failure into `../TODO.md` under the owning workstream with the step number. Deploy the normal build:

```bash
cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "killall waved"
```

Expected: Home within about 3 s; `adb shell grep -E "dim_after_s|off_after_s|device_password" /data/waved/config.toml` shows `120`, `600` and `""`.

- [ ] **Step 10: Commit**

```bash
git add docs/DISPLAY_HARDWARE.md ../TODO.md crates/waved-ui/src/runtime/gesture.rs
git commit -m "docs(display): record the milestone 2 device pass

- gestures, wake guard, messages and power states checked on the panel
- final swipe thresholds and what was observed
- open findings filed under their workstreams in TODO.md

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Self-review

- **Spec coverage:** the six bullets of the M1 plan's "End of milestone 2: runtime switch" map to Z5 (Router + registry + Dispatcher + mpsc), Z8 (snapshot per iteration), Z1/Z2 (device events → messages), Z3/Z8 (gestures, wake guard), Z4/Z8 (E's power controller via Rest/Wake and touch), Z9 (deletions, `LV_FONT_DEFAULT`), Z10 (device pass). M2.0 is S1 (+ S2, the drag convention the gestures need). TODO P0.5's four bullets: stack and Back (Router, Z5 tests), gestures (Z3), wake guard (Z3/Z4), meeting routing from every screen (Z5 BUG-13 test).
- **Types against the real APIs read:** `Router::{new, handle(&UiEvent, &UiSnapshot), sync, current, depth}`, `Dispatcher::new(broadcast::Sender<Command>, mpsc::Sender<UiEvent>)`, `ScreenCtx { dispatch, config: Arc<WavedConfig> }`, `Screen::{build(&mut self, Obj, &ScreenCtx, &UiSnapshot), update}`, `StatusArea::{plain, show(&Message, u64), update, message, cluster}`, `UiEvent` (Clone, Debug, PartialEq), `Message` (Eq), `Severity::duration_ms`, `Obj::{screen, plain, label, flag, has_flag, is_hidden, is_null, child, child_count, text, delete, from_raw}`, `testkit::{lvgl, Lvgl::screen, assert_targets_ok}`, `text_width(&str, Font)`, `Font::{Body, Small}.ptr()`, `MESSAGE_MAX_W`, `DeviceEvent` / `RecordingIntegrityFault::{requires_restart, blocks_recording}`, `PendingDecision`, `RecordingSummary`, `SettingsState` defaults (brightness 80, dim 120 s, off 600 s, tones on).
- **Known judgement calls** (flagged for the reviewer): copy for device-event messages is new ("Recording sent" and "No Wi-Fi, upload waiting" come from the mockup; the rest follow its rules); feedback sounds are gated by the Alert tones switch; `BOOT_MIN_MS` 1.5 s is a runtime floor the boot screen can shorten; the PIN is masked in the runtime rather than by clearing the setting.
