//! The MCU's messages (docs/MCU_PROTOCOL.md): decoding what it sends and
//! building the commands waved sends. `mcu_control` / `mcu_notify` payloads
//! are `u32 cmd` then `params[32]`, most with a sub-command in `params[0]`.

use crate::mavlink::Message;
use waved_state::MotorPosition;

pub const HEARTBEAT: u32 = 0;
pub const HEARTBEAT_REPLY: u32 = 1;
pub const CONTROL: u32 = 100;
pub const NOTIFY: u32 = 101;

/// `mcu_control` / `mcu_notify` command numbers waved uses.
pub mod cmd {
    pub const LED_CTRL: u32 = 1;
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

/// Where the lift is, in mm (`mcu_notify` cmd 4 sub 1, `params[1..2]` i16).
/// Seen on 2026-10-12 while the MCU retracted the lift (40, 30, 0); that the
/// field is an i16 is assumed (the values fit a byte).
pub fn lift_position(m: &Message) -> Option<i16> {
    if notify_cmd(m)? != cmd::MOTOR_CHECK || m.payload[4] != 1 {
        return None;
    }
    Some(i16::from_le_bytes([m.payload[5], m.payload[6]]))
}

/// The lift's height for a Mic height: stock's three stops
/// (`mcu_server_request_motor_ctrl_pos`). 0 mm is only for power off.
pub fn height_mm(position: MotorPosition) -> Option<i16> {
    match position {
        MotorPosition::Low => Some(30),
        MotorPosition::Medium => Some(40),
        MotorPosition::High => Some(50),
        MotorPosition::Moving => None,
    }
}

/// The motor's high and low speeds for a move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Speeds {
    pub high: i8,
    pub low: i8,
}

impl Speeds {
    /// To a height.
    pub const RAISE: Speeds = Speeds { high: 50, low: 25 };
    /// To 0 mm.
    pub const RETRACT: Speeds = Speeds { high: 40, low: 20 };
    /// Stock's retry after a stall (`mcu_motor_block_pos_control`).
    pub const BLOCKED: Speeds = Speeds { high: 80, low: 45 };

    pub fn for_target(mm: i16) -> Speeds {
        if mm == 0 {
            Speeds::RETRACT
        } else {
            Speeds::RAISE
        }
    }
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
    motor_move(mm, Speeds::for_target(mm))
}

/// Move the lift to `mm` at `speeds`, with stock's current limits.
pub fn motor_move(mm: i16, speeds: Speeds) -> Outgoing {
    let mut params = vec![0u8]; // sub 0: move
    params.extend(2u32.to_le_bytes()); // mode POS
    params.extend(mm.to_le_bytes());
    params.extend(4000i16.to_le_bytes()); // up current limit
    params.extend(3000i16.to_le_bytes()); // down current limit
    params.extend([speeds.high as u8, speeds.low as u8]);
    control(cmd::MOTOR_CTRL, &params)
}

/// The height the MCU raises the lift to at power-on (MOTOR_CTRL sub 4,
/// `mcu_init_pos_ctrl`: 0 = 50, 1 = 40, 2 = 30 mm). `None` for other heights.
pub fn start_height(mm: i16) -> Option<Outgoing> {
    let code = match mm {
        50 => 0u8,
        40 => 1,
        30 => 2,
        _ => return None,
    };
    Some(control(cmd::MOTOR_CTRL, &[4, code]))
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

/// What the status LEDs show: stock's `__mcu_led_ctrl_t` (docs/MCU_PROTOCOL.md,
/// "Status LEDs"). The values are stock's, from its event table (`leds.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedCtrl {
    /// 0 steady, 1 blink, 2 pulse (gradient), 3 chase (race), 4 progress,
    /// 5 hue change, 6 rise, 7 fall.
    pub pattern: u8,
    /// Brightness for steady (0 = 100 % .. 4 = 0 %), speed for blink and
    /// pulse (0 = 1 Hz, 1 = 3 Hz), direction for chase.
    pub sub: u8,
    /// How many of the 12 LEDs.
    pub leds: u8,
    /// 0 red, 1 green, 2 blue, 3 cyan, 4 yellow, 5 white, 6 orange.
    pub colour: u8,
    /// Stock's `target` (15 for most, 3 for the error blinks); meaning unknown.
    pub target: u8,
}

