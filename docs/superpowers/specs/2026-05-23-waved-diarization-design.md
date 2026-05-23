# waved DoA-based Diarization — Design

**Goal:** Per-meeting speaker diarization derived from the live DoA stream
F2 ships. Numeric speaker IDs (1, 2, 3, …) reset per meeting, persisted
to SQLite as time-windowed segments. The controller renders
`speaker_id = 3` literally as the string "Speaker 3" — no lookup, no
naming, no ML rescore.

**Why DoA-only:** Per [[feedback_diarization_scope]], we explicitly do
NOT build (a) a `speaker_mapping` table, (b) a rename UI, (c) a
pyannote-based ML rescore step. Decided 2026-05-22 after walking the
v3 firmware's speaker-mapping surface. The user's framing was: *"it's
already important that we know it's a different speaker"* — that's the
value, not naming.

**Scope:** One new module in the existing `waved-meeting` crate. F3
consumes `AudioService::subscribe_doa()` (shipped in F2) and writes
rows to a new `diarization_segments` SQLite table. Online streaming
— there is no batch-from-WAV path because the WAV doesn't carry DoA.

**Spec cross-refs:**
- F2 (DoaFrame stream): `docs/superpowers/specs/2026-05-23-waved-bf-doa-design.md`
- Diarization memory: [[feedback_diarization_scope]]

---

## 1. Architecture & Components

Diarization lives in `waved-meeting`. Single new module:

```
crates/waved-meeting/src/
├── lib.rs                  # MODIFY — gain doa_rx, drain into Diarizer, segment → DB
├── db.rs                   # MODIFY — diarization_segments table + insert fn
├── diarization.rs          # NEW — Diarizer state machine (clustering + segmentation)
├── recorder.rs             # existing
├── upload.rs               # existing
└── simulate.rs             # existing
```

**Two new responsibilities:**

1. **`diarization::Diarizer`** — pure synchronous state machine. Takes
   `DoaFrame` in, emits zero-or-one `DiarizationSegment` per call (when
   a segment closes). Owns its clusters + current-segment state. No
   I/O, no async — fully unit-testable.

2. **`db::insert_diarization_segment`** — single-row insert mirroring
   `insert_meeting` / `add_bookmark` patterns. Called from `lib.rs`
   when `Diarizer::on_doa_frame` returns `Some(segment)`.

**One existing module grows narrowly:**

- `MeetingService::run`'s `tokio::select!` gains an arm for the DoA
  receiver. When a meeting is active, frames flow into the `Diarizer`;
  segments that close get inserted via `insert_diarization_segment`.
  When no meeting is active, frames are discarded silently (cluster
  state is per-meeting; resets on next `StartRecording`).

**Wiring change in `crates/waved/src/main.rs`:**
- Where `audio_svc.subscribe_processed()` is called for
  `MeetingService::new`, also call `audio_svc.subscribe_doa()` and pass
  the receiver in.

**Total estimated change:** ~250 LoC added, ~30 LoC modified.

**Why not a new crate:** ~250 LoC is too small to justify a crate
boundary, and the diarizer is intrinsically per-meeting state — keeping
it in `waved-meeting` aligns lifecycle and dependencies naturally.

**Cluster state lifecycle:** `Diarizer::new()` is called on
`Command::StartRecording`; `next_speaker_id` resets to 1, clusters Vec
is empty. Numbering resets per meeting; we do not persist clusters
across meetings.

---

## 2. Data Flow & DB Schema

### Per-frame flow

```
AudioService                                MeetingService
   |                                          |
   | DoaFrame (every ~21ms)                   |
   | confidence, vad, azimuth_deg             |
   |─────── broadcast::Receiver<DoaFrame> ───▶|
                                              |
                                              | (if active.is_some())
                                              ▼
                              Diarizer::on_doa_frame(&frame)
                                              |
                                              ▼
                                  Option<DiarizationSegment>
                                              |
                                       (Some only when a
                                        segment closes)
                                              ▼
                              db::insert_diarization_segment(...)
```

### Backpressure / loss

`broadcast::Receiver` may lag past capacity (64 frames ≈ 1.3 s buffer
at 21 ms). On `Lagged(n)`:
- Treat the missed frames as silence in the diarizer (so the segment
  can close cleanly if the lag spanned a real silence).
- Log `warn!("diarization receiver lagged {n} frames")` — actionable
  signal that the `audio_rx + doa_rx` drain loop isn't keeping up.

### Segment-close triggers

