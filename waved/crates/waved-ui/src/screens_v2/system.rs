//! System (B6, system.png): the same rotary list as the menu. Rows that
//! aren't built yet (USB storage, PIN) stay hidden until their milestone 3
//! workstream lands. Power off and Factory reset go through the confirmation.
//! The two switch rows are optimistic: a tap shows the new value at once and
//! a stale snapshot does not revert it (see `menu::Pending`). Firmware
//! update (H3) checks, or opens the update screen when there is something
//! to install or watch.

use crate::kit::components::buttons::back_button;
use crate::kit::components::list::{ListItem, RotaryList};
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::Dispatcher;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::{ConfirmKind, Route};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::screens_v2::menu::Pending;
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use waved_state::{Command, OtaPhase, OtaState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemRow {
    Display,
    DateTime,
    AlertTones,
    PreRecording,
    About,
    FirmwareUpdate,
    FactoryReset,
    PowerOff,
}

pub const ROWS: [SystemRow; 8] = [
    SystemRow::Display,
    SystemRow::DateTime,
    SystemRow::AlertTones,
    SystemRow::PreRecording,
    SystemRow::About,
    SystemRow::FirmwareUpdate,
    SystemRow::FactoryReset,
    SystemRow::PowerOff,
];

/// Firmware update is centred when System opens, as in system.png.
pub const START_ROW: usize = 5;

fn on_off(on: bool) -> String {
    if on { "On" } else { "Off" }.into()
}

/// The Firmware update row's value (system.png: "Up to date").
pub fn update_value(ota: &OtaState) -> String {
    match &ota.phase {
        OtaPhase::Off => "Off".into(),
        OtaPhase::Idle => "Check now".into(),
        OtaPhase::Checking => "Checking".into(),
        OtaPhase::UpToDate => "Up to date".into(),
        OtaPhase::Available => "Available".into(),
        OtaPhase::Downloading | OtaPhase::Verifying => format!("{}%", ota.progress_pct.min(100)),
        OtaPhase::Waiting => "Ready".into(),
        OtaPhase::Installing | OtaPhase::Restarting => "Installing".into(),
        OtaPhase::RestartNeeded => "Restart".into(),
        OtaPhase::Failed { .. } => "Failed".into(),
    }
}

/// A Firmware update tap: check from the row when there is nothing to
/// install; install and watch when a version is available; otherwise open
/// the update screen. After a failure that screen says why (a version that
/// rolled back, say) and offers Retry; a row tap never reinstalls unseen.
fn tap_update(phase: &OtaPhase, dispatch: &Dispatcher) {
    match phase {
        OtaPhase::Idle | OtaPhase::UpToDate => dispatch.command(Command::CheckForUpdate),
        OtaPhase::Available => {
            dispatch.navigate(Route::Update);
            dispatch.command(Command::InstallUpdate);
        }
        _ => dispatch.navigate(Route::Update),
    }
}

impl SystemRow {
    pub fn title(self) -> &'static str {
        match self {
            SystemRow::Display => "Display",
            SystemRow::DateTime => "Date and time",
            SystemRow::AlertTones => "Alert tones",
            SystemRow::PreRecording => "Pre-recording",
            SystemRow::About => "About",
            SystemRow::FirmwareUpdate => "Firmware update",
            SystemRow::FactoryReset => "Factory reset",
            SystemRow::PowerOff => "Power off",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            SystemRow::Display => icons::SUN,
            SystemRow::DateTime => icons::CALENDAR,
            SystemRow::AlertTones => icons::BELL,
            SystemRow::PreRecording => icons::CLOCK,
            SystemRow::About => icons::INFO,
            SystemRow::FirmwareUpdate => icons::REFRESH,
            SystemRow::FactoryReset => icons::TRASH,
            SystemRow::PowerOff => icons::POWER,
        }
    }

    /// The value at the right of the row, as the snapshot reports it.
    pub fn value(self, snap: &UiSnapshot) -> Option<String> {
        match self {
            SystemRow::Display => {
                let brightness = snap.settings.brightness_pct;
                Some(format!("{brightness}%"))
            }
            SystemRow::DateTime => Some(snap.clock.clone()),
            SystemRow::AlertTones => Some(on_off(snap.settings.tones_enabled)),
            SystemRow::PreRecording => Some(on_off(snap.settings.pre_recording)),
            SystemRow::About => Some(super::about::version_line(snap)),
            SystemRow::FirmwareUpdate => Some(update_value(&snap.ota)),
            SystemRow::FactoryReset | SystemRow::PowerOff => None,
        }
    }
}

