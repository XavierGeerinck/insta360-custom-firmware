pub mod db;
pub mod diarization;
mod files;
pub mod metadata;
pub mod model;
pub mod player;
pub mod reconcile;
pub mod recorder;
mod removal;
pub mod retention;
pub mod upload;
pub mod worker;

#[cfg(test)]
mod service_tests;

#[cfg(feature = "simulate")]
pub mod simulate;

use crate::model::MeetingStatus;
use crate::recorder::{PreBuffer, PREBUFFER_SECONDS, RECORDING_SAMPLE_RATE};
use sqlx::SqlitePool;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch, Notify};
use tracing::{error, info, warn};
use uuid::Uuid;
use waved_audio::doa::DoaFrame;
use waved_audio::integrity::{IntegritySignal, RecordingIntegrity};
use waved_audio::processed_frame::ProcessedAudioFrame;
use waved_config::{UploadMode, WavedConfig};
use waved_state::{
    AudioState, Command, DeviceEvent, FilesState, MeetingState, PendingDecision,
    RecordingIntegrityFault, SettingsState, WifiState,
};

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
    integrity: Arc<RecordingIntegrity>,
    integrity_rx: mpsc::UnboundedReceiver<IntegritySignal>,
    device_id: String,
    webhook_test_marker: PathBuf,
    /// Live user settings (upload mode, pre-recording, ask timeout). Seeded
    /// from the boot config; `with_live_inputs` swaps in the bus watch.
    settings_rx: watch::Receiver<SettingsState>,
    /// Live audio state: the mode recorded in metadata, plus mute and capture
    /// health for the pre-recording indicator.
    audio_state_rx: watch::Receiver<AudioState>,
    /// Keeps the boot-config fallback channels open until live inputs replace them.
    _fallback: Option<(watch::Sender<SettingsState>, watch::Sender<AudioState>)>,
    /// Where the Files list is published, when someone listens.
    files_tx: Option<Arc<watch::Sender<FilesState>>>,
    /// Asks the loop to republish the Files list: a removal finished off-loop.
    files_refresh: Arc<Notify>,
}

pub struct MeetingPipelineInputs {
    pub audio_rx: mpsc::Receiver<ProcessedAudioFrame>,
    pub doa_rx: broadcast::Receiver<DoaFrame>,
    pub integrity: Arc<RecordingIntegrity>,
    pub integrity_rx: mpsc::UnboundedReceiver<IntegritySignal>,
}

/// Bus watches that make recording policy follow the device settings live
/// (C11). Without them the service uses the boot-time config.
pub struct MeetingLiveInputs {
    pub settings: watch::Receiver<SettingsState>,
    pub audio: watch::Receiver<AudioState>,
}

/// What happens to a take at Stop, read from the live settings at that moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StopPolicy {
    upload_mode: UploadMode,
    /// Seconds before an unanswered "Send this recording?" keeps the take.
    ask_timeout_s: u32,
    /// A webhook endpoint is configured. Without one, nothing is queued.
    server_configured: bool,
}

/// The upload worker's rule: an empty endpoint means there is nowhere to send.
fn upload_server_configured(endpoint: &str) -> bool {
    !endpoint.trim().is_empty()
}

/// The pre-recording dot (D3, C12): the buffer is filling right now.
pub(crate) fn pre_recording_active(setting_on: bool, recording: bool, audio: &AudioState) -> bool {
    setting_on && !recording && !audio.mic_muted && audio.capture_healthy
}

/// Whole seconds left before `deadline`, rounded up, so the countdown shows
/// 60 for the whole first second.
pub(crate) fn seconds_left(deadline: tokio::time::Instant, now: tokio::time::Instant) -> u32 {
    let millis = deadline.saturating_duration_since(now).as_millis();
    millis.div_ceil(1_000).min(u128::from(u32::MAX)) as u32
}

/// A fault while paused only ends the take when the pipeline itself is gone:
/// frame drops and xruns lose nothing, since nothing is written while paused.
fn paused_take_tolerates(rec: &recorder::ActiveRecording, fault: RecordingIntegrityFault) -> bool {
    rec.is_paused() && !fault.requires_restart()
}

#[derive(Clone)]
struct RecordingFinalizer {
    config: WavedConfig,
    pool: SqlitePool,
    recordings_dir: PathBuf,
    meeting_tx: Arc<watch::Sender<MeetingState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
    integrity: Arc<RecordingIntegrity>,
    device_id: String,
}

impl From<&MeetingService> for RecordingFinalizer {
    fn from(service: &MeetingService) -> Self {
        Self {
            config: service.config.clone(),
            pool: service.pool.clone(),
            recordings_dir: service.recordings_dir.clone(),
            meeting_tx: Arc::clone(&service.meeting_tx),
            events_tx: service.events_tx.clone(),
            integrity: Arc::clone(&service.integrity),
            device_id: service.device_id.clone(),
        }
    }
}

