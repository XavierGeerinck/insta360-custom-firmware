//! Confirmation (B6, confirm.png): one screen for every action that can't be
//! undone or drops a connection. The action pill repeats the action's name,
//! and the message afterwards uses it too (copy::POWERING_OFF). It handles kept
//! files only: a recording waiting on the decision screen is discarded there.

use crate::kit::components::buttons::Tone;
#[cfg(test)]
use crate::kit::components::row::ButtonRow;
use crate::kit::components::row::{confirm_dialog, ConfirmSpec};
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::{Dispatcher, Message, Severity, UiEvent};
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::{ConfirmKind, Route};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::screens_v2::messages::copy;
use waved_state::Command;

pub fn spec(kind: ConfirmKind, snap: &UiSnapshot) -> ConfirmSpec {
    let (icon, title, body, confirm, tone) = match kind {
        ConfirmKind::PowerOff => (
            icons::POWER,
            "Power off?",
            "A recording in progress is saved first. Turn Wave on again with the power button."
                .to_string(),
            "Power off",
            Tone::Rec,
        ),
        ConfirmKind::FactoryReset => (
            icons::TRASH,
            "Factory reset?",
            "Erases settings, recordings and Wi-Fi, then restarts Wave as new. This can't be undone."
                .to_string(),
            "Factory reset",
            Tone::RecText,
        ),
        ConfirmKind::DeleteRecording(_) => (
            icons::TRASH,
            "Delete this recording?",
            "It's removed from Wave. A copy already sent stays on your server.".to_string(),
            "Delete",
            Tone::RecText,
        ),
        ConfirmKind::ChangeNetwork => {
            let ssid = snap.wifi.ssid.trim();
            let network = if ssid.is_empty() { "its network".to_string() } else { ssid.to_string() };
            (
                icons::WIFI,
                "Change network?",
                format!("Wave leaves {network} while the setup network is open. Uploads wait until it's back."),
                "Change",
                Tone::Live,
            )
        }
        ConfirmKind::StopUsbStorage => (
            icons::USB,
            "Stop USB storage?",
            "Eject Wave on your computer first, so files being copied finish.".to_string(),
            "Stop",
            Tone::Neutral,
        ),
    };
    ConfirmSpec {
        icon,
        title: title.into(),
        body,
        confirm: confirm.into(),
        tone,
    }
}

/// What confirming does: a service command, screens to pop, a screen to
/// open after that, and a message.
#[derive(Debug)]
pub struct Outcome {
    pub command: Option<Command>,
    pub backs: usize,
    pub then: Option<Route>,
    pub message: Option<Message>,
}

pub fn outcome(kind: ConfirmKind) -> Outcome {
    let done = |command, message: Option<Message>| Outcome {
        command,
        backs: 1,
        then: None,
        message,
    };
    match kind {
        // The Powering off screen says it, and stays until the power is cut.
        ConfirmKind::PowerOff => Outcome {
            command: Some(Command::PowerOff),
            backs: 1,
            then: Some(Route::PoweringOff),
            message: None,
        },
        ConfirmKind::FactoryReset => done(
            Some(Command::FactoryReset),
            Some(Message::new(Severity::Info, copy::RESETTING)),
        ),
        // One back: the file's detail leaves by itself when its row disappears.
        ConfirmKind::DeleteRecording(id) => done(
            Some(Command::DeleteRecording(id)),
            Some(Message::new(Severity::Success, copy::DELETED)),
        ),
        ConfirmKind::ChangeNetwork => Outcome {
            command: None,
            backs: 1,
            then: Some(Route::WifiSetup),
            message: None,
        },
        // G1 (milestone 3) adds Command::StopUsbStorage and sends it here.
        ConfirmKind::StopUsbStorage => done(None, None),
    }
}

fn apply(dispatch: &Dispatcher, outcome: Outcome) {
    if let Some(command) = outcome.command {
        dispatch.command(command);
    }
    for _ in 0..outcome.backs {
        dispatch.back();
    }
    if let Some(route) = outcome.then {
        dispatch.navigate(route);
    }
    if let Some(message) = outcome.message {
        dispatch.ui(UiEvent::Show(message));
    }
}

pub struct ConfirmScreen {
    kind: ConfirmKind,
    status: Option<StatusArea>,
    /// Tests click the pills through this.
    #[cfg(test)]
    row: Option<ButtonRow>,
}

impl ConfirmScreen {
    pub fn new(kind: ConfirmKind) -> Self {
        Self {
            kind,
            status: None,
            #[cfg(test)]
            row: None,
        }
    }
}

