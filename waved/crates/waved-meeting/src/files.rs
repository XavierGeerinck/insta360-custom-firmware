//! The recordings list for Files (D1–D3): every recording on Wave, newest
//! first, with its delivery state, its marks and, when it needs attention,
//! the reason in a few plain words; plus free space and whether it is low.

use crate::db;
use crate::model::MeetingStatus;
use crate::retention;
use sqlx::SqlitePool;
use std::path::Path;
use waved_config::StorageConfig;
use waved_state::{FilesState, RecordingStatus, RecordingSummary, CLOCK_VALID_AFTER_UNIX_S};

/// What Files shows for a database status. `None` hides the row: the
/// recording is still running, between WAV and decision, or being deleted.
pub(crate) fn summary_status(
    status: MeetingStatus,
    attempt_cycle: u32,
    retry_max: u32,
) -> Option<RecordingStatus> {
    use MeetingStatus as M;
    Some(match status {
        M::Recording | M::Finalized | M::Deleting => return None,
        M::AwaitingDecision => RecordingStatus::AwaitingDecision,
        M::LocalOnly => RecordingStatus::LocalOnly,
        M::Queued => RecordingStatus::Waiting,
        M::Uploading => RecordingStatus::Uploading,
        M::RetryWait if attempt_cycle < retry_max.max(1) => RecordingStatus::Waiting,
        M::RetryWait | M::ConfigurationError => RecordingStatus::Failed,
        M::Uploaded => RecordingStatus::Uploaded,
        M::MissingArtifact | M::LegacyUnsupported | M::ArtifactError => RecordingStatus::Damaged,
    })
}

/// Why a Failed or Damaged recording needs attention: a short lower-case
/// phrase the UI writes after "Upload failed: " or "File damaged: ". The
/// detailed error stays in the log and the database.
pub(crate) fn plain_reason(
    status: MeetingStatus,
    summary: RecordingStatus,
    last_status_code: Option<i64>,
) -> Option<String> {
    use MeetingStatus as M;
    if !summary.needs_attention() {
        return None;
    }
    Some(match (status, last_status_code) {
        (M::ConfigurationError | M::RetryWait, Some(code)) if code >= 500 => {
            format!("server error ({code})")
        }
        (M::ConfigurationError | M::RetryWait, Some(code)) => {
            format!("server refused it ({code})")
        }
        (M::ConfigurationError, None) => "check upload settings".into(),
        (M::RetryWait, None) => "server didn't answer".into(),
        (M::MissingArtifact, _) => "audio file missing".into(),
        (M::LegacyUnsupported, _) => "old recording format".into(),
        (M::ArtifactError, _) => "recording incomplete".into(),
        _ => return None,
    })
}

/// SQLite `datetime('now')` (UTC) as RFC 3339, or `None` before the clock
/// was synchronized.
pub(crate) fn started_at(start_time: &str) -> Option<String> {
    let parsed = chrono::NaiveDateTime::parse_from_str(start_time, "%Y-%m-%d %H:%M:%S").ok()?;
    let utc = parsed.and_utc();
    (utc.timestamp() >= CLOCK_VALID_AFTER_UNIX_S).then(|| utc.to_rfc3339())
}

