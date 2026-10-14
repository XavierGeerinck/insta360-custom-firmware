//! Service-level tests for the milestone 2 recording backend. Each test boots
//! a real MeetingService on a temp SQLite file and drives it over its
//! channels, the same way main.rs wires it.

use super::*;
use std::time::Duration;
use waved_audio::integrity::{IntegritySignal, RecordingIntegrity};
use waved_codec::AudioFormat;
use waved_state::{
    AudioMode, AudioState, FilesState, RecordingFormat, SettingsState, UploadMode,
    UsbStoragePhase, UsbStorageState, WifiState,
};

pub(crate) fn quiet_config(upload_mode: UploadMode) -> WavedConfig {
    let mut config = WavedConfig::default();
    config.api.upload_mode = upload_mode;
    config.storage.prune_uploaded_at_percent = 100;
    config.storage.minimum_free_mb = 0;
    // Pre-recording is off by default since 2026-10-06; these tests exercise
    // the pre-roll, so they turn it on (tests that need it off say so).
    config.recording.pre_recording = true;
    config
}

pub(crate) fn settings_for(upload_mode: UploadMode) -> SettingsState {
    SettingsState {
        upload_mode,
        pre_recording: true,
        ..SettingsState::default()
    }
}

/// One 10 ms processed frame (480 samples) holding `value`.
pub(crate) fn frame_of(value: f32) -> ProcessedAudioFrame {
    ProcessedAudioFrame {
        mic_mono_f32: vec![value; 480],
        recording_f32: vec![value; 480],
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
    /// Keeps the service's Wi-Fi watch open; no test toggles Wi-Fi yet.
    #[allow(dead_code)]
    pub wifi: watch::Sender<WifiState>,
    pub integrity: Arc<RecordingIntegrity>,
    pub integrity_tx: mpsc::UnboundedSender<IntegritySignal>,
    pub db_path: PathBuf,
    pub recordings_dir: PathBuf,
    pub files: Arc<watch::Sender<FilesState>>,
    /// USB storage (G1); Off unless a test hands the volume to the computer.
    pub usb_storage: watch::Sender<UsbStorageState>,
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
    start_rig_on(tmp, config, settings, Arc::new(|| true)).await
}

/// As `start_rig_in`; `mounted` says whether the recordings volume is mounted (G1).
pub(crate) async fn start_rig_on(
    tmp: tempfile::TempDir,
    config: WavedConfig,
    settings: SettingsState,
    mounted: VolumeMounted,
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
    let files = Arc::new(watch::channel(FilesState::default()).0);
    let (usb_storage, usb_storage_rx) = watch::channel(UsbStorageState::default());
    let service = MeetingService::new_on_volume(
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
        mounted,
    )
    .await
    .unwrap()
    .with_live_inputs(MeetingLiveInputs {
        settings: settings_rx,
        audio: audio_rx,
    })
    .with_files_publisher(Arc::clone(&files))
    .with_usb_storage(usb_storage_rx);
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
        files,
        usb_storage,
        _tmp: tmp,
    }
}

impl Rig {
    pub fn send(&self, command: Command) {
        self.commands.send(command).unwrap();
    }

    /// Queue `count` frames holding `value` and wait until the service has
    /// taken all of them, so the next command lands after them (commands and
    /// frames arrive on different channels, and `select!` picks among ready
    /// branches at random). A taken frame is fully handled before the service
    /// looks at its next event.
    pub async fn feed(&self, count: usize, value: f32) {
        for _ in 0..count {
            self.frames.send(frame_of(value)).await.unwrap();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while self.frames.capacity() < self.frames.max_capacity() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the service stopped taking frames"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Queue `count` frames holding `value` without yielding, so they are all
    /// waiting in the channel when the command sent next is handled (the test
    /// runtime is single-threaded: the service only runs once the test awaits).
    pub fn queue(&self, count: usize, value: f32) {
        for _ in 0..count {
            self.frames.try_send(frame_of(value)).unwrap();
        }
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

    /// Samples in a take's finalized audio, whatever its format.
    pub fn audio_samples(&self, id: Uuid) -> u32 {
        waved_codec::open_audio(&self.dir(id))
            .unwrap()
            .total_samples() as u32
    }

    pub fn audio_format(&self, id: Uuid) -> Option<AudioFormat> {
        waved_codec::find_audio(&self.dir(id)).map(|(format, _)| format)
    }
}

#[tokio::test]
async fn each_take_has_the_format_set_when_it_started() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let first = rig.start().await;
    rig.feed(20, 0.1).await;
    // Changed during the take: the take keeps its format.
    rig.settings.send_modify(|s| s.recording_format = RecordingFormat::Flac);
    rig.feed(20, 0.1).await;
    rig.stop().await;
    assert_eq!(rig.audio_format(first), Some(AudioFormat::Opus), "the default");
    assert_eq!(rig.audio_samples(first), 40 * 480);
    let metadata = rig.metadata(first);
    assert_eq!(metadata.audio.codec, "opus");
    assert_eq!(metadata.audio.filename, format!("{first}.opus"));
    assert_eq!(metadata.audio.bitrate_bps, Some(32_000));
    assert_eq!(metadata.recording.audio_duration_ms, 400);

    let second = rig.start().await;
    rig.feed(10, 0.1).await;
    rig.stop().await;
    assert_eq!(rig.audio_format(second), Some(AudioFormat::Flac), "the next take");
    assert_eq!(rig.audio_samples(second), 10 * 480);
    assert_eq!(rig.metadata(second).audio.content_type, "audio/flac");
    assert_eq!(rig.status(second).await.as_deref(), Some("local_only"));
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

    // On (as the rig sets it): 50 idle frames (0.5 s) become the pre-roll.
    rig.feed(50, 0.5).await;
    let first = rig.start().await;
    rig.feed(20, 0.25).await;
    rig.stop().await;
    assert_eq!(rig.metadata(first).recording.pre_roll_duration_ms, 500);
    assert_eq!(rig.audio_samples(first), 70 * 480);

    // Off right before Record: the buffer is dropped and stays empty.
    rig.feed(50, 0.5).await;
    rig.settings.send_modify(|s| s.pre_recording = false);
    rig.feed(10, 0.5).await;
    let second = rig.start().await;
    rig.feed(20, 0.25).await;
    rig.stop().await;
    assert_eq!(rig.metadata(second).recording.pre_roll_duration_ms, 0);
    assert_eq!(rig.audio_samples(second), 20 * 480);
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
        assert_eq!(rig.audio_samples(id), 5 * 480, "round {round}");
    }
}

#[test]
fn pre_recording_indicator_rule() {
    let healthy = AudioState::default();
    assert!(pre_recording_active(true, false, &healthy));
    assert!(!pre_recording_active(false, false, &healthy), "setting off");
    assert!(!pre_recording_active(true, true, &healthy), "recording");
    let muted = AudioState {
        mic_muted: true,
        ..AudioState::default()
    };
    assert!(!pre_recording_active(true, false, &muted), "muted");
    let dead = AudioState {
        capture_healthy: false,
        ..AudioState::default()
    };
    assert!(!pre_recording_active(true, false, &dead), "no audio arriving");
}

#[tokio::test]
async fn pre_recording_dot_follows_setting_mute_and_recording() {
    let rig = start_rig(
        quiet_config(UploadMode::LocalOnly),
        settings_for(UploadMode::LocalOnly),
    )
    .await;
    rig.wait_for("dot on at idle", |s| s.pre_recording_active).await;
    rig.audio.send_modify(|a| a.mic_muted = true);
    rig.wait_for("dot off while muted", |s| !s.pre_recording_active).await;
    rig.audio.send_modify(|a| a.mic_muted = false);
    rig.wait_for("dot back after unmute", |s| s.pre_recording_active).await;
    rig.start().await;
    rig.wait_for("dot off while recording", |s| !s.pre_recording_active).await;
    rig.stop().await;
    rig.wait_for("dot back after stop", |s| s.pre_recording_active).await;
    rig.settings.send_modify(|s| s.pre_recording = false);
    rig.wait_for("dot off with the setting", |s| !s.pre_recording_active).await;
}

fn no_pre_roll(upload_mode: UploadMode) -> SettingsState {
    SettingsState {
        pre_recording: false,
        ..settings_for(upload_mode)
    }
}

#[tokio::test]
async fn pause_and_resume_cut_the_paused_audio_and_freeze_the_timer() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(100, 0.25).await; // 1 s
    rig.send(Command::PauseRecording);
    rig.wait_for("paused", |s| s.paused).await;
    rig.feed(100, 0.5).await; // talked over the pause: dropped
    tokio::time::sleep(Duration::from_millis(1_100)).await; // a tick passes
    assert_eq!(rig.meeting.borrow().duration_secs, 1);
    rig.send(Command::ResumeRecording);
    rig.wait_for("resumed", |s| !s.paused).await;
    rig.feed(100, 0.75).await;
    rig.stop().await;
    assert_eq!(rig.audio_samples(id), 200 * 480);
    let pauses = rig.metadata(id).recording.pauses;
    assert_eq!(pauses.len(), 1);
    assert_eq!(pauses[0].offset_ms, 1_000);
    assert!(pauses[0].duration_ms >= 1_100, "{pauses:?}");
}

