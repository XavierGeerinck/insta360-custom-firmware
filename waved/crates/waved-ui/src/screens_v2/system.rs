//! System (B6, system.png): the same rotary list as the menu. USB storage
//! (G1) starts at once; its screen takes over and confirms the stop. The PIN
//! row shows On/Off (D10): with a PIN set a tap confirms turning it off
//! (reaching System already required the PIN), and with none a tap says where
//! to set one — the setup page. Power off and Factory reset go through the
//! confirmation.
//! The two switch rows and Recording format are optimistic: a tap shows the
//! new value at once and a stale snapshot does not revert it (see
//! `menu::Pending`). Recording format toggles Opus / FLAC for the next take
//! (docs/AUDIO_HARDWARE.md "Recording formats"). Firmware update (H3)
//! checks, or opens the update screen when there is something to install or
//! watch.

use crate::kit::components::buttons::back_button;
use crate::kit::components::list::{ListItem, RotaryList};
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::{Dispatcher, Severity};
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::{ConfirmKind, Route};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::screens_v2::menu::Pending;
use crate::screens_v2::messages::copy;
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use waved_state::{Command, OtaPhase, OtaState, RecordingFormat};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemRow {
    Display,
    DateTime,
    AlertTones,
    PreRecording,
    RecordingFormat,
    UsbStorage,
    Pin,
    About,
    FirmwareUpdate,
    FactoryReset,
    PowerOff,
}

pub const ROWS: [SystemRow; 11] = [
    SystemRow::Display,
    SystemRow::DateTime,
    SystemRow::AlertTones,
    SystemRow::PreRecording,
    SystemRow::RecordingFormat,
    SystemRow::UsbStorage,
    SystemRow::Pin,
    SystemRow::About,
    SystemRow::FirmwareUpdate,
    SystemRow::FactoryReset,
    SystemRow::PowerOff,
];

/// Firmware update is centred when System opens, as in system.png.
pub const START_ROW: usize = 8;

fn on_off(on: bool) -> String {
    if on { "On" } else { "Off" }.into()
}

fn format_name(format: RecordingFormat) -> String {
    match format {
        RecordingFormat::Opus => "Opus",
        RecordingFormat::Flac => "FLAC",
    }
    .into()
}

/// The other format: a tap on the row switches between the two.
fn other_format(format: RecordingFormat) -> RecordingFormat {
    match format {
        RecordingFormat::Opus => RecordingFormat::Flac,
        RecordingFormat::Flac => RecordingFormat::Opus,
    }
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
            SystemRow::RecordingFormat => "Recording format",
            SystemRow::UsbStorage => "USB storage",
            SystemRow::Pin => "PIN",
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
            SystemRow::RecordingFormat => icons::RECORD,
            SystemRow::UsbStorage => icons::USB,
            SystemRow::Pin => icons::LOCK,
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
            SystemRow::RecordingFormat => Some(format_name(snap.settings.recording_format)),
            SystemRow::Pin => Some(on_off(snap.settings.pin_enabled)),
            SystemRow::About => Some(super::about::version_line(snap)),
            SystemRow::FirmwareUpdate => Some(update_value(&snap.ota)),
            SystemRow::UsbStorage | SystemRow::FactoryReset | SystemRow::PowerOff => None,
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
    format: Pending<RecordingFormat>,
    /// The update phase as of the last snapshot, for the row's tap.
    ota: OtaPhase,
    /// Whether a device PIN is set, for the PIN row's value and tap.
    pin_enabled: bool,
    /// What each row shows now.
    shown: Vec<Option<String>>,
    list: Weak<RotaryList>,
}

impl State {
    fn follow(&mut self, snap: &UiSnapshot) {
        self.tones.follow(snap.settings.tones_enabled);
        self.pre_recording.follow(snap.settings.pre_recording);
        self.format.follow(snap.settings.recording_format);
        self.ota = snap.ota.phase.clone();
        self.pin_enabled = snap.settings.pin_enabled;
    }

