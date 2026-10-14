//! Host → module commands (prefix `75 45`) and the acknowledgement the host
//! sends for every frame it receives (prefix `75 46`).
//!
//! Byte values and payloads are stock `bt_ctrl_cmd_send` (libins_entry.so.c
//! 74348-74619) and `bt_ctrl_cmd_send_without_ack` (74900-75033), 1.2.2.6;
//! the same set is in 3.0.0.1. docs/BLUETOOTH_PROTOCOL.md.
//!
//! Left out on purpose: the phone book (`0xAD` / `0xAF`, D-decision "phone
//! number only"), the factory radio test modes (`0xE2`-`0xE5`) and 2.4 GHz
//! dongle pairing (`0xCD` / `0xCE`); their bytes are in the doc.

use crate::frame::{Frame, Prefix};

/// Command bytes on the wire. Requests and the module's replies share one space.
pub mod id {
    pub const OTA_REQUEST: u8 = 0x80;
    pub const SET_NAME: u8 = 0x8B;
    pub const NAME_SET: u8 = 0x8C;
    pub const GET_VERSION: u8 = 0x91;
    pub const GET_BUILD_TIME: u8 = 0x93;
    pub const PHONE_MODE: u8 = 0x97;
    pub const GET_MAC: u8 = 0x9A;
    pub const OTA_FW_INFO: u8 = 0x9D;
    pub const OTA_FW_INFO_DONE: u8 = 0x9E;
    pub const OTA_PACKET: u8 = 0x9F;
    pub const OTA_PACKET_DONE: u8 = 0xA0;
    pub const OTA_CRC: u8 = 0xA1;
    pub const OTA_RESET: u8 = 0xA2;
    pub const DISCONNECT: u8 = 0xA3;
    pub const SWITCH_AUDIO_MODE: u8 = 0xA5;
    pub const AUDIO_MODE_SWITCHED: u8 = 0xA6;
    pub const ENTER_PAIRING: u8 = 0xA7;
    pub const GET_STATUS: u8 = 0xA9;
    pub const STATUS: u8 = 0xAA;
    pub const GET_24G_STATUS: u8 = 0xAB;
    pub const GET_PBAP_STATUS: u8 = 0xAD;
    pub const PBAP_STATUS: u8 = 0xAE;
    pub const GET_PHONE_BOOK: u8 = 0xAF;
    pub const PHONE_BOOK_ENTRY: u8 = 0xB0;
    pub const CALL_NUMBER: u8 = 0xB1;
    pub const WORK_STATUS: u8 = 0xB2;
    pub const HEARTBEAT: u8 = 0xB3;
    pub const LINK_REPORT: u8 = 0xB4;
    pub const DONGLE_INFO: u8 = 0xBD;
    pub const MODULE_INFO: u8 = 0xC4;
    pub const GET_NAME: u8 = 0xC5;
    pub const NAME: u8 = 0xC6;
    pub const GET_PHONE_MAC: u8 = 0xC7;
    pub const PHONE_MAC: u8 = 0xC8;
    pub const CLOSE_BT: u8 = 0xCB;
    pub const OPEN_BT: u8 = 0xCC;
    pub const DONGLE_PAIRING: u8 = 0xCD;
    pub const DONGLE_DISCONNECT: u8 = 0xCE;
    pub const BUTTON: u8 = 0xD0;
    pub const PHONE_EVENT: u8 = 0xD1;
    pub const DONGLE_CONTROL: u8 = 0xD4;
    pub const PHONE_VOLUME: u8 = 0xD5;
    pub const CONNECT_REQUEST: u8 = 0xD6;
    pub const CONNECT_REPLY: u8 = 0xD7;
    pub const AUTO_RECONNECT: u8 = 0xD8;
    pub const DONGLE_OTA_FW_INFO: u8 = 0xD9;
    pub const DONGLE_OTA_FW_INFO_DONE: u8 = 0xDA;
    pub const DONGLE_OTA_PACKET: u8 = 0xDB;
    pub const DONGLE_OTA_PACKET_DONE: u8 = 0xDC;
    pub const DONGLE_OTA_CRC: u8 = 0xDD;
    pub const DONGLE_OTA_CRC_DONE: u8 = 0xDE;
    pub const DONGLE_OTA_RESET: u8 = 0xDF;
    pub const SAVE_FLASH: u8 = 0xE0;
    pub const DEVICE_DISCONNECTED: u8 = 0xE1;
    pub const GET_DONGLE_VERSION: u8 = 0xE6;
    pub const GET_DONGLE_BUILD_TIME: u8 = 0xE7;
    pub const DONGLE_VERSION: u8 = 0xE8;
    pub const DONGLE_BUILD_TIME: u8 = 0xE9;
    pub const SET_PTA_MODE: u8 = 0xEA;
    pub const PTA_MODE_SET: u8 = 0xEB;
    pub const LOG: u8 = 0xF0;
}

