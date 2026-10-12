//! MAVLink v2 framing for the MCU link on `/dev/ttyS4` (docs/MCU_PROTOCOL.md).
//!
//! `FD len incompat compat seq sysid compid msgid[3 LE] payload crc[2 LE]`.
//! Trailing payload zeros are trimmed on the wire and zero-filled here; the
//! CRC is CRC-16/MCRF4XX over everything after `FD`, then the message's
//! `crc_extra`.

const MAGIC: u8 = 0xFD;
/// Bytes before the payload, magic included.
const HEADER_LEN: usize = 10;

/// A decoded message. `payload` is zero-filled to the message's full length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub seq: u8,
    pub msgid: u32,
    pub payload: Vec<u8>,
}

/// Full payload length and `crc_extra` of the messages the MCU link carries.
fn spec(msgid: u32) -> Option<(usize, u8)> {
    match msgid {
        0 => Some((32, 0x45)),
        1 => Some((32, 0x20)),
        100 => Some((36, 0x27)),
        101 => Some((36, 0x3A)),
        102 => Some((140, 0xA9)),
        103 => Some((68, 0x15)),
        _ => None,
    }
}

/// The full payload length of `msgid`, if this link carries it.
pub fn full_len(msgid: u32) -> Option<usize> {
    spec(msgid).map(|(len, _)| len)
}

fn crc_accumulate(crc: u16, byte: u8) -> u16 {
    let mut t = byte ^ (crc as u8);
    t ^= t << 4;
    (crc >> 8) ^ (u16::from(t) << 8) ^ (u16::from(t) << 3) ^ (u16::from(t) >> 4)
}

fn checksum(after_magic: &[u8], extra: u8) -> u16 {
    let crc = after_magic.iter().fold(0xFFFF, |crc, &b| crc_accumulate(crc, b));
    crc_accumulate(crc, extra)
}

/// One frame for `msgid`, with the payload zero-padded to its full length
/// and then trimmed as MAVLink v2 sends it.
///
/// Panics on a `msgid` this link does not carry (a programming error).
pub fn encode(seq: u8, msgid: u32, payload: &[u8]) -> Vec<u8> {
    let (full, extra) = spec(msgid).expect("msgid carried by the MCU link");
    let mut body = payload.to_vec();
    body.resize(full, 0);
    while body.len() > 1 && body.last() == Some(&0) {
        body.pop();
    }
    let mut frame = vec![MAGIC, body.len() as u8, 0, 0, seq, 0, 0];
    frame.extend_from_slice(&msgid.to_le_bytes()[..3]);
    frame.extend_from_slice(&body);
    let crc = checksum(&frame[1..], extra);
    frame.extend_from_slice(&crc.to_le_bytes());
    frame
}

/// Reassembles frames from the byte stream, one byte at a time.
#[derive(Default)]
pub struct Parser {
    buf: Vec<u8>,
}

impl Parser {
    /// Feed one received byte; returns a message when it completes one.
    pub fn feed(&mut self, byte: u8) -> Option<Message> {
        if self.buf.is_empty() && byte != MAGIC {
            return None;
        }
        self.buf.push(byte);
        // A header that can't be one of ours (e.g. a cut-off frame whose
        // next `FD` would read as length 253) is dropped up to the next magic.
        while self.buf.len() >= HEADER_LEN && !header_ok(&self.buf) {
            self.resync();
        }
        if self.buf.len() < HEADER_LEN {
            return None;
        }
        let total = HEADER_LEN + usize::from(self.buf[1]) + 2;
        if self.buf.len() < total {
            return None;
        }
        let frame: Vec<u8> = self.buf.drain(..total).collect();
        match decode(&frame) {
            Some(message) => Some(message),
            None => {
                self.buf = frame;
                self.resync();
                None
            }
        }
    }

    /// Drop the leading magic and everything up to the next one.
    fn resync(&mut self) {
        match self.buf[1..].iter().position(|&b| b == MAGIC) {
            Some(at) => {
                self.buf.drain(..=at);
            }
            None => self.buf.clear(),
        }
    }
}

