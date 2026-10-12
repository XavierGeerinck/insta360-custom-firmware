//! Vendor report 0x03: the 64-byte HID reports the PC link travels in
//! (docs/WAVE_CONTROLLER_PROTOCOL.md §1–2; stock `hid_protocol.c`).
//!
//! Transport messages: `03 type id[8 LE] len payload[52] 00`.
//! Short heartbeat: `03 EC|ED …` from the host, `03 EF 3A status[58] 00 00 00`
//! from the device.

pub const REPORT_ID: u8 = 0x03;
pub const REPORT_LEN: usize = 64;
/// Payload bytes one transport message carries.
pub const CHUNK_MAX: usize = 52;
/// Size of `pclink_heartbeat_short_status`.
pub const SHORT_STATUS_LEN: usize = 58;

pub type Report = [u8; REPORT_LEN];

const HEARTBEAT_REQ: u8 = 0xEC;
const HEARTBEAT_NEW_REQ: u8 = 0xED;
const HEARTBEAT_RESP: u8 = 0xEF;

/// Transport message types (`enum HID_MSG_TYPE`, values 0–6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mavlink,
    Resend,
    ResendResp,
    Reset,
    ResetResp,
    Sync,
    SyncResp,
}

impl Kind {
    pub fn code(self) -> u8 {
        match self {
            Kind::Mavlink => 0,
            Kind::Resend => 1,
            Kind::ResendResp => 2,
            Kind::Reset => 3,
            Kind::ResetResp => 4,
            Kind::Sync => 5,
            Kind::SyncResp => 6,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Kind::Mavlink,
            1 => Kind::Resend,
            2 => Kind::ResendResp,
            3 => Kind::Reset,
            4 => Kind::ResetResp,
            5 => Kind::Sync,
            6 => Kind::SyncResp,
            _ => return None,
        })
    }
}

/// One transport message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub kind: Kind,
    pub send_id: u64,
    /// At most `CHUNK_MAX` bytes.
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn mavlink(send_id: u64, chunk: &[u8]) -> Self {
        Self { kind: Kind::Mavlink, send_id, payload: chunk.to_vec() }
    }

    /// RESEND, SYNC and SYNC_RESP carry one zero byte.
    pub fn control(kind: Kind, send_id: u64) -> Self {
        Self { kind, send_id, payload: vec![0] }
    }

    /// `03 04 00×8 01 01`: stock's reply to a RESET.
    pub fn reset_resp() -> Self {
        Self { kind: Kind::ResetResp, send_id: 0, payload: vec![1] }
    }

    pub fn encode(&self) -> Report {
        let len = self.payload.len().min(CHUNK_MAX);
        let mut r = [0u8; REPORT_LEN];
        r[0] = REPORT_ID;
        r[1] = self.kind.code();
        r[2..10].copy_from_slice(&self.send_id.to_le_bytes());
        r[10] = len as u8;
        r[11..11 + len].copy_from_slice(&self.payload[..len]);
        r
    }
}

/// What one received report is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoming {
    Transport(Packet),
    /// `03 EC` or `03 ED`: answer with a short heartbeat at once.
    HeartbeatRequest,
    /// A vendor report waved ignores (cascade 0xA1–0xA5, factory switch 0xF0, …).
    Ignored(u8),
}

/// Decode a report read from `/dev/hidg0`; `None` unless it is a 64-byte
/// report 0x03.
pub fn decode(report: &[u8]) -> Option<Incoming> {
    if report.len() != REPORT_LEN || report[0] != REPORT_ID {
        return None;
    }
    let code = report[1];
    if code == HEARTBEAT_REQ || code == HEARTBEAT_NEW_REQ {
        return Some(Incoming::HeartbeatRequest);
    }
    let Some(kind) = Kind::from_code(code) else {
        return Some(Incoming::Ignored(code));
    };
    let mut id = [0u8; 8];
    id.copy_from_slice(&report[2..10]);
    let len = usize::from(report[10]).min(CHUNK_MAX);
    Some(Incoming::Transport(Packet {
        kind,
        send_id: u64::from_le_bytes(id),
        payload: report[11..11 + len].to_vec(),
    }))
}

