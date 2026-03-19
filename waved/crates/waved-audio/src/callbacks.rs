//! Callback bridge: C callbacks from TEA SDK -> tokio mpsc channels.

use tokio::sync::mpsc;

#[derive(Clone)]
pub struct AudioFrame {
    pub data: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
    pub timestamp_ms: u64,
}

pub fn create_audio_channel() -> (mpsc::Sender<AudioFrame>, mpsc::Receiver<AudioFrame>) {
    mpsc::channel(32)
}
