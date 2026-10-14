//! Route -> screen. The runtime builds a fresh screen from here on every
//! navigation (`kit/README.md`, "Registering a screen").

use crate::kit::route::Route;
use crate::kit::screen::Screen;
use crate::screens_v2::{
    about, ambient, audio_mode, bluetooth, boot, call, confirm, datetime, decision, display,
    file_detail, files, first_run, home, menu, mic_height, noise, ota, pin, powering_off,
    recording, system, upload_mode, uploads, usb_storage, volume, wifi, wifi_setup,
};

/// The screen for `route`. `None` would be a route with no screen yet, which
/// the runtime backs out of; every route has one since G1.
pub fn screen_for(route: &Route) -> Option<Box<dyn Screen>> {
    let screen: Box<dyn Screen> = match route {
        Route::Boot => Box::new(boot::BootScreen::default()),
        Route::PoweringOff => Box::new(powering_off::PoweringOffScreen),
        Route::FirstRun => Box::new(first_run::FirstRunScreen::default()),
        Route::Home => Box::new(home::HomeScreen::default()),
        Route::Recording => Box::new(recording::RecordingScreen::default()),
        Route::Decision => Box::new(decision::DecisionScreen::default()),
        Route::Resting => Box::new(ambient::AmbientScreen::default()),
        Route::Menu => Box::new(menu::MenuScreen::default()),
        Route::AudioMode => Box::new(audio_mode::AudioModeScreen::default()),
        Route::NoiseReduction => Box::new(noise::NoiseScreen::default()),
        Route::Volume => Box::new(volume::VolumeScreen::speaker()),
        Route::MicLevel => Box::new(volume::VolumeScreen::mic()),
        Route::MicHeight => Box::new(mic_height::MicHeightScreen::default()),
        Route::Files => Box::new(files::FilesScreen::default()),
        Route::FileDetail(id) => Box::new(file_detail::FileDetailScreen::new(*id)),
        Route::Wifi => Box::new(wifi::WifiScreen::default()),
        Route::WifiSetup => Box::new(wifi_setup::WifiSetupScreen::default()),
        Route::Uploads => Box::new(uploads::UploadsScreen::default()),
        Route::UploadMode => Box::new(upload_mode::UploadModeScreen::default()),
        Route::System => Box::new(system::SystemScreen::default()),
        Route::Display => Box::new(display::DisplayScreen::default()),
        Route::DimAfter => Box::new(display::DimAfterScreen::default()),
        Route::DateTime => Box::new(datetime::DateTimeScreen::default()),
        Route::TimeZone => Box::new(datetime::TimeZoneScreen::default()),
        Route::About => Box::new(about::AboutScreen::default()),
        Route::Confirm(kind) => Box::new(confirm::ConfirmScreen::new(*kind)),
        Route::Pin(_) => Box::new(pin::PinScreen::default()),
        Route::Update => Box::new(ota::UpdateScreen::default()),
        Route::Bluetooth => Box::new(bluetooth::BluetoothScreen::default()),
        Route::IncomingCall => Box::new(call::CallScreen::default()),
        Route::UsbStorage => Box::new(usb_storage::UsbStorageScreen::default()),
    };
    Some(screen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::{Dispatcher, Message, Severity};
    use crate::kit::route::ConfirmKind;
    use crate::kit::screen::ScreenCtx;
    use crate::kit::snapshot::{ActiveMessage, UiSnapshot};
    use crate::kit::testkit::{assert_targets_ok, lvgl};
    use crate::runtime::testing::shows_text;
    use std::path::PathBuf;
    use std::sync::Arc;
    use uuid::Uuid;
    use waved_config::WavedConfig;
    use waved_state::{PendingDecision, RecordingStatus, RecordingSummary};

    const FILE: Uuid = Uuid::from_u128(0x5eed);

    fn m2_routes() -> Vec<Route> {
        vec![
            Route::Boot,
            Route::PoweringOff,
            Route::FirstRun,
            Route::Home,
            Route::Recording,
            Route::Decision,
            Route::Resting,
            Route::Menu,
            Route::AudioMode,
            Route::NoiseReduction,
            Route::Volume,
            Route::MicLevel,
            Route::Files,
            Route::FileDetail(FILE),
            Route::Wifi,
            Route::WifiSetup,
            Route::Uploads,
            Route::UploadMode,
            Route::System,
            Route::Display,
            Route::DimAfter,
            Route::DateTime,
            Route::TimeZone,
            Route::About,
            Route::Confirm(ConfirmKind::PowerOff),
            Route::Confirm(ConfirmKind::FactoryReset),
            Route::Confirm(ConfirmKind::DeleteRecording(FILE)),
            Route::Confirm(ConfirmKind::StopUsbStorage),
            Route::Confirm(ConfirmKind::ChangeNetwork),
            Route::Confirm(ConfirmKind::TurnOffPin),
            Route::Confirm(ConfirmKind::PairAnotherPhone),
            // Milestone 3 screens built so far.
            Route::MicHeight,
            Route::Update,
            Route::Bluetooth,
            Route::IncomingCall,
            Route::UsbStorage,
        ]
    }

    fn fixture() -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.settings.first_run_done = true;
        snap.clock = "14:32".into();
        snap.battery.level = 64;
        snap.battery.voltage = 3_800;
        snap.mcu.usb_connected = true;
        snap.now_ms = 10_000;
        snap.now_unix_s = 1_759_588_320;
        snap.meeting.pending_decision = Some(PendingDecision {
            id: FILE,
            audio_path: PathBuf::from("/mnt/udisk/recordings/x/audio.wav"),
            duration_secs: 2537,
            size_bytes: 184_000_000,
            endpoint: "https://notes.example.com/hook".into(),
            speakers: 3,
            marks: 2,
        });
        snap.files.recordings = vec![RecordingSummary {
            id: FILE,
            started_at: Some("2026-10-05T14:32:00Z".into()),
            duration_secs: 2537,
            size_bytes: 184_000_000,
            status: RecordingStatus::Uploaded,
            marks: 2,
            marks_ms: vec![60_000, 900_000],
            last_error: None,
        }];
        snap.files.free_bytes = 12_000_000_000;
        snap
    }

    fn build(route: &Route, snap: &UiSnapshot, check: impl FnOnce(crate::kit::obj::Obj)) {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let ctx = ScreenCtx {
                dispatch,
                config: Arc::new(WavedConfig::default()),
            };
            let mut screen = screen_for(route).unwrap_or_else(|| panic!("no screen for {route:?}"));
            screen.build(root, &ctx, snap);
            screen.update(snap);
            check(root);
        });
    }

    #[test]
    fn every_m2_route_builds_inside_the_tap_limit() {
        let snap = fixture();
        for route in m2_routes() {
            build(&route, &snap, assert_targets_ok);
        }
    }

    #[test]
    fn screens_cope_with_missing_data() {
        // The runtime can show a screen one frame before or after its data.
        let snap = UiSnapshot::default();
        for route in m2_routes() {
            build(&route, &snap, |_| {});
        }
    }

    /// Routes whose approved mockup has no status slot (checked against
    /// `docs/mockups/png/`): `boot.png` shows only the spinner ring, "Starting"
    /// and the version; `ambient.png` shows only the clock ring and the
    /// "Pre-recording" line. Every other screen, `confirm.png` included, has
    /// the status bar row, and each is exercised.
    fn has_no_status_slot(route: &Route) -> bool {
        matches!(route, Route::Boot | Route::PoweringOff | Route::Resting)
    }

    #[test]
    fn every_screen_with_a_status_shows_runtime_messages() {
        let mut snap = fixture();
        snap.message = Some(ActiveMessage {
            seq: 1,
            message: Message::new(Severity::Info, "Probe message"),
            since_ms: snap.now_ms,
        });
        for route in m2_routes() {
            if has_no_status_slot(&route) {
                continue;
            }
            build(&route, &snap, |root| {
                assert!(shows_text(root, "Probe message"), "{route:?} hides runtime messages");
            });
        }
    }

    #[test]
    fn usb_storage_builds_with_its_way_out() {
        let mut snap = fixture();
        snap.usb_storage.phase = waved_state::UsbStoragePhase::On;
        build(&Route::UsbStorage, &snap, |root| {
            assert!(shows_text(root, usb_storage::STOP));
            assert_targets_ok(root);
        });
    }

    #[test]
    fn a_ringing_call_builds_with_its_number() {
        let mut snap = fixture();
        snap.bt.connected = true;
        snap.bt.call = waved_state::BtCall::Ringing {
            number: "+32470123456".into(),
        };
        build(&Route::IncomingCall, &snap, |root| {
            assert!(shows_text(root, "+32470123456"));
            assert_targets_ok(root);
        });
    }
}
