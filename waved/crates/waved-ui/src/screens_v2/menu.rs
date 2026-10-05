//! Menu (B1): every setting with its current value, opened from the status
//! pill (menu.png). Mic height (B5) and Bluetooth (I4) join in milestone 3.
//! This file also holds the value words other settings screens reuse.

use crate::kit::components::buttons::back_button;
use crate::kit::components::list::{ListItem, RotaryList};
use crate::kit::components::status::StatusArea;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use waved_state::{AudioMode, NrLevel, UploadMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuRow {
    Files,
    AudioMode,
    NoiseReduction,
    Volume,
    Wifi,
    Uploads,
    System,
}

/// Milestone 2's rows, in the mockup's order (how often people need them).
pub const ROWS: [MenuRow; 7] = [
    MenuRow::Files,
    MenuRow::AudioMode,
    MenuRow::NoiseReduction,
    MenuRow::Volume,
    MenuRow::Wifi,
    MenuRow::Uploads,
    MenuRow::System,
];

/// The row centred when the menu opens: Noise reduction, as in menu.png, so
/// the mid-meeting controls sit around the focus line.
pub const START_ROW: usize = 2;

impl MenuRow {
    pub fn route(self) -> Route {
        match self {
            MenuRow::Files => Route::Files,
            MenuRow::AudioMode => Route::AudioMode,
            MenuRow::NoiseReduction => Route::NoiseReduction,
            MenuRow::Volume => Route::Volume,
            MenuRow::Wifi => Route::Wifi,
            MenuRow::Uploads => Route::Uploads,
            MenuRow::System => Route::System,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            MenuRow::Files => "Files",
            MenuRow::AudioMode => "Audio mode",
            MenuRow::NoiseReduction => "Noise reduction",
            MenuRow::Volume => "Volume",
            MenuRow::Wifi => "Wi-Fi",
            MenuRow::Uploads => "Uploads",
            MenuRow::System => "System",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            MenuRow::Files => icons::FOLDER,
            MenuRow::AudioMode => icons::PATTERN,
            MenuRow::NoiseReduction => icons::NOISE,
            MenuRow::Volume => icons::VOLUME_HIGH,
            MenuRow::Wifi => icons::WIFI,
            MenuRow::Uploads => icons::SEND,
            MenuRow::System => icons::SETTINGS,
        }
    }

    /// The live value at the right of the row; `None` shows nothing.
    pub fn value(self, snap: &UiSnapshot) -> Option<String> {
        match self {
            MenuRow::Files => {
                let count = snap.files.recordings.len();
                (count > 0).then(|| count.to_string())
            }
            MenuRow::AudioMode => Some(mode_name(&snap.audio.mode).into()),
            MenuRow::NoiseReduction => Some(nr_name(&snap.audio.nr_level).into()),
            MenuRow::Volume => Some(if snap.audio.speaker_muted {
                "Muted".into()
            } else {
                let volume = snap.audio.speaker_volume.min(100);
                format!("{volume}%")
            }),
            MenuRow::Wifi => Some(if !snap.wifi.connected {
                "Not connected".into()
            } else if snap.wifi.ssid.is_empty() {
                "Connected".into()
            } else {
                snap.wifi.ssid.clone()
            }),
            MenuRow::Uploads => Some(upload_mode_name(snap.settings.upload_mode).into()),
            MenuRow::System => None,
        }
    }

    /// The lock marks PIN rows (D10) once a PIN is set. Enforcement is K2.
    pub fn locked(self, snap: &UiSnapshot) -> bool {
        snap.settings.pin_enabled && self.route().requires_pin()
    }
}

pub fn mode_name(mode: &AudioMode) -> &'static str {
    match mode {
        AudioMode::Omni => "Whole room",
        AudioMode::Steered { .. } => "Follow talker",
        AudioMode::Cardioid => "Focused",
        AudioMode::Supercardioid => "Narrow",
        AudioMode::Figure8 => "Two sides",
        AudioMode::Stereo => "Stereo",
    }
}

