//! Status structures the device sends: the 318-byte device status
//! (GET_DEV_STATUS), the 58-byte short heartbeat, and the system / Wi-Fi
//! info acks whose CRCs the heartbeat carries.
//! Layouts: docs/WAVE_CONTROLLER_PROTOCOL.md §2 and §7 (DWARF of the stock
//! `libins_entry.so`; multi-byte fields little-endian).

use super::snapshot::Snapshot;
use super::vendor::SHORT_STATUS_LEN;

pub const DEV_STATUS_LEN: usize = 0x13E;
pub const WIFI_STATUS_LEN: usize = 0x85;
pub const SYSTEM_BASE_INFO_LEN: usize = 0x85;
pub const WIFI_BASE_INFO_LEN: usize = 0x81;

/// waved's one language (D5) as stock names it.
pub const LANGUAGE_NAME: &str = "English";
pub const LANGUAGE_CODE: &str = "EN";

/// Stock `ENUM_CONNECT_STATUS` "internet reachable".
const WIFI_STATUS_ONLINE: u8 = 3;
const WIFI_ENCRY_SECURED: u8 = 2;

/// CRC-8 as stock computes it (`calc_crc8`): poly 0x07, init 0, MSB first.
pub fn crc8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |mut crc, &b| {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
        crc
    })
}

/// Copy `s` into a fixed field, keeping room for the NUL stock expects.
pub fn put_str(field: &mut [u8], s: &str) {
    field.fill(0);
    let n = s.len().min(field.len().saturating_sub(1));
    field[..n].copy_from_slice(&s.as_bytes()[..n]);
}

/// The four Wi-Fi bytes stock repeats in several structs: rssi, encry, status, switch.
fn wifi_bytes(s: &Snapshot) -> [u8; 4] {
    if s.wifi_connected {
        [s.wifi_rssi_dbm as u8, WIFI_ENCRY_SECURED, WIFI_STATUS_ONLINE, 1]
    } else {
        [0, 0, 0, 1]
    }
}

/// `ssid[64] mac[32] ipv4[32]`, the part of the Wi-Fi structs the CRC covers.
fn wifi_identity(s: &Snapshot) -> [u8; 128] {
    let mut b = [0u8; 128];
    if s.wifi_connected {
        put_str(&mut b[0..64], &s.wifi_ssid);
        put_str(&mut b[96..128], &s.wifi_ip);
    }
    b
}

/// `code[4] country[64] timezone[64]`, the part the system CRC covers.
fn system_identity(s: &Snapshot) -> [u8; 132] {
    let mut b = [0u8; 132];
    put_str(&mut b[0..4], LANGUAGE_CODE);
    put_str(&mut b[68..132], &s.time_zone);
    b
}

/// `ssid[64] mac[32] ipv4[32] rssi encry status switch result` (WIFI_GETSTATUS).
pub fn wifi_status(s: &Snapshot) -> [u8; WIFI_STATUS_LEN] {
    let mut b = [0u8; WIFI_STATUS_LEN];
    b[..128].copy_from_slice(&wifi_identity(s));
    b[128..132].copy_from_slice(&wifi_bytes(s));
    b
}

/// `code[4] country[64] timezone[64] result` (GET_SYSTEM_BASE_INFO).
pub fn system_base_info(s: &Snapshot) -> [u8; SYSTEM_BASE_INFO_LEN] {
    let mut b = [0u8; SYSTEM_BASE_INFO_LEN];
    b[..132].copy_from_slice(&system_identity(s));
    b
}

/// `ssid[64] mac[32] ipv4[32] result` (GET_WIFI_BASE_INFO).
pub fn wifi_base_info(s: &Snapshot) -> [u8; WIFI_BASE_INFO_LEN] {
    let mut b = [0u8; WIFI_BASE_INFO_LEN];
    b[..128].copy_from_slice(&wifi_identity(s));
    b
}

