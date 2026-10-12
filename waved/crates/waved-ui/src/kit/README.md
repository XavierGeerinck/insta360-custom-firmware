# UI kit

The building blocks for the revision-2 screens (`docs/mockups`). Milestone 2
builds every screen from these pieces. The runtime (`src/runtime/`) builds them
by route; the legacy screens are gone.

## Rules

- Screens use `Obj`, `event::on`/`on_click`/`on_swipe` and the components. Raw `crate::bindings`
  calls belong only in `kit/obj.rs`, `kit/event.rs`, `kit/testkit.rs` and `kit/components/`.
  `Obj::from_raw` is `unsafe`; only the kit's own code needs it.
- Every position and colour comes from `kit::tokens`. Every string is plain, sentence-case copy.
- Never hard-code a line height. The fonts report Title 27, Dial 32 and Num48 43 (the 48 px digit
  font embeds `a`/`m`/`p`, so its height includes the descender), which differ from the mockup's
  CSS values. Use `Font::line_height()` or `one_line()`, which sizes the label to one line of its
  font. The Num48 and Num26 label boxes carry 9 and 5 px of empty room below the digits (the
  descender of the am/pm glyphs), so position big digits by glyph/top, not by box centre; for a
  12-hour suffix prefer Num20.
- A label that can show user data (names, hosts, values) stays on one line. A plain `Obj::label`
  wraps instead. Pick one of:
  - `centered_text(width)` for centred text across a known width;
  - `one_line()` with an explicit `width(text_width(text, font).min(cap))` (`text_width` is in
    `kit::obj`);
  - `one_line()` inside a flex row or column with `grow()`.

  Do not use `max_width()` for truncation: in LVGL 9.2 a one-line label sized only by a maximum
  width collapses to "...". Multi-line text that must stop above a button row uses
  `wrapped_text(width)` then `max_lines(n)`, which ends in "..." when it needs more lines.
- Read user settings from the snapshot: `snap.settings` (display, time, recording, uploads,
  PIN) and `snap.audio` (mode, noise reduction, volumes, aim). Never read them from
  `ctx.config`: it is the boot-time copy of config.toml and does not see changes made on the
  device or by the setup page since.
  The exceptions are read-only, for values that do not change while waved runs: a screen may
  read `ctx.config.api.endpoint` (Uploads shows the server host, File detail checks that a server
  is set; only the setup page changes it, and saving there restarts waved) and
  `ctx.config.device.name` (About shows it). Screens never write to `ctx.config`.
- Screens never navigate by sending bus commands. Use `ctx.dispatch.navigate(..)`,
  `.back()` or `.show(..)`. Services get `ctx.dispatch.command(..)`.
- Every screen test calls `assert_targets_ok(root)` and `assert_golden(..)`, and the
  golden is checked against the screen's mockup PNG.

## Geometry

The panel shows a circle of **r 198 around (200, 106)**, cut flat at framebuffer row 0
(`PANEL_CX`, `PANEL_CY`, `PANEL_R`; see `docs/DISPLAY_HARDWARE.md`, "Visible Area"). Rows
below y 304 do not exist, so the bottom of the circle is cut by the framebuffer too.

