use waved_state::ButtonType;

pub fn parse_button_frame(data: &[u8]) -> Option<ButtonType> {
    if data.is_empty() {
        return None;
    }
    // NOTE: 0x48/0x49/0x4A were previously mapped here as physical
    // Mute/VolumeDown/VolumeUp presses, "measured on-device via strace". That
    // was a reverse-engineering error: those values are low-bit-corrupted
    // copies of the 0x4B heartbeat, not button input (see AUDIO_HARDWARE.md).
    // Only the framed simulation/legacy codes below are mapped.
    match data[0] {
        0x40 => Some(ButtonType::MuteToggle),
        0x41 => Some(ButtonType::VolumeUp),
        0x42 => Some(ButtonType::VolumeDown),
        0x43 => Some(ButtonType::MuteLongPress),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mute_toggle() {
        assert_eq!(parse_button_frame(&[0x40]), Some(ButtonType::MuteToggle));
    }

    #[test]
    fn parse_volume_up() {
        assert_eq!(parse_button_frame(&[0x41]), Some(ButtonType::VolumeUp));
    }

    #[test]
    fn parse_volume_down() {
        assert_eq!(parse_button_frame(&[0x42]), Some(ButtonType::VolumeDown));
    }

    #[test]
    fn parse_mute_long_press() {
        assert_eq!(parse_button_frame(&[0x43]), Some(ButtonType::MuteLongPress));
    }

    #[test]
    fn parse_unknown_byte_returns_none() {
        assert_eq!(parse_button_frame(&[0x44]), None);
        assert_eq!(parse_button_frame(&[0xFF]), None);
    }

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse_button_frame(&[]), None);
    }
}
