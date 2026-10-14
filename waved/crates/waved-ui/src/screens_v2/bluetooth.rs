//! Bluetooth (I4, bluetooth.png): the switch, the connected phone with
//! Disconnect and "Pair another phone", or "Pair a device", which makes Wave
//! visible for two minutes with a countdown ring and the name to pick on the
//! phone. Leaving the screen ends the window (`BtStopPairing`, the runtime's
//! leave commands). Behind the PIN (D10, the router).
//!
//! The switch is stock's Bluetooth switch (`SetBluetooth`: `OpenBt` /
//! `CloseBt`, persisted). It sits at the end of the device row, the only
//! target there, so a tap on the phone's name does nothing; it moves at once
//! and stays until waved-bt reports the change (`Pending`). Off, the screen
//! says so and offers nothing else.
//!
//! Disconnect turns the module's reconnect off first, so the phone stays
//! dropped until it connects again from its own Bluetooth settings. Pair
//! another phone is stock's "Connect New Device": the same pairing, which
//! drops the connected phone first, so it asks first as stock does
//! (`ConfirmKind::PairAnotherPhone`).
//!
//! The mockup's paired list is left out: the module keeps its pairings itself
//! and has no command to list, connect to or forget a phone
//! (docs/BLUETOOTH_PROTOCOL.md). Phones cannot pair classic Bluetooth from a
//! QR code, so pairing shows the module's name instead.

use crate::kit::components::bluetooth::{icon_button, DeviceLook, DeviceRow};
use crate::kit::components::buttons::{back_button, text_button};
use crate::kit::components::controls::Switch;
use crate::kit::components::dial::Dial;
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{text_width, Obj};
use crate::kit::route::{ConfirmKind, Route};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    Rect, LIVE, SCREEN_H, SCREEN_W, SUBTITLE_Y, TEXT, TEXT_2, TEXT_3, TITLE_Y,
};
use crate::screens_v2::menu::Pending;
use std::rc::Rc;
use waved_state::{BluetoothState, Command};

/// bluetooth.png: the first device row and the content pill.
const ROW: Rect = Rect::new(60, 82, 280, 48);
const ACTION: Rect = Rect::new(108, 192, 184, 44);
/// Connected: Disconnect in the mockup's second row, between the phone and
/// the pairing pill (which keeps its place on every view).
const DISCONNECT_PILL: Rect = Rect::new(108, 140, 184, 44);
/// The line under the row while no phone is connected.
const HINT_Y: i32 = 142;
const HINT_W: i32 = 280;
/// Pairing: the countdown ring and its time, the name, the instruction.
const RING: (i32, i32, i32) = (200, 104, 30);
const TIME_Y: i32 = 92;
const NAME_Y: i32 = 146;
const NAME_W: i32 = 340;
const STEP_Y: i32 = 182;
const STEP_W: i32 = 280;

/// The window `BtPair` opens (waved-bt `PAIRING_WINDOW_MS`).
pub const PAIRING_WINDOW_S: u16 = 120;
/// What the module calls itself before it said so (its default name lacks
/// the six hex digits of the serial).
pub const NAME_FALLBACK: &str = "Insta360 Wave BT";
pub const PAIR: &str = "Pair a device";
pub const PAIR_ANOTHER: &str = "Pair another phone";
pub const DISCONNECT: &str = "Disconnect";
pub const NO_PHONE: &str = "No phone connected";
pub const OFF: &str = "Bluetooth off";
/// After Disconnect the module no longer reconnects by itself; the phone can.
pub const RECONNECTS: &str = "Reconnect a phone from its Bluetooth settings";
pub const OFF_HINT: &str = "Phones can't find Wave or connect to it";
pub const PICK_NAME: &str = "Pick this name in your phone's Bluetooth list";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    /// The switch is off.
    Off,
    /// No phone, not pairing.
    Idle { shows_as: String },
    /// Wave is visible for `left_s` more seconds.
    Pairing { name: String, left_s: u16 },
    Connected { shows_as: String, phone: String },
}

