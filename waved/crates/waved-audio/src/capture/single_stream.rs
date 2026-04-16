// Stub — implemented in Task 7
use super::{AudioFrameSource, PcmSource};
use crate::callbacks::AudioFrame;

pub struct SingleStreamFrameSource<P: PcmSource> {
    _source: P,
}

impl<P: PcmSource> SingleStreamFrameSource<P> {
    pub fn new(source: P, _period_frames: usize, _channels: usize) -> Self {
        Self { _source: source }
    }
}

impl<P: PcmSource> AudioFrameSource for SingleStreamFrameSource<P> {
    fn next_frame(&mut self) -> std::io::Result<Option<AudioFrame>> {
        Ok(None)
    }
    fn sample_rate(&self) -> u32 { 0 }
}