#[tokio::test]
async fn a_dropped_frame_while_paused_keeps_the_take() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(10, 0.25).await;
    rig.send(Command::PauseRecording);
    rig.wait_for("paused", |s| s.paused).await;
    rig.integrity.record_fault(RecordingIntegrityFault::ProcessedFrameDropped);
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    rig.integrity_tx
        .send(IntegritySignal::with_acknowledgement(
            RecordingIntegrityFault::ProcessedFrameDropped,
            ack_tx,
        ))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), ack_rx).await.unwrap().unwrap();
    assert!(rig.meeting.borrow().recording, "nothing was being written; the take survives");
    rig.send(Command::ResumeRecording);
    rig.wait_for("resumed", |s| !s.paused).await;
    rig.feed(10, 0.25).await;
    rig.stop().await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
}

#[tokio::test]
async fn a_capture_failure_while_paused_still_ends_the_take() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.send(Command::PauseRecording);
    rig.wait_for("paused", |s| s.paused).await;
    rig.integrity.record_fault(RecordingIntegrityFault::CaptureEnded);
    rig.integrity_tx
        .send(IntegritySignal::new(RecordingIntegrityFault::CaptureEnded))
        .unwrap();
    rig.wait_for("take ended", |s| !s.recording).await;
    rig.wait_status(id, Some("artifact_error")).await;
}

/// A dying capture often reports an xrun just before it ends. Both faults are
/// recorded before the service reads the first signal: tolerating the xrun
/// must not swallow the capture failure behind it.
#[tokio::test]
async fn an_xrun_then_a_capture_failure_while_paused_ends_the_take() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.send(Command::PauseRecording);
    rig.wait_for("paused", |s| s.paused).await;
    rig.integrity.record_fault(RecordingIntegrityFault::CaptureXrun);
    rig.integrity.record_fault(RecordingIntegrityFault::CaptureEnded);
    rig.integrity_tx
        .send(IntegritySignal::new(RecordingIntegrityFault::CaptureXrun))
        .unwrap();
    rig.integrity_tx
        .send(IntegritySignal::new(RecordingIntegrityFault::CaptureEnded))
        .unwrap();
    rig.wait_for("take ended", |s| !s.recording).await;
    rig.wait_status(id, Some("artifact_error")).await;
}

/// A frame dropped while recording lost audio from the file. Pausing before
/// the service reads that signal must not turn it into a harmless pause fault.
#[tokio::test]
async fn a_drop_before_the_pause_still_ends_the_take() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(10, 0.25).await;
    rig.integrity.record_fault(RecordingIntegrityFault::ProcessedFrameDropped);
    rig.send(Command::PauseRecording);
    // Commands are handled in order, so once the bookmark row exists the
    // Pause before it has been handled.
    rig.send(Command::AddBookmark);
    let pool = rig.pool().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while db::get_bookmarks(&pool, &id).await.unwrap().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "bookmark never landed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!rig.meeting.borrow().paused, "a pending fault refuses the pause");
    rig.integrity_tx
        .send(IntegritySignal::new(RecordingIntegrityFault::ProcessedFrameDropped))
        .unwrap();
    rig.wait_for("take ended", |s| !s.recording).await;
    rig.wait_status(id, Some("artifact_error")).await;
}

/// All samples of a take, decoded. Compare values only on lossless takes.
fn audio_values(rig: &Rig, id: Uuid) -> Vec<i16> {
    let mut reader = waved_codec::open_audio(&rig.dir(id)).unwrap();
    waved_codec::read_to_end(reader.as_mut()).unwrap()
}

