//! Audio mode (B2, audio-mode.png): six modes around a live preview.
//! - Focused, Narrow and Two sides are aimed by dragging the shape (`SetBeamAim`, D2).
//! - Follow talker turns with the live direction of arrival.
//! - Stereo keeps the device orientation.
//!
//! Applying the aim is C3/C10's job. Until the audio service reports it in
//! `beam_aim_deg`, the screen keeps showing the aim the user chose.

use crate::kit::components::buttons::back_button;
use crate::kit::components::mode_picker::{AimEvent, Glyph, Lobe, ModePicker, Preview};
use crate::kit::components::status::StatusArea;
use crate::kit::direction::{device_from_screen, screen_from_device};
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::talker::{is_confident, TalkerGate};
use crate::kit::tokens::{SCREEN_W, TEXT, TEXT_2, TITLE_Y};
use crate::screens_v2::menu::{mode_name, Pending};
use std::cell::RefCell;
use std::rc::Rc;
use waved_state::{AudioMode, Command};

const NAME_Y: i32 = 192;
const HINT_Y: i32 = 217;
/// The preview follows the talker in 5° steps, so it doesn't redraw on noise.
const DOA_STEP: f32 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeChoice {
    WholeRoom,
    FollowTalker,
    Focused,
    Narrow,
    TwoSides,
    Stereo,
}

impl ModeChoice {
    /// Picker order (`mode_picker::ANGLES`): left column top to bottom, then right.
    pub const ALL: [ModeChoice; 6] = [
        ModeChoice::WholeRoom,
        ModeChoice::FollowTalker,
        ModeChoice::Focused,
        ModeChoice::Narrow,
        ModeChoice::TwoSides,
        ModeChoice::Stereo,
    ];

    pub fn of(mode: &AudioMode) -> Self {
        match mode {
            AudioMode::Omni => ModeChoice::WholeRoom,
            AudioMode::Steered { .. } => ModeChoice::FollowTalker,
            AudioMode::Cardioid => ModeChoice::Focused,
            AudioMode::Supercardioid => ModeChoice::Narrow,
            AudioMode::Figure8 => ModeChoice::TwoSides,
            AudioMode::Stereo => ModeChoice::Stereo,
        }
    }

    /// Follow talker starts facing forward until the first confident DoA frame.
    pub fn audio_mode(self) -> AudioMode {
        match self {
            ModeChoice::WholeRoom => AudioMode::Omni,
            ModeChoice::FollowTalker => AudioMode::Steered {
                initial_az_deg: 0.0,
            },
            ModeChoice::Focused => AudioMode::Cardioid,
            ModeChoice::Narrow => AudioMode::Supercardioid,
            ModeChoice::TwoSides => AudioMode::Figure8,
            ModeChoice::Stereo => AudioMode::Stereo,
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    pub fn name(self) -> &'static str {
        mode_name(&self.audio_mode())
    }

    pub fn hint(self) -> &'static str {
        match self {
            ModeChoice::WholeRoom => "Picks up the whole room evenly",
            ModeChoice::FollowTalker => "Turns toward whoever talks",
            ModeChoice::Focused | ModeChoice::Narrow | ModeChoice::TwoSides => {
                "Drag the shape to aim it"
            }
            ModeChoice::Stereo => "Left and right, as Wave sits",
        }
    }

    pub fn aimable(self) -> bool {
        matches!(
            self,
            ModeChoice::Focused | ModeChoice::Narrow | ModeChoice::TwoSides
        )
    }

    /// The button drawing (index.html data-polar on each button).
    pub fn glyph(self) -> Glyph {
        let lobe = |a, r, rot| Lobe { a, r, rot };
        let solid = |lobes| Glyph { lobes, ghost: None };
        match self {
            ModeChoice::WholeRoom => solid(vec![lobe(1.0, 14.0, 0.0)]),
            ModeChoice::FollowTalker => Glyph {
                lobes: vec![lobe(0.5, 13.0, 40.0)],
                ghost: Some(lobe(0.5, 12.0, -50.0)),
            },
            ModeChoice::Focused => solid(vec![lobe(0.5, 15.0, 0.0)]),
            ModeChoice::Narrow => solid(vec![lobe(0.366, 15.0, 0.0)]),
            ModeChoice::TwoSides => solid(vec![lobe(0.0, 14.0, 0.0)]),
            ModeChoice::Stereo => solid(vec![lobe(0.5, 12.0, -45.0), lobe(0.5, 12.0, 45.0)]),
        }
    }