/// Bytes of the advertised name stock sends: "Insta360 Wave BT " plus six
/// hex digits (`CTRL_SET_NAME_CMD`, len 0x17).
pub const NAME_LEN: usize = 23;

/// A key press the host reports to the module (`0xD0`, `bt_button_event`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonEvent {
    /// 0: hang up / decline (both call screens' red button).
    EndCall,
    /// 3: answer (incoming-call screen's green button; also music button 0).
    AnswerCall,
    /// 6: a recording started.
    RecordingStarted,
    /// 7: a recording paused.
    RecordingPaused,
    /// 8: a recording finished.
    RecordingStopped,
    Other(u8),
}

impl ButtonEvent {
    pub fn code(self) -> u8 {
        match self {
            ButtonEvent::EndCall => 0,
            ButtonEvent::AnswerCall => 3,
            ButtonEvent::RecordingStarted => 6,
            ButtonEvent::RecordingPaused => 7,
            ButtonEvent::RecordingStopped => 8,
            ButtonEvent::Other(code) => code,
        }
    }
}

/// Stock's battery byte for `0xB3`: `level / 10`, minus one unless already 0.
fn battery_step(percent: u8) -> u8 {
    (percent.min(100) / 10).saturating_sub(1)
}

/// `name` as [`NAME_LEN`] bytes: cut at a character boundary, NUL-padded.
fn name_bytes(name: &str) -> Vec<u8> {
    let mut end = name.len().min(NAME_LEN);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let mut bytes = name.as_bytes()[..end].to_vec();
    bytes.resize(NAME_LEN, 0);
    bytes
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `0x91`: reply `0x91` with the version ("0.0.0.33").
    GetVersion,
    /// `0x93`: reply `0x93` with the build time ("2026-06-12 12:43:40").
    GetBuildTime,
    /// `0xA9`: reply `0xAA`, connected or not.
    GetStatus,
    /// `0xC5`: reply `0xC6` with the advertised name.
    GetName,
    /// `0x9A`: the module's own MAC (reply unhandled by stock; `0xC4` carries it).
    GetMac,
    /// `0xC7`: reply `0xC8` with the phone's MAC.
    GetPhoneMac,
    /// `0xAB`: 2.4 GHz dongle link status.
    Get24gStatus,
    /// `0xB3`, every 5 s in stock: tells the module the battery, charging and
    /// Wi-Fi; the reply `0xB4` carries RSSI and doubles as the liveness check.
    Heartbeat { battery_percent: u8, charging: bool, wifi: bool },
    /// `0x8B`: the advertised name, NUL-padded to [`NAME_LEN`] bytes.
    SetName(String),
    /// `0xCC 00`: Bluetooth on (stock's switch, and at boot when it is on).
    /// Stock's `is_init` only decides whether the host saves the switch; the
    /// byte on the wire is always 0 (bt_ctrl_cmd_send 74445-74473).
    OpenBt,
    /// `0xCB`: Bluetooth off (stock's switch). Stock then counts the phone as
    /// gone and ignores `0xB2` bit 0 and `0xB4` until `OpenBt`.
    CloseBt,
    /// `0xA3 00`: drop the phone (stock `bt_disconnect`).
    Disconnect,
    /// `0xA7 00`: pairing mode (stock `bt_repair`).
    EnterPairing,
    /// `0xD7`: answer a `0xD6` connection request (meaning of the values unverified).
    ConnectReply(u8),
    /// `0xD8`: reconnect to the last phone on power-up.
    AutoReconnect(bool),
    /// `0xD0`: a key press, stamped with `time(NULL)`.
    Button { event: ButtonEvent, unix_time: u32 },
    /// `0xE0 00`: persist the module's settings (reply `0xE0`).
    SaveFlash,
    /// `0x97`: stock sends 1 every heartbeat (since 1.1.6.6).
    PhoneMode(u8),
    /// `0xEA`: Wi-Fi/Bluetooth coexistence, on when Wi-Fi exists.
    PtaMode { wifi: bool },
    /// `0xA5`: audio mode (reply `0xA6`, mode in byte 1). Stock sends 0 only.
    SwitchAudioMode(u8),
    /// `0xE6 00`: reply `0xE8`.
    GetDongleVersion,
    /// `0xE7 00`: reply `0xE9`.
    GetDongleBuildTime,
}

