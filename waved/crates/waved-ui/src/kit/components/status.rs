//! The status slot at the top: a plain cluster on sub-screens, a tappable pill
//! on Home and Recording, and one-line messages that replace it for a few
//! seconds without covering a button.
//!
//! The pill leads with a teal dot while the pre-recording buffer fills. It
//! follows `MeetingState::pre_recording_active` (the buffer is filling now),
//! not `SettingsState::pre_recording` (the setting), so the dot never claims
//! audio is buffered while it is not.

use crate::bindings::{lv_obj_set_style_margin_left, lv_style_selector_t, LV_PART_MAIN};
use crate::kit::dispatch::{Dispatcher, Message, Severity};
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{text_width, Obj};
use crate::kit::route::Route;
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    Rect, LIVE, MESSAGE_MAX_W, RAISED, REC, SCREEN_W, STATUS, STATUS_PILL, STATUS_PILL_HIT,
    SURFACE, TEXT, TEXT_2, TEXT_3, WARN,
};

/// Space between the cluster's items. The mockup's `.crown` gap is 8 with
/// 15 px icons; the kit's 16 px glyph boxes carry more side bearing, so 7
/// puts the items within about 1.5 px of home.png and menu.png.
const GAP: i32 = 7;
/// The pill's chevron: the mockup draws it 12 px with a -1 px margin. The
/// kit's 16 px glyph box is cut to 14 and pulled 2 px towards the battery
/// (both cuts are empty side bearing).
const CHEVRON_W: i32 = 14;
const CHEVRON_MARGIN: i32 = -2;

#[derive(Clone, Debug, PartialEq)]
struct Shown {
    usb: bool,
    wifi: bool,
    bt: bool,
    clock: String,
    /// The level, once the MCU has sent a battery frame.
    battery: Option<u8>,
    charging: bool,
    pre_recording: bool,
}

