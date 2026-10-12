//! The PIN keypad (K2, docs/mockups/png/pin.png): a 3 × 4 grid of 64 × 48
//! keys at x 96/168/240, y 56/110/164/218 (1–9, then Cancel, 0, Backspace),
//! and the row of entry dots at top 36.

use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::tokens::{Rect, PANEL_CX, RAISED, SURFACE, TEXT, TEXT_2, TEXT_3};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// One key of the keypad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Digit(u8),
    Cancel,
    Backspace,
}

pub const KEY_W: i32 = 64;
pub const KEY_H: i32 = 48;
/// Left edges of the three columns...
pub const KEY_X: [i32; 3] = [96, 168, 240];
/// ...and tops of the four rows.
pub const KEY_Y: [i32; 4] = [56, 110, 164, 218];
/// Keys in reading order.
pub const KEYS: [Key; 12] = [
    Key::Digit(1),
    Key::Digit(2),
    Key::Digit(3),
    Key::Digit(4),
    Key::Digit(5),
    Key::Digit(6),
    Key::Digit(7),
    Key::Digit(8),
    Key::Digit(9),
    Key::Cancel,
    Key::Digit(0),
    Key::Backspace,
];

/// Top of the dots, 12 px wide with 12 px between them, centred on x 200.
pub const DOTS_Y: i32 = 36;
pub const DOT: i32 = 12;
pub const DOT_GAP: i32 = 12;

const DOT_RING: i32 = 2;

impl Key {
    /// What the key shows, in which font and colour while enabled.
    fn face(self) -> (String, Font, u32) {
        match self {
            Key::Digit(digit) => (digit.to_string(), Font::Num20, TEXT),
            Key::Cancel => ("Cancel".into(), Font::Small, TEXT_2),
            Key::Backspace => (icons::BACKSPACE.into(), Font::Icon22, TEXT_2),
        }
    }
}

struct KeyObj {
    key: Key,
    obj: Obj,
    label: Obj,
}

/// Twelve Surface keys, Raised while pressed. Each tap reports its `Key`.
pub struct Keypad {
    keys: Vec<KeyObj>,
    enabled: Rc<Cell<bool>>,
}

impl Keypad {
    /// Build into `parent`, a full-screen container without a layout.
    pub fn new(parent: Obj, on_key: impl FnMut(Key) + 'static) -> Self {
        let on_key: Rc<RefCell<dyn FnMut(Key)>> = Rc::new(RefCell::new(on_key));
        let enabled = Rc::new(Cell::new(true));
        let keys = KEYS
            .into_iter()
            .enumerate()
            .map(|(i, key)| {
                let obj = Obj::plain(parent)
                    .rect(Rect::new(KEY_X[i % 3], KEY_Y[i / 3], KEY_W, KEY_H))
                    .bg(SURFACE)
                    .radius(KEY_H / 2)
                    .pressed_bg(RAISED)
                    .clickable(true);
                let (text, font, colour) = key.face();
                let label = Obj::label(obj, &text, font, colour).center();
                let (on_key, enabled) = (on_key.clone(), enabled.clone());
                on_click(obj, move || {
                    if key == Key::Cancel || enabled.get() {
                        (on_key.borrow_mut())(key);
                    }
                });
                KeyObj { key, obj, label }
            })
            .collect();
        Self { keys, enabled }
    }

    /// Digits and Backspace grey out to Text 3 and ignore taps; Cancel always works.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.set(enabled);
        for key in self.keys.iter().filter(|key| key.key != Key::Cancel) {
            key.obj.clickable(enabled);
            let colour = if enabled { key.key.face().2 } else { TEXT_3 };
            key.label.text_color(colour);
        }
    }

    pub fn key(&self, key: Key) -> Obj {
        self.keys
            .iter()
            .find(|candidate| candidate.key == key)
            .map(|found| found.obj)
            .expect("every key is on the keypad")
    }
}

/// One dot per PIN digit: filled (Text) for each digit typed, a Text 3 ring otherwise.
pub struct PinDots {
    dots: Vec<Obj>,
}

impl PinDots {
    pub fn new(parent: Obj, count: usize) -> Self {
        let n = count as i32;
        let left = PANEL_CX - (n * DOT + (n - 1) * DOT_GAP) / 2;
        let dots = (0..n)
            .map(|i| {
                Obj::plain(parent)
                    .rect(Rect::new(left + i * (DOT + DOT_GAP), DOTS_Y, DOT, DOT))
                    .round()
            })
            .collect();
        let dots = Self { dots };
        dots.set_filled(0);
        dots
    }

    pub fn set_filled(&self, filled: usize) {
        for (i, dot) in self.dots.iter().enumerate() {
            if i < filled {
                dot.bg(TEXT).border(0, TEXT);
            } else {
                dot.bg_opa(0).border(DOT_RING, TEXT_3);
            }
        }
    }