/// Frames still queued when Resume is handled were captured during the
/// pause: none reach the file. A frame drop from during the pause that is
/// still queued at Resume is a pause fault and keeps the take. Everything
/// is queued before the service runs, so Resume, the frames and the signal
/// are all ready at once; ten rounds make the old orders show up.
#[tokio::test]
async fn frames_and_drops_queued_at_resume_stay_in_the_pause() {
    // FLAC: lossless, so a value from a pause would show.
    let settings = SettingsState {
        recording_format: RecordingFormat::Flac,
        ..no_pre_roll(UploadMode::LocalOnly)
    };
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings).await;
    let id = rig.start().await;
    rig.feed(10, 0.25).await;
    for round in 0..10 {
        rig.send(Command::PauseRecording);
        rig.wait_for("paused", |s| s.paused).await;
        rig.queue(20, 0.5);
        rig.integrity.record_fault(RecordingIntegrityFault::ProcessedFrameDropped);
        rig.integrity_tx
            .send(IntegritySignal::new(RecordingIntegrityFault::ProcessedFrameDropped))
            .unwrap();
        rig.send(Command::ResumeRecording);
        rig.wait_for("resumed", |s| !s.paused).await;
        assert!(rig.meeting.borrow().recording, "round {round}: the take survives");
        rig.feed(10, 0.25).await;
    }
    rig.stop().await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
    let values = audio_values(&rig, id);
    assert_eq!(values.len(), 110 * 480, "only the frames from outside the pauses");
    assert!(
        values.iter().all(|&v| v == values[0]),
        "no audio from a pause in the file"
    );
}

/// A dead capture whose signal is still queued at Resume ends the take
/// there, before it writes again.
#[tokio::test]
async fn a_capture_failure_queued_at_resume_ends_the_take() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(10, 0.25).await;
    rig.send(Command::PauseRecording);
    rig.wait_for("paused", |s| s.paused).await;
    rig.queue(5, 0.5);
    rig.integrity.record_fault(RecordingIntegrityFault::CaptureEnded);
    rig.integrity_tx
        .send(IntegritySignal::new(RecordingIntegrityFault::CaptureEnded))
        .unwrap();
    rig.send(Command::ResumeRecording);
    rig.wait_for("take ended", |s| !s.recording).await;
    rig.wait_status(id, Some("artifact_error")).await;
}

/// Frames still queued when Pause is handled were captured before it: they
/// reach the file, so the last words before a pause are kept.
#[tokio::test]
async fn frames_queued_at_pause_are_written() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    for _ in 0..10 {
        rig.queue(20, 0.25);
        rig.send(Command::PauseRecording);
        rig.wait_for("paused", |s| s.paused).await;
        rig.send(Command::ResumeRecording);
        rig.wait_for("resumed", |s| !s.paused).await;
    }
    rig.stop().await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
    assert_eq!(rig.audio_samples(id), 200 * 480);
    assert_eq!(rig.metadata(id).recording.pauses.len(), 10);
}

/// PauseRecording and ResumeRecording are idempotent (lane A ruling).
#[tokio::test]
async fn repeated_pause_and_stray_resume_are_no_ops() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;

    // Resume without a pause leaves the take untouched.
    let first = rig.start().await;
    rig.feed(10, 0.25).await;
    rig.send(Command::ResumeRecording);
    rig.feed(10, 0.25).await;
    assert!(!rig.meeting.borrow().paused);
    rig.stop().await;
    assert_eq!(rig.audio_samples(first), 20 * 480);
    assert!(rig.metadata(first).recording.pauses.is_empty());
    assert_eq!(rig.status(first).await.as_deref(), Some("local_only"));

    // Resume while idle does nothing either.
    rig.send(Command::ResumeRecording);
    rig.send(Command::PauseRecording);

    // Pause, Pause, Resume: one closed pause span, and the take finalizes.
    let second = rig.start().await;
    assert!(!rig.meeting.borrow().paused, "an idle Pause is not carried into the take");
    rig.feed(10, 0.25).await;
    rig.send(Command::PauseRecording);
    rig.wait_for("paused", |s| s.paused).await;
    rig.send(Command::PauseRecording);
    rig.send(Command::ResumeRecording);
    rig.wait_for("resumed", |s| !s.paused).await;
    rig.feed(10, 0.25).await;
    rig.stop().await;
    assert_eq!(rig.audio_samples(second), 20 * 480);
    let pauses = rig.metadata(second).recording.pauses;
    assert_eq!(pauses.len(), 1, "{pauses:?}");
    assert_eq!(pauses[0].offset_ms, 100);
    assert_eq!(rig.status(second).await.as_deref(), Some("local_only"));
}

/// Pause, feed `talked_over` frames into the pause and leave it open long
/// enough that the span has a duration.
async fn pause_and_talk_over(rig: &Rig, talked_over: usize) {
    rig.send(Command::PauseRecording);
    rig.wait_for("paused", |s| s.paused).await;
    rig.feed(talked_over, 0.5).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
}

/// Stop while still paused, with a frame-drop signal queued and no ack
/// awaited. Stop and the signal are both ready when the service looks, so
/// either the run loop or `drain_before_stop` handles the signal; 20 rounds
/// make sure the drain path shows up (each round takes it with p = 1/2).
#[tokio::test]
async fn stop_while_paused_with_a_queued_drop_keeps_the_take() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    for round in 0..20 {
        let id = rig.start().await;
        rig.feed(10, 0.25).await;
        pause_and_talk_over(&rig, 10).await;
        rig.integrity.record_fault(RecordingIntegrityFault::ProcessedFrameDropped);
        rig.integrity_tx
            .send(IntegritySignal::new(RecordingIntegrityFault::ProcessedFrameDropped))
            .unwrap();
        rig.stop().await;
        assert_eq!(rig.status(id).await.as_deref(), Some("local_only"), "round {round}");
        assert_eq!(rig.audio_samples(id), 10 * 480, "round {round}");
        let pauses = rig.metadata(id).recording.pauses;
        assert_eq!(pauses.len(), 1, "round {round}: {pauses:?}");
        assert_eq!(pauses[0].offset_ms, 100, "round {round}");
        assert!(pauses[0].duration_ms > 0, "round {round}: {pauses:?}");
    }
}

/// A producer records its fault before it sends the signal. Stop can land in
/// between: `drain_before_stop` finds the epoch moved with no signal queued
/// and falls back to the last fault, which a paused take tolerates for a drop.
#[tokio::test]
async fn stop_while_paused_after_an_unsignalled_drop_keeps_the_take() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(10, 0.25).await;
    pause_and_talk_over(&rig, 10).await;
    rig.integrity.record_fault(RecordingIntegrityFault::ProcessedFrameDropped);
    rig.stop().await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
    assert_eq!(rig.audio_samples(id), 10 * 480);
    let pauses = rig.metadata(id).recording.pauses;
    assert_eq!(pauses.len(), 1, "{pauses:?}");
    assert!(pauses[0].duration_ms > 0, "{pauses:?}");
}

