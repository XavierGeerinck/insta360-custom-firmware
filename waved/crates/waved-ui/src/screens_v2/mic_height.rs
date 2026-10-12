//! Mic height (B5, mic-height.png): the lift's three stops, High 50 mm,
//! Medium 40 mm and Low 30 mm (stock's). A tap sends `SetMotorPosition`:
//! waved-mcu moves the lift and stores the height in the MCU, and
//! waved-settings keeps it in config.toml.
//!
//! The lift's motion comes from `McuState::lift`: while it moves, the chosen
//! row reads "Moving" and no row takes a tap; when a move ends blocked or
//! obstructed, the chosen row shows a warning and the hint says what to do
//! (the runtime also shows "Mic lift blocked"). A tap on the same height
//! tries again.

use crate::kit::components::buttons::back_button;
use crate::kit::components::status::StatusArea;
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{text_width, Obj};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    Rect, LIVE, RAISED, SCREEN_W, SURFACE, TEXT, TEXT_2, TEXT_3, TITLE_Y, WARN,
};
use crate::screens_v2::menu::Pending;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::{Command, LiftState, MotorPosition};

/// Top to bottom, as the lift goes.
pub const HEIGHTS: [MotorPosition; 3] = [MotorPosition::High, MotorPosition::Medium, MotorPosition::Low];
const ROW_X: i32 = 116;
const ROW_W: i32 = 168;
const ROW_H: i32 = 44;
const ROW_Y: [i32; 3] = [70, 120, 170];
/// The mockup's 18 px side padding: 2 px border plus 16.
const ROW_PAD: i32 = 16;
const HINT_Y: i32 = 222;

pub const HINT: &str = "Raise it when laptops block the table";
pub const BLOCKED_HINT: &str = "Blocked, clear it and tap again";
pub const MOVING: &str = "Moving";

/// The word for a height (also the Menu's value).
pub fn height_name(height: MotorPosition) -> &'static str {
    match height {
        MotorPosition::High => "High",
        MotorPosition::Medium => "Medium",
        MotorPosition::Low => "Low",
        MotorPosition::Moving => MOVING,
    }
}

fn height_index(height: MotorPosition) -> Option<usize> {
    HEIGHTS.iter().position(|h| *h == height)
}

struct Row {
    obj: Obj,
    label: Obj,
    /// The check, or the warning after a blocked move.
    mark: Obj,
    moving: Obj,
}

#[derive(Default)]
pub struct MicHeightScreen {
    status: Option<StatusArea>,
    rows: Vec<Row>,
    hint: Option<Obj>,
    /// The height the user tapped, shown until settings report it.
    chosen: Rc<Pending<MotorPosition>>,
    /// Rows take taps (not while the lift moves).
    enabled: Rc<Cell<bool>>,
    shown: Option<(MotorPosition, LiftState)>,
}

