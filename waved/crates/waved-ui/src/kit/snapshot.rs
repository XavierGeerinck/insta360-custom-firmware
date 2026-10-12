//! One consistent copy of every state channel per UI loop iteration.

use crate::kit::dispatch::Message;
use waved_state::{
    AudioState, BatteryState, BluetoothState, FilesState, McuState, MeetingState, OtaState,
    PlaybackState, ProvisioningState, SettingsState, StateBus, SystemState, WifiState,
    CLOCK_VALID_AFTER_UNIX_S,
};

/// The message in the status slot. The runtime keeps it, so it survives a
/// screen change (Send on After stop, then Home shows "Sending recording").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveMessage {
    /// Increments for every message shown, so the same text twice shows twice.
    pub seq: u64,
    pub message: Message,
    /// `now_ms` when the runtime accepted it. It expires at `until_ms()`.
    pub since_ms: u64,
}

impl ActiveMessage {
    /// When the message leaves the status slot:
    /// `since_ms + message.severity.duration_ms()`.
    pub fn until_ms(&self) -> u64 {
        self.since_ms + self.message.severity.duration_ms()
    }
}

/// The runtime's PIN check, for the PIN screen (K2, D10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PinStatus {
    /// Wrong tries since waved started. A PIN screen clears its dots and says
    /// "Wrong PIN" when this grows.
    pub wrong: u32,
    /// `now_ms` at which the keypad works again after five wrong tries;
    /// `None` while it is not waiting.
    pub locked_until_ms: Option<u64>,
}

impl PinStatus {
    pub fn locked(&self, now_ms: u64) -> bool {
        self.locked_until_ms.is_some_and(|until| now_ms < until)
    }
}

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
    /// Time zone offset and About (waved-system).
    pub system: SystemState,
    /// Firmware updates (waved-update).
    pub ota: OtaState,
    /// Wall clock, seconds since the Unix epoch.
    pub now_unix_s: i64,
    /// Monotonic milliseconds for UI timers (messages, gauge, hold to stop).
    pub now_ms: u64,
    /// Status clock text in the configured format and zone; `"--:--"` until
    /// the wall clock is believable (`CLOCK_VALID_AFTER_UNIX_S`).
    pub clock: String,
    /// Set by the runtime from `UiEvent::Show`; `capture` leaves it `None`.
    pub message: Option<ActiveMessage>,
    /// Set by the runtime for a screen's `build` only (it is `false` in
    /// `update`): the route has been built before since it entered the router
    /// stack, and this is a rebuild (Back from a screen that was on top of
    /// it). A screen that acts in `build` (Wi-Fi setup starts provisioning)
    /// must not act again. `capture` leaves it `false`.
    pub rebuilt: bool,
    /// Set by the runtime every step; `capture` leaves the default.
    pub pin: PinStatus,
}

impl UiSnapshot {
    pub fn capture(bus: &StateBus, now_unix_s: i64, now_ms: u64) -> Self {
        let settings = bus.settings.borrow().clone();
        let system = bus.system.borrow().clone();
        // Before NTP or the RTC set the clock, show dashes instead of a 1970 time.
        let clock = if now_unix_s >= CLOCK_VALID_AFTER_UNIX_S {
            format_clock(now_unix_s, system.utc_offset_min, settings.clock_24h)
        } else {
            "--:--".into()
        };
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
            system,
            ota: bus.ota.borrow().clone(),
            now_unix_s,
            now_ms,
            clock,
            message: None,
            rebuilt: false,
            pin: PinStatus::default(),
        }
    }

    /// Local wall-clock seconds (for date lines on Date and time and Resting).
    pub fn local_unix_s(&self) -> i64 {
        self.now_unix_s + i64::from(self.system.utc_offset_min) * 60
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
            "Computer is listening".into()
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
    use crate::kit::dispatch::Severity;

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
        // Fits inside the r 70 ring (contract R14).
        assert_eq!(snap.connection_line(), "Computer is listening");
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
        assert!(snap.message.is_none(), "the runtime adds the message");
    }

    #[test]
    fn capture_shows_the_clock_in_the_zone_waved_system_resolved() {
        let bus = StateBus::new();
        let _keep = bus.system.subscribe();
        bus.system.send_modify(|s| {
            s.utc_offset_min = 120;
            s.about.serial = "IWB0123".into();
        });
        let snap = UiSnapshot::capture(&bus, 1_759_588_320, 0);
        assert_eq!(snap.clock, "16:32");
        assert_eq!(snap.system.utc_offset_min, 120);
        assert_eq!(snap.system.about.serial, "IWB0123", "About travels too");
    }

    #[test]
    fn zones_without_daylight_saving_shift_by_their_fixed_offset() {
        let bus = StateBus::new();
        let _keep = bus.system.subscribe();
        // Asia/Kolkata, UTC+5:30 all year: 14:32 UTC is 20:02.
        bus.system.send_modify(|s| s.utc_offset_min = 330);
        let snap = UiSnapshot::capture(&bus, 1_759_588_320, 0);
        assert_eq!(snap.clock, "20:02");
        assert_eq!(snap.local_unix_s(), 1_759_588_320 + 330 * 60);
        // America/Sao_Paulo, UTC-3 all year: 14:32 UTC is 11:32.
        bus.system.send_modify(|s| s.utc_offset_min = -180);
        let snap = UiSnapshot::capture(&bus, 1_759_588_320, 0);
        assert_eq!(snap.clock, "11:32");
        assert_eq!(snap.local_unix_s(), 1_759_588_320 - 180 * 60);
    }

    #[test]
    fn the_clock_shows_dashes_until_it_is_believable() {
        let bus = StateBus::new();
        assert_eq!(UiSnapshot::capture(&bus, 0, 0).clock, "--:--");
        assert_eq!(
            UiSnapshot::capture(&bus, CLOCK_VALID_AFTER_UNIX_S - 1, 0).clock,
            "--:--"
        );
        assert_eq!(
            UiSnapshot::capture(&bus, CLOCK_VALID_AFTER_UNIX_S, 0).clock,
            "00:00"
        );
        // The zone does not make a 1970 clock believable.
        bus.system.send_modify(|s| s.utc_offset_min = 120);
        assert_eq!(UiSnapshot::capture(&bus, 60, 0).clock, "--:--");
    }

    #[test]
    fn local_time_is_utc_until_a_zone_arrives() {
        let snap = UiSnapshot {
            now_unix_s: 1_759_588_320,
            ..UiSnapshot::default()
        };
        assert_eq!(snap.local_unix_s(), 1_759_588_320);
    }

    #[test]
    fn active_messages_end_after_their_severity() {
        let sent = ActiveMessage {
            seq: 1,
            message: Message::new(Severity::Success, "Recording sent"),
            since_ms: 1_000,
        };
        assert_eq!(sent.until_ms(), 3_000);
        let failed = ActiveMessage {
            seq: 2,
            message: Message::new(Severity::Error, "Upload failed, see Uploads"),
            since_ms: 1_000,
        };
        assert_eq!(failed.until_ms(), 5_000);
    }
}
