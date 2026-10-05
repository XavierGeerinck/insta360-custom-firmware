//! Screen power for the UI runtime: a thin adapter over workstream E's
//! `DisplayPower` controller and `Backlight` (E IR3). It turns their
//! transitions into router events and acts as the wake gate for touches.

use crate::kit::dispatch::UiEvent;
use crate::kit::route::{ConfirmKind, Route};
use crate::kit::snapshot::UiSnapshot;
use waved_state::PowerKeyPress;
use waved_system::backlight::{Backlight, BACKLIGHT_DIR};
use waved_system::display::{DisplayPower, TouchDisposition, Transition};

/// The one host-test seam: where backlight levels go.
pub trait BacklightOut {
    fn set_percent(&mut self, pct: u8) -> std::io::Result<bool>;
}

impl BacklightOut for Backlight {
    fn set_percent(&mut self, pct: u8) -> std::io::Result<bool> {
        Backlight::set_percent(self, pct)
    }
}

/// Whether LVGL may see a touch sample, and what the router should hear.
#[derive(Clone, Debug, PartialEq)]
pub struct TouchGate {
    /// False while a press that only woke the screen is still down.
    pub deliver: bool,
    pub event: Option<UiEvent>,
}

pub struct PowerDriver {
    power: DisplayPower,
    backlight: Box<dyn BacklightOut>,
}

/// Recording or in a call: the screen never goes fully dark.
fn keep_lit(snap: &UiSnapshot) -> bool {
    snap.meeting.recording || snap.in_call()
}

/// The router event for a screen power transition. Resting to Off is silent:
/// the router already shows the resting screen.
fn event_for(transition: Option<Transition>) -> Option<UiEvent> {
    match transition {
        Some(t) if t.rests() => Some(UiEvent::Rest),
        Some(t) if t.wakes() => Some(UiEvent::Wake),
        _ => None,
    }
}

impl PowerDriver {
    pub fn new(now_ms: u64, backlight: Box<dyn BacklightOut>) -> Self {
        Self {
            power: DisplayPower::new(now_ms),
            backlight,
        }
    }

    /// The device's LCD backlight.
    pub fn device(now_ms: u64) -> Self {
        Self::new(now_ms, Box::new(Backlight::open(BACKLIGHT_DIR)))
    }

    /// Every touch sample, pressed or not. A press that wakes the screen is
    /// swallowed (`deliver` false) until it is released.
    pub fn on_touch(&mut self, pressed: bool, now_ms: u64) -> TouchGate {
        let out = self.power.on_touch(pressed, now_ms);
        TouchGate {
            deliver: out.disposition == TouchDisposition::Deliver,
            event: event_for(out.transition),
        }
    }

    /// Rest or Wake for the screen change, then the power-off confirmation
    /// when the controller asks for it.
    pub fn on_power_key(&mut self, press: PowerKeyPress, snap: &UiSnapshot) -> Vec<UiEvent> {
        let out = self.power.on_power_key(press, snap.now_ms, keep_lit(snap));
        let mut events: Vec<UiEvent> = event_for(out.transition).into_iter().collect();
        if out.confirm_power_off {
            events.push(UiEvent::Navigate(Route::Confirm(ConfirmKind::PowerOff)));
        }
        events
    }