pub fn nr_name(level: &NrLevel) -> &'static str {
    match level {
        NrLevel::Off => "Off",
        NrLevel::Low => "Low",
        NrLevel::Med => "Medium",
        NrLevel::High => "High",
    }
}

pub fn upload_mode_name(mode: UploadMode) -> &'static str {
    match mode {
        UploadMode::Automatic => "Automatic",
        UploadMode::Ask => "Ask me",
        UploadMode::LocalOnly => "Keep on Wave",
    }
}

#[derive(Default)]
pub struct MenuScreen {
    status: Option<StatusArea>,
    list: Option<RotaryList>,
    shown: Vec<Option<String>>,
}

impl Screen for MenuScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        self.shown = ROWS.iter().map(|row| row.value(snap)).collect();
        let items = ROWS
            .iter()
            .zip(&self.shown)
            .map(|(row, value)| {
                let item = ListItem::new(row.icon(), row.title()).locked(row.locked(snap));
                match value {
                    Some(value) => item.value(value.as_str()),
                    None => item,
                }
            })
            .collect();
        let dispatch = ctx.dispatch.clone();
        let list = RotaryList::new(root, items, move |index| {
            dispatch.navigate(ROWS[index].route())
        });
        list.scroll_to(START_ROW);
        self.list = Some(list);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let Some(list) = self.list.as_ref() else {
            return;
        };
        for (index, row) in ROWS.iter().enumerate() {
            let value = row.value(snap);
            if self.shown[index] != value {
                list.set_value(index, value.as_deref());
                self.shown[index] = value;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod fixture {
    use crate::kit::dispatch::{Dispatcher, UiEvent};
    use crate::kit::screen::ScreenCtx;
    use crate::kit::snapshot::UiSnapshot;
    use std::sync::{mpsc, Arc};
    use tokio::sync::broadcast;
    use uuid::Uuid;
    use waved_config::WavedConfig;
    use waved_state::{Command, RecordingStatus, RecordingSummary};

    pub struct Fixture {
        pub ctx: ScreenCtx,
        pub bus: broadcast::Receiver<Command>,
        pub ui: mpsc::Receiver<UiEvent>,
    }

    pub fn fixture() -> Fixture {
        fixture_with(WavedConfig::default())
    }

    pub fn fixture_with(config: WavedConfig) -> Fixture {
        let (dispatch, bus, ui) = Dispatcher::test();
        Fixture {
            ctx: ScreenCtx {
                dispatch,
                config: Arc::new(config),
            },
            bus,
            ui,
        }
    }

    impl Fixture {
        /// Every command sent since the last call.
        pub fn commands(&mut self) -> Vec<Command> {
            let mut out = Vec::new();
            while let Ok(command) = self.bus.try_recv() {
                out.push(command);
            }
            out
        }

        /// Every UI event sent since the last call.
        pub fn events(&self) -> Vec<UiEvent> {
            self.ui.try_iter().collect()
        }
    }

    /// The status cluster as the settings mockups draw it: USB, Wi-Fi, 14:33, 82 %.
    pub fn snap() -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.mcu.usb_connected = true;
        snap.wifi.connected = true;
        snap.wifi.ssid = "Office-5G".into();
        snap.wifi.ip_addr = "192.168.1.42".into();
        snap.wifi.signal_strength = -60;
        snap.clock = "14:33".into();
        snap.battery.level = 82;
        snap
    }

    pub fn recording(status: RecordingStatus) -> RecordingSummary {
        RecordingSummary {
            id: Uuid::new_v4(),
            started_at: None,
            duration_secs: 600,
            size_bytes: 10_000_000,
            status,
            marks: 0,
            marks_ms: Vec::new(),
            last_error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{fixture, recording, snap};
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use waved_state::RecordingStatus;

    fn menu_snap() -> UiSnapshot {
        let mut snap = snap();
        snap.clock = "14:32".into();
        snap.settings.pin_enabled = true;
        snap.files.recordings = (0..12).map(|_| recording(RecordingStatus::Uploaded)).collect();
        snap.audio.mode = AudioMode::Steered { initial_az_deg: 0.0 };
        snap.audio.nr_level = NrLevel::Med;
        snap.audio.speaker_volume = 60;
        snap
    }

    #[test]
    fn menu_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = MenuScreen::default();
            screen.build(root, &f.ctx, &menu_snap());
            // menu.png: Files (far), Audio mode, Noise reduction (focus), Volume, then
            // Wi-Fi where the mockup shows the hidden Mic height row.
            assert_golden("screen_menu", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn out_of_scope_rows_are_hidden() {
        assert_eq!(
            ROWS.map(MenuRow::title),
            ["Files", "Audio mode", "Noise reduction", "Volume", "Wi-Fi", "Uploads", "System"]
        );
    }

    #[test]
    fn rows_follow_the_snapshot() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = menu_snap();
            let mut screen = MenuScreen::default();
            screen.build(root, &f.ctx, &snap);
            let value = |screen: &MenuScreen, index: usize| {
                screen.list.as_ref().unwrap().row(index).child(2).child(0).text()
            };
            assert_eq!(value(&screen, 3), "60%");
            snap.audio.speaker_volume = 70;
            screen.update(&snap);
            assert_eq!(value(&screen, 3), "70%");
            snap.audio.speaker_muted = true;
            screen.update(&snap);
            assert_eq!(value(&screen, 3), "Muted");
            snap.wifi.connected = false;
            screen.update(&snap);
            assert_eq!(value(&screen, 4), "Not connected");
            assert_targets_ok(root);
        });
    }

    #[test]
    fn tapping_a_row_opens_its_screen() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = MenuScreen::default();
            screen.build(root, &f.ctx, &menu_snap());
            assert_targets_ok(root);
            lv.click(screen.list.as_ref().unwrap().row(1));
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::AudioMode)]);
        });
    }

    #[test]
    fn opening_a_row_sends_no_command() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = MenuScreen::default();
            screen.build(root, &f.ctx, &menu_snap());
            assert_targets_ok(root);
            lv.click(screen.list.as_ref().unwrap().row(4));
            assert!(f.commands().is_empty());
        });
    }

    #[test]
    fn the_lock_marks_pin_rows_only_when_a_pin_is_set() {
        let mut snap = snap();
        assert!(!MenuRow::Files.locked(&snap));
        snap.settings.pin_enabled = true;
        for row in [MenuRow::Files, MenuRow::Wifi, MenuRow::Uploads, MenuRow::System] {
            assert!(row.locked(&snap), "{row:?}");
        }
        for row in [MenuRow::AudioMode, MenuRow::NoiseReduction, MenuRow::Volume] {
            assert!(!row.locked(&snap), "{row:?}");
        }
    }

    #[test]
    fn the_value_vocabulary_matches_the_mockups() {
        assert_eq!(mode_name(&AudioMode::Omni), "Whole room");
        assert_eq!(mode_name(&AudioMode::Steered { initial_az_deg: 12.0 }), "Follow talker");
        assert_eq!(mode_name(&AudioMode::Cardioid), "Focused");
        assert_eq!(mode_name(&AudioMode::Supercardioid), "Narrow");
        assert_eq!(mode_name(&AudioMode::Figure8), "Two sides");
        assert_eq!(mode_name(&AudioMode::Stereo), "Stereo");
        assert_eq!(nr_name(&NrLevel::Med), "Medium");
        assert_eq!(upload_mode_name(UploadMode::Ask), "Ask me");
        assert_eq!(upload_mode_name(UploadMode::LocalOnly), "Keep on Wave");
    }
}
