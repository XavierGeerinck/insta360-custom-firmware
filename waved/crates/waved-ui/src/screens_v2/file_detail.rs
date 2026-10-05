//! Recording detail (docs/mockups file-detail.png): the dial becomes a
//! playback scrubber, Delete sits on the left, Send / Send again on the
//! right, and the upload state or failure reason under them.
//!
//! This half is pure logic, tested on the host; the screen builds on it.

use super::files::{sentence, status_look};
use crate::kit::icons;
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{TEXT_2, WARN};
use uuid::Uuid;
use waved_state::{PlaybackState, RecordingStatus, RecordingSummary};

/// After a seek, show its target until playback reports a position within
/// `SEEK_TOLERANCE_MS` of it, or for at most `SEEK_SETTLE_MS`.
pub const SEEK_SETTLE_MS: u64 = 1_500;
pub const SEEK_TOLERANCE_MS: u32 = 1_000;
pub const SENDING: &str = "Sending the recording";
pub const BUSY_RECORDING: &str = "Can't play while recording";
pub const BUSY_CALL: &str = "Can't play during a call";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayView {
    /// The speaker is playing (or paused on) this recording.
    pub active: bool,
    pub playing: bool,
    pub position_ms: u32,
    pub duration_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingSeek {
    pub target_ms: u32,
    pub sent_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CentreTap {
    Play { from_ms: u32 },
    Pause,
    Resume,
    Refuse(&'static str),
    Nothing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendAction {
    Send,
    SendAgain,
}

impl SendAction {
    pub fn icon(self) -> &'static str {
        match self {
            SendAction::Send => icons::SEND,
            SendAction::SendAgain => icons::REFRESH,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SendAction::Send => "Send",
            SendAction::SendAgain => "Send again",
        }
    }
}

/// What the scrubber shows for recording `id`. Playback of another recording
/// is ignored; until this one plays, the position is the locally chosen
/// start (`local_ms`) and the duration comes from the summary.
pub fn play_view(
    id: Uuid,
    rec: &RecordingSummary,
    playback: &PlaybackState,
    local_ms: u32,
) -> PlayView {
    let active = playback.recording == Some(id);
    let duration_ms = if active && playback.duration_ms > 0 {
        playback.duration_ms
    } else {
        rec.duration_secs.saturating_mul(1_000)
    };
    let position_ms = if active {
        playback.position_ms
    } else {
        local_ms
    }
    .min(duration_ms);
    PlayView {
        active,
        playing: active && playback.playing,
        position_ms,
        duration_ms,
    }
}

/// Marks from the playback when it is this recording's, else from the summary.
pub fn marks_for<'a>(
    id: Uuid,
    rec: &'a RecordingSummary,
    playback: &'a PlaybackState,
) -> &'a [u32] {
    if playback.recording == Some(id) && !playback.marks_ms.is_empty() {
        &playback.marks_ms
    } else {
        &rec.marks_ms
    }
}

pub fn shown_position(
    actual_ms: u32,
    pending: Option<PendingSeek>,
    now_ms: u64,
) -> (u32, Option<PendingSeek>) {
    match pending {
        Some(seek)
            if actual_ms.abs_diff(seek.target_ms) > SEEK_TOLERANCE_MS
                && now_ms.saturating_sub(seek.sent_at_ms) < SEEK_SETTLE_MS =>
        {
            (seek.target_ms, Some(seek))
        }
        _ => (actual_ms, None),
    }
}

pub fn playable(rec: &RecordingSummary) -> bool {
    rec.status != RecordingStatus::Damaged && rec.duration_secs > 0
}

/// Why playback can't start now: the speaker would play into a recording,
/// or over a call.
pub fn play_guard(snap: &UiSnapshot) -> Option<&'static str> {
    if snap.meeting.recording {
        Some(BUSY_RECORDING)
    } else if snap.in_call() {
        Some(BUSY_CALL)
    } else {
        None
    }
}

