//! After a recording (B8, upload-mode.png): what happens to each recording,
//! a privacy choice with one plain sentence per option
//! (`upload_mode = automatic | ask | local_only`).

use crate::kit::components::buttons::back_button;
use crate::kit::components::controls::{RadioList, RadioOption};
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{RADIO_TOP, SCREEN_W, TEXT, TITLE_Y};
use waved_state::{Command, UploadMode};

pub const MODES: [UploadMode; 3] = [
    UploadMode::Automatic,
    UploadMode::Ask,
    UploadMode::LocalOnly,
];

pub fn options() -> Vec<RadioOption> {
    vec![
        RadioOption::new("Send automatically", "As soon as Wi-Fi allows"),
        RadioOption::new("Ask me each time", "Choose Send, Keep or Delete"),
        RadioOption::new("Keep on Wave", "Nothing leaves the device"),
    ]
}

pub fn mode_index(mode: UploadMode) -> usize {
    MODES.iter().position(|m| *m == mode).unwrap_or(0)
}

#[derive(Default)]
pub struct UploadModeScreen {
    status: Option<StatusArea>,
    list: Option<RadioList>,
    seen: Option<UploadMode>,
}

impl Screen for UploadModeScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "After a recording", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        let dispatch = ctx.dispatch.clone();
        self.list = Some(RadioList::new(
            root,
            RADIO_TOP,
            options(),
            mode_index(snap.settings.upload_mode),
            move |index| dispatch.command(Command::SetUploadMode(MODES[index])),
        ));
        self.seen = Some(snap.settings.upload_mode);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        // Follow a change made elsewhere; a tap here already moved the radio.
        if self.seen != Some(snap.settings.upload_mode) {
            self.seen = Some(snap.settings.upload_mode);
            if let Some(list) = self.list.as_ref() {
                list.set_selected(mode_index(snap.settings.upload_mode));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    #[test]
    fn upload_mode_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.clock = "15:22".into();
            snap.battery.level = 79;
            snap.battery.voltage = 3_800;
            snap.settings.upload_mode = UploadMode::Ask;
            let mut screen = UploadModeScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_upload_mode", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn choosing_sends_the_mode_and_follows_changes_made_elsewhere() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.settings.upload_mode = UploadMode::Ask;
            let mut screen = UploadModeScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_targets_ok(root);
            let list = screen.list.as_ref().unwrap();
            lv.click(list.row(2));
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetUploadMode(UploadMode::LocalOnly)]
            ));
            screen.update(&snap); // stale: still Ask
            assert_eq!(screen.list.as_ref().unwrap().selected(), 2);
            snap.settings.upload_mode = UploadMode::Automatic; // the setup page changed it
            screen.update(&snap);
            assert_eq!(screen.list.as_ref().unwrap().selected(), 0);
        });
    }

    #[test]
    fn options_name_the_privacy_choice() {
        let titles: Vec<String> = options().into_iter().map(|o| o.title).collect();
        assert_eq!(
            titles,
            ["Send automatically", "Ask me each time", "Keep on Wave"]
        );
        assert_eq!(mode_index(UploadMode::LocalOnly), 2);
    }
}
