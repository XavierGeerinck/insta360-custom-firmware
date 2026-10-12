//! When an install may start, and when a new version counts as healthy.
//!
//! Installing restarts Wave, so it waits while anything is in use. A manual
//! install (the user tapped it) does not wait for "computer listening":
//! some apps keep the microphone open all day.

use std::time::Duration;
use waved_state::{
    AudioState, BatteryState, McuState, MeetingState, PlaybackState, ProvisioningState,
};

/// What the gate looks at, read from the state bus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// Recording or paused.
    pub recording: bool,
    /// "Send this recording?" is waiting for an answer.
    pub decision_pending: bool,
    pub in_call: bool,
    pub ringing: bool,
    /// The computer has the USB microphone open.
    pub host_listening: bool,
    pub playing: bool,
    /// Wi-Fi setup is running.
    pub provisioning: bool,
    /// Battery percent; `None` until the MCU has reported one.
    pub battery_pct: Option<u8>,
    pub usb_connected: bool,
}

impl Activity {
    pub fn read(
        meeting: &MeetingState,
        audio: &AudioState,
        playback: &PlaybackState,
        provisioning: &ProvisioningState,
        battery: &BatteryState,
        mcu: &McuState,
    ) -> Self {
        let battery_known = battery.level > 0 || battery.voltage > 0;
        Self {
            recording: meeting.recording,
            decision_pending: meeting.pending_decision.is_some(),
            in_call: audio.call_off_hook,
            ringing: audio.call_ringing,
            host_listening: audio.host_listening,
            playing: playback.playing,
            provisioning: *provisioning != ProvisioningState::Idle,
            battery_pct: battery_known.then_some(battery.level),
            usb_connected: mcu.usb_connected,
        }
    }
}

/// Below this, on battery, an install waits for a cable.
pub const MIN_BATTERY_PCT: u8 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocker {
    Recording,
    Decision,
    Call,
    ComputerListening,
    Playback,
    WifiSetup,
    LowBattery,
}

/// Why an install must wait now, or `None` when it may start.
pub fn blocker(activity: &Activity, manual: bool) -> Option<Blocker> {
    if activity.recording {
        Some(Blocker::Recording)
    } else if activity.decision_pending {
        Some(Blocker::Decision)
    } else if activity.in_call || activity.ringing {
        Some(Blocker::Call)
    } else if activity.host_listening && !manual {
        Some(Blocker::ComputerListening)
    } else if activity.playing {
        Some(Blocker::Playback)
    } else if activity.provisioning {
        Some(Blocker::WifiSetup)
    } else if !activity.usb_connected
        && activity.battery_pct.is_some_and(|pct| pct < MIN_BATTERY_PCT)
    {
        Some(Blocker::LowBattery)
    } else {
        None
    }
}

/// A new version commits once it has run this long with capture healthy.
/// `S60waved` kills the first instance after a boot at 5 s, so that one
/// never commits; the respawned one does.
pub const HEALTHY_AFTER: Duration = Duration::from_secs(60);

pub fn healthy(uptime: Duration, capture_healthy: bool) -> bool {
    uptime >= HEALTHY_AFTER && capture_healthy
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use uuid::Uuid;
    use waved_state::PendingDecision;

    fn idle() -> Activity {
        Activity {
            battery_pct: Some(80),
            ..Activity::default()
        }
    }

    #[test]
    fn an_idle_wave_may_install() {
        assert_eq!(blocker(&idle(), false), None);
        assert_eq!(blocker(&Activity::default(), false), None, "unknown battery");
    }

    #[test]
    fn recordings_decisions_calls_playback_and_setup_always_wait() {
        let cases: [(fn(&mut Activity), Blocker); 6] = [
            (|a| a.recording = true, Blocker::Recording),
            (|a| a.decision_pending = true, Blocker::Decision),
            (|a| a.in_call = true, Blocker::Call),
            (|a| a.ringing = true, Blocker::Call),
            (|a| a.playing = true, Blocker::Playback),
            (|a| a.provisioning = true, Blocker::WifiSetup),
        ];
        for (set, expected) in cases {
            let mut activity = idle();
            set(&mut activity);
            assert_eq!(blocker(&activity, false), Some(expected));
            assert_eq!(blocker(&activity, true), Some(expected), "manual too");
        }
    }

    #[test]
    fn a_listening_computer_holds_only_automatic_installs() {
        let activity = Activity {
            host_listening: true,
            ..idle()
        };
        assert_eq!(blocker(&activity, false), Some(Blocker::ComputerListening));
        assert_eq!(blocker(&activity, true), None);
    }

    #[test]
    fn a_low_battery_waits_for_a_cable() {
        let low = Activity {
            battery_pct: Some(12),
            ..idle()
        };
        assert_eq!(blocker(&low, true), Some(Blocker::LowBattery));
        let plugged = Activity {
            usb_connected: true,
            ..low
        };
        assert_eq!(blocker(&plugged, true), None);
    }

    #[test]
    fn activity_reads_the_bus_states() {
        let mut meeting = MeetingState::default();
        meeting.pending_decision = Some(PendingDecision {
            id: Uuid::nil(),
            audio_path: PathBuf::new(),
            duration_secs: 0,
            size_bytes: 0,
            endpoint: String::new(),
            speakers: 0,
            marks: 0,
        });
        let audio = AudioState {
            host_listening: true,
            ..AudioState::default()
        };
        let battery = BatteryState {
            level: 55,
            ..BatteryState::default()
        };
        let activity = Activity::read(
            &meeting,
            &audio,
            &PlaybackState::default(),
            &ProvisioningState::Scanning,
            &battery,
            &McuState::default(),
        );
        assert!(activity.decision_pending && activity.host_listening && activity.provisioning);
        assert_eq!(activity.battery_pct, Some(55));
        assert!(!activity.recording);
    }

    #[test]
    fn health_needs_a_minute_and_working_capture() {
        assert!(!healthy(Duration::from_secs(5), true));
        assert!(!healthy(HEALTHY_AFTER, false));
        assert!(healthy(HEALTHY_AFTER, true));
    }
}
