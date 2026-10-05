//! Wi-Fi (B7, wifi.png): the live connection with drawn signal arcs. Opening
//! it never drops the network. "Change network" warns first, because Wave
//! leaves the network while the setup network is open.

use crate::kit::components::buttons::{back_button, text_button};
use crate::kit::components::status::StatusArea;
use crate::kit::components::wifi::SignalArcs;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::route::{ConfirmKind, Route};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, LIVE, SCREEN_W, TEXT, TEXT_2, TEXT_3, TITLE_Y};
use std::cell::Cell;
use std::rc::Rc;
use waved_state::WifiState;

const ARCS: (i32, i32) = (200, 126);
const TEXT_X: i32 = 50;
const TEXT_W: i32 = 300;
const NAME_Y: i32 = 134;
const LINE_Y: i32 = 158;
const ADDRESS_Y: i32 = 174;
const ACTION: Rect = Rect::new(120, 192, 160, 44);

/// Lit signal arcs for a reading (0 while not connected).
pub fn bars_for(rssi: i8, connected: bool) -> u8 {
    match (connected, rssi) {
        (false, _) => 0,
        (true, r) if r >= -55 => 4,
        (true, r) if r >= -67 => 3,
        (true, r) if r >= -75 => 2,
        _ => 1,
    }
}

pub fn signal_word(rssi: i8) -> &'static str {
    match bars_for(rssi, true) {
        4 => "excellent",
        3 => "good",
        2 => "fair",
        _ => "weak",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WifiView {
    pub bars: u8,
    pub name: String,
    pub line: String,
    pub live: bool,
    pub address: Option<String>,
    pub action: &'static str,
}

pub fn wifi_view(wifi: &WifiState) -> WifiView {
    if wifi.connected {
        WifiView {
            bars: bars_for(wifi.signal_strength, true),
            name: if wifi.ssid.is_empty() {
                "Connected".into()
            } else {
                wifi.ssid.clone()
            },
            line: format!("Connected, signal {}", signal_word(wifi.signal_strength)),
            live: true,
            address: (!wifi.ip_addr.is_empty()).then(|| wifi.ip_addr.clone()),
            action: "Change network",
        }
    } else {
        WifiView {
            bars: 0,
            name: "Not connected".into(),
            line: "No Wi-Fi network".into(),
            live: false,
            address: None,
            action: "Set up Wi-Fi",
        }
    }
}

#[derive(Default)]
pub struct WifiScreen {
    status: Option<StatusArea>,
    arcs: Option<SignalArcs>,
    name: Option<Obj>,
    line: Option<Obj>,
    address: Option<Obj>,
    button: Option<Obj>,
    connected: Rc<Cell<bool>>,
    shown: Option<WifiView>,
}

impl Screen for WifiScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Wi-Fi", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        self.arcs = Some(SignalArcs::new(root, ARCS.0, ARCS.1));
        let line = |y, font, hex| {
            Obj::label(root, "", font, hex)
                .centered_text(TEXT_W)
                .pos(TEXT_X, y)
        };
        self.name = Some(line(NAME_Y, Font::Title, TEXT));
        self.line = Some(line(LINE_Y, Font::Small, TEXT_2));
        self.address = Some(line(ADDRESS_Y, Font::Num13, TEXT_3));
        let (dispatch, connected) = (ctx.dispatch.clone(), self.connected.clone());
        self.button = Some(text_button(root, ACTION, "", move || {
            if connected.get() {
                dispatch.navigate(Route::Confirm(ConfirmKind::ChangeNetwork));
            } else {
                dispatch.navigate(Route::WifiSetup);
            }
        }));
        back_button(root, &ctx.dispatch);
        self.update(snap);
        root.update_layout();
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        self.connected.set(snap.wifi.connected);
        let view = wifi_view(&snap.wifi);
        if self.shown.as_ref() == Some(&view) {
            return;
        }
        if let (Some(arcs), Some(name), Some(line), Some(address), Some(button)) = (
            self.arcs.as_ref(),
            self.name,
            self.line,
            self.address,
            self.button,
        ) {
            arcs.set_bars(view.bars);
            name.set_text(&view.name);
            line.text_color(if view.live { LIVE } else { TEXT_2 })
                .set_text(&view.line);
            address
                .hidden(view.address.is_none())
                .set_text(view.address.as_deref().unwrap_or(""));
            button.child(0).set_text(view.action);
        }
        self.shown = Some(view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::route::ConfirmKind;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    #[test]
    fn wifi_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.clock = "14:34".into();
            let mut screen = WifiScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_wifi", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.line.unwrap().text(), "Connected, signal good");
        });
    }

    #[test]
    fn not_connected_offers_setup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.wifi = Default::default();
            let mut screen = WifiScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_wifi_off", &lv.render());
            assert_targets_ok(root);
            lv.click(screen.button.unwrap());
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::WifiSetup)]);
        });
    }

    #[test]
    fn change_network_warns_first() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = WifiScreen::default();
            screen.build(root, &f.ctx, &snap());
            assert_targets_ok(root);
            lv.click(screen.button.unwrap());
            assert!(f.commands().is_empty(), "the station is not touched yet");
            assert_eq!(
                f.events(),
                vec![UiEvent::Navigate(Route::Confirm(ConfirmKind::ChangeNetwork))]
            );
        });
    }

    #[test]
    fn a_long_network_name_ends_in_dots() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            // 32 glyphs, each far wider than the average: certainly wider than 300 px.
            snap.wifi.ssid = "W".repeat(32);
            let mut screen = WifiScreen::default();
            screen.build(root, &f.ctx, &snap);
            root.update_layout();
            assert_targets_ok(root);
            let name = screen.name.unwrap();
            assert!(name.text().ends_with("..."), "{}", name.text());
            let a = name.coords();
            assert!(a.x1 >= 50 && a.x2 <= 349);
        });
    }

    #[test]
    fn signal_thresholds() {
        assert_eq!(bars_for(-50, true), 4);
        assert_eq!(bars_for(-60, true), 3);
        assert_eq!(bars_for(-70, true), 2);
        assert_eq!(bars_for(-80, true), 1);
        assert_eq!(bars_for(-50, false), 0);
        assert_eq!(signal_word(-60), "good");
        assert_eq!(signal_word(-80), "weak");
    }
}
