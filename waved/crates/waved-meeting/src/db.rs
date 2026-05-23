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
            upload_status TEXT NOT NULL DEFAULT 'pending',
            upload_attempts INTEGER DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
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

pub async fn insert_meeting(
    pool: &SqlitePool,
    id: &Uuid,
    audio_mode: &str,
    file_path: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO meetings (id, start_time, audio_mode, file_path, upload_status)
         VALUES (?, datetime('now'), ?, ?, 'pending')",
    )
    .bind(id.to_string())
    .bind(audio_mode)
    .bind(file_path)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn finish_meeting(
    pool: &SqlitePool,
    id: &Uuid,
    duration_secs: u32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE meetings SET end_time = datetime('now'), duration_secs = ? WHERE id = ?",
    )
    .bind(duration_secs)
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
    sqlx::query("UPDATE meetings SET upload_status = ?, upload_attempts = upload_attempts + 1 WHERE id = ?")
        .bind(status)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_pending_uploads(pool: &SqlitePool) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, file_path FROM meetings WHERE upload_status = 'pending' OR upload_status = 'upload_failed' ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
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
pub async fn delete_meeting_cascade(
    pool: &SqlitePool,
    id: &Uuid,
) -> Result<(), sqlx::Error> {
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
    async fn delete_meeting_cascade_removes_meeting_and_bookmarks() {
        let pool = fresh_pool().await;
        let id = Uuid::new_v4();
        insert_meeting(&pool, &id, "Omni", "/tmp/x").await.unwrap();
        add_bookmark(&pool, &id, 5).await.unwrap();
        add_bookmark(&pool, &id, 12).await.unwrap();

        delete_meeting_cascade(&pool, &id).await.unwrap();

        let meeting_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM meetings WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        let bookmark_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM bookmarks WHERE meeting_id = ?",
        )
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
                start_ms: i * 1000, end_ms: i * 1000 + 500,
                speaker_id: 1, azimuth_deg: 0.0,
            };
            insert_diarization_segment(&pool, &seg).await.unwrap();
        }
        let seg = DiarizationSegment {
            meeting_id: m2, start_ms: 0, end_ms: 1000,
            speaker_id: 1, azimuth_deg: 180.0,
        };
        insert_diarization_segment(&pool, &seg).await.unwrap();

        let cnt_m1: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM diarization_segments WHERE meeting_id = ?",
        )
        .bind(m1.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(cnt_m1.0, 3);
        let cnt_m2: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM diarization_segments WHERE meeting_id = ?",
        )
        .bind(m2.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(cnt_m2.0, 1);
    }

    #[tokio::test]
    async fn delete_meeting_cascade_removes_diarization() {
        let pool = fresh_pool().await;
        let mid = Uuid::new_v4();
        insert_meeting(&pool, &mid, "Omni", "/tmp/x").await.unwrap();
        let seg = DiarizationSegment {
            meeting_id: mid, start_ms: 0, end_ms: 500,
            speaker_id: 1, azimuth_deg: 0.0,
        };
        insert_diarization_segment(&pool, &seg).await.unwrap();

        delete_meeting_cascade(&pool, &mid).await.unwrap();

        let cnt: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM diarization_segments WHERE meeting_id = ?",
        )
        .bind(mid.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(cnt.0, 0, "cascade-delete should remove diarization rows");
    }
}
