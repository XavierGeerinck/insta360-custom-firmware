use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::AtomicI32;
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
    /// Set true at startup when AudioConfig.doa_backend = SrpPhat but the SRP
    /// estimator failed to initialise — service fell back to GCC-PHAT. The
    /// stats collector mirrors this into /tmp/audio_stats.json so operators
    /// see why their selected backend isn't active.
    pub doa_backend_fallback: bool,
    pub vu_level: [f32; 8],
    pub incoming_vu_level: [f32; 8],
    /// Magnitude spectrum of the mic signal binned into log-spaced
    /// frequency bands. Each value is normalised to ~[0, 1] so the UI
    /// can pulse a bar per band without rescaling.
    pub vu_freq_bands: [f32; FREQ_BAND_COUNT],
    /// Same shape, but for the reference / far-end channel.
    pub incoming_freq_bands: [f32; FREQ_BAND_COUNT],
    pub nr_level: NrLevel,
    pub playback_preset: PlaybackPreset, // SP#5
    /// Host wrote Report 0x05 (Off-hook LED) — `true` while the host
    /// signals "you are in a call". Plumbed by `waved-audio::hid::HidReader`.
    pub call_off_hook: bool,
    /// Host wrote Report 0x07 (Ring LED) — `true` while the host signals
    /// "you have an incoming call".
    pub call_ringing: bool,
    /// Host wrote Report 0x06 (Mute LED) — `true` while the host signals
    /// "the call is muted host-side". Informational only.
    pub call_host_muted: bool,
    /// The host has the USB microphone stream open (UAC "PCM Playback State").
    /// Drives "Computer is using the mic" and the teal ring on Home.
    pub host_listening: bool,
    /// Confidence of the latest DoA estimate, 0..1.
    pub doa_confidence: f32,
    /// Fixed aim for Focused / Narrow / Two sides, degrees clockwise from the
    /// top of the screen (D2).
    pub beam_aim_deg: f32,
    /// False after the live capture/DSP producer terminates unexpectedly.
    /// A watchdog restart creates a fresh healthy state.
    pub capture_healthy: bool,
    /// Recovered ALSA xruns observed since process start.
    pub capture_xruns: u64,
    /// Raw capture frames dropped before DSP since process start.
    pub raw_frames_dropped: u64,
    /// Processed frames dropped before the meeting recorder since process start.
    pub processed_frames_dropped: u64,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            mode: AudioMode::default(),
            mic_muted: false,
            speaker_muted: false,
            // Default unity gain — speaker plays at full digital volume on boot.
            // Pre-fix the playback loop ignored this field; post-fix (SP#6
            // follow-up) it scales the signal directly, so 0 would silence the
            // device until the host or user moves a slider.
            speaker_volume: 100,
            mic_volume: 100,
            doa_horizontal: 0.0,
            doa_pitch: 0.0,
            doa_active: false,
            doa_backend_fallback: false,
            vu_level: [0.0; 8],
            incoming_vu_level: [0.0; 8],
            vu_freq_bands: [0.0; FREQ_BAND_COUNT],
            incoming_freq_bands: [0.0; FREQ_BAND_COUNT],
            nr_level: NrLevel::default(),
            playback_preset: PlaybackPreset::default(),
            call_off_hook: false,
            call_ringing: false,
            call_host_muted: false,
            host_listening: false,
            doa_confidence: 0.0,
            beam_aim_deg: 0.0,
            capture_healthy: true,
            capture_xruns: 0,
            raw_frames_dropped: 0,
            processed_frames_dropped: 0,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordingIntegrityFault {
    CaptureEnded = 1,
    CaptureReadFailed = 2,
    CaptureXrun = 3,
    RawFrameDropped = 4,
    RawFrameChannelClosed = 5,
    ProcessedFrameDropped = 6,
    ProcessedFrameChannelClosed = 7,
    WavWriteFailed = 8,
}

