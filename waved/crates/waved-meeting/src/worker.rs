use crate::db;
use crate::model::MeetingStatus;
use crate::upload::UploadError;
#[cfg(not(feature = "simulate"))]
use crate::upload::WebhookClient;
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, watch, Notify};
use tracing::{error, info, warn};
use waved_config::{ApiConfig, StorageConfig};
use waved_state::{DeviceEvent, MeetingState, WifiState};

pub struct UploadWorker {
    pool: SqlitePool,
    api: ApiConfig,
    storage: StorageConfig,
    recordings_dir: PathBuf,
    webhook_test_marker: PathBuf,
    device_id: String,
    meeting_tx: Arc<watch::Sender<MeetingState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
    wake: Arc<Notify>,
}

pub struct WorkerSettings {
    pub api: ApiConfig,
    pub storage: StorageConfig,
    pub recordings_dir: PathBuf,
    pub webhook_test_marker: PathBuf,
    pub device_id: String,
}

impl UploadWorker {
    pub fn new(
        pool: SqlitePool,
        settings: WorkerSettings,
        meeting_tx: Arc<watch::Sender<MeetingState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
        wake: Arc<Notify>,
    ) -> Self {
        Self {
            pool,
            api: settings.api,
            storage: settings.storage,
            recordings_dir: settings.recordings_dir,
            webhook_test_marker: settings.webhook_test_marker,
            device_id: settings.device_id,
            meeting_tx,
            events_tx,
            wake,
        }
    }

    pub async fn run(self, mut wifi_rx: watch::Receiver<WifiState>) {
        #[cfg(feature = "simulate")]
        let _ = &self.device_id;
        #[cfg(not(feature = "simulate"))]
        let client = match WebhookClient::new() {
            Ok(client) => client,
            Err(error) => {
                error!("cannot construct webhook client: {error}");
                return;
            }
        };
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        self.refresh_counts().await;
        self.restore_last_success().await;
        self.restore_test_result().await;
        self.wake.notify_one();

        loop {
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tick.tick() => {}
                changed = wifi_rx.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }

            if !wifi_rx.borrow().connected || self.api.endpoint.trim().is_empty() {
                if self.webhook_test_marker.exists() && self.api.endpoint.trim().is_empty() {
                    self.finish_test("Not sent: endpoint is not configured")
                        .await;
                }
                self.refresh_counts().await;
                continue;
            }

            if self.webhook_test_marker.exists() {
                #[cfg(not(feature = "simulate"))]
                let test_result = client
                    .send_test(
                        &self.api.endpoint,
                        &self.api.auth_token,
                        &self.device_id,
                        &self.recordings_dir,
                    )
                    .await;
                #[cfg(feature = "simulate")]
                let test_result: Result<(), UploadError> = Ok(());
                match test_result {
                    Ok(()) => self.finish_test("Success").await,
                    Err(error) => {
                        self.finish_test(&format!("Failed: {}", test_summary(&error)))
                            .await
                    }
                }
            }

            loop {
                if !wifi_rx.borrow().connected {
                    break;
                }
                let candidate = match db::claim_next_upload(&self.pool, self.api.retry_max).await {
                    Ok(Some(candidate)) => candidate,
                    Ok(None) => break,
                    Err(error) => {
                        error!("failed to claim queued webhook: {error}");
                        break;
                    }
                };
                self.refresh_counts().await;
                let audio_path = PathBuf::from(&candidate.dir).join("audio.wav");
                let result = {
                    #[cfg(not(feature = "simulate"))]
                    {
                        client
                            .upload_recording(
                                &self.api.endpoint,
                                &self.api.auth_token,
                                &self.device_id,
                                &candidate.id.to_string(),
                                &audio_path,
                            )
                            .await
                    }
                    #[cfg(feature = "simulate")]
                    {
                        crate::simulate::upload_recording(
                            &self.api.endpoint,
                            &self.api.auth_token,
                            &candidate.id.to_string(),
                            &audio_path,
                        )
                        .await
                    }
                };

                match result {
                    Ok(()) => {
                        if let Err(error) = db::mark_uploaded(&self.pool, &candidate.id).await {
                            warn!("failed to mark {} uploaded: {error}", candidate.id);
                        } else {
                            info!("webhook delivery complete for {}", candidate.id);
                            let _ = self
                                .events_tx
                                .send(DeviceEvent::RecordingUploadSucceeded(candidate.id));
                            let timestamp = chrono::Utc::now().to_rfc3339();
                            self.meeting_tx.send_modify(|state| {
                                state.last_upload_success_at = Some(timestamp);
                                state.last_upload_error = None;
                            });
                            crate::retention::enforce(
                                &self.pool,
                                &self.recordings_dir,
                                &self.storage,
                                &self.meeting_tx,
                            )
                            .await;
                        }
                    }
                    Err(UploadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                        if let Err(db_error) = db::mark_upload_failure(
                            &self.pool,
                            &candidate.id,
                            MeetingStatus::MissingArtifact,
                            &error.to_string(),
                            None,
                            None,
                        )
                        .await
                        {
                            warn!(
                                "failed to persist missing artifact for {}: {db_error}",
                                candidate.id
                            );
                        } else {
                            let _ = self
                                .events_tx
                                .send(DeviceEvent::RecordingUploadNeedsAttention(candidate.id));
                        }
                    }
                    Err(error) if error.is_transient() => {
                        let delay = retry_delay_ms(
                            self.api.retry_backoff_ms,
                            self.api.retry_max_backoff_ms,
                            candidate.attempt_cycle,
                            candidate.id.as_bytes()[15],
                        );
                        let deadline = chrono::Utc::now()
                            .timestamp_millis()
                            .saturating_add(delay.min(i64::MAX as u64) as i64);
                        let status_code = error.status_code();
                        let summary = test_summary(&error);
                        if let Err(db_error) = db::mark_upload_failure(
                            &self.pool,
                            &candidate.id,
                            MeetingStatus::RetryWait,
                            &error.to_string(),
                            status_code,
                            Some(deadline),
                        )
                        .await
                        {
                            warn!(
                                "failed to persist retry state for {}: {db_error}",
                                candidate.id
                            );
                        } else {
                            let _ = self.events_tx.send(transient_failure_event(
                                candidate.id,
                                candidate.attempt_cycle,
                                self.api.retry_max,
                            ));
                        }
                        warn!(
                            "transient webhook failure for {}; retry {} in {}ms: {}",
                            candidate.id, candidate.attempt_cycle, delay, error
                        );
                        self.meeting_tx.send_modify(|state| {
                            state.last_upload_error = Some(summary);
                        });
                    }
                    Err(error) => {
                        let status = if matches!(error, UploadError::Io(_)) {
                            MeetingStatus::ArtifactError
                        } else {
                            MeetingStatus::ConfigurationError
                        };
                        if let Err(db_error) = db::mark_upload_failure(
                            &self.pool,
                            &candidate.id,
                            status,
                            &error.to_string(),
                            error.status_code(),
                            None,
                        )
                        .await
                        {
                            warn!(
                                "failed to persist blocked delivery for {}: {db_error}",
                                candidate.id
                            );
                        } else {
                            let _ = self
                                .events_tx
                                .send(DeviceEvent::RecordingUploadNeedsAttention(candidate.id));
                        }
                        let summary = test_summary(&error);
                        self.meeting_tx.send_modify(|state| {
                            state.last_upload_error = Some(summary);
                        });
                        warn!("webhook delivery blocked for {}: {error}", candidate.id);
                    }
                }
                self.refresh_counts().await;
            }
        }
    }

    async fn refresh_counts(&self) {
        match db::upload_counts(&self.pool, self.api.retry_max).await {
            Ok((pending, failed)) => {
                self.meeting_tx.send_modify(|state| {
                    state.pending_uploads = pending;
                    state.failed_uploads = failed;
                });
            }
            Err(error) => warn!("failed to publish webhook queue counts: {error}"),
        }
    }

    async fn restore_test_result(&self) {
        let result_path = self
            .webhook_test_marker
            .with_file_name("webhook-test.result");
        if let Ok(result) = tokio::fs::read_to_string(result_path).await {
            self.meeting_tx.send_modify(|state| {
                state.last_webhook_test = Some(result.trim().to_string());
            });
        }
    }

    async fn restore_last_success(&self) {
        match db::last_upload_success(&self.pool).await {
            Ok(last_success) => {
                self.meeting_tx.send_modify(|state| {
                    state.last_upload_success_at = last_success;
                });
            }
            Err(error) => warn!("failed to restore last webhook success: {error}"),
        }
    }

    async fn finish_test(&self, result: &str) {
        let timestamped = format!("{} — {result}", chrono::Utc::now().to_rfc3339());
        let result_path = self
            .webhook_test_marker
            .with_file_name("webhook-test.result");
        if let Err(error) = tokio::fs::write(&result_path, &timestamped).await {
            warn!("failed to persist webhook test result: {error}");
        }
        if let Err(error) = tokio::fs::remove_file(&self.webhook_test_marker).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                warn!("failed to consume webhook test marker: {error}");
            }
        }
        self.meeting_tx.send_modify(|state| {
            state.last_webhook_test = Some(timestamped);
        });
    }
}

