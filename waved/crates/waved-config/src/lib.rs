mod clamp;
mod form;
mod migrate;
mod persist;
mod pin;
mod validate;

pub use clamp::{
    clamp_ask_timeout, clamp_brightness, clamp_dim_after, clamp_mic_volume, clamp_off_after,
    clamp_speaker_volume, wrap_beam_aim,
};
pub use form::{
    apply_form, field_specs, FieldError, FieldKind, FieldSpec, FormSubmission, Section,
};
pub use pin::{
    hash_pin, is_valid_pin, pin_digits, verify_device_pin, PinAttempts, PinCheck, PIN_LOCKOUT_MS,
    PIN_MAX_DIGITS, PIN_MAX_TRIES, PIN_MIN_DIGITS,
};
pub use validate::{valid_time_zone, validate_config};

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};
pub use waved_state::UploadMode;
use waved_state::{AudioMode, MotorPosition, NrLevel};

pub const CURRENT_CONFIG_VERSION: u32 = 2;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("failed to serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("unsupported config version {0}")]
    UnsupportedVersion(i64),
    #[error("invalid config: {0}")]
    Validation(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WavedConfig {
    pub config_version: u32,
    pub device: DeviceConfig,
    pub audio: AudioConfig,
    pub display: DisplayConfig,
    pub time: TimeConfig,
    pub recording: RecordingConfig,
    pub tones: TonesConfig,
    pub api: ApiConfig,
    pub storage: StorageConfig,
    pub security: SecurityConfig,
    pub ntp: NtpConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceConfig {
    pub name: String,
    pub language: String,
    /// The first-start screen has been finished or skipped.
    pub first_run_done: bool,
    /// The chosen Mic height. waved-mcu also stores it in the MCU as the
    /// height the lift rises to at power-on.
    pub mic_height: MotorPosition,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioConfig {
    pub mode: AudioMode,
    /// Speaker volume in percent, 0..=100.
    pub speaker_volume: u16,
    /// Linear microphone gain in percent; 100 = unity.
    pub mic_volume: u16,
    pub nr_level: NrLevel,
    /// Fixed aim for Focused / Narrow / Two sides, degrees clockwise from the
    /// top of the screen, 0.0..360.0.
    pub beam_aim_deg: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplayConfig {
    pub brightness_pct: u8,
    /// Seconds without a touch before the resting screen; 0 = never.
    pub dim_after_s: u32,
    /// Seconds without a touch before the screen turns off; 0 = never.
    pub off_after_s: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimeConfig {
    /// IANA time zone name.
    pub zone: String,
    pub clock_24h: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecordingConfig {
    /// Keep the last 30 s of audio in RAM so Record can include it.
    pub pre_recording: bool,
    /// Seconds before an unanswered "Send this recording?" keeps the file.
    pub ask_timeout_s: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TonesConfig {
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApiConfig {
    pub endpoint: String,
    pub auth_token: String,
    pub upload_mode: UploadMode,
    pub allow_insecure_http: bool,
    pub retry_max: u32,
    pub retry_backoff_ms: u64,
    pub retry_max_backoff_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    pub prune_uploaded_at_percent: u8,
    pub minimum_free_mb: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecurityConfig {
    pub device_password: String,
    pub ota_server: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NtpConfig {
    pub servers: Vec<String>,
}

impl WavedConfig {
    pub fn parse(contents: &str) -> Result<Self, ConfigError> {
        let value: toml::Value = toml::from_str(contents)?;
        let migrated = migrate::migrate(value)?;
        let mut config: Self = migrated.try_into().map_err(ConfigError::Parse)?;
        if clamp::normalise(&mut config) {
            warn!("config had out-of-range settings; clamped them into range");
        }
        pin::normalise_stored(&mut config.security);
        validate_config(&config).map_err(|errors| {
            ConfigError::Validation(
                errors
                    .into_iter()
                    .map(|error| format!("{}: {}", error.path, error.message))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)?;
        let config = Self::parse(&contents)?;
        info!("loaded config from {}", path.display());
        Ok(config)
    }

    pub fn load_or_default(path: &Path) -> Self {
        match Self::load(path) {
            Ok(config) => config,
            Err(primary_error) => {
                let backup = backup_path(path);
                match Self::load(&backup) {
                    Ok(config) => {
                        warn!(
                            "failed to load config ({primary_error}); using backup {}",
                            backup.display()
                        );
                        config
                    }
                    Err(backup_error) => {
                        warn!(
                            "failed to load config ({primary_error}) and backup \
                             ({backup_error}); using visible built-in defaults"
                        );
                        Self::default()
                    }
                }
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        persist::save_atomic(self, path)?;
        info!("saved config to {}", path.display());
        Ok(())
    }

    /// Save, first moving aside a config.toml that exists but does not load.
    /// Only an explicit user save (the setup page) should call this; the
    /// settings service must keep refusing to write over an unloadable file.
    /// The old file is kept as `config.toml.unloadable` (numbered if that
    /// name is taken) and its path is returned; `None` means nothing needed
    /// keeping.
    pub fn save_replacing_unloadable(&self, path: &Path) -> Result<Option<PathBuf>, ConfigError> {
        let kept = persist::save_replacing_unloadable(self, path)?;
        if let Some(kept) = &kept {
            warn!(
                "config at {} did not load; kept it as {}",
                path.display(),
                kept.display()
            );
        }
        info!("saved config to {}", path.display());
        Ok(kept)
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

impl Default for WavedConfig {
    fn default() -> Self {
        Self {
            config_version: CURRENT_CONFIG_VERSION,
            device: DeviceConfig::default(),
            audio: AudioConfig::default(),
            display: DisplayConfig::default(),
            time: TimeConfig::default(),
            recording: RecordingConfig::default(),
            tones: TonesConfig::default(),
            api: ApiConfig::default(),
            storage: StorageConfig::default(),
            security: SecurityConfig::default(),
            ntp: NtpConfig::default(),
        }
    }
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            name: "Wave Open".into(),
            language: "en".into(),
            first_run_done: false,
            mic_height: MotorPosition::Medium,
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mode: AudioMode::Omni,
            speaker_volume: 100,
            mic_volume: 200,
            nr_level: NrLevel::Off,
            beam_aim_deg: 0.0,
        }
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            brightness_pct: 80,
            dim_after_s: 120,
            off_after_s: 600,
        }
    }
}

impl Default for TimeConfig {
    fn default() -> Self {
        Self {
            zone: "UTC".into(),
            clock_24h: true,
        }
    }
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            pre_recording: false,
            ask_timeout_s: 60,
        }
    }
}

impl Default for TonesConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            auth_token: String::new(),
            upload_mode: UploadMode::Automatic,
            allow_insecure_http: false,
            retry_max: 5,
            retry_backoff_ms: 1_000,
            retry_max_backoff_ms: 60_000,
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            prune_uploaded_at_percent: 85,
            minimum_free_mb: 2_048,
        }
    }
}

impl Default for NtpConfig {
    fn default() -> Self {
        Self {
            servers: vec!["pool.ntp.org".into(), "time.google.com".into()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy(auto_upload: bool, mode: &str) -> String {
        format!(
            r#"
[device]
name = "Wave Open"
language = "en"

[audio]
mode = "{mode}"
speaker_volume = 150
mic_volume = 200

[api]
endpoint = "https://example.com/meetings"
auth_token = "secret"
auto_upload = {auto_upload}
retry_max = 5
retry_backoff_ms = 1000

[security]
device_password = ""
ota_server = ""

[ntp]
servers = ["pool.ntp.org"]
"#
        )
    }

    #[test]
    fn default_toml_matches_default_model() {
        let parsed = WavedConfig::parse(include_str!("../../../config/default.toml")).unwrap();
        assert_eq!(parsed, WavedConfig::default());
    }

    #[test]
    fn legacy_config_migrates_auto_upload_and_noise_reduction() {
        let config = WavedConfig::parse(&legacy(true, "noise_reduction")).unwrap();
        assert_eq!(config.config_version, CURRENT_CONFIG_VERSION);
        assert_eq!(config.audio.mode, AudioMode::Omni);
        assert_eq!(config.api.upload_mode, UploadMode::Automatic);
        assert_eq!(config.api.auth_token, "secret");
        assert_eq!(config.storage, StorageConfig::default());

        let ask = WavedConfig::parse(&legacy(false, "omni")).unwrap();
        assert_eq!(ask.api.upload_mode, UploadMode::Ask);
    }

    #[test]
    fn unknown_future_version_is_rejected() {
        let source = include_str!("../../../config/default.toml").replacen(
            "config_version = 2",
            "config_version = 99",
            1,
        );
        assert!(matches!(
            WavedConfig::parse(&source),
            Err(ConfigError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn v1_config_migrates_to_v2_and_keeps_user_values() {
        let source = r#"config_version = 1

[device]
name = "Board room"
language = "en"

[audio]
mode = "cardioid"
speaker_volume = 150
mic_volume = 300
"#;
        let config = WavedConfig::parse(source).unwrap();
        assert_eq!(config.config_version, 2);
        assert_eq!(config.device.name, "Board room");
        assert!(
            config.device.first_run_done,
            "an existing install skips first start"
        );
        assert_eq!(config.audio.mode, AudioMode::Cardioid);
        assert_eq!(
            config.audio.speaker_volume, 100,
            "v1 0..=200 clamps to percent"
        );
        assert_eq!(config.audio.mic_volume, 300);
        assert_eq!(config.display, DisplayConfig::default());
        assert_eq!(config.recording, RecordingConfig::default());
        assert_eq!(config.time, TimeConfig::default());
    }

    fn v2_with(section: &str, line: &str) -> String {
        format!(
            r#"config_version = 2

[device]
name = "Board room"

[api]
endpoint = "https://notes.example.com/hook"
auth_token = "secret"

[security]
device_password = "4821"

[{section}]
{line}
"#
        )
    }

    #[test]
    fn v2_out_of_range_device_values_are_clamped_and_keep_user_values() {
        type Check = fn(&WavedConfig) -> bool;
        let cases: [(&str, &str, Check); 13] = [
            ("display", "brightness_pct = 3", |c| {
                c.display.brightness_pct == 5
            }),
            ("display", "brightness_pct = 200", |c| {
                c.display.brightness_pct == 100
            }),
            ("display", "dim_after_s = 5", |c| {
                c.display.dim_after_s == 15
            }),
            ("display", "dim_after_s = 99999", |c| {
                c.display.dim_after_s == 3_600 && c.display.off_after_s == 3_600
            }),
            ("display", "dim_after_s = 600\noff_after_s = 300", |c| {
                c.display.dim_after_s == 600 && c.display.off_after_s == 600
            }),
            ("display", "off_after_s = 10", |c| {
                c.display.off_after_s == 120
            }),
            ("display", "off_after_s = 999999", |c| {
                c.display.off_after_s == 86_400
            }),
            ("recording", "ask_timeout_s = 5", |c| {
                c.recording.ask_timeout_s == 15
            }),
            ("recording", "ask_timeout_s = 9000", |c| {
                c.recording.ask_timeout_s == 600
            }),
            ("audio", "speaker_volume = 150", |c| {
                c.audio.speaker_volume == 100
            }),
            ("audio", "mic_volume = 5000", |c| c.audio.mic_volume == 800),
            ("audio", "beam_aim_deg = 400.0", |c| {
                c.audio.beam_aim_deg == 40.0
            }),
            ("audio", "beam_aim_deg = nan", |c| {
                c.audio.beam_aim_deg == 0.0
            }),
        ];
        for (section, line, check) in cases {
            let source = v2_with(section, line);
            let config =
                WavedConfig::parse(&source).unwrap_or_else(|error| panic!("{line}: {error}"));
            assert!(check(&config), "{line}: {config:?}");
            assert!(validate_config(&config).is_ok(), "{line}");
            assert_eq!(config.device.name, "Board room", "{line}");
            assert_eq!(
                config.api.endpoint, "https://notes.example.com/hook",
                "{line}"
            );
            assert_eq!(config.api.auth_token, "secret", "{line}");
            assert!(verify_device_pin(&config.security, "4821"), "{line}");
        }
        let negative = WavedConfig::parse(&v2_with("audio", "beam_aim_deg = -60.0")).unwrap();
        assert_eq!(negative.audio.beam_aim_deg, 300.0);
    }

    #[test]
    fn a_plain_pin_loads_as_a_hash_and_other_passwords_are_dropped() {
        let config = WavedConfig::parse(&v2_with("tones", "enabled = true")).unwrap();
        assert!(config.security.device_password.starts_with("pin1$4$"));
        assert!(verify_device_pin(&config.security, "4821"));

        let source = v2_with("tones", "enabled = true").replace(
            "device_password = \"4821\"",
            "device_password = \"hunter2\"",
        );
        let config = WavedConfig::parse(&source).unwrap();
        assert_eq!(
            config.security.device_password, "",
            "the keypad can't type it"
        );
        assert_eq!(config.device.name, "Board room", "the rest still loads");
    }

    #[test]
    fn a_pin_record_round_trips_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = WavedConfig::default();
        config.security.device_password = hash_pin("482100");
        config.save(&path).unwrap();
        assert_eq!(WavedConfig::load(&path).unwrap(), config);
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("device_password = \"pin1$6$"), "{on_disk}");
    }

    #[test]
    fn structural_errors_are_still_rejected() {
        for source in [
            v2_with("time", "zone = \"../etc\""),
            v2_with("device", "future_flag = true"),
            v2_with("api", "retry_max = 0"),
        ] {
            assert!(WavedConfig::parse(&source).is_err(), "{source}");
        }
    }

    #[test]
    fn the_mic_height_is_kept_and_defaults_to_medium() {
        // Older files have no mic_height: stock's factory start height, 40 mm.
        let older = WavedConfig::parse(&v2_with("tones", "enabled = true")).unwrap();
        assert_eq!(older.device.mic_height, MotorPosition::Medium);
        assert_eq!(
            WavedConfig::default().device.mic_height,
            MotorPosition::Medium
        );
        let low = WavedConfig::parse(&v2_with("device", "mic_height = \"low\"").replacen(
            "[device]\nname = \"Board room\"\n",
            "",
            1,
        ))
        .unwrap();
        assert_eq!(low.device.mic_height, MotorPosition::Low);
        let moving = v2_with("device", "mic_height = \"moving\"").replacen(
            "[device]\nname = \"Board room\"\n",
            "",
            1,
        );
        assert!(
            WavedConfig::parse(&moving).is_err(),
            "moving is not a height"
        );
    }

    #[test]
    fn new_installs_show_first_start() {
        assert!(!WavedConfig::default().device.first_run_done);
        assert_eq!(WavedConfig::default().config_version, 2);
    }

    #[test]
    fn v0_config_chains_through_to_v2() {
        let source = "[audio]\nspeaker_volume = 150\n";
        let config = WavedConfig::parse(source).unwrap();
        assert_eq!(config.config_version, 2);
        assert_eq!(config.audio.speaker_volume, 100);
    }

    #[test]
    fn non_table_sections_are_errors_not_panics() {
        for source in [
            "config_version = 1\ndevice = \"x\"\n",
            "config_version = 1\naudio = 3\n",
            "device = \"x\"\n",
            "api = 1\n",
            "storage = 5\n",
        ] {
            assert!(WavedConfig::parse(source).is_err(), "{source}");
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "config_version = 1\ndevice = \"x\"\n").unwrap();
        assert_eq!(WavedConfig::load_or_default(&path), WavedConfig::default());
    }

    #[test]
    fn roundtrip_save_load_creates_secure_backup() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = WavedConfig::default();
        config.save(&path).unwrap();
        config.device.name = "Meeting room".into();
        config.save(&path).unwrap();

        assert_eq!(WavedConfig::load(&path).unwrap(), config);
        assert_eq!(
            WavedConfig::load(&backup_path(&path)).unwrap(),
            WavedConfig::default()
        );
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn load_or_default_uses_valid_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut backup = WavedConfig::default();
        backup.device.name = "Backup".into();
        backup.save(&backup_path(&path)).unwrap();
        std::fs::write(&path, "bad {{{").unwrap();

        assert_eq!(WavedConfig::load_or_default(&path).device.name, "Backup");
    }
}
