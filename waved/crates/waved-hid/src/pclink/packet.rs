//! The 128-byte PC-link packet carried in each PC_CMD_PKG message
//! (docs/WAVE_CONTROLLER_PROTOCOL.md §4, stock `pc_link.c`).
//!
//! `token[4 LE] ver flags cmd[2 LE] seq[4 LE] len[4 LE] payload[112]`; unused
//! payload bytes are `0xFF`. A message longer than 112 bytes is split; the
//! flags mark the first, middle and last packet.

pub const PACKET_LEN: usize = 128;
pub const HEADER_LEN: usize = 16;
pub const PAYLOAD_MAX: usize = PACKET_LEN - HEADER_LEN;
/// The session token before any authentication, and after UNAUTH_DEV.
pub const DEFAULT_TOKEN: u32 = 0x4748_494A;
pub const PROTOCOL_VERSION: u8 = 1;
/// Largest command waved reassembles (stock: 5 MiB); longer ones are
/// answered "unsupported" and not kept.
pub const REASSEMBLY_MAX: usize = 64 * 1024;

const TYPE_ACK: u8 = 1;
const ACK_FLAG: u8 = 1 << 2;
const ENCRYPTED: u8 = 1 << 4;
const END_LAST: u8 = 1;
const END_FIRST: u8 = 2;

/// The decoded 16-byte header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub token: u32,
    pub version: u8,
    pub flags: u8,
    pub cmd: u16,
    pub seq: u32,
    pub payload_len: u32,
}

impl Header {
    pub fn is_ack(&self) -> bool {
        self.flags & 0b11 == TYPE_ACK
    }

    pub fn encrypted(&self) -> bool {
        self.flags & ENCRYPTED != 0
    }

    /// 0 middle, 1 last (or only), 2 first.
    pub fn end(&self) -> u8 {
        (self.flags >> 5) & 0b11
    }
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Header and used payload of one packet; `None` when shorter than a header.
pub fn parse(packet: &[u8]) -> Option<(Header, &[u8])> {
    if packet.len() < HEADER_LEN {
        return None;
    }
    let header = Header {
        token: le32(&packet[0..4]),
        version: packet[4],
        flags: packet[5],
        cmd: u16::from_le_bytes([packet[6], packet[7]]),
        seq: le32(&packet[8..12]),
        payload_len: le32(&packet[12..16]),
    };
    let available = packet.len().min(PACKET_LEN) - HEADER_LEN;
    let used = (header.payload_len as usize).min(available);
    Some((header, &packet[HEADER_LEN..HEADER_LEN + used]))
}

/// Split as stock `pc_link_snd_msg_pkt` does; `seq` is `None` for acks.
fn segment(token: u32, ack: bool, cmd: u16, message: &[u8], mut seq: Option<&mut u32>) -> Vec<[u8; PACKET_LEN]> {
    let count = message.len().div_ceil(PAYLOAD_MAX).max(1);
    (0..count)
        .map(|i| {
            let last = i == count - 1;
            let len = if last && message.len() % PAYLOAD_MAX != 0 { message.len() % PAYLOAD_MAX } else { PAYLOAD_MAX };
            let end = if last { END_LAST } else if i == 0 { END_FIRST } else { 0 };
            let mut p = [0xFFu8; PACKET_LEN];
            p[0..4].copy_from_slice(&token.to_le_bytes());
            p[4] = PROTOCOL_VERSION;
            p[5] = u8::from(ack) | ACK_FLAG | (end << 5);
            p[6..8].copy_from_slice(&cmd.to_le_bytes());
            if let Some(seq) = seq.as_deref_mut() {
                p[8..12].copy_from_slice(&seq.to_le_bytes());
                *seq = seq.wrapping_add(1);
            }
            p[12..16].copy_from_slice(&(len as u32).to_le_bytes());
            let chunk = &message[(i * PAYLOAD_MAX).min(message.len())..];
            let n = chunk.len().min(len);
            p[HEADER_LEN..HEADER_LEN + n].copy_from_slice(&chunk[..n]);
            p
        })
        .collect()
}

/// The packets of one device ack (stock `pc_link_snd_ack_msg`): pktType 1,
/// ackFlag 1, seq `FF FF FF FF`.
pub fn segment_ack(token: u32, cmd: u16, message: &[u8]) -> Vec<[u8; PACKET_LEN]> {
    segment(token, true, cmd, message, None)
}

/// The packets of one command as a host sends it: pktType 0, ackFlag 1, `seq`
/// counting up per packet. waved never sends commands; tests use it.
pub fn segment_command(token: u32, cmd: u16, message: &[u8], seq: &mut u32) -> Vec<[u8; PACKET_LEN]> {
    segment(token, false, cmd, message, Some(seq))
}

/// An out-of-band ack (compid 1): an all-zero 16-byte header, then the payload
/// (stock `pc_link_slave_recv_ob_pkt_handle`).
pub fn ob_ack(payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; HEADER_LEN];
    out.extend_from_slice(payload);
    out
}