    /// Tests read the dots.
    #[cfg(test)]
    fn dot(&self, index: usize) -> Obj {
        self.dots[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::*;
    use crate::kit::testkit::{assert_targets_ok, lvgl};
    use crate::kit::tokens::{TEXT, TEXT_3};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn area(obj: Obj) -> (i32, i32, i32, i32) {
        let a = obj.coords();
        (a.x1, a.y1, a.x2 - a.x1 + 1, a.y2 - a.y1 + 1)
    }

    fn prop(obj: Obj, prop: u32) -> lv_style_value_t {
        // SAFETY: live object under the test lock.
        unsafe {
            lv_obj_get_style_prop(
                obj.raw(),
                LV_PART_MAIN as lv_style_selector_t,
                prop as lv_style_prop_t,
            )
        }
    }

    fn num(obj: Obj, which: u32) -> i32 {
        // SAFETY: opacity and width props are numbers.
        unsafe { prop(obj, which).num }
    }

    fn color_of(obj: Obj, which: u32) -> u32 {
        // SAFETY: colour props are colours.
        let c = unsafe { prop(obj, which).color };
        (u32::from(c.red) << 16) | (u32::from(c.green) << 8) | u32::from(c.blue)
    }

    /// The fill colour, or `None` when the background is transparent.
    fn fill(obj: Obj) -> Option<u32> {
        (num(obj, LV_STYLE_BG_OPA) != 0).then(|| color_of(obj, LV_STYLE_BG_COLOR))
    }

    /// The border colour, or `None` without a border.
    fn ring(obj: Obj) -> Option<u32> {
        (num(obj, LV_STYLE_BORDER_WIDTH) != 0).then(|| color_of(obj, LV_STYLE_BORDER_COLOR))
    }

    #[test]
    fn keys_sit_on_the_mockup_grid_inside_the_tap_limit() {
        lvgl(|lv| {
            let root = lv.screen();
            let pad = Keypad::new(root, |_| {});
            root.update_layout();
            for (i, key) in KEYS.into_iter().enumerate() {
                let (x, y) = (KEY_X[i % 3], KEY_Y[i / 3]);
                assert_eq!(area(pad.key(key)), (x, y, KEY_W, KEY_H), "{key:?}");
            }
            assert_targets_ok(root);
        });
    }

    #[test]
    fn taps_report_their_key() {
        lvgl(|lv| {
            let root = lv.screen();
            let seen = Rc::new(RefCell::new(Vec::new()));
            let log = seen.clone();
            let pad = Keypad::new(root, move |key| log.borrow_mut().push(key));
            for key in KEYS {
                lv.click(pad.key(key));
            }
            assert_eq!(*seen.borrow(), KEYS);
        });
    }

    #[test]
    fn a_disabled_keypad_only_cancels() {
        lvgl(|lv| {
            let root = lv.screen();
            let seen = Rc::new(RefCell::new(Vec::new()));
            let log = seen.clone();
            let pad = Keypad::new(root, move |key| log.borrow_mut().push(key));
            pad.set_enabled(false);
            for key in KEYS {
                lv.click(pad.key(key));
            }
            assert_eq!(*seen.borrow(), [Key::Cancel]);
            assert!(!pad.key(Key::Digit(5)).is_clickable());
            assert!(pad.key(Key::Cancel).is_clickable());
            pad.set_enabled(true);
            lv.click(pad.key(Key::Digit(5)));
            assert_eq!(*seen.borrow(), [Key::Cancel, Key::Digit(5)]);
        });
    }

    #[test]
    fn dots_are_centred_and_fill_from_the_left() {
        lvgl(|lv| {
            let root = lv.screen();
            for count in [4, 6] {
                let dots = PinDots::new(root, count);
                root.update_layout();
                let width = count as i32 * DOT + (count as i32 - 1) * DOT_GAP;
                let first = area(dots.dot(0));
                assert_eq!(first, (200 - width / 2, DOTS_Y, DOT, DOT), "{count}");
                let last = area(dots.dot(count - 1));
                assert_eq!(last.0 + DOT, 200 + width / 2, "{count}");
                dots.set_filled(2);
                assert_eq!(fill(dots.dot(0)), Some(TEXT));
                assert_eq!((fill(dots.dot(1)), ring(dots.dot(1))), (Some(TEXT), None));
                assert_eq!(fill(dots.dot(2)), None, "an empty dot is a ring");
                assert_eq!(ring(dots.dot(2)), Some(TEXT_3));
                dots.set_filled(0);
                assert_eq!(fill(dots.dot(0)), None);
            }
        });
    }
}
