//! The status slot at the top: a plain cluster on sub-screens, a tappable pill
//! on Home and Recording, and one-line messages that replace it for a few
//! seconds without covering a button.

use crate::kit::dispatch::{Dispatcher, Message, Severity};
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{text_width, Obj};
use crate::kit::route::Route;
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    LIVE, MESSAGE_MAX_W, RAISED, REC, SCREEN_W, STATUS, STATUS_PILL, SURFACE, TEXT, TEXT_2, TEXT_3, WARN,
};

#[derive(Clone, Debug, PartialEq)]
struct Shown {
    usb: bool,
    wifi: bool,
    bt: bool,
    clock: String,
    battery: u8,
    charging: bool,
}

pub struct StatusArea {
    cluster: Obj,
    usb: Obj,
    wifi: Obj,
    bt: Obj,
    clock: Obj,
    battery_icon: Obj,
    battery_text: Obj,
    message: Obj,
    message_icon: Obj,
    message_text: Obj,
    message_until_ms: Option<u64>,
    shown: Option<Shown>,
}

impl StatusArea {
    /// The status cluster on sub-screens. Not clickable.
    pub fn plain(parent: Obj) -> Self {
        Self::build(parent, None)
    }

    /// The status pill on Home and Recording. Tapping it opens the menu.
    pub fn pill(parent: Obj, dispatch: &Dispatcher) -> Self {
        Self::build(parent, Some(dispatch.clone()))
    }

    fn build(parent: Obj, dispatch: Option<Dispatcher>) -> Self {
        let is_pill = dispatch.is_some();
        let cluster = Obj::plain(parent)
            .rect(if is_pill { STATUS_PILL } else { STATUS })
            .row(7);
        if let Some(dispatch) = dispatch {
            cluster.bg(SURFACE).radius(14).clickable(true).ext_click(8);
            on_click(cluster, move || dispatch.navigate(Route::Menu));
        }
        let icon = |glyph: &str| Obj::label(cluster, glyph, Font::Icon16, TEXT_2);
        let usb = icon(icons::USB);
        let wifi = icon(icons::WIFI);
        let bt = icon(icons::BLUETOOTH);
        let clock = Obj::label(cluster, "", Font::Num13, TEXT_2);
        let battery = Obj::plain(cluster).fit().row(3);
        let battery_icon = Obj::label(battery, icons::BATTERY_4, Font::Icon16, TEXT_2);
        let battery_text = Obj::label(battery, "", Font::Num13, TEXT_2);
        if is_pill {
            Obj::label(cluster, icons::DOWN, Font::Icon16, TEXT_3);
        }
        let message = Obj::plain(parent)
            .size(MESSAGE_MAX_W, 28)
            .pos((SCREEN_W - MESSAGE_MAX_W) / 2, 7)
            .bg(RAISED)
            .radius(14)
            .row(6)
            .hidden(true);
        let message_icon = Obj::label(message, icons::INFO, Font::Icon16, TEXT_2);
        let message_text = Obj::label(message, "", Font::Small, TEXT);
        Self {
            cluster,
            usb,
            wifi,
            bt,
            clock,
            battery_icon,
            battery_text,
            message,
            message_icon,
            message_text,
            message_until_ms: None,
            shown: None,
        }
    }

    /// Expire messages and refresh icons, clock and battery when they change.
    pub fn update(&mut self, snap: &UiSnapshot) {
        if self.message_until_ms.is_some_and(|until| snap.now_ms >= until) {
            self.message_until_ms = None;
            self.message.hidden(true);
            self.cluster.hidden(false);
        }
        let next = Shown {
            usb: snap.mcu.usb_connected,
            wifi: snap.wifi.connected,
            bt: snap.bt.connected,
            clock: snap.clock.clone(),
            battery: snap.battery.level,
            charging: snap.battery.is_charging,
        };
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        self.usb.hidden(!next.usb);
        self.wifi.hidden(!next.wifi);
        self.bt.hidden(!next.bt);
        self.clock.set_text(&next.clock);
        let (glyph, color) = battery_look(next.battery, next.charging);
        self.battery_icon.text_color(color).set_text(glyph);
        self.battery_text.text_color(color).set_text(&format!("{}%", next.battery));
        self.shown = Some(next);
    }