impl RecordingIntegrityFault {
    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::CaptureEnded,
            2 => Self::CaptureReadFailed,
            3 => Self::CaptureXrun,
            4 => Self::RawFrameDropped,
            5 => Self::RawFrameChannelClosed,
            6 => Self::ProcessedFrameDropped,
            7 => Self::ProcessedFrameChannelClosed,
            8 => Self::WavWriteFailed,
            _ => return None,
        })
    }

    pub const fn diagnostic(self) -> &'static str {
        match self {
            Self::CaptureEnded => "audio capture ended unexpectedly",
            Self::CaptureReadFailed => "audio capture read failed",
            Self::CaptureXrun => "audio capture lost frames",
            Self::RawFrameDropped => "raw audio frame dropped",
            Self::RawFrameChannelClosed => "raw audio channel closed",
            Self::ProcessedFrameDropped => "recording audio frame dropped",
            Self::ProcessedFrameChannelClosed => "recording audio channel closed",
            Self::WavWriteFailed => "recording storage write failed",
        }
    }

    pub const fn requires_restart(self) -> bool {
        matches!(
            self,
            Self::CaptureEnded
                | Self::CaptureReadFailed
                | Self::RawFrameChannelClosed
                | Self::ProcessedFrameChannelClosed
        )
    }

    pub const fn blocks_recording(self) -> bool {
        matches!(self, Self::WavWriteFailed)
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
    /// DoA-tracking cardioid. The beam direction follows the live DoA
    /// estimate, smoothed via EWMA on the unit-vector (see beam/steered.rs).
    /// `initial_az_deg` is where the beam points before the first
    /// high-confidence DoA frame arrives (default 0 = forward).
    Steered {
        initial_az_deg: f32,
    },
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

/// What happens to a recording when it stops. Lives here (not in waved-config)
/// so state, commands and config share one type.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadMode {
    #[default]
    Automatic,
    Ask,
    LocalOnly,
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
    pub clock_synchronized: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ProvisioningState {
    /// Not provisioning — normal STA operation.
    #[default]
    Idle,
    /// Scanning for networks before bringing up the AP.
    Scanning,
    /// AP is up; user should join `ssid` and browse to `url`.
    ApActive {
        ssid: String,
        passphrase: String,
        url: String,
    },
    /// Credentials received; reconnecting as STA to `ssid`.
    Connecting { ssid: String },
    /// Validating and atomically persisting setup fields.
    Saving,
    /// Restoring a previously saved station profile.
    Reconnecting,
    /// Station connectivity is restored and services are restarting with the
    /// newly persisted configuration.
    Restarting,
    /// Provisioning finished — connected to `ssid`.
    Connected { ssid: String },
    /// Provisioning failed; `reason` is user-facing.
    Failed { reason: String },
}

#[derive(Clone, Debug, Default)]
pub struct MeetingState {
    pub recording: bool,
    pub paused: bool,
    pub meeting_id: Option<Uuid>,
    pub duration_secs: u32,
    pub pending_uploads: u32,
    pub failed_uploads: u32,
    pub last_upload_success_at: Option<String>,
    pub last_upload_error: Option<String>,
    pub last_webhook_test: Option<String>,
    /// True when storage is over its configured threshold and there are no
    /// delivered recordings that can be pruned safely.
    pub storage_blocked: bool,
    /// Persistent within the current process when recording storage cannot be
    /// trusted. New recording starts are rejected while set.
    pub recording_blocked_reason: Option<RecordingIntegrityFault>,
    /// Last sanitized recording-integrity fault observed in this process.
    pub last_recording_error: Option<RecordingIntegrityFault>,
    /// `Some(...)` while the recording-screen Stop has fired but the user
    /// has not yet chosen Save / Send / Discard. UI uses presence of this
    /// field to switch to the ConfirmationScreen, and clears it via one of
    /// the three new `Command` variants.
    pub pending_decision: Option<PendingDecision>,
    /// Bookmarks in the active recording.
    pub marks: u32,
    /// Per-meeting speaker number currently talking (diarization), if any.
    pub current_speaker: Option<u8>,
    /// Seconds left before an unanswered "Send this recording?" keeps the file (D4).
    pub ask_deadline_secs: Option<u32>,
    /// The pre-recording buffer is filling (setting on, not recording, not muted).
    pub pre_recording_active: bool,
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

/// User-facing settings mirrored from config.toml and persisted by waved-settings.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsState {
    /// Backlight brightness, 5..=100.
    pub brightness_pct: u8,
    /// Seconds without a touch before the resting screen; 0 = never.
    pub dim_after_s: u32,
    /// Seconds without a touch before the screen turns off; 0 = never.
    pub off_after_s: u32,
    /// IANA time zone name.
    pub time_zone: String,
    pub clock_24h: bool,
    pub pre_recording: bool,
    /// Seconds before an unanswered "Send this recording?" keeps the file.
    pub ask_timeout_s: u32,
    pub upload_mode: UploadMode,
    pub tones_enabled: bool,
    pub first_run_done: bool,
    /// A device password is set, so Files, Wi-Fi, Bluetooth, Uploads and System ask for it (D10).
    pub pin_enabled: bool,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            brightness_pct: 80,
            dim_after_s: 120,
            off_after_s: 600,
            time_zone: "UTC".into(),
            clock_24h: true,
            pre_recording: true,
            ask_timeout_s: 60,
            upload_mode: UploadMode::Automatic,
            tones_enabled: true,
            first_run_done: false,
            pin_enabled: false,
        }
    }
}