    fn value(&self, row: SystemRow, snap: &UiSnapshot) -> Option<String> {
        match row {
            SystemRow::AlertTones => self.tones.value().map(on_off),
            SystemRow::PreRecording => self.pre_recording.value().map(on_off),
            SystemRow::RecordingFormat => self.format.value().map(format_name),
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
            state.format = Pending::new(snap.settings.recording_format);
            state.ota = snap.ota.phase.clone();
            state.pin_enabled = snap.settings.pin_enabled;
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
                SystemRow::RecordingFormat => {
                    let mut state = state.borrow_mut();
                    let next = other_format(state.format.value().unwrap_or_default());
                    state.format.set(next);
                    state.show(index, Some(format_name(next)));
                    dispatch.command(Command::SetRecordingFormat(next));
                }
                SystemRow::UsbStorage => dispatch.command(Command::StartUsbStorage),
                SystemRow::Pin => {
                    if state.borrow().pin_enabled {
                        // System is already behind the PIN, so turning it off
                        // only confirms (D10). A new PIN is set on the setup page.
                        dispatch.navigate(Route::Confirm(ConfirmKind::TurnOffPin));
                    } else {
                        dispatch.show(Severity::Info, copy::PIN_ON_SETUP_PAGE);
                    }
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
    use crate::kit::dispatch::{Message, UiEvent};
    use crate::kit::route::ConfirmKind;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};
    use waved_state::{Command, RecordingFormat};

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
    fn the_usb_storage_and_pin_rows_show() {
        // USB storage (G1) slots in before PIN (K), as in the mockup's list.
        assert_eq!(
            ROWS.map(SystemRow::title),
            [
                "Display",
                "Date and time",
                "Alert tones",
                "Pre-recording",
                "Recording format",
                "USB storage",
                "PIN",
                "About",
                "Firmware update",
                "Factory reset",
                "Power off"
            ]
        );
    }

    #[test]
    fn the_recording_format_row_follows_the_setting() {
        let mut snap = snap();
        assert_eq!(
            SystemRow::RecordingFormat.value(&snap).as_deref(),
            Some("Opus")
        );
        snap.settings.recording_format = RecordingFormat::Flac;
        assert_eq!(
            SystemRow::RecordingFormat.value(&snap).as_deref(),
            Some("FLAC")
        );
        assert_eq!(ROWS[4], SystemRow::RecordingFormat, "right after Pre-recording");
    }

    #[test]
    fn a_tap_switches_the_recording_format_for_the_next_take() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            let row = screen.list.as_ref().unwrap().row(4);
            lv.click(row);
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetRecordingFormat(RecordingFormat::Flac)]
            ));
            assert_eq!(value(&screen, 4), "FLAC", "shown at once");
            // The service has not confirmed yet: a stale snapshot keeps FLAC.
            screen.update(&snap);
            assert_eq!(value(&screen, 4), "FLAC");
            snap.settings.recording_format = RecordingFormat::Flac;
            screen.update(&snap);
            lv.click(row);
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetRecordingFormat(RecordingFormat::Opus)]
            ));
            assert_eq!(value(&screen, 4), "Opus");
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_recording_format_row_in_view() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.clock = "14:37".into();
            snap.settings.pre_recording = true;
            snap.settings.recording_format = RecordingFormat::Flac;
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            screen.list.as_ref().unwrap().scroll_to(4);
            assert_golden("screen_system_recording_format", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_pin_row_reads_on_when_a_pin_is_set() {
        let mut snap = snap();
        assert_eq!(SystemRow::Pin.value(&snap).as_deref(), Some("Off"));
        snap.settings.pin_enabled = true;
        assert_eq!(SystemRow::Pin.value(&snap).as_deref(), Some("On"));
    }

    #[test]
    fn tapping_pin_with_a_pin_set_confirms_turning_it_off() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.settings.pin_enabled = true;
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            lv.click(screen.list.as_ref().unwrap().row(6));
            assert!(f.commands().is_empty(), "clearing waits for the confirm");
            assert_eq!(
                f.events(),
                vec![UiEvent::Navigate(Route::Confirm(ConfirmKind::TurnOffPin))]
            );
            assert_targets_ok(root);
        });
    }

    #[test]
    fn tapping_pin_with_no_pin_points_to_the_setup_page() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let snap = snap(); // pin_enabled is false by default
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap);
            lv.click(screen.list.as_ref().unwrap().row(6));
            assert!(f.commands().is_empty());
            assert_eq!(
                f.events(),
                vec![UiEvent::Show(Message::new(
                    Severity::Info,
                    copy::PIN_ON_SETUP_PAGE
                ))]
            );
            assert_targets_ok(root);
        });
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
            lv.click(list.row(9));
            lv.click(list.row(10));
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
            lv.click(list.row(7));
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
            assert_eq!(value(&screen, 7), "0.2.0 (abc1234)");
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_usb_storage_row_starts_it() {
        // No confirmation to start (the mockup confirms only the stop); the
        // takeover screen follows the state. Nothing else happens on the row.
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = SystemScreen::default();
            screen.build(root, &f.ctx, &snap());
            assert_eq!(SystemRow::UsbStorage.value(&snap()), None);
            lv.click(screen.list.as_ref().unwrap().row(5));
            assert!(matches!(f.commands().as_slice(), [Command::StartUsbStorage]));
            assert!(f.events().is_empty());
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
