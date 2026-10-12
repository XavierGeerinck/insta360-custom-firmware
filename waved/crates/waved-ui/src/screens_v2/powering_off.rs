//! Powering off (device pass 2026-10-12): after a 2 s hold, or System →
//! Power off, the MCU retracts the lift and cuts power, which takes a few
//! seconds. This screen stays up until then: the full ring and the words,
//! nothing to tap (`Route::PoweringOff` is final in the router).

use crate::kit::components::dial::Dial;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{DIAL_CX, DIAL_CY, DIAL_R, SCREEN_W, TEXT};
use crate::screens_v2::messages::copy;

/// Same place as "Starting" on the boot screen.
const TITLE_Y: i32 = 99;

pub struct PoweringOffScreen;

impl Screen for PoweringOffScreen {
    fn build(&mut self, root: Obj, _ctx: &ScreenCtx, _snap: &UiSnapshot) {
        let mut dial = Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3);
        dial.set_progress(Some(1.0));
        Obj::label(root, copy::POWERING_OFF, Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
    }

    fn update(&mut self, _snap: &UiSnapshot) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::main_parts::fixtures::{ctx, snap};

    #[test]
    fn powering_off_shows_the_full_ring_and_nothing_to_tap() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut screen = PoweringOffScreen;
            screen.build(root, &ctx, &snap());
            assert_golden("powering_off", &lv.render());
            assert_targets_ok(root);
        });
    }
}
