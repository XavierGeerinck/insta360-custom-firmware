pub mod db;
pub mod diarization;
pub mod metadata;
pub mod model;
pub mod reconcile;
pub mod recorder;
pub mod retention;
pub mod upload;
pub mod worker;

#[cfg(feature = "simulate")]
pub mod simulate;

use crate::model::MeetingStatus;
use crate::recorder::{PreBuffer, PREBUFFER_SECONDS, RECORDING_SAMPLE_RATE};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch, Notify};
use tracing::{error, info, warn};
use uuid::Uuid;
use waved_audio::doa::DoaFrame;
use waved_audio::processed_frame::ProcessedAudioFrame;
use waved_config::{UploadMode, WavedConfig};
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
    /// Per-frame DoA stream from F2's AudioService::subscribe_doa(). Drained
    /// only when a meeting is active; otherwise frames are discarded.
    doa_rx: broadcast::Receiver<DoaFrame>,
    device_id: String,
    webhook_test_marker: PathBuf,
}

impl MeetingService {
    pub async fn new(
        config: WavedConfig,
        db_path: &str,
        recordings_dir: PathBuf,
        meeting_tx: Arc<watch::Sender<MeetingState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
        audio_rx: mpsc::Receiver<ProcessedAudioFrame>,
        doa_rx: broadcast::Receiver<DoaFrame>,
    ) -> Result<Self, sqlx::Error> {
        let pool = db::init_pool(db_path).await?;
        if let Err(error) = reconcile::run(&pool, &recordings_dir).await {
            warn!("recording reconciliation failed: {error}");
        }
        let webhook_test_marker = Path::new(db_path)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("webhook-test.pending");
        if webhook_test_marker.exists() {
            if let Err(error) = db::reset_configuration_failures(&pool).await {
                warn!("failed to requeue deliveries after configuration change: {error}");
            }
        }
        Ok(Self {
            config,
            pool,
            recordings_dir,
            meeting_tx,
            events_tx,
            audio_rx,
            doa_rx,
            device_id: metadata::stable_device_id(),
            webhook_test_marker,
        })
    }

