//! Volume and Mic level (B4, volume.png): a native arc with − / + steps of 10
//! and a mute button in the middle. Swipe left on Volume for the microphone
//! level, which uses the same layout. `BigNumber` and `step_button` are shared
//! with Display and Date and time.

use crate::bindings::LV_STATE_PRESSED;
use crate::kit::components::buttons::{back_button, RoundButton, Tone};
use crate::kit::components::controls::ValueArc;
use crate::kit::components::status::StatusArea;
use crate::kit::event::{on_click, on_swipe, Swipe};
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{text_width, Obj};
use crate::kit::route::Route;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    Rect, MIN_TARGET, RAISED, REC, ROUND_LABEL_W, SCREEN_W, SURFACE, TEXT, TEXT_2, TEXT_3,
};
use std::cell::Cell;
use std::rc::Rc;
use waved_state::Command;

/// The on-device mic range: 0–200 % of unity (unity at the arc's middle). The
/// host may set more (up to 800 %); that value is shown and kept.
pub const MIC_MAX: u16 = 200;
const ARC_CX: i32 = 200;
const ARC_CY: i32 = 132;
const ARC_R: i32 = 88;
const CAPTION_Y: i32 = 80;
const VALUE_TOP: i32 = 96;
const CENTRE: Rect = Rect::new(180, 164, 40, 40);
const MINUS: Rect = Rect::new(30, 106, 52, 52);
const PLUS: Rect = Rect::new(318, 106, 52, 52);

/// Big digits are placed by baseline (Global Constraints, "Type"): a Num48
/// label's y is the mockup's CSS top + 11, as `tokens::REC_TIMER_Y` = 93 + 11.
const NUM48_DROP: i32 = 11;
/// A Num20 suffix shares the Num48 baseline: their baselines sit 34 and 19 px
/// below their label tops, so the suffix label is 15 px lower.
const SUFFIX_DROP: i32 = 15;

/// The next multiple of 10 up (at most `max`, but a value already past `max`
/// stays), or the previous one down.
pub fn step_level(volume: u16, up: bool, max: u16) -> u16 {
    if up {
        if volume >= max {
            volume
        } else {
            ((volume / 10 + 1) * 10).min(max)
        }
    } else {
        volume.saturating_sub(1) / 10 * 10
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Speaker,
    Mic,
}

impl Level {
    fn read(self, snap: &UiSnapshot) -> (u16, bool) {
        match self {
            Level::Speaker => (snap.audio.speaker_volume.min(100), snap.audio.speaker_muted),
            Level::Mic => (snap.audio.mic_volume, snap.audio.mic_muted),
        }
    }

    fn caption(self, muted: bool) -> &'static str {
        match (self, muted) {
            (Level::Speaker, false) => "Speaker",
            (Level::Speaker, true) => "Speaker muted",
            (Level::Mic, false) => "Microphone",
            (Level::Mic, true) => "Microphone muted",
        }
    }

    fn icon(self, muted: bool) -> &'static str {
        match (self, muted) {
            (Level::Speaker, false) => icons::VOLUME_HIGH,
            (Level::Speaker, true) => icons::VOLUME_OFF,
            (Level::Mic, false) => icons::MIC,
            (Level::Mic, true) => icons::MIC_OFF,
        }
    }

    /// Where `volume` sits on the 0–100 arc. A mic level past `MIC_MAX` (set
    /// by the host) pins the arc at its end.
    fn arc_value(self, volume: u16) -> u8 {
        match self {
            Level::Speaker => volume.min(100) as u8,
            Level::Mic => (volume.min(MIC_MAX) / 2) as u8,
        }
    }

    fn set(self, value: u16) -> Command {
        match self {
            Level::Speaker => Command::SetSpeakerVolume(value),
            Level::Mic => Command::SetMicVolume(value),
        }
    }

    fn arc_command(self, percent: u8) -> Command {
        match self {
            Level::Speaker => self.set(u16::from(percent)),
            Level::Mic => self.set(u16::from(percent) * 2),
        }
    }

    /// Commands for a − / + tap. Raising the speaker also unmutes it.
    fn step(self, volume: u16, muted: bool, up: bool) -> Vec<Command> {
        match self {
            Level::Speaker => {
                let mut out = vec![self.set(step_level(volume, up, 100))];
                if up && muted {
                    out.push(Command::ToggleSpeakerMute);
                }
                out
            }
            Level::Mic => vec![self.set(step_level(volume, up, MIC_MAX))],
        }
    }

    /// Commands for the centre button. Unmuting a speaker at 0 % sets 10 %
    /// first, so unmuting is audible.
    fn toggle(self, volume: u16, muted: bool) -> Vec<Command> {
        match self {
            Level::Speaker if muted && volume == 0 => {
                vec![Command::SetSpeakerVolume(10), Command::ToggleSpeakerMute]
            }
            Level::Speaker => vec![Command::ToggleSpeakerMute],
            Level::Mic => vec![Command::ToggleMicMute],
        }
    }
}

