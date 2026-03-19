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
