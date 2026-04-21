use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use uuid::Uuid;

// --- Continuous State Types (watch channels) ---

#[derive(Clone, Debug, Default)]
pub struct AudioState {
    pub mode: AudioMode,
    pub mic_muted: bool,
    pub speaker_muted: bool,
    pub speaker_volume: u16,
    pub mic_volume: u16,
    pub doa_horizontal: f32,
    pub doa_pitch: f32,
    pub vu_level: [f32; 8],
    pub nr_level: NrLevel,
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
    /// UI-only: navigate to the System Settings screen (pspapp page 0xe / `ui_setting_system`).
    OpenSystemSettings,
    /// UI-only: navigate to the Speaker Volume screen (pspapp page 0xd / `ui_volume_screen`).
    OpenVolume,
    /// Test hook: force the beamformer to steer at a fixed (azimuth_deg, elevation_deg)
    /// instead of tracking the live DOA estimate. `None` clears the override.
    /// Used by `scripts/test-beamforming.sh` Test 1 to measure front-to-back ratio
    /// against a known fixed axis.
    SetDoaOverride(Option<(f32, f32)>),
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
        assert_eq!(state.nr_level, NrLevel::Off);
    }

    #[test]
    fn command_set_nr_level_exists() {
        let cmd = Command::SetNrLevel(NrLevel::High);
        assert!(matches!(cmd, Command::SetNrLevel(NrLevel::High)));
    }
}
