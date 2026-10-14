//! WAV, PCM S16LE 48 kHz mono: recordings made before 2026-10-14, and the
//! lossless reference in tests.

use crate::reader::PcmReader;
use crate::writer::RecordingWriter;
use crate::AudioFormat;
use std::io::{self, Read, Seek, Write};

/// Samples between two header updates: one second.
const FLUSH_SAMPLES: u64 = crate::SAMPLE_RATE as u64;

fn spec() -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate: crate::SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

fn to_io(error: hound::Error) -> io::Error {
    match error {
        hound::Error::IoError(error) => error,
        other => io::Error::new(io::ErrorKind::InvalidData, other),
    }
}

/// `hound`'s writer, with the RIFF sizes brought up to date every second so a
/// killed take still reads to that point.
pub(crate) struct WavWriter<W: Write + Seek> {
    writer: Option<hound::WavWriter<W>>,
    written: u64,
    unflushed: u64,
}

impl<W: Write + Seek> WavWriter<W> {
    pub(crate) fn new(out: W) -> io::Result<Self> {
        let writer = hound::WavWriter::new(out, spec()).map_err(to_io)?;
        Ok(Self {
            writer: Some(writer),
            written: 0,
            unflushed: 0,
        })
    }
}

impl<W: Write + Seek + Send> RecordingWriter for WavWriter<W> {
    fn format(&self) -> AudioFormat {
        AudioFormat::Wav
    }

    fn write(&mut self, samples: &[i16]) -> io::Result<()> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| io::Error::other("wav stream already finished"))?;
        let mut samples_writer = writer.get_i16_writer(samples.len() as u32);
        for &sample in samples {
            samples_writer.write_sample(sample);
        }
        samples_writer.flush().map_err(to_io)?;
        self.written += samples.len() as u64;
        self.unflushed += samples.len() as u64;
        if self.unflushed >= FLUSH_SAMPLES {
            writer.flush().map_err(to_io)?;
            self.unflushed = 0;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.unflushed = 0;
        match self.writer.as_mut() {
            Some(writer) => writer.flush().map_err(to_io),
            None => Ok(()),
        }
    }

    fn finish(&mut self) -> io::Result<u64> {
        if let Some(writer) = self.writer.take() {
            writer.finalize().map_err(to_io)?;
        }
        Ok(self.written)
    }
}

/// `hound`'s reader for 48 kHz mono 16-bit files.
pub(crate) struct WavReader<R: Read> {
    reader: hound::WavReader<R>,
    total: u64,
    position: u64,
}

impl<R: Read + Seek> WavReader<R> {
    pub(crate) fn new(inner: R) -> io::Result<Self> {
        let reader = hound::WavReader::new(inner).map_err(to_io)?;
        let found = reader.spec();
        if (found.channels, found.sample_rate, found.bits_per_sample) != (1, crate::SAMPLE_RATE, 16)
            || found.sample_format != hound::SampleFormat::Int
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported WAV format {found:?}"),
            ));
        }
        Ok(Self {
            total: u64::from(reader.duration()),
            reader,
            position: 0,
        })
    }
}

impl<R: Read + Seek + Send> PcmReader for WavReader<R> {
    fn format(&self) -> AudioFormat {
        AudioFormat::Wav
    }

    fn total_samples(&self) -> u64 {
        self.total
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn seek(&mut self, sample: u64) -> io::Result<()> {
        let target = sample.min(self.total);
        self.reader.seek(target as u32)?;
        self.position = target;
        Ok(())
    }

    fn read(&mut self, out: &mut [i16]) -> io::Result<usize> {
        let mut filled = 0;
        for (slot, sample) in out.iter_mut().zip(self.reader.samples::<i16>()) {
            *slot = sample.map_err(to_io)?;
            filled += 1;
        }
        self.position += filled as u64;
        Ok(filled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::read_to_end;
    use crate::testsignal::speech_like;
    use std::fs::File;
    use std::io::Cursor;

    #[test]
    fn wav_round_trips_and_seeks_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav");
        let samples = speech_like(1.3, 21);
        let mut writer = WavWriter::new(File::create(&path).unwrap()).unwrap();
        writer.write(&samples).unwrap();
        assert_eq!(writer.finish().unwrap(), samples.len() as u64);
        assert_eq!(writer.finish().unwrap(), samples.len() as u64);
        let spec = hound::WavReader::open(&path).unwrap().spec();
        assert_eq!(
            (spec.channels, spec.sample_rate, spec.bits_per_sample),
            (1, 48_000, 16)
        );
        let mut reader = WavReader::new(File::open(&path).unwrap()).unwrap();
        assert_eq!(reader.total_samples(), samples.len() as u64);
        assert_eq!(read_to_end(&mut reader).unwrap(), samples);
        reader.seek(30_000).unwrap();
        let mut chunk = [0i16; 100];
        assert_eq!(reader.read(&mut chunk).unwrap(), 100);
        assert_eq!(&chunk[..], &samples[30_000..30_100]);
        reader.seek(u64::MAX).unwrap();
        assert_eq!(reader.position(), samples.len() as u64);
    }

    #[test]
    fn the_header_is_current_every_second() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav.partial");
        let samples = speech_like(2.5, 22);
        let mut writer = WavWriter::new(File::create(&path).unwrap()).unwrap();
        for chunk in samples.chunks(4_800) {
            writer.write(chunk).unwrap();
        }
        // The header was updated at 1 s and 2 s; a kill now keeps 2 s.
        assert_eq!(hound::WavReader::open(&path).unwrap().duration(), 96_000);
        drop(writer);
    }

    #[test]
    fn other_wav_layouts_are_refused() {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut bytes = Cursor::new(Vec::new());
        hound::WavWriter::new(&mut bytes, spec)
            .unwrap()
            .finalize()
            .unwrap();
        let error = WavReader::new(Cursor::new(bytes.into_inner()))
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
