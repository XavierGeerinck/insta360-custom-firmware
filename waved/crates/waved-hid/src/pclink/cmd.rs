//! Command ids (`enum pc_link_cmd_e`) and the size of each command's ack
//! (docs/WAVE_CONTROLLER_PROTOCOL.md §6).

pub const GET_DEV_INFO: u16 = 0x00;
pub const AUTH_UNBIND_DEV: u16 = 0x01;
pub const UNBIND_OFFLINE: u16 = 0x02;
pub const GET_BIND_START: u16 = 0x10;
pub const VERIFY_DEV_PASSWD: u16 = 0x11;
pub const BIND_DEV: u16 = 0x12;
pub const AUTH_DEV: u16 = 0x13;
pub const UNAUTH_DEV: u16 = 0x14;
pub const UNBIND_DEV: u16 = 0x15;
pub const GET_BIND_INFO: u16 = 0x16;
pub const WIFI_SCAN: u16 = 0x17;
pub const WIFI_LIST: u16 = 0x18;
pub const WIFI_CONNECT: u16 = 0x19;
pub const WIFI_DISCONNECT: u16 = 0x1A;
pub const WIFI_GETSTATUS: u16 = 0x1B;
pub const WIFI_FORGETSSID: u16 = 0x1C;
pub const WIFI_SWITCH: u16 = 0x1D;
pub const WIFI_AUTO_CONNECT_SWITCH: u16 = 0x1E;
pub const MEETING_OP: u16 = 0x1F;
pub const MEETING_GETSTATUS: u16 = 0x20;
pub const MEETING_MARK: u16 = 0x21;
pub const MEETING_MODE: u16 = 0x22;
pub const MEETING_GET_INFO: u16 = 0x23;
pub const MEETING_GET_CUR_ALL_INFO: u16 = 0x24;
pub const MEETING_SCREENSHOT: u16 = 0x25;
pub const SET_AUDIO_SPEAKER_VOLUME: u16 = 0x26;
pub const SET_AUDIO_SPEAKER_MODE: u16 = 0x27;
pub const SET_AUDIO_SPEAKER_MUTE: u16 = 0x28;
pub const SET_AUDIO_MIC_VOLUME: u16 = 0x29;
pub const SET_AUDIO_MIC_MODE: u16 = 0x2A;
pub const SET_AUDIO_MIC_MUTE: u16 = 0x2B;
pub const GET_SYS_LANGUAGE_LIST: u16 = 0x2D;
pub const GET_SYS_LANGUAGE_COUNTRY_LIST: u16 = 0x2E;
pub const GET_SYS_COUNTRY_TIMEZONE_LIST: u16 = 0x2F;
pub const GET_SYS_COUNTRY: u16 = 0x30;
pub const GET_SYS_LANGUAGE: u16 = 0x31;
pub const GET_SYS_CITY: u16 = 0x32;
pub const GET_SYS_ALL_SETTING_LIST: u16 = 0x33;
pub const GET_SYS_DEFAULT_SETTING: u16 = 0x34;
pub const SET_SYS_ENABLE_UDISK_MODE: u16 = 0x35;
pub const SET_SYS_TIME_SHOW_MODE: u16 = 0x36;
pub const SET_SYS_TIME_AUTO_SYNC: u16 = 0x37;
pub const SET_SYS_ALL_DATE_TIME_SETTING: u16 = 0x38;
pub const SET_SYS_LANGUAGE: u16 = 0x3A;
pub const SET_SYS_COUNTRY: u16 = 0x3B;
pub const SET_SYS_TIME: u16 = 0x3C;
pub const SET_SYS_TIMEZONE: u16 = 0x3D;
pub const SET_SYS_RECOVERY: u16 = 0x3E;
pub const SET_SYS_REBOOT: u16 = 0x3F;
pub const GET_DEV_STATUS: u16 = 0x40;
pub const OTA_ENABLE_UDISK: u16 = 0x43;
pub const OTA_OP: u16 = 0x44;
pub const OTA_GETSTATUS: u16 = 0x45;
pub const OTA_FINISH_SURE: u16 = 0x46;
pub const FILE_TRANS: u16 = 0x47;
pub const MEETING_PC_STATUS: u16 = 0x48;
pub const PUC_ZOOM_CONTROL: u16 = 0x49;
pub const PUC_POS_RESET: u16 = 0x4A;
pub const PUC_POS_CONTROL: u16 = 0x4B;
pub const PUC_SET_MODE: u16 = 0x4C;
/// Out-of-band (compid 1) commands.
pub const OB_LAST_PKT_SEQ: u16 = 0x70;
pub const OB_BACKUP_OFFSET: u16 = 0x71;
pub const DEBUG: u16 = 0x80;
pub const GET_SYSTEM_BASE_INFO: u16 = 0x90;
pub const GET_WIFI_BASE_INFO: u16 = 0x91;

/// The ack of one command: its size and where its result byte is, if it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AckSpec {
    pub len: usize,
    pub result_at: Option<usize>,
}

const fn ack(len: usize, result_at: usize) -> AckSpec {
    AckSpec { len, result_at: Some(result_at) }
}

