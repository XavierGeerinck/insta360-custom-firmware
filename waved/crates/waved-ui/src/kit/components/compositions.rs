//! Components assembled like real screens. The goldens are compared against
//! docs/mockups/png/home.png, recording.png and upload-mode.png before
//! milestone 2 starts.

use crate::kit::components::buttons::{
    back_button, BottomSpot, RoundButton, Side, SideButton, Tone,
};
use crate::kit::components::controls::{RadioList, RadioOption};
use crate::kit::components::dial::{Dial, RingStyle};
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::Dispatcher;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::snapshot::UiSnapshot;
use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
use crate::kit::tokens::{
    DIAL_CX, DIAL_CY, DIAL_HINT_Y, DIAL_ICON_X, DIAL_ICON_Y, DIAL_LINE_Y, DIAL_R, DIAL_TITLE_Y,
    HOLD_TO_STOP_MS, LIVE, RADIO_TOP, REC, SCREEN_W, TEXT, TEXT_2, TEXT_3, TITLE_Y,
};

/// Recording's text stack inside the dial (tops of the kit labels): the
/// "Recording" chip, the timer (placed by its digits' baseline, at y 138 as
/// in recording.png), the talker line and the hint.
const REC_CHIP_Y: i32 = 76;
const REC_TIMER_Y: i32 = 104;
const REC_LINE_Y: i32 = 140;
const REC_HINT_Y: i32 = 161;
/// The chip's dot and the gap before its word.
const REC_DOT: i32 = 7;
const REC_DOT_GAP: i32 = 5;

#[test]
fn home_from_kit_parts() {
    lvgl(|lv| {
        let root = lv.screen();
        let (dispatch, _bus, _ui) = Dispatcher::test();
        let mut snap = UiSnapshot::default();
        snap.mcu.usb_connected = true;
        snap.wifi.connected = true;
        snap.clock = "14:32".into();
        snap.battery.level = 82;
        snap.audio.call_off_hook = true;
        snap.meeting.pre_recording_active = true;

        let mut status = StatusArea::pill(root, &dispatch);
        status.update(&snap);
        let mut dial = Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3).on_tap(|| {});
        dial.set_ring(RingStyle::Live);
        dial.set_talker(Some(307.0));
        Obj::label(root, icons::MIC, Font::Icon20, TEXT_2).pos(DIAL_ICON_X, DIAL_ICON_Y);
        Obj::label(root, "Mic on", Font::Dial, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, DIAL_TITLE_Y);
        Obj::label(root, &snap.connection_line(), Font::Body, LIVE)
            .centered_text(SCREEN_W)
            .pos(0, DIAL_LINE_Y);
        Obj::label(root, "Tap to mute", Font::Micro, TEXT_3)
            .centered_text(SCREEN_W)
            .pos(0, DIAL_HINT_Y);
        SideButton::new(root, Side::Left, icons::VOLUME_LOW, "Quieter", || {});
        SideButton::new(root, Side::Right, icons::VOLUME_HIGH, "Louder", || {});
        RoundButton::new(
            root,
            BottomSpot::Main,
            icons::RECORD,
            "Record",
            Tone::Rec,
            || {},
        );

        assert_golden("kit_home", &lv.render());
        assert_targets_ok(root);
    });
}

#[test]
fn recording_from_kit_parts() {
    lvgl(|lv| {
        let root = lv.screen();
        let (dispatch, _bus, _ui) = Dispatcher::test();
        let mut snap = UiSnapshot::default();
        snap.mcu.usb_connected = true;
        snap.wifi.connected = true;
        snap.clock = "14:32".into();
        snap.battery.level = 82;

        let mut status = StatusArea::pill(root, &dispatch);
        status.update(&snap);
        let mut dial = Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3).on_tap(|| {});
        dial.set_ring(RingStyle::Recording);
        dial.set_talker(Some(60.0));
        let chip = Obj::plain(root)
            .pos(0, REC_CHIP_Y)
            .size(SCREEN_W, Font::Small.line_height())
            .row(REC_DOT_GAP);
        Obj::plain(chip).size(REC_DOT, REC_DOT).round().bg(REC);
        Obj::label(chip, "Recording", Font::Small, REC);
        Obj::label(root, "42:17", Font::Num48, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, REC_TIMER_Y);
        Obj::label(root, "Speaker 2 talking", Font::Small, TEXT_2)
            .centered_text(SCREEN_W)
            .pos(0, REC_LINE_Y);
        Obj::label(root, "Tap to mute", Font::Micro, TEXT_3)
            .centered_text(SCREEN_W)
            .pos(0, REC_HINT_Y);
        SideButton::new(root, Side::Left, icons::VOLUME_LOW, "Quieter", || {});
        SideButton::new(root, Side::Right, icons::VOLUME_HIGH, "Louder", || {});
        RoundButton::new(
            root,
            BottomSpot::Left,
            icons::MARK,
            "Mark",
            Tone::Neutral,
            || {},
        );
        RoundButton::new(
            root,
            BottomSpot::Main,
            icons::STOP,
            "Stop",
            Tone::RecText,
            || {},
        )
        .hold(HOLD_TO_STOP_MS as u32, || {});
        RoundButton::new(
            root,
            BottomSpot::Right,
            icons::PAUSE,
            "Pause",
            Tone::Neutral,
            || {},
        );

        assert_golden("kit_recording", &lv.render());
        assert_targets_ok(root);
    });
}

#[test]
fn after_a_recording_from_kit_parts() {
    lvgl(|lv| {
        let root = lv.screen();
        let (dispatch, _bus, _ui) = Dispatcher::test();
        let mut snap = UiSnapshot::default();
        snap.mcu.usb_connected = true;
        snap.wifi.connected = true;
        snap.clock = "15:22".into();
        snap.battery.level = 79;

        let mut status = StatusArea::plain(root);
        status.update(&snap);
        Obj::label(root, "After a recording", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        RadioList::new(
            root,
            RADIO_TOP,
            vec![
                RadioOption::new("Send automatically", "As soon as Wi-Fi allows"),
                RadioOption::new("Ask me each time", "Choose Send, Keep or Delete"),
                RadioOption::new("Keep on Wave", "Nothing leaves the device"),
            ],
            1,
            |_| {},
        );
        back_button(root, &dispatch);

        assert_golden("kit_upload_mode", &lv.render());
        assert_targets_ok(root);
    });
}
