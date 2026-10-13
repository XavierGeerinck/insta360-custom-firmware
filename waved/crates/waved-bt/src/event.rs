//! Module → host frames, decoded. Layouts are stock `bt_msg_recv_thread`
//! (libins_entry.so.c 75280-77403, 1.2.2.6); docs/BLUETOOTH_PROTOCOL.md.
//!
//! Stock does not look at the prefix of a received frame, so neither does
//! this: a frame that echoes one of our requests decodes like any other and
//! must be harmless (empty strings, `Other`).

use crate::command::id;
use crate::frame::Frame;

pub type Mac = [u8; 6];

/// `aa:bb:cc:dd:ee:ff`, bytes in the order received (stock's `%02x:` format).
pub fn format_mac(mac: &Mac) -> String {
    mac.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

/// `0xB2` byte 0 (stock `audio_listen_bt_work_status_event`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkStatus(pub u8);

impl WorkStatus {
    /// Bit 0: a phone is connected.
    pub fn phone_connected(self) -> bool {
        self.0 & 0x01 != 0
    }
    /// Bit 1: phone audio plays (speaker path, e.g. A2DP).
    pub fn phone_speaker(self) -> bool {
        self.0 & 0x02 != 0
    }
    /// Bit 2: phone audio both ways (microphone too: a call).
    pub fn phone_mic(self) -> bool {
        self.0 & 0x04 != 0
    }
    /// Bit 3: pairing mode (0x08 right after `EnterPairing`, seen on the
    /// device 2026-10-13; not named in stock's code).
    pub fn pairing(self) -> bool {
        self.0 & 0x08 != 0
    }
    /// Bit 4: the 2.4 GHz dongle is connected.
    pub fn dongle_connected(self) -> bool {
        self.0 & 0x10 != 0
    }
    /// Bit 5: dongle audio plays.
    pub fn dongle_speaker(self) -> bool {
        self.0 & 0x20 != 0
    }
    /// Bit 6: dongle audio both ways.
    pub fn dongle_mic(self) -> bool {
        self.0 & 0x40 != 0
    }
}

/// A phone as the module names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phone {
    pub name: String,
    pub mac: Option<Mac>,
}

/// One side of an `0xB4` link report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link<T> {
    /// State byte 0: connected, with the signal values.
    Connected(T),
    /// State byte 0xFE: nothing connected.
    Absent,
    /// Any other state byte (stock leaves its state alone).
    Unknown(u8),
}