/// LED_CTRL (cmd 1, no sub-command): stock's 8 bytes, the last 3 reserved.
pub fn led_ctrl(led: LedCtrl) -> Outgoing {
    control(cmd::LED_CTRL, &[led.pattern, led.sub, led.leds, led.colour, led.target, 0, 0, 0])
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
    fn led_ctrl_is_cmd_1_with_stocks_eight_bytes() {
        // `mcu_led_ctrl` (1.2.2.6 85254): `mcu_ctrl_cmd_send(1, &led_ctrl, 8)`, the
        // bytes `__mcu_led_ctrl_t`: pattern, sub-pattern, LED count, colour, target, res[3].
        // IDLE: LIGHT, 100 %, 12 LEDs, green, target 15; trailing zeros trimmed.
        let idle = led_ctrl(LedCtrl { pattern: 0, sub: 0, leds: 12, colour: 1, target: 15 });
        assert_eq!(idle.payload.len(), 36);
        assert_eq!(&idle.payload[..12], &[1, 0, 0, 0, 0, 0, 12, 1, 15, 0, 0, 0]);
        assert_eq!(&wire(&idle)[..19], &hex("FD 09 00 00 00 00 00 64 00 00 01 00 00 00 00 00 0C 01 0F")[..]);
        assert_eq!(notify_cmd(&parse_wire(&idle)), None, "a control, not a notify");
    }

    fn parse_wire(out: &Outgoing) -> Message {
        let mut parser = Parser::default();
        wire(out).into_iter().find_map(|b| parser.feed(b)).unwrap()
    }

    #[test]
    fn heights_are_stocks_three_stops() {
        // libui `mcu_server_request_motor_ctrl_pos`: lvl 0/1/2 -> 30/40/50 mm.
        assert_eq!(height_mm(MotorPosition::Low), Some(30));
        assert_eq!(height_mm(MotorPosition::Medium), Some(40));
        assert_eq!(height_mm(MotorPosition::High), Some(50));
        assert_eq!(height_mm(MotorPosition::Moving), None);
    }

    #[test]
    fn a_retry_after_a_stall_uses_stocks_block_speeds() {
        // `mcu_motor_block_pos_control`: high 0x50, low 0x2D, same limits.
        assert_eq!(&wire(&motor_move(40, Speeds::BLOCKED))[19..25], &hex("28 00 A0 0F B8 0B")[..]);
        assert_eq!(&wire(&motor_move(40, Speeds::BLOCKED))[25..27], &hex("50 2D")[..]);
        assert_eq!(motor_move(50, Speeds::for_target(50)), motor_to_mm(50));
        assert_eq!(Speeds::for_target(0), Speeds::RETRACT);
    }

    #[test]
    fn the_start_height_is_motor_ctrl_sub_4() {
        // `mcu_init_pos_ctrl`: cmd 3 sub 4, data 0 = 50, 1 = 40, 2 = 30 mm.
        assert_eq!(&wire(&start_height(50).unwrap())[10..15], &hex("03 00 00 00 04")[..]);
        assert_eq!(start_height(50).unwrap().payload[5], 0);
        assert_eq!(start_height(40).unwrap().payload[5], 1);
        assert_eq!(start_height(30).unwrap().payload[5], 2);
        assert_eq!(start_height(0), None, "0 mm is not a start height");
    }

    #[test]
    fn position_reports_decode() {
        // Captured 2026-10-12 while the MCU retracted the lift: cmd 4 sub 1, 40 / 30 / 0 mm.
        for mm in [40i16, 30, 0] {
            let mut payload = cmd::MOTOR_CHECK.to_le_bytes().to_vec();
            payload.push(1);
            payload.extend(mm.to_le_bytes());
            let mut parser = Parser::default();
            let m = encode(4, NOTIFY, &payload).into_iter().find_map(|b| parser.feed(b)).unwrap();
            assert_eq!(lift_position(&m), Some(mm));
            assert_eq!(motor_state(&m), None);
        }
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
