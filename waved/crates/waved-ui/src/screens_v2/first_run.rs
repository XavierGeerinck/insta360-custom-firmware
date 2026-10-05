//! First start (A5, docs/mockups/png/first-run.png): shown once, until Wi-Fi
//! is set up or skipped. Skipping is a full answer: Wave is a speakerphone
//! and local recorder without any network.

use crate::kit::components::buttons::{text_button, BottomSpot, RoundButton, Tone};
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, SCREEN_W, TEXT, TEXT_2};
use std::cell::Cell;
use std::rc::Rc;
use waved_state::Command;

/// first-run.png: the dial-size title (CSS top 40), the body (x 65, w 270,
/// top 78) and the "Use without Wi-Fi" content pill.
const SETUP_TITLE_Y: i32 = 40;
const BODY_X: i32 = 65;
const BODY_Y: i32 = 78;
const BODY_W: i32 = 270;
const SKIP: Rect = Rect::new(112, 146, 176, 44);
const BODY: &str =
    "Wave works as a USB speakerphone right away. Connect Wi-Fi to send recordings to your own server.";

#[derive(Default)]
pub struct FirstRunScreen {
    parts: Option<Parts>,
}

#[cfg_attr(not(test), allow(dead_code))] // `skip` and `setup` are read only by tests
struct Parts {
    status: StatusArea,
    skip: Obj,
    setup: RoundButton,
}

impl Screen for FirstRunScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        let status = StatusArea::plain(root);
        Obj::label(root, "Set up Wave", Font::Dial, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, SETUP_TITLE_Y);
        Obj::label(root, BODY, Font::Body, TEXT_2)
            .wrapped_text(BODY_W)
            .max_lines(3)
            .pos(BODY_X, BODY_Y);
        let done = Rc::new(Cell::new(false));
        let skip = {
            let (done, dispatch) = (done.clone(), ctx.dispatch.clone());
            text_button(root, SKIP, "Use without Wi-Fi", move || {
                if !done.replace(true) {
                    dispatch.command(Command::MarkFirstRunDone);
                }
            })
        };
        // Skipping is the quieter choice: grey words.
        skip.child(0).text_color(TEXT_2);
        let setup = {
            let dispatch = ctx.dispatch.clone();
            RoundButton::new(root, BottomSpot::Main, icons::WIFI, "Set up Wi-Fi", Tone::Live, move || {
                if !done.replace(true) {
                    dispatch.command(Command::MarkFirstRunDone);
                    dispatch.navigate(Route::WifiSetup);
                }
            })
        };
        self.parts = Some(Parts { status, skip, setup });
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(p) = self.parts.as_mut() {
            p.status.update(snap);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::main_parts::fixtures::{ctx, events, sent, snap};

    /// first-run.png: nothing connected yet, 09:02.
    fn fresh() -> UiSnapshot {
        let mut snap = snap();
        snap.mcu.usb_connected = false;
        snap.wifi.connected = false;
        snap.clock = "09:02".into();
        snap.settings.first_run_done = false;
        snap
    }

    #[test]
    fn first_run_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            FirstRunScreen::default().build(root, &ctx, &fresh());
            assert_golden("first_run", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn skipping_marks_first_start_done_once() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut screen = FirstRunScreen::default();
            screen.build(root, &ctx, &fresh());
            let skip = screen.parts.as_ref().unwrap().skip;
            lv.click(skip);
            lv.click(skip);
            assert_eq!(sent(&mut bus), ["MarkFirstRunDone"]);
            assert!(events(&ui).is_empty(), "the router moves on by itself");
        });
    }

    #[test]
    fn set_up_wifi_marks_done_and_opens_wifi_setup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut screen = FirstRunScreen::default();
            screen.build(root, &ctx, &fresh());
            let (setup, skip) = {
                let p = screen.parts.as_ref().unwrap();
                (p.setup.obj(), p.skip)
            };
            lv.click(setup);
            lv.click(skip);
            assert_eq!(sent(&mut bus), ["MarkFirstRunDone"]);
            assert_eq!(events(&ui), [UiEvent::Navigate(Route::WifiSetup)]);
        });
    }
}
