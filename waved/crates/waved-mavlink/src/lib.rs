//! MAVLink v2 framing shared by the MCU link (`/dev/ttyS4`, docs/MCU_PROTOCOL.md)
//! and the Wave Controller PC link (HID report 0x03,
//! docs/WAVE_CONTROLLER_PROTOCOL.md).
//!
//! `FD len incompat compat seq sysid compid msgid[3 LE] payload crc[2 LE]`.
//! Trailing payload zeros are trimmed on the wire (one byte always stays) and
//! zero-filled on receive; the CRC is CRC-16/MCRF4XX over everything after
//! `FD`, then the message's `crc_extra`.

const MAGIC: u8 = 0xFD;
/// Bytes before the payload, magic included.
const HEADER_LEN: usize = 10;

/// Full payload length and `crc_extra` of one message id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MsgSpec {
    pub msgid: u32,
    pub len: usize,
    pub crc_extra: u8,
}

const fn m(msgid: u32, len: usize, crc_extra: u8) -> MsgSpec {
    MsgSpec { msgid, len, crc_extra }
}

/// The stock 1.2.2.6 dialect: `mavlink_message_crcs` in `libins_entry.so`
/// (two identical copies, at `0x231098` and `0x1e37f8`).
pub const STOCK_MESSAGES: &[MsgSpec] = &[
    m(0, 32, 0x45),
    m(1, 32, 0x20),
    m(100, 36, 0x27),
    m(101, 36, 0x3A),
    m(102, 140, 0xA9),
    m(103, 68, 0x15),
    m(1000, 128, 0xF8),
    m(2000, 4, 0x94),
    m(2001, 12, 0x74),
    m(2002, 8, 0x08),
    m(2003, 6, 0x74),
    m(2004, 5, 0x6E),
    m(2005, 5, 0xBD),
    m(2006, 1, 0x41),
    m(2007, 16, 0x8D),
    m(2008, 32, 0xA6),
    m(2009, 1, 0x59),
    m(2010, 1, 0xF6),
    m(2011, 1, 0x9A),
    m(2012, 4, 0x72),
    m(2013, 234, 0x66),
    m(2014, 5, 0xDC),
    m(2015, 22, 0xE3),
    m(2016, 65, 0x8F),
];

/// The messages the MCU link carries: heartbeat, its reply, control, notify, OTA.
pub const MCU_LINK: &[MsgSpec] = &[
    m(0, 32, 0x45),
    m(1, 32, 0x20),
    m(100, 36, 0x27),
    m(101, 36, 0x3A),
    m(102, 140, 0xA9),
    m(103, 68, 0x15),
];

/// The messages the PC link carries: heartbeat_control and PC_CMD_PKG.
pub const PC_LINK: &[MsgSpec] = &[m(0, 32, 0x45), m(1000, 128, 0xF8)];

/// The spec of `msgid` in `table`.
pub fn spec(table: &[MsgSpec], msgid: u32) -> Option<MsgSpec> {
    table.iter().copied().find(|s| s.msgid == msgid)
}

/// Sender fields of a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub seq: u8,
    pub sysid: u8,
    pub compid: u8,
}

/// A decoded message. `payload` is zero-filled to the message's full length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub seq: u8,
    pub sysid: u8,
    pub compid: u8,
    pub msgid: u32,
    pub payload: Vec<u8>,
}

/// CRC-16/MCRF4XX (the MAVLink `crc_accumulate`), init 0xFFFF.
pub fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0xFFFF, |crc, &b| crc_accumulate(crc, b))
}

fn crc_accumulate(crc: u16, byte: u8) -> u16 {
    let mut t = byte ^ (crc as u8);
    t ^= t << 4;
    (crc >> 8) ^ (u16::from(t) << 8) ^ (u16::from(t) << 3) ^ (u16::from(t) >> 4)
}

fn checksum(after_magic: &[u8], extra: u8) -> u16 {
    crc_accumulate(crc16(after_magic), extra)
}