    /// Replace the status with `message` until its duration has passed.
    pub fn show(&mut self, message: &Message, now_ms: u64) {
        let (glyph, color) = match message.severity {
            Severity::Success => (icons::CHECK_CIRCLE, LIVE),
            Severity::Info => (icons::INFO, TEXT_2),
            Severity::Warning => (icons::ALERT, WARN),
            Severity::Error => (icons::ALERT, REC),
        };
        self.message_icon.text_color(color).set_text(glyph);
        // One spare pixel, so text that fits exactly is never cut to "...".
        let text_w = (text_width(&message.text, Font::Small) + 1).min(MESSAGE_MAX_W - 44);
        self.message_text.width(text_w).one_line().set_text(&message.text);
        let width = (9 + 16 + 6 + text_w + 13).min(MESSAGE_MAX_W);
        self.message
            .size(width, 28)
            .pos((SCREEN_W - width) / 2, 7)
            .hidden(false);
        self.cluster.hidden(true);
        self.message_until_ms = Some(now_ms + message.severity.duration_ms());
    }

    pub fn cluster(&self) -> Obj {
        self.cluster
    }

    pub fn message(&self) -> Obj {
        self.message
    }
}

/// Battery glyph and colour: teal while charging, amber at or below 20 %,
/// red at or below 10 %.
pub fn battery_look(level: u8, charging: bool) -> (&'static str, u32) {
    if charging {
        return (icons::BATTERY_CHARGING, LIVE);
    }
    let glyph = match level {
        0..=12 => icons::BATTERY_0,
        13..=37 => icons::BATTERY_1,
        38..=62 => icons::BATTERY_2,
        63..=87 => icons::BATTERY_3,
        _ => icons::BATTERY_4,
    };
    let color = match level {
        0..=10 => REC,
        11..=20 => WARN,
        _ => TEXT_2,
    };
    (glyph, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};

    fn snap() -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.mcu.usb_connected = true;
        snap.wifi.connected = true;
        snap.clock = "14:32".into();
        snap.battery.level = 82;
        snap
    }

    #[test]
    fn plain_status_shows_connections_time_and_battery() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut status = StatusArea::plain(root);
            status.update(&snap());
            assert!(!status.cluster().is_clickable());
            assert_golden("status_plain", &lv.render());
        });
    }

    #[test]
    fn the_pill_opens_the_menu() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, ui) = Dispatcher::test();
            let mut status = StatusArea::pill(root, &dispatch);
            status.update(&snap());
            assert_golden("status_pill", &lv.render());
            assert_targets_ok(root);
            lv.click(status.cluster());
            assert_eq!(ui.try_recv().unwrap(), UiEvent::Navigate(Route::Menu));
        });
    }

    #[test]
    fn messages_replace_the_status_and_expire() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut status = StatusArea::plain(root);
            let mut now = snap();
            now.now_ms = 1_000;
            status.update(&now);
            status.show(&Message::new(Severity::Success, "Recording sent"), 1_000);
            assert!(status.cluster().is_hidden());
            assert!(!status.message().is_clickable(), "messages never take taps");
            assert_golden("status_message_success", &lv.render());
            now.now_ms = 2_999;
            status.update(&now);
            assert!(status.cluster().is_hidden());
            now.now_ms = 3_000;
            status.update(&now);
            assert!(!status.cluster().is_hidden() && status.message().is_hidden());
        });
    }

    #[test]
    fn long_messages_stay_within_230_px() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut status = StatusArea::plain(root);
            status.show(
                &Message::new(Severity::Warning, "No Wi-Fi, so the upload of the 42-minute recording is waiting"),
                0,
            );
            status.message().update_layout();
            let a = status.message().coords();
            assert!(a.x2 - a.x1 < MESSAGE_MAX_W);
            let text = status.message().child(1).coords();
            assert_eq!(text.y2 - text.y1 + 1, Font::Small.line_height(), "one line, not wrapped");
        });
    }

    #[test]
    fn battery_colours_follow_the_level() {
        assert_eq!(battery_look(82, false), (icons::BATTERY_3, TEXT_2));
        assert_eq!(battery_look(18, false).1, WARN);
        assert_eq!(battery_look(8, false), (icons::BATTERY_0, REC));
        assert_eq!(battery_look(8, true), (icons::BATTERY_CHARGING, LIVE));
    }
}
