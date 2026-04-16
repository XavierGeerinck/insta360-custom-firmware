//! SingleStreamFrameSource — wraps a PcmSource + channel count + period size
//! into an AudioFrameSource emitting mic-only AudioFrames (reference = None).

use super::{AudioFrameSource, PcmSource};
use crate::callbacks::AudioFrame;
use std::time::Instant;

pub struct SingleStreamFrameSource<P: PcmSource> {
    source: P,
    channels: usize,
    raw: Vec<i32>,
    out: Vec<i16>,
    start: Instant,
}

impl<P: PcmSource> SingleStreamFrameSource<P> {
    pub fn new(source: P, period_frames: usize, channels: usize) -> Self {
        Self {
            channels,
            raw: vec![0i32; period_frames * channels],
            out: vec![0i16; period_frames * channels],
            source,
            start: Instant::now(),
        }
    }
}

impl<P: PcmSource> AudioFrameSource for SingleStreamFrameSource<P> {
    fn next_frame(&mut self) -> std::io::Result<Option<AudioFrame>> {
        let expected = self.raw.len();
        let n = self.source.read(&mut self.raw)?;
        if n == 0 {
            return Ok(None);
        }
        if n != expected {
            // Partial read — treat as EOF for this period.
            return Ok(None);
        }
        for (i, &s) in self.raw.iter().enumerate() {
            self.out[i] = (s >> 16) as i16;
        }
        Ok(Some(AudioFrame::mic_only(
            self.out.clone(),
            self.channels as u16,
            self.source.sample_rate(),
            self.start.elapsed().as_millis() as u64,
        )))
    }

    fn sample_rate(&self) -> u32 { self.source.sample_rate() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::mock::MockPcmSource;

    #[test]
    fn emits_mic_only_frame() {
        let mock = MockPcmSource::new(2, 48000)
            .with_batch(vec![10 << 16, 99 << 16, 20 << 16, 99 << 16]);
        let mut src = SingleStreamFrameSource::new(mock, 2, 2);
        let frame = src.next_frame().unwrap().expect("frame");
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.reference_channels, 0);
        assert!(frame.reference.is_none());
        assert_eq!(frame.data, vec![10, 99, 20, 99]);
    }

    #[test]
    fn returns_none_on_eof() {
        let mock = MockPcmSource::new(1, 48000);
        let mut src = SingleStreamFrameSource::new(mock, 2, 1);
        assert!(src.next_frame().unwrap().is_none());
    }
}
