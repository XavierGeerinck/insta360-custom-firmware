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
