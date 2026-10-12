//! What waved does with each message from the MCU, without any I/O: answer
//! heartbeats, publish the battery, turn the key into `PowerKey` presses
//! (click = sleep / wake, 2 s = power off; user decisions 2026-10-12) and
//! hand the motor's reports to the lift (`lift.rs`).

use crate::lift::Lift;
use crate::mavlink::Message;
use crate::protocol::{self, Button, Outgoing};
use waved_state::{BatteryState, DeviceEvent, LiftState, MotorPosition, PowerKeyPress};

/// Stock reports the link lost after this long without a heartbeat.
pub const LINK_LOST_MS: u64 = 5_000;
/// Levels at or above this are not a percentage (stock ignores them).
const LEVEL_INVALID: u16 = 111;
/// A jump this large between heartbeats is a glitch (stock drops it).
const LEVEL_MAX_STEP: u16 = 6;
/// Readings that must agree before the first level is trusted.
const SEED_HEARTBEATS: u8 = 2;
/// Readings in a row that make a jump real (re-seed the filter).
const RESEED_HEARTBEATS: u8 = 3;
/// `LowBattery` is raised below this level, on battery power, when the level
/// falls and has held for `LOW_HOLD_HEARTBEATS` (review 2026-10-12: a single
/// reading must never stop a recording or power the unit off).
const LOW_LEVEL: u16 = 10;
const LOW_HOLD_HEARTBEATS: u8 = 3;
/// The level that re-arms the low reports: charged back up, not flickering
/// at the threshold.
const LOW_REARM_LEVEL: u16 = LOW_LEVEL + 3;
/// USB current above this counts as charging; there is no low battery then.
const CHARGING_MA: u16 = 100;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Send(Outgoing),
    Battery(BatteryState),
    Event(DeviceEvent),
    /// `McuState::lift`.
    Lift(LiftState),
    /// The lift reached this height (`McuState::motor_position`).
    Height(MotorPosition),
}

#[derive(Default)]
pub struct Handler {
    level: LevelFilter,
    /// The low level last reported, and the level currently holding below
    /// it with its count.
    last_low: Option<u16>,
    low_streak: Option<(u16, u8)>,
    last_heartbeat_ms: Option<u64>,
    lost_reported: bool,
    lift: Lift,
}

/// The battery level as stock trusts it: values of `LEVEL_INVALID` or more
/// are not levels, jumps of `LEVEL_MAX_STEP` or more are glitches unless they
/// hold, and the first level needs `SEED_HEARTBEATS` readings that agree.
#[derive(Default)]
struct LevelFilter {
    level: Option<u16>,
    candidate: Option<(u16, u8)>,
}

impl LevelFilter {
    fn accept(&mut self, raw: u16) -> Option<u16> {
        if raw >= LEVEL_INVALID {
            return self.level;
        }
        match self.level {
            Some(current) if current.abs_diff(raw) < LEVEL_MAX_STEP => {
                self.level = Some(raw);
                self.candidate = None;
            }
            _ => {
                let count = match self.candidate {
                    Some((value, n)) if value.abs_diff(raw) < LEVEL_MAX_STEP => n.saturating_add(1),
                    _ => 1,
                };
                self.candidate = Some((raw, count));
                let needed = if self.level.is_none() { SEED_HEARTBEATS } else { RESEED_HEARTBEATS };
                if count >= needed {
                    self.level = Some(raw);
                    self.candidate = None;
                }
            }
        }
        self.level
    }
}

impl Handler {
    pub fn on_message(&mut self, m: &Message, now_ms: u64) -> Vec<Action> {
        if let Some(hb) = protocol::heartbeat(m) {
            return self.on_heartbeat(hb, now_ms);
        }
        if let Some(actions) = self.lift.on_message(m, now_ms) {
            return actions;
        }
        let press = match protocol::button(m) {
            Some(Button::Clicked | Button::Released1To2s) => PowerKeyPress::Short,
            Some(Button::Held2s) => PowerKeyPress::Long,
            _ => return Vec::new(),
        };
        vec![Action::Event(DeviceEvent::PowerKey(press))]
    }

    /// The user chose a Mic height.
    pub fn move_lift(&mut self, position: MotorPosition, now_ms: u64) -> Vec<Action> {
        self.lift.request(position, now_ms)
    }

