//! Bluetooth pieces (workstream I): the device row (bluetooth.png), a pill
//! with an icon ("Pair a device") and the call rings (call.png).

use crate::kit::components::buttons::pill;
use crate::kit::components::dial::{arc, arc_style, full_ring, INDICATOR, MAIN};
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::obj::{text_width, Obj};
use crate::kit::tokens::{Rect, LIVE, SURFACE, TEXT, TEXT_2};

/// The mockup's `.dev`: radius 20, 16 px side padding, 12 px between parts.
const ROW_RADIUS: i32 = 20;
const ROW_PAD: i32 = 16;
const ROW_GAP: i32 = 12;
/// The value never takes more than this share of the row.
const VALUE_CAP_DIV: i32 = 3;
/// The mockup's `.btn` gap between the icon and the words.
const BUTTON_GAP: i32 = 6;

/// What a device row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceLook {
    pub icon: &'static str,
    pub name: String,
    pub name_color: u32,
    /// "Connected" and its colour; empty hides it.
    pub value: String,
    pub value_color: u32,
}

/// A Surface row: icon, a one-line name, a short state at the right. Not
/// tappable: the module offers no per-device action (no connect or forget).
pub struct DeviceRow {
    obj: Obj,
    icon: Obj,
    name: Obj,
    value: Obj,
    cap: i32,
}

impl DeviceRow {
    pub fn new(parent: Obj, rect: Rect) -> Self {
        let obj = Obj::plain(parent)
            .rect(rect)
            .bg(SURFACE)
            .radius(ROW_RADIUS)
            .row_start(ROW_GAP, ROW_PAD);
        let icon = Obj::label(obj, "", Font::Icon20, TEXT_2);
        let name = Obj::label(obj, "", Font::Body, TEXT).grow().one_line();
        let value = Obj::label(obj, "", Font::Small, LIVE).one_line();
        Self {
            obj,
            icon,
            name,
            value,
            cap: rect.w / VALUE_CAP_DIV,
        }
    }

    pub fn set(&self, look: &DeviceLook) {
        self.icon.set_text(look.icon);
        self.name.text_color(look.name_color).set_text(&look.name);
        // An explicit width, not `max_width` (kit README).
        self.value
            .hidden(look.value.is_empty())
            .text_color(look.value_color)
            .width(text_width(&look.value, Font::Small).min(self.cap))
            .set_text(&look.value);
    }

    pub fn obj(&self) -> Obj {
        self.obj
    }

    pub fn name(&self) -> Obj {
        self.name
    }
}

/// A content pill with an icon before its words: "Pair a device".
pub fn icon_button(
    parent: Obj,
    rect: Rect,
    icon: &str,
    text: &str,
    on_tap: impl FnMut() + 'static,
) -> Obj {
    let button = pill(parent, rect).row(BUTTON_GAP);
    Obj::label(button, icon, Font::Icon16, TEXT);
    Obj::label(button, text, Font::Body, TEXT);
    on_click(button, on_tap);
    button
}

/// The incoming call's rings: r 76 / 82 / 88 around (cx, cy) in Live at
/// full, 30 % and 12 % (docs/mockups/README.md). Drawn still, as the mockup
/// shows them; the outer two are the ripples, hidden once the call is up.
pub struct CallRings {
    ring: Obj,
    ripples: [Obj; 2],
}

/// (radius, stroke, opacity) of the ring and the two ripples.
pub const CALL_RINGS: [(i32, i32, u8); 3] = [(76, 3, 255), (82, 2, 77), (88, 2, 31)];

impl CallRings {
    pub fn new(parent: Obj, cx: i32, cy: i32) -> Self {
        let ring = |(r, width, opa): (i32, i32, u8)| {
            let obj = arc(parent, cx, cy, r, width);
            full_ring(obj);
            arc_style(obj, MAIN, LIVE, opa, width);
            arc_style(obj, INDICATOR, LIVE, 0, width);
            obj
        };
        Self {
            ring: ring(CALL_RINGS[0]),
            ripples: [ring(CALL_RINGS[1]), ring(CALL_RINGS[2])],
        }
    }

    /// Ripples while the phone rings; the ring alone once the call is up.
    pub fn set_ripples(&self, on: bool) {
        for ripple in self.ripples {
            ripple.hidden(!on);
        }
    }

    pub fn ring(&self) -> Obj {
        self.ring
    }

    pub fn ripples_visible(&self) -> bool {
        !self.ripples[0].is_hidden()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::icons;
    use crate::kit::testkit::{assert_targets_ok, lvgl};
    use crate::kit::tokens::TEXT_3;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn a_device_row_shows_its_look_and_takes_no_taps() {
        lvgl(|lv| {
            let root = lv.screen();
            let row = DeviceRow::new(root, Rect::new(60, 82, 280, 48));
            row.set(&DeviceLook {
                icon: icons::MOBILE,
                name: "iPhone".into(),
                name_color: TEXT,
                value: "Connected".into(),
                value_color: LIVE,
            });
            root.update_layout();
            assert_eq!(row.name().text(), "iPhone");
            assert!(!row.obj().is_clickable());
            row.set(&DeviceLook {
                icon: icons::MOBILE,
                name: "No phone connected".into(),
                name_color: TEXT_3,
                value: String::new(),
                value_color: TEXT_3,
            });
            assert!(row.obj().child(2).is_hidden(), "no value");
            assert_targets_ok(root);
        });
    }

    #[test]
    fn a_long_name_ends_in_dots_inside_the_row() {
        lvgl(|lv| {
            let root = lv.screen();
            let row = DeviceRow::new(root, Rect::new(60, 82, 280, 48));
            row.set(&DeviceLook {
                icon: icons::MOBILE,
                name: "W".repeat(40),
                name_color: TEXT,
                value: "Connected".into(),
                value_color: LIVE,
            });
            root.update_layout();
            let name = row.name();
            assert!(name.text().ends_with("..."), "{}", name.text());
            let (a, value) = (name.coords(), row.obj().child(2).coords());
            assert!(a.x1 >= 60 && a.x2 < value.x1, "the name stops before the value");
            assert!(value.x2 <= 339);
        });
    }

    #[test]
    fn the_icon_button_taps() {
        lvgl(|lv| {
            let root = lv.screen();
            let taps = Rc::new(Cell::new(0));
            let count = taps.clone();
            let button = icon_button(root, Rect::new(108, 192, 184, 44), icons::BLUETOOTH, "Pair a device", move || {
                count.set(count.get() + 1)
            });
            lv.click(button);
            assert_eq!(taps.get(), 1);
            assert_eq!(button.child(1).text(), "Pair a device");
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_ripples_go_once_the_call_is_up() {
        lvgl(|lv| {
            let root = lv.screen();
            let rings = CallRings::new(root, 200, 118);
            assert!(rings.ripples_visible());
            rings.set_ripples(false);
            assert!(!rings.ripples_visible());
            assert!(!rings.ring().is_hidden());
            assert!(!rings.ring().is_clickable());
            assert_targets_ok(root);
        });
    }
}