/// The same two Stop races with a dead capture pipeline still end the take.
#[tokio::test]
async fn stop_while_paused_with_a_capture_failure_ends_the_take() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    for _ in 0..10 {
        let id = rig.start().await;
        rig.feed(10, 0.25).await;
        pause_and_talk_over(&rig, 10).await;
        rig.integrity.record_fault(RecordingIntegrityFault::CaptureEnded);
        rig.integrity_tx
            .send(IntegritySignal::new(RecordingIntegrityFault::CaptureEnded))
            .unwrap();
        rig.send(Command::StopRecording);
        rig.wait_for("take ended", |s| !s.recording).await;
        rig.wait_status(id, Some("artifact_error")).await;
    }
    // Recorded but not yet signalled when Stop lands.
    let id = rig.start().await;
    rig.feed(10, 0.25).await;
    pause_and_talk_over(&rig, 10).await;
    rig.integrity.record_fault(RecordingIntegrityFault::CaptureEnded);
    rig.send(Command::StopRecording);
    rig.wait_for("take ended", |s| !s.recording).await;
    rig.wait_status(id, Some("artifact_error")).await;
}

fn voiced(ms: u64, az: f32) -> DoaFrame {
    DoaFrame {
        frame_idx: 0,
        timestamp_ms: ms,
        azimuth_deg: az,
        elevation_deg: 0.0,
        confidence: 0.8,
        vad: true,
        backend: waved_audio::doa::DoaBackend::GccPhat,
    }
}

#[tokio::test]
async fn marks_count_per_take_and_ignore_paused_marks() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    rig.start().await;
    rig.feed(10, 0.1).await;
    rig.send(Command::AddBookmark);
    rig.wait_for("one mark", |s| s.marks == 1).await;
    rig.send(Command::PauseRecording);
    rig.wait_for("paused", |s| s.paused).await;
    rig.send(Command::AddBookmark);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.meeting.borrow().marks, 1, "Mark is dimmed while paused");
    rig.send(Command::ResumeRecording);
    rig.wait_for("resumed", |s| !s.paused).await;
    rig.stop().await;
    rig.start().await;
    assert_eq!(rig.meeting.borrow().marks, 0, "a new take starts at zero");
}

#[tokio::test]
async fn current_speaker_follows_the_talker_and_clears_at_stop() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    rig.start().await;
    for i in 0..12u64 {
        rig.doa.send(voiced(i * 21, 45.0)).unwrap();
    }
    rig.wait_for("speaker 1", |s| s.current_speaker == Some(1)).await;
    rig.stop().await;
    assert_eq!(rig.meeting.borrow().current_speaker, None);
}

#[tokio::test]
async fn the_pending_decision_counts_marks_and_speakers() {
    let rig = start_rig(quiet_config(UploadMode::Ask), settings_for(UploadMode::Ask)).await;
    rig.start().await;
    rig.feed(10, 0.1).await;
    rig.send(Command::AddBookmark);
    rig.wait_for("one mark", |s| s.marks == 1).await;
    for i in 0..12u64 {
        rig.doa.send(voiced(i * 21, 45.0)).unwrap();
    }
    // A closing silence makes the segment durable, which tells us the
    // service has taken every voiced frame.
    rig.doa
        .send(DoaFrame::silent(0, 1_000, waved_audio::doa::DoaBackend::GccPhat))
        .unwrap();
    let id = rig.meeting.borrow().meeting_id.unwrap();
    let pool = rig.pool().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while db::get_speaker_segments(&pool, &id).await.unwrap().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "no speaker segment was saved");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    rig.send(Command::StopRecording);
    rig.wait_for("a decision to make", |s| s.pending_decision.is_some()).await;
    let decision = rig.meeting.borrow().pending_decision.clone().unwrap();
    assert_eq!(decision.marks, 1);
    assert_eq!(decision.speakers, 1);
}

#[test]
fn seconds_left_rounds_up_and_stops_at_zero() {
    let now = tokio::time::Instant::now();
    assert_eq!(seconds_left(now + Duration::from_millis(59_001), now), 60);
    assert_eq!(seconds_left(now + Duration::from_secs(60), now), 60);
    assert_eq!(seconds_left(now + Duration::from_millis(1), now), 1);
    assert_eq!(seconds_left(now, now), 0);
    assert_eq!(seconds_left(now, now + Duration::from_secs(5)), 0);
}

fn ask_after(seconds: u32) -> SettingsState {
    SettingsState {
        ask_timeout_s: seconds,
        ..settings_for(UploadMode::Ask)
    }
}

/// A webhook server is configured. Wi-Fi stays down in the rig, so a queued
/// take stays queued and nothing is ever dialled.
fn with_server(mut config: WavedConfig) -> WavedConfig {
    config.api.endpoint = "http://127.0.0.1:9/hook".into();
    config
}

#[tokio::test]
async fn unanswered_decision_is_kept_after_the_timeout() {
    let rig = start_rig(quiet_config(UploadMode::Ask), ask_after(1)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    assert_eq!(rig.meeting.borrow().ask_deadline_secs, Some(1));
    rig.wait_for("kept", |s| s.pending_decision.is_none()).await;
    assert_eq!(rig.meeting.borrow().ask_deadline_secs, None);
    rig.wait_status(id, Some("local_only")).await;
}

#[tokio::test]
async fn answer_before_timeout_cancels_auto_keep() {
    let rig = start_rig(with_server(quiet_config(UploadMode::Ask)), ask_after(2)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    rig.send(Command::SendRecording(id));
    rig.wait_for("answered", |s| s.pending_decision.is_none()).await;
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("queued"), "Send must win");
    assert_eq!(rig.meeting.borrow().ask_deadline_secs, None);
}

/// The countdown steps once a second, and an answer given while it shows its
/// last second still wins over the auto-Keep.
#[tokio::test]
async fn an_answer_in_the_last_second_still_wins() {
    let rig = start_rig(with_server(quiet_config(UploadMode::Ask)), ask_after(2)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    assert_eq!(rig.meeting.borrow().ask_deadline_secs, Some(2));
    rig.wait_for("last second", |s| s.ask_deadline_secs == Some(1)).await;
    rig.send(Command::SendRecording(id));
    rig.wait_for("answered", |s| s.pending_decision.is_none()).await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("queued"), "Send must win");
    assert_eq!(rig.meeting.borrow().ask_deadline_secs, None);
}

#[tokio::test]
async fn a_decision_left_by_a_restart_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let id = Uuid::new_v4();
    let dir = tmp.path().join("rec").join(id.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("audio.wav"), b"RIFF").unwrap();
    std::fs::write(dir.join("metadata.json"), b"{}").unwrap();
    let pool = db::init_pool(tmp.path().join("t.db").to_str().unwrap()).await.unwrap();
    db::insert_meeting(&pool, &id, "Omni", &dir.to_string_lossy()).await.unwrap();
    db::finish_meeting(&pool, &id, 3, MeetingStatus::AwaitingDecision).await.unwrap();
    pool.close().await;

    let rig = start_rig_in(tmp, quiet_config(UploadMode::Ask), settings_for(UploadMode::Ask)).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
    assert!(rig.meeting.borrow().pending_decision.is_none());
}