/// One frame of `msgid`, the payload zero-padded to its full length and then
/// trimmed as MAVLink v2 sends it. `None` for a message `table` does not carry
/// or a payload longer than the message.
pub fn encode(table: &[MsgSpec], header: Header, msgid: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let spec = spec(table, msgid)?;
    if payload.len() > spec.len {
        return None;
    }
    let mut body = payload.to_vec();
    body.resize(spec.len, 0);
    while body.len() > 1 && body.last() == Some(&0) {
        body.pop();
    }
    let mut frame = vec![MAGIC, body.len() as u8, 0, 0, header.seq, header.sysid, header.compid];
    frame.extend_from_slice(&msgid.to_le_bytes()[..3]);
    frame.extend_from_slice(&body);
    let crc = checksum(&frame[1..], spec.crc_extra);
    frame.extend_from_slice(&crc.to_le_bytes());
    Some(frame)
}

/// Reassembles frames from a byte stream, one byte at a time.
pub struct Parser {
    table: &'static [MsgSpec],
    buf: Vec<u8>,
}

impl Parser {
    pub fn new(table: &'static [MsgSpec]) -> Self {
        Self { table, buf: Vec::new() }
    }

    /// Forget a partly received frame.
    pub fn reset(&mut self) {
        self.buf.clear();
    }

