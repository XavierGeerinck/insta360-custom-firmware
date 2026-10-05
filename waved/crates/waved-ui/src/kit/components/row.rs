//! The pill pair (two equal choices whose words matter, at
//! `BOTTOM_PILL_PAIR`) and the shared confirmation dialog built on it. Actions
//! that an icon says on its own use round buttons (`buttons::RoundButton`).

use crate::kit::components::buttons::{pill, Tone};
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::tokens::{Rect, BOTTOM_PILL_PAIR, SCREEN_W, TEXT, TEXT_2};
use std::cell::RefCell;
use std::rc::Rc;

/// The confirmation's icon disc, title top and body box (left, top, width;
/// the height follows the text, at most three lines).
const CONFIRM_DISC: Rect = Rect::new(172, 40, 56, 56);
const CONFIRM_TITLE_Y: i32 = 104;
const CONFIRM_BODY_X: i32 = 65;
const CONFIRM_BODY_Y: i32 = 142;
const CONFIRM_BODY_W: i32 = 270;
/// Space between a pill's icon and its words (the mockup's `.bp` gap).
const PILL_GAP: i32 = 8;

#[derive(Clone, Debug, PartialEq)]
pub struct ButtonSpec {
    pub icon: Option<&'static str>,
    pub text: String,
    pub tone: Tone,
}

impl ButtonSpec {
    pub fn new(icon: Option<&'static str>, text: impl Into<String>, tone: Tone) -> Self {
        Self {
            icon,
            text: text.into(),
            tone,
        }
    }
}

struct RowButton {
    obj: Obj,
    icon: Obj,
    text: Obj,
}

impl RowButton {
    /// Surface pill; the tone colours the icon and the words.
    fn apply(&self, spec: &ButtonSpec) {
        let fg = spec.tone.fg();
        self.icon
            .hidden(spec.icon.is_none())
            .text_color(fg)
            .set_text(spec.icon.unwrap_or(""));
        self.text.text_color(fg).set_text(&spec.text);
    }
}

pub struct ButtonRow {
    buttons: Vec<RowButton>,
}

impl ButtonRow {
    /// The pill pair mirrored around x 200: button 0 left, button 1 right.
    pub fn two(parent: Obj, specs: [ButtonSpec; 2], on_tap: impl FnMut(usize) + 'static) -> Self {
        let handler = Rc::new(RefCell::new(on_tap));
        let buttons = BOTTOM_PILL_PAIR
            .iter()
            .zip(&specs)
            .enumerate()
            .map(|(index, (rect, spec))| {
                let obj = pill(parent, *rect).row(PILL_GAP);
                let icon = Obj::label(obj, "", Font::Icon20, TEXT);
                let text = Obj::label(obj, "", Font::Body, TEXT);
                let handler = handler.clone();
                // A disabled button ignores taps even when the event is sent directly.
                on_click(obj, move || {
                    if obj.is_clickable() {
                        (handler.borrow_mut())(index)
                    }
                });
                let button = RowButton { obj, icon, text };
                button.apply(spec);
                button
            })
            .collect();
        Self { buttons }
    }

    pub fn set(&self, index: usize, spec: &ButtonSpec) {
        self.buttons[index].apply(spec);
    }

    /// Disabled buttons fade to 35 % and ignore taps.
    pub fn set_enabled(&self, index: usize, enabled: bool) {
        self.buttons[index]
            .obj
            .clickable(enabled)
            .opa(if enabled { 255 } else { 89 });
    }

    pub fn obj(&self, index: usize) -> Obj {
        self.buttons[index].obj
    }
}

#[derive(Clone, Debug)]
pub struct ConfirmSpec {
    pub icon: &'static str,
    pub title: String,
    pub body: String,
    pub confirm: String,
    pub tone: Tone,
}

/// A full-screen confirmation: icon, question, one line on the consequence,
/// then Cancel and the action (button 0 and button 1). The action pill
/// repeats the icon in the tone's accent.
pub fn confirm_dialog(
    root: Obj,
    spec: &ConfirmSpec,
    on_confirm: impl FnMut() + 'static,
    on_cancel: impl FnMut() + 'static,
) -> ButtonRow {
    let accent = spec.tone.accent();
    let disc = Obj::plain(root)
        .rect(CONFIRM_DISC)
        .round()
        .bg(tint(accent, 36));
    Obj::label(disc, spec.icon, Font::Icon26, accent).center();
    Obj::label(root, &spec.title, Font::Dial, TEXT)
        .centered_text(SCREEN_W)
        .pos(0, CONFIRM_TITLE_Y);
    Obj::label(root, &spec.body, Font::Small, TEXT_2)
        .wrapped_text(CONFIRM_BODY_W)
        .max_lines(3)
        .pos(CONFIRM_BODY_X, CONFIRM_BODY_Y);
    let (mut confirm, mut cancel) = (on_confirm, on_cancel);
    ButtonRow::two(
        root,
        [
            ButtonSpec::new(None, "Cancel", Tone::Neutral),
            ButtonSpec::new(Some(spec.icon), spec.confirm.clone(), spec.tone),
        ],
        move |index| if index == 0 { cancel() } else { confirm() },
    )
}