/// "1:42".
pub fn countdown(seconds: u16) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn module_name(bt: &BluetoothState) -> String {
    if bt.advertised_name.is_empty() {
        NAME_FALLBACK.into()
    } else {
        bt.advertised_name.clone()
    }
}

/// What shows, with the switch `on` (the user's choice until waved-bt
/// reports it, else `bt.bt_on`).
pub fn view(bt: &BluetoothState, on: bool) -> View {
    let shows_as = format!("Shows up as {}", module_name(bt));
    if !on {
        View::Off
    } else if bt.connected {
        View::Connected {
            shows_as,
            phone: if bt.device_name.is_empty() {
                "Phone".into()
            } else {
                bt.device_name.clone()
            },
        }
    } else if bt.pairing_left_s > 0 {
        View::Pairing {
            name: module_name(bt),
            left_s: bt.pairing_left_s,
        }
    } else {
        View::Idle { shows_as }
    }
}

/// The big name in the Dial font when it fits, else the Title font.
fn name_font(name: &str) -> Font {
    if text_width(name, Font::Dial) <= NAME_W {
        Font::Dial
    } else {
        Font::Title
    }
}

/// A full-screen container without a layout, so the kit's pieces can sit in it.
fn layer(root: Obj) -> Obj {
    Obj::plain(root).rect(Rect::new(0, 0, SCREEN_W, SCREEN_H))
}

fn centered(parent: Obj, font: Font, hex: u32, width: i32, y: i32) -> Obj {
    Obj::label(parent, "", font, hex)
        .centered_text(width)
        .pos((SCREEN_W - width) / 2, y)
}

struct Parts {
    subtitle: Obj,
    /// Not connected and connected: the row and its actions.
    list: Obj,
    row: DeviceRow,
    switch: Switch,
    hint: Obj,
    pair: Obj,
    pair_another: Obj,
    disconnect: Obj,
    /// Pairing: the countdown, the name and the instruction.
    pairing: Obj,
    dial: Dial,
    time: Obj,
    name: Obj,
}

#[derive(Default)]
pub struct BluetoothScreen {
    status: Option<StatusArea>,
    parts: Option<Parts>,
    shown: Option<View>,
    /// The switch: a tap shows at once, until waved-bt reports it.
    on: Rc<Pending<bool>>,
}

