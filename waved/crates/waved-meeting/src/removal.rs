//! Audit-safe removal of a recording (Files → Delete, After stop → Delete).
//!
//! The order guarantees that a power cut at any step never brings a deleted
//! recording back and never leaves one half deleted:
//! 1. claim the row (`deleting`). The upload worker never picks such a row,
//!    and rows that are uploading right now are refused;
//! 2. rename `<id>/` to `.deleted-<id>/` and sync the parent directory.
//!    Reconciliation never imports it because the name is not a UUID;
//! 3. delete the database rows (meeting, bookmarks, speaker segments);
//! 4. remove the renamed directory.
//!
//! `finish_interrupted` completes steps 2–4 at boot for anything left over.

use crate::db::{self, RemovalClaim};
use crate::model::MeetingStatus;
use sqlx::SqlitePool;
use std::path::Path;
use tracing::{info, warn};
use uuid::Uuid;

pub(crate) const DELETED_PREFIX: &str = ".deleted-";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoveOutcome {
    Removed { previous_status: String },
    Uploading,
    NotFound,
}

pub(crate) async fn remove_recording(
    pool: &SqlitePool,
    recordings_dir: &Path,
    id: Uuid,
) -> std::io::Result<RemoveOutcome> {
    let previous_status = match db::claim_for_removal(pool, &id)
        .await
        .map_err(std::io::Error::other)?
    {
        RemovalClaim::Claimed { previous_status } => previous_status,
        RemovalClaim::Uploading => return Ok(RemoveOutcome::Uploading),
        RemovalClaim::NotFound => return Ok(RemoveOutcome::NotFound),
    };
    finish_one(pool, recordings_dir, id).await?;
    Ok(RemoveOutcome::Removed { previous_status })
}

/// Steps 2–4 for a row the caller already claimed (`deleting`).
pub(crate) async fn finish_claimed(
    pool: &SqlitePool,
    recordings_dir: &Path,
    id: Uuid,
) -> std::io::Result<()> {
    finish_one(pool, recordings_dir, id).await
}

/// Step 2 alone, for when the claim cannot be saved: boot finishes the
/// delete of a `.deleted-<id>` directory whatever its row says.
pub(crate) async fn move_aside(recordings_dir: &Path, id: Uuid) -> std::io::Result<()> {
    let parent = recordings_dir.to_path_buf();
    let trash = recordings_dir.join(format!("{DELETED_PREFIX}{id}"));
    tokio::task::spawn_blocking(move || move_to_trash(&parent, id, &trash))
        .await
        .map_err(std::io::Error::other)?
}

async fn finish_one(pool: &SqlitePool, recordings_dir: &Path, id: Uuid) -> std::io::Result<()> {
    let trash = recordings_dir.join(format!("{DELETED_PREFIX}{id}"));
    let parent = recordings_dir.to_path_buf();
    let target = trash.clone();
    tokio::task::spawn_blocking(move || move_to_trash(&parent, id, &target))
        .await
        .map_err(std::io::Error::other)??;
    db::delete_meeting_cascade(pool, &id)
        .await
        .map_err(std::io::Error::other)?;
    match tokio::fs::remove_dir_all(&trash).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Rename `<id>/` to the trash name and make the rename durable. Blocking.
fn move_to_trash(recordings_dir: &Path, id: Uuid, trash: &Path) -> std::io::Result<()> {
    match std::fs::rename(recordings_dir.join(id.to_string()), trash) {
        Ok(()) => std::fs::File::open(recordings_dir)?.sync_all(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Finish deletes a power cut interrupted: rows still marked `deleting`
/// and `.deleted-<id>` directories. Runs before reconciliation.
pub(crate) async fn finish_interrupted(pool: &SqlitePool, recordings_dir: &Path) -> u32 {
    let mut ids = match db::ids_with_status(pool, MeetingStatus::Deleting).await {
        Ok(ids) => ids,
        Err(error) => {
            warn!("cannot list interrupted deletes: {error}");
            Vec::new()
        }
    };
    if let Ok(mut entries) = tokio::fs::read_dir(recordings_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(rest) = name
                .to_str()
                .and_then(|name| name.strip_prefix(DELETED_PREFIX))
            else {
                continue;
            };
            match rest.parse::<Uuid>() {
                Ok(id) if !ids.contains(&id) => ids.push(id),
                Ok(_) => {}
                Err(_) => warn!("leaving unexpected {} in place", entry.path().display()),
            }
        }
    }
    let mut finished = 0;
    for id in ids {
        match finish_one(pool, recordings_dir, id).await {
            Ok(()) => {
                finished += 1;
                info!("audit: finished the interrupted delete of recording {id}");
            }
            Err(error) => warn!("could not finish deleting recording {id}: {error}"),
        }
    }
    finished
}