/// Flags zero, a message this link carries, and a length within it.
fn header_ok(h: &[u8]) -> bool {
    let msgid = u32::from_le_bytes([h[7], h[8], h[9], 0]);
    h[2] == 0 && h[3] == 0 && spec(msgid).is_some_and(|(full, _)| usize::from(h[1]) <= full)
}

fn decode(frame: &[u8]) -> Option<Message> {
    let len = usize::from(frame[1]);
    let msgid = u32::from_le_bytes([frame[7], frame[8], frame[9], 0]);
    let (full, extra) = spec(msgid)?;
    let crc = u16::from_le_bytes([frame[HEADER_LEN + len], frame[HEADER_LEN + len + 1]]);
    if checksum(&frame[1..HEADER_LEN + len], extra) != crc {
        return None;
    }
    let mut payload = frame[HEADER_LEN..HEADER_LEN + len].to_vec();
    payload.resize(full.max(len), 0);
    Some(Message { seq: frame[4], msgid, payload })
}
#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    // Captured on the device, 2026-10-12 (docs/MCU_PROTOCOL.md).
    const HEARTBEAT: &str = "FD 10 00 00 1C 00 00 00 00 00 00 00 00 00 64 00 52 10 24 00 00 00 00 00 00 02 62 A9";
    const PRESSED: &str = "FD 01 00 00 1D 00 00 65 00 00 0E 31 C7";
    const CLICKED: &str = "FD 06 00 00 1E 00 00 65 00 00 0E 00 00 00 00 01 06 66";

    fn parse_all(bytes: &[u8]) -> Vec<Message> {
        let mut parser = Parser::default();
        bytes.iter().filter_map(|&b| parser.feed(b)).collect()
    }

    #[test]
    fn captured_frames_decode_with_zero_filled_payloads() {
        let msgs = parse_all(&[hex(HEARTBEAT), hex(PRESSED), hex(CLICKED)].concat());
        assert_eq!(msgs.len(), 3);
        assert_eq!((msgs[0].msgid, msgs[0].seq, msgs[0].payload.len()), (0, 0x1C, 32));
        assert_eq!(&msgs[0].payload[4..6], &[0x64, 0x00]);
        assert_eq!((msgs[1].msgid, msgs[1].payload.len()), (101, 36));
        assert!(msgs[1].payload[1..].iter().all(|&b| b == 0), "trimmed payload is zero-filled");
        assert_eq!(msgs[2].payload[5], 1);
    }

    #[test]
    fn encode_reproduces_the_stock_frames() {
        assert_eq!(encode(0, 1, &[0; 32]), hex("FD 01 00 00 00 00 00 01 00 00 00 FC 8F"));
        let mut build_time = 0x0Fu32.to_le_bytes().to_vec();
        build_time.resize(36, 0);
        assert_eq!(encode(0, 100, &build_time), hex("FD 01 00 00 00 00 00 64 00 00 0F 6C FF"));
    }

    #[test]
    fn encode_then_parse_round_trips() {
        let payload: Vec<u8> = (1..=10).collect();
        let msgs = parse_all(&encode(200, 100, &payload));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].seq, 200);
        assert_eq!(&msgs[0].payload[..10], &payload[..]);
    }

    #[test]
    fn a_bad_frame_does_not_eat_the_next_one() {
        let mut corrupt = hex(CLICKED);
        corrupt[15] ^= 0x40; // the CRC no longer matches
        let mut line = vec![0x4B, 0x00, 0xFD]; // noise and a cut-off start
        line.extend(corrupt);
        line.extend(hex(PRESSED));
        let msgs = parse_all(&line);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].seq, 0x1D);
    }

    #[test]
    fn unknown_message_ids_are_skipped() {
        let mut parser = Parser::default();
        // A well-formed v2 header with msgid 7, which this link never carries.
        for b in hex("FD 01 00 00 00 00 00 07 00 00 00 00 00") {
            assert!(parser.feed(b).is_none());
        }
        assert!(parse_all(&hex(PRESSED)).len() == 1);
    }
}
