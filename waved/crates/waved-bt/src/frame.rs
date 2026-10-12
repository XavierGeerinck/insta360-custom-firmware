//! Framing of the Bluetooth module's UART (`/dev/ttyS1`, 921600 8N1),
//! docs/BLUETOOTH_PROTOCOL.md.
//!
//! `P0 P1 LEN CMD DATA[LEN-1] SUM`
//!
//! - `P0 P1` is `75 45` ("uE") for a request, `75 46` ("uF") for an
//!   acknowledgement; the stock parser also accepts `DC CD`.
//! - `LEN` counts `CMD` and `DATA` (so it is never 0).
//! - `SUM` is the low byte of the sum of `LEN`, `CMD` and `DATA`: the two
//!   prefix bytes are **not** summed (stock `bt_ctrl_cmd_send`, `bt_cmd_parser`).

/// `75 45`: a request (host commands, module replies and events).
pub const PREFIX_REQUEST: [u8; 2] = [0x75, 0x45];
/// `75 46`: an acknowledgement of a received frame.
pub const PREFIX_ACK: [u8; 2] = [0x75, 0x46];
/// `DC CD`: accepted by the stock parser on this port, never sent by stock.
pub const PREFIX_LEGACY: [u8; 2] = [0xDC, 0xCD];

/// Longest `DATA`: `LEN` is one byte and includes `CMD`.
pub const MAX_DATA: usize = 254;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefix {
    Request,
    Ack,
    Legacy,
}

impl Prefix {
    pub fn bytes(self) -> [u8; 2] {
        match self {
            Prefix::Request => PREFIX_REQUEST,
            Prefix::Ack => PREFIX_ACK,
            Prefix::Legacy => PREFIX_LEGACY,
        }
    }

    /// The prefix that `first` then `second` spell, if any.
    fn from_bytes(first: u8, second: u8) -> Option<Self> {
        match [first, second] {
            PREFIX_REQUEST => Some(Prefix::Request),
            PREFIX_ACK => Some(Prefix::Ack),
            PREFIX_LEGACY => Some(Prefix::Legacy),
            _ => None,
        }
    }
}

/// Whether `byte` can start a prefix.
fn starts_prefix(byte: u8) -> bool {
    byte == PREFIX_REQUEST[0] || byte == PREFIX_LEGACY[0]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub prefix: Prefix,
    pub cmd: u8,
    pub data: Vec<u8>,
}

impl Frame {
    pub fn request(cmd: u8, data: &[u8]) -> Self {
        Self { prefix: Prefix::Request, cmd, data: data.to_vec() }
    }

    pub fn ack(cmd: u8, data: &[u8]) -> Self {
        Self { prefix: Prefix::Ack, cmd, data: data.to_vec() }
    }

    /// The bytes on the wire.
    ///
    /// Panics if `data` is longer than [`MAX_DATA`] (a programming error).
    pub fn encode(&self) -> Vec<u8> {
        assert!(self.data.len() <= MAX_DATA, "Bluetooth frame data too long: {}", self.data.len());
        let mut raw = Vec::with_capacity(self.data.len() + 5);
        raw.extend_from_slice(&self.prefix.bytes());
        raw.push(self.data.len() as u8 + 1);
        raw.push(self.cmd);
        raw.extend_from_slice(&self.data);
        raw.push(checksum(&raw[2..]));
        raw
    }
}

/// The checksum over `LEN CMD DATA`.
pub fn checksum(len_cmd_data: &[u8]) -> u8 {
    len_cmd_data.iter().fold(0u8, |sum, &b| sum.wrapping_add(b))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Prefix0(u8),
    Len,
    Cmd,
    Data,
    Sum,
}

/// Reassembles frames from the byte stream, one byte at a time.
///
/// Like stock `bt_cmd_parser`: a zero `LEN` or a wrong `SUM` drops the frame.
/// Unlike stock, a `75` / `DC` that breaks a prefix is retried as the start of
/// the next one, so `75 75 45 ...` still parses.
pub struct Parser {
    state: State,
    prefix: Prefix,
    len: u8,
    cmd: u8,
    data: Vec<u8>,
    sum: u8,
    /// Frames dropped for a wrong `SUM`.
    pub bad_sums: u32,
}