    pub async fn run(
        mut self,
        mut cmd_rx: broadcast::Receiver<Command>,
        mut wifi_rx: watch::Receiver<WifiState>,
    ) {
        info!("meeting_srv started");
        let mut active: Option<recorder::ActiveRecording> = None;
        let mut diarizer: Option<crate::diarization::Diarizer> = None;
        let mut doa_closed = false;
        retention::enforce(
            &self.pool,
            &self.recordings_dir,
            &self.config.storage,
            &self.meeting_tx,
        )
        .await;
        let upload_wake = Arc::new(Notify::new());
        let upload_worker = worker::UploadWorker::new(
            self.pool.clone(),
            worker::WorkerSettings {
                api: self.config.api.clone(),
                storage: self.config.storage.clone(),
                recordings_dir: self.recordings_dir.clone(),
                webhook_test_marker: self.webhook_test_marker.clone(),
                device_id: self.device_id.clone(),
            },
            Arc::clone(&self.meeting_tx),
            Arc::clone(&upload_wake),
        );
        let _upload_task = tokio::spawn(upload_worker.run(wifi_rx.clone()));

        // Post-DSP mono samples kept around for the pre-record splice.
        // See docs/superpowers/specs/2026-05-22-prebuffer-design.md.
        let mut prebuffer = PreBuffer::new(PREBUFFER_SECONDS, RECORDING_SAMPLE_RATE, 1);

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
                Some(frame) = self.audio_rx.recv() => {
                    match active.as_mut() {
                        Some(rec) => rec.append_samples(&frame.mic_mono_f32),
                        None => prebuffer.push(&frame.mic_mono_f32),
                    }
                }
                // Drain DoA frames into the active diarizer. Frames arriving
                // outside a recording are silently dropped (diarizer is None).
                doa_recv = self.doa_rx.recv(), if !doa_closed => {
                    match doa_recv {
                        Ok(frame) => {
                            if let Some(diar) = diarizer.as_mut() {
                                if let Some(seg) = diar.on_doa_frame(&frame) {
                                    if let Err(e) = db::insert_diarization_segment(&self.pool, &seg).await {
                                        warn!("failed to insert diarization segment: {e}");
                                    }
                                }
                            }
                            // No active meeting → drop the frame silently.
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("diarization receiver lagged {n} frames");
                            // Synthesize a silence frame at advanced timestamp so any
                            // open segment can close cleanly. u32::MAX forces the
                            // saturating_sub in on_silent_frame to produce a huge gap.
                            if let Some(diar) = diarizer.as_mut() {
                                if let Some(seg) = diar.on_silent_frame_for_lag(u32::MAX) {
                                    let _ = db::insert_diarization_segment(&self.pool, &seg).await;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            // DoA stream gone — finalize any open segment, leave the
                            // diarizer state for the next StartRecording reset.
                            if let Some(diar) = diarizer.as_mut() {
                                if let Some(seg) = diar.finalize() {
                                    let _ = db::insert_diarization_segment(&self.pool, &seg).await;
                                }
                            }
                            warn!("DoA stream closed; diarization paused");
                            doa_closed = true;
                        }
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
                                    if self.meeting_tx.borrow().storage_blocked {
                                        warn!("recording start blocked by configured storage limits");
                                        continue;
                                    }
                                    if active.is_some() {
                                        warn!("already recording, ignoring start");
                                        continue;
                                    }
                                    if self.meeting_tx.borrow().pending_decision.is_some() {
                                        warn!("ignoring StartRecording: previous take awaits decision");
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
                                            diarizer = Some(crate::diarization::Diarizer::new(id));
                                            // Splice the pre-record prefix onto the front of the
                                            // new WAV before any live frames arrive.
                                            let prefix = prebuffer.drain_all();
                                            // active was just set above; the unwrap is safe by construction.
                                            active.as_mut().unwrap().append_pre_roll(&prefix);
                                            // Freeze the pre-roll for the duration of the take — the
                                            // recording's own writer owns the audio now. Resumed on stop.
                                            prebuffer.pause();
                                        }
                                        Err(e) => error!("failed to start recording: {e}"),
                                    }
                                }
                                Command::StopRecording => {
                                    if let Some(rec) = active.take() {
                                        if let Some(mut d) = diarizer.take() {
                                            if let Some(seg) = d.finalize() {
                                                if let Err(e) = db::insert_diarization_segment(&self.pool, &seg).await {
                                                    warn!("failed to insert final diarization segment: {e}");
                                                }
                                            }
                                        }
                                        self.stop_recording(rec).await;
                                        upload_wake.notify_one();
                                        // Re-arm the pre-roll now that the take is closed.
                                        prebuffer.resume();
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
                                    self.handle_send_recording(id).await;
                                    upload_wake.notify_one();
                                }
                                Command::DiscardRecording(id) => {
                                    self.handle_discard_recording(id).await;
                                }
                                Command::RetryUpload(id) => {
                                    if let Err(error) =
                                        db::reset_attempt_cycle(&self.pool, &id).await
                                    {
                                        warn!("retry: db update failed for {id}: {error}");
                                    } else {
                                        upload_wake.notify_one();
                                    }
                                }
                                Command::RetryFailedUploads => {
                                    match db::reset_retryable_failures(&self.pool).await {
                                        Ok(count) => {
                                            info!("queued {count} failed webhook deliveries");
                                            upload_wake.notify_one();
                                        }
                                        Err(error) => {
                                            warn!("failed to queue webhook retries: {error}");
                                        }
                                    }
                                }
                                Command::Shutdown => {
                                    if let Some(rec) = active.take() {
                                        info!("shutdown: stopping active recording first");
                                        if let Some(mut d) = diarizer.take() {
                                            if let Some(seg) = d.finalize() {
                                                if let Err(e) = db::insert_diarization_segment(&self.pool, &seg).await {
                                                    warn!("failed to insert final diarization segment on shutdown: {e}");
                                                }
                                            }
                                        }
                                        self.stop_recording(rec).await;
                                    }
                                    // If a decision is still pending at shutdown,
                                    // implicit-save: never auto-discard data the
                                    // user hasn't decided on.
                                    let pending_id = self.meeting_tx
                                        .borrow()
                                        .pending_decision
                                        .as_ref()
                                        .map(|d| d.id);
                                    if let Some(id) = pending_id {
                                        info!("shutdown: implicit save for pending {id}");
                                        if let Err(e) = db::set_status(
                                            &self.pool, &id, MeetingStatus::LocalOnly,
                                        ).await {
                                            warn!("shutdown: db update failed for {id}: {e}");
                                        }
                                        self.clear_pending();
                                    }
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
                        upload_wake.notify_one();
                    }
                }
            }
        }
    }

