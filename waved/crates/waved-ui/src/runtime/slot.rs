//! The runtime's one status-slot message. Screens (`UiEvent::Show`) and
//! `MessageFeed` (which owns all message copy, R4) both post here.

use crate::kit::dispatch::{Message, Severity};
use crate::kit::snapshot::{ActiveMessage, UiSnapshot};
use crate::screens_v2::messages::MessageFeed;
use waved_state::{Command, DeviceEvent};

/// The message in the status slot. Screens' `UiEvent::Show` and A's
/// `MessageFeed` both post here; each post is a new `ActiveMessage` (new seq),
/// the newest replaces the one showing, and it leaves at `until_ms()`.
#[derive(Debug, Default)]
pub struct MessageSlot {
    feed: MessageFeed,
    active: Option<ActiveMessage>,
    /// The seq of the last post; the next one is this plus one.
    last_seq: u64,
}

impl MessageSlot {
    /// Shows `message` from `now_ms`, replacing whatever is showing.
    pub fn post(&mut self, message: Message, now_ms: u64) {
        self.last_seq += 1;
        self.active = Some(ActiveMessage {
            seq: self.last_seq,
            message,
            since_ms: now_ms,
        });
    }

    /// `MessageFeed::on_event`; posts what it returns.
    pub fn on_event(&mut self, event: &DeviceEvent, snap: &UiSnapshot) {
        if let Some(message) = self.feed.on_event(event, snap) {
            self.post(message, snap.now_ms);
        }
    }

    /// `MessageFeed::on_command` (tracks manual sends and retries).
    pub fn on_command(&mut self, command: &Command) {
        self.feed.on_command(command);
    }

    /// `MessageFeed::on_snapshot`, every loop iteration from the first. When it
    /// returns several, posts the most severe (Error > Warning > Info >
    /// Success, first among equals).
    pub fn on_snapshot(&mut self, snap: &UiSnapshot) {
        let mut best: Option<Message> = None;
        for message in self.feed.on_snapshot(snap) {
            if best
                .as_ref()
                .is_none_or(|kept| rank(message.severity) > rank(kept.severity))
            {
                best = Some(message);
            }
        }
        if let Some(message) = best {
            self.post(message, snap.now_ms);
        }
    }

    /// What goes in `UiSnapshot::message` at `now_ms`; drops the message once
    /// `now_ms >= until_ms()`.
    pub fn current(&mut self, now_ms: u64) -> Option<ActiveMessage> {
        if self
            .active
            .as_ref()
            .is_some_and(|active| now_ms >= active.until_ms())
        {
            self.active = None;
        }
        self.active.clone()
    }
}

