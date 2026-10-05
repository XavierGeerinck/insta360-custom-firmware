# UI kit

The building blocks for the revision-2 screens (`docs/mockups`). Milestone 2
builds every screen from these pieces. The legacy screens in `src/screens/` are
deleted when the runtime switches over.

## Rules

- Screens use `Obj`, `event::on`/`on_click` and the components. Raw `crate::bindings`
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
  and the left confirmation pill pass only because their corners are rounded.
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
  `BOTTOM_LABEL_W`-wide label; `BOTTOM_PILL_WIDE` and `BOTTOM_PILL_PAIR` for actions whose
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

## Components

| Component | Constructor | Used by |
|---|---|---|
| Status cluster / pill / messages | `StatusArea::plain(root)`, `StatusArea::pill(root, &dispatch)`, `.show(&message, now_ms)`; the pill leads with a teal dot while `snap.meeting.pre_recording_active` | every screen |
| Back | `back_button(root, &dispatch)` | every sub-screen |
| Text button | `text_button(root, rect, "Cancel", on_tap)` | Wi-Fi setup, USB storage |
| Primary (milestone 1 pill, replaced by round bottom buttons) | `PrimaryButton::new(root, PRIMARY, Tone::Rec, icons::RECORD, "Record", None, on_tap)` | Home, After stop, First start |
| Side buttons | `SideButton::new(root, Side::Left, icons::VOLUME_LOW, "Quieter", on_step)` | Home, Recording, File detail |
| Button rows | `ButtonRow::two(..)` is the pill pair at `BOTTOM_PILL_PAIR`; `ButtonRow::three(..)` is the milestone 1 row (replaced by round bottom buttons) | Confirmation; Recording, After stop, Call until then |
| Confirmation | `confirm_dialog(root, &ConfirmSpec { .. }, on_confirm, on_cancel)` | Power off, Factory reset, Delete, Stop USB storage |
| Dial | `Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3).on_tap(..)`, `set_ring`, `set_talker`, `set_gauge`, `set_progress` | Home, Recording, Resting, File detail, Update, Call, Start-up |
| Rotary list | `RotaryList::new(root, items, on_select)` | Menu, System |
| Radio list | `RadioList::new(root, RADIO_TOP, options, selected, on_select)` | After a recording, Time zone, Dim after |
| Value / switch rows | `ValueRow::new(..)`, `SwitchRow::new(..)` | Uploads, Display, Date and time |
| Value arc | `ValueArc::new(root, cx, cy, r, on_change)` | Volume, Mic level, Display |

Notes on the parts that carry state:

- The Dial is in one of three modes: ring (`set_ring`, `set_talker`), gauge (`set_gauge`) or
  progress (`set_progress`). Setting a gauge or progress replaces the ring; clearing it
  (`None`) returns to the ring.
- `Tone` picks a button's fill and text colour (`Tone::colors()`). `Tone::accent()` is the colour a
  dialog uses for that tone: red for destructive tones, teal otherwise.
- The rotary list has uniform 48 px slots centred on y 144 inside `list::VIEW` (y 34–240),
  with 14 / 16 px fades at the top and bottom and an arc scrollbar of r 188 around the panel
  centre over 62°–122°. Rows fade and change band with distance from the focus line.
- The status pill's pre-recording dot follows `MeetingState::pre_recording_active` (the buffer
  is filling now), not the `SettingsState::pre_recording` setting.

The radial audio-mode picker (B2), the PIN keypad (K2), file rows (D1) and Bluetooth device rows (I4)
are owned by their workstreams.

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
- Taps in tests: `lv.click(obj)`, `lv.send(obj, code)`, and `lv.advance(ms)` for held buttons.
- `components/compositions.rs` shows Home and After a recording built from kit parts.

## Fonts and icons

`crates/waved-ui/fonts/build_fonts.sh` regenerates `src/kit/fonts/*.c` and `src/kit/icons.rs`; see
`crates/waved-ui/fonts/README.md`. To add an icon, add it to `crates/waved-ui/fonts/icons.py` and
rerun the script.