/// What a tap in the middle does. Pausing is always allowed.
pub fn centre_tap(
    view: &PlayView,
    local_ms: u32,
    playable: bool,
    guard: Option<&'static str>,
) -> CentreTap {
    if !playable {
        return CentreTap::Nothing;
    }
    if view.playing {
        return CentreTap::Pause;
    }
    if let Some(reason) = guard {
        return CentreTap::Refuse(reason);
    }
    if view.active {
        CentreTap::Resume
    } else {
        CentreTap::Play { from_ms: local_ms }
    }
}

/// "Send" for a recording kept on Wave, "Send again" after a failure; nothing
/// once it is sent or on its way, for damaged files, or without a server.
pub fn send_action(status: RecordingStatus, server_set: bool) -> Option<SendAction> {
    if !server_set {
        return None;
    }
    match status {
        RecordingStatus::LocalOnly => Some(SendAction::Send),
        RecordingStatus::Failed => Some(SendAction::SendAgain),
        _ => None,
    }
}

/// The line under the scrubber: the failure and its reason in amber, or the
/// upload state in Text 2.
pub fn note_line(rec: &RecordingSummary, wifi_connected: bool) -> (String, u32) {
    let (_, _, words) = status_look(rec.status, wifi_connected);
    let head = sentence(words);
    match rec.status {
        RecordingStatus::Failed | RecordingStatus::Damaged => {
            let reason = rec
                .last_error
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty());
            let text = match reason {
                Some(reason) => format!("{head}: {reason}"),
                None => head,
            };
            (text, WARN)
        }
        _ => (head, TEXT_2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::icons;
    use crate::kit::tokens::{TEXT_2, WARN};
    use crate::screens_v2::files::recording;
    use waved_state::{AudioState, MeetingState};

    const ID: Uuid = Uuid::from_u128(1);
    const OTHER: Uuid = Uuid::from_u128(2);

    fn failed() -> RecordingSummary {
        let mut rec = recording(ID, RecordingStatus::Failed, "2026-10-05T14:02:00Z", 2_537);
        rec.last_error = Some("server didn't answer".into());
        rec.marks_ms = vec![384_000];
        rec
    }

    fn playing(id: Uuid, position_ms: u32) -> PlaybackState {
        PlaybackState {
            recording: Some(id),
            playing: true,
            position_ms,
            duration_ms: 2_537_500,
            marks_ms: vec![384_000, 1_413_000],
        }
    }

    #[test]
    fn another_recordings_playback_is_not_shown_here() {
        let rec = failed();
        let view = play_view(ID, &rec, &playing(OTHER, 900_000), 12_000);
        assert_eq!(
            view,
            PlayView {
                active: false,
                playing: false,
                position_ms: 12_000,
                duration_ms: 2_537_000
            }
        );
        assert_eq!(marks_for(ID, &rec, &playing(OTHER, 0)), &[384_000]);
        let mine = play_view(ID, &rec, &playing(ID, 900_000), 12_000);
        assert_eq!(
            mine,
            PlayView {
                active: true,
                playing: true,
                position_ms: 900_000,
                duration_ms: 2_537_500
            }
        );
        assert_eq!(marks_for(ID, &rec, &playing(ID, 0)), &[384_000, 1_413_000]);
        // Before C8 reports a duration, the summary's stands in.
        let early = PlaybackState {
            duration_ms: 0,
            ..playing(ID, 0)
        };
        assert_eq!(play_view(ID, &rec, &early, 0).duration_ms, 2_537_000);
    }

    #[test]
    fn a_seek_holds_its_target_until_playback_catches_up() {
        let pending = Some(PendingSeek {
            target_ms: 600_000,
            sent_at_ms: 10_000,
        });
        // Playback still reports the old spot: keep showing the target.
        assert_eq!(shown_position(100_000, pending, 10_400), (600_000, pending));
        // Within a second of the target: follow playback and forget the seek.
        assert_eq!(shown_position(600_400, pending, 10_500), (600_400, None));
        // 1.5 s without catching up: give up and show what plays.
        assert_eq!(shown_position(100_000, pending, 11_500), (100_000, None));
        assert_eq!(shown_position(42, None, 0), (42, None));
    }

    #[test]
    fn the_middle_plays_pauses_resumes_or_explains() {
        let idle = PlayView {
            duration_ms: 1_000,
            ..PlayView::default()
        };
        assert_eq!(
            centre_tap(&idle, 0, true, None),
            CentreTap::Play { from_ms: 0 }
        );
        assert_eq!(
            centre_tap(&idle, 250, true, None),
            CentreTap::Play { from_ms: 250 }
        );
        let paused = PlayView {
            active: true,
            ..idle
        };
        assert_eq!(centre_tap(&paused, 0, true, None), CentreTap::Resume);
        let on = PlayView {
            playing: true,
            ..paused
        };
        assert_eq!(
            centre_tap(&on, 0, true, Some(BUSY_RECORDING)),
            CentreTap::Pause
        );
        assert_eq!(
            centre_tap(&idle, 0, true, Some(BUSY_CALL)),
            CentreTap::Refuse(BUSY_CALL)
        );
        assert_eq!(centre_tap(&idle, 0, false, None), CentreTap::Nothing);
    }

    #[test]
    fn play_is_refused_while_recording_or_in_a_call() {
        let mut snap = UiSnapshot::default();
        assert_eq!(play_guard(&snap), None);
        snap = UiSnapshot {
            audio: AudioState {
                call_off_hook: true,
                ..AudioState::default()
            },
            ..UiSnapshot::default()
        };
        assert_eq!(play_guard(&snap), Some(BUSY_CALL));
        snap = UiSnapshot {
            audio: AudioState {
                call_off_hook: true,
                ..AudioState::default()
            },
            meeting: MeetingState {
                recording: true,
                ..MeetingState::default()
            },
            ..UiSnapshot::default()
        };
        assert_eq!(play_guard(&snap), Some(BUSY_RECORDING));
    }

    #[test]
    fn the_right_button_follows_the_upload_state() {
        use RecordingStatus::*;
        assert_eq!(send_action(LocalOnly, true), Some(SendAction::Send));
        assert_eq!(send_action(Failed, true), Some(SendAction::SendAgain));
        for status in [Uploaded, Waiting, Uploading, Damaged, AwaitingDecision] {
            assert_eq!(send_action(status, true), None, "{status:?}");
        }
        assert_eq!(
            send_action(Failed, false),
            None,
            "no server, nothing to send to"
        );
        assert_eq!(
            (SendAction::Send.icon(), SendAction::Send.label()),
            (icons::SEND, "Send")
        );
        assert_eq!(
            (SendAction::SendAgain.icon(), SendAction::SendAgain.label()),
            (icons::REFRESH, "Send again")
        );
    }

    #[test]
    fn the_note_names_the_state_or_the_failure() {
        assert_eq!(
            note_line(&failed(), true),
            ("Upload failed: server didn't answer".into(), WARN)
        );
        let mut bare = failed();
        bare.last_error = Some("  ".into());
        assert_eq!(note_line(&bare, true), ("Upload failed".into(), WARN));
        let mut damaged = failed();
        damaged.status = RecordingStatus::Damaged;
        damaged.last_error = Some("audio file missing".into());
        assert_eq!(
            note_line(&damaged, true),
            ("File damaged: audio file missing".into(), WARN)
        );
        assert!(!playable(&damaged));
        let mut kept = failed();
        kept.status = RecordingStatus::LocalOnly;
        assert_eq!(note_line(&kept, true), ("Kept on Wave".into(), TEXT_2));
        kept.status = RecordingStatus::Waiting;
        assert_eq!(
            note_line(&kept, false),
            ("Waiting for Wi-Fi".into(), TEXT_2)
        );
        assert!(playable(&kept));
        kept.duration_secs = 0;
        assert!(!playable(&kept), "an empty file has nothing to play");
    }
}
