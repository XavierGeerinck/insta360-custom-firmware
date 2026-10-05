use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use tokio::sync::oneshot;
use waved_state::RecordingIntegrityFault;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureExit {
    StopRequested,
    EndOfStream,
    RawFrameChannelClosed,
    FatalRead,
}

impl CaptureExit {
    pub const fn fault(self) -> Option<RecordingIntegrityFault> {
        Some(match self {
            Self::StopRequested => return None,
            Self::EndOfStream => RecordingIntegrityFault::CaptureEnded,
            Self::RawFrameChannelClosed => RecordingIntegrityFault::RawFrameChannelClosed,
            Self::FatalRead => RecordingIntegrityFault::CaptureReadFailed,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioServiceExit {
    StopRequested,
    RawFrameChannelClosed,
    ProcessedFrameChannelClosed,
}

impl AudioServiceExit {
    pub const fn fault(self) -> Option<RecordingIntegrityFault> {
        Some(match self {
            Self::StopRequested => return None,
            Self::RawFrameChannelClosed => RecordingIntegrityFault::RawFrameChannelClosed,
            Self::ProcessedFrameChannelClosed => {
                RecordingIntegrityFault::ProcessedFrameChannelClosed
            }
        })
    }
}

#[derive(Debug)]
pub struct IntegritySignal {
    pub fault: RecordingIntegrityFault,
    pub acknowledgement: Option<oneshot::Sender<()>>,
}

impl IntegritySignal {
    pub const fn new(fault: RecordingIntegrityFault) -> Self {
        Self {
            fault,
            acknowledgement: None,
        }
    }

    pub const fn with_acknowledgement(
        fault: RecordingIntegrityFault,
        acknowledgement: oneshot::Sender<()>,
    ) -> Self {
        Self {
            fault,
            acknowledgement: Some(acknowledgement),
        }
    }
}

#[derive(Debug, Default)]
pub struct RecordingIntegrity {
    epoch: AtomicU64,
    last_fault: AtomicU8,
    xruns: AtomicU64,
    raw_frames_dropped: AtomicU64,
    processed_frames_dropped: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecordingIntegritySnapshot {
    pub epoch: u64,
    pub last_fault: Option<RecordingIntegrityFault>,
    pub xruns: u64,
    pub raw_frames_dropped: u64,
    pub processed_frames_dropped: u64,
}

impl RecordingIntegrity {
    /// Count `fault` and move the epoch on by one; returns the new epoch.
    ///
    /// Contract with the meeting service: every call made by the audio
    /// pipeline (capture thread, `AudioService`, the binary's watchers) is
    /// followed by exactly one [`IntegritySignal`] for that fault. The
    /// meeting service depends on it: a signal tolerated while a take is
    /// paused accepts exactly one fault (the take's epoch moves on by one),
    /// and Pause is refused while the epoch is ahead of the take, because
    /// the signal still on its way will end the take. A fault recorded
    /// without a signal would keep that take from ever pausing; one signal
    /// for two faults would let a capture failure through as a pause fault.
    /// The meeting service's own calls end the take on the spot and send no
    /// signal.
    pub fn record_fault(&self, fault: RecordingIntegrityFault) -> u64 {
        match fault {
            RecordingIntegrityFault::CaptureXrun => {
                self.xruns.fetch_add(1, Ordering::Relaxed);
            }
            RecordingIntegrityFault::RawFrameDropped => {
                self.raw_frames_dropped.fetch_add(1, Ordering::Relaxed);
            }
            RecordingIntegrityFault::ProcessedFrameDropped => {
                self.processed_frames_dropped
                    .fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        self.last_fault.store(fault as u8, Ordering::Relaxed);
        self.epoch.fetch_add(1, Ordering::Release) + 1
    }

    pub fn snapshot(&self) -> RecordingIntegritySnapshot {
        let epoch = self.epoch.load(Ordering::Acquire);
        RecordingIntegritySnapshot {
            epoch,
            last_fault: RecordingIntegrityFault::from_code(self.last_fault.load(Ordering::Relaxed)),
            xruns: self.xruns.load(Ordering::Relaxed),
            raw_frames_dropped: self.raw_frames_dropped.load(Ordering::Relaxed),
            processed_frames_dropped: self.processed_frames_dropped.load(Ordering::Relaxed),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    pub fn last_fault(&self) -> Option<RecordingIntegrityFault> {
        let _ = self.epoch.load(Ordering::Acquire);
        RecordingIntegrityFault::from_code(self.last_fault.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_fault_publishes_kind_before_new_epoch() {
        let integrity = RecordingIntegrity::default();
        assert_eq!(integrity.snapshot(), RecordingIntegritySnapshot::default());

        assert_eq!(
            integrity.record_fault(RecordingIntegrityFault::RawFrameDropped),
            1
        );
        assert_eq!(
            integrity.record_fault(RecordingIntegrityFault::CaptureXrun),
            2
        );
        assert_eq!(
            integrity.record_fault(RecordingIntegrityFault::ProcessedFrameDropped),
            3
        );

        assert_eq!(
            integrity.snapshot(),
            RecordingIntegritySnapshot {
                epoch: 3,
                last_fault: Some(RecordingIntegrityFault::ProcessedFrameDropped),
                xruns: 1,
                raw_frames_dropped: 1,
                processed_frames_dropped: 1,
            }
        );
    }

    #[test]
    fn normal_exits_do_not_map_to_faults() {
        assert_eq!(CaptureExit::StopRequested.fault(), None);
        assert_eq!(AudioServiceExit::StopRequested.fault(), None);
        assert_eq!(
            CaptureExit::FatalRead.fault(),
            Some(RecordingIntegrityFault::CaptureReadFailed)
        );
        assert_eq!(
            AudioServiceExit::ProcessedFrameChannelClosed.fault(),
            Some(RecordingIntegrityFault::ProcessedFrameChannelClosed)
        );
    }
}