/// Where a recording stands, as shown in Files.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordingStatus {
    AwaitingDecision,
    LocalOnly,
    Waiting,
    Uploading,
    Uploaded,
    Failed,
    Damaged,
}

impl RecordingStatus {
    /// Sorted to the top of Files and shown in amber.
    pub fn needs_attention(&self) -> bool {
        matches!(self, Self::Failed | Self::Damaged)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordingSummary {
    pub id: Uuid,
    /// RFC 3339 start time, when the clock was synchronized.
    pub started_at: Option<String>,
    pub duration_secs: u32,
    pub size_bytes: u64,
    pub status: RecordingStatus,
    pub marks: u32,
    /// Plain-language reason for Failed / Damaged.
    pub last_error: Option<String>,
}

/// Recordings on the device, newest first, published by waved-meeting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilesState {
    pub recordings: Vec<RecordingSummary>,
    pub free_bytes: u64,
    /// Free space is below the threshold that blocks new recordings.
    pub low_space: bool,
}

/// Local playback of a recording through the speaker.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaybackState {
    pub recording: Option<Uuid>,
    pub playing: bool,
    pub position_ms: u32,
    pub duration_ms: u32,
    /// Bookmark offsets from the start of the file.
    pub marks_ms: Vec<u32>,
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
    RecordingUploadSucceeded(Uuid),
    RecordingUploadDeferred(Uuid),
    RecordingUploadNeedsAttention(Uuid),
    RecordingIntegrityFailed {
        meeting_id: Option<Uuid>,
        fault: RecordingIntegrityFault,
    },
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
    RetryFailedUploads,
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
    /// UI-only: show webhook configuration and delivery status.
    OpenWebhookStatus,
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
    /// UI-only: open the non-disruptive Wi-Fi status/management screen.
    OpenWifiSettings,
    /// UI-only: enter WiFi provisioning (scan → AP → web form).
    OpenWifiProvisioning,
    /// Provisioning web form / WifiService: join this network.
    WifiConnect {
        ssid: String,
        password: String,
    },
    /// UI-only: abort provisioning and revert to STA.
    CancelWifiProvisioning,
    /// Internal lifecycle command emitted after setup has persisted settings,
    /// restored station connectivity, and is ready to reload all services.
    RestartForConfig,
}

// --- State Bus ---
// Errata #1: watch::Sender is not Clone, so wrap in Arc.

pub struct StateBus {
    pub audio: Arc<watch::Sender<AudioState>>,
    pub battery: Arc<watch::Sender<BatteryState>>,
    pub bt: Arc<watch::Sender<BluetoothState>>,
    pub wifi: Arc<watch::Sender<WifiState>>,
    pub provisioning: Arc<watch::Sender<ProvisioningState>>,
    pub meeting: Arc<watch::Sender<MeetingState>>,
    pub mcu: Arc<watch::Sender<McuState>>,
    pub puc: Arc<watch::Sender<PucState>>,
    pub settings: Arc<watch::Sender<SettingsState>>,
    pub files: Arc<watch::Sender<FilesState>>,
    pub playback: Arc<watch::Sender<PlaybackState>>,
    pub events: broadcast::Sender<DeviceEvent>,
    pub commands: broadcast::Sender<Command>,
    /// Echo-suppress marker for Wave→Mac HID volume events. When Mac
    /// changes `speaker_volume` (via UAC2 FU SET_CUR → `uac/control.rs`),
    /// the dispatcher writes the new value here. `spawn_volume_watch`
    /// then sees the `AudioState` change and consumes the marker — it
    /// updates its `last` cursor but skips emitting `ConsumerVolumeStep`
    /// HID events, breaking the feedback loop (Mac F11 → mixer → state →
    /// HID → Mac F11 again). Sentinel `-1` = no pending host echo.
    pub host_pending_speaker_vol: Arc<AtomicI32>,
}

