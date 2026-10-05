//! Noise reduction (B3, noise.png). Taller bars remove more noise, and every
//! bar's tap target is its full 44 x 88 column, so the short Off bar is as
//! easy to hit as High.

use crate::kit::components::buttons::back_button;
use crate::kit::components::status::StatusArea;
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, LINE, LIVE, SCREEN_W, SURFACE, TEXT, TEXT_2, TEXT_3, TITLE_Y};
use crate::screens_v2::menu::nr_name;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::{Command, NrLevel};

pub const LEVELS: [NrLevel; 4] = [NrLevel::Off, NrLevel::Low, NrLevel::Med, NrLevel::High];
const BAR_X: [i32; 4] = [94, 150, 206, 262];
const BAR_H: [i32; 4] = [28, 48, 68, 88];
const BAR_W: i32 = 44;
const COLUMN_TOP: i32 = 76;
const COLUMN_H: i32 = 88;
const BAR_RADIUS: i32 = 10;
const LABEL_W: i32 = 56;
const LABEL_Y: i32 = 170;
const NAME_Y: i32 = 192;
const DETAIL_Y: i32 = 217;

pub fn detail(level: &NrLevel) -> &'static str {
    match level {
        NrLevel::Off => "Room sound untouched",
        NrLevel::Low => "Softens fans and hum",
        NrLevel::Med => "Removes most background noise",
        NrLevel::High => "Strongest, voices only",
    }
}

pub fn level_index(level: &NrLevel) -> usize {
    LEVELS.iter().position(|l| l == level).unwrap_or(0)
}

#[derive(Default)]
pub struct NoiseScreen {
    status: Option<StatusArea>,
    /// The tappable 44 x 88 columns; only the tests look at them again.
    #[cfg(test)]
    columns: Vec<Obj>,
    bars: Vec<Obj>,
    labels: Vec<Obj>,
    name: Option<Obj>,
    detail: Option<Obj>,
    /// The level the user tapped, shown until the snapshot reports it.
    chosen: Rc<Cell<Option<usize>>>,
    shown: Option<usize>,
}

impl Screen for NoiseScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Noise reduction", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        for (index, (&x, &h)) in BAR_X.iter().zip(&BAR_H).enumerate() {
            let column = Obj::plain(root)
                .rect(Rect::new(x, COLUMN_TOP, BAR_W, COLUMN_H))
                .clickable(true);
            let bar = Obj::plain(column)
                .rect(Rect::new(0, COLUMN_H - h, BAR_W, h))
                .radius(BAR_RADIUS);
            let label = Obj::label(root, nr_name(&LEVELS[index]), Font::Micro, TEXT_3)
                .centered_text(LABEL_W)
                .pos(x + BAR_W / 2 - LABEL_W / 2, LABEL_Y);
            let (chosen, dispatch) = (self.chosen.clone(), ctx.dispatch.clone());
            on_click(column, move || {
                chosen.set(Some(index));
                dispatch.command(Command::SetNrLevel(LEVELS[index].clone()));
            });
            #[cfg(test)]
            self.columns.push(column);
            self.bars.push(bar);
            self.labels.push(label);
        }
        self.name = Some(
            Obj::label(root, "", Font::Title, TEXT)
                .centered_text(SCREEN_W)
                .pos(0, NAME_Y),
        );
        self.detail = Some(
            Obj::label(root, "", Font::Small, TEXT_2)
                .centered_text(SCREEN_W)
                .pos(0, DETAIL_Y),
        );
        back_button(root, &ctx.dispatch);
        self.update(snap);
        root.update_layout();
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let reported = level_index(&snap.audio.nr_level);
        if self.chosen.get() == Some(reported) {
            self.chosen.set(None);
        }
        let selected = self.chosen.get().unwrap_or(reported);
        if self.shown == Some(selected) {
            return;
        }
        for (index, (bar, label)) in self.bars.iter().zip(&self.labels).enumerate() {
            if index <= selected {
                bar.bg(LIVE).border(0, LINE);
            } else {
                bar.bg(SURFACE).border(2, LINE);
            }
            label.text_color(if index == selected { LIVE } else { TEXT_3 });
        }
        if let (Some(name), Some(detail_label)) = (self.name, self.detail) {
            name.set_text(nr_name(&LEVELS[selected]));
            detail_label.set_text(detail(&LEVELS[selected]));
        }
        self.shown = Some(selected);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::kit::tokens::Rect;
    use crate::screens_v2::menu::fixture::{fixture, snap};
    use waved_state::Command;

    fn rect(obj: Obj) -> Rect {
        let a = obj.coords();
        Rect::new(a.x1, a.y1, a.x2 - a.x1 + 1, a.y2 - a.y1 + 1)
    }

    #[test]
    fn noise_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.audio.nr_level = NrLevel::Med;
            let mut screen = NoiseScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_noise", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.detail.unwrap().text(), "Removes most background noise");
        });
    }

    #[test]
    fn every_column_is_a_full_height_target() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = NoiseScreen::default();
            screen.build(root, &f.ctx, &snap());
            root.update_layout();
            assert_targets_ok(root);
            for (column, x) in screen.columns.iter().zip([94, 150, 206, 262]) {
                assert_eq!(rect(*column), Rect::new(x, 76, 44, 88));
            }
            assert_eq!(rect(screen.bars[0]), Rect::new(94, 136, 44, 28));
        });
    }

    #[test]
    fn a_tap_shows_the_new_level_before_the_service_confirms() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.audio.nr_level = NrLevel::Med;
            let mut screen = NoiseScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_targets_ok(root);
            lv.click(screen.columns[0]); // the short Off bar
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetNrLevel(NrLevel::Off)]
            ));
            screen.update(&snap); // still Medium in the snapshot
            assert_eq!(screen.name.unwrap().text(), "Off");
            snap.audio.nr_level = NrLevel::Off;
            screen.update(&snap);
            assert!(screen.chosen.get().is_none());
            assert_eq!(screen.name.unwrap().text(), "Off");
        });
    }

    #[test]
    fn descriptions_are_plain() {
        assert_eq!(detail(&NrLevel::Off), "Room sound untouched");
        assert_eq!(detail(&NrLevel::Low), "Softens fans and hum");
        assert_eq!(detail(&NrLevel::High), "Strongest, voices only");
        assert_eq!(level_index(&NrLevel::High), 3);
    }
}
