//! Persists settings changes: updates the watch at once, writes config.toml
//! once per quiet period, and merges into the file as it is on disk so a
//! config saved by the setup page in the meantime keeps its fields.

use crate::change::{apply_to_config, apply_to_settings, Change};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tracing::{info, warn};
use waved_config::WavedConfig;
use waved_state::{Command, SettingsState};

/// Quiet period before writing. Holding Louder sends about three commands a second.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_secs(1);

pub struct SettingsService {
    path: PathBuf,
    settings_tx: Arc<watch::Sender<SettingsState>>,
    debounce: Duration,
    pending: Vec<Change>,
}

impl SettingsService {
    pub fn new(path: impl Into<PathBuf>, settings_tx: Arc<watch::Sender<SettingsState>>) -> Self {
        Self {
            path: path.into(),
            settings_tx,
            debounce: DEFAULT_DEBOUNCE,
            pending: Vec::new(),
        }
    }

    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    /// Runs until Shutdown, RestartForConfig or a closed channel, flushing first.
    pub async fn run(mut self, mut commands: broadcast::Receiver<Command>) {
        let mut deadline: Option<tokio::time::Instant> = None;
        loop {
            let wake_at = deadline;
            let quiet_period_over = async move {
                match wake_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                received = commands.recv() => match received {
                    Ok(Command::Shutdown) | Ok(Command::RestartForConfig) => {
                        self.flush();
                        return;
                    }
                    Ok(command) => {
                        if let Some(change) = Change::from_command(&command) {
                            self.settings_tx
                                .send_modify(|settings| apply_to_settings(settings, &change));
                            self.pending.push(change);
                            deadline.get_or_insert_with(|| tokio::time::Instant::now() + self.debounce);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("settings service lagged {skipped} commands");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        self.flush();
                        return;
                    }
                },
                _ = quiet_period_over => {
                    self.flush();
                    deadline = None;
                }
            }
        }
    }

    /// Re-read the file and apply only our changes, so fields written by the
    /// setup page in the meantime survive.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let mut config = WavedConfig::load_or_default(&self.path);
        for change in self.pending.drain(..) {
            apply_to_config(&mut config, &change);
        }
        match config.save(&self.path) {
            Ok(()) => info!("settings saved to {}", self.path.display()),
            Err(error) => warn!("settings not saved: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use waved_state::StateBus;

    fn start(path: &Path) -> (StateBus, tokio::task::JoinHandle<()>) {
        let bus = StateBus::new();
        let service = SettingsService::new(path, Arc::clone(&bus.settings))
            .with_debounce(Duration::from_millis(500));
        let handle = tokio::spawn(service.run(bus.commands.subscribe()));
        (bus, handle)
    }

    fn saved(path: &Path) -> WavedConfig {
        WavedConfig::load(path).unwrap()
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn bursts_are_saved_once_with_the_final_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        WavedConfig::default().save(&path).unwrap();
        let (bus, handle) = start(&path);

        bus.commands.send(Command::SetBrightness(40)).unwrap();
        for volume in [50u16, 60, 70, 80, 90] {
            bus.commands
                .send(Command::SetSpeakerVolume(volume))
                .unwrap();
        }
        settle().await;
        assert_eq!(
            bus.settings.borrow().brightness_pct,
            40,
            "watch updates at once"
        );
        assert_eq!(
            saved(&path).display.brightness_pct,
            80,
            "nothing written yet"
        );

        tokio::time::sleep(Duration::from_millis(600)).await;
        let config = saved(&path);
        assert_eq!(config.display.brightness_pct, 40);
        assert_eq!(config.audio.speaker_volume, 90);
        let before_last_write = saved(&dir.path().join("config.toml.bak"));
        assert_eq!(before_last_write.display.brightness_pct, 80, "written once");

        bus.commands.send(Command::Shutdown).unwrap();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_config_saved_by_setup_keeps_its_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        WavedConfig::default().save(&path).unwrap();
        let (bus, handle) = start(&path);

        bus.commands.send(Command::SetSpeakerVolume(30)).unwrap();
        settle().await;
        let mut from_setup = saved(&path);
        from_setup.api.endpoint = "https://notes.example.com/hook".into();
        from_setup.save(&path).unwrap();

        tokio::time::sleep(Duration::from_millis(600)).await;
        let config = saved(&path);
        assert_eq!(config.api.endpoint, "https://notes.example.com/hook");
        assert_eq!(config.audio.speaker_volume, 30);

        bus.commands.send(Command::Shutdown).unwrap();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_flushes_without_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        WavedConfig::default().save(&path).unwrap();
        let (bus, handle) = start(&path);

        bus.commands.send(Command::SetPreRecording(false)).unwrap();
        bus.commands.send(Command::Shutdown).unwrap();
        handle.await.unwrap();
        assert!(!saved(&path).recording.pre_recording);
    }

    #[tokio::test(start_paused = true)]
    async fn invalid_values_never_reach_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        WavedConfig::default().save(&path).unwrap();
        let (bus, handle) = start(&path);

        bus.commands
            .send(Command::SetTimeZone("../etc".into()))
            .unwrap();
        bus.commands.send(Command::SetBrightness(0)).unwrap();
        bus.commands
            .send(Command::SetBeamAim(f32::INFINITY))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;

        let config = saved(&path);
        assert_eq!(config.time.zone, "UTC");
        assert_eq!(config.display.brightness_pct, 5);
        assert_eq!(config.audio.beam_aim_deg, 0.0);
        bus.commands.send(Command::Shutdown).unwrap();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn unrelated_commands_do_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        WavedConfig::default().save(&path).unwrap();
        let (bus, handle) = start(&path);

        bus.commands.send(Command::ToggleMicMute).unwrap();
        bus.commands.send(Command::StartRecording).unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(
            !dir.path().join("config.toml.bak").exists(),
            "save() was never called"
        );

        bus.commands.send(Command::RestartForConfig).unwrap();
        handle.await.unwrap();
    }
}