impl Screen for MicHeightScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Mic height", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        self.enabled.set(true);
        for (index, (&height, &y)) in HEIGHTS.iter().zip(&ROW_Y).enumerate() {
            let obj = Obj::plain(root)
                .rect(Rect::new(ROW_X, y, ROW_W, ROW_H))
                .radius(ROW_H / 2)
                .bg(SURFACE)
                .border(2, SURFACE)
                .pressed_bg(RAISED)
                .clickable(true)
                .row_start(8, ROW_PAD);
            let label = Obj::label(obj, height_name(height), Font::Body, TEXT)
                .one_line()
                .grow();
            let mark = Obj::label(obj, icons::CHECK, Font::Icon20, LIVE).hidden(true);
            let moving = Obj::label(obj, MOVING, Font::Small, TEXT_2)
                .one_line()
                .width(text_width(MOVING, Font::Small) + 1)
                .hidden(true);
            let (chosen, enabled, dispatch) =
                (self.chosen.clone(), self.enabled.clone(), ctx.dispatch.clone());
            on_click(obj, move || {
                if !enabled.get() {
                    return;
                }
                chosen.set(HEIGHTS[index]);
                dispatch.command(Command::SetMotorPosition(HEIGHTS[index]));
            });
            self.rows.push(Row { obj, label, mark, moving });
        }
        self.hint = Some(
            Obj::label(root, HINT, Font::Small, TEXT_2)
                .centered_text(SCREEN_W)
                .pos(0, HINT_Y),
        );
        back_button(root, &ctx.dispatch);
        self.update(snap);
        root.update_layout();
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let height = self.chosen.follow(snap.settings.mic_height);
        let lift = snap.mcu.lift;
        if self.shown == Some((height, lift)) {
            return;
        }
        self.shown = Some((height, lift));
        let selected = height_index(height);
        let moving = lift.is_moving();
        self.enabled.set(!moving);
        for (index, row) in self.rows.iter().enumerate() {
            let on = selected == Some(index);
            if on {
                row.obj.bg(RAISED).border(2, LIVE);
            } else {
                row.obj.bg(SURFACE).border(2, SURFACE);
            }
            row.obj.clickable(!moving);
            row.label.text_color(if moving && !on { TEXT_3 } else { TEXT });
            row.moving.hidden(!(on && moving));
            row.mark.hidden(!on || moving);
            if lift.is_stopped() {
                row.mark.text_color(WARN).set_text(icons::ALERT);
            } else {
                row.mark.text_color(LIVE).set_text(icons::CHECK);
            }
        }
        if let Some(hint) = self.hint {
            if lift.is_stopped() {
                hint.text_color(WARN).set_text(BLOCKED_HINT);
            } else {
                hint.text_color(TEXT_2).set_text(HINT);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    fn mockup() -> UiSnapshot {
        let mut snap = snap();
        snap.clock = "14:34".into();
        snap.settings.mic_height = MotorPosition::Medium;
        snap
    }

    fn rect(obj: Obj) -> Rect {
        let a = obj.coords();
        Rect::new(a.x1, a.y1, a.x2 - a.x1 + 1, a.y2 - a.y1 + 1)
    }

    #[test]
    fn mic_height_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = MicHeightScreen::default();
            screen.build(root, &f.ctx, &mockup());
            assert_golden("screen_mic_height", &lv.render());
            assert_targets_ok(root);
            for (row, y) in screen.rows.iter().zip(ROW_Y) {
                assert_eq!(rect(row.obj), Rect::new(116, y, 168, 44));
            }
            assert!(!screen.rows[1].mark.is_hidden(), "Medium is checked");
            assert!(screen.rows[0].mark.is_hidden() && screen.rows[2].mark.is_hidden());
            assert_eq!(screen.hint.unwrap().text(), HINT);
        });
    }

    #[test]
    fn a_tap_sends_the_height_and_shows_it_before_settings_report_it() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let snap = mockup();
            let mut screen = MicHeightScreen::default();
            screen.build(root, &f.ctx, &snap);
            lv.click(screen.rows[0].obj);
            assert_eq!(f.commands(), [Command::SetMotorPosition(MotorPosition::High)]);
            screen.update(&snap); // settings still say Medium
            assert!(!screen.rows[0].mark.is_hidden(), "High is checked at once");
            assert!(screen.rows[1].mark.is_hidden());
        });
    }

    #[test]
    fn while_the_lift_moves_the_chosen_row_says_moving_and_no_row_takes_a_tap() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = mockup();
            snap.settings.mic_height = MotorPosition::High;
            snap.mcu.lift = LiftState::Moving;
            let mut screen = MicHeightScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_mic_height_moving", &lv.render());
            assert!(!screen.rows[0].moving.is_hidden());
            assert!(screen.rows[0].mark.is_hidden(), "no check while moving");
            assert!(screen.rows.iter().all(|row| !row.obj.is_clickable()));
            lv.click(screen.rows[2].obj);
            lv.click(screen.rows[0].obj);
            assert!(f.commands().is_empty());
            // Arrived: the check is back and the rows take taps again.
            snap.mcu.lift = LiftState::Idle;
            screen.update(&snap);
            assert!(screen.rows[0].moving.is_hidden() && !screen.rows[0].mark.is_hidden());
            assert_eq!(screen.rows[0].mark.text(), icons::CHECK);
            assert_targets_ok(root);
            lv.click(screen.rows[2].obj);
            assert_eq!(f.commands(), [Command::SetMotorPosition(MotorPosition::Low)]);
        });
    }

    #[test]
    fn a_blocked_lift_says_what_to_do_and_a_tap_tries_again() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = mockup();
            snap.settings.mic_height = MotorPosition::High;
            snap.mcu.lift = LiftState::Blocked;
            let mut screen = MicHeightScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_mic_height_blocked", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.hint.unwrap().text(), BLOCKED_HINT);
            assert_eq!(screen.rows[0].mark.text(), icons::ALERT);
            lv.click(screen.rows[0].obj);
            assert_eq!(f.commands(), [Command::SetMotorPosition(MotorPosition::High)]);
            snap.mcu.lift = LiftState::Obstructed;
            screen.update(&snap);
            assert_eq!(screen.hint.unwrap().text(), BLOCKED_HINT, "an obstruction reads the same");
            snap.mcu.lift = LiftState::Moving;
            screen.update(&snap);
            assert_eq!(screen.hint.unwrap().text(), HINT);
        });
    }

    #[test]
    fn copy_is_plain_and_fits() {
        assert_eq!(HEIGHTS.map(height_name), ["High", "Medium", "Low"]);
        lvgl(|_| {
            // The hint row is about 295 px wide at y 222 (the circle's chord).
            for text in [HINT, BLOCKED_HINT] {
                assert!(text_width(text, Font::Small) <= 280, "{text}");
            }
            // "Moving" fits beside the longest height name in a 168 px row.
            let inner = ROW_W - 2 * (ROW_PAD + 2) - 8;
            assert!(text_width("Medium", Font::Body) + text_width(MOVING, Font::Small) < inner);
        });
    }
}
