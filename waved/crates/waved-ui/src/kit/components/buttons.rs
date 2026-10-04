//! Buttons: the one Back button, text buttons, the primary action and the
//! side buttons (Quieter and Louder).

use crate::bindings::{
    lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT, lv_event_code_t_LV_EVENT_PRESSED, lv_tick_elaps, lv_tick_get,
};
use crate::kit::dispatch::Dispatcher;
use crate::kit::event::{on, on_click};
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::tokens::{
    Rect, BACK, LIVE, ON_LIVE, RAISED, REC, SIDE_LABEL_Y, SIDE_LEFT, SIDE_RIGHT, SURFACE, TEXT, TEXT_2, WARN, WHITE,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Colour roles for filled buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Live,
    Rec,
    /// Neutral fill, red text: Delete.
    RecText,
    /// Neutral fill, amber text: Resume.
    WarnText,
}

impl Tone {
    /// (fill, text and icon colour)
    pub fn colors(self) -> (u32, u32) {
        match self {
            Tone::Neutral => (SURFACE, TEXT),
            Tone::Live => (LIVE, ON_LIVE),
            Tone::Rec => (REC, WHITE),
            Tone::RecText => (SURFACE, REC),
            Tone::WarnText => (SURFACE, WARN),
        }
    }

    /// The accent a dialog uses for this tone: red for destructive tones,
    /// teal otherwise.
    pub fn accent(self) -> u32 {
        match self {
            Tone::Rec | Tone::RecText => REC,
            Tone::Neutral | Tone::Live | Tone::WarnText => LIVE,
        }
    }

    /// The fill while pressed.
    pub fn pressed(self) -> u32 {
        match self {
            Tone::Live | Tone::Rec => shade(self.colors().0),
            Tone::Neutral | Tone::RecText | Tone::WarnText => RAISED,
        }
    }
}

/// The pressed shade of a fill: 85 % brightness per channel.
pub fn shade(hex: u32) -> u32 {
    let channel = |shift: u32| (((hex >> shift) & 0xff) * 85 / 100) << shift;
    channel(16) | channel(8) | channel(0)
}

fn filled(parent: Obj, rect: Rect, tone: Tone) -> Obj {
    Obj::plain(parent)
        .rect(rect)
        .bg(tone.colors().0)
        .radius(rect.h / 2)
        .pressed_bg(tone.pressed())
        .clickable(true)
}

/// The one Back button: 112×44 at the bottom centre.
pub fn back_button(parent: Obj, dispatch: &Dispatcher) -> Obj {
    let button = filled(parent, BACK, Tone::Neutral).row(2);
    Obj::label(button, icons::LEFT, Font::Icon16, TEXT);
    Obj::label(button, "Back", Font::Body, TEXT);
    let dispatch = dispatch.clone();
    on_click(button, move || dispatch.back());
    button
}

/// A text button in the Back style: "Cancel", "Stop USB storage".
pub fn text_button(parent: Obj, rect: Rect, text: &str, on_tap: impl FnMut() + 'static) -> Obj {
    let button = filled(parent, rect, Tone::Neutral).row(0);
    Obj::label(button, text, Font::Body, TEXT);
    on_click(button, on_tap);
    button
}

pub struct PrimaryButton {
    obj: Obj,
    subtitle: Obj,
}

impl PrimaryButton {
    /// The main action: icon, title and an optional second line.
    pub fn new(
        parent: Obj,
        rect: Rect,
        tone: Tone,
        icon: &str,
        title: &str,
        subtitle: Option<&str>,
        on_tap: impl FnMut() + 'static,
    ) -> Self {
        let fg = tone.colors().1;
        let obj = filled(parent, rect, tone).row(10);
        Obj::label(obj, icon, Font::Icon20, fg);
        let stack = Obj::plain(obj).fit().column(2);
        Obj::label(stack, title, Font::Title, fg);
        let subtitle_label = Obj::label(stack, subtitle.unwrap_or(""), Font::Micro, fg)
            .opa(217)
            .hidden(subtitle.is_none());
        on_click(obj, on_tap);
        Self { obj, subtitle: subtitle_label }
    }

    pub fn set_subtitle(&self, subtitle: Option<&str>) {
        self.subtitle.hidden(subtitle.is_none()).set_text(subtitle.unwrap_or(""));
    }

