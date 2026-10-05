# Display Redesign — Milestone 2, Workstream B: Settings Screens — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build every settings screen of the revision-2 design as a host-tested `kit::screen::Screen`: Menu, Audio mode (with a new radial mode-picker kit component and drag-to-aim), Noise reduction, Volume and Mic level, System with Display / Dim after / Date and time / Time zone / About, the shared confirmation screen, Wi-Fi and Wi-Fi setup, Uploads and After a recording. Each screen gets a golden compared with its mockup PNG and passes `assert_targets_ok`. Nothing is wired into the runtime: the integrator's runtime switch registers the screens at the end of milestone 2.

**Architecture:**
- One file per screen in `crates/waved-ui/src/screens_v2/` (files listed under "File Structure"). A file may hold a second screen that only it opens (`volume.rs`: Mic level; `display.rs`: Dim after; `datetime.rs`: Time zone).
- Screens read live values from `UiSnapshot` (`snap.audio`, `snap.settings`, `snap.wifi`, `snap.provisioning`, `snap.meeting`, `snap.files`), send service commands through `ctx.dispatch.command(..)` and navigate with `ctx.dispatch.navigate(..)` / `.back()`.
- Optimistic UI: a tap or drag updates the screen at once and keeps the chosen value until the snapshot reports it. A stale snapshot never overrides what the user just chose, and a pressed control is never moved by the snapshot.
- New kit code lives only in the two B-owned component files. `kit/components/mode_picker.rs` holds the radial picker: six round buttons, a canvas preview with Rust-side fill and LVGL layer strokes, and the drag-to-aim input. `kit/components/wifi.rs` holds the signal arcs and the QR panel, because screens may not call raw LVGL.
- Pure helpers carry the logic and are unit-tested without LVGL: pattern geometry, aim angle, RGB565 blending, dash splitting, level steps, confirmation outcomes, the Wi-Fi setup view model, RFC 3339 parsing, relative times, civil dates, time-zone choices.

**Tech Stack:** Rust 2021, LVGL 9.2 through bindgen (`lv_canvas`, `lv_draw_line`, `lv_draw_arc`, `lv_qrcode`, `lv_arc`), the kit testkit (headless LVGL, `png` 0.17 goldens).

**Spec:**
- `../docs/mockups/README.md`, `../docs/mockups/index.html` and `../docs/mockups/png/*.png` (commit `c244c9f`, approved on the panel). These are authoritative over the TODO wording.
- `../TODO.md`: decisions D1–D10 and items B1–B8.
- `crates/waved-ui/src/kit/README.md`: kit rules and component API.
- `docs/superpowers/plans/2026-10-04-display-redesign-m1-foundation.md` and `docs/superpowers/plans/2026-10-05-display-relayout-kit.md`.

All code paths are relative to `waved/`. Paths starting with `../` point into the monorepo root.

---

## Scope

| TODO item | Covered here | Left out |
|---|---|---|
| **B1 Menu** | Rotary list with live values: Files, Audio mode, Noise reduction, Volume, Wi-Fi, Uploads, System. A lock icon marks PIN rows when a PIN is set. | Mic height (B5, needs F3) and Bluetooth (I1–I4) rows stay hidden until milestone 3. PIN *enforcement* is K2 (milestone 3); the lock is only a marker. |
| **B2 Audio mode** | `ModePicker` kit component; six modes; live preview on `lv_canvas`; drag to aim Focused / Narrow / Two sides (`SetBeamAim`); live DoA overlay for Follow talker. | The audio backends: C3 fixed aim and C10 applying `SetBeamAim`. This screen only relies on the existing command contract and `AudioState.beam_aim_deg`. |
| **B3 Noise reduction** | Four full-column targets, plain descriptions. | — |
| **B4 Volume and Mic level** | `lv_arc` with − / +; speaker mute in the centre; swipe left to Mic level (same layout, mic mute in the centre). | Mic gain in the capture path (C5). |
| **B5 Mic height** | — | Excluded (needs F3, milestone 3). |
| **B6 System** | Rotary list: Display, Date and time, Alert tones, Pre-recording, About, Factory reset, Power off. Display (+ Dim after), Date and time (+ Time zone), About, the shared confirmation screen. Power off and Factory reset confirm first. | USB storage (G1), PIN (K2) and Firmware update (H3) rows stay hidden until milestone 3. The backlight, time zone, factory-reset and about-info backends belong to E. Languages are deferred (E4, D5). |
| **B7 Wi-Fi and setup** | Revision-2 layout with drawn signal arcs, no "…" boxes, a warning before "Change network" drops the network, and the QR setup screen. | — |
| **B8 Uploads and After a recording** | Counts, relative "Last sent", Retry only when something failed, the radio list for the upload mode. | Resend backend (C7). |

---

## Global Constraints

Copied from the spec and the milestone 1 / 1.1 plans. Every task includes them.

- **Panel.** Framebuffer 400 × 304. Visible circle r 198 at (200, 106), cut flat at row 0.
- **Tap limit.**
  - Every tappable pixel lies within r 190 of (200, 106) and at y ≥ 6 (`TAP_LIMIT_R`, `TAP_MIN_Y`).
  - Targets are ≥ 44 px both ways (`MIN_TARGET`); `ext_click` may provide the size, but the visible box must meet the limit.
  - Main actions are ≥ 50 px.
- **Zones.**
  - Status cluster (80, 8, 240, 20) via `StatusArea::plain`.
  - Title 20 px semibold at `TITLE_Y` 38, centred across 400.
  - Content from y 62 (sub-line) / `CONTENT_TOP` 70; radio rows 296 × 52 at x 52 from `RADIO_TOP` 68, 4 px apart.
  - Back is 112 × 44 at (144, 244), radius 22 (`BACK`, `back_button`).
- **Rotary list.** Viewport y 34–240; focus line y 144; 48 px slots; fades 14 / 16 px; arc scrollbar r 188 around (200, 106) over 62°–122°.
- **Confirmation.** Icon disc 56 at (172, 40); title (Dial 26) top 104; body (Small) at (65, 142), 270 wide, at most 3 lines; pill pair 124 × 50 at (70, 196) / (206, 196), radius 25: Cancel / action.
- **Value arc.** Volume r 88 at (200, 132); Display r 68 at (200, 140); 270° sweep (−135°…135°), stroke 8, knob 22; − / + are 52 px circles centred at x 56 / 344 on the arc's centre line.
- **Radial mode picker.** Six 48 px buttons on r 132 around (200, 134), at device angles 300° / 270° / 240° (left: Whole room, Follow talker, Focused) and 60° / 90° / 120° (right: Narrow, Two sides, Stereo). The preview is r 50 (inner ring r 25). Mode name (Title) at top 192, hint (Small, Text 2) at top 217.
- **Colours** (0xRRGGBB, `kit::tokens`):

  | Token | Hex | Token | Hex |
  |---|---|---|---|
  | BG | 000000 | TEXT_3 | 5d6a6a |
  | SURFACE | 141c1e | LIVE | 3fd6b1 |
  | RAISED | 1f292b | REC | ff4b3e |
  | LINE | 2a3639 | WARN | ffb547 |
  | TEXT | f2f5f4 | WHITE | ffffff |
  | TEXT_2 | 9aa7a6 | | |

  Teal means live, selected or healthy. Red means recording, muted or destructive. Amber means paused or needs attention. Everything else is grey on black. Red is never a button fill.
- **Type.** Micro 12, Small 13, Body 15, Title 20 (semibold), Dial 26 (semibold); Num13, Num20, Num26 and Num48 Barlow tabular.
  - Kit line heights: Title 27, Dial 32, Num26 24 (base 5), Num48 43 (base 9), Num20 23 (base 4).
  - Text labels use the mockup's CSS `top` as the label y (as `TITLE_Y` and `DIAL_*_Y` do).
  - Big digits are placed by baseline. Num48 label y = CSS top + 11 (as `REC_TIMER_Y` = 93 + 11). Num26 label y = CSS top + 6 (CSS baseline top + 25.4, label baseline y + 19). A Num20 suffix sits 15 px below the Num48 label top (baselines 34 and 19).
- **Labels.**
  - Labels showing user data stay on one line (`centered_text(w)`, or `one_line()` with an explicit `width(text_width(..).min(cap))`).
  - Never use `max_width()` for truncation.
  - Never hard-code a line height.
- **Copy.**
  - Sentence case, plain words.
  - A button names its action and the message afterwards uses the same verb ("Power off" → "Powering off").
  - Errors say what happened and what to do next.
  - No "…" placeholder text.
- **Timings.** Success and info messages 2 s; warnings and errors 4 s (`Severity::duration_ms`).
- **Kit rules.**
  - Screens use `Obj`, `event::on`/`on_click`/`on_swipe` and components.
  - Raw `crate::bindings` *calls* only in `kit/` (the two B components included). Binding constants such as `LV_STATE_PRESSED` and event codes may be used in screens.
  - Settings are read from the snapshot, never from `ctx.config`. The exceptions are `api.endpoint` (Uploads) and `device.name` (About), which only change through the setup page, and the setup page restarts waved.
- **Ownership.** Edit only:
  - `crates/waved-ui/src/screens_v2/{menu,audio_mode,noise,volume,system,display,datetime,wifi,wifi_setup,uploads,upload_mode,confirm,about}.rs`;
  - `crates/waved-ui/src/kit/components/mode_picker.rs`;
  - `crates/waved-ui/src/kit/components/wifi.rs` (requested below);
  - goldens `crates/waved-ui/golden/screen_*.png`, `picker_*.png`, `wifi_*.png`.

  Legacy UI files are not touched.
- **Commits.** `type(scope): lowercase summary`, a bullet-list body, ending in:

  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
