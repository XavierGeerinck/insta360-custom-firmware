//! AudioConfig — forward-compatible configuration for waved-audio capture.
//!
//! Loaded from `/etc/waved-audio-config.json` on device; falls back to the
//! embedded default (mics on `hw:0,2`, references on `hw:0,3`) when the file
//! is missing or malformed. Version-bumped when future sub-projects add
//! fields (mic_geometry, playback, etc.).

use crate::beam::MicGeometry;
use serde::Deserialize;
use std::path::Path;
use thiserror::Error;
use tracing::warn;

const DEFAULT_JSON: &str = include_str!("../config/audio_config_default.json");

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AudioConfig {
    pub version: u32,
    pub capture: CaptureConfig,
    /// Optional playback + software-reference configuration. Present when
    /// waved owns the playback pipeline (required for AEC on hardware where
    /// the loopback PCM isn't usable — see playback module docs).
    #[serde(default)]
    pub playback: Option<PlaybackSection>,
    pub period_frames: usize,
    pub buffer_periods: u32,
    /// Teardown-measured 8-mic geometry; absent in older JSONs, synthesized
    /// from `MicGeometry::default()` via serde.
    #[serde(default = "default_mic_geometry")]
    pub mic_geometry: Option<MicGeometry>,
    #[serde(default)]
    pub uac: Option<UacConfig>,
}