A segment closes when ANY of:

1. **Speaker change** — new high-confidence frame whose nearest
   cluster differs from current.
2. **Silence gap > 500 ms** — `silence_frames * 21 ms > 500` of
   consecutive non-vad-or-low-confidence frames.
3. **Meeting stops** (`StopRecording`) — flush current segment if any.

Segments shorter than **200 ms total speech duration** (counted in
voice-frame count × period, NOT wall-clock) are discarded.

### DB Schema

New table mirrors `bookmarks` structure (auto-increment id + FK to
meetings):

```sql
CREATE TABLE IF NOT EXISTS diarization_segments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    meeting_id TEXT NOT NULL,
    start_ms INTEGER NOT NULL,   -- offset from meeting start in ms
    end_ms INTEGER NOT NULL,
    speaker_id INTEGER NOT NULL, -- 1, 2, 3, ... per meeting
    azimuth_deg REAL NOT NULL,   -- cluster centroid at segment open
    FOREIGN KEY (meeting_id) REFERENCES meetings(id)
);

CREATE INDEX IF NOT EXISTS idx_diarization_meeting
    ON diarization_segments(meeting_id);
```

- `start_ms` / `end_ms` use the meeting's local clock (`frame.timestamp_ms`
  minus the first-frame's timestamp), NOT wall clock. Aligns with WAV
  timestamps (also ms-since-start).