impl Command {
    pub fn frame(&self) -> Frame {
        let (cmd, data): (u8, Vec<u8>) = match self {
            Command::GetVersion => (id::GET_VERSION, vec![]),
            Command::GetBuildTime => (id::GET_BUILD_TIME, vec![]),
            Command::GetStatus => (id::GET_STATUS, vec![]),
            Command::GetName => (id::GET_NAME, vec![]),
            Command::GetMac => (id::GET_MAC, vec![]),
            Command::GetPhoneMac => (id::GET_PHONE_MAC, vec![]),
            Command::Get24gStatus => (id::GET_24G_STATUS, vec![]),
            Command::Heartbeat { battery_percent, charging, wifi } => (
                id::HEARTBEAT,
                vec![battery_step(*battery_percent), u8::from(*charging), u8::from(*wifi)],
            ),
            Command::SetName(name) => (id::SET_NAME, name_bytes(name)),
            Command::OpenBt => (id::OPEN_BT, vec![0]),
            Command::CloseBt => (id::CLOSE_BT, vec![]),
            Command::Disconnect => (id::DISCONNECT, vec![0]),
            Command::EnterPairing => (id::ENTER_PAIRING, vec![0]),
            Command::ConnectReply(reply) => (id::CONNECT_REPLY, vec![*reply]),
            Command::AutoReconnect(on) => (id::AUTO_RECONNECT, vec![u8::from(*on)]),
            Command::Button { event, unix_time } => {
                let mut data = vec![event.code()];
                data.extend_from_slice(&unix_time.to_le_bytes());
                // Stock's reset_level byte; the UI never sets it.
                data.push(0);
                (id::BUTTON, data)
            }
            Command::SaveFlash => (id::SAVE_FLASH, vec![0]),
            Command::PhoneMode(mode) => (id::PHONE_MODE, vec![*mode]),
            Command::PtaMode { wifi } => (id::SET_PTA_MODE, vec![u8::from(*wifi), 0]),
            Command::SwitchAudioMode(mode) => (id::SWITCH_AUDIO_MODE, vec![*mode]),
            Command::GetDongleVersion => (id::GET_DONGLE_VERSION, vec![0]),
            Command::GetDongleBuildTime => (id::GET_DONGLE_BUILD_TIME, vec![0]),
        };
        Frame::request(cmd, &data)
    }

    /// The bytes on the wire.
    pub fn encode(&self) -> Vec<u8> {
        self.frame().encode()
    }
}

/// What the host sends back for a received frame: stock's acknowledgement
/// ([`ack_for`]) for every `75 45` frame but `0xE0`, nothing for a `75 46`
/// frame.
///
/// The module repeats a `75 45` frame every 500 ms until it is acknowledged.
/// Its `75 46` frames answer our own requests (`91 00 <version>`, `A9 00`,
/// `C5 00`, `A7 00`) and come once, acknowledged or not (capture
/// s1-pair-20261013.bin). Stock acknowledges those too (it never looks at the
/// prefix); waved does not, because such an acknowledgement carries our own
/// request's byte back to the module (`75 46 02 A9 00 AB` is GetStatus'
/// `0xA9`), which may read it as the request again (docs/BLUETOOTH_PROTOCOL.md,
/// "Acknowledgements and replies").
pub fn ack_for_frame(frame: &Frame) -> Option<Frame> {
    match frame.prefix {
        Prefix::Ack => None,
        Prefix::Request | Prefix::Legacy => ack_for(frame.cmd),
    }
}

