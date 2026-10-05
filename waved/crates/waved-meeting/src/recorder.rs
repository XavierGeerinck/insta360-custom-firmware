use std::collections::VecDeque;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::info;
use uuid::Uuid;

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
    /// When paused, `push` is a no-op so the buffer freezes. Mirrors stock's
    /// `preRecordPause`/`preRecordResume`: the pre-roll is paused while a
    /// recording is active (the take's own writer owns the audio) and resumed
    /// once idle, so a take's audio never bleeds into the next pre-roll.
    paused: bool,
    /// The user's pre-recording setting (D3). Off means nothing is buffered.
    enabled: bool,
}

impl PreBuffer {
    pub(crate) fn new(seconds: u32, sample_rate: u32, channels: u16) -> Self {
        let capacity = (seconds as usize) * (sample_rate as usize) * (channels as usize);
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
            paused: false,
            enabled: true,
        }
    }

    /// Freeze the pre-roll: subsequent `push` calls are dropped until `resume`.
    pub(crate) fn pause(&mut self) {
        self.paused = true;
    }

    /// Re-arm the pre-roll so `push` accumulates again.
    pub(crate) fn resume(&mut self) {
        self.paused = false;
    }

    /// Turn pre-recording on or off. Off drops everything buffered at once,
    /// because the setting is a privacy control, and frees the ring's memory.
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.samples = VecDeque::new();
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[cfg(test)]
    pub(crate) fn is_paused(&self) -> bool {
        self.paused
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    pub(crate) fn clear(&mut self) {
        self.samples.clear();
    }

    pub(crate) fn push(&mut self, samples: &[f32]) {
        if self.capacity == 0 || self.paused || !self.enabled {
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
}

pub const RECORDING_SAMPLE_RATE: u32 = 48_000;

pub struct ActiveRecording {
    pub id: Uuid,
    pub dir: PathBuf,
    pub audio_mode: String,
    pub started_at: Option<String>,
    pub clock_synchronized: bool,
    integrity_epoch: u64,
    total_samples: u64,
    pre_roll_samples: u64,
    /// Open WAV writer. `Some` while recording, `None` once finalized.
    /// We keep the writer hot so each `ProcessedAudioFrame` appends with
    /// no per-frame open/seek overhead.
    writer: Option<hound::WavWriter<BufWriter<File>>>,
    #[cfg(test)]
    fail_writes_after_samples: Option<u64>,
}

pub struct FinalizedRecording {
    pub id: Uuid,
    pub dir: PathBuf,
    pub audio_path: PathBuf,
    pub audio_mode: String,
    pub started_at: Option<String>,
    pub clock_synchronized: bool,
    pub total_samples: u64,
    pub pre_roll_samples: u64,
    pub size_bytes: u64,
}

impl FinalizedRecording {
    pub fn audio_duration_ms(&self) -> u64 {
        samples_to_ms(self.total_samples)
    }

    pub fn pre_roll_duration_ms(&self) -> u64 {
        samples_to_ms(self.pre_roll_samples)
    }

    pub fn recording_duration_ms(&self) -> u64 {
        samples_to_ms(self.total_samples.saturating_sub(self.pre_roll_samples))
    }
}

impl ActiveRecording {
    pub async fn start(base_dir: &Path, audio_mode: &str) -> std::io::Result<Self> {
        Self::start_with_integrity_epoch(base_dir, audio_mode, 0).await
    }

    pub async fn start_with_integrity_epoch(
        base_dir: &Path,
        audio_mode: &str,
        integrity_epoch: u64,
    ) -> std::io::Result<Self> {
        let id = Uuid::new_v4();
        let dir = base_dir.join(id.to_string());
        fs::create_dir_all(&dir).await?;

        // Create the WAV file synchronously — `hound::WavWriter` is sync, and
        // open is cheap (one ~44-byte header write). The hot path is
        // `append_samples`, also sync. 16-bit PCM: the capture/AEC pipeline is
        // 16-bit (see docs/AUDIO_HARDWARE.md "Recording bit depth"), so a wider
        // sample format would carry no extra resolution.
        let wav_path = dir.join("audio.wav.partial");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RECORDING_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(&wav_path, spec).map_err(std::io::Error::other)?;

        info!("recording started: {} at {}", id, dir.display());
        let now = chrono::Utc::now();
        let clock_synchronized = now.timestamp() >= 1_577_836_800;
        Ok(Self {
            id,
            dir,
            audio_mode: audio_mode.to_string(),
            started_at: clock_synchronized.then(|| now.to_rfc3339()),
            clock_synchronized,
            integrity_epoch,
            total_samples: 0,
            pre_roll_samples: 0,
            writer: Some(writer),
            #[cfg(test)]
            fail_writes_after_samples: None,
        })
    }

    pub fn duration_secs(&self) -> u32 {
        (self.total_samples.saturating_sub(self.pre_roll_samples) / RECORDING_SAMPLE_RATE as u64)
            as u32
    }

    pub fn audio_path(&self) -> PathBuf {
        self.dir.join("audio.wav")
    }

    pub fn partial_audio_path(&self) -> PathBuf {
        self.dir.join("audio.wav.partial")
    }

    pub fn integrity_epoch(&self) -> u64 {
        self.integrity_epoch
    }

    pub fn append_pre_roll(&mut self, samples: &[f32]) -> std::io::Result<u64> {
        let written = self.append_samples_inner(samples)?;
        self.pre_roll_samples = self.pre_roll_samples.saturating_add(written);
        Ok(written)
    }

    /// Append a block of f32 mono samples in [-1.0, 1.0], saturating to S16.
    /// Any write error is returned so the meeting service can fail the take
    /// closed and preserve the partial WAV for diagnosis.
    pub fn append_samples(&mut self, samples: &[f32]) -> std::io::Result<u64> {
        self.append_samples_inner(samples)
    }

    fn append_samples_inner(&mut self, samples: &[f32]) -> std::io::Result<u64> {
        #[cfg(test)]
        let fail_after_samples = self.fail_writes_after_samples;
        let Some(writer) = self.writer.as_mut() else {
            return Err(std::io::Error::other("recording writer unavailable"));
        };
        let mut written = 0_u64;
        for &s in samples {
            #[cfg(test)]
            if fail_after_samples.is_some_and(|limit| self.total_samples + written >= limit) {
                self.total_samples = self.total_samples.saturating_add(written);
                return Err(std::io::Error::other("injected wav write failure"));
            }
            let clamped = (s * 32768.0).clamp(i16::MIN as f32, i16::MAX as f32);
            if let Err(error) = writer.write_sample(clamped as i16) {
                self.total_samples = self.total_samples.saturating_add(written);
                return Err(std::io::Error::other(error));
            }
            written += 1;
        }
        self.total_samples = self.total_samples.saturating_add(written);
        Ok(written)
    }

    #[cfg(test)]
    pub fn fail_writes_after(&mut self, samples: u64) {
        self.fail_writes_after_samples = Some(samples);
    }

    /// Make an interrupted take inspectable without exposing it as a complete
    /// or uploadable artifact.
    pub fn abort(mut self) -> std::io::Result<PathBuf> {
        let writer = self
            .writer
            .take()
            .ok_or_else(|| std::io::Error::other("recording already closed"))?;
        let finalize_error = writer.finalize().err().map(std::io::Error::other);
        let partial = self.partial_audio_path();
        let file_sync_error = File::open(&partial).and_then(|file| file.sync_all()).err();
        let dir_sync_error = File::open(&self.dir).and_then(|dir| dir.sync_all()).err();
        if let Some(error) = finalize_error.or(file_sync_error).or(dir_sync_error) {
            return Err(error);
        }
        Ok(partial)
    }

    /// Close and sync the WAV, then atomically expose it as `audio.wav`.
    pub fn finalize(mut self) -> std::io::Result<FinalizedRecording> {
        let writer = self
            .writer
            .take()
            .ok_or_else(|| std::io::Error::other("recording already finalized"))?;
        writer.finalize().map_err(std::io::Error::other)?;

        let partial = self.partial_audio_path();
        let final_path = self.audio_path();
        File::open(&partial)?.sync_all()?;
        std::fs::rename(&partial, &final_path)?;
        File::open(&self.dir)?.sync_all()?;
        let size_bytes = std::fs::metadata(&final_path)?.len();
        Ok(FinalizedRecording {
            id: self.id,
            dir: self.dir.clone(),
            audio_path: final_path,
            audio_mode: self.audio_mode.clone(),
            started_at: self.started_at.clone(),
            clock_synchronized: self.clock_synchronized,
            total_samples: self.total_samples,
            pre_roll_samples: self.pre_roll_samples,
            size_bytes,
        })
    }
}

impl Drop for ActiveRecording {
    fn drop(&mut self) {
        // Safety net: make an interrupted partial WAV inspectable, but never
        // rename it to the finalized filename or make it uploadable.
        if let Some(writer) = self.writer.take() {
            if let Err(error) = writer.finalize() {
                tracing::warn!("partial wav finalize error: {error}");
            }
        }
    }
}

fn samples_to_ms(samples: u64) -> u64 {
    samples.saturating_mul(1_000) / RECORDING_SAMPLE_RATE as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn active_recording_creates_only_a_partial_wav() {
        let dir = tempfile::tempdir().unwrap();
        let rec = ActiveRecording::start(dir.path(), "Omni").await.unwrap();

        assert!(rec.dir.exists());
        assert!(rec.dir.join("audio.wav.partial").exists());
        assert!(!rec.dir.join("audio.wav").exists());
        assert!(!rec.dir.join("metadata.json").exists());
    }

    #[tokio::test]
    async fn audio_path_returns_correct_path() {
        let dir = tempfile::tempdir().unwrap();
        let rec = ActiveRecording::start(dir.path(), "Omni").await.unwrap();
        assert_eq!(rec.audio_path(), rec.dir.join("audio.wav"));
    }

    #[tokio::test]
    async fn append_then_finalize_produces_readable_wav() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Omni").await.unwrap();
        // 100 ms of silence
        let n = (RECORDING_SAMPLE_RATE / 10) as usize;
        let zeros = vec![0.0f32; n];
        rec.append_samples(&zeros).unwrap();
        let finalized = rec.finalize().unwrap();

        let reader = hound::WavReader::open(finalized.audio_path).unwrap();
        assert_eq!(reader.spec().sample_rate, 48_000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.duration() as usize, n);
    }

    #[test]
    fn prebuffer_new_is_empty() {
        let pb = PreBuffer::new(2, 48_000, 1);
        assert_eq!(pb.len(), 0);
        assert!(!pb.is_paused());
    }

    #[test]
    fn prebuffer_pause_drops_pushes_resume_rearms() {
        let mut pb = PreBuffer::new(2, 48_000, 1);
        pb.push(&[0.1, 0.2, 0.3]);
        assert_eq!(pb.len(), 3);

        pb.pause();
        assert!(pb.is_paused());
        pb.push(&[0.4; 1000]); // dropped while paused
        assert_eq!(pb.len(), 3, "paused buffer must not accumulate");

        pb.resume();
        assert!(!pb.is_paused());
        pb.push(&[0.5, 0.6]);
        assert_eq!(pb.len(), 5, "resumed buffer accumulates again");
    }

    #[test]
    fn prebuffer_clear_resets_len() {
        let mut pb = PreBuffer::new(2, 48_000, 1);
        pb.push(&[0.1_f32; 100]);
        assert_eq!(pb.len(), 100);
        pb.clear();
        assert_eq!(pb.len(), 0);
    }

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
        pb.push(&[1.0, 2.0, 3.0]); // [1,2,3]
        pb.push(&[4.0, 5.0, 6.0]); // overflow → [3,4,5,6]
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

    #[tokio::test]
    async fn prebuffer_splice_then_live_produces_correct_wav() {
        // PreBuffer capacity of 2 s @ 48 kHz mono = 96_000 samples.
        let mut pb = PreBuffer::new(2, RECORDING_SAMPLE_RATE, 1);

        // Push 3 s of 0.25 — first 1 s should be evicted, last 2 s retained.
        let three_seconds: Vec<f32> = vec![0.25_f32; 3 * RECORDING_SAMPLE_RATE as usize];
        pb.push(&three_seconds);
        assert_eq!(pb.len(), 2 * RECORDING_SAMPLE_RATE as usize);

        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Test").await.unwrap();

        // Splice prebuffer prefix, then write 1 s of live audio at 0.75.
        rec.append_pre_roll(&pb.drain_all()).unwrap();
        rec.append_samples(&vec![0.75_f32; RECORDING_SAMPLE_RATE as usize])
            .unwrap();
        let finalized = rec.finalize().unwrap();

        let mut reader = hound::WavReader::open(finalized.audio_path).unwrap();
        let samples: Vec<i16> = reader
            .samples::<i16>()
            .collect::<Result<_, _>>()
            .expect("wav decode");

        let prefix_len = 2 * RECORDING_SAMPLE_RATE as usize; // 96_000
        let live_len = RECORDING_SAMPLE_RATE as usize; // 48_000
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

    #[tokio::test]
    async fn prebuffer_splice_when_not_yet_full_produces_short_prefix() {
        // 2 s capacity, only 1 s pushed before recording starts.
        let mut pb = PreBuffer::new(2, RECORDING_SAMPLE_RATE, 1);
        pb.push(&vec![0.5_f32; RECORDING_SAMPLE_RATE as usize]); // 1 s

        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Test").await.unwrap();
        rec.append_pre_roll(&pb.drain_all()).unwrap();
        rec.append_samples(&vec![0.5_f32; RECORDING_SAMPLE_RATE as usize])
            .unwrap(); // 1 s live
        let finalized = rec.finalize().unwrap();

        let mut reader = hound::WavReader::open(finalized.audio_path).unwrap();
        let samples: Vec<i16> = reader
            .samples::<i16>()
            .collect::<Result<_, _>>()
            .expect("wav decode");

        // 1 s prefix + 1 s live = 2 s total. No padding, no duplication.
        assert_eq!(samples.len(), 2 * RECORDING_SAMPLE_RATE as usize);
        // 0.5 * 32768 = 16384 = 0x4000
        assert!(samples.iter().all(|&s| s == 0x4000));
    }

    #[tokio::test]
    async fn injected_write_failure_is_reported_and_abort_preserves_partial_wav() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start_with_integrity_epoch(dir.path(), "Test", 7)
            .await
            .unwrap();
        rec.fail_writes_after(2);

        let error = rec.append_samples(&[0.25, 0.5, 0.75]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert_eq!(rec.integrity_epoch(), 7);

        let partial = rec.abort().unwrap();
        assert!(partial.exists());
        assert!(!partial.with_file_name("audio.wav").exists());
        let reader = hound::WavReader::open(partial).unwrap();
        assert_eq!(reader.duration(), 2);
    }

    #[tokio::test]
    async fn injected_pre_roll_failure_is_returned_without_exposing_a_final_wav() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Test").await.unwrap();
        rec.fail_writes_after(1);

        rec.append_pre_roll(&[0.1, 0.2]).unwrap_err();
        let partial = rec.abort().unwrap();

        assert!(partial.exists());
        assert!(!partial.with_file_name("audio.wav").exists());
        assert_eq!(hound::WavReader::open(partial).unwrap().duration(), 1);
    }

    #[test]
    fn prebuffer_disabled_drops_buffered_audio_and_ignores_pushes() {
        let mut pb = PreBuffer::new(1, 100, 1);
        pb.push(&[0.1; 50]);
        pb.set_enabled(false);
        assert!(!pb.is_enabled());
        assert_eq!(pb.len(), 0, "turning it off drops what was buffered");
        pb.push(&[0.2; 10]);
        assert_eq!(pb.len(), 0, "nothing is buffered while off");
        pb.set_enabled(true);
        pb.push(&[0.3; 10]);
        assert_eq!(pb.drain_all(), vec![0.3; 10]);
    }
}
