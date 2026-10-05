//! When the screen rests, turns off and wakes (TODO E1). A pure controller:
//! the UI runtime feeds it touches, power-key presses and the time, and applies
//! what it returns (router events and the backlight level). No I/O here.

use waved_state::{PowerKeyPress, SettingsState};

/// Share of the chosen brightness the resting screen keeps, in percent.
pub const RESTING_SHARE_PCT: u16 = 30;
/// The dimmest lit level, in percent of full backlight.
pub const MIN_LIT_PCT: u8 = 5;

/// Ordered from lit to dark, so "deeper" is `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenPower {
    Awake,
    Resting,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub from: ScreenPower,
    pub to: ScreenPower,
}

impl Transition {
    /// Leaving Awake: the router shows the resting screen and relocks the PIN
    /// (`UiEvent::Rest`).
    pub fn rests(&self) -> bool {
        self.from == ScreenPower::Awake && self.to != ScreenPower::Awake
    }

    /// Back to Awake (`UiEvent::Wake`).
    pub fn wakes(&self) -> bool {
        self.to == ScreenPower::Awake && self.from != ScreenPower::Awake
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchDisposition {
    /// Report the touch to LVGL.
    Deliver,
    /// Report "released": this press only woke the screen.
    Swallow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TouchOutcome {
    pub disposition: TouchDisposition,
    pub transition: Option<Transition>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerKeyOutcome {
    pub transition: Option<Transition>,
    /// Open the power-off confirmation (`Route::Confirm(ConfirmKind::PowerOff)`).
    pub confirm_power_off: bool,
}

#[derive(Clone, Debug)]
pub struct DisplayPower {
    state: ScreenPower,
    last_activity_ms: u64,
    touching: bool,
    swallowing: bool,
    slept_by_key: bool,
}

impl DisplayPower {
    pub fn new(now_ms: u64) -> Self {
        Self {
            state: ScreenPower::Awake,
            last_activity_ms: now_ms,
            touching: false,
            swallowing: false,
            slept_by_key: false,
        }
    }

    pub fn state(&self) -> ScreenPower {
        self.state
    }

    /// One touch sample. A press that starts while the screen is not awake
    /// wakes it and is swallowed until it is released.
    pub fn on_touch(&mut self, pressed: bool, now_ms: u64) -> TouchOutcome {
        let started = pressed && !self.touching;
        self.touching = pressed;
        if self.swallowing {
            if !pressed {
                self.swallowing = false;
            }
            return TouchOutcome {
                disposition: TouchDisposition::Swallow,
                transition: None,
            };
        }
        if started && self.state != ScreenPower::Awake {
            let transition = self.wake(now_ms);
            self.swallowing = true;
            return TouchOutcome {
                disposition: TouchDisposition::Swallow,
                transition,
            };
        }
        if pressed {
            self.last_activity_ms = now_ms;
        }
        TouchOutcome {
            disposition: TouchDisposition::Deliver,
            transition: None,
        }
    }

    /// Short: sleep (off, or resting while `keep_lit`) or wake. Long: wake and
    /// ask to power off.
    pub fn on_power_key(&mut self, press: PowerKeyPress, now_ms: u64, keep_lit: bool) -> PowerKeyOutcome {
        match press {
            PowerKeyPress::Short if self.state == ScreenPower::Awake => {
                self.slept_by_key = true;
                let to = if keep_lit { ScreenPower::Resting } else { ScreenPower::Off };
                PowerKeyOutcome {
                    transition: self.set(to),
                    confirm_power_off: false,
                }
            }
            PowerKeyPress::Short => PowerKeyOutcome {
                transition: self.wake(now_ms),
                confirm_power_off: false,
            },
            PowerKeyPress::Long => PowerKeyOutcome {
                transition: self.wake(now_ms),
                confirm_power_off: true,
            },
        }
    }

    /// Apply the timers. Only moves deeper (Awake → Resting → Off), except that
    /// `keep_lit` (recording or in a call) never lets the screen go fully off
    /// and lifts Off back to Resting.
    pub fn tick(&mut self, now_ms: u64, settings: &SettingsState, keep_lit: bool) -> Option<Transition> {
        let target = if self.slept_by_key {
            ScreenPower::Off
        } else {
            self.timed_target(now_ms, settings)
        };
        let next = if keep_lit {
            if self.state == ScreenPower::Off {
                ScreenPower::Resting
            } else {
                self.state.max(target.min(ScreenPower::Resting))
            }
        } else {
            self.state.max(target)
        };
        self.set(next)
    }

    /// Backlight in percent for the current state.
    pub fn backlight_pct(&self, brightness_pct: u8) -> u8 {
        match self.state {
            ScreenPower::Awake => brightness_pct.clamp(MIN_LIT_PCT, 100),
            ScreenPower::Resting => resting_pct(brightness_pct),
            ScreenPower::Off => 0,
        }
    }

    fn timed_target(&self, now_ms: u64, settings: &SettingsState) -> ScreenPower {
        let idle_ms = now_ms.saturating_sub(self.last_activity_ms);
        let elapsed = |seconds: u32| seconds != 0 && idle_ms >= u64::from(seconds) * 1_000;
        if elapsed(settings.off_after_s) {
            ScreenPower::Off
        } else if elapsed(settings.dim_after_s) {
            ScreenPower::Resting
        } else {
            ScreenPower::Awake
        }
    }

    fn wake(&mut self, now_ms: u64) -> Option<Transition> {
        self.last_activity_ms = now_ms;
        self.slept_by_key = false;
        self.set(ScreenPower::Awake)
    }

    fn set(&mut self, to: ScreenPower) -> Option<Transition> {
        if to == self.state {
            return None;
        }
        let transition = Transition { from: self.state, to };
        self.state = to;
        Some(transition)
    }
}

/// The resting screen's backlight: a share of the chosen brightness, never
/// below `MIN_LIT_PCT`.
pub fn resting_pct(brightness_pct: u8) -> u8 {
    let share = u16::from(brightness_pct.min(100)) * RESTING_SHARE_PCT / 100;
    (share as u8).max(MIN_LIT_PCT)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    fn timers(dim_after_s: u32, off_after_s: u32) -> SettingsState {
        SettingsState {
            dim_after_s,
            off_after_s,
            ..SettingsState::default()
        }
    }

    #[test]
    fn rests_after_dim_after_and_turns_off_after_off_after() {
        let s = SettingsState::default(); // 120 s / 600 s
        let mut p = DisplayPower::new(0);
        assert_eq!(p.tick(2 * MIN - 1, &s, false), None);
        let rest = p.tick(2 * MIN, &s, false).unwrap();
        assert!(rest.rests() && !rest.wakes());
        assert_eq!(p.state(), ScreenPower::Resting);
        assert_eq!(p.tick(10 * MIN - 1, &s, false), None);
        let off = p.tick(10 * MIN, &s, false).unwrap();
        assert_eq!(
            off,
            Transition {
                from: ScreenPower::Resting,
                to: ScreenPower::Off
            }
        );
        assert!(!off.rests() && !off.wakes(), "the router already shows Resting");
    }

    #[test]
    fn never_fully_off_while_recording_or_in_a_call() {
        let mut p = DisplayPower::new(0);
        let s = SettingsState::default();
        p.tick(2 * MIN, &s, true);
        assert_eq!(p.tick(10 * MIN, &s, true), None);
        assert_eq!(p.tick(60 * MIN, &s, true), None);
        assert_eq!(p.state(), ScreenPower::Resting);

        // With dimming off, the off timer rests the screen instead.
        let mut q = DisplayPower::new(0);
        let t = q.tick(10 * MIN, &timers(0, 600), true).unwrap();
        assert_eq!(t.to, ScreenPower::Resting);
        assert!(t.rests());
    }

    #[test]
    fn a_recording_that_starts_while_off_lights_the_resting_screen() {
        let mut p = DisplayPower::new(0);
        let s = SettingsState::default();
        p.tick(10 * MIN, &s, false);
        assert_eq!(p.state(), ScreenPower::Off);
        let t = p.tick(10 * MIN + 500, &s, true).unwrap();
        assert_eq!(
            t,
            Transition {
                from: ScreenPower::Off,
                to: ScreenPower::Resting
            }
        );
        assert!(!t.rests() && !t.wakes());
        assert_eq!(p.backlight_pct(80), 24);
    }

    #[test]
    fn the_first_touch_only_wakes() {
        let s = SettingsState::default();
        let mut p = DisplayPower::new(0);
        p.tick(2 * MIN, &s, false);
        let wake = p.on_touch(true, 2 * MIN + 10);
        assert_eq!(wake.disposition, TouchDisposition::Swallow);
        assert!(wake.transition.unwrap().wakes());
        assert_eq!(p.state(), ScreenPower::Awake);
        // The rest of that press, held or released, never reaches the UI.
        assert_eq!(p.on_touch(true, 2 * MIN + 50).disposition, TouchDisposition::Swallow);
        assert_eq!(p.on_touch(false, 2 * MIN + 90).disposition, TouchDisposition::Swallow);
        // The next press does.
        assert_eq!(
            p.on_touch(true, 2 * MIN + 500),
            TouchOutcome {
                disposition: TouchDisposition::Deliver,
                transition: None
            }
        );
    }

    #[test]
    fn a_touch_wakes_an_off_screen_and_only_wakes() {
        let s = SettingsState::default();
        let mut p = DisplayPower::new(0);
        p.tick(10 * MIN, &s, false);
        assert_eq!(p.state(), ScreenPower::Off);
        let wake = p.on_touch(true, 10 * MIN + 10);
        assert_eq!(wake.disposition, TouchDisposition::Swallow);
        assert_eq!(
            wake.transition,
            Some(Transition {
                from: ScreenPower::Off,
                to: ScreenPower::Awake
            })
        );
        assert!(wake.transition.unwrap().wakes());
        assert_eq!(p.state(), ScreenPower::Awake);
        assert_eq!(p.backlight_pct(80), 80);
        assert_eq!(p.on_touch(false, 10 * MIN + 90).disposition, TouchDisposition::Swallow);
        assert_eq!(p.on_touch(true, 10 * MIN + 500).disposition, TouchDisposition::Deliver);
    }

    #[test]
    fn a_touch_after_a_key_sleep_resumes_the_timers() {
        let s = SettingsState::default(); // 120 s / 600 s
        let mut p = DisplayPower::new(0);
        p.on_power_key(PowerKeyPress::Short, 9 * MIN, false);
        assert_eq!(p.state(), ScreenPower::Off);
        let wake = p.on_touch(true, 11 * MIN);
        assert_eq!(wake.disposition, TouchDisposition::Swallow);
        assert!(wake.transition.unwrap().wakes());
        p.on_touch(false, 11 * MIN + 100);
        // Both timers have run out since the last touch before the key sleep,
        // but not since the touch that woke the screen: it stays awake.
        assert_eq!(p.tick(11 * MIN + 1_000, &s, false), None);
        assert_eq!(p.state(), ScreenPower::Awake);
        assert_eq!(p.tick(13 * MIN - 1, &s, false), None);
        assert!(p.tick(13 * MIN, &s, false).unwrap().rests());
        assert_eq!(p.tick(21 * MIN - 1, &s, false), None);
        assert_eq!(p.tick(21 * MIN, &s, false).unwrap().to, ScreenPower::Off);
    }

    #[test]
    fn touches_while_awake_keep_the_screen_awake() {
        let s = SettingsState::default();
        let mut p = DisplayPower::new(0);
        p.on_touch(true, 100_000);
        p.on_touch(true, 110_000); // held: still activity
        p.on_touch(false, 111_000);
        assert_eq!(p.tick(110_000 + 2 * MIN - 1, &s, false), None);
        assert!(p.tick(110_000 + 2 * MIN, &s, false).unwrap().rests());
    }

    #[test]
    fn zero_means_never() {
        let mut p = DisplayPower::new(0);
        assert_eq!(p.tick(10 * 24 * 60 * MIN, &timers(0, 0), false), None);
        assert_eq!(p.state(), ScreenPower::Awake);
    }

    #[test]
    fn a_longer_dim_setting_does_not_wake_a_resting_screen() {
        let mut p = DisplayPower::new(0);
        p.tick(2 * MIN, &SettingsState::default(), false);
        assert_eq!(p.tick(3 * MIN, &timers(600, 600), false), None);
        assert_eq!(p.state(), ScreenPower::Resting);
    }

    #[test]
    fn power_key_short_press_sleeps_and_wakes() {
        let mut p = DisplayPower::new(0);
        let sleep = p.on_power_key(PowerKeyPress::Short, 1_000, false);
        assert_eq!(sleep.transition.unwrap().to, ScreenPower::Off);
        assert!(sleep.transition.unwrap().rests() && !sleep.confirm_power_off);
        let wake = p.on_power_key(PowerKeyPress::Short, 2_000, false);
        assert!(wake.transition.unwrap().wakes());
        let resting = p.on_power_key(PowerKeyPress::Short, 3_000, true);
        assert_eq!(resting.transition.unwrap().to, ScreenPower::Resting);
    }

    #[test]
    fn key_sleep_during_a_recording_turns_off_when_it_ends() {
        let s = SettingsState::default();
        let mut p = DisplayPower::new(0);
        p.on_power_key(PowerKeyPress::Short, 1_000, true);
        assert_eq!(p.tick(5_000, &s, true), None);
        assert_eq!(p.state(), ScreenPower::Resting);
        assert_eq!(p.tick(6_000, &s, false).unwrap().to, ScreenPower::Off);
    }

    #[test]
    fn long_press_wakes_and_asks_to_power_off() {
        let mut p = DisplayPower::new(0);
        p.tick(10 * MIN, &SettingsState::default(), false);
        let out = p.on_power_key(PowerKeyPress::Long, 10 * MIN + 1, false);
        assert!(out.confirm_power_off && out.transition.unwrap().wakes());
        let awake = p.on_power_key(PowerKeyPress::Long, 10 * MIN + 2, false);
        assert_eq!(
            awake,
            PowerKeyOutcome {
                transition: None,
                confirm_power_off: true
            }
        );
    }

    #[test]
    fn backlight_follows_the_state() {
        let mut p = DisplayPower::new(0);
        assert_eq!(p.backlight_pct(80), 80);
        assert_eq!(p.backlight_pct(0), MIN_LIT_PCT);
        p.tick(2 * MIN, &SettingsState::default(), false);
        assert_eq!(p.backlight_pct(80), 24);
        p.tick(10 * MIN, &SettingsState::default(), false);
        assert_eq!(p.backlight_pct(80), 0);
        assert_eq!((resting_pct(100), resting_pct(5), resting_pct(250)), (30, 5, 30));
    }
}