- `azimuth_deg` carried for downstream tooling (the controller could
  render an arrow + numeric ID; we don't enforce that).
- Single index on `meeting_id` since the dominant query is "all
  segments for meeting X" when the controller fetches the transcript.
- SQLite foreign-key enforcement is OFF by default and we do NOT
  enable it here — matches how `bookmarks` is wired.

### Rust types

```rust
// crates/waved-meeting/src/diarization.rs
#[derive(Debug, Clone, Copy)]
pub struct DiarizationSegment {
    pub meeting_id: uuid::Uuid,
    pub start_ms: u32,
    pub end_ms: u32,
    pub speaker_id: u32,
    pub azimuth_deg: f32,
}
```

`meeting_id` carried on the segment so the `lib.rs` insert call is
one-shot — no need to thread the current meeting ID through the
diarizer's state.

---

## 3. Clustering Algorithm

Synchronous Rust state machine; no async, no I/O.

```rust
const CLUSTER_TOLERANCE_DEG: f32 = 30.0;      // ±30° same-speaker
const CONFIDENCE_THRESHOLD: f32 = 0.30;       // matches SteeredBeam
const MAX_SEGMENT_SILENCE_MS: u32 = 500;      // close on >500ms gap
const MIN_SEGMENT_DURATION_MS: u32 = 200;     // drop shorter
const PERIOD_MS: u32 = 21;                    // ALSA period at 1024/48k
const MAX_CLUSTERS: usize = 20;               // overflow guard

struct Cluster {
    speaker_id: u32,
    centroid_deg: f32,
    /// Number of high-confidence frames assigned so far. Used for
    /// running-mean centroid update (no full history retained).
    frame_count: u32,
}

struct ActiveSegment {
    speaker_id: u32,
    start_ms: u32,                  // when this segment opened
    last_voice_ms: u32,             // last high-confidence frame
    voice_frame_count: u32,         // for the <200ms drop guard
    centroid_at_open_deg: f32,      // for the emitted segment record
}

pub struct Diarizer {
    meeting_id: uuid::Uuid,
    meeting_start_ms: u32,          // first frame's timestamp_ms
    clusters: Vec<Cluster>,         // Vec::with_capacity(8) preallocated
    next_speaker_id: u32,
    active: Option<ActiveSegment>,
    primed: bool,                   // false until first frame seeds meeting_start_ms
}
```

### `on_doa_frame` entry point

```rust
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

    /// Feed one DoA frame. Returns Some(segment) when a segment closes
    /// AND its total speech duration ≥ MIN_SEGMENT_DURATION_MS.
    pub fn on_doa_frame(&mut self, frame: &DoaFrame) -> Option<DiarizationSegment> {
        if !self.primed {
            self.meeting_start_ms = frame.timestamp_ms as u32;
            self.primed = true;
        }
        let rel_ms = (frame.timestamp_ms as u32).saturating_sub(self.meeting_start_ms);

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

    /// Called from MeetingService on StopRecording to flush any open segment.
    pub fn finalize(&mut self) -> Option<DiarizationSegment> {
        self.close_active(0)
    }
}
```

### Cluster assignment (running-mean on the unit circle)

```rust
fn assign_cluster(&mut self, az_deg: f32) -> u32 {
    // Find nearest cluster — circular distance since az wraps at 360°.
    let mut best_id = None;
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
        // Update centroid via running mean over the unit-circle to
        // handle wraparound (170° + 190° → 180°, not 0°).
        let id = best_id.unwrap();
        let c = self.clusters.iter_mut().find(|c| c.speaker_id == id).unwrap();
        c.frame_count += 1;
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
        // Cluster overflow — return the nearest existing cluster's ID,
        // effectively absorbing the frame into the closest speaker.
        // Log-once handled externally.
        best_id.unwrap_or(1)
    }
}
```

### Segment state updates

```rust
fn on_confident_frame(
    &mut self, speaker_id: u32, az_deg: f32, rel_ms: u32,
) -> Option<DiarizationSegment> {
    match self.active.as_mut() {
        Some(seg) if seg.speaker_id == speaker_id => {
            seg.last_voice_ms = rel_ms;
            seg.voice_frame_count += 1;
            None
        }
        Some(_) => {
            // Different speaker — close current, start new.
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

fn on_silent_frame(&mut self, rel_ms: u32) -> Option<DiarizationSegment> {
    let active = self.active.as_ref()?;
    let gap_ms = rel_ms.saturating_sub(active.last_voice_ms);
    if gap_ms > MAX_SEGMENT_SILENCE_MS {
        self.close_active(rel_ms)
    } else {
        None
    }
}

fn close_active(&mut self, _now_ms: u32) -> Option<DiarizationSegment> {
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

### Key design choices

- **Running-mean centroid over unit vectors** — handles azimuth
  wraparound (170° + 190° → 180°, not 0°). Same pattern as
  `doa::smooth_angle_deg`, `beam::steered::slerp`.
- **`voice_frame_count` not wall duration** — the <200 ms drop uses
  *actual speech frames* not wall-clock. A 2-second segment with only
  100 ms of high-confidence frames (rest sub-threshold) gets dropped.
- **`end_ms` = `last_voice_ms`** — not the time when we decide to
  close. The segment ends when the last voice frame was, even though
  the close-decision happens 500 ms later.
- **No cluster splitting/merging mid-meeting** — once a cluster
  exists, it stays. A speaker who moves >30° will spawn a second ID.
  Acceptable per memory.

---

## 4. Error Handling & Edge Cases

**Receiver lag (`broadcast::error::RecvError::Lagged(n)`):**
- Log `warn!("diarization receiver lagged {n} frames")`.
- Advance the silence-gap timer by `n * PERIOD_MS`: call
  `on_silent_frame(rel_ms + n * PERIOD_MS)` once with the adjusted
  timestamp — the gap-check naturally handles closure.
- Don't crash, don't reset clusters.

**Receiver closed (`RecvError::Closed`):**
- Flush any active segment via `finalize()`; log warn. Won't happen
  in practice (DoaPublisher outlives AudioService); paranoia guard.

**No DoA frames during entire meeting:**
- Diarizer never primes; no segments emitted; DB has zero rows for
  that meeting_id. Controller renders "no speakers detected".

**Single-frame meeting:**
- One frame opens segment. StopRecording → finalize returns segment
  but `voice_frame_count = 1` → 21 ms < 200 ms → dropped.

**StartRecording fires while DoA is mid-stream:**
- Diarizer constructed on the StartRecording transition. Frames
  arriving BEFORE that are dropped. First post-Start DoA frame primes
  `meeting_start_ms`. Pre-record prefix (5-s prebuffer) does NOT get
  diarization rows — DoA wasn't being captured.

**StopRecording mid-segment:**
- `finalize()` returns the open segment (if it passes the 200 ms
  guard). `end_ms` = `last_voice_ms`, not stop time.

**Concurrent with Steered audio mode:**
- The steered beam also subscribes to DoaPublisher. Each consumer has
  its own broadcast Receiver; no coupling. Diarizer doesn't affect
  beam, vice versa.

**Mute / silence mode:**
- DoA still emits frames per period — `vad=false, NaN angles`.
  Diarizer treats as silence-gap. Empty rows for that meeting.

**DB insert failure:**
- `db::insert_diarization_segment` returns `Result`. On error: log
  `warn!` and CONTINUE. Missed segment is preferable to a lost
  recording.

**Hot-path safety:**
- `on_doa_frame`: one Vec lookup (≤ 20 entries), one Vec::push when
  creating new cluster. `Vec::with_capacity(8)` preallocated; suppresses
  growth in the common case.
- `Diarizer` is `Send + Sync` by structure. MeetingService calls it
  synchronously in `tokio::select!` — no `.await` inside.

**`meeting_id` mismatch (paranoia):**
- Diarizer is re-constructed inside `Command::StartRecording`. Old
  instance dropped. Can't carry stale `meeting_id` unless ordering
  breaks.

**Cluster overflow:**
- Hard cap at `MAX_CLUSTERS = 20`. Beyond that, frames absorb into
  the nearest existing cluster (`best_id` from the failed-tolerance
  search). Logged once on first overflow per meeting. Real meetings
  rarely > 10 people; 20 is generous.

---

## 5. Testing

### 5.1 Unit tests (`cargo test -p waved-meeting --lib`)

Synthetic DoaFrames only. No DB, no MeetingService.

**Helper:**

```rust
fn high(ms: u64, az: f32) -> DoaFrame {
    DoaFrame {
        frame_idx: 0, timestamp_ms: ms,
        azimuth_deg: az, elevation_deg: 0.0,
        confidence: 0.8, vad: true,
        backend: DoaBackend::GccPhat,
    }
}
fn silent(ms: u64) -> DoaFrame {
    DoaFrame::silent(0, ms, DoaBackend::GccPhat)
}
```

| Test | Asserts |
|---|---|
| `single_speaker_one_segment` | 1 s at az=45° → finalize() returns one segment, speaker_id=1, az≈45°, duration ≈ 1000 ms. |
| `speaker_change_emits_segment` | 500 ms at 45° then 500 ms at 180° → two segments, speakers 1 and 2. |
| `silence_under_500ms_keeps_segment_open` | 100 ms voice, 300 ms silence, 100 ms voice (same az) → finalize returns ONE segment with end_ms = 2nd voice burst. |
| `silence_over_500ms_closes_segment` | 200 ms voice, 600 ms silence, 200 ms voice → first burst returns segment when silence triggers close; second burst closed on finalize. |
| `segment_under_200ms_dropped` | 100 ms (5 frames) of voice → finalize returns None. |
| `wraparound_clustering_at_350_and_10_merges` | Alternating 350° and 10° (20° apart) → one cluster, centroid near 0°. |
| `wraparound_clustering_at_350_and_50_splits` | 350° then 50° (60° apart) → two clusters. |
| `centroid_running_mean_handles_wraparound` | 30 frames at 355°, then 5° → centroid migrates ~0°, not 180°. |
| `low_confidence_frame_treated_as_silence` | High-conf opens segment, then 1 s of `confidence=0.1` → segment closes due to silence-gap. |
| `nan_azimuth_treated_as_silence` | `vad=true, confidence=0.8, azimuth_deg=NaN` → treated as silence. Documents F2's NaN-when-solve-failed contract. |
| `cluster_overflow_caps_at_20` | 25 frames at evenly-spaced unique azimuths → first 20 spawn clusters, 21st-25th absorb into nearest. |
| `finalize_returns_open_segment` | 500 ms voice (no silence trigger), then finalize() → returns segment. |
| `finalize_returns_none_if_no_active` | finalize() after new() → None. |
| `lagged_synthesizes_gap` | Open segment, call `on_silent_frame(rel_ms + 1000)` → segment closes correctly. |

### 5.2 DB integration test (`#[cfg(test)]` in `db.rs`, `tempfile::TempDir`)

| Test | Asserts |
|---|---|
| `diarization_segments_table_created` | `init_pool` succeeds; PRAGMA table_info shows correct shape. |
| `insert_and_query_one_segment` | Insert one segment, query back by meeting_id, fields round-trip. |
| `multiple_meetings_isolated` | Two meeting_ids; query by one returns only that meeting's rows. |

### 5.3 MeetingService wiring test (in `lib.rs`)

```rust
#[tokio::test]
async fn meeting_service_writes_diarization_segments() {
    // tempdir for db
    // construct MeetingService with mock channels
    // Send Command::StartRecording
    // Push DoaFrames via the doa_tx side
    // Sleep briefly
    // Send Command::StopRecording
    // Query db: assert N segments inserted, speaker_ids 1..N
}
```

Mid-fidelity test that exercises the full per-frame path without real
audio. Validates Receiver → Diarizer → DB-insert wiring end-to-end.

### 5.4 On-device acceptance (manual)

Add to existing `scripts/test-doa-improvements.sh` pattern OR create
minimal `scripts/test-diarization.sh`:

| Step | Verify |
|---|---|
| 1. Start meeting via touch UI / Command::StartRecording. | `meetings` row appears. |
| 2. Talk from az=0° ~5 s. Move to az=180° ~5 s. Move back to az=0° ~5 s. Stop. | `diarization_segments` shows ≥ 2 rows. Speaker_id pattern is `1, 2, 1`. |
| 3. `adb pull` the SQLite file. Inspect with `sqlite3`. | Segment timestamps roughly match the recording timeline. |

### 5.5 What's NOT tested

- DoA backend behavior under real reverb / multi-speaker — F2 / F2b
  concerns.
- Latency from first speech to first segment row — the 500 ms gap
  threshold means first segment isn't written until 500 ms after the
  speaker stops. Live UI rendering is downstream's problem.
- WAV / diarization timestamp alignment — both use ms-since-start;
  trust the math, no paired end-to-end test.

---

## 6. Out of Scope (explicit deferrals)

**Per [[feedback_diarization_scope]] — these will NOT be built, even
if requested mid-implementation. Revisiting requires a fresh
brainstorm:**

- **Speaker naming / rename UI.** No `speaker_mapping` table, no
  `(id, name)` CRUD, no on-device gesture. Controller renders the raw
  integer as `"Speaker 3"`.
- **Pyannote / ML rescore.** No host-side Python service to re-cluster
  using voice embeddings. Pure DoA-based clustering.
- **Cross-meeting speaker identity.** Speaker_id=1 in meeting A and B
  are unrelated. No persistence of centroids across meetings.

**Seemingly in-scope but explicitly deferred:**

- **Live diarization rendering on the device's OLED.** F3 writes to
  DB; on-screen rendering during the meeting is a UI task. The 400×304
  circular display is also visually crowded.
- **Confidence-weighted segments.** Segment is just
  `(start, end, speaker_id, az)`. No "this segment is 0.7 confident"
  field. Downstream doesn't consume it.
- **Multi-source / overlapping speakers.** DoA reports ONE direction
  per frame. Simultaneous speakers cluster incorrectly. Documented;
  no special handling. F2b candidate.
- **Cluster centroid recomputation on big shifts.** Moving speaker
  spawns new ID. We don't re-merge.
- **Diarization for non-meeting audio.** DoA stream is per-AudioService;
  non-meeting audio doesn't reach this path.
- **Segment metadata beyond `azimuth_deg`.** No elevation, no
  confidence, no backend identifier. Adding fields = schema migration;
  YAGNI.
- **A custom on-device CLI for inspecting diarization.**
  `sqlite3 /data/.../recordings/meeting.db "SELECT * FROM
  diarization_segments"` is the operator path.

**Triggers for revisiting:**

- Controller users report "two people sitting in line get merged" →
  revisit clustering tolerance (maybe pivot to vendor `libmodel_infer.so`
  DoA SRP via F2b vendor binding).
- Controller wants live speaker arrows → render from `subscribe_doa()`
  directly in UI layer, not through the diarization DB.
- A different downstream wants per-frame confidence → consider as v2
  schema migration, but only after a real consumer exists.

**Non-promises:**

- F3 does NOT guarantee speaker_id stability across re-runs of the
  same recording. (We don't re-run; diarization is real-time once.)
- F3 does NOT guarantee that two adjacent meetings with the same
  physical speaker layout produce the same numeric IDs.
- F3 does NOT guarantee any specific accuracy figure. Tolerance is
  set to ±30° per memory; whether that yields useful clustering is
  what on-device acceptance reveals.

---

## 7. Acceptance Summary

F3 is "done" when all of the following hold:

1. `cargo test -p waved-meeting --lib` — all 14 diarizer unit tests +
   3 DB tests + 1 service wiring test pass.
2. `cargo test -p waved-meeting -p waved-audio -p waved-state` — full
   F2 + F3 surface compiles and tests pass.
3. Manual on-device run per §5.4: a 3-speaker scenario produces
   `1, 2, 1` segment pattern in `diarization_segments`.
4. SQLite schema introspection on a live device shows the
   `diarization_segments` table + `idx_diarization_meeting` index.
5. `crates/waved/src/main.rs` passes `subscribe_doa()` receiver into
   `MeetingService::new`.
6. No regression in existing waved-meeting tests (recorder, upload,
   db init).
