//! Home (A1, docs/mockups/png/home.png): the dial tells the table what the
//! microphones do and mutes on a tap; Quieter / Louder at the sides; Record
//! in the main spot; the status pill opens the menu (its teal dot shows
//! pre-recording, which replaced Record's old "includes last 30 s" line).

use crate::kit::components::buttons::RoundButton;
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::Severity;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::talker::TalkerGate;
use crate::kit::tokens::{
    DIAL_CX, DIAL_HINT_Y, DIAL_ICON_X, DIAL_ICON_Y, DIAL_LINE_Y, DIAL_TITLE_Y, LIVE, REC, SCREEN_W,
    TEXT, TEXT_2, TEXT_3,
};
use crate::screens_v2::main_parts::{home_ring, ring_font, MainDial, RING_TEXT_W};
use crate::screens_v2::messages::copy;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::Command;

/// A second Record tap within this window is ignored (the Recording screen
/// takes over within a frame or two once the service starts).
pub const RECORD_DEBOUNCE_MS: u64 = 2_000;

/// The text inside Home's dial ("Dial states" in docs/mockups/README.md).
#[derive(Clone, Debug, PartialEq)]
pub struct HomeView {
    pub icon: &'static str,
    pub icon_color: u32,
    pub title: &'static str,
    pub title_color: u32,
    pub line: String,
    pub line_color: u32,
    pub hint: &'static str,
}

impl HomeView {
    pub fn from_snapshot(snap: &UiSnapshot) -> Self {
        let muted = snap.audio.mic_muted;
        let live = !muted && (snap.in_call() || snap.audio.host_listening);
        Self {
            icon: if muted { icons::MIC_OFF } else { icons::MIC },
            icon_color: if muted { REC } else { TEXT_2 },
            title: if muted { "Muted" } else { "Mic on" },
            title_color: if muted { REC } else { TEXT },
            line: snap.connection_line(),
            line_color: if live { LIVE } else { TEXT_2 },
            hint: if muted {
                "Tap to unmute"
            } else {
                "Tap to mute"
            },
        }
    }
}

/// Why Record can't start now, as the message to show instead. Low space
/// alone is no block: the service prunes old recordings and still records.
pub fn record_blocked(snap: &UiSnapshot) -> Option<&'static str> {
    (snap.meeting.storage_blocked || snap.meeting.recording_blocked_reason.is_some())
        .then_some(copy::CANT_RECORD)
}

/// What Record's tap needs from the latest snapshot.
#[derive(Clone, Copy, Default)]
struct RecordGate {
    blocked: Option<&'static str>,
    now_ms: u64,
    started_ms: Option<u64>,
}

#[derive(Default)]
pub struct HomeScreen {
    parts: Option<Parts>,
}

#[cfg_attr(not(test), allow(dead_code))] // `record` is read only by tests
struct Parts {
    status: StatusArea,
    dial: MainDial,
    icon: Obj,
    title: Obj,
    line: Obj,
    hint: Obj,
    record: RoundButton,
    gate: Rc<Cell<RecordGate>>,
    shown: Option<HomeView>,
    stack_hidden: bool,
    /// Hysteresis for the talker wedge (`kit::talker`).
    talker: TalkerGate,
}

