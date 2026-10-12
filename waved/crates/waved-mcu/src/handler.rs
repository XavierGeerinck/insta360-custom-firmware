//! What waved does with each message from the MCU, without any I/O: answer
//! heartbeats, publish the battery, turn the key into `PowerKey` presses
//! (click = sleep / wake, 2 s = "Power off?"; user decision 2026-10-12).

use crate::mavlink::Message;
use crate::protocol::{self, Button, Outgoing};
use waved_state::{BatteryState, DeviceEvent, PowerKeyPress};

/// Stock reports the link lost after this long without a heartbeat.
pub const LINK_LOST_MS: u64 = 5_000;
/// Levels at or above this are not a percentage (stock ignores them).
const LEVEL_INVALID: u16 = 111;
/// A jump this large between heartbeats is a glitch (stock drops it).
const LEVEL_MAX_STEP: u16 = 6;
/// `LowBattery` is raised below this level...
const LOW_LEVEL: u16 = 10;
/// ...and only while USB delivers less than this (stock's charging threshold).
const ON_USB_MA: u16 = 900;
/// USB current above this counts as charging.
const CHARGING_MA: u16 = 100;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Send(Outgoing),
    Battery(BatteryState),
    Event(DeviceEvent),
}

#[derive(Default)]
pub struct Handler {
    level: Option<u16>,
    last_low: Option<u16>,
    last_heartbeat_ms: Option<u64>,
    lost_reported: bool,
}

impl Handler {
    pub fn on_message(&mut self, m: &Message, now_ms: u64) -> Vec<Action> {
        if let Some(hb) = protocol::heartbeat(m) {
            return self.on_heartbeat(hb, now_ms);
        }
        let press = match protocol::button(m) {
            Some(Button::Clicked | Button::Released1To2s) => PowerKeyPress::Short,
            Some(Button::Held2s) => PowerKeyPress::Long,
            _ => return Vec::new(),
        };
        vec![Action::Event(DeviceEvent::PowerKey(press))]
    }

    fn on_heartbeat(&mut self, hb: protocol::Heartbeat, now_ms: u64) -> Vec<Action> {
        self.last_heartbeat_ms = Some(now_ms);
        self.lost_reported = false;
        let mut actions = vec![Action::Send(protocol::heartbeat_reply())];
        let plausible = hb.level < LEVEL_INVALID
            && self.level.is_none_or(|last| last.abs_diff(hb.level) < LEVEL_MAX_STEP);
        if plausible {
            self.level = Some(hb.level);
        }
        let Some(level) = self.level else {
            return actions;
        };
        actions.push(Action::Battery(BatteryState {
            level: level.min(100) as u8,
            voltage: hb.voltage_mv,
            temperature: hb.temperature_c.clamp(i16::from(i8::MIN), i16::from(i8::MAX)) as i8,
            is_charging: hb.usb_current_ma > CHARGING_MA && level < 100,
        }));
        if plausible && level < LOW_LEVEL && hb.usb_current_ma < ON_USB_MA && self.last_low != Some(level) {
            self.last_low = Some(level);
            actions.push(Action::Event(DeviceEvent::LowBattery(level as u8)));
        }
        actions
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

    #[test]
    fn every_heartbeat_is_answered_and_published() {
        let mut handler = Handler::default();
        let actions = handler.on_message(&hb(80, 0), 0);
        assert!(actions.contains(&Action::Send(crate::protocol::heartbeat_reply())));
        assert!(actions.iter().any(|a| matches!(a, Action::Battery(b) if b.level == 80 && b.voltage == 4100 && b.temperature == 30 && !b.is_charging)));
        let charging = handler.on_message(&hb(81, 500), 2_000);
        assert!(charging.iter().any(|a| matches!(a, Action::Battery(b) if b.is_charging)));
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

    #[test]
    fn low_battery_is_raised_once_per_new_low_level_and_not_on_usb_power() {
        let mut handler = Handler::default();
        let mut t = 0;
        let mut low = Vec::new();
        for level in [12u16, 11, 9, 9, 9, 8] {
            t += 2_000;
            low.extend(events(&handler.on_message(&hb(level, 0), t)));
        }
        assert_eq!(low, [DeviceEvent::LowBattery(9), DeviceEvent::LowBattery(8)]);
        let mut plugged = Handler::default();
        assert!(events(&plugged.on_message(&hb(4, 1200), 0)).is_empty());
    }

    #[test]
    fn glitched_levels_are_ignored() {
        let mut handler = Handler::default();
        handler.on_message(&hb(100, 0), 0);
        for (t, bad) in [(2_000, 255u16), (4_000, 0)] {
            let actions = handler.on_message(&hb(bad, 0), t);
            assert!(events(&actions).is_empty(), "level {bad}");
            assert!(actions.iter().any(|a| matches!(a, Action::Battery(b) if b.level == 100)), "level {bad} kept 100");
        }
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
