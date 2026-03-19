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
