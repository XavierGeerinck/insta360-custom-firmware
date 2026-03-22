use waved_uart::frame::Frame;

const BT_HEADER: [u8; 2] = [0x75, 0x46];

pub fn build_get_status() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x0E]).unwrap_or_default()
}

pub fn build_get_mac() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x09]).unwrap_or_default()
}

pub fn build_get_name() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x0A]).unwrap_or_default()
}

pub fn build_get_version() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x0D]).unwrap_or_default()
}

pub fn build_open() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x07]).unwrap_or_default()
}

pub fn build_close() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x03]).unwrap_or_default()
}

pub fn build_disconnect() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x02]).unwrap_or_default()
}

pub fn build_set_auto_reconnect(enable: bool) -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x04, if enable { 1 } else { 0 }]).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_bt_frame(raw: &[u8], expected_cmd: u8) {
        assert!(!raw.is_empty(), "frame should not be empty");
        assert_eq!(raw[0..2], [0x75, 0x46], "should have BT header");
        assert_eq!(raw[3], expected_cmd, "wrong command byte");
    }

    #[test]
    fn get_status() { assert_bt_frame(&build_get_status(), 0x0E); }
    #[test]
    fn get_mac() { assert_bt_frame(&build_get_mac(), 0x09); }
    #[test]
    fn get_name() { assert_bt_frame(&build_get_name(), 0x0A); }
    #[test]
    fn get_version() { assert_bt_frame(&build_get_version(), 0x0D); }
    #[test]
    fn open() { assert_bt_frame(&build_open(), 0x07); }
    #[test]
    fn close() { assert_bt_frame(&build_close(), 0x03); }
    #[test]
    fn disconnect() { assert_bt_frame(&build_disconnect(), 0x02); }

    #[test]
    fn set_auto_reconnect_enable() {
        let raw = build_set_auto_reconnect(true);
        assert_eq!(raw[0..2], [0x75, 0x46]);
        assert_eq!(raw[3], 0x04);
        assert_eq!(raw[4], 1);
    }

    #[test]
    fn set_auto_reconnect_disable() {
        let raw = build_set_auto_reconnect(false);
        assert_eq!(raw[4], 0);
    }
}