    /// Time-driven work: the lift's resends, polls and time limit.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Action> {
        self.lift.tick(now_ms)
    }

    /// The power-off sequence used the link: end any move with `state`.
    pub fn lift_after_power_off(&mut self, state: LiftState) -> Vec<Action> {
        self.lift.force(state)
    }

    fn on_heartbeat(&mut self, hb: protocol::Heartbeat, now_ms: u64) -> Vec<Action> {
        self.last_heartbeat_ms = Some(now_ms);
        self.lost_reported = false;
        let mut actions = vec![Action::Send(protocol::heartbeat_reply())];
        let Some(level) = self.level.accept(hb.level) else {
            return actions;
        };
        let on_usb = hb.usb_current_ma > CHARGING_MA;
        actions.push(Action::Battery(BatteryState {
            level: level.min(100) as u8,
            voltage: hb.voltage_mv,
            temperature: hb.temperature_c.clamp(i16::from(i8::MIN), i16::from(i8::MAX)) as i8,
            is_charging: on_usb && level < 100,
        }));
        if let Some(low) = self.low_level(level, on_usb) {
            actions.push(Action::Event(DeviceEvent::LowBattery(low as u8)));
        }
        actions
    }

    /// A newly reached low level, once it held on battery power.
    fn low_level(&mut self, level: u16, on_usb: bool) -> Option<u16> {
        if level >= LOW_REARM_LEVEL {
            self.last_low = None;
        }
        let falling = level < LOW_LEVEL && self.last_low.is_none_or(|last| level < last);
        if on_usb || !falling {
            self.low_streak = None;
            return None;
        }
        let count = match self.low_streak {
            Some((value, n)) if value == level => n.saturating_add(1),
            _ => 1,
        };
        self.low_streak = Some((level, count));
        if count < LOW_HOLD_HEARTBEATS {
            return None;
        }
        self.low_streak = None;
        self.last_low = Some(level);
        Some(level)
    }

    /// True once when heartbeats stopped for longer than `LINK_LOST_MS`.
    pub fn link_lost(&mut self, now_ms: u64) -> bool {
        let silent = self.last_heartbeat_ms.is_some_and(|t| now_ms.saturating_sub(t) > LINK_LOST_MS);
        if silent && !self.lost_reported {
            self.lost_reported = true;
            return true;
        }
        false
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::{encode, Parser};
    use crate::protocol::{cmd, HEARTBEAT, NOTIFY};

    fn msg(msgid: u32, payload: &[u8]) -> Message {
        let mut parser = Parser::default();
        encode(0, msgid, payload).into_iter().find_map(|b| parser.feed(b)).unwrap()
    }

    fn hb(level: u16, usb_ma: u16) -> Message {
        let mut p = vec![0u8; 32];
        p[4..6].copy_from_slice(&level.to_le_bytes());
        p[6..8].copy_from_slice(&4100u16.to_le_bytes());
        p[8..10].copy_from_slice(&30i16.to_le_bytes());
        p[14..16].copy_from_slice(&usb_ma.to_le_bytes());
        msg(HEARTBEAT, &p)
    }

    fn key(code: u8) -> Message {
        let mut p = cmd::MISC_CHECK.to_le_bytes().to_vec();
        p.extend([0, code]);
        msg(NOTIFY, &p)
    }

    fn events(actions: &[Action]) -> Vec<DeviceEvent> {
        actions.iter().filter_map(|a| match a { Action::Event(e) => Some(e.clone()), _ => None }).collect()
    }

    fn battery(actions: &[Action]) -> Option<BatteryState> {
        actions.iter().find_map(|a| match a { Action::Battery(b) => Some(b.clone()), _ => None })
    }

    #[test]
    fn every_heartbeat_is_answered_and_the_level_is_published_once_two_agree() {
        let mut handler = Handler::default();
        let first = handler.on_message(&hb(80, 0), 0);
        assert!(first.contains(&Action::Send(crate::protocol::heartbeat_reply())));
        assert_eq!(battery(&first), None, "one reading is not trusted yet");
        let second = handler.on_message(&hb(80, 0), 2_000);
        let b = battery(&second).unwrap();
        assert_eq!((b.level, b.voltage, b.temperature, b.is_charging), (80, 4100, 30, false));
        let charging = handler.on_message(&hb(81, 500), 4_000);
        assert!(battery(&charging).unwrap().is_charging);
    }

    #[test]
    fn a_glitched_first_reading_is_not_trusted() {
        // Review 2026-10-12: a first heartbeat of 0 powered the unit off and
        // then locked the filter at 0.
        let mut handler = Handler::default();
        let mut all = handler.on_message(&hb(0, 0), 0);
        let mut last = None;
        for t in 1..=4 {
            let actions = handler.on_message(&hb(80, 0), t * 2_000);
            last = battery(&actions).or(last);
            all.extend(actions);
        }
        assert!(events(&all).is_empty());
        assert_eq!(last.unwrap().level, 80);
    }

    #[test]
    fn a_new_level_that_holds_reseeds_the_filter() {
        let mut handler = Handler::default();
        handler.on_message(&hb(100, 0), 0);
        handler.on_message(&hb(100, 0), 2_000);
        let mut level = 100;
        for t in 2..=4 {
            if let Some(b) = battery(&handler.on_message(&hb(50, 0), t * 2_000)) {
                level = b.level;
            }
        }
        assert_eq!(level, 50, "three heartbeats in a row at 50 are real");
    }

    #[test]
    fn click_and_two_second_hold_become_power_key_presses() {
        let mut handler = Handler::default();
        assert!(events(&handler.on_message(&key(0), 0)).is_empty(), "key down alone does nothing");
        assert_eq!(events(&handler.on_message(&key(1), 0)), [DeviceEvent::PowerKey(PowerKeyPress::Short)]);
        assert_eq!(events(&handler.on_message(&key(5), 0)), [DeviceEvent::PowerKey(PowerKeyPress::Short)]);
        assert_eq!(events(&handler.on_message(&key(6), 0)), [DeviceEvent::PowerKey(PowerKeyPress::Long)]);
        for ignored in [2u8, 4, 7] {
            assert!(events(&handler.on_message(&key(ignored), 0)).is_empty(), "code {ignored}");
        }
    }

    fn low_events(handler: &mut Handler, levels: &[(u16, u16)], t: &mut u64) -> Vec<DeviceEvent> {
        let mut low = Vec::new();
        for &(level, usb) in levels {
            *t += 2_000;
            low.extend(events(&handler.on_message(&hb(level, usb), *t)));
        }
        low
    }

    #[test]
    fn a_low_level_on_battery_is_reported_once_it_holds_and_only_when_falling() {
        let mut handler = Handler::default();
        let mut t = 0;
        let drain = [(12, 0), (12, 0), (11, 0), (9, 0), (9, 0)];
        assert!(low_events(&mut handler, &drain, &mut t).is_empty(), "9 held only twice");
        assert_eq!(low_events(&mut handler, &[(9, 0)], &mut t), [DeviceEvent::LowBattery(9)]);
        assert!(low_events(&mut handler, &[(9, 0), (9, 0), (10, 0), (9, 0), (9, 0), (9, 0)], &mut t).is_empty(), "not again for 9");
        assert_eq!(low_events(&mut handler, &[(8, 0), (8, 0), (8, 0)], &mut t), [DeviceEvent::LowBattery(8)]);
    }

    #[test]
    fn on_usb_power_there_is_no_low_battery() {
        // A flat unit on a laptop's USB port (a few hundred mA) charges; it
        // must not power off a few seconds after every start.
        let mut handler = Handler::default();
        let mut t = 0;
        let plugged = [(3, 500), (3, 500), (3, 500), (3, 450), (4, 500), (4, 500), (4, 500)];
        assert!(low_events(&mut handler, &plugged, &mut t).is_empty());
    }

    #[test]
    fn glitched_levels_are_ignored() {
        let mut handler = Handler::default();
        handler.on_message(&hb(100, 0), 0);
        handler.on_message(&hb(100, 0), 1_000);
        for (t, bad) in [(2_000, 255u16), (4_000, 0)] {
            let actions = handler.on_message(&hb(bad, 0), t);
            assert!(events(&actions).is_empty(), "level {bad}");
            assert!(actions.iter().any(|a| matches!(a, Action::Battery(b) if b.level == 100)), "level {bad} kept 100");
        }
    }

    #[test]
    fn motor_reports_drive_the_lift_while_heartbeats_are_still_answered() {
        let mut handler = Handler::default();
        let started = handler.move_lift(MotorPosition::High, 0);
        assert!(started.contains(&Action::Send(crate::protocol::motor_move(50, crate::protocol::Speeds::RAISE))));
        assert!(started.contains(&Action::Lift(LiftState::Moving)));
        let state = |code: u8| {
            let mut p = cmd::MOTOR_CHECK.to_le_bytes().to_vec();
            p.extend([2, code]);
            msg(NOTIFY, &p)
        };
        handler.on_message(&state(1), 200);
        assert!(handler.on_message(&hb(80, 0), 1_000).contains(&Action::Send(crate::protocol::heartbeat_reply())));
        let done = handler.on_message(&state(0), 1_500);
        assert!(done.contains(&Action::Height(MotorPosition::High)));
        assert!(done.contains(&Action::Lift(LiftState::Idle)));
        // The power key still decodes.
        assert_eq!(events(&handler.on_message(&key(1), 2_000)), [DeviceEvent::PowerKey(PowerKeyPress::Short)]);
    }

    #[test]
    fn a_silent_link_is_reported_once() {
        let mut handler = Handler::default();
        assert!(!handler.link_lost(60_000), "never heard: not lost, just absent");
        handler.on_message(&hb(50, 0), 0);
        assert!(!handler.link_lost(4_000));
        assert!(handler.link_lost(6_000));
        assert!(!handler.link_lost(8_000), "reported once");
        handler.on_message(&hb(50, 0), 9_000);
        assert!(handler.link_lost(15_000), "again after it came back and went silent");
    }
}
