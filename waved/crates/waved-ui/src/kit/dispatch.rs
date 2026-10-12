//! How screens talk to services (bus commands) and to the UI runtime (UiEvent).

use crate::kit::route::Route;
use crate::kit::tokens::{MESSAGE_MS_LONG, MESSAGE_MS_SHORT};
use std::sync::mpsc;
use tokio::sync::broadcast;
use waved_state::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Success,
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn duration_ms(self) -> u64 {
        match self {
            Severity::Success | Severity::Info => MESSAGE_MS_SHORT,
            Severity::Warning | Severity::Error => MESSAGE_MS_LONG,
        }
    }
}

/// A one-line message shown in the status slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub severity: Severity,
    /// A glyph in place of the severity's icon, in the severity's colour
    /// (the battery messages show the battery, as in the mockup).
    pub icon: Option<&'static str>,
}

impl Message {
    pub fn new(severity: Severity, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            severity,
            icon: None,
        }
    }

    pub fn with_icon(mut self, glyph: &'static str) -> Self {
        self.icon = Some(glyph);
        self
    }
}

/// Digits typed on the PIN screen, on their way to the runtime's check
/// (`UiEvent::PinEntered`). Debug output never shows them.
#[derive(Clone, PartialEq, Eq)]
pub struct PinCode(String);

impl PinCode {
    pub fn new(digits: impl Into<String>) -> Self {
        Self(digits.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PinCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PinCode(..)")
    }
}

/// Requests for the UI runtime (navigation, messages, power states).
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Navigate(Route),
    Back,
    /// Return to the meeting base (Home, Recording or Decision).
    ToBase,
    /// The runtime's PIN check accepted the code; open the route it guarded.
    /// Only the runtime routes this: one a screen sends is ignored.
    PinAccepted,
    /// The PIN screen's digits, for the runtime to check (K2).
    PinEntered(PinCode),
    /// Services are up; leave the start-up screen.
    BootDone,
    /// No touch for the dim timeout: show the resting screen and relock.
    Rest,
    /// A touch or the power key woke the screen.
    Wake,
    Show(Message),
}

#[derive(Clone)]
pub struct Dispatcher {
    bus: broadcast::Sender<Command>,
    ui: mpsc::Sender<UiEvent>,
}

impl Dispatcher {
    pub fn new(bus: broadcast::Sender<Command>, ui: mpsc::Sender<UiEvent>) -> Self {
        Self { bus, ui }
    }

    /// Send a command to the services. Like the legacy UI, a send with no
    /// subscribers is dropped.
    pub fn command(&self, command: Command) {
        let _ = self.bus.send(command);
    }

    pub fn ui(&self, event: UiEvent) {
        let _ = self.ui.send(event);
    }

    pub fn navigate(&self, route: Route) {
        self.ui(UiEvent::Navigate(route));
    }

    pub fn back(&self) {
        self.ui(UiEvent::Back);
    }

    pub fn show(&self, severity: Severity, text: impl Into<String>) {
        self.ui(UiEvent::Show(Message::new(severity, text)));
    }

    /// A dispatcher wired to fresh channels, for tests.
    #[cfg(test)]
    pub fn test() -> (Self, broadcast::Receiver<Command>, mpsc::Receiver<UiEvent>) {
        let (bus, bus_rx) = broadcast::channel(64);
        let (ui, ui_rx) = mpsc::channel();
        (Self::new(bus, ui), bus_rx, ui_rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waved_state::Command;

    #[test]
    fn commands_reach_the_bus_and_ui_events_the_runtime() {
        let (dispatch, mut bus, ui) = Dispatcher::test();
        dispatch.command(Command::ToggleMicMute);
        dispatch.navigate(Route::Menu);
        dispatch.back();
        dispatch.show(Severity::Success, "Recording sent");
        assert!(matches!(bus.try_recv(), Ok(Command::ToggleMicMute)));
        assert_eq!(ui.try_recv().unwrap(), UiEvent::Navigate(Route::Menu));
        assert_eq!(ui.try_recv().unwrap(), UiEvent::Back);
        assert_eq!(
            ui.try_recv().unwrap(),
            UiEvent::Show(Message::new(Severity::Success, "Recording sent"))
        );
    }

    #[test]
    fn a_pin_never_shows_in_debug_output() {
        let event = UiEvent::PinEntered(PinCode::new("4821"));
        assert!(!format!("{event:?}").contains("4821"), "{event:?}");
        assert_eq!(PinCode::new("4821").as_str(), "4821");
    }

    #[test]
    fn warnings_and_errors_stay_longer() {
        assert_eq!(Severity::Success.duration_ms(), 2_000);
        assert_eq!(Severity::Info.duration_ms(), 2_000);
        assert_eq!(Severity::Warning.duration_ms(), 4_000);
        assert_eq!(Severity::Error.duration_ms(), 4_000);
    }
}
