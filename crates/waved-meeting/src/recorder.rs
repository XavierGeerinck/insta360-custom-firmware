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