    /// Finalize both durable artifacts before making this take eligible for
    /// either a user decision or background delivery.
    async fn stop_recording(&self, rec: recorder::ActiveRecording) {
        let duration = rec.duration_secs();
        let id = rec.id;
        let finalized = match rec.finalize() {
            Ok(recording) => recording,
            Err(error) => {
                error!("failed to finalize recording {id}: {error}");
                let _ = db::set_status(&self.pool, &id, MeetingStatus::ArtifactError).await;
                self.meeting_tx.send_modify(|state| {
                    state.recording = false;
                    state.paused = false;
                    state.meeting_id = None;
                    state.pending_decision = None;
                });
                return;
            }
        };

        let bookmarks = db::get_bookmarks(&self.pool, &id)
            .await
            .unwrap_or_else(|error| {
                warn!("failed to load bookmarks for {id}: {error}");
                Vec::new()
            });
        let segments = db::get_speaker_segments(&self.pool, &id)
            .await
            .unwrap_or_else(|error| {
                warn!("failed to load speaker segments for {id}: {error}");
                Vec::new()
            });
        if let Err(error) = metadata::build_and_write(
            &finalized,
            &self.config,
            &self.device_id,
            bookmarks,
            segments,
        )
        .await
        {
            error!("failed to persist metadata for {id}: {error}");
            let _ = db::set_status(&self.pool, &id, MeetingStatus::ArtifactError).await;
            self.meeting_tx.send_modify(|state| {
                state.recording = false;
                state.paused = false;
                state.meeting_id = None;
                state.pending_decision = None;
            });
            return;
        }

        let status = match self.config.api.upload_mode {
            UploadMode::Automatic => MeetingStatus::Queued,
            UploadMode::Ask => MeetingStatus::AwaitingDecision,
            UploadMode::LocalOnly => MeetingStatus::LocalOnly,
        };
        if let Err(error) = db::finish_meeting(&self.pool, &id, duration, status).await {
            error!("failed to persist final state for {id}: {error}");
            self.meeting_tx.send_modify(|state| {
                state.recording = false;
                state.paused = false;
                state.meeting_id = None;
                state.duration_secs = duration;
                state.pending_decision = None;
            });
            let _ = self.events_tx.send(DeviceEvent::RecordingStopped(id));
            return;
        }
        let _ = self.events_tx.send(DeviceEvent::RecordingStopped(id));
        let decision = (self.config.api.upload_mode == UploadMode::Ask).then(|| PendingDecision {
            id,
            audio_path: finalized.audio_path.clone(),
            duration_secs: duration,
            size_bytes: finalized.size_bytes,
            endpoint: self.config.api.endpoint.clone(),
        });

        self.meeting_tx.send_modify(|s| {
            s.recording = false;
            s.paused = false;
            s.meeting_id = None;
            s.duration_secs = duration;
            s.pending_decision = decision;
        });
        info!(
            "recording stopped: {} ({}s, {} bytes) — {}",
            id,
            duration,
            finalized.size_bytes,
            status.as_str()
        );
        retention::enforce(
            &self.pool,
            &self.recordings_dir,
            &self.config.storage,
            &self.meeting_tx,
        )
        .await;
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
        self.meeting_tx.send_modify(|s| s.pending_decision = None);
    }

