//! What the PC link reports about the device, read from the state bus once
//! per command and per heartbeat. Plain data, so the replies are pure
//! functions of it.

use waved_state::{AudioState, BatteryState, MeetingState, SettingsState, SystemState, WifiState};

use super::mode;

/// `MEETING_SERVER_RECORD_STATE` values waved reports.
pub mod meet {
    pub const IDLE: u8 = 0;
    pub const RECORDING: u8 = 1;
    pub const PAUSED: u8 = 2;
    pub const PRE_RECORDING: u8 = 8;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// Percent, 0–100.
    pub speaker_volume: u8,
    pub speaker_muted: bool,
    /// Mic gain percent as the app sees it, 0–100 (100 = unity).
    pub mic_volume: u8,
    pub mic_muted: bool,
    /// Stock mic mode 1–6 (J4).
    pub mic_mode: u8,
    /// Fixed beam aim, degrees (the start of a Follow talker beam).
    pub beam_aim_deg: f32,
    /// The host has the USB microphone open.
    pub uac_input: bool,
    /// `meet::*`.
    pub meet_status: u8,
    pub meet_secs: u32,
    pub storage_blocked: bool,
    pub battery_level: u8,
    pub charging: bool,
    pub wifi_connected: bool,
    pub wifi_ssid: String,
    pub wifi_ip: String,
    pub wifi_rssi_dbm: i8,
    pub clock_synced: bool,
    pub time_zone: String,
    pub clock_24h: bool,
    pub pin_enabled: bool,
    pub first_run_done: bool,
    /// waved's version, e.g. "0.1.0 (374015f)".
    pub version: String,
    pub storage_total_mib: u32,
    pub storage_used_mib: u32,
}

impl Snapshot {
    pub fn from_state(
        audio: &AudioState,
        meeting: &MeetingState,
        battery: &BatteryState,
        wifi: &WifiState,
        settings: &SettingsState,
        system: &SystemState,
    ) -> Self {
        let meet_status = if meeting.recording && meeting.paused {
            meet::PAUSED
        } else if meeting.recording {
            meet::RECORDING
        } else if meeting.pre_recording_active {
            meet::PRE_RECORDING
        } else {
            meet::IDLE
        };
        let total = system.about.storage_total_bytes;
        let used = total.saturating_sub(system.about.storage_free_bytes);
        let mib = |bytes: u64| u32::try_from(bytes >> 20).unwrap_or(u32::MAX);
        Self {
            speaker_volume: audio.speaker_volume.min(100) as u8,
            speaker_muted: audio.speaker_muted,
            mic_volume: audio.mic_volume.min(100) as u8,
            mic_muted: audio.mic_muted,
            mic_mode: mode::to_stock(&audio.mode),
            beam_aim_deg: audio.beam_aim_deg,
            uac_input: audio.host_listening,
            meet_status,
            meet_secs: if meeting.recording { meeting.duration_secs } else { 0 },
            storage_blocked: meeting.storage_blocked || meeting.recording_blocked_reason.is_some(),
            battery_level: battery.level.min(100),
            charging: battery.is_charging,
            wifi_connected: wifi.connected,
            wifi_ssid: wifi.ssid.clone(),
            wifi_ip: wifi.ip_addr.clone(),
            wifi_rssi_dbm: wifi.signal_strength,
            clock_synced: wifi.clock_synchronized,
            time_zone: settings.time_zone.clone(),
            clock_24h: settings.clock_24h,
            pin_enabled: settings.pin_enabled,
            first_run_done: settings.first_run_done,
            version: system.about.version.clone(),
            storage_total_mib: mib(total),
            storage_used_mib: mib(used),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waved_state::AudioMode;

    fn states() -> (AudioState, MeetingState, BatteryState, WifiState, SettingsState, SystemState) {
        let audio = AudioState {
            speaker_volume: 40,
            mic_volume: 250,
            mode: AudioMode::Cardioid,
            beam_aim_deg: 120.0,
            host_listening: true,
            ..AudioState::default()
        };
        let meeting = MeetingState { recording: true, duration_secs: 75, ..MeetingState::default() };
        let battery = BatteryState { level: 64, voltage: 3900, temperature: 25, is_charging: true };
        let wifi = WifiState {
            connected: true,
            ssid: "Office".into(),
            ip_addr: "192.168.1.20".into(),
            signal_strength: -55,
            clock_synchronized: true,
        };
        let settings = SettingsState {
            time_zone: "Europe/Brussels".into(),
            pin_enabled: true,
            first_run_done: true,
            ..SettingsState::default()
        };
        let mut system = SystemState::default();
        system.about.version = "0.1.0 (abc1234)".into();
        system.about.storage_total_bytes = 8 << 30;
        system.about.storage_free_bytes = 6 << 30;
        (audio, meeting, battery, wifi, settings, system)
    }

    #[test]
    fn the_snapshot_reads_every_field_from_the_bus() {
        let (a, m, b, w, s, y) = states();
        let snap = Snapshot::from_state(&a, &m, &b, &w, &s, &y);
        assert_eq!(snap.speaker_volume, 40);
        assert_eq!(snap.mic_volume, 100, "boosts above unity read as 100");
        assert_eq!(snap.mic_mode, 3);
        assert_eq!(snap.beam_aim_deg, 120.0);
        assert!(snap.uac_input);
        assert_eq!((snap.meet_status, snap.meet_secs), (meet::RECORDING, 75));
        assert_eq!((snap.battery_level, snap.charging), (64, true));
        assert_eq!((snap.wifi_ssid.as_str(), snap.wifi_ip.as_str(), snap.wifi_rssi_dbm), ("Office", "192.168.1.20", -55));
        assert!(snap.wifi_connected && snap.clock_synced);
        assert_eq!(snap.time_zone, "Europe/Brussels");
        assert!(snap.pin_enabled && snap.first_run_done && snap.clock_24h);
        assert_eq!(snap.version, "0.1.0 (abc1234)");
        assert_eq!((snap.storage_total_mib, snap.storage_used_mib), (8192, 2048));
    }

    #[test]
    fn meeting_states_map_to_stock_record_states() {
        let (a, mut m, b, w, s, y) = states();
        m.paused = true;
        assert_eq!(Snapshot::from_state(&a, &m, &b, &w, &s, &y).meet_status, meet::PAUSED);
        m.recording = false;
        m.paused = false;
        m.pre_recording_active = true;
        assert_eq!(Snapshot::from_state(&a, &m, &b, &w, &s, &y).meet_status, meet::PRE_RECORDING);
        m.pre_recording_active = false;
        assert_eq!(Snapshot::from_state(&a, &m, &b, &w, &s, &y).meet_status, meet::IDLE);
    }

    #[test]
    fn volumes_above_100_are_capped() {
        let (mut a, m, b, w, s, y) = states();
        a.speaker_volume = 300;
        assert_eq!(Snapshot::from_state(&a, &m, &b, &w, &s, &y).speaker_volume, 100);
    }
}
