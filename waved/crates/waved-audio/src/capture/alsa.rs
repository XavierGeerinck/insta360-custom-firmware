//! AlsaPcmSource: opens one ALSA PCM capture device, validates requested
//! parameters against the hardware's supported ranges, and streams raw i32
//! samples into a buffer. Recovers from xruns via snd_pcm_prepare.

use super::PcmSource;
use crate::audio_config::SampleFormat;
use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use thiserror::Error;
use tracing::info;

pub struct AlsaStreamConfig<'a> {
    pub device: &'a str,
    pub channels: u32,
    pub sample_rate: u32,
    pub format: SampleFormat,
    pub period_frames: usize,
    pub buffer_periods: u32,
}

#[derive(Debug, Error)]
pub enum AlsaOpenError {
    #[error("alsa error: {0}")]
    Alsa(#[from] alsa::Error),
    #[error("hardware/config mismatch on {device}:\n{diagnosis}")]
    Mismatch { device: String, diagnosis: String },
}

pub struct AlsaPcmSource {
    pcm: PCM,
    channels: usize,
    sample_rate: u32,
}

impl AlsaPcmSource {
    pub fn open(cfg: &AlsaStreamConfig<'_>) -> Result<Self, AlsaOpenError> {
        let pcm = PCM::new(cfg.device, Direction::Capture, false)?;
        probe_and_set(&pcm, cfg)?;
        info!(
            "alsa capture opened: device={} ch={} rate={} period={} buffer={}",
            cfg.device, cfg.channels, cfg.sample_rate, cfg.period_frames,
            cfg.period_frames * cfg.buffer_periods as usize
        );
        Ok(Self {
            pcm,
            channels: cfg.channels as usize,
            sample_rate: cfg.sample_rate,
        })
    }

    /// Access the inner PCM for advanced operations (e.g., snd_pcm_link).
    pub fn as_pcm(&self) -> &PCM { &self.pcm }
}

fn probe_and_set(pcm: &PCM, cfg: &AlsaStreamConfig<'_>) -> Result<(), AlsaOpenError> {
    let hwp = HwParams::any(pcm)?;

    let ch_min = hwp.get_channels_min()?;
    let ch_max = hwp.get_channels_max()?;
    let rate_min = hwp.get_rate_min()?;
    let rate_max = hwp.get_rate_max()?;

    let fmt = match cfg.format {
        SampleFormat::S16_LE => Format::s16(),
        SampleFormat::S24_LE => Format::s24_3(),
        SampleFormat::S32_LE => Format::s32(),
    };
    let format_ok = hwp.test_format(fmt).is_ok();

    let ch_in_range = (ch_min..=ch_max).contains(&cfg.channels);
    let rate_in_range = (rate_min..=rate_max).contains(&cfg.sample_rate);

    if !(ch_in_range && rate_in_range && format_ok) {
        let diagnosis = format!(
            "  channels: config={} supported=[{}..{}] {}\n  rate:     config={} supported=[{}..{}] {}\n  format:   config={:?} {}",
            cfg.channels, ch_min, ch_max, if ch_in_range { "OK" } else { "MISMATCH" },
            cfg.sample_rate, rate_min, rate_max, if rate_in_range { "OK" } else { "MISMATCH" },
            cfg.format, if format_ok { "OK" } else { "MISMATCH" },
        );
        return Err(AlsaOpenError::Mismatch {
            device: cfg.device.to_string(),
            diagnosis,
        });
    }

    hwp.set_access(Access::RWInterleaved)?;
    hwp.set_format(fmt)?;
    hwp.set_channels(cfg.channels)?;
    hwp.set_rate(cfg.sample_rate, ValueOr::Nearest)?;
    hwp.set_period_size(cfg.period_frames as i64, ValueOr::Nearest)?;
    hwp.set_buffer_size((cfg.period_frames * cfg.buffer_periods as usize) as i64)?;
    pcm.hw_params(&hwp)?;
    pcm.prepare()?;
    Ok(())
}

impl PcmSource for AlsaPcmSource {
    fn read(&mut self, buf: &mut [i32]) -> std::io::Result<usize> {
        let io = self.pcm.io_i32().map_err(std::io::Error::other)?;
        match io.readi(buf) {
            Ok(frames) => Ok(frames * self.channels),
            Err(e) => {
                if e.errno() == libc::EPIPE {
                    drop(io);
                    let _ = self.pcm.prepare();
                    return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, e));
                }
                Err(std::io::Error::other(e))
            }
        }
    }

    fn channels(&self) -> usize { self.channels }
    fn sample_rate(&self) -> u32 { self.sample_rate }
}