    async fn handle_save_recording(&self, id: Uuid) {
        if self.validate_pending(&id).is_none() {
            return;
        }
        if let Err(e) = db::set_status(&self.pool, &id, MeetingStatus::LocalOnly).await {
            warn!("save: db update failed for {id}: {e}");
        }
        self.clear_pending();
        info!("save: {id} kept locally");
    }

    async fn handle_send_recording(&self, id: Uuid) {
        if self.validate_pending(&id).is_none() {
            return;
        }
        if let Err(e) = db::set_status(&self.pool, &id, MeetingStatus::Queued).await {
            warn!("send: db update failed for {id}: {e}");
        }
        info!("send: {id} queued for the background webhook worker");
        self.clear_pending();
    }

    async fn handle_discard_recording(&self, id: Uuid) {
        if self.validate_pending(&id).is_none() {
            return;
        }
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

#[cfg(test)]
mod wiring_tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::{broadcast, mpsc, watch};
    use waved_audio::doa::{DoaBackend, DoaFrame};
    use waved_config::WavedConfig;
    use waved_state::{Command, DeviceEvent, MeetingState};

    fn high(ms: u64, az: f32) -> DoaFrame {
        DoaFrame {
            frame_idx: 0,
            timestamp_ms: ms,
            azimuth_deg: az,
            elevation_deg: 0.0,
            confidence: 0.8,
            vad: true,
            backend: DoaBackend::GccPhat,
        }
    }

    #[tokio::test]
    async fn meeting_service_writes_diarization_segments_end_to_end() {
        // Per-test SQLite tempdir.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("t.db");
        let recordings_dir = tmp.path().join("rec");
        std::fs::create_dir_all(&recordings_dir).unwrap();

        // Channels.
        let (meeting_tx_inner, _meeting_rx) = watch::channel(MeetingState::default());
        let meeting_tx = Arc::new(meeting_tx_inner);
        let (events_tx, _events_rx) = broadcast::channel::<DeviceEvent>(16);
        let (audio_processed_tx, audio_processed_rx) = mpsc::channel(64);
        let (doa_tx_handle, _) = broadcast::channel::<DoaFrame>(64);

        // Construct service.
        let mut config = WavedConfig::default();
        config.storage.prune_uploaded_at_percent = 100;
        config.storage.minimum_free_mb = 0;
        let svc = MeetingService::new(
            config,
            db_path.to_str().unwrap(),
            recordings_dir,
            Arc::clone(&meeting_tx),
            events_tx.clone(),
            audio_processed_rx,
            doa_tx_handle.subscribe(),
        )
        .await
        .unwrap();

        // Run the service in the background.
        let (cmd_tx, cmd_rx) = broadcast::channel::<Command>(16);
        let (wifi_tx, wifi_rx) = watch::channel(waved_state::WifiState::default());
        let _ = (audio_processed_tx, wifi_tx); // keep alive for the run loop
        let _svc_handle = tokio::spawn(svc.run(cmd_rx, wifi_rx));

        // Start recording.
        cmd_tx.send(Command::StartRecording).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Push DoA frames: 500ms at az=45°, then 500ms at az=180°.
        for i in 0..24u64 {
            doa_tx_handle.send(high(i * 21, 45.0)).unwrap();
        }
        for i in 24..48u64 {
            doa_tx_handle.send(high(i * 21, 180.0)).unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Stop recording → finalize diarizer.
        cmd_tx.send(Command::StopRecording).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // Query the db directly.
        let pool = db::init_pool(db_path.to_str().unwrap()).await.unwrap();
        let rows: Vec<(i64,)> =
            sqlx::query_as("SELECT speaker_id FROM diarization_segments ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(rows.len() >= 2, "expected ≥ 2 segments, got {}", rows.len());
        assert_eq!(rows[0].0, 1, "first segment should be speaker_id=1");
        assert_eq!(rows[1].0, 2, "second segment should be speaker_id=2");
    }
}