fn spoken_volumes(s: &Snapshot) -> (u8, u8) {
    let speaker = if s.speaker_muted { 0 } else { s.speaker_volume };
    let mic = if s.mic_muted { 0 } else { s.mic_volume };
    (speaker, mic)
}

/// `pc_link_dev_status_ack_t` (GET_DEV_STATUS).
pub fn dev_status(s: &Snapshot) -> [u8; DEV_STATUS_LEN] {
    let mut d = [0u8; DEV_STATUS_LEN];
    d[0x01] = u8::from(s.pin_enabled) << 2;
    let wifi = wifi_identity(s);
    d[0x12..0x52].copy_from_slice(&wifi[0..64]);
    d[0x52..0x72].copy_from_slice(&wifi[64..96]);
    d[0x72..0x92].copy_from_slice(&wifi[96..128]);
    d[0x92..0x96].copy_from_slice(&wifi_bytes(s));
    let (speaker, mic) = spoken_volumes(s);
    let mic_muted = s.mic_muted || mic == 0;
    d[0x96] = (u8::from(s.speaker_muted) << 6) | (u8::from(mic_muted) << 7);
    d[0x97] = speaker;
    d[0x98] = mic;
    d[0x9A] = (s.mic_mode & 0x0F) << 4;
    d[0x9B] = u8::from(s.uac_input) | ((s.meet_status & 1) << 7);
    d[0x9C] = (s.meet_status & 0x0F) >> 1;
    d[0x9D..0xA1].copy_from_slice(&s.meet_secs.to_le_bytes());
    d[0xA2] = 1;
    d[0xA3] = u8::from(s.charging);
    d[0xA4] = s.battery_level;
    d[0xAB..0xAF].copy_from_slice(&s.storage_total_mib.to_le_bytes());
    d[0xAF..0xB3].copy_from_slice(&s.storage_used_mib.to_le_bytes());
    d[0xB3] = u8::from(s.clock_synced);
    d[0xB4] = u8::from(s.clock_24h);
    d[0xB5..0x139].copy_from_slice(&system_identity(s));
    let mut covered = [0u8; 133];
    covered[..132].copy_from_slice(&d[0xB5..0x139]);
    covered[132] = s.battery_level;
    d[0x02] = crc8(&covered);
    d
}

/// `pclink_heartbeat_short_status`.
pub fn short_status(s: &Snapshot) -> [u8; SHORT_STATUS_LEN] {
    let mut h = [0u8; SHORT_STATUS_LEN];
    h[0x06] = s.meet_status;
    h[0x07..0x0B].copy_from_slice(&s.meet_secs.to_le_bytes());
    h[0x0C] = 1;
    h[0x0D] = u8::from(s.charging);
    h[0x0E] = s.battery_level;
    h[0x12..0x16].copy_from_slice(&wifi_bytes(s));
    let (speaker, mic) = spoken_volumes(s);
    h[0x16] = speaker;
    h[0x17] = mic;
    h[0x19] = s.mic_mode;
    h[0x1A] = u8::from(s.mic_muted);
    h[0x1B] = u8::from(s.speaker_muted);
    h[0x1C] = u8::from(s.uac_input);
    h[0x1D] = u8::from(s.clock_synced);
    h[0x1E] = u8::from(s.clock_24h);
    h[0x2C] = u8::from(s.pin_enabled);
    h[0x2F..0x33].copy_from_slice(&s.storage_total_mib.to_le_bytes());
    h[0x33..0x37].copy_from_slice(&s.storage_used_mib.to_le_bytes());
    h[0x37] = crc8(&system_identity(s));
    h[0x38] = crc8(&wifi_identity(s));
    h
}

/// Offsets `is_difference_of_heartbeat_status_info` compares.
const COMPARED: &[usize] = &[
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x0D, 0x0E, 0x0F, 0x10, 0x13, 0x14, 0x15, 0x16, 0x18,
    0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20, 0x2C, 0x37, 0x38,
];