fn transient_failure_event(id: uuid::Uuid, attempt: u32, retry_max: u32) -> DeviceEvent {
    if attempt < retry_max.max(1) {
        DeviceEvent::RecordingUploadDeferred(id)
    } else {
        DeviceEvent::RecordingUploadNeedsAttention(id)
    }
}

fn test_summary(error: &UploadError) -> String {
    match error.status_code() {
        Some(status) => format!("HTTP {status}"),
        None if error.is_transient() => "Network error".into(),
        None => match error {
            UploadError::Io(_) => "Artifact error".into(),
            _ => "Configuration error".into(),
        },
    }
}

fn retry_delay_ms(base: u64, cap: u64, attempt: u32, jitter_seed: u8) -> u64 {
    let exponent = attempt.saturating_sub(1).min(20);
    let exponential = base.saturating_mul(1_u64 << exponent).min(cap.max(base));
    // Deterministic ±10% jitter prevents a fleet reconnect from retrying in
    // lockstep without requiring an RNG or making tests flaky.
    let percentage = i64::from(jitter_seed % 21) - 10;
    let jitter = (exponential / 100) as i64 * percentage;
    (exponential as i64).saturating_add(jitter).max(1) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_exponential_capped_and_jittered_within_ten_percent() {
        assert!((900..=1_100).contains(&retry_delay_ms(1_000, 60_000, 1, 0)));
        assert!((1_800..=2_200).contains(&retry_delay_ms(1_000, 60_000, 2, 20)));
        assert!((54_000..=66_000).contains(&retry_delay_ms(1_000, 60_000, 20, 10)));
    }

    #[test]
    fn transient_outcome_requires_attention_when_attempts_are_exhausted() {
        let id = uuid::Uuid::new_v4();
        assert!(matches!(
            transient_failure_event(id, 1, 3),
            DeviceEvent::RecordingUploadDeferred(received) if received == id
        ));
        assert!(matches!(
            transient_failure_event(id, 3, 3),
            DeviceEvent::RecordingUploadNeedsAttention(received) if received == id
        ));
        assert!(matches!(
            transient_failure_event(id, 1, 0),
            DeviceEvent::RecordingUploadNeedsAttention(received) if received == id
        ));
    }
}
