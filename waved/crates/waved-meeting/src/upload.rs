use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use reqwest::{multipart, redirect, Client, StatusCode};
use std::path::Path;
use tokio_util::io::ReaderStream;
use tracing::info;

const MAX_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 4 * 1024;
const USER_AGENT_VALUE: &str = concat!("waved/", env!("CARGO_PKG_VERSION"));

#[derive(Clone)]
pub struct WebhookClient {
    client: Client,
}

impl WebhookClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .redirect(redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(30 * 60))
            .build()?;
        Ok(Self { client })
    }

    pub async fn upload_recording(
        &self,
        endpoint: &str,
        auth_token: &str,
        device_id: &str,
        meeting_id: &str,
        audio_path: &Path,
    ) -> Result<(), UploadError> {
        if endpoint.trim().is_empty() {
            return Err(UploadError::Configuration(
                "webhook endpoint is not configured".into(),
            ));
        }

        let metadata_path = audio_path.with_file_name("metadata.json");
        let audio = tokio::fs::File::open(audio_path)
            .await
            .map_err(UploadError::Io)?;
        let audio_length = audio.metadata().await.map_err(UploadError::Io)?.len();
        let metadata_length = tokio::fs::metadata(&metadata_path)
            .await
            .map_err(UploadError::Io)?
            .len();
        if metadata_length > MAX_METADATA_BYTES {
            return Err(UploadError::Configuration(format!(
                "metadata exceeds {MAX_METADATA_BYTES} bytes"
            )));
        }
        let metadata = tokio::fs::read(&metadata_path)
            .await
            .map_err(UploadError::Io)?;
        serde_json::from_slice::<serde_json::Value>(&metadata)
            .map_err(|error| UploadError::Configuration(format!("invalid metadata: {error}")))?;

        let audio_filename = format!("{meeting_id}.wav");
        let audio_part = multipart::Part::stream_with_length(
            reqwest::Body::wrap_stream(ReaderStream::new(audio)),
            audio_length,
        )
        .file_name(audio_filename)
        .mime_str("audio/wav")
        .map_err(UploadError::Http)?;
        let metadata_part = multipart::Part::bytes(metadata)
            .file_name("metadata.json")
            .mime_str("application/json")
            .map_err(UploadError::Http)?;
        let form = multipart::Form::new()
            .text("meeting_id", meeting_id.to_string())
            .part("metadata", metadata_part)
            .part("audio", audio_part);

        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
        headers.insert("Idempotency-Key", header_value(meeting_id, "meeting id")?);
        headers.insert("X-Wave-Device-ID", header_value(device_id, "device id")?);
        if !auth_token.is_empty() {
            let authorization = format!("Bearer {auth_token}");
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&authorization).map_err(|_| {
                    UploadError::Configuration("auth token is not a valid HTTP header".into())
                })?,
            );
        }

        let mut response = self
            .client
            .post(endpoint)
            .headers(headers)
            .multipart(form)
            .send()
            .await
            .map_err(classify_reqwest)?;
        let status = response.status();
        if status.is_success() {
            info!("uploaded meeting {meeting_id} successfully");
            return Ok(());
        }
        let diagnostic = bounded_diagnostic(&mut response).await;
        if is_transient_status(status) {
            Err(UploadError::Transient {
                status: Some(status.as_u16()),
                message: diagnostic,
            })
        } else {
            Err(UploadError::ConfigurationResponse {
                status: status.as_u16(),
                message: diagnostic,
            })
        }
    }

    pub async fn send_test(
        &self,
        endpoint: &str,
        auth_token: &str,
        device_id: &str,
        work_dir: &Path,
    ) -> Result<(), UploadError> {
        let test_id = uuid::Uuid::new_v4().to_string();
        let dir = work_dir.join(format!(".webhook-test-{test_id}"));
        tokio::fs::create_dir_all(&dir).await?;
        let audio_path = dir.join("audio.wav");
        let result = async {
            tokio::fs::write(&audio_path, silent_wav()).await?;
            let metadata = serde_json::json!({
                "schema_version": 1,
                "event": "webhook.test",
                "meeting_id": test_id,
                "device": {
                    "id": device_id,
                    "software_version": env!("CARGO_PKG_VERSION")
                },
                "audio": {
                    "filename": format!("{test_id}.wav"),
                    "content_type": "audio/wav",
                    "codec": "pcm_s16le",
                    "sample_rate_hz": 48_000,
                    "channels": 1,
                    "bit_depth": 16
                }
            });
            tokio::fs::write(
                dir.join("metadata.json"),
                serde_json::to_vec_pretty(&metadata).map_err(|error| {
                    UploadError::Configuration(format!("cannot serialize test metadata: {error}"))
                })?,
            )
            .await?;
            self.upload_recording(endpoint, auth_token, device_id, &test_id, &audio_path)
                .await
        }
        .await;
        if let Err(error) = tokio::fs::remove_dir_all(&dir).await {
            tracing::warn!(
                "failed to remove webhook test artifact {}: {error}",
                dir.display()
            );
        }
        result
    }
}