/// `hex` at `alpha`/255 over black.
pub fn tint(hex: u32, alpha: u32) -> u32 {
    let channel = |shift: u32| (((hex >> shift) & 0xff) * alpha / 255) << shift;
    channel(16) | channel(8) | channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::icons;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::kit::tokens::BOTTOM_PILL_PAIR;
    use std::cell::Cell;

    #[test]
    fn the_confirmation_names_the_action() {
        lvgl(|lv| {
            let root = lv.screen();
            let confirmed = Rc::new(Cell::new(false));
            let flag = confirmed.clone();
            let row = confirm_dialog(
                root,
                &ConfirmSpec {
                    icon: icons::POWER,
                    title: "Power off?".into(),
                    body: "A recording in progress is saved first. Turn Wave on again with the power button.".into(),
                    confirm: "Power off".into(),
                    tone: Tone::Rec,
                },
                move || flag.set(true),
                || {},
            );
            assert_golden("confirm_power_off", &lv.render());
            assert_targets_ok(root);
            let rect = |obj: Obj| {
                let a = obj.coords();
                Rect::new(a.x1, a.y1, a.x2 - a.x1 + 1, a.y2 - a.y1 + 1)
            };
            // Icon disc, title, body, then the pill pair.
            assert_eq!(rect(root.child(0)), Rect::new(172, 40, 56, 56));
            assert_eq!(rect(root.child(1)).y, 104);
            let body = rect(root.child(2));
            assert_eq!((body.x, body.y, body.w), (65, 142, 270));
            assert_eq!([rect(row.obj(0)), rect(row.obj(1))], BOTTOM_PILL_PAIR);
            lv.click(row.obj(1));
            assert!(confirmed.get());
        });
    }

    #[test]
    fn a_long_confirmation_body_stops_at_three_lines_above_the_buttons() {
        lvgl(|lv| {
            let root = lv.screen();
            confirm_dialog(
                root,
                &ConfirmSpec {
                    icon: icons::POWER,
                    title: "Power off?".into(),
                    body: "A recording in progress is saved first and uploaded when Wave is back online. \
                           Anything still queued for upload stays on the device until then. Turn Wave on \
                           again with the power button when you are ready to carry on."
                        .into(),
                    confirm: "Power off".into(),
                    tone: Tone::Rec,
                },
                || {},
                || {},
            );
            lv.render();
            let body = root.child(2);
            assert!(body.coords().y2 < BOTTOM_PILL_PAIR[0].y);
            assert!(body.text().ends_with("..."), "{}", body.text());
        });
    }

    #[test]
    fn a_destructive_confirmation_is_red_even_for_a_text_tone() {
        lvgl(|lv| {
            let root = lv.screen();
            confirm_dialog(
                root,
                &ConfirmSpec {
                    icon: icons::TRASH,
                    title: "Delete?".into(),
                    body: "This recording is removed from Wave.".into(),
                    confirm: "Delete".into(),
                    tone: Tone::RecText,
                },
                || {},
                || {},
            );
            let [r, g, b] = lv.render().pixel(200, 62);
            assert!(r > g && r > b, "disc is not red: {r},{g},{b}");
        });
    }

    #[test]
    fn a_disabled_button_ignores_taps_until_it_is_enabled_again() {
        lvgl(|lv| {
            let root = lv.screen();
            let taps = Rc::new(Cell::new(0));
            let count = taps.clone();
            let row = ButtonRow::two(
                root,
                [
                    ButtonSpec::new(None, "No", Tone::Neutral),
                    ButtonSpec::new(None, "Yes", Tone::Live),
                ],
                move |_| count.set(count.get() + 1),
            );
            row.set_enabled(1, false);
            lv.click(row.obj(1));
            assert_eq!(taps.get(), 0);
            row.set_enabled(1, true);
            assert!(row.obj(1).is_clickable());
            lv.click(row.obj(1));
            assert_eq!(taps.get(), 1);
        });
    }

    #[test]
    fn the_action_pill_is_surface_with_a_red_icon_and_red_words() {
        lvgl(|lv| {
            let root = lv.screen();
            let row = confirm_dialog(
                root,
                &ConfirmSpec {
                    icon: icons::POWER,
                    title: "Power off?".into(),
                    body: "Turn Wave on again with the power button.".into(),
                    confirm: "Power off".into(),
                    tone: Tone::Rec,
                },
                || {},
                || {},
            );
            let frame = lv.render();
            let action = row.obj(1);
            let (icon, words) = (action.child(0), action.child(1));
            assert!(!icon.is_hidden(), "the action repeats the dialog's icon");
            assert_eq!(icon.text(), icons::POWER);
            assert_eq!(words.text(), "Power off");
            // Red is never a fill: the pill is Surface (0x141c1e) near its edge.
            let [r, g, b] = frame.pixel(BOTTOM_PILL_PAIR[1].x + 14, 221);
            assert!(r < 40 && g < 40 && b < 40, "action pill fill {r},{g},{b}");
            // Its words are red, Cancel's are not.
            let reddest = |x0: i32, x1: i32| {
                (x0..x1)
                    .flat_map(|x| (210..232).map(move |y| (x, y)))
                    .map(|(x, y)| frame.pixel(x, y))
                    .filter(|[r, g, b]| *r > 200 && *g < 120 && *b < 120)
                    .count()
            };
            assert!(reddest(206, 330) > 20);
            assert_eq!(reddest(70, 194), 0);
            assert!(row.obj(0).child(0).is_hidden(), "Cancel has no icon");
        });
    }

    #[test]
    fn tint_blends_over_black() {
        assert_eq!(tint(0xff4b3e, 36), 0x240a08);
    }
}
