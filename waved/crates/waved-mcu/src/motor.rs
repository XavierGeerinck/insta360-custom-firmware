use waved_state::MotorPosition;
use waved_uart::frame::Frame;

const MCU_HEADER: [u8; 2] = [0xDC, 0xCD];

pub fn build_motor_command(position: &MotorPosition) -> Vec<u8> {
    let cmd_byte = match position {
        MotorPosition::Low => 0x01,
        MotorPosition::Medium => 0x02,
        MotorPosition::High => 0x03,
        MotorPosition::Moving => return vec![],
    };
    Frame::build(MCU_HEADER, &[0x20, cmd_byte]).unwrap_or_default()
}

pub fn build_motor_stop() -> Vec<u8> {
    Frame::build(MCU_HEADER, &[0x21]).unwrap_or_default()
}

pub fn build_shutdown_command() -> Vec<u8> {
    Frame::build(MCU_HEADER, &[0x30]).unwrap_or_default()
}

pub fn build_reboot_command() -> Vec<u8> {
    Frame::build(MCU_HEADER, &[0x31]).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motor_low_builds_valid_frame() {
        let raw = build_motor_command(&MotorPosition::Low);
        assert!(!raw.is_empty());
        assert_eq!(raw[0..2], [0xDC, 0xCD]);
        assert_eq!(raw[3], 0x20);
        assert_eq!(raw[4], 0x01);
    }

    #[test]
    fn motor_medium_builds_valid_frame() {
        let raw = build_motor_command(&MotorPosition::Medium);
        assert_eq!(raw[4], 0x02);
    }

    #[test]
    fn motor_high_builds_valid_frame() {
        let raw = build_motor_command(&MotorPosition::High);
        assert_eq!(raw[4], 0x03);
    }

    #[test]
    fn motor_moving_returns_empty() {
        let raw = build_motor_command(&MotorPosition::Moving);
        assert!(raw.is_empty());
    }

    #[test]
    fn motor_stop_builds_valid_frame() {
        let raw = build_motor_stop();
        assert!(!raw.is_empty());
        assert_eq!(raw[0..2], [0xDC, 0xCD]);
        assert_eq!(raw[3], 0x21);
    }

    #[test]
    fn shutdown_command_builds_valid_frame() {
        let raw = build_shutdown_command();
        assert!(!raw.is_empty());
        assert_eq!(raw[3], 0x30);
    }

    #[test]
    fn reboot_command_builds_valid_frame() {
        let raw = build_reboot_command();
        assert!(!raw.is_empty());
        assert_eq!(raw[3], 0x31);
    }
}
