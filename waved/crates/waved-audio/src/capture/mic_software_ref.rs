//! MicWithSoftwareReferenceSource — mic PCM capture paired with a software
//! reference signal streamed from the playback thread.
//!
//! Used on platforms where the hardware loopback PCM isn't viable (see
//! `playback` module docs). Mic is read the same way `SingleStreamFrameSource`
//! reads it; the reference slot of `AudioFrame` is populated with the most
//! recent batch broadcast by `PlaybackService`.
//!
//! Pairing strategy: we take the freshest available reference batch on each
//! `next_frame()`. AEC filters compensate internally for the speaker→mic
//! acoustic delay (typical tap lengths of 100–200ms dwarf any jitter from
//! broadcast buffering). If the broadcast hasn't produced a new batch yet we
//! carry forward the previous one so the reference stream never has holes.

use super::{AudioFrameSource, PcmSource};
use crate::callbacks::AudioFrame;
use std::time::Instant;
use tokio::sync::broadcast;

pub struct MicWithSoftwareReferenceSource<P: PcmSource> {
    mic: P,
    mic_channels: usize,
    reference_channels: usize,
    raw_mic: Vec<i32>,
    out_mic: Vec<i16>,
    ref_silence: Vec<i16>,
    latest_ref: Vec<i16>,
    ref_rx: broadcast::Receiver<Vec<i16>>,
    start: Instant,
}

impl<P: PcmSource> MicWithSoftwareReferenceSource<P> {
    /// `period_frames` must match the mic capture period so raw_mic is sized
    /// correctly. `reference_channels` and `reference_period_samples` describe
    /// the expected shape of batches coming from the playback broadcast —
    /// used only to pre-allocate the silence fallback; mismatched batches
    /// pass through verbatim.
    pub fn new(
        mic: P,
        mic_channels: usize,
        period_frames: usize,
        reference_channels: usize,
        reference_period_samples: usize,
        ref_rx: broadcast::Receiver<Vec<i16>>,
    ) -> Self {
        let ref_silence = vec![0i16; reference_period_samples];
        Self {
            raw_mic: vec![0i32; period_frames * mic_channels],
            out_mic: vec![0i16; period_frames * mic_channels],
            mic,
            mic_channels,
            reference_channels,
            latest_ref: ref_silence.clone(),
            ref_silence,
            ref_rx,
            start: Instant::now(),
        }
    }
}

impl<P: PcmSource> AudioFrameSource for MicWithSoftwareReferenceSource<P> {
    fn next_frame(&mut self) -> std::io::Result<Option<AudioFrame>> {
        let m = self.mic.read(&mut self.raw_mic)?;
        if m == 0 {
            return Ok(None);
        }
        if m != self.raw_mic.len() {
            return Ok(None);
        }

        // Drain the broadcast, keeping only the freshest batch. Lag counts as
        // "caught up"; we're fine pulling the most recent available.
        loop {
            match self.ref_rx.try_recv() {
                Ok(b) => self.latest_ref = b,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => {
                    // Playback thread is gone; fall back to silence from here on.
                    self.latest_ref = self.ref_silence.clone();
                    break;
                }
            }
        }

        for (i, &s) in self.raw_mic.iter().enumerate() {
            self.out_mic[i] = (s >> 16) as i16;
        }

        Ok(Some(AudioFrame::paired(
            self.out_mic.clone(),
            self.mic_channels as u16,
            self.latest_ref.clone(),
            self.reference_channels as u16,
            self.mic.sample_rate(),
            self.start.elapsed().as_millis() as u64,
        )))
    }

    fn sample_rate(&self) -> u32 {
        self.mic.sample_rate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::mock::MockPcmSource;

    #[test]
    fn pairs_mic_with_latest_reference() {
        // 2-channel mic, 1 period = 2 frames, 2-channel reference of the same period.
        let mock_mic = MockPcmSource::new(2, 48000)
            .with_batch(vec![1 << 16, 2 << 16, 3 << 16, 4 << 16]);
        let (tx, rx) = broadcast::channel::<Vec<i16>>(4);
        let mut src = MicWithSoftwareReferenceSource::new(mock_mic, 2, 2, 2, 4, rx);

        // Publish a reference batch before consuming.
        tx.send(vec![10i16, 11, 12, 13]).unwrap();

        let frame = src.next_frame().unwrap().expect("frame");
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.data, vec![1i16, 2, 3, 4]);
        assert_eq!(frame.reference_channels, 2);
        assert_eq!(frame.reference.as_deref(), Some(&[10i16, 11, 12, 13][..]));
    }

    #[test]
    fn carries_forward_previous_ref_when_broadcast_is_empty() {
        let mock_mic = MockPcmSource::new(1, 48000)
            .with_batch(vec![1 << 16, 2 << 16])
            .with_batch(vec![3 << 16, 4 << 16]);
        let (tx, rx) = broadcast::channel::<Vec<i16>>(4);
        let mut src = MicWithSoftwareReferenceSource::new(mock_mic, 1, 2, 2, 4, rx);

        tx.send(vec![7i16, 8, 9, 10]).unwrap();
        // First frame consumes the broadcast.
        let f1 = src.next_frame().unwrap().expect("f1");
        assert_eq!(f1.reference.as_deref(), Some(&[7i16, 8, 9, 10][..]));

        // Second frame has no new broadcast — should carry the same reference.
        let f2 = src.next_frame().unwrap().expect("f2");
        assert_eq!(f2.reference.as_deref(), Some(&[7i16, 8, 9, 10][..]));
    }

    #[test]
    fn falls_back_to_silence_when_playback_closed() {
        let mock_mic = MockPcmSource::new(1, 48000).with_batch(vec![1 << 16, 2 << 16]);
        let (tx, rx) = broadcast::channel::<Vec<i16>>(4);
        drop(tx); // simulate playback thread gone before any broadcast
        let mut src = MicWithSoftwareReferenceSource::new(mock_mic, 1, 2, 2, 4, rx);

        let f = src.next_frame().unwrap().expect("frame");
        assert_eq!(f.reference.as_deref(), Some(&[0i16, 0, 0, 0][..]));
    }

    #[test]
    fn drains_to_the_freshest_reference_when_broadcast_has_backlog() {
        let mock_mic = MockPcmSource::new(1, 48000).with_batch(vec![1 << 16, 2 << 16]);
        let (tx, rx) = broadcast::channel::<Vec<i16>>(4);
        let mut src = MicWithSoftwareReferenceSource::new(mock_mic, 1, 2, 2, 4, rx);

        tx.send(vec![1i16, 2, 3, 4]).unwrap();
        tx.send(vec![5i16, 6, 7, 8]).unwrap();
        tx.send(vec![9i16, 10, 11, 12]).unwrap();
        let f = src.next_frame().unwrap().expect("frame");
        // Should have drained all three and kept the last.
        assert_eq!(f.reference.as_deref(), Some(&[9i16, 10, 11, 12][..]));
    }
}
