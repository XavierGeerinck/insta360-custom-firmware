//! Rows of two or three equal buttons, and the shared confirmation dialog.

use crate::kit::components::buttons::Tone;
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::tokens::{Rect, ROW2, ROW3, SCREEN_W, TEXT, TEXT_2};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub struct ButtonSpec {
    pub icon: Option<&'static str>,
    pub text: String,
    pub tone: Tone,
}

impl ButtonSpec {
    pub fn new(icon: Option<&'static str>, text: impl Into<String>, tone: Tone) -> Self {
        Self { icon, text: text.into(), tone }
    }
}

struct RowButton {
    obj: Obj,
    icon: Obj,
    text: Obj,
}

impl RowButton {
    fn apply(&self, spec: &ButtonSpec) {
        let (fill, fg) = spec.tone.colors();
        self.obj.bg(fill).pressed_bg(spec.tone.pressed());
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
    pub fn two(parent: Obj, specs: [ButtonSpec; 2], on_tap: impl FnMut(usize) + 'static) -> Self {
        Self::build(parent, &ROW2, &specs, on_tap)
    }

    pub fn three(parent: Obj, specs: [ButtonSpec; 3], on_tap: impl FnMut(usize) + 'static) -> Self {
        Self::build(parent, &ROW3, &specs, on_tap)
    }

    fn build(parent: Obj, rects: &[Rect], specs: &[ButtonSpec], on_tap: impl FnMut(usize) + 'static) -> Self {
        let handler = Rc::new(RefCell::new(on_tap));
        let buttons = rects
            .iter()
            .zip(specs)
            .enumerate()
            .map(|(index, (rect, spec))| {
                let obj = Obj::plain(parent).rect(*rect).radius(rect.h / 2).clickable(true).row(6);
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
/// then Cancel and the action (button 0 and button 1).
pub fn confirm_dialog(
    root: Obj,
    spec: &ConfirmSpec,
    on_confirm: impl FnMut() + 'static,
    on_cancel: impl FnMut() + 'static,
) -> ButtonRow {
    let accent = spec.tone.accent();
    let disc = Obj::plain(root).rect(Rect::new(172, 58, 56, 56)).round().bg(tint(accent, 36));
    Obj::label(disc, spec.icon, Font::Icon26, accent).center();
    Obj::label(root, &spec.title, Font::Dial, TEXT).centered_text(SCREEN_W).pos(0, 128);
    Obj::label(root, &spec.body, Font::Small, TEXT_2).wrapped_text(260).max_lines(3).pos(70, 166);
    let (mut confirm, mut cancel) = (on_confirm, on_cancel);
    ButtonRow::two(
        root,
        [
            ButtonSpec::new(None, "Cancel", Tone::Neutral),
            ButtonSpec::new(None, spec.confirm.clone(), spec.tone),
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
    use std::cell::Cell;

    #[test]
    fn the_recording_row_reports_taps_and_can_disable_a_button() {
        lvgl(|lv| {
            let root = lv.screen();
            let taps = Rc::new(RefCell::new(Vec::new()));
            let log = taps.clone();
            let row = ButtonRow::three(
                root,
                [
                    ButtonSpec::new(Some(icons::MARK), "Mark", Tone::Neutral),
                    ButtonSpec::new(Some(icons::PAUSE), "Pause", Tone::Neutral),
                    ButtonSpec::new(Some(icons::STOP), "Stop", Tone::Rec),
                ],
                move |index| log.borrow_mut().push(index),
            );
            assert_golden("row_recording", &lv.render());
            assert_targets_ok(root);
            lv.click(row.obj(2));
            lv.click(row.obj(0));
            assert_eq!(*taps.borrow(), vec![2, 0]);

            row.set_enabled(0, false);
            row.set(1, &ButtonSpec::new(Some(icons::PLAY), "Resume", Tone::WarnText));
            assert!(!row.obj(0).is_clickable());
            assert_golden("row_paused", &lv.render());
        });
    }

    #[test]
    fn the_call_row_has_decline_and_answer() {
        lvgl(|lv| {
            let root = lv.screen();
            ButtonRow::two(
                root,
                [
                    ButtonSpec::new(Some(icons::PHONE_OFF), "Decline", Tone::Rec),
                    ButtonSpec::new(Some(icons::PHONE), "Answer", Tone::Live),
                ],
                |_| {},
            );
            assert_golden("row_call", &lv.render());
            assert_targets_ok(root);
        });
    }

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
            assert!(body.coords().y2 < ROW2[0].y);
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
    fn tint_blends_over_black() {
        assert_eq!(tint(0xff4b3e, 36), 0x240a08);
    }
}
