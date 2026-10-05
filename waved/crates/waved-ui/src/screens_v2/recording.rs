//! Recording (A2, docs/mockups/png/recording.png): Home's layout, so mute and
//! volume stay where they were; the bottom becomes Mark, Stop and Pause on
//! the arc. Stop takes Record's spot and needs a 1 s hold (D1).

use crate::bindings::lv_event_code_t_LV_EVENT_PRESSED;
use crate::kit::components::buttons::{BottomSpot, RoundButton, Tone};
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::Severity;
use crate::kit::event::on;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    DIAL_CX, REC, REC_CHIP_Y, REC_DOT, REC_DOT_GAP, REC_HINT_Y, REC_LINE_Y, REC_TIMER_Y, SCREEN_W,
    TEXT, TEXT_2, TEXT_3, WARN,
};
use crate::screens_v2::main_parts::{
    format_timer, recording_ring, timer_font, MainDial, RING_TEXT_W,
};
use crate::screens_v2::messages::copy;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::{Command, UploadMode};

/// The text inside Recording's dial ("Dial states": Recording, Recording
/// muted, Paused).
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingView {
    pub chip: &'static str,
    pub chip_color: u32,
    pub chip_dot: bool,
    pub timer: String,
    pub timer_font: Font,
    pub timer_y: i32,
    pub line: String,
    pub line_color: u32,
    pub hint: &'static str,
    pub paused: bool,
}

impl RecordingView {
    pub fn from_snapshot(snap: &UiSnapshot) -> Self {
        let (audio, meeting) = (&snap.audio, &snap.meeting);
        let (timer_font, timer_y) = timer_font(meeting.duration_secs);
        let (line, line_color) = if audio.mic_muted {
            ("Muted, not recorded".to_string(), REC)
        } else if meeting.paused {
            ("Not recording".to_string(), TEXT_2)
        } else if let Some(speaker) = meeting.current_speaker {
            (format!("Speaker {speaker} talking"), TEXT_2)
        } else {
            ("Listening".to_string(), TEXT_2)
        };
        Self {
            chip: if meeting.paused {
                "Paused"
            } else {
                "Recording"
            },
            chip_color: if meeting.paused { WARN } else { REC },
            chip_dot: !meeting.paused,
            timer: format_timer(meeting.duration_secs),
            timer_font,
            timer_y,
            line,
            line_color,
            hint: if audio.mic_muted {
                "Tap to unmute"
            } else {
                "Tap to mute"
            },
            paused: meeting.paused,
        }
    }
}

/// What the button handlers need from the latest snapshot.
#[derive(Clone, Copy, Default)]
struct Live {
    paused: bool,
    duration_secs: u32,
    upload_mode: UploadMode,
}

#[derive(Default)]
pub struct RecordingScreen {
    parts: Option<Parts>,
}

#[cfg_attr(not(test), allow(dead_code))] // `stop` is read only by tests
struct Parts {
    status: StatusArea,
    dial: MainDial,
    chip: Obj,
    chip_dot: Obj,
    chip_text: Obj,
    timer: Obj,
    line: Obj,
    hint: Obj,
    mark: RoundButton,
    stop: RoundButton,
    pause: RoundButton,
    live: Rc<Cell<Live>>,
    shown: Option<RecordingView>,
    stack_hidden: bool,
}

