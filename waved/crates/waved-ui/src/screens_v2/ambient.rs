//! Resting screen (A4, docs/mockups/png/ambient.png): after "Dim after"
//! without a touch, Home shrinks to a clock, the mic state and the
//! pre-recording chip; while recording it shows the ring and the timer. The
//! backlight level and the off timer are E1's; the first-touch wake guard is
//! the runtime's, so nothing here is tappable.

use crate::kit::components::dial::{Dial, RingStyle};
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    DIAL_CX, DIAL_CY, DIAL_R, LIVE, REC, REC_DOT, REC_DOT_GAP, REC_TIMER_Y, SCREEN_W, TEXT, TEXT_2,
    WARN,
};
use crate::screens_v2::main_parts::{format_timer, timer_font};

/// ambient.png: the ring is drawn at stroke 2.
const RING_WIDTH: i32 = 2;
/// The mic line, CSS top 145; the ring's inner chord there is ~115 px.
const LINE_Y: i32 = 145;
const LINE_W: i32 = 112;
/// The pre-recording chip, CSS top 236.
const CHIP_Y: i32 = 236;

#[derive(Clone, Debug, PartialEq)]
pub struct AmbientView {
    pub ring: RingStyle,
    pub value: String,
    pub value_font: Font,
    pub value_y: i32,
    pub line: &'static str,
    pub line_color: u32,
    pub pre_recording: bool,
}

pub fn ambient_view(snap: &UiSnapshot) -> AmbientView {
    let (audio, meeting) = (&snap.audio, &snap.meeting);
    if meeting.recording {
        let (value_font, value_y) = timer_font(meeting.duration_secs);
        let (ring, line, line_color) = if audio.mic_muted {
            (RingStyle::RecordingMuted, "Muted, not recorded", REC)
        } else if meeting.paused {
            (RingStyle::Paused, "Paused", WARN)
        } else {
            (RingStyle::Recording, "Recording", REC)
        };
        AmbientView {
            ring,
            value: format_timer(meeting.duration_secs),
            value_font,
            value_y,
            line,
            line_color,
            pre_recording: false,
        }
    } else {
        let (ring, line, line_color) = if audio.mic_muted {
            (RingStyle::Muted, "Mic off", REC)
        } else if snap.in_call() {
            (RingStyle::Live, "Mic on, in a call", LIVE)
        } else if audio.host_listening {
            (RingStyle::Live, "Computer is listening", LIVE)
        } else {
            (RingStyle::Idle, "Mic on", TEXT_2)
        };
        AmbientView {
            ring,
            // `snap.clock` is already "--:--" before the wall clock is valid.
            value: resting_clock(&snap.clock).to_string(),
            value_font: Font::Num48,
            value_y: REC_TIMER_Y,
            line,
            line_color,
            pre_recording: meeting.pre_recording_active,
        }
    }
}

/// The clock without its 12-hour suffix: "12:32 pm" would cross the ring.
pub fn resting_clock(clock: &str) -> &str {
    clock
        .strip_suffix(" am")
        .or_else(|| clock.strip_suffix(" pm"))
        .unwrap_or(clock)
}

#[derive(Default)]
pub struct AmbientScreen {
    parts: Option<Parts>,
}

struct Parts {
    dial: Dial,
    value: Obj,
    line: Obj,
    chip: Obj,
    shown: Option<AmbientView>,
}