impl Screen for HomeScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        let status = StatusArea::pill(root, &ctx.dispatch);
        let dial = MainDial::new(root, &ctx.dispatch, snap);
        let icon = Obj::label(root, icons::MIC, Font::Icon20, TEXT_2).pos(DIAL_ICON_X, DIAL_ICON_Y);
        let title = Obj::label(root, "", Font::Dial, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, DIAL_TITLE_Y);
        let line = Obj::label(root, "", Font::Body, TEXT_2)
            .centered_text(RING_TEXT_W)
            .pos(DIAL_CX - RING_TEXT_W / 2, DIAL_LINE_Y);
        let hint = Obj::label(root, "", Font::Micro, TEXT_3)
            .centered_text(SCREEN_W)
            .pos(0, DIAL_HINT_Y);
        let gate = Rc::new(Cell::new(RecordGate::default()));
        let record = {
            let (gate, dispatch) = (gate.clone(), ctx.dispatch.clone());
            RoundButton::record(root, move || {
                let mut now = gate.get();
                if let Some(text) = now.blocked {
                    dispatch.show(Severity::Error, text);
                } else if now
                    .started_ms
                    .is_none_or(|at| now.now_ms >= at + RECORD_DEBOUNCE_MS)
                {
                    now.started_ms = Some(now.now_ms);
                    gate.set(now);
                    dispatch.command(Command::StartRecording);
                }
            })
        };
        self.parts = Some(Parts {
            status,
            dial,
            icon,
            title,
            line,
            hint,
            record,
            gate,
            shown: None,
            stack_hidden: false,
            talker: TalkerGate::default(),
        });
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        p.status.update(snap);
        let mut gate = p.gate.get();
        gate.blocked = record_blocked(snap);
        gate.now_ms = snap.now_ms;
        p.gate.set(gate);
        let gauge = p.dial.update(snap, home_ring(snap, &mut p.talker));
        if gauge != p.stack_hidden {
            for obj in [p.icon, p.title, p.line, p.hint] {
                obj.hidden(gauge);
            }
            p.stack_hidden = gauge;
        }
        let view = HomeView::from_snapshot(snap);
        if p.shown.as_ref() == Some(&view) {
            return;
        }
        p.icon.text_color(view.icon_color).set_text(view.icon);
        p.title.text_color(view.title_color).set_text(view.title);
        p.line
            .font(ring_font(&view.line, Font::Body))
            .centered_text(RING_TEXT_W)
            .pos(DIAL_CX - RING_TEXT_W / 2, DIAL_LINE_Y)
            .text_color(view.line_color)
            .set_text(&view.line);
        p.hint.set_text(view.hint);
        p.shown = Some(view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::LV_STATE_PRESSED;
    use crate::kit::dispatch::{Message, UiEvent};
    use crate::kit::obj::text_width;
    use crate::kit::route::Route;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::main_parts::fixtures::{ctx, events, sent, settle, snap, with_message};

    /// home.png: in a call, someone talking at 307°, pre-recording on.
    fn in_a_call() -> UiSnapshot {
        let mut snap = snap();
        snap.audio.call_off_hook = true;
        snap.audio.doa_confidence = 0.8;
        snap.audio.doa_horizontal = 127.0; // screen 307°
        snap.meeting.pre_recording_active = true;
        snap
    }

    #[test]
    fn home_view_follows_the_dial_table() {
        let mut snap = snap();
        let view = HomeView::from_snapshot(&snap);
        assert_eq!(
            (view.title, view.line.as_str(), view.line_color, view.hint),
            ("Mic on", "Connected by USB", TEXT_2, "Tap to mute")
        );
        snap.audio.host_listening = true;
        let view = HomeView::from_snapshot(&snap);
        assert_eq!(
            (view.line.as_str(), view.line_color),
            ("Computer is listening", LIVE)
        );
        snap.audio.call_off_hook = true;
        assert_eq!(HomeView::from_snapshot(&snap).line, "In a call");
        snap.audio.mic_muted = true;
        let view = HomeView::from_snapshot(&snap);
        assert_eq!(
            (view.icon, view.icon_color, view.title, view.title_color),
            (icons::MIC_OFF, REC, "Muted", REC)
        );
        assert_eq!(
            (view.line.as_str(), view.line_color, view.hint),
            ("Call can't hear you", TEXT_2, "Tap to unmute")
        );
        let mut idle = snap_without_usb();
        assert_eq!(HomeView::from_snapshot(&idle).line, "Not connected");
        idle.audio.mic_muted = true;
        assert_eq!(HomeView::from_snapshot(&idle).line, "Mic is off");
    }

    fn snap_without_usb() -> UiSnapshot {
        let mut snap = snap();
        snap.mcu.usb_connected = false;
        snap
    }

    #[test]
    fn home_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = in_a_call();
            let mut screen = HomeScreen::default();
            screen.build(root, &ctx, &snap);
            settle(&mut screen, &mut snap);
            assert_golden("home", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn home_muted_with_a_message_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = snap();
            snap.audio.call_off_hook = true;
            snap.audio.mic_muted = true;
            with_message(&mut snap, Severity::Success, "Recording sent");
            HomeScreen::default().build(root, &ctx, &snap);
            assert_golden("home_muted", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn home_volume_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, _ui) = ctx();
            let mut snap = in_a_call();
            snap.audio.speaker_volume = 50;
            let mut screen = HomeScreen::default();
            screen.build(root, &ctx, &snap);
            let louder = screen.parts.as_ref().unwrap().dial.louder();
            lv.click(louder);
            louder.set_state(LV_STATE_PRESSED, true);
            screen.update(&snap);
            assert_eq!(sent(&mut bus), ["SetSpeakerVolume(60)"]);
            let parts = screen.parts.as_ref().unwrap();
            assert!(parts.title.is_hidden(), "the gauge replaces the dial text");
            assert_golden("home_volume", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_dial_mutes_and_the_pill_opens_the_menu() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut screen = HomeScreen::default();
            screen.build(root, &ctx, &snap());
            let parts = screen.parts.as_ref().unwrap();
            lv.click(parts.dial.hit());
            lv.click(parts.status.cluster());
            assert_eq!(sent(&mut bus), ["ToggleMicMute"]);
            assert_eq!(events(&ui), [UiEvent::Navigate(Route::Menu)]);
        });
    }

    #[test]
    fn record_starts_once_and_explains_when_blocked() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut snap = snap();
            let mut screen = HomeScreen::default();
            screen.build(root, &ctx, &snap);
            let record = screen.parts.as_ref().unwrap().record.obj();
            lv.click(record);
            lv.click(record);
            assert_eq!(
                sent(&mut bus),
                ["StartRecording"],
                "a double tap starts once"
            );
            snap.now_ms += RECORD_DEBOUNCE_MS;
            screen.update(&snap);
            lv.click(record);
            assert_eq!(sent(&mut bus), ["StartRecording"], "a later tap retries");
            snap.meeting.storage_blocked = true;
            screen.update(&snap);
            lv.click(record);
            assert!(sent(&mut bus).is_empty());
            assert_eq!(
                events(&ui),
                [UiEvent::Show(Message::new(
                    Severity::Error,
                    copy::CANT_RECORD
                ))]
            );
            snap.meeting.storage_blocked = false;
            snap.meeting.recording_blocked_reason =
                Some(waved_state::RecordingIntegrityFault::WavWriteFailed);
            assert_eq!(record_blocked(&snap), Some(copy::CANT_RECORD));
            // Low space alone is not a block: the service prunes and still records.
            snap.meeting.recording_blocked_reason = None;
            snap.files.low_space = true;
            assert_eq!(record_blocked(&snap), None);
        });
    }

    #[test]
    fn every_home_line_fits_inside_the_ring() {
        lvgl(|lv| {
            let mut cases = Vec::new();
            let mut s = snap();
            cases.push(s.clone()); // Connected by USB
            s.mcu.usb_connected = false;
            cases.push(s.clone()); // Not connected
            s.bt.connected = true;
            cases.push(s.clone()); // Connected by Bluetooth
            s.audio.host_listening = true;
            cases.push(s.clone()); // Computer is listening
            s.audio.call_off_hook = true;
            cases.push(s.clone()); // In a call
            s.audio.mic_muted = true;
            cases.push(s.clone()); // Call can't hear you
            for case in cases {
                let line = HomeView::from_snapshot(&case).line;
                let width = text_width(&line, ring_font(&line, Font::Body));
                assert!(width <= RING_TEXT_W, "{line:?} is {width} px");
            }
            // A long Bluetooth name stays one truncated line inside the ring.
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut long = snap_without_usb();
            long.bt.connected = true;
            long.bt.device_name = "Conference room phone on the third floor".into();
            let mut screen = HomeScreen::default();
            screen.build(root, &ctx, &long);
            let line = screen.parts.as_ref().unwrap().line;
            line.update_layout();
            let area = line.coords();
            assert_eq!(
                (area.x1, area.x2 - area.x1 + 1),
                (DIAL_CX - RING_TEXT_W / 2, RING_TEXT_W)
            );
            assert_eq!(area.y2 - area.y1 + 1, Font::Small.line_height(), "one line");
        });
    }
}