impl Screen for BluetoothScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Bluetooth", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        let subtitle = centered(root, Font::Micro, TEXT_3, 300, SUBTITLE_Y);

        let list = layer(root);
        let row = DeviceRow::new(list, ROW);
        let (dispatch, pending) = (ctx.dispatch.clone(), self.on.clone());
        let switch = Switch::new(row.obj(), snap.bt.bt_on, move |on| {
            pending.set(on);
            dispatch.command(Command::SetBluetooth(on))
        });
        let hint = Obj::label(list, RECONNECTS, Font::Small, TEXT_3)
            .wrapped_text(HINT_W)
            .max_lines(2)
            .pos((SCREEN_W - HINT_W) / 2, HINT_Y);
        let dispatch = ctx.dispatch.clone();
        let pair = icon_button(list, ACTION, icons::BLUETOOTH, PAIR, move || {
            dispatch.command(Command::BtPair)
        });
        // Stock asks before Connect New Device drops the phone.
        let dispatch = ctx.dispatch.clone();
        let pair_another = icon_button(list, ACTION, icons::BLUETOOTH, PAIR_ANOTHER, move || {
            dispatch.navigate(Route::Confirm(ConfirmKind::PairAnotherPhone))
        });
        let dispatch = ctx.dispatch.clone();
        let disconnect = text_button(list, DISCONNECT_PILL, DISCONNECT, move || {
            dispatch.command(Command::BtDisconnect)
        });

        let pairing = layer(root);
        let (cx, cy, r) = RING;
        let dial = Dial::new(pairing, cx, cy, r, 3);
        let time = centered(pairing, Font::Num20, TEXT, 2 * r, TIME_Y);
        let name = centered(pairing, Font::Dial, TEXT, NAME_W, NAME_Y);
        Obj::label(pairing, PICK_NAME, Font::Small, TEXT_2)
            .wrapped_text(STEP_W)
            .max_lines(2)
            .pos((SCREEN_W - STEP_W) / 2, STEP_Y);

        back_button(root, &ctx.dispatch);
        self.parts = Some(Parts {
            subtitle,
            list,
            row,
            switch,
            hint,
            pair,
            pair_another,
            disconnect,
            pairing,
            dial,
            time,
            name,
        });
        self.update(snap);
        root.update_layout();
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let on = self.on.follow(snap.bt.bt_on);
        let next = view(&snap.bt, on);
        let Some(parts) = self.parts.as_mut() else {
            return;
        };
        if parts.switch.is_on() != on {
            parts.switch.set_on(on);
        }
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        let pairing = matches!(next, View::Pairing { .. });
        parts.list.hidden(pairing);
        parts.pairing.hidden(!pairing);
        match &next {
            View::Off => {
                parts.subtitle.hidden(true);
                parts.row.set(&DeviceLook {
                    icon: icons::BLUETOOTH,
                    name: OFF.into(),
                    name_color: TEXT_2,
                    value: String::new(),
                    value_color: TEXT_3,
                });
                parts.hint.hidden(false).set_text(OFF_HINT);
                parts.pair.hidden(true);
                parts.pair_another.hidden(true);
                parts.disconnect.hidden(true);
            }
            View::Idle { shows_as } => {
                parts.subtitle.hidden(false).set_text(shows_as);
                parts.row.set(&DeviceLook {
                    icon: icons::MOBILE,
                    name: NO_PHONE.into(),
                    name_color: TEXT_2,
                    value: String::new(),
                    value_color: TEXT_3,
                });
                parts.hint.hidden(false).set_text(RECONNECTS);
                parts.pair.hidden(false);
                parts.pair_another.hidden(true);
                parts.disconnect.hidden(true);
            }
            View::Connected { shows_as, phone } => {
                parts.subtitle.hidden(false).set_text(shows_as);
                parts.row.set(&DeviceLook {
                    icon: icons::MOBILE,
                    name: phone.clone(),
                    name_color: TEXT,
                    value: "Connected".into(),
                    value_color: LIVE,
                });
                parts.hint.hidden(true);
                parts.pair.hidden(true);
                parts.pair_another.hidden(false);
                parts.disconnect.hidden(false);
            }
            View::Pairing { name, left_s } => {
                parts.subtitle.hidden(true);
                parts
                    .dial
                    .set_progress(Some(f32::from(*left_s) / f32::from(PAIRING_WINDOW_S)));
                parts.time.set_text(&countdown(*left_s));
                parts
                    .name
                    .font(name_font(name))
                    .one_line()
                    .set_text(name);
            }
        }
        self.shown = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::runtime::testing::shows_text;
    use crate::screens_v2::menu::fixture::{fixture, snap};

    const MODULE: &str = "Insta360 Wave BT 0A1B2C";

    /// The status cluster as bluetooth.png draws it: USB, Bluetooth, 14:36, 82 %.
    fn bt_snap() -> UiSnapshot {
        let mut snap = snap();
        snap.wifi = Default::default();
        snap.clock = "14:36".into();
        snap.bt.advertised_name = MODULE.into();
        snap
    }

    fn connected() -> UiSnapshot {
        let mut snap = bt_snap();
        snap.bt.connected = true;
        snap.bt.device_name = "iPhone".into();
        snap
    }

    fn pairing(left_s: u16) -> UiSnapshot {
        let mut snap = bt_snap();
        snap.bt.pairing_left_s = left_s;
        snap.bt.pairing = true;
        snap
    }

    fn off() -> UiSnapshot {
        let mut snap = bt_snap();
        snap.bt.bt_on = false;
        snap
    }

    #[test]
    fn the_view_follows_the_state() {
        let mut bt = BluetoothState::default();
        assert_eq!(view(&bt, true), View::Idle { shows_as: format!("Shows up as {NAME_FALLBACK}") });
        bt.advertised_name = MODULE.into();
        bt.pairing_left_s = 102;
        assert_eq!(view(&bt, true), View::Pairing { name: MODULE.into(), left_s: 102 });
        bt.connected = true;
        bt.device_name = "Pixel".into();
        assert_eq!(
            view(&bt, true),
            View::Connected { shows_as: format!("Shows up as {MODULE}"), phone: "Pixel".into() },
            "a connected phone wins"
        );
        bt.device_name.clear();
        assert!(matches!(view(&bt, true), View::Connected { phone, .. } if phone == "Phone"));
        assert_eq!(view(&bt, false), View::Off, "the switch wins over everything");
        assert_eq!(countdown(120), "2:00");
        assert_eq!(countdown(102), "1:42");
        assert_eq!(countdown(9), "0:09");
    }

    #[test]
    fn connected_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = BluetoothScreen::default();
            screen.build(root, &f.ctx, &connected());
            assert_golden("screen_bluetooth", &lv.render());
            assert_targets_ok(root);
            assert!(shows_text(root, "iPhone"));
            assert!(shows_text(root, &format!("Shows up as {MODULE}")));
            assert!(shows_text(root, DISCONNECT));
            assert!(shows_text(root, PAIR_ANOTHER));
            assert!(!shows_text(root, PAIR), "one pairing action at a time");
            let parts = screen.parts.as_ref().unwrap();
            assert!(parts.switch.is_on());
            lv.click(parts.disconnect);
            assert_eq!(f.commands(), vec![Command::BtDisconnect]);
            // Stock's "Connect New Device" asks first: it drops the phone.
            lv.click(parts.pair_another);
            assert!(f.commands().is_empty(), "nothing before the confirmation");
            assert_eq!(
                f.events(),
                vec![UiEvent::Navigate(Route::Confirm(ConfirmKind::PairAnotherPhone))]
            );
            // The phone's row itself is no target: only the switch is.
            lv.click(parts.row.obj());
            assert!(f.commands().is_empty());
        });
    }

    #[test]
    fn the_pairing_actions_fit_their_pills() {
        lvgl(|_| {
            // The pill's icon (16 px), its 6 px gap and 12 px each side.
            for text in [PAIR, PAIR_ANOTHER] {
                let needs = text_width(text, Font::Body) + 16 + 6 + 2 * 12;
                assert!(needs <= ACTION.w, "{text:?} needs {needs} of {}", ACTION.w);
            }
            let needs = text_width(DISCONNECT, Font::Body) + 2 * 12;
            assert!(needs <= DISCONNECT_PILL.w);
            assert!(DISCONNECT_PILL.y >= ROW.y + ROW.h + 8, "below the phone");
            assert!(DISCONNECT_PILL.y + DISCONNECT_PILL.h + 8 <= ACTION.y, "above pairing");
        });
    }

    #[test]
    fn no_phone_offers_pairing() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = BluetoothScreen::default();
            screen.build(root, &f.ctx, &bt_snap());
            assert_golden("screen_bluetooth_idle", &lv.render());
            assert_targets_ok(root);
            assert!(shows_text(root, NO_PHONE));
            assert!(shows_text(root, RECONNECTS));
            assert!(!shows_text(root, DISCONNECT));
            assert!(!shows_text(root, PAIR_ANOTHER));
            lv.click(screen.parts.as_ref().unwrap().pair);
            assert_eq!(f.commands(), vec![Command::BtPair]);
            assert!(f.events().is_empty(), "pairing stays on this screen");
        });
    }

    #[test]
    fn the_switch_turns_bluetooth_off_at_once() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = BluetoothScreen::default();
            screen.build(root, &f.ctx, &connected());
            let switch = screen.parts.as_ref().unwrap().switch.obj();
            lv.click(switch);
            assert_eq!(f.commands(), vec![Command::SetBluetooth(false)]);
            // Shown at once, even while the snapshot still says on.
            screen.update(&connected());
            assert!(shows_text(root, OFF));
            assert!(shows_text(root, OFF_HINT));
            assert!(!shows_text(root, DISCONNECT) && !shows_text(root, PAIR_ANOTHER));
            assert!(!screen.parts.as_ref().unwrap().switch.is_on());
            // waved-bt reports it: the snapshot is the truth again.
            screen.update(&off());
            assert!(shows_text(root, OFF));
            screen.update(&bt_snap());
            assert!(shows_text(root, NO_PHONE), "switched on elsewhere");
            assert!(screen.parts.as_ref().unwrap().switch.is_on());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn off_offers_only_the_switch() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = BluetoothScreen::default();
            screen.build(root, &f.ctx, &off());
            assert_golden("screen_bluetooth_off", &lv.render());
            assert_targets_ok(root);
            assert!(shows_text(root, OFF));
            assert!(shows_text(root, OFF_HINT));
            for text in [PAIR, PAIR_ANOTHER, DISCONNECT, "Shows up as"] {
                assert!(!shows_text(root, text), "{text}");
            }
            lv.click(screen.parts.as_ref().unwrap().switch.obj());
            assert_eq!(f.commands(), vec![Command::SetBluetooth(true)]);
            screen.update(&off());
            assert!(shows_text(root, PAIR), "on at once");
        });
    }

    #[test]
    fn pairing_shows_the_name_to_pick_and_the_countdown() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = BluetoothScreen::default();
            screen.build(root, &f.ctx, &pairing(102));
            assert_golden("screen_bluetooth_pairing", &lv.render());
            assert_targets_ok(root);
            assert!(shows_text(root, MODULE));
            assert!(shows_text(root, "1:42"));
            assert!(shows_text(root, PICK_NAME));
            assert!(!shows_text(root, PAIR), "no second pairing while pairing");
            assert!(!shows_text(root, PAIR_ANOTHER));
            assert!(!shows_text(root, DISCONNECT));
        });
    }

    #[test]
    fn the_screen_follows_pairing_to_a_connected_phone() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = BluetoothScreen::default();
            screen.build(root, &f.ctx, &bt_snap());
            screen.update(&pairing(120));
            assert!(shows_text(root, "2:00"));
            screen.update(&pairing(119));
            assert!(shows_text(root, "1:59"));
            screen.update(&connected());
            assert!(shows_text(root, "iPhone"));
            assert!(shows_text(root, DISCONNECT));
            assert!(shows_text(root, PAIR_ANOTHER));
            assert!(!shows_text(root, "1:59"));
            screen.update(&bt_snap());
            assert!(shows_text(root, NO_PHONE));
            assert!(!shows_text(root, PAIR_ANOTHER));
            assert_targets_ok(root);
        });
    }

    #[test]
    fn a_long_phone_name_and_no_module_name() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = connected();
            snap.bt.device_name = "Conference room phone on the third floor".into();
            snap.bt.advertised_name.clear();
            let mut screen = BluetoothScreen::default();
            screen.build(root, &f.ctx, &snap);
            root.update_layout();
            assert_targets_ok(root);
            assert!(shows_text(root, &format!("Shows up as {NAME_FALLBACK}")));
            let name = screen.parts.as_ref().unwrap().row.name();
            assert!(name.text().ends_with("..."), "{}", name.text());
            snap.bt.connected = false;
            snap.bt.pairing_left_s = 30;
            screen.update(&snap);
            assert!(shows_text(root, NAME_FALLBACK));
        });
    }
}
