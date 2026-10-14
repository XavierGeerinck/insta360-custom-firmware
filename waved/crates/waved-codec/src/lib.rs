//! Recording file formats (docs/AUDIO_HARDWARE.md "Recording formats").
//!
//! New recordings are Ogg Opus (default) or FLAC (a setting), 48 kHz mono.
//! WAV is kept for recordings made before 2026-10-14: they still play,
//! upload and reconcile, and tests use it as a lossless reference.
//!
//! A recording directory holds one `audio.<ext>`, or `audio.<ext>.partial`
//! while the take runs. [`find_audio`] and [`find_partial`] are the one place
//! that knows the names.

mod flac;
mod ogg;
mod opus;
mod reader;
#[doc(hidden)]
pub mod testsignal;
mod wav;
mod writer;

pub use opus::{OPUS_BITRATE_BPS, OPUS_COMPLEXITY, OPUS_FRAME_SAMPLES};
pub use reader::{open_audio, open_reader, read_to_end, PcmReader};
pub use writer::{create_writer, RecordingWriter};

use std::path::{Path, PathBuf};

/// Every recording format runs at 48 kHz, mono.
pub const SAMPLE_RATE: u32 = 48_000;

/// The container and codec of one recording file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioFormat {
    /// PCM S16LE in RIFF/WAVE: recordings made before 2026-10-14.
    Wav,
    /// Ogg Opus (RFC 7845), the default.
    Opus,
    /// FLAC, 16-bit, the lossless setting.
    Flac,
}

impl AudioFormat {
    /// Every format, in the order a directory is searched.
    pub const ALL: [AudioFormat; 3] = [AudioFormat::Wav, AudioFormat::Opus, AudioFormat::Flac];

    pub fn extension(self) -> &'static str {
        match self {
            AudioFormat::Wav => "wav",
            AudioFormat::Opus => "opus",
            AudioFormat::Flac => "flac",
        }
    }

    /// The finalized file in a recording directory.
    pub fn file_name(self) -> &'static str {
        match self {
            AudioFormat::Wav => "audio.wav",
            AudioFormat::Opus => "audio.opus",
            AudioFormat::Flac => "audio.flac",
        }
    }

    /// The file while the take is running, or after it was interrupted.
    pub fn partial_file_name(self) -> &'static str {
        match self {
            AudioFormat::Wav => "audio.wav.partial",
            AudioFormat::Opus => "audio.opus.partial",
            AudioFormat::Flac => "audio.flac.partial",
        }
    }

    /// The MIME type of the webhook's `audio` part (docs/WEBHOOKS.md).
    pub fn content_type(self) -> &'static str {
        match self {
            AudioFormat::Wav => "audio/wav",
            AudioFormat::Opus => "audio/ogg; codecs=opus",
            AudioFormat::Flac => "audio/flac",
        }
    }

    /// `audio.codec` in the webhook metadata.
    pub fn codec(self) -> &'static str {
        match self {
            AudioFormat::Wav => "pcm_s16le",
            AudioFormat::Opus => "opus",
            AudioFormat::Flac => "flac",
        }
    }

    /// The target bitrate of a lossy format.
    pub fn bitrate_bps(self) -> Option<u32> {
        match self {
            AudioFormat::Opus => Some(OPUS_BITRATE_BPS),
            AudioFormat::Wav | AudioFormat::Flac => None,
        }
    }

    /// Bytes one hour of recording takes, for storage estimates. Opus is its
    /// bitrate plus Ogg pages; FLAC is measured on speech with a room noise
    /// floor (`examples/encode_bench.rs`, rounded up); WAV is exact.
    pub fn estimated_bytes_per_hour(self) -> u64 {
        match self {
            AudioFormat::Wav => 345_600_044,
            AudioFormat::Opus => 14_700_000,
            AudioFormat::Flac => 210_000_000,
        }
    }

    /// The format a file name says: `audio.opus`, `audio.flac.partial`,
    /// `<id>.wav`.
    pub fn from_path(path: &Path) -> Option<AudioFormat> {
        let name = path.file_name()?.to_str()?;
        let name = name.strip_suffix(".partial").unwrap_or(name);
        let extension = name.rsplit_once('.')?.1;
        AudioFormat::ALL
            .into_iter()
            .find(|format| format.extension().eq_ignore_ascii_case(extension))
    }
}

