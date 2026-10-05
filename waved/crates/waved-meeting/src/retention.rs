use crate::db;
use nix::sys::statvfs::statvfs;
use sqlx::SqlitePool;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{info, warn};
use waved_config::StorageConfig;
use waved_state::MeetingState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DiskUsage {
    pub(crate) used_percent: u8,
    /// Space available to the recorder, in bytes and in whole MiB.
    pub(crate) free_bytes: u64,
    pub(crate) free_mb: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetentionOutcome {
    WithinLimits,
    Pruned,
    Blocked,
}

pub async fn enforce(
    pool: &SqlitePool,
    recordings_dir: &Path,
    config: &StorageConfig,
    meeting_tx: &Arc<watch::Sender<MeetingState>>,
) {
    if let Err(error) = tokio::fs::create_dir_all(recordings_dir).await {
        warn!(
            "cannot prepare recording storage at {}: {error}",
            recordings_dir.display()
        );
        meeting_tx.send_modify(|state| state.storage_blocked = true);
        return;
    }
    loop {
        let usage = match disk_usage(recordings_dir) {
            Ok(usage) => usage,
            Err(error) => {
                warn!("cannot inspect recording storage: {error}");
                meeting_tx.send_modify(|state| state.storage_blocked = true);
                return;
            }
        };
        match enforce_for_usage(pool, recordings_dir, config, meeting_tx, usage).await {
            RetentionOutcome::Pruned => {}
            RetentionOutcome::WithinLimits | RetentionOutcome::Blocked => return,
        }
    }
}

pub(crate) async fn enforce_for_usage(
    pool: &SqlitePool,
    recordings_dir: &Path,
    config: &StorageConfig,
    meeting_tx: &Arc<watch::Sender<MeetingState>>,
    usage: DiskUsage,
) -> RetentionOutcome {
    if !needs_pruning(usage, config) {
        meeting_tx.send_modify(|state| state.storage_blocked = false);
        return RetentionOutcome::WithinLimits;
    }

    let candidate = match db::oldest_uploaded_artifact(pool).await {
        Ok(candidate) => candidate,
        Err(error) => {
            warn!("cannot select uploaded recording for pruning: {error}");
            meeting_tx.send_modify(|state| state.storage_blocked = true);
            return RetentionOutcome::Blocked;
        }
    };
    let Some((id, dir)) = candidate else {
        meeting_tx.send_modify(|state| state.storage_blocked = true);
        warn!(
            "recording storage is constrained ({}% used, {} MiB free) \
             and no delivered recording is safe to prune",
            usage.used_percent, usage.free_mb
        );
        return RetentionOutcome::Blocked;
    };

    if let Err(error) = remove_uploaded_dir(recordings_dir, Path::new(&dir)).await {
        warn!("failed to prune delivered recording {id} at {dir}: {error}");
        meeting_tx.send_modify(|state| state.storage_blocked = true);
        return RetentionOutcome::Blocked;
    }
    if let Err(error) = db::delete_meeting_cascade(pool, &id).await {
        warn!("pruned files for {id} but failed to remove database row: {error}");
        meeting_tx.send_modify(|state| state.storage_blocked = true);
        return RetentionOutcome::Blocked;
    }
    info!("pruned delivered recording {id} under storage pressure");
    RetentionOutcome::Pruned
}

async fn remove_uploaded_dir(recordings_dir: &Path, candidate: &Path) -> std::io::Result<()> {
    let metadata = match tokio::fs::symlink_metadata(candidate).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "recording artifact is not a directory",
        ));
    }

    let root = tokio::fs::canonicalize(recordings_dir).await?;
    let candidate = tokio::fs::canonicalize(candidate).await?;
    if candidate.parent() != Some(root.as_path()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing to prune outside the recording root",
        ));
    }
    tokio::fs::remove_dir_all(candidate).await
}