pub struct StatusArea {
    /// What hides while a message shows: the cluster, or the pill's hit area.
    slot: Obj,
    pre_recording: Option<Obj>,
    usb: Obj,
    wifi: Obj,
    bt: Obj,
    clock: Obj,
    battery: Obj,
    battery_icon: Obj,
    battery_text: Obj,
    message: Obj,
    message_icon: Obj,
    message_text: Obj,
    message_until_ms: Option<u64>,
    /// The last `UiSnapshot::message` seen, so each one shows once.
    message_seq: Option<u64>,
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
        let (slot, cluster) = match dispatch {
            Some(dispatch) => {
                // The hit area reaches 16 px below the pill, down to the dial's
                // tap circle; LVGL's extended click area would grow it upwards too.
                let hit = Obj::plain(parent).rect(STATUS_PILL_HIT).clickable(true);
                on_click(hit, move || dispatch.navigate(Route::Menu));
                let pill = Obj::plain(hit)
                    .rect(Rect::new(0, 0, STATUS_PILL.w, STATUS_PILL.h))
                    .bg(SURFACE)
                    .radius(STATUS_PILL.h / 2)
                    .row(GAP);
                (hit, pill)
            }
            None => {
                let cluster = Obj::plain(parent).rect(STATUS).row(GAP);
                (cluster, cluster)
            }
        };
        let pre_recording =
            is_pill.then(|| Obj::plain(cluster).size(7, 7).round().bg(LIVE).hidden(true));
        let icon = |glyph: &str| Obj::label(cluster, glyph, Font::Icon16, TEXT_2);
        let usb = icon(icons::USB);
        let wifi = icon(icons::WIFI);
        let bt = icon(icons::BLUETOOTH);
        let clock = Obj::label(cluster, "", Font::Num13, TEXT_2);
        let battery = Obj::plain(cluster).fit().row(3);
        let battery_icon = Obj::label(battery, icons::BATTERY_4, Font::Icon16, TEXT_2);
        let battery_text = Obj::label(battery, "", Font::Num13, TEXT_2);
        if is_pill {
            let chevron = Obj::label(cluster, icons::DOWN, Font::Icon16, TEXT_3).width(CHEVRON_W);
            // SAFETY: live object.
            unsafe {
                lv_obj_set_style_margin_left(
                    chevron.raw(),
                    CHEVRON_MARGIN,
                    LV_PART_MAIN as lv_style_selector_t,
                )
            };
        }
        let message = Obj::plain(parent)
            .size(MESSAGE_MAX_W, STATUS_PILL.h)
            .pos((SCREEN_W - MESSAGE_MAX_W) / 2, STATUS_PILL.y)
            .bg(RAISED)
            .radius(14)
            .row(6)
            .hidden(true);
        let message_icon = Obj::label(message, icons::INFO, Font::Icon16, TEXT_2);
        let message_text = Obj::label(message, "", Font::Small, TEXT);
        Self {
            slot,
            pre_recording,
            usb,
            wifi,
            bt,
            clock,
            battery,
            battery_icon,
            battery_text,
            message,
            message_icon,
            message_text,
            message_until_ms: None,
            message_seq: None,
            shown: None,
        }
    }

    /// Show the runtime's message, expire messages and refresh icons, clock
    /// and battery when they change.
    pub fn update(&mut self, snap: &UiSnapshot) {
        // Runtime messages arrive in the snapshot. Show each one once, timed
        // from when it was posted, so a screen built mid-message shows the rest.
        if let Some(active) = &snap.message {
            if self.message_seq != Some(active.seq) {
                self.message_seq = Some(active.seq);
                if snap.now_ms < active.until_ms() {
                    self.show(&active.message, active.since_ms);
                }
            }
        }
        if self
            .message_until_ms
            .is_some_and(|until| snap.now_ms >= until)
        {
            self.message_until_ms = None;
            self.message.hidden(true);
            self.slot.hidden(false);
        }
        let next = Shown {
            usb: snap.mcu.usb_connected,
            wifi: snap.wifi.connected,
            bt: snap.bt.connected,
            clock: snap.clock.clone(),
            battery: battery_reading(&snap.battery),
            charging: snap.battery.is_charging,
            pre_recording: snap.meeting.pre_recording_active,
        };
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        if let Some(dot) = self.pre_recording {
            dot.hidden(!next.pre_recording);
        }
        self.usb.hidden(!next.usb);
        self.wifi.hidden(!next.wifi);
        self.bt.hidden(!next.bt);
        self.clock.set_text(&next.clock);
        self.battery.hidden(next.battery.is_none());
        if let Some(level) = next.battery {
            let (glyph, color) = battery_look(level, next.charging);
            self.battery_icon.text_color(color).set_text(glyph);
            self.battery_text
                .text_color(color)
                .set_text(&format!("{level}%"));
        }
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
        let glyph = message.icon.unwrap_or(glyph);
        self.message_icon.text_color(color).set_text(glyph);
        // One spare pixel, so text that fits exactly is never cut to "...".
        let text_w = (text_width(&message.text, Font::Small) + 1).min(MESSAGE_MAX_W - 44);
        self.message_text
            .width(text_w)
            .one_line()
            .set_text(&message.text);
        let width = (9 + 16 + 6 + text_w + 13).min(MESSAGE_MAX_W);
        self.message
            .size(width, STATUS_PILL.h)
            .pos((SCREEN_W - width) / 2, STATUS_PILL.y)
            .hidden(false);
        self.slot.hidden(true);
        self.message_until_ms = Some(now_ms + message.severity.duration_ms());
    }

    /// What hides while a message shows. On `plain` it is the cluster itself
    /// (not clickable). On `pill` it is the pill's hit area
    /// (`STATUS_PILL_HIT`, the tap target), and the visible 28 px pill is its
    /// `child(0)`.
    pub fn cluster(&self) -> Obj {
        self.slot
    }

    /// The pill's pre-recording dot; the plain cluster has none.
    pub fn pre_recording_dot(&self) -> Option<Obj> {
        self.pre_recording
    }

    pub fn message(&self) -> Obj {
        self.message
    }
}

