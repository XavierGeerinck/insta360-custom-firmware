use crate::model::MeetingStatus;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tracing::info;
use uuid::Uuid;

pub async fn init_pool(path: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite:{path}?mode=rwc"))
        .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS meetings (
            id TEXT PRIMARY KEY,
            start_time TEXT NOT NULL,
            end_time TEXT,
            duration_secs INTEGER DEFAULT 0,
            audio_mode TEXT NOT NULL,
            file_path TEXT NOT NULL,
            upload_status TEXT NOT NULL DEFAULT 'recording',
            upload_attempts INTEGER DEFAULT 0,
            attempt_cycle INTEGER DEFAULT 0,
            last_error TEXT,
            last_status_code INTEGER,
            next_attempt_at_ms INTEGER,
            uploaded_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(&pool)
    .await?;

    ensure_column(&pool, "meetings", "attempt_cycle", "INTEGER DEFAULT 0").await?;
    ensure_column(&pool, "meetings", "last_error", "TEXT").await?;
    ensure_column(&pool, "meetings", "last_status_code", "INTEGER").await?;
    ensure_column(&pool, "meetings", "next_attempt_at_ms", "INTEGER").await?;
    ensure_column(&pool, "meetings", "uploaded_at", "TEXT").await?;
    sqlx::query("PRAGMA user_version = 3")
        .execute(&pool)
        .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS bookmarks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            meeting_id TEXT NOT NULL,
            timestamp_secs INTEGER NOT NULL,
            FOREIGN KEY (meeting_id) REFERENCES meetings(id)
        )",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS diarization_segments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            meeting_id TEXT NOT NULL,
            start_ms INTEGER NOT NULL,
            end_ms INTEGER NOT NULL,
            speaker_id INTEGER NOT NULL,
            azimuth_deg REAL NOT NULL,
            FOREIGN KEY (meeting_id) REFERENCES meetings(id)
        )",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_diarization_meeting \
         ON diarization_segments(meeting_id)",
    )
    .execute(&pool)
    .await?;

    info!("meeting database initialized at {path}");
    Ok(pool)
}

