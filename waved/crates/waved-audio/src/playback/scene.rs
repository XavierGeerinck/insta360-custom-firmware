//! Speaker scenes: what the speaker is set up for, chosen the way stock does.
//!
//! Stock (`audio_server_select_out_path_policy`, libins_entry.so 0xc847c)
//! switches the amplifier's `Set EQ MODE` between MUSIC (2) and MEETING (1)
//! on whether a microphone consumer is active; its speaker path runs no DSP
//! (`docs/AUDIO_HARDWARE.md` "Stock speaker tuning"). waved does the same and
//! also picks the playback preset per scene:
//!
//! | Scene | When | Preset | Amp `Set EQ MODE` |
//! |---|---|---|---|
//! | Music | nothing reads the microphone | Media (identity, stock's path) | 2 MUSIC |
//! | Meeting | the host reads the USB mic, or a Bluetooth call | Voice | 1 MEETING |
//!
//! Manual pins (`Command::SetPlaybackPreset`, `Command::SetAmpEqMode(Some)`)
//! hold until `Command::SetPlaybackPresetAuto` / `SetAmpEqMode(None)`.

use std::sync::Arc;

use tokio::sync::{broadcast, watch};
use tracing::{info, warn};
use waved_state::{AmpEqMode, AudioState, BluetoothState, Command, PlaybackPreset};

use super::DspMetrics;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpeakerScene {
    #[default]
    Music,
    Meeting,
}

impl SpeakerScene {
    pub fn as_str(self) -> &'static str {
        match self {
            SpeakerScene::Music => "music",
            SpeakerScene::Meeting => "meeting",
        }
    }
}

/// What the scene depends on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SceneInputs {
    /// The host reads the USB microphone (`AudioState::host_listening`).
    pub host_listening: bool,
    /// A Bluetooth call: phone audio both ways (`BluetoothState::call_audio`).
    pub bt_call_audio: bool,
}

impl SceneInputs {
    pub fn from_states(audio: &AudioState, bt: &BluetoothState) -> Self {
        Self { host_listening: audio.host_listening, bt_call_audio: bt.call_audio }
    }
}

/// Meeting while something reads the microphone, as stock's
/// `is_cur_mic_work`. Phone music over Bluetooth (`phone_audio` without
/// `call_audio`) and USB playback without a capture stream are Music. Stock
/// also counts local recording/ASR and the pre-record setting; waved does
/// not (pre-record would pin the speaker to meeting all day).
pub fn scene_for(inputs: SceneInputs) -> SpeakerScene {
    if inputs.host_listening || inputs.bt_call_audio {
        SpeakerScene::Meeting
    } else {
        SpeakerScene::Music
    }
}

pub fn preset_for(scene: SpeakerScene) -> PlaybackPreset {
    match scene {
        SpeakerScene::Music => PlaybackPreset::Media,
        SpeakerScene::Meeting => PlaybackPreset::Voice,
    }
}

/// Stock's rule before 1.2.2.6 and for its TEA algorithm (this unit logged
/// it): MEETING while the mic works, MUSIC otherwise. 1.2.2.6 with the SK
/// algorithm keeps MUSIC in calls too; pin `/tmp/waved-amp-eq-mode` 2 to hear it.
pub fn amp_mode_for(scene: SpeakerScene) -> AmpEqMode {
    match scene {
        SpeakerScene::Music => AmpEqMode::Music,
        SpeakerScene::Meeting => AmpEqMode::Meeting,
    }
}

/// The speaker amplifier's `Set EQ MODE` control.
pub trait AmpEqControl: Send {
    fn set_eq_mode(&mut self, mode: AmpEqMode) -> Result<(), String>;
}

/// Where the chosen preset goes (`PlaybackService`).
pub trait PresetSink: Send + Sync {
    fn set_preset(&self, preset: PlaybackPreset);
}

impl<T: PresetSink + ?Sized> PresetSink for Arc<T> {
    fn set_preset(&self, preset: PlaybackPreset) {
        (**self).set_preset(preset)
    }
}

/// What to do now; `None` = nothing to change.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Actions {
    pub preset: Option<PlaybackPreset>,
    pub amp: Option<AmpEqMode>,
}

/// The scene, the pins and what has been applied: decides what to change.
/// No I/O, so the whole policy is tested on the host.
#[derive(Debug, Default)]
pub struct SceneController {
    scene: SpeakerScene,
    preset_pin: Option<PlaybackPreset>,
    amp_pin: Option<AmpEqMode>,
    applied_preset: Option<PlaybackPreset>,
    written_amp: Option<AmpEqMode>,
    /// A mode whose write failed: not retried until the wanted mode changes.
    failed_amp: Option<AmpEqMode>,
}

