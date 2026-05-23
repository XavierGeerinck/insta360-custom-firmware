# waved DoA-Based Diarization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Per-meeting numeric speaker IDs derived from the F2 DoaFrame stream, persisted to a new SQLite `diarization_segments` table.

**Architecture:** Single new module `crates/waved-meeting/src/diarization.rs` exposing a `Diarizer` state machine. `MeetingService::run` gains a `tokio::select!` arm for the broadcast `Receiver<DoaFrame>` subscribed in `main.rs`. Per-frame: feed Diarizer, insert any returned segment via a new `db::insert_diarization_segment`. State resets per meeting.

**Tech Stack:** Rust, `sqlx` (SQLite), `tokio::sync::broadcast`. No new crate dependencies — `waved-audio` already exports the needed types via T6 of F2.

**Spec reference:** `docs/superpowers/specs/2026-05-23-waved-diarization-design.md`

---

## Task 1: `Diarizer` skeleton — types, constants, `new()`, `finalize()` stub

**Files:**
- Create: `waved/crates/waved-meeting/src/diarization.rs`
- Modify: `waved/crates/waved-meeting/src/lib.rs` (add `pub mod diarization;`)

This task lays down the scaffolding: type definitions, module-level constants, public `new()`, and a `finalize()` stub returning `None`. Subsequent tasks fill in `on_doa_frame` and the close-segment logic.

- [ ] **Step 1: Write failing test for skeleton existence**