pub(crate) fn disk_usage(path: &Path) -> Result<DiskUsage, nix::Error> {
    let stats = statvfs(path)?;
    let blocks = stats.blocks() as u64;
    let used_blocks = blocks.saturating_sub(stats.blocks_free() as u64);
    let used_percent = if blocks == 0 {
        0
    } else {
        used_blocks
            .saturating_mul(100)
            .saturating_div(blocks)
            .min(100) as u8
    };
    let fragment_size = (stats.fragment_size() as u64).max(1);
    let free_bytes = (stats.blocks_available() as u64).saturating_mul(fragment_size);
    Ok(DiskUsage {
        used_percent,
        free_bytes,
        free_mb: free_bytes / (1024 * 1024),
    })
}

fn needs_pruning(usage: DiskUsage, config: &StorageConfig) -> bool {
    usage.used_percent >= config.prune_uploaded_at_percent || usage.free_mb < config.minimum_free_mb
}

#[cfg(feature = "recording-integrity-test-hooks")]
pub async fn run_acceptance_fixture(
    fixture_root: &Path,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    validate_fixture_root(fixture_root)?;
    tokio::fs::create_dir(fixture_root).await?;
    let recordings_dir = fixture_root.join("recordings");
    tokio::fs::create_dir(&recordings_dir).await?;
    let database_path = fixture_root.join("meetings.db");
    let database_path = database_path.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "fixture path is not UTF-8",
        )
    })?;
    let pool = db::init_pool(database_path).await?;
    let (meeting_tx, _meeting_rx) = watch::channel(MeetingState::default());
    let meeting_tx = Arc::new(meeting_tx);
    let config = StorageConfig {
        prune_uploaded_at_percent: 85,
        minimum_free_mb: 2_048,
    };

    let protected = [
        crate::model::MeetingStatus::Recording,
        crate::model::MeetingStatus::Finalized,
        crate::model::MeetingStatus::AwaitingDecision,
        crate::model::MeetingStatus::LocalOnly,
        crate::model::MeetingStatus::Queued,
        crate::model::MeetingStatus::Uploading,
        crate::model::MeetingStatus::RetryWait,
        crate::model::MeetingStatus::ConfigurationError,
        crate::model::MeetingStatus::MissingArtifact,
        crate::model::MeetingStatus::LegacyUnsupported,
        crate::model::MeetingStatus::ArtifactError,
    ];
    let mut protected_artifacts = Vec::new();
    for status in protected {
        protected_artifacts.push(add_acceptance_artifact(&pool, &recordings_dir, status).await?);
    }
    let oldest = add_acceptance_artifact(
        &pool,
        &recordings_dir,
        crate::model::MeetingStatus::Uploaded,
    )
    .await?;
    let newer = add_acceptance_artifact(
        &pool,
        &recordings_dir,
        crate::model::MeetingStatus::Uploaded,
    )
    .await?;

    let percent_outcome = enforce_for_usage(
        &pool,
        &recordings_dir,
        &config,
        &meeting_tx,
        DiskUsage {
            used_percent: 85,
            free_bytes: 10_000 * 1024 * 1024,
            free_mb: 10_000,
        },
    )
    .await;
    require_acceptance(
        percent_outcome == RetentionOutcome::Pruned,
        "percentage threshold did not prune",
    )?;
    require_acceptance(
        !oldest.1.exists() && newer.1.exists(),
        "percentage threshold did not prune the oldest uploaded artifact only",
    )?;

    let free_space_outcome = enforce_for_usage(
        &pool,
        &recordings_dir,
        &config,
        &meeting_tx,
        DiskUsage {
            used_percent: 10,
            free_bytes: 2_047 * 1024 * 1024,
            free_mb: 2_047,
        },
    )
    .await;
    require_acceptance(
        free_space_outcome == RetentionOutcome::Pruned,
        "minimum-free threshold did not prune",
    )?;
    require_acceptance(
        !newer.1.exists(),
        "minimum-free threshold did not prune the remaining uploaded artifact",
    )?;

    let blocked_outcome = enforce_for_usage(
        &pool,
        &recordings_dir,
        &config,
        &meeting_tx,
        DiskUsage {
            used_percent: 85,
            free_bytes: 2_047 * 1024 * 1024,
            free_mb: 2_047,
        },
    )
    .await;
    require_acceptance(
        blocked_outcome == RetentionOutcome::Blocked && meeting_tx.borrow().storage_blocked,
        "storage did not block after safe candidates were exhausted",
    )?;
    let rows = db::artifact_rows(&pool).await?;
    for (id, dir) in &protected_artifacts {
        require_acceptance(
            dir.join("sentinel").is_file() && rows.iter().any(|row| row.id == id.to_string()),
            &format!("protected artifact {id} was pruned"),
        )?;
    }
    require_acceptance(
        rows.iter()
            .all(|row| row.id != oldest.0.to_string() && row.id != newer.0.to_string()),
        "an uploaded database row survived pruning",
    )?;

    pool.close().await;
    Ok(serde_json::json!({
        "result": "PASS",
        "fixture_root": fixture_root,
        "protected_artifacts": protected_artifacts.len(),
        "pruned_uploaded": [oldest.0, newer.0],
        "percentage_threshold": "PASS",
        "minimum_free_threshold": "PASS",
        "blocked_without_candidate": true,
    }))
}