/// Stock pushes an unsolicited heartbeat when one of these bytes changes
/// (`is_difference_of_heartbeat_status_info`); duration, RSSI, mic volume
/// and storage do not count.
pub fn worth_pushing(old: &[u8; SHORT_STATUS_LEN], new: &[u8; SHORT_STATUS_LEN]) -> bool {
    COMPARED.iter().any(|&i| old[i] != new[i])
        || (old[0x2D] & 0x3F) != (new[0x2D] & 0x3F)
        || (old[0x39] & 1) != (new[0x39] & 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pclink::snapshot::meet;

    fn snap() -> Snapshot {
        Snapshot {
            speaker_volume: 40,
            mic_volume: 90,
            mic_mode: 3,
            uac_input: true,
            meet_status: meet::RECORDING,
            meet_secs: 0x0102_0304,
            battery_level: 64,
            charging: true,
            wifi_connected: true,
            wifi_ssid: "Office".into(),
            wifi_ip: "192.168.1.20".into(),
            wifi_rssi_dbm: -55,
            clock_synced: true,
            time_zone: "Europe/Brussels".into(),
            clock_24h: true,
            pin_enabled: true,
            first_run_done: true,
            version: "0.1.0".into(),
            storage_total_mib: 8192,
            storage_used_mib: 2048,
            ..Snapshot::default()
        }
    }

    fn cstr(field: &[u8]) -> &str {
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        std::str::from_utf8(&field[..end]).unwrap()
    }

    #[test]
    fn crc8_is_crc8_smbus() {
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc8(&[]), 0);
    }

    #[test]
    fn put_str_truncates_and_keeps_a_nul() {
        let mut f = [0xAAu8; 4];
        put_str(&mut f, "abcdef");
        assert_eq!(f, [b'a', b'b', b'c', 0]);
    }

    #[test]
    fn wifi_status_layout() {
        let w = wifi_status(&snap());
        assert_eq!(cstr(&w[0..64]), "Office");
        assert_eq!(cstr(&w[64..96]), "");
        assert_eq!(cstr(&w[96..128]), "192.168.1.20");
        assert_eq!(&w[128..133], &[(-55i8) as u8, 2, 3, 1, 0]);
        let mut off = snap();
        off.wifi_connected = false;
        assert_eq!(&wifi_status(&off)[129..131], &[0, 0], "open / not connected");
    }

    #[test]
    fn system_and_wifi_base_info_layouts() {
        let b = system_base_info(&snap());
        assert_eq!(cstr(&b[0..4]), "EN");
        assert_eq!(cstr(&b[4..68]), "");
        assert_eq!(cstr(&b[68..132]), "Europe/Brussels");
        assert_eq!(b[132], 0);
        let w = wifi_base_info(&snap());
        assert_eq!(cstr(&w[0..64]), "Office");
        assert_eq!(cstr(&w[96..128]), "192.168.1.20");
        assert_eq!(w[128], 0);
    }

    #[test]
    fn dev_status_layout_matches_the_stock_struct() {
        let d = dev_status(&snap());
        assert_eq!(d[0x00], 0, "status ok");
        assert_eq!(d[0x01], 0b0000_0100, "PIN set → dev_passwd_status, nothing bound");
        assert_eq!(d[0x02], 0x14, "CRC-8 of code, country, timezone, battery");
        assert_eq!(cstr(&d[0x12..0x52]), "Office");
        assert_eq!(cstr(&d[0x72..0x92]), "192.168.1.20");
        assert_eq!(&d[0x92..0x96], &[(-55i8) as u8, 2, 3, 1]);
        assert_eq!(d[0x96], 0, "nothing muted");
        assert_eq!((d[0x97], d[0x98]), (40, 90));
        assert_eq!(d[0x9A], 3 << 4, "mic mode in the high nibble");
        assert_eq!(d[0x9B], 0b1000_0001, "uac input, meet_status bit 0");
        assert_eq!(d[0x9C], 0, "meet_status bits 1–3 are 0 for 'recording' (1)");
        assert_eq!(&d[0x9D..0xA1], &[4, 3, 2, 1]);
        assert_eq!(d[0xA2], 1, "meet_type");
        assert_eq!((d[0xA3], d[0xA4]), (1, 64), "charging, level");
        assert_eq!(&d[0xAB..0xAF], &8192u32.to_le_bytes());
        assert_eq!(&d[0xAF..0xB3], &2048u32.to_le_bytes());
        assert_eq!((d[0xB3], d[0xB4]), (1, 1), "NTP synced, 24 h");
        assert_eq!(cstr(&d[0xB5..0xB9]), "EN");
        assert_eq!(cstr(&d[0xF9..0x139]), "Europe/Brussels");
        assert!(d[0x139..].iter().all(|&b| b == 0));
    }

    #[test]
    fn dev_status_meet_status_straddles_two_bytes() {
        let mut s = snap();
        s.meet_status = meet::PRE_RECORDING; // 8 = 0b1000
        let d = dev_status(&s);
        assert_eq!(d[0x9B] >> 7, 0);
        assert_eq!(d[0x9C] & 0b111, 0b100);
        s.meet_status = meet::PAUSED; // 2 = 0b0010
        let d = dev_status(&s);
        assert_eq!((d[0x9B] >> 7, d[0x9C] & 0b111), (0, 0b001));
    }

    #[test]
    fn muted_volumes_read_zero_and_set_the_mute_bits() {
        let mut s = snap();
        s.speaker_muted = true;
        s.mic_muted = true;
        let d = dev_status(&s);
        assert_eq!(d[0x96], 0b1100_0000);
        assert_eq!((d[0x97], d[0x98]), (0, 0));
        let mut s = snap();
        s.mic_volume = 0;
        assert_eq!(dev_status(&s)[0x96], 0b1000_0000, "stock flags a zero mic volume as muted");
    }

    #[test]
    fn short_status_layout_matches_the_stock_struct() {
        let h = short_status(&snap());
        assert_eq!(h[0x06], meet::RECORDING);
        assert_eq!(&h[0x07..0x0B], &[4, 3, 2, 1]);
        assert_eq!(h[0x0C], 1, "meet_type");
        assert_eq!((h[0x0D], h[0x0E]), (1, 64));
        assert_eq!(&h[0x12..0x16], &[(-55i8) as u8, 2, 3, 1]);
        assert_eq!((h[0x16], h[0x17]), (40, 90));
        assert_eq!(h[0x19], 3);
        assert_eq!((h[0x1A], h[0x1B], h[0x1C]), (0, 0, 1));
        assert_eq!((h[0x1D], h[0x1E]), (1, 1));
        assert_eq!(h[0x2C], 1, "PIN set");
        assert_eq!(&h[0x2F..0x33], &8192u32.to_le_bytes());
        assert_eq!(&h[0x33..0x37], &2048u32.to_le_bytes());
        assert_eq!(h[0x37], 0x9E, "CRC-8 of the system base info");
        assert_eq!(h[0x38], 0x4A, "CRC-8 of the Wi-Fi base info");
        assert_eq!(h[0x39], 0);
    }

    #[test]
    fn pushes_follow_the_stock_comparison() {
        let base = short_status(&snap());
        let mut s = snap();
        s.meet_secs += 1;
        s.wifi_rssi_dbm = -70;
        s.mic_volume = 10;
        s.storage_used_mib += 5;
        assert!(!worth_pushing(&base, &short_status(&s)), "ignored fields");
        let mut s = snap();
        s.speaker_volume = 41;
        assert!(worth_pushing(&base, &short_status(&s)));
        let mut s = snap();
        s.mic_muted = true;
        assert!(worth_pushing(&base, &short_status(&s)));
        let mut s = snap();
        s.time_zone = "UTC".into();
        assert!(worth_pushing(&base, &short_status(&s)), "system CRC");
    }
}