impl Default for StateBus {
    fn default() -> Self {
        Self::new()
    }
}

impl StateBus {
    pub fn new() -> Self {
        let (audio_tx, _) = watch::channel(AudioState::default());
        let (battery_tx, _) = watch::channel(BatteryState::default());
        let (bt_tx, _) = watch::channel(BluetoothState::default());
        let (wifi_tx, _) = watch::channel(WifiState::default());
        let (provisioning_tx, _) = watch::channel(ProvisioningState::default());
        let (meeting_tx, _) = watch::channel(MeetingState::default());
        let (mcu_tx, _) = watch::channel(McuState::default());
        let (puc_tx, _) = watch::channel(PucState::default());
        let (settings_tx, _) = watch::channel(SettingsState::default());
        let (files_tx, _) = watch::channel(FilesState::default());
        let (playback_tx, _) = watch::channel(PlaybackState::default());
        let (events_tx, _) = broadcast::channel(64);
        let (commands_tx, _) = broadcast::channel(64);

        Self {
            audio: Arc::new(audio_tx),
            battery: Arc::new(battery_tx),
            bt: Arc::new(bt_tx),
            wifi: Arc::new(wifi_tx),
            provisioning: Arc::new(provisioning_tx),
            meeting: Arc::new(meeting_tx),
            mcu: Arc::new(mcu_tx),
            puc: Arc::new(puc_tx),
            settings: Arc::new(settings_tx),
            files: Arc::new(files_tx),
            playback: Arc::new(playback_tx),
            events: events_tx,
            commands: commands_tx,
            host_pending_speaker_vol: Arc::new(AtomicI32::new(-1)),
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

    #[tokio::test]
    async fn recording_upload_events_keep_their_meeting_id() {
        let bus = StateBus::new();
        let mut rx = bus.events.subscribe();
        let id = Uuid::new_v4();

        for event in [
            DeviceEvent::RecordingUploadSucceeded(id),
            DeviceEvent::RecordingUploadDeferred(id),
            DeviceEvent::RecordingUploadNeedsAttention(id),
        ] {
            bus.events.send(event).unwrap();
            let received_id = match rx.recv().await.unwrap() {
                DeviceEvent::RecordingUploadSucceeded(received)
                | DeviceEvent::RecordingUploadDeferred(received)
                | DeviceEvent::RecordingUploadNeedsAttention(received) => received,
                other => panic!("unexpected event: {other:?}"),
            };
            assert_eq!(received_id, id);
        }
    }

    #[tokio::test]
    async fn recording_integrity_event_keeps_fault_and_meeting_id() {
        let bus = StateBus::new();
        let mut rx = bus.events.subscribe();
        let id = Uuid::new_v4();
        bus.events
            .send(DeviceEvent::RecordingIntegrityFailed {
                meeting_id: Some(id),
                fault: RecordingIntegrityFault::ProcessedFrameDropped,
            })
            .unwrap();

        assert!(matches!(
            rx.recv().await.unwrap(),
            DeviceEvent::RecordingIntegrityFailed {
                meeting_id: Some(received),
                fault: RecordingIntegrityFault::ProcessedFrameDropped,
            } if received == id
        ));
    }

    #[test]
    fn recording_integrity_fault_codes_and_copy_are_stable() {
        for fault in [
            RecordingIntegrityFault::CaptureEnded,
            RecordingIntegrityFault::CaptureReadFailed,
            RecordingIntegrityFault::CaptureXrun,
            RecordingIntegrityFault::RawFrameDropped,
            RecordingIntegrityFault::RawFrameChannelClosed,
            RecordingIntegrityFault::ProcessedFrameDropped,
            RecordingIntegrityFault::ProcessedFrameChannelClosed,
            RecordingIntegrityFault::WavWriteFailed,
        ] {
            assert_eq!(RecordingIntegrityFault::from_code(fault as u8), Some(fault));
            assert!(fault.diagnostic().is_ascii());
        }
        assert_eq!(RecordingIntegrityFault::from_code(0), None);
        assert!(RecordingIntegrityFault::CaptureEnded.requires_restart());
        assert!(!RecordingIntegrityFault::CaptureXrun.requires_restart());
        assert!(RecordingIntegrityFault::WavWriteFailed.blocks_recording());
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
        assert!(state.capture_healthy);
        assert_eq!(state.capture_xruns, 0);
        assert_eq!(state.raw_frames_dropped, 0);
        assert_eq!(state.processed_frames_dropped, 0);
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
        assert!(matches!(
            cmd,
            Command::SetPlaybackPreset(PlaybackPreset::Media)
        ));
    }

    #[test]
    fn audio_state_seed_helper_writes_mic_volume() {
        let bus = StateBus::new();
        // Simulate the seed call main.rs will make.
        bus.audio.send_modify(|s| s.mic_volume = 250);
        assert_eq!(bus.audio.borrow().mic_volume, 250);
    }

    #[test]
    fn audio_state_default_call_fields_false() {
        let s = AudioState::default();
        assert!(!s.call_off_hook);
        assert!(!s.call_ringing);
        assert!(!s.call_host_muted);
    }

    #[test]
    fn audio_mode_serializes_steered_variant() {
        use serde_json;
        let m = AudioMode::Steered {
            initial_az_deg: 90.0,
        };
        let s = serde_json::to_string(&m).expect("serializes");
        assert!(s.contains("steered"), "json: {s}");
        assert!(s.contains("initial_az_deg"), "json: {s}");
        let back: AudioMode = serde_json::from_str(&s).expect("round-trips");
        assert!(
            matches!(back, AudioMode::Steered { initial_az_deg } if (initial_az_deg - 90.0).abs() < 1e-6)
        );
    }

    #[test]
    fn audio_mode_default_remains_omni() {
        // Steered must not displace Omni as the default.
        assert!(matches!(AudioMode::default(), AudioMode::Omni));
    }

    #[test]
    fn provisioning_channel_defaults_idle() {
        let bus = StateBus::new();
        let rx = bus.provisioning.subscribe();
        assert_eq!(*rx.borrow(), ProvisioningState::Idle);
    }

    #[test]
    fn upload_mode_serializes_snake_case_and_defaults_to_automatic() {
        assert_eq!(UploadMode::default(), UploadMode::Automatic);
        assert_eq!(
            serde_json::to_string(&UploadMode::LocalOnly).unwrap(),
            "\"local_only\""
        );
    }

    #[test]
    fn state_bus_carries_settings_files_and_playback() {
        let bus = StateBus::new();
        assert_eq!(bus.settings.borrow().brightness_pct, 80);
        assert!(bus.files.borrow().recordings.is_empty());
        assert!(bus.playback.borrow().recording.is_none());
    }

    #[test]
    fn settings_defaults_match_the_design_timings() {
        let s = SettingsState::default();
        assert_eq!((s.dim_after_s, s.off_after_s, s.ask_timeout_s), (120, 600, 60));
        assert!(s.pre_recording && s.clock_24h && s.tones_enabled);
        assert!(!s.first_run_done && !s.pin_enabled);
        assert_eq!(s.time_zone, "UTC");
        assert_eq!(s.upload_mode, UploadMode::Automatic);
    }

    #[test]
    fn new_audio_and_meeting_fields_default_to_idle() {
        let audio = AudioState::default();
        assert!(!audio.host_listening);
        assert_eq!((audio.doa_confidence, audio.beam_aim_deg), (0.0, 0.0));
        let meeting = MeetingState::default();
        assert_eq!(meeting.marks, 0);
        assert!(meeting.current_speaker.is_none());
        assert!(meeting.ask_deadline_secs.is_none());
        assert!(!meeting.pre_recording_active);
    }

    #[test]
    fn only_failed_or_damaged_recordings_need_attention() {
        assert!(RecordingStatus::Failed.needs_attention());
        assert!(RecordingStatus::Damaged.needs_attention());
        for ok in [
            RecordingStatus::AwaitingDecision,
            RecordingStatus::LocalOnly,
            RecordingStatus::Waiting,
            RecordingStatus::Uploading,
            RecordingStatus::Uploaded,
        ] {
            assert!(!ok.needs_attention(), "{ok:?}");
        }
    }
}
