use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use waved_meeting::upload::{UploadError, WebhookClient};

async fn receiver(status: &str) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let status = status.to_string();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let header_end = loop {
            let mut buffer = [0_u8; 8 * 1024];
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before HTTP headers");
            request.extend_from_slice(&buffer[..read]);
            if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(str::trim)
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .expect("multipart request should have a known content length");
        while request.len() < header_end + content_length {
            let mut buffer = [0_u8; 8 * 1024];
            let read = socket.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        let response =
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        socket.write_all(response.as_bytes()).await.unwrap();
        request
    });
    (format!("http://{address}/webhook"), task)
}

async fn artifact(dir: &Path) -> std::path::PathBuf {
    artifact_named(dir, "audio.wav", b"RIFF----WAVEfixture").await
}

async fn artifact_named(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let audio = dir.join(name);
    tokio::fs::write(&audio, bytes).await.unwrap();
    tokio::fs::write(
        dir.join("metadata.json"),
        br#"{"schema_version":1,"event":"meeting.recording.completed"}"#,
    )
    .await
    .unwrap();
    audio
}

#[tokio::test]
async fn multipart_contract_streams_named_audio_metadata_and_headers() {
    let (endpoint, request_task) = receiver("204 No Content").await;
    let temp = tempfile::tempdir().unwrap();
    let audio = artifact(temp.path()).await;

    WebhookClient::new()
        .unwrap()
        .upload_recording(
            &endpoint,
            "test-token",
            "wave-device-1",
            "21b17c4c-7390-4d52-8cbf-477fab102bc0",
            &audio,
        )
        .await
        .unwrap();

    let request = request_task.await.unwrap();
    let text = String::from_utf8_lossy(&request);
    let lowercase = text.to_ascii_lowercase();
    assert!(lowercase.contains("authorization: bearer test-token"));
    assert!(lowercase.contains("idempotency-key: 21b17c4c-7390-4d52-8cbf-477fab102bc0"));
    assert!(lowercase.contains("x-wave-device-id: wave-device-1"));
    assert!(lowercase.contains("user-agent: waved/"));
    assert!(text.contains("name=\"meeting_id\""));
    assert!(text.contains("name=\"metadata\"; filename=\"metadata.json\""));
    assert!(text.contains("Content-Type: application/json"));
    assert!(text.contains("name=\"audio\"; filename=\"21b17c4c-7390-4d52-8cbf-477fab102bc0.wav\""));
    assert!(text.contains("Content-Type: audio/wav"));
    assert!(text.contains("RIFF----WAVEfixture"));
}

#[tokio::test]
async fn opus_and_flac_parts_carry_their_extension_and_content_type() {
    for (name, extension, content_type) in [
        ("audio.opus", "opus", "audio/ogg; codecs=opus"),
        ("audio.flac", "flac", "audio/flac"),
    ] {
        let (endpoint, request_task) = receiver("204 No Content").await;
        let temp = tempfile::tempdir().unwrap();
        let audio = artifact_named(temp.path(), name, b"fixture-audio").await;
        WebhookClient::new()
            .unwrap()
            .upload_recording(&endpoint, "", "wave-device-1", "meeting-1", &audio)
            .await
            .unwrap();
        let request = request_task.await.unwrap();
        let text = String::from_utf8_lossy(&request);
        assert!(
            text.contains(&format!("name=\"audio\"; filename=\"meeting-1.{extension}\"")),
            "{name}"
        );
        assert!(text.contains(&format!("Content-Type: {content_type}")), "{name}");
        assert!(text.contains("fixture-audio"));
    }
}

#[tokio::test]
async fn empty_token_omits_authorization_and_redirect_is_not_followed() {
    let (endpoint, request_task) = receiver("302 Found").await;
    let temp = tempfile::tempdir().unwrap();
    let audio = artifact(temp.path()).await;

    let error = WebhookClient::new()
        .unwrap()
        .upload_recording(&endpoint, "", "wave-device-1", "test-id", &audio)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        UploadError::ConfigurationResponse { status: 302, .. }
    ));
    let request = request_task.await.unwrap();
    assert!(!String::from_utf8_lossy(&request)
        .to_ascii_lowercase()
        .contains("authorization:"));
}