impl Screen for AmbientScreen {
    fn build(&mut self, root: Obj, _ctx: &ScreenCtx, snap: &UiSnapshot) {
        let dial = Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, RING_WIDTH);
        let value = Obj::label(root, "", Font::Num48, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, REC_TIMER_Y);
        let line = Obj::label(root, "", Font::Small, TEXT_2)
            .centered_text(LINE_W)
            .pos(DIAL_CX - LINE_W / 2, LINE_Y);
        let chip = Obj::plain(root)
            .pos(0, CHIP_Y)
            .size(SCREEN_W, Font::Micro.line_height())
            .row(REC_DOT_GAP)
            .hidden(true);
        Obj::plain(chip).size(REC_DOT, REC_DOT).round().bg(LIVE);
        Obj::label(chip, "Pre-recording", Font::Micro, LIVE);
        self.parts = Some(Parts {
            dial,
            value,
            line,
            chip,
            shown: None,
        });
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        let view = ambient_view(snap);
        if p.shown.as_ref() == Some(&view) {
            return;
        }
        if p.shown.as_ref().is_none_or(|shown| shown.ring != view.ring) {
            p.dial.set_ring(view.ring);
        }
        p.value
            .font(view.value_font)
            .centered_text(SCREEN_W)
            .pos(0, view.value_y)
            .set_text(&view.value);
        p.line.text_color(view.line_color).set_text(view.line);
        p.chip.hidden(!view.pre_recording);
        p.shown = Some(view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::obj::text_width;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::main_parts::fixtures::{ctx, snap};
    use crate::screens_v2::main_parts::RING_TEXT_W;

    fn clickable(obj: Obj) -> u32 {
        (0..obj.child_count() as i32)
            .map(|i| {
                let child = obj.child(i);
                u32::from(child.is_clickable()) + clickable(child)
            })
            .sum()
    }

    #[test]
    fn ambient_view_shows_the_mic_state_and_the_recording() {
        let mut snap = snap();
        snap.meeting.pre_recording_active = true;
        let view = ambient_view(&snap);
        assert_eq!(
            (
                view.ring,
                view.value.as_str(),
                view.line,
                view.line_color,
                view.pre_recording
            ),
            (RingStyle::Idle, "14:32", "Mic on", TEXT_2, true)
        );
        snap.audio.host_listening = true;
        let view = ambient_view(&snap);
        assert_eq!(
            (view.ring, view.line, view.line_color),
            (RingStyle::Live, "Computer is listening", LIVE)
        );
        snap.audio.call_off_hook = true;
        let view = ambient_view(&snap);
        assert_eq!(
            (view.ring, view.line, view.line_color),
            (RingStyle::Live, "Mic on, in a call", LIVE)
        );
        snap.audio.mic_muted = true;
        let view = ambient_view(&snap);
        assert_eq!(
            (view.ring, view.line, view.line_color),
            (RingStyle::Muted, "Mic off", REC)
        );
        let mut rec = recording();
        let view = ambient_view(&rec);
        assert_eq!(
            (
                view.ring,
                view.value.as_str(),
                view.line,
                view.pre_recording
            ),
            (RingStyle::Recording, "42:17", "Recording", false)
        );
        rec.meeting.paused = true;
        let view = ambient_view(&rec);
        assert_eq!(
            (view.ring, view.line, view.line_color),
            (RingStyle::Paused, "Paused", WARN)
        );
        rec.audio.mic_muted = true;
        let view = ambient_view(&rec);
        assert_eq!(
            (view.ring, view.line),
            (RingStyle::RecordingMuted, "Muted, not recorded")
        );
        assert_eq!(resting_clock("2:32 pm"), "2:32");
        assert_eq!(resting_clock("12:05 am"), "12:05");
        assert_eq!(resting_clock("14:32"), "14:32");
        assert_eq!(resting_clock("--:--"), "--:--");
    }

    fn recording() -> UiSnapshot {
        let mut snap = snap();
        snap.meeting.recording = true;
        snap.meeting.duration_secs = 2_537;
        snap
    }

    /// Every snapshot the view table distinguishes, with the widest clocks and timers.
    fn cases() -> Vec<UiSnapshot> {
        let mut cases = Vec::new();
        for clock in ["14:32", "12:59", "12:59 pm", "11:11 am", "--:--"] {
            let mut s = snap();
            s.clock = clock.into();
            cases.push(s.clone()); // Mic on
            s.meeting.pre_recording_active = true;
            cases.push(s.clone()); // Mic on, pre-recording
            s.audio.host_listening = true;
            cases.push(s.clone()); // Computer is listening
            s.audio.call_off_hook = true;
            cases.push(s.clone()); // Mic on, in a call
            s.audio.mic_muted = true;
            cases.push(s); // Mic off
        }
        for secs in [0, 59, 2_537, 3_599, 3_600, 3_723, 35_999, 359_999] {
            let mut s = recording();
            s.meeting.duration_secs = secs;
            cases.push(s.clone()); // Recording
            s.meeting.paused = true;
            cases.push(s.clone()); // Paused
            s.audio.mic_muted = true;
            cases.push(s); // Muted, not recorded
        }
        cases
    }

    #[test]
    fn every_resting_line_fits_inside_the_ring() {
        lvgl(|_| {
            let mut lines = std::collections::BTreeSet::new();
            for case in cases() {
                let view = ambient_view(&case);
                let width = text_width(view.line, Font::Small);
                assert!(width <= LINE_W, "{:?} is {width} px", view.line);
                let width = text_width(&view.value, view.value_font);
                assert!(width <= RING_TEXT_W, "{:?} is {width} px", view.value);
                lines.insert(view.line);
            }
            assert_eq!(lines.len(), 7, "the cases cover every mic line: {lines:?}");
        });
    }

    #[test]
    fn ambient_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = snap();
            snap.audio.call_off_hook = true;
            snap.meeting.pre_recording_active = true;
            AmbientScreen::default().build(root, &ctx, &snap);
            assert_golden("ambient", &lv.render());
            assert_targets_ok(root);
            assert_eq!(clickable(root), 0, "the first touch only wakes");
        });
    }

    #[test]
    fn the_recording_variant_shows_the_ring_and_timer() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut screen = AmbientScreen::default();
            screen.build(root, &ctx, &recording());
            let parts = screen.parts.as_ref().unwrap();
            parts.value.update_layout(); // a one-line label reads "..." until its first layout
            assert_eq!(parts.value.text(), "42:17");
            assert!(parts.chip.is_hidden());
            assert_golden("ambient_recording", &lv.render());
            assert_targets_ok(root);
            assert_eq!(clickable(root), 0);
        });
    }
}
