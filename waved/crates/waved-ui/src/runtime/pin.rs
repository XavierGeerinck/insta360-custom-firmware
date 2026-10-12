//! The PIN check behind the PIN screen (K2, D10). The screen sends its digits
//! (`UiEvent::PinEntered`); the runtime checks them here against config.toml's
//! PIN (`waved_config::PinAttempts`: five wrong tries, then a one-minute wait)
//! and only then routes `PinAccepted`. One gate serves every PIN screen, so
//! Cancel and reopen does not reset the count; a restart does.

use crate::kit::dispatch::PinCode;
use crate::kit::snapshot::PinStatus;
use waved_config::{PinAttempts, PinCheck, SecurityConfig};

#[derive(Debug, Default)]
pub struct PinGate {
    attempts: PinAttempts,
    /// Wrong tries since start (`PinStatus::wrong`).
    wrong: u32,
}

impl PinGate {
    /// Whether `code` opens the lock. A wrong try counts; a try during the
    /// wait is not checked and does not count.
    pub fn check(&mut self, security: &SecurityConfig, code: &PinCode, now_ms: u64) -> bool {
        match self.attempts.check(security, code.as_str(), now_ms) {
            PinCheck::Accepted => true,
            PinCheck::Wrong { .. } => {
                self.wrong += 1;
                false
            }
            PinCheck::Locked { .. } => false,
        }
    }

