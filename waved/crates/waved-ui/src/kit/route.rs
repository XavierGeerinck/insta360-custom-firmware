//! Every screen the redesign has, as a navigation target.

use uuid::Uuid;

/// What a confirmation dialog confirms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmKind {
    PowerOff,
    FactoryReset,
    DeleteRecording(Uuid),
    StopUsbStorage,
    /// Wi-Fi "Change network": leave the current network for the setup network (B7).
    ChangeNetwork,
    /// System -> PIN: turn the device PIN off (D10). Reaching System already
    /// needed the PIN, so this only confirms; a new PIN is set on the setup page.
    TurnOffPin,
    /// Bluetooth "Pair another phone": drops the connected phone (stock's
    /// "Connecting a new device will disconnect from <phone>. Continue?").
    PairAnotherPhone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Boot,
    /// The unit is powering off (a 2 s hold, or System → Power off). Final:
    /// nothing navigates away while the MCU retracts the lift and cuts power.
    PoweringOff,
    FirstRun,
    Home,
    Recording,
    Decision,
    Resting,
    Menu,
    AudioMode,
    NoiseReduction,
    Volume,
    MicLevel,
    MicHeight,
    Files,
    FileDetail(Uuid),
    Wifi,
    WifiSetup,
    Bluetooth,
    Uploads,
    UploadMode,
    System,
    Display,
    /// Dim after radio list, opened from Display.
    DimAfter,
    DateTime,
    /// Time zone radio list, opened from Date and time.
    TimeZone,
    About,
    Confirm(ConfirmKind),
    /// PIN entry in front of a locked route.
    Pin(Box<Route>),
    UsbStorage,
    Update,
    IncomingCall,
}

impl Route {
    /// The bottom of the navigation stack, chosen by the meeting state.
    pub fn is_base(&self) -> bool {
        matches!(self, Route::Home | Route::Recording | Route::Decision)
    }

    /// The PIN-protected entry point (D10) this screen belongs to; an entry
    /// point returns itself. `None` for screens the PIN never locks.
    pub fn guarded_by(&self) -> Option<Route> {
        match self {
            Route::Files
            | Route::FileDetail(_)
            | Route::Confirm(ConfirmKind::DeleteRecording(_)) => Some(Route::Files),
            Route::Wifi | Route::WifiSetup | Route::Confirm(ConfirmKind::ChangeNetwork) => {
                Some(Route::Wifi)
            }
            Route::Bluetooth | Route::Confirm(ConfirmKind::PairAnotherPhone) => {
                Some(Route::Bluetooth)
            }
            Route::Uploads | Route::UploadMode => Some(Route::Uploads),
            Route::System
            | Route::Display
            | Route::DimAfter
            | Route::DateTime
            | Route::TimeZone
            | Route::About
            | Route::Update
            | Route::Confirm(ConfirmKind::FactoryReset | ConfirmKind::TurnOffPin) => {
                Some(Route::System)
            }
            // Power off stays open: the power key's long press must always work.
            // USB storage is started from System, so its PIN guards that; the
            // takeover and stopping stay open (G1): pulling the cable stops it
            // anyway.
            _ => None,
        }
    }

    /// Entry points the PIN protects (D10). Screens opened from them inherit the unlock.
    pub fn requires_pin(&self) -> bool {
        self.guarded_by().as_ref() == Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pin_guards_exactly_the_d10_entry_points() {
        let guarded = [
            Route::Files,
            Route::Wifi,
            Route::Bluetooth,
            Route::Uploads,
            Route::System,
        ];
        for route in guarded {
            assert!(route.requires_pin(), "{route:?}");
        }
        for open in [
            Route::Home,
            Route::Menu,
            Route::AudioMode,
            Route::NoiseReduction,
            Route::Volume,
            Route::MicHeight,
        ] {
            assert!(!open.requires_pin(), "{open:?}");
        }
    }

    #[test]
    fn every_screen_under_a_locked_entry_point_is_guarded_by_it() {
        let id = Uuid::new_v4();
        let cases = [
            (Route::Files, Route::Files),
            (Route::FileDetail(id), Route::Files),
            (
                Route::Confirm(ConfirmKind::DeleteRecording(id)),
                Route::Files,
            ),
            (Route::Wifi, Route::Wifi),
            (Route::WifiSetup, Route::Wifi),
            (Route::Bluetooth, Route::Bluetooth),
            (Route::Uploads, Route::Uploads),
            (Route::UploadMode, Route::Uploads),
            (Route::System, Route::System),
            (Route::Display, Route::System),
            (Route::DateTime, Route::System),
            (Route::About, Route::System),
            (Route::Update, Route::System),
            (Route::Confirm(ConfirmKind::FactoryReset), Route::System),
            (Route::Confirm(ConfirmKind::TurnOffPin), Route::System),
            (Route::TimeZone, Route::System),
            (Route::DimAfter, Route::System),
            (Route::Confirm(ConfirmKind::ChangeNetwork), Route::Wifi),
            (Route::Confirm(ConfirmKind::PairAnotherPhone), Route::Bluetooth),
        ];
        for (route, entry) in cases {
            assert_eq!(route.guarded_by(), Some(entry), "{route:?}");
        }
        for open in [
            Route::Home,
            Route::Menu,
            Route::AudioMode,
            Route::NoiseReduction,
            Route::Volume,
            Route::MicLevel,
            Route::MicHeight,
            Route::Confirm(ConfirmKind::PowerOff),
            Route::Pin(Box::new(Route::Files)),
            Route::Resting,
            Route::IncomingCall,
            Route::UsbStorage,
            Route::Confirm(ConfirmKind::StopUsbStorage),
        ] {
            assert_eq!(open.guarded_by(), None, "{open:?}");
        }
    }

    #[test]
    fn sub_lists_and_warnings_are_not_entry_points() {
        for route in [
            Route::TimeZone,
            Route::DimAfter,
            Route::Confirm(ConfirmKind::ChangeNetwork),
            Route::Confirm(ConfirmKind::PairAnotherPhone),
        ] {
            assert!(!route.requires_pin(), "{route:?}");
            assert!(!route.is_base(), "{route:?}");
        }
    }

    #[test]
    fn only_meeting_screens_are_bases() {
        assert!(Route::Home.is_base() && Route::Recording.is_base() && Route::Decision.is_base());
        assert!(!Route::Menu.is_base() && !Route::Resting.is_base());
    }
}