/// One reassembled command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// The header of the last packet.
    pub header: Header,
    pub data: Vec<u8>,
    /// Longer than `REASSEMBLY_MAX`; `data` holds only the start.
    pub overflowed: bool,
}

#[derive(Default)]
pub struct Reassembler {
    buf: Vec<u8>,
    overflowed: bool,
}

impl Reassembler {
    /// Add one packet; returns the command when this was its last packet.
    pub fn push(&mut self, header: Header, payload: &[u8]) -> Option<Message> {
        if header.end() == END_FIRST {
            self.reset();
        }
        if self.buf.len() + payload.len() > REASSEMBLY_MAX {
            self.overflowed = true;
        } else if !self.overflowed {
            self.buf.extend_from_slice(payload);
        }
        if header.end() != END_LAST {
            return None;
        }
        let message = Message { header, data: std::mem::take(&mut self.buf), overflowed: self.overflowed };
        self.overflowed = false;
        Some(message)
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.overflowed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    fn padded(prefix: &str) -> Vec<u8> {
        let mut p = hex(prefix);
        p.resize(PACKET_LEN, 0xFF);
        p
    }

    // Reference model of pc_link_snd_msg_pkt (stock 1.2.2.6).
    const ACK_MIC_MUTE: &str = "4A 49 48 47 01 25 2B 00 FF FF FF FF 01 00 00 00 00";
    const CMD_MIC_MUTE_SEQ5: &str = "4A 49 48 47 01 24 2B 00 05 00 00 00 01 00 00 00 01";

    #[test]
    fn a_one_byte_ack_is_one_packet_like_stock() {
        let packets = segment_ack(DEFAULT_TOKEN, 0x2B, &[0]);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].to_vec(), padded(ACK_MIC_MUTE));
    }

    #[test]
    fn a_host_command_packet_matches_the_reference() {
        let mut seq = 5;
        let packets = segment_command(DEFAULT_TOKEN, 0x2B, &[1], &mut seq);
        assert_eq!(packets[0].to_vec(), padded(CMD_MIC_MUTE_SEQ5));
        assert_eq!(seq, 6);
    }

    #[test]
    fn a_318_byte_ack_splits_112_112_94_with_first_middle_last_flags() {
        let message: Vec<u8> = (0..318).map(|i| i as u8).collect();
        let packets = segment_ack(DEFAULT_TOKEN, 0x40, &message);
        let flags: Vec<u8> = packets.iter().map(|p| p[5]).collect();
        assert_eq!(flags, vec![0x45, 0x05, 0x25]);
        let lens: Vec<u32> = packets.iter().map(|p| u32::from_le_bytes(p[12..16].try_into().unwrap())).collect();
        assert_eq!(lens, vec![112, 112, 94]);
        assert!(packets.iter().all(|p| p[8..12] == [0xFF; 4]), "ack seq is never set");
        assert_eq!(&packets[2][16..110], &message[224..]);
        assert!(packets[2][110..].iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn an_exact_multiple_of_112_ends_with_a_full_packet() {
        let packets = segment_ack(DEFAULT_TOKEN, 0x01, &[7; 224]);
        assert_eq!(packets.len(), 2);
        assert_eq!(u32::from_le_bytes(packets[1][12..16].try_into().unwrap()), 112);
    }

    #[test]
    fn an_empty_message_is_one_packet_of_112_ff_bytes_like_stock() {
        let packets = segment_ack(DEFAULT_TOKEN, 0x01, &[]);
        assert_eq!(packets.len(), 1);
        assert_eq!(u32::from_le_bytes(packets[0][12..16].try_into().unwrap()), 112);
        assert!(packets[0][16..].iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn parse_reads_the_header_and_the_used_payload() {
        let raw = padded(CMD_MIC_MUTE_SEQ5);
        let (h, payload) = parse(&raw).unwrap();
        assert_eq!(h, Header { token: DEFAULT_TOKEN, version: 1, flags: 0x24, cmd: 0x2B, seq: 5, payload_len: 1 });
        assert!(!h.is_ack());
        assert!(!h.encrypted());
        assert_eq!(h.end(), 1);
        assert_eq!(payload, &[1]);
        assert!(parse(&raw[..15]).is_none());
    }

    #[test]
    fn a_payload_length_past_the_packet_is_clamped() {
        let mut raw = padded("4A 49 48 47 01 24 2B 00 05 00 00 00 FF 00 00 00");
        raw[16] = 9;
        let (_, payload) = parse(&raw).unwrap();
        assert_eq!(payload.len(), PAYLOAD_MAX);
    }

    #[test]
    fn reassembly_joins_first_middle_last() {
        let message: Vec<u8> = (0..300).map(|i| (i * 7) as u8).collect();
        let mut seq = 0;
        let mut r = Reassembler::default();
        let mut done = None;
        for p in segment_command(DEFAULT_TOKEN, 0x12, &message, &mut seq) {
            let (h, payload) = parse(&p).unwrap();
            assert!(done.is_none());
            done = r.push(h, payload);
        }
        let m = done.unwrap();
        assert_eq!((m.header.cmd, m.data, m.overflowed), (0x12, message, false));
    }

    #[test]
    fn a_new_first_packet_drops_a_half_message() {
        let mut seq = 0;
        let mut r = Reassembler::default();
        let first = segment_command(DEFAULT_TOKEN, 0x12, &[1; 200], &mut seq);
        let (h, p) = parse(&first[0]).unwrap();
        assert!(r.push(h, p).is_none());
        let again = segment_command(DEFAULT_TOKEN, 0x12, &[2; 200], &mut seq);
        let mut done = None;
        for p in &again {
            let (h, p) = parse(p).unwrap();
            done = r.push(h, p);
        }
        assert_eq!(done.unwrap().data, vec![2; 200]);
    }

    #[test]
    fn an_oversized_command_is_marked_and_not_kept() {
        let mut seq = 0;
        let mut r = Reassembler::default();
        let mut done = None;
        for p in segment_command(DEFAULT_TOKEN, 0x47, &vec![5; REASSEMBLY_MAX + 500], &mut seq) {
            let (h, p) = parse(&p).unwrap();
            done = r.push(h, p);
        }
        let m = done.unwrap();
        assert!(m.overflowed);
        assert!(m.data.len() <= REASSEMBLY_MAX);
        let mut seq = 0;
        let p = &segment_command(DEFAULT_TOKEN, 0x2B, &[1], &mut seq)[0];
        let (h, p) = parse(p).unwrap();
        assert_eq!(r.push(h, p).unwrap(), Message { header: h, data: vec![1], overflowed: false }, "fresh after");
    }

    #[test]
    fn an_ob_ack_has_a_zero_header() {
        let ack = ob_ack(&[1, 2, 3, 4]);
        assert_eq!(ack.len(), 20);
        assert!(ack[..16].iter().all(|&b| b == 0));
        assert_eq!(&ack[16..], &[1, 2, 3, 4]);
    }
}
