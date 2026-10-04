//! One consistent copy of every state channel per UI loop iteration.

use waved_state::{
    AudioState, BatteryState, BluetoothState, FilesState, McuState, MeetingState, PlaybackState,
    ProvisioningState, SettingsState, StateBus, WifiState,
};

#[derive(Clone, Debug, Default)]
pub struct UiSnapshot {
    pub audio: AudioState,
    pub meeting: MeetingState,
    pub battery: BatteryState,
    pub bt: BluetoothState,
    pub wifi: WifiState,
    pub provisioning: ProvisioningState,
    pub mcu: McuState,
    pub settings: SettingsState,
    pub files: FilesState,
    pub playback: PlaybackState,
    /// Wall clock, seconds since the Unix epoch.
    pub now_unix_s: i64,
    /// Monotonic milliseconds for UI timers (messages, gauge, hold to stop).
    pub now_ms: u64,
    /// Status clock text in the configured format.
    pub clock: String,
}

impl UiSnapshot {
    pub fn capture(bus: &StateBus, now_unix_s: i64, now_ms: u64) -> Self {
        let settings = bus.settings.borrow().clone();
        // Time zones arrive with workstream E3; until then the clock shows UTC.
        let clock = format_clock(now_unix_s, 0, settings.clock_24h);
        Self {
            audio: bus.audio.borrow().clone(),
            meeting: bus.meeting.borrow().clone(),
            battery: bus.battery.borrow().clone(),
            bt: bus.bt.borrow().clone(),
            wifi: bus.wifi.borrow().clone(),
            provisioning: bus.provisioning.borrow().clone(),
            mcu: bus.mcu.borrow().clone(),
            settings,
            files: bus.files.borrow().clone(),
            playback: bus.playback.borrow().clone(),
            now_unix_s,
            now_ms,
            clock,
        }
    }

    /// The host signals an active call (HID off-hook).
    pub fn in_call(&self) -> bool {
        self.audio.call_off_hook
    }

    /// The microphone signal goes somewhere: a call, the computer or a recording.
    pub fn audio_in_use(&self) -> bool {
        self.audio.call_off_hook || self.audio.host_listening || self.meeting.recording
    }

    /// Second line of the Home dial ("Dial states" in docs/mockups/README.md).
    pub fn connection_line(&self) -> String {
        if self.audio.mic_muted {
            return if self.in_call() || self.audio.host_listening {
                "Call can't hear you".into()
            } else {
                "Mic is off".into()
            };
        }
        if self.in_call() {
            "In a call".into()
        } else if self.audio.host_listening {
            "Computer is using the mic".into()
        } else if self.mcu.usb_connected {
            "Connected by USB".into()
        } else if self.bt.connected && !self.bt.device_name.is_empty() {
            format!("Bluetooth: {}", self.bt.device_name)
        } else if self.bt.connected {
            "Connected by Bluetooth".into()
        } else {
            "Not connected".into()
        }
    }
}

/// "14:32" or "2:32 pm".
pub fn format_clock(unix_s: i64, offset_minutes: i32, clock_24h: bool) -> String {
    let minutes = (unix_s + i64::from(offset_minutes) * 60).rem_euclid(86_400) / 60;
    let (hour, minute) = (minutes / 60, minutes % 60);
    if clock_24h {
        format!("{hour:02}:{minute:02}")
    } else {
        let hour12 = if hour % 12 == 0 { 12 } else { hour % 12 };
        let suffix = if hour < 12 { "am" } else { "pm" };
        format!("{hour12}:{minute:02} {suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats_follow_the_setting() {
        let t = 1_759_588_320; // 2025-10-04 14:32:00 UTC
        assert_eq!(format_clock(t, 0, true), "14:32");
        assert_eq!(format_clock(t, 0, false), "2:32 pm");
        assert_eq!(format_clock(t, 120, true), "16:32");
        assert_eq!(format_clock(0, 0, false), "12:00 am");
        assert_eq!(format_clock(-60, 0, true), "23:59");
    }

    #[test]
    fn connection_line_matches_the_dial_table() {
        let mut snap = UiSnapshot::default();
        assert_eq!(snap.connection_line(), "Not connected");
        snap.mcu.usb_connected = true;
        assert_eq!(snap.connection_line(), "Connected by USB");
        snap.audio.host_listening = true;
        assert_eq!(snap.connection_line(), "Computer is using the mic");
        snap.audio.call_off_hook = true;
        assert_eq!(snap.connection_line(), "In a call");
        snap.audio.mic_muted = true;
        assert_eq!(snap.connection_line(), "Call can't hear you");

        let mut bt = UiSnapshot::default();
        bt.bt.connected = true;
        bt.bt.device_name = "iPhone".into();
        assert_eq!(bt.connection_line(), "Bluetooth: iPhone");
        bt.audio.mic_muted = true;
        assert_eq!(bt.connection_line(), "Mic is off");
    }

    #[test]
    fn audio_in_use_means_the_mic_signal_goes_somewhere() {
        let mut snap = UiSnapshot::default();
        assert!(!snap.audio_in_use());
        snap.meeting.recording = true;
        assert!(snap.audio_in_use());
    }

    #[test]
    fn capture_copies_the_bus_and_formats_the_clock() {
        let bus = StateBus::new();
        let _keep = bus.settings.subscribe();
        bus.settings.send_modify(|s| s.clock_24h = false);
        let snap = UiSnapshot::capture(&bus, 1_759_588_320, 42);
        assert!(!snap.settings.clock_24h);
        assert_eq!(snap.clock, "2:32 pm");
        assert_eq!(snap.now_ms, 42);
    }
}
