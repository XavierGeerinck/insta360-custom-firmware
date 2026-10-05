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
    assert_eq!(rig.wav_samples(id), 200 * 480);
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
    assert_eq!(rig.wav_samples(first), 20 * 480);
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
    assert_eq!(rig.wav_samples(second), 20 * 480);
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
        assert_eq!(rig.wav_samples(id), 10 * 480, "round {round}");
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
    assert_eq!(rig.wav_samples(id), 10 * 480);
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
    assert!(rig.dir(id).join("audio.wav").exists());
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
    assert_eq!(rig.wav_samples(take), 35 * 480);
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

/// A Send that lands right after Discard finds no question to answer, so a
/// discarded take is never queued, even while its removal still waits for
/// the database (the test holds the write lock).
#[tokio::test]
async fn send_right_after_discard_never_queues_it() {
    let rig = start_rig(with_server(quiet_config(UploadMode::Ask)), settings_for(UploadMode::Ask)).await;
    let id = rig.start().await;
    rig.feed(5, 0.1).await;
    rig.send(Command::StopRecording);
    rig.wait_for("pending", |s| s.pending_decision.is_some()).await;

    let blocker = rig.pool().await;
    let mut lock = blocker.acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *lock).await.unwrap();
    rig.send(Command::DiscardRecording(id));
    rig.send(Command::SendRecording(id));
    rig.wait_for("answered", |s| s.pending_decision.is_none() && s.ask_deadline_secs.is_none())
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Read through the lock holder: opening a new pool writes the schema.
    let (status,): (String,) = sqlx::query_as("SELECT upload_status FROM meetings WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    assert_eq!(status, "awaiting_decision", "the stray Send must not queue it");
    sqlx::query("ROLLBACK").execute(&mut *lock).await.unwrap();
    drop(lock);

    wait_gone(&rig.dir(id)).await;
    rig.wait_status(id, None).await;
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
