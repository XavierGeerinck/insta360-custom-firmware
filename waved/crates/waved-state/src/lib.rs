use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use uuid::Uuid;

// --- Continuous State Types (watch channels) ---

/// Number of log-spaced frequency bands the audio service publishes for
/// the recording-screen pill. 32 covers 100 Hz – 8 kHz at speech-friendly
/// resolution and stays well below the FFT bin count (512 from a 1024-pt
/// real FFT).
pub const FREQ_BAND_COUNT: usize = 32;

#[derive(Clone, Debug)]
pub struct AudioState {
    pub mode: AudioMode,
    pub mic_muted: bool,
    pub speaker_muted: bool,
    pub speaker_volume: u16,
    /// Linear-percent mic gain. 100 = unity (0 dB). Range 0..=400.
    /// Default 100 so the DSP chain is transparent unless explicitly
    /// configured. Production startup seeds this from TOML (Task 3).
    pub mic_volume: u16,
    pub doa_horizontal: f32,
    pub doa_pitch: f32,
    /// True when the DOA VAD gate is open (speech energy detected). The
    /// arrow may still display a stale `doa_horizontal` on frames where
    /// the solver fails to converge — `doa_active` reflects VAD only.
    pub doa_active: bool,
    pub vu_level: [f32; 8],
    pub incoming_vu_level: [f32; 8],
    /// Magnitude spectrum of the mic signal binned into log-spaced
    /// frequency bands. Each value is normalised to ~[0, 1] so the UI
    /// can pulse a bar per band without rescaling.
    pub vu_freq_bands: [f32; FREQ_BAND_COUNT],
    /// Same shape, but for the reference / far-end channel.
    pub incoming_freq_bands: [f32; FREQ_BAND_COUNT],
    pub nr_level: NrLevel,
    pub playback_preset: PlaybackPreset,  // SP#5
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            mode: AudioMode::default(),
            mic_muted: false,
            speaker_muted: false,
            speaker_volume: 0,
            mic_volume: 100,
            doa_horizontal: 0.0,
            doa_pitch: 0.0,
            doa_active: false,
            vu_level: [0.0; 8],
            incoming_vu_level: [0.0; 8],
            vu_freq_bands: [0.0; FREQ_BAND_COUNT],
            incoming_freq_bands: [0.0; FREQ_BAND_COUNT],
            nr_level: NrLevel::default(),
            playback_preset: PlaybackPreset::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AudioMode {
    #[default]
    Omni,
    Cardioid,
    Supercardioid,
    Figure8,
    Stereo,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NrLevel {
    #[default]
    Off,
    Low,
    Med,
    High,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackPreset {
    #[default]
    Voice,
    Media,
}

#[derive(Clone, Debug, Default)]
pub struct BatteryState {
    pub level: u8,
    pub voltage: u16,
    pub temperature: i8,
    pub is_charging: bool,
}

#[derive(Clone, Debug, Default)]
pub struct BluetoothState {
    pub connected: bool,
    pub device_name: String,
    pub mac: String,
    pub bt_on: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WifiState {
    pub connected: bool,
    pub ssid: String,
    pub ip_addr: String,
    pub signal_strength: i8,
}

#[derive(Clone, Debug, Default)]
pub struct MeetingState {
    pub recording: bool,
    pub paused: bool,
    pub meeting_id: Option<Uuid>,
    pub duration_secs: u32,
    pub pending_uploads: u32,
    pub failed_uploads: u32,
    /// `Some(...)` while the recording-screen Stop has fired but the user
    /// has not yet chosen Save / Send / Discard. UI uses presence of this
    /// field to switch to the ConfirmationScreen, and clears it via one of
    /// the three new `Command` variants.
    pub pending_decision: Option<PendingDecision>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PendingDecision {
    pub id: Uuid,
    pub audio_path: PathBuf,
    pub duration_secs: u32,
    pub size_bytes: u64,
    /// Snapshot of `config.api.endpoint` at stop time.
    pub endpoint: String,
}

#[derive(Clone, Debug, Default)]
pub struct McuState {
    pub motor_position: MotorPosition,
    pub usb_connected: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum MotorPosition {
    #[default]
    Low,
    Medium,
    High,
    Moving,
}

#[derive(Clone, Debug, Default)]
pub struct PucState {
    pub connected: bool,
    pub auto_framing: bool,
}

// --- Discrete Event Types (broadcast channel) ---

#[derive(Clone, Debug)]
pub enum DeviceEvent {
    ButtonPress(ButtonType),
    UsbConnected,
    UsbDisconnected,
    BtConnected(String),
    BtDisconnected,
    WifiConnected(String),
    WifiDisconnected,
    RecordingStarted(Uuid),
    RecordingStopped(Uuid),
    LowBattery(u8),
    MotorObstruction,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ButtonType {
    MuteToggle,
    VolumeUp,
    VolumeDown,
    MuteLongPress,
}

// --- Command Types (UI/external → modules) ---

#[derive(Clone, Debug)]
pub enum Command {
    SetAudioMode(AudioMode),
    SetNrLevel(NrLevel),
    SetPlaybackPreset(PlaybackPreset),
    ToggleMicMute,
    ToggleSpeakerMute,
    SetSpeakerVolume(u16),
    SetMicVolume(u16),
    StartRecording,
    StopRecording,
    PauseRecording,
    ResumeRecording,
    AddBookmark,
    RetryUpload(Uuid),
    SetMotorPosition(MotorPosition),
    Shutdown,
    /// UI-only: navigate to the "More" screen (pspapp page 0xb / `ui_setting_screen`).
    OpenSettings,
    /// UI-only: navigate back one level (from AudioMode → More, More → Home/Recording,
    /// System → More).
    CloseSettings,
    /// UI-only: show CC overlay on the Audio Mode sub-page.
    OpenAudioMode,
    /// UI-only: navigate to the Noise Reduction screen (SP#4 — DFN3 strength selector).
    OpenNoiseReduction,
    /// UI-only: navigate to the System Settings screen (pspapp page 0xe / `ui_setting_system`).
    OpenSystemSettings,
    /// UI-only: navigate to the Speaker Volume screen (pspapp page 0xd / `ui_volume_screen`).
    OpenVolume,
    /// Test hook: force the beamformer to steer at a fixed (azimuth_deg, elevation_deg)
    /// instead of tracking the live DOA estimate. `None` clears the override.
    /// Used by `scripts/test-beamforming.sh` Test 1 to measure front-to-back ratio
    /// against a known fixed axis.
    SetDoaOverride(Option<(f32, f32)>),
    /// User chose Save on the confirmation screen — keep the file locally,
    /// mark `upload_status='local_only'`, do not upload.
    SaveRecording(Uuid),
    /// User chose Send on the confirmation screen — keep the file locally
    /// and queue an upload through the existing retry-with-backoff path.
    SendRecording(Uuid),
    /// User chose Discard on the confirmation screen — hard-delete the
    /// recording dir, the meeting row, and any bookmarks for it.
    DiscardRecording(Uuid),
}

// --- State Bus ---
// Errata #1: watch::Sender is not Clone, so wrap in Arc.

pub struct StateBus {
    pub audio: Arc<watch::Sender<AudioState>>,
    pub battery: Arc<watch::Sender<BatteryState>>,
    pub bt: Arc<watch::Sender<BluetoothState>>,
    pub wifi: Arc<watch::Sender<WifiState>>,
    pub meeting: Arc<watch::Sender<MeetingState>>,
    pub mcu: Arc<watch::Sender<McuState>>,
    pub puc: Arc<watch::Sender<PucState>>,
    pub events: broadcast::Sender<DeviceEvent>,
    pub commands: broadcast::Sender<Command>,
}

impl StateBus {
    pub fn new() -> Self {
        let (audio_tx, _) = watch::channel(AudioState::default());
        let (battery_tx, _) = watch::channel(BatteryState::default());
        let (bt_tx, _) = watch::channel(BluetoothState::default());
        let (wifi_tx, _) = watch::channel(WifiState::default());
        let (meeting_tx, _) = watch::channel(MeetingState::default());
        let (mcu_tx, _) = watch::channel(McuState::default());
        let (puc_tx, _) = watch::channel(PucState::default());
        let (events_tx, _) = broadcast::channel(64);
        let (commands_tx, _) = broadcast::channel(64);

        Self {
            audio: Arc::new(audio_tx),
            battery: Arc::new(battery_tx),
            bt: Arc::new(bt_tx),
            wifi: Arc::new(wifi_tx),
            meeting: Arc::new(meeting_tx),
            mcu: Arc::new(mcu_tx),
            puc: Arc::new(puc_tx),
            events: events_tx,
            commands: commands_tx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_bus_creates_with_defaults() {
        let bus = StateBus::new();
        let audio = bus.audio.subscribe();
        assert!(!audio.borrow().mic_muted);
        assert_eq!(audio.borrow().mode, AudioMode::Omni);
    }

    #[test]
    fn watch_channel_sees_latest_value() {
        let bus = StateBus::new();
        let rx = bus.battery.subscribe();

        bus.battery.send_modify(|s| {
            s.level = 85;
            s.is_charging = true;
        });

        assert_eq!(rx.borrow().level, 85);
        assert!(rx.borrow().is_charging);
    }

    #[tokio::test]
    async fn broadcast_multiple_subscribers() {
        let bus = StateBus::new();
        let mut rx1 = bus.events.subscribe();
        let mut rx2 = bus.events.subscribe();

        let _ = bus.events.send(DeviceEvent::UsbConnected);

        match rx1.recv().await.unwrap() {
            DeviceEvent::UsbConnected => {}
            other => panic!("unexpected event: {:?}", other),
        }
        match rx2.recv().await.unwrap() {
            DeviceEvent::UsbConnected => {}
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn motor_position_default_is_low() {
        assert_eq!(MotorPosition::default(), MotorPosition::Low);
    }

    #[test]
    fn audio_mode_default_is_omni() {
        assert_eq!(AudioMode::default(), AudioMode::Omni);
    }

    #[test]
    fn audio_state_has_vu_level() {
        let state = AudioState::default();
        assert_eq!(state.vu_level, [0.0f32; 8]);
        assert_eq!(state.incoming_vu_level, [0.0f32; 8]);
        assert_eq!(state.nr_level, NrLevel::Off);
    }

    #[test]
    fn command_set_nr_level_exists() {
        let cmd = Command::SetNrLevel(NrLevel::High);
        assert!(matches!(cmd, Command::SetNrLevel(NrLevel::High)));
    }

    #[test]
    fn meeting_state_default_has_no_pending_decision() {
        let s = MeetingState::default();
        assert!(s.pending_decision.is_none());
    }

    #[test]
    fn save_send_discard_commands_carry_uuid() {
        let id = Uuid::new_v4();
        let _save = Command::SaveRecording(id);
        let _send = Command::SendRecording(id);
        let _discard = Command::DiscardRecording(id);
    }

    #[test]
    fn playback_preset_default_is_voice() {
        let s = AudioState::default();
        assert_eq!(s.playback_preset, PlaybackPreset::Voice);
    }

    #[test]
    fn set_playback_preset_command_routes() {
        let cmd = Command::SetPlaybackPreset(PlaybackPreset::Media);
        assert!(matches!(cmd, Command::SetPlaybackPreset(PlaybackPreset::Media)));
    }
}