    pub fn obj(&self) -> Obj {
        self.obj
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Repeat interval while a side button is held.
pub const REPEAT_MS: u32 = 300;

pub struct SideButton {
    obj: Obj,
    icon: Obj,
    label: Obj,
}

impl SideButton {
    /// A 60 px circle at the left or right edge with a 12 px label below.
    /// `on_step` runs on press and every 300 ms while held.
    pub fn new(parent: Obj, side: Side, icon: &str, label: &str, on_step: impl FnMut() + 'static) -> Self {
        let rect = match side {
            Side::Left => SIDE_LEFT,
            Side::Right => SIDE_RIGHT,
        };
        let obj = Obj::plain(parent)
            .rect(rect)
            .bg(SURFACE)
            .round()
            .pressed_bg(RAISED)
            .pressed_border(2, LIVE)
            .clickable(true);
        let icon_label = Obj::label(obj, icon, Font::Icon26, TEXT).center();
        let label_obj = Obj::label(parent, label, Font::Micro, TEXT_2)
            .centered_text(90)
            .pos(rect.x + rect.w / 2 - 45, SIDE_LABEL_Y);
        let step = Rc::new(RefCell::new(on_step));
        let last = Rc::new(Cell::new(0u32));
        {
            let (step, last) = (step.clone(), last.clone());
            on(obj, lv_event_code_t_LV_EVENT_PRESSED, move |_| {
                // SAFETY: LVGL tick read on the UI thread.
                last.set(unsafe { lv_tick_get() });
                (step.borrow_mut())();
            });
        }
        on(obj, lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT, move |_| {
            // SAFETY: LVGL tick reads on the UI thread.
            if unsafe { lv_tick_elaps(last.get()) } >= REPEAT_MS {
                last.set(unsafe { lv_tick_get() });
                (step.borrow_mut())();
            }
        });
        Self { obj, icon: icon_label, label: label_obj }
    }

    pub fn set_icon(&self, icon: &str) {
        self.icon.set_text(icon);
    }

    pub fn set_label(&self, text: &str) {
        self.label.set_text(text);
    }

    pub fn obj(&self) -> Obj {
        self.obj
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::{lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT, lv_event_code_t_LV_EVENT_PRESSED};
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::kit::tokens::PRIMARY;

    #[test]
    fn back_goes_back() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, ui) = Dispatcher::test();
            let back = back_button(root, &dispatch);
            assert_golden("back_button", &lv.render());
            assert_targets_ok(root);
            lv.click(back);
            assert_eq!(ui.try_recv().unwrap(), UiEvent::Back);
        });
    }

    #[test]
    fn primary_button_carries_the_pre_recording_line() {
        lvgl(|lv| {
            let root = lv.screen();
            let taps = Rc::new(Cell::new(0));
            let counter = taps.clone();
            let record = PrimaryButton::new(
                root,
                PRIMARY,
                Tone::Rec,
                icons::RECORD,
                "Record",
                Some("includes last 30 s"),
                move || counter.set(counter.get() + 1),
            );
            assert_golden("primary_record", &lv.render());
            assert_targets_ok(root);
            lv.click(record.obj());
            assert_eq!(taps.get(), 1);
            record.set_subtitle(None);
            assert_golden("primary_record_no_subtitle", &lv.render());
        });
    }

    #[test]
    fn side_buttons_step_on_press_and_every_300_ms_while_held() {
        lvgl(|lv| {
            let root = lv.screen();
            let steps = Rc::new(Cell::new(0));
            let counter = steps.clone();
            SideButton::new(root, Side::Left, icons::VOLUME_LOW, "Quieter", || {});
            let louder = SideButton::new(root, Side::Right, icons::VOLUME_HIGH, "Louder", move || {
                counter.set(counter.get() + 1)
            });
            assert_golden("side_buttons", &lv.render());
            assert_targets_ok(root);
            lv.send(louder.obj(), lv_event_code_t_LV_EVENT_PRESSED);
            assert_eq!(steps.get(), 1);
            lv.advance(100);
            lv.send(louder.obj(), lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT);
            assert_eq!(steps.get(), 1, "repeats wait 300 ms");
            lv.advance(250);
            lv.send(louder.obj(), lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT);
            assert_eq!(steps.get(), 2);
        });
    }

    #[test]
    fn shade_darkens_each_channel() {
        assert_eq!(shade(0xff4b3e), 0xd83f34);
    }
}
