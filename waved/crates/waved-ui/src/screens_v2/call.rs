//! Incoming call (I3, call.png): a phone call through Bluetooth. The number
//! only, never a contact name (no phone book, by design), with Decline and
//! Answer. Once the call is up the same screen says "In a call" with Hang up,
//! as stock's calling page does; Back (a swipe right) shows Home, which says
//! "In a call" too. The router shows this screen while a call rings
//! (`kit::router`).

use crate::kit::components::bluetooth::CallRings;
use crate::kit::components::buttons::{BottomSpot, RoundButton, Tone};
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{text_width, Obj};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, LIVE, SCREEN_H, SCREEN_W, TEXT, TEXT_2};
use waved_state::{BluetoothState, BtCall, Command};

/// call.png: the rings around (200, 118), then the text stack inside.
const RINGS: (i32, i32) = (200, 118);
const CAPTION_Y: i32 = 86;
const NUMBER_Y: i32 = 108;
const VIA_Y: i32 = 134;
/// Inside the r 76 ring (stroke 3) with a little air.
const NUMBER_W: i32 = 144;
const VIA_W: i32 = 130;

pub const INCOMING: &str = "Incoming call";
pub const IN_A_CALL: &str = "In a call";
pub const CALL_ENDED: &str = "Call ended";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Ringing,
    Active,
    /// The call ended; the router takes the screen away.
    Ended,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallView {
    pub phase: Phase,
    pub number: String,
    /// "via iPhone": the phone's own Bluetooth name; empty hides the line.
    pub via: String,
}

pub fn call_view(bt: &BluetoothState) -> CallView {
    let phase = match bt.call {
        BtCall::Idle => Phase::Ended,
        BtCall::Ringing { .. } => Phase::Ringing,
        BtCall::Active { .. } => Phase::Active,
    };
    CallView {
        phase,
        number: bt.call.number().unwrap_or_default().to_string(),
        via: if bt.device_name.is_empty() {
            String::new()
        } else {
            format!("via {}", bt.device_name)
        },
    }
}

/// The number in the 20 px digits when it fits inside the ring, else smaller.
fn number_font(number: &str) -> Font {
    if text_width(number, Font::Num20) <= NUMBER_W {
        Font::Num20
    } else {
        Font::Body
    }
}

struct Parts {
    rings: CallRings,
    caption: Obj,
    number: Obj,
    via: Obj,
    /// Full-screen layers without a layout (round buttons need one): Decline
    /// and Answer while ringing, Hang up once the call is up.
    ringing: Obj,
    active: Obj,
    decline: RoundButton,
    answer: RoundButton,
    hang_up: RoundButton,
}

fn layer(root: Obj) -> Obj {
    Obj::plain(root).rect(Rect::new(0, 0, SCREEN_W, SCREEN_H))
}

#[derive(Default)]
pub struct CallScreen {
    status: Option<StatusArea>,
    parts: Option<Parts>,
    shown: Option<CallView>,
}