/// Flip an on/off setting and return the value to send. Two taps within one
/// frame flip twice, so the same value is never sent twice in a row.
fn flip(switch: &Pending<bool>) -> bool {
    let on = !switch.value().unwrap_or_default();
    switch.set(on);
    on
}

/// Shared between the screen and the list's tap handler.
#[derive(Default)]
struct State {
    tones: Pending<bool>,
    pre_recording: Pending<bool>,
    /// The update phase as of the last snapshot, for the row's tap.
    ota: OtaPhase,
    /// What each row shows now.
    shown: Vec<Option<String>>,
    list: Weak<RotaryList>,
}

impl State {
    fn follow(&mut self, snap: &UiSnapshot) {
        self.tones.follow(snap.settings.tones_enabled);
        self.pre_recording.follow(snap.settings.pre_recording);
        self.ota = snap.ota.phase.clone();
    }

    fn value(&self, row: SystemRow, snap: &UiSnapshot) -> Option<String> {
        match row {
            SystemRow::AlertTones => self.tones.value().map(on_off),
            SystemRow::PreRecording => self.pre_recording.value().map(on_off),
            _ => row.value(snap),
        }
    }

    /// Put `value` on the row if it differs from what it shows.
    fn show(&mut self, index: usize, value: Option<String>) {
        if self.shown[index] == value {
            return;
        }
        if let Some(list) = self.list.upgrade() {
            list.set_value(index, value.as_deref());
        }
        self.shown[index] = value;
    }
}

#[derive(Default)]
pub struct SystemScreen {
    status: Option<StatusArea>,
    list: Option<Rc<RotaryList>>,
    state: Rc<RefCell<State>>,
}

