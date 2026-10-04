# UI kit

The building blocks for the revision-2 screens (`docs/mockups`). Milestone 2
builds every screen from these pieces. The legacy screens in `src/screens/` are
deleted when the runtime switches over.

## Rules

- Screens use `Obj`, `event::on`/`on_click` and the components. Raw `crate::bindings`
  calls belong only in `kit/obj.rs`, `kit/event.rs`, `kit/testkit.rs` and `kit/components/`.
  `Obj::from_raw` is `unsafe`; only the kit's own code needs it.
- Every position and colour comes from `kit::tokens`. Every string is plain, sentence-case copy.
- Never hard-code a line height. The fonts report Title 27, Dial 32 and Num48 34, which differ
  from the mockup's CSS values. Use `Font::line_height()` or `one_line()`, which sizes the label to
  one line of its font.
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
| Status cluster / pill / messages | `StatusArea::plain(root)`, `StatusArea::pill(root, &dispatch)`, `.show(&message, now_ms)` | every screen |
| Back | `back_button(root, &dispatch)` | every sub-screen |
| Text button | `text_button(root, rect, "Cancel", on_tap)` | Wi-Fi setup, USB storage |
| Primary | `PrimaryButton::new(root, PRIMARY, Tone::Rec, icons::RECORD, "Record", Some("includes last 30 s"), on_tap)` | Home, After stop, First start |
| Side buttons | `SideButton::new(root, Side::Left, icons::VOLUME_LOW, "Quieter", on_step)` | Home, Recording, File detail |
| Button rows | `ButtonRow::two/three(root, [ButtonSpec; N], on_tap)` | Recording, After stop, Call |
| Confirmation | `confirm_dialog(root, &ConfirmSpec { .. }, on_confirm, on_cancel)` | Power off, Factory reset, Delete, Stop USB storage |
| Dial | `Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3).on_tap(..)`, `set_ring`, `set_talker`, `set_gauge`, `set_progress` | Home, Recording, Resting, File detail, Update, Call, Start-up |
| Rotary list | `RotaryList::new(root, items, on_select)` | Menu, System |
| Radio list | `RadioList::new(root, top, options, selected, on_select)` | After a recording, Time zone, Dim after |
| Value / switch rows | `ValueRow::new(..)`, `SwitchRow::new(..)` | Uploads, Display, Date and time |
| Value arc | `ValueArc::new(root, cx, cy, r, on_change)` | Volume, Mic level, Display |

Notes on the parts that carry state:

- The Dial is in one of three modes: ring (`set_ring`, `set_talker`), gauge (`set_gauge`) or
  progress (`set_progress`). Setting a gauge or progress replaces the ring; clearing it
  (`None`) returns to the ring.
- `Tone` picks a button's fill and text colour (`Tone::colors()`). `Tone::accent()` is the colour a
  dialog uses for that tone: red for destructive tones, teal otherwise.
- The rotary list has 48 px slots centred on y 152 inside `list::VIEW`. Rows fade and change
  band with distance from the focus line.

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
- `UI_PREVIEW=1` also writes panel-masked copies to `target/ui-preview/` for comparison
  with `docs/mockups/png/`. The mockup PNGs are 880×880 at 2×; the 400×304 framebuffer
  sits at x 40..840, y 136..744.
- A mismatch writes the actual frame to `target/golden-diff/<name>.actual.png`.
- A new screen needs a `Route` variant in `kit/route.rs` (the router builds screens from it);
  `Route::About` already exists.
- Taps in tests: `lv.click(obj)`, `lv.send(obj, code)`, and `lv.advance(ms)` for held buttons.
- `components/compositions.rs` shows Home and After a recording built from kit parts.

## Fonts and icons

`crates/waved-ui/fonts/build_fonts.sh` regenerates `src/kit/fonts/*.c` and `src/kit/icons.rs`; see
`crates/waved-ui/fonts/README.md`. To add an icon, add it to `crates/waved-ui/fonts/icons.py` and
rerun the script.