    /// Every loop iteration: advance the timers and apply the backlight level
    /// (the backlight writes only when the level changes).
    pub fn tick(&mut self, snap: &UiSnapshot) -> Option<UiEvent> {
        let transition = self.power.tick(snap.now_ms, &snap.settings, keep_lit(snap));
        let pct = self.power.backlight_pct(snap.settings.brightness_pct);
        if let Err(error) = self.backlight.set_percent(pct) {
            tracing::warn!("backlight not set: {error}");
        }
        event_for(transition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::route::{ConfirmKind, Route};
    use std::cell::RefCell;
    use std::rc::Rc;
    use waved_state::PowerKeyPress;
    use waved_system::display::resting_pct;

    const MIN: u64 = 60_000;

    struct Panel(Rc<RefCell<Vec<u8>>>);

    impl BacklightOut for Panel {
        fn set_percent(&mut self, pct: u8) -> std::io::Result<bool> {
            self.0.borrow_mut().push(pct);
            Ok(true)
        }
    }

    fn rig() -> (PowerDriver, Rc<RefCell<Vec<u8>>>) {
        let writes = Rc::new(RefCell::new(Vec::new()));
        (PowerDriver::new(0, Box::new(Panel(writes.clone()))), writes)
    }

    fn at(now_ms: u64) -> UiSnapshot {
        UiSnapshot {
            now_ms,
            ..UiSnapshot::default()
        }
    }

    fn confirm() -> UiEvent {
        UiEvent::Navigate(Route::Confirm(ConfirmKind::PowerOff))
    }

    #[test]
    fn timers_become_rest_and_wake_for_the_router() {
        let (mut p, writes) = rig();
        assert_eq!(p.tick(&at(0)), None);
        assert_eq!(p.tick(&at(2 * MIN)), Some(UiEvent::Rest));
        assert_eq!(p.tick(&at(10 * MIN)), None, "already resting, now off");
        let gate = p.on_touch(true, 10 * MIN + 10);
        assert_eq!(gate.event, Some(UiEvent::Wake));
        assert_eq!(p.tick(&at(10 * MIN + 10)), None);
        assert_eq!(*writes.borrow(), [80, resting_pct(80), 0, 80]);
    }

    #[test]
    fn a_press_that_wakes_never_reaches_lvgl() {
        let (mut p, _) = rig();
        p.tick(&at(2 * MIN));
        let wake = p.on_touch(true, 2 * MIN + 10);
        assert_eq!(
            wake,
            TouchGate {
                deliver: false,
                event: Some(UiEvent::Wake)
            }
        );
        assert!(!p.on_touch(true, 2 * MIN + 50).deliver, "held");
        assert!(!p.on_touch(false, 2 * MIN + 90).deliver, "released");
        let next = p.on_touch(true, 2 * MIN + 500);
        assert_eq!(
            next,
            TouchGate {
                deliver: true,
                event: None
            }
        );
    }

    #[test]
    fn a_press_while_awake_is_delivered_and_restarts_the_dim_timer() {
        let (mut p, _) = rig();
        assert!(p.on_touch(true, 100_000).deliver);
        p.on_touch(false, 100_100);
        assert_eq!(p.tick(&at(2 * MIN)), None, "touched at 100 s");
        assert_eq!(p.tick(&at(100_000 + 2 * MIN)), Some(UiEvent::Rest));
    }

    #[test]
    fn power_key_short_sleeps_and_wakes() {
        let (mut p, writes) = rig();
        assert_eq!(
            p.on_power_key(PowerKeyPress::Short, &at(1_000)),
            [UiEvent::Rest]
        );
        p.tick(&at(1_000));
        assert_eq!(writes.borrow().last(), Some(&0));
        assert_eq!(
            p.on_power_key(PowerKeyPress::Short, &at(2_000)),
            [UiEvent::Wake]
        );
        p.tick(&at(2_000));
        assert_eq!(writes.borrow().last(), Some(&80));
    }

    #[test]
    fn power_key_long_asks_to_power_off() {
        let (mut p, _) = rig();
        assert_eq!(p.on_power_key(PowerKeyPress::Long, &at(1_000)), [confirm()]);
        p.on_power_key(PowerKeyPress::Short, &at(2_000));
        assert_eq!(
            p.on_power_key(PowerKeyPress::Long, &at(3_000)),
            [UiEvent::Wake, confirm()]
        );
    }

    #[test]
    fn recording_or_a_call_keeps_the_screen_lit() {
        for call in [false, true] {
            let (mut p, writes) = rig();
            let mut snap = at(0);
            snap.meeting.recording = !call;
            snap.audio.call_off_hook = call;
            p.tick(&snap);
            snap.now_ms = 10 * MIN;
            assert_eq!(p.tick(&snap), Some(UiEvent::Rest));
            snap.now_ms = 60 * MIN;
            p.tick(&snap);
            assert!(!writes.borrow().contains(&0), "never dark (call: {call})");
            assert_eq!(writes.borrow().last(), Some(&resting_pct(80)));

            let (mut p, writes) = rig();
            let mut snap = at(1_000);
            snap.meeting.recording = !call;
            snap.audio.call_off_hook = call;
            assert_eq!(p.on_power_key(PowerKeyPress::Short, &snap), [UiEvent::Rest]);
            p.tick(&snap);
            assert_eq!(writes.borrow().last(), Some(&resting_pct(80)));
        }
    }

    #[test]
    fn a_brightness_change_applies_on_the_next_tick() {
        let (mut p, writes) = rig();
        let mut snap = at(0);
        p.tick(&snap);
        snap.now_ms = 10;
        snap.settings.brightness_pct = 50;
        p.tick(&snap);
        assert_eq!(*writes.borrow(), [80, 50]);
    }
}