async fn ensure_column(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), sqlx::Error> {
    let rows: Vec<(i64, String, String, i64, Option<String>, i64)> =
        sqlx::query_as(&format!("PRAGMA table_info({table})"))
            .fetch_all(pool)
            .await?;
    if rows.iter().any(|row| row.1 == column) {
        return Ok(());
    }
    sqlx::query(&format!(
        "ALTER TABLE {table} ADD COLUMN {column} {definition}"
    ))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_meeting(
    pool: &SqlitePool,
    id: &Uuid,
    audio_mode: &str,
    file_path: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO meetings (id, start_time, audio_mode, file_path, upload_status)
         VALUES (?, datetime('now'), ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(audio_mode)
    .bind(file_path)
    .bind(MeetingStatus::Recording.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn finish_meeting(
    pool: &SqlitePool,
    id: &Uuid,
    duration_secs: u32,
    status: MeetingStatus,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE meetings SET end_time = datetime('now'), duration_secs = ?, \
         upload_status = ?, last_error = NULL, last_status_code = NULL WHERE id = ?",
    )
    .bind(duration_secs)
    .bind(status.as_str())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_upload_status(
    pool: &SqlitePool,
    id: &Uuid,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE meetings SET upload_status = ? WHERE id = ?")
        .bind(status)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_status(
    pool: &SqlitePool,
    id: &Uuid,
    status: MeetingStatus,
) -> Result<(), sqlx::Error> {
    set_upload_status(pool, id, status.as_str()).await
}

pub async fn begin_upload_attempt(pool: &SqlitePool, id: &Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE meetings SET upload_status = ?, \
         upload_attempts = upload_attempts + 1, attempt_cycle = attempt_cycle + 1 \
         WHERE id = ?",
    )
    .bind(MeetingStatus::Uploading.as_str())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct UploadCandidate {
    pub id: Uuid,
    pub dir: String,
    pub attempt_cycle: u32,
}

/// Atomically claim the oldest eligible recording. The conditional update
/// makes concurrent worker wakeups harmless even if another process opens the
/// same database.
pub async fn claim_next_upload(
    pool: &SqlitePool,
    max_attempts: u32,
) -> Result<Option<UploadCandidate>, sqlx::Error> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut transaction = pool.begin().await?;
    let row: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT id, file_path, attempt_cycle FROM meetings \
         WHERE upload_status IN ('queued', 'retry_wait') \
         AND attempt_cycle < ? \
         AND (next_attempt_at_ms IS NULL OR next_attempt_at_ms <= ?) \
         ORDER BY rowid LIMIT 1",
    )
    .bind(i64::from(max_attempts.max(1)))
    .bind(now_ms)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((id_string, dir, attempt_cycle)) = row else {
        transaction.commit().await?;
        return Ok(None);
    };
    let updated = sqlx::query(
        "UPDATE meetings SET upload_status = ?, upload_attempts = upload_attempts + 1, \
         attempt_cycle = attempt_cycle + 1, last_error = NULL, last_status_code = NULL \
         WHERE id = ? AND upload_status IN ('queued', 'retry_wait') \
         AND attempt_cycle < ?",
    )
    .bind(MeetingStatus::Uploading.as_str())
    .bind(&id_string)
    .bind(i64::from(max_attempts.max(1)))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    if updated.rows_affected() != 1 {
        return Ok(None);
    }
    let Ok(id) = id_string.parse() else {
        return Ok(None);
    };
    Ok(Some(UploadCandidate {
        id,
        dir,
        attempt_cycle: attempt_cycle.max(0) as u32 + 1,
    }))
}

pub async fn mark_upload_failure(
    pool: &SqlitePool,
    id: &Uuid,
    status: MeetingStatus,
    error: &str,
    status_code: Option<u16>,
    next_attempt_at_ms: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE meetings SET upload_status = ?, last_error = ?, \
         last_status_code = ?, next_attempt_at_ms = ? WHERE id = ?",
    )
    .bind(status.as_str())
    .bind(truncate_diagnostic(error))
    .bind(status_code.map(i64::from))
    .bind(next_attempt_at_ms)
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn reset_attempt_cycle(pool: &SqlitePool, id: &Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE meetings SET upload_status = ?, attempt_cycle = 0, \
         last_error = NULL, last_status_code = NULL, next_attempt_at_ms = NULL \
         WHERE id = ?",
    )
    .bind(MeetingStatus::Queued.as_str())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn reset_retryable_failures(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE meetings SET upload_status = ?, attempt_cycle = 0, \
         last_error = NULL, last_status_code = NULL, next_attempt_at_ms = NULL \
         WHERE upload_status IN ('retry_wait', 'configuration_error')",
    )
    .bind(MeetingStatus::Queued.as_str())
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn reset_configuration_failures(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE meetings SET upload_status = ?, attempt_cycle = 0, \
         last_error = NULL, last_status_code = NULL, next_attempt_at_ms = NULL \
         WHERE upload_status = 'configuration_error'",
    )
    .bind(MeetingStatus::Queued.as_str())
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn mark_uploaded(pool: &SqlitePool, id: &Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE meetings SET upload_status = ?, uploaded_at = datetime('now'), \
         last_error = NULL, last_status_code = NULL, next_attempt_at_ms = NULL WHERE id = ?",
    )
    .bind(MeetingStatus::Uploaded.as_str())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn last_upload_success(pool: &SqlitePool) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT COALESCE(uploaded_at, end_time, created_at) FROM meetings \
         WHERE upload_status = 'uploaded' ORDER BY rowid DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(timestamp,)| timestamp))
}

