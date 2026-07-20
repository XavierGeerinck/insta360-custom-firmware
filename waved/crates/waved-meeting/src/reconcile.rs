use crate::db;
use crate::model::MeetingStatus;
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReconcileReport {
    pub normalized: u32,
    pub incomplete: u32,
    pub imported: u32,
}

/// Repair interrupted states without deleting or rewriting any recording
/// artifact. Upload eligibility requires both finalized files.
pub async fn run(pool: &SqlitePool, recordings_dir: &Path) -> Result<ReconcileReport, sqlx::Error> {
    let rows = db::artifact_rows(pool).await?;
    let mut report = ReconcileReport::default();
    let mut known = HashSet::new();

    for row in rows {
        known.insert(row.id.clone());
        let Ok(id) = row.id.parse::<Uuid>() else {
            warn!("meeting database contains invalid UUID {}", row.id);
            continue;
        };
        let dir = PathBuf::from(&row.dir);
        let wav = dir.join("audio.wav").is_file();
        let metadata = dir.join("metadata.json").is_file();
        let partial = dir.join("audio.wav.partial").is_file();
        let legacy_opus = dir.join("audio.opus").is_file();
        let current = MeetingStatus::parse(&row.status);
        let legacy_status = matches!(row.status.as_str(), "pending" | "upload_failed");
        let metadata_v1 =
            legacy_status && metadata && metadata_schema_is_v1(&dir.join("metadata.json")).await;

        let repaired = match row.status.as_str() {
            "pending" if wav && metadata_v1 => Some(MeetingStatus::Queued),
            "upload_failed" if wav && metadata_v1 => Some(MeetingStatus::RetryWait),
            "pending" | "upload_failed" if wav && metadata => {
                Some(MeetingStatus::LegacyUnsupported)
            }
            _ => match current {
                Some(MeetingStatus::Uploading) if wav && metadata => Some(MeetingStatus::Queued),
                Some(MeetingStatus::Queued) if wav && metadata => None,
                Some(MeetingStatus::RetryWait) if wav && metadata => None,
                Some(MeetingStatus::Recording) if wav && metadata => Some(MeetingStatus::LocalOnly),
                Some(MeetingStatus::Recording) if partial => Some(MeetingStatus::ArtifactError),
                Some(MeetingStatus::LocalOnly | MeetingStatus::Uploaded) if wav && metadata => None,
                Some(MeetingStatus::AwaitingDecision) if wav && metadata => None,
                Some(
                    MeetingStatus::MissingArtifact
                    | MeetingStatus::LegacyUnsupported
                    | MeetingStatus::ArtifactError
                    | MeetingStatus::ConfigurationError,
                ) => None,
                _ if legacy_opus && !wav => Some(MeetingStatus::LegacyUnsupported),
                _ if partial && !wav => Some(MeetingStatus::ArtifactError),
                _ if !wav || !metadata => Some(MeetingStatus::MissingArtifact),
                _ => None,
            },
        };
        if let Some(status) = repaired {
            db::set_status(pool, &id, status).await?;
            report.normalized += 1;
            if matches!(
                status,
                MeetingStatus::MissingArtifact
                    | MeetingStatus::LegacyUnsupported
                    | MeetingStatus::ArtifactError
            ) {
                report.incomplete += 1;
            }
        }
    }

    // A finalized artifact pair with no database row is useful data. Import
    // conservatively as local-only so reconciliation can never send data the
    // user did not explicitly queue.
    if let Ok(mut entries) = tokio::fs::read_dir(recordings_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if known.contains(&name) {
                continue;
            }
            let Ok(id) = name.parse::<Uuid>() else {
                continue;
            };
            let dir = entry.path();
            if dir.join("audio.wav").is_file() && dir.join("metadata.json").is_file() {
                db::import_local_artifact(pool, &id, &dir.to_string_lossy()).await?;
                report.imported += 1;
            }
        }
    }

    info!(
        "recording reconciliation: {} normalized, {} incomplete, {} imported",
        report.normalized, report.incomplete, report.imported
    );
    Ok(report)
}