- **Tap limit:** every tappable pixel lies within `TAP_LIMIT_R` (190) of (200, 106) and at
  or below `TAP_MIN_Y` (6). `Rect::within_tap_limit()` checks a square box;
  `within_tap_limit_rounded(radius)` counts only the pixels a rounded box draws, which is
  what `assert_targets_ok` uses (it reads each object's style radius). The third radio row
  and the left confirmation pill pass only because their corners are rounded. Like LVGL's hit
  test, the check cuts each target to its ancestors' boxes: a row scrolled half out of a list
  counts only where it shows (with square corners at the cut), and a row scrolled out entirely is
  skipped. The 44 px size check still uses the whole target.
- **Zones:** status 6–34 (`STATUS`, `STATUS_PILL` 240 wide; the pill's hit area
  `STATUS_PILL_HIT` is 44 tall and ends at y 50, where the dial's tap circle begins); title
  at `TITLE_Y` 38; content from `CONTENT_TOP` 70 (`RADIO_TOP` 68 for radio rows); Back at
  `BACK` (144, 244).
- **Dial:** r 70 at (200, 121) (`DIAL_CX/CY/R`), tap circle r 71, gauge stroke 5, progress stroke 6;
  the talker wedge spans 38° (plus round caps) at stroke 6 with its
  dot on r 82; the Home text stack uses `DIAL_ICON_Y`, `DIAL_TITLE_Y`, `DIAL_LINE_Y`,
  `DIAL_HINT_Y`. Side buttons (`SIDE_LEFT/RIGHT`) share its centre line, labels at
  `SIDE_LABEL_Y`.
- **Bottom actions:** round buttons centred on `BOTTOM_ARC_R` (135) around the panel centre:
  `BOTTOM_MAIN` (Record and Stop, Send, Set up Wi-Fi), `BOTTOM_LEFT/RIGHT` companions,
  `BOTTOM_PAIR_LEFT/RIGHT` (Decline / Answer), each with a `*_LABEL` position for a
  `ROUND_LABEL_W`-wide label; `BOTTOM_PILL_WIDE` and `BOTTOM_PILL_PAIR` for actions whose
  words matter. Content above companions ends by y 166, above the main button by y 193.

## A screen

```rust
use crate::kit::components::buttons::back_button;
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{SCREEN_W, TEXT, TITLE_Y};

#[derive(Default)]
pub struct AboutScreen {
    status: Option<StatusArea>,
}

impl Screen for AboutScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "About", Font::Title, TEXT).centered_text(SCREEN_W).pos(0, TITLE_Y);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
    }
}
```

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
| `MicHeight` | `mic_height` | `MicHeightScreen::default()` |
| `Files` | `files` | `FilesScreen::default()` |
| `FileDetail(id)` | `file_detail` | `FileDetailScreen::new(id: Uuid)` |
| `Wifi` | `wifi` | `WifiScreen::default()` |
| `WifiSetup` | `wifi_setup` | `WifiSetupScreen::default()` |
| `Uploads` | `uploads` | `UploadsScreen::default()` |
| `UploadMode` | `upload_mode` | `UploadModeScreen::default()` |
| `System` | `system` | `SystemScreen::default()` |
| `Display` | `display` | `DisplayScreen::default()` |
| `DimAfter` | `display` | `DimAfterScreen::default()` |
| `DateTime` | `datetime` | `DateTimeScreen::default()` |
| `TimeZone` | `datetime` | `TimeZoneScreen::default()` |
| `About` | `about` | `AboutScreen::default()` |
| `Confirm(kind)` | `confirm` | `ConfirmScreen::new(kind: ConfirmKind)` |

`messages.rs` is not a screen; it holds the message copy screens send. Nor is
`main_parts.rs`, the pieces Home, Recording, After stop and Resting share. Bluetooth,
PIN, USB storage, Update and Incoming call are milestone 3 and have no module yet. A screen must cope with any snapshot (for example `FileDetail` for an
id no longer in `snap.files`, or `Decision` while `pending_decision` is `None` for a
frame) without panicking. Every screen with a status area calls
`status.update(snap)` from `update`: that is how runtime messages appear.

## Drags and the screen gestures

The runtime turns a swipe right into Back (on sub-screens) and a swipe down from
the top edge (y ≤ 34) into Menu (Home, Recording). A control that takes drags —
the value arc, the audio-mode aim, the playback scrubber — calls
`.keep_gestures()` on its tappable object, so a drag that starts on it is never
read as a swipe. `ValueArc` already does. Screens never handle these two swipes
themselves.

Other swipes belong to the screen: `on_swipe(root, Swipe::Left, ..)` (Volume opens Mic
level). There is one recogniser, `Swipe::recognise` (at least `SWIPE_MIN_PX` 60 along one
axis, at most half that across, within `SWIPE_MAX_MS` 700), applied per press by
`SwipeTracker`. The runtime feeds it the touch samples and hands a swipe it does not take
itself to `send_swipe(obj_under_the_press, direction)`, which delivers it as LVGL routes
gestures: to the nearest object from the pressed one up that keeps its drags, else to the
screen. LVGL's own `LV_EVENT_GESTURE` is not used. Two runtime rules go with it:

- The runtime blocks swipes that start on a `keep_gestures` object before recognising anything:
  such a press never swipes, in any direction.
- If `send_swipe` returns true (a screen took the swipe), the runtime cancels the press
  (`lv_indev_wait_release`) exactly as for its own gestures, so a swipe never also fires a tap.

## Runtime

What the runtime (`src/runtime/`) does around screens, so a screen need not.

- **Parking.** A screen is built fresh on each navigation and deleted when it is
  replaced. The exception is the screen under `Resting`: when the resting screen is pushed
  right on top of a route, that screen is parked (kept alive, not shown), and on wake it is
  re-loaded as it was, not rebuilt. A screen that acts in `build` (Wi-Fi setup starts
  provisioning) therefore does not act again on waking. Any other return rebuilds the screen
  (Back from the power-off confirmation). For that `build` call `UiSnapshot::rebuilt` is
  `true`, so a screen that acts in `build` can skip it. It is `false` on a first visit, and
  `false` again once the route has left the stack: a route that comes back after leaving is a
  first visit.
- **Leave commands.** A route can send commands when it goes away (Wi-Fi setup sends
  `CancelWifiProvisioning`, File detail sends `StopPlayback` while its recording plays).
  The runtime sends them once, when the route leaves the router stack by any path: a button
  or swipe Back, `ToBase`, a base navigate or a meeting takeover (`runtime::gesture::leave_commands`).
  A screen's own Back button may send the same command too; a duplicate is accepted.
- **Press cancel.** A press is cancelled (`lv_indev_wait_release`) when the screen changes
  under it, or when a swipe is taken (by the runtime, or by a screen's `on_swipe`), so it never
  also taps. When the screen is parked under a press, the pressed object also gets `LV_EVENT_PRESS_LOST`
  at once (LVGL sends its own only on the release), so a hold-to-Stop in progress resets instead of
  completing on the hidden screen. A press that only woke the screen is never delivered to LVGL.
- **Wake on arrival.** A new pending decision (a recording stopped) or an incoming call
  (`audio.call_ringing`) wakes a resting or dark screen without delivering a press.

## Components

| Component | Constructor | Used by |
|---|---|---|
| Status cluster / pill / messages | `StatusArea::plain(root)`, `StatusArea::pill(root, &dispatch)`, `.show(&message, now_ms)`; the pill leads with a teal dot while `snap.meeting.pre_recording_active`; the battery (from `snap.battery`, which waved-mcu fills from the MCU's heartbeats) is hidden until the first reading, teal with the charging glyph while charging, amber at 20 % or less and red at 10 % or less; `Message::with_icon(glyph)` replaces the severity's icon (the battery messages) | every screen |
| Back | `back_button(root, &dispatch)` | every sub-screen |
| Text button | `text_button(root, rect, "Cancel", on_tap)` | Wi-Fi setup, USB storage |
| Round bottom buttons | `RoundButton::record(root, on_tap)`, `RoundButton::stop(root, on_tap, on_stop)` (holds for `HOLD_TO_STOP_MS`), `RoundButton::new(root, BottomSpot::Left, icons::MARK, "Mark", Tone::Neutral, on_tap)`; `.hold(ms, on_complete)`, `.icon_font(font)` to override the icon size; `set_icon` / `set_label` / `set_accent` / `set_enabled` | Home (Record), Recording (Mark / Stop / Pause), After stop (Keep / Send / Delete), First start, Call (Decline / Answer) |
| Round button at a spot | `RoundButton::at(root, DETAIL_SIDE_RIGHT, DETAIL_SIDE_RIGHT_LABEL, icons::SEND, "Send", Tone::LiveIcon, on_tap)`: 26 px icon, taps only (no hold, no repeat) | File detail (Delete / Send) |
| Side buttons | `SideButton::new(root, Side::Left, icons::VOLUME_LOW, "Quieter", on_step)` (a round button at the side spot) | Home, Recording, File detail |
| Pill pair | `ButtonRow::two(root, [cancel, action], on_tap)` at `BOTTOM_PILL_PAIR` | Confirmation |
| Confirmation | `confirm_dialog(root, &ConfirmSpec { .. }, on_confirm, on_cancel)` | Power off, Factory reset, Delete, Stop USB storage |
| Dial | `Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3).on_tap(..)`, `set_ring`, `set_talker`, `set_gauge`, `set_progress` | Home, Recording, Resting, File detail, Update, Call, Start-up |
| Rotary list | `RotaryList::new(root, items, on_select)` | Menu, System |
| Radio list | `RadioList::new(root, RADIO_TOP, options, selected, on_select)`: whole rows only (three from `RADIO_TOP`, y 68–231), snapping to rows when longer; `.scroll_to(index)` puts the row at the top without animation, or shows the last rows | After a recording, Time zone, Dim after |
| Value / switch rows | `ValueRow::new(..)`, `SwitchRow::new(..)` | Uploads, Display, Date and time |
| Value arc | `ValueArc::new(root, cx, cy, r, on_change)` | Volume, Mic level, Display |
| Audio mode picker | `ModePicker::new(root, glyphs: [Glyph; 6], selected, on_select, on_aim)`; `set_selected`, `set_preview(&Preview)`, `set_aimable`; `AimEvent` reports the aim while dragging and on release. Six round buttons on a ring around a drawn pickup-pattern preview; the preview disc takes aim drags and keeps its gestures (`components/mode_picker.rs`) | Audio mode |
| Signal arcs | `SignalArcs::new(root, cx, cy)`, `set_bars(lit)` (0-4 of four concentric arcs, innermost first) (`components/wifi.rs`) | Wi-Fi |
| QR code | `Qr::new(root, rect, quiet)`, `set_data(text) -> bool` (white rounded panel with a quiet zone; false when the text does not fit) (`components/wifi.rs`) | Wi-Fi setup |
| File list | `FileList::new(root, on_open(id))`, `set_rows(Vec<RowLook>)`, `scroll_to`, `empty_state()`; `RowLook` is what one 296 x 52 recording card shows (`components/file_row.rs`) | Files |
| Playback scrubber | `Scrubber::new(root, on_event)` with `ScrubEvent::{TogglePlay, Seek(ms)}`; `set(position_ms, duration_ms, marks_ms, playing, playable)`. A ring (r 60 at (200, 140)) with mark ticks, a knob and the time caption; centre taps play or pause, the ring seeks and keeps its gestures (`components/scrubber.rs`) | File detail |

Notes on the parts that carry state:

- The Dial is in one of three modes: ring (`set_ring`, `set_talker`), gauge (`set_gauge`) or
  progress (`set_progress`). Setting a gauge or progress replaces the ring; clearing it
  (`None`) returns to the ring.
- Every button is a Surface fill (Raised while pressed); red is never a fill. `Tone` is the
  accent on what is drawn on it: `Tone::fg()` colours the icon (and a pill's words),
  `Tone::label()` a round button's label. `Neutral` is a Text icon with a Text 2 label (Mark,
  Pause), `Rec` a red icon with a calm label (Record's dot), `RecText` red icon and label (Stop,
  Delete, Decline), `Live` teal (Send, Answer), `LiveIcon` a teal icon over a Text 2 label
  (File detail's Send), `WarnText` amber (Resume). `Tone::accent()` is the colour a dialog uses
  for that tone: red for destructive tones, teal otherwise.
- Round buttons (`RoundButton`, `SideButton`) share one implementation: a Surface circle at a
  spot (`BottomSpot::{Main, Left, Right, PairLeft, PairRight}` or `Side`), a 26 px icon (22 px
  at the companions) and a micro label under it. Pressed is Raised with a 2 px teal ring drawn
  as a border, so it stays inside the circle and the tap target. Disabled shows the icon and
  label in Text 3 and ignores taps (Mark while paused).
- `RoundButton::hold` (Stop) draws a red ring of r 35 (`BOTTOM_HOLD_R`, stroke 3, Line track)
  around the circle while pressed. It fills clockwise over the duration and calls back once when
  full; letting go early empties it, and that release still counts as a tap (Stop explains the
  hold), a completed hold does not. While held the circle is Raised without the teal ring. The
  ring is not tappable; it is a sibling of the button (it reaches past the circle) and is
  deleted with it, so a deleted button's hold never completes.
- Tabler's solid media glyphs fill more of the em than the mockup's symbols, so Record, Stop,
  Pause and Play (Resume) are drawn at 20 px automatically (a 12–13 px dot or square, as in the
  mockups); other glyphs take the spot's size. `set_icon` re-picks the size for the new glyph;
  `icon_font(font)` fixes it instead.
- A round button's label and hold ring are siblings of its circle, placed in screen
  coordinates: its parent must be the screen root or a full-screen container without a layout.
  Deleting the button deletes them too.
- Disabling differs by form: `ButtonRow::set_enabled` fades a pill to 35 % opacity, while
  `RoundButton::set_enabled` keeps the Surface circle and greys the icon and label to Text 3
  (paused.png). Both ignore taps.
- The rotary list has uniform 48 px slots centred on y 144 inside `list::VIEW` (y 34–240),
  with 14 / 16 px fades at the top and bottom and an arc scrollbar of r 188 around the panel
  centre over 62°–122°. Rows fade and change band with distance from the focus line.
- The talker wedge (Home, Recording) and Audio mode's Follow talker preview never read
  `audio.doa_active`: the VAD flag reads open in a quiet room. They gate on
  `audio.doa_confidence >= 0.30` through `kit::talker::TalkerGate` (state per screen): a talker
  shows after 150 ms of confident samples and goes after 100 ms without one, and the last
  confident direction is held through dropouts. `home_ring` / `recording_ring` take the screen's
  gate; screens are updated every loop iteration (~10 ms), so the delays are in time, not samples.
- The status pill's pre-recording dot follows `MeetingState::pre_recording_active` (the buffer
  is filling now), not the `SettingsState::pre_recording` setting.

Some components came from workstreams: the radial audio-mode picker (B2), the Wi-Fi signal arcs
and QR code (B7), file rows (D1) and the playback scrubber (D2); later the PIN keypad (K2) and
Bluetooth device rows (I4).

Not components, but shared by the screens:

- `kit::time`: `days_from_civil`, `civil_from_days`, `is_leap`, `month_len` and `parse_rfc3339`
  (the offset is required, the date must exist). Use it instead of a private copy.
- `screens_v2::messages::copy`: every message a screen sends. Screens never write message
  literals; add the text to `copy` (and to its width test) instead.

## Testing

```rust
use crate::kit::dispatch::Dispatcher;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
use std::sync::Arc;
use waved_config::WavedConfig;

#[test]
fn about_matches_the_mockup() {
    lvgl(|lv| {
        let root = lv.screen();
        let (dispatch, _bus, _ui) = Dispatcher::test();
        let ctx = ScreenCtx { dispatch, config: Arc::new(WavedConfig::default()) };
        let snap = UiSnapshot::default(); // set the fixture fields the screen reads
        AboutScreen::default().build(root, &ctx, &snap);
        assert_golden("about", &lv.render());
        assert_targets_ok(root);
    });
}
```

- `UPDATE_GOLDEN=1 cargo test -p waved-ui --lib` writes the goldens to `golden/`.
- `UI_PREVIEW=1` also writes copies dimmed outside the visible circle to `target/ui-preview/`
  for comparison with `docs/mockups/png/`. The mockup PNGs are 880×880 at 2×; the 400×304
  framebuffer sits at x 40..840, y 228..836.
- A mismatch writes the actual frame to `target/golden-diff/<name>.actual.png`.
- A new screen needs a `Route` variant in `kit/route.rs` (the router builds screens from it);
  `Route::About` already exists.
- Taps in tests: `lv.click(obj)`, `lv.send(obj, code)`, and `lv.advance(ms)` for held buttons;
  `lv.swipe(obj, Swipe::Left)` delivers a recognised swipe.
- `components/compositions.rs` shows Home, Recording and After a recording built from kit parts.

## Fonts and icons

`crates/waved-ui/fonts/build_fonts.sh` regenerates `src/kit/fonts/*.c` and `src/kit/icons.rs`; see
`crates/waved-ui/fonts/README.md`. To add an icon, add it to `crates/waved-ui/fonts/icons.py` and
rerun the script. Names ending in `-filled` come from Tabler's filled face (the media
controls use it); the icon sizes are 16, 20, 22 (companion bottom buttons), 26 and 44.
