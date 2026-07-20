use waved_state::DeviceEvent;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrityToast {
    RecordingInterrupted,
    AudioRestarting,
    RecordingUnavailable,
}

impl IntegrityToast {
    pub const fn message(self) -> &'static str {
        match self {
            Self::RecordingInterrupted => "Recording interrupted",
            Self::AudioRestarting => "Audio restarting...",
            Self::RecordingUnavailable => "Recording unavailable",
        }
    }

    pub const fn duration_ms(self) -> u64 {
        match self {
            Self::RecordingInterrupted => 3_000,
            Self::AudioRestarting => 2_500,
            Self::RecordingUnavailable => 2_500,
        }
    }
}

pub fn for_event(event: &DeviceEvent) -> Option<IntegrityToast> {
    let DeviceEvent::RecordingIntegrityFailed { meeting_id, fault } = event else {
        return None;
    };
    if meeting_id.is_some() {
        Some(IntegrityToast::RecordingInterrupted)
    } else if fault.requires_restart() {
        Some(IntegrityToast::AudioRestarting)
    } else if fault.blocks_recording() {
        Some(IntegrityToast::RecordingUnavailable)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use waved_state::RecordingIntegrityFault;

    #[test]
    fn active_take_fault_is_always_an_interruption() {
        let event = DeviceEvent::RecordingIntegrityFailed {
            meeting_id: Some(Uuid::new_v4()),
            fault: RecordingIntegrityFault::WavWriteFailed,
        };
        assert_eq!(
            for_event(&event),
            Some(IntegrityToast::RecordingInterrupted)
        );
    }

    #[test]
    fn idle_fatal_fault_announces_restart() {
        let event = DeviceEvent::RecordingIntegrityFailed {
            meeting_id: None,
            fault: RecordingIntegrityFault::CaptureReadFailed,
        };
        assert_eq!(for_event(&event), Some(IntegrityToast::AudioRestarting));
    }

    #[test]
    fn storage_fault_announces_unavailable() {
        let event = DeviceEvent::RecordingIntegrityFailed {
            meeting_id: None,
            fault: RecordingIntegrityFault::WavWriteFailed,
        };
        assert_eq!(
            for_event(&event),
            Some(IntegrityToast::RecordingUnavailable)
        );
    }
}
