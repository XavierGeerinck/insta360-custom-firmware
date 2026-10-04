//! Every screen the redesign has, as a navigation target.

use uuid::Uuid;

/// What a confirmation dialog confirms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmKind {
    PowerOff,
    FactoryReset,
    DeleteRecording(Uuid),
    StopUsbStorage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Boot,
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
    DateTime,
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

    /// Entry points the PIN protects (D10). Screens opened from them inherit the unlock.
    pub fn requires_pin(&self) -> bool {
        matches!(
            self,
            Route::Files | Route::Wifi | Route::Bluetooth | Route::Uploads | Route::System
        )
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
    fn only_meeting_screens_are_bases() {
        assert!(Route::Home.is_base() && Route::Recording.is_base() && Route::Decision.is_base());
        assert!(!Route::Menu.is_base() && !Route::Resting.is_base());
    }
}
