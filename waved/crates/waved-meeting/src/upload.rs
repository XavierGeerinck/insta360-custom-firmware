use reqwest::multipart;
use std::path::Path;
use tracing::{error, info};

pub async fn upload_recording(
    endpoint: &str,
    auth_token: &str,
    meeting_id: &str,
    file_path: &Path,
) -> Result<(), UploadError> {
    let file_bytes = tokio::fs::read(file_path).await.map_err(UploadError::Io)?;
    let file_name = file_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let metadata_path = file_path.with_file_name("metadata.json");
    let metadata_bytes = if metadata_path.exists() {
        tokio::fs::read(&metadata_path).await.ok()
    } else {
        None
    };

    let mut form = multipart::Form::new()
        .text("meeting_id", meeting_id.to_string())
        .part(
            "audio",
            multipart::Part::bytes(file_bytes)
                .file_name(file_name)
                .mime_str("audio/wav")
                .unwrap(),
        );

    if let Some(meta) = metadata_bytes {
        form = form.part(
            "metadata",
            multipart::Part::bytes(meta)
                .file_name("metadata.json")
                .mime_str("application/json")
                .unwrap(),
        );
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(endpoint)
        .bearer_auth(auth_token)
        .multipart(form)
        .send()
        .await
        .map_err(UploadError::Http)?;

    if resp.status().is_success() {
        info!("uploaded meeting {meeting_id} successfully");
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        error!("upload failed: {status} -- {body}");
        Err(UploadError::ServerError(status.as_u16(), body))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server error {0}: {1}")]
    ServerError(u16, String),
}
