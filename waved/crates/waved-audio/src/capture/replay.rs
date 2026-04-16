//! ReplayPcmSource — reads raw interleaved i16 PCM from a file.
//! Used by waved-audio-bench and integration tests on the host;
//! completely off-path on-device.

use super::PcmSource;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

pub struct ReplayPcmSource {
    reader: BufReader<File>,
    channels: usize,
    sample_rate: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum ReplayFormat {
    /// N-channel interleaved i16 (downmixed/mic-only recordings).
    I16Interleaved { channels: usize },
}

// Backwards-compat alias used in tests and existing callers until they migrate.
impl ReplayFormat {
    /// 8-channel interleaved i16 (what our downmixed fixtures/recordings use).
    pub const I16_EIGHT: ReplayFormat = ReplayFormat::I16Interleaved { channels: 8 };

    pub fn channels(&self) -> usize {
        match self {
            ReplayFormat::I16Interleaved { channels } => *channels,
        }
    }
}

impl ReplayPcmSource {
    pub fn open(path: &Path, format: ReplayFormat, sample_rate: u32) -> std::io::Result<Self> {
        let file = File::open(path)?;
        Ok(Self {
            reader: BufReader::new(file),
            channels: format.channels(),
            sample_rate,
        })
    }
}

impl PcmSource for ReplayPcmSource {
    fn read(&mut self, buf: &mut [i32]) -> std::io::Result<usize> {
        // Read i16 samples and widen each to i32 (left-shift 16 to match ALSA
        // S32_LE convention) so downstream S32→S16 conversion recovers the
        // original value without precision loss.
        let mut byte_buf = vec![0u8; buf.len() * 2];
        let mut total = 0;
        while total < byte_buf.len() {
            let n = self.reader.read(&mut byte_buf[total..])?;
            if n == 0 { break; }
            total += n;
        }
        // Align to whole samples (truncate trailing partial sample).
        let whole_samples = total / 2;
        for i in 0..whole_samples {
            let s = i16::from_le_bytes([byte_buf[2*i], byte_buf[2*i+1]]);
            buf[i] = (s as i32) << 16;
        }
        Ok(whole_samples)
    }

    fn channels(&self) -> usize { self.channels }
    fn sample_rate(&self) -> u32 { self.sample_rate }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_pcm_i16(samples: &[i16]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for &s in samples {
            f.write_all(&s.to_le_bytes()).unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[test]
    fn reads_i16_file_and_widens_to_i32() {
        let data: Vec<i16> = vec![1, -1, 100, -100, i16::MAX, i16::MIN];
        let f = write_pcm_i16(&data);
        let mut src = ReplayPcmSource::open(f.path(), ReplayFormat::I16_EIGHT, 48000).unwrap();
        let mut buf = vec![0i32; 6];
        let n = src.read(&mut buf).unwrap();
        assert_eq!(n, 6);
        assert_eq!(buf[0], 1i32 << 16);
        assert_eq!(buf[1], -1i32 << 16);
        assert_eq!(buf[4], (i16::MAX as i32) << 16);
        assert_eq!(buf[5], (i16::MIN as i32) << 16);
    }

    #[test]
    fn returns_zero_at_eof() {
        let f = write_pcm_i16(&[1, 2, 3, 4]);
        let mut src = ReplayPcmSource::open(f.path(), ReplayFormat::I16_EIGHT, 48000).unwrap();
        let mut buf = vec![0i32; 4];
        assert_eq!(src.read(&mut buf).unwrap(), 4);
        assert_eq!(src.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn truncates_trailing_partial_sample() {
        // 5 bytes = 2 full i16 + 1 orphaned byte — should read 2 samples.
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&[0x01, 0x00, 0x02, 0x00, 0xAB]).unwrap();
        f.flush().unwrap();
        let mut src = ReplayPcmSource::open(f.path(), ReplayFormat::I16_EIGHT, 48000).unwrap();
        let mut buf = vec![0i32; 4];
        let n = src.read(&mut buf).unwrap();
        assert_eq!(n, 2);
        assert_eq!(buf[0], 1i32 << 16);
        assert_eq!(buf[1], 2i32 << 16);
    }

    #[test]
    fn i16_interleaved_channels_matches() {
        let fmt = ReplayFormat::I16Interleaved { channels: 4 };
        assert_eq!(fmt.channels(), 4);
    }
}
