//! Audio capture: ALSA on device, file replay on host, mock in tests.

pub mod channel_map;

#[cfg(test)]
pub mod mock;

pub use channel_map::{ChannelMap, ChannelMapError, ChannelMapSource};

/// Produces raw interleaved 32-bit PCM frames from some source.
///
/// Implementations:
/// - `AlsaPcmSource` (device): wraps `snd_pcm_readi`
/// - `ReplayPcmSource` (host): reads from a PCM file
/// - `MockPcmSource` (tests): yields pre-built frame sequences
pub trait PcmSource: Send {
    /// Read raw interleaved samples into `buf`. Returns the number of samples
    /// written (NOT frames — `samples = frames * channels`). Returns 0 on EOF.
    ///
    /// Implementations may block (ALSA) or return transient errors (xrun),
    /// which the caller decides how to handle.
    fn read(&mut self, buf: &mut [i32]) -> std::io::Result<usize>;

    /// Number of interleaved channels per frame (typically 10).
    fn channels(&self) -> usize;

    /// Sample rate in Hz.
    fn sample_rate(&self) -> u32;
}

/// Downmix 10-channel i32 interleaved input to 8-channel i16 interleaved,
/// keeping only the physical mic channels (references dropped), in the order
/// given by `map.physical_mics`.
///
/// `input.len() / map.total_channels` frames are read; `out.len()` must be
/// `frames * map.physical_mics.len()`.
pub fn downmix_interleaved(
    input: &[i32],
    out: &mut [i16],
    map: &ChannelMap,
) -> Result<(), &'static str> {
    let total = map.total_channels as usize;
    let mics = map.physical_mics.len();
    if total == 0 || mics == 0 { return Err("empty channel map"); }
    if input.len() % total != 0 { return Err("input length not multiple of total_channels"); }
    let frames = input.len() / total;
    if out.len() != frames * mics {
        return Err("output length does not match frames * physical_mics");
    }
    for f in 0..frames {
        for (mi, &src_idx) in map.physical_mics.iter().enumerate() {
            let src = input[f * total + src_idx as usize];
            // S32 → S16: shift right 16 bits (ALSA stores 32-bit right-justified samples).
            out[f * mics + mi] = (src >> 16) as i16;
        }
    }
    Ok(())
}

#[cfg(test)]
mod downmix_tests {
    use super::*;

    #[test]
    fn downmix_drops_reference_channels() {
        let map = ChannelMap::from_json(
            r#"{"version":1,"total_channels":4,"physical_mics":[0,2],"reference_channels":[1,3]}"#
        ).unwrap();
        // 2 frames, 4 channels each. Channel 0 = 1<<16, 1 = 2<<16, 2 = 3<<16, 3 = 4<<16
        let input: Vec<i32> = vec![
            1 << 16, 2 << 16, 3 << 16, 4 << 16,
            5 << 16, 6 << 16, 7 << 16, 8 << 16,
        ];
        let mut out = vec![0i16; 2 * 2]; // 2 frames * 2 mics
        downmix_interleaved(&input, &mut out, &map).unwrap();
        // ch0 kept, ch2 kept (in that order)
        assert_eq!(out, vec![1, 3, 5, 7]);
    }

    #[test]
    fn downmix_reorders_per_map() {
        let map = ChannelMap::from_json(
            r#"{"version":1,"total_channels":3,"physical_mics":[2,0,1],"reference_channels":[]}"#
        ).unwrap();
        let input: Vec<i32> = vec![10 << 16, 20 << 16, 30 << 16];
        let mut out = vec![0i16; 3];
        downmix_interleaved(&input, &mut out, &map).unwrap();
        assert_eq!(out, vec![30, 10, 20]);
    }

    #[test]
    fn downmix_rejects_misaligned_input() {
        let map = ChannelMap::default_embedded();
        let input = vec![0i32; 11]; // not a multiple of 10
        let mut out = vec![0i16; 8];
        assert!(downmix_interleaved(&input, &mut out, &map).is_err());
    }

    #[test]
    fn downmix_rejects_wrong_output_size() {
        let map = ChannelMap::default_embedded();
        let input = vec![0i32; 10];
        let mut out = vec![0i16; 7];
        assert!(downmix_interleaved(&input, &mut out, &map).is_err());
    }
}
