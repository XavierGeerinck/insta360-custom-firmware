use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::info;
use uuid::Uuid;

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

pub struct ActiveRecording {
    pub id: Uuid,
    pub dir: PathBuf,
    pub start: std::time::Instant,
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
            sample_rate: 48000,
            channels: 2,
            codec: "opus".to_string(),
        };

        let meta_json = serde_json::to_string_pretty(&metadata).unwrap();
        fs::write(dir.join("metadata.json"), meta_json).await?;
        fs::write(dir.join("audio.opus"), []).await?;

        info!("recording started: {} at {}", id, dir.display());
        Ok(Self {
            id,
            dir,
            start: std::time::Instant::now(),
        })
    }

    pub fn duration_secs(&self) -> u32 {
        self.start.elapsed().as_secs() as u32
    }

    pub fn audio_path(&self) -> PathBuf {
        self.dir.join("audio.opus")
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
            channels: 2,
            codec: "opus".to_string(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: MeetingMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "test-id");
        assert_eq!(parsed.sample_rate, 48000);
        assert_eq!(parsed.channels, 2);
        assert_eq!(parsed.codec, "opus");
        assert_eq!(parsed.duration_secs, 300);
    }

    #[tokio::test]
    async fn active_recording_creates_directory_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let rec = ActiveRecording::start(dir.path(), "Omni").await.unwrap();

        assert!(rec.dir.exists());
        assert!(rec.dir.join("metadata.json").exists());
        assert!(rec.dir.join("audio.opus").exists());

        let meta_json = tokio::fs::read_to_string(rec.dir.join("metadata.json")).await.unwrap();
        let meta: MeetingMetadata = serde_json::from_str(&meta_json).unwrap();
        assert_eq!(meta.id, rec.id.to_string());
        assert_eq!(meta.audio_mode, "Omni");
        assert_eq!(meta.sample_rate, 48000);
    }

    #[tokio::test]
    async fn audio_path_returns_correct_path() {
        let dir = tempfile::tempdir().unwrap();
        let rec = ActiveRecording::start(dir.path(), "Omni").await.unwrap();
        assert_eq!(rec.audio_path(), rec.dir.join("audio.opus"));
    }
}
