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
struct DiskUsage {
    used_percent: u8,
    free_mb: u64,
}

pub async fn enforce(
    pool: &SqlitePool,
    recordings_dir: &Path,
    config: &StorageConfig,
    meeting_tx: &Arc<watch::Sender<MeetingState>>,
) {
    loop {
        let usage = match disk_usage(recordings_dir) {
            Ok(usage) => usage,
            Err(error) => {
                warn!("cannot inspect recording storage: {error}");
                return;
            }
        };
        if !needs_pruning(usage, config) {
            meeting_tx.send_modify(|state| state.storage_blocked = false);
            return;
        }

        let candidate = match db::oldest_uploaded_artifact(pool).await {
            Ok(candidate) => candidate,
            Err(error) => {
                warn!("cannot select uploaded recording for pruning: {error}");
                return;
            }
        };
        let Some((id, dir)) = candidate else {
            meeting_tx.send_modify(|state| state.storage_blocked = true);
            warn!(
                "recording storage is constrained ({}% used, {} MiB free) \
                 and no delivered recording is safe to prune",
                usage.used_percent, usage.free_mb
            );
            return;
        };

        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                warn!("failed to prune delivered recording {id} at {dir}: {error}");
                meeting_tx.send_modify(|state| state.storage_blocked = true);
                return;
            }
        }
        if let Err(error) = db::delete_meeting_cascade(pool, &id).await {
            warn!("pruned files for {id} but failed to remove database row: {error}");
            meeting_tx.send_modify(|state| state.storage_blocked = true);
            return;
        }
        info!("pruned delivered recording {id} under storage pressure");
    }
}

fn disk_usage(path: &Path) -> Result<DiskUsage, nix::Error> {
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
    let free_mb = (stats.blocks_available() as u64)
        .saturating_mul(fragment_size)
        .saturating_div(1024 * 1024);
    Ok(DiskUsage {
        used_percent,
        free_mb,
    })
}

fn needs_pruning(usage: DiskUsage, config: &StorageConfig) -> bool {
    usage.used_percent >= config.prune_uploaded_at_percent || usage.free_mb < config.minimum_free_mb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn either_storage_threshold_triggers_pruning() {
        let config = StorageConfig {
            prune_uploaded_at_percent: 85,
            minimum_free_mb: 2_048,
        };
        assert!(!needs_pruning(
            DiskUsage {
                used_percent: 84,
                free_mb: 2_048,
            },
            &config
        ));
        assert!(needs_pruning(
            DiskUsage {
                used_percent: 85,
                free_mb: 10_000,
            },
            &config
        ));
        assert!(needs_pruning(
            DiskUsage {
                used_percent: 10,
                free_mb: 2_047,
            },
            &config
        ));
    }
}