const fn data(len: usize) -> AckSpec {
    AckSpec { len, result_at: None }
}

/// The 62 entries of stock `g_pc_link_slave_cmd_handle_array`, with ack sizes.
/// An id not listed here gets no ack at all (stock: "Invalid cmd type").
pub fn ack_spec(cmd: u16) -> Option<AckSpec> {
    Some(match cmd {
        GET_DEV_INFO => data(0x3A5),
        GET_BIND_START => ack(0x11, 0),
        VERIFY_DEV_PASSWD => ack(1, 0),
        AUTH_DEV => ack(0x55, 0),
        GET_BIND_INFO => ack(0x2D2, 0),
        GET_DEV_STATUS => data(0x13E),
        MEETING_GETSTATUS => ack(0x205, 0),
        MEETING_GET_INFO => ack(0x284, 0x283),
        MEETING_GET_CUR_ALL_INFO => data(0x321),
        GET_SYS_LANGUAGE_LIST => data(0x501),
        GET_SYS_LANGUAGE_COUNTRY_LIST => data(0x4102),
        GET_SYS_COUNTRY_TIMEZONE_LIST => data(0x1E02),
        GET_SYS_COUNTRY | GET_SYS_LANGUAGE | SET_SYS_LANGUAGE => ack(0x41, 0x40),
        GET_SYS_CITY => ack(0x41, 0),
        GET_SYS_ALL_SETTING_LIST => ack(0x6406, 0x6405),
        GET_SYS_DEFAULT_SETTING => ack(0xC1, 0xC0),
        WIFI_LIST => ack(0x10C2, 0),
        WIFI_DISCONNECT => ack(0x41, 0x40),
        WIFI_GETSTATUS => ack(0x85, 0x84),
        OTA_GETSTATUS => ack(4, 0),
        GET_SYSTEM_BASE_INFO => ack(0x85, 0x84),
        GET_WIFI_BASE_INFO => ack(0x81, 0x80),
        AUTH_UNBIND_DEV | UNBIND_OFFLINE | BIND_DEV | UNAUTH_DEV | UNBIND_DEV | WIFI_SCAN
        | WIFI_CONNECT | WIFI_FORGETSSID | WIFI_SWITCH | WIFI_AUTO_CONNECT_SWITCH | MEETING_OP
        | MEETING_MARK | MEETING_MODE | MEETING_SCREENSHOT | SET_AUDIO_SPEAKER_VOLUME
        | SET_AUDIO_SPEAKER_MODE | SET_AUDIO_SPEAKER_MUTE | SET_AUDIO_MIC_VOLUME
        | SET_AUDIO_MIC_MODE | SET_AUDIO_MIC_MUTE | SET_SYS_ENABLE_UDISK_MODE
        | SET_SYS_TIME_SHOW_MODE | SET_SYS_TIME_AUTO_SYNC | SET_SYS_ALL_DATE_TIME_SETTING
        | SET_SYS_COUNTRY | SET_SYS_TIME | SET_SYS_TIMEZONE | SET_SYS_RECOVERY | SET_SYS_REBOOT
        | OTA_ENABLE_UDISK | OTA_OP | OTA_FINISH_SURE | FILE_TRANS | MEETING_PC_STATUS
        | PUC_ZOOM_CONTROL | PUC_POS_RESET | PUC_POS_CONTROL | PUC_SET_MODE => ack(1, 0),
        _ => return None,
    })
}

/// The "unsupported" ack: the stock size, zeros, result 1 where there is a
/// result byte. `None` for ids stock has no handler for.
pub fn unsupported(cmd: u16) -> Option<Vec<u8>> {
    let spec = ack_spec(cmd)?;
    let mut ack = vec![0u8; spec.len];
    if let Some(at) = spec.result_at {
        ack[at] = 1;
    }
    Some(ack)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stock_table_has_62_entries() {
        let handled = (0..=0xFFu16).filter(|&c| ack_spec(c).is_some()).count();
        assert_eq!(handled, 62);
        // DEBUG has a handler function in stock, but no entry in the table.
        for missing in [0x2C, 0x39, 0x41, 0x42, OB_LAST_PKT_SEQ, OB_BACKUP_OFFSET, DEBUG] {
            assert_eq!(ack_spec(missing), None, "{missing:#x} has no stock handler");
        }
    }

    #[test]
    fn unsupported_acks_have_the_stock_size_and_result_1() {
        assert_eq!(unsupported(BIND_DEV), Some(vec![1]));
        let get_bind_start = unsupported(GET_BIND_START).unwrap();
        assert_eq!((get_bind_start.len(), get_bind_start[0]), (17, 1));
        let info = unsupported(MEETING_GET_INFO).unwrap();
        assert_eq!((info.len(), info[0x283]), (0x284, 1));
        let ota = unsupported(OTA_GETSTATUS).unwrap();
        assert_eq!(ota, vec![1, 0, 0, 0]);
        let cloud = unsupported(MEETING_GET_CUR_ALL_INFO).unwrap();
        assert!(cloud.len() == 0x321 && cloud.iter().all(|&b| b == 0), "no result byte, all zero");
        assert_eq!(unsupported(0x39), None);
    }
}
