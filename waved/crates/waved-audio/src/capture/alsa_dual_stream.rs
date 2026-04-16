//! DualStreamSource<M, R> — pairs mic + reference PCM frames, tagging each
//! pair with a monotonic timestamp. ALSA-flavoured type alias
//! (`AlsaDualStreamSource`) tries `snd_pcm_link` at startup; on failure, the
//! generic aligner emits on best-effort timestamp pairing.

use super::{AudioFrameSource, PcmSource};
use crate::callbacks::AudioFrame;
use std::time::Instant;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignmentMethod {
    KernelLinked,       // snd_pcm_link honored
    TimestampFallback,  // per-thread timestamps
}

impl AlignmentMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            AlignmentMethod::KernelLinked => "kernel-linked",
            AlignmentMethod::TimestampFallback => "timestamp-fallback",
        }
    }
}

pub struct DualStreamSource<M: PcmSource, R: PcmSource> {
    mic: M,
    mic_channels: usize,
    reference: R,
    reference_channels: usize,
    raw_mic: Vec<i32>,
    raw_ref: Vec<i32>,
    out_mic: Vec<i16>,
    out_ref: Vec<i16>,
    alignment: AlignmentMethod,
    start: Instant,
}

impl<M: PcmSource, R: PcmSource> DualStreamSource<M, R> {
    pub fn new(
        mic: M,
        mic_channels: usize,
        reference: R,
        reference_channels: usize,
        period_frames: usize,
        alignment: AlignmentMethod,
    ) -> Self {
        Self {
            raw_mic: vec![0i32; period_frames * mic_channels],
            raw_ref: vec![0i32; period_frames * reference_channels],
            out_mic: vec![0i16; period_frames * mic_channels],
            out_ref: vec![0i16; period_frames * reference_channels],
            mic,
            mic_channels,
            reference,
            reference_channels,
            alignment,
            start: Instant::now(),
        }
    }

    pub fn alignment(&self) -> AlignmentMethod { self.alignment }
}

impl<M: PcmSource, R: PcmSource> AudioFrameSource for DualStreamSource<M, R> {
    fn next_frame(&mut self) -> std::io::Result<Option<AudioFrame>> {
        // Read mic first (determines cadence); then reference.
        let m = self.mic.read(&mut self.raw_mic)?;
        if m == 0 { return Ok(None); }
        if m != self.raw_mic.len() { return Ok(None); }
        let r = self.reference.read(&mut self.raw_ref)?;
        if r == 0 { return Ok(None); }
        if r != self.raw_ref.len() { return Ok(None); }

        for (i, &s) in self.raw_mic.iter().enumerate() { self.out_mic[i] = (s >> 16) as i16; }
        for (i, &s) in self.raw_ref.iter().enumerate() { self.out_ref[i] = (s >> 16) as i16; }

        Ok(Some(AudioFrame::paired(
            self.out_mic.clone(),
            self.mic_channels as u16,
            self.out_ref.clone(),
            self.reference_channels as u16,
            self.mic.sample_rate(),
            self.start.elapsed().as_millis() as u64,
        )))
    }

    fn sample_rate(&self) -> u32 { self.mic.sample_rate() }
}

#[cfg(feature = "capture-alsa")]
pub use alsa_impl::AlsaDualStreamSource;

#[cfg(feature = "capture-alsa")]
mod alsa_impl {
    use super::*;
    use crate::audio_config::CaptureConfig;
    use crate::capture::alsa::{AlsaOpenError, AlsaPcmSource, AlsaStreamConfig};

    pub type AlsaDualStreamSource = DualStreamSource<AlsaPcmSource, AlsaPcmSource>;

    impl AlsaDualStreamSource {
        pub fn from_capture_config(
            cfg: &CaptureConfig,
            period_frames: usize,
            buffer_periods: u32,
        ) -> Result<Self, AlsaOpenError> {
            let mic = AlsaPcmSource::open(&AlsaStreamConfig {
                device: &cfg.mic_device,
                channels: cfg.mic_channels,
                sample_rate: cfg.mic_sample_rate,
                format: cfg.mic_format,
                period_frames,
                buffer_periods,
            })?;
            let ref_dev = cfg.reference_device.as_deref().ok_or_else(|| {
                AlsaOpenError::Mismatch {
                    device: "(reference)".into(),
                    diagnosis: "reference_device missing in config but DualStream requested".into(),
                }
            })?;
            let reference = AlsaPcmSource::open(&AlsaStreamConfig {
                device: ref_dev,
                channels: cfg.reference_channels,
                sample_rate: cfg.reference_sample_rate,
                format: cfg.reference_format.expect("validated present"),
                period_frames,
                buffer_periods,
            })?;

            // Try snd_pcm_link via the alsa crate.
            let alignment = match mic.as_pcm().link(reference.as_pcm()) {
                Ok(()) => {
                    info!("alignment: snd_pcm_link honored");
                    AlignmentMethod::KernelLinked
                }
                Err(e) => {
                    warn!("snd_pcm_link failed ({e}) — falling back to timestamp alignment");
                    AlignmentMethod::TimestampFallback
                }
            };

            Ok(DualStreamSource::new(
                mic,
                cfg.mic_channels as usize,
                reference,
                cfg.reference_channels as usize,
                period_frames,
                alignment,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::mock::MockPcmSource;

    #[test]
    fn paired_frames_populated() {
        let mic = MockPcmSource::new(2, 48000)
            .with_batch(vec![1 << 16, 2 << 16, 3 << 16, 4 << 16]);
        let refr = MockPcmSource::new(1, 48000)
            .with_batch(vec![9 << 16, 8 << 16]);
        let mut src = DualStreamSource::new(mic, 2, refr, 1, 2, AlignmentMethod::TimestampFallback);
        let frame = src.next_frame().unwrap().expect("frame");
        assert_eq!(frame.data, vec![1, 2, 3, 4]);
        assert_eq!(frame.reference.as_deref(), Some(&[9, 8][..]));
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.reference_channels, 1);
    }

    #[test]
    fn returns_none_on_mic_eof() {
        let mic = MockPcmSource::new(1, 48000); // no batches → EOF
        let refr = MockPcmSource::new(1, 48000)
            .with_batch(vec![1 << 16, 2 << 16]);
        let mut src = DualStreamSource::new(mic, 1, refr, 1, 2, AlignmentMethod::TimestampFallback);
        assert!(src.next_frame().unwrap().is_none());
    }

    #[test]
    fn alignment_method_is_exposed() {
        let mic = MockPcmSource::new(1, 48000);
        let refr = MockPcmSource::new(1, 48000);
        let src = DualStreamSource::new(mic, 1, refr, 1, 2, AlignmentMethod::KernelLinked);
        assert_eq!(src.alignment(), AlignmentMethod::KernelLinked);
    }
}
