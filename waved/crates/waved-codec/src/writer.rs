//! The recorder writes PCM into a [`RecordingWriter`]; the format behind it
//! is chosen per take.

use crate::flac::FlacWriter;
use crate::opus::OpusWriter;
use crate::wav::WavWriter;
use crate::AudioFormat;
use std::fs::File;
use std::io::{self, BufWriter};

/// Bytes buffered before a write reaches the file. The writers push their
/// buffer out at every Ogg page / every second of FLAC frames anyway.
const FILE_BUFFER_BYTES: usize = 64 * 1024;

/// Encodes 48 kHz mono S16 samples into one recording file.
///
/// Crash safety: each writer hands complete Ogg pages / FLAC frames / WAV
/// data to the file at least once per second of audio, so a process kill
/// leaves a file that plays up to that point. Syncing to the medium (power
/// loss) is the recorder's job.
pub trait RecordingWriter: Send {
    fn format(&self) -> AudioFormat;

    /// Encode `samples` and append whatever is complete.
    fn write(&mut self, samples: &[i16]) -> io::Result<()>;

    /// Push everything complete to the file now. Up to one codec frame (and
    /// for Opus the newest packet, which the end trimming needs) stays in
    /// memory until [`RecordingWriter::finish`].
    fn flush(&mut self) -> io::Result<()>;

    /// Encode the rest, close the stream (Ogg end-of-stream page, FLAC
    /// STREAMINFO totals, WAV sizes) and flush. Returns the samples in the
    /// file. A second call does nothing and returns the same count.
    fn finish(&mut self) -> io::Result<u64>;
}

/// A writer for a new, empty `file` (the take's `.partial`). The stream
/// header is on the file when this returns.
pub fn create_writer(format: AudioFormat, file: File) -> io::Result<Box<dyn RecordingWriter>> {
    let out = BufWriter::with_capacity(FILE_BUFFER_BYTES, file);
    Ok(match format {
        AudioFormat::Wav => Box::new(WavWriter::new(out)?),
        AudioFormat::Opus => Box::new(OpusWriter::new(out)?),
        AudioFormat::Flac => Box::new(FlacWriter::new(out)?),
    })
}
