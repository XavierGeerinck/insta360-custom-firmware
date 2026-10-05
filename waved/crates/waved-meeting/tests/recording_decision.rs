//! Integration tests for the Save / Send / Discard recording flow.
//!
//! Uploads go to a local webhook receiver on 127.0.0.1, so these tests need
//! neither the network nor the `simulate` feature. With `simulate` on, the
//! worker copies files to ./test_uploads instead of using HTTP and never
//! reaches the receiver, so this file only builds without it.
#![cfg(not(feature = "simulate"))]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, watch};
use waved_audio::callbacks::AudioFrame;
use waved_audio::processed_frame::ProcessedAudioFrame;
use waved_config::{UploadMode, WavedConfig};
use waved_meeting::MeetingService;
use waved_state::{Command, DeviceEvent, MeetingState, WifiState};

fn test_config(endpoint: &str, upload_mode: UploadMode) -> WavedConfig {
    let mut config = WavedConfig::default();
    config.device.name = "test".into();
    config.api.endpoint = endpoint.into();
    config.api.allow_insecure_http = true;
    config.api.upload_mode = upload_mode;
    config.api.retry_max = 1;
    config.api.retry_backoff_ms = 10;
    config.storage.prune_uploaded_at_percent = 100;
    config.storage.minimum_free_mb = 0;
    config
}

/// A webhook receiver that answers every request with 204 and reports each
/// request's Idempotency-Key, which is the meeting id.
async fn webhook_receiver() -> (String, mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (keys_tx, keys_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let keys_tx = keys_tx.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0_u8; 16 * 1024];
                let header_end = loop {
                    let read = match socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => read,
                    };
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while request.len() < header_end + content_length {
                    match socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&buffer[..read]),
                    }
                }
                let key = headers.lines().find_map(|line| {
                    line.strip_prefix("idempotency-key:")
                        .map(|value| value.trim().to_string())
                });
                let _ = socket
                    .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                if let Some(key) = key {
                    let _ = keys_tx.send(key);
                }
            });
        }
    });
    (format!("http://{address}/webhook"), keys_rx)
}

fn fake_frame() -> ProcessedAudioFrame {
    let frame = AudioFrame::mic_only(vec![0i16; 480], 1, 48000, 0);
    let mic_mono = vec![0i16; 480];
    let ref_mono = vec![0i16; 480];
    ProcessedAudioFrame::from_raw(&frame, &mic_mono, &ref_mono)
}

struct Harness {
    cmd_tx: broadcast::Sender<Command>,
    meeting_rx: watch::Receiver<MeetingState>,
    wifi_tx: watch::Sender<WifiState>,
    audio_tx: mpsc::Sender<ProcessedAudioFrame>,
    recordings_dir: PathBuf,
    events_rx: broadcast::Receiver<DeviceEvent>,
    /// Meeting ids the receiver has accepted, in arrival order.
    uploads: mpsc::UnboundedReceiver<String>,
}

async fn boot(connected: bool) -> Harness {
    boot_with_mode(connected, UploadMode::Ask).await
}

async fn boot_with_mode(connected: bool, upload_mode: UploadMode) -> Harness {
    let (endpoint, uploads) = webhook_receiver().await;
    let dir = tempfile::tempdir().unwrap();
    let recordings_dir = dir.path().join("recordings");
    let db_path = dir.path().join("t.db").to_string_lossy().into_owned();
    Box::leak(Box::new(dir)); // outlive the test

    let (cmd_tx, _) = broadcast::channel::<Command>(16);
    let (meeting_tx, meeting_rx) = watch::channel(MeetingState::default());
    let (events_tx, events_rx) = broadcast::channel::<DeviceEvent>(16);
    let (wifi_tx, wifi_rx) = watch::channel(WifiState {
        connected,
        ..Default::default()
    });
    let (audio_tx, audio_rx) = mpsc::channel::<ProcessedAudioFrame>(16);
    let (doa_tx, _) = broadcast::channel(16);

    let svc = MeetingService::new(
        test_config(&endpoint, upload_mode),
        &db_path,
        recordings_dir.clone(),
        Arc::new(meeting_tx),
        events_tx,
        audio_rx,
        doa_tx.subscribe(),
    )
    .await
    .unwrap();

    let cmd_rx = cmd_tx.subscribe();
    tokio::spawn(async move { svc.run(cmd_rx, wifi_rx).await });

    Harness {
        cmd_tx,
        meeting_rx,
        wifi_tx,
        audio_tx,
        recordings_dir,
        events_rx,
        uploads,
    }
}

/// Wait until `predicate(state)` returns true or the timeout elapses.
async fn wait_for<F: Fn(&MeetingState) -> bool>(
    rx: &mut watch::Receiver<MeetingState>,
    predicate: F,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if predicate(&rx.borrow()) {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "timed out waiting for predicate; state = {:?}",
                *rx.borrow()
            );
        }
        let _ = tokio::time::timeout(Duration::from_millis(50), rx.changed()).await;
    }
}

