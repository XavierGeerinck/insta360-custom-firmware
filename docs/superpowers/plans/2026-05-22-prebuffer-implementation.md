# Pre-record ring buffer — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the user hits Record, prepend the last 30 s of post-DSP audio to the new `audio.wav` so the file contains audio from *before* the button press.

**Architecture:** A `PreBuffer` struct (a `VecDeque<f32>` with a fixed capacity) lives in `waved-meeting/src/recorder.rs` and is owned by `MeetingService::run()` in `waved-meeting/src/lib.rs`. The audio-receive loop always drains the mpsc channel; frames either flow to the active `WavWriter` (recording in progress) or get pushed into the prebuffer (idle). On `Command::StartRecording`, the prebuffer is drained into the new `ActiveRecording` before any live frames are written.

**Tech Stack:** Rust async, tokio mpsc, hound WAV writer (already in the crate), `VecDeque`.

**Spec:** `docs/superpowers/specs/2026-05-22-prebuffer-design.md`.

---

## File Structure

- `waved/crates/waved-meeting/src/recorder.rs` — defines `PreBuffer`, `PREBUFFER_SECONDS`, unit tests for the buffer, and the splice integration tests. ~150 new lines on top of the existing 184.
- `waved/crates/waved-meeting/src/lib.rs` — owns the `PreBuffer` instance inside `MeetingService::run()`, removes the existing `if active.is_some()` guard on the recv arm, and adds the splice call in the `StartRecording` branch. ~10 lines net change.

No other files touched. No new dependencies.

---

## Task 1: `PreBuffer` skeleton — struct, const, `new`, `len`, `clear`

**Files:**
- Modify: `waved/crates/waved-meeting/src/recorder.rs`
- Test: same file, `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` block in `waved/crates/waved-meeting/src/recorder.rs`:

```rust
    #[test]
    fn prebuffer_new_is_empty() {
        let pb = PreBuffer::new(2, 48_000, 1);
        assert_eq!(pb.len(), 0);
    }

    #[test]
    fn prebuffer_clear_resets_len() {
        let mut pb = PreBuffer::new(2, 48_000, 1);
        pb.push(&[0.1_f32; 100]);
        assert_eq!(pb.len(), 100);
        pb.clear();
        assert_eq!(pb.len(), 0);
    }
```