/// `03 EF 3A <status> 00 00 00`.
pub fn heartbeat_response(status: &[u8; SHORT_STATUS_LEN]) -> Report {
    let mut r = [0u8; REPORT_LEN];
    r[0] = REPORT_ID;
    r[1] = HEARTBEAT_RESP;
    r[2] = SHORT_STATUS_LEN as u8;
    r[3..3 + SHORT_STATUS_LEN].copy_from_slice(status);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    fn report(prefix: &str) -> Report {
        let mut r = [0u8; REPORT_LEN];
        let p = hex(prefix);
        r[..p.len()].copy_from_slice(&p);
        r
    }

    #[test]
    fn control_messages_encode_like_stock() {
        assert_eq!(Packet::reset_resp().encode(), report("03 04 00 00 00 00 00 00 00 00 01 01"));
        assert_eq!(Packet::control(Kind::Resend, 7).encode(), report("03 01 07 00 00 00 00 00 00 00 01 00"));
        assert_eq!(Packet::control(Kind::Sync, 300).encode(), report("03 05 2C 01 00 00 00 00 00 00 01 00"));
        assert_eq!(Packet::control(Kind::SyncResp, 300).encode(), report("03 06 2C 01 00 00 00 00 00 00 01 00"));
    }

    // From the reference model: the first chunk of a host SET_AUDIO_MIC_MUTE.
    const HOST_CHUNK_1: &str = "03 00 01 00 00 00 00 00 00 00 34 FD 80 00 00 09 00 00 E8 03 00 4A 49 48 47 01 24 2B 00 05 00 00 00 01 00 00 00 01 FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF 00";

    #[test]
    fn a_mavlink_chunk_round_trips() {
        let raw = hex(HOST_CHUNK_1);
        let Some(Incoming::Transport(p)) = decode(&raw) else { panic!("transport") };
        assert_eq!((p.kind, p.send_id, p.payload.len()), (Kind::Mavlink, 1, 52));
        assert_eq!(&p.payload[..4], &[0xFD, 0x80, 0x00, 0x00]);
        assert_eq!(p.encode().to_vec(), raw);
    }

    #[test]
    fn a_short_chunk_keeps_its_length_and_zero_tail() {
        let p = Packet::mavlink(0x0102_0304_0506_0708, &[0xAA, 0xBB]);
        let r = p.encode();
        assert_eq!(&r[..13], &hex("03 00 08 07 06 05 04 03 02 01 02 AA BB")[..]);
        assert!(r[13..].iter().all(|&b| b == 0));
        assert_eq!(decode(&r), Some(Incoming::Transport(p)));
    }

    #[test]
    fn heartbeat_requests_are_recognised() {
        assert_eq!(decode(&report("03 EC")), Some(Incoming::HeartbeatRequest));
        assert_eq!(decode(&report("03 ED 01 02")), Some(Incoming::HeartbeatRequest));
    }

    #[test]
    fn other_vendor_types_are_ignored_and_other_reports_are_not_ours() {
        assert_eq!(decode(&report("03 A1 01")), Some(Incoming::Ignored(0xA1)));
        assert_eq!(decode(&report("03 F0 00 FE F0 EE")), Some(Incoming::Ignored(0xF0)));
        assert_eq!(decode(&report("05 01")), None);
        assert_eq!(decode(&hex("03 00 01")), None, "short read");
    }

    #[test]
    fn an_oversized_length_byte_is_clamped() {
        let mut r = report("03 00 05 00 00 00 00 00 00 00 FF");
        r[11] = 0x11;
        let Some(Incoming::Transport(p)) = decode(&r) else { panic!("transport") };
        assert_eq!(p.payload.len(), CHUNK_MAX);
        assert_eq!(p.payload[0], 0x11);
    }

    #[test]
    fn the_heartbeat_response_has_length_58_and_three_zero_bytes() {
        let mut status = [0u8; SHORT_STATUS_LEN];
        status[0] = 0x11;
        status[57] = 0x22;
        let r = heartbeat_response(&status);
        assert_eq!(&r[..4], &[0x03, 0xEF, 0x3A, 0x11]);
        assert_eq!(r[60], 0x22);
        assert_eq!(&r[61..], &[0, 0, 0]);
    }
}