/// Battery glyph and colour: teal while charging, amber at or below 20 %,
/// red at or below 10 %.
/// The battery level, or `None` before the MCU has reported one:
/// `BatteryState::default()` is level 0 with voltage 0, which must not show
/// as an empty battery (the same rule as the battery messages).
pub fn battery_reading(battery: &waved_state::BatteryState) -> Option<u8> {
    (battery.voltage > 0).then_some(battery.level)
}

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
    use crate::kit::snapshot::ActiveMessage;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};

    fn snap() -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.mcu.usb_connected = true;
        snap.wifi.connected = true;
        snap.clock = "14:32".into();
        snap.battery.level = 82;
        snap.battery.voltage = 3_800;
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

    fn area(obj: Obj) -> Rect {
        obj.update_layout();
        let a = obj.coords();
        Rect::new(a.x1, a.y1, a.x2 - a.x1 + 1, a.y2 - a.y1 + 1)
    }

    #[test]
    fn the_pill_opens_the_menu() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, ui) = Dispatcher::test();
            let mut status = StatusArea::pill(root, &dispatch);
            let mut snap = snap();
            snap.meeting.pre_recording_active = true;
            status.update(&snap);
            assert_golden("status_pill", &lv.render());
            assert_targets_ok(root);
            // A 28 px pill with a 44 px hit area below it, ending at the dial.
            assert_eq!(area(status.cluster()), STATUS_PILL_HIT);
            assert_eq!(area(status.cluster().child(0)), STATUS_PILL);
            lv.click(status.cluster());
            assert_eq!(ui.try_recv().unwrap(), UiEvent::Navigate(Route::Menu));
        });
    }

    #[test]
    fn the_pre_recording_dot_leads_the_pill_while_buffering() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let mut status = StatusArea::pill(root, &dispatch);
            let dot = status.pre_recording_dot().expect("the pill has a dot");
            assert!(dot.is_hidden(), "no dot before the first update");
            let mut snap = snap();
            status.update(&snap);
            assert!(dot.is_hidden(), "no dot while the buffer is idle");
            snap.meeting.pre_recording_active = true;
            status.update(&snap);
            assert!(!dot.is_hidden());
            let a = area(dot);
            assert_eq!((a.w, a.h), (7, 7));
            assert_eq!(
                status.cluster().child(0).child(0),
                dot,
                "the dot leads the contents"
            );
            let [r, g, b] = lv.render().pixel(a.x + 3, a.y + 3);
            assert!(g > 180 && r < 100 && b > 140, "teal dot, got {r},{g},{b}");
            snap.meeting.pre_recording_active = false;
            status.update(&snap);
            assert!(dot.is_hidden());
            // The plain cluster on sub-screens has no dot.
            assert!(StatusArea::plain(root).pre_recording_dot().is_none());
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

    fn active(seq: u64, severity: Severity, text: &str, since_ms: u64) -> Option<ActiveMessage> {
        Some(ActiveMessage {
            seq,
            message: Message::new(severity, text),
            since_ms,
        })
    }

    #[test]
    fn a_snapshot_message_shows_on_a_new_status_area_until_it_expires() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut now = snap();
            // Posted at 1 000 on the screen before; this one is built at 4 500.
            now.now_ms = 4_500;
            now.message = active(7, Severity::Error, "Upload failed, see Uploads", 1_000);
            let mut status = StatusArea::plain(root);
            status.update(&now);
            assert!(!status.message().is_hidden() && status.cluster().is_hidden());
            now.now_ms = 4_999;
            status.update(&now);
            assert!(!status.message().is_hidden());
            now.now_ms = 5_000;
            status.update(&now);
            assert!(
                status.message().is_hidden() && !status.cluster().is_hidden(),
                "gone 4 s after it was posted, not after the screen was built"
            );
        });
    }

    #[test]
    fn each_snapshot_message_shows_once_per_seq() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut status = StatusArea::plain(root);
            let mut now = snap();
            now.now_ms = 1_500;
            now.message = active(1, Severity::Success, "Recording sent", 1_000);
            status.update(&now);
            assert!(!status.message().is_hidden());
            now.now_ms = 3_000;
            status.update(&now);
            assert!(status.message().is_hidden(), "expired, and not shown again");
            now.now_ms = 3_100;
            status.update(&now);
            assert!(status.message().is_hidden(), "same seq: still gone");
            now.message = active(2, Severity::Success, "Recording sent", 3_100);
            status.update(&now);
            assert!(
                !status.message().is_hidden(),
                "the same text again is a new message"
            );
        });
    }

    #[test]
    fn an_expired_snapshot_message_never_shows() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut now = snap();
            now.now_ms = 10_000;
            now.message = active(3, Severity::Info, "Saved", 1_000);
            let mut status = StatusArea::plain(root);
            status.update(&now);
            assert!(status.message().is_hidden() && !status.cluster().is_hidden());
        });
    }

    #[test]
    fn long_messages_stay_within_240_px_in_the_status_slot() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut status = StatusArea::plain(root);
            status.show(
                &Message::new(
                    Severity::Warning,
                    "No Wi-Fi, so the upload of the 42-minute recording is waiting",
                ),
                0,
            );
            status.message().update_layout();
            let a = area(status.message());
            assert_eq!((a.w, a.y, a.h), (MESSAGE_MAX_W, STATUS_PILL.y, 28));
            assert_eq!(a.x * 2 + a.w, SCREEN_W, "centred");
            let text = status.message().child(1).coords();
            assert_eq!(
                text.y2 - text.y1 + 1,
                Font::Small.line_height(),
                "one line, not wrapped"
            );
        });
    }

    #[test]
    fn no_battery_reading_hides_the_battery() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut status = StatusArea::plain(root);
            let mut snap = snap();
            snap.battery = Default::default();
            status.update(&snap);
            assert!(status.battery.is_hidden(), "no frame yet: no 0 %");
            snap.battery.level = 64;
            snap.battery.voltage = 3_700;
            status.update(&snap);
            assert!(!status.battery.is_hidden());
            assert_eq!(status.battery_text.text(), "64%");
            let _ = lv;
        });
    }

    #[test]
    fn the_pill_shows_charging_in_teal() {
        // The Messages strip in docs/mockups/index.html: USB, Wi-Fi, 14:32, 64 % charging.
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let mut status = StatusArea::pill(root, &dispatch);
            let mut snap = snap();
            snap.battery.level = 64;
            snap.battery.is_charging = true;
            status.update(&snap);
            assert_eq!(status.battery_icon.text(), icons::BATTERY_CHARGING);
            assert_eq!(status.battery_text.text(), "64%");
            assert_golden("status_pill_charging", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_pill_shows_a_low_battery_in_red() {
        // The Messages strip: on battery (no USB), Wi-Fi, 14:32, 8 % in red.
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let mut status = StatusArea::pill(root, &dispatch);
            let mut snap = snap();
            snap.mcu.usb_connected = false;
            snap.battery.level = 8;
            status.update(&snap);
            assert_eq!(status.battery_icon.text(), icons::BATTERY_0);
            assert_eq!(status.battery_text.text(), "8%");
            let frame = lv.render();
            assert_golden("status_pill_low_battery", &frame);
            assert_targets_ok(root);
            // The level is drawn in red.
            let text = status.battery_text.coords();
            let red = (text.x1..=text.x2)
                .flat_map(|x| (text.y1..=text.y2).map(move |y| (x, y)))
                .any(|(x, y)| matches!(frame.pixel(x, y), [r, g, _] if r > 200 && g < 120));
            assert!(red, "8 % is red");
        });
    }

    #[test]
    fn the_status_reads_the_battery_the_mcu_publishes() {
        // waved-mcu writes BatteryState into bus.battery from the heartbeats;
        // the snapshot copies it and the status shows it.
        lvgl(|lv| {
            let bus = waved_state::StateBus::new();
            let mut status = StatusArea::plain(lv.screen());
            status.update(&UiSnapshot::capture(&bus, 0, 0));
            assert!(status.battery.is_hidden(), "nothing before the first heartbeat");
            bus.battery.send_replace(waved_state::BatteryState {
                level: 9,
                voltage: 3_550,
                temperature: 31,
                is_charging: false,
            });
            status.update(&UiSnapshot::capture(&bus, 0, 0));
            assert!(!status.battery.is_hidden());
            assert_eq!(status.battery_text.text(), "9%");
            assert_eq!(status.battery_icon.text(), icons::BATTERY_0);
            bus.battery.send_modify(|b| b.is_charging = true);
            status.update(&UiSnapshot::capture(&bus, 0, 0));
            assert_eq!(status.battery_icon.text(), icons::BATTERY_CHARGING);
        });
    }

    #[test]
    fn a_message_can_carry_its_own_icon() {
        // Messages strip: "Battery 10%, plug in Wave" with the battery, in amber.
        lvgl(|lv| {
            let root = lv.screen();
            let mut status = StatusArea::plain(root);
            status.update(&snap());
            let message = Message::new(Severity::Warning, "Battery 10%, plug in Wave")
                .with_icon(icons::BATTERY_0);
            status.show(&message, 0);
            assert_eq!(status.message_icon.text(), icons::BATTERY_0);
            assert_golden("status_message_battery_low", &lv.render());
            status.show(&Message::new(Severity::Warning, "Storage almost full"), 0);
            assert_eq!(status.message_icon.text(), icons::ALERT, "the severity's icon otherwise");
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