impl Screen for CallScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        let rings = CallRings::new(root, RINGS.0, RINGS.1);
        let line = |font, hex, width, y| {
            Obj::label(root, "", font, hex)
                .centered_text(width)
                .pos((SCREEN_W - width) / 2, y)
        };
        let caption = line(Font::Small, LIVE, NUMBER_W, CAPTION_Y);
        let number = line(Font::Num20, TEXT, NUMBER_W, NUMBER_Y);
        let via = line(Font::Small, TEXT_2, VIA_W, VIA_Y);
        let send = |command: Command| {
            let dispatch = ctx.dispatch.clone();
            move || dispatch.command(command.clone())
        };
        let (ringing, active) = (layer(root), layer(root));
        let decline = RoundButton::new(
            ringing,
            BottomSpot::PairLeft,
            icons::PHONE_OFF,
            "Decline",
            Tone::RecText,
            send(Command::BtDeclineCall),
        );
        let answer = RoundButton::new(
            ringing,
            BottomSpot::PairRight,
            icons::PHONE,
            "Answer",
            Tone::Live,
            send(Command::BtAnswerCall),
        );
        let hang_up = RoundButton::new(
            active,
            BottomSpot::Main,
            icons::PHONE_OFF,
            "Hang up",
            Tone::RecText,
            send(Command::BtDeclineCall),
        );
        self.parts = Some(Parts {
            rings,
            caption,
            number,
            via,
            ringing,
            active,
            decline,
            answer,
            hang_up,
        });
        self.update(snap);
        root.update_layout();
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let next = call_view(&snap.bt);
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        let Some(parts) = self.parts.as_ref() else {
            return;
        };
        let (caption, ringing, active) = match next.phase {
            Phase::Ringing => (INCOMING, true, false),
            Phase::Active => (IN_A_CALL, false, true),
            Phase::Ended => (CALL_ENDED, false, false),
        };
        parts.rings.set_ripples(ringing);
        parts.caption.set_text(caption);
        // Ended keeps the last number for the frame before the screen goes.
        if next.phase != Phase::Ended {
            parts
                .number
                .font(number_font(&next.number))
                .one_line()
                .set_text(&next.number);
        }
        parts.via.hidden(next.via.is_empty()).set_text(&next.via);
        parts.ringing.hidden(!ringing);
        parts.active.hidden(!active);
        self.shown = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::runtime::testing::shows_text;
    use crate::screens_v2::menu::fixture::{fixture, snap};

    const NUMBER: &str = "+32470123456";

    /// call.png's status: Bluetooth, 14:36, 82 % (no USB, no Wi-Fi).
    fn ringing() -> UiSnapshot {
        let mut snap = snap();
        snap.mcu.usb_connected = false;
        snap.wifi = Default::default();
        snap.clock = "14:36".into();
        snap.bt.connected = true;
        snap.bt.device_name = "iPhone".into();
        snap.bt.call = BtCall::Ringing { number: NUMBER.into() };
        snap
    }

    fn active() -> UiSnapshot {
        let mut snap = ringing();
        snap.bt.call = BtCall::Active { number: NUMBER.into() };
        snap
    }

    #[test]
    fn the_view_shows_the_number_only() {
        let snap = ringing();
        assert_eq!(
            call_view(&snap.bt),
            CallView { phase: Phase::Ringing, number: NUMBER.into(), via: "via iPhone".into() }
        );
        let mut bt = snap.bt.clone();
        bt.device_name.clear();
        assert_eq!(call_view(&bt).via, "");
        bt.call = BtCall::Idle;
        assert_eq!(call_view(&bt).phase, Phase::Ended);
    }

    #[test]
    fn incoming_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = CallScreen::default();
            screen.build(root, &f.ctx, &ringing());
            assert_golden("screen_call", &lv.render());
            assert_targets_ok(root);
            assert!(shows_text(root, INCOMING));
            assert!(shows_text(root, NUMBER));
            assert!(shows_text(root, "via iPhone"));
            let parts = screen.parts.as_ref().unwrap();
            lv.click(parts.answer.obj());
            lv.click(parts.decline.obj());
            assert_eq!(f.commands(), vec![Command::BtAnswerCall, Command::BtDeclineCall]);
            assert!(f.events().is_empty(), "the router decides when the screen goes");
        });
    }

    #[test]
    fn an_answered_call_offers_hang_up() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = CallScreen::default();
            screen.build(root, &f.ctx, &ringing());
            screen.update(&active());
            assert_golden("screen_call_active", &lv.render());
            assert_targets_ok(root);
            assert!(shows_text(root, IN_A_CALL));
            let parts = screen.parts.as_ref().unwrap();
            assert!(parts.ringing.is_hidden() && !parts.active.is_hidden());
            assert!(!parts.rings.ripples_visible());
            lv.click(parts.hang_up.obj());
            assert_eq!(f.commands(), vec![Command::BtDeclineCall]);
        });
    }

    #[test]
    fn an_ended_call_offers_nothing() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = CallScreen::default();
            screen.build(root, &f.ctx, &ringing());
            let mut ended = ringing();
            ended.bt.call = BtCall::Idle;
            screen.update(&ended);
            assert!(shows_text(root, CALL_ENDED));
            assert!(shows_text(root, NUMBER), "the last number stays for the last frame");
            let parts = screen.parts.as_ref().unwrap();
            assert!(parts.ringing.is_hidden() && parts.active.is_hidden());
        });
    }

    #[test]
    fn a_long_number_fits_inside_the_ring() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = ringing();
            snap.bt.call = BtCall::Ringing { number: "+1 (555) 010-9999 ext. 12345".into() };
            snap.bt.device_name = "Conference room phone on the third floor".into();
            let mut screen = CallScreen::default();
            screen.build(root, &f.ctx, &snap);
            root.update_layout();
            assert_targets_ok(root);
            let parts = screen.parts.as_ref().unwrap();
            for label in [parts.number, parts.via] {
                let a = label.coords();
                assert!(a.x1 >= 200 - 76 && a.x2 <= 200 + 76, "{}", label.text());
            }
            assert!(parts.via.text().ends_with("..."));
        });
    }
}
