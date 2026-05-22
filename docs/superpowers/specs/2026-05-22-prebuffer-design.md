# Pre-record ring buffer — design

**Status:** Brainstormed 2026-05-22. Approved scope (fixed 30 s, in-crate, flat f32). Ready for implementation plan.

**Owning crate:** `waved-meeting`.

## Goal

When the user hits Record, write the last 30 seconds of post-DSP audio onto the front of the new `audio.wav`, so the file contains audio from *before* the button press.

This mirrors the "pre-recording" feature added in Insta360 firmware v1.2.0.6 (consumer package `fw_v3.0.0.1`); see `firmwares/fw_v3.0.0.1/CHANGELOG.md` and the reference symbols in `firmwares/fw_v3.0.0.1/extracted/rootfs/usr/lib/libInstaAudio.so` (`Insta360_AudioRingBuffer`, `Insta360_CreatePreRecordProccess`, `Insta360_OutputDeviceBase::preRecordSwitchToFileRecord`).

## Non-goals

- Configurable duration. Fixed `const PREBUFFER_SECONDS: u32 = 30;`. Bumping later is a one-line change; ship the simple version first.
- Multi-tap / multi-consumer ring buffers (Insta360 has them; we don't need them yet).
- Storing full `ProcessedAudioFrame`s with raw mic + reference channels. We only store the same mono f32 samples that `ActiveRecording::append_samples` would consume from a live frame.
- Feeding ASR or any other consumer from the prebuffer — out of scope per [[feedback-diarization-scope]] and [[project_sp4_npu_status]].
- Recording-format / sample-rate / channel-count changes. The recorder writes mono 48 kHz i16 today (`waved-meeting/src/recorder.rs:45` and `:59`); the prebuffer mirrors that.

## Architecture

The prebuffer is a new struct `PreBuffer` **defined in `waved-meeting/src/recorder.rs`** (next to `ActiveRecording`) but **owned and driven by `MeetingService::run()` in `waved-meeting/src/lib.rs`** — that's the function that owns the long-lived `audio_rx: mpsc::Receiver<ProcessedAudioFrame>` and the `let mut active: Option<ActiveRecording>` state. No new crate. No new public types outside `waved-meeting`.

Current pipeline:

```
waved-audio  ──(mpsc::Receiver<ProcessedAudioFrame>)──▶  MeetingService::run()  ──▶  ActiveRecording::append_samples(&[f32])  ──▶  hound::WavWriter  ──▶  audio.wav
```

Today `MeetingService::run()` only pulls from `audio_rx` when `active.is_some()` (per the `if active.is_some()` guard on the `Some(frame) = self.audio_rx.recv()` arm at `lib.rs:71`). Frames pile up in the channel during idle. **This guard must be removed** so the loop drains the channel continuously.

After this change, the `tokio::select!` recv arm has two states:

```rust
Some(frame) = self.audio_rx.recv() => {
    match active.as_mut() {
        Some(rec) => rec.append_samples(&frame.mic_mono_f32),     // active — unchanged
        None      => prebuffer.push(&frame.mic_mono_f32),         // idle — NEW
    }
}
```

When the `Command::StartRecording` branch creates a new `ActiveRecording`, immediately after the existing `active = Some(rec);` we splice:

```rust
// Splice the pre-record prefix onto the new WAV before any live frames arrive.
let prefix = prebuffer.drain_all();
if let Some(rec) = active.as_mut() {
    rec.append_samples(&prefix);
}
```

The prebuffer is therefore guaranteed empty during an active recording, which simplifies the "Record pressed twice" edge case (no-op — the second press is already rejected at `lib.rs:89-91` and the prebuffer would be empty anyway).

## `PreBuffer` API

`pub(crate)` struct in `waved-meeting/src/recorder.rs`, used from `lib.rs`. Not exported from the crate.

```rust
pub(crate) struct PreBuffer {
    samples: VecDeque<f32>,
    capacity: usize,  // PREBUFFER_SECONDS * sample_rate * channels
}

impl PreBuffer {
    /// Allocate capacity for `seconds * sample_rate * channels` samples.
    /// Called once from `MeetingService::run` as
    /// `PreBuffer::new(PREBUFFER_SECONDS, RECORDING_SAMPLE_RATE, 1)`.
    pub(crate) fn new(seconds: u32, sample_rate: u32, channels: u16) -> Self;

    /// Append `samples`, evicting oldest samples until len <= capacity.
    pub(crate) fn push(&mut self, samples: &[f32]);

    /// Take all buffered samples, leaving the ring empty.
    /// Returns up to `capacity` samples in oldest-to-newest order.
    pub(crate) fn drain_all(&mut self) -> Vec<f32>;

    /// Drop all buffered samples (used post-drain and on errors).
    pub(crate) fn clear(&mut self);

    /// Number of f32 samples currently buffered (0..=capacity).
    pub(crate) fn len(&self) -> usize;
}
```

`pub(crate) const PREBUFFER_SECONDS: u32 = 30;` lives next to the struct. Storing `f32` matches the input type of `ActiveRecording::append_samples(&[f32])` and the `frame.mic_mono_f32` field — no type juggling in the hot path.

**Memory budget** at the current mono 48 kHz f32: `30 × 48_000 × 1 × 4 = 5_760_000 bytes` = ~5.76 MB. Allocated once at `MeetingService::run()` start.

## Data flow & lifecycle

| Event | Behaviour |
|---|---|
| `MeetingService::run()` starts | Allocates `let mut prebuffer = PreBuffer::new(PREBUFFER_SECONDS, RECORDING_SAMPLE_RATE, 1);` once before the `tokio::select!` loop. |
| Frame arrives, `active.is_none()` | `prebuffer.push(&frame.mic_mono_f32)`. Oldest samples evicted when full. |
| `Command::StartRecording` succeeds | `ActiveRecording::start` runs as today → splice `prebuffer.drain_all()` → `rec.append_samples(&prefix)` → `active = Some(rec)`. WAV now contains up-to-30 s of pre-press audio. |
| Frame arrives, `active.is_some()` | `rec.append_samples(&frame.mic_mono_f32)` as today. Prebuffer is untouched. |
| `Command::StopRecording` | Existing `stop_recording` path. After this, frames again go to the prebuffer. |
| `Command::StartRecording` while already recording | Existing `warn!("already recording, ignoring start")` at `lib.rs:90` continues to apply; no splice happens because we never reach the splice point. |

## Error handling

- **Prebuffer empty at Record-start** (first ~30 s after boot, or just after a stop_recording): `drain_all` returns an empty `Vec`; recording starts with no prefix. Not an error.
- **WavWriter sample-write failure during prefix splice**: the existing `ActiveRecording::append_samples` already swallows individual write errors with a `tracing::warn!` and an early-return (`recorder.rs:92-95`). We rely on that same behaviour — no new error type. The prefix is logically a normal block of samples; if writing it fails, the recording is in the same degraded state it would be in if a live frame failed.
- **Sample-rate / channel-count mismatch**: not possible at runtime — `MeetingService::run` uses fixed `RECORDING_SAMPLE_RATE` and the channel is mono. No runtime check needed; if a future change makes either dynamic, the construction site (`PreBuffer::new` call) has to be updated regardless.

## Testing

Two layers — additive to the existing `recorder.rs` tests. The `MeetingService::run()` loop is wide enough that we test the splice via `ActiveRecording` + `PreBuffer` directly rather than spinning the whole service.

### a) Unit tests on `PreBuffer` (pure logic, no I/O)

- `push` then `drain_all` round-trips the same samples in order, when not at capacity.
- Filling past capacity → `drain_all` returns exactly `capacity` samples, and they are the *most recent* `capacity` samples (oldest evicted).
- `clear` empties; subsequent `len` == 0.
- `drain_all` on an empty buffer returns an empty `Vec`.
- Push of a slice larger than capacity in a single call: `drain_all` returns the last `capacity` samples of that slice.

### b) Splice integration test in `recorder.rs`

Demonstrates the prebuffer + ActiveRecording interaction without involving the full MeetingService loop. Pure end-to-end on the WAV file:

1. Create `PreBuffer::new(2, 48_000, 1)` — small (2 s) capacity so the test runs in milliseconds.
2. `push` 3 s × 48 000 = 144 000 samples of value `0.25` (idle — overflows the buffer; only the most-recent 2 s × 48 000 = 96 000 samples should remain).
3. `ActiveRecording::start(tmpdir, "Test")`.
4. `rec.append_samples(&prebuffer.drain_all())` — the splice.
5. `rec.append_samples(&vec![0.75_f32; 48_000])` — 1 s of live audio.
6. `rec.finalize()`.

Open the produced `audio.wav` with `hound::WavReader` (existing pattern at `recorder.rs:178`) and assert:
- Total length = `96_000 + 48_000 = 144_000` samples.
- First 96 000 samples decode (via the existing `* 32768.0` clamp) to `0x2000` (= 0.25 × 32768).
- Last 48 000 samples decode to `0x6000` (= 0.75 × 32768).

This indirectly proves: (i) the prebuffer correctly retains the most-recent 2 s after a 3 s push, (ii) the splice plus live append produces a consistent WAV, (iii) the values survive the f32 → i16 conversion in `append_samples`.

### c) "Record happens before prebuffer is full" path

Same shape as (b) but: push only `48_000` samples (1 s) into a `PreBuffer::new(2, 48_000, 1)` (capacity 2 s) before splicing. WAV should be `48_000 + 48_000 = 96_000` samples — no padding, no duplication.

No new dev-deps: `hound` and `tempfile` are already in `waved-meeting/Cargo.toml`.

## Files touched

- `waved/crates/waved-meeting/src/recorder.rs` — add `PreBuffer` struct + `PREBUFFER_SECONDS` const + unit tests + the splice integration test.
- `waved/crates/waved-meeting/src/lib.rs` — declare and allocate `PreBuffer` in `MeetingService::run`; remove the `if active.is_some()` guard on the `Some(frame) = self.audio_rx.recv()` arm (around line 71) so frames are always pulled; push to prebuffer or recorder based on `active` state; splice in the `Command::StartRecording` branch after `ActiveRecording::start` succeeds.

No other crate is touched. No public-API change to `waved-meeting`. No changes to `waved-audio`, `waved-config`, `waved-state`, or the controller IPC.

## Reference

- Insta360's reference implementation: `firmwares/fw_v3.0.0.1/extracted/rootfs/usr/lib/libInstaAudio.so` — see symbols `Insta360_AudioRingBuffer{C1,C2,D1,D2}Ejjjjjj`, `Insta360_AudioRingBuffer::{put,get,clear,isFull,getReady,getAvailSize,releaseBuffer}`, `Insta360_OutputDeviceBase::preRecordSwitchToFileRecord`. Constructor takes `(sample_rate, channel, bit_width, is_pre_record, max_pre_record_time_ms, _)`.
- v3 changelog adopt-list entry: `firmwares/fw_v3.0.0.1/CHANGELOG.md` § "Worth porting".
- Related memories: [[feedback-diarization-scope]], [[project_sp4_npu_status]], [[project_sp5_status]], [[project_sp6_status]].