impl Screen for RecordingScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        let status = StatusArea::pill(root, &ctx.dispatch);
        let dial = MainDial::new(root, &ctx.dispatch, snap);
        let chip = Obj::plain(root)
            .pos(0, REC_CHIP_Y)
            .size(SCREEN_W, Font::Small.line_height())
            .row(REC_DOT_GAP);
        let chip_dot = Obj::plain(chip).size(REC_DOT, REC_DOT).round().bg(REC);
        let chip_text = Obj::label(chip, "Recording", Font::Small, REC);
        let timer = Obj::label(root, "", Font::Num48, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, REC_TIMER_Y);
        let line = Obj::label(root, "", Font::Small, TEXT_2)
            .centered_text(RING_TEXT_W)
            .pos(DIAL_CX - RING_TEXT_W / 2, REC_LINE_Y);
        let hint = Obj::label(root, "", Font::Micro, TEXT_3)
            .centered_text(SCREEN_W)
            .pos(0, REC_HINT_Y);
        let live = Rc::new(Cell::new(Live::default()));

        let mark = {
            let (dispatch, live) = (ctx.dispatch.clone(), live.clone());
            RoundButton::new(
                root,
                BottomSpot::Left,
                icons::MARK,
                "Mark",
                Tone::Neutral,
                move || {
                    let now = live.get();
                    if !now.paused {
                        dispatch.command(Command::AddBookmark);
                        dispatch.show(Severity::Info, copy::marked_at(now.duration_secs));
                    }
                },
            )
        };
        let stop = {
            let (tap, done, live) = (ctx.dispatch.clone(), ctx.dispatch.clone(), live.clone());
            RoundButton::stop(
                root,
                move || tap.show(Severity::Info, copy::HOLD_STOP),
                move || {
                    done.command(Command::StopRecording);
                    // Also replaces "Keep holding to stop" before the screen changes.
                    let message = copy::after_stop(live.get().upload_mode);
                    done.show(message.severity, message.text);
                },
            )
        };
        {
            let (dispatch, obj) = (ctx.dispatch.clone(), stop.obj());
            on(obj, lv_event_code_t_LV_EVENT_PRESSED, move |_| {
                if obj.is_clickable() {
                    dispatch.show(Severity::Info, copy::KEEP_HOLDING);
                }
            });
        }
        let pause = {
            let (dispatch, live) = (ctx.dispatch.clone(), live.clone());
            RoundButton::new(
                root,
                BottomSpot::Right,
                icons::PAUSE,
                "Pause",
                Tone::Neutral,
                move || {
                    dispatch.command(if live.get().paused {
                        Command::ResumeRecording
                    } else {
                        Command::PauseRecording
                    });
                },
            )
        };
        self.parts = Some(Parts {
            status,
            dial,
            chip,
            chip_dot,
            chip_text,
            timer,
            line,
            hint,
            mark,
            stop,
            pause,
            live,
            shown: None,
            stack_hidden: false,
        });
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        p.status.update(snap);
        p.live.set(Live {
            paused: snap.meeting.paused,
            duration_secs: snap.meeting.duration_secs,
            upload_mode: snap.settings.upload_mode,
        });
        let gauge = p.dial.update(snap, recording_ring(snap));
        if gauge != p.stack_hidden {
            for obj in [p.chip, p.timer, p.line, p.hint] {
                obj.hidden(gauge);
            }
            p.stack_hidden = gauge;
        }
        let view = RecordingView::from_snapshot(snap);
        if p.shown.as_ref() == Some(&view) {
            return;
        }
        let paused_changed = p
            .shown
            .as_ref()
            .is_none_or(|shown| shown.paused != view.paused);
        p.chip_dot.hidden(!view.chip_dot);
        p.chip_text.text_color(view.chip_color).set_text(view.chip);
        p.timer
            .font(view.timer_font)
            .centered_text(SCREEN_W)
            .pos(0, view.timer_y)
            .set_text(&view.timer);
        p.line.text_color(view.line_color).set_text(&view.line);
        p.hint.set_text(view.hint);
        if paused_changed {
            p.mark.set_enabled(!view.paused);
            if view.paused {
                p.pause.set_icon(icons::PLAY);
                p.pause.set_label("Resume");
                p.pause.set_accent(Tone::WarnText);
            } else {
                p.pause.set_icon(icons::PAUSE);
                p.pause.set_label("Pause");
                p.pause.set_accent(Tone::Neutral);
            }
        }
        p.shown = Some(view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::{lv_event_code_t_LV_EVENT_PRESSED, LV_STATE_PRESSED};
    use crate::kit::components::dial::RingStyle;
    use crate::kit::dispatch::{Message, UiEvent};
    use crate::kit::obj::text_width;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::kit::tokens::HOLD_TO_STOP_MS;
    use crate::screens_v2::main_parts::fixtures::{ctx, events, sent, snap, with_message};
    use crate::screens_v2::main_parts::TIMER_HOURS_Y;

    /// recording.png: speaker 2 talking at 60°.
    fn recording(secs: u32) -> UiSnapshot {
        let mut snap = snap();
        snap.meeting.recording = true;
        snap.meeting.duration_secs = secs;
        snap.meeting.current_speaker = Some(2);
        snap.audio.doa_active = true;
        snap.audio.doa_horizontal = 60.0;
        snap
    }

    fn show(severity: Severity, text: &str) -> UiEvent {
        UiEvent::Show(Message::new(severity, text))
    }

    #[test]
    fn recording_view_follows_the_dial_table() {
        let mut snap = recording(2_537);
        let view = RecordingView::from_snapshot(&snap);
        assert_eq!(
            (
                view.chip,
                view.chip_color,
                view.chip_dot,
                view.timer.as_str()
            ),
            ("Recording", REC, true, "42:17")
        );
        assert_eq!(
            (view.line.as_str(), view.hint),
            ("Speaker 2 talking", "Tap to mute")
        );
        snap.meeting.current_speaker = None;
        assert_eq!(RecordingView::from_snapshot(&snap).line, "Listening");
        snap.meeting.paused = true;
        let view = RecordingView::from_snapshot(&snap);
        assert_eq!(
            (
                view.chip,
                view.chip_color,
                view.chip_dot,
                view.line.as_str()
            ),
            ("Paused", WARN, false, "Not recording")
        );
        snap.audio.mic_muted = true;
        let view = RecordingView::from_snapshot(&snap);
        assert_eq!(
            (view.line.as_str(), view.line_color, view.hint),
            ("Muted, not recorded", REC, "Tap to unmute")
        );
        assert_eq!(recording_ring(&snap).style, RingStyle::RecordingMuted);
    }

    #[test]
    fn recording_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            RecordingScreen::default().build(root, &ctx, &recording(2_537));
            assert_golden("recording", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn recording_muted_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = recording(2_702);
            snap.clock = "14:35".into();
            snap.audio.mic_muted = true;
            RecordingScreen::default().build(root, &ctx, &snap);
            assert_golden("recording_muted", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn holding_stop_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, ui) = ctx();
            let mut snap = recording(2_538);
            let mut screen = RecordingScreen::default();
            screen.build(root, &ctx, &snap);
            let stop = screen.parts.as_ref().unwrap().stop.obj();
            stop.set_state(LV_STATE_PRESSED, true);
            lv.send(stop, lv_event_code_t_LV_EVENT_PRESSED);
            lv.advance(620);
            assert_eq!(events(&ui), [show(Severity::Info, copy::KEEP_HOLDING)]);
            with_message(&mut snap, Severity::Info, copy::KEEP_HOLDING);
            screen.update(&snap);
            assert_golden("recording_hold_stop", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn paused_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = recording(2_537);
            snap.clock = "14:40".into();
            snap.battery.level = 81;
            snap.meeting.paused = true;
            let mut screen = RecordingScreen::default();
            screen.build(root, &ctx, &snap);
            let parts = screen.parts.as_ref().unwrap();
            assert!(!parts.mark.obj().is_clickable(), "Mark is off while paused");
            assert_golden("recording_paused", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn mark_pause_and_stop_send_their_commands() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut snap = recording(2_537);
            let mut screen = RecordingScreen::default();
            screen.build(root, &ctx, &snap);
            let (mark, pause, stop) = {
                let p = screen.parts.as_ref().unwrap();
                (p.mark.obj(), p.pause.obj(), p.stop.obj())
            };
            lv.click(mark);
            lv.click(pause);
            lv.click(stop); // a short tap
            assert_eq!(sent(&mut bus), ["AddBookmark", "PauseRecording"]);
            assert_eq!(
                events(&ui),
                [
                    show(Severity::Info, "Marked at 42:17"),
                    show(Severity::Info, copy::KEEP_HOLDING),
                    show(Severity::Info, copy::HOLD_STOP),
                ]
            );
            snap.meeting.paused = true;
            screen.update(&snap);
            lv.click(mark);
            lv.click(pause);
            assert_eq!(
                sent(&mut bus),
                ["ResumeRecording"],
                "Mark does nothing while paused"
            );
            lv.send(stop, lv_event_code_t_LV_EVENT_PRESSED);
            lv.advance(HOLD_TO_STOP_MS as u32 + 100);
            assert_eq!(sent(&mut bus), ["StopRecording"]);
            assert_eq!(
                events(&ui).last(),
                Some(&UiEvent::Show(copy::after_stop(UploadMode::Automatic)))
            );
        });
    }

    #[test]
    fn the_timer_past_an_hour_fits_inside_the_ring() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut screen = RecordingScreen::default();
            let snap = recording(3_723);
            screen.build(root, &ctx, &snap);
            let view = RecordingView::from_snapshot(&snap);
            assert_eq!(
                (view.timer.as_str(), view.timer_font),
                ("1:02:03", Font::Num26)
            );
            assert_eq!(view.timer_y, TIMER_HOURS_Y);
            let timer = screen.parts.as_ref().unwrap().timer;
            timer.update_layout();
            assert_eq!(timer.text(), "1:02:03");
            // The built label uses Num26: its box is one Num26 line tall.
            let area = timer.coords();
            assert_eq!(area.y2 - area.y1 + 1, Font::Num26.line_height());
            assert_eq!(area.y1, TIMER_HOURS_Y);
            let width = text_width(&timer.text(), view.timer_font);
            assert!(width <= RING_TEXT_W, "1:02:03 is {width} px");
            // Every timer from an hour to a day stays inside the ring.
            for secs in [3_600, 36_000, 86_399] {
                let (font, _) = timer_font(secs);
                let text = format_timer(secs);
                let width = text_width(&text, font);
                assert!(width <= RING_TEXT_W, "{text} is {width} px");
            }
            assert_golden("recording_hours", &lv.render());
            assert_targets_ok(root);
        });
    }
}
