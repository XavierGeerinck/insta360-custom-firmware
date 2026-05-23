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
        self.close_active(0)
    }

    pub(crate) fn on_confident_frame(
        &mut self,
        speaker_id: u32,
        az_deg: f32,
        rel_ms: u32,
    ) -> Option<DiarizationSegment> {
        match self.active.as_mut() {
            Some(seg) if seg.speaker_id == speaker_id => {
                seg.last_voice_ms = rel_ms;
                seg.voice_frame_count += 1;
                None
            }
            Some(_) => {
                let closed = self.close_active(rel_ms);
                self.active = Some(ActiveSegment {
                    speaker_id,
                    start_ms: rel_ms,
                    last_voice_ms: rel_ms,
                    voice_frame_count: 1,
                    centroid_at_open_deg: az_deg,
                });
                closed
            }
            None => {
                self.active = Some(ActiveSegment {
                    speaker_id,
                    start_ms: rel_ms,
                    last_voice_ms: rel_ms,
                    voice_frame_count: 1,
                    centroid_at_open_deg: az_deg,
                });
                None
            }
        }
    }

    pub(crate) fn on_silent_frame(&mut self, rel_ms: u32) -> Option<DiarizationSegment> {
        let active = self.active.as_ref()?;
        let gap_ms = rel_ms.saturating_sub(active.last_voice_ms);
        if gap_ms > MAX_SEGMENT_SILENCE_MS {
            self.close_active(rel_ms)
        } else {
            None
        }
    }

    /// Close the active segment. Returns Some only if the segment's
    /// total voice-frame duration ≥ MIN_SEGMENT_DURATION_MS. The
    /// `_now_ms` parameter exists for API symmetry / future use; not
    /// currently consumed (end_ms = last_voice_ms, not now).
    pub(crate) fn close_active(&mut self, _now_ms: u32) -> Option<DiarizationSegment> {
        let seg = self.active.take()?;
        let speech_ms = seg.voice_frame_count * PERIOD_MS;
        if speech_ms < MIN_SEGMENT_DURATION_MS {
            return None;
        }
        Some(DiarizationSegment {
            meeting_id: self.meeting_id,
            start_ms: seg.start_ms,
            end_ms: seg.last_voice_ms,
            speaker_id: seg.speaker_id,
            azimuth_deg: seg.centroid_at_open_deg,
        })
    }

    /// Returns the speaker ID for this azimuth, creating a new cluster if
    /// nothing is within CLUSTER_TOLERANCE_DEG. Updates the matched
    /// cluster's centroid via running-mean over the unit circle (handles
    /// wraparound: 350° + 10° → 0°, not 180°).
    ///
    /// If the cluster Vec is at MAX_CLUSTERS and the new azimuth is outside
    /// tolerance of all existing clusters, the frame is absorbed into the
    /// nearest existing cluster (does NOT spawn a new ID).
    pub(crate) fn assign_cluster(&mut self, az_deg: f32) -> u32 {
        // Find nearest existing cluster (circular distance).
        let mut best_id: Option<u32> = None;
        let mut best_dist = f32::INFINITY;
        for c in &self.clusters {
            let raw = (c.centroid_deg - az_deg).abs();
            let dist = raw.min(360.0 - raw);
            if dist < best_dist {
                best_dist = dist;
                best_id = Some(c.speaker_id);
            }
        }
        if best_dist <= CLUSTER_TOLERANCE_DEG {
            let id = best_id.expect("if best_dist is finite, best_id is Some");
            let c = self.clusters
                .iter_mut()
                .find(|c| c.speaker_id == id)
                .expect("speaker_id we just found must exist");
            c.frame_count += 1;
            // Running mean over unit-circle vectors → atan2 back to degrees.
            let n = c.frame_count as f32;
            let (cs, sn) = (c.centroid_deg.to_radians().cos(),
                            c.centroid_deg.to_radians().sin());
            let (fc, fs) = (az_deg.to_radians().cos(), az_deg.to_radians().sin());
            let new_cs = (cs * (n - 1.0) + fc) / n;
            let new_sn = (sn * (n - 1.0) + fs) / n;
            let new_az = new_sn.atan2(new_cs).to_degrees();
            c.centroid_deg = if new_az < 0.0 { new_az + 360.0 } else { new_az };
            id
        } else if self.clusters.len() < MAX_CLUSTERS {
            let id = self.next_speaker_id;
            self.next_speaker_id += 1;
            self.clusters.push(Cluster {
                speaker_id: id,
                centroid_deg: az_deg,
                frame_count: 1,
            });
            id
        } else {
            // Cluster overflow — absorb into nearest existing cluster.
            // best_id is guaranteed Some because we have ≥ 1 cluster
            // (MAX_CLUSTERS > 0); cluster overflow only triggers when
            // self.clusters.len() ≥ MAX_CLUSTERS.
            best_id.unwrap_or(1)
        }
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

    #[test]
    fn assign_cluster_creates_first_speaker() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        let id = d.assign_cluster(45.0);
        assert_eq!(id, 1);
        assert_eq!(d.clusters.len(), 1);
        assert!((d.clusters[0].centroid_deg - 45.0).abs() < 0.01);
    }

    #[test]
    fn assign_cluster_merges_within_tolerance() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        let id1 = d.assign_cluster(45.0);
        let id2 = d.assign_cluster(60.0); // 15° away, within ±30°
        assert_eq!(id1, id2);
        assert_eq!(d.clusters.len(), 1);
    }

    #[test]
    fn assign_cluster_splits_outside_tolerance() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        let id1 = d.assign_cluster(45.0);
        let id2 = d.assign_cluster(180.0);
        assert_ne!(id1, id2);
        assert_eq!(d.clusters.len(), 2);
        assert_eq!(d.next_speaker_id, 3);
    }

    #[test]
    fn assign_cluster_wraparound_350_and_10_merges() {
        // 350° and 10° are 20° apart (circular), within ±30° tolerance.
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        let id1 = d.assign_cluster(350.0);
        let id2 = d.assign_cluster(10.0);
        assert_eq!(id1, id2, "wraparound merge failed: {} vs {}", id1, id2);
        assert_eq!(d.clusters.len(), 1);
    }

    #[test]
    fn assign_cluster_wraparound_centroid_stays_near_zero() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        // Alternate 355° and 5° — centroid should converge to ~0°, not 180°.
        for _ in 0..15 {
            d.assign_cluster(355.0);
            d.assign_cluster(5.0);
        }
        let c = &d.clusters[0];
        let raw = (c.centroid_deg - 0.0).abs();
        let dist = raw.min(360.0 - raw);
        assert!(dist < 5.0,
            "centroid {} should be near 0° (lerped through 180° would be ~180°)",
            c.centroid_deg);
    }

    #[test]
    fn assign_cluster_overflow_absorbs_into_nearest() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        // Pre-seed MAX_CLUSTERS=20 clusters spread evenly around the circle.
        // Using direct insertion since uniform spacing (360°/20 = 18°) is
        // narrower than CLUSTER_TOLERANCE_DEG (30°), so assign_cluster alone
        // cannot produce 20 distinct clusters from 20 sequential calls.
        for i in 0..MAX_CLUSTERS as u32 {
            d.clusters.push(Cluster {
                speaker_id: i + 1,
                centroid_deg: i as f32 * (360.0 / MAX_CLUSTERS as f32),
                frame_count: 1,
            });
        }
        d.next_speaker_id = MAX_CLUSTERS as u32 + 1;
        assert_eq!(d.clusters.len(), MAX_CLUSTERS);
        let before_next = d.next_speaker_id;
        // 21st cluster attempt at a far-off azimuth — should NOT create new.
        let id = d.assign_cluster(900.0 % 360.0); // 180°
        assert!(d.clusters.len() <= MAX_CLUSTERS);
        assert_eq!(d.next_speaker_id, before_next, "no new speaker_id consumed");
        // Returned ID is one of the existing clusters (the nearest).
        assert!(d.clusters.iter().any(|c| c.speaker_id == id),
            "returned id={} should be an existing cluster", id);
    }

    fn high(ms: u64, az: f32) -> DoaFrame {
        DoaFrame {
            frame_idx: 0,
            timestamp_ms: ms,
            azimuth_deg: az,
            elevation_deg: 0.0,
            confidence: 0.8,
            vad: true,
            backend: waved_audio::doa::DoaBackend::GccPhat,
        }
    }

    #[test]
    fn close_active_returns_none_when_no_active() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        assert!(d.close_active(0).is_none());
    }

    #[test]
    fn close_active_drops_short_segments() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        // Manually craft an active segment with only 5 frames * 21ms = 105ms,
        // below the 200ms minimum.
        d.active = Some(ActiveSegment {
            speaker_id: 1,
            start_ms: 0,
            last_voice_ms: 100,
            voice_frame_count: 5,
            centroid_at_open_deg: 45.0,
        });
        assert!(d.close_active(500).is_none());
        assert!(d.active.is_none(), "active should be cleared even when dropped");
    }

    #[test]
    fn close_active_emits_segment_when_long_enough() {
        let mid = uuid::Uuid::new_v4();
        let mut d = Diarizer::new(mid);
        // 10 frames * 21ms = 210ms ≥ 200ms minimum.
        d.active = Some(ActiveSegment {
            speaker_id: 1,
            start_ms: 0,
            last_voice_ms: 210,
            voice_frame_count: 10,
            centroid_at_open_deg: 45.0,
        });
        let seg = d.close_active(500).expect("should emit segment");
        assert_eq!(seg.meeting_id, mid);
        assert_eq!(seg.speaker_id, 1);
        assert_eq!(seg.start_ms, 0);
        assert_eq!(seg.end_ms, 210);
        assert!((seg.azimuth_deg - 45.0).abs() < 0.01);
        assert!(d.active.is_none());
    }

    #[test]
    fn on_confident_frame_opens_first_segment() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        let out = d.on_confident_frame(1, 45.0, 0);
        assert!(out.is_none(), "first frame should not emit (segment is just opening)");
        let seg = d.active.as_ref().expect("segment should be open");
        assert_eq!(seg.speaker_id, 1);
        assert_eq!(seg.start_ms, 0);
        assert_eq!(seg.voice_frame_count, 1);
    }

    #[test]
    fn on_confident_frame_extends_same_speaker() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        d.on_confident_frame(1, 45.0, 0);
        let out = d.on_confident_frame(1, 45.0, 21);
        assert!(out.is_none());
        let seg = d.active.as_ref().unwrap();
        assert_eq!(seg.voice_frame_count, 2);
        assert_eq!(seg.last_voice_ms, 21);
    }

    #[test]
    fn on_confident_frame_speaker_change_emits_and_opens_new() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        // Build up a long-enough segment for speaker 1.
        for i in 0..10 {
            d.on_confident_frame(1, 45.0, i * 21);
        }
        // Now speaker 2 arrives.
        let out = d.on_confident_frame(2, 180.0, 10 * 21);
        let seg = out.expect("should emit closed segment for speaker 1");
        assert_eq!(seg.speaker_id, 1);
        let new = d.active.as_ref().unwrap();
        assert_eq!(new.speaker_id, 2);
        assert_eq!(new.start_ms, 10 * 21);
    }

    #[test]
    fn on_silent_frame_returns_none_when_no_active() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        assert!(d.on_silent_frame(100).is_none());
    }

    #[test]
    fn on_silent_frame_under_threshold_keeps_active() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        for i in 0..10 {
            d.on_confident_frame(1, 45.0, i * 21);
        }
        // Silence at +300ms after last voice (189) → gap = 111ms, under 500ms.
        let out = d.on_silent_frame(300);
        assert!(out.is_none());
        assert!(d.active.is_some());
    }

    #[test]
    fn on_silent_frame_over_threshold_closes() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        for i in 0..10 {
            d.on_confident_frame(1, 45.0, i * 21);
        }
        // last_voice_ms = 9*21 = 189. Silence at 800 → gap = 611 > 500.
        let out = d.on_silent_frame(800);
        let seg = out.expect("should close segment");
        assert_eq!(seg.speaker_id, 1);
        assert_eq!(seg.end_ms, 189, "end_ms should be last_voice_ms, not the silence time");
        assert!(d.active.is_none());
    }

    #[test]
    fn finalize_emits_open_segment() {
        let mut d = Diarizer::new(uuid::Uuid::new_v4());
        for i in 0..10 {
            d.on_confident_frame(1, 45.0, i * 21);
        }
        let seg = d.finalize().expect("should flush");
        assert_eq!(seg.speaker_id, 1);
    }
}
