pub mod db;
pub mod recorder;
pub mod upload;

#[cfg(feature = "simulate")]
pub mod simulate;

use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{error, info, warn};
use waved_audio::processed_frame::ProcessedAudioFrame;
use waved_config::WavedConfig;
use waved_state::{Command, DeviceEvent, MeetingState, WifiState};

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
                                        self.stop_recording(rec, &wifi_rx).await;
                                    }
                                }
                                Command::AddBookmark => {
                                    if let Some(ref rec) = active {
                                        let ts = rec.duration_secs();
                                        let _ = db::add_bookmark(&self.pool, &rec.id, ts).await;
                                        info!("bookmark at {}s", ts);
                                    }
                                }
                                // Errata #8: handle Shutdown
                                Command::Shutdown => {
                                    if let Some(rec) = active.take() {
                                        info!("shutdown: stopping active recording first");
                                        self.stop_recording(rec, &wifi_rx).await;
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

    async fn stop_recording(
        &self,
        mut rec: recorder::ActiveRecording,
        wifi_rx: &watch::Receiver<WifiState>,
    ) {
        let duration = rec.duration_secs();
        let id = rec.id;
        // Close the WAV header before anyone (upload, UI playback) reads
        // the file. The Drop impl is a fallback for panic paths only.
        rec.finalize();
        let _ = db::finish_meeting(&self.pool, &id, duration).await;
        let _ = self.events_tx.send(DeviceEvent::RecordingStopped(id));
        self.meeting_tx.send_modify(|s| {
            s.recording = false;
            s.paused = false;
            s.meeting_id = None;
            s.duration_secs = duration;
        });
        info!("recording stopped: {} ({}s)", id, duration);
        if self.config.api.auto_upload && wifi_rx.borrow().connected {
            self.try_upload_with_retry(&id.to_string(), &rec.audio_path())
                .await;
        }
    }

    // Errata #7: upload with retry loop
    async fn try_upload_with_retry(&self, meeting_id: &str, audio_path: &std::path::Path) {
        let max_retries = self.config.api.retry_max;
        let backoff_ms = self.config.api.retry_backoff_ms;

        for attempt in 1..=max_retries {
            #[cfg(not(feature = "simulate"))]
            let upload_result = upload::upload_recording(
                &self.config.api.endpoint,
                &self.config.api.auth_token,
                meeting_id,
                audio_path,
            )
            .await;

            #[cfg(feature = "simulate")]
            let upload_result = simulate::upload_recording(
                &self.config.api.endpoint,
                &self.config.api.auth_token,
                meeting_id,
                audio_path,
            )
            .await;

            match upload_result {
                Ok(()) => {
                    if let Ok(uuid) = meeting_id.parse() {
                        let _ = db::set_upload_status(&self.pool, &uuid, "uploaded").await;
                    }
                    return;
                }
                Err(e) => {
                    warn!("upload attempt {attempt}/{max_retries} failed for {meeting_id}: {e}");
                    if attempt < max_retries {
                        let delay = Duration::from_millis(backoff_ms * attempt as u64);
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
        warn!("all upload attempts exhausted for {meeting_id}");
        if let Ok(uuid) = meeting_id.parse() {
            let _ = db::set_upload_status(&self.pool, &uuid, "upload_failed").await;
        }
    }

    async fn flush_pending_uploads(&self) {
        match db::get_pending_uploads(&self.pool).await {
            Ok(pending) => {
                for (id, path) in pending {
                    let audio_path = std::path::PathBuf::from(&path).join("audio.wav");
                    self.try_upload_with_retry(&id, &audio_path).await;
                }
            }
            Err(e) => error!("failed to get pending uploads: {e}"),
        }
    }
}