impl SceneController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scene(&self) -> SpeakerScene {
        self.scene
    }

    /// The preset follows the scene (no pin).
    pub fn preset_auto(&self) -> bool {
        self.preset_pin.is_none()
    }

    /// The amp mode follows the scene (no pin).
    pub fn amp_auto(&self) -> bool {
        self.amp_pin.is_none()
    }

    /// The amp mode last written successfully.
    pub fn written_amp(&self) -> Option<AmpEqMode> {
        self.written_amp
    }

    pub fn wanted_preset(&self) -> PlaybackPreset {
        self.preset_pin.unwrap_or(preset_for(self.scene))
    }

    pub fn wanted_amp(&self) -> AmpEqMode {
        self.amp_pin.unwrap_or(amp_mode_for(self.scene))
    }

    pub fn set_inputs(&mut self, inputs: SceneInputs) {
        self.scene = scene_for(inputs);
    }

    /// Pins and releases. Returns true when `cmd` was one of them.
    pub fn on_command(&mut self, cmd: &Command) -> bool {
        match cmd {
            Command::SetPlaybackPreset(p) => self.preset_pin = Some(*p),
            Command::SetPlaybackPresetAuto => self.preset_pin = None,
            Command::SetAmpEqMode(mode) => {
                self.amp_pin = *mode;
                // An explicit request retries a failed write.
                self.failed_amp = None;
            }
            _ => return false,
        }
        true
    }

    pub fn actions(&self) -> Actions {
        let preset = self.wanted_preset();
        let amp = self.wanted_amp();
        Actions {
            preset: (self.applied_preset != Some(preset)).then_some(preset),
            amp: (self.written_amp != Some(amp) && self.failed_amp != Some(amp)).then_some(amp),
        }
    }

    pub fn preset_applied(&mut self, preset: PlaybackPreset) {
        self.applied_preset = Some(preset);
    }

    pub fn amp_written(&mut self, mode: AmpEqMode, ok: bool) {
        if ok {
            self.written_amp = Some(mode);
            self.failed_amp = None;
        } else {
            // The amp's state is unknown now.
            self.written_amp = None;
            self.failed_amp = Some(mode);
        }
    }
}

/// Apply what `ctl` asks for and publish the result in `metrics`.
fn apply<S: PresetSink, A: AmpEqControl>(
    ctl: &mut SceneController,
    sink: &S,
    amp: &mut A,
    metrics: &DspMetrics,
) {
    let actions = ctl.actions();
    if let Some(preset) = actions.preset {
        info!(
            "speaker scene {} → preset {preset:?}{}",
            ctl.scene().as_str(),
            if ctl.preset_auto() { "" } else { " (pinned)" }
        );
        sink.set_preset(preset);
        ctl.preset_applied(preset);
    }
    if let Some(mode) = actions.amp {
        match amp.set_eq_mode(mode) {
            Ok(()) => {
                info!(
                    "speaker scene {} → amp Set EQ MODE {}{}",
                    ctl.scene().as_str(),
                    mode.value(),
                    if ctl.amp_auto() { "" } else { " (pinned)" }
                );
                ctl.amp_written(mode, true);
            }
            Err(e) => {
                warn!("speaker amp: Set EQ MODE {} failed: {e}", mode.value());
                ctl.amp_written(mode, false);
            }
        }
    }
    metrics.set_scene(ctl.scene(), ctl.preset_auto());
    metrics.set_amp_eq_mode(ctl.written_amp(), ctl.amp_auto());
}