async fn metadata_schema_is_v1(path: &Path) -> bool {
    const MAX_METADATA_BYTES: u64 = 1024 * 1024;

    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return false;
    };
    if metadata.len() > MAX_METADATA_BYTES {
        return false;
    }
    let Ok(bytes) = tokio::fs::read(path).await else {
        return false;
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| value.get("schema_version")?.as_u64())
        == Some(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (tempfile::TempDir, SqlitePool) {
        let temp = tempfile::tempdir().unwrap();
        let pool = db::init_pool(temp.path().join("meetings.db").to_str().unwrap())
            .await
            .unwrap();
        (temp, pool)
    }

    async fn insert(pool: &SqlitePool, root: &Path, status: MeetingStatus, files: &[&str]) -> Uuid {
        let id = Uuid::new_v4();
        let dir = root.join(id.to_string());
        tokio::fs::create_dir_all(&dir).await.unwrap();
        db::insert_meeting(pool, &id, "omni", &dir.to_string_lossy())
            .await
            .unwrap();
        db::set_status(pool, &id, status).await.unwrap();
        for file in files {
            tokio::fs::write(dir.join(file), b"fixture").await.unwrap();
        }
        id
    }

    #[tokio::test]
    async fn interrupted_and_legacy_rows_are_not_left_uploadable() {
        let (temp, pool) = fixture().await;
        let recordings = temp.path().join("recordings");
        tokio::fs::create_dir_all(&recordings).await.unwrap();
        let interrupted = insert(
            &pool,
            &recordings,
            MeetingStatus::Uploading,
            &["audio.wav", "metadata.json"],
        )
        .await;
        let legacy = insert(
            &pool,
            &recordings,
            MeetingStatus::RetryWait,
            &["audio.opus"],
        )
        .await;
        let partial = insert(
            &pool,
            &recordings,
            MeetingStatus::Recording,
            &["audio.wav.partial"],
        )
        .await;
        let finalized_before_db_update = insert(
            &pool,
            &recordings,
            MeetingStatus::Recording,
            &["audio.wav", "metadata.json"],
        )
        .await;

        run(&pool, &recordings).await.unwrap();
        run(&pool, &recordings).await.unwrap();

        for (id, expected) in [
            (interrupted, MeetingStatus::Queued),
            (legacy, MeetingStatus::LegacyUnsupported),
            (partial, MeetingStatus::ArtifactError),
            (finalized_before_db_update, MeetingStatus::LocalOnly),
        ] {
            let (status,): (String,) =
                sqlx::query_as("SELECT upload_status FROM meetings WHERE id = ?")
                    .bind(id.to_string())
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(status, expected.as_str());
        }
    }

    #[tokio::test]
    async fn orphan_pair_is_imported_local_only_once() {
        let (temp, pool) = fixture().await;
        let recordings = temp.path().join("recordings");
        let id = Uuid::new_v4();
        let dir = recordings.join(id.to_string());
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("audio.wav"), b"wav")
            .await
            .unwrap();
        tokio::fs::write(dir.join("metadata.json"), b"{}")
            .await
            .unwrap();

        assert_eq!(run(&pool, &recordings).await.unwrap().imported, 1);
        assert_eq!(run(&pool, &recordings).await.unwrap().imported, 0);
        let (status,): (String,) =
            sqlx::query_as("SELECT upload_status FROM meetings WHERE id = ?")
                .bind(id.to_string())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, MeetingStatus::LocalOnly.as_str());
    }

    #[tokio::test]
    async fn legacy_queue_statuses_are_normalized_only_for_v1_metadata() {
        let (temp, pool) = fixture().await;
        let recordings = temp.path().join("recordings");
        tokio::fs::create_dir_all(&recordings).await.unwrap();

        let legacy_failed = insert(
            &pool,
            &recordings,
            MeetingStatus::RetryWait,
            &["audio.wav", "metadata.json"],
        )
        .await;
        sqlx::query("UPDATE meetings SET upload_status = 'upload_failed' WHERE id = ?")
            .bind(legacy_failed.to_string())
            .execute(&pool)
            .await
            .unwrap();

        let v1_pending = insert(
            &pool,
            &recordings,
            MeetingStatus::Queued,
            &["audio.wav", "metadata.json"],
        )
        .await;
        tokio::fs::write(
            recordings
                .join(v1_pending.to_string())
                .join("metadata.json"),
            br#"{"schema_version":1}"#,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE meetings SET upload_status = 'pending' WHERE id = ?")
            .bind(v1_pending.to_string())
            .execute(&pool)
            .await
            .unwrap();

        let v1_failed = insert(
            &pool,
            &recordings,
            MeetingStatus::RetryWait,
            &["audio.wav", "metadata.json"],
        )
        .await;
        tokio::fs::write(
            recordings.join(v1_failed.to_string()).join("metadata.json"),
            br#"{"schema_version":1}"#,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE meetings SET upload_status = 'upload_failed' WHERE id = ?")
            .bind(v1_failed.to_string())
            .execute(&pool)
            .await
            .unwrap();

        run(&pool, &recordings).await.unwrap();

        for (id, expected) in [
            (legacy_failed, MeetingStatus::LegacyUnsupported),
            (v1_pending, MeetingStatus::Queued),
            (v1_failed, MeetingStatus::RetryWait),
        ] {
            let (status,): (String,) =
                sqlx::query_as("SELECT upload_status FROM meetings WHERE id = ?")
                    .bind(id.to_string())
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(status, expected.as_str());
        }
    }
}