    /// What the preview shows at this aim and talker direction, both in device
    /// degrees; the preview is in screen degrees (`kit::direction`). Angles are
    /// rounded (1° aim, 5° talker), so equal frames compare equal and skip redraws.
    pub fn preview(self, aim_deg: f32, doa_deg: f32) -> Preview {
        let aim = screen_from_device(aim_deg).round().rem_euclid(360.0);
        let doa = ((screen_from_device(doa_deg) / DOA_STEP).round() * DOA_STEP).rem_euclid(360.0);
        let aimed = |a: f32| Preview {
            lobes: vec![(a, aim)],
            handle: Some(aim),
        };
        match self {
            ModeChoice::WholeRoom => Preview {
                lobes: vec![(1.0, 0.0)],
                handle: None,
            },
            ModeChoice::FollowTalker => Preview {
                lobes: vec![(0.5, doa)],
                handle: None,
            },
            ModeChoice::Focused => aimed(0.5),
            ModeChoice::Narrow => aimed(0.366),
            ModeChoice::TwoSides => aimed(0.0),
            ModeChoice::Stereo => Preview {
                lobes: vec![(0.5, -45.0), (0.5, 45.0)],
                handle: None,
            },
        }
    }
}

/// The aim the user is dragging, and the one sent but not yet reported back.
#[derive(Default)]
struct Aim {
    dragging: Option<f32>,
    pending: Option<f32>,
}

fn same_angle(a: f32, b: f32) -> bool {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d) < 0.5
}

#[derive(Default)]
pub struct AudioModeScreen {
    status: Option<StatusArea>,
    picker: Option<ModePicker>,
    name: Option<Obj>,
    hint: Option<Obj>,
    /// A mode tapped but not yet reported by the audio service.
    chosen: Rc<Pending<ModeChoice>>,
    aim: Rc<RefCell<Aim>>,
    /// The last direction that held confidently (device degrees).
    last_doa: f32,
    /// Hysteresis for `last_doa` (`kit::talker`).
    talker: TalkerGate,
    shown: Option<ModeChoice>,
}