/// With a server, Automatic queues the take for the worker (unchanged).
#[tokio::test]
async fn automatic_with_a_server_queues_the_take() {
    let rig = start_rig(
        with_server(quiet_config(UploadMode::Automatic)),
        settings_for(UploadMode::Automatic),
    )
    .await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    rig.wait_for("one upload waiting", |s| s.pending_uploads == 1).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("queued"));
    assert!(rig.meeting.borrow().pending_decision.is_none());
}

/// No server configured: Automatic has nowhere to send the take, so it is
/// kept on Wave instead of looking like it is on its way.
#[tokio::test]
async fn automatic_without_a_server_keeps_the_take_on_wave() {
    let rig = start_rig(
        quiet_config(UploadMode::Automatic),
        settings_for(UploadMode::Automatic),
    )
    .await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    rig.wait_status(id, Some("local_only")).await;
    let meeting = rig.meeting.borrow().clone();
    assert!(meeting.pending_decision.is_none());
    assert_eq!(meeting.ask_deadline_secs, None);
    assert_eq!(meeting.pending_uploads, 0);
}

/// No server configured: Ask still asks, but Send keeps the take on Wave.
#[tokio::test]
async fn send_without_a_server_keeps_the_take_on_wave() {
    let rig = start_rig(quiet_config(UploadMode::Ask), settings_for(UploadMode::Ask)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("awaiting_decision"));
    rig.send(Command::SendRecording(id));
    rig.wait_for("answered", |s| s.pending_decision.is_none()).await;
    rig.wait_status(id, Some("local_only")).await;
    let meeting = rig.meeting.borrow().clone();
    assert_eq!(meeting.ask_deadline_secs, None);
    assert_eq!(meeting.pending_uploads, 0);
}

async fn wait_gone(path: &std::path::Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while path.exists() {
        assert!(tokio::time::Instant::now() < deadline, "{} still exists", path.display());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn delete_removes_a_kept_recording() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::AddBookmark);
    rig.wait_for("mark", |s| s.marks == 1).await;
    rig.stop().await;
    rig.send(Command::DeleteRecording(id));
    wait_gone(&rig.dir(id)).await;
    rig.wait_status(id, None).await;
    let pool = rig.pool().await;
    let (bookmarks,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM bookmarks WHERE meeting_id = ?")
        .bind(id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(bookmarks, 0);
    let leftovers: Vec<_> = std::fs::read_dir(&rig.recordings_dir).unwrap().collect();
    assert!(leftovers.is_empty(), "no .deleted-* directory left behind");
}

#[tokio::test]
async fn delete_is_refused_while_uploading() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    db::set_status(&rig.pool().await, &id, MeetingStatus::Uploading).await.unwrap();
    rig.send(Command::DeleteRecording(id));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rig.audio_format(id).is_some());
    assert_eq!(rig.status(id).await.as_deref(), Some("uploading"));
}

#[tokio::test]
async fn delete_of_the_pending_recording_discards_it() {
    let rig = start_rig(quiet_config(UploadMode::Ask), settings_for(UploadMode::Ask)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    rig.send(Command::DeleteRecording(id));
    rig.wait_for("cleared", |s| s.pending_decision.is_none() && s.ask_deadline_secs.is_none())
        .await;
    wait_gone(&rig.dir(id)).await;
}

#[tokio::test]
async fn delete_of_the_running_recording_is_ignored() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.send(Command::DeleteRecording(id));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rig.meeting.borrow().recording);
    assert!(rig.dir(id).exists());
}

#[tokio::test]
async fn interrupted_delete_finishes_on_boot() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = tmp.path().join("rec");
    let pool = db::init_pool(tmp.path().join("t.db").to_str().unwrap()).await.unwrap();
    // Power cut after the rename: the row still says local_only.
    let renamed = Uuid::new_v4();
    let trash = rec.join(format!("{}{renamed}", removal::DELETED_PREFIX));
    std::fs::create_dir_all(&trash).unwrap();
    std::fs::write(trash.join("audio.wav"), b"RIFF").unwrap();
    std::fs::write(trash.join("metadata.json"), b"{}").unwrap();
    db::insert_meeting(&pool, &renamed, "Omni", &rec.join(renamed.to_string()).to_string_lossy())
        .await
        .unwrap();
    db::set_status(&pool, &renamed, MeetingStatus::LocalOnly).await.unwrap();
    // Power cut right after the claim: the directory is intact.
    let claimed = Uuid::new_v4();
    let dir = rec.join(claimed.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("audio.wav"), b"RIFF").unwrap();
    std::fs::write(dir.join("metadata.json"), b"{}").unwrap();
    db::insert_meeting(&pool, &claimed, "Omni", &dir.to_string_lossy()).await.unwrap();
    db::set_status(&pool, &claimed, MeetingStatus::Deleting).await.unwrap();
    pool.close().await;

    let rig = start_rig_in(tmp, quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    assert!(!trash.exists());
    assert!(!rig.dir(renamed).exists(), "reconciliation must not bring it back");
    assert!(!rig.dir(claimed).exists());
    assert_eq!(rig.status(renamed).await, None);
    assert_eq!(rig.status(claimed).await, None);
}

// Ruling: freeing space through Files must lift the storage block at once.
// statvfs cannot be pushed over a limit from a test, so the block is set
// by hand; the limits in quiet_config are within reach, so the recompute
// after the removal is what clears it. Nothing else recomputes it here: no
// server is configured, so the upload worker never runs.
#[tokio::test]
async fn delete_recomputes_the_storage_block() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    rig.meeting.send_modify(|s| s.storage_blocked = true);
    rig.send(Command::DeleteRecording(id));
    wait_gone(&rig.dir(id)).await;
    rig.wait_for("unblocked", |s| !s.storage_blocked).await;
}

