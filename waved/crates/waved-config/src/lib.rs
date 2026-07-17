mod form;
mod migrate;
mod persist;
mod validate;

pub use form::{
    apply_form, field_specs, FieldError, FieldKind, FieldSpec, FormSubmission, Section,
};
pub use validate::validate_config;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use waved_state::AudioMode;

pub const CURRENT_CONFIG_VERSION: u32 = 1;

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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioConfig {
    pub mode: AudioMode,
    pub speaker_volume: u16,
    pub mic_volume: u16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadMode {
    #[default]
    Automatic,
    Ask,
    LocalOnly,
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
        let config: Self = migrated.try_into().map_err(ConfigError::Parse)?;
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
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mode: AudioMode::Omni,
            speaker_volume: 150,
            mic_volume: 200,
        }
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
            "config_version = 1",
            "config_version = 99",
            1,
        );
        assert!(matches!(
            WavedConfig::parse(&source),
            Err(ConfigError::UnsupportedVersion(99))
        ));
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