impl Screen for AudioModeScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Audio mode", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        let current = ModeChoice::of(&snap.audio.mode);
        let (chosen, select_dispatch) = (self.chosen.clone(), ctx.dispatch.clone());
        let (aim, aim_dispatch) = (self.aim.clone(), ctx.dispatch.clone());
        self.picker = Some(ModePicker::new(
            root,
            ModeChoice::ALL.map(ModeChoice::glyph),
            current.index(),
            move |index| {
                let choice = ModeChoice::ALL[index];
                chosen.set(choice);
                select_dispatch.command(Command::SetAudioMode(choice.audio_mode()));
            },
            move |event| {
                let mut aim = aim.borrow_mut();
                match event {
                    // The picker reports screen degrees; aims are device degrees.
                    AimEvent::Moving(degrees) => aim.dragging = Some(device_from_screen(degrees)),
                    AimEvent::Done(degrees) => {
                        let degrees = device_from_screen(degrees);
                        aim.dragging = None;
                        aim.pending = Some(degrees);
                        aim_dispatch.command(Command::SetBeamAim(degrees));
                    }
                }
            },
        ));
        self.name = Some(
            Obj::label(root, current.name(), Font::Title, TEXT)
                .centered_text(SCREEN_W)
                .pos(0, NAME_Y),
        );
        self.hint = Some(
            Obj::label(root, current.hint(), Font::Small, TEXT_2)
                .centered_text(SCREEN_W)
                .pos(0, HINT_Y),
        );
        back_button(root, &ctx.dispatch);
        // Resolve the labels' widths now: LVGL's dot mode truncates text set on a
        // label whose width is not laid out yet to "...".
        root.update_layout();
        self.last_doa = snap.audio.doa_horizontal;
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let (Some(picker), Some(name), Some(hint)) = (self.picker.as_ref(), self.name, self.hint)
        else {
            return;
        };
        let mode = self.chosen.follow(ModeChoice::of(&snap.audio.mode));
        // Not `doa_active`: the VAD flag reads open in a quiet room.
        let direction =
            is_confident(snap.audio.doa_confidence).then_some(snap.audio.doa_horizontal);
        if let Some(doa) = self.talker.step(direction, snap.now_ms) {
            self.last_doa = doa;
        }
        let aim = {
            let mut aim = self.aim.borrow_mut();
            if self.shown.is_some_and(|shown| shown != mode) {
                // The mode changed under the finger: that drag is over (the
                // picker forgets its last aim in set_aimable below).
                aim.dragging = None;
            }
            if aim
                .pending
                .is_some_and(|p| same_angle(p, snap.audio.beam_aim_deg))
            {
                aim.pending = None;
            }
            aim.dragging
                .or(aim.pending)
                .unwrap_or(snap.audio.beam_aim_deg)
        };
        picker.set_preview(&mode.preview(aim, self.last_doa));
        if self.shown != Some(mode) {
            picker.set_selected(mode.index());
            picker.set_aimable(mode.aimable());
            name.set_text(mode.name());
            hint.set_text(mode.hint());
            self.shown = Some(mode);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::main_parts::fixtures::settle;
    use crate::screens_v2::menu::fixture::{fixture, snap};
    use waved_state::Command;

    fn focused_snap() -> UiSnapshot {
        let mut snap = snap();
        snap.audio.mode = AudioMode::Cardioid;
        snap.audio.beam_aim_deg = 120.0; // screen 300°, as in the mockup
        snap
    }

    #[test]
    fn audio_mode_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = AudioModeScreen::default();
            screen.build(root, &f.ctx, &focused_snap());
            assert_golden("screen_audio_mode", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.name.unwrap().text(), "Focused");
            assert_eq!(screen.hint.unwrap().text(), "Drag the shape to aim it");
        });
    }

    fn follow_snap() -> UiSnapshot {
        let mut snap = snap();
        snap.audio.mode = AudioMode::Steered {
            initial_az_deg: 0.0,
        };
        snap.audio.doa_confidence = 0.8;
        snap.audio.doa_horizontal = 220.0; // screen 40°
        snap
    }

    #[test]
    fn follow_talker_turns_with_the_talker() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = follow_snap();
            let mut screen = AudioModeScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_targets_ok(root);
            let first = lv.render();
            assert_golden("screen_audio_mode_follow", &first);
            assert!(!screen.picker.as_ref().unwrap().aim_area().is_clickable());
            // Losing the talker keeps the last direction; a talker from 200° turns it.
            snap.audio.doa_confidence = 0.1;
            snap.audio.doa_horizontal = 20.0; // screen 200°
            settle(&mut screen, &mut snap);
            assert_eq!(first.pixel(219, 104), lv.render().pixel(219, 104));
            snap.audio.doa_confidence = 0.8;
            settle(&mut screen, &mut snap);
            assert_ne!(first.pixel(219, 104), lv.render().pixel(219, 104));
        });
    }

    #[test]
    fn room_noise_does_not_turn_the_preview() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = follow_snap();
            let mut screen = AudioModeScreen::default();
            screen.build(root, &f.ctx, &snap);
            settle(&mut screen, &mut snap);
            let steady = lv.render();
            // The VAD flag open, the direction jumping, the confidence low.
            snap.audio.doa_active = true;
            snap.audio.doa_confidence = 0.1;
            for deg in [20.0, 100.0, 300.0, 180.0, 60.0, 250.0] {
                snap.audio.doa_horizontal = deg;
                settle(&mut screen, &mut snap);
                assert_eq!(steady.pixel(219, 104), lv.render().pixel(219, 104), "{deg}");
            }
            // A flicker of confidence shorter than the on-delay does not move it either.
            snap.audio.doa_confidence = 0.9;
            snap.audio.doa_horizontal = 20.0;
            snap.now_ms += 10;
            screen.update(&snap);
            snap.audio.doa_confidence = 0.1;
            settle(&mut screen, &mut snap);
            assert_eq!(steady.pixel(219, 104), lv.render().pixel(219, 104));
        });
    }

    #[test]
    fn a_pending_aim_survives_a_stale_snapshot() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let snap = focused_snap();
            let mut screen = AudioModeScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_targets_ok(root);
            let picker = screen.picker.as_ref().unwrap();
            picker.drag_to(250, 134, false);
            picker.drag_to(250, 134, true);
            let commands = f.commands();
            assert!(
                // Screen 90° (right of the dial) is the listener's right: device 270°.
                matches!(commands.as_slice(), [Command::SetBeamAim(d)] if (d - 270.0).abs() < 0.5),
                "{commands:?}"
            );
            // The service has not applied it yet: beam_aim_deg is still 120.
            screen.update(&snap);
            let bright = |[r, g, b]: [u8; 3]| r > 200 && g > 200 && b > 200;
            assert!(bright(lv.render().pixel(250, 134)), "handle stays at 90°");
            // Once the snapshot agrees, the pending aim is dropped.
            let mut applied = snap.clone();
            applied.audio.beam_aim_deg = 270.0;
            screen.update(&applied);
            assert!(screen.aim.borrow().pending.is_none());
        });
    }

    #[test]
    fn the_finger_aim_wins_until_the_mode_changes_under_it() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = focused_snap();
            let mut screen = AudioModeScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_targets_ok(root);
            let bright = |[r, g, b]: [u8; 3]| r > 200 && g > 200 && b > 200;
            screen.picker.as_ref().unwrap().drag_to(250, 134, false);
            // Mid-drag, a snapshot still at 300° doesn't pull the handle away.
            screen.update(&snap);
            assert!(
                bright(lv.render().pixel(250, 134)),
                "handle under the finger"
            );
            // The mode changes elsewhere mid-drag: the drag is over.
            snap.audio.mode = AudioMode::Supercardioid;
            screen.update(&snap);
            assert!(screen.aim.borrow().dragging.is_none());
            let frame = lv.render();
            assert!(
                bright(frame.pixel(157, 109)),
                "handle back at the reported 300°"
            );
            assert!(!bright(frame.pixel(250, 134)));
        });
    }

    #[test]
    fn tapping_a_mode_switches_and_shows_it_at_once() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let snap = focused_snap();
            let mut screen = AudioModeScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_targets_ok(root);
            lv.click(screen.picker.as_ref().unwrap().button(3));
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetAudioMode(AudioMode::Supercardioid)]
            ));
            screen.update(&snap); // still Cardioid in the snapshot
            assert_eq!(screen.name.unwrap().text(), "Narrow");
        });
    }

    #[test]
    fn modes_map_both_ways() {
        for choice in ModeChoice::ALL {
            assert_eq!(ModeChoice::of(&choice.audio_mode()), choice);
            assert_eq!(ModeChoice::ALL[choice.index()], choice);
            assert_eq!(
                choice.name(),
                crate::screens_v2::menu::mode_name(&choice.audio_mode())
            );
        }
        assert!(ModeChoice::Focused.aimable() && ModeChoice::TwoSides.aimable());
        assert!(!ModeChoice::FollowTalker.aimable() && !ModeChoice::Stereo.aimable());
        // Device degrees in, screen degrees out (`kit::direction`).
        assert_eq!(ModeChoice::Focused.preview(120.4, 0.0).handle, Some(300.0));
        assert_eq!(
            ModeChoice::FollowTalker.preview(0.0, 223.0).lobes,
            vec![(0.5, 45.0)]
        );
        // Talking front-left (device 45°) draws at the dial's lower left.
        assert_eq!(
            ModeChoice::FollowTalker.preview(0.0, 45.0).lobes,
            vec![(0.5, 225.0)]
        );
    }
}
