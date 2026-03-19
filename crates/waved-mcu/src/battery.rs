use waved_state::BatteryState;

/// Parse battery status from MCU frame data.
pub fn parse_battery_frame(data: &[u8]) -> Option<BatteryState> {
    if data.len() < 5 {
        return None;
    }
    Some(BatteryState {
        level: data[0],
        voltage: u16::from_le_bytes([data[1], data[2]]),
        temperature: data[3] as i8,
        is_charging: data[4] != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_battery() {
        let data = [85, 0x20, 0x0F, 25, 1];
        let state = parse_battery_frame(&data).unwrap();
        assert_eq!(state.level, 85);
        assert!(state.is_charging);
        assert_eq!(state.temperature, 25);
    }

    #[test]
    fn parse_short_frame_returns_none() {
        assert!(parse_battery_frame(&[1, 2]).is_none());
    }
}