#[tokio::test]
async fn discard_recomputes_the_storage_block() {
    let rig = start_rig(quiet_config(UploadMode::Ask), settings_for(UploadMode::Ask)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    rig.meeting.send_modify(|s| s.storage_blocked = true);
    rig.send(Command::DiscardRecording(id));
    wait_gone(&rig.dir(id)).await;
    rig.wait_for("unblocked", |s| !s.storage_blocked && s.pending_decision.is_none())
        .await;
}

/// A Delete of another recording during a take never stalls the take's frame
/// intake: the producer only `try_send`s, and a full channel ends the take.
/// The test holds the database write lock, so the removal cannot even claim
/// its row while frames are fed; the take must keep draining them anyway.
#[tokio::test]
async fn deleting_another_recording_keeps_the_running_take() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let old = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    let take = rig.start().await;
    rig.feed(10, 0.2).await;

    let blocker = rig.pool().await;
    let mut lock = blocker.acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *lock).await.unwrap();
    rig.send(Command::DeleteRecording(old));
    tokio::time::sleep(Duration::from_millis(100)).await;
    rig.feed(20, 0.3).await;
    assert!(rig.dir(old).exists(), "the removal is still waiting for the lock");
    sqlx::query("ROLLBACK").execute(&mut *lock).await.unwrap();
    drop(lock);

    wait_gone(&rig.dir(old)).await;
    rig.wait_status(old, None).await;
    rig.feed(5, 0.4).await;
    rig.stop().await;
    assert_eq!(rig.status(take).await.as_deref(), Some("local_only"));
    assert_eq!(rig.audio_samples(take), 35 * 480);
}

/// A Delete of the pending take that fails after its claim still answers
/// "Send this recording?", so the auto-Keep never writes over the claim.
/// The rename fails because `.deleted-<id>` already exists and is not empty.
#[tokio::test]
async fn a_failed_delete_of_the_pending_take_answers_the_question() {
    let rig = start_rig(quiet_config(UploadMode::Ask), ask_after(1)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    let trash = rig.recordings_dir.join(format!("{}{id}", removal::DELETED_PREFIX));
    std::fs::create_dir_all(&trash).unwrap();
    std::fs::write(trash.join("in-the-way"), b"x").unwrap();

    rig.send(Command::DeleteRecording(id));
    rig.wait_for("answered", |s| s.pending_decision.is_none() && s.ask_deadline_secs.is_none())
        .await;
    rig.wait_status(id, Some("deleting")).await;
    rig.send(Command::SaveRecording(id));
    tokio::time::sleep(Duration::from_millis(1_500)).await; // past the ask timeout
    assert_eq!(rig.status(id).await.as_deref(), Some("deleting"));
}

/// Discard answers the question at once: a Record tapped right after it
/// starts the next take instead of being refused as "awaiting decision".
#[tokio::test]
async fn start_right_after_discard_starts_the_next_take() {
    let rig = start_rig(quiet_config(UploadMode::Ask), settings_for(UploadMode::Ask)).await;
    let discarded = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    rig.send(Command::DiscardRecording(discarded));
    rig.send(Command::StartRecording);
    rig.wait_for("next take", |s| s.recording).await;
    assert_ne!(rig.meeting.borrow().meeting_id, Some(discarded));
    wait_gone(&rig.dir(discarded)).await;
}

/// A Send that lands right after Discard finds no question to answer and a
/// row already claimed for deletion, so a discarded take is never queued.
/// The removal cannot finish here (`.deleted-<id>` is in the way), so the
/// claim stays for the test to read.
#[tokio::test]
async fn send_right_after_discard_never_queues_it() {
    let rig = start_rig(with_server(quiet_config(UploadMode::Ask)), settings_for(UploadMode::Ask)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    let trash = rig.recordings_dir.join(format!("{}{id}", removal::DELETED_PREFIX));
    std::fs::create_dir_all(&trash).unwrap();
    std::fs::write(trash.join("in-the-way"), b"x").unwrap();

    rig.send(Command::DiscardRecording(id));
    rig.send(Command::SendRecording(id));
    // Commands are handled in order: once the next take runs, Send was handled.
    rig.send(Command::StartRecording);
    rig.wait_for("next take", |s| s.recording).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("deleting"), "the stray Send must not queue it");
}

/// Discard is saved (the row claimed for deletion) before the question is
/// answered: a power cut right after the answer must not bring the take back
/// as kept. The test holds the database write lock for a moment.
#[tokio::test]
async fn discard_is_saved_before_it_is_answered() {
    let rig = start_rig(quiet_config(UploadMode::Ask), settings_for(UploadMode::Ask)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;

    let blocker = rig.pool().await;
    let mut lock = blocker.acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *lock).await.unwrap();
    rig.send(Command::DiscardRecording(id));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        rig.meeting.borrow().pending_decision.is_some(),
        "not answered before the claim is saved"
    );
    sqlx::query("ROLLBACK").execute(&mut *lock).await.unwrap();
    drop(lock);

    rig.wait_for("answered", |s| s.pending_decision.is_none()).await;
    let status = rig.status(id).await;
    assert!(
        matches!(status.as_deref(), None | Some("deleting")),
        "claimed when answered: {status:?}"
    );
    wait_gone(&rig.dir(id)).await;
    rig.wait_status(id, None).await;
}

