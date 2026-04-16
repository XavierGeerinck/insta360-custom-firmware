//! Channel map: decides which of the 10 ALSA capture channels are physical
//! mics (kept) vs references (dropped) before emitting AudioFrames.

use serde::Deserialize;
use std::path::Path;
use tracing::warn;

pub const DEFAULT_JSON: &str = include_str!("../../config/channel_map_default.json");

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelMap {
    pub version: u32,
    pub total_channels: u8,
    pub physical_mics: Vec<u8>,
    pub reference_channels: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMapSource {
    File,
    Fallback,
}

#[derive(thiserror::Error, Debug)]
pub enum ChannelMapError {
    #[error("parse failure: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("invalid: {0}")]
    Invalid(String),
}

impl ChannelMap {
    /// Parse from a JSON string. Validates indices are in range and no duplicates.
    pub fn from_json(s: &str) -> Result<Self, ChannelMapError> {
        let m: ChannelMap = serde_json::from_str(s)?;
        m.validate()?;
        Ok(m)
    }

    /// Embedded default fallback (mics 0-7, refs 8-9).
    pub fn default_embedded() -> Self {
        Self::from_json(DEFAULT_JSON).expect("embedded default must parse")
    }

    /// Try to load from `path`; on any failure (missing, malformed, invalid),
    /// fall back to the embedded default and return the source used.
    pub fn load_or_default(path: &Path) -> (Self, ChannelMapSource) {
        match std::fs::read_to_string(path) {
            Ok(s) => match Self::from_json(&s) {
                Ok(m) => (m, ChannelMapSource::File),
                Err(e) => {
                    warn!("channel map at {} invalid ({e}), using fallback", path.display());
                    (Self::default_embedded(), ChannelMapSource::Fallback)
                }
            },
            Err(e) => {
                warn!("channel map at {} unreadable ({e}), using fallback", path.display());
                (Self::default_embedded(), ChannelMapSource::Fallback)
            }
        }
    }

    fn validate(&self) -> Result<(), ChannelMapError> {
        if self.total_channels == 0 {
            return Err(ChannelMapError::Invalid("total_channels must be > 0".into()));
        }
        for &m in &self.physical_mics {
            if m >= self.total_channels {
                return Err(ChannelMapError::Invalid(format!("mic index {m} >= total_channels {}", self.total_channels)));
            }
        }
        for &r in &self.reference_channels {
            if r >= self.total_channels {
                return Err(ChannelMapError::Invalid(format!("ref index {r} >= total_channels {}", self.total_channels)));
            }
        }
        // Disallow a channel appearing as both mic and reference.
        for &m in &self.physical_mics {
            if self.reference_channels.contains(&m) {
                return Err(ChannelMapError::Invalid(format!("channel {m} in both mic and ref")));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_default_parses() {
        let m = ChannelMap::default_embedded();
        assert_eq!(m.physical_mics, vec![0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(m.reference_channels, vec![8, 9]);
        assert_eq!(m.total_channels, 10);
    }

    #[test]
    fn rejects_out_of_range_mic_index() {
        let json = r#"{"version":1,"total_channels":4,"physical_mics":[0,1,2,3,99],"reference_channels":[]}"#;
        assert!(matches!(ChannelMap::from_json(json), Err(ChannelMapError::Invalid(_))));
    }

    #[test]
    fn rejects_channel_in_both_mic_and_ref() {
        let json = r#"{"version":1,"total_channels":4,"physical_mics":[0,1],"reference_channels":[1,2]}"#;
        assert!(matches!(ChannelMap::from_json(json), Err(ChannelMapError::Invalid(_))));
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(ChannelMap::from_json("{not json"), Err(ChannelMapError::Parse(_))));
    }

    #[test]
    fn load_or_default_missing_file_uses_fallback() {
        let (m, src) = ChannelMap::load_or_default(Path::new("/definitely/not/here.json"));
        assert_eq!(src, ChannelMapSource::Fallback);
        assert_eq!(m.physical_mics.len(), 8);
    }

    #[test]
    fn load_or_default_valid_file_uses_file() {
        let tmp = std::env::temp_dir().join(format!("chanmap-test-{}.json", std::process::id()));
        std::fs::write(&tmp, r#"{"version":1,"total_channels":4,"physical_mics":[0,1],"reference_channels":[2,3]}"#).unwrap();
        let (m, src) = ChannelMap::load_or_default(&tmp);
        assert_eq!(src, ChannelMapSource::File);
        assert_eq!(m.physical_mics, vec![0, 1]);
        std::fs::remove_file(&tmp).ok();
    }
}
