pub mod db;
pub mod diarization;
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
use waved_audio::doa::DoaFrame;
use waved_audio::processed_frame::ProcessedAudioFrame;
use waved_config::WavedConfig;
use waved_state::{Command, DeviceEvent, MeetingState, PendingDecision, WifiState};
use crate::recorder::{PreBuffer, PREBUFFER_SECONDS, RECORDING_SAMPLE_RATE};

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
        Ok(Self {
            config,
            pool,
            recordings_dir,
            meeting_tx,
            events_tx,
            audio_rx,
            doa_rx,
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
                                    if active.is_some() {
                                        warn!("already recording, ignoring start");
                                        continue;
                                    }
                                    if self.meeting_tx.borrow().pending_decision.is_some() {
                                        warn!("ignoring StartRecording: previous take awaits decision");
                                        continue;
                                    }
                                    match recorder::ActiveRecording::start_with_quality(
                                        &self.recordings_dir,
                                        &format!("{:?}", self.config.audio.mode),
                                        self.config.audio.recording_quality,
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
                                            active.as_mut().unwrap().append_samples(&prefix);
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
                                    self.handle_send_recording(id, &wifi_rx).await;
                                }
                                Command::DiscardRecording(id) => {
                                    self.handle_discard_recording(id).await;
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
                                        if let Err(e) = db::set_upload_status(
                                            &self.pool, &id, "local_only",
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
        let config = WavedConfig::default();
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
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT speaker_id FROM diarization_segments ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(rows.len() >= 2, "expected ≥ 2 segments, got {}", rows.len());
        assert_eq!(rows[0].0, 1, "first segment should be speaker_id=1");
        assert_eq!(rows[1].0, 2, "second segment should be speaker_id=2");
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
