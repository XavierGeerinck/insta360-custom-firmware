//! Service-level tests for the milestone 2 recording backend. Each test boots
//! a real MeetingService on a temp SQLite file and drives it over its
//! channels, the same way main.rs wires it.

// The rig grows with the tasks: some fields only keep channels open and not
// every test uses every helper.
#![allow(dead_code)]

use super::*;
use std::time::Duration;
use waved_audio::integrity::{IntegritySignal, RecordingIntegrity};
use waved_state::{AudioMode, AudioState, SettingsState, UploadMode, WifiState};

pub(crate) fn quiet_config(upload_mode: UploadMode) -> WavedConfig {
    let mut config = WavedConfig::default();
    config.api.upload_mode = upload_mode;
    config.storage.prune_uploaded_at_percent = 100;
    config.storage.minimum_free_mb = 0;
    config
}

pub(crate) fn settings_for(upload_mode: UploadMode) -> SettingsState {
    SettingsState {
        upload_mode,
        ..SettingsState::default()
    }
}

/// One 10 ms processed frame (480 samples) holding `value`.
pub(crate) fn frame_of(value: f32) -> ProcessedAudioFrame {
    ProcessedAudioFrame {
        mic_mono_f32: vec![value; 480],
        mic_stereo_f32: None,
        reference_f32: vec![0.0; 480],
        sample_rate: 48_000,
        timestamp_ms: 0,
        raw_mic_i16: vec![0; 480],
        raw_mic_channels: 1,
        aec_metrics: Default::default(),
    }
}

pub(crate) struct Rig {
    pub meeting: Arc<watch::Sender<MeetingState>>,
    pub frames: mpsc::Sender<ProcessedAudioFrame>,
    pub settings: watch::Sender<SettingsState>,
    pub audio: watch::Sender<AudioState>,
    pub commands: broadcast::Sender<Command>,
    pub doa: broadcast::Sender<DoaFrame>,
    pub wifi: watch::Sender<WifiState>,
    pub integrity: Arc<RecordingIntegrity>,
    pub integrity_tx: mpsc::UnboundedSender<IntegritySignal>,
    pub db_path: PathBuf,
    pub recordings_dir: PathBuf,
    _tmp: tempfile::TempDir,
}

pub(crate) async fn start_rig(config: WavedConfig, settings: SettingsState) -> Rig {
    start_rig_in(tempfile::tempdir().unwrap(), config, settings).await
}

/// Boot in `tmp`, which a test may have seeded with `t.db` and `rec/` first.
pub(crate) async fn start_rig_in(
    tmp: tempfile::TempDir,
    config: WavedConfig,
    settings: SettingsState,
) -> Rig {
    let db_path = tmp.path().join("t.db");
    let recordings_dir = tmp.path().join("rec");
    std::fs::create_dir_all(&recordings_dir).unwrap();
    let meeting = Arc::new(watch::channel(MeetingState::default()).0);
    let (events, _) = broadcast::channel(64);
    let (frames, frames_rx) = mpsc::channel(512);
    let (doa, _) = broadcast::channel::<DoaFrame>(64);
    let (settings_tx, settings_rx) = watch::channel(settings);
    let (audio, audio_rx) = watch::channel(AudioState {
        mode: config.audio.mode.clone(),
        ..AudioState::default()
    });
    let integrity = Arc::new(RecordingIntegrity::default());
    let (integrity_tx, integrity_rx) = mpsc::unbounded_channel();
    let service = MeetingService::new_with_integrity(
        config,
        db_path.to_str().unwrap(),
        recordings_dir.clone(),
        Arc::clone(&meeting),
        events,
        MeetingPipelineInputs {
            audio_rx: frames_rx,
            doa_rx: doa.subscribe(),
            integrity: Arc::clone(&integrity),
            integrity_rx,
        },
    )
    .await
    .unwrap()
    .with_live_inputs(MeetingLiveInputs {
        settings: settings_rx,
        audio: audio_rx,
    });
    let (commands, commands_rx) = broadcast::channel(32);
    let (wifi, wifi_rx) = watch::channel(WifiState::default());
    tokio::spawn(service.run(commands_rx, wifi_rx));
    Rig {
        meeting,
        frames,
        settings: settings_tx,
        audio,
        commands,
        doa,
        wifi,
        integrity,
        integrity_tx,
        db_path,
        recordings_dir,
        _tmp: tmp,
    }
}

impl Rig {
    pub fn send(&self, command: Command) {
        self.commands.send(command).unwrap();
    }