impl Screen for ConfirmScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        let kind = self.kind;
        let (dispatch, cancel) = (ctx.dispatch.clone(), ctx.dispatch.clone());
        let _row = confirm_dialog(
            root,
            &spec(kind, snap),
            move || apply(&dispatch, outcome(kind)),
            move || cancel.back(),
        );
        #[cfg(test)]
        {
            self.row = Some(_row);
        }
        root.update_layout();
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap, Fixture};
    use uuid::Uuid;

    fn build(
        kind: ConfirmKind,
        snap: &UiSnapshot,
        lv: &mut crate::kit::testkit::Lvgl,
    ) -> (Fixture, ConfirmScreen, Obj) {
        let root = lv.screen();
        let f = fixture();
        let mut screen = ConfirmScreen::new(kind);
        screen.build(root, &f.ctx, snap);
        (f, screen, root)
    }

    #[test]
    fn confirm_matches_the_mockup() {
        lvgl(|lv| {
            let mut snap = snap();
            snap.clock = "14:37".into();
            let (_f, _screen, root) = build(ConfirmKind::PowerOff, &snap, lv);
            assert_golden("screen_confirm", &lv.render());
            assert_targets_ok(root);
        });
        lvgl(|lv| {
            let (_f, _s, root) = build(ConfirmKind::FactoryReset, &snap(), lv);
            assert_golden("screen_confirm_factory_reset", &lv.render());
            assert_targets_ok(root);
        });
        lvgl(|lv| {
            let (_f, _s, root) = build(ConfirmKind::ChangeNetwork, &snap(), lv);
            assert_golden("screen_confirm_change_network", &lv.render());
            assert_targets_ok(root);
        });
        lvgl(|lv| {
            let (_f, _s, root) = build(ConfirmKind::DeleteRecording(Uuid::new_v4()), &snap(), lv);
            assert_golden("screen_confirm_delete_recording", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn power_off_only_after_confirming() {
        lvgl(|lv| {
            let (mut f, screen, root) = build(ConfirmKind::PowerOff, &snap(), lv);
            assert!(f.commands().is_empty());
            lv.click(screen.row.as_ref().unwrap().obj(0)); // Cancel
            assert!(f.commands().is_empty());
            assert_eq!(f.events(), vec![UiEvent::Back]);
            lv.click(screen.row.as_ref().unwrap().obj(1)); // Power off
            assert!(matches!(f.commands().as_slice(), [Command::PowerOff]));
            assert_eq!(
                f.events(),
                vec![UiEvent::Back, UiEvent::Navigate(Route::PoweringOff)]
            );
            assert_targets_ok(root);
        });
    }

    #[test]
    fn factory_reset_only_after_confirming() {
        lvgl(|lv| {
            let (mut f, screen, root) = build(ConfirmKind::FactoryReset, &snap(), lv);
            assert_targets_ok(root);
            assert!(f.commands().is_empty());
            lv.click(screen.row.as_ref().unwrap().obj(0)); // Cancel
            assert!(f.commands().is_empty());
            assert_eq!(f.events(), vec![UiEvent::Back]);
            lv.click(screen.row.as_ref().unwrap().obj(1)); // Factory reset
            assert!(matches!(f.commands().as_slice(), [Command::FactoryReset]));
            assert_eq!(
                f.events(),
                vec![
                    UiEvent::Back,
                    UiEvent::Show(Message::new(Severity::Info, copy::RESETTING))
                ]
            );
        });
    }

    #[test]
    fn deleting_a_kept_file_sends_delete_recording() {
        let kept = Uuid::new_v4();
        let done = outcome(ConfirmKind::DeleteRecording(kept));
        assert!(matches!(done.command, Some(Command::DeleteRecording(id)) if id == kept));
        assert_eq!(done.backs, 1);
        assert_eq!(
            done.message,
            Some(Message::new(Severity::Success, copy::DELETED))
        );
        lvgl(|lv| {
            let (mut f, screen, root) = build(ConfirmKind::DeleteRecording(kept), &snap(), lv);
            lv.click(screen.row.as_ref().unwrap().obj(1));
            assert!(
                matches!(f.commands().as_slice(), [Command::DeleteRecording(id)] if *id == kept)
            );
            // One back: the file's detail leaves by itself when its row disappears.
            assert_eq!(
                f.events(),
                vec![
                    UiEvent::Back,
                    UiEvent::Show(Message::new(Severity::Success, copy::DELETED))
                ]
            );
            assert_targets_ok(root);
        });
    }

    #[test]
    fn power_off_continues_to_the_powering_off_screen() {
        let next = outcome(ConfirmKind::PowerOff);
        assert_eq!(next.command, Some(Command::PowerOff));
        assert_eq!((next.backs, next.then), (1, Some(Route::PoweringOff)));
        assert!(next.message.is_none(), "the screen says it");
    }

    #[test]
    fn change_network_continues_to_wifi_setup() {
        let next = outcome(ConfirmKind::ChangeNetwork);
        assert!(next.command.is_none());
        assert_eq!((next.backs, next.then), (1, Some(Route::WifiSetup)));
        let mut snap = snap();
        snap.wifi.ssid = "Office-5G".into();
        assert!(spec(ConfirmKind::ChangeNetwork, &snap)
            .body
            .contains("Office-5G"));
        snap.wifi.ssid.clear();
        assert!(spec(ConfirmKind::ChangeNetwork, &snap)
            .body
            .starts_with("Wave leaves its network"));
        lvgl(|lv| {
            let (mut f, screen, root) = build(ConfirmKind::ChangeNetwork, &snap, lv);
            lv.click(screen.row.as_ref().unwrap().obj(1));
            assert!(f.commands().is_empty());
            assert_eq!(
                f.events(),
                vec![UiEvent::Back, UiEvent::Navigate(Route::WifiSetup)]
            );
            assert_targets_ok(root);
        });
    }

    #[test]
    fn every_action_button_repeats_the_action() {
        let snap = snap();
        for (kind, word) in [
            (ConfirmKind::PowerOff, "Power off"),
            (ConfirmKind::FactoryReset, "Factory reset"),
            (ConfirmKind::DeleteRecording(Uuid::new_v4()), "Delete"),
            (ConfirmKind::ChangeNetwork, "Change"),
            (ConfirmKind::StopUsbStorage, "Stop"),
        ] {
            assert_eq!(spec(kind, &snap).confirm, word, "{kind:?}");
        }
    }
}