impl RecordingFinalizer {
    /// Finalize both durable artifacts before making this take eligible for
    /// either a user decision or background delivery.
    async fn stop_recording(self, rec: recorder::ActiveRecording, policy: StopPolicy) {
        let duration = rec.duration_secs();
        let id = rec.id;
        let finalized = match tokio::task::spawn_blocking(move || rec.finalize()).await {
            Ok(Ok(recording)) => recording,
            Ok(Err(error)) => {
                error!("failed to finalize recording {id}: {error}");
                self.integrity
                    .record_fault(RecordingIntegrityFault::WavWriteFailed);
                self.mark_recording_fault(
                    id,
                    duration,
                    RecordingIntegrityFault::WavWriteFailed,
                )
                .await;
                return;
            }
            Err(error) => {
                error!("recording finalization task failed for {id}: {error}");
                self.integrity
                    .record_fault(RecordingIntegrityFault::WavWriteFailed);
                self.mark_recording_fault(
                    id,
                    duration,
                    RecordingIntegrityFault::WavWriteFailed,
                )
                .await;
                return;
            }
        };
        #[cfg(feature = "recording-integrity-test-hooks")]
        finalization_test_checkpoint(FinalizationCheckpoint::WavFinalized, id).await;

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
        let mark_count = bookmarks.len() as u32;
        let speaker_count = segments
            .iter()
            .map(|segment| segment.speaker_id)
            .collect::<std::collections::HashSet<_>>()
            .len()
            .min(usize::from(u8::MAX)) as u8;
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
                state.current_speaker = None;
                state.paused = false;
                state.meeting_id = None;
                state.pending_decision = None;
            });
            return;
        }
        #[cfg(feature = "recording-integrity-test-hooks")]
        finalization_test_checkpoint(FinalizationCheckpoint::MetadataFinalized, id).await;

        let status = match policy.upload_mode {
            UploadMode::Automatic if policy.server_configured => MeetingStatus::Queued,
            // Nowhere to send it: keep it on Wave rather than look queued.
            UploadMode::Automatic => MeetingStatus::LocalOnly,
            UploadMode::Ask => MeetingStatus::AwaitingDecision,
            UploadMode::LocalOnly => MeetingStatus::LocalOnly,
        };
        if let Err(error) = db::finish_meeting(&self.pool, &id, duration, status).await {
            error!("failed to persist final state for {id}: {error}");
            self.meeting_tx.send_modify(|state| {
                state.recording = false;
                state.current_speaker = None;
                state.paused = false;
                state.meeting_id = None;
                state.duration_secs = duration;
                state.pending_decision = None;
            });
            let _ = self.events_tx.send(DeviceEvent::RecordingStopped(id));
            return;
        }
        let _ = self.events_tx.send(DeviceEvent::RecordingStopped(id));
        let decision = (policy.upload_mode == UploadMode::Ask).then(|| PendingDecision {
            id,
            audio_path: finalized.audio_path.clone(),
            duration_secs: duration,
            size_bytes: finalized.size_bytes,
            endpoint: self.config.api.endpoint.clone(),
            speakers: speaker_count,
            marks: mark_count,
        });
        let ask_deadline_secs = decision.as_ref().map(|_| policy.ask_timeout_s);

        self.meeting_tx.send_modify(|state| {
            state.recording = false;
            state.current_speaker = None;
            state.paused = false;
            state.meeting_id = None;
            state.duration_secs = duration;
            state.pending_decision = decision;
            state.ask_deadline_secs = ask_deadline_secs;
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

    async fn mark_recording_fault(
        &self,
        id: Uuid,
        duration: u32,
        fault: RecordingIntegrityFault,
    ) {
        if let Err(error) = db::mark_artifact_error(&self.pool, &id, duration, fault).await {
            warn!("failed to persist recording integrity fault for {id}: {error}");
        }
        self.meeting_tx.send_modify(|state| {
            state.recording = false;
            state.current_speaker = None;
            state.paused = false;
            state.meeting_id = None;
            state.duration_secs = duration;
            state.pending_decision = None;
            state.last_recording_error = Some(fault);
            if fault.blocks_recording() {
                state.recording_blocked_reason = Some(fault);
            }
        });
        let _ = self
            .events_tx
            .send(DeviceEvent::RecordingIntegrityFailed {
                meeting_id: Some(id),
                fault,
            });
    }
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
        let (_integrity_tx, integrity_rx) = mpsc::unbounded_channel();
        Self::new_with_integrity(
            config,
            db_path,
            recordings_dir,
            meeting_tx,
            events_tx,
            MeetingPipelineInputs {
                audio_rx,
                doa_rx,
                integrity: Arc::new(RecordingIntegrity::default()),
                integrity_rx,
            },
        )
        .await
    }

    pub async fn new_with_integrity(
        config: WavedConfig,
        db_path: &str,
        recordings_dir: PathBuf,
        meeting_tx: Arc<watch::Sender<MeetingState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
        inputs: MeetingPipelineInputs,
    ) -> Result<Self, sqlx::Error> {
        let pool = db::init_pool(db_path).await?;
        let finished = removal::finish_interrupted(&pool, &recordings_dir).await;
        if finished > 0 {
            info!("finished {finished} interrupted delete(s)");
        }
        if let Err(error) = reconcile::run(&pool, &recordings_dir).await {
            warn!("recording reconciliation failed: {error}");
        }
        match db::keep_stale_decisions(&pool).await {
            Ok(0) => {}
            Ok(kept) => info!("kept {kept} recording(s) whose decision was interrupted by a restart"),
            Err(error) => warn!("failed to keep interrupted decisions: {error}"),
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
        let (settings_fallback, settings_rx) =
            watch::channel(waved_settings::settings_from_config(&config));
        let (audio_fallback, audio_state_rx) = watch::channel(AudioState {
            mode: config.audio.mode.clone(),
            ..AudioState::default()
        });
        Ok(Self {
            config,
            pool,
            recordings_dir,
            meeting_tx,
            events_tx,
            audio_rx: inputs.audio_rx,
            doa_rx: inputs.doa_rx,
            integrity: inputs.integrity,
            integrity_rx: inputs.integrity_rx,
            device_id: metadata::stable_device_id(),
            webhook_test_marker,
            settings_rx,
            audio_state_rx,
            _fallback: Some((settings_fallback, audio_fallback)),
            files_tx: None,
            files_refresh: Arc::new(Notify::new()),
        })
    }

    /// Follow the live settings and audio state instead of the boot config.
    pub fn with_live_inputs(mut self, inputs: MeetingLiveInputs) -> Self {
        self.settings_rx = inputs.settings;
        self.audio_state_rx = inputs.audio;
        self._fallback = None;
        self
    }

    /// Publish the Files list on this watch (refreshed every 5 s and after
    /// any command that changes a recording).
    pub fn with_files_publisher(mut self, files: Arc<watch::Sender<FilesState>>) -> Self {
        self.files_tx = Some(files);
        self
    }

    async fn publish_files(&self) {
        let Some(files_tx) = self.files_tx.as_ref() else {
            return;
        };
        let storage_blocked = self.meeting_tx.borrow().storage_blocked;
        match files::snapshot(
            &self.pool,
            &self.recordings_dir,
            &self.config.storage,
            self.config.api.retry_max,
            storage_blocked,
        )
        .await
        {
            Ok(state) => {
                files_tx.send_if_modified(|current| {
                    if *current == state {
                        return false;
                    }
                    *current = state;
                    true
                });
            }
            Err(error) => warn!("failed to publish the recordings list: {error}"),
        }
    }

    fn stop_policy(&self) -> StopPolicy {
        let settings = self.settings_rx.borrow();
        StopPolicy {
            upload_mode: settings.upload_mode,
            ask_timeout_s: settings.ask_timeout_s,
            server_configured: upload_server_configured(&self.config.api.endpoint),
        }
    }

    /// Start the auto-Keep countdown for a decision `stop_recording` just posted.
    fn arm_ask_deadline(&self) -> Option<tokio::time::Instant> {
        let state = self.meeting_tx.borrow();
        state.pending_decision.as_ref()?;
        let seconds = state.ask_deadline_secs?;
        Some(tokio::time::Instant::now() + Duration::from_secs(u64::from(seconds)))
    }

    /// Nobody answered "Send this recording?": keep it on Wave (D4).
    async fn keep_unanswered(&self) {
        let Some(id) = self.meeting_tx.borrow().pending_decision.as_ref().map(|d| d.id) else {
            return;
        };
        info!("no answer to \"Send this recording?\" for {id}; keeping it on Wave");
        self.handle_save_recording(id).await;
    }

    fn live_audio_mode(&self) -> String {
        format!("{:?}", self.audio_state_rx.borrow().mode)
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
            self.events_tx.clone(),
            Arc::clone(&upload_wake),
        );
        let _upload_task = tokio::spawn(upload_worker.run(wifi_rx.clone()));

        // Post-DSP mono samples kept around for the pre-record splice.
        // See docs/superpowers/specs/2026-05-22-prebuffer-design.md.
        let mut prebuffer = PreBuffer::new(PREBUFFER_SECONDS, RECORDING_SAMPLE_RATE, 1);
        self.sync_pre_recording(&mut prebuffer);
        let mut settings_rx = self.settings_rx.clone();
        let mut audio_state_rx = self.audio_state_rx.clone();
        self.publish_pre_recording(false);

        // 1 Hz tick to publish elapsed-seconds onto MeetingState while
        // recording, and the "Send this recording?" countdown while it runs.
        // The watch broadcasts to anyone holding a Receiver (e.g. the UI
        // thread); subscribers see the new value next time they `borrow()`.
        // Tick fires every second regardless — the branch is gated by an
        // active take or a running countdown.
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // When an unanswered "Send this recording?" is kept on Wave (D4).
        let mut ask_deadline: Option<tokio::time::Instant> = None;
        // The Files list: the first tick fires at once (the boot publish).
        let mut files_tick = tokio::time::interval(Duration::from_secs(5));
        files_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let files_refresh = Arc::clone(&self.files_refresh);

        loop {
            tokio::select! {
                // Drain post-AEC mono frames into the active recording's WAV
                // writer. Append is synchronous (BufWriter under the hood),
                // a few hundred microseconds per ~21 ms ALSA period.
                Some(frame) = self.audio_rx.recv() => {
                    if let Some(rec) = active.as_mut() {
                        if let Err(error) = rec.append_samples(&frame.mic_mono_f32) {
                            warn!("recording WAV write failed: {error}");
                            self.integrity
                                .record_fault(RecordingIntegrityFault::WavWriteFailed);
                            let rec = active.take().expect("active recording disappeared");
                            diarizer = None;
                            self.abort_recording(
                                rec,
                                RecordingIntegrityFault::WavWriteFailed,
                            )
                            .await;
                            prebuffer.clear();
                            prebuffer.resume();
                        }
                    } else {
                        self.sync_pre_recording(&mut prebuffer);
                        prebuffer.push(&frame.mic_mono_f32);
                    }
                }
                Some(signal) = self.integrity_rx.recv() => {
                    self.handle_integrity_signal(signal, &mut active, &mut diarizer, &mut prebuffer)
                        .await;
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
                            self.publish_speaker(diarizer.as_ref().and_then(|d| d.current_speaker()));
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
                            self.publish_speaker(diarizer.as_ref().and_then(|d| d.current_speaker()));
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            // DoA stream gone — finalize any open segment, leave the
                            // diarizer state for the next StartRecording reset.
                            if let Some(diar) = diarizer.as_mut() {
                                if let Some(seg) = diar.finalize() {
                                    let _ = db::insert_diarization_segment(&self.pool, &seg).await;
                                }
                            }
                            self.publish_speaker(diarizer.as_ref().and_then(|d| d.current_speaker()));
                            warn!("DoA stream closed; diarization paused");
                            doa_closed = true;
                        }
                    }
                }
                _ = tick.tick(), if active.is_some() || ask_deadline.is_some() => {
                    if let Some(rec) = active.as_ref() {
                        let secs = rec.duration_secs();
                        self.meeting_tx.send_modify(|s| {
                            s.duration_secs = secs;
                        });
                    }
                    if ask_deadline.is_some() && self.meeting_tx.borrow().pending_decision.is_none() {
                        // Answered (Save / Send / Discard / Delete) since the last tick.
                        ask_deadline = None;
                    }
                    if let Some(deadline) = ask_deadline {
                        match seconds_left(deadline, tokio::time::Instant::now()) {
                            0 => {
                                ask_deadline = None;
                                self.keep_unanswered().await;
                                self.publish_files().await;
                            }
                            left => {
                                self.meeting_tx.send_if_modified(|s| {
                                    if s.ask_deadline_secs == Some(left) {
                                        return false;
                                    }
                                    s.ask_deadline_secs = Some(left);
                                    true
                                });
                            }
                        }
                    }
                }
                _ = files_tick.tick(), if self.files_tx.is_some() => {
                    self.publish_files().await;
                }
                _ = files_refresh.notified(), if self.files_tx.is_some() => {
                    self.publish_files().await;
                }
                cmd_result = cmd_rx.recv() => {
                    match cmd_result {
                        Ok(cmd) => {
                            let refresh_files = changes_files(&cmd);
                            match cmd {
                                Command::StartRecording => {
                                    if let Some(fault) =
                                        self.meeting_tx.borrow().recording_blocked_reason
                                    {
                                        warn!("recording start blocked: {}", fault.diagnostic());
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
                                    retention::enforce(
                                        &self.pool,
                                        &self.recordings_dir,
                                        &self.config.storage,
                                        &self.meeting_tx,
                                    )
                                    .await;
                                    if self.meeting_tx.borrow().storage_blocked {
                                        warn!("recording start blocked by configured storage limits");
                                        continue;
                                    }
                                    let audio_mode = self.live_audio_mode();
                                    match recorder::ActiveRecording::start_with_integrity_epoch(
                                        &self.recordings_dir,
                                        &audio_mode,
                                        self.integrity.epoch(),
                                    )
                                    .await
                                    {
                                        Ok(rec) => {
                                            let id = rec.id;
                                            let _ = db::insert_meeting(
                                                &self.pool, &id,
                                                &audio_mode,
                                                &rec.dir.to_string_lossy(),
                                            ).await;
                                            let _ = self.events_tx.send(DeviceEvent::RecordingStarted(id));
                                            self.meeting_tx.send_modify(|s| {
                                                s.recording = true;
                                                s.paused = false;
                                                s.meeting_id = Some(id);
                                                s.duration_secs = 0;
                                                s.marks = 0;
                                                s.current_speaker = None;
                                            });
                                            active = Some(rec);
                                            diarizer = Some(crate::diarization::Diarizer::new(id));
                                            // Splice the pre-record prefix onto the front of the
                                            // new WAV before any live frames arrive.
                                            // The setting is read again here: `select!` is
                                            // unbiased, so a just-flipped "off" may not have
                                            // been handled yet, and its audio must not be used.
                                            self.sync_pre_recording(&mut prebuffer);
                                            let prefix = prebuffer.drain_all();
                                            // active was just set above; the unwrap is safe by construction.
                                            if let Err(error) =
                                                active.as_mut().unwrap().append_pre_roll(&prefix)
                                            {
                                                warn!("recording pre-roll write failed: {error}");
                                                self.integrity.record_fault(
                                                    RecordingIntegrityFault::WavWriteFailed,
                                                );
                                                let rec = active
                                                    .take()
                                                    .expect("active recording disappeared");
                                                diarizer = None;
                                                self.abort_recording(
                                                    rec,
                                                    RecordingIntegrityFault::WavWriteFailed,
                                                )
                                                .await;
                                                prebuffer.clear();
                                                prebuffer.resume();
                                            } else {
                                                // Freeze the pre-roll for the duration of the take — the
                                                // recording's own writer owns the audio now. Resumed on stop.
                                                prebuffer.pause();
                                                #[cfg(feature = "recording-integrity-test-hooks")]
                                                finalization_test_checkpoint(
                                                    FinalizationCheckpoint::ActivePartial,
                                                    id,
                                                )
                                                .await;
                                            }
                                        }
                                        Err(e) => error!("failed to start recording: {e}"),
                                    }
                                }
                                Command::StopRecording => {
                                    if let Some(mut rec) = active.take() {
                                        if let Some(mut d) = diarizer.take() {
                                            if let Some(seg) = d.finalize() {
                                                if let Err(e) = db::insert_diarization_segment(&self.pool, &seg).await {
                                                    warn!("failed to insert final diarization segment: {e}");
                                                }
                                            }
                                        }
                                        let (fault, acknowledgements) =
                                            self.drain_before_stop(&mut rec);
                                        // Audio received after the accepted Stop belongs to
                                        // the next take's pre-roll, including while this take
                                        // is being finalized and hashed.
                                        prebuffer.clear();
                                        prebuffer.resume();
                                        if let Some(fault) = fault {
                                            self.abort_recording(rec, fault).await;
                                        } else {
                                            let finalization = self.stop_recording(rec, self.stop_policy());
                                            self.await_finalization_with_drain(
                                                &mut prebuffer,
                                                &mut doa_closed,
                                                finalization,
                                            )
                                            .await;
                                            upload_wake.notify_one();
                                            ask_deadline = self.arm_ask_deadline();
                                            if ask_deadline.is_some() {
                                                // Step the countdown on whole seconds from now,
                                                // so it reaches zero at the deadline itself.
                                                tick.reset();
                                            }
                                        }
                                        for acknowledgement in acknowledgements {
                                            let _ = acknowledgement.send(());
                                        }
                                    }
                                }
                                Command::PauseRecording => {
                                    let Some(rec) = active.as_mut() else { continue };
                                    if !rec.is_paused() {
                                        // Frames still queued were captured before the
                                        // pause: write them, or the last words are lost.
                                        if let Some(fault) = self.append_queued_frames(rec) {
                                            let rec = active.take().expect("active recording disappeared");
                                            diarizer = None;
                                            self.abort_recording(rec, fault).await;
                                            prebuffer.clear();
                                            prebuffer.resume();
                                            continue;
                                        }
                                    }
                                    if self.integrity.epoch() != rec.integrity_epoch() {
                                        // A fault from while audio was being written is
                                        // still queued; pausing now would excuse it. Its
                                        // signal is certain to follow (see
                                        // `RecordingIntegrity::record_fault`) and ends the take.
                                        warn!("pause ignored: an integrity fault is ending the take");
                                        continue;
                                    }
                                    if rec.pause_at(std::time::Instant::now()) {
                                        if let Some(seg) = diarizer.as_mut().and_then(|d| d.pause()) {
                                            if let Err(e) = db::insert_diarization_segment(&self.pool, &seg).await {
                                                warn!("failed to insert diarization segment at pause: {e}");
                                            }
                                        }
                                        self.meeting_tx.send_modify(|s| {
                                            s.paused = true;
                                            s.current_speaker = None;
                                        });
                                        info!("recording paused at {}s", rec.duration_secs());
                                    }
                                }
                                Command::ResumeRecording => {
                                    if active.as_ref().is_some_and(|rec| rec.is_paused()) {
                                        // Still paused: frames queued now were captured
                                        // during the pause and never reach the file, and
                                        // faults from during the pause are judged as
                                        // pause faults before the take writes again.
                                        self.discard_queued_frames();
                                        while let Ok(signal) = self.integrity_rx.try_recv() {
                                            self.handle_integrity_signal(
                                                signal,
                                                &mut active,
                                                &mut diarizer,
                                                &mut prebuffer,
                                            )
                                            .await;
                                        }
                                    }
                                    if let Some(rec) = active.as_mut() {
                                        if rec.resume_at(std::time::Instant::now()) {
                                            if let Some(d) = diarizer.as_mut() {
                                                d.resume();
                                            }
                                            self.meeting_tx.send_modify(|s| s.paused = false);
                                            info!("recording resumed at {}s", rec.duration_secs());
                                        }
                                    }
                                }
                                Command::AddBookmark => {
                                    if let Some(ref rec) = active {
                                        if rec.is_paused() {
                                            info!("bookmark ignored while paused");
                                            continue;
                                        }
                                        let ts = rec.duration_secs();
                                        match db::add_bookmark(&self.pool, &rec.id, ts).await {
                                            Ok(()) => {
                                                self.meeting_tx
                                                    .send_modify(|s| s.marks = s.marks.saturating_add(1));
                                                info!("bookmark at {}s", ts);
                                            }
                                            Err(error) => warn!("bookmark at {ts}s not saved: {error}"),
                                        }
                                    }
                                }
                                Command::SaveRecording(id) => {
                                    self.handle_save_recording(id).await;
                                }
                                Command::SendRecording(id) => {
                                    let pending = self
                                        .meeting_tx
                                        .borrow()
                                        .pending_decision
                                        .as_ref()
                                        .is_some_and(|decision| decision.id == id);
                                    if pending {
                                        self.handle_send_recording(id).await;
                                        upload_wake.notify_one();
                                    } else if self.requeue(id, "send").await {
                                        upload_wake.notify_one();
                                    }
                                }
                                Command::DiscardRecording(id) => {
                                    self.handle_discard_recording(id);
                                }
                                Command::DeleteRecording(id) => {
                                    if active.as_ref().is_some_and(|rec| rec.id == id) {
                                        warn!("delete ignored: recording {id} is still running");
                                        continue;
                                    }
                                    self.spawn_removal(id);
                                }
                                Command::RetryUpload(id) => {
                                    if self.requeue(id, "retry").await {
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
                                        prebuffer.clear();
                                        prebuffer.resume();
                                        let finalization = self.stop_recording(rec, self.stop_policy());
                                        self.await_finalization_with_drain(
                                            &mut prebuffer,
                                            &mut doa_closed,
                                            finalization,
                                        )
                                        .await;
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
                            if refresh_files {
                                self.publish_files().await;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("meeting_srv lagged by {n} commands");
                        }
                    }
                }
                Ok(()) = settings_rx.changed() => {
                    settings_rx.borrow_and_update();
                    self.sync_pre_recording(&mut prebuffer);
                }
                Ok(()) = audio_state_rx.changed() => {
                    // Mute and capture health feed the dot; the bottom of
                    // the loop republishes it. The audio watch changes about
                    // 47 times per second (VU levels), so only mark it seen.
                    let _ = audio_state_rx.borrow_and_update();
                }
                Ok(()) = wifi_rx.changed() => {
                    if wifi_rx.borrow().connected {
                        upload_wake.notify_one();
                    }
                }
            }
            self.publish_pre_recording(active.is_some());
        }
    }

    fn stop_recording(
        &self,
        rec: recorder::ActiveRecording,
        policy: StopPolicy,
    ) -> impl Future<Output = ()> + Send + 'static {
        RecordingFinalizer::from(self).stop_recording(rec, policy)
    }

    /// Make the pre-roll follow the latest `pre_recording` setting. Cheap, and
    /// called wherever the buffer is pushed to or spliced from, so a change
    /// that the settings branch has not handled yet is never missed.
    fn sync_pre_recording(&self, prebuffer: &mut PreBuffer) {
        let pre_recording = self.settings_rx.borrow().pre_recording;
        if pre_recording != prebuffer.is_enabled() {
            prebuffer.set_enabled(pre_recording);
            info!(
                "pre-recording {}",
                if pre_recording { "on" } else { "off; buffered audio dropped" }
            );
        }
    }

    /// Publish the D3 dot from the live setting, the take state and the audio
    /// state. Notifies subscribers only when the value actually changes.
    fn publish_pre_recording(&self, recording: bool) {
        let active = pre_recording_active(
            self.settings_rx.borrow().pre_recording,
            recording,
            &self.audio_state_rx.borrow(),
        );
        self.meeting_tx.send_if_modified(|state| {
            if state.pre_recording_active == active {
                return false;
            }
            state.pre_recording_active = active;
            true
        });
    }

    async fn await_finalization_with_drain<F>(
        &mut self,
        prebuffer: &mut PreBuffer,
        doa_closed: &mut bool,
        finalization: F,
    ) where
        F: Future<Output = ()>,
    {
        tokio::pin!(finalization);
        let mut audio_closed = false;
        let mut integrity_closed = false;

        loop {
            tokio::select! {
                biased;

                () = &mut finalization => {
                    // `biased` gives durable completion priority. Drain frames
                    // that were already queued at that exact boundary before
                    // command handling resumes, so an immediately queued Start
                    // cannot overtake its own pre-roll.
                    while let Ok(frame) = self.audio_rx.try_recv() {
                        self.sync_pre_recording(prebuffer);
                        prebuffer.push(&frame.mic_mono_f32);
                    }
                    break;
                },
                frame = self.audio_rx.recv(), if !audio_closed => {
                    match frame {
                        Some(frame) => {
                            self.sync_pre_recording(prebuffer);
                            prebuffer.push(&frame.mic_mono_f32);
                        }
                        None => audio_closed = true,
                    }
                }
                signal = self.integrity_rx.recv(), if !integrity_closed => {
                    match signal {
                        Some(signal) => {
                            self.publish_idle_fault(signal.fault);
                            if let Some(acknowledgement) = signal.acknowledgement {
                                let _ = acknowledgement.send(());
                            }
                        }
                        None => integrity_closed = true,
                    }
                }
                doa_recv = self.doa_rx.recv(), if !*doa_closed => {
                    match doa_recv {
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("diarization receiver lagged {n} frames during finalization");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            warn!("DoA stream closed; diarization paused");
                            *doa_closed = true;
                        }
                    }
                }
            }
        }
    }

    /// Write every frame already queued to the take: they were captured
    /// before the command being handled (Stop, Pause). A paused take drops
    /// them (`append_samples`). On a failure the fault is recorded and
    /// returned, and the caller ends the take.
    fn append_queued_frames(
        &mut self,
        rec: &mut recorder::ActiveRecording,
    ) -> Option<RecordingIntegrityFault> {
        loop {
            let fault = match self.audio_rx.try_recv() {
                Ok(frame) => match rec.append_samples(&frame.mic_mono_f32) {
                    Ok(_) => continue,
                    Err(_) => RecordingIntegrityFault::WavWriteFailed,
                },
                Err(mpsc::error::TryRecvError::Empty) => return None,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    RecordingIntegrityFault::ProcessedFrameChannelClosed
                }
            };
            self.integrity.record_fault(fault);
            return Some(fault);
        }
    }

    /// Drop every frame already queued: they were captured during a pause.
    fn discard_queued_frames(&mut self) {
        while self.audio_rx.try_recv().is_ok() {}
    }

    /// Judge one integrity signal against the running take, then acknowledge
    /// it. A fault since the take's epoch ends the take, unless the take is
    /// paused and the pipeline is still there: then nothing was being written
    /// and the take goes on. Without a take the fault is published as idle.
    ///
    /// Relies on each `RecordingIntegrity::record_fault` by the audio
    /// pipeline being followed by exactly one signal: a tolerated signal
    /// accepts exactly one fault (+1 on the take's epoch), so a capture
    /// failure recorded behind it still ends the take when its own signal
    /// lands.
    async fn handle_integrity_signal(
        &mut self,
        signal: IntegritySignal,
        active: &mut Option<recorder::ActiveRecording>,
        diarizer: &mut Option<crate::diarization::Diarizer>,
        prebuffer: &mut PreBuffer,
    ) {
        let fault = signal.fault;
        let epoch = self.integrity.epoch();
        let moved = active
            .as_ref()
            .is_some_and(|rec| epoch != rec.integrity_epoch());
        let tolerated = moved
            && active
                .as_ref()
                .is_some_and(|rec| paused_take_tolerates(rec, fault));
        if tolerated {
            if let Some(rec) = active.as_mut() {
                warn!(
                    "{} while paused; nothing was being written, the take continues",
                    fault.diagnostic()
                );
                rec.rebase_integrity_epoch(rec.integrity_epoch() + 1);
            }
        } else if moved {
            let rec = active.take().expect("active recording disappeared");
            *diarizer = None;
            self.abort_recording(rec, fault).await;
            prebuffer.clear();
            prebuffer.resume();
        } else {
            self.publish_idle_fault(fault);
        }
        if let Some(acknowledgement) = signal.acknowledgement {
            let _ = acknowledgement.send(());
        }
    }

    fn drain_before_stop(
        &mut self,
        rec: &mut recorder::ActiveRecording,
    ) -> (
        Option<RecordingIntegrityFault>,
        Vec<tokio::sync::oneshot::Sender<()>>,
    ) {
        let mut fault = self.append_queued_frames(rec);
        let mut acknowledgements = Vec::new();

        while let Ok(signal) = self.integrity_rx.try_recv() {
            if self.integrity.epoch() != rec.integrity_epoch()
                && !paused_take_tolerates(rec, signal.fault)
            {
                fault.get_or_insert(signal.fault);
            }
            if let Some(acknowledgement) = signal.acknowledgement {
                acknowledgements.push(acknowledgement);
            }
        }
        if self.integrity.epoch() != rec.integrity_epoch() {
            fault = fault.or_else(|| {
                self.integrity
                    .last_fault()
                    .filter(|last| !paused_take_tolerates(rec, *last))
            });
        }

        (fault, acknowledgements)
    }

    async fn abort_recording(
        &self,
        rec: recorder::ActiveRecording,
        fault: RecordingIntegrityFault,
    ) {
        let id = rec.id;
        let duration = rec.duration_secs();
        match rec.abort() {
            Ok(path) => warn!(
                "recording interrupted: {id}; partial WAV preserved at {} ({})",
                path.display(),
                fault.diagnostic()
            ),
            Err(error) => warn!("failed to close partial recording {id}: {error}"),
        }
        self.mark_recording_fault(id, duration, fault).await;
    }

    async fn mark_recording_fault(
        &self,
        id: Uuid,
        duration: u32,
        fault: RecordingIntegrityFault,
    ) {
        if let Err(error) = db::mark_artifact_error(&self.pool, &id, duration, fault).await {
            warn!("failed to persist recording integrity fault for {id}: {error}");
        }
        self.meeting_tx.send_modify(|state| {
            state.recording = false;
            state.current_speaker = None;
            state.paused = false;
            state.meeting_id = None;
            state.duration_secs = duration;
            state.pending_decision = None;
            state.last_recording_error = Some(fault);
            if fault.blocks_recording() {
                state.recording_blocked_reason = Some(fault);
            }
        });
        let _ = self.events_tx.send(DeviceEvent::RecordingIntegrityFailed {
            meeting_id: Some(id),
            fault,
        });
    }

    fn publish_speaker(&self, speaker: Option<u8>) {
        self.meeting_tx.send_if_modified(|state| {
            if state.current_speaker == speaker {
                return false;
            }
            state.current_speaker = speaker;
            true
        });
    }

    fn publish_idle_fault(&self, fault: RecordingIntegrityFault) {
        self.meeting_tx.send_modify(|state| {
            state.last_recording_error = Some(fault);
            if fault.blocks_recording() {
                state.recording_blocked_reason = Some(fault);
            }
        });
        let _ = self.events_tx.send(DeviceEvent::RecordingIntegrityFailed {
            meeting_id: None,
            fault,
        });
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
        self.meeting_tx.send_modify(|s| {
            s.pending_decision = None;
            s.ask_deadline_secs = None;
        });
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

    /// Queue a kept or failed recording for delivery again, from Files.
    /// Returns true when it was queued. Without an upload server nothing is
    /// queued: the recording stays as it was instead of looking like it is on
    /// its way.
    async fn requeue(&self, id: Uuid, why: &str) -> bool {
        if !upload_server_configured(&self.config.api.endpoint) {
            info!("{why} ignored: no webhook endpoint is configured; {id} stays on Wave");
            return false;
        }
        match db::requeue_for_delivery(&self.pool, &id).await {
            Ok(true) => {
                info!("{why}: {id} queued for delivery");
                true
            }
            Ok(false) => {
                warn!("{why} ignored: {id} is not a kept or failed recording");
                false
            }
            Err(error) => {
                warn!("{why}: db update failed for {id}: {error}");
                false
            }
        }
    }

    async fn handle_send_recording(&self, id: Uuid) {
        let Some(decision) = self.validate_pending(&id) else {
            return;
        };
        // The endpoint snapshot from Stop: without a server there is nowhere
        // to send it, so it is kept on Wave rather than left looking queued.
        let status = if upload_server_configured(&decision.endpoint) {
            MeetingStatus::Queued
        } else {
            MeetingStatus::LocalOnly
        };
        if let Err(e) = db::set_status(&self.pool, &id, status).await {
            warn!("send: db update failed for {id}: {e}");
        }
        if status == MeetingStatus::Queued {
            info!("send: {id} queued for the background webhook worker");
        } else {
            info!("send: no webhook endpoint is configured; {id} kept on Wave");
        }
        self.clear_pending();
    }

    fn handle_discard_recording(&self, id: Uuid) {
        if self.validate_pending(&id).is_none() {
            return;
        }
        // Answered here, before the removal runs, so the commands that follow
        // (Record, a stray Send) already see no pending decision. The question
        // is answered even if the files cannot be removed.
        self.clear_pending();
        self.spawn_removal(id);
    }

    /// Remove a recording off the service loop: the removal syncs the disk,
    /// deletes a WAV of up to hundreds of MB and waits for the database,
    /// while the loop must keep draining a running take's frames.
    fn spawn_removal(&self, id: Uuid) {
        let pool = self.pool.clone();
        let recordings_dir = self.recordings_dir.clone();
        let storage = self.config.storage.clone();
        let meeting_tx = Arc::clone(&self.meeting_tx);
        let files_refresh = Arc::clone(&self.files_refresh);
        tokio::spawn(async move {
            let outcome = removal::remove_recording(&pool, &recordings_dir, id).await;
            // A Delete of the pending take answers its question once the row
            // is claimed (`deleting` even if the files remain) or the attempt
            // failed, so the auto-Keep never writes over it.
            let answered = !matches!(
                outcome,
                Ok(removal::RemoveOutcome::Uploading | removal::RemoveOutcome::NotFound)
            );
            match &outcome {
                Ok(removal::RemoveOutcome::Removed { previous_status }) => {
                    info!("audit: recording {id} deleted on request (was {previous_status})")
                }
                Ok(removal::RemoveOutcome::Uploading) => {
                    warn!("delete refused: recording {id} is uploading right now")
                }
                Ok(removal::RemoveOutcome::NotFound) => warn!("delete ignored: no recording {id}"),
                Err(error) => warn!("delete of recording {id} failed: {error}"),
            }
            if answered {
                meeting_tx.send_if_modified(|state| {
                    if !state.pending_decision.as_ref().is_some_and(|d| d.id == id) {
                        return false;
                    }
                    state.pending_decision = None;
                    state.ask_deadline_secs = None;
                    true
                });
            }
            if matches!(outcome, Ok(removal::RemoveOutcome::Removed { .. })) {
                // Space was freed: lift a storage block now, not at the next Record.
                retention::enforce(&pool, &recordings_dir, &storage, &meeting_tx).await;
            }
            // The loop's refresh after the command may have run before the
            // removal finished; publish the list as it is now.
            files_refresh.notify_one();
        });
    }
}

/// Commands after which the Files list is republished right away.
fn changes_files(command: &Command) -> bool {
    matches!(
        command,
        Command::StopRecording
            | Command::SaveRecording(_)
            | Command::SendRecording(_)
            | Command::DiscardRecording(_)
            | Command::RetryUpload(_)
            | Command::RetryFailedUploads
            | Command::DeleteRecording(_)
    )
}

#[cfg(feature = "recording-integrity-test-hooks")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FinalizationCheckpoint {
    ActivePartial,
    WavFinalized,
    MetadataFinalized,
}