/// The finalized audio of a recording directory, whatever its format.
///
/// An empty `audio.opus` is not audio: it is the placeholder the first
/// prototype (2026-03) wrote before it recorded anything, which
/// reconciliation keeps as `legacy_unsupported`. A real Ogg Opus file always
/// starts with its header pages.
pub fn find_audio(dir: &Path) -> Option<(AudioFormat, PathBuf)> {
    AudioFormat::ALL.into_iter().find_map(|format| {
        let path = dir.join(format.file_name());
        let metadata = std::fs::metadata(&path).ok()?;
        let usable = metadata.is_file() && (format != AudioFormat::Opus || metadata.len() > 0);
        usable.then_some((format, path))
    })
}

/// The interrupted (or still running) take of a recording directory.
pub fn find_partial(dir: &Path) -> Option<(AudioFormat, PathBuf)> {
    AudioFormat::ALL.into_iter().find_map(|format| {
        let path = dir.join(format.partial_file_name());
        path.is_file().then_some((format, path))
    })
}

/// The 2026-03 prototype's empty `audio.opus`, with no real audio beside it.
pub fn is_legacy_placeholder(dir: &Path) -> bool {
    std::fs::metadata(dir.join(AudioFormat::Opus.file_name()))
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_format_names_its_files_type_and_codec() {
        let rows = [
            (
                AudioFormat::Wav,
                "wav",
                "audio.wav",
                "audio/wav",
                "pcm_s16le",
            ),
            (
                AudioFormat::Opus,
                "opus",
                "audio.opus",
                "audio/ogg; codecs=opus",
                "opus",
            ),
            (
                AudioFormat::Flac,
                "flac",
                "audio.flac",
                "audio/flac",
                "flac",
            ),
        ];
        for (format, extension, file, content_type, codec) in rows {
            assert_eq!(format.extension(), extension);
            assert_eq!(format.file_name(), file);
            assert_eq!(format.partial_file_name(), format!("{file}.partial"));
            assert_eq!(format.content_type(), content_type);
            assert_eq!(format.codec(), codec);
        }
        assert_eq!(AudioFormat::Opus.bitrate_bps(), Some(32_000));
        assert_eq!(AudioFormat::Flac.bitrate_bps(), None);
    }

    #[test]
    fn the_format_comes_from_the_file_name() {
        for (path, format) in [
            ("/r/x/audio.opus", Some(AudioFormat::Opus)),
            ("audio.flac.partial", Some(AudioFormat::Flac)),
            ("21b17c4c.wav", Some(AudioFormat::Wav)),
            ("AUDIO.FLAC", Some(AudioFormat::Flac)),
            ("metadata.json", None),
            ("audio", None),
        ] {
            assert_eq!(AudioFormat::from_path(Path::new(path)), format, "{path}");
        }
    }

    #[test]
    fn opus_is_a_small_fraction_of_wav_and_flac_sits_between() {
        let opus = AudioFormat::Opus.estimated_bytes_per_hour();
        let flac = AudioFormat::Flac.estimated_bytes_per_hour();
        let wav = AudioFormat::Wav.estimated_bytes_per_hour();
        // 32 kbps for an hour is 14.4 MB before the Ogg pages.
        assert!((14_400_000..15_000_000).contains(&opus));
        assert!(opus < flac && flac < wav);
        assert_eq!(wav, 48_000 * 2 * 3_600 + 44);
    }

    #[test]
    fn the_finder_takes_any_format_and_skips_the_empty_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_audio(dir.path()), None);
        assert_eq!(find_partial(dir.path()), None);

        std::fs::write(dir.path().join("audio.opus"), b"").unwrap();
        assert_eq!(find_audio(dir.path()), None, "the empty prototype file");
        assert!(is_legacy_placeholder(dir.path()));

        std::fs::write(dir.path().join("audio.flac.partial"), b"fLaC").unwrap();
        assert_eq!(
            find_partial(dir.path()),
            Some((AudioFormat::Flac, dir.path().join("audio.flac.partial")))
        );
        assert_eq!(find_audio(dir.path()), None, "a partial is not finalized");

        std::fs::write(dir.path().join("audio.opus"), b"OggS").unwrap();
        assert_eq!(
            find_audio(dir.path()),
            Some((AudioFormat::Opus, dir.path().join("audio.opus")))
        );
        assert!(!is_legacy_placeholder(dir.path()));

        let wav = tempfile::tempdir().unwrap();
        std::fs::write(wav.path().join("audio.wav"), b"RIFF").unwrap();
        assert_eq!(
            find_audio(wav.path()).map(|found| found.0),
            Some(AudioFormat::Wav)
        );
    }
}
