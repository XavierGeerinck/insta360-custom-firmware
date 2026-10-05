use crate::db::{BookmarkRecord, SpeakerSegmentRecord};
use crate::recorder::{FinalizedRecording, RECORDING_SAMPLE_RATE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;
use waved_config::WavedConfig;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WebhookMetadata {
    pub schema_version: u32,
    pub event: String,
    pub meeting_id: String,
    pub device: DeviceMetadata,
    pub recording: RecordingMetadata,
    pub audio: AudioMetadata,
    pub bookmarks: Vec<BookmarkMetadata>,
    pub speaker_segments: Vec<SpeakerSegmentMetadata>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeviceMetadata {
    pub id: String,
    pub name: String,
    pub software_version: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PauseMetadata {
    pub offset_ms: u64,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecordingMetadata {
    pub started_at: Option<String>,
    pub clock_synchronized: bool,
    pub recording_duration_ms: u64,
    pub audio_duration_ms: u64,
    pub pre_roll_duration_ms: u64,
    pub audio_mode: String,
    /// Pauses in the take (C1). Offsets use the recording timeline, like
    /// bookmarks. Absent in files written before milestone 2.
    #[serde(default)]
    pub pauses: Vec<PauseMetadata>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AudioMetadata {
    pub filename: String,
    pub content_type: String,
    pub codec: String,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub bit_depth: u16,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BookmarkMetadata {
    pub offset_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpeakerSegmentMetadata {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker_id: i64,
    pub azimuth_deg: f64,
}

pub async fn build_and_write(
    recording: &FinalizedRecording,
    config: &WavedConfig,
    device_id: &str,
    bookmarks: Vec<BookmarkRecord>,
    segments: Vec<SpeakerSegmentRecord>,
) -> Result<WebhookMetadata, MetadataError> {
    let sha256 = sha256_file(&recording.audio_path).await?;
    let filename = format!("{}.wav", recording.id);
    let metadata = WebhookMetadata {
        schema_version: 1,
        event: "meeting.recording.completed".into(),
        meeting_id: recording.id.to_string(),
        device: DeviceMetadata {
            id: device_id.to_string(),
            name: config.device.name.clone(),
            software_version: env!("CARGO_PKG_VERSION").into(),
        },
        recording: RecordingMetadata {
            started_at: recording.started_at.clone(),
            clock_synchronized: recording.clock_synchronized,
            recording_duration_ms: recording.recording_duration_ms(),
            audio_duration_ms: recording.audio_duration_ms(),
            pre_roll_duration_ms: recording.pre_roll_duration_ms(),
            audio_mode: recording.audio_mode.clone(),
            pauses: recording
                .pauses
                .iter()
                .map(|pause| PauseMetadata {
                    offset_ms: pause.offset_ms,
                    duration_ms: pause.duration_ms,
                })
                .collect(),
        },
        audio: AudioMetadata {
            filename,
            content_type: "audio/wav".into(),
            codec: "pcm_s16le".into(),
            sample_rate_hz: RECORDING_SAMPLE_RATE,
            channels: 1,
            bit_depth: 16,
            size_bytes: recording.size_bytes,
            sha256,
        },
        bookmarks: bookmarks
            .into_iter()
            .map(|bookmark| BookmarkMetadata {
                offset_ms: bookmark.timestamp_secs.saturating_mul(1_000) as u64,
            })
            .collect(),
        speaker_segments: segments
            .into_iter()
            .map(|segment| SpeakerSegmentMetadata {
                start_ms: segment.start_ms.max(0) as u64,
                end_ms: segment.end_ms.max(0) as u64,
                speaker_id: segment.speaker_id,
                azimuth_deg: segment.azimuth_deg,
            })
            .collect(),
    };
    write_atomic_json(&recording.dir, &metadata).await?;
    Ok(metadata)
}

pub fn stable_device_id() -> String {
    const PATHS: &[&str] = &[
        "/proc/device-tree/serial-number",
        "/sys/class/dmi/id/product_uuid",
        "/etc/machine-id",
        "/sys/class/net/wlan0/address",
    ];
    for path in PATHS {
        if let Ok(value) = std::fs::read_to_string(path) {
            let value = value.trim_matches(['\0', '\r', '\n', ' ']);
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    "wave-unknown".into()
}

async fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

async fn write_atomic_json(
    dir: &Path,
    metadata: &WebhookMetadata,
) -> Result<PathBuf, MetadataError> {
    let final_path = dir.join("metadata.json");
    let temporary = dir.join("metadata.json.new");
    let bytes = serde_json::to_vec_pretty(metadata)?;
    let mut file = tokio::fs::File::create(&temporary).await?;
    use tokio::io::AsyncWriteExt;
    file.write_all(&bytes).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&temporary, &final_path).await?;
    std::fs::File::open(dir)?.sync_all()?;
    Ok(final_path)
}

#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    #[error("metadata IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("metadata serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::ActiveRecording;

    #[tokio::test]
    async fn complete_metadata_uses_samples_and_never_empty_device_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut recording = ActiveRecording::start(dir.path(), "omni").await.unwrap();
        recording.append_pre_roll(&vec![0.0; 48_000]).unwrap();
        recording.append_samples(&vec![0.0; 96_000]).unwrap();
        let finalized = recording.finalize().unwrap();
        let metadata = build_and_write(
            &finalized,
            &WavedConfig::default(),
            "wave-test",
            vec![BookmarkRecord { timestamp_secs: 1 }],
            vec![SpeakerSegmentRecord {
                start_ms: 100,
                end_ms: 500,
                speaker_id: 1,
                azimuth_deg: 45.0,
            }],
        )
        .await
        .unwrap();

        assert_eq!(metadata.recording.pre_roll_duration_ms, 1_000);
        assert_eq!(metadata.recording.recording_duration_ms, 2_000);
        assert_eq!(metadata.recording.audio_duration_ms, 3_000);
        assert_eq!(metadata.device.id, "wave-test");
        assert_eq!(metadata.bookmarks[0].offset_ms, 1_000);
        assert_eq!(metadata.speaker_segments.len(), 1);
        assert_eq!(metadata.audio.sha256.len(), 64);
        assert!(finalized.dir.join("metadata.json").exists());
        assert!(metadata.recording.pauses.is_empty());
    }

    #[tokio::test]
    async fn pauses_are_written_and_old_metadata_still_parses() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let mut recording = ActiveRecording::start(dir.path(), "omni").await.unwrap();
        let t0 = Instant::now();
        recording.append_samples(&vec![0.0; 48_000]).unwrap();
        recording.pause_at(t0);
        recording.resume_at(t0 + Duration::from_secs(5));
        let finalized = recording.finalize().unwrap();
        let metadata = build_and_write(
            &finalized,
            &WavedConfig::default(),
            "wave-test",
            vec![],
            vec![],
        )
        .await
        .unwrap();
        assert_eq!(metadata.recording.pauses.len(), 1);
        assert_eq!(metadata.recording.pauses[0].offset_ms, 1_000);
        assert_eq!(metadata.recording.pauses[0].duration_ms, 5_000);
        let json = std::fs::read_to_string(finalized.dir.join("metadata.json")).unwrap();
        assert!(json.contains("\"pauses\""));

        let old = r#"{"started_at":null,"clock_synchronized":false,"recording_duration_ms":1,
            "audio_duration_ms":1,"pre_roll_duration_ms":0,"audio_mode":"Omni"}"#;
        let parsed: RecordingMetadata = serde_json::from_str(old).unwrap();
        assert!(parsed.pauses.is_empty());
    }
}