#[cfg(feature = "recording-integrity-test-hooks")]
fn validate_fixture_root(
    fixture_root: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let safe = fixture_root.is_absolute()
        && fixture_root.parent() == Some(Path::new("/tmp"))
        && fixture_root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("waved-retention-acceptance-"));
    require_acceptance(
        safe,
        "fixture root must be a direct /tmp/waved-retention-acceptance-* child",
    )
}

#[cfg(feature = "recording-integrity-test-hooks")]
async fn add_acceptance_artifact(
    pool: &SqlitePool,
    recordings_dir: &Path,
    status: crate::model::MeetingStatus,
) -> Result<(uuid::Uuid, std::path::PathBuf), Box<dyn std::error::Error + Send + Sync>> {
    let id = uuid::Uuid::new_v4();
    let dir = recordings_dir.join(id.to_string());
    tokio::fs::create_dir(&dir).await?;
    tokio::fs::write(dir.join("sentinel"), status.as_str()).await?;
    db::insert_meeting(pool, &id, "Omni", &dir.to_string_lossy()).await?;
    db::set_status(pool, &id, status).await?;
    Ok((id, dir))
}

#[cfg(feature = "recording-integrity-test-hooks")]
fn require_acceptance(
    condition: bool,
    message: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MeetingStatus;
    use uuid::Uuid;

    async fn fixture() -> (
        tempfile::TempDir,
        SqlitePool,
        Arc<watch::Sender<MeetingState>>,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let recordings = temp.path().join("recordings");
        tokio::fs::create_dir_all(&recordings).await.unwrap();
        let pool = db::init_pool(temp.path().join("meetings.db").to_str().unwrap())
            .await
            .unwrap();
        let (meeting_tx, _meeting_rx) = watch::channel(MeetingState::default());
        (temp, pool, Arc::new(meeting_tx))
    }

    async fn add_artifact(
        pool: &SqlitePool,
        recordings: &Path,
        status: MeetingStatus,
    ) -> (Uuid, std::path::PathBuf) {
        let id = Uuid::new_v4();
        let dir = recordings.join(id.to_string());
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("sentinel"), status.as_str())
            .await
            .unwrap();
        db::insert_meeting(pool, &id, "Omni", &dir.to_string_lossy())
            .await
            .unwrap();
        db::set_status(pool, &id, status).await.unwrap();
        (id, dir)
    }

    #[test]
    fn either_storage_threshold_triggers_pruning() {
        let config = StorageConfig {
            prune_uploaded_at_percent: 85,
            minimum_free_mb: 2_048,
        };
        assert!(!needs_pruning(
            DiskUsage {
                used_percent: 84,
                free_bytes: 2_048 * 1024 * 1024,
                free_mb: 2_048,
            },
            &config
        ));
        assert!(needs_pruning(
            DiskUsage {
                used_percent: 85,
                free_bytes: 10_000 * 1024 * 1024,
                free_mb: 10_000,
            },
            &config
        ));
        assert!(needs_pruning(
            DiskUsage {
                used_percent: 10,
                free_bytes: 2_047 * 1024 * 1024,
                free_mb: 2_047,
            },
            &config
        ));
    }

    #[tokio::test]
    async fn prunes_only_oldest_uploaded_and_blocks_without_a_safe_candidate() {
        let (temp, pool, meeting_tx) = fixture().await;
        let recordings = temp.path().join("recordings");
        let (oldest_id, oldest_dir) =
            add_artifact(&pool, &recordings, MeetingStatus::Uploaded).await;
        let (newer_id, newer_dir) = add_artifact(&pool, &recordings, MeetingStatus::Uploaded).await;
        let protected = [
            MeetingStatus::Recording,
            MeetingStatus::Finalized,
            MeetingStatus::AwaitingDecision,
            MeetingStatus::LocalOnly,
            MeetingStatus::Queued,
            MeetingStatus::Uploading,
            MeetingStatus::RetryWait,
            MeetingStatus::ConfigurationError,
            MeetingStatus::MissingArtifact,
            MeetingStatus::LegacyUnsupported,
            MeetingStatus::ArtifactError,
        ];
        let mut protected_artifacts = Vec::new();
        for status in protected {
            protected_artifacts.push(add_artifact(&pool, &recordings, status).await);
        }
        let config = StorageConfig {
            prune_uploaded_at_percent: 85,
            minimum_free_mb: 2_048,
        };
        let constrained = DiskUsage {
            used_percent: 85,
            free_bytes: 10_000 * 1024 * 1024,
            free_mb: 10_000,
        };

        assert_eq!(
            enforce_for_usage(&pool, &recordings, &config, &meeting_tx, constrained).await,
            RetentionOutcome::Pruned
        );
        assert!(!oldest_dir.exists());
        assert!(newer_dir.exists());
        assert!(db::artifact_rows(&pool)
            .await
            .unwrap()
            .iter()
            .all(|row| row.id != oldest_id.to_string()));

        assert_eq!(
            enforce_for_usage(&pool, &recordings, &config, &meeting_tx, constrained).await,
            RetentionOutcome::Pruned
        );
        assert!(!newer_dir.exists());
        assert!(db::artifact_rows(&pool)
            .await
            .unwrap()
            .iter()
            .all(|row| row.id != newer_id.to_string()));

        assert_eq!(
            enforce_for_usage(&pool, &recordings, &config, &meeting_tx, constrained).await,
            RetentionOutcome::Blocked
        );
        assert!(meeting_tx.borrow().storage_blocked);
        for (id, dir) in protected_artifacts {
            assert!(dir.join("sentinel").is_file(), "{} was deleted", id);
            assert!(db::artifact_rows(&pool)
                .await
                .unwrap()
                .iter()
                .any(|row| row.id == id.to_string()));
        }

        assert_eq!(
            enforce_for_usage(
                &pool,
                &recordings,
                &config,
                &meeting_tx,
                DiskUsage {
                    used_percent: 84,
                    free_bytes: 2_048 * 1024 * 1024,
                    free_mb: 2_048,
                },
            )
            .await,
            RetentionOutcome::WithinLimits
        );
        assert!(!meeting_tx.borrow().storage_blocked);
    }

    #[tokio::test]
    async fn minimum_free_threshold_prunes_and_outside_path_is_never_removed() {
        let (temp, pool, meeting_tx) = fixture().await;
        let recordings = temp.path().join("recordings");
        let outside = temp.path().join("outside");
        tokio::fs::create_dir_all(&outside).await.unwrap();
        tokio::fs::write(outside.join("sentinel"), b"keep")
            .await
            .unwrap();
        let id = Uuid::new_v4();
        db::insert_meeting(&pool, &id, "Omni", &outside.to_string_lossy())
            .await
            .unwrap();
        db::set_status(&pool, &id, MeetingStatus::Uploaded)
            .await
            .unwrap();
        let config = StorageConfig {
            prune_uploaded_at_percent: 85,
            minimum_free_mb: 2_048,
        };

        assert_eq!(
            enforce_for_usage(
                &pool,
                &recordings,
                &config,
                &meeting_tx,
                DiskUsage {
                    used_percent: 10,
                    free_bytes: 2_047 * 1024 * 1024,
                    free_mb: 2_047,
                },
            )
            .await,
            RetentionOutcome::Blocked
        );
        assert!(outside.join("sentinel").is_file());
        assert!(meeting_tx.borrow().storage_blocked);
        assert!(db::artifact_rows(&pool)
            .await
            .unwrap()
            .iter()
            .any(|row| row.id == id.to_string()));
    }
}