#[cfg(feature = "recording-integrity-test-hooks")]
impl FinalizationCheckpoint {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ActivePartial => "active-partial",
            Self::WavFinalized => "wav-finalized",
            Self::MetadataFinalized => "metadata-finalized",
        }
    }
}

#[cfg(feature = "recording-integrity-test-hooks")]
async fn finalization_test_checkpoint(checkpoint: FinalizationCheckpoint, id: Uuid) {
    const REQUEST_PATH: &str = "/tmp/waved-finalization-checkpoint";
    const READY_PATH: &str = "/tmp/waved-finalization-ready";

    let requested = match tokio::fs::read_to_string(REQUEST_PATH).await {
        Ok(requested) => requested,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            warn!("finalization TEST CHECKPOINT request read failed: {error}");
            return;
        }
    };
    if requested.trim() != checkpoint.as_str() {
        return;
    }

    let ready_new = format!("{READY_PATH}.new");
    let ready = format!("{} {id}\n", checkpoint.as_str());
    if let Err(error) = tokio::fs::write(&ready_new, ready).await {
        warn!("finalization TEST CHECKPOINT marker write failed: {error}");
        return;
    }
    if let Err(error) = tokio::fs::rename(&ready_new, READY_PATH).await {
        warn!("finalization TEST CHECKPOINT marker publish failed: {error}");
        let _ = tokio::fs::remove_file(&ready_new).await;
        return;
    }
    warn!(
        "finalization TEST CHECKPOINT {} reached for {id}",
        checkpoint.as_str()
    );

    while tokio::fs::try_exists(REQUEST_PATH)
        .await
        .unwrap_or(false)
    {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let _ = tokio::fs::remove_file(READY_PATH).await;
    warn!(
        "finalization TEST CHECKPOINT {} released for {id}",
        checkpoint.as_str()
    );
}

