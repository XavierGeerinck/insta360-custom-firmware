//! Every message the main screens show (A6), and `MessageFeed`, which turns
//! service events, state changes and the commands the UI sends into messages.
//! Messages replace the status pill for 2 s (success, info) or 4 s (warning,
//! error), never take a tap, and fit one line of at most 240 px. Workstream A
//! owns all message copy, including the runtime's and the Files screens'.
//!
//! | Trigger | Severity | Text |
//! |---|---|---|
//! | Upload succeeded (recording tracked) | Success | Recording sent |
//! | Upload deferred, Wi-Fi down (first time) | Warning | No Wi-Fi, upload waiting |
//! | Upload deferred, Wi-Fi up (first time) | Warning | Upload failed, trying again |
//! | Upload needs attention | Error | Upload failed, see Uploads |
//! | Integrity fault during a recording | Error | Recording failed, see Files |
//! | Integrity fault needing a restart | Warning | Audio restarting |
//! | Integrity fault blocking recording, Record while blocked or storage full | Error | Can't record, check storage |
//! | Storage below the minimum (rising edge) | Warning | Storage almost full |
//! | Stop held: Ask / Automatic / Local only | Info / Info / Success | Recording stopped / Recording saved, sending / Recording saved on Wave |
//! | Mark | Info | Marked at 42:17 |
//! | Short tap on Stop / pressing Stop | Info | Hold Stop to end the recording / Keep holding to stop |
//! | After stop: Send / Keep / Delete | Info / Success / Success | Sending recording / Kept on Wave / Recording deleted |
//! | Battery 10 % or less, not charging | Warning | Battery 10%, plug in Wave |
//! | Battery 5 % or less, not charging | Error | Battery 5%, plug in Wave now |
//! | Microphones not responding | Error | Microphones not responding |
//! | Play while recording / during a call | Error | Can't play while recording / Can't play during a call |

use crate::kit::dispatch::{Message, Severity};
use crate::kit::snapshot::UiSnapshot;
use std::collections::VecDeque;
use uuid::Uuid;
use waved_state::{Command, DeviceEvent, MicHealth};

/// The words, in one place, so every screen says the same thing.
pub mod copy {
    use crate::kit::dispatch::{Message, Severity};
    use crate::screens_v2::main_parts::format_timer;
    use waved_state::UploadMode;

    pub const HOLD_STOP: &str = "Hold Stop to end the recording";
    pub const KEEP_HOLDING: &str = "Keep holding to stop";
    pub const RECORDING_SENT: &str = "Recording sent";
    pub const SENDING: &str = "Sending recording";
    pub const KEPT: &str = "Kept on Wave";
    pub const DELETED: &str = "Recording deleted";
    pub const NO_WIFI_UPLOAD_WAITING: &str = "No Wi-Fi, upload waiting";
    pub const UPLOAD_WILL_RETRY: &str = "Upload failed, trying again";
    pub const UPLOAD_FAILED: &str = "Upload failed, see Uploads";
    pub const RECORDING_FAILED: &str = "Recording failed, see Files";
    pub const AUDIO_RESTARTING: &str = "Audio restarting";
    pub const CANT_RECORD: &str = "Can't record, check storage";
    pub const MICS_NOT_RESPONDING: &str = "Microphones not responding";
    pub const CANT_PLAY_RECORDING: &str = "Can't play while recording";
    pub const CANT_PLAY_CALL: &str = "Can't play during a call";
    pub const STORAGE_ALMOST_FULL: &str = "Storage almost full";

    pub fn marked_at(secs: u32) -> String {
        format!("Marked at {}", format_timer(secs))
    }

    pub fn battery_low(level: u8) -> String {
        format!("Battery {level}%, plug in Wave")
    }

    pub fn battery_critical(level: u8) -> String {
        format!("Battery {level}%, plug in Wave now")
    }

    /// What Stop's completed hold says, by what happens to the recording next.
    pub fn after_stop(mode: UploadMode) -> Message {
        match mode {
            UploadMode::Ask => Message::new(Severity::Info, "Recording stopped"),
            UploadMode::Automatic => Message::new(Severity::Info, "Recording saved, sending"),
            UploadMode::LocalOnly => Message::new(Severity::Success, "Recording saved on Wave"),
        }
    }
}