(`push` is referenced here so this test forces us to introduce a no-op `push` in the next step. That's intentional — we ship a stub first, then real behaviour in Task 2.)

- [ ] **Step 2: Run tests to verify they fail with "PreBuffer not found"**

Run: `cargo test -p waved-meeting --lib prebuffer_`
Expected: compile error, `cannot find type 'PreBuffer' in this scope` / `cannot find function 'push'` etc.

- [ ] **Step 3: Add the skeleton struct + const**

In `waved/crates/waved-meeting/src/recorder.rs`, add near the top (after the `use` block, before `MeetingMetadata`):

```rust
use std::collections::VecDeque;

/// Seconds of pre-record audio kept in [`PreBuffer`] during idle.
/// Bumping this is a one-line change; see the design doc for the memory tradeoff.
pub(crate) const PREBUFFER_SECONDS: u32 = 30;

/// Ring buffer of post-DSP mono `f32` samples, capped at
/// `seconds * sample_rate * channels`. When full, `push` evicts the oldest
/// samples. Used by `MeetingService::run` to keep the last N seconds of
/// audio available for splicing onto the front of a new recording.
pub(crate) struct PreBuffer {
    samples: VecDeque<f32>,
    capacity: usize,
}

impl PreBuffer {
    pub(crate) fn new(seconds: u32, sample_rate: u32, channels: u16) -> Self {
        let capacity = (seconds as usize)
            * (sample_rate as usize)
            * (channels as usize);
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    pub(crate) fn clear(&mut self) {
        self.samples.clear();
    }

    pub(crate) fn push(&mut self, _samples: &[f32]) {
        // Real impl in Task 2.
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p waved-meeting --lib prebuffer_`
Expected: `prebuffer_new_is_empty ... ok`, `prebuffer_clear_resets_len ... FAILED` (because `push` is still a no-op — `len()` after push will be 0, not 100).

That's fine — we deliberately split: Task 1 ships the skeleton + the `new`/`len`/`clear` paths green; Task 2 fixes `push`.

Mark `prebuffer_clear_resets_len` as `#[ignore]` for this commit so CI stays green:

```rust
    #[test]
    #[ignore = "push not yet implemented — completed in Task 2"]
    fn prebuffer_clear_resets_len() {
```

Re-run: `cargo test -p waved-meeting --lib prebuffer_`
Expected: 1 passed, 0 failed, 1 ignored.

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-meeting/src/recorder.rs
git commit -m "feat(prebuffer): scaffold PreBuffer struct + new/len/clear

Skeleton only; push() is a no-op placeholder pending Task 2.
See docs/superpowers/specs/2026-05-22-prebuffer-design.md."
```

---

## Task 2: `PreBuffer::push` + `drain_all` with eviction

**Files:**
- Modify: `waved/crates/waved-meeting/src/recorder.rs`

- [ ] **Step 1: Write the failing tests**

Append to the same `mod tests`:

```rust
    #[test]
    fn prebuffer_push_then_drain_round_trips_under_capacity() {
        let mut pb = PreBuffer::new(1, 100, 1); // capacity 100
        pb.push(&[0.1, 0.2, 0.3, 0.4, 0.5]);
        let drained = pb.drain_all();
        assert_eq!(drained, vec![0.1, 0.2, 0.3, 0.4, 0.5]);
        assert_eq!(pb.len(), 0, "drain_all should empty the buffer");
    }

    #[test]
    fn prebuffer_evicts_oldest_when_overflowing() {
        let mut pb = PreBuffer::new(1, 4, 1); // capacity 4
        pb.push(&[1.0, 2.0, 3.0]);            // [1,2,3]
        pb.push(&[4.0, 5.0, 6.0]);            // overflow → [3,4,5,6]
        let drained = pb.drain_all();
        assert_eq!(drained, vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn prebuffer_push_slice_larger_than_capacity_keeps_tail() {
        let mut pb = PreBuffer::new(1, 4, 1); // capacity 4
        pb.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        let drained = pb.drain_all();
        assert_eq!(drained, vec![6.0, 7.0, 8.0, 9.0]);
    }

    #[test]
    fn prebuffer_drain_all_on_empty_returns_empty_vec() {
        let mut pb = PreBuffer::new(1, 100, 1);
        let drained = pb.drain_all();
        assert!(drained.is_empty());
        assert_eq!(pb.len(), 0);
    }
```

Also remove the `#[ignore]` attribute from `prebuffer_clear_resets_len` so it runs again.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p waved-meeting --lib prebuffer_`
Expected: `prebuffer_push_then_drain_round_trips_under_capacity ... FAILED` (drain returns empty because push is a no-op), `prebuffer_clear_resets_len ... FAILED` (same reason), and either `prebuffer_drain_all_on_empty_returns_empty_vec` failing on `drain_all` not being defined, or a compile error for the missing `drain_all` method.

- [ ] **Step 3: Implement `push` + `drain_all`**

In `waved/crates/waved-meeting/src/recorder.rs`, replace the placeholder `push` and add `drain_all`:

```rust
    pub(crate) fn push(&mut self, samples: &[f32]) {
        if self.capacity == 0 {
            return;
        }
        // If the incoming slice itself exceeds capacity, only the last
        // `capacity` samples can possibly survive — skip the front of the
        // slice entirely instead of pushing-and-evicting one-by-one.
        let start = samples.len().saturating_sub(self.capacity);
        let useful = &samples[start..];

        // Evict from the front to make room for `useful.len()` new samples.
        let need = (self.samples.len() + useful.len()).saturating_sub(self.capacity);
        if need > 0 {
            self.samples.drain(..need);
        }
        self.samples.extend(useful.iter().copied());
    }

    pub(crate) fn drain_all(&mut self) -> Vec<f32> {
        self.samples.drain(..).collect()
    }
```

- [ ] **Step 4: Run tests to verify all pass**

Run: `cargo test -p waved-meeting --lib prebuffer_`
Expected: 6 passed, 0 failed, 0 ignored.
(2 from Task 1 — `prebuffer_new_is_empty` + `prebuffer_clear_resets_len` un-ignored — plus the 4 new tests above.)

Also run the whole crate's tests to confirm nothing else broke:

Run: `cargo test -p waved-meeting --lib`
Expected: all green (existing `metadata_*`, `active_recording_*`, `audio_path_*`, `append_then_finalize_*` tests still pass).

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-meeting/src/recorder.rs
git commit -m "feat(prebuffer): implement push + drain_all with eviction

push() keeps only the most-recent capacity samples; handles the case
where a single push slice exceeds capacity by trimming the slice itself.
drain_all() empties the buffer and returns oldest-to-newest."
```

---

## Task 3: Splice integration test (full WAV roundtrip)

**Files:**
- Modify: `waved/crates/waved-meeting/src/recorder.rs` (tests only)

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
    #[tokio::test]
    async fn prebuffer_splice_then_live_produces_correct_wav() {
        // PreBuffer capacity of 2 s @ 48 kHz mono = 96_000 samples.
        let mut pb = PreBuffer::new(2, RECORDING_SAMPLE_RATE, 1);

        // Push 3 s of 0.25 — first 1 s should be evicted, last 2 s retained.
        let three_seconds: Vec<f32> =
            vec![0.25_f32; 3 * RECORDING_SAMPLE_RATE as usize];
        pb.push(&three_seconds);
        assert_eq!(pb.len(), 2 * RECORDING_SAMPLE_RATE as usize);

        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Test").await.unwrap();

        // Splice prebuffer prefix, then write 1 s of live audio at 0.75.
        rec.append_samples(&pb.drain_all());
        rec.append_samples(&vec![0.75_f32; RECORDING_SAMPLE_RATE as usize]);
        rec.finalize();

        let mut reader = hound::WavReader::open(rec.audio_path()).unwrap();
        let samples: Vec<i16> = reader
            .samples::<i16>()
            .collect::<Result<_, _>>()
            .expect("wav decode");

        let prefix_len = 2 * RECORDING_SAMPLE_RATE as usize; // 96_000
        let live_len = RECORDING_SAMPLE_RATE as usize;       // 48_000
        assert_eq!(samples.len(), prefix_len + live_len);

        // 0.25 * 32768 = 8192 = 0x2000
        assert!(
            samples[..prefix_len].iter().all(|&s| s == 0x2000),
            "expected all 0x2000 in prefix; first mismatch = {}",
            samples[..prefix_len]
                .iter()
                .position(|&s| s != 0x2000)
                .map(|i| format!("idx {i} = {}", samples[i]))
                .unwrap_or_else(|| "none".into())
        );

        // 0.75 * 32768 = 24576 = 0x6000
        assert!(
            samples[prefix_len..].iter().all(|&s| s == 0x6000),
            "expected all 0x6000 in live section"
        );

        // Drain the buffer was used; prebuffer is empty.
        assert_eq!(pb.len(), 0);
    }
```

- [ ] **Step 2: Run test to verify it passes (PreBuffer is already implemented)**

Run: `cargo test -p waved-meeting --lib prebuffer_splice_then_live_produces_correct_wav`
Expected: PASS. (`PreBuffer` and `ActiveRecording` are both already correct from Task 2 and the pre-existing code; this test just exercises them together.)

If it fails, the most likely cause is the eviction maths in `PreBuffer::push` — re-read Task 2 step 3.

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-meeting/src/recorder.rs
git commit -m "test(prebuffer): splice + live WAV roundtrip

Pushes 3s into a 2s buffer, splices into a fresh ActiveRecording,
appends 1s of live audio, and asserts the resulting audio.wav has
exactly the expected 96k+48k samples at the expected values."
```

---

## Task 4: "Record fires before prebuffer is full" path

**Files:**
- Modify: `waved/crates/waved-meeting/src/recorder.rs` (tests only)

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
    #[tokio::test]
    async fn prebuffer_splice_when_not_yet_full_produces_short_prefix() {
        // 2 s capacity, only 1 s pushed before recording starts.
        let mut pb = PreBuffer::new(2, RECORDING_SAMPLE_RATE, 1);
        pb.push(&vec![0.5_f32; RECORDING_SAMPLE_RATE as usize]); // 1 s

        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Test").await.unwrap();
        rec.append_samples(&pb.drain_all());
        rec.append_samples(&vec![0.5_f32; RECORDING_SAMPLE_RATE as usize]); // 1 s live
        rec.finalize();

        let mut reader = hound::WavReader::open(rec.audio_path()).unwrap();
        let samples: Vec<i16> = reader
            .samples::<i16>()
            .collect::<Result<_, _>>()
            .expect("wav decode");

        // 1 s prefix + 1 s live = 2 s total. No padding, no duplication.
        assert_eq!(samples.len(), 2 * RECORDING_SAMPLE_RATE as usize);
        // 0.5 * 32768 = 16384 = 0x4000
        assert!(samples.iter().all(|&s| s == 0x4000));
    }
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p waved-meeting --lib prebuffer_splice_when_not_yet_full`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-meeting/src/recorder.rs
git commit -m "test(prebuffer): short-prefix splice when buffer not yet full

Asserts no padding/duplication when start_recording fires before the
prebuffer has reached its capacity."
```

---

## Task 5: Wire `PreBuffer` into `MeetingService::run`

**Files:**
- Modify: `waved/crates/waved-meeting/src/lib.rs`

Three coordinated edits to the `run()` method (no new tests — Tasks 3 and 4 cover the behaviour; this task is glue verified by `cargo check` + existing tests).

- [ ] **Step 1: Import `PreBuffer` and `PREBUFFER_SECONDS`**

In `waved/crates/waved-meeting/src/lib.rs`, change the existing `recorder` use (currently `recorder::ActiveRecording` is referenced fully-qualified) by adding an import line near the top of the file alongside the other `use` statements:

```rust
use crate::recorder::{PreBuffer, PREBUFFER_SECONDS, RECORDING_SAMPLE_RATE};
```

(`RECORDING_SAMPLE_RATE` is currently only used inside `recorder.rs`; importing it here keeps the `PreBuffer::new` call readable.)

- [ ] **Step 2: Allocate the prebuffer at the top of `run()`**

In `MeetingService::run()`, after the existing `let mut active: Option<recorder::ActiveRecording> = None;` line (currently `lib.rs:56`), add:

```rust
        // Post-DSP mono samples kept around for the pre-record splice.
        // See docs/superpowers/specs/2026-05-22-prebuffer-design.md.
        let mut prebuffer = PreBuffer::new(PREBUFFER_SECONDS, RECORDING_SAMPLE_RATE, 1);
```

- [ ] **Step 3: Rewrite the audio-recv arm to always drain the channel**

In the `tokio::select!` block, replace:

```rust
                Some(frame) = self.audio_rx.recv(), if active.is_some() => {
                    if let Some(rec) = active.as_mut() {
                        rec.append_samples(&frame.mic_mono_f32);
                    }
                }
```

with:

```rust
                Some(frame) = self.audio_rx.recv() => {
                    match active.as_mut() {
                        Some(rec) => rec.append_samples(&frame.mic_mono_f32),
                        None => prebuffer.push(&frame.mic_mono_f32),
                    }
                }
```

(The `if active.is_some()` guard is gone; the loop now always pulls from the mpsc channel, preventing backlog during idle.)

- [ ] **Step 4: Splice in the `Command::StartRecording` branch**

In the same `run()` method, find the `Command::StartRecording` branch (currently `lib.rs:88-118`). Inside the `Ok(rec) =>` arm, replace the line `active = Some(rec);` with:

```rust
                                        active = Some(rec);
                                        // Splice the pre-record prefix onto the front of the
                                        // new WAV before any live frames arrive.
                                        let prefix = prebuffer.drain_all();
                                        if let Some(rec) = active.as_mut() {
                                            rec.append_samples(&prefix);
                                        }
```

- [ ] **Step 5: Verify the crate still compiles and existing tests pass**

Run: `cargo check -p waved-meeting`
Expected: no warnings or errors.

Run: `cargo test -p waved-meeting --lib`
Expected: all tests pass (the four new prebuffer tests from Tasks 1-4, plus the pre-existing `metadata_serialization_roundtrip`, `active_recording_creates_directory_and_files`, `audio_path_returns_correct_path`, `append_then_finalize_produces_readable_wav`).

- [ ] **Step 6: Verify the full workspace still builds**

Run: `cargo check --workspace`
Expected: no errors. (Catches anything that depended on the old import-less `RECORDING_SAMPLE_RATE` private constant.)

- [ ] **Step 7: Commit**

```bash
git add waved/crates/waved-meeting/src/lib.rs
git commit -m "feat(prebuffer): splice last 30s of audio onto new recordings

MeetingService::run now keeps a PreBuffer of the most-recent
PREBUFFER_SECONDS of post-DSP audio, fed continuously from the mpsc
audio channel. When StartRecording arrives, the prebuffer is drained
into the new ActiveRecording before any live frames are written.

Removes the if-active-is_some guard on the recv arm so frames don't
back up in the channel during idle."
```

---

## Done

After Task 5, recording any new meeting on-device will include up to 30 s of audio captured before the Record button was pressed. The feature is fully internal to `waved-meeting`; no controller / IPC / config changes.

## Verification on a real device (optional but recommended)

```bash
# 1. Build + deploy
cross build --release --target aarch64-unknown-linux-gnu \
    --features "ui,capture-alsa" -p waved
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "killall waved 2>/dev/null; sleep 2; \
           (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &)"

# 2. Let it idle for >30 s while you make some noise into the mic.
# 3. Hit Record on the device (via touch or via the controller).
# 4. Wait 5 s, hit Stop.
# 5. Pull the WAV:
adb pull /data/waved/recordings/<uuid>/audio.wav .
# 6. The file should be ~35 s long (30 s prebuffered + 5 s live).
soxi audio.wav   # or `ffprobe`
```