async fn record_and_stop(h: &mut Harness) -> uuid::Uuid {
    h.cmd_tx.send(Command::StartRecording).unwrap();
    wait_for(&mut h.meeting_rx, |s| s.recording).await;
    let id = h.meeting_rx.borrow().meeting_id.unwrap();
    let _ = h.audio_tx.send(fake_frame()).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    h.cmd_tx.send(Command::StopRecording).unwrap();
    wait_for(&mut h.meeting_rx, |s| {
        s.pending_decision.is_some() && !s.recording
    })
    .await;
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_recording_populates_pending_decision() {
    let mut h = boot(false).await;
    let id = record_and_stop(&mut h).await;
    let s = h.meeting_rx.borrow().clone();
    let d = s.pending_decision.expect("pending_decision must be set");
    assert_eq!(d.id, id);
    assert!(d.audio_path.exists(), "audio.wav must exist");
    assert!(
        d.endpoint.starts_with("http://127.0.0.1:"),
        "endpoint snapshot: {}",
        d.endpoint
    );
    assert!(d.size_bytes > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn save_clears_pending_and_marks_local_only() {
    let mut h = boot(false).await;
    let id = record_and_stop(&mut h).await;
    h.cmd_tx.send(Command::SaveRecording(id)).unwrap();
    wait_for(&mut h.meeting_rx, |s| s.pending_decision.is_none()).await;
    // File still exists.
    let dir = h.recordings_dir.join(id.to_string());
    assert!(dir.join("audio.wav").exists());
    // Even if Wi-Fi comes up, nothing is sent: the status is local_only.
    let _ = h.wifi_tx.send(WifiState {
        connected: true,
        ..Default::default()
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        h.uploads.try_recv().is_err(),
        "save must not trigger an upload"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_queues_upload_when_online() {
    let mut h = boot(true).await;
    let id = record_and_stop(&mut h).await;
    h.cmd_tx.send(Command::SendRecording(id)).unwrap();
    wait_for(&mut h.meeting_rx, |s| s.pending_decision.is_none()).await;
    let delivered = tokio::time::timeout(Duration::from_secs(5), h.uploads.recv())
        .await
        .expect("the receiver got no upload within 5 s")
        .expect("receiver stopped");
    assert_eq!(delivered, id.to_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discard_removes_file_and_db_row() {
    let mut h = boot(false).await;
    let id = record_and_stop(&mut h).await;
    h.cmd_tx.send(Command::DiscardRecording(id)).unwrap();
    wait_for(&mut h.meeting_rx, |s| s.pending_decision.is_none()).await;
    // Discard answers at once; the files go in the background.
    let dir = h.recordings_dir.join(id.to_string());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while dir.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "discard must rm -rf the recording dir"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_during_pending_decision_implicit_saves() {
    let mut h = boot(false).await;
    let id = record_and_stop(&mut h).await;
    h.cmd_tx.send(Command::Shutdown).unwrap();
    // The service saves implicitly and clears the decision before it exits.
    // Wait for that instead of sleeping a fixed time: the DB write can take
    // longer than 100 ms in the container.
    wait_for(&mut h.meeting_rx, |s| s.pending_decision.is_none()).await;
    let dir = h.recordings_dir.join(id.to_string());
    assert!(
        dir.join("audio.wav").exists(),
        "implicit save must preserve the audio file"
    );
    // pending_decision should be cleared in state too (last seen value).
    assert!(h.meeting_rx.borrow().pending_decision.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_during_pending_decision_is_ignored() {
    let mut h = boot(false).await;
    let id = record_and_stop(&mut h).await;
    let pending_before = h.meeting_rx.borrow().pending_decision.clone();
    h.cmd_tx.send(Command::StartRecording).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let s = h.meeting_rx.borrow().clone();
    assert!(!s.recording, "StartRecording must be ignored while pending");
    assert_eq!(s.pending_decision, pending_before);
    let _ = id;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_mode_skips_confirmation_and_queues_offline() {
    let mut h = boot_with_mode(false, UploadMode::Automatic).await;
    h.cmd_tx.send(Command::StartRecording).unwrap();
    wait_for(&mut h.meeting_rx, |state| state.recording).await;
    let _ = h.audio_tx.send(fake_frame()).await;
    h.cmd_tx.send(Command::StopRecording).unwrap();
    wait_for(&mut h.meeting_rx, |state| {
        !state.recording && state.pending_uploads == 1
    })
    .await;
    assert!(h.meeting_rx.borrow().pending_decision.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_upload_emits_success_for_the_recording_id() {
    let mut h = boot_with_mode(true, UploadMode::Automatic).await;
    h.cmd_tx.send(Command::StartRecording).unwrap();
    wait_for(&mut h.meeting_rx, |state| state.recording).await;
    let id = h.meeting_rx.borrow().meeting_id.unwrap();
    let _ = h.audio_tx.send(fake_frame()).await;
    h.cmd_tx.send(Command::StopRecording).unwrap();
    wait_for(&mut h.meeting_rx, |state| !state.recording).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let event = tokio::time::timeout(remaining, h.events_rx.recv())
            .await
            .expect("timed out waiting for upload event")
            .expect("event channel closed");
        if matches!(
            event,
            DeviceEvent::RecordingUploadSucceeded(received) if received == id
        ) {
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_only_mode_skips_confirmation_without_queueing() {
    let mut h = boot_with_mode(false, UploadMode::LocalOnly).await;
    h.cmd_tx.send(Command::StartRecording).unwrap();
    wait_for(&mut h.meeting_rx, |state| state.recording).await;
    let id = h.meeting_rx.borrow().meeting_id.unwrap();
    let _ = h.audio_tx.send(fake_frame()).await;
    h.cmd_tx.send(Command::StopRecording).unwrap();
    wait_for(&mut h.meeting_rx, |state| !state.recording).await;
    assert!(h.meeting_rx.borrow().pending_decision.is_none());
    assert_eq!(h.meeting_rx.borrow().pending_uploads, 0);
    assert!(h
        .recordings_dir
        .join(id.to_string())
        .join("metadata.json")
        .exists());
}