/// A big Num48 value with a small Num20 suffix on its baseline, centred on
/// x 200 ("60%", "2:37 pm"). `top` is the mockup's CSS top of the 48 px line.
pub(crate) struct BigNumber {
    digits: Obj,
    suffix: Obj,
    top: i32,
}

impl BigNumber {
    pub(crate) fn new(parent: Obj, top: i32) -> Self {
        Self {
            digits: Obj::label(parent, "", Font::Num48, TEXT),
            suffix: Obj::label(parent, "", Font::Num20, TEXT_2),
            top,
        }
    }

    /// Show `digits` in `hex` and `suffix` (Text 2) `gap` px after them; the
    /// pair is centred as one. An empty suffix takes no room.
    pub(crate) fn set(&self, digits: &str, suffix: &str, gap: i32, hex: u32) {
        let digits_w = text_width(digits, Font::Num48);
        let suffix_w = if suffix.is_empty() {
            0
        } else {
            gap + text_width(suffix, Font::Num20)
        };
        let x = SCREEN_W / 2 - (digits_w + suffix_w) / 2;
        let y = self.top + NUM48_DROP;
        self.digits.text_color(hex).pos(x, y).set_text(digits);
        self.suffix
            .pos(x + digits_w + gap, y + SUFFIX_DROP)
            .set_text(suffix);
    }

    #[cfg(test)]
    pub(crate) fn digits(&self) -> Obj {
        self.digits
    }

    #[cfg(test)]
    pub(crate) fn suffix_text(&self) -> String {
        self.suffix.text()
    }
}

/// A − / + step circle: the kit's round button (Surface, Raised with a 2 px
/// Live ring while pressed) filling `rect`, with a 20 px Text icon (the
/// mockup draws the − / + at about 12 px; the 26 px glyph is too big) and no
/// label. Taps only, no repeat. Its parent must be the screen root (the
/// round button's empty label is a sibling, see `RoundButton`).
pub(crate) fn step_button(
    parent: Obj,
    rect: Rect,
    icon: &str,
    on_tap: impl FnMut() + 'static,
) -> Obj {
    let label = (rect.x + rect.w / 2 - ROUND_LABEL_W / 2, rect.y + rect.h);
    RoundButton::at(parent, rect, label, icon, "", Tone::Neutral, on_tap)
        .icon_font(Font::Icon20)
        .obj()
}

pub struct VolumeScreen {
    level: Level,
    status: Option<StatusArea>,
    arc: Option<ValueArc>,
    value: Option<BigNumber>,
    caption: Option<Obj>,
    centre_icon: Option<Obj>,
    /// The mute disc and the − / + circles; only the tests look at them again.
    #[cfg(test)]
    centre: Option<Obj>,
    #[cfg(test)]
    minus: Option<Obj>,
    #[cfg(test)]
    plus: Option<Obj>,
    /// The latest (volume, muted), for the button handlers.
    current: Rc<Cell<(u16, bool)>>,
    shown: Option<(u16, bool)>,
}

impl VolumeScreen {
    pub fn speaker() -> Self {
        Self::new(Level::Speaker)
    }

    pub fn mic() -> Self {
        Self::new(Level::Mic)
    }

    fn new(level: Level) -> Self {
        Self {
            level,
            status: None,
            arc: None,
            value: None,
            caption: None,
            centre_icon: None,
            #[cfg(test)]
            centre: None,
            #[cfg(test)]
            minus: None,
            #[cfg(test)]
            plus: None,
            current: Rc::new(Cell::new((0, false))),
            shown: None,
        }
    }
}