fn silent_wav() -> Vec<u8> {
    const SAMPLE_COUNT: u32 = 4_800;
    const DATA_BYTES: u32 = SAMPLE_COUNT * 2;
    let mut bytes = Vec::with_capacity((44 + DATA_BYTES) as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + DATA_BYTES).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&(48_000_u32 * 2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&DATA_BYTES.to_le_bytes());
    bytes.resize((44 + DATA_BYTES) as usize, 0);
    bytes
}

fn header_value(value: &str, name: &str) -> Result<HeaderValue, UploadError> {
    HeaderValue::from_str(value)
        .map_err(|_| UploadError::Configuration(format!("{name} is not a valid HTTP header")))
}

fn classify_reqwest(error: reqwest::Error) -> UploadError {
    if error.is_builder() {
        UploadError::Configuration(error.to_string())
    } else {
        UploadError::Transient {
            status: error.status().map(|status| status.as_u16()),
            message: error.to_string(),
        }
    }
}

fn is_transient_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

async fn bounded_diagnostic(response: &mut reqwest::Response) -> String {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let mut bytes = Vec::new();
    while bytes.len() < MAX_DIAGNOSTIC_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = MAX_DIAGNOSTIC_BYTES - bytes.len();
                bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    let sanitized = text
        .chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .collect::<String>();
    if sanitized.is_empty() {
        format!("HTTP {} ({content_type})", response.status())
    } else {
        sanitized
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("artifact IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP request error: {0}")]
    Http(reqwest::Error),
    #[error("webhook configuration error: {0}")]
    Configuration(String),
    #[error("transient webhook error{status_suffix}: {message}", status_suffix = status.map(|value| format!(" ({value})")).unwrap_or_default())]
    Transient {
        status: Option<u16>,
        message: String,
    },
    #[error("webhook rejected request ({status}): {message}")]
    ConfigurationResponse { status: u16, message: String },
}

impl UploadError {
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Transient { .. })
    }

    pub fn status_code(&self) -> Option<u16> {
        match self {
            Self::Transient { status, .. } => *status,
            Self::ConfigurationResponse { status, .. } => Some(*status),
            Self::Io(_) | Self::Http(_) | Self::Configuration(_) => None,
        }
    }
}

pub async fn upload_recording(
    endpoint: &str,
    auth_token: &str,
    meeting_id: &str,
    file_path: &Path,
) -> Result<(), UploadError> {
    WebhookClient::new()
        .map_err(UploadError::Http)?
        .upload_recording(endpoint, auth_token, "wave-unknown", meeting_id, file_path)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_classification_is_intentionally_narrow() {
        assert!(is_transient_status(StatusCode::REQUEST_TIMEOUT));
        assert!(is_transient_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_transient_status(StatusCode::BAD_GATEWAY));
        assert!(!is_transient_status(StatusCode::BAD_REQUEST));
        assert!(!is_transient_status(StatusCode::UNAUTHORIZED));
        assert!(!is_transient_status(StatusCode::TEMPORARY_REDIRECT));
    }

    #[test]
    fn test_payload_is_a_valid_pcm_wav() {
        let bytes = silent_wav();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            48_000
        );
        assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 16);
        assert_eq!(bytes.len(), 9_644);
    }
}
