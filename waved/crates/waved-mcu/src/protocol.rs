//! The MCU's messages (docs/MCU_PROTOCOL.md): decoding what it sends and
//! building the commands waved sends. `mcu_control` / `mcu_notify` payloads
//! are `u32 cmd` then `params[32]`, most with a sub-command in `params[0]`.

use crate::mavlink::Message;

pub const HEARTBEAT: u32 = 0;
pub const HEARTBEAT_REPLY: u32 = 1;
pub const CONTROL: u32 = 100;
pub const NOTIFY: u32 = 101;

/// `mcu_control` / `mcu_notify` command numbers waved uses.
pub mod cmd {
    pub const MOTOR_CTRL: u32 = 3;
    pub const MOTOR_CHECK: u32 = 4;
    pub const POWER_CTRL: u32 = 5;
    pub const CHARGE_CTRL: u32 = 7;
    pub const MISC_CHECK: u32 = 0x0E;
}

/// The battery fields of a heartbeat (msgid 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heartbeat {
    pub battery_current_ma: i32,
    pub level: u16,
    pub voltage_mv: u16,
    pub temperature_c: i16,
    pub usb_current_ma: u16,
}

pub fn heartbeat(m: &Message) -> Option<Heartbeat> {
    if m.msgid != HEARTBEAT || m.payload.len() < 16 {
        return None;
    }
    let p = &m.payload;
    Some(Heartbeat {
        battery_current_ma: i32::from_le_bytes([p[0], p[1], p[2], p[3]]),
        level: u16::from_le_bytes([p[4], p[5]]),
        voltage_mv: u16::from_le_bytes([p[6], p[7]]),
        temperature_c: i16::from_le_bytes([p[8], p[9]]),
        usb_current_ma: u16::from_le_bytes([p[14], p[15]]),
    })
}

/// The `cmd` of an `mcu_notify`.
pub fn notify_cmd(m: &Message) -> Option<u32> {
    (m.msgid == NOTIFY && m.payload.len() >= 4)
        .then(|| u32::from_le_bytes([m.payload[0], m.payload[1], m.payload[2], m.payload[3]]))
}

/// What the MCU reports about the power key (`mcu_notify` cmd 0x0E, sub 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Pressed,
    Clicked,
    DoubleClicked,
    /// Still held at 1 s.
    Held1s,
    /// Released between 1 and 2 s.
    Released1To2s,
    /// Still held at 2 s.
    Held2s,
    /// Released between 2 and 5 s.
    Released2To5s,
}

