use std::collections::VecDeque;
use uuid::Uuid;
use waved_state::DeviceEvent;

const MAX_TRACKED_RECORDINGS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UploadToast {
    Uploaded,
    Deferred,
    NeedsAttention,
}

impl UploadToast {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Uploaded => "Recording uploaded",
            Self::Deferred => "Saved - upload queued",
            Self::NeedsAttention => "Saved - check Webhook",
        }
    }

    pub const fn duration_ms(self) -> u64 {
        match self {
            Self::Uploaded => 2_000,
            Self::Deferred => 2_500,
            Self::NeedsAttention => 3_000,
        }
    }
}

#[derive(Debug, Default)]
pub struct UploadNoticeTracker {
    ids: VecDeque<Uuid>,
}

impl UploadNoticeTracker {
    pub fn track(&mut self, id: Uuid) {
        if let Some(position) = self.ids.iter().position(|tracked| *tracked == id) {
            self.ids.remove(position);
        }
        if self.ids.len() == MAX_TRACKED_RECORDINGS {
            self.ids.pop_front();
        }
        self.ids.push_back(id);
    }

    pub fn handle(&mut self, event: &DeviceEvent) -> Option<UploadToast> {
        let (id, toast) = match event {
            DeviceEvent::RecordingStopped(id) => {
                self.track(*id);
                return None;
            }
            DeviceEvent::RecordingUploadSucceeded(id) => (*id, UploadToast::Uploaded),
            DeviceEvent::RecordingUploadDeferred(id) => (*id, UploadToast::Deferred),
            DeviceEvent::RecordingUploadNeedsAttention(id) => (*id, UploadToast::NeedsAttention),
            _ => return None,
        };
        let position = self.ids.iter().position(|tracked| *tracked == id)?;
        if toast == UploadToast::Uploaded {
            self.ids.remove(position);
        }
        Some(toast)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_matching_outcomes_and_removes_only_success() {
        let mut tracker = UploadNoticeTracker::default();
        let id = Uuid::new_v4();
        tracker.track(id);

        assert_eq!(
            tracker.handle(&DeviceEvent::RecordingUploadDeferred(id)),
            Some(UploadToast::Deferred)
        );
        assert_eq!(
            tracker.handle(&DeviceEvent::RecordingUploadNeedsAttention(id)),
            Some(UploadToast::NeedsAttention)
        );
        assert_eq!(
            tracker.handle(&DeviceEvent::RecordingUploadSucceeded(id)),
            Some(UploadToast::Uploaded)
        );
        assert_eq!(
            tracker.handle(&DeviceEvent::RecordingUploadSucceeded(id)),
            None
        );
    }

    #[test]
    fn ignores_unrelated_backlog() {
        let mut tracker = UploadNoticeTracker::default();
        tracker.track(Uuid::new_v4());
        assert_eq!(
            tracker.handle(&DeviceEvent::RecordingUploadSucceeded(Uuid::new_v4())),
            None
        );
    }

    #[test]
    fn recording_stopped_event_tracks_when_command_state_was_already_cleared() {
        let mut tracker = UploadNoticeTracker::default();
        let id = Uuid::new_v4();
        assert_eq!(tracker.handle(&DeviceEvent::RecordingStopped(id)), None);
        assert_eq!(
            tracker.handle(&DeviceEvent::RecordingUploadSucceeded(id)),
            Some(UploadToast::Uploaded)
        );
    }

    #[test]
    fn evicts_the_oldest_recording_at_the_bound() {
        let mut tracker = UploadNoticeTracker::default();
        let first = Uuid::new_v4();
        tracker.track(first);
        for _ in 1..MAX_TRACKED_RECORDINGS {
            tracker.track(Uuid::new_v4());
        }
        let newest = Uuid::new_v4();
        tracker.track(newest);

        assert_eq!(
            tracker.handle(&DeviceEvent::RecordingUploadSucceeded(first)),
            None
        );
        assert_eq!(
            tracker.handle(&DeviceEvent::RecordingUploadSucceeded(newest)),
            Some(UploadToast::Uploaded)
        );
    }

    #[test]
    fn duplicate_tracking_does_not_consume_capacity() {
        let mut tracker = UploadNoticeTracker::default();
        let id = Uuid::new_v4();
        tracker.track(id);
        tracker.track(id);
        assert_eq!(tracker.ids.len(), 1);
    }

    #[test]
    fn text_is_ascii_and_matches_the_approved_copy() {
        for (toast, message, duration) in [
            (UploadToast::Uploaded, "Recording uploaded", 2_000),
            (UploadToast::Deferred, "Saved - upload queued", 2_500),
            (UploadToast::NeedsAttention, "Saved - check Webhook", 3_000),
        ] {
            assert_eq!(toast.message(), message);
            assert!(message.is_ascii());
            assert_eq!(toast.duration_ms(), duration);
        }
    }
}