/// Recordings remembered for upload messages (the legacy tracker's bound).
const TRACKED: usize = 16;
const BATTERY_LOW: u8 = 10;
const BATTERY_CRITICAL: u8 = 5;
/// Back at or above this (or charging) re-arms the battery messages, so a
/// level bouncing between 10 and 11 % warns once.
const BATTERY_REARM: u8 = 15;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum BatteryStage {
    #[default]
    Normal,
    Low,
    Critical,
}

/// Turns service events, state changes and UI commands into messages. The
/// runtime owns one and shows what it returns: `on_event` for every
/// `DeviceEvent`, `on_snapshot` for every new snapshot, and `on_command` for
/// every `Command` the UI sends.
#[derive(Debug, Default)]
pub struct MessageFeed {
    /// Recordings whose upload outcome is announced, oldest first, with
    /// whether their first deferral was already announced. These are the ones
    /// stopped while this UI ran and the ones the user sent or retried by
    /// hand; uploads of older recordings (the retry backlog after a restart)
    /// stay quiet.
    stopped: VecDeque<(Uuid, bool)>,
    battery: BatteryStage,
    mics_dead: bool,
    low_space: bool,
}

impl MessageFeed {
    pub fn new() -> Self {
        Self::default()
    }

    /// The message for one service event, if it has one.
    pub fn on_event(&mut self, event: &DeviceEvent, snap: &UiSnapshot) -> Option<Message> {
        match event {
            DeviceEvent::RecordingStopped(id) => {
                self.track(*id);
                None
            }
            DeviceEvent::RecordingUploadSucceeded(id) => {
                let at = self.position(id)?;
                self.stopped.remove(at);
                Some(Message::new(Severity::Success, copy::RECORDING_SENT))
            }
            DeviceEvent::RecordingUploadDeferred(id) => {
                let at = self.position(id)?;
                if std::mem::replace(&mut self.stopped[at].1, true) {
                    return None;
                }
                let text = if snap.wifi.connected {
                    copy::UPLOAD_WILL_RETRY
                } else {
                    copy::NO_WIFI_UPLOAD_WAITING
                };
                Some(Message::new(Severity::Warning, text))
            }
            DeviceEvent::RecordingUploadNeedsAttention(id) => {
                self.position(id)?;
                Some(Message::new(Severity::Error, copy::UPLOAD_FAILED))
            }
            DeviceEvent::RecordingIntegrityFailed { meeting_id, fault } => {
                if meeting_id.is_some() {
                    Some(Message::new(Severity::Error, copy::RECORDING_FAILED))
                } else if fault.requires_restart() {
                    Some(Message::new(Severity::Warning, copy::AUDIO_RESTARTING))
                } else if fault.blocks_recording() {
                    Some(Message::new(Severity::Error, copy::CANT_RECORD))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Notes a command the UI sent. A manual send or retry starts tracking its
    /// recording, so the outcome ("Recording sent", a deferral, "Upload failed,
    /// see Uploads") is announced like one of a recording stopped here. Other
    /// commands do nothing.
    pub fn on_command(&mut self, command: &Command) {
        if let Command::SendRecording(id) | Command::RetryUpload(id) = command {
            self.track(*id);
        }
    }

    /// Messages for what changed since the previous snapshot: battery
    /// thresholds, the microphone check and low storage.
    pub fn on_snapshot(&mut self, snap: &UiSnapshot) -> Vec<Message> {
        let mut messages = Vec::new();
        let battery = &snap.battery;
        // BatteryState::default() is level 0 with voltage 0: no reading yet.
        let stage = if battery.voltage == 0 || battery.is_charging {
            BatteryStage::Normal
        } else if battery.level <= BATTERY_CRITICAL {
            BatteryStage::Critical
        } else if battery.level <= BATTERY_LOW {
            // Never back down a stage before the re-arm level: 5 <-> 6 % warns once.
            self.battery.max(BatteryStage::Low)
        } else if battery.level >= BATTERY_REARM {
            BatteryStage::Normal
        } else {
            self.battery
        };
        if stage > self.battery {
            match stage {
                BatteryStage::Low => messages.push(Message::new(
                    Severity::Warning,
                    copy::battery_low(battery.level),
                )),
                BatteryStage::Critical => messages.push(Message::new(
                    Severity::Error,
                    copy::battery_critical(battery.level),
                )),
                BatteryStage::Normal => {}
            }
        }
        self.battery = stage;

        let mics_dead = matches!(snap.audio.mic_health, MicHealth::NotResponding { .. });
        if mics_dead && !self.mics_dead {
            messages.push(Message::new(Severity::Error, copy::MICS_NOT_RESPONDING));
        }
        self.mics_dead = mics_dead;

        // The first snapshot counts too: space that is already low at start-up
        // is worth saying once.
        let low_space = snap.files.low_space;
        if low_space && !self.low_space {
            messages.push(Message::new(Severity::Warning, copy::STORAGE_ALMOST_FULL));
        }
        self.low_space = low_space;
        messages
    }

    fn track(&mut self, id: Uuid) {
        if let Some(at) = self.position(&id) {
            self.stopped.remove(at);
        }
        if self.stopped.len() == TRACKED {
            self.stopped.pop_front();
        }
        self.stopped.push_back((id, false));
    }

    fn position(&self, id: &Uuid) -> Option<usize> {
        self.stopped.iter().position(|(tracked, _)| tracked == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fonts::Font;
    use crate::kit::obj::text_width;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::MESSAGE_MAX_W;
    use crate::screens_v2::main_parts::fixtures;
    use waved_state::{MicHealth, RecordingIntegrityFault, UploadMode};

    fn message(severity: Severity, text: &str) -> Message {
        Message::new(severity, text)
    }

    #[test]
    fn every_message_fits_one_line_of_the_status_slot() {
        // StatusArea::show leaves MESSAGE_MAX_W - 44 px for the text (icon and padding).
        let text_max = MESSAGE_MAX_W - 44;
        let mut texts: Vec<String> = [
            copy::HOLD_STOP,
            copy::KEEP_HOLDING,
            copy::RECORDING_SENT,
            copy::SENDING,
            copy::KEPT,
            copy::DELETED,
            copy::NO_WIFI_UPLOAD_WAITING,
            copy::UPLOAD_WILL_RETRY,
            copy::UPLOAD_FAILED,
            copy::RECORDING_FAILED,
            copy::AUDIO_RESTARTING,
            copy::CANT_RECORD,
            copy::MICS_NOT_RESPONDING,
            copy::CANT_PLAY_RECORDING,
            copy::CANT_PLAY_CALL,
            copy::STORAGE_ALMOST_FULL,
        ]
        .map(String::from)
        .into();
        texts.push(copy::marked_at(359_999));
        texts.push(copy::battery_low(10));
        texts.push(copy::battery_critical(5));
        for mode in [
            UploadMode::Ask,
            UploadMode::Automatic,
            UploadMode::LocalOnly,
        ] {
            texts.push(copy::after_stop(mode).text);
        }
        lvgl(|_| {
            for text in &texts {
                let width = text_width(text, Font::Small);
                assert!(width <= text_max, "{text:?} is {width} px");
                assert!(!text.contains("..."), "{text:?}");
            }
        });
    }

    #[test]
    fn the_copy_is_the_agreed_wording() {
        assert_eq!(copy::UPLOAD_WILL_RETRY, "Upload failed, trying again");
        assert_eq!(copy::UPLOAD_FAILED, "Upload failed, see Uploads");
        assert_eq!(copy::RECORDING_FAILED, "Recording failed, see Files");
        assert_eq!(copy::CANT_RECORD, "Can't record, check storage");
        assert_eq!(copy::CANT_PLAY_RECORDING, "Can't play while recording");
        assert_eq!(copy::CANT_PLAY_CALL, "Can't play during a call");
        assert_eq!(copy::STORAGE_ALMOST_FULL, "Storage almost full");
    }

    #[test]
    fn stop_messages_follow_the_upload_mode() {
        assert_eq!(
            copy::after_stop(UploadMode::Ask),
            message(Severity::Info, "Recording stopped")
        );
        assert_eq!(
            copy::after_stop(UploadMode::Automatic),
            message(Severity::Info, "Recording saved, sending")
        );
        assert_eq!(
            copy::after_stop(UploadMode::LocalOnly),
            message(Severity::Success, "Recording saved on Wave")
        );
        assert_eq!(copy::marked_at(2_537), "Marked at 42:17");
    }

    #[test]
    fn uploads_are_announced_only_for_recordings_stopped_here() {
        let mut feed = MessageFeed::new();
        let snap = fixtures::snap();
        let (ours, backlog) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingStopped(ours), &snap),
            None
        );
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadSucceeded(backlog), &snap),
            None
        );
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadNeedsAttention(ours), &snap),
            Some(message(Severity::Error, copy::UPLOAD_FAILED))
        );
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadSucceeded(ours), &snap),
            Some(message(Severity::Success, copy::RECORDING_SENT))
        );
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadSucceeded(ours), &snap),
            None,
            "announced once"
        );
    }

    #[test]
    fn a_manual_resend_is_announced_after_on_command() {
        let mut feed = MessageFeed::new();
        let snap = fixtures::snap();
        let (sent, retried, other) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadSucceeded(sent), &snap),
            None,
            "an untracked upload is silent"
        );
        feed.on_command(&Command::SendRecording(sent));
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadSucceeded(sent), &snap),
            Some(message(Severity::Success, copy::RECORDING_SENT))
        );
        feed.on_command(&Command::RetryUpload(retried));
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadNeedsAttention(retried), &snap),
            Some(message(Severity::Error, copy::UPLOAD_FAILED))
        );
        feed.on_command(&Command::SetSpeakerVolume(50));
        feed.on_command(&Command::DiscardRecording(other));
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadSucceeded(other), &snap),
            None,
            "other commands track nothing"
        );
    }

    #[test]
    fn a_deferral_is_announced_once_with_its_reason() {
        let mut feed = MessageFeed::new();
        let mut snap = UiSnapshot::default();
        let (first, second) = (Uuid::from_u128(1), Uuid::from_u128(2));
        feed.on_event(&DeviceEvent::RecordingStopped(first), &snap);
        feed.on_event(&DeviceEvent::RecordingStopped(second), &snap);
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadDeferred(first), &snap),
            Some(message(Severity::Warning, copy::NO_WIFI_UPLOAD_WAITING))
        );
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadDeferred(first), &snap),
            None,
            "retries stay quiet"
        );
        snap.wifi.connected = true;
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadDeferred(second), &snap),
            Some(message(Severity::Warning, copy::UPLOAD_WILL_RETRY))
        );
        assert_eq!(
            feed.on_event(&DeviceEvent::RecordingUploadSucceeded(first), &snap),
            Some(message(Severity::Success, copy::RECORDING_SENT))
        );
    }

    #[test]
    fn integrity_faults_map_to_plain_messages() {
        let mut feed = MessageFeed::new();
        let snap = UiSnapshot::default();
        let event = |meeting_id, fault| DeviceEvent::RecordingIntegrityFailed { meeting_id, fault };
        assert_eq!(
            feed.on_event(
                &event(
                    Some(Uuid::from_u128(1)),
                    RecordingIntegrityFault::CaptureXrun
                ),
                &snap
            ),
            Some(message(Severity::Error, copy::RECORDING_FAILED))
        );
        assert_eq!(
            feed.on_event(&event(None, RecordingIntegrityFault::CaptureEnded), &snap),
            Some(message(Severity::Warning, copy::AUDIO_RESTARTING))
        );
        assert_eq!(
            feed.on_event(&event(None, RecordingIntegrityFault::WavWriteFailed), &snap),
            Some(message(Severity::Error, copy::CANT_RECORD))
        );
        assert_eq!(
            feed.on_event(&event(None, RecordingIntegrityFault::CaptureXrun), &snap),
            None
        );
        assert_eq!(feed.on_event(&DeviceEvent::UsbConnected, &snap), None);
    }

    #[test]
    fn battery_messages_fire_once_per_threshold_and_not_before_a_reading() {
        let mut feed = MessageFeed::new();
        let mut snap = UiSnapshot::default(); // level 0, voltage 0: no reading yet
        assert!(
            feed.on_snapshot(&snap).is_empty(),
            "no warning before the MCU reports"
        );
        snap.battery.voltage = 3_600;
        snap.battery.level = 11;
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.battery.level = 10;
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Warning, "Battery 10%, plug in Wave")]
        );
        snap.battery.level = 11;
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.battery.level = 10;
        assert!(
            feed.on_snapshot(&snap).is_empty(),
            "flapping around 10 % warns once"
        );
        snap.battery.level = 5;
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Error, "Battery 5%, plug in Wave now")]
        );
        for level in [6, 5, 6, 5, 6, 5] {
            snap.battery.level = level;
            assert!(
                feed.on_snapshot(&snap).is_empty(),
                "flapping 5 <-> 6 % warns once ({level} %)"
            );
        }
        snap.battery.level = 14;
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.battery.level = 5;
        assert!(
            feed.on_snapshot(&snap).is_empty(),
            "still armed below the re-arm level"
        );
        snap.battery.level = 15;
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.battery.level = 5;
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Error, "Battery 5%, plug in Wave now")],
            "re-armed at 15 %"
        );
        snap.battery.is_charging = true;
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.battery.is_charging = false;
        snap.battery.level = 9;
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Warning, "Battery 9%, plug in Wave")],
            "unplugging re-arms the warning"
        );
    }

    #[test]
    fn a_microphone_fault_is_announced_when_it_starts() {
        let mut feed = MessageFeed::new();
        let mut snap = UiSnapshot::default();
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.audio.mic_health = MicHealth::Degraded { low_mask: 0b1 };
        assert!(
            feed.on_snapshot(&snap).is_empty(),
            "degraded has no message"
        );
        snap.audio.mic_health = MicHealth::NotResponding { dead_mask: 0b1 };
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Error, copy::MICS_NOT_RESPONDING)]
        );
        assert!(feed.on_snapshot(&snap).is_empty(), "once per fault");
        snap.audio.mic_health = MicHealth::Ok;
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.audio.mic_health = MicHealth::NotResponding { dead_mask: 0b10 };
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Error, copy::MICS_NOT_RESPONDING)],
            "again after it cleared"
        );
    }

    #[test]
    fn low_storage_is_announced_on_the_rising_edge_only() {
        let mut feed = MessageFeed::new();
        let mut snap = UiSnapshot::default();
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.files.low_space = true;
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Warning, copy::STORAGE_ALMOST_FULL)]
        );
        assert!(
            feed.on_snapshot(&snap).is_empty(),
            "not again while it stays low"
        );
        snap.files.low_space = false;
        assert!(feed.on_snapshot(&snap).is_empty());
        snap.files.low_space = true;
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Warning, copy::STORAGE_ALMOST_FULL)],
            "again after it cleared"
        );
    }

    #[test]
    fn low_storage_already_true_on_the_first_snapshot_is_announced() {
        let mut feed = MessageFeed::new();
        let mut snap = UiSnapshot::default();
        snap.files.low_space = true;
        assert_eq!(
            feed.on_snapshot(&snap),
            [message(Severity::Warning, copy::STORAGE_ALMOST_FULL)]
        );
    }

    #[test]
    fn tracking_keeps_the_16_newest_recordings() {
        let mut feed = MessageFeed::new();
        let snap = UiSnapshot::default();
        for n in 0..=16 {
            feed.on_event(&DeviceEvent::RecordingStopped(Uuid::from_u128(n)), &snap);
        }
        assert_eq!(
            feed.on_event(
                &DeviceEvent::RecordingUploadSucceeded(Uuid::from_u128(0)),
                &snap
            ),
            None,
            "the oldest fell out"
        );
        assert!(feed
            .on_event(
                &DeviceEvent::RecordingUploadSucceeded(Uuid::from_u128(16)),
                &snap
            )
            .is_some());
    }
}
