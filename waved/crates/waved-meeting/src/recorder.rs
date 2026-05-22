use serde::{Deserialize, Serialize};
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

#[derive(Serialize, Deserialize)]
pub struct MeetingMetadata {
    pub id: String,
    pub start_time: String,
    pub duration_secs: u32,
    pub audio_mode: String,
    pub device_id: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub codec: String,
}

pub const RECORDING_SAMPLE_RATE: u32 = 48_000;

pub struct ActiveRecording {
    pub id: Uuid,
    pub dir: PathBuf,
    pub start: std::time::Instant,
    /// Open WAV writer. `Some` while recording, `None` once finalized.
    /// We keep the writer hot so each `ProcessedAudioFrame` appends with
    /// no per-frame open/seek overhead.
    writer: Option<hound::WavWriter<BufWriter<File>>>,
}

impl ActiveRecording {
    pub async fn start(base_dir: &Path, audio_mode: &str) -> std::io::Result<Self> {
        let id = Uuid::new_v4();
        let dir = base_dir.join(id.to_string());
        fs::create_dir_all(&dir).await?;

        let metadata = MeetingMetadata {
            id: id.to_string(),
            start_time: chrono::Utc::now().to_rfc3339(),
            duration_secs: 0,
            audio_mode: audio_mode.to_string(),
            device_id: String::new(),
            sample_rate: RECORDING_SAMPLE_RATE,
            channels: 1,
            codec: "wav_s16le".to_string(),
        };

        let meta_json = serde_json::to_string_pretty(&metadata).unwrap();
        fs::write(dir.join("metadata.json"), meta_json).await?;

        // Create the WAV file synchronously — `hound::WavWriter` is sync, and
        // open is cheap (one ~44-byte header write). The hot path is
        // `append_samples`, also sync.
        let wav_path = dir.join("audio.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RECORDING_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(&wav_path, spec)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        info!("recording started: {} at {}", id, dir.display());
        Ok(Self {
            id,
            dir,
            start: std::time::Instant::now(),
            writer: Some(writer),
        })
    }

    pub fn duration_secs(&self) -> u32 {
        self.start.elapsed().as_secs() as u32
    }

    pub fn audio_path(&self) -> PathBuf {
        self.dir.join("audio.wav")
    }

    /// Append a block of f32 mono samples in [-1.0, 1.0]. Saturating
    /// clamp to S16. Errors are logged but don't tear down the recording —
    /// a bad frame mid-meeting is better than losing the whole take.
    pub fn append_samples(&mut self, samples: &[f32]) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        for &s in samples {
            let clamped = (s * 32768.0).clamp(i16::MIN as f32, i16::MAX as f32);
            if let Err(e) = writer.write_sample(clamped as i16) {
                tracing::warn!("wav write error: {e}");
                return;
            }
        }
    }

    /// Close the WAV file (writes header sizes). Must be called before
    /// upload — the file is unreadable until then.
    pub fn finalize(&mut self) {
        if let Some(writer) = self.writer.take() {
            if let Err(e) = writer.finalize() {
                tracing::warn!("wav finalize error: {e}");
            }
        }
    }
}

impl Drop for ActiveRecording {
    fn drop(&mut self) {
        // Safety net: if the recording is dropped without an explicit
        // `finalize` (e.g. on panic), still flush the header.
        self.finalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_serialization_roundtrip() {
        let meta = MeetingMetadata {
            id: "test-id".to_string(),
            start_time: "2026-03-21T10:00:00Z".to_string(),
            duration_secs: 300,
            audio_mode: "Omni".to_string(),
            device_id: "wave-001".to_string(),
            sample_rate: 48000,
            channels: 1,
            codec: "wav_s16le".to_string(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: MeetingMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "test-id");
        assert_eq!(parsed.sample_rate, 48000);
        assert_eq!(parsed.channels, 1);
        assert_eq!(parsed.codec, "wav_s16le");
        assert_eq!(parsed.duration_secs, 300);
    }

    #[tokio::test]
    async fn active_recording_creates_directory_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let rec = ActiveRecording::start(dir.path(), "Omni").await.unwrap();

        assert!(rec.dir.exists());
        assert!(rec.dir.join("metadata.json").exists());
        assert!(rec.dir.join("audio.wav").exists());

        let meta_json = tokio::fs::read_to_string(rec.dir.join("metadata.json"))
            .await
            .unwrap();
        let meta: MeetingMetadata = serde_json::from_str(&meta_json).unwrap();
        assert_eq!(meta.id, rec.id.to_string());
        assert_eq!(meta.audio_mode, "Omni");
        assert_eq!(meta.sample_rate, 48000);
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
        rec.append_samples(&zeros);
        rec.finalize();

        let reader = hound::WavReader::open(rec.audio_path()).unwrap();
        assert_eq!(reader.spec().sample_rate, 48_000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.duration() as usize, n);
    }

    #[test]
    fn prebuffer_new_is_empty() {
        let pb = PreBuffer::new(2, 48_000, 1);
        assert_eq!(pb.len(), 0);
    }

    #[test]
    #[ignore = "push not yet implemented — completed in Task 2"]
    fn prebuffer_clear_resets_len() {
        let mut pb = PreBuffer::new(2, 48_000, 1);
        pb.push(&[0.1_f32; 100]);
        assert_eq!(pb.len(), 100);
        pb.clear();
        assert_eq!(pb.len(), 0);
    }
}
