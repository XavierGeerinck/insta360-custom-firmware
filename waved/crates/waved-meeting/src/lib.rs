pub mod db;
pub mod recorder;
pub mod upload;

#[cfg(feature = "simulate")]
pub mod simulate;

use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{error, info, warn};
use uuid::Uuid;
use waved_audio::processed_frame::ProcessedAudioFrame;
use waved_config::WavedConfig;
use waved_state::{Command, DeviceEvent, MeetingState, PendingDecision, WifiState};

pub struct MeetingService {
    config: WavedConfig,
    pool: SqlitePool,
    recordings_dir: PathBuf,
    meeting_tx: Arc<watch::Sender<MeetingState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
    /// Post-AEC, post-NS mono frames for recording. Raw 8-ch mic is carried
    /// as a sidecar on each `ProcessedAudioFrame` for diagnostic replay.
    audio_rx: mpsc::Receiver<ProcessedAudioFrame>,
}

impl MeetingService {
    pub async fn new(
        config: WavedConfig,
        db_path: &str,
        recordings_dir: PathBuf,
        meeting_tx: Arc<watch::Sender<MeetingState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
        audio_rx: mpsc::Receiver<ProcessedAudioFrame>,
    ) -> Result<Self, sqlx::Error> {
        let pool = db::init_pool(db_path).await?;
        Ok(Self {
            config,
            pool,
            recordings_dir,
            meeting_tx,
            events_tx,
            audio_rx,
        })
    }

