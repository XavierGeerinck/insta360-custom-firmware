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
/// No `<id>/` is fine (moved aside before, or never written), but not when
/// the recordings directory itself is missing: the volume is not mounted
/// (G1), and the recording would come back with it. The row then stays
/// `deleting` until a start-up with the volume finishes it.
fn move_to_trash(recordings_dir: &Path, id: Uuid, trash: &Path) -> std::io::Result<()> {
    match std::fs::rename(recordings_dir.join(id.to_string()), trash) {
        Ok(()) => std::fs::File::open(recordings_dir)?.sync_all(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && recordings_dir.is_dir() => {
            Ok(())
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_delete_with_the_volume_away_stays_claimed_until_it_is_back() {
        // The recordings volume is not mounted (G1): its recordings
        // directory is not there. Deleting the rows now would bring the
        // recording back as an orphan once the volume is mounted.
        let temp = tempfile::tempdir().unwrap();
        let pool = db::init_pool(temp.path().join("meetings.db").to_str().unwrap())
            .await
            .unwrap();
        let recordings = temp.path().join("recordings");
        let id = Uuid::new_v4();
        let dir = recordings.join(id.to_string());
        db::insert_meeting(&pool, &id, "omni", &dir.to_string_lossy())
            .await
            .unwrap();
        db::set_status(&pool, &id, MeetingStatus::LocalOnly).await.unwrap();

        assert!(remove_recording(&pool, &recordings, id).await.is_err());
        assert_eq!(
            db::ids_with_status(&pool, MeetingStatus::Deleting).await.unwrap(),
            [id]
        );

        // Mounted again, files and all: the delete completes.
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("audio.wav"), b"wav").await.unwrap();
        assert_eq!(finish_interrupted(&pool, &recordings).await, 1);
        assert!(!dir.exists());
        assert!(db::ids_with_status(&pool, MeetingStatus::Deleting)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn a_recording_already_moved_aside_still_deletes() {
        let temp = tempfile::tempdir().unwrap();
        let pool = db::init_pool(temp.path().join("meetings.db").to_str().unwrap())
            .await
            .unwrap();
        let recordings = temp.path().join("recordings");
        tokio::fs::create_dir_all(&recordings).await.unwrap();
        let id = Uuid::new_v4();
        db::insert_meeting(&pool, &id, "omni", &recordings.join(id.to_string()).to_string_lossy())
            .await
            .unwrap();
        db::set_status(&pool, &id, MeetingStatus::LocalOnly).await.unwrap();

        assert!(matches!(
            remove_recording(&pool, &recordings, id).await.unwrap(),
            RemoveOutcome::Removed { .. }
        ));
        assert!(db::ids_with_status(&pool, MeetingStatus::Deleting)
            .await
            .unwrap()
            .is_empty());
    }
}