    /// Feed one received byte; returns a message when it completes one.
    pub fn feed(&mut self, byte: u8) -> Option<Message> {
        if self.buf.is_empty() && byte != MAGIC {
            return None;
        }
        self.buf.push(byte);
        // A header that can't be one of ours (e.g. a cut-off frame whose
        // next `FD` would read as length 253) is dropped up to the next magic.
        while self.buf.len() >= HEADER_LEN && !self.header_ok() {
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
        match self.decode(&frame) {
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

    /// Flags zero, a message this link carries, and a length within it.
    fn header_ok(&self) -> bool {
        let h = &self.buf;
        let msgid = u32::from_le_bytes([h[7], h[8], h[9], 0]);
        h[2] == 0 && h[3] == 0 && spec(self.table, msgid).is_some_and(|s| usize::from(h[1]) <= s.len)
    }

    fn decode(&self, frame: &[u8]) -> Option<Message> {
        let len = usize::from(frame[1]);
        let msgid = u32::from_le_bytes([frame[7], frame[8], frame[9], 0]);
        let spec = spec(self.table, msgid)?;
        let crc = u16::from_le_bytes([frame[HEADER_LEN + len], frame[HEADER_LEN + len + 1]]);
        if checksum(&frame[1..HEADER_LEN + len], spec.crc_extra) != crc {
            return None;
        }
        let mut payload = frame[HEADER_LEN..HEADER_LEN + len].to_vec();
        payload.resize(spec.len.max(len), 0);
        Some(Message { seq: frame[4], sysid: frame[5], compid: frame[6], msgid, payload })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    fn parse_all(table: &'static [MsgSpec], bytes: &[u8]) -> Vec<Message> {
        let mut parser = Parser::new(table);
        bytes.iter().filter_map(|&b| parser.feed(b)).collect()
    }

    const PC: Header = Header { seq: 0, sysid: 0, compid: 0 };

    #[test]
    fn crc_is_mcrf4xx() {
        assert_eq!(crc16(b"123456789"), 0x6F91);
    }

    #[test]
    fn stock_table_has_24_messages_and_the_subsets_agree_with_it() {
        assert_eq!(STOCK_MESSAGES.len(), 24);
        for s in MCU_LINK.iter().chain(PC_LINK) {
            assert_eq!(spec(STOCK_MESSAGES, s.msgid), Some(*s));
        }
        assert_eq!(spec(PC_LINK, 1000), Some(MsgSpec { msgid: 1000, len: 128, crc_extra: 0xF8 }));
        assert_eq!(spec(MCU_LINK, 1000), None);
    }

    // Captured from the MCU on 2026-10-12 (docs/MCU_PROTOCOL.md).
    const MCU_HEARTBEAT: &str = "FD 10 00 00 1C 00 00 00 00 00 00 00 00 00 64 00 52 10 24 00 00 00 00 00 00 02 62 A9";

    #[test]
    fn a_captured_mcu_heartbeat_decodes() {
        let msgs = parse_all(MCU_LINK, &hex(MCU_HEARTBEAT));
        assert_eq!(msgs.len(), 1);
        assert_eq!((msgs[0].msgid, msgs[0].seq, msgs[0].payload.len()), (0, 0x1C, 32));
        assert_eq!(&msgs[0].payload[4..6], &[0x64, 0x00]);
    }

    #[test]
    fn mcu_stock_frames_encode_byte_exact() {
        assert_eq!(encode(MCU_LINK, PC, 1, &[0; 32]).unwrap(), hex("FD 01 00 00 00 00 00 01 00 00 00 FC 8F"));
    }

    // From the reference model of pc_mavlink.c (pc_cmd_pkg, sysid 0 compid 0):
    // the device's ack to SET_AUDIO_MIC_MUTE, seq 0.
    fn ack_packet() -> Vec<u8> {
        let mut p = hex("4A 49 48 47 01 25 2B 00 FF FF FF FF 01 00 00 00 00");
        p.resize(128, 0xFF);
        p
    }

    #[test]
    fn a_pc_cmd_pkg_frame_encodes_byte_exact() {
        let frame = encode(PC_LINK, PC, 1000, &ack_packet()).unwrap();
        assert_eq!(frame.len(), 140);
        assert_eq!(&frame[..22], &hex("FD 80 00 00 00 00 00 E8 03 00 4A 49 48 47 01 25 2B 00 FF FF FF FF")[..]);
        assert_eq!(&frame[138..], &[0xA9, 0x8C]);
    }

    #[test]
    fn a_payload_ending_in_zeros_is_trimmed() {
        let mut p = hex("4A 49 48 47 01 25 14 00 FF FF FF FF 70");
        p.resize(128, 0);
        let frame = encode(PC_LINK, Header { seq: 3, ..PC }, 1000, &p).unwrap();
        assert_eq!(frame, hex("FD 0D 00 00 03 00 00 E8 03 00 4A 49 48 47 01 25 14 00 FF FF FF FF 70 8D F2"));
        let msgs = parse_all(PC_LINK, &frame);
        assert_eq!(msgs[0].payload, p, "zero-filled back to 128 bytes");
    }

    #[test]
    fn compid_is_carried_both_ways() {
        let ob = vec![0u8; 20];
        let frame = encode(PC_LINK, Header { seq: 2, sysid: 0, compid: 1 }, 1000, &ob).unwrap();
        assert_eq!(frame, hex("FD 01 00 00 02 00 01 E8 03 00 00 32 85"));
        let msgs = parse_all(PC_LINK, &frame);
        assert_eq!((msgs[0].compid, msgs[0].seq, msgs[0].msgid), (1, 2, 1000));
    }

    #[test]
    fn encode_refuses_unknown_messages_and_oversized_payloads() {
        assert!(encode(MCU_LINK, PC, 1000, &[0]).is_none());
        assert!(encode(PC_LINK, PC, 0, &[0; 33]).is_none());
    }

    #[test]
    fn a_frame_split_anywhere_still_parses() {
        let frame = encode(PC_LINK, PC, 1000, &ack_packet()).unwrap();
        let mut parser = Parser::new(PC_LINK);
        let mut got = Vec::new();
        for chunk in frame.chunks(52) {
            got.extend(chunk.iter().filter_map(|&b| parser.feed(b)));
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].payload, ack_packet());
    }

    #[test]
    fn a_bad_frame_does_not_eat_the_next_one() {
        let good = encode(PC_LINK, Header { seq: 7, ..PC }, 1000, &ack_packet()).unwrap();
        let mut bad = good.clone();
        bad[30] ^= 0x40;
        let mut line = vec![0x11, 0xFD];
        line.extend(bad);
        line.extend(&good);
        let msgs = parse_all(PC_LINK, &line);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].seq, 7);
    }

    #[test]
    fn reset_drops_a_partial_frame() {
        let frame = encode(PC_LINK, PC, 1000, &ack_packet()).unwrap();
        let mut parser = Parser::new(PC_LINK);
        for &b in &frame[..40] {
            assert!(parser.feed(b).is_none());
        }
        parser.reset();
        assert_eq!(frame.iter().filter_map(|&b| parser.feed(b)).count(), 1);
    }
}
