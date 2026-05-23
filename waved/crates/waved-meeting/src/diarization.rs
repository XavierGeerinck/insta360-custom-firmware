//! DoA-based diarization state machine. See
//! `docs/superpowers/specs/2026-05-23-waved-diarization-design.md`.
//!
//! Pure synchronous Rust — no async, no I/O. Consumes `DoaFrame`s (one per
//! ALSA period), emits `Option<DiarizationSegment>` per call. Cluster +
//! segment state is per-meeting; reset on `Diarizer::new`.

use waved_audio::doa::DoaFrame;

/// Two DoA frames within this angular distance (degrees, circular) are
/// the same speaker. ±30° per spec §3 — permissive; merges close speakers.
pub const CLUSTER_TOLERANCE_DEG: f32 = 30.0;

/// Minimum DoaFrame confidence to count as a high-conf vote. Matches
/// SteeredBeam's threshold so behavior is consistent across F2/F3.
pub const CONFIDENCE_THRESHOLD: f32 = 0.30;

/// Continuous low/no-confidence frames spanning more than this duration
/// close the current segment.
pub const MAX_SEGMENT_SILENCE_MS: u32 = 500;

/// Segments whose total *voice-frame duration* falls below this are
/// dropped (throat-clears, single-word utterances).
pub const MIN_SEGMENT_DURATION_MS: u32 = 200;

/// ALSA period at 1024 samples / 48 kHz = 21.33 ms. Truncated to integer
/// ms for arithmetic; the 0.33 ms error per frame is well below the
/// segment-boundary tolerances.
pub const PERIOD_MS: u32 = 21;

/// Hard cap on cluster count per meeting. Beyond this, frames absorb
/// into the nearest existing cluster. Real meetings rarely > 10 people.
pub const MAX_CLUSTERS: usize = 20;

#[derive(Debug, Clone, Copy)]
pub struct DiarizationSegment {
    pub meeting_id: uuid::Uuid,
    pub start_ms: u32,
    pub end_ms: u32,
    pub speaker_id: u32,
    pub azimuth_deg: f32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Cluster {
    pub speaker_id: u32,
    pub centroid_deg: f32,
    /// Number of high-confidence frames assigned. Drives the running-mean
    /// centroid update.
    pub frame_count: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ActiveSegment {
    pub speaker_id: u32,
    pub start_ms: u32,
    pub last_voice_ms: u32,
    pub voice_frame_count: u32,
    pub centroid_at_open_deg: f32,
}

pub struct Diarizer {
    pub(crate) meeting_id: uuid::Uuid,
    pub(crate) meeting_start_ms: u32,
    pub(crate) clusters: Vec<Cluster>,
    pub(crate) next_speaker_id: u32,
    pub(crate) active: Option<ActiveSegment>,
    /// False until the first frame seeds `meeting_start_ms`.
    pub(crate) primed: bool,
}

impl Diarizer {
    pub fn new(meeting_id: uuid::Uuid) -> Self {
        Self {
            meeting_id,
            meeting_start_ms: 0,
            clusters: Vec::with_capacity(8),
            next_speaker_id: 1,
            active: None,
            primed: false,
        }
    }

    /// Feed one DoA frame; returns `Some(segment)` when a segment closes
    /// AND its voice-frame duration >= MIN_SEGMENT_DURATION_MS. Filled in
    /// by Tasks 2-4.
    pub fn on_doa_frame(&mut self, _frame: &DoaFrame) -> Option<DiarizationSegment> {
        // Implementation lands in Task 4.
        None
    }

    /// Flush any open segment. Called from MeetingService on StopRecording.
    pub fn finalize(&mut self) -> Option<DiarizationSegment> {
        // Implementation lands in Task 3 (close_active).
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diarizer_new_starts_with_speaker_id_1() {
        let id = uuid::Uuid::new_v4();
        let d = Diarizer::new(id);
        assert_eq!(d.meeting_id, id);
        assert_eq!(d.next_speaker_id, 1);
        assert!(d.clusters.is_empty());
        assert!(d.active.is_none());
        assert!(!d.primed);
    }

    #[test]
    fn diarizer_finalize_on_fresh_returns_none() {
        let d = &mut Diarizer::new(uuid::Uuid::new_v4());
        assert!(d.finalize().is_none());
    }
}