    pub async fn run(
        mut self,
        mut cmd_rx: broadcast::Receiver<Command>,
        mut wifi_rx: watch::Receiver<WifiState>,
    ) {
        info!("meeting_srv started");
        let mut active: Option<recorder::ActiveRecording> = None;

        // 1 Hz tick to publish elapsed-seconds onto MeetingState while
        // recording. The watch broadcasts to anyone holding a Receiver
        // (e.g. the UI thread); subscribers see the new value next time
        // they `borrow()`. Tick fires every second regardless of recording
        // state — branch is gated by `active.is_some()`.
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                // Drain post-AEC mono frames into the active recording's WAV
                // writer. Append is synchronous (BufWriter under the hood),
                // a few hundred microseconds per ~21 ms ALSA period.
                Some(frame) = self.audio_rx.recv(), if active.is_some() => {
                    if let Some(rec) = active.as_mut() {
                        rec.append_samples(&frame.mic_mono_f32);
                    }
                }
                _ = tick.tick(), if active.is_some() => {
                    if let Some(rec) = active.as_ref() {
                        let secs = rec.duration_secs();
                        self.meeting_tx.send_modify(|s| {
                            s.duration_secs = secs;
                        });
                    }
                }
                cmd_result = cmd_rx.recv() => {
                    match cmd_result {
                        Ok(cmd) => {
                            match cmd {
                                Command::StartRecording => {
                                    if active.is_some() {
                                        warn!("already recording, ignoring start");
                                        continue;
                                    }
                                    match recorder::ActiveRecording::start(
                                        &self.recordings_dir,
                                        &format!("{:?}", self.config.audio.mode),
                                    ).await {
                                        Ok(rec) => {
                                            let id = rec.id;
                                            let _ = db::insert_meeting(
                                                &self.pool, &id,
                                                &format!("{:?}", self.config.audio.mode),
                                                &rec.dir.to_string_lossy(),
                                            ).await;
                                            let _ = self.events_tx.send(DeviceEvent::RecordingStarted(id));
                                            self.meeting_tx.send_modify(|s| {
                                                s.recording = true;
                                                s.meeting_id = Some(id);
                                                s.duration_secs = 0;
                                            });
                                            active = Some(rec);
                                        }
                                        Err(e) => error!("failed to start recording: {e}"),
                                    }
                                }
                                Command::StopRecording => {
                                    if let Some(rec) = active.take() {
                                        self.stop_recording(rec).await;
                                    }
                                }
                                Command::AddBookmark => {
                                    if let Some(ref rec) = active {
                                        let ts = rec.duration_secs();
                                        let _ = db::add_bookmark(&self.pool, &rec.id, ts).await;
                                        info!("bookmark at {}s", ts);
                                    }
                                }
                                Command::SaveRecording(id) => {
                                    self.handle_save_recording(id).await;
                                }
                                Command::SendRecording(id) => {
                                    self.handle_send_recording(id, &wifi_rx).await;
                                }
                                Command::DiscardRecording(id) => {
                                    self.handle_discard_recording(id).await;
                                }
                                // Errata #8: handle Shutdown
                                Command::Shutdown => {
                                    if let Some(rec) = active.take() {
                                        info!("shutdown: stopping active recording first");
                                        self.stop_recording(rec).await;
                                    }
                                    // Pending-decision handling on shutdown is
                                    // added in a later task. For now, rely on
                                    // the post-stop pending_decision being a
                                    // no-op shutdown-side.
                                    return;
                                }
                                _ => {}
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("meeting_srv lagged by {n} commands");
                        }
                    }
                }
                Ok(()) = wifi_rx.changed() => {
                    if wifi_rx.borrow().connected {
                        self.flush_pending_uploads().await;
                    }
                }
            }
        }
    }

    /// Finalise the WAV, persist end_time/duration to the db, and publish
    /// a `pending_decision` for the confirmation screen. Does NOT upload —
    /// that's the SendRecording handler's job.
    async fn stop_recording(
        &self,
        mut rec: recorder::ActiveRecording,
    ) {
        let duration = rec.duration_secs();
        let id = rec.id;
        let audio_path = rec.audio_path();
        rec.finalize();
        let _ = db::finish_meeting(&self.pool, &id, duration).await;
        let _ = self.events_tx.send(DeviceEvent::RecordingStopped(id));

        let size_bytes = tokio::fs::metadata(&audio_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);

        let decision = PendingDecision {
            id,
            audio_path: audio_path.clone(),
            duration_secs: duration,
            size_bytes,
            endpoint: self.config.api.endpoint.clone(),
        };

        self.meeting_tx.send_modify(|s| {
            s.recording = false;
            s.paused = false;
            s.meeting_id = None;
            s.duration_secs = duration;
            s.pending_decision = Some(decision);
        });
        info!(
            "recording stopped: {} ({}s, {} bytes) — awaiting decision",
            id, duration, size_bytes
        );
    }

    async fn flush_pending_uploads(&self) {
        match db::get_pending_uploads(&self.pool).await {
            Ok(pending) => {
                for (id_str, path) in pending {
                    let Ok(id) = id_str.parse::<Uuid>() else {
                        warn!("flush: skipping unparseable id {id_str}");
                        continue;
                    };
                    let audio_path = std::path::PathBuf::from(&path).join("audio.wav");
                    upload_with_retry_standalone(
                        &self.pool,
                        &self.config.api.endpoint,
                        &self.config.api.auth_token,
                        self.config.api.retry_max,
                        self.config.api.retry_backoff_ms,
                        &id,
                        &audio_path,
                    )
                    .await;
                }
            }
            Err(e) => error!("failed to get pending uploads: {e}"),
        }
    }

    /// Returns the recording dir for an id under self.recordings_dir.
    fn dir_for(&self, id: &Uuid) -> PathBuf {
        self.recordings_dir.join(id.to_string())
    }

    /// Verify the carried id matches the active pending_decision and
    /// return the decision. None if mismatch / absent — caller should
    /// warn and ignore (stale tap).
    fn validate_pending(&self, id: &Uuid) -> Option<PendingDecision> {
        let s = self.meeting_tx.borrow().clone();
        match s.pending_decision {
            Some(d) if &d.id == id => Some(d),
            Some(d) => {
                warn!(
                    "ignoring decision for {id}: pending_decision is for {}",
                    d.id
                );
                None
            }
            None => {
                warn!("ignoring decision for {id}: no pending_decision");
                None
            }
        }
    }

    fn clear_pending(&self) {
        self.meeting_tx
            .send_modify(|s| s.pending_decision = None);
    }

    async fn handle_save_recording(&self, id: Uuid) {
        if self.validate_pending(&id).is_none() { return; }
        if let Err(e) = db::set_upload_status(&self.pool, &id, "local_only").await {
            warn!("save: db update failed for {id}: {e}");
        }
        self.clear_pending();
        info!("save: {id} kept locally");
    }

    async fn handle_send_recording(
        &self,
        id: Uuid,
        wifi_rx: &watch::Receiver<WifiState>,
    ) {
        let Some(decision) = self.validate_pending(&id) else { return; };
        if let Err(e) = db::set_upload_status(&self.pool, &id, "pending").await {
            warn!("send: db update failed for {id}: {e}");
        }
        if wifi_rx.borrow().connected {
            // Spawn so the command-loop iteration completes immediately
            // (the modal-clearing send_modify below must run before the
            // upload's first await yields).
            let pool = self.pool.clone();
            let endpoint = self.config.api.endpoint.clone();
            let token = self.config.api.auth_token.clone();
            let max = self.config.api.retry_max;
            let backoff = self.config.api.retry_backoff_ms;
            let audio_path = decision.audio_path.clone();
            tokio::spawn(async move {
                upload_with_retry_standalone(
                    &pool, &endpoint, &token, max, backoff, &id, &audio_path,
                )
                .await;
            });
        } else {
            info!("send: {id} queued (offline) — wifi-flush will retry");
        }
        self.clear_pending();
    }

    async fn handle_discard_recording(&self, id: Uuid) {
        if self.validate_pending(&id).is_none() { return; }
        if let Err(e) = db::delete_meeting_cascade(&self.pool, &id).await {
            warn!("discard: db delete failed for {id}: {e}");
        }
        let dir = self.dir_for(&id);
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => info!("discard: {id} dir removed ({})", dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("discard: rm -rf {} failed: {e}", dir.display()),
        }
        self.clear_pending();
    }
}

async fn upload_with_retry_standalone(
    pool: &SqlitePool,
    endpoint: &str,
    auth_token: &str,
    max_retries: u32,
    backoff_ms: u64,
    meeting_id: &Uuid,
    audio_path: &Path,
) {
    let id_str = meeting_id.to_string();
    for attempt in 1..=max_retries {
        #[cfg(not(feature = "simulate"))]
        let upload_result = upload::upload_recording(
            endpoint, auth_token, &id_str, audio_path,
        ).await;
        #[cfg(feature = "simulate")]
        let upload_result = simulate::upload_recording(
            endpoint, auth_token, &id_str, audio_path,
        ).await;

        match upload_result {
            Ok(()) => {
                let _ = db::set_upload_status(pool, meeting_id, "uploaded").await;
                return;
            }
            Err(e) => {
                warn!("upload attempt {attempt}/{max_retries} failed for {id_str}: {e}");
                if attempt < max_retries {
                    tokio::time::sleep(Duration::from_millis(backoff_ms * attempt as u64)).await;
                }
            }
        }
    }
    let _ = db::set_upload_status(pool, meeting_id, "upload_failed").await;
}
