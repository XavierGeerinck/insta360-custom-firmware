//! Firmware update (H3, docs/mockups/png/ota.png). While an update is under
//! way the dial is a progress ring, with nothing to tap: interrupting would
//! leave Wave unusable. Otherwise one message and at most one action: Retry
//! after a failure ("Update failed. Wave is still on 0.9.3."), Install when a
//! version is available, Check now when up to date, nothing when the update
//! is installed but Wave did not restart ("Restart Wave"). The phase comes
//! from `snap.ota` (waved-update, docs/UPDATES.md).

use crate::kit::components::buttons::{back_button, text_button};
use crate::kit::components::dial::Dial;
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::Dispatcher;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    Rect, DIAL_CX, DIAL_CY, DIAL_R, SCREEN_H, SCREEN_W, TEXT, TEXT_2, TEXT_3, WARN,
};
use crate::screens_v2::volume::BigNumber;
use waved_state::{Command, OtaPhase, OtaState};

/// ota.png, CSS tops in framebuffer pixels.
const CAPTION_Y: i32 = 77;
const NUMBER_TOP: i32 = 93;
const VERSION_Y: i32 = 145;
const BODY_Y: i32 = 216;
const SMALL_Y: i32 = 238;

/// The message layout: icon, title, two lines, then the action above Back.
const ICON_Y: i32 = 56;
const TITLE_Y: i32 = 108;
const LINE_Y: i32 = 140;
const DETAIL_Y: i32 = 160;
const ACTION: Rect = Rect::new(130, 184, 140, 44);
const TEXT_W: i32 = 300;