pub fn button(m: &Message) -> Option<Button> {
    if notify_cmd(m)? != cmd::MISC_CHECK || m.payload[4] != 0 {
        return None;
    }
    Some(match m.payload[5] {
        0 => Button::Pressed,
        1 => Button::Clicked,
        2 => Button::DoubleClicked,
        4 => Button::Held1s,
        5 => Button::Released1To2s,
        6 => Button::Held2s,
        7 => Button::Released2To5s,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotorState {
    Idle,
    Working,
    Blocked,
    WaitData,
    /// The lift's sensor saw an obstruction.
    Obstructed,
}

/// `mcu_notify` cmd 4 sub 2.
pub fn motor_state(m: &Message) -> Option<MotorState> {
    if notify_cmd(m)? != cmd::MOTOR_CHECK || m.payload[4] != 2 {
        return None;
    }
    Some(match m.payload[5] {
        0 => MotorState::Idle,
        1 => MotorState::Working,
        2 => MotorState::Blocked,
        3 => MotorState::WaitData,
        4 => MotorState::Obstructed,
        _ => return None,
    })
}

/// A message for the MCU, before framing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub msgid: u32,
    pub payload: Vec<u8>,
}

fn control(command: u32, params: &[u8]) -> Outgoing {
    let mut payload = command.to_le_bytes().to_vec();
    payload.extend_from_slice(params);
    payload.resize(36, 0);
    Outgoing { msgid: CONTROL, payload }
}

/// The reply to every heartbeat: 32 zero bytes.
pub fn heartbeat_reply() -> Outgoing {
    Outgoing { msgid: HEARTBEAT_REPLY, payload: vec![0; 32] }
}

/// Move the lift to `mm` (0, 30, 40 or 50) with stock's limits and speeds.
pub fn motor_to_mm(mm: i16) -> Outgoing {
    let (high, low): (i8, i8) = if mm == 0 { (40, 20) } else { (50, 25) };
    let mut params = vec![0u8]; // sub 0: move
    params.extend(2u32.to_le_bytes()); // mode POS
    params.extend(mm.to_le_bytes());
    params.extend(4000i16.to_le_bytes()); // up current limit
    params.extend(3000i16.to_le_bytes()); // down current limit
    params.extend([high as u8, low as u8]);
    control(cmd::MOTOR_CTRL, &params)
}

pub fn motor_state_query() -> Outgoing {
    control(cmd::MOTOR_CHECK, &[2])
}

/// The MCU's power-off timer (POWER_CTRL sub 1): start or stop.
pub fn power_off_timer(start: bool) -> Outgoing {
    control(cmd::POWER_CTRL, &[1, u8::from(start)])
}

/// Charging on (CHARGE_CTRL sub 2), so a powered-off unit charges.
pub fn charging_on() -> Outgoing {
    control(cmd::CHARGE_CTRL, &[2, 1])
}

/// Power everything off (POWER_CTRL sub 0, ALL_SHUTDOWN).
pub fn all_shutdown() -> Outgoing {
    control(cmd::POWER_CTRL, &[0, 4])
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::{encode, Parser};

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    fn parse(s: &str) -> Message {
        let mut parser = Parser::default();
        hex(s).into_iter().find_map(|b| parser.feed(b)).expect("a frame")
    }

    fn wire(out: &Outgoing) -> Vec<u8> {
        encode(0, out.msgid, &out.payload)
    }

    #[test]
    fn a_captured_heartbeat_is_the_battery() {
        let hb = heartbeat(&parse("FD 10 00 00 1C 00 00 00 00 00 00 00 00 00 64 00 52 10 24 00 00 00 00 00 00 02 62 A9")).unwrap();
        assert_eq!(
            hb,
            Heartbeat { battery_current_ma: 0, level: 100, voltage_mv: 4178, temperature_c: 36, usb_current_ma: 512 }
        );
    }

    #[test]
    fn captured_key_gestures_decode() {
        let cases = [
            ("FD 01 00 00 1D 00 00 65 00 00 0E 31 C7", Button::Pressed),
            ("FD 06 00 00 1E 00 00 65 00 00 0E 00 00 00 00 01 06 66", Button::Clicked),
            ("FD 06 00 00 21 00 00 65 00 00 0E 00 00 00 00 02 35 96", Button::DoubleClicked),
            ("FD 06 00 00 25 00 00 65 00 00 0E 00 00 00 00 04 51 D4", Button::Held1s),
            ("FD 06 00 00 26 00 00 65 00 00 0E 00 00 00 00 05 7E C3", Button::Released1To2s),
        ];
        for (frame, expected) in cases {
            assert_eq!(button(&parse(frame)), Some(expected), "{frame}");
        }
    }

    #[test]
    fn other_notifications_are_not_buttons() {
        // MISC_CHECK sub 1 (battery status) and a heartbeat.
        let mut battery = 0x0Eu32.to_le_bytes().to_vec();
        battery.extend([1, 1]);
        let mut parser = Parser::default();
        let m = encode(3, NOTIFY, &battery).into_iter().find_map(|b| parser.feed(b)).unwrap();
        assert_eq!(button(&m), None);
        assert_eq!(button(&parse("FD 10 00 00 1C 00 00 00 00 00 00 00 00 00 64 00 52 10 24 00 00 00 00 00 00 02 62 A9")), None);
    }

    #[test]
    fn commands_match_the_stock_frames() {
        assert_eq!(wire(&heartbeat_reply()), hex("FD 01 00 00 00 00 00 01 00 00 00 FC 8F"));
        assert_eq!(
            wire(&motor_to_mm(50)),
            hex("FD 11 00 00 00 00 00 64 00 00 03 00 00 00 00 02 00 00 00 32 00 A0 0F B8 0B 32 19 B9 86")
        );
        assert_eq!(&wire(&motor_to_mm(0))[19..], &hex("00 00 A0 0F B8 0B 28 14 79 53")[..]);
        assert_eq!(wire(&power_off_timer(true)), hex("FD 06 00 00 00 00 00 64 00 00 05 00 00 00 01 01 4D C7"));
        assert_eq!(&wire(&all_shutdown())[10..16], &hex("05 00 00 00 00 04")[..]);
        assert_eq!(&wire(&charging_on())[10..16], &hex("07 00 00 00 02 01")[..]);
        assert_eq!(&wire(&motor_state_query())[10..15], &hex("04 00 00 00 02")[..]);
    }

    #[test]
    fn motor_state_notifications_decode() {
        for (code, state) in [(0u8, MotorState::Idle), (2, MotorState::Blocked), (4, MotorState::Obstructed)] {
            let mut payload = cmd::MOTOR_CHECK.to_le_bytes().to_vec();
            payload.extend([2, code]);
            let mut parser = Parser::default();
            let m = encode(9, NOTIFY, &payload).into_iter().find_map(|b| parser.feed(b)).unwrap();
            assert_eq!(motor_state(&m), Some(state));
        }
    }
}
