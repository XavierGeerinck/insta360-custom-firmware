//! AudioConfig — forward-compatible configuration for waved-audio capture.
//!
//! Loaded from `/etc/waved-audio-config.json` on device; falls back to the
//! embedded default (mics on `hw:0,2`, references on `hw:0,3`) when the file
//! is missing or malformed. Version-bumped when future sub-projects add
//! fields (mic_geometry, playback, etc.).

use serde::Deserialize;
use std::path::Path;
use thiserror::Error;
use tracing::warn;

const DEFAULT_JSON: &str = include_str!("../config/audio_config_default.json");

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AudioConfig {
    pub version: u32,
    pub capture: CaptureConfig,
    pub period_frames: usize,
    pub buffer_periods: u32,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CaptureConfig {
    pub mic_device: String,
    pub mic_channels: u32,
    pub mic_sample_rate: u32,
    pub mic_format: SampleFormat,
    /// Absent when reference capture is disabled.
    #[serde(default)]
    pub reference_device: Option<String>,
    #[serde(default)]
    pub reference_channels: u32,
    #[serde(default = "default_sample_rate")]
    pub reference_sample_rate: u32,
    #[serde(default)]
    pub reference_format: Option<SampleFormat>,
}

fn default_sample_rate() -> u32 { 48000 }

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum SampleFormat {
    S16_LE,
    S24_LE,
    S32_LE,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    File,
    Fallback,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("parse failure: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("invalid: {0}")]
    Invalid(String),
}

impl AudioConfig {
    pub fn from_json(json: &str) -> Result<Self, ConfigError> {
        let cfg: AudioConfig = serde_json::from_str(json)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn default_embedded() -> Self {
        Self::from_json(DEFAULT_JSON).expect("embedded default must parse")
    }

    pub fn load_or_default(path: &Path) -> (Self, ConfigSource) {
        match std::fs::read_to_string(path) {
            Ok(s) => match Self::from_json(&s) {
                Ok(cfg) => (cfg, ConfigSource::File),
                Err(e) => {
                    warn!("audio config at {} invalid ({e}), using fallback", path.display());
                    (Self::default_embedded(), ConfigSource::Fallback)
                }
            },
            Err(e) => {
                warn!("audio config at {} unreadable ({e}), using fallback", path.display());
                (Self::default_embedded(), ConfigSource::Fallback)
            }
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.version != 2 {
            return Err(ConfigError::Invalid(format!(
                "unsupported config version {}, expected 2", self.version
            )));
        }
        if self.capture.mic_channels == 0 {
            return Err(ConfigError::Invalid("mic_channels must be > 0".into()));
        }
        if self.capture.mic_sample_rate == 0 {
            return Err(ConfigError::Invalid("mic_sample_rate must be > 0".into()));
        }
        if self.period_frames == 0 {
            return Err(ConfigError::Invalid("period_frames must be > 0".into()));
        }
        if self.buffer_periods < 2 {
            return Err(ConfigError::Invalid("buffer_periods must be >= 2".into()));
        }
        // Reference: all-or-nothing.
        let has_ref_device = self.capture.reference_device.is_some();
        let has_ref_channels = self.capture.reference_channels > 0;
        let has_ref_format = self.capture.reference_format.is_some();
        match (has_ref_device, has_ref_channels, has_ref_format) {
            (true, true, true) => {}
            (false, false, false) => {}
            _ => return Err(ConfigError::Invalid(
                "reference_device, reference_channels, reference_format must all be set or all absent".into()
            )),
        }
        Ok(())
    }

    pub fn reference_enabled(&self) -> bool {
        self.capture.reference_device.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_default_parses_and_is_version_2() {
        let cfg = AudioConfig::default_embedded();
        assert_eq!(cfg.version, 2);
        assert_eq!(cfg.capture.mic_device, "hw:0,2");
        assert_eq!(cfg.capture.mic_channels, 8);
        assert_eq!(cfg.capture.reference_device.as_deref(), Some("hw:0,3"));
        assert_eq!(cfg.capture.reference_channels, 2);
        assert!(cfg.reference_enabled());
    }

    #[test]
    fn rejects_wrong_version() {
        let json = r#"{"version":1,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},"period_frames":256,"buffer_periods":4}"#;
        assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_zero_mic_channels() {
        let json = r#"{"version":2,"capture":{"mic_device":"hw:0,2","mic_channels":0,"mic_sample_rate":48000,"mic_format":"S32_LE"},"period_frames":256,"buffer_periods":4}"#;
        assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_partial_reference_config() {
        let json = r#"{"version":2,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE","reference_device":"hw:0,3"},"period_frames":256,"buffer_periods":4}"#;
        assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn mic_only_config_is_valid() {
        let json = r#"{"version":2,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},"period_frames":256,"buffer_periods":4}"#;
        let cfg = AudioConfig::from_json(json).unwrap();
        assert!(!cfg.reference_enabled());
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(AudioConfig::from_json("{not json"), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn load_or_default_missing_file_uses_fallback() {
        let (cfg, src) = AudioConfig::load_or_default(Path::new("/definitely/not/here.json"));
        assert_eq!(src, ConfigSource::Fallback);
        assert_eq!(cfg.version, 2);
    }

    #[test]
    fn load_or_default_valid_file_uses_file() {
        let tmp = std::env::temp_dir().join(format!("audiocfg-test-{}.json", std::process::id()));
        std::fs::write(&tmp, r#"{"version":2,"capture":{"mic_device":"hw:5,5","mic_channels":4,"mic_sample_rate":16000,"mic_format":"S16_LE"},"period_frames":128,"buffer_periods":2}"#).unwrap();
        let (cfg, src) = AudioConfig::load_or_default(&tmp);
        assert_eq!(src, ConfigSource::File);
        assert_eq!(cfg.capture.mic_device, "hw:5,5");
        assert_eq!(cfg.capture.mic_channels, 4);
        std::fs::remove_file(&tmp).ok();
    }
}