/// If the claim cannot be saved (the database stays locked past its busy
/// timeout), the files are moved to `.deleted-<id>` before the question is
/// answered: boot finishes such a delete, so the take never comes back as
/// kept. Takes a little over the 5 s busy timeout.
#[tokio::test]
async fn a_discard_whose_claim_fails_is_still_never_kept() {
    let rig = start_rig(quiet_config(UploadMode::Ask), settings_for(UploadMode::Ask)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;

    let blocker = rig.pool().await;
    let mut lock = blocker.acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *lock).await.unwrap();
    rig.send(Command::DiscardRecording(id));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while rig.meeting.borrow().pending_decision.is_some() {
        assert!(tokio::time::Instant::now() < deadline, "the question was never answered");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let trash = rig.recordings_dir.join(format!("{}{id}", removal::DELETED_PREFIX));
    assert!(!rig.dir(id).exists(), "files moved aside before the answer");
    assert!(waved_codec::find_audio(&trash).is_some());
    // Read through the lock holder: the claim never landed.
    let (status,): (String,) = sqlx::query_as("SELECT upload_status FROM meetings WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    assert_eq!(status, "awaiting_decision");
    sqlx::query("ROLLBACK").execute(&mut *lock).await.unwrap();
    drop(lock);

    // What boot does with it: finish the delete before keeping stale decisions.
    let pool = rig.pool().await;
    removal::finish_interrupted(&pool, &rig.recordings_dir).await;
    db::keep_stale_decisions(&pool).await.unwrap();
    assert_eq!(Rig::status_in(&pool, id).await, None);
    assert!(!trash.exists());
}

/// A Discard that loses the race to the auto-Keep (the take was kept a
/// moment before) still deletes the take; one long after the Keep does not.
#[tokio::test]
async fn a_discard_just_after_the_auto_keep_deletes_the_take() {
    let rig = start_rig(quiet_config(UploadMode::Ask), ask_after(1)).await;
    let late = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("kept", |s| s.pending_decision.is_none() && !s.recording).await;
    rig.wait_status(late, Some("local_only")).await;
    tokio::time::sleep(Duration::from_millis(2_300)).await;
    rig.send(Command::DiscardRecording(late));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.status(late).await.as_deref(), Some("local_only"), "too late to count");

    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;
    rig.wait_for("kept", |s| s.pending_decision.is_none()).await;
    rig.send(Command::DiscardRecording(id));
    wait_gone(&rig.dir(id)).await;
    rig.wait_status(id, None).await;
}

/// Deleting a failed take updates the upload counts at once, not at the
/// worker's next 5 s round (no server here, so the worker sends nothing).
#[tokio::test]
async fn deleting_a_failed_take_updates_the_upload_counts() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    db::set_status(&rig.pool().await, &id, MeetingStatus::ConfigurationError).await.unwrap();
    // What the worker publishes for it.
    rig.meeting.send_modify(|s| s.failed_uploads = 1);
    rig.send(Command::DeleteRecording(id));
    rig.wait_status(id, None).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while rig.meeting.borrow().failed_uploads != 0 {
        assert!(tokio::time::Instant::now() < deadline, "failed_uploads still counts the deleted take");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn resend_only_requeues_kept_or_failed_recordings() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = db::init_pool(tmp.path().join("t.db").to_str().unwrap()).await.unwrap();
    use MeetingStatus as M;
    for status in [
        M::LocalOnly,
        M::RetryWait,
        M::ConfigurationError,
        M::Uploaded,
        M::ArtifactError,
        M::Uploading,
        M::AwaitingDecision,
        M::Deleting,
    ] {
        let id = Uuid::new_v4();
        db::insert_meeting(&pool, &id, "Omni", "/nonexistent").await.unwrap();
        db::set_status(&pool, &id, status).await.unwrap();
        let requeued = db::requeue_for_delivery(&pool, &id).await.unwrap();
        let expected = matches!(status, M::LocalOnly | M::RetryWait | M::ConfigurationError);
        assert_eq!(requeued, expected, "{status:?}");
        let want = if expected { "queued" } else { status.as_str() };
        assert_eq!(Rig::status_in(&pool, id).await.as_deref(), Some(want), "{status:?}");
    }
}

#[tokio::test]
async fn send_from_files_queues_a_kept_recording() {
    let rig = start_rig(
        with_server(quiet_config(UploadMode::LocalOnly)),
        settings_for(UploadMode::LocalOnly),
    )
    .await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
    rig.send(Command::SendRecording(id));
    rig.wait_status(id, Some("queued")).await;
}

#[tokio::test]
async fn retry_resets_a_failed_recording_and_ignores_others() {
    let rig = start_rig(
        with_server(quiet_config(UploadMode::LocalOnly)),
        settings_for(UploadMode::LocalOnly),
    )
    .await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    let pool = rig.pool().await;
    db::set_status(&pool, &id, MeetingStatus::Uploaded).await.unwrap();
    rig.send(Command::RetryUpload(id));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("uploaded"), "already delivered");
    db::set_status(&pool, &id, MeetingStatus::RetryWait).await.unwrap();
    rig.send(Command::RetryUpload(id));
    rig.wait_status(id, Some("queued")).await;
}

/// No upload server configured (empty endpoint): Send and Retry on a kept or
/// failed take queue nothing and leave it as it was, never a fake "sending".
#[tokio::test]
async fn send_and_retry_without_a_server_queue_nothing() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
    rig.send(Command::SendRecording(id));
    rig.send(Command::RetryUpload(id));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
    assert_eq!(rig.meeting.borrow().pending_uploads, 0);
    let pool = rig.pool().await;
    db::set_status(&pool, &id, MeetingStatus::RetryWait).await.unwrap();
    rig.send(Command::RetryUpload(id));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("retry_wait"), "a failed take stays failed");
}

async fn wait_files(rig: &Rig, what: &str, predicate: impl Fn(&FilesState) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(7);
    while !predicate(&rig.files.borrow()) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}: {:?}",
            *rig.files.borrow()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn files_list_tracks_takes_marks_and_deletes() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), settings_for(UploadMode::LocalOnly)).await;
    // 0.2 s of pre-roll, so the mark sits 200 ms into the file.
    rig.feed(20, 0.1).await;
    let id = rig.start().await;
    rig.feed(10, 0.1).await;
    rig.send(Command::AddBookmark);
    rig.wait_for("mark", |s| s.marks == 1).await;
    rig.stop().await;
    wait_files(&rig, "one recording", |f| f.recordings.len() == 1).await;
    let files = rig.files.borrow().clone();
    let entry = &files.recordings[0];
    assert_eq!(entry.id, id);
    assert_eq!(entry.status, waved_state::RecordingStatus::LocalOnly);
    assert_eq!(entry.marks, 1);
    assert_eq!(entry.marks_ms.len() as u32, entry.marks);
    assert_eq!(entry.marks_ms.len(), 1);
    assert_eq!(rig.metadata(id).recording.pre_roll_duration_ms, 200);
    assert_eq!(entry.marks_ms, vec![200], "the mark at 0 s of the take, after the pre-roll");
    assert!(entry.size_bytes > 44, "WAV header plus audio");
    assert!(entry.last_error.is_none());
    assert!(files.free_bytes > 0);
    assert!(!files.low_space);
    let asked = tokio::time::Instant::now();
    rig.send(Command::DeleteRecording(id));
    wait_files(&rig, "empty list", |f| f.recordings.is_empty()).await;
    assert!(
        asked.elapsed() < Duration::from_secs(3),
        "the finished removal refreshes Files, not only the 5 s tick"
    );
}