Create `waved/crates/waved-meeting/src/diarization.rs` with empty content (we'll fill it in Step 3). Then add a test FILE at the bottom of the same file:

```rust
//! Stub — Task 1 of F3 diarization. Real content lands in Step 3.
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
```

Also append to `waved/crates/waved-meeting/src/lib.rs` at the top, alongside the existing `pub mod` lines:

```rust
pub mod db;
pub mod diarization;
pub mod recorder;
pub mod upload;
```

- [ ] **Step 2: Run to verify failure**

Run: `cd waved && cargo test -p waved-meeting --lib diarization`
Expected: FAIL — "cannot find type/struct `Diarizer`".

- [ ] **Step 3: Write the skeleton**

Replace `waved/crates/waved-meeting/src/diarization.rs` with the full skeleton:

```rust
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
    /// AND its voice-frame duration ≥ MIN_SEGMENT_DURATION_MS. Filled in
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
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cd waved && cargo test -p waved-meeting --lib diarization`
Expected: PASS — 2 tests in `diarization::tests`.

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-meeting/src/diarization.rs crates/waved-meeting/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(diarization): skeleton — types, constants, new/finalize stubs

New diarization.rs module under waved-meeting. Constants
(CLUSTER_TOLERANCE_DEG=30, CONFIDENCE_THRESHOLD=0.30, etc.) match
the F3 spec §3 verbatim. Diarizer struct + DiarizationSegment +
crate-private Cluster / ActiveSegment helpers.

on_doa_frame and finalize land in Tasks 2-4. This task is pure
scaffolding so the type names are stable for subsequent tasks.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `assign_cluster` — circular distance + running-mean centroid

**Files:**
- Modify: `waved/crates/waved-meeting/src/diarization.rs`

Adds the cluster-matching/creation private method. Includes wrap-around correctness (e.g., az=350° and az=10° are 20° apart, not 340°).

- [ ] **Step 1: Write failing tests**

Append inside `mod tests` in `waved/crates/waved-meeting/src/diarization.rs`:

```rust
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
    // 20 clusters at 15° intervals → fills MAX_CLUSTERS=20.
    // (5° step would also work; 15° avoids the ±30° merge.)
    for i in 0..MAX_CLUSTERS as u32 {
        d.assign_cluster(i as f32 * 17.0);
    }
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
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cd waved && cargo test -p waved-meeting --lib diarization::tests::assign_cluster`
Expected: FAIL — "no method named `assign_cluster`".

- [ ] **Step 3: Implement `assign_cluster`**

Inside `impl Diarizer { ... }` in `diarization.rs`, add the method (place it after `finalize`):

```rust
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
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cd waved && cargo test -p waved-meeting --lib diarization`
Expected: PASS — 2 (Task 1) + 6 (Task 2) = 8 tests.

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-meeting/src/diarization.rs
git commit -m "$(cat <<'EOF'
feat(diarization): assign_cluster with wraparound running-mean centroid

Circular distance for cluster matching (350° + 10° = 20° apart, not
340°). Running-mean centroid update over unit-circle vectors so a
cluster straddling 0° converges to ~0°, not 180°. Hard cap at
MAX_CLUSTERS = 20; overflow absorbs into nearest existing cluster
without consuming a new speaker_id.

6 unit tests: first-speaker, within-tolerance merge, outside-
tolerance split, wraparound merge, wraparound centroid stability,
overflow absorption.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Segment lifecycle — `on_confident_frame`, `on_silent_frame`, `close_active`

**Files:**
- Modify: `waved/crates/waved-meeting/src/diarization.rs`

Adds the segment open/close mechanics. The `finalize()` stub from Task 1 now calls `close_active`.

- [ ] **Step 1: Write failing tests**

Append inside `mod tests` in `diarization.rs`:

```rust
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
    // Silence at +300ms after last voice (210) → gap = 90ms, under 500ms.
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cd waved && cargo test -p waved-meeting --lib diarization::tests::close_active`
Expected: FAIL — "no method named `close_active`".

- [ ] **Step 3: Implement the three methods + wire finalize**

Inside `impl Diarizer { ... }`, replace the stub `finalize()` and add three new methods:

```rust
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
```

- [ ] **Step 4: Run tests**

Run: `cd waved && cargo test -p waved-meeting --lib diarization`
Expected: PASS — 8 (T1+T2) + 10 (T3) = 18 tests.

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-meeting/src/diarization.rs
git commit -m "$(cat <<'EOF'
feat(diarization): segment lifecycle — open/close/finalize

on_confident_frame opens new segments on first frame, extends same-
speaker frames, and closes-then-opens on speaker change. on_silent_frame
closes when the gap since last_voice_ms exceeds 500ms. close_active
applies the 200ms voice-frame-duration minimum and emits the segment
with end_ms = last_voice_ms (not the close-decision time). finalize
flushes for StopRecording.

10 unit tests covering: empty close, short-segment drop, normal close,
first-frame open, same-speaker extend, speaker change, silence under/
over threshold, finalize.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `on_doa_frame` — full entry point + remaining edge-case tests

**Files:**
- Modify: `waved/crates/waved-meeting/src/diarization.rs`

Wires assign_cluster + on_confident_frame + on_silent_frame behind the public `on_doa_frame`. Adds the priming logic + the confidence/VAD/NaN gate. Adds the remaining edge-case tests from spec §5.1.

- [ ] **Step 1: Write failing end-to-end tests**

Append inside `mod tests`:

```rust
fn silent(ms: u64) -> DoaFrame {
    DoaFrame::silent(0, ms, waved_audio::doa::DoaBackend::GccPhat)
}

#[test]
fn single_speaker_one_segment() {
    let mut d = Diarizer::new(uuid::Uuid::new_v4());
    // 50 frames * 21ms = 1050ms at az=45°.
    for i in 0..50 {
        let out = d.on_doa_frame(&high(i * 21, 45.0));
        assert!(out.is_none(), "no segment should close mid-speech");
    }
    let seg = d.finalize().expect("finalize emits");
    assert_eq!(seg.speaker_id, 1);
    assert!((seg.azimuth_deg - 45.0).abs() < 5.0);
    assert_eq!(seg.start_ms, 0);
    assert!(seg.end_ms >= 800 && seg.end_ms <= 1100,
        "end_ms ≈ 1029 (49 * 21), got {}", seg.end_ms);
}

#[test]
fn speaker_change_emits_segment() {
    let mut d = Diarizer::new(uuid::Uuid::new_v4());
    // 500ms at az=45°.
    for i in 0..24 {
        d.on_doa_frame(&high(i * 21, 45.0));
    }
    // Switch to az=180°. The change-frame returns the closed speaker-1
    // segment.
    let closed = d.on_doa_frame(&high(24 * 21, 180.0));
    let seg1 = closed.expect("speaker-change emits");
    assert_eq!(seg1.speaker_id, 1);
    // Continue 500ms at 180°, then finalize for speaker 2.
    for i in 25..49 {
        d.on_doa_frame(&high(i * 21, 180.0));
    }
    let seg2 = d.finalize().expect("finalize speaker-2 segment");
    assert_eq!(seg2.speaker_id, 2);
}

#[test]
fn silence_under_500ms_keeps_segment_open() {
    let mut d = Diarizer::new(uuid::Uuid::new_v4());
    // 5 frames voice (105ms), 14 frames silence (~294ms), 5 frames voice.
    for i in 0..5 {
        d.on_doa_frame(&high(i * 21, 45.0));
    }
    for i in 5..19 {
        let out = d.on_doa_frame(&silent(i * 21));
        assert!(out.is_none(), "silence under 500ms shouldn't close");
    }
    for i in 19..24 {
        d.on_doa_frame(&high(i * 21, 45.0));
    }
    let seg = d.finalize().expect("one segment after finalize");
    assert!(seg.end_ms >= 23 * 21 - 10);
}

#[test]
fn silence_over_500ms_closes_segment() {
    let mut d = Diarizer::new(uuid::Uuid::new_v4());
    for i in 0..12 {
        d.on_doa_frame(&high(i * 21, 45.0));
    }
    // 30 frames silence ≈ 630ms (> 500ms threshold).
    let mut closed = None;
    for i in 12..42 {
        if let Some(s) = d.on_doa_frame(&silent(i * 21)) {
            closed = Some(s);
        }
    }
    let s = closed.expect("silence over 500ms should close");
    assert_eq!(s.speaker_id, 1);
    assert_eq!(s.end_ms, 11 * 21, "end_ms is last voice frame's ms");
}

#[test]
fn segment_under_200ms_dropped() {
    let mut d = Diarizer::new(uuid::Uuid::new_v4());
    // 5 frames * 21ms = 105ms, below 200ms minimum.
    for i in 0..5 {
        d.on_doa_frame(&high(i * 21, 45.0));
    }
    assert!(d.finalize().is_none(), "short segment should be dropped");
}

#[test]
fn low_confidence_frame_treated_as_silence() {
    let mut d = Diarizer::new(uuid::Uuid::new_v4());
    for i in 0..12 {
        d.on_doa_frame(&high(i * 21, 45.0));
    }
    // Frames with vad=true but confidence below threshold.
    let mut closed = None;
    for i in 12..42 {
        let low = DoaFrame {
            frame_idx: 0, timestamp_ms: i * 21,
            azimuth_deg: 45.0, elevation_deg: 0.0,
            confidence: 0.1, vad: true,
            backend: waved_audio::doa::DoaBackend::GccPhat,
        };
        if let Some(s) = d.on_doa_frame(&low) {
            closed = Some(s);
        }
    }
    assert!(closed.is_some(), "low-confidence frames should be silence");
}

#[test]
fn nan_azimuth_treated_as_silence() {
    let mut d = Diarizer::new(uuid::Uuid::new_v4());
    for i in 0..12 {
        d.on_doa_frame(&high(i * 21, 45.0));
    }
    // VAD on, confidence high, but az is NaN (F2 emits this when the
    // solver fails mid-speech).
    let mut closed = None;
    for i in 12..42 {
        let nan = DoaFrame {
            frame_idx: 0, timestamp_ms: i * 21,
            azimuth_deg: f32::NAN, elevation_deg: f32::NAN,
            confidence: 0.8, vad: true,
            backend: waved_audio::doa::DoaBackend::GccPhat,
        };
        if let Some(s) = d.on_doa_frame(&nan) {
            closed = Some(s);
        }
    }
    assert!(closed.is_some(), "NaN azimuth must be treated as silence");
}

#[test]
fn first_frame_primes_meeting_start_ms() {
    let mut d = Diarizer::new(uuid::Uuid::new_v4());
    d.on_doa_frame(&high(1_000, 45.0)); // timestamp 1000ms
    assert!(d.primed);
    assert_eq!(d.meeting_start_ms, 1000);
    // Subsequent frames use rel_ms = timestamp - 1000.
    d.on_doa_frame(&high(1_021, 45.0));
    let active = d.active.as_ref().unwrap();
    assert_eq!(active.last_voice_ms, 21, "rel_ms = 1021 - 1000 = 21");
}

#[test]
fn lagged_synthesizes_gap() {
    let mut d = Diarizer::new(uuid::Uuid::new_v4());
    for i in 0..12 {
        d.on_doa_frame(&high(i * 21, 45.0));
    }
    // Simulate 1000ms of dropped frames by calling on_silent_frame
    // directly with an advanced timestamp.
    let closed = d.on_silent_frame(12 * 21 + 1_000);
    assert!(closed.is_some(), "lag-synthesized gap should close segment");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd waved && cargo test -p waved-meeting --lib diarization::tests::single_speaker_one_segment`
Expected: FAIL (returns None instead of segment because `on_doa_frame` is still the stub).

- [ ] **Step 3: Replace the stub `on_doa_frame` with the full implementation**

In `diarization.rs`, replace the existing `on_doa_frame` stub with:

```rust
    /// Feed one DoA frame; returns `Some(segment)` when a segment closes
    /// AND its voice-frame duration ≥ MIN_SEGMENT_DURATION_MS.
    pub fn on_doa_frame(&mut self, frame: &DoaFrame) -> Option<DiarizationSegment> {
        if !self.primed {
            self.meeting_start_ms = frame.timestamp_ms as u32;
            self.primed = true;
        }
        let rel_ms = (frame.timestamp_ms as u32).saturating_sub(self.meeting_start_ms);

        // Spec §3: confident = VAD on AND confidence above threshold AND
        // azimuth is finite (F2 emits NaN angles when the solver failed
        // mid-speech, even with VAD on).
        let confident = frame.vad
            && frame.confidence > CONFIDENCE_THRESHOLD
            && !frame.azimuth_deg.is_nan();

        if confident {
            let speaker_id = self.assign_cluster(frame.azimuth_deg);
            self.on_confident_frame(speaker_id, frame.azimuth_deg, rel_ms)
        } else {
            self.on_silent_frame(rel_ms)
        }
    }
```

- [ ] **Step 4: Run all diarization tests**

Run: `cd waved && cargo test -p waved-meeting --lib diarization`
Expected: PASS — 18 (T1-T3) + 9 (T4) = 27 tests.

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-meeting/src/diarization.rs
git commit -m "$(cat <<'EOF'
feat(diarization): on_doa_frame wires priming + cluster + segment

Full end-to-end Diarizer entry point: first-frame primes
meeting_start_ms (so rel_ms is meeting-local); confidence gate
combines VAD + threshold + NaN-rejection (F2's contract for
solver-fail-with-VAD); confident frames route through assign_cluster
then on_confident_frame; non-confident frames route to on_silent_frame.

9 new tests cover end-to-end paths: single speaker, speaker change,
silence under/over 500ms threshold, short-segment drop, low-confidence
as silence, NaN-azimuth as silence, priming arithmetic, lag-
synthesized gap.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: DB schema + `insert_diarization_segment` + cascade-delete update + tests

**Files:**
- Modify: `waved/crates/waved-meeting/src/db.rs`

Adds the `diarization_segments` table to `init_pool`, a typed insert function, an index, and extends `delete_meeting_cascade` so discarded meetings clean up their diarization rows too.

- [ ] **Step 1: Write failing tests**

Append to the existing `#[cfg(test)] mod tests` block in `waved/crates/waved-meeting/src/db.rs`:

```rust
    use crate::diarization::DiarizationSegment;

    #[tokio::test]
    async fn diarization_segments_table_is_created() {
        let pool = fresh_pool().await;
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='table' AND name='diarization_segments'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, 1, "diarization_segments table missing");
    }

    #[tokio::test]
    async fn insert_and_query_diarization_segment() {
        let pool = fresh_pool().await;
        let mid = Uuid::new_v4();
        insert_meeting(&pool, &mid, "Omni", "/tmp/x").await.unwrap();
        let seg = DiarizationSegment {
            meeting_id: mid,
            start_ms: 1000,
            end_ms: 3500,
            speaker_id: 2,
            azimuth_deg: 137.5,
        };
        insert_diarization_segment(&pool, &seg).await.unwrap();
        let rows: Vec<(i64, String, i64, i64, i64, f64)> = sqlx::query_as(
            "SELECT id, meeting_id, start_ms, end_ms, speaker_id, azimuth_deg \
             FROM diarization_segments WHERE meeting_id = ?",
        )
        .bind(mid.to_string())
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, mid.to_string());
        assert_eq!(rows[0].2, 1000);
        assert_eq!(rows[0].3, 3500);
        assert_eq!(rows[0].4, 2);
        assert!((rows[0].5 - 137.5).abs() < 0.001);
    }

    #[tokio::test]
    async fn multiple_meetings_isolated_in_diarization() {
        let pool = fresh_pool().await;
        let m1 = Uuid::new_v4();
        let m2 = Uuid::new_v4();
        insert_meeting(&pool, &m1, "Omni", "/tmp/x").await.unwrap();
        insert_meeting(&pool, &m2, "Omni", "/tmp/y").await.unwrap();
        for i in 0..3 {
            let seg = DiarizationSegment {
                meeting_id: m1,
                start_ms: i * 1000, end_ms: i * 1000 + 500,
                speaker_id: 1, azimuth_deg: 0.0,
            };
            insert_diarization_segment(&pool, &seg).await.unwrap();
        }
        let seg = DiarizationSegment {
            meeting_id: m2, start_ms: 0, end_ms: 1000,
            speaker_id: 1, azimuth_deg: 180.0,
        };
        insert_diarization_segment(&pool, &seg).await.unwrap();

        let cnt_m1: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM diarization_segments WHERE meeting_id = ?",
        )
        .bind(m1.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(cnt_m1.0, 3);
        let cnt_m2: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM diarization_segments WHERE meeting_id = ?",
        )
        .bind(m2.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(cnt_m2.0, 1);
    }

    #[tokio::test]
    async fn delete_meeting_cascade_removes_diarization() {
        let pool = fresh_pool().await;
        let mid = Uuid::new_v4();
        insert_meeting(&pool, &mid, "Omni", "/tmp/x").await.unwrap();
        let seg = DiarizationSegment {
            meeting_id: mid, start_ms: 0, end_ms: 500,
            speaker_id: 1, azimuth_deg: 0.0,
        };
        insert_diarization_segment(&pool, &seg).await.unwrap();

        delete_meeting_cascade(&pool, &mid).await.unwrap();

        let cnt: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM diarization_segments WHERE meeting_id = ?",
        )
        .bind(mid.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(cnt.0, 0, "cascade-delete should remove diarization rows");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd waved && cargo test -p waved-meeting --lib db::tests::diarization_segments_table_is_created`
Expected: FAIL — the table doesn't exist yet (count returns 0).

- [ ] **Step 3: Add the table to `init_pool`**

In `waved/crates/waved-meeting/src/db.rs`, inside `init_pool`, AFTER the existing `bookmarks` table creation and BEFORE the `info!` line:

```rust
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS diarization_segments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            meeting_id TEXT NOT NULL,
            start_ms INTEGER NOT NULL,
            end_ms INTEGER NOT NULL,
            speaker_id INTEGER NOT NULL,
            azimuth_deg REAL NOT NULL,
            FOREIGN KEY (meeting_id) REFERENCES meetings(id)
        )",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_diarization_meeting \
         ON diarization_segments(meeting_id)",
    )
    .execute(&pool)
    .await?;
```

- [ ] **Step 4: Add the insert function**

In the same file, after `add_bookmark` (around line 108), add:

```rust
pub async fn insert_diarization_segment(
    pool: &SqlitePool,
    seg: &crate::diarization::DiarizationSegment,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO diarization_segments \
         (meeting_id, start_ms, end_ms, speaker_id, azimuth_deg) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(seg.meeting_id.to_string())
    .bind(seg.start_ms)
    .bind(seg.end_ms)
    .bind(seg.speaker_id)
    .bind(seg.azimuth_deg)
    .execute(pool)
    .await?;
    Ok(())
}
```

- [ ] **Step 5: Extend `delete_meeting_cascade`**

In `delete_meeting_cascade` (around line 112), add a third DELETE inside the transaction, BEFORE the existing bookmarks DELETE:

```rust
pub async fn delete_meeting_cascade(
    pool: &SqlitePool,
    id: &Uuid,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM diarization_segments WHERE meeting_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM bookmarks WHERE meeting_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM meetings WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}
```

- [ ] **Step 6: Run all db tests**

Run: `cd waved && cargo test -p waved-meeting --lib db`
Expected: PASS — existing tests (cascade-with-bookmarks etc.) + 4 new diarization tests.

- [ ] **Step 7: Commit**

```bash
cd waved
git add crates/waved-meeting/src/db.rs
git commit -m "$(cat <<'EOF'
feat(diarization): db — table, insert, cascade-delete

New diarization_segments table (id, meeting_id FK, start_ms, end_ms,
speaker_id, azimuth_deg) created idempotently in init_pool with a
single index on meeting_id for the "all segments for meeting X" query
F3's downstream will run.

insert_diarization_segment takes a DiarizationSegment by reference.
delete_meeting_cascade extended to remove diarization rows in the
same atomic transaction as the meeting/bookmarks cleanup.

4 new tests: table creation, round-trip insert/query, multi-meeting
isolation, cascade-delete cleanup.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `MeetingService` wiring — accept `doa_rx`, drain in select arm

**Files:**
- Modify: `waved/crates/waved-meeting/src/lib.rs`

Adds the `Receiver<DoaFrame>` to `MeetingService` (constructor + field), constructs a `Diarizer` on `Command::StartRecording`, drains DoA frames into it in the `tokio::select!`, inserts segments to DB, finalizes on `Command::StopRecording` / `Command::Shutdown`.

- [ ] **Step 1: Add the field + constructor parameter**

In `waved/crates/waved-meeting/src/lib.rs`, modify the imports near the top to add the broadcast Receiver type:

```rust
use tokio::sync::{broadcast, mpsc, watch};
use waved_audio::doa::DoaFrame;
```

(The `tokio::sync::broadcast` import is likely already present; just verify `DoaFrame` is added to the imports.)

Add a field to the `MeetingService` struct (right after `audio_rx`):

```rust
pub struct MeetingService {
    config: WavedConfig,
    pool: SqlitePool,
    recordings_dir: PathBuf,
    meeting_tx: Arc<watch::Sender<MeetingState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
    audio_rx: mpsc::Receiver<ProcessedAudioFrame>,
    /// Per-frame DoA stream from F2's AudioService::subscribe_doa(). Drained
    /// only when a meeting is active; otherwise frames are discarded.
    doa_rx: broadcast::Receiver<DoaFrame>,
}
```

Extend `MeetingService::new` to accept the receiver:

```rust
impl MeetingService {
    pub async fn new(
        config: WavedConfig,
        db_path: &str,
        recordings_dir: PathBuf,
        meeting_tx: Arc<watch::Sender<MeetingState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
        audio_rx: mpsc::Receiver<ProcessedAudioFrame>,
        doa_rx: broadcast::Receiver<DoaFrame>,
    ) -> Result<Self, sqlx::Error> {
        let pool = db::init_pool(db_path).await?;
        Ok(Self {
            config,
            pool,
            recordings_dir,
            meeting_tx,
            events_tx,
            audio_rx,
            doa_rx,
        })
    }
}
```

- [ ] **Step 2: Add `Diarizer` to the run-loop local state**

In `MeetingService::run`, near the top alongside `let mut active = None;`, add:

```rust
let mut diarizer: Option<crate::diarization::Diarizer> = None;
```

- [ ] **Step 3: Construct/destroy `Diarizer` on Start/Stop**

Inside the `Command::StartRecording` handler, AFTER `active = Some(rec);` (and the prebuffer splice), add:

```rust
diarizer = Some(crate::diarization::Diarizer::new(id));
```

Inside the `Command::StopRecording` handler, before / after `self.stop_recording(rec).await;`, flush the diarizer (changes the existing block):

```rust
Command::StopRecording => {
    if let Some(rec) = active.take() {
        if let Some(mut d) = diarizer.take() {
            if let Some(seg) = d.finalize() {
                if let Err(e) = db::insert_diarization_segment(&self.pool, &seg).await {
                    warn!("failed to insert final diarization segment: {e}");
                }
            }
        }
        self.stop_recording(rec).await;
    }
}
```

Similarly in `Command::Shutdown`, before `self.stop_recording(rec).await;`:

```rust
Command::Shutdown => {
    if let Some(rec) = active.take() {
        info!("shutdown: stopping active recording first");
        if let Some(mut d) = diarizer.take() {
            if let Some(seg) = d.finalize() {
                if let Err(e) = db::insert_diarization_segment(&self.pool, &seg).await {
                    warn!("failed to insert final diarization segment on shutdown: {e}");
                }
            }
        }
        self.stop_recording(rec).await;
    }
    // ... rest of Shutdown handler unchanged ...
}
```

- [ ] **Step 4: Add the `tokio::select!` arm for DoA frames**

Inside the `loop { tokio::select! { ... } }`, add a new arm. Place it near the existing `audio_rx.recv()` arm — they're conceptually related (both per-frame audio data). The new arm:

```rust
doa_recv = self.doa_rx.recv() => {
    match doa_recv {
        Ok(frame) => {
            if let Some(diar) = diarizer.as_mut() {
                if let Some(seg) = diar.on_doa_frame(&frame) {
                    if let Err(e) = db::insert_diarization_segment(&self.pool, &seg).await {
                        warn!("failed to insert diarization segment: {e}");
                    }
                }
            }
            // No active meeting → drop the frame silently.
        }
        Err(broadcast::error::RecvError::Lagged(n)) => {
            warn!("diarization receiver lagged {n} frames");
            // Synthesize a silence frame at advanced timestamp so any
            // open segment can close cleanly.
            if let Some(diar) = diarizer.as_mut() {
                // Use the current ALSA period of the lagged span.
                // Spec §4: advance silence timer by n * PERIOD_MS.
                // We don't have an absolute timestamp here without a frame;
                // call on_silent_frame at a synthetic rel_ms beyond the
                // last_voice_ms by n*PERIOD_MS plus 1ms (force >500ms gap).
                // close_active result is captured.
                let synthetic_rel_ms = u32::MAX; // forces saturating sub to
                                                  // produce a huge gap → close
                if let Some(seg) = diar.on_silent_frame(synthetic_rel_ms) {
                    let _ = db::insert_diarization_segment(&self.pool, &seg).await;
                }
            }
        }
        Err(broadcast::error::RecvError::Closed) => {
            // DoA stream gone — finalize any open segment, leave the
            // diarizer state for the next StartRecording reset.
            if let Some(diar) = diarizer.as_mut() {
                if let Some(seg) = diar.finalize() {
                    let _ = db::insert_diarization_segment(&self.pool, &seg).await;
                }
            }
            warn!("DoA stream closed; diarization paused");
        }
    }
}
```

- [ ] **Step 5: Build to verify the wiring compiles**

Run: `cd waved && cargo build -p waved-meeting`
Expected: PASS — compiles. `main.rs` is now temporarily broken (calls `MeetingService::new` without the new `doa_rx` arg) — Task 7 fixes that.

- [ ] **Step 6: Run waved-meeting tests**

Run: `cd waved && cargo test -p waved-meeting --lib`
Expected: PASS — all 27 diarization tests + 4 db tests + existing tests. The MeetingService wiring isn't directly tested yet (Task 8 covers that).

- [ ] **Step 7: Commit**

```bash
cd waved
git add crates/waved-meeting/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(meeting): wire DoA stream into MeetingService for diarization

MeetingService::new accepts a broadcast::Receiver<DoaFrame>. A new
tokio::select! arm drains frames, routing them through the per-meeting
Diarizer when active.is_some(). Insert any returned segment to the db
on the spot; log-and-continue on insert failure (missed segment is
preferable to a lost recording).

Diarizer is constructed on StartRecording (resets clusters + speaker_id
counter) and finalized on StopRecording / Shutdown. RecvError::Lagged
synthesizes a far-future silence frame so any open segment closes.

main.rs still needs to pass the receiver — wired in Task 7.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `main.rs` wiring — `subscribe_doa()` receiver into `MeetingService::new`

**Files:**
- Modify: `waved/crates/waved/src/main.rs`

- [ ] **Step 1: Locate the existing `MeetingService::new` call site**

Run: `cd /Users/xaviergeerinck/Projects/insta360 && grep -n "MeetingService::new" waved/crates/waved/src/main.rs`
Expected output: `539:    match waved_meeting::MeetingService::new(`

- [ ] **Step 2: Add a `subscribe_doa()` call near the other subscribe_* calls**

Locate the existing `let mic_record_rx = audio_svc.subscribe_processed();` line (search for `subscribe_processed`). Just after the existing audio subscription lines (around line 197), add:

```rust
let meeting_doa_rx = audio_svc.subscribe_doa();
```

- [ ] **Step 3: Pass the receiver into `MeetingService::new`**

Modify the `MeetingService::new` call (currently around line 539) to add `meeting_doa_rx` as the 7th positional argument:

```rust
match waved_meeting::MeetingService::new(
    config.clone(),
    DB_PATH,
    PathBuf::from(RECORDINGS_DIR),
    Arc::clone(&bus.meeting),
    bus.events.clone(),
    meeting_rx,
    meeting_doa_rx,
).await {
```

- [ ] **Step 4: Build the full workspace**

Run: `cd waved && cargo build --features "ui,capture-alsa" -p waved`
Expected: PASS — full waved binary compiles with the new wiring.

- [ ] **Step 5: Run the full test suite for the touched crates**

Run: `cd waved && cargo test -p waved-meeting -p waved --lib`
Expected: PASS — all tests still pass.

- [ ] **Step 6: Commit**

```bash
cd waved
git add crates/waved/src/main.rs
git commit -m "$(cat <<'EOF'
feat(waved): pass AudioService::subscribe_doa() into MeetingService

Completes the F3 wiring chain: AudioService publishes DoaFrames →
MeetingService receives them → Diarizer clusters them → db row per
closed segment. No diarization happens unless a meeting is active.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: MeetingService end-to-end wiring test

**Files:**
- Modify: `waved/crates/waved-meeting/src/lib.rs` (add a test in the existing `#[cfg(test)] mod tests` block, or create one if none exists)

This test exercises the full per-frame path through `MeetingService::run` (not via the public `on_doa_frame` directly). Drives Start → frames → Stop, asserts the DB row count.

- [ ] **Step 1: Verify whether `lib.rs` has a `mod tests`**

Run: `cd /Users/xaviergeerinck/Projects/insta360 && grep -n "#\[cfg(test)\]" waved/crates/waved-meeting/src/lib.rs`

If a `mod tests` exists, append to it. If not, append a new block at the end of `lib.rs`.

- [ ] **Step 2: Write the wiring test**

Add this test (creating a new `#[cfg(test)] mod tests` block at the end of `waved/crates/waved-meeting/src/lib.rs` if needed):

```rust
#[cfg(test)]
mod wiring_tests {
    use super::*;
    use crate::diarization::DiarizationSegment;
    use std::sync::Arc;
    use tokio::sync::{broadcast, mpsc, watch};
    use waved_audio::doa::{DoaBackend, DoaFrame};
    use waved_config::WavedConfig;
    use waved_state::{Command, DeviceEvent, MeetingState};

    fn high(ms: u64, az: f32) -> DoaFrame {
        DoaFrame {
            frame_idx: 0, timestamp_ms: ms,
            azimuth_deg: az, elevation_deg: 0.0,
            confidence: 0.8, vad: true,
            backend: DoaBackend::GccPhat,
        }
    }

    #[tokio::test]
    async fn meeting_service_writes_diarization_segments_end_to_end() {
        // Per-test SQLite tempdir.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("t.db");
        let recordings_dir = tmp.path().join("rec");
        std::fs::create_dir_all(&recordings_dir).unwrap();

        // Channels.
        let (meeting_tx_inner, _meeting_rx) = watch::channel(MeetingState::default());
        let meeting_tx = Arc::new(meeting_tx_inner);
        let (events_tx, _events_rx) = broadcast::channel::<DeviceEvent>(16);
        let (audio_processed_tx, audio_processed_rx) = mpsc::channel(64);
        let (doa_tx_handle, _) = broadcast::channel::<DoaFrame>(64);

        // Construct service.
        let config = WavedConfig::default();
        let svc = MeetingService::new(
            config,
            db_path.to_str().unwrap(),
            recordings_dir,
            Arc::clone(&meeting_tx),
            events_tx.clone(),
            audio_processed_rx,
            doa_tx_handle.subscribe(),
        ).await.unwrap();

        // Run the service in the background.
        let (cmd_tx, cmd_rx) = broadcast::channel::<Command>(16);
        let (wifi_tx, wifi_rx) = watch::channel(waved_state::WifiState::default());
        let _ = (audio_processed_tx, wifi_tx); // keep alive for the run loop
        let _svc_handle = tokio::spawn(svc.run(cmd_rx, wifi_rx));

        // Start recording.
        cmd_tx.send(Command::StartRecording).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Push DoA frames: 500ms at az=45°, then 500ms at az=180°.
        for i in 0..24u64 {
            doa_tx_handle.send(high(i * 21, 45.0)).unwrap();
        }
        for i in 24..48u64 {
            doa_tx_handle.send(high(i * 21, 180.0)).unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Stop recording → finalize diarizer.
        cmd_tx.send(Command::StopRecording).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // Query the db directly.
        let pool = db::init_pool(db_path.to_str().unwrap()).await.unwrap();
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT speaker_id FROM diarization_segments ORDER BY id",
        )
        .fetch_all(&pool).await.unwrap();
        assert!(rows.len() >= 2, "expected ≥ 2 segments, got {}", rows.len());
        assert_eq!(rows[0].0, 1, "first segment should be speaker_id=1");
        assert_eq!(rows[1].0, 2, "second segment should be speaker_id=2");
    }
}
```

- [ ] **Step 3: Run the new wiring test**

Run: `cd waved && cargo test -p waved-meeting --lib wiring_tests`
Expected: PASS.

If the test is flaky due to timing (the `tokio::time::sleep` calls), bump the durations a bit — but don't add retry loops; if the wiring is wrong the test should fail deterministically.

- [ ] **Step 4: Run the full waved-meeting suite to confirm no regression**

Run: `cd waved && cargo test -p waved-meeting --lib`
Expected: PASS — all tests.

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-meeting/src/lib.rs
git commit -m "$(cat <<'EOF'
test(meeting): end-to-end wiring test for diarization

Drives a real MeetingService through Start → DoA frames → Stop. Pushes
500ms of az=45° then 500ms of az=180° via the broadcast channel,
asserts ≥ 2 diarization_segments rows are written with speaker_ids 1
and 2. Validates the receiver + Diarizer + db-insert chain without
real audio.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: On-device acceptance script

**Files:**
- Create: `waved/scripts/test-diarization.sh`

Operator-driven acceptance for the full F3 path on a real device.

- [ ] **Step 1: Create the script**

Create `waved/scripts/test-diarization.sh`:

```bash
#!/usr/bin/env bash
# F3 on-device acceptance: drive a real meeting on the Wave with the
# operator standing at different azimuths, then pull the SQLite db and
# verify diarization_segments rows.
#
# Prereqs:
# - device reachable via adb
# - waved binary at /data/waved/waved (per scripts/test-doa-improvements.sh)
# - operator able to physically reposition themselves

set -euo pipefail

DEVICE="${DEVICE:-PSP_0000000000000000}"
WAVED_BIN="${WAVED_BIN:-/data/waved/waved}"
DB_PATH="${DB_PATH:-/data/waved/meetings.db}"

adb_run() { adb -s "$DEVICE" shell "$@"; }
adb_kill_waved() {
    adb_run "killall waved 2>/dev/null; killall S60waved 2>/dev/null; sleep 1" || true
}
adb_start_waved() {
    adb_run "(setsid $WAVED_BIN >/tmp/waved.log 2>&1 </dev/null &); sleep 4"
}
prompt() {
    echo
    echo "==> $1"
    read -r -p "press ENTER when ready: " _
}

main() {
    adb_kill_waved
    adb_start_waved

    prompt "stand at az=0° (front of device); confirm waved is running"

    # Start a meeting via the SIGHUP / command bus is hardware-dependent.
    # Easier: rely on the touch UI. Operator triggers Start themselves.
    prompt "start recording via touch UI; talk for ~5 seconds"
    prompt "WALK to az=180° (behind device); talk for ~5 seconds"
    prompt "WALK back to az=0°; talk for ~5 seconds"
    prompt "stop recording via touch UI"

    # Pull the db and inspect.
    LOCAL_DB="/tmp/meetings-$(date +%Y%m%d-%H%M%S).db"
    adb pull "$DB_PATH" "$LOCAL_DB"

    echo
    echo "=== Diarization rows from latest meeting ==="
    sqlite3 "$LOCAL_DB" \
        "SELECT meeting_id, speaker_id, start_ms, end_ms, ROUND(azimuth_deg,1) \
         FROM diarization_segments \
         WHERE meeting_id = (SELECT id FROM meetings ORDER BY created_at DESC LIMIT 1) \
         ORDER BY start_ms"

    # Sanity check: at least 2 distinct speaker_ids in this meeting.
    DISTINCT=$(sqlite3 "$LOCAL_DB" \
        "SELECT COUNT(DISTINCT speaker_id) FROM diarization_segments \
         WHERE meeting_id = (SELECT id FROM meetings ORDER BY created_at DESC LIMIT 1)")
    if [ "$DISTINCT" -ge 2 ]; then
        echo "PASS: $DISTINCT distinct speakers detected (expected ≥ 2)"
    else
        echo "FAIL: only $DISTINCT distinct speakers"
        exit 1
    fi

    # Inspect the speaker_id sequence — expect [1, 2, 1] or similar pattern.
    SEQ=$(sqlite3 "$LOCAL_DB" \
        "SELECT speaker_id FROM diarization_segments \
         WHERE meeting_id = (SELECT id FROM meetings ORDER BY created_at DESC LIMIT 1) \
         ORDER BY start_ms")
    echo "Speaker sequence: $(echo $SEQ | tr '\n' ' ')"
    echo "(Expected: pattern like 1 2 1 for the az=0→180→0 walk.)"
}

main "$@"
```

- [ ] **Step 2: Make executable + syntax-check**

```bash
cd waved
chmod +x scripts/test-diarization.sh
bash -n scripts/test-diarization.sh
```
Expected: no output (syntax OK).

- [ ] **Step 3: Commit**

```bash
cd waved
git add scripts/test-diarization.sh
git commit -m "$(cat <<'EOF'
test(diarization): on-device acceptance script

scripts/test-diarization.sh drives a real meeting on a connected Wave:
operator stands at az=0°, walks to 180°, walks back, stops the meeting.
Script pulls meetings.db, prints the diarization_segments rows for the
latest meeting, and asserts ≥ 2 distinct speaker_ids.

Operator-driven; meeting Start/Stop via touch UI (the command bus is
not externally accessible from a shell). Auto-pass on speaker count;
sequence pattern (typically [1, 2, 1]) is informational for manual
review.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

Cross-checking the plan against the spec:

| Spec section | Implementing task | Status |
|---|---|---|
| §1 — `diarization.rs` new file + `pub mod` | Task 1 | OK |
| §1 — MeetingService gains DoA arm | Task 6 | OK |
| §1 — main.rs subscribe_doa wiring | Task 7 | OK |
| §2 — Data flow: DoaFrame → Diarizer → DB | Tasks 4 + 6 | OK |
| §2 — Lagged synthesizes silence | Task 6 (RecvError::Lagged arm) | OK |
| §2 — DB schema (`diarization_segments` + index) | Task 5 | OK |
| §2 — `DiarizationSegment` struct | Task 1 | OK |
| §3 — Constants (CLUSTER_TOLERANCE_DEG etc.) | Task 1 | OK |
| §3 — `assign_cluster` running-mean + wraparound | Task 2 | OK |
| §3 — Cluster overflow → absorb into nearest | Task 2 | OK |
| §3 — `on_confident_frame` / `on_silent_frame` | Task 3 | OK |
| §3 — `close_active` with 200ms drop | Task 3 | OK |
| §3 — `finalize()` flush | Task 3 | OK |
| §3 — `on_doa_frame` priming + confidence gate | Task 4 | OK |
| §4 — Receiver lag handling | Task 6 | OK |
| §4 — Receiver closed handling | Task 6 | OK |
| §4 — DB insert failure → warn + continue | Task 6 | OK |
| §4 — Diarizer reconstructed on Start | Task 6 | OK |
| §4 — `Vec::with_capacity(8)` preallocation | Task 1 | OK |
| §5.1 — 14 unit tests | Tasks 1-4 (2 + 6 + 10 + 9 = 27 tests, exceeds spec — extra coverage for the wiring boundaries) | OK |
| §5.2 — DB tests | Task 5 (4 tests) | OK |
| §5.3 — MeetingService wiring test | Task 8 | OK |
| §5.4 — On-device acceptance | Task 9 | OK |
| §7 — `subscribe_doa` flows through to MeetingService | Tasks 6 + 7 | OK |
| §7 — `diarization_segments` table + index visible on device | Task 5 + 7 (init runs on startup) | OK |

**No spec gaps.** All five completion criteria in §7 are covered.

**Placeholder scan:** searched for "TBD", "TODO", "implement later", "fill in" — none found.

**Type consistency:**
- `DiarizationSegment` struct defined once (Task 1), used unchanged in Tasks 3, 5, 8.
- `Diarizer::new(uuid::Uuid)` signature consistent in Tasks 1, 4, 6, 8.
- `Diarizer::on_doa_frame(&DoaFrame) -> Option<DiarizationSegment>` consistent in Tasks 1 (stub), 4 (real impl), 6 (caller).
- `Diarizer::finalize() -> Option<DiarizationSegment>` consistent in Tasks 1 (stub), 3 (real), 6 (caller), 8 (test).
- `db::insert_diarization_segment(&SqlitePool, &DiarizationSegment) -> Result<(), sqlx::Error>` consistent in Tasks 5 (definition), 6 (caller), 8 (test indirectly via wiring).

No issues. Plan ready for execution.