fn default_mic_geometry() -> Option<MicGeometry> {
    Some(MicGeometry::default())
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PlaybackSection {
    pub device: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub format: SampleFormat,
    pub period_frames: usize,
    pub buffer_periods: u32,
    #[serde(default)]
    pub dsp: Option<PlaybackDspConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PlaybackDspConfig {
    #[serde(default)]
    pub voice: Option<PlaybackPresetTuningJson>,
    #[serde(default)]
    pub media: Option<PlaybackPresetTuningJson>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PlaybackPresetTuningJson {
    pub hpf_cutoff_hz: f32,
    pub eq: [[crate::playback::dsp::eq::BiquadParams; 4]; 2],
    pub mbagc: crate::playback::dsp::mbagc::MbagcParams,
    pub limiter: crate::playback::dsp::limiter::LimiterParams,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct UacConfig {
    #[serde(default = "default_uac_enabled")]
    pub enabled: bool,
    #[serde(default = "default_uac_dev")]
    pub downstream_device: String,
    #[serde(default = "default_uac_dev")]
    pub upstream_device: String,
    #[serde(default = "default_uac_period")]
    pub period_frames: usize,
    #[serde(default = "default_uac_periods")]
    pub buffer_periods: u32,
    #[serde(default = "default_uac_mixer_card")]
    pub mixer_card: String,
}

fn default_uac_enabled() -> bool { true }
fn default_uac_dev() -> String { "hw:1,0".to_string() }
fn default_uac_period() -> usize { 1024 }
fn default_uac_periods() -> u32 { 4 }
fn default_uac_mixer_card() -> String { "hw:1".to_string() }

impl Default for UacConfig {
    fn default() -> Self {
        Self {
            enabled: default_uac_enabled(),
            downstream_device: default_uac_dev(),
            upstream_device: default_uac_dev(),
            period_frames: default_uac_period(),
            buffer_periods: default_uac_periods(),
            mixer_card: default_uac_mixer_card(),
        }
    }
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
    /// PDM hardware preamp gain (ALSA `numid=9` on `hw:0`). Range 0..=48,
    /// where each step is ~0.5 dB; 24 (default) ≈ +12 dB above the ADC
    /// floor. Set once at daemon startup; static across the device's run so
    /// DSP tuning sees a stable input level. See the mic-volume-pipeline spec.
    #[serde(default = "default_pdm_gain")]
    pub pdm_gain: u8,
}

fn default_sample_rate() -> u32 { 48000 }
fn default_pdm_gain() -> u8 { 24 }

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[allow(non_camel_case_types)] // intentional: matches ALSA format string names
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
        if self.version != 3 && self.version != 4 {
            return Err(ConfigError::Invalid(format!(
                "unsupported config version {}, expected 3 or 4", self.version
            )));
        }
        if self.capture.mic_channels == 0 {
            return Err(ConfigError::Invalid("mic_channels must be > 0".into()));
        }
        if self.capture.mic_sample_rate == 0 {
            return Err(ConfigError::Invalid("mic_sample_rate must be > 0".into()));
        }
        if self.capture.pdm_gain > 48 {
            return Err(ConfigError::Invalid(format!(
                "pdm_gain must be in [0, 48], got {}", self.capture.pdm_gain
            )));
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
        // Playback: validate shape when present.
        if let Some(p) = &self.playback {
            if p.channels == 0 {
                return Err(ConfigError::Invalid("playback.channels must be > 0".into()));
            }
            if p.sample_rate == 0 {
                return Err(ConfigError::Invalid("playback.sample_rate must be > 0".into()));
            }
            if p.period_frames == 0 {
                return Err(ConfigError::Invalid("playback.period_frames must be > 0".into()));
            }
            if p.buffer_periods < 2 {
                return Err(ConfigError::Invalid("playback.buffer_periods must be >= 2".into()));
            }
            if let Some(dsp) = &p.dsp {
                for (name, t) in [("voice", &dsp.voice), ("media", &dsp.media)] {
                    if let Some(t) = t {
                        if !(20.0..=200.0).contains(&t.hpf_cutoff_hz) {
                            return Err(ConfigError::Invalid(format!(
                                "{name}.hpf_cutoff_hz must be in [20, 200], got {}", t.hpf_cutoff_hz
                            )));
                        }
                        if !(-3.0..=0.0).contains(&t.limiter.ceiling_dbfs) {
                            return Err(ConfigError::Invalid(format!(
                                "{name}.limiter.ceiling_dbfs must be in [-3, 0], got {}", t.limiter.ceiling_dbfs
                            )));
                        }
                        let cf = t.mbagc.crossover_freq_hz;
                        if !(cf[0] < cf[1] && cf[1] < cf[2]) {
                            return Err(ConfigError::Invalid(format!(
                                "{name}.mbagc.crossover_freq_hz must be strictly increasing, got {cf:?}"
                            )));
                        }
                    }
                }
            }
        }
        // Hardware loopback and software reference are mutually exclusive;
        // enabling both is ambiguous for stats_collector reporting.
        if self.capture.reference_device.is_some() && self.playback.is_some() {
            return Err(ConfigError::Invalid(
                "reference_device (hardware loopback) and playback (software reference) are mutually exclusive".into()
            ));
        }
        if let Some(u) = &self.uac {
            if u.period_frames == 0 {
                return Err(ConfigError::Invalid("uac.period_frames must be > 0".into()));
            }
            if u.buffer_periods < 2 {
                return Err(ConfigError::Invalid("uac.buffer_periods must be >= 2".into()));
            }
            if u.downstream_device.is_empty() || u.upstream_device.is_empty()
               || u.mixer_card.is_empty() {
                return Err(ConfigError::Invalid("uac device strings must be non-empty".into()));
            }
        }
        Ok(())
    }

    pub fn reference_enabled(&self) -> bool {
        self.capture.reference_device.is_some()
    }

    pub fn software_reference_enabled(&self) -> bool {
        self.playback.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_default_parses_and_is_version_4() {
        let cfg = AudioConfig::default_embedded();
        assert_eq!(cfg.version, 4);
        assert_eq!(cfg.capture.mic_device, "hw:0,2");
        assert_eq!(cfg.capture.mic_channels, 8);
        assert!(cfg.capture.reference_device.is_none());
        assert!(!cfg.reference_enabled());
        assert!(cfg.software_reference_enabled());
        let p = cfg.playback.as_ref().expect("playback section");
        assert_eq!(p.device, "hw:0,1");
        assert_eq!(p.channels, 2);
        assert_eq!(p.sample_rate, 48000);
    }

    #[test]
    fn embedded_default_is_v4_with_wave_geometry() {
        let cfg = AudioConfig::default_embedded();
        assert_eq!(cfg.version, 4);
        let geom = cfg.mic_geometry.as_ref().expect("default includes geometry");
        assert_eq!(geom.positions_mm.len(), 8);
        assert_eq!(geom.positions_mm[0].z, 0.0);
        assert_eq!(geom.positions_mm[4].z, -78.2);
    }

    #[test]
    fn v3_json_without_geometry_synthesizes_default() {
        let json = r#"{
            "version":3,
            "capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},
            "playback":{"device":"hw:0,1","channels":2,"sample_rate":48000,"format":"S16_LE","period_frames":1024,"buffer_periods":4},
            "period_frames":1024,"buffer_periods":8
        }"#;
        let cfg = AudioConfig::from_json(json).unwrap();
        assert!(cfg.mic_geometry.is_some(), "missing geometry should default");
        assert_eq!(cfg.mic_geometry.unwrap().positions_mm[0].z, 0.0);
    }

    #[test]
    fn rejects_v2_version_now() {
        let json = r#"{"version":2,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},"period_frames":256,"buffer_periods":4}"#;
        assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_both_hardware_and_software_reference() {
        let json = r#"{
            "version":3,
            "capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE","reference_device":"hw:0,3","reference_channels":2,"reference_format":"S32_LE"},
            "playback":{"device":"hw:0,1","channels":2,"sample_rate":48000,"format":"S16_LE","period_frames":1024,"buffer_periods":4},
            "period_frames":1024,"buffer_periods":8
        }"#;
        assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn accepts_playback_only() {
        let json = r#"{
            "version":3,
            "capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},
            "playback":{"device":"hw:0,1","channels":2,"sample_rate":48000,"format":"S16_LE","period_frames":1024,"buffer_periods":4},
            "period_frames":1024,"buffer_periods":8
        }"#;
        let cfg = AudioConfig::from_json(json).unwrap();
        assert!(cfg.software_reference_enabled());
        assert!(!cfg.reference_enabled());
    }

    #[test]
    fn rejects_wrong_version() {
        let json = r#"{"version":5,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},"period_frames":256,"buffer_periods":4}"#;
        assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_zero_mic_channels() {
        let json = r#"{"version":3,"capture":{"mic_device":"hw:0,2","mic_channels":0,"mic_sample_rate":48000,"mic_format":"S32_LE"},"period_frames":256,"buffer_periods":4}"#;
        assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_partial_reference_config() {
        let json = r#"{"version":3,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE","reference_device":"hw:0,3"},"period_frames":256,"buffer_periods":4}"#;
        assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn mic_only_config_is_valid() {
        let json = r#"{"version":3,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},"period_frames":256,"buffer_periods":4}"#;
        let cfg = AudioConfig::from_json(json).unwrap();
        assert!(!cfg.reference_enabled());
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(AudioConfig::from_json("{not json"), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn rejects_wrong_shape_eq_array() {
        // Spec §5.1 EQ shape (eq.len() == 2, eq[0].len() == 4) is enforced
        // by the [[BiquadParams; 4]; 2] field type — serde rejects any other
        // shape at parse time. ConfigError::Parse is the right classification:
        // the JSON is structurally malformed, not just out of policy.
        let bad = r#"{
            "version": 4,
            "capture": {"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},
            "period_frames": 1024,
            "buffer_periods": 4,
            "playback": {
                "device": "hw:0,1",
                "channels": 2,
                "sample_rate": 48000,
                "format": "S16_LE",
                "period_frames": 1024,
                "buffer_periods": 4,
                "dsp": {
                    "voice": {
                        "hpf_cutoff_hz": 80.0,
                        "eq": [
                            [{"kind":"bypass"}, {"kind":"bypass"}, {"kind":"bypass"}]
                        ],
                        "mbagc": {"enabled":false,"crossover_freq_hz":[300.0,4000.0,7000.0],"target_dbfs":[-12.0,-6.0,-29.0,-35.0],"max_gain_db":[6.0,20.0,10.0,10.0],"knee_db":[5.0,5.0,5.0,5.0],"release_ms":[200.0,200.0,200.0,200.0]},
                        "limiter": {"ceiling_dbfs":-1.0,"lookahead_samples":96,"release_ms":50.0}
                    }
                }
            }
        }"#;
        let err = AudioConfig::from_json(bad).expect_err("wrong-shape eq must fail");
        assert!(matches!(err, ConfigError::Parse(_)),
                "expected Parse error for malformed eq array, got {err:?}");
    }

    #[test]
    fn load_or_default_missing_file_uses_fallback() {
        let (cfg, src) = AudioConfig::load_or_default(Path::new("/definitely/not/here.json"));
        assert_eq!(src, ConfigSource::Fallback);
        assert_eq!(cfg.version, 4);
    }

    #[test]
    fn load_or_default_valid_file_uses_file() {
        let tmp = std::env::temp_dir().join(format!("audiocfg-test-{}.json", std::process::id()));
        std::fs::write(&tmp, r#"{"version":3,"capture":{"mic_device":"hw:5,5","mic_channels":4,"mic_sample_rate":16000,"mic_format":"S16_LE"},"period_frames":128,"buffer_periods":2}"#).unwrap();
        let (cfg, src) = AudioConfig::load_or_default(&tmp);
        assert_eq!(src, ConfigSource::File);
        assert_eq!(cfg.capture.mic_device, "hw:5,5");
        assert_eq!(cfg.capture.mic_channels, 4);
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn v3_config_still_parses() {
        let json = r#"{
            "version": 3,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE"
            },
            "playback": {
                "device": "hw:0,1",
                "channels": 2,
                "sample_rate": 48000,
                "format": "S16_LE",
                "period_frames": 1024,
                "buffer_periods": 4
            },
            "period_frames": 1024,
            "buffer_periods": 8
        }"#;
        let cfg = AudioConfig::from_json(json).expect("v3 should parse");
        assert_eq!(cfg.version, 3);
        assert!(cfg.playback.unwrap().dsp.is_none());
    }

    #[test]
    fn v4_config_with_dsp_block_parses() {
        let json = r#"{
            "version": 4,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE"
            },
            "playback": {
                "device": "hw:0,1",
                "channels": 2,
                "sample_rate": 48000,
                "format": "S16_LE",
                "period_frames": 1024,
                "buffer_periods": 4,
                "dsp": {
                    "voice": {
                        "hpf_cutoff_hz": 80.0,
                        "eq": [
                            [
                                {"kind":"peaking","center_freq_hz":150.0,"gain_db":-6.0,"q":1.0},
                                {"kind":"peaking","center_freq_hz":800.0,"gain_db":2.0,"q":2.0},
                                {"kind":"peaking","center_freq_hz":400.0,"gain_db":-5.0,"q":1.5},
                                {"kind":"peaking","center_freq_hz":3500.0,"gain_db":5.0,"q":2.0}
                            ],
                            [
                                {"kind":"peaking","center_freq_hz":150.0,"gain_db":-6.0,"q":1.0},
                                {"kind":"peaking","center_freq_hz":800.0,"gain_db":2.0,"q":2.0},
                                {"kind":"peaking","center_freq_hz":400.0,"gain_db":-5.0,"q":1.5},
                                {"kind":"peaking","center_freq_hz":3500.0,"gain_db":5.0,"q":2.0}
                            ]
                        ],
                        "mbagc": {
                            "enabled": true,
                            "crossover_freq_hz": [300.0, 4000.0, 7000.0],
                            "target_dbfs": [-12.0, -6.0, -29.0, -35.0],
                            "max_gain_db": [6.0, 20.0, 10.0, 10.0],
                            "knee_db": [5.0, 5.0, 5.0, 5.0],
                            "release_ms": [200.0, 200.0, 200.0, 200.0]
                        },
                        "limiter": {
                            "ceiling_dbfs": -1.0,
                            "lookahead_samples": 96,
                            "release_ms": 50.0
                        }
                    }
                }
            },
            "period_frames": 1024,
            "buffer_periods": 8
        }"#;
        let cfg = AudioConfig::from_json(json).expect("v4 should parse");
        assert_eq!(cfg.version, 4);
        let voice = cfg.playback.unwrap().dsp.unwrap().voice.unwrap();
        assert_eq!(voice.hpf_cutoff_hz, 80.0);
        assert!(voice.mbagc.enabled);
    }

    #[test]
    fn uac_config_default_when_absent() {
        let json = r#"{
            "version": 4,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE"
            },
            "period_frames": 1024,
            "buffer_periods": 8
        }"#;
        let cfg = AudioConfig::from_json(json).expect("v4 should parse");
        assert!(cfg.uac.is_none(), "uac block defaults to None when absent");
    }

    #[test]
    fn uac_config_with_overrides_parses() {
        let json = r#"{
            "version": 4,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE"
            },
            "period_frames": 1024,
            "buffer_periods": 8,
            "uac": {
                "enabled": false,
                "downstream_device": "hw:2,0",
                "upstream_device": "hw:2,0",
                "period_frames": 512,
                "buffer_periods": 8,
                "mixer_card": "hw:2"
            }
        }"#;
        let cfg = AudioConfig::from_json(json).expect("v4 should parse");
        let u = cfg.uac.unwrap();
        assert!(!u.enabled);
        assert_eq!(u.downstream_device, "hw:2,0");
        assert_eq!(u.period_frames, 512);
    }

    #[test]
    fn uac_config_rejects_zero_period() {
        let json = r#"{
            "version": 4,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE"
            },
            "period_frames": 1024,
            "buffer_periods": 8,
            "uac": {
                "enabled": true,
                "downstream_device": "hw:1,0",
                "upstream_device": "hw:1,0",
                "period_frames": 0,
                "buffer_periods": 4,
                "mixer_card": "hw:1"
            }
        }"#;
        assert!(AudioConfig::from_json(json).is_err());
    }

    #[test]
    fn uac_config_default_round_trip() {
        let d = UacConfig::default();
        assert!(d.enabled);
        assert_eq!(d.downstream_device, "hw:1,0");
        assert_eq!(d.upstream_device, "hw:1,0");
        assert_eq!(d.period_frames, 1024);
        assert_eq!(d.buffer_periods, 4);
        assert_eq!(d.mixer_card, "hw:1");
    }

    #[test]
    fn embedded_default_has_pdm_gain_24() {
        let cfg = AudioConfig::default_embedded();
        assert_eq!(cfg.capture.pdm_gain, 24);
    }

    #[test]
    fn pdm_gain_above_48_is_rejected() {
        let json = r#"{
            "version": 4,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE",
                "pdm_gain": 49
            },
            "period_frames": 1024,
            "buffer_periods": 8
        }"#;
        assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn pdm_gain_at_48_is_accepted() {
        let json = r#"{
            "version": 4,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE",
                "pdm_gain": 48
            },
            "period_frames": 1024,
            "buffer_periods": 8
        }"#;
        let cfg = AudioConfig::from_json(json).expect("48 should validate");
        assert_eq!(cfg.capture.pdm_gain, 48);
    }

    #[test]
    fn pdm_gain_absent_uses_serde_default() {
        let json = r#"{
            "version": 4,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE"
            },
            "period_frames": 1024,
            "buffer_periods": 8
        }"#;
        let cfg = AudioConfig::from_json(json).expect("missing pdm_gain should default");
        assert_eq!(cfg.capture.pdm_gain, 24);
    }

    #[test]
    fn pdm_gain_at_0_is_accepted() {
        let json = r#"{
            "version": 4,
            "capture": {
                "mic_device": "hw:0,2",
                "mic_channels": 8,
                "mic_sample_rate": 48000,
                "mic_format": "S32_LE",
                "pdm_gain": 0
            },
            "period_frames": 1024,
            "buffer_periods": 8
        }"#;
        let cfg = AudioConfig::from_json(json).expect("0 should validate");
        assert_eq!(cfg.capture.pdm_gain, 0);
    }

    #[test]
    fn invalid_hpf_cutoff_rejected() {
        let mut json = r#"{"version":4,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},"playback":{"device":"hw:0,1","channels":2,"sample_rate":48000,"format":"S16_LE","period_frames":1024,"buffer_periods":4,"dsp":{"voice":{"hpf_cutoff_hz":5.0,"eq":[[{"kind":"bypass","center_freq_hz":0.0,"gain_db":0.0,"q":0.0},{"kind":"bypass","center_freq_hz":0.0,"gain_db":0.0,"q":0.0},{"kind":"bypass","center_freq_hz":0.0,"gain_db":0.0,"q":0.0},{"kind":"bypass","center_freq_hz":0.0,"gain_db":0.0,"q":0.0}],[{"kind":"bypass","center_freq_hz":0.0,"gain_db":0.0,"q":0.0},{"kind":"bypass","center_freq_hz":0.0,"gain_db":0.0,"q":0.0},{"kind":"bypass","center_freq_hz":0.0,"gain_db":0.0,"q":0.0},{"kind":"bypass","center_freq_hz":0.0,"gain_db":0.0,"q":0.0}]],"mbagc":{"enabled":false,"crossover_freq_hz":[300.0,4000.0,7000.0],"target_dbfs":[0.0,0.0,0.0,0.0],"max_gain_db":[0.0,0.0,0.0,0.0],"knee_db":[5.0,5.0,5.0,5.0],"release_ms":[200.0,200.0,200.0,200.0]},"limiter":{"ceiling_dbfs":-1.0,"lookahead_samples":96,"release_ms":50.0}}}},"period_frames":1024,"buffer_periods":8}"#.to_string();
        let res = AudioConfig::from_json(&json);
        assert!(res.is_err(), "hpf_cutoff_hz=5 should be rejected");
        json = json.replace("\"hpf_cutoff_hz\":5.0", "\"hpf_cutoff_hz\":80.0");
        let cfg = AudioConfig::from_json(&json).expect("80 Hz should validate");
        assert_eq!(cfg.version, 4);
    }
}