impl Screen for VolumeScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        let level = self.level;
        self.current.set(level.read(snap));
        self.status = Some(StatusArea::plain(root));
        let dispatch = ctx.dispatch.clone();
        self.arc = Some(ValueArc::new(root, ARC_CX, ARC_CY, ARC_R, move |percent| {
            dispatch.command(level.arc_command(percent))
        }));
        self.caption = Some(
            Obj::label(root, "", Font::Small, TEXT_2)
                .centered_text(SCREEN_W)
                .pos(0, CAPTION_Y),
        );
        self.value = Some(BigNumber::new(root, VALUE_TOP));
        let centre = Obj::plain(root)
            .rect(CENTRE)
            .round()
            .bg(SURFACE)
            .pressed_bg(RAISED)
            .clickable(true)
            .ext_click((MIN_TARGET - CENTRE.w) / 2);
        self.centre_icon = Some(Obj::label(centre, "", Font::Icon20, TEXT).center());
        let (dispatch, current) = (ctx.dispatch.clone(), self.current.clone());
        on_click(centre, move || {
            let (volume, muted) = current.get();
            level
                .toggle(volume, muted)
                .into_iter()
                .for_each(|c| dispatch.command(c));
        });
        #[cfg(test)]
        {
            self.centre = Some(centre);
        }
        for (rect, icon, up) in [(MINUS, icons::MINUS, false), (PLUS, icons::PLUS, true)] {
            let (dispatch, current) = (ctx.dispatch.clone(), self.current.clone());
            let button = step_button(root, rect, icon, move || {
                let (volume, muted) = current.get();
                level
                    .step(volume, muted, up)
                    .into_iter()
                    .for_each(|c| dispatch.command(c));
            });
            #[cfg(test)]
            if up {
                self.plus = Some(button);
            } else {
                self.minus = Some(button);
            }
            #[cfg(not(test))]
            let _ = button;
        }
        if level == Level::Speaker {
            let dispatch = ctx.dispatch.clone();
            on_swipe(root, Swipe::Left, move || {
                dispatch.navigate(Route::MicLevel)
            });
        }
        back_button(root, &ctx.dispatch);
        self.update(snap);
        root.update_layout();
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let level = self.level;
        let (volume, muted) = level.read(snap);
        self.current.set((volume, muted));
        // The arc follows the snapshot unless a finger is on it.
        if let Some(arc) = self.arc.as_ref() {
            let target = level.arc_value(volume);
            if !arc.obj().has_state(LV_STATE_PRESSED) && arc.value() != target {
                arc.set_value(target);
            }
        }
        if self.shown == Some((volume, muted)) {
            return;
        }
        if let (Some(value), Some(caption), Some(icon)) =
            (self.value.as_ref(), self.caption, self.centre_icon)
        {
            value.set(
                &volume.to_string(),
                "%",
                0,
                if muted { TEXT_3 } else { TEXT },
            );
            caption
                .text_color(if muted { REC } else { TEXT_2 })
                .set_text(level.caption(muted));
            icon.text_color(if muted { REC } else { TEXT })
                .set_text(level.icon(muted));
        }
        self.shown = Some((volume, muted));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::LV_STATE_PRESSED;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::event::Swipe;
    use crate::kit::route::Route;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    #[test]
    fn volume_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.audio.speaker_volume = 60;
            let mut screen = VolumeScreen::speaker();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_volume", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.value.as_ref().unwrap().digits().text(), "60");
        });
    }

    #[test]
    fn mic_level_uses_the_same_layout() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.audio.mic_volume = 100;
            let mut screen = VolumeScreen::mic();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_mic_level", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.arc.as_ref().unwrap().value(), 50); // unity sits mid-arc
            assert_eq!(screen.caption.unwrap().text(), "Microphone");
        });
    }

    #[test]
    fn steps_round_to_tens_and_plus_unmutes_the_speaker() {
        assert_eq!(step_level(60, true, 100), 70);
        assert_eq!(step_level(65, true, 100), 70);
        assert_eq!(step_level(100, true, 100), 100);
        assert_eq!(step_level(60, false, 100), 50);
        assert_eq!(step_level(65, false, 100), 60);
        assert_eq!(step_level(5, false, 100), 0);
        assert_eq!(step_level(0, false, 100), 0);
        // Mic gain set above the on-device range by the host is kept, and stepped down from.
        assert_eq!(step_level(250, true, MIC_MAX), 250);
        assert_eq!(step_level(195, true, MIC_MAX), 200);
        assert_eq!(step_level(250, false, MIC_MAX), 240);
        assert!(matches!(
            Level::Speaker.step(60, true, true).as_slice(),
            [Command::SetSpeakerVolume(70), Command::ToggleSpeakerMute]
        ));
        assert!(matches!(
            Level::Speaker.toggle(0, true).as_slice(),
            [Command::SetSpeakerVolume(10), Command::ToggleSpeakerMute]
        ));
        assert!(matches!(
            Level::Mic.toggle(100, false).as_slice(),
            [Command::ToggleMicMute]
        ));
    }

    #[test]
    fn buttons_send_the_steps() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.audio.speaker_volume = 60;
            let mut screen = VolumeScreen::speaker();
            screen.build(root, &f.ctx, &snap);
            assert_targets_ok(root);
            lv.click(screen.plus.unwrap());
            lv.click(screen.minus.unwrap());
            lv.click(screen.centre.unwrap());
            assert!(matches!(
                f.commands().as_slice(),
                [
                    Command::SetSpeakerVolume(70),
                    Command::SetSpeakerVolume(50),
                    Command::ToggleSpeakerMute
                ]
            ));
        });
    }

    #[test]
    fn a_host_set_mic_level_past_the_arc_is_shown_and_stepped() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.audio.mic_volume = 400;
            let mut screen = VolumeScreen::mic();
            screen.build(root, &f.ctx, &snap);
            assert_targets_ok(root);
            assert_eq!(screen.value.as_ref().unwrap().digits().text(), "400");
            assert_eq!(screen.arc.as_ref().unwrap().value(), 100); // the arc's end
            lv.click(screen.plus.unwrap());
            lv.click(screen.minus.unwrap());
            // + keeps the host's 400 % (step_level never raises past MIC_MAX,
            // nor lowers to it); − steps down by ten from there.
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetMicVolume(400), Command::SetMicVolume(390)]
            ));
        });
    }

    #[test]
    fn the_arc_ignores_snapshots_while_pressed() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.audio.speaker_volume = 60;
            let mut screen = VolumeScreen::speaker();
            screen.build(root, &f.ctx, &snap);
            assert_targets_ok(root);
            let arc = screen.arc.as_ref().unwrap().obj();
            arc.set_state(LV_STATE_PRESSED, true);
            snap.audio.speaker_volume = 30;
            screen.update(&snap);
            assert_eq!(screen.arc.as_ref().unwrap().value(), 60);
            arc.set_state(LV_STATE_PRESSED, false);
            screen.update(&snap);
            assert_eq!(screen.arc.as_ref().unwrap().value(), 30);
        });
    }

    #[test]
    fn swiping_left_opens_mic_level() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = VolumeScreen::speaker();
            screen.build(root, &f.ctx, &snap());
            assert_targets_ok(root);
            lv.swipe(root, Swipe::Left);
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::MicLevel)]);
        });
    }

    #[test]
    fn a_swipe_that_starts_on_the_arc_stays_on_volume() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = VolumeScreen::speaker();
            screen.build(root, &f.ctx, &snap());
            assert_targets_ok(root);
            // The arc keeps its drags, so a sideways drag on it is a value change.
            lv.swipe(screen.arc.as_ref().unwrap().obj(), Swipe::Left);
            assert_eq!(f.events(), vec![]);
        });
    }

    #[test]
    fn mic_level_takes_no_left_swipe() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = VolumeScreen::mic();
            screen.build(root, &f.ctx, &snap());
            assert_targets_ok(root);
            lv.swipe(root, Swipe::Left);
            assert_eq!(f.events(), vec![]);
        });
    }

    #[test]
    fn muted_turns_the_caption_and_icon_red() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.audio.speaker_muted = true;
            let mut screen = VolumeScreen::speaker();
            screen.build(root, &f.ctx, &snap);
            assert_targets_ok(root);
            assert_eq!(screen.caption.unwrap().text(), "Speaker muted");
            assert_eq!(
                screen.centre_icon.unwrap().text(),
                crate::kit::icons::VOLUME_OFF
            );
        });
    }
}
