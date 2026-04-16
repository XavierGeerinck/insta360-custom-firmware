//! Callback bridge: C callbacks from TEA SDK -> tokio mpsc channels.

use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct AudioFrame {
    /// Mic PCM samples (interleaved if multi-channel).
    pub data: Vec<i16>,
    /// Aligned reference PCM (loopback), same timestamp. `None` when
    /// reference capture is disabled or for mic-only replay / tests.
    pub reference: Option<Vec<i16>>,
    pub sample_rate: u32,
    /// Mic channel count (applies to `data`).
    pub channels: u16,
    /// Reference channel count (applies to `reference` when Some). `0` when
    /// reference is `None`.
    pub reference_channels: u16,
    pub timestamp_ms: u64,
}

impl AudioFrame {
    /// Construct a mic-only frame (no reference sidecar). Intended for
    /// single-stream sources (replay, mock, simulate).
    pub fn mic_only(data: Vec<i16>, channels: u16, sample_rate: u32, timestamp_ms: u64) -> Self {
        Self { data, reference: None, sample_rate, channels, reference_channels: 0, timestamp_ms }
    }

    /// Construct a paired frame (mic + aligned reference). Intended for
    /// dual-stream ALSA capture.
    pub fn paired(
        data: Vec<i16>,
        channels: u16,
        reference: Vec<i16>,
        reference_channels: u16,
        sample_rate: u32,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            data,
            reference: Some(reference),
            sample_rate,
            channels,
            reference_channels,
            timestamp_ms,
        }
    }
}

pub fn create_audio_channel() -> (mpsc::Sender<AudioFrame>, mpsc::Receiver<AudioFrame>) {
    mpsc::channel(32)
}