impl Default for Parser {
    fn default() -> Self {
        Self {
            state: State::Idle,
            prefix: Prefix::Request,
            len: 0,
            cmd: 0,
            data: Vec::new(),
            sum: 0,
            bad_sums: 0,
        }
    }
}

impl Parser {
    /// Feed one received byte; returns a frame when it completes one.
    pub fn feed(&mut self, byte: u8) -> Option<Frame> {
        match self.state {
            State::Idle => {
                if starts_prefix(byte) {
                    self.state = State::Prefix0(byte);
                }
            }
            State::Prefix0(first) => match Prefix::from_bytes(first, byte) {
                Some(prefix) => {
                    self.prefix = prefix;
                    self.state = State::Len;
                }
                None if starts_prefix(byte) => self.state = State::Prefix0(byte),
                None => self.state = State::Idle,
            },
            State::Len => {
                if byte == 0 {
                    self.state = State::Idle;
                } else {
                    self.len = byte;
                    self.sum = byte;
                    self.state = State::Cmd;
                }
            }
            State::Cmd => {
                self.cmd = byte;
                self.sum = self.sum.wrapping_add(byte);
                self.data.clear();
                self.state = if self.len == 1 { State::Sum } else { State::Data };
            }
            State::Data => {
                self.data.push(byte);
                self.sum = self.sum.wrapping_add(byte);
                if self.data.len() == usize::from(self.len) - 1 {
                    self.state = State::Sum;
                }
            }
            State::Sum => {
                self.state = State::Idle;
                if byte == self.sum {
                    return Some(Frame {
                        prefix: self.prefix,
                        cmd: self.cmd,
                        data: std::mem::take(&mut self.data),
                    });
                }
                self.bad_sums = self.bad_sums.wrapping_add(1);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(bytes: &[u8]) -> (Vec<Frame>, Parser) {
        let mut p = Parser::default();
        let frames = bytes.iter().filter_map(|&b| p.feed(b)).collect();
        (frames, p)
    }

    // Fixtures built with the stock rule (bt_ctrl_cmd_send 74620-74639):
    // see docs/BLUETOOTH_PROTOCOL.md "Example frames".

    #[test]
    fn checksum_skips_the_prefix() {
        // GET_BT_STATUS: 75 45 01 A9 AA (0x01 + 0xA9 = 0xAA).
        assert_eq!(checksum(&[0x01, 0xA9]), 0xAA);
        // Wraps at 256: OTA_REQUEST 75 45 02 80 AB -> 0x12D -> 0x2D.
        assert_eq!(checksum(&[0x02, 0x80, 0xAB]), 0x2D);
    }

    #[test]
    fn encodes_a_request_without_data() {
        assert_eq!(Frame::request(0xA9, &[]).encode(), [0x75, 0x45, 0x01, 0xA9, 0xAA]);
    }

    #[test]
    fn encodes_a_request_with_data() {
        assert_eq!(
            Frame::request(0xEA, &[0x01, 0x00]).encode(),
            [0x75, 0x45, 0x03, 0xEA, 0x01, 0x00, 0xEE]
        );
    }

    #[test]
    fn encodes_an_ack() {
        assert_eq!(Frame::ack(0xB2, &[0x00]).encode(), [0x75, 0x46, 0x02, 0xB2, 0x00, 0xB4]);
        assert_eq!(Frame::ack(0xD4, &[]).encode(), [0x75, 0x46, 0x01, 0xD4, 0xD5]);
    }

    #[test]
    fn prefix_bytes() {
        assert_eq!(Prefix::Request.bytes(), PREFIX_REQUEST);
        assert_eq!(Prefix::Ack.bytes(), PREFIX_ACK);
        assert_eq!(Prefix::Legacy.bytes(), PREFIX_LEGACY);
    }

    #[test]
    #[should_panic]
    fn refuses_data_longer_than_a_len_byte_allows() {
        Frame::request(0x9F, &[0u8; MAX_DATA + 1]).encode();
    }

    #[test]
    fn encodes_the_longest_frame() {
        let raw = Frame::request(0x9F, &[1u8; MAX_DATA]).encode();
        assert_eq!(raw.len(), 2 + 1 + 1 + MAX_DATA + 1);
        assert_eq!(raw[2], 0xFF);
    }

    #[test]
    fn parses_a_module_event() {
        // Work status, bits 0x07, name "Pixel", MAC 11:22:33:44:55:66.
        let raw = [
            0x75, 0x45, 0x11, 0xB2, 0x07, 0x00, 0x05, 0x50, 0x69, 0x78, 0x65, 0x6C, 0x00, 0x06,
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x3C,
        ];
        let (frames, _) = parse_all(&raw);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].prefix, Prefix::Request);
        assert_eq!(frames[0].cmd, 0xB2);
        assert_eq!(frames[0].data, raw[4..20].to_vec());
    }

    #[test]
    fn parses_ack_and_legacy_prefixes() {
        let mut raw = Frame::ack(0xA9, &[0x00]).encode();
        raw.extend(Frame { prefix: Prefix::Legacy, cmd: 0x10, data: vec![0x55] }.encode());
        let (frames, _) = parse_all(&raw);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], Frame::ack(0xA9, &[0x00]));
        assert_eq!(frames[1].prefix, Prefix::Legacy);
        assert_eq!(frames[1].data, vec![0x55]);
    }

    #[test]
    fn parses_a_frame_without_data() {
        let (frames, _) = parse_all(&[0x75, 0x45, 0x01, 0xB1, 0xB2]);
        assert_eq!(frames, vec![Frame::request(0xB1, &[])]);
    }

    #[test]
    fn round_trips_every_length() {
        for n in [0usize, 1, 2, 63, 76, MAX_DATA] {
            let f = Frame::request(0x9F, &(0..n).map(|i| i as u8).collect::<Vec<_>>());
            let (frames, _) = parse_all(&f.encode());
            assert_eq!(frames, vec![f], "data length {n}");
        }
    }

    #[test]
    fn drops_a_frame_with_a_wrong_sum_and_counts_it() {
        let mut raw = Frame::request(0xAA, &[0x01]).encode();
        *raw.last_mut().unwrap() ^= 0xFF;
        raw.extend(Frame::request(0xAA, &[0x00]).encode());
        let (frames, p) = parse_all(&raw);
        assert_eq!(frames, vec![Frame::request(0xAA, &[0x00])]);
        assert_eq!(p.bad_sums, 1);
    }

    #[test]
    fn a_zero_len_is_not_a_frame() {
        // Stock: LEN 0 -> back to idle. The bytes after it resync.
        let mut raw = vec![0x75, 0x45, 0x00];
        raw.extend(Frame::request(0xC5, &[]).encode());
        let (frames, _) = parse_all(&raw);
        assert_eq!(frames, vec![Frame::request(0xC5, &[])]);
    }

    #[test]
    fn resyncs_after_garbage_and_a_broken_prefix() {
        let mut raw = vec![0xFF, 0x00, 0x4B, 0x75, 0x00, 0xDC, 0x01, 0x75];
        raw.extend(Frame::request(0x91, &[]).encode());
        let (frames, _) = parse_all(&raw);
        assert_eq!(frames, vec![Frame::request(0x91, &[])]);
    }

    #[test]
    fn a_repeated_prefix_byte_restarts_the_prefix() {
        let mut raw = vec![0x75];
        raw.extend(Frame::request(0x93, &[]).encode());
        let (frames, _) = parse_all(&raw);
        assert_eq!(frames, vec![Frame::request(0x93, &[])]);
    }

    #[test]
    fn back_to_back_frames() {
        let mut raw = Vec::new();
        for cmd in [0xAA, 0xB4, 0xC6] {
            raw.extend(Frame::request(cmd, &[cmd]).encode());
        }
        let (frames, _) = parse_all(&raw);
        assert_eq!(frames.iter().map(|f| f.cmd).collect::<Vec<_>>(), [0xAA, 0xB4, 0xC6]);
    }
}