/// Lets a reader blocked on a FIFO go on: opening it read-write never blocks
/// on Linux and counts as the writer the reader waits for.
struct FifoRelease(PathBuf);

impl Drop for FifoRelease {
    fn drop(&mut self) {
        use std::io::Write;
        if let Ok(mut fifo) = std::fs::OpenOptions::new().read(true).write(true).open(&self.0) {
            let _ = fifo.write_all(b"{}");
        }
    }
}

/// Building the Files list never holds up a running take. The listed
/// recording's metadata.json is a FIFO nobody writes to, so reading it
/// blocks until the test ends; the take must keep draining its frames
/// meanwhile (a full frame channel would end it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stuck_files_refresh_never_holds_up_the_take() {
    let rig = start_rig(quiet_config(UploadMode::LocalOnly), no_pre_roll(UploadMode::LocalOnly)).await;
    let take = rig.start().await;
    rig.feed(10, 0.2).await;

    let listed = Uuid::new_v4();
    let dir = rig.dir(listed);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("audio.wav"), b"RIFF").unwrap();
    let fifo = dir.join("metadata.json");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).unwrap();
    let _release = FifoRelease(fifo);
    let pool = rig.pool().await;
    db::insert_meeting(&pool, &listed, "Omni", &dir.to_string_lossy()).await.unwrap();
    db::set_status(&pool, &listed, MeetingStatus::LocalOnly).await.unwrap();
    db::add_bookmark(&pool, &listed, 1).await.unwrap();

    // Changes the recordings, so the list is refreshed right away.
    rig.send(Command::RetryFailedUploads);
    tokio::time::sleep(Duration::from_millis(200)).await;
    rig.feed(50, 0.2).await;
    rig.stop().await;
    assert_eq!(rig.status(take).await.as_deref(), Some("local_only"));
    assert_eq!(rig.audio_samples(take), 60 * 480);
}

#[tokio::test]
async fn no_take_starts_while_usb_storage_has_the_volume() {
    let rig = start_rig(
        quiet_config(UploadMode::LocalOnly),
        settings_for(UploadMode::LocalOnly),
    )
    .await;
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::On);
    rig.send(Command::StartRecording);
    // The test runtime is single-threaded: the service handles the command
    // while this test sleeps.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!rig.meeting.borrow().recording, "the computer has the volume");
    assert_eq!(
        std::fs::read_dir(&rig.recordings_dir).unwrap().count(),
        0,
        "nothing written where the volume was"
    );
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::Off);
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"));
}

#[tokio::test]
async fn a_recording_deleted_on_the_computer_shows_as_missing_once_the_volume_is_back() {
    let rig = start_rig(
        quiet_config(UploadMode::LocalOnly),
        settings_for(UploadMode::LocalOnly),
    )
    .await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::On);
    std::fs::remove_dir_all(rig.dir(id)).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.status(id).await.as_deref(),
        Some("local_only"),
        "nothing is reconciled while the volume is away"
    );
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::Off);
    rig.wait_status(id, Some("missing_artifact")).await;
}

/// The volume probe for `start_rig_on`, and the switch that flips it.
fn volume(mounted: bool) -> (Arc<std::sync::atomic::AtomicBool>, VolumeMounted) {
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(mounted));
    let probe = Arc::clone(&flag);
    (flag, Arc::new(move || probe.load(std::sync::atomic::Ordering::SeqCst)))
}

#[tokio::test]
async fn nothing_is_reconciled_while_the_volume_is_not_mounted() {
    // waved starts with /mnt/udisk unmounted (a mount that failed after USB
    // storage, or the boot still checking it): the bare mount point has no
    // recordings, and each one would be marked missing for good.
    let tmp = tempfile::tempdir().unwrap();
    let rec = tmp.path().join("rec");
    std::fs::create_dir_all(&rec).unwrap();
    let pool = db::init_pool(tmp.path().join("t.db").to_str().unwrap()).await.unwrap();
    let id = Uuid::new_v4();
    db::insert_meeting(&pool, &id, "Omni", &rec.join(id.to_string()).to_string_lossy())
        .await
        .unwrap();
    db::set_status(&pool, &id, MeetingStatus::LocalOnly).await.unwrap();
    pool.close().await;
    let (mounted, probe) = volume(false);

    let rig = start_rig_on(
        tmp,
        quiet_config(UploadMode::LocalOnly),
        settings_for(UploadMode::LocalOnly),
        probe,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"), "not at start-up");

    // USB storage ends, but the volume did not mount: still nothing.
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::On);
    tokio::time::sleep(Duration::from_millis(50)).await;
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::Off);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.status(id).await.as_deref(), Some("local_only"), "not on return");

    // Mounted, and the recording really is gone: now it is missing.
    mounted.store(true, std::sync::atomic::Ordering::SeqCst);
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::On);
    tokio::time::sleep(Duration::from_millis(50)).await;
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::Off);
    rig.wait_status(id, Some("missing_artifact")).await;
}

#[tokio::test]
async fn a_recording_marked_missing_comes_back_with_its_files() {
    let rig = start_rig(
        quiet_config(UploadMode::LocalOnly),
        settings_for(UploadMode::LocalOnly),
    )
    .await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    let aside = rig.recordings_dir.join("aside");
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::On);
    tokio::time::sleep(Duration::from_millis(50)).await;
    std::fs::rename(rig.dir(id), &aside).unwrap();
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::Off);
    rig.wait_status(id, Some("missing_artifact")).await;

    // The computer puts it back.
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::On);
    tokio::time::sleep(Duration::from_millis(50)).await;
    std::fs::rename(&aside, rig.dir(id)).unwrap();
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::Off);
    rig.wait_status(id, Some("local_only")).await;
}

#[tokio::test]
async fn delete_waits_while_usb_storage_has_the_volume() {
    let rig = start_rig(
        quiet_config(UploadMode::LocalOnly),
        settings_for(UploadMode::LocalOnly),
    )
    .await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.stop().await;
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::On);
    rig.send(Command::DeleteRecording(id));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        rig.status(id).await.as_deref(),
        Some("local_only"),
        "a delete now would bring it back once the volume is mounted"
    );
    assert!(rig.dir(id).exists());
    rig.usb_storage.send_modify(|s| s.phase = UsbStoragePhase::Off);
    rig.send(Command::DeleteRecording(id));
    rig.wait_status(id, None).await;
}