fn rank(severity: Severity) -> u8 {
    match severity {
        Severity::Success => 0,
        Severity::Info => 1,
        Severity::Warning => 2,
        Severity::Error => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens_v2::messages::copy;
    use uuid::Uuid;
    use waved_state::MicHealth;

    fn msg(severity: Severity, text: &str) -> Message {
        Message::new(severity, text)
    }

    fn at(now_ms: u64) -> UiSnapshot {
        UiSnapshot {
            now_ms,
            ..UiSnapshot::default()
        }
    }

    /// A battery reading the feed accepts: voltage set, not charging.
    fn low_battery(snap: &mut UiSnapshot, level: u8) {
        snap.battery.voltage = 3_600;
        snap.battery.level = level;
        snap.battery.is_charging = false;
    }

    #[test]
    fn each_post_gets_a_new_seq_even_with_the_same_text() {
        let mut slot = MessageSlot::default();
        assert_eq!(slot.current(0), None);
        slot.post(msg(Severity::Success, copy::RECORDING_SENT), 100);
        let first = slot.current(100).unwrap();
        slot.post(msg(Severity::Success, copy::RECORDING_SENT), 200);
        let second = slot.current(200).unwrap();
        assert_eq!(first.seq, 1);
        assert_eq!(second.seq, 2);
        assert_eq!(first.message, second.message);
    }

    #[test]
    fn a_message_leaves_after_its_severity_duration() {
        let mut slot = MessageSlot::default();
        slot.post(msg(Severity::Success, copy::RECORDING_SENT), 1_000);
        assert!(slot.current(2_999).is_some());
        assert_eq!(slot.current(3_000), None, "gone at since_ms + 2 000");
        assert_eq!(slot.current(3_001), None, "and stays gone");

        slot.post(msg(Severity::Error, copy::UPLOAD_FAILED), 10_000);
        assert!(slot.current(13_999).is_some());
        assert_eq!(slot.current(14_000), None, "errors last 4 s");
        slot.post(msg(Severity::Warning, copy::AUDIO_RESTARTING), 20_000);
        assert!(slot.current(23_999).is_some());
        assert_eq!(slot.current(24_000), None, "warnings last 4 s");
    }

    #[test]
    fn a_newer_post_replaces_the_active_one_and_is_timed_from_itself() {
        let mut slot = MessageSlot::default();
        slot.post(msg(Severity::Error, copy::UPLOAD_FAILED), 1_000);
        slot.post(msg(Severity::Success, copy::RECORDING_SENT), 2_000);
        let active = slot.current(2_500).unwrap();
        assert_eq!(active.message.text, copy::RECORDING_SENT);
        assert_eq!(active.since_ms, 2_000);
        assert_eq!(active.seq, 2);
        assert_eq!(slot.current(4_000), None, "2 s from its own post");
    }

    #[test]
    fn a_tracked_upload_that_succeeds_posts_recording_sent() {
        let mut slot = MessageSlot::default();
        let id = Uuid::from_u128(1);
        slot.on_event(&DeviceEvent::RecordingStopped(id), &at(500));
        assert_eq!(slot.current(500), None, "stopping posts nothing");
        slot.on_event(&DeviceEvent::RecordingUploadSucceeded(id), &at(900));
        let active = slot.current(900).unwrap();
        assert_eq!(active.message, msg(Severity::Success, copy::RECORDING_SENT));
        assert_eq!(active.since_ms, 900, "timed from the snapshot's now_ms");
    }

    #[test]
    fn an_untracked_backlog_upload_posts_nothing() {
        let mut slot = MessageSlot::default();
        let backlog = Uuid::from_u128(2);
        slot.on_event(&DeviceEvent::RecordingUploadSucceeded(backlog), &at(900));
        assert_eq!(slot.current(900), None);
    }

    #[test]
    fn a_manual_send_makes_its_outcome_announced() {
        let mut slot = MessageSlot::default();
        let id = Uuid::from_u128(3);
        slot.on_command(&Command::SendRecording(id));
        slot.on_event(&DeviceEvent::RecordingUploadSucceeded(id), &at(700));
        assert_eq!(
            slot.current(700).unwrap().message,
            msg(Severity::Success, copy::RECORDING_SENT)
        );
    }

    #[test]
    fn a_low_battery_posts_once_per_snapshot_change() {
        let mut slot = MessageSlot::default();
        let mut snap = at(1_000);
        low_battery(&mut snap, 10);
        slot.on_snapshot(&snap);
        let active = slot.current(1_000).unwrap();
        assert_eq!(active.message.text, copy::battery_low(10));
        assert_eq!(active.message.severity, Severity::Warning);
        assert_eq!(active.seq, 1);
        snap.now_ms = 1_500;
        slot.on_snapshot(&snap);
        assert_eq!(
            slot.current(1_500).unwrap().seq,
            1,
            "the same snapshot posts nothing new"
        );
    }

    #[test]
    fn a_batch_shows_the_most_severe_message() {
        let mut slot = MessageSlot::default();
        let mut snap = at(1_000);
        low_battery(&mut snap, 10); // Warning
        snap.files.low_space = true; // Warning
        snap.audio.mic_health = MicHealth::NotResponding { dead_mask: 0b1 }; // Error
        slot.on_snapshot(&snap);
        let active = slot.current(1_000).unwrap();
        assert_eq!(
            active.message,
            msg(Severity::Error, copy::MICS_NOT_RESPONDING)
        );
        assert_eq!(active.seq, 1, "one post for the batch");
    }

    #[test]
    fn among_equal_severities_the_first_one_wins() {
        let mut slot = MessageSlot::default();
        let mut snap = at(1_000);
        low_battery(&mut snap, 10);
        snap.files.low_space = true;
        slot.on_snapshot(&snap);
        assert_eq!(
            slot.current(1_000).unwrap().message.text,
            copy::battery_low(10)
        );
    }
}
