//! Display (B6, display.png): backlight brightness on a value arc and when
//! the resting screen takes over ("Dim after", a radio list). E1 drives
//! /sys/class/leds/lcd-backlight from the brightness setting.

use crate::bindings::LV_STATE_PRESSED;
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
    /// The − / + circles; only the tests look at them again.
    #[cfg(test)]
    minus: Option<Obj>,
    #[cfg(test)]
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
                dispatch.command(Command::SetBrightness(brightness_step(
                    brightness.get(),
                    up,
                )))
            });
            #[cfg(test)]
            if up {
                self.plus = Some(button);
            } else {
                self.minus = Some(button);
            }
            #[cfg(not(test))]
            let _ = button;
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
        root.update_layout();
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let brightness = snap.settings.brightness_pct;
        self.brightness.set(brightness);
        // The arc follows the snapshot unless a finger is on it.
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
            .map(|&s| {
                RadioOption::new(
                    duration_label(s),
                    screen_off_detail(s, snap.settings.off_after_s),
                )
            })
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

#[cfg(test)]
mod tests {
    use super::*;
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
            assert_targets_ok(root);
            lv.click(screen.minus.unwrap());
            lv.click(screen.minus.unwrap()); // the handler only sees the next snapshot
            lv.click(screen.plus.unwrap());
            assert!(matches!(
                f.commands().as_slice(),
                [
                    Command::SetBrightness(5),
                    Command::SetBrightness(5),
                    Command::SetBrightness(20)
                ]
            ));
            let arc = screen.arc.as_ref().unwrap();
            arc.set_value(2);
            lv.send(
                arc.obj(),
                crate::bindings::lv_event_code_t_LV_EVENT_VALUE_CHANGED,
            );
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetBrightness(5)]
            ));
        });
    }

    #[test]
    fn dim_after_opens_its_list() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = DisplayScreen::default();
            screen.build(root, &f.ctx, &snap());
            assert_targets_ok(root);
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
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetDimAfter(300)]
            ));
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