- **Commands** (run from `waved/`, with `set -o pipefail`):
  - Tests: `cargo test -p waved-ui --lib <filter>`.
  - Goldens: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib <filter>` writes `crates/waved-ui/golden/<name>.png` and a panel-masked copy in `target/ui-preview/<name>.png`. Compare that copy with `../docs/mockups/png/<id>.png`: the mockup is 880 × 880 at 2×, with the framebuffer at x 40..840, y 228..836. Then re-run without `UPDATE_GOLDEN`.
  - Lint gate (clippy clean in `src/kit/**` and `src/screens_v2/**`):

    ```bash
    cargo clippy -p waved-ui --all-targets --message-format=short 2>&1 | tee target/clippy-b.txt; ! grep -E 'src/(kit|screens_v2)/' target/clippy-b.txt
    ```

## Review Focus

These five inputs aren't covered by the happy-path tests but are likely to hurt someone using the device. Each one has a test in the task named after it.

1. **Drag-to-aim edge input.**
   - A press on the exact centre of the preview has no direction.
   - Angles near 0°/360° must wrap.
   - A release in the dead zone must keep the last valid aim instead of jumping.
   - A sideways drag must not trigger the global swipe-right Back.

   *(Task 2: `aim_angle_wraps_and_ignores_the_centre`; Task 3: `a_release_in_the_dead_zone_keeps_the_last_aim`, `the_aim_area_does_not_bubble_gestures`)*
2. **Stale snapshots fighting the user.** The services confirm a change one or more UI frames later (or, for `SetBeamAim`, not at all until C10 lands). A tap, a drag or an arc being held must not snap back to the old value. *(Task 4: `a_pending_aim_survives_a_stale_snapshot`; Task 5: `a_tap_shows_the_new_level_before_the_service_confirms`; Task 6: `the_arc_ignores_snapshots_while_pressed`)*
3. **Long user strings.** A 32-character SSID, a 63-character setup passphrase, a long upload host or a long device name must end in "..." inside their boxes and never push other text off the panel. *(Task 13: `a_long_network_name_ends_in_dots`; Task 14: `long_setup_details_stay_on_their_lines`; Task 15: `a_long_host_stays_on_one_line`)*
4. **Irreversible actions hit the right target.**
   - Delete from a file uses `DeleteRecording`; the confirmation does one back and the detail leaves itself (R6: the confirmation handles kept files only; After stop discards a pending recording in its own dialog, workstream A).
   - Power off and Factory reset must only be sent after the confirmation.

   *(Task 7: `deleting_a_kept_file_sends_delete_recording`, `power_off_only_after_confirming`, `factory_reset_only_after_confirming`)*
5. **Wi-Fi setup lifecycle.**
   - Opening Wi-Fi setup starts provisioning once.
   - Rebuilding the screen while the access point is up must not restart it (waking from the resting screen rebuilds the screen).
   - Cancel must stop it.
   - "Change network" must warn before the station is dropped.

   *(Task 14: `rebuilding_during_setup_does_not_restart_it`; Task 13: `change_network_warns_first`)*

---

## Contract requests

The integrator lands these in M2.0 before any task below runs. Every task assumes they exist.

1. **Routes** (`crates/waved-ui/src/kit/route.rs`):
   ```rust
   pub enum ConfirmKind {
       PowerOff,
       FactoryReset,
       DeleteRecording(Uuid),
       StopUsbStorage,
       /// Wi-Fi "Change network": leave the current network for the setup network (B7).
       ChangeNetwork,
   }
   pub enum Route {
       // ...existing variants...
       /// Time zone radio list, opened from Date and time.
       TimeZone,
       /// Dim after radio list, opened from Display.
       DimAfter,
   }
   // in Route::guarded_by:
   //   Route::TimeZone | Route::DimAfter => Some(Route::System),
   //   Route::Confirm(ConfirmKind::ChangeNetwork) => Some(Route::Wifi),
   ```
   Why: Date and time, Display and Wi-Fi each open a sub-list or a warning that must work with `back_button` and the PIN guard.
2. **Local time in the snapshot** (`crates/waved-ui/src/kit/snapshot.rs`):
   ```rust
   pub struct UiSnapshot {
       // ...
       /// Offset of `settings.time_zone` from UTC at `now_unix_s`, in minutes
       /// (workstream E3 resolves it); 0 until then.
       pub utc_offset_min: i32,
       pub device: waved_state::DeviceInfo,
   }
   impl UiSnapshot {
       /// Local wall-clock seconds: `now_unix_s` shifted by the zone offset.
       pub fn local_unix_s(&self) -> i64 {
           self.now_unix_s + i64::from(self.utc_offset_min) * 60
       }
   }
   // capture(): clock = format_clock(now_unix_s, utc_offset_min, settings.clock_24h)
   ```
   Why: Date and time shows the big local time and the date ("Saturday 4 October").
3. **Device info** (`crates/waved-state/src/lib.rs`):
   ```rust
   #[derive(Clone, Debug, Default, PartialEq)]
   pub struct DeviceInfo {
       /// Hardware serial number; empty when unknown. Workstream E7 fills it at boot.
       pub serial: String,
   }
   // StateBus: pub device: Arc<watch::Sender<DeviceInfo>>  (default DeviceInfo)
   // UiSnapshot::capture copies bus.device into snap.device.
   ```
   Why: About shows the serial (E7). If E's plan requests an equivalent type, merge the two; B reads only `serial`.
4. **Swipe events** (`crates/waved-ui/src/kit/event.rs`, `testkit.rs`):
   ```rust
   #[derive(Clone, Copy, Debug, PartialEq, Eq)]
   pub enum Swipe { Left, Right, Up, Down }
   /// Run `handler` when a swipe in `direction` ends on `obj`
   /// (LV_EVENT_GESTURE + lv_indev_get_gesture_dir).
   pub fn on_swipe(obj: Obj, direction: Swipe, handler: impl FnMut() + 'static);
   // testkit:
   impl Lvgl {
       /// Deliver a swipe in `direction` to `obj`'s swipe handlers.
       pub fn swipe(&mut self, obj: Obj, direction: Swipe);
   }
   ```
   Why: the Volume screen opens Mic level on a swipe left (`volume.png`: "Swipe left for the microphone level"). The global swipe-right Back stays with the runtime.
5. **Radio list scrolling** (`crates/waved-ui/src/kit/components/controls.rs`):
   ```rust
   impl RadioList {
       /// Scroll the list so the row at `index` is fully visible, without animation.
       pub fn scroll_to(&self, index: usize);
   }
   ```
   Why: Time zone (about 30 rows) and Dim after (6 rows) must open on the selected row.
6. **Tap check clipped to the parent** (`crates/waved-ui/src/kit/testkit.rs`, `target_problems`): check each clickable object's visible box *intersected with every ancestor's box*. LVGL hit-tests a child only inside its parent, as the comment in `list.rs` `visible_rows_stay_inside_the_tap_limit` notes. Skip objects whose clipped box is empty. Keep the size check on the unclipped click area.
   Why: Menu, System, Time zone and Dim after scroll rows far outside their viewport. `assert_targets_ok(root)` would flag those rows, though they can never be touched there.
7. **Two icons** (`crates/waved-ui/fonts/icons.py`, then `fonts/build_fonts.sh`): `("BELL", "bell")` and `("CALENDAR", "calendar")`.
   Why: in `system.png` Pre-recording already uses the clock. Date and time needs the calendar and Alert tones the bell.
8. **Component modules** (`crates/waved-ui/src/kit/components/mod.rs`): add `pub mod mode_picker;` and `pub mod wifi;` with empty stub files, and add `kit/components/wifi.rs` to B's ownership.
   Why: the signal arcs and the QR code need raw LVGL calls (`lv_arc_set_bg_angles`, `lv_qrcode_*`), which screens may not make.
9. **Keep `Command::OpenWifiProvisioning` and `Command::CancelWifiProvisioning`** when the `Open*` commands are deleted at the runtime switch. They are service commands that `waved-provision/src/lib.rs:51,89,490` handles, not navigation. Renaming them (for example to `StartWifiProvisioning`) is fine if B's two uses follow.
10. **Registry for the runtime switch** (information for the integrator; no M2.0 code):

    | Route | Screen |
    |---|---|
    | Menu | `menu::MenuScreen::default()` |
    | AudioMode | `audio_mode::AudioModeScreen::default()` |
    | NoiseReduction | `noise::NoiseScreen::default()` |
    | Volume | `volume::VolumeScreen::speaker()` |
    | MicLevel | `volume::VolumeScreen::mic()` |
    | System | `system::SystemScreen::default()` |
    | Display | `display::DisplayScreen::default()` |
    | DimAfter | `display::DimAfterScreen::default()` |
    | DateTime | `datetime::DateTimeScreen::default()` |
    | TimeZone | `datetime::TimeZoneScreen::default()` |
    | About | `about::AboutScreen::default()` |
    | Confirm(kind) | `confirm::ConfirmScreen::new(kind)` |
    | Wifi | `wifi::WifiScreen::default()` |
    | WifiSetup | `wifi_setup::WifiSetupScreen::default()` |
    | Uploads | `uploads::UploadsScreen::default()` |
    | UploadMode | `upload_mode::UploadModeScreen::default()` |

    One cross-workstream note:
    - **A5:** First start's "Set up Wi-Fi" navigates to `Route::WifiSetup` without sending `OpenWifiProvisioning`, because the setup screen starts provisioning itself (Task 14).

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/waved-ui/src/screens_v2/menu.rs` | `MenuScreen`; the value vocabulary (`mode_name`, `nr_name`, `upload_mode_name`); B's test fixture | Fill stub |
| `crates/waved-ui/src/kit/components/mode_picker.rs` | `ModePicker`, pattern geometry, `aim_angle`, `blend565`, `dash_segments`, canvas painter | Fill stub |
| `crates/waved-ui/src/screens_v2/audio_mode.rs` | `AudioModeScreen`, `ModeChoice` | Fill stub |
| `crates/waved-ui/src/screens_v2/noise.rs` | `NoiseScreen` | Fill stub |
| `crates/waved-ui/src/screens_v2/volume.rs` | `VolumeScreen` (Volume, Mic level), `step_level`, `BigNumber` | Fill stub |
| `crates/waved-ui/src/screens_v2/confirm.rs` | `ConfirmScreen`, `spec`, `outcome` | Fill stub |
| `crates/waved-ui/src/screens_v2/system.rs` | `SystemScreen` | Fill stub |
| `crates/waved-ui/src/screens_v2/display.rs` | `DisplayScreen`, `DimAfterScreen`, `duration_label` | Fill stub |
| `crates/waved-ui/src/screens_v2/datetime.rs` | `DateTimeScreen`, `TimeZoneScreen`, civil-date helpers, `ZONES` | Fill stub |
| `crates/waved-ui/src/screens_v2/about.rs` | `AboutScreen`, `format_bytes` | Fill stub |
| `crates/waved-ui/src/kit/components/wifi.rs` | `SignalArcs`, `Qr` | Fill stub |
| `crates/waved-ui/src/screens_v2/wifi.rs` | `WifiScreen`, `wifi_view` | Fill stub |
| `crates/waved-ui/src/screens_v2/wifi_setup.rs` | `WifiSetupScreen`, `SetupView`, QR payload | Fill stub |
| `crates/waved-ui/src/screens_v2/uploads.rs` | `UploadsScreen`, `parse_rfc3339`, `relative` | Fill stub |
| `crates/waved-ui/src/screens_v2/upload_mode.rs` | `UploadModeScreen` | Fill stub |
| `crates/waved-ui/golden/{screen_*,picker_*,wifi_*}.png` | Goldens | Create |

---

### Task 1: Menu (B1) and B's test fixture

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/menu.rs` (stub from M2.0)
- Create: `crates/waved-ui/golden/screen_menu.png`

**Interfaces:**
- Consumes:
  - `RotaryList::new(parent, Vec<ListItem>, FnMut(usize))`, `.scroll_to`, `.set_value(usize, Option<&str>)`, `.row`;
  - `ListItem::new(icon, title).value(..).locked(bool)`;
  - `StatusArea::plain`, `back_button`;
  - `Route::requires_pin`;
  - `UiSnapshot` fields;
  - `Dispatcher::test()`.
- Produces:
  ```rust
  pub enum MenuRow { Files, AudioMode, NoiseReduction, Volume, Wifi, Uploads, System }
  pub const ROWS: [MenuRow; 7];
  pub const START_ROW: usize; // 2
  impl MenuRow {
      pub fn route(self) -> Route;
      pub fn title(self) -> &'static str;
      pub fn value(self, snap: &UiSnapshot) -> Option<String>;
      pub fn locked(self, snap: &UiSnapshot) -> bool;
  }
  pub fn mode_name(mode: &AudioMode) -> &'static str;
  pub fn nr_name(level: &NrLevel) -> &'static str;
  pub fn upload_mode_name(mode: UploadMode) -> &'static str;
  #[derive(Default)] pub struct MenuScreen; // impl Screen
  #[cfg(test)] pub(crate) mod fixture {
      pub struct Fixture { pub ctx: ScreenCtx, pub bus: broadcast::Receiver<Command>, pub ui: mpsc::Receiver<UiEvent> }
      pub fn fixture() -> Fixture;
      pub fn fixture_with(config: WavedConfig) -> Fixture;
      pub fn snap() -> UiSnapshot;
      pub fn recording(status: RecordingStatus) -> RecordingSummary;
      impl Fixture { pub fn commands(&mut self) -> Vec<Command>; pub fn events(&self) -> Vec<UiEvent>; }
  }
  ```

- [ ] **Step 1: Write the failing tests** (bottom of `menu.rs`)

```rust
#[cfg(test)]
pub(crate) mod fixture {
    use crate::kit::dispatch::{Dispatcher, UiEvent};
    use crate::kit::screen::ScreenCtx;
    use crate::kit::snapshot::UiSnapshot;
    use std::sync::{mpsc, Arc};
    use tokio::sync::broadcast;
    use uuid::Uuid;
    use waved_config::WavedConfig;
    use waved_state::{Command, RecordingStatus, RecordingSummary};

    pub struct Fixture {
        pub ctx: ScreenCtx,
        pub bus: broadcast::Receiver<Command>,
        pub ui: mpsc::Receiver<UiEvent>,
    }

    pub fn fixture() -> Fixture {
        fixture_with(WavedConfig::default())
    }

    pub fn fixture_with(config: WavedConfig) -> Fixture {
        let (dispatch, bus, ui) = Dispatcher::test();
        Fixture {
            ctx: ScreenCtx {
                dispatch,
                config: Arc::new(config),
            },
            bus,
            ui,
        }
    }

    impl Fixture {
        /// Every command sent since the last call.
        pub fn commands(&mut self) -> Vec<Command> {
            let mut out = Vec::new();
            while let Ok(command) = self.bus.try_recv() {
                out.push(command);
            }
            out
        }

        /// Every UI event sent since the last call.
        pub fn events(&self) -> Vec<UiEvent> {
            self.ui.try_iter().collect()
        }
    }

    /// The status cluster as the settings mockups draw it: USB, Wi-Fi, 14:33, 82 %.
    pub fn snap() -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.mcu.usb_connected = true;
        snap.wifi.connected = true;
        snap.wifi.ssid = "Office-5G".into();
        snap.wifi.ip_addr = "192.168.1.42".into();
        snap.wifi.signal_strength = -60;
        snap.clock = "14:33".into();
        snap.battery.level = 82;
        snap
    }

    pub fn recording(status: RecordingStatus) -> RecordingSummary {
        RecordingSummary {
            id: Uuid::new_v4(),
            started_at: None,
            duration_secs: 600,
            size_bytes: 10_000_000,
            status,
            marks: 0,
            last_error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{fixture, recording, snap};
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use waved_state::RecordingStatus;

    fn menu_snap() -> UiSnapshot {
        let mut snap = snap();
        snap.clock = "14:32".into();
        snap.settings.pin_enabled = true;
        snap.files.recordings = (0..12).map(|_| recording(RecordingStatus::Uploaded)).collect();
        snap.audio.mode = AudioMode::Steered { initial_az_deg: 0.0 };
        snap.audio.nr_level = NrLevel::Med;
        snap.audio.speaker_volume = 60;
        snap
    }

    #[test]
    fn menu_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = MenuScreen::default();
            screen.build(root, &f.ctx, &menu_snap());
            // menu.png: Files (far), Audio mode, Noise reduction (focus), Volume, then
            // Wi-Fi where the mockup shows the hidden Mic height row.
            assert_golden("screen_menu", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn out_of_scope_rows_are_hidden() {
        assert_eq!(
            ROWS.map(MenuRow::title),
            ["Files", "Audio mode", "Noise reduction", "Volume", "Wi-Fi", "Uploads", "System"]
        );
    }

    #[test]
    fn rows_follow_the_snapshot() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = menu_snap();
            let mut screen = MenuScreen::default();
            screen.build(root, &f.ctx, &snap);
            let value = |screen: &MenuScreen, index: usize| {
                screen.list.as_ref().unwrap().row(index).child(2).child(0).text()
            };
            assert_eq!(value(&screen, 3), "60%");
            snap.audio.speaker_volume = 70;
            screen.update(&snap);
            assert_eq!(value(&screen, 3), "70%");
            snap.audio.speaker_muted = true;
            screen.update(&snap);
            assert_eq!(value(&screen, 3), "Muted");
            snap.wifi.connected = false;
            screen.update(&snap);
            assert_eq!(value(&screen, 4), "Not connected");
        });
    }

    #[test]
    fn tapping_a_row_opens_its_screen() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = MenuScreen::default();
            screen.build(root, &f.ctx, &menu_snap());
            lv.click(screen.list.as_ref().unwrap().row(1));
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::AudioMode)]);
        });
    }

    #[test]
    fn the_lock_marks_pin_rows_only_when_a_pin_is_set() {
        let mut snap = snap();
        assert!(!MenuRow::Files.locked(&snap));
        snap.settings.pin_enabled = true;
        for row in [MenuRow::Files, MenuRow::Wifi, MenuRow::Uploads, MenuRow::System] {
            assert!(row.locked(&snap), "{row:?}");
        }
        for row in [MenuRow::AudioMode, MenuRow::NoiseReduction, MenuRow::Volume] {
            assert!(!row.locked(&snap), "{row:?}");
        }
    }

    #[test]
    fn the_value_vocabulary_matches_the_mockups() {
        assert_eq!(mode_name(&AudioMode::Omni), "Whole room");
        assert_eq!(mode_name(&AudioMode::Steered { initial_az_deg: 12.0 }), "Follow talker");
        assert_eq!(mode_name(&AudioMode::Cardioid), "Focused");
        assert_eq!(mode_name(&AudioMode::Supercardioid), "Narrow");
        assert_eq!(mode_name(&AudioMode::Figure8), "Two sides");
        assert_eq!(mode_name(&AudioMode::Stereo), "Stereo");
        assert_eq!(nr_name(&NrLevel::Med), "Medium");
        assert_eq!(upload_mode_name(UploadMode::Ask), "Ask me");
        assert_eq!(upload_mode_name(UploadMode::LocalOnly), "Keep on Wave");
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::menu`
Expected: errors `cannot find type MenuScreen`, `cannot find value ROWS`.

- [ ] **Step 3: Implement** (top of `menu.rs`)

```rust
//! Menu (B1): every setting with its current value, opened from the status
//! pill (menu.png). Mic height (B5) and Bluetooth (I4) join in milestone 3.
//! This file also holds the value words other settings screens reuse.

use crate::kit::components::buttons::back_button;
use crate::kit::components::list::{ListItem, RotaryList};
use crate::kit::components::status::StatusArea;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use waved_state::{AudioMode, NrLevel, UploadMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuRow {
    Files,
    AudioMode,
    NoiseReduction,
    Volume,
    Wifi,
    Uploads,
    System,
}

/// Milestone 2's rows, in the mockup's order (how often people need them).
pub const ROWS: [MenuRow; 7] = [
    MenuRow::Files,
    MenuRow::AudioMode,
    MenuRow::NoiseReduction,
    MenuRow::Volume,
    MenuRow::Wifi,
    MenuRow::Uploads,
    MenuRow::System,
];

/// The row centred when the menu opens: Noise reduction, as in menu.png, so
/// the mid-meeting controls sit around the focus line.
pub const START_ROW: usize = 2;

impl MenuRow {
    pub fn route(self) -> Route {
        match self {
            MenuRow::Files => Route::Files,
            MenuRow::AudioMode => Route::AudioMode,
            MenuRow::NoiseReduction => Route::NoiseReduction,
            MenuRow::Volume => Route::Volume,
            MenuRow::Wifi => Route::Wifi,
            MenuRow::Uploads => Route::Uploads,
            MenuRow::System => Route::System,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            MenuRow::Files => "Files",
            MenuRow::AudioMode => "Audio mode",
            MenuRow::NoiseReduction => "Noise reduction",
            MenuRow::Volume => "Volume",
            MenuRow::Wifi => "Wi-Fi",
            MenuRow::Uploads => "Uploads",
            MenuRow::System => "System",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            MenuRow::Files => icons::FOLDER,
            MenuRow::AudioMode => icons::PATTERN,
            MenuRow::NoiseReduction => icons::NOISE,
            MenuRow::Volume => icons::VOLUME_HIGH,
            MenuRow::Wifi => icons::WIFI,
            MenuRow::Uploads => icons::SEND,
            MenuRow::System => icons::SETTINGS,
        }
    }

    /// The live value at the right of the row; `None` shows nothing.
    pub fn value(self, snap: &UiSnapshot) -> Option<String> {
        match self {
            MenuRow::Files => {
                let count = snap.files.recordings.len();
                (count > 0).then(|| count.to_string())
            }
            MenuRow::AudioMode => Some(mode_name(&snap.audio.mode).into()),
            MenuRow::NoiseReduction => Some(nr_name(&snap.audio.nr_level).into()),
            MenuRow::Volume => Some(if snap.audio.speaker_muted {
                "Muted".into()
            } else {
                let volume = snap.audio.speaker_volume.min(100);
                format!("{volume}%")
            }),
            MenuRow::Wifi => Some(if !snap.wifi.connected {
                "Not connected".into()
            } else if snap.wifi.ssid.is_empty() {
                "Connected".into()
            } else {
                snap.wifi.ssid.clone()
            }),
            MenuRow::Uploads => Some(upload_mode_name(snap.settings.upload_mode).into()),
            MenuRow::System => None,
        }
    }

    /// The lock marks PIN rows (D10) once a PIN is set. Enforcement is K2.
    pub fn locked(self, snap: &UiSnapshot) -> bool {
        snap.settings.pin_enabled && self.route().requires_pin()
    }
}

pub fn mode_name(mode: &AudioMode) -> &'static str {
    match mode {
        AudioMode::Omni => "Whole room",
        AudioMode::Steered { .. } => "Follow talker",
        AudioMode::Cardioid => "Focused",
        AudioMode::Supercardioid => "Narrow",
        AudioMode::Figure8 => "Two sides",
        AudioMode::Stereo => "Stereo",
    }
}

pub fn nr_name(level: &NrLevel) -> &'static str {
    match level {
        NrLevel::Off => "Off",
        NrLevel::Low => "Low",
        NrLevel::Med => "Medium",
        NrLevel::High => "High",
    }
}

pub fn upload_mode_name(mode: UploadMode) -> &'static str {
    match mode {
        UploadMode::Automatic => "Automatic",
        UploadMode::Ask => "Ask me",
        UploadMode::LocalOnly => "Keep on Wave",
    }
}

#[derive(Default)]
pub struct MenuScreen {
    status: Option<StatusArea>,
    list: Option<RotaryList>,
    shown: Vec<Option<String>>,
}

impl Screen for MenuScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        self.shown = ROWS.iter().map(|row| row.value(snap)).collect();
        let items = ROWS
            .iter()
            .zip(&self.shown)
            .map(|(row, value)| {
                let item = ListItem::new(row.icon(), row.title()).locked(row.locked(snap));
                match value {
                    Some(value) => item.value(value.as_str()),
                    None => item,
                }
            })
            .collect();
        let dispatch = ctx.dispatch.clone();
        let list = RotaryList::new(root, items, move |index| {
            dispatch.navigate(ROWS[index].route())
        });
        list.scroll_to(START_ROW);
        self.list = Some(list);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let Some(list) = self.list.as_ref() else {
            return;
        };
        for (index, row) in ROWS.iter().enumerate() {
            let value = row.value(snap);
            if self.shown[index] != value {
                list.set_value(index, value.as_deref());
                self.shown[index] = value;
            }
        }
    }
}
```

The lock flag is fixed when the menu is built. `RotaryList` has no API to change it later, and the PIN can only change on the setup page, which restarts waved.

- [ ] **Step 4: Generate the golden and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::menu`
Compare `target/ui-preview/screen_menu.png` with `../docs/mockups/png/menu.png`. It must match except for:
- the bottom far row, which reads "Wi-Fi Office-5G" with a lock where the mockup shows "Mic height";
- the scrollbar thumb length, which differs because there are 7 rows instead of 9.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::menu`
Expected: 6 passed.

- [ ] **Step 6: Lint** — run the lint gate from Global Constraints; expect no output from `grep`.

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/menu.rs crates/waved-ui/golden/screen_menu.png
git commit -m "feat(ui): settings menu with live values

- rotary list of files, audio mode, noise reduction, volume, wi-fi, uploads, system
- values follow the snapshot; a lock marks pin rows once a pin is set
- mic height and bluetooth stay hidden until milestone 3
- shared value words and the b screens' test fixture

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Mode picker geometry (pure)

**Files:**
- Modify: `crates/waved-ui/src/kit/components/mode_picker.rs` (stub from M2.0)

**Interfaces:**
- Produces:
  ```rust
  pub fn pattern_gain(a: f32, theta_deg: f32) -> f32;
  pub fn lobe_points(cx: f32, cy: f32, r: f32, a: f32, rot: f32, step: f32) -> Vec<(f32, f32)>;
  pub fn inside_lobe(cx: f32, cy: f32, r: f32, a: f32, rot: f32, x: f32, y: f32) -> bool;
  pub fn aim_angle(cx: i32, cy: i32, x: i32, y: i32, dead: i32) -> Option<f32>;
  pub fn blend565(dst: u16, rgb: u32, alpha: u8) -> u16;
  pub fn dash_segments(points: &[(f32, f32)], closed: bool, on: f32, off: f32) -> Vec<((f32, f32), (f32, f32))>;
  pub fn polar_f(cx: f32, cy: f32, r: f32, device_deg: f32) -> (f32, f32);
  ```

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod geometry_tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }

    #[test]
    fn patterns_follow_the_first_order_formula() {
        // r(θ) = |a + (1 − a)·cos θ| (index.html, data-polar).
        assert!((pattern_gain(1.0, 123.0) - 1.0).abs() < 1e-6); // omni
        assert!((pattern_gain(0.5, 0.0) - 1.0).abs() < 1e-6); // cardioid front
        assert!(pattern_gain(0.5, 180.0).abs() < 1e-6); // cardioid null
        assert!((pattern_gain(0.0, 180.0) - 1.0).abs() < 1e-6); // figure-8 back lobe
        assert!(pattern_gain(0.0, 90.0).abs() < 1e-6); // figure-8 side null
    }

    #[test]
    fn lobe_points_turn_with_the_aim() {
        let points = lobe_points(200.0, 134.0, 50.0, 0.5, 300.0, 3.0);
        assert_eq!(points.len(), 120);
        // θ = 0 is the peak, pointing at the aim: the handle spot in audio-mode.png.
        assert!(close(points[0], polar_f(200.0, 134.0, 50.0, 300.0)));
        // θ = 180° is the cardioid's null: the centre.
        assert!(close(points[60], (200.0, 134.0)));
        assert!(close(polar_f(200.0, 134.0, 50.0, 90.0), (250.0, 134.0)));
    }

    #[test]
    fn inside_lobe_matches_the_outline() {
        let (cx, cy) = (200.0, 134.0);
        assert!(inside_lobe(cx, cy, 50.0, 0.5, 0.0, 200.0, 94.0)); // 40 px in front
        assert!(!inside_lobe(cx, cy, 50.0, 0.5, 0.0, 200.0, 174.0)); // 40 px behind
        assert!(inside_lobe(cx, cy, 50.0, 0.0, 0.0, 200.0, 174.0)); // figure-8 back
        assert!(!inside_lobe(cx, cy, 50.0, 0.0, 0.0, 240.0, 134.0)); // figure-8 side
        assert!(!inside_lobe(cx, cy, 50.0, 0.0, 0.0, cx, cy)); // figure-8 centre is a null
        assert!(inside_lobe(cx, cy, 50.0, 1.0, 0.0, 230.0, 160.0));
        assert!(!inside_lobe(cx, cy, 50.0, 1.0, 0.0, 250.0, 180.0));
    }

    #[test]
    fn aim_angle_wraps_and_ignores_the_centre() {
        let a = |x, y| aim_angle(200, 134, x, y, 8);
        assert_eq!(a(200, 84), Some(0.0));
        assert_eq!(a(250, 134), Some(90.0));
        assert_eq!(a(200, 184), Some(180.0));
        assert_eq!(a(150, 134), Some(270.0));
        let just_left_of_top = a(199, 34).unwrap();
        assert!((359.0..360.0).contains(&just_left_of_top), "{just_left_of_top}");
        assert_eq!(a(200, 134), None);
        assert_eq!(a(205, 130), None); // 6.4 px out: inside the dead zone
        assert!(a(208, 134).is_some());
    }

    #[test]
    fn blending_over_rgb565() {
        assert_eq!(blend565(0x0000, 0xffffff, 255), 0xffff);
        assert_eq!(blend565(0xffff, 0x000000, 0), 0xffff);
        // Live at 16 % over black, the preview fill.
        assert_eq!(blend565(0x0000, 0x3fd6b1, 41), (1 << 11) | (9 << 5) | 4);
    }

    #[test]
    fn dashes_split_a_path_by_length() {
        let dashes = dash_segments(&[(0.0, 0.0), (20.0, 0.0)], false, 3.0, 4.0);
        let spans: Vec<(f32, f32)> = dashes.iter().map(|(a, b)| (a.0, b.0)).collect();
        assert_eq!(spans, vec![(0.0, 3.0), (7.0, 10.0), (14.0, 17.0)]);
        // The pattern carries over corners and around a closed path.
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let total: f32 = dash_segments(&square, true, 2.0, 3.0)
            .iter()
            .map(|(a, b)| (b.0 - a.0).hypot(b.1 - a.1))
            .sum();
        assert!((total - 16.0).abs() < 0.01, "{total}"); // 40 px at 2 on / 3 off
        assert!(dash_segments(&[(1.0, 1.0)], true, 2.0, 3.0).is_empty());
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib kit::components::mode_picker`
Expected: `cannot find function pattern_gain` (and the others).

- [ ] **Step 3: Implement** (top of `mode_picker.rs`; Task 3 adds the component below)

```rust
//! The radial audio-mode picker (B2, audio-mode.png). There are six 48 px
//! round buttons on r 132 around (200, 134) and a live preview of the pickup
//! pattern (r 50) drawn on an `lv_canvas`. Directional modes are aimed by
//! dragging inside the preview. The screen decides what each mode looks like
//! and what the preview shows.

/// Pickup-pattern gain at `theta_deg` from the peak: |a + (1 − a)·cos θ|.
/// a = 1 is omni, 0.5 cardioid, 0.366 supercardioid, 0 figure-8.
pub fn pattern_gain(a: f32, theta_deg: f32) -> f32 {
    (a + (1.0 - a) * theta_deg.to_radians().cos()).abs()
}

/// The point `r` px from (cx, cy) at a device angle (0° up, clockwise).
pub fn polar_f(cx: f32, cy: f32, r: f32, device_deg: f32) -> (f32, f32) {
    let t = device_deg.to_radians();
    (cx + r * t.sin(), cy - r * t.cos())
}

/// Outline of a lobe with peak radius `r` around (cx, cy), its peak turned
/// `rot` degrees clockwise from the top, one point every `step` degrees
/// (closed; the first point is not repeated).
pub fn lobe_points(cx: f32, cy: f32, r: f32, a: f32, rot: f32, step: f32) -> Vec<(f32, f32)> {
    let count = (360.0 / step).round() as usize;
    (0..count)
        .map(|i| {
            let theta = i as f32 * step;
            polar_f(cx, cy, r * pattern_gain(a, theta), theta + rot)
        })
        .collect()
}

/// Whether (x, y) lies inside the lobe drawn by `lobe_points`.
pub fn inside_lobe(cx: f32, cy: f32, r: f32, a: f32, rot: f32, x: f32, y: f32) -> bool {
    let (dx, dy) = (x - cx, y - cy);
    let distance = dx.hypot(dy);
    if distance < 0.5 {
        return a > 0.0; // a figure-8 has no pickup at its centre
    }
    let theta = dx.atan2(-dy).to_degrees() - rot;
    distance <= r * pattern_gain(a, theta)
}

/// The device angle in [0, 360) of (x, y) seen from (cx, cy); `None` within
/// `dead` px of the centre, where a press has no direction.
pub fn aim_angle(cx: i32, cy: i32, x: i32, y: i32, dead: i32) -> Option<f32> {
    let (dx, dy) = ((x - cx) as f32, (y - cy) as f32);
    if dx.hypot(dy) < dead as f32 {
        return None;
    }
    let degrees = dx.atan2(-dy).to_degrees().rem_euclid(360.0);
    Some(if degrees >= 360.0 { 0.0 } else { degrees })
}

/// `rgb` (0xRRGGBB) at `alpha`/255 over an RGB565 pixel.
pub fn blend565(dst: u16, rgb: u32, alpha: u8) -> u16 {
    let a = u32::from(alpha);
    let dst = u32::from(dst);
    let (dr, dg, db) = ((dst >> 11) & 0x1f, (dst >> 5) & 0x3f, dst & 0x1f);
    let (sr, sg, sb) = ((rgb >> 19) & 0x1f, (rgb >> 10) & 0x3f, (rgb >> 3) & 0x1f);
    let mix = |s: u32, d: u32| (s * a + d * (255 - a) + 127) / 255;
    ((mix(sr, dr) << 11) | (mix(sg, dg) << 5) | mix(sb, db)) as u16
}

type Segment = ((f32, f32), (f32, f32));

/// Split a path into dashes of `on` px separated by `off` px, measured along
/// the path so the rhythm carries across corners.
pub fn dash_segments(points: &[(f32, f32)], closed: bool, on: f32, off: f32) -> Vec<Segment> {
    let mut out = Vec::new();
    if points.len() < 2 {
        return out;
    }
    let edges = if closed { points.len() } else { points.len() - 1 };
    let period = on + off;
    let mut walked = 0.0f32;
    for (&a, &b) in points.iter().zip(points.iter().cycle().skip(1)).take(edges) {
        let length = (b.0 - a.0).hypot(b.1 - a.1);
        let at = |t: f32| (a.0 + (b.0 - a.0) * t / length, a.1 + (b.1 - a.1) * t / length);
        let mut s = 0.0f32;
        while s < length {
            let phase = (walked + s) % period;
            let (drawn, left) = if phase < on { (true, on - phase) } else { (false, period - phase) };
            let end = (s + left.max(1e-3)).min(length);
            if drawn {
                out.push((at(s), at(end)));
            }
            s = end;
        }
        walked += length;
    }
    out
}
```

- [ ] **Step 4: Run, expect a pass**

Run: `cargo test -p waved-ui --lib kit::components::mode_picker`
Expected: 6 passed.

- [ ] **Step 5: Lint** (lint gate).

- [ ] **Step 6: Commit**

```bash
git add crates/waved-ui/src/kit/components/mode_picker.rs
git commit -m "feat(ui): pickup-pattern geometry for the mode picker

- first-order pattern gain, lobe outlines and inside test
- aim angle with a centre dead zone and 0-360 wrap
- rgb565 blending and dash splitting for the canvas painter

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Mode picker component

**Files:**
- Modify: `crates/waved-ui/src/kit/components/mode_picker.rs`
- Create: `crates/waved-ui/golden/picker_focused.png`

**Interfaces:**
- Consumes:
  - `Obj` (`plain`, `rect`, `round`, `bg`, `border`, `pressed_bg`, `pressed_border`, `clickable`, `flag`, `center`, `from_raw`);
  - `event::{on, on_click}`;
  - `tokens::{polar, Rect, BG, LINE, LIVE, RAISED, SURFACE, TEXT, TEXT_2, TEXT_3}`;
  - LVGL `lv_canvas_*`, `lv_draw_line`, `lv_draw_arc`, `lv_indev_active`, `lv_indev_get_point`.
- Produces:
  ```rust
  pub const CENTRE: (i32, i32);            // (200, 134)
  pub const RING_R: f32;                   // 132
  pub const ANGLES: [f32; 6];              // [300, 270, 240, 60, 90, 120]
  pub const BUTTON: i32;                   // 48
  pub const PREVIEW_R: f32;                // 50
  pub const PREVIEW_BOX: Rect;             // (138, 72, 124, 124)
  pub const DEAD_ZONE: i32;                // 8
  #[derive(Clone, Copy, Debug, PartialEq)] pub struct Lobe { pub a: f32, pub r: f32, pub rot: f32 }
  #[derive(Clone, Debug, PartialEq)] pub struct Glyph { pub lobes: Vec<Lobe>, pub ghost: Option<Lobe> }
  #[derive(Clone, Debug, PartialEq)] pub struct Preview { pub lobes: Vec<(f32, f32)>, pub handle: Option<f32> }
  #[derive(Clone, Copy, Debug, PartialEq)] pub enum AimEvent { Moving(f32), Done(f32) }
  pub fn button_rect(angle: f32) -> Rect;
  pub struct ModePicker;
  impl ModePicker {
      pub fn new(parent: Obj, glyphs: [Glyph; 6], selected: usize,
                 on_select: impl FnMut(usize) + 'static,
                 on_aim: impl FnMut(AimEvent) + 'static) -> Self;
      pub fn set_selected(&self, index: usize);
      pub fn selected(&self) -> usize;
      pub fn set_preview(&self, preview: &Preview);
      pub fn set_aimable(&self, on: bool);
      pub fn drag_to(&self, x: i32, y: i32, done: bool);
      pub fn button(&self, index: usize) -> Obj;
      pub fn aim_area(&self) -> Obj;
  }
  ```

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod picker_tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// The six glyphs from audio-mode.png (data-polar on each button).
    fn mockup_glyphs() -> [Glyph; 6] {
        let lobe = |a, r, rot| Lobe { a, r, rot };
        let solid = |lobes: Vec<Lobe>| Glyph { lobes, ghost: None };
        [
            solid(vec![lobe(1.0, 14.0, 0.0)]),
            Glyph { lobes: vec![lobe(0.5, 13.0, 40.0)], ghost: Some(lobe(0.5, 12.0, -50.0)) },
            solid(vec![lobe(0.5, 15.0, 0.0)]),
            solid(vec![lobe(0.366, 15.0, 0.0)]),
            solid(vec![lobe(0.0, 14.0, 0.0)]),
            solid(vec![lobe(0.5, 12.0, -45.0), lobe(0.5, 12.0, 45.0)]),
        ]
    }

    fn focused(aim: f32) -> Preview {
        Preview { lobes: vec![(0.5, aim)], handle: Some(aim) }
    }

    #[test]
    fn the_picker_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let picker = ModePicker::new(root, mockup_glyphs(), 2, |_| {}, |_| {});
            picker.set_preview(&focused(300.0));
            picker.set_aimable(true);
            let frame = lv.render();
            assert_golden("picker_focused", &frame);
            assert_targets_ok(root);
            // The handle sits on r 50 at the aim, the centre dot is Text.
            let bright = |[r, g, b]: [u8; 3]| r > 200 && g > 200 && b > 200;
            assert!(bright(frame.pixel(157, 109)), "handle");
            assert!(bright(frame.pixel(200, 134)), "centre dot");
            // Inside the lobe away from strokes: Live at 16 % over black.
            let [r, g, b] = frame.pixel(178, 107);
            assert!(g > r && g > 20 && g < 80 && b > r, "fill {r},{g},{b}");
            // Behind the cardioid (its null): untouched black.
            assert_eq!(frame.pixel(230, 152), [0, 0, 0]);
        });
    }

    #[test]
    fn buttons_sit_on_the_ring_and_report_taps() {
        lvgl(|lv| {
            let root = lv.screen();
            let picked = Rc::new(RefCell::new(Vec::new()));
            let log = picked.clone();
            let picker = ModePicker::new(root, mockup_glyphs(), 0, move |i| log.borrow_mut().push(i), |_| {});
            assert_eq!(button_rect(300.0), Rect::new(62, 44, 48, 48));
            assert_eq!(button_rect(90.0), Rect::new(308, 110, 48, 48));
            lv.click(picker.button(4));
            assert_eq!(*picked.borrow(), vec![4]);
            assert_eq!(picker.selected(), 4);
        });
    }

    #[test]
    fn drags_report_moving_then_done() {
        lvgl(|lv| {
            let root = lv.screen();
            let seen = Rc::new(RefCell::new(Vec::new()));
            let log = seen.clone();
            let picker = ModePicker::new(root, mockup_glyphs(), 2, |_| {}, move |e| log.borrow_mut().push(e));
            picker.set_aimable(true);
            picker.drag_to(250, 134, false);
            picker.drag_to(200, 184, true);
            assert_eq!(*seen.borrow(), vec![AimEvent::Moving(90.0), AimEvent::Done(180.0)]);
            // Not aimable: drags are ignored.
            picker.set_aimable(false);
            picker.drag_to(250, 134, true);
            assert_eq!(seen.borrow().len(), 2);
        });
    }

    #[test]
    fn a_release_in_the_dead_zone_keeps_the_last_aim() {
        lvgl(|lv| {
            let root = lv.screen();
            let seen = Rc::new(RefCell::new(Vec::new()));
            let log = seen.clone();
            let picker = ModePicker::new(root, mockup_glyphs(), 2, |_| {}, move |e| log.borrow_mut().push(e));
            picker.set_aimable(true);
            picker.drag_to(250, 134, false);
            picker.drag_to(201, 135, true); // released on the centre dot
            assert_eq!(*seen.borrow(), vec![AimEvent::Moving(90.0), AimEvent::Done(90.0)]);
            // A press that never left the centre reports nothing.
            picker.drag_to(200, 134, true);
            assert_eq!(seen.borrow().len(), 2);
        });
    }

    #[test]
    fn the_aim_area_does_not_bubble_gestures() {
        lvgl(|lv| {
            let root = lv.screen();
            let picker = ModePicker::new(root, mockup_glyphs(), 2, |_| {}, |_| {});
            let area = picker.aim_area();
            assert!(!area.has_flag(lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE));
            assert!(area.has_flag(lv_obj_flag_t_LV_OBJ_FLAG_PRESS_LOCK));
        });
    }

    #[test]
    fn an_unchanged_preview_is_not_redrawn() {
        lvgl(|lv| {
            let root = lv.screen();
            let picker = ModePicker::new(root, mockup_glyphs(), 2, |_| {}, |_| {});
            picker.set_preview(&focused(300.0));
            assert_eq!(picker.inner.paints.get(), 1);
            picker.set_preview(&focused(300.0));
            assert_eq!(picker.inner.paints.get(), 1);
            picker.set_preview(&focused(301.0));
            assert_eq!(picker.inner.paints.get(), 2);
        });
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib kit::components::mode_picker`
Expected: `cannot find type ModePicker`.

- [ ] **Step 3: Implement** (append to `mode_picker.rs`, after the geometry functions; add the imports to the top of the file)

```rust
use crate::bindings::*;
use crate::kit::event::{on, on_click};
use crate::kit::obj::{color, Obj};
use crate::kit::tokens::{polar, Rect, BG, LINE, LIVE, RAISED, SURFACE, TEXT, TEXT_2, TEXT_3};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Centre of the button ring and of the preview.
pub const CENTRE: (i32, i32) = (200, 134);
/// The buttons sit on this circle...
pub const RING_R: f32 = 132.0;
/// ...at these device angles: the left column top to bottom, then the right.
pub const ANGLES: [f32; 6] = [300.0, 270.0, 240.0, 60.0, 90.0, 120.0];
pub const BUTTON: i32 = 48;
/// The preview's outer ring (the inner ring is half of it).
pub const PREVIEW_R: f32 = 50.0;
/// The preview canvas and the drag area: the pattern plus the r 8 handle and its 3 px rim.
pub const PREVIEW_BOX: Rect = Rect::new(138, 72, 124, 124);
/// Presses this close to the centre have no direction.
pub const DEAD_ZONE: i32 = 8;
/// A glyph canvas, centred in its button.
const GLYPH: i32 = 40;
/// The pattern fill: Live at 16 % (`rgba(63,214,177,.16)`).
const FILL_OPA: u8 = 41;
const OUTLINE_W: i32 = 3;
const RING_W: i32 = 2;
const RAY_W: i32 = 2;
const GLYPH_W: i32 = 2;
const DOT_R: f32 = 4.0;
const HANDLE_R: f32 = 8.0;
const HANDLE_RIM: f32 = 3.0;
/// Degrees between outline points: 3° for the preview, 6° for the glyphs.
const PREVIEW_STEP: f32 = 3.0;
const GLYPH_STEP: f32 = 6.0;

/// One pickup lobe: shape `a`, peak radius `r` px, peak turned `rot` degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lobe {
    pub a: f32,
    pub r: f32,
    pub rot: f32,
}

/// A button's drawing: solid lobes, plus an optional dashed Text 3 lobe
/// behind them (Follow talker's "where it was").
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub lobes: Vec<Lobe>,
    pub ghost: Option<Lobe>,
}

/// What the preview shows: lobes as (a, rot) at `PREVIEW_R`, filled together
/// and outlined in Live, and the aim handle with its dashed ray when aimable.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub lobes: Vec<(f32, f32)>,
    pub handle: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AimEvent {
    /// The finger is dragging the aim to this angle.
    Moving(f32),
    /// The finger lifted with the aim at this angle.
    Done(f32),
}

/// The 48 px box of the button at `angle` on the ring.
pub fn button_rect(angle: f32) -> Rect {
    let (x, y) = polar(CENTRE.0, CENTRE.1, RING_R, angle);
    Rect::new(x - BUTTON / 2, y - BUTTON / 2, BUTTON, BUTTON)
}

pub struct ModePicker {
    inner: Rc<Inner>,
}

struct Inner {
    buttons: Vec<Obj>,
    glyphs: Vec<Canvas>,
    specs: Vec<Glyph>,
    preview: Canvas,
    area: Obj,
    selected: Cell<usize>,
    shown: RefCell<Option<Preview>>,
    /// The last valid angle of the current drag, for a release in the dead zone.
    last_aim: Cell<Option<f32>>,
    /// Preview repaints, for tests.
    paints: Cell<u32>,
    on_select: RefCell<Box<dyn FnMut(usize)>>,
    on_aim: RefCell<Box<dyn FnMut(AimEvent)>>,
}

impl ModePicker {
    /// The picker on `parent`, which must be the screen root (positions are
    /// in screen coordinates). `glyphs` follow `ANGLES`.
    pub fn new(
        parent: Obj,
        glyphs: [Glyph; 6],
        selected: usize,
        on_select: impl FnMut(usize) + 'static,
        on_aim: impl FnMut(AimEvent) + 'static,
    ) -> Self {
        let preview = Canvas::new(parent, PREVIEW_BOX, false);
        // The drag area is a disc over the preview. Press-lock keeps the drag
        // going when the finger slides out; no gesture bubbling, so a sideways
        // drag never becomes the global swipe-right Back.
        let area = Obj::plain(parent)
            .rect(PREVIEW_BOX)
            .round()
            .flag(lv_obj_flag_t_LV_OBJ_FLAG_PRESS_LOCK, true)
            .flag(lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE, false);
        let mut buttons = Vec::with_capacity(6);
        let mut canvases = Vec::with_capacity(6);
        for angle in ANGLES {
            let button = Obj::plain(parent)
                .rect(button_rect(angle))
                .round()
                .bg(SURFACE)
                .pressed_bg(RAISED)
                .pressed_border(2, LIVE)
                .clickable(true);
            let glyph = Canvas::new(button, Rect::new(0, 0, GLYPH, GLYPH), true);
            // Centred, so the 2 px selection border does not shift the glyph.
            glyph.obj.center();
            buttons.push(button);
            canvases.push(glyph);
        }
        let inner = Rc::new(Inner {
            buttons,
            glyphs: canvases,
            specs: Vec::from(glyphs),
            preview,
            area,
            selected: Cell::new(selected.min(5)),
            shown: RefCell::new(None),
            last_aim: Cell::new(None),
            paints: Cell::new(0),
            on_select: RefCell::new(Box::new(on_select)),
            on_aim: RefCell::new(Box::new(on_aim)),
        });
        for (index, &button) in inner.buttons.iter().enumerate() {
            let shared = inner.clone();
            on_click(button, move || {
                shared.select(index);
                (shared.on_select.borrow_mut())(index);
            });
        }
        for (code, done) in [
            (lv_event_code_t_LV_EVENT_PRESSED, false),
            (lv_event_code_t_LV_EVENT_PRESSING, false),
            (lv_event_code_t_LV_EVENT_RELEASED, true),
        ] {
            let shared = inner.clone();
            on(area, code, move |_| shared.drag_from_indev(done));
        }
        inner.select(inner.selected.get());
        Self { inner }
    }

    pub fn set_selected(&self, index: usize) {
        if index != self.inner.selected.get() {
            self.inner.select(index.min(5));
        }
    }

    pub fn selected(&self) -> usize {
        self.inner.selected.get()
    }

    /// Redraw the preview when `preview` differs from what it shows.
    pub fn set_preview(&self, preview: &Preview) {
        if self.inner.shown.borrow().as_ref() == Some(preview) {
            return;
        }
        self.inner.paint_preview(preview);
        *self.inner.shown.borrow_mut() = Some(preview.clone());
    }

    /// Directional modes take drags; the others ignore them.
    pub fn set_aimable(&self, on: bool) {
        self.inner.area.clickable(on);
        self.inner.last_aim.set(None);
    }

    /// What a finger at (x, y) does: the input handler calls this with the
    /// indev point, and tests call it directly.
    pub fn drag_to(&self, x: i32, y: i32, done: bool) {
        self.inner.drag_to(x, y, done);
    }

    pub fn button(&self, index: usize) -> Obj {
        self.inner.buttons[index]
    }

    pub fn aim_area(&self) -> Obj {
        self.inner.area
    }
}

impl Inner {
    fn select(&self, index: usize) {
        self.selected.set(index);
        for (i, (button, glyph)) in self.buttons.iter().zip(&self.glyphs).enumerate() {
            let on = i == index;
            if on {
                button.bg(RAISED).border(2, LIVE);
            } else {
                button.bg(SURFACE).border(0, LIVE);
            }
            paint_glyph(glyph, &self.specs[i], on);
        }
    }

    fn drag_to(&self, x: i32, y: i32, done: bool) {
        if !self.area.is_clickable() {
            return;
        }
        let angle = aim_angle(CENTRE.0, CENTRE.1, x, y, DEAD_ZONE);
        if done {
            if let Some(angle) = angle.or(self.last_aim.get()) {
                (self.on_aim.borrow_mut())(AimEvent::Done(angle));
            }
            self.last_aim.set(None);
        } else if let Some(angle) = angle {
            self.last_aim.set(Some(angle));
            (self.on_aim.borrow_mut())(AimEvent::Moving(angle));
        }
    }

    fn drag_from_indev(&self, done: bool) {
        // SAFETY: called from an LVGL event on the UI thread; the active
        // input device (if any) is valid for the duration of the event.
        let indev = unsafe { lv_indev_active() };
        if indev.is_null() {
            return;
        }
        let mut point = lv_point_t::default();
        // SAFETY: live indev, point is a valid out parameter.
        unsafe { lv_indev_get_point(indev, &mut point) };
        self.drag_to(point.x, point.y, done);
    }

    fn paint_preview(&self, preview: &Preview) {
        self.paints.set(self.paints.get() + 1);
        let canvas = &self.preview;
        let centre = (CENTRE.0 as f32, CENTRE.1 as f32);
        canvas.clear();
        canvas.draw(|pen| {
            pen.ring(centre, PREVIEW_R, RING_W, LINE);
            pen.ring(centre, PREVIEW_R / 2.0, RING_W, LINE);
        });
        canvas.tint(LIVE, FILL_OPA, |x, y| {
            preview
                .lobes
                .iter()
                .any(|&(a, rot)| inside_lobe(centre.0, centre.1, PREVIEW_R, a, rot, x, y))
        });
        canvas.draw(|pen| {
            for &(a, rot) in &preview.lobes {
                let points = lobe_points(centre.0, centre.1, PREVIEW_R, a, rot, PREVIEW_STEP);
                pen.outline(&points, OUTLINE_W, LIVE);
            }
            if let Some(aim) = preview.handle {
                let tip = polar_f(centre.0, centre.1, PREVIEW_R, aim);
                for (from, to) in dash_segments(&[centre, tip], false, 3.0, 4.0) {
                    pen.line(from, to, RAY_W, TEXT_2);
                }
                pen.disc(centre, DOT_R, TEXT);
                pen.disc(tip, HANDLE_R + HANDLE_RIM, BG);
                pen.disc(tip, HANDLE_R, TEXT);
            } else {
                pen.disc(centre, DOT_R, TEXT);
            }
        });
    }
}

fn paint_glyph(canvas: &Canvas, glyph: &Glyph, selected: bool) {
    let centre = (GLYPH as f32 / 2.0, GLYPH as f32 / 2.0);
    canvas.clear();
    canvas.draw(|pen| {
        if let Some(ghost) = glyph.ghost {
            let points = lobe_points(centre.0, centre.1, ghost.r, ghost.a, ghost.rot, GLYPH_STEP);
            for (from, to) in dash_segments(&points, true, 2.0, 3.0) {
                pen.line(from, to, GLYPH_W, TEXT_3);
            }
        }
        let hex = if selected { LIVE } else { TEXT_2 };
        for lobe in &glyph.lobes {
            let points = lobe_points(centre.0, centre.1, lobe.r, lobe.a, lobe.rot, GLYPH_STEP);
            pen.outline(&points, GLYPH_W, hex);
        }
    });
}

/// An `lv_canvas` with a buffer that lives exactly as long as the object:
/// RGB565 and opaque for the preview, ARGB8888 and transparent for glyphs.
/// Relies on LVGL's default stride alignment of 1 (lv_conf.h sets none).
struct Canvas {
    obj: Obj,
    w: i32,
    h: i32,
    /// Screen position of the buffer's top-left, to map screen points.
    origin: (i32, i32),
    pixels: *mut u32,
    argb: bool,
}

impl Canvas {
    fn new(parent: Obj, rect: Rect, argb: bool) -> Self {
        let bytes = if argb { 4 } else { 2 };
        let words = ((rect.w * rect.h * bytes) as usize).div_ceil(4);
        let pixels: *mut [u32] = Box::into_raw(vec![0u32; words].into_boxed_slice());
        // SAFETY: parent is a live object (callers created it just before).
        let obj = unsafe { Obj::from_raw(lv_canvas_create(parent.raw())) };
        // SAFETY: live canvas; the buffer holds rect.w × rect.h pixels of the format.
        unsafe {
            lv_obj_remove_style(
                obj.raw(),
                std::ptr::null(),
                (LV_PART_ANY | LV_STATE_ANY) as lv_style_selector_t,
            );
            let format = if argb {
                lv_color_format_t_LV_COLOR_FORMAT_ARGB8888
            } else {
                lv_color_format_t_LV_COLOR_FORMAT_RGB565
            };
            lv_canvas_set_buffer(obj.raw(), pixels.cast(), rect.w, rect.h, format);
        }
        let obj = obj
            .clickable(false)
            .flag(lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE, false)
            .pos(rect.x, rect.y);
        let mut freed = false;
        on(obj, lv_event_code_t_LV_EVENT_DELETE, move |_| {
            if !freed {
                freed = true;
                // SAFETY: allocated above and freed once, when the canvas is deleted.
                unsafe { drop(Box::from_raw(pixels)) };
            }
        });
        Self {
            obj,
            w: rect.w,
            h: rect.h,
            origin: (rect.x, rect.y),
            pixels: pixels.cast(),
            argb,
        }
    }

    fn clear(&self) {
        let opa = (if self.argb { LV_OPA_TRANSP } else { LV_OPA_COVER }) as lv_opa_t;
        // SAFETY: live canvas with a buffer.
        unsafe { lv_canvas_fill_bg(self.obj.raw(), color(BG), opa) };
    }

    fn draw(&self, paint: impl FnOnce(&mut Pen)) {
        let mut layer = lv_layer_t::default();
        // SAFETY: live canvas; the layer is finished before it goes out of scope.
        unsafe { lv_canvas_init_layer(self.obj.raw(), &mut layer) };
        paint(&mut Pen {
            layer: &mut layer,
            origin: self.origin,
        });
        // SAFETY: same canvas and layer.
        unsafe { lv_canvas_finish_layer(self.obj.raw(), &mut layer) };
    }

    /// Blend `hex` at `opa` over every pixel whose centre (in screen
    /// coordinates) satisfies `inside`. RGB565 canvases only.
    fn tint(&self, hex: u32, opa: u8, inside: impl Fn(f32, f32) -> bool) {
        debug_assert!(!self.argb, "tint works on the RGB565 preview");
        let pixels = self.pixels.cast::<u16>();
        for y in 0..self.h {
            for x in 0..self.w {
                let sx = (self.origin.0 + x) as f32 + 0.5;
                let sy = (self.origin.1 + y) as f32 + 0.5;
                if inside(sx, sy) {
                    // SAFETY: x < w and y < h; the buffer holds w × h RGB565 pixels.
                    unsafe {
                        let pixel = pixels.add((y * self.w + x) as usize);
                        *pixel = blend565(*pixel, hex, opa);
                    }
                }
            }
        }
        // SAFETY: live canvas.
        unsafe { lv_obj_invalidate(self.obj.raw()) };
    }
}

/// Draws on a canvas layer in screen coordinates.
struct Pen<'a> {
    layer: &'a mut lv_layer_t,
    origin: (i32, i32),
}

impl Pen<'_> {
    fn point(&self, (x, y): (f32, f32)) -> lv_point_precise_t {
        lv_point_precise_t {
            x: x.round() as i32 - self.origin.0,
            y: y.round() as i32 - self.origin.1,
        }
    }

    fn line(&mut self, from: (f32, f32), to: (f32, f32), width: i32, hex: u32) {
        let mut dsc = lv_draw_line_dsc_t::default();
        // SAFETY: valid descriptor.
        unsafe { lv_draw_line_dsc_init(&mut dsc) };
        dsc.p1 = self.point(from);
        dsc.p2 = self.point(to);
        dsc.width = width;
        dsc.color = color(hex);
        dsc.opa = LV_OPA_COVER as lv_opa_t;
        dsc.set_round_start(1);
        dsc.set_round_end(1);
        // SAFETY: the layer is initialised and alive for this Pen.
        unsafe { lv_draw_line(self.layer, &dsc) };
    }

    /// A closed outline through `points`; round caps join the segments.
    fn outline(&mut self, points: &[(f32, f32)], width: i32, hex: u32) {
        for (&from, &to) in points.iter().zip(points.iter().cycle().skip(1)) {
            self.line(from, to, width, hex);
        }
    }

    fn circle(&mut self, centre: (f32, f32), outer_r: f32, width: i32, hex: u32) {
        let mut dsc = lv_draw_arc_dsc_t::default();
        // SAFETY: valid descriptor.
        unsafe { lv_draw_arc_dsc_init(&mut dsc) };
        let c = self.point(centre);
        dsc.center = lv_point_t { x: c.x, y: c.y };
        dsc.radius = outer_r.round() as u16;
        dsc.width = width;
        dsc.start_angle = 0;
        dsc.end_angle = 360;
        dsc.color = color(hex);
        dsc.opa = LV_OPA_COVER as lv_opa_t;
        // SAFETY: the layer is initialised and alive for this Pen.
        unsafe { lv_draw_arc(self.layer, &dsc) };
    }

    /// A ring whose stroke is centred on `r`.
    fn ring(&mut self, centre: (f32, f32), r: f32, width: i32, hex: u32) {
        self.circle(centre, r + width as f32 / 2.0, width, hex);
    }

    fn disc(&mut self, centre: (f32, f32), r: f32, hex: u32) {
        self.circle(centre, r, r.round() as i32, hex);
    }
}
```

The test module reads `picker.inner.paints`; `picker_tests` is a child module, so it can see the private field.

- [ ] **Step 4: Generate the golden and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib kit::components::mode_picker`
Compare `target/ui-preview/picker_focused.png` with `../docs/mockups/png/audio-mode.png`. The status cluster, title, name, hint and Back are missing here; the screen adds them in Task 4. Check that:
- the button discs, glyph shapes and the selected teal ring on Focused match;
- the preview rings, lobe, fill, dashed ray, handle and centre dot match.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib kit::components::mode_picker`
Expected: 12 passed (6 geometry + 6 picker).

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/kit/components/mode_picker.rs crates/waved-ui/golden/picker_focused.png
git commit -m "feat(ui): radial audio-mode picker component

- six round buttons on r 132 with canvas-drawn pattern glyphs
- live preview: rings, 16 % fill, outline, dashed ray and aim handle
- drag-to-aim with a dead zone, press lock and no gesture bubbling
- canvas buffers are freed with their objects

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Audio mode screen (B2)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/audio_mode.rs`
- Create: `crates/waved-ui/golden/screen_audio_mode.png`, `screen_audio_mode_follow.png`

**Interfaces:**
- Consumes:
  - `ModePicker`, `Glyph`, `Lobe`, `Preview`, `AimEvent` (Task 3);
  - `menu::mode_name` (Task 1);
  - `Command::SetAudioMode(AudioMode)`, `Command::SetBeamAim(f32)`;
  - `AudioState.{mode, beam_aim_deg, doa_horizontal, doa_active}`.
- Produces:
  ```rust
  pub enum ModeChoice { WholeRoom, FollowTalker, Focused, Narrow, TwoSides, Stereo }
  impl ModeChoice {
      pub const ALL: [ModeChoice; 6];
      pub fn of(mode: &AudioMode) -> Self;
      pub fn audio_mode(self) -> AudioMode;
      pub fn index(self) -> usize;
      pub fn name(self) -> &'static str;
      pub fn hint(self) -> &'static str;
      pub fn aimable(self) -> bool;
      pub fn glyph(self) -> Glyph;
      pub fn preview(self, aim_deg: f32, doa_deg: f32) -> Preview;
  }
  #[derive(Default)] pub struct AudioModeScreen; // impl Screen
  ```

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens_v2::menu::fixture::{fixture, snap};
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use waved_state::Command;

    fn focused_snap() -> UiSnapshot {
        let mut snap = snap();
        snap.audio.mode = AudioMode::Cardioid;
        snap.audio.beam_aim_deg = 300.0;
        snap
    }

    #[test]
    fn audio_mode_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = AudioModeScreen::default();
            screen.build(root, &f.ctx, &focused_snap());
            assert_golden("screen_audio_mode", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.name.unwrap().text(), "Focused");
            assert_eq!(screen.hint.unwrap().text(), "Drag the shape to aim it");
        });
    }

    #[test]
    fn follow_talker_turns_with_the_talker() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.audio.mode = AudioMode::Steered { initial_az_deg: 0.0 };
            snap.audio.doa_active = true;
            snap.audio.doa_horizontal = 40.0;
            let mut screen = AudioModeScreen::default();
            screen.build(root, &f.ctx, &snap);
            let first = lv.render();
            assert_golden("screen_audio_mode_follow", &first);
            assert!(!screen.picker.as_ref().unwrap().aim_area().is_clickable());
            // Silence keeps the last direction; voice from 200° turns it.
            snap.audio.doa_active = false;
            snap.audio.doa_horizontal = 200.0;
            screen.update(&snap);
            assert_eq!(first.pixel(219, 104), lv.render().pixel(219, 104));
            snap.audio.doa_active = true;
            screen.update(&snap);
            assert_ne!(first.pixel(219, 104), lv.render().pixel(219, 104));
        });
    }

    #[test]
    fn a_pending_aim_survives_a_stale_snapshot() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let snap = focused_snap();
            let mut screen = AudioModeScreen::default();
            screen.build(root, &f.ctx, &snap);
            let picker = screen.picker.as_ref().unwrap();
            picker.drag_to(250, 134, false);
            picker.drag_to(250, 134, true);
            let commands = f.commands();
            assert!(
                matches!(commands.as_slice(), [Command::SetBeamAim(d)] if (d - 90.0).abs() < 0.5),
                "{commands:?}"
            );
            // The service has not applied it yet: beam_aim_deg is still 300.
            screen.update(&snap);
            let bright = |[r, g, b]: [u8; 3]| r > 200 && g > 200 && b > 200;
            assert!(bright(lv.render().pixel(250, 134)), "handle stays at 90°");
            // Once the snapshot agrees, the pending aim is dropped.
            let mut applied = snap.clone();
            applied.audio.beam_aim_deg = 90.0;
            screen.update(&applied);
            assert!(screen.aim.borrow().pending.is_none());
        });
    }

    #[test]
    fn tapping_a_mode_switches_and_shows_it_at_once() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let snap = focused_snap();
            let mut screen = AudioModeScreen::default();
            screen.build(root, &f.ctx, &snap);
            lv.click(screen.picker.as_ref().unwrap().button(3));
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetAudioMode(AudioMode::Supercardioid)]
            ));
            screen.update(&snap); // still Cardioid in the snapshot
            assert_eq!(screen.name.unwrap().text(), "Narrow");
        });
    }

    #[test]
    fn modes_map_both_ways() {
        for choice in ModeChoice::ALL {
            assert_eq!(ModeChoice::of(&choice.audio_mode()), choice);
            assert_eq!(ModeChoice::ALL[choice.index()], choice);
            assert_eq!(choice.name(), crate::screens_v2::menu::mode_name(&choice.audio_mode()));
        }
        assert!(ModeChoice::Focused.aimable() && ModeChoice::TwoSides.aimable());
        assert!(!ModeChoice::FollowTalker.aimable() && !ModeChoice::Stereo.aimable());
        assert_eq!(ModeChoice::Focused.preview(300.4, 0.0).handle, Some(300.0));
        assert_eq!(ModeChoice::FollowTalker.preview(0.0, 43.0).lobes, vec![(0.5, 45.0)]);
    }
}
```

The pixel (219, 104) lies 35 px from the centre at 33°: inside the Follow talker cardioid when it points at 40° and outside it when it points at 200°. It sits clear of the r 25 / r 50 rings.

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::audio_mode`
Expected: `cannot find type AudioModeScreen`.

- [ ] **Step 3: Implement**

```rust
//! Audio mode (B2, audio-mode.png): six modes around a live preview.
//! - Focused, Narrow and Two sides are aimed by dragging the shape (`SetBeamAim`, D2).
//! - Follow talker turns with the live direction of arrival.
//! - Stereo keeps the device orientation.
//! Applying the aim is C3/C10's job. Until the audio service reports it in
//! `beam_aim_deg`, the screen keeps showing the aim the user chose.

use crate::kit::components::buttons::back_button;
use crate::kit::components::mode_picker::{AimEvent, Glyph, Lobe, ModePicker, Preview};
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{SCREEN_W, TEXT, TEXT_2, TITLE_Y};
use crate::screens_v2::menu::mode_name;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use waved_state::{AudioMode, Command};

const NAME_Y: i32 = 192;
const HINT_Y: i32 = 217;
/// The preview follows the talker in 5° steps, so it doesn't redraw on noise.
const DOA_STEP: f32 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeChoice {
    WholeRoom,
    FollowTalker,
    Focused,
    Narrow,
    TwoSides,
    Stereo,
}

impl ModeChoice {
    /// Picker order (`mode_picker::ANGLES`): left column top to bottom, then right.
    pub const ALL: [ModeChoice; 6] = [
        ModeChoice::WholeRoom,
        ModeChoice::FollowTalker,
        ModeChoice::Focused,
        ModeChoice::Narrow,
        ModeChoice::TwoSides,
        ModeChoice::Stereo,
    ];

    pub fn of(mode: &AudioMode) -> Self {
        match mode {
            AudioMode::Omni => ModeChoice::WholeRoom,
            AudioMode::Steered { .. } => ModeChoice::FollowTalker,
            AudioMode::Cardioid => ModeChoice::Focused,
            AudioMode::Supercardioid => ModeChoice::Narrow,
            AudioMode::Figure8 => ModeChoice::TwoSides,
            AudioMode::Stereo => ModeChoice::Stereo,
        }
    }

    /// Follow talker starts facing forward until the first confident DoA frame.
    pub fn audio_mode(self) -> AudioMode {
        match self {
            ModeChoice::WholeRoom => AudioMode::Omni,
            ModeChoice::FollowTalker => AudioMode::Steered { initial_az_deg: 0.0 },
            ModeChoice::Focused => AudioMode::Cardioid,
            ModeChoice::Narrow => AudioMode::Supercardioid,
            ModeChoice::TwoSides => AudioMode::Figure8,
            ModeChoice::Stereo => AudioMode::Stereo,
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    pub fn name(self) -> &'static str {
        mode_name(&self.audio_mode())
    }

    pub fn hint(self) -> &'static str {
        match self {
            ModeChoice::WholeRoom => "Picks up the whole room evenly",
            ModeChoice::FollowTalker => "Turns toward whoever talks",
            ModeChoice::Focused | ModeChoice::Narrow | ModeChoice::TwoSides => {
                "Drag the shape to aim it"
            }
            ModeChoice::Stereo => "Left and right, as Wave sits",
        }
    }

    pub fn aimable(self) -> bool {
        matches!(self, ModeChoice::Focused | ModeChoice::Narrow | ModeChoice::TwoSides)
    }

    /// The button drawing (index.html data-polar on each button).
    pub fn glyph(self) -> Glyph {
        let lobe = |a, r, rot| Lobe { a, r, rot };
        let solid = |lobes| Glyph { lobes, ghost: None };
        match self {
            ModeChoice::WholeRoom => solid(vec![lobe(1.0, 14.0, 0.0)]),
            ModeChoice::FollowTalker => Glyph {
                lobes: vec![lobe(0.5, 13.0, 40.0)],
                ghost: Some(lobe(0.5, 12.0, -50.0)),
            },
            ModeChoice::Focused => solid(vec![lobe(0.5, 15.0, 0.0)]),
            ModeChoice::Narrow => solid(vec![lobe(0.366, 15.0, 0.0)]),
            ModeChoice::TwoSides => solid(vec![lobe(0.0, 14.0, 0.0)]),
            ModeChoice::Stereo => solid(vec![lobe(0.5, 12.0, -45.0), lobe(0.5, 12.0, 45.0)]),
        }
    }

    /// What the preview shows at this aim and talker direction. Angles are
    /// rounded (1° aim, 5° talker), so equal frames compare equal and skip redraws.
    pub fn preview(self, aim_deg: f32, doa_deg: f32) -> Preview {
        let aim = aim_deg.round().rem_euclid(360.0);
        let doa = ((doa_deg / DOA_STEP).round() * DOA_STEP).rem_euclid(360.0);
        let aimed = |a: f32| Preview { lobes: vec![(a, aim)], handle: Some(aim) };
        match self {
            ModeChoice::WholeRoom => Preview { lobes: vec![(1.0, 0.0)], handle: None },
            ModeChoice::FollowTalker => Preview { lobes: vec![(0.5, doa)], handle: None },
            ModeChoice::Focused => aimed(0.5),
            ModeChoice::Narrow => aimed(0.366),
            ModeChoice::TwoSides => aimed(0.0),
            ModeChoice::Stereo => Preview { lobes: vec![(0.5, -45.0), (0.5, 45.0)], handle: None },
        }
    }
}

/// The aim the user is dragging, and the one sent but not yet reported back.
#[derive(Default)]
struct Aim {
    dragging: Option<f32>,
    pending: Option<f32>,
}

fn same_angle(a: f32, b: f32) -> bool {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d) < 0.5
}

#[derive(Default)]
pub struct AudioModeScreen {
    status: Option<StatusArea>,
    picker: Option<ModePicker>,
    name: Option<Obj>,
    hint: Option<Obj>,
    /// A mode tapped but not yet reported by the audio service.
    chosen: Rc<Cell<Option<ModeChoice>>>,
    aim: Rc<RefCell<Aim>>,
    last_doa: f32,
    shown: Option<ModeChoice>,
}

impl Screen for AudioModeScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Audio mode", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        let current = ModeChoice::of(&snap.audio.mode);
        let (chosen, select_dispatch) = (self.chosen.clone(), ctx.dispatch.clone());
        let (aim, aim_dispatch) = (self.aim.clone(), ctx.dispatch.clone());
        self.picker = Some(ModePicker::new(
            root,
            ModeChoice::ALL.map(ModeChoice::glyph),
            current.index(),
            move |index| {
                let choice = ModeChoice::ALL[index];
                chosen.set(Some(choice));
                select_dispatch.command(Command::SetAudioMode(choice.audio_mode()));
            },
            move |event| {
                let mut aim = aim.borrow_mut();
                match event {
                    AimEvent::Moving(degrees) => aim.dragging = Some(degrees),
                    AimEvent::Done(degrees) => {
                        aim.dragging = None;
                        aim.pending = Some(degrees);
                        aim_dispatch.command(Command::SetBeamAim(degrees));
                    }
                }
            },
        ));
        self.name = Some(
            Obj::label(root, "", Font::Title, TEXT)
                .centered_text(SCREEN_W)
                .pos(0, NAME_Y),
        );
        self.hint = Some(
            Obj::label(root, "", Font::Small, TEXT_2)
                .centered_text(SCREEN_W)
                .pos(0, HINT_Y),
        );
        back_button(root, &ctx.dispatch);
        self.last_doa = snap.audio.doa_horizontal;
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let (Some(picker), Some(name), Some(hint)) = (self.picker.as_ref(), self.name, self.hint)
        else {
            return;
        };
        let reported = ModeChoice::of(&snap.audio.mode);
        if self.chosen.get() == Some(reported) {
            self.chosen.set(None);
        }
        let mode = self.chosen.get().unwrap_or(reported);
        if snap.audio.doa_active {
            self.last_doa = snap.audio.doa_horizontal;
        }
        let aim = {
            let mut aim = self.aim.borrow_mut();
            if aim.pending.is_some_and(|p| same_angle(p, snap.audio.beam_aim_deg)) {
                aim.pending = None;
            }
            aim.dragging.or(aim.pending).unwrap_or(snap.audio.beam_aim_deg)
        };
        picker.set_preview(&mode.preview(aim, self.last_doa));
        if self.shown != Some(mode) {
            picker.set_selected(mode.index());
            picker.set_aimable(mode.aimable());
            name.set_text(mode.name());
            hint.set_text(mode.hint());
            self.shown = Some(mode);
        }
    }
}
```

- [ ] **Step 4: Generate the goldens and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::audio_mode`
Compare `target/ui-preview/screen_audio_mode.png` with `../docs/mockups/png/audio-mode.png` element by element: status, title, six buttons, preview, "Focused", hint, Back. `screen_audio_mode_follow.png` has no mockup; check that:
- the Follow talker button is selected;
- the cardioid points to about 40°;
- there is no handle;
- the hint reads "Turns toward whoever talks".

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::audio_mode`
Expected: 5 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/audio_mode.rs crates/waved-ui/golden/screen_audio_mode*.png
git commit -m "feat(ui): audio mode screen with drag-to-aim

- six modes on the radial picker, selection shown before the service confirms
- focused, narrow and two sides send SetBeamAim on release
- follow talker turns with the live direction of arrival
- the chosen aim stays until beam_aim_deg reports it (c10)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Noise reduction (B3)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/noise.rs`
- Create: `crates/waved-ui/golden/screen_noise.png`

**Interfaces:**
- Consumes: `menu::nr_name`, `Command::SetNrLevel(NrLevel)`, `AudioState.nr_level`.
- Produces: `pub const LEVELS: [NrLevel; 4]`, `pub fn detail(level: &NrLevel) -> &'static str`, `pub fn level_index(level: &NrLevel) -> usize`, `#[derive(Default)] pub struct NoiseScreen` (impl `Screen`).

Layout (noise.png):
- Bars are 44 wide at x 94 / 150 / 206 / 262, bottom-aligned at y 164, 28 / 48 / 68 / 88 tall, radius 10.
- Bars up to the selected level are Live. Higher bars are Surface with a 2 px Line border.
- Each tap target is the whole 44 × 88 column (y 76–164).
- Labels are Micro, 56 wide, centred on the bars at top 170: Live when selected, Text 3 otherwise.
- The level name (Title) is at top 192 and the description (Small, Text 2) at top 217.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens_v2::menu::fixture::{fixture, snap};
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::kit::tokens::Rect;

    fn rect(obj: Obj) -> Rect {
        let a = obj.coords();
        Rect::new(a.x1, a.y1, a.x2 - a.x1 + 1, a.y2 - a.y1 + 1)
    }

    #[test]
    fn noise_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.audio.nr_level = NrLevel::Med;
            let mut screen = NoiseScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_noise", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.detail.unwrap().text(), "Removes most background noise");
        });
    }

    #[test]
    fn every_column_is_a_full_height_target() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = NoiseScreen::default();
            screen.build(root, &f.ctx, &snap());
            root.update_layout();
            for (column, x) in screen.columns.iter().zip([94, 150, 206, 262]) {
                assert_eq!(rect(*column), Rect::new(x, 76, 44, 88));
            }
            assert_eq!(rect(screen.bars[0]), Rect::new(94, 136, 44, 28));
        });
    }

    #[test]
    fn a_tap_shows_the_new_level_before_the_service_confirms() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.audio.nr_level = NrLevel::Med;
            let mut screen = NoiseScreen::default();
            screen.build(root, &f.ctx, &snap);
            lv.click(screen.columns[0]); // the short Off bar
            assert!(matches!(f.commands().as_slice(), [Command::SetNrLevel(NrLevel::Off)]));
            screen.update(&snap); // still Medium in the snapshot
            assert_eq!(screen.name.unwrap().text(), "Off");
            snap.audio.nr_level = NrLevel::Off;
            screen.update(&snap);
            assert!(screen.chosen.get().is_none());
        });
    }

    #[test]
    fn descriptions_are_plain() {
        assert_eq!(detail(&NrLevel::Off), "Room sound untouched");
        assert_eq!(detail(&NrLevel::Low), "Softens fans and hum");
        assert_eq!(detail(&NrLevel::High), "Strongest, voices only");
        assert_eq!(level_index(&NrLevel::High), 3);
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::noise`
Expected: `cannot find type NoiseScreen`.

- [ ] **Step 3: Implement**

```rust
//! Noise reduction (B3, noise.png). Taller bars remove more noise, and every
//! bar's tap target is its full 44 × 88 column, so the short Off bar is as
//! easy to hit as High.

use crate::kit::components::buttons::back_button;
use crate::kit::components::status::StatusArea;
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, LINE, LIVE, SCREEN_W, SURFACE, TEXT, TEXT_2, TEXT_3, TITLE_Y};
use crate::screens_v2::menu::nr_name;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::{Command, NrLevel};

pub const LEVELS: [NrLevel; 4] = [NrLevel::Off, NrLevel::Low, NrLevel::Med, NrLevel::High];
const BAR_X: [i32; 4] = [94, 150, 206, 262];
const BAR_H: [i32; 4] = [28, 48, 68, 88];
const BAR_W: i32 = 44;
const COLUMN_TOP: i32 = 76;
const COLUMN_H: i32 = 88;
const BAR_RADIUS: i32 = 10;
const LABEL_W: i32 = 56;
const LABEL_Y: i32 = 170;
const NAME_Y: i32 = 192;
const DETAIL_Y: i32 = 217;

pub fn detail(level: &NrLevel) -> &'static str {
    match level {
        NrLevel::Off => "Room sound untouched",
        NrLevel::Low => "Softens fans and hum",
        NrLevel::Med => "Removes most background noise",
        NrLevel::High => "Strongest, voices only",
    }
}

pub fn level_index(level: &NrLevel) -> usize {
    LEVELS.iter().position(|l| l == level).unwrap_or(0)
}

#[derive(Default)]
pub struct NoiseScreen {
    status: Option<StatusArea>,
    columns: Vec<Obj>,
    bars: Vec<Obj>,
    labels: Vec<Obj>,
    name: Option<Obj>,
    detail: Option<Obj>,
    chosen: Rc<Cell<Option<usize>>>,
    shown: Option<usize>,
}

impl Screen for NoiseScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Noise reduction", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        for (index, (&x, &h)) in BAR_X.iter().zip(&BAR_H).enumerate() {
            let column = Obj::plain(root)
                .rect(Rect::new(x, COLUMN_TOP, BAR_W, COLUMN_H))
                .clickable(true);
            let bar = Obj::plain(column)
                .rect(Rect::new(0, COLUMN_H - h, BAR_W, h))
                .radius(BAR_RADIUS);
            let label = Obj::label(root, nr_name(&LEVELS[index]), Font::Micro, TEXT_3)
                .centered_text(LABEL_W)
                .pos(x + BAR_W / 2 - LABEL_W / 2, LABEL_Y);
            let (chosen, dispatch) = (self.chosen.clone(), ctx.dispatch.clone());
            on_click(column, move || {
                chosen.set(Some(index));
                dispatch.command(Command::SetNrLevel(LEVELS[index].clone()));
            });
            self.columns.push(column);
            self.bars.push(bar);
            self.labels.push(label);
        }
        self.name = Some(
            Obj::label(root, "", Font::Title, TEXT)
                .centered_text(SCREEN_W)
                .pos(0, NAME_Y),
        );
        self.detail = Some(
            Obj::label(root, "", Font::Small, TEXT_2)
                .centered_text(SCREEN_W)
                .pos(0, DETAIL_Y),
        );
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let reported = level_index(&snap.audio.nr_level);
        if self.chosen.get() == Some(reported) {
            self.chosen.set(None);
        }
        let selected = self.chosen.get().unwrap_or(reported);
        if self.shown == Some(selected) {
            return;
        }
        for (index, (bar, label)) in self.bars.iter().zip(&self.labels).enumerate() {
            if index <= selected {
                bar.bg(LIVE).border(0, LINE);
            } else {
                bar.bg(SURFACE).border(2, LINE);
            }
            label.text_color(if index == selected { LIVE } else { TEXT_3 });
        }
        if let (Some(name), Some(detail_label)) = (self.name, self.detail) {
            name.set_text(nr_name(&LEVELS[selected]));
            detail_label.set_text(detail(&LEVELS[selected]));
        }
        self.shown = Some(selected);
    }
}
```

- [ ] **Step 4: Generate the golden and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::noise`
Compare with `../docs/mockups/png/noise.png`: bar positions, colours, the High outline, labels, name, description, Back.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::noise`
Expected: 4 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/noise.rs crates/waved-ui/golden/screen_noise.png
git commit -m "feat(ui): noise reduction screen with full-column targets

- four bars with 44 x 88 tap columns, levels shown before the service confirms
- plain descriptions per level

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Volume and Mic level (B4)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/volume.rs`
- Create: `crates/waved-ui/golden/screen_volume.png`, `screen_mic_level.png`

**Interfaces:**
- Consumes:
  - `ValueArc::new(parent, cx, cy, r, FnMut(u8))`, `.set_value`, `.value`, `.obj`;
  - `event::{on_click, on_swipe, Swipe}` (contract 4);
  - `Lvgl::swipe` (contract 4);
  - `Command::{SetSpeakerVolume(u16), ToggleSpeakerMute, SetMicVolume(u16), ToggleMicMute}`;
  - `LV_STATE_PRESSED`.
- Produces:
  ```rust
  pub const MIC_MAX: u16; // 200
  pub fn step_level(volume: u16, up: bool, max: u16) -> u16;
  pub enum Level { Speaker, Mic }
  pub struct LevelScreen; impl LevelScreen { pub fn speaker() -> Self; pub fn mic() -> Self } // impl Screen
  pub(crate) struct BigNumber;
  impl BigNumber {
      pub(crate) fn new(parent: Obj, top: i32) -> Self;
      pub(crate) fn set(&self, digits: &str, suffix: &str, gap: i32, hex: u32);
      pub(crate) fn digits(&self) -> Obj;
  }
  pub(crate) fn step_button(parent: Obj, rect: Rect, icon: &str, on_tap: impl FnMut() + 'static) -> Obj;
  ```

Layout (volume.png):
- Arc r 88 at (200, 132).
- Caption (Small, Text 2) at top 80: "Speaker" or "Microphone"; in red with " muted" when muted.
- Value is `BigNumber` at CSS top 96: Num48 label y 107 and a "%" Num20 suffix in Text 2.
- Centre button is a 40 × 40 Surface disc at (180, 164) with `ext_click(2)`. Its icon is Icon20: `VOLUME_HIGH`/`VOLUME_OFF` or `MIC`/`MIC_OFF`, Text, or red when muted.
- − / + are 52 px Surface circles at (30, 106) / (318, 106), pressed Raised with a 2 px Live ring, with Icon26 `MINUS` / `PLUS` icons.
- Mic level maps the arc's 0–100 to `mic_volume` 0–200 (100 = unity, at the arc's middle). The label shows the real `mic_volume` %.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::LV_STATE_PRESSED;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::event::Swipe;
    use crate::kit::route::Route;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    #[test]
    fn volume_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.audio.speaker_volume = 60;
            let mut screen = LevelScreen::speaker();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_volume", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.value.as_ref().unwrap().digits().text(), "60");
        });
    }

    #[test]
    fn mic_level_uses_the_same_layout() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.audio.mic_volume = 100;
            let mut screen = LevelScreen::mic();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_mic_level", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.arc.as_ref().unwrap().value(), 50); // unity sits mid-arc
            assert_eq!(screen.caption.unwrap().text(), "Microphone");
        });
    }

    #[test]
    fn steps_round_to_tens_and_plus_unmutes_the_speaker() {
        assert_eq!(step_level(60, true, 100), 70);
        assert_eq!(step_level(65, true, 100), 70);
        assert_eq!(step_level(100, true, 100), 100);
        assert_eq!(step_level(60, false, 100), 50);
        assert_eq!(step_level(65, false, 100), 60);
        assert_eq!(step_level(5, false, 100), 0);
        assert_eq!(step_level(0, false, 100), 0);
        // Mic gain set above the on-device range by the host is kept, and stepped down from.
        assert_eq!(step_level(250, true, MIC_MAX), 250);
        assert_eq!(step_level(195, true, MIC_MAX), 200);
        assert_eq!(step_level(250, false, MIC_MAX), 240);
        assert!(matches!(
            Level::Speaker.step(60, true, true).as_slice(),
            [Command::SetSpeakerVolume(70), Command::ToggleSpeakerMute]
        ));
        assert!(matches!(
            Level::Speaker.toggle(0, true).as_slice(),
            [Command::SetSpeakerVolume(10), Command::ToggleSpeakerMute]
        ));
        assert!(matches!(Level::Mic.toggle(100, false).as_slice(), [Command::ToggleMicMute]));
    }

    #[test]
    fn buttons_send_the_steps() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.audio.speaker_volume = 60;
            let mut screen = LevelScreen::speaker();
            screen.build(root, &f.ctx, &snap);
            lv.click(screen.plus.unwrap());
            lv.click(screen.minus.unwrap());
            lv.click(screen.centre.unwrap());
            assert!(matches!(
                f.commands().as_slice(),
                [
                    Command::SetSpeakerVolume(70),
                    Command::SetSpeakerVolume(50),
                    Command::ToggleSpeakerMute
                ]
            ));
        });
    }

    #[test]
    fn the_arc_ignores_snapshots_while_pressed() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.audio.speaker_volume = 60;
            let mut screen = LevelScreen::speaker();
            screen.build(root, &f.ctx, &snap);
            let arc = screen.arc.as_ref().unwrap().obj();
            arc.set_state(LV_STATE_PRESSED, true);
            snap.audio.speaker_volume = 30;
            screen.update(&snap);
            assert_eq!(screen.arc.as_ref().unwrap().value(), 60);
            arc.set_state(LV_STATE_PRESSED, false);
            screen.update(&snap);
            assert_eq!(screen.arc.as_ref().unwrap().value(), 30);
        });
    }

    #[test]
    fn swiping_left_opens_mic_level() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = LevelScreen::speaker();
            screen.build(root, &f.ctx, &snap());
            lv.swipe(root, Swipe::Left);
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::MicLevel)]);
        });
    }

    #[test]
    fn muted_turns_the_caption_and_icon_red() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.audio.speaker_muted = true;
            let mut screen = LevelScreen::speaker();
            screen.build(root, &f.ctx, &snap);
            assert_eq!(screen.caption.unwrap().text(), "Speaker muted");
            assert_eq!(screen.centre_icon.unwrap().text(), crate::kit::icons::VOLUME_OFF);
        });
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::volume`
Expected: `cannot find type LevelScreen`.

- [ ] **Step 3: Implement**

```rust
//! Volume and Mic level (B4, volume.png): a native arc with − / + steps of 10
//! and a mute button in the middle. Swipe left on Volume for the microphone
//! level, which uses the same layout. `BigNumber` and `step_button` are shared
//! with Display and Date and time.

use crate::bindings::LV_STATE_PRESSED;
use crate::kit::components::buttons::back_button;
use crate::kit::components::controls::ValueArc;
use crate::kit::components::status::StatusArea;
use crate::kit::event::{on_click, on_swipe, Swipe};
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{text_width, Obj};
use crate::kit::route::Route;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    Rect, LIVE, RAISED, REC, SCREEN_W, SURFACE, TEXT, TEXT_2, TEXT_3,
};
use std::cell::Cell;
use std::rc::Rc;
use waved_state::Command;

/// The on-device mic range: 0–200 % of unity (unity at the arc's middle).
pub const MIC_MAX: u16 = 200;
const ARC_CX: i32 = 200;
const ARC_CY: i32 = 132;
const ARC_R: i32 = 88;
const CAPTION_Y: i32 = 80;
const VALUE_TOP: i32 = 96;
const CENTRE: Rect = Rect::new(180, 164, 40, 40);
const MINUS: Rect = Rect::new(30, 106, 52, 52);
const PLUS: Rect = Rect::new(318, 106, 52, 52);

/// Num48 digits sit 11 px below the mockup's CSS top (as REC_TIMER_Y = 93 + 11).
const NUM48_DROP: i32 = 11;
/// A Num20 label's baseline is 19 px below its top, Num48's 34: 15 px apart.
const SUFFIX_DROP: i32 = 15;

/// The next multiple of 10 up (at most `max`, but a value already past `max`
/// stays), or the previous one down.
pub fn step_level(volume: u16, up: bool, max: u16) -> u16 {
    if up {
        if volume >= max {
            volume
        } else {
            ((volume / 10 + 1) * 10).min(max)
        }
    } else {
        volume.saturating_sub(1) / 10 * 10
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Speaker,
    Mic,
}

impl Level {
    fn read(self, snap: &UiSnapshot) -> (u16, bool) {
        match self {
            Level::Speaker => (snap.audio.speaker_volume.min(100), snap.audio.speaker_muted),
            Level::Mic => (snap.audio.mic_volume, snap.audio.mic_muted),
        }
    }

    fn caption(self, muted: bool) -> &'static str {
        match (self, muted) {
            (Level::Speaker, false) => "Speaker",
            (Level::Speaker, true) => "Speaker muted",
            (Level::Mic, false) => "Microphone",
            (Level::Mic, true) => "Microphone muted",
        }
    }

    fn icon(self, muted: bool) -> &'static str {
        match (self, muted) {
            (Level::Speaker, false) => icons::VOLUME_HIGH,
            (Level::Speaker, true) => icons::VOLUME_OFF,
            (Level::Mic, false) => icons::MIC,
            (Level::Mic, true) => icons::MIC_OFF,
        }
    }

    fn arc_value(self, volume: u16) -> u8 {
        match self {
            Level::Speaker => volume.min(100) as u8,
            Level::Mic => (volume.min(MIC_MAX) / 2) as u8,
        }
    }

    fn set(self, value: u16) -> Command {
        match self {
            Level::Speaker => Command::SetSpeakerVolume(value),
            Level::Mic => Command::SetMicVolume(value),
        }
    }

    fn from_arc(self, percent: u8) -> Command {
        match self {
            Level::Speaker => self.set(u16::from(percent)),
            Level::Mic => self.set(u16::from(percent) * 2),
        }
    }

    /// Commands for a − / + tap. Raising the speaker also unmutes it.
    fn step(self, volume: u16, muted: bool, up: bool) -> Vec<Command> {
        match self {
            Level::Speaker => {
                let mut out = vec![self.set(step_level(volume, up, 100))];
                if up && muted {
                    out.push(Command::ToggleSpeakerMute);
                }
                out
            }
            Level::Mic => vec![self.set(step_level(volume, up, MIC_MAX))],
        }
    }

    /// Commands for the centre button. Unmuting a speaker at 0 % sets 10 %
    /// first, so unmuting is audible.
    fn toggle(self, volume: u16, muted: bool) -> Vec<Command> {
        match self {
            Level::Speaker if muted && volume == 0 => {
                vec![Command::SetSpeakerVolume(10), Command::ToggleSpeakerMute]
            }
            Level::Speaker => vec![Command::ToggleSpeakerMute],
            Level::Mic => vec![Command::ToggleMicMute],
        }
    }
}

/// A big Num48 value with a small Num20 suffix on its baseline, centred on
/// x 200 ("60%", "2:37 pm"). `top` is the mockup's CSS top of the 48 px line.
pub(crate) struct BigNumber {
    digits: Obj,
    suffix: Obj,
    top: i32,
}

impl BigNumber {
    pub(crate) fn new(parent: Obj, top: i32) -> Self {
        Self {
            digits: Obj::label(parent, "", Font::Num48, TEXT),
            suffix: Obj::label(parent, "", Font::Num20, TEXT_2),
            top,
        }
    }

    pub(crate) fn set(&self, digits: &str, suffix: &str, gap: i32, hex: u32) {
        let digits_w = text_width(digits, Font::Num48);
        let suffix_w = if suffix.is_empty() { 0 } else { gap + text_width(suffix, Font::Num20) };
        let x = SCREEN_W / 2 - (digits_w + suffix_w) / 2;
        let y = self.top + NUM48_DROP;
        self.digits.text_color(hex).pos(x, y).set_text(digits);
        self.suffix.pos(x + digits_w + gap, y + SUFFIX_DROP).set_text(suffix);
    }

    pub(crate) fn digits(&self) -> Obj {
        self.digits
    }
}

/// A 52 px − / + circle in the side-button style.
pub(crate) fn step_button(parent: Obj, rect: Rect, icon: &str, on_tap: impl FnMut() + 'static) -> Obj {
    let button = Obj::plain(parent)
        .rect(rect)
        .round()
        .bg(SURFACE)
        .pressed_bg(RAISED)
        .pressed_border(2, LIVE)
        .clickable(true);
    Obj::label(button, icon, Font::Icon26, TEXT).center();
    on_click(button, on_tap);
    button
}

pub struct LevelScreen {
    level: Level,
    status: Option<StatusArea>,
    arc: Option<ValueArc>,
    value: Option<BigNumber>,
    caption: Option<Obj>,
    centre: Option<Obj>,
    centre_icon: Option<Obj>,
    minus: Option<Obj>,
    plus: Option<Obj>,
    /// The latest (volume, muted), for the button handlers.
    current: Rc<Cell<(u16, bool)>>,
    shown: Option<(u16, bool)>,
}

impl LevelScreen {
    pub fn speaker() -> Self {
        Self::new(Level::Speaker)
    }

    pub fn mic() -> Self {
        Self::new(Level::Mic)
    }

    fn new(level: Level) -> Self {
        Self {
            level,
            status: None,
            arc: None,
            value: None,
            caption: None,
            centre: None,
            centre_icon: None,
            minus: None,
            plus: None,
            current: Rc::new(Cell::new((0, false))),
            shown: None,
        }
    }
}

impl Screen for LevelScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        let level = self.level;
        self.current.set(level.read(snap));
        self.status = Some(StatusArea::plain(root));
        let dispatch = ctx.dispatch.clone();
        self.arc = Some(ValueArc::new(root, ARC_CX, ARC_CY, ARC_R, move |percent| {
            dispatch.command(level.from_arc(percent))
        }));
        self.caption = Some(
            Obj::label(root, "", Font::Small, TEXT_2)
                .centered_text(SCREEN_W)
                .pos(0, CAPTION_Y),
        );
        self.value = Some(BigNumber::new(root, VALUE_TOP));
        let centre = Obj::plain(root)
            .rect(CENTRE)
            .round()
            .bg(SURFACE)
            .pressed_bg(RAISED)
            .clickable(true)
            .ext_click((44 - CENTRE.w) / 2);
        self.centre_icon = Some(Obj::label(centre, "", Font::Icon20, TEXT).center());
        let (dispatch, current) = (ctx.dispatch.clone(), self.current.clone());
        on_click(centre, move || {
            let (volume, muted) = current.get();
            level.toggle(volume, muted).into_iter().for_each(|c| dispatch.command(c));
        });
        self.centre = Some(centre);
        for (rect, icon, up) in [(MINUS, icons::MINUS, false), (PLUS, icons::PLUS, true)] {
            let (dispatch, current) = (ctx.dispatch.clone(), self.current.clone());
            let button = step_button(root, rect, icon, move || {
                let (volume, muted) = current.get();
                level.step(volume, muted, up).into_iter().for_each(|c| dispatch.command(c));
            });
            if up {
                self.plus = Some(button);
            } else {
                self.minus = Some(button);
            }
        }
        if level == Level::Speaker {
            let dispatch = ctx.dispatch.clone();
            on_swipe(root, Swipe::Left, move || dispatch.navigate(Route::MicLevel));
        }
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let level = self.level;
        let (volume, muted) = level.read(snap);
        self.current.set((volume, muted));
        // The arc follows the snapshot unless a finger is on it.
        if let Some(arc) = self.arc.as_ref() {
            let target = level.arc_value(volume);
            if !arc.obj().has_state(LV_STATE_PRESSED) && arc.value() != target {
                arc.set_value(target);
            }
        }
        if self.shown == Some((volume, muted)) {
            return;
        }
        if let (Some(value), Some(caption), Some(icon)) =
            (self.value.as_ref(), self.caption, self.centre_icon)
        {
            value.set(&volume.to_string(), "%", 0, if muted { TEXT_3 } else { TEXT });
            caption
                .text_color(if muted { REC } else { TEXT_2 })
                .set_text(level.caption(muted));
            icon.text_color(if muted { REC } else { TEXT })
                .set_text(level.icon(muted));
        }
        self.shown = Some((volume, muted));
    }
}
```

- [ ] **Step 4: Generate the goldens and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::volume`
Compare `screen_volume.png` with `../docs/mockups/png/volume.png`: the arc (stroke 8, fill to 60 %, knob), "Speaker", the "60%" baseline, the centre speaker disc, − / +, Back. `screen_mic_level.png` has no mockup: check the same geometry with "Microphone", "100%", the arc at its middle and the mic icon.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::volume`
Expected: 7 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/volume.rs crates/waved-ui/golden/screen_volume.png crates/waved-ui/golden/screen_mic_level.png
git commit -m "feat(ui): volume and mic level screens

- value arc with steps of 10 and mute in the centre
- swipe left from volume to the microphone level
- the arc never moves under a finger; plus unmutes the speaker
- shared big-number and step-button helpers

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Confirmation screen (B6)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/confirm.rs`
- Create: `crates/waved-ui/golden/screen_confirm.png`, `screen_confirm_factory_reset.png`, `screen_confirm_change_network.png`

**Interfaces:**
- Consumes:
  - `confirm_dialog(root, &ConfirmSpec, FnMut(), FnMut()) -> ButtonRow`, `ButtonRow::obj`, `Tone`;
  - `ConfirmKind` (with contract 1's `ChangeNetwork`);
  - `Command::{Shutdown, FactoryReset, DeleteRecording(Uuid), DiscardRecording(Uuid)}`;
  - `MeetingState.pending_decision`.
- Produces:
  ```rust
  pub fn spec(kind: ConfirmKind, snap: &UiSnapshot) -> ConfirmSpec;
  pub struct Outcome { pub command: Option<Command>, pub backs: usize, pub then: Option<Route>, pub message: Option<Message> }
  pub fn outcome(kind: ConfirmKind, pending_decision: Option<Uuid>) -> Outcome;
  pub struct ConfirmScreen; impl ConfirmScreen { pub fn new(kind: ConfirmKind) -> Self } // impl Screen
  ```

The confirmation is a full screen: status cluster, then `confirm_dialog` (icon disc, Dial title at 104, Small body at (65, 142) w 270, pill pair). Cancel goes back one screen. The action:

| Kind | Command | Then | Message |
|---|---|---|---|
| PowerOff | `Shutdown` | back 1 | Info "Powering off" |
| FactoryReset | `FactoryReset` | back 1 | Info "Resetting Wave" |
| DeleteRecording(id), id is the pending decision | `DiscardRecording(id)` | back 1 (to Decision, which the router then replaces with Home) | Success "Recording deleted" |
| DeleteRecording(id), a kept file | `DeleteRecording(id)` | back 2 (past the deleted file's detail, to Files) | Success "Recording deleted" |
| ChangeNetwork | none (Wi-Fi setup starts provisioning) | back 1, then navigate `WifiSetup` | none |
| StopUsbStorage | none until G1 adds `StopUsbStorage` (milestone 3) | back 1 | none |

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};
    use std::path::PathBuf;
    use waved_state::PendingDecision;

    fn build(kind: ConfirmKind, snap: &UiSnapshot, lv: &mut crate::kit::testkit::Lvgl)
        -> (crate::screens_v2::menu::fixture::Fixture, ConfirmScreen, Obj) {
        let root = lv.screen();
        let f = fixture();
        let mut screen = ConfirmScreen::new(kind);
        screen.build(root, &f.ctx, snap);
        (f, screen, root)
    }

    #[test]
    fn confirm_matches_the_mockup() {
        lvgl(|lv| {
            let mut snap = snap();
            snap.clock = "14:37".into();
            let (_f, _screen, root) = build(ConfirmKind::PowerOff, &snap, lv);
            assert_golden("screen_confirm", &lv.render());
            assert_targets_ok(root);
        });
        lvgl(|lv| {
            let (_f, _s, root) = build(ConfirmKind::FactoryReset, &snap(), lv);
            assert_golden("screen_confirm_factory_reset", &lv.render());
            assert_targets_ok(root);
        });
        lvgl(|lv| {
            let (_f, _s, root) = build(ConfirmKind::ChangeNetwork, &snap(), lv);
            assert_golden("screen_confirm_change_network", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn power_off_only_after_confirming() {
        lvgl(|lv| {
            let (mut f, screen, _root) = build(ConfirmKind::PowerOff, &snap(), lv);
            assert!(f.commands().is_empty());
            lv.click(screen.row.as_ref().unwrap().obj(0)); // Cancel
            assert!(f.commands().is_empty());
            assert_eq!(f.events(), vec![UiEvent::Back]);
            lv.click(screen.row.as_ref().unwrap().obj(1)); // Power off
            assert!(matches!(f.commands().as_slice(), [Command::Shutdown]));
            assert_eq!(
                f.events(),
                vec![UiEvent::Back, UiEvent::Show(Message::new(Severity::Info, "Powering off"))]
            );
        });
    }

    #[test]
    fn deleting_the_pending_recording_discards_it() {
        let pending = Uuid::new_v4();
        let kept = Uuid::new_v4();
        let discard = outcome(ConfirmKind::DeleteRecording(pending), Some(pending));
        assert!(matches!(discard.command, Some(Command::DiscardRecording(id)) if id == pending));
        assert_eq!(discard.backs, 1);
        let delete = outcome(ConfirmKind::DeleteRecording(kept), Some(pending));
        assert!(matches!(delete.command, Some(Command::DeleteRecording(id)) if id == kept));
        assert_eq!(delete.backs, 2);
        assert_eq!(delete.message, Some(Message::new(Severity::Success, "Recording deleted")));
        lvgl(|lv| {
            let mut snap = snap();
            snap.meeting.pending_decision = Some(PendingDecision {
                id: pending,
                audio_path: PathBuf::from("/mnt/udisk/recordings/x/audio.wav"),
                duration_secs: 2537,
                size_bytes: 184_000_000,
                endpoint: "https://notes.example.com/hook".into(),
            });
            let (mut f, screen, _root) = build(ConfirmKind::DeleteRecording(pending), &snap, lv);
            lv.click(screen.row.as_ref().unwrap().obj(1));
            assert!(matches!(f.commands().as_slice(), [Command::DiscardRecording(id)] if *id == pending));
        });
    }

    #[test]
    fn change_network_continues_to_wifi_setup() {
        let next = outcome(ConfirmKind::ChangeNetwork, None);
        assert!(next.command.is_none());
        assert_eq!((next.backs, next.then), (1, Some(Route::WifiSetup)));
        let mut snap = snap();
        snap.wifi.ssid = "Office-5G".into();
        assert!(spec(ConfirmKind::ChangeNetwork, &snap).body.contains("Office-5G"));
        snap.wifi.ssid.clear();
        assert!(spec(ConfirmKind::ChangeNetwork, &snap).body.starts_with("Wave leaves its network"));
    }

    #[test]
    fn every_action_button_repeats_the_action() {
        let snap = snap();
        for (kind, word) in [
            (ConfirmKind::PowerOff, "Power off"),
            (ConfirmKind::FactoryReset, "Factory reset"),
            (ConfirmKind::DeleteRecording(Uuid::new_v4()), "Delete"),
            (ConfirmKind::ChangeNetwork, "Change"),
            (ConfirmKind::StopUsbStorage, "Stop"),
        ] {
            assert_eq!(spec(kind, &snap).confirm, word, "{kind:?}");
        }
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::confirm`
Expected: `cannot find type ConfirmScreen`.

- [ ] **Step 3: Implement**

```rust
//! Confirmation (B6, confirm.png): one screen for every action that can't be
//! undone or drops a connection. The action pill repeats the action's name,
//! and the message afterwards uses it too ("Powering off").

use crate::kit::components::buttons::Tone;
use crate::kit::components::row::{confirm_dialog, ButtonRow, ConfirmSpec};
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::{Dispatcher, Message, Severity, UiEvent};
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::{ConfirmKind, Route};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use std::cell::Cell;
use std::rc::Rc;
use uuid::Uuid;
use waved_state::Command;

pub fn spec(kind: ConfirmKind, snap: &UiSnapshot) -> ConfirmSpec {
    let (icon, title, body, confirm, tone) = match kind {
        ConfirmKind::PowerOff => (
            icons::POWER,
            "Power off?",
            "A recording in progress is saved first. Turn Wave on again with the power button."
                .to_string(),
            "Power off",
            Tone::Rec,
        ),
        ConfirmKind::FactoryReset => (
            icons::TRASH,
            "Factory reset?",
            "Erases settings, recordings and Wi-Fi, then restarts Wave as new. This can't be undone."
                .to_string(),
            "Factory reset",
            Tone::RecText,
        ),
        ConfirmKind::DeleteRecording(_) => (
            icons::TRASH,
            "Delete recording?",
            "It is removed from Wave. Copies already sent to your server stay there.".to_string(),
            "Delete",
            Tone::RecText,
        ),
        ConfirmKind::ChangeNetwork => {
            let ssid = snap.wifi.ssid.trim();
            let network = if ssid.is_empty() { "its network".to_string() } else { ssid.to_string() };
            (
                icons::WIFI,
                "Change network?",
                format!("Wave leaves {network} while the setup network is open. Uploads wait until it's back."),
                "Change",
                Tone::Live,
            )
        }
        ConfirmKind::StopUsbStorage => (
            icons::USB,
            "Stop USB storage?",
            "Eject Wave on your computer first, so files being copied finish.".to_string(),
            "Stop",
            Tone::Neutral,
        ),
    };
    ConfirmSpec {
        icon,
        title: title.into(),
        body,
        confirm: confirm.into(),
        tone,
    }
}

/// What confirming does: a service command, screens to pop, a screen to
/// open after that, and a message.
#[derive(Debug)]
pub struct Outcome {
    pub command: Option<Command>,
    pub backs: usize,
    pub then: Option<Route>,
    pub message: Option<Message>,
}

pub fn outcome(kind: ConfirmKind, pending_decision: Option<Uuid>) -> Outcome {
    let done = |command, backs, message: Option<Message>| Outcome {
        command,
        backs,
        then: None,
        message,
    };
    match kind {
        ConfirmKind::PowerOff => done(
            Some(Command::Shutdown),
            1,
            Some(Message::new(Severity::Info, "Powering off")),
        ),
        ConfirmKind::FactoryReset => done(
            Some(Command::FactoryReset),
            1,
            Some(Message::new(Severity::Info, "Resetting Wave")),
        ),
        ConfirmKind::DeleteRecording(id) => {
            let deleted = Some(Message::new(Severity::Success, "Recording deleted"));
            if pending_decision == Some(id) {
                // "Send this recording?" is the base; the router replaces it with Home.
                done(Some(Command::DiscardRecording(id)), 1, deleted)
            } else {
                // Pop this dialog and the deleted file's detail, back to Files.
                done(Some(Command::DeleteRecording(id)), 2, deleted)
            }
        }
        ConfirmKind::ChangeNetwork => Outcome {
            command: None,
            backs: 1,
            then: Some(Route::WifiSetup),
            message: None,
        },
        // G1 (milestone 3) adds Command::StopUsbStorage and sends it here.
        ConfirmKind::StopUsbStorage => done(None, 1, None),
    }
}

fn apply(dispatch: &Dispatcher, outcome: Outcome) {
    if let Some(command) = outcome.command {
        dispatch.command(command);
    }
    for _ in 0..outcome.backs {
        dispatch.back();
    }
    if let Some(route) = outcome.then {
        dispatch.navigate(route);
    }
    if let Some(message) = outcome.message {
        dispatch.ui(UiEvent::Show(message));
    }
}

pub struct ConfirmScreen {
    kind: ConfirmKind,
    status: Option<StatusArea>,
    row: Option<ButtonRow>,
    pending: Rc<Cell<Option<Uuid>>>,
}

impl ConfirmScreen {
    pub fn new(kind: ConfirmKind) -> Self {
        Self {
            kind,
            status: None,
            row: None,
            pending: Rc::new(Cell::new(None)),
        }
    }
}

impl Screen for ConfirmScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        let kind = self.kind;
        let (pending, dispatch) = (self.pending.clone(), ctx.dispatch.clone());
        let cancel = ctx.dispatch.clone();
        self.row = Some(confirm_dialog(
            root,
            &spec(kind, snap),
            move || apply(&dispatch, outcome(kind, pending.get())),
            move || cancel.back(),
        ));
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        self.pending
            .set(snap.meeting.pending_decision.as_ref().map(|d| d.id));
    }
}
```

`Message` derives `PartialEq`, but `Outcome` holds a `Command` (no `PartialEq`), so it derives `Debug` only and the tests use `matches!`.

- [ ] **Step 4: Generate the goldens and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::confirm`
Compare `screen_confirm.png` with `../docs/mockups/png/confirm.png`: status, red disc and icon, "Power off?", the two-line body, Cancel / Power off. The other two have no mockup: check the same layout with a teal Wi-Fi disc ("Change network?"), and a red trash disc and red "Factory reset" words.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::confirm`
Expected: 5 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/confirm.rs crates/waved-ui/golden/screen_confirm*.png
git commit -m "feat(ui): shared confirmation screen

- power off, factory reset, delete recording, change network, stop usb storage
- delete discards the pending recording or deletes a kept one and leaves its detail
- the message after the action repeats its name

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: System list (B6)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/system.rs`
- Create: `crates/waved-ui/golden/screen_system.png`

**Interfaces:**
- Consumes:
  - `RotaryList`, `ListItem`;
  - icons `SUN`, `CALENDAR`, `BELL` (contract 7), `CLOCK`, `INFO`, `TRASH`, `POWER`;
  - `Command::{SetAlertTones(bool), SetPreRecording(bool)}`;
  - routes `Display`, `DateTime`, `About`, `Confirm(FactoryReset)`, `Confirm(PowerOff)`.
- Produces: `pub enum SystemRow { Display, DateTime, AlertTones, PreRecording, About, FactoryReset, PowerOff }`, `pub const ROWS: [SystemRow; 7]`, `pub const START_ROW: usize` (4), `impl SystemRow { pub fn title(self) -> &'static str; pub fn value(self, snap: &UiSnapshot) -> Option<String> }`, `#[derive(Default)] pub struct SystemScreen` (impl `Screen`).

Hidden until milestone 3: USB storage (G1), PIN (K2), Firmware update (H3).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::route::ConfirmKind;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    #[test]
    fn system_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.clock = "14:37".into();
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            // system.png: Pre-recording, About (focus), ..., Power off in red.
            assert_golden("screen_system", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn rows_left_for_milestone_3_are_hidden() {
        assert_eq!(
            ROWS.map(SystemRow::title),
            ["Display", "Date and time", "Alert tones", "Pre-recording", "About", "Factory reset", "Power off"]
        );
    }

    #[test]
    fn switch_rows_toggle_the_live_value() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.settings.pre_recording = true;
            snap.settings.tones_enabled = false;
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            let list = screen.list.as_ref().unwrap();
            lv.click(list.row(3));
            lv.click(list.row(2));
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetPreRecording(false), Command::SetAlertTones(true)]
            ));
            assert_eq!(list.row(3).child(2).child(0).text(), "On");
            snap.settings.pre_recording = false;
            screen.update(&snap);
            assert_eq!(screen.list.as_ref().unwrap().row(3).child(2).child(0).text(), "Off");
        });
    }

    #[test]
    fn destructive_rows_open_the_confirmation() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap());
            let list = screen.list.as_ref().unwrap();
            lv.click(list.row(5));
            lv.click(list.row(6));
            assert!(f.commands().is_empty(), "nothing happens before confirming");
            assert_eq!(
                f.events(),
                vec![
                    UiEvent::Navigate(Route::Confirm(ConfirmKind::FactoryReset)),
                    UiEvent::Navigate(Route::Confirm(ConfirmKind::PowerOff)),
                ]
            );
        });
    }

    #[test]
    fn values_read_the_snapshot() {
        let mut snap = snap();
        snap.settings.brightness_pct = 70;
        assert_eq!(SystemRow::Display.value(&snap).as_deref(), Some("70%"));
        assert_eq!(SystemRow::DateTime.value(&snap).as_deref(), Some("14:33"));
        assert_eq!(
            SystemRow::About.value(&snap),
            Some(format!("waved {}", env!("CARGO_PKG_VERSION")))
        );
        assert_eq!(SystemRow::PowerOff.value(&snap), None);
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::system`
Expected: `cannot find type SystemScreen`.

- [ ] **Step 3: Implement**

```rust
//! System (B6, system.png): the same rotary list as the menu. Rows that
//! aren't built yet (USB storage, PIN, Firmware update) stay hidden until
//! milestone 3. Power off and Factory reset go through the confirmation.

use crate::kit::components::buttons::back_button;
use crate::kit::components::list::{ListItem, RotaryList};
use crate::kit::components::status::StatusArea;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::{ConfirmKind, Route};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemRow {
    Display,
    DateTime,
    AlertTones,
    PreRecording,
    About,
    FactoryReset,
    PowerOff,
}

pub const ROWS: [SystemRow; 7] = [
    SystemRow::Display,
    SystemRow::DateTime,
    SystemRow::AlertTones,
    SystemRow::PreRecording,
    SystemRow::About,
    SystemRow::FactoryReset,
    SystemRow::PowerOff,
];

/// About is centred when System opens, as in system.png.
pub const START_ROW: usize = 4;

fn on_off(on: bool) -> String {
    if on { "On" } else { "Off" }.into()
}

impl SystemRow {
    pub fn title(self) -> &'static str {
        match self {
            SystemRow::Display => "Display",
            SystemRow::DateTime => "Date and time",
            SystemRow::AlertTones => "Alert tones",
            SystemRow::PreRecording => "Pre-recording",
            SystemRow::About => "About",
            SystemRow::FactoryReset => "Factory reset",
            SystemRow::PowerOff => "Power off",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            SystemRow::Display => icons::SUN,
            SystemRow::DateTime => icons::CALENDAR,
            SystemRow::AlertTones => icons::BELL,
            SystemRow::PreRecording => icons::CLOCK,
            SystemRow::About => icons::INFO,
            SystemRow::FactoryReset => icons::TRASH,
            SystemRow::PowerOff => icons::POWER,
        }
    }

    pub fn value(self, snap: &UiSnapshot) -> Option<String> {
        match self {
            SystemRow::Display => {
                let brightness = snap.settings.brightness_pct;
                Some(format!("{brightness}%"))
            }
            SystemRow::DateTime => Some(snap.clock.clone()),
            SystemRow::AlertTones => Some(on_off(snap.settings.tones_enabled)),
            SystemRow::PreRecording => Some(on_off(snap.settings.pre_recording)),
            SystemRow::About => Some(format!("waved {}", env!("CARGO_PKG_VERSION"))),
            SystemRow::FactoryReset | SystemRow::PowerOff => None,
        }
    }
}

#[derive(Default)]
pub struct SystemScreen {
    status: Option<StatusArea>,
    list: Option<RotaryList>,
    shown: Vec<Option<String>>,
    /// (tones_enabled, pre_recording) for the switch rows' taps.
    switches: Rc<Cell<(bool, bool)>>,
}

impl Screen for SystemScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        self.switches
            .set((snap.settings.tones_enabled, snap.settings.pre_recording));
        self.shown = ROWS.iter().map(|row| row.value(snap)).collect();
        let items = ROWS
            .iter()
            .zip(&self.shown)
            .map(|(row, value)| {
                let item = ListItem::new(row.icon(), row.title());
                let item = if *row == SystemRow::PowerOff { item.danger() } else { item };
                match value {
                    Some(value) => item.value(value.as_str()),
                    None => item,
                }
            })
            .collect();
        let (dispatch, switches) = (ctx.dispatch.clone(), self.switches.clone());
        let list = RotaryList::new(root, items, move |index| {
            let (tones, pre) = switches.get();
            match ROWS[index] {
                SystemRow::Display => dispatch.navigate(Route::Display),
                SystemRow::DateTime => dispatch.navigate(Route::DateTime),
                SystemRow::AlertTones => {
                    switches.set((!tones, pre));
                    dispatch.command(Command::SetAlertTones(!tones));
                }
                SystemRow::PreRecording => {
                    switches.set((tones, !pre));
                    dispatch.command(Command::SetPreRecording(!pre));
                }
                SystemRow::About => dispatch.navigate(Route::About),
                SystemRow::FactoryReset => {
                    dispatch.navigate(Route::Confirm(ConfirmKind::FactoryReset))
                }
                SystemRow::PowerOff => dispatch.navigate(Route::Confirm(ConfirmKind::PowerOff)),
            }
        });
        list.scroll_to(START_ROW);
        self.list = Some(list);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let Some(list) = self.list.as_ref() else {
            return;
        };
        for (index, row) in ROWS.iter().enumerate() {
            let value = row.value(snap);
            if self.shown[index] != value {
                list.set_value(index, value.as_deref());
                self.shown[index] = value;
                // A switch value reported by the settings service is the truth again.
                self.switches
                    .set((snap.settings.tones_enabled, snap.settings.pre_recording));
            }
        }
    }
}
```

The test expects "On" right after the tap: the row keeps showing the snapshot value until the settings service reports the change, which takes one UI frame on the device. The second tap reads `switches`, so a double tap within one frame doesn't send the same value twice.

- [ ] **Step 4: Generate the golden and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::system`
Compare with `../docs/mockups/png/system.png`. The focus row reads "About waved 0.1.0" where the mockup focuses on "Firmware update Up to date" (that row is hidden). The far rows are Alert tones / Power off; Power off must be red.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::system`
Expected: 5 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/system.rs crates/waved-ui/golden/screen_system.png
git commit -m "feat(ui): system list

- display, date and time, alert tones, pre-recording, about, factory reset, power off
- switch rows toggle the live setting; destructive rows open the confirmation
- usb storage, pin and firmware update stay hidden until milestone 3

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Display and Dim after (B6)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/display.rs`
- Create: `crates/waved-ui/golden/screen_display.png`, `screen_dim_after.png`

**Interfaces:**
- Consumes:
  - `ValueArc`, `ValueRow`, `RadioList` (+ `scroll_to`, contract 5), `RadioOption`;
  - `volume::{BigNumber, step_button, step_level}`;
  - `waved_config::clamp_off_after`;
  - `Command::{SetBrightness(u8), SetDimAfter(u32)}`;
  - `Route::DimAfter` (contract 1).
- Produces: `pub fn duration_label(seconds: u32) -> String`, `pub fn screen_off_detail(dim_s: u32, off_s: u32) -> String`, `pub const DIM_CHOICES: [u32; 6]`, `pub fn dim_choices(current: u32) -> (Vec<u32>, usize)`, `#[derive(Default)] pub struct DisplayScreen`, `#[derive(Default)] pub struct DimAfterScreen` (both impl `Screen`).

Layout (display.png):
- Title "Display".
- Sun Icon20, Text 2, at (190, 96).
- `BigNumber` at CSS top 114 (Num48 y 125) with a "%" suffix.
- Arc r 68 at (200, 140).
- − / + 52 px at (30, 114) / (318, 114).
- "Dim after" `ValueRow` 220 × 38 at (90, 198), `ext_click(3)` → `Route::DimAfter`.
- Back.

The brightness range is 5–100 (`clamp_brightness`), in steps of 10.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::LV_STATE_PRESSED;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    #[test]
    fn display_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.clock = "14:38".into();
            snap.settings.brightness_pct = 70;
            snap.settings.dim_after_s = 120;
            let mut screen = DisplayScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_display", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn brightness_steps_stay_in_range() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.settings.brightness_pct = 10;
            let mut screen = DisplayScreen::default();
            screen.build(root, &f.ctx, &snap);
            lv.click(screen.minus.unwrap());
            lv.click(screen.minus.unwrap()); // the handler only sees the next snapshot
            lv.click(screen.plus.unwrap());
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetBrightness(5), Command::SetBrightness(5), Command::SetBrightness(20)]
            ));
            let arc = screen.arc.as_ref().unwrap();
            arc.set_value(2);
            lv.send(arc.obj(), crate::bindings::lv_event_code_t_LV_EVENT_VALUE_CHANGED);
            assert!(matches!(f.commands().as_slice(), [Command::SetBrightness(5)]));
            arc.obj().set_state(LV_STATE_PRESSED, false);
        });
    }

    #[test]
    fn dim_after_opens_its_list() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = DisplayScreen::default();
            screen.build(root, &f.ctx, &snap());
            lv.click(screen.dim.as_ref().unwrap().obj());
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::DimAfter)]);
        });
    }

    #[test]
    fn dim_after_matches_its_radio_list() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.settings.dim_after_s = 120;
            snap.settings.off_after_s = 600;
            let mut screen = DimAfterScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_dim_after", &lv.render());
            assert_targets_ok(root);
            let list = screen.list.as_ref().unwrap();
            assert_eq!(list.selected(), 2);
            lv.click(list.row(3));
            assert!(matches!(f.commands().as_slice(), [Command::SetDimAfter(300)]));
        });
    }

    #[test]
    fn durations_and_details_read_plainly() {
        assert_eq!(duration_label(0), "Never");
        assert_eq!(duration_label(30), "30 s");
        assert_eq!(duration_label(120), "2 min");
        assert_eq!(duration_label(7_200), "2 h");
        assert_eq!(screen_off_detail(120, 600), "Screen off after 10 min");
        assert_eq!(screen_off_detail(900, 600), "Screen off after 15 min");
        assert_eq!(screen_off_detail(120, 0), "Screen stays on");
        assert_eq!(dim_choices(45), (vec![45, 30, 60, 120, 300, 600, 0], 0));
        assert_eq!(dim_choices(0).1, 5);
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::display`
Expected: `cannot find type DisplayScreen`.

- [ ] **Step 3: Implement**

```rust
//! Display (B6, display.png): backlight brightness on a value arc and when
//! the resting screen takes over ("Dim after", a radio list). E1 drives
//! /sys/class/leds/lcd-backlight from the brightness setting.

use crate::kit::components::buttons::back_button;
use crate::kit::components::controls::{RadioList, RadioOption, ValueArc, ValueRow};
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, RADIO_TOP, SCREEN_W, TEXT, TEXT_2, TITLE_Y};
use crate::screens_v2::volume::{step_button, step_level, BigNumber};
use crate::bindings::LV_STATE_PRESSED;
use std::cell::Cell;
use std::rc::Rc;
use waved_config::clamp_off_after;
use waved_state::Command;

const SUN: (i32, i32) = (190, 96);
const VALUE_TOP: i32 = 114;
const ARC_CX: i32 = 200;
const ARC_CY: i32 = 140;
const ARC_R: i32 = 68;
const MINUS: Rect = Rect::new(30, 114, 52, 52);
const PLUS: Rect = Rect::new(318, 114, 52, 52);
const DIM_ROW: Rect = Rect::new(90, 198, 220, 38);
const MIN_BRIGHTNESS: u8 = 5;

/// Dim after choices, in seconds; 0 means never.
pub const DIM_CHOICES: [u32; 6] = [30, 60, 120, 300, 600, 0];

pub fn duration_label(seconds: u32) -> String {
    match seconds {
        0 => "Never".into(),
        s if s < 60 => format!("{s} s"),
        s if s < 3_600 => format!("{} min", s / 60),
        s => format!("{} h", s / 3_600),
    }
}

/// When the screen goes fully off after this dim time (the settings service
/// moves "off" out so it never comes before dimming).
pub fn screen_off_detail(dim_s: u32, off_s: u32) -> String {
    match clamp_off_after(dim_s, off_s) {
        0 => "Screen stays on".into(),
        off => format!("Screen off after {}", duration_label(off)),
    }
}

/// The choices with a value set elsewhere (setup page) added first, and the
/// index of `current`.
pub fn dim_choices(current: u32) -> (Vec<u32>, usize) {
    let mut choices = DIM_CHOICES.to_vec();
    match choices.iter().position(|c| *c == current) {
        Some(index) => (choices, index),
        None => {
            choices.insert(0, current);
            (choices, 0)
        }
    }
}

fn brightness_step(value: u8, up: bool) -> u8 {
    (step_level(u16::from(value), up, 100) as u8).max(MIN_BRIGHTNESS)
}

#[derive(Default)]
pub struct DisplayScreen {
    status: Option<StatusArea>,
    arc: Option<ValueArc>,
    value: Option<BigNumber>,
    dim: Option<ValueRow>,
    minus: Option<Obj>,
    plus: Option<Obj>,
    brightness: Rc<Cell<u8>>,
    shown: Option<(u8, u32)>,
}

impl Screen for DisplayScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.brightness.set(snap.settings.brightness_pct);
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Display", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        let dispatch = ctx.dispatch.clone();
        self.arc = Some(ValueArc::new(root, ARC_CX, ARC_CY, ARC_R, move |percent| {
            dispatch.command(Command::SetBrightness(percent.max(MIN_BRIGHTNESS)))
        }));
        Obj::label(root, icons::SUN, Font::Icon20, TEXT_2).pos(SUN.0, SUN.1);
        self.value = Some(BigNumber::new(root, VALUE_TOP));
        for (rect, icon, up) in [(MINUS, icons::MINUS, false), (PLUS, icons::PLUS, true)] {
            let (dispatch, brightness) = (ctx.dispatch.clone(), self.brightness.clone());
            let button = step_button(root, rect, icon, move || {
                dispatch.command(Command::SetBrightness(brightness_step(brightness.get(), up)))
            });
            if up {
                self.plus = Some(button);
            } else {
                self.minus = Some(button);
            }
        }
        // Created after the arc, so the row wins where the arc's box overlaps it.
        let dispatch = ctx.dispatch.clone();
        let dim = ValueRow::new(
            root,
            DIM_ROW,
            "Dim after",
            &duration_label(snap.settings.dim_after_s),
            move || dispatch.navigate(Route::DimAfter),
        );
        dim.obj().ext_click((44 - DIM_ROW.h) / 2);
        self.dim = Some(dim);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let brightness = snap.settings.brightness_pct;
        self.brightness.set(brightness);
        if let Some(arc) = self.arc.as_ref() {
            if !arc.obj().has_state(LV_STATE_PRESSED) && arc.value() != brightness {
                arc.set_value(brightness);
            }
        }
        let next = (brightness, snap.settings.dim_after_s);
        if self.shown == Some(next) {
            return;
        }
        if let (Some(value), Some(dim)) = (self.value.as_ref(), self.dim.as_ref()) {
            value.set(&brightness.to_string(), "%", 0, TEXT);
            dim.set_value(&duration_label(next.1));
        }
        self.shown = Some(next);
    }
}

#[derive(Default)]
pub struct DimAfterScreen {
    status: Option<StatusArea>,
    list: Option<RadioList>,
    choices: Vec<u32>,
    seen: Option<u32>,
}

impl Screen for DimAfterScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Dim after", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        let (choices, selected) = dim_choices(snap.settings.dim_after_s);
        let options = choices
            .iter()
            .map(|&s| RadioOption::new(duration_label(s), screen_off_detail(s, snap.settings.off_after_s)))
            .collect();
        let (dispatch, picks) = (ctx.dispatch.clone(), choices.clone());
        let list = RadioList::new(root, RADIO_TOP, options, selected, move |index| {
            dispatch.command(Command::SetDimAfter(picks[index]))
        });
        list.scroll_to(selected);
        self.list = Some(list);
        self.choices = choices;
        self.seen = Some(snap.settings.dim_after_s);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        // Follow a change made elsewhere; a tap here already moved the radio.
        let current = snap.settings.dim_after_s;
        if self.seen != Some(current) {
            self.seen = Some(current);
            if let (Some(list), Some(index)) = (
                self.list.as_ref(),
                self.choices.iter().position(|c| *c == current),
            ) {
                list.set_selected(index);
            }
        }
    }
}
```

In `brightness_steps_stay_in_range`, the arc is set to 2 and VALUE_CHANGED is sent directly, which exercises the `max(MIN_BRIGHTNESS)` clamp. The last line leaves the arc unpressed for the next test.

- [ ] **Step 4: Generate the goldens and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::display`
Compare `screen_display.png` with `../docs/mockups/png/display.png`: sun, "70%", arc fill to 70 % with knob, − / +, the Dim after row with "2 min" and chevron, Back. `screen_dim_after.png` follows the `upload-mode.png` radio-list look, with "2 min" selected, three rows visible and the rest scrolled.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::display`
Expected: 5 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/display.rs crates/waved-ui/golden/screen_display.png crates/waved-ui/golden/screen_dim_after.png
git commit -m "feat(ui): display and dim after screens

- brightness arc and steps clamped to 5-100
- dim after radio list with the screen-off time for each choice
- a value set on the setup page is kept as its own choice

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Date and time and Time zone (B6)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/datetime.rs`
- Create: `crates/waved-ui/golden/screen_datetime.png`, `screen_time_zone.png`

**Interfaces:**
- Consumes:
  - `ValueRow`, `SwitchRow`, `RadioList` (+ `scroll_to`), `RadioOption`;
  - `volume::BigNumber`;
  - `UiSnapshot::local_unix_s` (contract 2);
  - `Command::{SetTimeZone(String), SetClock24h(bool)}`;
  - `Route::TimeZone` (contract 1).
- Produces:
  ```rust
  pub(crate) fn civil(days: i64) -> (i64, u32, u32);
  pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64;
  pub fn date_line(local_s: i64) -> String;
  pub fn clock_parts(local_s: i64, clock_24h: bool) -> (String, &'static str);
  pub fn zone_label(zone: &str) -> String;
  pub const ZONES: [&str; 29];
  pub fn zone_choices(current: &str) -> (Vec<String>, usize);
  #[derive(Default)] pub struct DateTimeScreen; #[derive(Default)] pub struct TimeZoneScreen; // impl Screen
  ```

Layout (datetime.png):
- Title "Date and time".
- Time: `BigNumber` at CSS top 62 (Num48 y 73). In 12-hour mode it gets an "am"/"pm" Num20 suffix 6 px after the digits.
- Date (Body, Text 2) at top 114.
- "Time zone" `ValueRow` 280 × 44 at (60, 140) → `Route::TimeZone`.
- "24-hour clock" `SwitchRow` 280 × 44 at (60, 190).
- Back.

The time comes from the network (or the RTC, E3), so only the zone and the format are settable.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    /// 12:37 UTC on Saturday 4 October 2025 is 14:37 in Brussels (UTC+2).
    fn brussels() -> UiSnapshot {
        let mut snap = snap();
        snap.clock = "14:37".into();
        snap.now_unix_s = 1_759_581_420;
        snap.utc_offset_min = 120;
        snap.settings.time_zone = "Europe/Brussels".into();
        snap.settings.clock_24h = true;
        snap
    }

    #[test]
    fn datetime_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = DateTimeScreen::default();
            screen.build(root, &f.ctx, &brussels());
            assert_golden("screen_datetime", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.time.as_ref().unwrap().digits().text(), "14:37");
            assert_eq!(screen.date.unwrap().text(), "Saturday 4 October");
        });
    }

    #[test]
    fn civil_dates_round_trip() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(civil(20_365), (2025, 10, 4));
        assert_eq!(civil(19_782), (2024, 2, 29));
        for days in [-800_000, -1, 0, 59, 19_782, 20_365, 2_932_896] {
            let (y, m, d) = civil(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(date_line(1_759_588_620), "Saturday 4 October");
        assert_eq!(date_line(0), "Thursday 1 January");
    }

    #[test]
    fn clocks_follow_the_format() {
        let local = 1_759_588_620; // 14:37
        assert_eq!(clock_parts(local, true), ("14:37".to_string(), ""));
        assert_eq!(clock_parts(local, false), ("2:37".to_string(), "pm"));
        assert_eq!(clock_parts(0, false), ("12:00".to_string(), "am"));
        assert_eq!(clock_parts(-60, true), ("23:59".to_string(), ""));
    }

    #[test]
    fn the_switch_and_the_zone_row_work() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = DateTimeScreen::default();
            screen.build(root, &f.ctx, &brussels());
            lv.click(screen.clock.as_ref().unwrap().obj());
            assert!(matches!(f.commands().as_slice(), [Command::SetClock24h(false)]));
            lv.click(screen.zone.as_ref().unwrap().obj());
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::TimeZone)]);
            // A stale snapshot does not flip the switch back.
            screen.update(&brussels());
            assert!(!screen.clock.as_ref().unwrap().is_on());
        });
    }

    #[test]
    fn time_zone_matches_its_radio_list() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = TimeZoneScreen::default();
            screen.build(root, &f.ctx, &brussels());
            assert_golden("screen_time_zone", &lv.render());
            assert_targets_ok(root);
            let list = screen.list.as_ref().unwrap();
            assert_eq!(list.selected(), 4);
            lv.click(list.row(5));
            assert!(matches!(f.commands().as_slice(), [Command::SetTimeZone(z)] if z == "Europe/Amsterdam"));
        });
    }

    #[test]
    fn zones_read_as_cities() {
        assert_eq!(zone_label("Europe/Brussels"), "Brussels");
        assert_eq!(zone_label("America/New_York"), "New York");
        assert_eq!(zone_label("UTC"), "UTC");
        let (zones, index) = zone_choices("America/Argentina/Buenos_Aires");
        assert_eq!((zones[0].as_str(), index), ("America/Argentina/Buenos_Aires", 0));
        assert_eq!(zone_label(&zones[0]), "Buenos Aires");
        assert!(ZONES.iter().all(|z| waved_config::valid_time_zone(z)));
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::datetime`
Expected: `cannot find type DateTimeScreen`.

- [ ] **Step 3: Implement**

```rust
//! Date and time (B6, datetime.png) and the Time zone radio list. The time
//! itself comes from the network or the RTC (E3); only the zone and the clock
//! format are settable here.

use crate::kit::components::buttons::back_button;
use crate::kit::components::controls::{RadioList, RadioOption, SwitchRow, ValueRow};
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, RADIO_TOP, SCREEN_W, TEXT, TEXT_2, TITLE_Y};
use crate::screens_v2::volume::BigNumber;
use waved_state::Command;

const TIME_TOP: i32 = 62;
const DATE_Y: i32 = 114;
const ZONE_ROW: Rect = Rect::new(60, 140, 280, 44);
const CLOCK_ROW: Rect = Rect::new(60, 190, 280, 44);
const SUFFIX_GAP: i32 = 6;

const WEEKDAYS: [&str; 7] = [
    "Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday",
];
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August",
    "September", "October", "November", "December",
];

/// Zones offered on the device; a zone set on the setup page is added to them.
pub const ZONES: [&str; 29] = [
    "UTC",
    "Europe/London",
    "Europe/Dublin",
    "Europe/Lisbon",
    "Europe/Brussels",
    "Europe/Amsterdam",
    "Europe/Paris",
    "Europe/Berlin",
    "Europe/Madrid",
    "Europe/Rome",
    "Europe/Zurich",
    "Europe/Stockholm",
    "Europe/Warsaw",
    "Europe/Athens",
    "Europe/Helsinki",
    "Europe/Istanbul",
    "Asia/Dubai",
    "Asia/Kolkata",
    "Asia/Singapore",
    "Asia/Hong_Kong",
    "Asia/Tokyo",
    "Australia/Sydney",
    "Pacific/Auckland",
    "America/Sao_Paulo",
    "America/New_York",
    "America/Toronto",
    "America/Chicago",
    "America/Denver",
    "America/Los_Angeles",
];

/// (year, month 1–12, day 1–31) of a day count since 1970-01-01 (H. Hinnant).
pub(crate) fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Days since 1970-01-01 of a civil date (H. Hinnant).
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "Saturday 4 October".
pub fn date_line(local_s: i64) -> String {
    let days = local_s.div_euclid(86_400);
    let (_, month, day) = civil(days);
    let weekday = WEEKDAYS[(days + 4).rem_euclid(7) as usize];
    let month_name = MONTHS[month as usize - 1];
    format!("{weekday} {day} {month_name}")
}

/// ("14:37", "") or ("2:37", "pm").
pub fn clock_parts(local_s: i64, clock_24h: bool) -> (String, &'static str) {
    let minutes = local_s.rem_euclid(86_400) / 60;
    let (hour, minute) = (minutes / 60, minutes % 60);
    if clock_24h {
        (format!("{hour:02}:{minute:02}"), "")
    } else {
        let hour12 = if hour % 12 == 0 { 12 } else { hour % 12 };
        (format!("{hour12}:{minute:02}"), if hour < 12 { "am" } else { "pm" })
    }
}

/// The city of an IANA zone: "Europe/Brussels" → "Brussels".
pub fn zone_label(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).replace('_', " ")
}

pub fn zone_choices(current: &str) -> (Vec<String>, usize) {
    let mut zones: Vec<String> = ZONES.iter().map(|z| z.to_string()).collect();
    match zones.iter().position(|z| z == current) {
        Some(index) => (zones, index),
        None => {
            zones.insert(0, current.to_string());
            (zones, 0)
        }
    }
}

#[derive(Default)]
pub struct DateTimeScreen {
    status: Option<StatusArea>,
    time: Option<BigNumber>,
    date: Option<Obj>,
    zone: Option<ValueRow>,
    clock: Option<SwitchRow>,
    shown: Option<(String, &'static str, String, String)>,
    seen_24h: Option<bool>,
}

impl Screen for DateTimeScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Date and time", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        self.time = Some(BigNumber::new(root, TIME_TOP));
        self.date = Some(
            Obj::label(root, "", Font::Body, TEXT_2)
                .centered_text(SCREEN_W)
                .pos(0, DATE_Y),
        );
        let dispatch = ctx.dispatch.clone();
        self.zone = Some(ValueRow::new(
            root,
            ZONE_ROW,
            "Time zone",
            &zone_label(&snap.settings.time_zone),
            move || dispatch.navigate(Route::TimeZone),
        ));
        let dispatch = ctx.dispatch.clone();
        self.clock = Some(SwitchRow::new(
            root,
            CLOCK_ROW,
            "24-hour clock",
            snap.settings.clock_24h,
            move |on| dispatch.command(Command::SetClock24h(on)),
        ));
        self.seen_24h = Some(snap.settings.clock_24h);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        // Follow a format change made elsewhere; a tap already moved the switch.
        if self.seen_24h != Some(snap.settings.clock_24h) {
            self.seen_24h = Some(snap.settings.clock_24h);
            if let Some(clock) = self.clock.as_ref() {
                clock.set_on(snap.settings.clock_24h);
            }
        }
        let local = snap.local_unix_s();
        let (digits, suffix) = clock_parts(local, snap.settings.clock_24h);
        let next = (digits, suffix, date_line(local), zone_label(&snap.settings.time_zone));
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        if let (Some(time), Some(date), Some(zone)) = (self.time.as_ref(), self.date, self.zone.as_ref()) {
            time.set(&next.0, next.1, SUFFIX_GAP, TEXT);
            date.set_text(&next.2);
            zone.set_value(&next.3);
        }
        self.shown = Some(next);
    }
}

#[derive(Default)]
pub struct TimeZoneScreen {
    status: Option<StatusArea>,
    list: Option<RadioList>,
    zones: Vec<String>,
    seen: Option<String>,
}

impl Screen for TimeZoneScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Time zone", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        let (zones, selected) = zone_choices(&snap.settings.time_zone);
        let options = zones
            .iter()
            .map(|zone| RadioOption::new(zone_label(zone), zone.as_str()))
            .collect();
        let (dispatch, picks) = (ctx.dispatch.clone(), zones.clone());
        let list = RadioList::new(root, RADIO_TOP, options, selected, move |index| {
            dispatch.command(Command::SetTimeZone(picks[index].clone()))
        });
        list.scroll_to(selected);
        self.list = Some(list);
        self.zones = zones;
        self.seen = Some(snap.settings.time_zone.clone());
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        if self.seen.as_deref() != Some(snap.settings.time_zone.as_str()) {
            self.seen = Some(snap.settings.time_zone.clone());
            if let (Some(list), Some(index)) = (
                self.list.as_ref(),
                self.zones.iter().position(|z| *z == snap.settings.time_zone),
            ) {
                list.set_selected(index);
            }
        }
    }
}
```

- [ ] **Step 4: Generate the goldens and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::datetime`
Compare `screen_datetime.png` with `../docs/mockups/png/datetime.png`: "14:37" on its baseline, "Saturday 4 October", the Time zone row with "Brussels", the switch on (teal), Back. In `screen_time_zone.png`, "Brussels" (detail "Europe/Brussels") must be visible and selected.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::datetime`
Expected: 6 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/datetime.rs crates/waved-ui/golden/screen_datetime.png crates/waved-ui/golden/screen_time_zone.png
git commit -m "feat(ui): date and time and time zone screens

- local time and date from the snapshot's zone offset, 12/24-hour switch
- time zone radio list of city names; a zone set elsewhere is kept
- civil date helpers shared with uploads

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: About (B6, E7 UI)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/about.rs`
- Create: `crates/waved-ui/golden/screen_about.png`

**Interfaces:**
- Consumes: `snap.device.serial` (contract 3), `snap.wifi`, `snap.files.free_bytes`, `ctx.config.device.name`.
- Produces: `pub enum AboutRow { Name, Version, Serial, Address, Space }`, `pub fn about_value(row: AboutRow, snap: &UiSnapshot, name: &str) -> Option<String>`, `pub fn format_bytes(bytes: u64) -> String`, `#[derive(Default)] pub struct AboutScreen` (impl `Screen`).

There is no mockup. The layout follows the settings rhythm:
- Title "About".
- Information rows are not tappable: 280 × 30 at x 60 from y 66, every 32 px.
- Each row has a label (Small, Text 2) on the left and a value (Body, Text, one line, capped at 160 px) on the right.
- Back.

The Serial row is left out while the serial is unknown.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture_with, snap};
    use waved_config::WavedConfig;

    fn config(name: &str) -> WavedConfig {
        let mut config = WavedConfig::default();
        config.device.name = name.into();
        config
    }

    #[test]
    fn about_lists_the_device() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture_with(config("Wave Boardroom"));
            let mut snap = snap();
            snap.device.serial = "IW1A2B3C4D".into();
            snap.files.free_bytes = 3_200_000_000;
            let mut screen = AboutScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_about", &lv.render());
            assert_targets_ok(root);
            let values: Vec<String> = screen.rows.iter().map(|(_, v)| v.text()).collect();
            assert_eq!(
                values,
                vec![
                    "Wave Boardroom".to_string(),
                    format!("waved {}", env!("CARGO_PKG_VERSION")),
                    "IW1A2B3C4D".into(),
                    "192.168.1.42".into(),
                    "3.2 GB".into(),
                ]
            );
        });
    }

    #[test]
    fn unknown_values_read_plainly() {
        let mut snap = snap();
        assert_eq!(about_value(AboutRow::Serial, &snap, ""), None);
        assert_eq!(about_value(AboutRow::Name, &snap, "  ").as_deref(), Some("Wave"));
        snap.wifi.connected = false;
        assert_eq!(about_value(AboutRow::Address, &snap, "").as_deref(), Some("Not connected"));
        assert_eq!(format_bytes(850_000_000), "850 MB");
        assert_eq!(format_bytes(0), "0 MB");
    }

    #[test]
    fn a_long_device_name_ends_in_dots() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture_with(config("The large meeting room on the third floor, east wing"));
            let mut screen = AboutScreen::default();
            screen.build(root, &f.ctx, &snap());
            root.update_layout();
            let value = screen.rows[0].1;
            assert!(value.text().ends_with("..."), "{}", value.text());
            assert!(value.coords().x2 <= 340 - 16);
        });
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::about`
Expected: `cannot find type AboutScreen`.

- [ ] **Step 3: Implement**

```rust
//! About (B6, E7): device name, firmware version, serial, address and free
//! space. There is no mockup; it uses the settings rhythm (rows under the
//! title, Back at the bottom).

use crate::kit::components::buttons::back_button;
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::obj::{text_width, Obj};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, SCREEN_W, TEXT, TEXT_2, TITLE_Y};

const ROW_X: i32 = 60;
const ROW_TOP: i32 = 66;
const ROW_PITCH: i32 = 32;
const ROW_W: i32 = 280;
const ROW_H: i32 = 30;
const VALUE_CAP: i32 = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AboutRow {
    Name,
    Version,
    Serial,
    Address,
    Space,
}

const ROWS: [AboutRow; 5] = [
    AboutRow::Name,
    AboutRow::Version,
    AboutRow::Serial,
    AboutRow::Address,
    AboutRow::Space,
];

impl AboutRow {
    fn label(self) -> &'static str {
        match self {
            AboutRow::Name => "Name",
            AboutRow::Version => "Version",
            AboutRow::Serial => "Serial",
            AboutRow::Address => "IP address",
            AboutRow::Space => "Free space",
        }
    }
}

/// "3.2 GB" or "850 MB" (decimal, as Files shows free space).
pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else {
        format!("{} MB", bytes / 1_000_000)
    }
}

/// The row's value; `None` leaves the row out.
pub fn about_value(row: AboutRow, snap: &UiSnapshot, name: &str) -> Option<String> {
    match row {
        AboutRow::Name => Some(if name.trim().is_empty() { "Wave".into() } else { name.trim().into() }),
        AboutRow::Version => Some(format!("waved {}", env!("CARGO_PKG_VERSION"))),
        AboutRow::Serial => (!snap.device.serial.is_empty()).then(|| snap.device.serial.clone()),
        AboutRow::Address => Some(if snap.wifi.connected && !snap.wifi.ip_addr.is_empty() {
            snap.wifi.ip_addr.clone()
        } else {
            "Not connected".into()
        }),
        AboutRow::Space => Some(format_bytes(snap.files.free_bytes)),
    }
}

#[derive(Default)]
pub struct AboutScreen {
    status: Option<StatusArea>,
    name: String,
    rows: Vec<(AboutRow, Obj)>,
    /// The text each value shows (LVGL keeps dotted text in its buffer).
    shown: Vec<String>,
}

impl Screen for AboutScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "About", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        // device.name only changes through the setup page, which restarts waved.
        self.name = ctx.config.device.name.clone();
        let visible = ROWS
            .iter()
            .filter(|row| about_value(**row, snap, &self.name).is_some());
        for (index, row) in visible.enumerate() {
            let y = ROW_TOP + index as i32 * ROW_PITCH;
            let line = Obj::plain(root)
                .rect(Rect::new(ROW_X, y, ROW_W, ROW_H))
                .row_start(8, 16);
            Obj::label(line, row.label(), Font::Small, TEXT_2).grow().one_line();
            let value = Obj::label(line, "", Font::Body, TEXT).one_line();
            self.rows.push((*row, value));
            self.shown.push(String::new());
        }
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        for ((row, value), shown) in self.rows.iter().zip(self.shown.iter_mut()) {
            let text = about_value(*row, snap, &self.name).unwrap_or_default();
            if *shown != text {
                let width = text_width(&text, Font::Body).min(VALUE_CAP);
                value.width(width).set_text(&text);
                *shown = text;
            }
        }
    }
}
```

- [ ] **Step 4: Generate the golden and review**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::about`
Review `target/ui-preview/screen_about.png` against the settings look (`datetime.png` rows): five rows inside the visible circle, values right-aligned, Back clear.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::about`
Expected: 3 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/about.rs crates/waved-ui/golden/screen_about.png
git commit -m "feat(ui): about screen

- name, version, serial, ip address and free space
- the serial row appears once e7 reports one; long names end in dots

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: Wi-Fi kit pieces: signal arcs and QR panel

**Files:**
- Modify: `crates/waved-ui/src/kit/components/wifi.rs` (stub from M2.0, contract 8)
- Create: `crates/waved-ui/golden/wifi_signal_3.png`, `wifi_qr.png`

**Interfaces:**
- Consumes: `components::dial::{arc, arc_style, MAIN, INDICATOR}` (`pub(super)`, visible inside `kit::components`), `tokens::lvgl_angle`, LVGL `lv_arc_set_bg_angles`, `lv_qrcode_*`.
- Produces:
  ```rust
  pub const SIGNAL_RADII: [i32; 4]; // [9, 22, 35, 48]
  pub struct SignalArcs;
  impl SignalArcs { pub fn new(parent: Obj, cx: i32, cy: i32) -> Self; pub fn set_bars(&self, lit: u8); pub fn arc(&self, index: usize) -> Obj }
  pub struct Qr;
  impl Qr { pub fn new(parent: Obj, rect: Rect, quiet: i32) -> Self; pub fn set_data(&self, data: &str) -> bool; pub fn obj(&self) -> Obj }
  ```

Signal arcs (wifi.png): four arcs around (cx, cy) at r 9 / 22 / 35 / 48, from −42° to 42°, stroke 5, round caps. The inner `lit` arcs are Live and the rest Line.

QR: a white panel with radius 8, the code inset by `quiet` px, dark modules black.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, lvgl};
    use crate::kit::tokens::polar;

    #[test]
    fn signal_arcs_light_from_the_inside() {
        lvgl(|lv| {
            let root = lv.screen();
            let arcs = SignalArcs::new(root, 200, 126);
            arcs.set_bars(3);
            let frame = lv.render();
            assert_golden("wifi_signal_3", &frame);
            let teal = |[r, g, b]: [u8; 3]| g > 150 && g > r + 60 && b > 100;
            for (radius, lit) in [(9, true), (22, true), (35, true), (48, false)] {
                let (x, y) = polar(200, 126, radius as f32, 0.0);
                assert_eq!(teal(frame.pixel(x, y)), lit, "r {radius}");
            }
            // Nothing below the arcs' ends (±42°).
            let (x, y) = polar(200, 126, 48.0, 60.0);
            assert_eq!(frame.pixel(x, y), [0, 0, 0]);
        });
    }

    #[test]
    fn the_qr_panel_encodes_and_rejects_oversized_data() {
        lvgl(|lv| {
            let root = lv.screen();
            let qr = Qr::new(root, Rect::new(68, 68, 120, 120), 8);
            assert!(qr.set_data("WIFI:T:WPA;S:Wave-7F2A;P:k3x9p2vw;H:false;;"));
            let frame = lv.render();
            assert_golden("wifi_qr", &frame);
            assert_eq!(frame.pixel(72, 128), [255, 255, 255], "quiet zone");
            assert!(!qr.set_data(&"x".repeat(5_000)));
            assert!(!qr.obj().is_clickable());
        });
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib kit::components::wifi`
Expected: `cannot find type SignalArcs`.

- [ ] **Step 3: Implement**

```rust
//! Wi-Fi pieces that need raw LVGL: the drawn signal arcs (wifi.png) and the
//! QR panel (wifi-setup.png).

use crate::bindings::*;
use crate::kit::components::dial::{arc, arc_style, INDICATOR, MAIN};
use crate::kit::obj::{color, Obj};
use crate::kit::tokens::{lvgl_angle, Rect, BG, LINE, LIVE, WHITE};

/// Arc radii, innermost first.
pub const SIGNAL_RADII: [i32; 4] = [9, 22, 35, 48];
const SIGNAL_SPAN: f32 = 42.0;
const SIGNAL_WIDTH: i32 = 5;
const QR_RADIUS: i32 = 8;

pub struct SignalArcs {
    arcs: Vec<Obj>,
}

impl SignalArcs {
    pub fn new(parent: Obj, cx: i32, cy: i32) -> Self {
        let arcs = SIGNAL_RADII
            .iter()
            .map(|&r| {
                let obj = arc(parent, cx, cy, r, SIGNAL_WIDTH);
                // SAFETY: live arc.
                unsafe {
                    lv_arc_set_bg_angles(
                        obj.raw(),
                        lvgl_angle(-SIGNAL_SPAN),
                        lvgl_angle(SIGNAL_SPAN),
                    );
                }
                arc_style(obj, MAIN, LINE, 255, SIGNAL_WIDTH);
                arc_style(obj, INDICATOR, LINE, 0, SIGNAL_WIDTH);
                obj
            })
            .collect();
        Self { arcs }
    }

    /// Light the inner `lit` arcs (0–4) in Live; the rest are Line.
    pub fn set_bars(&self, lit: u8) {
        for (index, obj) in self.arcs.iter().enumerate() {
            let hex = if index < usize::from(lit) { LIVE } else { LINE };
            arc_style(*obj, MAIN, hex, 255, SIGNAL_WIDTH);
        }
    }

    pub fn arc(&self, index: usize) -> Obj {
        self.arcs[index]
    }
}

pub struct Qr {
    panel: Obj,
    code: Obj,
}

impl Qr {
    /// A white rounded panel at `rect` with the code inset `quiet` px.
    pub fn new(parent: Obj, rect: Rect, quiet: i32) -> Self {
        let panel = Obj::plain(parent).rect(rect).bg(WHITE).radius(QR_RADIUS);
        // SAFETY: panel is live; the qrcode object belongs to it.
        let code = unsafe { Obj::from_raw(lv_qrcode_create(panel.raw())) };
        // SAFETY: live qrcode.
        unsafe {
            lv_obj_remove_style(
                code.raw(),
                std::ptr::null(),
                (LV_PART_ANY | LV_STATE_ANY) as lv_style_selector_t,
            );
            lv_qrcode_set_size(code.raw(), rect.w - 2 * quiet);
            lv_qrcode_set_dark_color(code.raw(), color(BG));
            lv_qrcode_set_light_color(code.raw(), color(WHITE));
        }
        let code = code.clickable(false).center();
        Self { panel, code }
    }

    /// Encode `data`; false when it doesn't fit in a QR code.
    pub fn set_data(&self, data: &str) -> bool {
        // SAFETY: live qrcode; LVGL reads `data.len()` bytes and copies the result.
        let result = unsafe {
            lv_qrcode_update(self.code.raw(), data.as_ptr().cast(), data.len() as u32)
        };
        result == lv_result_t_LV_RESULT_OK
    }

    /// The panel (show or hide it as a whole).
    pub fn obj(&self) -> Obj {
        self.panel
    }
}
```

- [ ] **Step 4: Generate the goldens and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib kit::components::wifi`
Compare `wifi_signal_3.png` with the arcs in `../docs/mockups/png/wifi.png`: geometry, stroke and caps. The lit set intentionally differs; see "Open points". `wifi_qr.png` must show a white rounded square with a code.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib kit::components::wifi`
Expected: 2 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/kit/components/wifi.rs crates/waved-ui/golden/wifi_signal_3.png crates/waved-ui/golden/wifi_qr.png
git commit -m "feat(ui): wi-fi signal arcs and qr panel components

- four drawn signal arcs lit from the inside
- white qr panel with a quiet zone; oversized data is rejected

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Wi-Fi (B7)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/wifi.rs`
- Create: `crates/waved-ui/golden/screen_wifi.png`, `screen_wifi_off.png`

**Interfaces:**
- Consumes:
  - `SignalArcs` (Task 12), `text_button`;
  - `Route::{WifiSetup, Confirm(ConfirmKind::ChangeNetwork)}` (contract 1);
  - `WifiState`.
- Produces: `pub fn bars_for(rssi: i8, connected: bool) -> u8`, `pub fn signal_word(rssi: i8) -> &'static str`, `pub struct WifiView { pub bars: u8, pub name: String, pub line: String, pub live: bool, pub address: Option<String>, pub action: &'static str }`, `pub fn wifi_view(wifi: &WifiState) -> WifiView`, `#[derive(Default)] pub struct WifiScreen` (impl `Screen`).

Layout (wifi.png):
- Title "Wi-Fi".
- Signal arcs around (200, 126).
- Network name (Title, Text) centred across 300 at (50, 134), ending in "...".
- Status line (Small) at (50, 158), 300 wide: Live when connected, Text 2 otherwise.
- IP (Num13, Text 3) at (50, 174), 300 wide.
- Action pill 160 × 44 at (120, 192): "Change network" when connected (opens the warning), "Set up Wi-Fi" otherwise (opens setup directly; nothing to drop).
- Back.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::route::ConfirmKind;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    #[test]
    fn wifi_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.clock = "14:34".into();
            let mut screen = WifiScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_wifi", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.line.unwrap().text(), "Connected, signal good");
        });
    }

    #[test]
    fn not_connected_offers_setup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.wifi = Default::default();
            let mut screen = WifiScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_wifi_off", &lv.render());
            lv.click(screen.button.unwrap());
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::WifiSetup)]);
        });
    }

    #[test]
    fn change_network_warns_first() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = WifiScreen::default();
            screen.build(root, &f.ctx, &snap());
            lv.click(screen.button.unwrap());
            assert!(f.commands().is_empty(), "the station is not touched yet");
            assert_eq!(
                f.events(),
                vec![UiEvent::Navigate(Route::Confirm(ConfirmKind::ChangeNetwork))]
            );
        });
    }

    #[test]
    fn a_long_network_name_ends_in_dots() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.wifi.ssid = "Guest network of the third floor".into(); // 32 characters
            let mut screen = WifiScreen::default();
            screen.build(root, &f.ctx, &snap);
            root.update_layout();
            let name = screen.name.unwrap();
            assert!(name.text().ends_with("..."), "{}", name.text());
            let a = name.coords();
            assert!(a.x1 >= 50 && a.x2 <= 349);
        });
    }

    #[test]
    fn signal_thresholds() {
        assert_eq!(bars_for(-50, true), 4);
        assert_eq!(bars_for(-60, true), 3);
        assert_eq!(bars_for(-70, true), 2);
        assert_eq!(bars_for(-80, true), 1);
        assert_eq!(bars_for(-50, false), 0);
        assert_eq!(signal_word(-60), "good");
        assert_eq!(signal_word(-80), "weak");
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::wifi::tests`
Expected: `cannot find type WifiScreen`.

- [ ] **Step 3: Implement**

```rust
//! Wi-Fi (B7, wifi.png): the live connection with drawn signal arcs. Opening
//! it never drops the network. "Change network" warns first, because Wave
//! leaves the network while the setup network is open.

use crate::kit::components::buttons::{back_button, text_button};
use crate::kit::components::status::StatusArea;
use crate::kit::components::wifi::SignalArcs;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::route::{ConfirmKind, Route};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, LIVE, SCREEN_W, TEXT, TEXT_2, TEXT_3, TITLE_Y};
use std::cell::Cell;
use std::rc::Rc;
use waved_state::WifiState;

const ARCS: (i32, i32) = (200, 126);
const TEXT_X: i32 = 50;
const TEXT_W: i32 = 300;
const NAME_Y: i32 = 134;
const LINE_Y: i32 = 158;
const ADDRESS_Y: i32 = 174;
const ACTION: Rect = Rect::new(120, 192, 160, 44);

pub fn bars_for(rssi: i8, connected: bool) -> u8 {
    match (connected, rssi) {
        (false, _) => 0,
        (true, r) if r >= -55 => 4,
        (true, r) if r >= -67 => 3,
        (true, r) if r >= -75 => 2,
        _ => 1,
    }
}

pub fn signal_word(rssi: i8) -> &'static str {
    match bars_for(rssi, true) {
        4 => "excellent",
        3 => "good",
        2 => "fair",
        _ => "weak",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WifiView {
    pub bars: u8,
    pub name: String,
    pub line: String,
    pub live: bool,
    pub address: Option<String>,
    pub action: &'static str,
}

pub fn wifi_view(wifi: &WifiState) -> WifiView {
    if wifi.connected {
        WifiView {
            bars: bars_for(wifi.signal_strength, true),
            name: if wifi.ssid.is_empty() { "Connected".into() } else { wifi.ssid.clone() },
            line: format!("Connected, signal {}", signal_word(wifi.signal_strength)),
            live: true,
            address: (!wifi.ip_addr.is_empty()).then(|| wifi.ip_addr.clone()),
            action: "Change network",
        }
    } else {
        WifiView {
            bars: 0,
            name: "Not connected".into(),
            line: "No Wi-Fi network".into(),
            live: false,
            address: None,
            action: "Set up Wi-Fi",
        }
    }
}

#[derive(Default)]
pub struct WifiScreen {
    status: Option<StatusArea>,
    arcs: Option<SignalArcs>,
    name: Option<Obj>,
    line: Option<Obj>,
    address: Option<Obj>,
    button: Option<Obj>,
    connected: Rc<Cell<bool>>,
    shown: Option<WifiView>,
}

impl Screen for WifiScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Wi-Fi", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        self.arcs = Some(SignalArcs::new(root, ARCS.0, ARCS.1));
        let line = |y, font, hex| {
            Obj::label(root, "", font, hex)
                .centered_text(TEXT_W)
                .pos(TEXT_X, y)
        };
        self.name = Some(line(NAME_Y, Font::Title, TEXT));
        self.line = Some(line(LINE_Y, Font::Small, TEXT_2));
        self.address = Some(line(ADDRESS_Y, Font::Num13, TEXT_3));
        let (dispatch, connected) = (ctx.dispatch.clone(), self.connected.clone());
        self.button = Some(text_button(root, ACTION, "", move || {
            if connected.get() {
                dispatch.navigate(Route::Confirm(ConfirmKind::ChangeNetwork));
            } else {
                dispatch.navigate(Route::WifiSetup);
            }
        }));
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        self.connected.set(snap.wifi.connected);
        let view = wifi_view(&snap.wifi);
        if self.shown.as_ref() == Some(&view) {
            return;
        }
        if let (Some(arcs), Some(name), Some(line), Some(address), Some(button)) =
            (self.arcs.as_ref(), self.name, self.line, self.address, self.button)
        {
            arcs.set_bars(view.bars);
            name.set_text(&view.name);
            line.text_color(if view.live { LIVE } else { TEXT_2 }).set_text(&view.line);
            address
                .hidden(view.address.is_none())
                .set_text(view.address.as_deref().unwrap_or(""));
            button.child(0).set_text(view.action);
        }
        self.shown = Some(view);
    }
}
```

- [ ] **Step 4: Generate the goldens and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::wifi::tests`
Compare `screen_wifi.png` with `../docs/mockups/png/wifi.png`: arcs, "Office-5G", the teal status line, the IP, the pill, Back. `screen_wifi_off.png`: grey arcs, "Not connected", "No Wi-Fi network", "Set up Wi-Fi".

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::wifi::tests`
Expected: 5 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/wifi.rs crates/waved-ui/golden/screen_wifi.png crates/waved-ui/golden/screen_wifi_off.png
git commit -m "feat(ui): wi-fi screen

- drawn signal arcs, network name with dots, status and address
- change network goes through a warning; set up wi-fi when not connected

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: Wi-Fi setup (B7)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/wifi_setup.rs`
- Create: `crates/waved-ui/golden/screen_wifi_setup.png`, `screen_wifi_setup_failed.png`

**Interfaces:**
- Consumes:
  - `Qr` (Task 12), `text_button`;
  - `Command::{OpenWifiProvisioning, CancelWifiProvisioning}` (contract 9);
  - `ProvisioningState`;
  - `obj::{text_width, SIZE_CONTENT}`.
- Produces: `pub fn wifi_qr_payload(ssid: &str, passphrase: &str) -> String`, `pub enum SetupView { Starting, Code { ssid: String, passphrase: String, host: String }, Busy(String), Failed(String) }`, `pub fn view(state: &ProvisioningState, starting: bool) -> SetupView`, `pub fn should_start(state: &ProvisioningState) -> bool`, `#[derive(Default)] pub struct WifiSetupScreen` (impl `Screen`).

Layout (wifi-setup.png):
- Title "Set up Wi-Fi".
- QR panel 120 × 120 at (68, 68), quiet 8.
- Numbered steps at (202, 72), 140 wide. The number is Small Live, the text Small Text wrapping at 124, 8 px between steps.
- Manual line 1 at y 198, centred: "No camera? Join" (Text 2) plus the SSID (Text, capped at 160 px).
- Manual line 2 at y 216: "password" (Text 2) plus the passphrase with its comma (Text, capped at 140 px) plus "then open 10.42.0.1" (Text 2, capped at 160 px).
- A Cancel pill at `BACK`.
- Messages (Starting, Busy, Failed) use a Body label wrapped at 260 at (70, 100), at most 3 lines. Failed adds "Try again" (pill 160 × 44 at (120, 192)).

Provisioning starts when the screen is built, unless it is already running. That way every caller (Wi-Fi, the Change network warning, A's First start) just navigates here.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    fn ap(ssid: &str, passphrase: &str) -> ProvisioningState {
        ProvisioningState::ApActive {
            ssid: ssid.into(),
            passphrase: passphrase.into(),
            url: "http://10.42.0.1/".into(),
        }
    }

    #[test]
    fn wifi_setup_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.clock = "14:35".into();
            snap.wifi.connected = false;
            snap.provisioning = ap("Wave-7F2A", "k3x9p2vw");
            let mut screen = WifiSetupScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_wifi_setup", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn rebuilding_during_setup_does_not_restart_it() {
        lvgl(|lv| {
            let mut f = fixture();
            let mut snap = snap();
            let mut screen = WifiSetupScreen::default();
            screen.build(lv.screen(), &f.ctx, &snap);
            assert!(matches!(f.commands().as_slice(), [Command::OpenWifiProvisioning]));
            // Starting until the service reports progress, even though the state is Idle.
            assert_eq!(screen.shown, Some(SetupView::Starting));
            for state in [
                ProvisioningState::Scanning,
                ap("Wave-7F2A", "k3x9p2vw"),
                ProvisioningState::Connecting { ssid: "Office-5G".into() },
                ProvisioningState::Saving,
                ProvisioningState::Reconnecting,
                ProvisioningState::Restarting,
            ] {
                snap.provisioning = state;
                let mut again = WifiSetupScreen::default();
                again.build(lv.screen(), &f.ctx, &snap);
                assert!(f.commands().is_empty(), "{:?}", snap.provisioning);
            }
        });
    }

    #[test]
    fn cancel_stops_setup_and_goes_back() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.provisioning = ap("Wave-7F2A", "k3x9p2vw");
            let mut screen = WifiSetupScreen::default();
            screen.build(root, &f.ctx, &snap);
            lv.click(screen.cancel.unwrap());
            assert!(matches!(f.commands().as_slice(), [Command::CancelWifiProvisioning]));
            assert_eq!(f.events(), vec![UiEvent::Back]);
        });
    }

    #[test]
    fn a_failure_offers_try_again() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.provisioning = ProvisioningState::Failed { reason: "Wrong password".into() };
            let mut screen = WifiSetupScreen::default();
            screen.build(root, &f.ctx, &snap);
            f.commands(); // opening retries once
            screen.started.set(false);
            screen.update(&snap);
            assert_golden("screen_wifi_setup_failed", &lv.render());
            assert_eq!(screen.message.unwrap().text(), "Couldn't connect: Wrong password");
            assert!(!screen.retry.unwrap().is_hidden());
            lv.click(screen.retry.unwrap());
            assert!(matches!(f.commands().as_slice(), [Command::OpenWifiProvisioning]));
        });
    }

    #[test]
    fn long_setup_details_stay_on_their_lines() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.provisioning = ap(
                "Wave-setup-network-with-long-nam",
                "a-sixty-three-character-passphrase-that-would-wrap-into-the-qr",
            );
            let mut screen = WifiSetupScreen::default();
            screen.build(root, &f.ctx, &snap);
            root.update_layout();
            for line in [screen.line1.unwrap(), screen.line2.unwrap()] {
                for i in 0..line.child_count() {
                    let a = line.child(i as i32).coords();
                    assert!(a.x1 >= 36 && a.x2 <= 364, "child {i} at {}..{}", a.x1, a.x2);
                }
            }
            assert!(screen.ssid.unwrap().text().ends_with("..."));
            assert!(screen.passphrase.unwrap().text().ends_with("..."));
        });
    }

    #[test]
    fn views_and_payloads() {
        assert_eq!(
            wifi_qr_payload("Wave:guest;lab", "p,ass\\\"word"),
            "WIFI:T:WPA;S:Wave\\:guest\\;lab;P:p\\,ass\\\\\\\"word;H:false;;"
        );
        assert_eq!(
            view(&ap("W", "p"), false),
            SetupView::Code { ssid: "W".into(), passphrase: "p".into(), host: "10.42.0.1".into() }
        );
        assert_eq!(view(&ProvisioningState::Idle, true), SetupView::Starting);
        assert_eq!(
            view(&ProvisioningState::Connecting { ssid: "Office".into() }, false),
            SetupView::Busy("Connecting to Office".into())
        );
        assert!(should_start(&ProvisioningState::Idle));
        assert!(should_start(&ProvisioningState::Failed { reason: "x".into() }));
        assert!(!should_start(&ProvisioningState::Scanning));
    }
}
```

The failure golden clears `started` because opening the screen on a stale Failed state retries at once. The test shows the state the user sees when *that* attempt fails.

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::wifi_setup`
Expected: `cannot find type WifiSetupScreen`.

- [ ] **Step 3: Implement**

```rust
//! Wi-Fi setup (B7, wifi-setup.png). Scan the code to join Wave's setup
//! network, pick a network on the page that opens, and Wave restarts and
//! connects. The steps are numbered because they are a sequence. The manual
//! details sit under the code, so a long password can't wrap into it.

use crate::kit::components::buttons::text_button;
use crate::kit::components::status::StatusArea;
use crate::kit::components::wifi::Qr;
use crate::kit::dispatch::Dispatcher;
use crate::kit::fonts::Font;
use crate::kit::obj::{text_width, Obj, SIZE_CONTENT};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, BACK, LIVE, SCREEN_W, TEXT, TEXT_2, TITLE_Y};
use std::cell::Cell;
use std::rc::Rc;
use waved_state::{Command, ProvisioningState};

const QR: Rect = Rect::new(68, 68, 120, 120);
const QR_QUIET: i32 = 8;
const STEPS: (i32, i32) = (202, 72);
const STEPS_W: i32 = 140;
const STEP_TEXT_W: i32 = 124;
const STEP_GAP: i32 = 8;
const LINE1_Y: i32 = 198;
const LINE2_Y: i32 = 216;
const LINE_GAP: i32 = 4;
const SSID_CAP: i32 = 160;
const PASS_CAP: i32 = 140;
const HOST_CAP: i32 = 160;
const MESSAGE: (i32, i32) = (70, 100);
const MESSAGE_W: i32 = 260;
const RETRY: Rect = Rect::new(120, 192, 160, 44);
const STEP_TEXT: [&str; 3] = [
    "Scan with your phone",
    "Pick your network on the page that opens",
    "Wave restarts and connects",
];

fn wifi_qr_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | ';' | ',' | ':' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// The standard Wi-Fi join code for the setup network.
pub fn wifi_qr_payload(ssid: &str, passphrase: &str) -> String {
    format!(
        "WIFI:T:WPA;S:{};P:{};H:false;;",
        wifi_qr_escape(ssid),
        wifi_qr_escape(passphrase)
    )
}

#[derive(Clone, Debug, PartialEq)]
pub enum SetupView {
    Starting,
    Code { ssid: String, passphrase: String, host: String },
    Busy(String),
    Failed(String),
}

/// Whether opening the screen starts provisioning: not while it already runs.
pub fn should_start(state: &ProvisioningState) -> bool {
    matches!(
        state,
        ProvisioningState::Idle | ProvisioningState::Connected { .. } | ProvisioningState::Failed { .. }
    )
}

/// What to show. `starting` is set from sending the start command until the
/// service reports progress, so a stale Idle / Connected / Failed isn't shown.
pub fn view(state: &ProvisioningState, starting: bool) -> SetupView {
    if starting && should_start(state) {
        return SetupView::Starting;
    }
    match state {
        ProvisioningState::Idle | ProvisioningState::Scanning => SetupView::Starting,
        ProvisioningState::ApActive { ssid, passphrase, url } => SetupView::Code {
            ssid: ssid.clone(),
            passphrase: passphrase.clone(),
            host: url
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .trim_end_matches('/')
                .to_string(),
        },
        ProvisioningState::Connecting { ssid } => SetupView::Busy(format!("Connecting to {ssid}")),
        ProvisioningState::Saving => SetupView::Busy("Saving the settings".into()),
        ProvisioningState::Reconnecting => {
            SetupView::Busy("Reconnecting to the saved network".into())
        }
        ProvisioningState::Restarting => SetupView::Busy("Settings saved. Wave is restarting.".into()),
        ProvisioningState::Connected { ssid } => SetupView::Busy(format!("Connected to {ssid}")),
        ProvisioningState::Failed { reason } => SetupView::Failed(reason.clone()),
    }
}

fn start(dispatch: &Dispatcher, started: &Cell<bool>) {
    dispatch.command(Command::OpenWifiProvisioning);
    started.set(true);
}

/// A label capped at `cap` px, one line, ending in "..." when longer.
fn capped(parent: Obj, font: Font, hex: u32) -> Obj {
    Obj::label(parent, "", font, hex).one_line()
}

fn set_capped(label: Obj, text: &str, cap: i32) {
    label.width(text_width(text, Font::Small).min(cap)).set_text(text);
}

#[derive(Default)]
pub struct WifiSetupScreen {
    status: Option<StatusArea>,
    qr: Option<Qr>,
    steps: Option<Obj>,
    line1: Option<Obj>,
    line2: Option<Obj>,
    ssid: Option<Obj>,
    passphrase: Option<Obj>,
    host: Option<Obj>,
    message: Option<Obj>,
    retry: Option<Obj>,
    cancel: Option<Obj>,
    started: Rc<Cell<bool>>,
    shown: Option<SetupView>,
}

impl Screen for WifiSetupScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Set up Wi-Fi", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        self.qr = Some(Qr::new(root, QR, QR_QUIET));
        let steps = Obj::plain(root)
            .pos(STEPS.0, STEPS.1)
            .size(STEPS_W, SIZE_CONTENT)
            .column(STEP_GAP);
        for (number, text) in STEP_TEXT.iter().enumerate() {
            // No layout inside a step: the number stays on the first line of a wrapped step.
            let step = Obj::plain(steps).size(STEPS_W, SIZE_CONTENT);
            Obj::label(step, &(number + 1).to_string(), Font::Small, LIVE).pos(0, 0);
            Obj::label(step, text, Font::Small, TEXT)
                .width(STEP_TEXT_W)
                .pos(STEPS_W - STEP_TEXT_W, 0);
        }
        self.steps = Some(steps);
        let line1 = Obj::plain(root)
            .rect(Rect::new(0, LINE1_Y, SCREEN_W, Font::Small.line_height()))
            .row(LINE_GAP);
        Obj::label(line1, "No camera? Join", Font::Small, TEXT_2);
        self.ssid = Some(capped(line1, Font::Small, TEXT));
        let line2 = Obj::plain(root)
            .rect(Rect::new(0, LINE2_Y, SCREEN_W, Font::Small.line_height()))
            .row(LINE_GAP);
        Obj::label(line2, "password", Font::Small, TEXT_2);
        self.passphrase = Some(capped(line2, Font::Small, TEXT));
        self.host = Some(capped(line2, Font::Small, TEXT_2));
        self.line1 = Some(line1);
        self.line2 = Some(line2);
        self.message = Some(
            Obj::label(root, "", Font::Body, TEXT)
                .wrapped_text(MESSAGE_W)
                .max_lines(3)
                .pos(MESSAGE.0, MESSAGE.1),
        );
        let (dispatch, started) = (ctx.dispatch.clone(), self.started.clone());
        self.retry = Some(text_button(root, RETRY, "Try again", move || start(&dispatch, &started)));
        let dispatch = ctx.dispatch.clone();
        self.cancel = Some(text_button(root, BACK, "Cancel", move || {
            dispatch.command(Command::CancelWifiProvisioning);
            dispatch.back();
        }));
        if should_start(&snap.provisioning) {
            start(&ctx.dispatch, &self.started);
        }
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        if self.started.get() && !should_start(&snap.provisioning) {
            self.started.set(false); // the service took over
        }
        let next = view(&snap.provisioning, self.started.get());
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        let code = matches!(next, SetupView::Code { .. });
        for part in [self.steps, self.line1, self.line2].into_iter().flatten() {
            part.hidden(!code);
        }
        if let Some(message) = self.message {
            message.hidden(code);
            match &next {
                SetupView::Starting => message.set_text("Starting the setup network"),
                SetupView::Busy(text) => message.set_text(text),
                SetupView::Failed(reason) => message.set_text(&format!("Couldn't connect: {reason}")),
                SetupView::Code { .. } => {}
            }
        }
        if let Some(retry) = self.retry {
            retry.hidden(!matches!(next, SetupView::Failed(_)));
        }
        match (&next, self.qr.as_ref(), self.ssid, self.passphrase, self.host) {
            (SetupView::Code { ssid, passphrase, host }, Some(qr), Some(s), Some(p), Some(h)) => {
                // If the code can't be built, the manual details below still work.
                qr.obj().hidden(!qr.set_data(&wifi_qr_payload(ssid, passphrase)));
                set_capped(s, ssid, SSID_CAP);
                set_capped(p, &format!("{passphrase},"), PASS_CAP);
                set_capped(h, &format!("then open {host}"), HOST_CAP);
            }
            (_, Some(qr), ..) => {
                qr.obj().hidden(true);
            }
            _ => {}
        }
        self.shown = Some(next);
    }
}
```

The legacy `wifi_qr_payload` tests carry over in `views_and_payloads`. The legacy screen (`src/screens/wifi_provisioning.rs`) is left untouched.

- [ ] **Step 4: Generate the goldens and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::wifi_setup`
Compare `screen_wifi_setup.png` with `../docs/mockups/png/wifi-setup.png`: the QR panel position, three numbered steps (step 2 on two lines), the two manual lines with white values, Cancel. The code pattern itself differs (it is real). `screen_wifi_setup_failed.png`: the message centred, "Try again" above Cancel.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::wifi_setup`
Expected: 6 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/wifi_setup.rs crates/waved-ui/golden/screen_wifi_setup*.png
git commit -m "feat(ui): wi-fi setup screen

- qr join code, numbered steps and manual details under the code
- starts provisioning once on opening, never again while it runs
- cancel stops it; a failure explains itself and offers try again

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: Uploads (B8)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/uploads.rs`
- Create: `crates/waved-ui/golden/screen_uploads.png`, `screen_uploads_no_server.png`

**Interfaces:**
- Consumes:
  - `ValueRow`, `menu::upload_mode_name`, `datetime::days_from_civil`;
  - `MeetingState.{pending_uploads, failed_uploads, last_upload_success_at}`;
  - `FilesState.recordings`;
  - `ctx.config.api.endpoint`;
  - `Command::RetryFailedUploads`.
- Produces: `pub fn host(endpoint: &str) -> Option<String>`, `pub fn parse_rfc3339(text: &str) -> Option<i64>`, `pub fn relative(now_s: i64, then_s: i64) -> Option<String>`, `pub fn last_sent_line(last: Option<&str>, now_s: i64) -> String`, `pub struct Counts { pub sent: usize, pub waiting: u32, pub failed: u32 }`, `pub fn counts(snap: &UiSnapshot) -> Counts`, `pub fn retry_label(failed: u32) -> String`, `#[derive(Default)] pub struct UploadsScreen` (impl `Screen`).

Layout (uploads.png):
- Title "Uploads".
- Host (Body, Text 2) at (50, 62), 300 wide, centred.
- "Last sent 12 min ago" (Small, Text 3) at (50, 80).
- "After a recording" `ValueRow` 248 × 40 at (76, 100), `ext_click(2)` → `Route::UploadMode`.
- Three count columns, 76 wide, at x 80 / 162 / 244. The count is Num26 at y 150 (CSS top 144 + 6) and the label (Micro) at y 174. Sent and waiting values are Text with Text 2 labels; failed turns amber (value and label) when > 0.
- "Retry n failed" pill 160 × 44 at (120, 192) with a refresh Icon16, only when failed > 0.
- Back.

The "sent" count is the delivered recordings still on Wave (`FilesState`). Waiting and failed come from the meeting service's queue counts.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::{Message, Severity, UiEvent};
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture_with, recording, snap};
    use waved_config::WavedConfig;
    use waved_state::{RecordingStatus, UploadMode};

    fn config(endpoint: &str) -> WavedConfig {
        let mut config = WavedConfig::default();
        config.api.endpoint = endpoint.into();
        config
    }

    fn uploads_snap() -> UiSnapshot {
        let mut snap = snap();
        snap.clock = "15:22".into();
        snap.battery.level = 79;
        snap.now_unix_s = 1_759_588_320;
        snap.meeting.last_upload_success_at = Some("2025-10-04T14:20:00+00:00".into());
        snap.meeting.pending_uploads = 1;
        snap.meeting.failed_uploads = 1;
        snap.files.recordings = (0..38).map(|_| recording(RecordingStatus::Uploaded)).collect();
        snap.settings.upload_mode = UploadMode::Ask;
        snap
    }

    #[test]
    fn uploads_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture_with(config("https://notes.example.com/hook"));
            let mut screen = UploadsScreen::default();
            screen.build(root, &f.ctx, &uploads_snap());
            assert_golden("screen_uploads", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.last.unwrap().text(), "Last sent 12 min ago");
        });
    }

    #[test]
    fn retry_appears_only_when_something_failed() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture_with(config("https://notes.example.com/hook"));
            let mut snap = uploads_snap();
            let mut screen = UploadsScreen::default();
            screen.build(root, &f.ctx, &snap);
            let retry = screen.retry.unwrap();
            assert!(!retry.is_hidden());
            lv.click(retry);
            assert!(matches!(f.commands().as_slice(), [Command::RetryFailedUploads]));
            assert_eq!(
                f.events(),
                vec![UiEvent::Show(Message::new(Severity::Info, "Retrying 1 upload"))]
            );
            snap.meeting.failed_uploads = 0;
            screen.update(&snap);
            assert!(screen.retry.unwrap().is_hidden());
        });
    }

    #[test]
    fn no_server_says_where_to_add_one() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture_with(config(""));
            let mut screen = UploadsScreen::default();
            screen.build(root, &f.ctx, &uploads_snap());
            assert_golden("screen_uploads_no_server", &lv.render());
            assert_eq!(screen.host.unwrap().text(), "No upload server");
            assert!(screen.counts.iter().all(|(value, _)| value.is_hidden()));
            assert!(screen.retry.unwrap().is_hidden());
        });
    }

    #[test]
    fn a_long_host_stays_on_one_line() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture_with(config(
                "https://recordings.meeting-rooms.example-company.internal/hooks/wave",
            ));
            let mut screen = UploadsScreen::default();
            screen.build(root, &f.ctx, &uploads_snap());
            root.update_layout();
            let host = screen.host.unwrap();
            assert!(host.text().ends_with("..."), "{}", host.text());
            assert_eq!(host.coords().y2 - host.coords().y1 + 1, Font::Body.line_height());
        });
    }

    #[test]
    fn timestamps_parse_and_read_relatively() {
        let t = 1_759_587_600; // 2025-10-04 14:20:00 UTC
        assert_eq!(parse_rfc3339("2025-10-04T14:20:00+00:00"), Some(t));
        assert_eq!(parse_rfc3339("2025-10-04T16:20:00.123456789+02:00"), Some(t));
        assert_eq!(parse_rfc3339("2025-10-04 14:20:00"), Some(t));
        assert_eq!(parse_rfc3339("2025-10-04T14:20:00Z"), Some(t));
        assert_eq!(parse_rfc3339("2025-13-04T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("+é:00 garbage text here"), None);
        assert_eq!(relative(1_000, 970).as_deref(), Some("just now"));
        assert_eq!(relative(10_000, 10_000 - 720).as_deref(), Some("12 min ago"));
        assert_eq!(relative(10_000, 10_000 - 7_200).as_deref(), Some("2 h ago"));
        assert_eq!(relative(200_000, 200_000 - 90_000).as_deref(), Some("yesterday"));
        assert_eq!(relative(400_000, 400_000 - 3 * 86_400).as_deref(), Some("3 days ago"));
        assert_eq!(relative(1_000, 1_030).as_deref(), Some("just now")); // small clock skew
        assert_eq!(relative(1_000, 5_000), None); // the clock isn't set yet
        assert_eq!(last_sent_line(None, 0), "Nothing sent yet");
        assert_eq!(last_sent_line(Some("2025-10-04T14:20:00Z"), 0), "Last upload time unknown");
        assert_eq!(host("https://notes.example.com/hook").as_deref(), Some("notes.example.com"));
        assert_eq!(host(""), None);
        assert_eq!(retry_label(3), "Retry 3 failed");
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::uploads`
Expected: `cannot find type UploadsScreen`.

- [ ] **Step 3: Implement**

```rust
//! Uploads (B8, uploads.png): counts first, then one action. "Retry" only
//! appears when something failed, and names how many. Times are relative
//! instead of raw ISO timestamps. A failure's reason lives on that recording
//! in Files.

use crate::kit::components::buttons::back_button;
use crate::kit::components::controls::ValueRow;
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::Severity;
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    Rect, RAISED, SCREEN_W, SURFACE, TEXT, TEXT_2, TEXT_3, TITLE_Y, WARN,
};
use crate::screens_v2::datetime::days_from_civil;
use crate::screens_v2::menu::upload_mode_name;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::{Command, RecordingStatus};

const TEXT_X: i32 = 50;
const TEXT_W: i32 = 300;
const HOST_Y: i32 = 62;
const LAST_Y: i32 = 80;
const MODE_ROW: Rect = Rect::new(76, 100, 248, 40);
const COLUMN_X: [i32; 3] = [80, 162, 244];
const COLUMN_W: i32 = 76;
const COUNT_Y: i32 = 150;
const COUNT_LABEL_Y: i32 = 174;
const RETRY: Rect = Rect::new(120, 192, 160, 44);
/// A timestamp up to this far in the future reads "just now" (clock skew).
const SKEW_S: i64 = 60;

pub fn host(endpoint: &str) -> Option<String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return None;
    }
    let rest = endpoint.split_once("://").map_or(endpoint, |(_, rest)| rest);
    Some(rest.split('/').next().unwrap_or(rest).to_string())
}

/// Seconds since the epoch of "YYYY-MM-DD[T ]HH:MM:SS[.frac][Z|±HH:MM]"
/// (no offset means UTC); `None` when it doesn't parse.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let number = |from: usize, to: usize| text.get(from..to)?.parse::<i64>().ok();
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = text.get(19..)?;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.find(|c: char| !c.is_ascii_digit()).unwrap_or(fraction.len());
        if digits == 0 {
            return None;
        }
        rest = &fraction[digits..];
    }
    let offset_s = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            if rest.len() != 6 || rest.as_bytes()[3] != b':' {
                return None;
            }
            let hours: i64 = rest.get(1..3)?.parse().ok()?;
            let minutes: i64 = rest.get(4..6)?.parse().ok()?;
            sign * (hours * 3_600 + minutes * 60)
        }
    };
    let days = days_from_civil(year, month as u32, day as u32);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset_s)
}

/// "12 min ago"; `None` when `then` lies in the future (the clock isn't set).
pub fn relative(now_s: i64, then_s: i64) -> Option<String> {
    let age = now_s - then_s;
    if age < -SKEW_S {
        return None;
    }
    Some(match age {
        a if a < 60 => "just now".into(),
        a if a < 3_600 => format!("{} min ago", a / 60),
        a if a < 86_400 => format!("{} h ago", a / 3_600),
        a if a < 2 * 86_400 => "yesterday".into(),
        a => format!("{} days ago", a / 86_400),
    })
}

pub fn last_sent_line(last: Option<&str>, now_s: i64) -> String {
    match last {
        None => "Nothing sent yet".into(),
        Some(text) => match parse_rfc3339(text).and_then(|then| relative(now_s, then)) {
            Some(ago) => format!("Last sent {ago}"),
            None => "Last upload time unknown".into(),
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    pub sent: usize,
    pub waiting: u32,
    pub failed: u32,
}

pub fn counts(snap: &UiSnapshot) -> Counts {
    Counts {
        sent: snap
            .files
            .recordings
            .iter()
            .filter(|r| r.status == RecordingStatus::Uploaded)
            .count(),
        waiting: snap.meeting.pending_uploads,
        failed: snap.meeting.failed_uploads,
    }
}

pub fn retry_label(failed: u32) -> String {
    format!("Retry {failed} failed")
}

#[derive(Default)]
pub struct UploadsScreen {
    status: Option<StatusArea>,
    configured: bool,
    host: Option<Obj>,
    last: Option<Obj>,
    mode: Option<ValueRow>,
    counts: Vec<(Obj, Obj)>,
    retry: Option<Obj>,
    retry_text: Option<Obj>,
    failed: Rc<Cell<u32>>,
    shown: Option<(Counts, String, &'static str)>,
}

impl Screen for UploadsScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Uploads", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        // api.endpoint only changes through the setup page, which restarts waved.
        let server = host(&ctx.config.api.endpoint);
        self.configured = server.is_some();
        self.host = Some(
            Obj::label(root, server.as_deref().unwrap_or("No upload server"), Font::Body, TEXT_2)
                .centered_text(TEXT_W)
                .pos(TEXT_X, HOST_Y),
        );
        self.last = Some(
            Obj::label(root, "", Font::Small, TEXT_3)
                .centered_text(TEXT_W)
                .pos(TEXT_X, LAST_Y),
        );
        let dispatch = ctx.dispatch.clone();
        let mode = ValueRow::new(
            root,
            MODE_ROW,
            "After a recording",
            upload_mode_name(snap.settings.upload_mode),
            move || dispatch.navigate(Route::UploadMode),
        );
        mode.obj().ext_click((44 - MODE_ROW.h) / 2);
        self.mode = Some(mode);
        for (&x, word) in COLUMN_X.iter().zip(["sent", "waiting", "failed"]) {
            let value = Obj::label(root, "", Font::Num26, TEXT)
                .centered_text(COLUMN_W)
                .pos(x, COUNT_Y);
            let label = Obj::label(root, word, Font::Micro, TEXT_2)
                .centered_text(COLUMN_W)
                .pos(x, COUNT_LABEL_Y);
            value.hidden(!self.configured);
            label.hidden(!self.configured);
            self.counts.push((value, label));
        }
        let retry = Obj::plain(root)
            .rect(RETRY)
            .bg(SURFACE)
            .radius(RETRY.h / 2)
            .pressed_bg(RAISED)
            .clickable(true)
            .row(6);
        Obj::label(retry, icons::REFRESH, Font::Icon16, TEXT);
        self.retry_text = Some(Obj::label(retry, "", Font::Body, TEXT));
        let (dispatch, failed) = (ctx.dispatch.clone(), self.failed.clone());
        on_click(retry, move || {
            let n = failed.get();
            dispatch.command(Command::RetryFailedUploads);
            let noun = if n == 1 { "upload" } else { "uploads" };
            dispatch.show(Severity::Info, format!("Retrying {n} {noun}"));
        });
        self.retry = Some(retry);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let now = counts(snap);
        self.failed.set(now.failed);
        let last = if self.configured {
            last_sent_line(snap.meeting.last_upload_success_at.as_deref(), snap.now_unix_s)
        } else {
            "Add one on the setup page".into()
        };
        let next = (now, last, upload_mode_name(snap.settings.upload_mode));
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        if let (Some(last_label), Some(mode)) = (self.last, self.mode.as_ref()) {
            last_label.set_text(&next.1);
            mode.set_value(next.2);
        }
        let values = [now.sent.to_string(), now.waiting.to_string(), now.failed.to_string()];
        for (index, ((value, label), text)) in self.counts.iter().zip(&values).enumerate() {
            value.set_text(text);
            let alert = index == 2 && now.failed > 0;
            value.text_color(if alert { WARN } else { TEXT });
            label.text_color(if alert { WARN } else { TEXT_2 });
        }
        if let (Some(retry), Some(text)) = (self.retry, self.retry_text) {
            retry.hidden(!self.configured || now.failed == 0);
            text.set_text(&retry_label(now.failed));
        }
        self.shown = Some(next);
    }
}
```

- [ ] **Step 4: Generate the goldens and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::uploads`
Compare `screen_uploads.png` with `../docs/mockups/png/uploads.png`: host, "Last sent 12 min ago", the value row "Ask me", counts 38 / 1 / amber 1, the "Retry 1 failed" pill with icon, Back. `screen_uploads_no_server.png`: "No upload server", "Add one on the setup page", the value row, no counts, no Retry.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::uploads`
Expected: 5 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/uploads.rs crates/waved-ui/golden/screen_uploads*.png
git commit -m "feat(ui): uploads screen

- server host, relative last-sent time, sent / waiting / failed counts
- retry appears only when something failed and names how many
- no server: says where to add one

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 16: After a recording (B8)

**Files:**
- Modify: `crates/waved-ui/src/screens_v2/upload_mode.rs`
- Create: `crates/waved-ui/golden/screen_upload_mode.png`

**Interfaces:**
- Consumes: `RadioList`, `RadioOption`, `Command::SetUploadMode(UploadMode)`, `SettingsState.upload_mode`.
- Produces: `pub const MODES: [UploadMode; 3]`, `pub fn options() -> Vec<RadioOption>`, `pub fn mode_index(mode: UploadMode) -> usize`, `#[derive(Default)] pub struct UploadModeScreen` (impl `Screen`).

Layout (upload-mode.png): title "After a recording", three radio rows from `RADIO_TOP`, Back. This is exactly the `kit_upload_mode` composition.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    #[test]
    fn upload_mode_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.clock = "15:22".into();
            snap.battery.level = 79;
            snap.settings.upload_mode = UploadMode::Ask;
            let mut screen = UploadModeScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_upload_mode", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn choosing_sends_the_mode_and_follows_changes_made_elsewhere() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.settings.upload_mode = UploadMode::Ask;
            let mut screen = UploadModeScreen::default();
            screen.build(root, &f.ctx, &snap);
            let list = screen.list.as_ref().unwrap();
            lv.click(list.row(2));
            assert!(matches!(f.commands().as_slice(), [Command::SetUploadMode(UploadMode::LocalOnly)]));
            screen.update(&snap); // stale: still Ask
            assert_eq!(screen.list.as_ref().unwrap().selected(), 2);
            snap.settings.upload_mode = UploadMode::Automatic; // the setup page changed it
            screen.update(&snap);
            assert_eq!(screen.list.as_ref().unwrap().selected(), 0);
        });
    }

    #[test]
    fn options_name_the_privacy_choice() {
        let titles: Vec<String> = options().into_iter().map(|o| o.title).collect();
        assert_eq!(titles, ["Send automatically", "Ask me each time", "Keep on Wave"]);
        assert_eq!(mode_index(UploadMode::LocalOnly), 2);
    }
}
```

- [ ] **Step 2: Run, expect a compile failure**

Run: `cargo test -p waved-ui --lib screens_v2::upload_mode`
Expected: `cannot find type UploadModeScreen`.

- [ ] **Step 3: Implement**

```rust
//! After a recording (B8, upload-mode.png): what happens to each recording,
//! a privacy choice with one plain sentence per option
//! (`upload_mode = automatic | ask | local_only`).

use crate::kit::components::buttons::back_button;
use crate::kit::components::controls::{RadioList, RadioOption};
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{RADIO_TOP, SCREEN_W, TEXT, TITLE_Y};
use waved_state::{Command, UploadMode};

pub const MODES: [UploadMode; 3] = [UploadMode::Automatic, UploadMode::Ask, UploadMode::LocalOnly];

pub fn options() -> Vec<RadioOption> {
    vec![
        RadioOption::new("Send automatically", "As soon as Wi-Fi allows"),
        RadioOption::new("Ask me each time", "Choose Send, Keep or Delete"),
        RadioOption::new("Keep on Wave", "Nothing leaves the device"),
    ]
}

pub fn mode_index(mode: UploadMode) -> usize {
    MODES.iter().position(|m| *m == mode).unwrap_or(0)
}

#[derive(Default)]
pub struct UploadModeScreen {
    status: Option<StatusArea>,
    list: Option<RadioList>,
    seen: Option<UploadMode>,
}

impl Screen for UploadModeScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "After a recording", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        let dispatch = ctx.dispatch.clone();
        self.list = Some(RadioList::new(
            root,
            RADIO_TOP,
            options(),
            mode_index(snap.settings.upload_mode),
            move |index| dispatch.command(Command::SetUploadMode(MODES[index])),
        ));
        self.seen = Some(snap.settings.upload_mode);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        // Follow a change made elsewhere; a tap here already moved the radio.
        if self.seen != Some(snap.settings.upload_mode) {
            self.seen = Some(snap.settings.upload_mode);
            if let Some(list) = self.list.as_ref() {
                list.set_selected(mode_index(snap.settings.upload_mode));
            }
        }
    }
}
```

- [ ] **Step 4: Generate the golden and compare**

Run: `UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2::upload_mode`
Compare `screen_upload_mode.png` with `../docs/mockups/png/upload-mode.png`, and confirm it is pixel-identical to `golden/kit_upload_mode.png`: run `cmp crates/waved-ui/golden/screen_upload_mode.png crates/waved-ui/golden/kit_upload_mode.png`, which should print nothing.

- [ ] **Step 5: Run, expect a pass**

Run: `cargo test -p waved-ui --lib screens_v2::upload_mode`
Expected: 3 passed.

- [ ] **Step 6: Lint** (lint gate).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens_v2/upload_mode.rs crates/waved-ui/golden/screen_upload_mode.png
git commit -m "feat(ui): after a recording screen

- radio list for send automatically, ask me each time, keep on wave
- follows changes made on the setup page without fighting a fresh tap

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 17: Workstream verification and the device-pass checklist

**Files:** none (verification only; any fix goes back to the task that owns the file).

- [ ] **Step 1: Full host run**

Run: `cargo test -p waved-ui --lib`
Expected: every test passes, including the legacy and kit suites, with no golden diffs.

- [ ] **Step 2: Lint gate** — expect no output from `grep`.

- [ ] **Step 3: Visual pass**

Run: `UI_PREVIEW=1 cargo test -p waved-ui --lib screens_v2 kit::components::mode_picker kit::components::wifi`. Lay every `target/ui-preview/screen_*.png` next to its mockup:

| Golden | Mockup |
|---|---|
| `screen_menu` | `menu.png` |
| `screen_audio_mode` | `audio-mode.png` |
| `screen_noise` | `noise.png` |
| `screen_volume` | `volume.png` |
| `screen_system` | `system.png` |
| `screen_display` | `display.png` |
| `screen_datetime` | `datetime.png` |
| `screen_confirm` | `confirm.png` |
| `screen_wifi` | `wifi.png` |
| `screen_wifi_setup` | `wifi-setup.png` |
| `screen_uploads` | `uploads.png` |
| `screen_upload_mode` | `upload-mode.png` |

Note every deviation in the PR description. Known ones are under "Open points".

- [ ] **Step 4: Hand over the device checklist** (for the end-of-M2 integrator device pass, after the runtime switch):
  1. Audio mode:
     - Drag the handle with a finger: it follows smoothly. On release, `SetBeamAim` lands in `/tmp/audio_stats.json` once C10 is in.
     - A sideways drag never triggers Back.
     - Follow talker's preview turns toward a speaker walking around the table.
  2. Volume: a swipe left opens Mic level and a swipe right returns. Dragging the arc changes the speaker live without jumping back. Mic level reaches 0–200 % (100 % mid-arc); a host-set gain above 200 % still shows its real percentage.
  3. Display: dragging brightness changes the backlight live (E1). "Dim after 30 s" rests the screen after 30 s.
  4. Date and time: switching zones updates the status clock within a minute (E3); the 12-hour format shows "pm" on its baseline; before the clock is set the time reads "--:--".
  5. Wi-Fi:
     - "Change network" shows the warning first.
     - Continuing brings up the setup AP, and the QR joins it from an iPhone and an Android phone.
     - Cancel restores the station.
     - A wrong password shows "Couldn't connect: …" with Try again.
     - Left open for 5 minutes, the setup network closes and the screen says "The setup network closed. Try again to reopen it." with Try again.
     - Signal arcs: confirm with the user that lighting inside-out (P18) is what they want versus the mockup's outer-lit look.
  6. Confirmation:
     - Power off from System and from the power key (E2) powers down only after the action pill.
     - Factory reset runs only on a test unit (E6).
     - Delete from File detail: the confirmation does one back and the detail leaves itself, landing on Files; Cancel does one back to the detail.
  7. Uploads: Retry re-queues failed uploads (watch `waved.log`), and the count drops.
  8. Every settings screen: every target is reachable with a fingertip at the panel's bottom curve (Back, the Display "Dim after" row, the radio rows at y 180).
  9. Status messages: "Powering off", "Resetting Wave", "Recording deleted", "Retrying n uploads" appear in the status area (UiSnapshot.message + StatusArea::update).

---

## Open points

- **Wi-Fi signal arcs.** wifi.png ("signal good") greys the *innermost* r 9 arc and lights the outer three. Implemented inside-out (controller ruling P18; 3 bars = r 9 / 22 / 35), the usual reading of "arcs grey out as the signal drops". Confirm with the user at the device pass. Flipping it is one line in `SignalArcs::set_bars`.
- **Mic level range.** Decided (user decision 2026-10-05): 0–200 % on the device (unity mid-arc). The arc maps 0–100 to `mic_volume` 0–200 %, and the label shows the real percentage. Higher gains set by the host (up to 800) are kept and shown, but can't be reached on the device.
- **"Sent" count.** It comes from `FilesState` (Uploaded recordings still on Wave). Retention prunes delivered files, so it is not a lifetime total. A lifetime count needs a `MeetingState` field from C, which this plan does not request.
- **Time zones.** The device offers a curated list of 29 zones. E3 must resolve each of them, through the zoneinfo database or its own table.
- **Mic level discoverability.** Only the swipe reaches it, as the mockup says. A second Menu row "Mic level" would be a spec change.