pub async fn upload_counts(
    pool: &SqlitePool,
    max_attempts: u32,
) -> Result<(u32, u32), sqlx::Error> {
    let (pending, failed): (i64, i64) = sqlx::query_as(
        "SELECT \
         COALESCE(SUM(CASE WHEN upload_status IN ('queued','uploading') \
             OR (upload_status = 'retry_wait' AND attempt_cycle < ?) \
             THEN 1 ELSE 0 END), 0), \
         COALESCE(SUM(CASE WHEN upload_status IN ('configuration_error','missing_artifact',\
             'legacy_unsupported','artifact_error') \
             OR (upload_status = 'retry_wait' AND attempt_cycle >= ?) \
             THEN 1 ELSE 0 END), 0) \
         FROM meetings",
    )
    .bind(i64::from(max_attempts.max(1)))
    .bind(i64::from(max_attempts.max(1)))
    .fetch_one(pool)
    .await?;
    Ok((pending.max(0) as u32, failed.max(0) as u32))
}

pub async fn get_pending_uploads(pool: &SqlitePool) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, file_path FROM meetings \
         WHERE upload_status IN ('queued', 'retry_wait', 'pending', 'upload_failed') \
         ORDER BY rowid ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtifactRow {
    pub id: String,
    pub dir: String,
    pub status: String,
}

pub async fn artifact_rows(pool: &SqlitePool) -> Result<Vec<ArtifactRow>, sqlx::Error> {
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, file_path, upload_status FROM meetings ORDER BY rowid")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, dir, status)| ArtifactRow { id, dir, status })
        .collect())
}

pub async fn import_local_artifact(
    pool: &SqlitePool,
    id: &Uuid,
    dir: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO meetings \
         (id, start_time, end_time, audio_mode, file_path, upload_status) \
         VALUES (?, datetime('now'), datetime('now'), 'unknown', ?, ?)",
    )
    .bind(id.to_string())
    .bind(dir)
    .bind(MeetingStatus::LocalOnly.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn oldest_uploaded_artifact(
    pool: &SqlitePool,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT id, file_path FROM meetings WHERE upload_status = 'uploaded' \
         ORDER BY rowid LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(id, dir)| id.parse().ok().map(|id| (id, dir))))
}

#[derive(Clone, Debug, PartialEq)]
pub struct BookmarkRecord {
    pub timestamp_secs: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpeakerSegmentRecord {
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker_id: i64,
    pub azimuth_deg: f64,
}

pub async fn get_bookmarks(
    pool: &SqlitePool,
    meeting_id: &Uuid,
) -> Result<Vec<BookmarkRecord>, sqlx::Error> {
    let rows: Vec<(i64,)> =
        sqlx::query_as("SELECT timestamp_secs FROM bookmarks WHERE meeting_id = ? ORDER BY id")
            .bind(meeting_id.to_string())
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(timestamp_secs,)| BookmarkRecord {
            timestamp_secs: timestamp_secs.max(0) as u32,
        })
        .collect())
}

pub async fn get_speaker_segments(
    pool: &SqlitePool,
    meeting_id: &Uuid,
) -> Result<Vec<SpeakerSegmentRecord>, sqlx::Error> {
    let rows: Vec<(i64, i64, i64, f64)> = sqlx::query_as(
        "SELECT start_ms, end_ms, speaker_id, azimuth_deg \
         FROM diarization_segments WHERE meeting_id = ? ORDER BY id",
    )
    .bind(meeting_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(start_ms, end_ms, speaker_id, azimuth_deg)| SpeakerSegmentRecord {
                start_ms,
                end_ms,
                speaker_id,
                azimuth_deg,
            },
        )
        .collect())
}