/// The acknowledgement stock sends for a received frame with `cmd`
/// (`bt_msg_recv_thread` 75277: every frame except `0xE0`;
/// `bt_ctrl_cmd_send_without_ack` 74888 for the bytes).
pub fn ack_for(cmd: u8) -> Option<Frame> {
    match cmd {
        id::SAVE_FLASH => None,
        0x8B | 0xA4 | 0xAC | 0xAE | id::MODULE_INFO | id::PHONE_EVENT | id::DONGLE_CONTROL
        | id::PHONE_VOLUME => Some(Frame::ack(cmd, &[])),
        _ => Some(Frame::ack(cmd, &[0])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }

    fn wire(c: Command) -> String {
        hex(&c.encode())
    }

    #[test]
    fn queries() {
        assert_eq!(wire(Command::GetStatus), "75 45 01 A9 AA");
        assert_eq!(wire(Command::GetName), "75 45 01 C5 C6");
        assert_eq!(wire(Command::GetVersion), "75 45 01 91 92");
        assert_eq!(wire(Command::GetBuildTime), "75 45 01 93 94");
        assert_eq!(wire(Command::GetMac), "75 45 01 9A 9B");
        assert_eq!(wire(Command::GetPhoneMac), "75 45 01 C7 C8");
        assert_eq!(wire(Command::Get24gStatus), "75 45 01 AB AC");
        assert_eq!(wire(Command::GetDongleVersion), "75 45 02 E6 00 E8");
        assert_eq!(wire(Command::GetDongleBuildTime), "75 45 02 E7 00 E9");
    }

    #[test]
    fn connection_control() {
        assert_eq!(wire(Command::Disconnect), "75 45 02 A3 00 A5");
        assert_eq!(wire(Command::EnterPairing), "75 45 02 A7 00 A9");
        // Stock sends `CC 00` from the switch and at boot alike: `is_init`
        // only decides whether it saves the switch, then data[0] is set to 0
        // (bt_ctrl_cmd_send 74445-74473).
        assert_eq!(wire(Command::OpenBt), "75 45 02 CC 00 CE");
        assert_eq!(wire(Command::CloseBt), "75 45 01 CB CC");
        assert_eq!(wire(Command::AutoReconnect(true)), "75 45 02 D8 01 DB");
        assert_eq!(wire(Command::AutoReconnect(false)), "75 45 02 D8 00 DA");
        assert_eq!(wire(Command::ConnectReply(0)), "75 45 02 D7 00 D9");
        assert_eq!(wire(Command::ConnectReply(1)), "75 45 02 D7 01 DA");
    }

    #[test]
    fn settings() {
        assert_eq!(wire(Command::SaveFlash), "75 45 02 E0 00 E2");
        assert_eq!(wire(Command::PhoneMode(1)), "75 45 02 97 01 9A");
        assert_eq!(wire(Command::PtaMode { wifi: true }), "75 45 03 EA 01 00 EE");
        assert_eq!(wire(Command::PtaMode { wifi: false }), "75 45 03 EA 00 00 ED");
        assert_eq!(wire(Command::SwitchAudioMode(0)), "75 45 02 A5 00 A7");
    }

    #[test]
    fn heartbeat_maps_battery_like_stock() {
        // Stock: level / 10, minus one unless that is already 0.
        let hb = |battery_percent, charging, wifi| {
            wire(Command::Heartbeat { battery_percent, charging, wifi })
        };
        assert_eq!(hb(85, true, true), "75 45 04 B3 07 01 01 C0");
        assert_eq!(hb(5, false, false), "75 45 04 B3 00 00 00 B7");
        assert_eq!(hb(100, false, true), "75 45 04 B3 09 00 01 C1");
        assert_eq!(hb(10, false, false), "75 45 04 B3 00 00 00 B7");
        assert_eq!(hb(20, false, false), "75 45 04 B3 01 00 00 B8");
    }

    #[test]
    fn set_name_is_23_bytes() {
        assert_eq!(
            wire(Command::SetName("Insta360 Wave BT 0A1B2C".into())),
            "75 45 18 8B 49 6E 73 74 61 33 36 30 20 57 61 76 65 20 42 54 20 30 41 31 42 32 43 1D"
        );
    }

    #[test]
    fn set_name_pads_a_short_name_and_cuts_a_long_one() {
        let short = Command::SetName("Wave".into()).frame();
        assert_eq!(short.data.len(), NAME_LEN);
        assert_eq!(&short.data[..4], b"Wave");
        assert!(short.data[4..].iter().all(|&b| b == 0));

        let long = Command::SetName("Insta360 Wave BT 0A1B2C and more".into()).frame();
        assert_eq!(long.data, b"Insta360 Wave BT 0A1B2C".to_vec());

        // Never split a UTF-8 character.
        let utf8 = Command::SetName("Wave de Thérèse – bureau".into()).frame();
        assert_eq!(utf8.data.len(), NAME_LEN);
        let text: Vec<u8> = utf8.data.iter().copied().take_while(|&b| b != 0).collect();
        assert!(std::str::from_utf8(&text).is_ok());
    }

    #[test]
    fn buttons_carry_the_time() {
        let t = 0x6720_0000;
        assert_eq!(
            wire(Command::Button { event: ButtonEvent::AnswerCall, unix_time: t }),
            "75 45 07 D0 03 00 00 20 67 00 61"
        );
        assert_eq!(
            wire(Command::Button { event: ButtonEvent::EndCall, unix_time: t }),
            "75 45 07 D0 00 00 00 20 67 00 5E"
        );
    }

    #[test]
    fn button_codes() {
        assert_eq!(ButtonEvent::EndCall.code(), 0);
        assert_eq!(ButtonEvent::AnswerCall.code(), 3);
        assert_eq!(ButtonEvent::RecordingStarted.code(), 6);
        assert_eq!(ButtonEvent::RecordingPaused.code(), 7);
        assert_eq!(ButtonEvent::RecordingStopped.code(), 8);
        assert_eq!(ButtonEvent::Other(4).code(), 4);
    }

    #[test]
    fn acks_follow_the_stock_table() {
        let ack = |cmd| ack_for(cmd).map(|f| hex(&f.encode()));
        // One zero byte for most...
        assert_eq!(ack(0xB2).as_deref(), Some("75 46 02 B2 00 B4"));
        assert_eq!(ack(0xAA).as_deref(), Some("75 46 02 AA 00 AC"));
        assert_eq!(ack(0x42).as_deref(), Some("75 46 02 42 00 44"));
        // ...none for these...
        for cmd in [0x8B, 0xA4, 0xAC, 0xAE, 0xC4, 0xD1, 0xD4, 0xD5] {
            let f = ack_for(cmd).unwrap();
            assert!(f.data.is_empty(), "{cmd:#04x}");
        }
        assert_eq!(ack(0xD4).as_deref(), Some("75 46 01 D4 D5"));
        // The phone book is dropped unread but still acknowledged, or the
        // module repeats each contact every 500 ms.
        assert_eq!(ack(0xB0).as_deref(), Some("75 46 02 B0 00 B2"));
        // ...and 0xE0 (flash saved) is never acknowledged.
        assert_eq!(ack(0xE0), None);
    }

    /// Parse one wire frame (fixtures as captured or as stock builds them).
    fn wire_frame(bytes: &[u8]) -> Frame {
        let mut p = crate::frame::Parser::default();
        let frames: Vec<Frame> = bytes.iter().filter_map(|&b| p.feed(b)).collect();
        assert_eq!(frames.len(), 1, "one valid frame");
        frames.into_iter().next().unwrap()
    }

    #[test]
    fn every_module_frame_is_acknowledged_but_not_its_replies() {
        let ack = |bytes: &[u8]| ack_for_frame(&wire_frame(bytes)).map(|f| hex(&f.encode()));
        // `75 45` frames: the module repeats them every 500 ms until
        // acknowledged (device 2026-10-13). Stock's bytes, replies included.
        assert_eq!(ack(&[0x75, 0x45, 0x03, 0xB2, 0x00, 0x00, 0xB5]).as_deref(), Some("75 46 02 B2 00 B4"));
        assert_eq!(ack(&[0x75, 0x45, 0x02, 0xAA, 0x01, 0xAD]).as_deref(), Some("75 46 02 AA 00 AC"));
        let mut name = vec![0x75, 0x45, 0x19, 0xC6, 0x00];
        name.extend_from_slice(b"Insta360 Wave BT 0A1B2C");
        name.push(crate::frame::checksum(&name[2..]));
        assert_eq!(ack(&name).as_deref(), Some("75 46 02 C6 00 C8"));
        assert_eq!(ack(&[0x75, 0x45, 0x02, 0xD1, 0x00, 0xD3]).as_deref(), Some("75 46 01 D1 D2"));
        // ...but never 0xE0.
        assert_eq!(ack(&[0x75, 0x45, 0x01, 0xE0, 0xE1]), None);
        // `75 46` frames are the module's answers to our requests, sent once
        // (capture s1-pair-20261013.bin: 91, 93, A9, C5, A7 once each with
        // nothing acknowledged). Acknowledging one would hand the module our
        // own request byte back (`75 46 02 A9 00 AB` carries GetStatus).
        assert_eq!(ack(&[0x75, 0x46, 0x02, 0xA9, 0x00, 0xAB]), None);
        assert_eq!(ack(&[0x75, 0x46, 0x02, 0xC5, 0x00, 0xC7]), None);
        assert_eq!(ack(&[0x75, 0x46, 0x02, 0xA7, 0x00, 0xA9]), None);
        let version = Frame::ack(0x91, b"\x000.0.0.28").encode();
        assert_eq!(ack(&version), None);
    }
}
