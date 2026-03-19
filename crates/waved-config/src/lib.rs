use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{info, warn};
use waved_state::AudioMode;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("failed to serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WavedConfig {
    pub device: DeviceConfig,
    pub audio: AudioConfig,
    pub api: ApiConfig,
    pub security: SecurityConfig,
    pub ntp: NtpConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub name: String,
    pub language: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioConfig {
    pub mode: AudioMode,
    pub speaker_volume: u16,
    pub mic_volume: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiConfig {
    pub endpoint: String,
    pub auth_token: String,
    pub auto_upload: bool,
    pub retry_max: u32,
    pub retry_backoff_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub device_password: String,
    pub ota_server: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NtpConfig {
    pub servers: Vec<String>,
}

impl WavedConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&contents)?;
        info!("loaded config from {}", path.display());
        Ok(config)
    }

    pub fn load_or_default(path: &Path) -> Self {
        match Self::load(path) {
            Ok(config) => config,
            Err(e) => {
                warn!("failed to load config ({}), using defaults", e);
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        info!("saved config to {}", path.display());
        Ok(())
    }
}

impl Default for WavedConfig {
    fn default() -> Self {
        toml::from_str(include_str!("../../../config/default.toml"))
            .expect("default.toml must be valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let config = WavedConfig::default();
        assert_eq!(config.device.name, "Wave Open");
        assert_eq!(config.audio.mode, AudioMode::NoiseReduction);
        assert_eq!(config.audio.speaker_volume, 150);
        assert_eq!(config.ntp.servers.len(), 2);
    }

    #[test]
    fn roundtrip_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = WavedConfig::default();
        config.save(&path).unwrap();
        let loaded = WavedConfig::load(&path).unwrap();
        assert_eq!(loaded.device.name, config.device.name);
        assert_eq!(loaded.api.retry_max, config.api.retry_max);
    }
}
