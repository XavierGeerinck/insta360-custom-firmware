//! Audio capture: ALSA on device, file replay on host, mock in tests.

#[cfg(test)]
pub mod mock;

#[cfg(feature = "capture-alsa")]
pub mod alsa;

#[cfg(feature = "capture-replay")]
pub mod replay;

pub mod thread;
pub mod single_stream;
pub mod mic_software_ref;

pub mod alsa_dual_stream;

pub use thread::{CaptureStats, run_capture_loop};
pub use single_stream::SingleStreamFrameSource;
pub use mic_software_ref::MicWithSoftwareReferenceSource;

#[cfg(feature = "capture-replay")]
pub use replay::{ReplayFormat, ReplayPcmSource};

/// Indicates whether the channel map (now AudioConfig) was loaded from a file
/// or fell back to the embedded default. Kept here so stats_collector can
/// report the source without depending on the removed ChannelMap type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMapSource {
    File,
    Fallback,
}

/// Produces raw interleaved 32-bit PCM frames from one device.
///
/// Implementations:
/// - `AlsaPcmSource` (device): wraps `snd_pcm_readi`
/// - `ReplayPcmSource` (host): reads from a PCM file
/// - `MockPcmSource` (tests): yields pre-built frame sequences
pub trait PcmSource: Send {
    /// Read raw interleaved samples into `buf`. Returns the number of
    /// samples written (NOT frames — `samples = frames * channels`).
    /// Returns 0 on EOF.
    fn read(&mut self, buf: &mut [i32]) -> std::io::Result<usize>;

    fn channels(&self) -> usize;
    fn sample_rate(&self) -> u32;
}

/// Produces complete `AudioFrame`s. Higher-level than `PcmSource`; consumers
/// of the capture loop work against this.
///
/// Implementations:
/// - `SingleStreamFrameSource<P>` (mic-only replay, mock, tests)
/// - `AlsaDualStreamSource` (on-device, mic + reference)
pub trait AudioFrameSource: Send {
    /// Return the next frame, or `Err` on unrecoverable error.
    /// Implementations handle their own xrun recovery where applicable.
    fn next_frame(&mut self) -> std::io::Result<Option<crate::callbacks::AudioFrame>>;

    fn sample_rate(&self) -> u32;
}