/// Follow the scene: on start and on every change of the inputs or the
/// override commands, apply the preset and the amp mode. Ends when the state
/// or command channels close.
pub async fn run_scene_follower<S: PresetSink, A: AmpEqControl>(
    mut audio_rx: watch::Receiver<AudioState>,
    mut bt_rx: watch::Receiver<BluetoothState>,
    mut cmd_rx: broadcast::Receiver<Command>,
    sink: S,
    mut amp: A,
    metrics: Arc<DspMetrics>,
) {
    let mut ctl = SceneController::new();
    loop {
        let inputs = SceneInputs::from_states(&audio_rx.borrow_and_update(), &bt_rx.borrow_and_update());
        ctl.set_inputs(inputs);
        apply(&mut ctl, &sink, &mut amp, &metrics);
        tokio::select! {
            changed = audio_rx.changed() => if changed.is_err() { return },
            changed = bt_rx.changed() => if changed.is_err() { return },
            cmd = cmd_rx.recv() => match cmd {
                Ok(cmd) => { ctl.on_command(&cmd); }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}

/// `Set EQ MODE` on an ALSA card (`hw:0` on the Wave), through the control
/// interface the way stock's `Insta360_OutputDeviceBase::setCtrlValue` does.
#[cfg(feature = "capture-alsa")]
pub struct AlsaAmpEqControl {
    card: String,
}

#[cfg(feature = "capture-alsa")]
impl AlsaAmpEqControl {
    pub const CONTROL: &'static str = "Set EQ MODE";

    pub fn new(card: impl Into<String>) -> Self {
        Self { card: card.into() }
    }
}

#[cfg(feature = "capture-alsa")]
impl AmpEqControl for AlsaAmpEqControl {
    fn set_eq_mode(&mut self, mode: AmpEqMode) -> Result<(), String> {
        use alsa::ctl::{ElemId, ElemIface};
        let hctl = alsa::HCtl::new(&self.card, false).map_err(|e| format!("{}: {e}", self.card))?;
        hctl.load().map_err(|e| format!("{}: load: {e}", self.card))?;
        let mut id = ElemId::new(ElemIface::Mixer);
        let name = std::ffi::CString::new(Self::CONTROL).expect("no NUL in the control name");
        id.set_name(&name);
        let elem = hctl
            .find_elem(&id)
            .ok_or_else(|| format!("{}: no control '{}'", self.card, Self::CONTROL))?;
        let count = elem.info().map_err(|e| format!("info: {e}"))?.get_count();
        let mut value = elem.read().map_err(|e| format!("read: {e}"))?;
        for index in 0..count {
            value
                .set_integer(index, mode.value())
                .ok_or_else(|| format!("'{}' is not an integer control", Self::CONTROL))?;
        }
        elem.write(&value).map(|_| ()).map_err(|e| format!("write: {e}"))
    }
}

/// Stand-in where ALSA is not built in: logs and succeeds.
pub struct NoAmpEqControl;

impl AmpEqControl for NoAmpEqControl {
    fn set_eq_mode(&mut self, mode: AmpEqMode) -> Result<(), String> {
        info!("speaker amp: no ALSA, Set EQ MODE {} not written", mode.value());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    #[test]
    fn scene_follows_the_microphone_consumers() {
        let s = |host_listening, bt_call_audio| scene_for(SceneInputs { host_listening, bt_call_audio });
        assert_eq!(s(false, false), SpeakerScene::Music);
        assert_eq!(s(true, false), SpeakerScene::Meeting);
        assert_eq!(s(false, true), SpeakerScene::Meeting);
        assert_eq!(s(true, true), SpeakerScene::Meeting);
    }

    #[test]
    fn bluetooth_music_is_music_and_a_bluetooth_call_is_a_meeting() {
        let audio = AudioState::default();
        let music = BluetoothState { phone_audio: true, call_audio: false, ..Default::default() };
        let call = BluetoothState { phone_audio: true, call_audio: true, ..Default::default() };
        assert_eq!(scene_for(SceneInputs::from_states(&audio, &music)), SpeakerScene::Music);
        assert_eq!(scene_for(SceneInputs::from_states(&audio, &call)), SpeakerScene::Meeting);
        let listening = AudioState { host_listening: true, ..Default::default() };
        let idle = BluetoothState::default();
        assert_eq!(scene_for(SceneInputs::from_states(&listening, &idle)), SpeakerScene::Meeting);
    }

    #[test]
    fn scenes_map_to_presets_and_amp_modes() {
        assert_eq!(preset_for(SpeakerScene::Music), PlaybackPreset::Media);
        assert_eq!(preset_for(SpeakerScene::Meeting), PlaybackPreset::Voice);
        assert_eq!(amp_mode_for(SpeakerScene::Music), AmpEqMode::Music);
        assert_eq!(amp_mode_for(SpeakerScene::Meeting), AmpEqMode::Meeting);
    }

    fn applied(ctl: &mut SceneController) -> Actions {
        let a = ctl.actions();
        if let Some(p) = a.preset {
            ctl.preset_applied(p);
        }
        if let Some(m) = a.amp {
            ctl.amp_written(m, true);
        }
        a
    }

    #[test]
    fn controller_applies_the_start_scene_once() {
        let mut ctl = SceneController::new();
        ctl.set_inputs(SceneInputs::default());
        assert_eq!(
            applied(&mut ctl),
            Actions { preset: Some(PlaybackPreset::Media), amp: Some(AmpEqMode::Music) }
        );
        assert_eq!(applied(&mut ctl), Actions::default(), "nothing changed, nothing to do");
    }

    #[test]
    fn controller_switches_with_the_scene() {
        let mut ctl = SceneController::new();
        applied(&mut ctl);
        ctl.set_inputs(SceneInputs { bt_call_audio: true, ..Default::default() });
        assert_eq!(
            applied(&mut ctl),
            Actions { preset: Some(PlaybackPreset::Voice), amp: Some(AmpEqMode::Meeting) }
        );
        ctl.set_inputs(SceneInputs::default());
        assert_eq!(
            applied(&mut ctl),
            Actions { preset: Some(PlaybackPreset::Media), amp: Some(AmpEqMode::Music) }
        );
    }

    #[test]
    fn a_preset_pin_holds_until_auto() {
        let mut ctl = SceneController::new();
        applied(&mut ctl);
        assert!(ctl.on_command(&Command::SetPlaybackPreset(PlaybackPreset::Voice)));
        assert!(!ctl.preset_auto());
        assert_eq!(applied(&mut ctl).preset, Some(PlaybackPreset::Voice));
        // A call and its end do not move the pinned preset; the amp still follows.
        ctl.set_inputs(SceneInputs { host_listening: true, ..Default::default() });
        assert_eq!(applied(&mut ctl), Actions { preset: None, amp: Some(AmpEqMode::Meeting) });
        ctl.set_inputs(SceneInputs::default());
        assert_eq!(applied(&mut ctl), Actions { preset: None, amp: Some(AmpEqMode::Music) });
        assert!(ctl.on_command(&Command::SetPlaybackPresetAuto));
        assert!(ctl.preset_auto());
        assert_eq!(applied(&mut ctl).preset, Some(PlaybackPreset::Media));
    }

    #[test]
    fn an_amp_pin_holds_until_released() {
        let mut ctl = SceneController::new();
        applied(&mut ctl);
        ctl.on_command(&Command::SetAmpEqMode(Some(AmpEqMode::Bypass)));
        assert!(!ctl.amp_auto());
        assert_eq!(applied(&mut ctl).amp, Some(AmpEqMode::Bypass));
        ctl.set_inputs(SceneInputs { bt_call_audio: true, ..Default::default() });
        assert_eq!(applied(&mut ctl), Actions { preset: Some(PlaybackPreset::Voice), amp: None });
        ctl.on_command(&Command::SetAmpEqMode(None));
        assert_eq!(applied(&mut ctl).amp, Some(AmpEqMode::Meeting));
    }

    #[test]
    fn a_failed_amp_write_is_not_retried_until_the_wanted_mode_changes() {
        let mut ctl = SceneController::new();
        let a = ctl.actions();
        ctl.preset_applied(a.preset.unwrap());
        ctl.amp_written(a.amp.unwrap(), false);
        assert_eq!(ctl.written_amp(), None);
        assert_eq!(ctl.actions(), Actions::default(), "no retry storm on every state change");
        ctl.set_inputs(SceneInputs { bt_call_audio: true, ..Default::default() });
        assert_eq!(ctl.actions().amp, Some(AmpEqMode::Meeting));
        ctl.amp_written(AmpEqMode::Meeting, false);
        // An explicit request retries.
        ctl.on_command(&Command::SetAmpEqMode(None));
        assert_eq!(ctl.actions().amp, Some(AmpEqMode::Meeting));
    }

    #[test]
    fn other_commands_are_ignored() {
        let mut ctl = SceneController::new();
        assert!(!ctl.on_command(&Command::ToggleMicMute));
    }

    #[derive(Clone, Default)]
    struct FakeSink(Arc<Mutex<Vec<PlaybackPreset>>>);
    impl PresetSink for FakeSink {
        fn set_preset(&self, preset: PlaybackPreset) {
            self.0.lock().unwrap().push(preset);
        }
    }

    #[derive(Clone, Default)]
    struct FakeAmp {
        writes: Arc<Mutex<Vec<AmpEqMode>>>,
        fail: bool,
    }
    impl AmpEqControl for FakeAmp {
        fn set_eq_mode(&mut self, mode: AmpEqMode) -> Result<(), String> {
            self.writes.lock().unwrap().push(mode);
            if self.fail { Err("no such control".into()) } else { Ok(()) }
        }
    }

    async fn until(what: &str, mut check: impl FnMut() -> bool) {
        for _ in 0..200 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test]
    async fn follower_applies_scenes_overrides_and_publishes_them() {
        let (audio_tx, audio_rx) = watch::channel(AudioState::default());
        let (bt_tx, bt_rx) = watch::channel(BluetoothState::default());
        let (cmd_tx, cmd_rx) = broadcast::channel(16);
        let sink = FakeSink::default();
        let amp = FakeAmp::default();
        let metrics = Arc::new(DspMetrics::default());
        let task = tokio::spawn(run_scene_follower(
            audio_rx, bt_rx, cmd_rx, sink.clone(), amp.clone(), Arc::clone(&metrics),
        ));

        until("start scene", || sink.0.lock().unwrap().len() == 1).await;
        assert_eq!(*sink.0.lock().unwrap(), [PlaybackPreset::Media]);
        assert_eq!(*amp.writes.lock().unwrap(), [AmpEqMode::Music]);
        assert_eq!(metrics.scene(), Some(SpeakerScene::Music));
        assert!(metrics.scene_auto());
        assert_eq!(metrics.amp_eq_mode(), Some(AmpEqMode::Music));

        // Phone music: still music, nothing written again.
        bt_tx.send_modify(|b| b.phone_audio = true);
        // A Bluetooth call.
        bt_tx.send_modify(|b| b.call_audio = true);
        until("meeting", || sink.0.lock().unwrap().len() == 2).await;
        assert_eq!(sink.0.lock().unwrap()[1], PlaybackPreset::Voice);
        assert_eq!(amp.writes.lock().unwrap()[1], AmpEqMode::Meeting);
        until("meeting metrics", || metrics.scene() == Some(SpeakerScene::Meeting)).await;

        // Frequent audio state updates that do not change the scene do nothing.
        for level in 0..20 {
            audio_tx.send_modify(|a| a.vu_level[0] = level as f32);
        }
        // Pin media and the bypass amp mode for an A/B.
        cmd_tx.send(Command::SetPlaybackPreset(PlaybackPreset::Media)).unwrap();
        cmd_tx.send(Command::SetAmpEqMode(Some(AmpEqMode::Bypass))).unwrap();
        until("pins", || amp.writes.lock().unwrap().len() == 3).await;
        assert_eq!(*sink.0.lock().unwrap(), [PlaybackPreset::Media, PlaybackPreset::Voice, PlaybackPreset::Media]);
        assert_eq!(amp.writes.lock().unwrap()[2], AmpEqMode::Bypass);
        until("pin metrics", || !metrics.scene_auto() && !metrics.amp_eq_mode_auto()).await;

        // The call ends; the pins hold. Release both.
        bt_tx.send_modify(|b| b.call_audio = false);
        until("call ended", || metrics.scene() == Some(SpeakerScene::Music)).await;
        assert_eq!(amp.writes.lock().unwrap().len(), 3, "the amp pin holds");
        cmd_tx.send(Command::SetPlaybackPresetAuto).unwrap();
        cmd_tx.send(Command::SetAmpEqMode(None)).unwrap();
        until("released", || amp.writes.lock().unwrap().len() == 4).await;
        assert_eq!(amp.writes.lock().unwrap()[3], AmpEqMode::Music);
        assert_eq!(sink.0.lock().unwrap().len(), 3, "media was already running");
        until("auto metrics", || metrics.scene_auto() && metrics.amp_eq_mode_auto()).await;

        drop(audio_tx);
        tokio::time::timeout(Duration::from_secs(1), task).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn follower_reports_an_unwritable_amp() {
        let (_audio_tx, audio_rx) = watch::channel(AudioState::default());
        let (_bt_tx, bt_rx) = watch::channel(BluetoothState::default());
        let (_cmd_tx, cmd_rx) = broadcast::channel(16);
        let amp = FakeAmp { fail: true, ..Default::default() };
        let metrics = Arc::new(DspMetrics::default());
        tokio::spawn(run_scene_follower(
            audio_rx, bt_rx, cmd_rx, FakeSink::default(), amp.clone(), Arc::clone(&metrics),
        ));
        until("first write", || amp.writes.lock().unwrap().len() == 1).await;
        until("scene published", || metrics.scene().is_some()).await;
        assert_eq!(metrics.amp_eq_mode(), None);
    }
}