/// `0xD1` byte 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhoneEvent {
    /// 0: a call is up (answered here or on the phone).
    CallActive,
    /// 1: the call ended.
    CallEnded,
    /// 2-5: recording controls (stock: start, restart, pause, stop).
    RecordStart,
    RecordRestart,
    RecordPause,
    RecordStop,
    Other(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeKind {
    Music,
    Call,
    Other(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtaReply {
    /// `0x80`: update accepted.
    Ready,
    /// `0x9E` / `0xDA`: size taken.
    FwInfo,
    /// `0xA0` / `0xDC`.
    Packet { ok: bool },
    /// `0xA1` / `0xDE`.
    Crc { ok: bool },
    /// `0xA2`.
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// `0xAA`, reply to `GetStatus`.
    Status { connected: bool },
    /// `0xB2`, unsolicited. `phone` when connected and the frame names it.
    WorkStatus { status: WorkStatus, phone: Option<Phone> },
    /// `0xB3`: proof of life; may name the connected phone.
    Heartbeat { phone: Option<Phone> },
    /// `0xB4`, reply to `Heartbeat`: RSSI of the dongle (dBm, 2.4 GHz link)
    /// and of the phone (dBm).
    LinkReport { dongle: Link<(i8, i8)>, phone: Link<i8>, counters: Option<(u32, u32)> },
    /// `0xB1` with a number: a call is ringing (or dialling).
    CallNumber(String),
    /// `0xB1` empty: the number is gone.
    CallCleared,
    /// `0xD1`.
    Phone(PhoneEvent),
    /// `0xD5`: the phone changed its volume; `level` of `max`.
    PhoneVolume { kind: VolumeKind, level: u8, max: u8 },
    /// `0xD6`: a phone asks to connect.
    ConnectRequest { mac: Mac, name: String },
    /// `0xE1`: the phone with this MAC dropped.
    DeviceDisconnected { mac: Mac },
    /// `0xC4`: the module's own MAC (also UUID, serial and a frequency byte,
    /// deliberately not decoded).
    ModuleInfo { mac: Mac },
    /// `0xC6`, reply to `GetName`.
    Name(String),
    /// `0xC8`, reply to `GetPhoneMac`.
    PhoneMac(Mac),
    /// `0x91`, reply to `GetVersion`.
    Version(String),
    /// `0x93`, reply to `GetBuildTime`.
    BuildTime(String),
    /// `0xA6`, reply to `SwitchAudioMode`.
    AudioModeSwitched(u8),
    /// `0xD4`: the dongle's keys.
    DongleControl { speaker_mute: bool, volume: u8, mic_mute: bool },
    /// `0xE8` / `0xE9`: `None` when no dongle is connected.
    DongleVersion(Option<String>),
    DongleBuildTime(Option<String>),
    /// `0xE0`: settings saved.
    FlashSaved,
    /// `0x8C`, `0x97`, `0xD0`, `0xEB`: the module took `SetName`, `PhoneMode`,
    /// `Button` or `PtaMode` (stock waits for these).
    Done(u8),
    Ota(OtaReply),
    /// `0xF0`: the module's own log line.
    Log(String),
    /// Echoes of our requests and anything not decoded.
    Other { cmd: u8, data: Vec<u8> },
}

/// Decode a received frame.
pub fn decode(frame: &Frame) -> Event {
    let d = frame.data.as_slice();
    let decoded = match frame.cmd {
        id::STATUS => d.first().map(|&b| Event::Status { connected: b != 0 }),
        id::WORK_STATUS => d.first().map(|&b| {
            let status = WorkStatus(b);
            let phone = if status.phone_connected() { phone_at(d, 2) } else { None };
            Event::WorkStatus { status, phone }
        }),
        id::HEARTBEAT => Some(Event::Heartbeat { phone: phone_at(d, 1) }),
        id::LINK_REPORT => link_report(d),
        id::CALL_NUMBER => {
            let number = text(d);
            Some(if number.is_empty() { Event::CallCleared } else { Event::CallNumber(number) })
        }
        id::PHONE_EVENT => d.first().map(|&b| Event::Phone(phone_event(b))),
        id::PHONE_VOLUME => match d {
            [kind, scale, level, ..] => Some(Event::PhoneVolume {
                kind: match kind {
                    0 => VolumeKind::Music,
                    1 => VolumeKind::Call,
                    k => VolumeKind::Other(*k),
                },
                level: *level,
                max: match scale {
                    0 => 15,
                    1 => 16,
                    _ => 0,
                },
            }),
            _ => None,
        },
        id::CONNECT_REQUEST => connect_request(d),
        id::DEVICE_DISCONNECTED => mac_at(d, 0).map(|mac| Event::DeviceDisconnected { mac }),
        id::MODULE_INFO if d.len() > 9 => mac_at(d, 0).map(|mac| Event::ModuleInfo { mac }),
        id::NAME if !d.is_empty() => Some(Event::Name(text(&d[1..d.len().min(1 + crate::command::NAME_LEN)]))),
        id::PHONE_MAC => mac_at(d, 1).map(Event::PhoneMac),
        id::GET_VERSION => Some(Event::Version(text(d.get(1..).unwrap_or(&[])))),
        id::GET_BUILD_TIME => Some(Event::BuildTime(text(d.get(1..).unwrap_or(&[])))),
        id::AUDIO_MODE_SWITCHED => d.get(1).map(|&m| Event::AudioModeSwitched(m)),
        id::DONGLE_CONTROL => match d {
            [_, _, spk, vol, mic, ..] => Some(Event::DongleControl {
                speaker_mute: *spk == 1,
                volume: *vol,
                mic_mute: *mic != 0,
            }),
            _ => None,
        },
        id::DONGLE_VERSION => Some(Event::DongleVersion(dongle_text(d))),
        id::DONGLE_BUILD_TIME => Some(Event::DongleBuildTime(dongle_text(d))),
        id::SAVE_FLASH => Some(Event::FlashSaved),
        id::NAME_SET | id::PHONE_MODE | id::BUTTON | id::PTA_MODE_SET => Some(Event::Done(frame.cmd)),
        id::OTA_REQUEST => Some(Event::Ota(OtaReply::Ready)),
        id::OTA_FW_INFO_DONE | id::DONGLE_OTA_FW_INFO_DONE => Some(Event::Ota(OtaReply::FwInfo)),
        id::OTA_PACKET_DONE => Some(Event::Ota(OtaReply::Packet { ok: d.first() != Some(&3) })),
        id::DONGLE_OTA_PACKET_DONE => Some(Event::Ota(OtaReply::Packet { ok: d.first().is_none_or(|&b| b == 0) })),
        id::OTA_CRC => Some(Event::Ota(OtaReply::Crc { ok: d.first() == Some(&1) })),
        id::DONGLE_OTA_CRC_DONE => Some(Event::Ota(OtaReply::Crc { ok: d.first().is_none_or(|&b| b == 0) })),
        id::OTA_RESET => Some(Event::Ota(OtaReply::Reset)),
        id::LOG => match d.split_first() {
            Some((1, rest)) => Some(Event::Log(text(rest))),
            Some((2, rest)) => Some(Event::Log(
                rest.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "),
            )),
            _ => None,
        },
        _ => None,
    };
    decoded.unwrap_or_else(|| Event::Other { cmd: frame.cmd, data: frame.data.clone() })
}

/// A C string: up to the first NUL, lossy UTF-8.
fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn mac_at(d: &[u8], at: usize) -> Option<Mac> {
    d.get(at..at + 6).map(|m| m.try_into().unwrap())
}

/// `len name[len] ? ? mac[6]` starting at `len_at` (stock `0xB2` / `0xB3`).
/// `None` when the length is 0, the name is cut off or is not text (an echo
/// of one of our own frames).
fn phone_at(d: &[u8], len_at: usize) -> Option<Phone> {
    let n = usize::from(*d.get(len_at)?);
    let start = len_at + 1;
    let raw = d.get(start..start + n).filter(|_| n > 0)?;
    let name = text(raw);
    if name.is_empty() || name.chars().any(char::is_control) {
        return None;
    }
    Some(Phone { name, mac: mac_at(d, start + n + 2) })
}

fn link_report(d: &[u8]) -> Option<Event> {
    if d.len() <= 5 {
        return None;
    }
    let dongle = match d[0] {
        0 => Link::Connected((d[1] as i8, d[2] as i8)),
        0xFE => Link::Absent,
        b => Link::Unknown(b),
    };
    let phone = match d[3] {
        0 => Link::Connected(d[4] as i8),
        0xFE => Link::Absent,
        b => Link::Unknown(b),
    };
    let counters = (d.len() >= 14).then(|| {
        (u32::from_le_bytes(d[6..10].try_into().unwrap()), u32::from_le_bytes(d[10..14].try_into().unwrap()))
    });
    Some(Event::LinkReport { dongle, phone, counters })
}

fn phone_event(b: u8) -> PhoneEvent {
    match b {
        0 => PhoneEvent::CallActive,
        1 => PhoneEvent::CallEnded,
        2 => PhoneEvent::RecordStart,
        3 => PhoneEvent::RecordRestart,
        4 => PhoneEvent::RecordPause,
        5 => PhoneEvent::RecordStop,
        other => PhoneEvent::Other(other),
    }
}

/// `mac_len mac[6] name_len name` (stock reads the MAC at 1..7 whatever `mac_len` says).
fn connect_request(d: &[u8]) -> Option<Event> {
    let mac = mac_at(d, 1)?;
    let at = usize::from(*d.first()?) + 1;
    let n = usize::from(*d.get(at)?);
    let name = text(d.get(at + 1..at + 1 + n)?);
    Some(Event::ConnectRequest { mac, name })
}

/// `0xFE` = no dongle; otherwise text after the first byte.
fn dongle_text(d: &[u8]) -> Option<String> {
    match d.split_first() {
        Some((0xFE, _)) | None => None,
        Some((_, rest)) => Some(text(rest)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Parser;

    const MAC: Mac = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];

    /// Decode one frame given as wire bytes (fixtures: docs, "Example frames").
    fn wire(bytes: &[u8]) -> Event {
        let mut p = Parser::default();
        let frames: Vec<Frame> = bytes.iter().filter_map(|&b| p.feed(b)).collect();
        assert_eq!(frames.len(), 1, "fixture must be one valid frame");
        decode(&frames[0])
    }

    fn ev(cmd: u8, data: &[u8]) -> Event {
        decode(&Frame::request(cmd, data))
    }

    #[test]
    fn mac_format() {
        assert_eq!(format_mac(&MAC), "11:22:33:44:55:66");
        assert_eq!(format_mac(&[0xAB, 0, 1, 2, 3, 0xFF]), "ab:00:01:02:03:ff");
    }

    #[test]
    fn status() {
        assert_eq!(wire(&[0x75, 0x45, 0x02, 0xAA, 0x01, 0xAD]), Event::Status { connected: true });
        assert_eq!(wire(&[0x75, 0x45, 0x02, 0xAA, 0x00, 0xAC]), Event::Status { connected: false });
        // An empty 0xAA is our own request echoed.
        assert!(matches!(ev(0xAA, &[]), Event::Other { cmd: 0xAA, .. }));
    }

    #[test]
    fn work_status_bits() {
        let s = WorkStatus(0x07);
        assert!(s.phone_connected() && s.phone_speaker() && s.phone_mic());
        assert!(!s.dongle_connected());
        let d = WorkStatus(0x30);
        assert!(!d.phone_connected() && d.dongle_connected() && d.dongle_speaker() && !d.dongle_mic());
        assert!(WorkStatus(0x40).dongle_mic());
    }

    #[test]
    fn work_status_names_the_phone() {
        let e = wire(&[
            0x75, 0x45, 0x11, 0xB2, 0x07, 0x00, 0x05, 0x50, 0x69, 0x78, 0x65, 0x6C, 0x00, 0x06,
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x3C,
        ]);
        assert_eq!(
            e,
            Event::WorkStatus {
                status: WorkStatus(0x07),
                phone: Some(Phone { name: "Pixel".into(), mac: Some(MAC) }),
            }
        );
    }

    #[test]
    fn work_status_without_a_phone() {
        assert_eq!(ev(0xB2, &[0x10]), Event::WorkStatus { status: WorkStatus(0x10), phone: None });
        // Not connected: stock ignores any name in the frame.
        assert_eq!(
            ev(0xB2, &[0x00, 0x00, 0x02, b'h', b'i']),
            Event::WorkStatus { status: WorkStatus(0x00), phone: None }
        );
        // Connected, name present, MAC cut off.
        assert_eq!(
            ev(0xB2, &[0x01, 0x00, 0x02, b'h', b'i']),
            Event::WorkStatus {
                status: WorkStatus(0x01),
                phone: Some(Phone { name: "hi".into(), mac: None }),
            }
        );
    }

    #[test]
    fn heartbeat_may_name_the_phone() {
        let mut data = vec![0x00, 0x05];
        data.extend_from_slice(b"Pixel");
        data.extend_from_slice(&[0x00, 0x06]);
        data.extend_from_slice(&MAC);
        assert_eq!(
            ev(0xB3, &data),
            Event::Heartbeat { phone: Some(Phone { name: "Pixel".into(), mac: Some(MAC) }) }
        );
        // An echo of our own heartbeat (battery, charging, wifi) names nobody.
        assert_eq!(ev(0xB3, &[0x07, 0x01, 0x01]), Event::Heartbeat { phone: None });
        assert_eq!(ev(0xB3, &[]), Event::Heartbeat { phone: None });
    }

    #[test]
    fn link_report() {
        // Dongle connected (-60, -55), phone connected (-48), counters 1000 / 2000.
        let mut data = vec![0x00, 0xC4, 0xC9, 0x00, 0xD0, 0x00];
        data.extend_from_slice(&1000u32.to_le_bytes());
        data.extend_from_slice(&2000u32.to_le_bytes());
        assert_eq!(
            ev(0xB4, &data),
            Event::LinkReport {
                dongle: Link::Connected((-60, -55)),
                phone: Link::Connected(-48),
                counters: Some((1000, 2000)),
            }
        );
        assert_eq!(
            ev(0xB4, &[0xFE, 0, 0, 0xFE, 0, 0]),
            Event::LinkReport { dongle: Link::Absent, phone: Link::Absent, counters: None }
        );
        assert_eq!(
            ev(0xB4, &[0x07, 0, 0, 0x01, 0, 0]),
            Event::LinkReport { dongle: Link::Unknown(7), phone: Link::Unknown(1), counters: None }
        );
        // Stock needs more than five bytes.
        assert!(matches!(ev(0xB4, &[0, 0, 0, 0, 0]), Event::Other { .. }));
    }

    #[test]
    fn calls() {
        assert_eq!(
            wire(&[0x75, 0x45, 0x0C, 0xB1, 0x2B, 0x33, 0x32, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0xF1]),
            Event::CallNumber("+3212345678".into())
        );
        assert_eq!(wire(&[0x75, 0x45, 0x01, 0xB1, 0xB2]), Event::CallCleared);
        assert_eq!(ev(0xB1, &[0x00, 0x00]), Event::CallCleared);
        assert_eq!(ev(0xB1, b"0471\0\0"), Event::CallNumber("0471".into()));
        assert_eq!(wire(&[0x75, 0x45, 0x02, 0xD1, 0x00, 0xD3]), Event::Phone(PhoneEvent::CallActive));
        assert_eq!(wire(&[0x75, 0x45, 0x02, 0xD1, 0x01, 0xD4]), Event::Phone(PhoneEvent::CallEnded));
        assert_eq!(ev(0xD1, &[2]), Event::Phone(PhoneEvent::RecordStart));
        assert_eq!(ev(0xD1, &[3]), Event::Phone(PhoneEvent::RecordRestart));
        assert_eq!(ev(0xD1, &[4]), Event::Phone(PhoneEvent::RecordPause));
        assert_eq!(ev(0xD1, &[5]), Event::Phone(PhoneEvent::RecordStop));
        assert_eq!(ev(0xD1, &[9]), Event::Phone(PhoneEvent::Other(9)));
    }

    #[test]
    fn phone_volume() {
        assert_eq!(
            ev(0xD5, &[0, 0, 7]),
            Event::PhoneVolume { kind: VolumeKind::Music, level: 7, max: 15 }
        );
        assert_eq!(ev(0xD5, &[1, 1, 16]), Event::PhoneVolume { kind: VolumeKind::Call, level: 16, max: 16 });
        assert!(matches!(ev(0xD5, &[1, 1]), Event::Other { .. }));
    }

    #[test]
    fn connect_request() {
        assert_eq!(
            wire(&[
                0x75, 0x45, 0x0E, 0xD6, 0x06, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x05, 0x50, 0x69,
                0x78, 0x65, 0x6C, 0x56,
            ]),
            Event::ConnectRequest { mac: MAC, name: "Pixel".into() }
        );
        assert!(matches!(ev(0xD6, &[0x06, 1, 2]), Event::Other { .. }));
    }

    #[test]
    fn identities() {
        let mut info = MAC.to_vec();
        info.extend_from_slice(&[0u8; 49]);
        assert_eq!(ev(0xC4, &info), Event::ModuleInfo { mac: MAC });
        assert!(matches!(ev(0xC4, &MAC), Event::Other { .. }), "stock needs more than 9 bytes");

        let mut name = vec![0x00];
        name.extend_from_slice(b"Insta360 Wave BT 0A1B2C");
        assert_eq!(ev(0xC6, &name), Event::Name("Insta360 Wave BT 0A1B2C".into()));
        assert_eq!(ev(0xC6, b"\0Wave\0\0\0"), Event::Name("Wave".into()));

        let mut pm = vec![0x06];
        pm.extend_from_slice(&MAC);
        assert_eq!(ev(0xC8, &pm), Event::PhoneMac(MAC));
        assert_eq!(ev(0xE1, &MAC), Event::DeviceDisconnected { mac: MAC });
    }

    #[test]
    fn versions() {
        assert_eq!(ev(0x91, b"\x000.0.0.33"), Event::Version("0.0.0.33".into()));
        assert_eq!(ev(0x93, b"\x002026-06-12 12:43:40\0"), Event::BuildTime("2026-06-12 12:43:40".into()));
        // Our own request echoed: empty.
        assert_eq!(ev(0x91, &[]), Event::Version(String::new()));
        assert_eq!(ev(0xE8, b"\x000.0.0.33"), Event::DongleVersion(Some("0.0.0.33".into())));
        assert_eq!(ev(0xE8, &[0xFE]), Event::DongleVersion(None));
        assert_eq!(ev(0xE9, &[0xFE]), Event::DongleBuildTime(None));
    }

    #[test]
    fn small_replies() {
        assert_eq!(ev(0xE0, &[]), Event::FlashSaved);
        for cmd in [0x8C, 0x97, 0xD0, 0xEB] {
            assert_eq!(ev(cmd, &[0]), Event::Done(cmd));
        }
        assert_eq!(ev(0xA6, &[0, 1]), Event::AudioModeSwitched(1));
        assert_eq!(
            ev(0xD4, &[0, 0, 1, 55, 1]),
            Event::DongleControl { speaker_mute: true, volume: 55, mic_mute: true }
        );
        assert_eq!(ev(0xF0, b"\x01hello\0"), Event::Log("hello".into()));
        assert_eq!(ev(0xF0, &[0x02, 0xAB, 0x01]), Event::Log("AB 01".into()));
    }

    #[test]
    fn ota_replies() {
        assert_eq!(ev(0x80, &[0]), Event::Ota(OtaReply::Ready));
        assert_eq!(ev(0x9E, &[0]), Event::Ota(OtaReply::FwInfo));
        assert_eq!(ev(0xDA, &[0]), Event::Ota(OtaReply::FwInfo));
        assert_eq!(ev(0xA0, &[0]), Event::Ota(OtaReply::Packet { ok: true }));
        assert_eq!(ev(0xA0, &[3]), Event::Ota(OtaReply::Packet { ok: false }));
        assert_eq!(ev(0xDC, &[0]), Event::Ota(OtaReply::Packet { ok: true }));
        assert_eq!(ev(0xDC, &[1]), Event::Ota(OtaReply::Packet { ok: false }));
        assert_eq!(ev(0xA1, &[1]), Event::Ota(OtaReply::Crc { ok: true }));
        assert_eq!(ev(0xA1, &[0]), Event::Ota(OtaReply::Crc { ok: false }));
        assert_eq!(ev(0xDE, &[0]), Event::Ota(OtaReply::Crc { ok: true }));
        assert_eq!(ev(0xA2, &[0]), Event::Ota(OtaReply::Reset));
    }

    #[test]
    fn echoes_of_requests_are_other() {
        for cmd in [0xA9, 0xC5, 0xCC, 0xD8, 0xA3, 0xA7] {
            assert!(matches!(ev(cmd, &[0]), Event::Other { .. }), "{cmd:#04x}");
        }
    }
}
