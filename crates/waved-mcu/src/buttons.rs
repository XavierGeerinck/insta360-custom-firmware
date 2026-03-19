use waved_state::ButtonType;

pub fn parse_button_frame(data: &[u8]) -> Option<ButtonType> {
    if data.is_empty() {
        return None;
    }
    match data[0] {
        0x40 => Some(ButtonType::MuteToggle),
        0x41 => Some(ButtonType::VolumeUp),
        0x42 => Some(ButtonType::VolumeDown),
        0x43 => Some(ButtonType::MuteLongPress),
        _ => None,
    }
}
