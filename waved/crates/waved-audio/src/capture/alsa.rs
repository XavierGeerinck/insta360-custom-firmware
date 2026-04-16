//! AlsaPcmSource — wraps `alsa::pcm::PCM` for 10-ch / 48 kHz / S32_LE capture
//! from pcmC0D0c. Lives behind the `capture-alsa` feature flag so host builds
//! don't pull in libasound.

use super::PcmSource;
use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use std::io::{Error, ErrorKind};
use tracing::{info, warn};

pub struct AlsaPcmSource {
    pcm: PCM,
    channels: u32,
    sample_rate: u32,
}

impl AlsaPcmSource {
    /// Open and configure a capture stream. `hw_id` is e.g. "hw:0,0".
    pub fn open(
        hw_id: &str,
        sample_rate: u32,
        channels: u32,
        period_frames: u32,
        periods: u32,
    ) -> std::io::Result<Self> {
        let pcm = PCM::new(hw_id, Direction::Capture, false)
            .map_err(|e| Error::new(ErrorKind::Other, format!("pcm open: {e}")))?;

        {
            let hwp = HwParams::any(&pcm)
                .map_err(|e| Error::new(ErrorKind::Other, format!("hw params: {e}")))?;
            hwp.set_channels(channels)
                .map_err(|e| Error::new(ErrorKind::Other, format!("set channels: {e}")))?;
            hwp.set_rate(sample_rate, ValueOr::Nearest)
                .map_err(|e| Error::new(ErrorKind::Other, format!("set rate: {e}")))?;
            hwp.set_format(Format::s32())
                .map_err(|e| Error::new(ErrorKind::Other, format!("set format: {e}")))?;
            hwp.set_access(Access::RWInterleaved)
                .map_err(|e| Error::new(ErrorKind::Other, format!("set access: {e}")))?;
            hwp.set_period_size(period_frames as alsa::pcm::Frames, ValueOr::Nearest)
                .map_err(|e| Error::new(ErrorKind::Other, format!("set period: {e}")))?;
            hwp.set_buffer_size((period_frames * periods) as alsa::pcm::Frames)
                .map_err(|e| Error::new(ErrorKind::Other, format!("set buffer: {e}")))?;
            pcm.hw_params(&hwp)
                .map_err(|e| Error::new(ErrorKind::Other, format!("apply hw params: {e}")))?;

            let actual_rate = hwp
                .get_rate()
                .map_err(|e| Error::new(ErrorKind::Other, format!("get rate: {e}")))?;
            let actual_period = hwp
                .get_period_size()
                .map_err(|e| Error::new(ErrorKind::Other, format!("get period: {e}")))?;
            let actual_buffer = hwp
                .get_buffer_size()
                .map_err(|e| Error::new(ErrorKind::Other, format!("get buffer: {e}")))?;
            info!(
                "ALSA capture opened: {hw_id} rate={actual_rate} period={actual_period} buffer={actual_buffer}"
            );
            if actual_rate != sample_rate {
                warn!("requested {sample_rate} Hz, got {actual_rate} Hz");
            }
        }

        pcm.prepare()
            .map_err(|e| Error::new(ErrorKind::Other, format!("prepare: {e}")))?;
        pcm.start()
            .map_err(|e| Error::new(ErrorKind::Other, format!("start: {e}")))?;

        Ok(Self {
            pcm,
            channels,
            sample_rate,
        })
    }

    fn recover(&self, err: &alsa::Error) -> std::io::Result<()> {
        if err.errno() == libc::EPIPE {
            self.pcm.prepare().map_err(|e| {
                Error::new(ErrorKind::Other, format!("prepare after xrun: {e}"))
            })?;
            return Err(Error::from(ErrorKind::BrokenPipe));
        }
        Err(Error::new(ErrorKind::Other, format!("alsa: {err}")))
    }
}

impl PcmSource for AlsaPcmSource {
    fn read(&mut self, buf: &mut [i32]) -> std::io::Result<usize> {
        let io = self
            .pcm
            .io_i32()
            .map_err(|e| Error::new(ErrorKind::Other, format!("io_i32: {e}")))?;
        let channels = self.channels as usize;
        let frames = buf.len() / channels;
        let result = io.readi(&mut buf[..frames * channels]);
        drop(io);
        match result {
            Ok(got) => Ok(got * channels),
            Err(e) => self.recover(&e).map(|_| 0),
        }
    }

    fn channels(&self) -> usize {
        self.channels as usize
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

// No unit tests: real ALSA requires hardware. Error-mapping logic is the
// only testable piece; covered implicitly by the xrun test in thread.rs
// via MockPcmSource returning BrokenPipe.