    /// Queue `count` frames holding `value`, then give the service time to
    /// drain them before the next command (commands and frames arrive on
    /// different channels, and `select!` picks among ready branches at random).
    pub async fn feed(&self, count: usize, value: f32) {
        for _ in 0..count {
            self.frames.send(frame_of(value)).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    pub async fn wait_for(&self, what: &str, predicate: impl Fn(&MeetingState) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !predicate(&self.meeting.borrow()) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}: {:?}",
                *self.meeting.borrow()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub async fn start(&self) -> Uuid {
        self.send(Command::StartRecording);
        self.wait_for("recording", |s| s.recording).await;
        self.meeting.borrow().meeting_id.expect("recording has an id")
    }

    pub async fn stop(&self) {
        self.send(Command::StopRecording);
        self.wait_for("stopped", |s| !s.recording && s.meeting_id.is_none())
            .await;
    }

    pub async fn pool(&self) -> SqlitePool {
        db::init_pool(self.db_path.to_str().unwrap()).await.unwrap()
    }

    pub async fn status_in(pool: &SqlitePool, id: Uuid) -> Option<String> {
        sqlx::query_as::<_, (String,)>("SELECT upload_status FROM meetings WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await
            .unwrap()
            .map(|(status,)| status)
    }

    pub async fn status(&self, id: Uuid) -> Option<String> {
        Self::status_in(&self.pool().await, id).await
    }

    pub async fn wait_status(&self, id: Uuid, want: Option<&str>) {
        let pool = self.pool().await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let got = Self::status_in(&pool, id).await;
            if got.as_deref() == want {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "status of {id} is {got:?}, want {want:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub fn dir(&self, id: Uuid) -> PathBuf {
        self.recordings_dir.join(id.to_string())
    }

    pub fn metadata(&self, id: Uuid) -> metadata::WebhookMetadata {
        serde_json::from_slice(&std::fs::read(self.dir(id).join("metadata.json")).unwrap())
            .unwrap()
    }

    pub fn wav_samples(&self, id: Uuid) -> u32 {
        hound::WavReader::open(self.dir(id).join("audio.wav"))
            .unwrap()
            .duration()
    }
}

#[tokio::test]
async fn upload_mode_changed_during_a_take_applies_at_stop() {
    let rig = start_rig(
        quiet_config(UploadMode::Automatic),
        settings_for(UploadMode::Automatic),
    )
    .await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    // What SettingsService does when the user picks "Keep on Wave only".
    rig.settings.send_modify(|s| s.upload_mode = UploadMode::LocalOnly);
    rig.stop().await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
    assert!(rig.meeting.borrow().pending_decision.is_none());
}

#[tokio::test]
async fn metadata_records_the_live_audio_mode() {
    // The boot config says Omni; the user switched to Focused since.
    let rig = start_rig(
        quiet_config(UploadMode::LocalOnly),
        settings_for(UploadMode::LocalOnly),
    )
    .await;
    rig.audio.send_modify(|a| a.mode = AudioMode::Cardioid);
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    assert_eq!(rig.metadata(id).recording.audio_mode, "Cardioid");
}

#[tokio::test]
async fn pre_roll_follows_the_setting() {
    let rig = start_rig(
        quiet_config(UploadMode::LocalOnly),
        settings_for(UploadMode::LocalOnly),
    )
    .await;

    // On (the default): 50 idle frames (0.5 s) become the pre-roll.
    rig.feed(50, 0.5).await;
    let first = rig.start().await;
    rig.feed(20, 0.25).await;
    rig.stop().await;
    assert_eq!(rig.metadata(first).recording.pre_roll_duration_ms, 500);
    assert_eq!(rig.wav_samples(first), 70 * 480);

    // Off right before Record: the buffer is dropped and stays empty.
    rig.feed(50, 0.5).await;
    rig.settings.send_modify(|s| s.pre_recording = false);
    rig.feed(10, 0.5).await;
    let second = rig.start().await;
    rig.feed(20, 0.25).await;
    rig.stop().await;
    assert_eq!(rig.metadata(second).recording.pre_roll_duration_ms, 0);
    assert_eq!(rig.wav_samples(second), 20 * 480);
}

/// The setting change and Record are both ready when the service looks, and
/// `select!` picks at random. Run it enough times that either order shows up.
#[tokio::test]
async fn pre_roll_off_then_record_back_to_back_never_splices_old_audio() {
    let rig = start_rig(
        quiet_config(UploadMode::LocalOnly),
        settings_for(UploadMode::LocalOnly),
    )
    .await;
    for round in 0..20 {
        rig.settings.send_modify(|s| s.pre_recording = true);
        rig.feed(50, 0.5).await;
        // No wait between the two: both are pending at the same time.
        rig.settings.send_modify(|s| s.pre_recording = false);
        rig.send(Command::StartRecording);
        rig.wait_for("recording", |s| s.recording).await;
        let id = rig.meeting.borrow().meeting_id.expect("recording has an id");
        rig.feed(5, 0.25).await;
        rig.stop().await;
        assert_eq!(
            rig.metadata(id).recording.pre_roll_duration_ms,
            0,
            "round {round}"
        );
        assert_eq!(rig.wav_samples(id), 5 * 480, "round {round}");
    }
}
