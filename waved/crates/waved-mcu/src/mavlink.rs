//! MAVLink v2 framing for the MCU link on `/dev/ttyS4` (docs/MCU_PROTOCOL.md).
//!
//! A thin layer over `waved-mavlink`, which the Wave Controller link shares:
//! `sysid = compid = 0` and only the MCU's messages (`MCU_LINK`), so frames
//! from another dialect are skipped like any other unknown message.

use waved_mavlink::{Header, MCU_LINK};

pub use waved_mavlink::Message;

/// The full payload length of `msgid`, if this link carries it.
pub fn full_len(msgid: u32) -> Option<usize> {
    waved_mavlink::spec(MCU_LINK, msgid).map(|s| s.len)
}

/// One frame for `msgid`, with the payload zero-padded to its full length
/// and then trimmed as MAVLink v2 sends it.
///
/// Panics on a `msgid` this link does not carry, or a payload longer than the
/// message (programming errors).
pub fn encode(seq: u8, msgid: u32, payload: &[u8]) -> Vec<u8> {
    waved_mavlink::encode(MCU_LINK, Header { seq, sysid: 0, compid: 0 }, msgid, payload)
        .expect("msgid carried by the MCU link, payload within its length")
}

/// Reassembles frames from the byte stream, one byte at a time.
pub struct Parser(waved_mavlink::Parser);

impl Default for Parser {
    fn default() -> Self {
        Self(waved_mavlink::Parser::new(MCU_LINK))
    }
}

impl Parser {
    /// Feed one received byte; returns a message when it completes one.
    pub fn feed(&mut self, byte: u8) -> Option<Message> {
        self.0.feed(byte)
    }
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