#[cfg(test)]
mod wiring_tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::{broadcast, mpsc, watch};
    use waved_audio::doa::{DoaBackend, DoaFrame};
    use waved_audio::integrity::{IntegritySignal, RecordingIntegrity};
    use waved_audio::processed_frame::ProcessedAudioFrame;
    use waved_config::WavedConfig;
    use waved_state::{
        Command, DeviceEvent, MeetingState, RecordingIntegrityFault,
    };

    #[cfg(feature = "recording-integrity-test-hooks")]
    #[test]
    fn finalization_checkpoint_names_are_stable() {
        assert_eq!(
            FinalizationCheckpoint::ActivePartial.as_str(),
            "active-partial"
        );
        assert_eq!(
            FinalizationCheckpoint::WavFinalized.as_str(),
            "wav-finalized"
        );
        assert_eq!(
            FinalizationCheckpoint::MetadataFinalized.as_str(),
            "metadata-finalized"
        );
    }

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

    fn processed_frame(timestamp_ms: u64) -> ProcessedAudioFrame {
        ProcessedAudioFrame {
            mic_mono_f32: vec![0.25; 480],
            mic_stereo_f32: None,
            reference_f32: vec![0.0; 480],
            sample_rate: 48_000,
            timestamp_ms,
            raw_mic_i16: vec![0; 480],
            raw_mic_channels: 1,
            aec_metrics: Default::default(),
        }
    }

    /// Longest any wiring test waits for an asynchronous condition. Generous
    /// on purpose: a slow container must not fail a test that is only waiting.
    const WAIT_DEADLINE: Duration = Duration::from_secs(5);

    async fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + WAIT_DEADLINE;
        while !ready() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Waits until the service has published an active take and returns its id.
    async fn wait_for_recording(meeting_tx: &watch::Sender<MeetingState>) -> Uuid {
        wait_until("the recording to start", || {
            let state = meeting_tx.borrow();
            state.recording && state.meeting_id.is_some()
        })
        .await;
        let id = meeting_tx.borrow().meeting_id;
        id.expect("recording started")
    }

    async fn wait_for_upload_status(pool: &sqlx::SqlitePool, id: Uuid, want: MeetingStatus) {
        let deadline = tokio::time::Instant::now() + WAIT_DEADLINE;
        loop {
            let row: Option<(String,)> =
                sqlx::query_as("SELECT upload_status FROM meetings WHERE id = ?")
                    .bind(id.to_string())
                    .fetch_optional(pool)
                    .await
                    .unwrap();
            let got = row.map(|(status,)| status);
            if got.as_deref() == Some(want.as_str()) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for upload_status {} of {id}, last seen {got:?}",
                want.as_str()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn wait_for_diarization_rows(pool: &sqlx::SqlitePool, at_least: usize) -> Vec<(i64,)> {
        let deadline = tokio::time::Instant::now() + WAIT_DEADLINE;
        loop {
            let rows: Vec<(i64,)> =
                sqlx::query_as("SELECT speaker_id FROM diarization_segments ORDER BY id")
                    .fetch_all(pool)
                    .await
                    .unwrap();
            if rows.len() >= at_least {
                return rows;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {at_least} diarization segments, got {}",
                rows.len()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
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

        // Start recording. DoA frames sent before the take exists are dropped.
        cmd_tx.send(Command::StartRecording).unwrap();
        wait_for_recording(&meeting_tx).await;

        // Push DoA frames: 500ms at az=45°, then 500ms at az=180°.
        for i in 0..24u64 {
            doa_tx_handle.send(high(i * 21, 45.0)).unwrap();
        }
        for i in 24..48u64 {
            doa_tx_handle.send(high(i * 21, 180.0)).unwrap();
        }
        // The service holds the only receiver, so an empty queue means every
        // frame was handled. Stop must not overtake queued frames.
        wait_until("the service to drain the DoA frames", || {
            doa_tx_handle.is_empty()
        })
        .await;

        // Stop recording → finalize diarizer; wait for the segments it writes.
        cmd_tx.send(Command::StopRecording).unwrap();
        let pool = db::init_pool(db_path.to_str().unwrap()).await.unwrap();
        let rows = wait_for_diarization_rows(&pool, 2).await;
        assert!(rows.len() >= 2, "expected ≥ 2 segments, got {}", rows.len());
        assert_eq!(rows[0].0, 1, "first segment should be speaker_id=1");
        assert_eq!(rows[1].0, 2, "second segment should be speaker_id=2");
    }

    #[tokio::test]
    async fn integrity_signal_aborts_take_before_ack_and_excludes_upload() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("t.db");
        let recordings_dir = tmp.path().join("rec");
        std::fs::create_dir_all(&recordings_dir).unwrap();
        let (meeting_tx_inner, _meeting_rx) = watch::channel(MeetingState::default());
        let meeting_tx = Arc::new(meeting_tx_inner);
        let (events_tx, mut events_rx) = broadcast::channel::<DeviceEvent>(16);
        let (audio_tx, audio_rx) = mpsc::channel(64);
        let (doa_tx, _) = broadcast::channel::<DoaFrame>(4);
        let integrity = Arc::new(RecordingIntegrity::default());
        let (integrity_tx, integrity_rx) = mpsc::unbounded_channel();
        let mut config = WavedConfig::default();
        config.storage.prune_uploaded_at_percent = 100;
        config.storage.minimum_free_mb = 0;
        let svc = MeetingService::new_with_integrity(
            config,
            db_path.to_str().unwrap(),
            recordings_dir.clone(),
            Arc::clone(&meeting_tx),
            events_tx,
            MeetingPipelineInputs {
                audio_rx,
                doa_rx: doa_tx.subscribe(),
                integrity: Arc::clone(&integrity),
                integrity_rx,
            },
        )
        .await
        .unwrap();
        let (cmd_tx, cmd_rx) = broadcast::channel::<Command>(16);
        let (_wifi_tx, wifi_rx) = watch::channel(WifiState::default());
        let _handle = tokio::spawn(svc.run(cmd_rx, wifi_rx));

        cmd_tx.send(Command::StartRecording).unwrap();
        let id = wait_for_recording(&meeting_tx).await;
        audio_tx.send(processed_frame(0)).await.unwrap();
        // The fault must hit a take that already holds audio: wait until the
        // service has taken the frame off the channel.
        wait_until("the service to consume the audio frame", || {
            audio_tx.capacity() == audio_tx.max_capacity()
        })
        .await;

        integrity.record_fault(RecordingIntegrityFault::ProcessedFrameDropped);
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        integrity_tx
            .send(IntegritySignal::with_acknowledgement(
                RecordingIntegrityFault::ProcessedFrameDropped,
                ack_tx,
            ))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), ack_rx)
            .await
            .expect("fault acknowledgement timed out")
            .unwrap();

        let pool = db::init_pool(db_path.to_str().unwrap()).await.unwrap();
        let row: (String, Option<String>) =
            sqlx::query_as("SELECT upload_status, last_error FROM meetings WHERE id = ?")
                .bind(id.to_string())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, MeetingStatus::ArtifactError.as_str());
        assert_eq!(
            row.1.as_deref(),
            Some(RecordingIntegrityFault::ProcessedFrameDropped.diagnostic())
        );
        let dir = recordings_dir.join(id.to_string());
        assert!(dir.join("audio.wav.partial").exists());
        assert!(!dir.join("audio.wav").exists());
        assert!(!dir.join("metadata.json").exists());
        assert!(!meeting_tx.borrow().recording);
        assert_eq!(
            meeting_tx.borrow().last_recording_error,
            Some(RecordingIntegrityFault::ProcessedFrameDropped)
        );
        assert_eq!(meeting_tx.borrow().recording_blocked_reason, None);
        assert!(matches!(
            events_rx.try_recv(),
            Ok(DeviceEvent::RecordingStarted(event_id)) if event_id == id
        ));
        assert!(matches!(
            events_rx.try_recv(),
            Ok(DeviceEvent::RecordingIntegrityFailed {
                meeting_id: Some(event_id),
                fault: RecordingIntegrityFault::ProcessedFrameDropped,
            }) if event_id == id
        ));
        assert!(db::claim_next_upload(&pool, 5).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn finalization_drain_prevents_processed_channel_backpressure() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("t.db");
        let recordings_dir = tmp.path().join("rec");
        std::fs::create_dir_all(&recordings_dir).unwrap();
        let (meeting_tx_inner, _meeting_rx) = watch::channel(MeetingState::default());
        let meeting_tx = Arc::new(meeting_tx_inner);
        let (events_tx, mut events_rx) = broadcast::channel::<DeviceEvent>(16);
        let (audio_tx, audio_rx) = mpsc::channel(2);
        let (doa_tx, _) = broadcast::channel::<DoaFrame>(4);
        let integrity = Arc::new(RecordingIntegrity::default());
        let (integrity_tx, integrity_rx) = mpsc::unbounded_channel();
        let mut config = WavedConfig::default();
        config.storage.prune_uploaded_at_percent = 100;
        config.storage.minimum_free_mb = 0;
        let mut svc = MeetingService::new_with_integrity(
            config,
            db_path.to_str().unwrap(),
            recordings_dir,
            Arc::clone(&meeting_tx),
            events_tx,
            MeetingPipelineInputs {
                audio_rx,
                doa_rx: doa_tx.subscribe(),
                integrity: Arc::clone(&integrity),
                integrity_rx,
            },
        )
        .await
        .unwrap();

        let streamed_frames = 12_usize;
        let boundary_frames = 2_usize;
        let expected_frames = streamed_frames + boundary_frames;
        let integrity_for_producer = Arc::clone(&integrity);
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let producer = tokio::spawn(async move {
            for index in 0..streamed_frames {
                audio_tx
                    .send(processed_frame(index as u64 * 10))
                    .await
                    .unwrap();
            }
            while audio_tx.capacity() != audio_tx.max_capacity() {
                tokio::task::yield_now().await;
            }

            integrity_for_producer
                .record_fault(RecordingIntegrityFault::ProcessedFrameDropped);
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            integrity_tx
                .send(IntegritySignal::with_acknowledgement(
                    RecordingIntegrityFault::ProcessedFrameDropped,
                    ack_tx,
                ))
                .unwrap();
            tokio::time::timeout(Duration::from_secs(1), ack_rx)
                .await
                .expect("idle integrity acknowledgement timed out")
                .unwrap();

            // Queue a full channel without yielding, then release finalization.
            // The completion branch must drain these boundary frames before it
            // returns to command handling.
            for index in 0..boundary_frames {
                audio_tx
                    .try_send(processed_frame((streamed_frames + index) as u64 * 10))
                    .unwrap();
            }
            release_tx.send(()).unwrap();
        });

        let mut prebuffer = PreBuffer::new(1, RECORDING_SAMPLE_RATE, 1);
        let mut doa_closed = false;
        tokio::time::timeout(
            Duration::from_secs(1),
            svc.await_finalization_with_drain(
                &mut prebuffer,
                &mut doa_closed,
                async move {
                    release_rx.await.unwrap();
                },
            ),
        )
        .await
        .expect("finalization drain deadlocked");
        producer.await.unwrap();

        let samples = prebuffer.drain_all();
        assert_eq!(samples.len(), expected_frames * 480);
        assert!(samples.iter().all(|sample| *sample == 0.25));
        assert_eq!(
            meeting_tx.borrow().last_recording_error,
            Some(RecordingIntegrityFault::ProcessedFrameDropped)
        );
        assert!(matches!(
            events_rx.try_recv(),
            Ok(DeviceEvent::RecordingIntegrityFailed {
                meeting_id: None,
                fault: RecordingIntegrityFault::ProcessedFrameDropped,
            })
        ));
    }

    #[tokio::test]
    async fn stop_drains_queued_fault_instead_of_finalizing_and_storage_fault_blocks_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("t.db");
        let recordings_dir = tmp.path().join("rec");
        std::fs::create_dir_all(&recordings_dir).unwrap();
        let (meeting_tx_inner, _meeting_rx) = watch::channel(MeetingState::default());
        let meeting_tx = Arc::new(meeting_tx_inner);
        let (events_tx, _events_rx) = broadcast::channel::<DeviceEvent>(16);
        let (audio_tx, audio_rx) = mpsc::channel(64);
        let (doa_tx, _) = broadcast::channel::<DoaFrame>(4);
        let integrity = Arc::new(RecordingIntegrity::default());
        let (integrity_tx, integrity_rx) = mpsc::unbounded_channel();
        let mut config = WavedConfig::default();
        config.storage.prune_uploaded_at_percent = 100;
        config.storage.minimum_free_mb = 0;
        let svc = MeetingService::new_with_integrity(
            config,
            db_path.to_str().unwrap(),
            recordings_dir.clone(),
            Arc::clone(&meeting_tx),
            events_tx,
            MeetingPipelineInputs {
                audio_rx,
                doa_rx: doa_tx.subscribe(),
                integrity: Arc::clone(&integrity),
                integrity_rx,
            },
        )
        .await
        .unwrap();
        let (cmd_tx, cmd_rx) = broadcast::channel::<Command>(16);
        let (_wifi_tx, wifi_rx) = watch::channel(WifiState::default());
        let _handle = tokio::spawn(svc.run(cmd_rx, wifi_rx));

        cmd_tx.send(Command::StartRecording).unwrap();
        let id = wait_for_recording(&meeting_tx).await;
        audio_tx.send(processed_frame(0)).await.unwrap();
        integrity.record_fault(RecordingIntegrityFault::WavWriteFailed);
        integrity_tx
            .send(IntegritySignal::new(
                RecordingIntegrityFault::WavWriteFailed,
            ))
            .unwrap();
        cmd_tx.send(Command::StopRecording).unwrap();

        let pool = db::init_pool(db_path.to_str().unwrap()).await.unwrap();
        wait_for_upload_status(&pool, id, MeetingStatus::ArtifactError).await;
        wait_until("the storage fault to block restarts", || {
            meeting_tx.borrow().recording_blocked_reason.is_some()
        })
        .await;
        let (status,): (String,) =
            sqlx::query_as("SELECT upload_status FROM meetings WHERE id = ?")
                .bind(id.to_string())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, MeetingStatus::ArtifactError.as_str());
        let dir = recordings_dir.join(id.to_string());
        assert!(dir.join("audio.wav.partial").exists());
        assert!(!dir.join("audio.wav").exists());
        assert_eq!(
            meeting_tx.borrow().recording_blocked_reason,
            Some(RecordingIntegrityFault::WavWriteFailed)
        );

        cmd_tx.send(Command::StartRecording).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!meeting_tx.borrow().recording);
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM meetings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "blocked start created another meeting");
    }
}
