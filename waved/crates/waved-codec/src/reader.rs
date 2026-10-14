//! Decoding recordings for playback on the Wave (Files → play).

use crate::flac::FlacReader;
use crate::opus::OpusReader;
use crate::wav::WavReader;
use crate::{find_audio, AudioFormat};
use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;

/// 48 kHz mono S16 out of one recording file, with sample-accurate seeking.
pub trait PcmReader: Send {
    fn format(&self) -> AudioFormat;

    /// Samples in the file. For a file cut by a crash: the samples up to its
    /// last complete Ogg page or FLAC frame.
    fn total_samples(&self) -> u64;

    /// The sample the next [`PcmReader::read`] starts at.
    fn position(&self) -> u64;

    /// Move to `sample`, clamped to the end.
    fn seek(&mut self, sample: u64) -> io::Result<()>;

    /// Fill `out` from the current position; fewer samples (0) at the end.
    fn read(&mut self, out: &mut [i16]) -> io::Result<usize>;
}

/// Open `path`, written in `format`.
pub fn open_reader(format: AudioFormat, path: &Path) -> io::Result<Box<dyn PcmReader>> {
    let file = File::open(path)?;
    Ok(match format {
        // hound reads two bytes at a time; the others buffer themselves.
        AudioFormat::Wav => Box::new(WavReader::new(BufReader::new(file))?),
        AudioFormat::Opus => Box::new(OpusReader::new(file)?),
        AudioFormat::Flac => Box::new(FlacReader::new(file)?),
    })
}

/// Open the finalized audio of a recording directory, whatever its format.
pub fn open_audio(dir: &Path) -> io::Result<Box<dyn PcmReader>> {
    let (format, path) = find_audio(dir).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no recording audio in {}", dir.display()),
        )
    })?;
    open_reader(format, &path)
}

/// Read everything from the current position to the end.
pub fn read_to_end(reader: &mut dyn PcmReader) -> io::Result<Vec<i16>> {
    let mut all =
        Vec::with_capacity(reader.total_samples().saturating_sub(reader.position()) as usize);
    let mut chunk = vec![0i16; 4_096];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            return Ok(all);
        }
        all.extend_from_slice(&chunk[..read]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsignal::speech_like;
    use crate::writer::create_writer;

    fn record(dir: &Path, format: AudioFormat, samples: &[i16]) {
        let file = File::create(dir.join(format.file_name())).unwrap();
        let mut writer = create_writer(format, file).unwrap();
        writer.write(samples).unwrap();
        assert_eq!(writer.finish().unwrap(), samples.len() as u64);
    }

    #[test]
    fn a_directory_opens_whatever_format_it_holds() {
        let samples = speech_like(1.5, 3);
        for format in AudioFormat::ALL {
            let dir = tempfile::tempdir().unwrap();
            record(dir.path(), format, &samples);
            let mut reader = open_audio(dir.path()).unwrap();
            assert_eq!(reader.format(), format);
            assert_eq!(reader.total_samples(), samples.len() as u64, "{format:?}");
            let decoded = read_to_end(reader.as_mut()).unwrap();
            assert_eq!(decoded.len(), samples.len(), "{format:?}");
            assert_eq!(reader.position(), samples.len() as u64);
        }
    }

    #[test]
    fn a_directory_without_audio_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let error = open_audio(dir.path()).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