pub async fn add_bookmark(
    pool: &SqlitePool,
    meeting_id: &Uuid,
    timestamp_secs: u32,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO bookmarks (meeting_id, timestamp_secs) VALUES (?, ?)")
        .bind(meeting_id.to_string())
        .bind(timestamp_secs)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn insert_diarization_segment(
    pool: &SqlitePool,
    seg: &crate::diarization::DiarizationSegment,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO diarization_segments \
         (meeting_id, start_ms, end_ms, speaker_id, azimuth_deg) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(seg.meeting_id.to_string())
    .bind(seg.start_ms)
    .bind(seg.end_ms)
    .bind(seg.speaker_id)
    .bind(seg.azimuth_deg)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a meeting and all its bookmarks and diarization segments atomically.
/// Used by `Command::DiscardRecording`. NotFound is a no-op (returns Ok).
pub async fn delete_meeting_cascade(pool: &SqlitePool, id: &Uuid) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM diarization_segments WHERE meeting_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM bookmarks WHERE meeting_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM meetings WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

fn truncate_diagnostic(value: &str) -> String {
    let mut output = value
        .chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .take(512)
        .collect::<String>();
    if value.chars().count() > 512 {
        output.push('…');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diarization::DiarizationSegment;
    use uuid::Uuid;

    async fn fresh_pool() -> SqlitePool {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db").to_string_lossy().into_owned();
        // The tempdir must outlive the pool; leak it here for test purposes.
        Box::leak(Box::new(dir));
        init_pool(&path).await.unwrap()
    }

    #[tokio::test]
    async fn migrates_the_legacy_meetings_schema_without_losing_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db").to_string_lossy().into_owned();
        let legacy = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite:{path}?mode=rwc"))
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE meetings (
                id TEXT PRIMARY KEY,
                start_time TEXT NOT NULL,
                end_time TEXT,
                duration_secs INTEGER DEFAULT 0,
                audio_mode TEXT NOT NULL,
                file_path TEXT NOT NULL,
                upload_status TEXT NOT NULL DEFAULT 'pending',
                upload_attempts INTEGER DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .execute(&legacy)
        .await
        .unwrap();
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO meetings (id, start_time, audio_mode, file_path) \
             VALUES (?, datetime('now'), 'omni', '/tmp/legacy')",
        )
        .bind(id.to_string())
        .execute(&legacy)
        .await
        .unwrap();
        legacy.close().await;

        let migrated = init_pool(&path).await.unwrap();
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM meetings WHERE id = ?")
            .bind(id.to_string())
            .fetch_one(&migrated)
            .await
            .unwrap();
        let (version,): (i64,) = sqlx::query_as("PRAGMA user_version")
            .fetch_one(&migrated)
            .await
            .unwrap();
        let columns: Vec<(i64, String, String, i64, Option<String>, i64)> =
            sqlx::query_as("PRAGMA table_info(meetings)")
                .fetch_all(&migrated)
                .await
                .unwrap();
        assert_eq!(count, 1);
        assert_eq!(version, 3);
        for expected in [
            "attempt_cycle",
            "last_error",
            "last_status_code",
            "next_attempt_at_ms",
            "uploaded_at",
        ] {
            assert!(
                columns.iter().any(|column| column.1 == expected),
                "missing migrated column {expected}"
            );
        }
    }

    #[tokio::test]
    async fn claim_is_atomic_and_attempt_count_changes_only_on_claim() {
        let pool = fresh_pool().await;
        let id = Uuid::new_v4();
        insert_meeting(&pool, &id, "omni", "/tmp/artifact")
            .await
            .unwrap();
        set_status(&pool, &id, MeetingStatus::Queued).await.unwrap();
        let (before,): (i64,) = sqlx::query_as("SELECT upload_attempts FROM meetings WHERE id = ?")
            .bind(id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
        let claim = claim_next_upload(&pool, 5).await.unwrap().unwrap();
        let second = claim_next_upload(&pool, 5).await.unwrap();
        let (after, status): (i64, String) =
            sqlx::query_as("SELECT upload_attempts, upload_status FROM meetings WHERE id = ?")
                .bind(id.to_string())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(before, 0);
        assert_eq!(claim.id, id);
        assert!(second.is_none());
        assert_eq!(after, 1);
        assert_eq!(status, MeetingStatus::Uploading.as_str());
    }

    #[tokio::test]
    async fn delete_meeting_cascade_removes_meeting_and_bookmarks() {
        let pool = fresh_pool().await;
        let id = Uuid::new_v4();
        insert_meeting(&pool, &id, "Omni", "/tmp/x").await.unwrap();
        add_bookmark(&pool, &id, 5).await.unwrap();
        add_bookmark(&pool, &id, 12).await.unwrap();

        delete_meeting_cascade(&pool, &id).await.unwrap();

        let meeting_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM meetings WHERE id = ?")
            .bind(id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
        let bookmark_count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM bookmarks WHERE meeting_id = ?")
                .bind(id.to_string())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(meeting_count.0, 0);
        assert_eq!(bookmark_count.0, 0);
    }

    #[tokio::test]
    async fn delete_meeting_cascade_is_noop_for_unknown_id() {
        let pool = fresh_pool().await;
        let id = Uuid::new_v4();
        // No insert. Should still succeed.
        delete_meeting_cascade(&pool, &id).await.unwrap();
    }

    #[tokio::test]
    async fn diarization_segments_table_is_created() {
        let pool = fresh_pool().await;
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='table' AND name='diarization_segments'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, 1, "diarization_segments table missing");
    }

    #[tokio::test]
    async fn insert_and_query_diarization_segment() {
        let pool = fresh_pool().await;
        let mid = Uuid::new_v4();
        insert_meeting(&pool, &mid, "Omni", "/tmp/x").await.unwrap();
        let seg = DiarizationSegment {
            meeting_id: mid,
            start_ms: 1000,
            end_ms: 3500,
            speaker_id: 2,
            azimuth_deg: 137.5,
        };
        insert_diarization_segment(&pool, &seg).await.unwrap();
        let rows: Vec<(i64, String, i64, i64, i64, f64)> = sqlx::query_as(
            "SELECT id, meeting_id, start_ms, end_ms, speaker_id, azimuth_deg \
             FROM diarization_segments WHERE meeting_id = ?",
        )
        .bind(mid.to_string())
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, mid.to_string());
        assert_eq!(rows[0].2, 1000);
        assert_eq!(rows[0].3, 3500);
        assert_eq!(rows[0].4, 2);
        assert!((rows[0].5 - 137.5).abs() < 0.001);
    }

    #[tokio::test]
    async fn multiple_meetings_isolated_in_diarization() {
        let pool = fresh_pool().await;
        let m1 = Uuid::new_v4();
        let m2 = Uuid::new_v4();
        insert_meeting(&pool, &m1, "Omni", "/tmp/x").await.unwrap();
        insert_meeting(&pool, &m2, "Omni", "/tmp/y").await.unwrap();
        for i in 0..3 {
            let seg = DiarizationSegment {
                meeting_id: m1,
                start_ms: i * 1000,
                end_ms: i * 1000 + 500,
                speaker_id: 1,
                azimuth_deg: 0.0,
            };
            insert_diarization_segment(&pool, &seg).await.unwrap();
        }
        let seg = DiarizationSegment {
            meeting_id: m2,
            start_ms: 0,
            end_ms: 1000,
            speaker_id: 1,
            azimuth_deg: 180.0,
        };
        insert_diarization_segment(&pool, &seg).await.unwrap();

        let cnt_m1: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM diarization_segments WHERE meeting_id = ?")
                .bind(m1.to_string())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(cnt_m1.0, 3);
        let cnt_m2: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM diarization_segments WHERE meeting_id = ?")
                .bind(m2.to_string())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(cnt_m2.0, 1);
    }

    #[tokio::test]
    async fn delete_meeting_cascade_removes_diarization() {
        let pool = fresh_pool().await;
        let mid = Uuid::new_v4();
        insert_meeting(&pool, &mid, "Omni", "/tmp/x").await.unwrap();
        let seg = DiarizationSegment {
            meeting_id: mid,
            start_ms: 0,
            end_ms: 500,
            speaker_id: 1,
            azimuth_deg: 0.0,
        };
        insert_diarization_segment(&pool, &seg).await.unwrap();

        delete_meeting_cascade(&pool, &mid).await.unwrap();

        let cnt: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM diarization_segments WHERE meeting_id = ?")
                .bind(mid.to_string())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(cnt.0, 0, "cascade-delete should remove diarization rows");
    }
}
