//! USB storage (G1, docs/mockups/png/usb.png): takes over the screen while
//! the computer has the recordings volume (`Router::sync`). The way out is
//! the only action: "Stop USB storage" opens the confirmation, which sends
//! `StopUsbStorage`. The computer's own eject or a pulled cable ends it too.
//! Audio keeps working (`docs/USB_STORAGE.md`): only recording is off, so the
//! mockup's "Audio is off until you stop" became "Recording is off".

use crate::kit::components::buttons::text_button;
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::{ConfirmKind, Route};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{BOTTOM_PILL_WIDE, LIVE, SCREEN_W, TEXT, TEXT_2};
use waved_state::UsbStoragePhase;

/// usb.png, framebuffer pixels: a 3 px Live ring of r 42 around the icon.
const RING_CX: i32 = 200;
const RING_CY: i32 = 86;
const RING_R: i32 = 42;
const RING_W: i32 = 3;
const TITLE_Y: i32 = 138;
const BODY_X: i32 = 70;
const BODY_Y: i32 = 164;
const BODY_W: i32 = 260;

pub const STOP: &str = "Stop USB storage";

/// What the screen says in a phase, and whether Stop shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub title: &'static str,
    pub body: &'static str,
    pub stop: bool,
}

pub fn look(phase: UsbStoragePhase) -> Look {
    match phase {
        UsbStoragePhase::Starting => Look {
            title: "Starting USB storage",
            body: "Wave is handing the recordings to your computer.",
            stop: false,
        },
        UsbStoragePhase::On => Look {
            title: "USB storage is on",
            body: "Your computer can open the recordings. Recording is off until you stop.",
            stop: true,
        },
        // Off shows for at most a frame, until the router leaves.
        UsbStoragePhase::Stopping | UsbStoragePhase::Off => Look {
            title: "Stopping USB storage",
            body: "Wave is checking the recordings before it uses them again.",
            stop: false,
        },
    }
}

struct Parts {
    title: Obj,
    body: Obj,
    stop: Obj,
}

#[derive(Default)]
pub struct UsbStorageScreen {
    status: Option<StatusArea>,
    parts: Option<Parts>,
    shown: Option<Look>,
}

impl Screen for UsbStorageScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        let size = 2 * RING_R + RING_W;
        Obj::plain(root)
            .pos(RING_CX - size / 2, RING_CY - size / 2)
            .size(size, size)
            .round()
            .border(RING_W, LIVE);
        Obj::label(root, icons::USB, Font::Icon44, LIVE)
            .centered_text(SCREEN_W)
            .pos(0, RING_CY - Font::Icon44.line_height() / 2);
        let title = Obj::label(root, "", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        let body = Obj::label(root, "", Font::Small, TEXT_2)
            .wrapped_text(BODY_W)
            .max_lines(2)
            .pos(BODY_X, BODY_Y);
        let dispatch = ctx.dispatch.clone();
        let stop = text_button(root, BOTTOM_PILL_WIDE, STOP, move || {
            dispatch.navigate(Route::Confirm(ConfirmKind::StopUsbStorage))
        });
        self.parts = Some(Parts { title, body, stop });
        self.shown = None;
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let next = look(snap.usb_storage.phase);
        if self.shown == Some(next) {
            return;
        }
        let Some(parts) = self.parts.as_ref() else {
            return;
        };
        parts.title.set_text(next.title);
        parts.body.set_text(next.body);
        parts.stop.hidden(!next.stop);
        self.shown = Some(next);
    }
}

#[cfg(test)]
impl UsbStorageScreen {
    fn stop_button(&self) -> Obj {
        self.parts.as_ref().expect("built").stop
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::runtime::testing::shows_text;
    use crate::screens_v2::main_parts::fixtures::{ctx, events, sent, snap};

    fn on() -> UiSnapshot {
        let mut snap = snap();
        snap.clock = "14:38".into();
        snap.wifi.connected = false; // as in usb.png
        snap.usb_storage.phase = UsbStoragePhase::On;
        snap
    }

    #[test]
    fn usb_storage_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut screen = UsbStorageScreen::default();
            screen.build(root, &ctx, &on());
            assert_golden("usb_storage", &lv.render());
            assert_targets_ok(root);
            for text in ["USB storage is on", STOP] {
                assert!(shows_text(root, text), "{text}");
            }
        });
    }

    #[test]
    fn stop_asks_first_and_sends_nothing_itself() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut screen = UsbStorageScreen::default();
            screen.build(root, &ctx, &on());
            lv.click(screen.stop_button());
            assert!(sent(&mut bus).is_empty());
            assert_eq!(
                events(&ui),
                vec![UiEvent::Navigate(Route::Confirm(ConfirmKind::StopUsbStorage))]
            );
        });
    }

    #[test]
    fn handing_over_and_taking_back_show_no_button() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = on();
            snap.usb_storage.phase = UsbStoragePhase::Starting;
            let mut screen = UsbStorageScreen::default();
            screen.build(root, &ctx, &snap);
            assert!(shows_text(root, "Starting USB storage"));
            assert!(screen.stop_button().is_hidden());
            snap.usb_storage.phase = UsbStoragePhase::On;
            screen.update(&snap);
            assert!(shows_text(root, "USB storage is on"));
            assert!(!screen.stop_button().is_hidden());
            snap.usb_storage.phase = UsbStoragePhase::Stopping;
            screen.update(&snap);
            assert!(shows_text(root, "Stopping USB storage"));
            assert!(screen.stop_button().is_hidden());
            assert_golden("usb_storage_stopping", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_copy_fits_two_lines() {
        lvgl(|lv| {
            let root = lv.screen();
            for phase in [
                UsbStoragePhase::Starting,
                UsbStoragePhase::On,
                UsbStoragePhase::Stopping,
            ] {
                let body = look(phase).body;
                let label = Obj::label(root, body, Font::Small, TEXT_2).wrapped_text(BODY_W);
                root.update_layout();
                let area = label.coords();
                let lines = (area.y2 - area.y1 + 1) / Font::Small.line_height();
                assert!(lines <= 2, "{body:?} needs {lines} lines");
                label.delete();
            }
            let _ = lv;
        });
    }
}