fn audio_size(dir: &Path) -> u64 {
    ["audio.wav", "audio.wav.partial"]
        .iter()
        .find_map(|name| std::fs::metadata(dir.join(name)).ok())
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

pub(crate) async fn snapshot(
    pool: &SqlitePool,
    recordings_dir: &Path,
    storage: &StorageConfig,
    retry_max: u32,
    storage_blocked: bool,
) -> Result<FilesState, sqlx::Error> {
    let rows = db::recording_rows(pool).await?;
    let mut recordings = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(status) = MeetingStatus::parse(&row.status) else {
            continue;
        };
        let Some(summary) = summary_status(status, row.attempt_cycle, retry_max) else {
            continue;
        };
        let dir = Path::new(&row.dir);
        recordings.push(RecordingSummary {
            id: row.id,
            started_at: started_at(&row.start_time),
            duration_secs: row.duration_secs,
            size_bytes: audio_size(dir),
            status: summary,
            marks: row.marks,
            marks_ms: crate::metadata::marks_from_metadata(&dir.join("metadata.json")),
            last_error: plain_reason(status, summary, row.last_status_code),
        });
    }
    // Unknown free space reads as none left, so Files warns rather than hides it.
    let (free_bytes, free_mb) = retention::disk_usage(recordings_dir)
        .map(|usage| (usage.free_bytes, usage.free_mb))
        .unwrap_or((0, 0));
    Ok(FilesState {
        recordings,
        free_bytes,
        low_space: storage_blocked || free_mb < storage.minimum_free_mb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MeetingStatus as M;
    use waved_state::RecordingStatus as R;

    #[test]
    fn statuses_map_to_what_files_shows() {
        assert_eq!(summary_status(M::Recording, 0, 5), None);
        assert_eq!(summary_status(M::Finalized, 0, 5), None);
        assert_eq!(summary_status(M::Deleting, 0, 5), None);
        assert_eq!(
            summary_status(M::AwaitingDecision, 0, 5),
            Some(R::AwaitingDecision)
        );
        assert_eq!(summary_status(M::LocalOnly, 0, 5), Some(R::LocalOnly));
        assert_eq!(summary_status(M::Queued, 0, 5), Some(R::Waiting));
        assert_eq!(summary_status(M::Uploading, 1, 5), Some(R::Uploading));
        assert_eq!(summary_status(M::RetryWait, 4, 5), Some(R::Waiting));
        assert_eq!(summary_status(M::RetryWait, 5, 5), Some(R::Failed));
        assert_eq!(summary_status(M::ConfigurationError, 1, 5), Some(R::Failed));
        assert_eq!(summary_status(M::Uploaded, 1, 5), Some(R::Uploaded));
        for damaged in [M::MissingArtifact, M::LegacyUnsupported, M::ArtifactError] {
            assert_eq!(
                summary_status(damaged, 0, 5),
                Some(R::Damaged),
                "{damaged:?}"
            );
        }
    }

    /// Reasons are short lower-case phrases the UI puts after "Upload
    /// failed: " or "File damaged: " (lane D ruling).
    #[test]
    fn only_rows_that_need_attention_get_a_reason() {
        assert_eq!(
            plain_reason(M::LocalOnly, R::LocalOnly, None),
            None,
            "kept, no server"
        );
        assert_eq!(plain_reason(M::RetryWait, R::Waiting, None), None);
        assert_eq!(
            plain_reason(M::ConfigurationError, R::Failed, Some(401)).as_deref(),
            Some("server refused it (401)")
        );
        assert_eq!(
            plain_reason(M::ConfigurationError, R::Failed, None).as_deref(),
            Some("check upload settings")
        );
        assert_eq!(
            plain_reason(M::RetryWait, R::Failed, None).as_deref(),
            Some("server didn't answer")
        );
        assert_eq!(
            plain_reason(M::RetryWait, R::Failed, Some(503)).as_deref(),
            Some("server error (503)")
        );
        assert_eq!(
            plain_reason(M::MissingArtifact, R::Damaged, None).as_deref(),
            Some("audio file missing")
        );
        assert_eq!(
            plain_reason(M::LegacyUnsupported, R::Damaged, None).as_deref(),
            Some("old recording format")
        );
        assert_eq!(
            plain_reason(M::ArtifactError, R::Damaged, None).as_deref(),
            Some("recording incomplete")
        );
        for status in [
            M::ConfigurationError,
            M::RetryWait,
            M::MissingArtifact,
            M::LegacyUnsupported,
            M::ArtifactError,
        ] {
            for code in [None, Some(404), Some(500)] {
                let summary = summary_status(status, 9, 5).unwrap();
                let reason = plain_reason(status, summary, code).unwrap();
                assert!(
                    reason.starts_with(|c: char| c.is_ascii_lowercase())
                        && !reason.ends_with('.')
                        && !reason.contains('…')
                        && reason.chars().count() <= 24,
                    "{reason}"
                );
            }
        }
    }

    #[test]
    fn start_times_need_a_synchronized_clock() {
        assert_eq!(
            started_at("2026-10-05 14:02:00").as_deref(),
            Some("2026-10-05T14:02:00+00:00")
        );
        assert_eq!(started_at("1970-01-01 00:00:12"), None, "clock not set yet");
        assert_eq!(started_at("garbage"), None);
    }

    async fn add_row(
        pool: &SqlitePool,
        recordings: &Path,
        status: M,
        attempt_cycle: u32,
    ) -> (uuid::Uuid, std::path::PathBuf) {
        let id = uuid::Uuid::new_v4();
        let dir = recordings.join(id.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        db::insert_meeting(pool, &id, "Omni", &dir.to_string_lossy())
            .await
            .unwrap();
        sqlx::query("UPDATE meetings SET upload_status = ?, attempt_cycle = ? WHERE id = ?")
            .bind(status.as_str())
            .bind(attempt_cycle)
            .bind(id.to_string())
            .execute(pool)
            .await
            .unwrap();
        (id, dir)
    }

    #[tokio::test]
    async fn snapshot_lists_visible_rows_newest_first_with_marks_and_space() {
        let temp = tempfile::tempdir().unwrap();
        let recordings = temp.path().join("rec");
        std::fs::create_dir_all(&recordings).unwrap();
        let pool = db::init_pool(temp.path().join("t.db").to_str().unwrap())
            .await
            .unwrap();

        let (kept, kept_dir) = add_row(&pool, &recordings, M::LocalOnly, 0).await;
        std::fs::write(kept_dir.join("audio.wav"), [0_u8; 100]).unwrap();
        std::fs::write(
            kept_dir.join("metadata.json"),
            r#"{"recording":{"pre_roll_duration_ms":1500},
                "bookmarks":[{"offset_ms":0},{"offset_ms":2000}]}"#,
        )
        .unwrap();
        db::add_bookmark(&pool, &kept, 0).await.unwrap();
        db::add_bookmark(&pool, &kept, 2).await.unwrap();
        add_row(&pool, &recordings, M::Deleting, 0).await;
        let (failed, _) = add_row(&pool, &recordings, M::RetryWait, 5).await;
        add_row(&pool, &recordings, M::Recording, 0).await;

        let storage = StorageConfig {
            prune_uploaded_at_percent: 100,
            minimum_free_mb: 0,
        };
        let files = snapshot(&pool, &recordings, &storage, 5, false)
            .await
            .unwrap();
        let ids: Vec<_> = files.recordings.iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec![failed, kept],
            "newest first, deleting and recording hidden"
        );

        let failed_entry = &files.recordings[0];
        assert_eq!(failed_entry.status, R::Failed);
        assert_eq!(
            failed_entry.last_error.as_deref(),
            Some("server didn't answer")
        );
        assert_eq!(failed_entry.size_bytes, 0, "no audio file");
        assert!(failed_entry.marks_ms.is_empty());

        let kept_entry = &files.recordings[1];
        assert_eq!(kept_entry.status, R::LocalOnly);
        assert_eq!(kept_entry.size_bytes, 100);
        assert_eq!(kept_entry.marks, 2);
        assert_eq!(kept_entry.marks_ms, vec![1_500, 3_500], "pre-roll included");
        assert!(kept_entry.last_error.is_none());
        assert!(kept_entry.started_at.is_some());

        assert!(files.free_bytes > 0);
        assert!(!files.low_space);
        let blocked = snapshot(&pool, &recordings, &storage, 5, true)
            .await
            .unwrap();
        assert!(blocked.low_space, "storage blocked");
        let demanding = StorageConfig {
            minimum_free_mb: u64::MAX,
            ..storage
        };
        let low = snapshot(&pool, &recordings, &demanding, 5, false)
            .await
            .unwrap();
        assert!(low.low_space, "below storage.minimum_free_mb");
    }
}