    /// What the PIN screen shows (`UiSnapshot::pin`).
    pub fn status(&self, now_ms: u64) -> PinStatus {
        PinStatus {
            wrong: self.wrong,
            locked_until_ms: self.attempts.locked_until(now_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::route::Route;
    use crate::kit::screen::Screen;
    use crate::kit::snapshot::UiSnapshot;
    use crate::kit::testkit::{lvgl, Lvgl};
    use crate::runtime::power::{BacklightOut, PowerDriver};
    use crate::runtime::{registry, Runtime, BOOT_MIN_MS};
    use std::cell::Cell;
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use waved_config::{hash_pin, WavedConfig, PIN_LOCKOUT_MS};
    use waved_state::Command;

    fn security(pin: &str) -> SecurityConfig {
        SecurityConfig {
            device_password: hash_pin(pin),
            ..SecurityConfig::default()
        }
    }

    fn code(digits: &str) -> PinCode {
        PinCode::new(digits)
    }

    #[test]
    fn the_gate_counts_wrong_tries_and_waits_after_five() {
        let pin = security("4821");
        let mut gate = PinGate::default();
        assert_eq!(gate.status(0), PinStatus::default());
        for n in 1..=4 {
            assert!(!gate.check(&pin, &code("0000"), 1_000));
            assert_eq!(gate.status(1_000).wrong, n);
            assert_eq!(gate.status(1_000).locked_until_ms, None);
        }
        assert!(!gate.check(&pin, &code("0000"), 1_000));
        let until = 1_000 + PIN_LOCKOUT_MS;
        assert_eq!(
            gate.status(1_000),
            PinStatus {
                wrong: 5,
                locked_until_ms: Some(until)
            }
        );
        assert!(!gate.check(&pin, &code("4821"), until - 1), "waiting");
        assert_eq!(
            gate.status(until - 1).wrong,
            5,
            "a try while waiting does not count"
        );
        assert_eq!(gate.status(until).locked_until_ms, None);
        assert!(gate.check(&pin, &code("4821"), until));
    }

    #[test]
    fn without_a_saved_pin_nothing_opens() {
        let mut gate = PinGate::default();
        assert!(!gate.check(&SecurityConfig::default(), &code("0000"), 0));
    }

    struct Dark;

    impl BacklightOut for Dark {
        fn set_percent(&mut self, _pct: u8) -> std::io::Result<bool> {
            Ok(true)
        }
    }

    thread_local! {
        /// PIN screens the registry built.
        static PIN_BUILDS: Cell<u32> = const { Cell::new(0) };
    }

    /// The real registry, counting PIN screens.
    fn counting(route: &Route) -> Option<Box<dyn Screen>> {
        if matches!(route, Route::Pin(_)) {
            PIN_BUILDS.with(|n| n.set(n.get() + 1));
        }
        registry::screen_for(route)
    }

    /// The real screens, a config with PIN 4821, booted to Home.
    struct Rig {
        rt: Runtime,
        _bus: broadcast::Receiver<Command>,
        pin_enabled: bool,
    }

    impl Rig {
        fn new(lv: &mut Lvgl, pin: Option<&str>) -> Self {
            lv.screen();
            let mut config = WavedConfig::default();
            if let Some(pin) = pin {
                config.security.device_password = hash_pin(pin);
            }
            let (bus, rx) = broadcast::channel(16);
            PIN_BUILDS.with(|n| n.set(0));
            let rt = Runtime::new(
                bus,
                Arc::new(config),
                Box::new(counting),
                PowerDriver::new(0, Box::new(Dark)),
            );
            let mut rig = Rig {
                rt,
                _bus: rx,
                pin_enabled: pin.is_some(),
            };
            rig.step(0);
            rig.step(BOOT_MIN_MS);
            assert_eq!(rig.rt.shown_route(), Some(&Route::Home));
            rig
        }

        fn snap(&self, now_ms: u64) -> UiSnapshot {
            let mut snap = UiSnapshot::default();
            snap.settings.first_run_done = true;
            snap.settings.pin_enabled = self.pin_enabled;
            snap.now_ms = now_ms;
            snap
        }

        fn step(&mut self, now_ms: u64) -> Option<Route> {
            let snap = self.snap(now_ms);
            self.rt.step(snap, &[]);
            self.rt.shown_route().cloned()
        }

        fn send(&mut self, event: UiEvent, now_ms: u64) -> Option<Route> {
            self.rt.dispatcher().ui(event);
            self.step(now_ms)
        }

        fn enter(&mut self, digits: &str, now_ms: u64) -> Option<Route> {
            self.send(UiEvent::PinEntered(code(digits)), now_ms)
        }
    }

    fn pin_for(route: Route) -> Option<Route> {
        Some(Route::Pin(Box::new(route)))
    }

    #[test]
    fn a_set_pin_puts_the_keypad_in_front_of_files() {
        lvgl(|lv| {
            let mut rig = Rig::new(lv, Some("4821"));
            assert_eq!(
                rig.send(UiEvent::Navigate(Route::Files), 2_000),
                pin_for(Route::Files)
            );
            assert_eq!(rig.enter("4821", 2_100), Some(Route::Files));
            assert_eq!(rig.rt.depth(), 2, "the keypad gave way to Files");
            rig.rt.close();
        });
    }

    #[test]
    fn no_pin_means_no_keypad() {
        lvgl(|lv| {
            let mut rig = Rig::new(lv, None);
            assert_eq!(
                rig.send(UiEvent::Navigate(Route::Files), 2_000),
                Some(Route::Files)
            );
            rig.rt.close();
        });
    }

    #[test]
    fn five_wrong_tries_hold_the_keypad_for_a_minute() {
        lvgl(|lv| {
            let mut rig = Rig::new(lv, Some("4821"));
            rig.send(UiEvent::Navigate(Route::System), 2_000);
            for _ in 0..5 {
                assert_eq!(rig.enter("1111", 2_100), pin_for(Route::System));
            }
            assert_eq!(rig.enter("4821", 2_200), pin_for(Route::System), "waiting");
            // Cancel and reopen: still waiting.
            rig.send(UiEvent::Back, 2_300);
            rig.send(UiEvent::Navigate(Route::System), 2_400);
            assert_eq!(rig.enter("4821", 2_500), pin_for(Route::System));
            assert_eq!(
                rig.enter("4821", 2_100 + PIN_LOCKOUT_MS),
                Some(Route::System)
            );
            rig.rt.close();
        });
    }

    #[test]
    fn a_screen_cannot_unlock_by_itself() {
        lvgl(|lv| {
            let mut rig = Rig::new(lv, Some("4821"));
            rig.send(UiEvent::Navigate(Route::Uploads), 2_000);
            assert_eq!(
                rig.send(UiEvent::PinAccepted, 2_100),
                pin_for(Route::Uploads)
            );
            rig.rt.close();
        });
    }

    #[test]
    fn resting_relocks_and_the_keypad_comes_back_fresh() {
        lvgl(|lv| {
            let mut rig = Rig::new(lv, Some("4821"));
            rig.send(UiEvent::Navigate(Route::Menu), 2_000);
            rig.send(UiEvent::Navigate(Route::Files), 2_100);
            assert_eq!(rig.enter("4821", 2_200), Some(Route::Files));
            assert_eq!(rig.send(UiEvent::Rest, 3_000), Some(Route::Resting));
            assert_eq!(rig.send(UiEvent::Wake, 4_000), Some(Route::Menu));
            assert_eq!(
                rig.send(UiEvent::Navigate(Route::Wifi), 4_100),
                pin_for(Route::Wifi)
            );
            assert_eq!(PIN_BUILDS.with(Cell::get), 2);

            // A keypad under the resting screen is rebuilt on waking, so
            // half-typed digits never survive a rest.
            rig.send(UiEvent::Rest, 5_000);
            assert_eq!(rig.send(UiEvent::Wake, 6_000), pin_for(Route::Wifi));
            assert_eq!(PIN_BUILDS.with(Cell::get), 3, "rebuilt, not parked");
            rig.rt.close();
        });
    }
}
