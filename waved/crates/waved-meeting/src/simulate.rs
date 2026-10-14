//! Simulated meeting upload — writes to local files instead of HTTP.
use std::path::Path;
use tracing::info;

pub async fn upload_recording(
    _endpoint: &str,
    _auth_token: &str,
    meeting_id: &str,
    file_path: &Path,
) -> Result<(), crate::upload::UploadError> {
    let dest_dir = std::path::PathBuf::from("./test_uploads");
    tokio::fs::create_dir_all(&dest_dir)
        .await
        .map_err(crate::upload::UploadError::Io)?;
    let (file_name, _) = crate::upload::audio_part(meeting_id, file_path);
    let dest = dest_dir.join(file_name);
    if file_path.exists() {
        tokio::fs::copy(file_path, &dest)
            .await
            .map_err(crate::upload::UploadError::Io)?;
        info!("[SIM] uploaded {meeting_id} -> {}", dest.display());
    } else {
        info!("[SIM] upload {meeting_id}: source file not found, skipping copy");
    }
    Ok(())
}
