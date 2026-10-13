//! When an install may start, and when a new version counts as healthy.
//!
//! Installing restarts Wave, so it waits while anything is in use. A manual
//! install (the user tapped it) does not wait for "computer listening":
//! some apps keep the microphone open all day. Everything else holds a
//! manual install too, USB storage included: the restart would pull the
//! recordings drive from under the computer.

use std::time::Duration;
use waved_state::ProvisioningState;

use crate::service::Inputs;

/// What the gate looks at, read from the state bus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// Recording or paused.
    pub recording: bool,
    /// "Send this recording?" is waiting for an answer.
    pub decision_pending: bool,
    /// The computer's call (USB HID hook state).
    pub in_call: bool,
    pub ringing: bool,
    /// A phone call through the Bluetooth module: ringing, up, or its audio
    /// still running both ways (`BluetoothState::call`, `call_audio`).
    pub phone_call: bool,
    /// The computer has the USB microphone open.
    pub host_listening: bool,
    pub playing: bool,
    /// Wi-Fi setup is running.
    pub provisioning: bool,
    /// USB storage (G1) is not off: the computer has the recordings volume
    /// as a drive, or Wave is handing it over or taking it back.
    pub usb_storage: bool,
    /// Battery percent; `None` until the MCU has reported one.
    pub battery_pct: Option<u8>,
    pub usb_connected: bool,
}

impl Activity {
    pub fn read(inputs: &Inputs) -> Self {
        let meeting = inputs.meeting.borrow();
        let audio = inputs.audio.borrow();
        let battery = inputs.battery.borrow();
        let bt = inputs.bt.borrow();
        let battery_known = battery.level > 0 || battery.voltage > 0;
        Self {
            recording: meeting.recording,
            decision_pending: meeting.pending_decision.is_some(),
            in_call: audio.call_off_hook,
            ringing: audio.call_ringing,
            phone_call: bt.call.is_ringing() || bt.call.is_active() || bt.call_audio,
            host_listening: audio.host_listening,
            playing: inputs.playback.borrow().playing,
            provisioning: *inputs.provisioning.borrow() != ProvisioningState::Idle,
            usb_storage: inputs.usb_storage.borrow().recordings_away(),
            battery_pct: battery_known.then_some(battery.level),
            usb_connected: inputs.mcu.borrow().usb_connected,
        }
    }
}

/// Below this, on battery, an install waits for a cable.
pub const MIN_BATTERY_PCT: u8 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocker {
    Recording,
    Decision,
    UsbStorage,
    /// The computer's call or a phone call through Bluetooth.
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
    } else if activity.usb_storage {
        Some(Blocker::UsbStorage)
    } else if activity.in_call || activity.ringing || activity.phone_call {
        Some(Blocker::Call)
    } else if activity.host_listening && !manual {
        Some(Blocker::ComputerListening)
    } else if activity.playing {
        Some(Blocker::Playback)
    } else if activity.provisioning {
        Some(Blocker::WifiSetup)
    } else if !activity.usb_connected
        && activity
            .battery_pct
            .is_some_and(|pct| pct < MIN_BATTERY_PCT)
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
    use waved_state::{
        AudioState, BatteryState, BtCall, MeetingState, PendingDecision, StateBus, UsbStoragePhase,
    };

    fn idle() -> Activity {
        Activity {
            battery_pct: Some(80),
            ..Activity::default()
        }
    }

    #[test]
    fn an_idle_wave_may_install() {
        assert_eq!(blocker(&idle(), false), None);
        assert_eq!(
            blocker(&Activity::default(), false),
            None,
            "unknown battery"
        );
    }

    #[test]
    fn recordings_decisions_calls_usb_storage_playback_and_setup_always_wait() {
        type Case = (fn(&mut Activity), Blocker);
        let cases: [Case; 8] = [
            (|a| a.recording = true, Blocker::Recording),
            (|a| a.decision_pending = true, Blocker::Decision),
            (|a| a.usb_storage = true, Blocker::UsbStorage),
            (|a| a.in_call = true, Blocker::Call),
            (|a| a.ringing = true, Blocker::Call),
            (|a| a.phone_call = true, Blocker::Call),
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
        let bus = StateBus::new();
        bus.meeting.send_replace(MeetingState {
            pending_decision: Some(PendingDecision {
                id: Uuid::nil(),
                audio_path: PathBuf::new(),
                duration_secs: 0,
                size_bytes: 0,
                endpoint: String::new(),
                speakers: 0,
                marks: 0,
            }),
            ..MeetingState::default()
        });
        bus.audio.send_replace(AudioState {
            host_listening: true,
            ..AudioState::default()
        });
        bus.provisioning.send_replace(ProvisioningState::Scanning);
        bus.battery.send_replace(BatteryState {
            level: 55,
            ..BatteryState::default()
        });
        let activity = Activity::read(&Inputs::from_bus(&bus));
        assert!(activity.decision_pending && activity.host_listening && activity.provisioning);
        assert_eq!(activity.battery_pct, Some(55));
        assert!(!activity.recording && !activity.phone_call && !activity.usb_storage);
    }

    #[test]
    fn activity_reads_phone_calls_and_usb_storage() {
        let bus = StateBus::new();
        let read = || Activity::read(&Inputs::from_bus(&bus));
        let number = || "+3212345678".to_string();
        bus.bt
            .send_modify(|b| b.call = BtCall::Ringing { number: number() });
        assert!(read().phone_call, "ringing");
        bus.bt
            .send_modify(|b| b.call = BtCall::Active { number: number() });
        assert!(read().phone_call, "up");
        bus.bt.send_modify(|b| {
            b.call = BtCall::Idle;
            b.call_audio = true;
        });
        assert!(read().phone_call, "call audio still running");
        bus.bt.send_modify(|b| {
            b.call_audio = false;
            b.phone_audio = true;
        });
        assert!(!read().phone_call, "music from a phone is not a call");

        for (phase, away) in [
            (UsbStoragePhase::Off, false),
            (UsbStoragePhase::Starting, true),
            (UsbStoragePhase::On, true),
            (UsbStoragePhase::Stopping, true),
        ] {
            bus.usb_storage.send_modify(|u| u.phase = phase);
            assert_eq!(read().usb_storage, away, "{phase:?}");
        }
    }

    #[test]
    fn health_needs_a_minute_and_working_capture() {
        assert!(!healthy(Duration::from_secs(5), true));
        assert!(!healthy(HEALTHY_AFTER, false));
        assert!(healthy(HEALTHY_AFTER, true));
    }
}