pub const KEEP_PLUGGED_IN: &str = "Keep Wave plugged in";
pub const RESTARTS_BY_ITSELF: &str = "It restarts by itself when done";
pub const RESTART_TO_FINISH: &str = "Turn Wave off and on to finish";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Retry,
    Install,
    CheckNow,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Retry => "Retry",
            Action::Install => "Install",
            Action::CheckNow => "Check now",
        }
    }

    pub fn command(self) -> Command {
        match self {
            Action::Retry | Action::Install => Command::InstallUpdate,
            Action::CheckNow => Command::CheckForUpdate,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgressLook {
    pub caption: &'static str,
    pub pct: u8,
    /// "waved 0.10.0", or empty while the version is not known yet.
    pub version: String,
    pub body: &'static str,
    pub small: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageLook {
    pub failed: bool,
    pub title: &'static str,
    pub line: String,
    pub detail: String,
    pub action: Option<Action>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Look {
    Progress(ProgressLook),
    Message(MessageLook),
}

fn waved(version: &str) -> String {
    format!("waved {version}")
}

/// What the screen shows for `ota`.
pub fn look(ota: &OtaState) -> Look {
    let current = if ota.current_version.is_empty() {
        env!("CARGO_PKG_VERSION")
    } else {
        ota.current_version.as_str()
    };
    let target = ota.target_version.as_deref().map(waved).unwrap_or_default();
    let progress = |caption, body, small| {
        Look::Progress(ProgressLook {
            caption,
            pct: ota.progress_pct.min(100),
            version: target.clone(),
            body,
            small,
        })
    };
    let message = |failed, title, line: String, detail: String, action| {
        Look::Message(MessageLook {
            failed,
            title,
            line,
            detail,
            action,
        })
    };
    match &ota.phase {
        OtaPhase::Checking => progress("Checking for updates", "", ""),
        OtaPhase::Downloading | OtaPhase::Verifying | OtaPhase::Installing => {
            progress("Installing update", KEEP_PLUGGED_IN, RESTARTS_BY_ITSELF)
        }
        OtaPhase::Waiting => progress(
            "Update ready",
            "Installs when Wave is free",
            "After the recording or call ends",
        ),
        OtaPhase::Restarting => {
            progress("Restarting", KEEP_PLUGGED_IN, "This takes about a minute")
        }
        // The reboot failed. Installing again would undo the armed trial,
        // so there is no Retry: the user restarts Wave.
        OtaPhase::RestartNeeded => message(
            true,
            "Restart Wave",
            if target.is_empty() {
                "The update is installed".into()
            } else {
                format!("{target} is installed")
            },
            RESTART_TO_FINISH.into(),
            None,
        ),
        OtaPhase::Failed { reason } => message(
            true,
            "Update failed",
            format!("Wave is still on {current}"),
            reason.clone(),
            Some(Action::Retry),
        ),
        OtaPhase::Available => message(
            false,
            "Update available",
            target.clone(),
            String::new(),
            Some(Action::Install),
        ),
        OtaPhase::Idle | OtaPhase::UpToDate => message(
            false,
            "Up to date",
            waved(current),
            String::new(),
            Some(Action::CheckNow),
        ),
        OtaPhase::Off => message(
            false,
            "Updates are off",
            "Set an update server on the setup page".into(),
            String::new(),
            None,
        ),
    }
}

/// A full-screen container without a layout, so the kit's buttons can sit in it.
fn layer(root: Obj) -> Obj {
    Obj::plain(root).rect(Rect::new(0, 0, SCREEN_W, SCREEN_H))
}

fn line(parent: Obj, text: &str, font: Font, hex: u32, y: i32) -> Obj {
    let label = Obj::label(parent, text, font, hex)
        .centered_text(TEXT_W)
        .pos((SCREEN_W - TEXT_W) / 2, y);
    label.hidden(text.is_empty())
}

struct ProgressParts {
    layer: Obj,
    dial: Dial,
    caption: Obj,
    number: BigNumber,
    version: Obj,
    body: Obj,
    small: Obj,
    shown: ProgressLook,
}

impl ProgressParts {
    fn build(root: Obj, look: &ProgressLook) -> Self {
        let layer = layer(root);
        let mut parts = Self {
            layer,
            dial: Dial::new(layer, DIAL_CX, DIAL_CY, DIAL_R, 3),
            caption: line(layer, "", Font::Small, TEXT_2, CAPTION_Y),
            number: BigNumber::new(layer, NUMBER_TOP),
            version: line(layer, "", Font::Small, TEXT_2, VERSION_Y),
            body: line(layer, "", Font::Body, TEXT, BODY_Y),
            small: line(layer, "", Font::Small, TEXT_2, SMALL_Y),
            shown: look.clone(),
        };
        parts.show(look, true);
        parts
    }

    fn show(&mut self, look: &ProgressLook, force: bool) {
        if !force && *look == self.shown {
            return;
        }
        if force || look.pct != self.shown.pct {
            self.dial.set_progress(Some(f32::from(look.pct) / 100.0));
            self.number.set(&look.pct.to_string(), "%", 0, TEXT);
        }
        for (label, text) in [
            (self.caption, look.caption),
            (self.version, look.version.as_str()),
            (self.body, look.body),
            (self.small, look.small),
        ] {
            label.hidden(text.is_empty()).set_text(text);
        }
        self.shown = look.clone();
    }
}

struct MessageParts {
    layer: Obj,
    shown: MessageLook,
    /// The action button, for tests to tap.
    #[cfg_attr(not(test), allow(dead_code))]
    action: Option<Obj>,
}

impl MessageParts {
    fn build(root: Obj, look: &MessageLook, dispatch: &Dispatcher) -> Self {
        let layer = layer(root);
        let (icon, hex) = if look.failed {
            (icons::ALERT, WARN)
        } else {
            (icons::REFRESH, TEXT_2)
        };
        Obj::label(layer, icon, Font::Icon44, hex)
            .centered_text(SCREEN_W)
            .pos(0, ICON_Y);
        Obj::label(layer, look.title, Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        line(layer, &look.line, Font::Small, TEXT_2, LINE_Y);
        line(layer, &look.detail, Font::Micro, TEXT_3, DETAIL_Y);
        let action = look.action.map(|action| {
            let dispatch = dispatch.clone();
            text_button(layer, ACTION, action.label(), move || {
                dispatch.command(action.command())
            })
        });
        back_button(layer, dispatch);
        Self {
            layer,
            shown: look.clone(),
            action,
        }
    }
}

enum Parts {
    Progress(ProgressParts),
    Message(MessageParts),
}

impl Parts {
    fn layer(&self) -> Obj {
        match self {
            Parts::Progress(p) => p.layer,
            Parts::Message(m) => m.layer,
        }
    }
}

#[derive(Default)]
pub struct UpdateScreen {
    root: Option<Obj>,
    dispatch: Option<Dispatcher>,
    status: Option<StatusArea>,
    parts: Option<Parts>,
}

impl UpdateScreen {
    #[cfg(test)]
    fn action(&self) -> Option<Obj> {
        match &self.parts {
            Some(Parts::Message(m)) => m.action,
            _ => None,
        }
    }
}

impl Screen for UpdateScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.root = Some(root);
        self.dispatch = Some(ctx.dispatch.clone());
        self.status = Some(StatusArea::plain(root));
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let (Some(root), Some(dispatch)) = (self.root, self.dispatch.as_ref()) else {
            return;
        };
        let look = look(&snap.ota);
        match (&mut self.parts, &look) {
            (Some(Parts::Progress(parts)), Look::Progress(progress)) => parts.show(progress, false),
            (Some(Parts::Message(parts)), Look::Message(message)) if parts.shown == *message => {}
            _ => {
                if let Some(old) = self.parts.take() {
                    old.layer().delete();
                }
                self.parts = Some(match &look {
                    Look::Progress(progress) => {
                        Parts::Progress(ProgressParts::build(root, progress))
                    }
                    Look::Message(message) => {
                        Parts::Message(MessageParts::build(root, message, dispatch))
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::runtime::testing::shows_text;
    use crate::screens_v2::main_parts::fixtures::{ctx, sent, snap};

    fn ota(phase: OtaPhase, pct: u8) -> OtaState {
        OtaState {
            phase,
            current_version: "0.9.3".into(),
            target_version: Some("0.10.0".into()),
            progress_pct: pct,
        }
    }

    fn charging() -> UiSnapshot {
        let mut snap = snap();
        snap.clock = "02:14".into();
        snap.battery.level = 100;
        snap.battery.is_charging = true;
        snap
    }

    #[test]
    fn installing_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = charging();
            snap.ota = ota(OtaPhase::Downloading, 64);
            let mut screen = UpdateScreen::default();
            screen.build(root, &ctx, &snap);
            assert_golden("update_installing", &lv.render());
            assert_targets_ok(root);
            for text in [
                "Installing update",
                "64",
                "waved 0.10.0",
                KEEP_PLUGGED_IN,
                RESTARTS_BY_ITSELF,
            ] {
                assert!(shows_text(root, text), "{text}");
            }
            assert!(!shows_text(root, "Back"), "nothing to tap while installing");
        });
    }

    #[test]
    fn a_failure_says_what_runs_and_offers_retry() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, _ui) = ctx();
            let mut snap = charging();
            snap.ota = ota(
                OtaPhase::Failed {
                    reason: "The download stopped".into(),
                },
                0,
            );
            let mut screen = UpdateScreen::default();
            screen.build(root, &ctx, &snap);
            assert_golden("update_failed", &lv.render());
            assert_targets_ok(root);
            for text in [
                "Update failed",
                "Wave is still on 0.9.3",
                "The download stopped",
                "Retry",
                "Back",
            ] {
                assert!(shows_text(root, text), "{text}");
            }
            lv.click(screen.action().unwrap());
            assert_eq!(sent(&mut bus), ["InstallUpdate"]);
        });
    }

    #[test]
    fn a_failed_restart_asks_for_one_and_offers_no_retry() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = charging();
            snap.ota = ota(OtaPhase::RestartNeeded, 100);
            let mut screen = UpdateScreen::default();
            screen.build(root, &ctx, &snap);
            assert_golden("update_restart_needed", &lv.render());
            for text in [
                "Restart Wave",
                "waved 0.10.0 is installed",
                RESTART_TO_FINISH,
                "Back",
            ] {
                assert!(shows_text(root, text), "{text}");
            }
            // Installing again would undo the armed trial's fall-back.
            assert!(screen.action().is_none());
            assert!(!shows_text(root, "Retry"));
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_screen_follows_the_update_through_its_phases() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, _ui) = ctx();
            let mut snap = snap();
            snap.ota = ota(OtaPhase::Available, 0);
            let mut screen = UpdateScreen::default();
            screen.build(root, &ctx, &snap);
            assert!(shows_text(root, "Update available"));
            lv.click(screen.action().unwrap());
            assert_eq!(sent(&mut bus), ["InstallUpdate"]);

            for pct in [0, 12, 45, 90] {
                snap.ota = ota(OtaPhase::Downloading, pct);
                screen.update(&snap);
                assert!(shows_text(root, &pct.to_string()), "{pct}");
            }
            assert!(screen.action().is_none());
            snap.ota = ota(OtaPhase::Waiting, 98);
            screen.update(&snap);
            assert!(shows_text(root, "Update ready"));
            assert!(shows_text(root, "Installs when Wave is free"));
            snap.ota = ota(OtaPhase::Restarting, 100);
            screen.update(&snap);
            assert!(shows_text(root, "Restarting"));
            assert_targets_ok(root);

            snap.ota = ota(OtaPhase::UpToDate, 0);
            snap.ota.target_version = None;
            screen.update(&snap);
            assert!(shows_text(root, "Up to date"));
            assert!(shows_text(root, "waved 0.9.3"));
            lv.click(screen.action().unwrap());
            assert_eq!(sent(&mut bus), ["CheckForUpdate"]);
            assert_targets_ok(root);
        });
    }

    #[test]
    fn with_updates_off_there_is_nothing_to_do_but_go_back() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut screen = UpdateScreen::default();
            screen.build(root, &ctx, &UiSnapshot::default());
            assert!(shows_text(root, "Updates are off"));
            assert!(shows_text(root, "Back"));
            assert!(screen.action().is_none());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn every_phase_has_a_look() {
        let failed = OtaPhase::Failed { reason: "x".into() };
        for phase in [
            OtaPhase::Off,
            OtaPhase::Idle,
            OtaPhase::Checking,
            OtaPhase::UpToDate,
            OtaPhase::Available,
            OtaPhase::Downloading,
            OtaPhase::Verifying,
            OtaPhase::Waiting,
            OtaPhase::Installing,
            OtaPhase::Restarting,
            OtaPhase::RestartNeeded,
            failed,
        ] {
            let in_progress = phase.in_progress();
            let look = look(&ota(phase, 50));
            assert_eq!(matches!(look, Look::Progress(_)), in_progress, "{look:?}");
        }
    }
}