impl Screen for SystemScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        {
            let mut state = self.state.borrow_mut();
            state.tones = Pending::new(snap.settings.tones_enabled);
            state.pre_recording = Pending::new(snap.settings.pre_recording);
            state.ota = snap.ota.phase.clone();
            state.shown = ROWS.iter().map(|row| state.value(*row, snap)).collect();
        }
        let items = ROWS
            .iter()
            .zip(self.state.borrow().shown.iter())
            .map(|(row, value)| {
                let item = ListItem::new(row.icon(), row.title());
                let item = if *row == SystemRow::PowerOff {
                    item.danger()
                } else {
                    item
                };
                match value {
                    Some(value) => item.value(value.as_str()),
                    None => item,
                }
            })
            .collect();
        let (dispatch, state) = (ctx.dispatch.clone(), self.state.clone());
        let list = Rc::new(RotaryList::new(root, items, move |index| {
            match ROWS[index] {
                SystemRow::Display => dispatch.navigate(Route::Display),
                SystemRow::DateTime => dispatch.navigate(Route::DateTime),
                SystemRow::AlertTones => {
                    let mut state = state.borrow_mut();
                    let on = flip(&state.tones);
                    state.show(index, Some(on_off(on)));
                    dispatch.command(Command::SetAlertTones(on));
                }
                SystemRow::PreRecording => {
                    let mut state = state.borrow_mut();
                    let on = flip(&state.pre_recording);
                    state.show(index, Some(on_off(on)));
                    dispatch.command(Command::SetPreRecording(on));
                }
                SystemRow::About => dispatch.navigate(Route::About),
                SystemRow::FirmwareUpdate => {
                    let phase = state.borrow().ota.clone();
                    tap_update(&phase, &dispatch);
                }
                SystemRow::FactoryReset => {
                    dispatch.navigate(Route::Confirm(ConfirmKind::FactoryReset))
                }
                SystemRow::PowerOff => dispatch.navigate(Route::Confirm(ConfirmKind::PowerOff)),
            }
        }));
        list.scroll_to(START_ROW);
        self.state.borrow_mut().list = Rc::downgrade(&list);
        self.list = Some(list);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let mut state = self.state.borrow_mut();
        state.follow(snap);
        for (index, row) in ROWS.iter().enumerate() {
            let value = state.value(*row, snap);
            state.show(index, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::route::ConfirmKind;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};
    use waved_state::Command;

    fn value(screen: &SystemScreen, index: usize) -> String {
        screen
            .list
            .as_ref()
            .unwrap()
            .row(index)
            .child(2)
            .child(0)
            .text()
    }

    #[test]
    fn system_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.clock = "14:37".into();
            snap.settings.pre_recording = true; // as in system.png
            snap.ota.phase = OtaPhase::UpToDate;
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            // system.png: Pre-recording, About, Firmware update (focus, "Up to
            // date"), Factory reset, Power off in red.
            assert_golden("screen_system", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn rows_left_for_milestone_3_are_hidden() {
        assert_eq!(
            ROWS.map(SystemRow::title),
            [
                "Display",
                "Date and time",
                "Alert tones",
                "Pre-recording",
                "About",
                "Firmware update",
                "Factory reset",
                "Power off"
            ]
        );
    }

    #[test]
    fn switch_rows_toggle_and_show_the_new_value_at_once() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.settings.pre_recording = true;
            snap.settings.tones_enabled = false;
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            let list = screen.list.as_ref().unwrap();
            lv.click(list.row(3));
            lv.click(list.row(2));
            assert!(matches!(
                f.commands().as_slice(),
                [
                    Command::SetPreRecording(false),
                    Command::SetAlertTones(true)
                ]
            ));
            assert_eq!(value(&screen, 3), "Off");
            assert_eq!(value(&screen, 2), "On");
            assert_targets_ok(root);
        });
    }

    #[test]
    fn a_stale_snapshot_does_not_revert_a_tap() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.settings.pre_recording = true;
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            lv.click(screen.list.as_ref().unwrap().row(3));
            // The service has not confirmed yet: the snapshot still says On.
            screen.update(&snap);
            screen.update(&snap);
            assert_eq!(value(&screen, 3), "Off");
            // Confirmed: the snapshot is the truth again.
            snap.settings.pre_recording = false;
            screen.update(&snap);
            assert_eq!(value(&screen, 3), "Off");
            // A change made elsewhere is followed.
            snap.settings.pre_recording = true;
            screen.update(&snap);
            assert_eq!(value(&screen, 3), "On");
            assert_targets_ok(root);
        });
    }

    #[test]
    fn switches_follow_changes_made_elsewhere() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.settings.tones_enabled = true;
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            snap.settings.tones_enabled = false;
            screen.update(&snap);
            assert_eq!(value(&screen, 2), "Off");
            lv.click(screen.list.as_ref().unwrap().row(2));
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetAlertTones(true)]
            ));
            assert_targets_ok(root);
        });
    }

    #[test]
    fn a_double_tap_within_a_frame_never_sends_the_same_value_twice() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.settings.tones_enabled = false;
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            let row = screen.list.as_ref().unwrap().row(2);
            lv.click(row);
            lv.click(row);
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetAlertTones(true), Command::SetAlertTones(false)]
            ));
            assert_eq!(value(&screen, 2), "Off");
            // The service confirms the first value, then the second.
            snap.settings.tones_enabled = true;
            screen.update(&snap);
            assert_eq!(value(&screen, 2), "Off", "the second tap is still pending");
            snap.settings.tones_enabled = false;
            screen.update(&snap);
            assert_eq!(value(&screen, 2), "Off");
            assert_targets_ok(root);
        });
    }

    #[test]
    fn destructive_rows_open_the_confirmation() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap());
            let list = screen.list.as_ref().unwrap();
            lv.click(list.row(6));
            lv.click(list.row(7));
            assert!(f.commands().is_empty(), "nothing happens before confirming");
            assert_eq!(
                f.events(),
                vec![
                    UiEvent::Navigate(Route::Confirm(ConfirmKind::FactoryReset)),
                    UiEvent::Navigate(Route::Confirm(ConfirmKind::PowerOff)),
                ]
            );
            assert_targets_ok(root);
        });
    }

    #[test]
    fn link_rows_open_their_screens() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap());
            let list = screen.list.as_ref().unwrap();
            lv.click(list.row(0));
            lv.click(list.row(1));
            lv.click(list.row(4));
            assert!(f.commands().is_empty());
            assert_eq!(
                f.events(),
                vec![
                    UiEvent::Navigate(Route::Display),
                    UiEvent::Navigate(Route::DateTime),
                    UiEvent::Navigate(Route::About),
                ]
            );
            assert_targets_ok(root);
        });
    }

    #[test]
    fn values_read_the_snapshot() {
        let mut snap = snap();
        snap.settings.brightness_pct = 70;
        assert_eq!(SystemRow::Display.value(&snap).as_deref(), Some("70%"));
        assert_eq!(SystemRow::DateTime.value(&snap).as_deref(), Some("14:33"));
        assert_eq!(
            SystemRow::About.value(&snap),
            Some(format!("waved {}", env!("CARGO_PKG_VERSION")))
        );
        snap.system.about.version = "0.1.0 (374015f)".into();
        assert_eq!(
            SystemRow::About.value(&snap).as_deref(),
            Some("0.1.0 (374015f)")
        );
        assert_eq!(SystemRow::PowerOff.value(&snap), None);
        assert_eq!(SystemRow::FactoryReset.value(&snap), None);
    }

    #[test]
    fn the_about_value_follows_the_snapshot_after_build() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            snap.system.about.version = "0.2.0 (abc1234)".into();
            screen.update(&snap);
            assert_eq!(value(&screen, 4), "0.2.0 (abc1234)");
            assert_targets_ok(root);
        });
    }

    #[test]
    fn system_opens_on_firmware_update() {
        assert_eq!(ROWS[START_ROW], SystemRow::FirmwareUpdate);
    }

    #[test]
    fn the_update_row_follows_the_update() {
        let mut ota = OtaState::default();
        let cases = [
            (OtaPhase::Off, "Off"),
            (OtaPhase::Idle, "Check now"),
            (OtaPhase::Checking, "Checking"),
            (OtaPhase::UpToDate, "Up to date"),
            (OtaPhase::Available, "Available"),
            (OtaPhase::Downloading, "42%"),
            (OtaPhase::Waiting, "Ready"),
            (OtaPhase::Installing, "Installing"),
            (OtaPhase::RestartNeeded, "Restart"),
            (OtaPhase::Failed { reason: "x".into() }, "Failed"),
        ];
        ota.progress_pct = 42;
        for (phase, expected) in cases {
            ota.phase = phase;
            assert_eq!(update_value(&ota), expected);
        }
    }

    #[test]
    fn the_update_row_checks_or_opens_the_update_screen() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.ota.phase = OtaPhase::UpToDate;
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            let row = screen.list.as_ref().unwrap().row(START_ROW);
            lv.click(row);
            assert!(matches!(f.commands().as_slice(), [Command::CheckForUpdate]));
            assert!(f.events().is_empty(), "a check stays on System");

            snap.ota.phase = OtaPhase::Available;
            screen.update(&snap);
            assert_eq!(value(&screen, START_ROW), "Available");
            lv.click(row);
            assert!(matches!(f.commands().as_slice(), [Command::InstallUpdate]));
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::Update)]);

            snap.ota.phase = OtaPhase::Downloading;
            screen.update(&snap);
            lv.click(row);
            assert!(f.commands().is_empty(), "watching sends nothing");
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::Update)]);
            assert_targets_ok(root);
        });
    }

    #[test]
    fn after_a_failure_the_row_shows_why_before_anything_installs() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            // A version that rolled back: a tap must not reinstall it unseen.
            snap.ota.phase = OtaPhase::Failed {
                reason: "Version 0.10.0 didn't start, so Wave went back".into(),
            };
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            let row = screen.list.as_ref().unwrap().row(START_ROW);
            lv.click(row);
            assert!(f.commands().is_empty(), "the update screen has Retry");
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::Update)]);

            snap.ota.phase = OtaPhase::RestartNeeded;
            screen.update(&snap);
            assert_eq!(value(&screen, START_ROW), "Restart");
            lv.click(row);
            assert!(f.commands().is_empty(), "installed; only a restart is left");
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::Update)]);
            assert_targets_ok(root);
        });
    }
}
