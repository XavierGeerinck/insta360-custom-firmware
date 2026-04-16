//! Synthetic fixture generators — produce deterministic interleaved i16 PCM
//! for tests and manual diagnostics. No file I/O; callers can write the
//! result to disk if needed.

const SAMPLE_RATE: u32 = 48_000;

/// 8-channel silence for `seconds` seconds. Useful baseline.
pub fn silence_8ch(seconds: u32) -> Vec<i16> {
    vec![0i16; (SAMPLE_RATE * seconds) as usize * 8]
}

/// 8-channel output: `tone_channel` carries a sine at `freq_hz`, all other
/// channels are silent. `amplitude` in [0, i16::MAX].
pub fn tone_mono_8ch(seconds: u32, freq_hz: f32, tone_channel: usize, amplitude: i16) -> Vec<i16> {
    assert!(tone_channel < 8, "tone_channel must be 0..8");
    let n = (SAMPLE_RATE * seconds) as usize;
    let mut out = vec![0i16; n * 8];
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;
        let s = (2.0 * std::f32::consts::PI * freq_hz * t).sin();
        out[i * 8 + tone_channel] = (s * amplitude as f32) as i16;
    }
    out
}

/// 8-channel i16::MAX on every sample — clipping test input.
pub fn saturated_8ch(seconds: u32) -> Vec<i16> {
    vec![i16::MAX; (SAMPLE_RATE * seconds) as usize * 8]
}

/// A 20-ms 5 kHz burst on channel 0, silence elsewhere. Reference for the
/// latency probe: short enough to localize precisely, loud enough to survive
/// a noisy mic.
pub fn chirp_5khz_burst(total_seconds: u32) -> Vec<i16> {
    let n = (SAMPLE_RATE * total_seconds) as usize;
    let burst_samples = (SAMPLE_RATE as f32 * 0.020) as usize; // 20ms
    let mut out = vec![0i16; n * 8];
    for i in 0..burst_samples.min(n) {
        let t = i as f32 / SAMPLE_RATE as f32;
        let s = (2.0 * std::f32::consts::PI * 5000.0 * t).sin();
        out[i * 8 + 0] = (s * 20000.0) as i16;
    }
    out
}

/// 8-channel: ch 0 and ch 1 both carry an identical tone (correlation ≈ 1.0
/// between them), others silent. Used to validate correlation_matrix.
pub fn correlation_pair_8ch(seconds: u32, freq_hz: f32, amplitude: i16) -> Vec<i16> {
    let n = (SAMPLE_RATE * seconds) as usize;
    let mut out = vec![0i16; n * 8];
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;
        let s = (2.0 * std::f32::consts::PI * freq_hz * t).sin();
        let v = (s * amplitude as f32) as i16;
        out[i * 8 + 0] = v;
        out[i * 8 + 1] = v;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{rms_per_channel, clipping_rate, correlation_matrix};

    #[test]
    fn silence_is_actually_silent() {
        let data = silence_8ch(1);
        let rms = rms_per_channel(&data, 8);
        assert!(rms.iter().all(|&r| r == 0.0));
    }

    #[test]
    fn tone_only_on_target_channel() {
        let data = tone_mono_8ch(1, 1000.0, 3, 10_000);
        let rms = rms_per_channel(&data, 8);
        assert!(rms[3] > 1000.0, "tone channel should have significant RMS, got {}", rms[3]);
        for (i, &r) in rms.iter().enumerate() {
            if i != 3 {
                assert_eq!(r, 0.0, "ch {i} should be silent");
            }
        }
    }

    #[test]
    fn saturated_has_full_clipping_rate() {
        let data = saturated_8ch(1);
        let r = clipping_rate(&data, i16::MAX);
        assert!((r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn chirp_is_on_channel_zero() {
        let data = chirp_5khz_burst(1);
        let rms = rms_per_channel(&data, 8);
        assert!(rms[0] > 1000.0);
        for &r in &rms[1..] {
            assert_eq!(r, 0.0);
        }
    }

    #[test]
    fn correlation_pair_is_correlated() {
        let data = correlation_pair_8ch(1, 1000.0, 10_000);
        let m = correlation_matrix(&data, 8);
        assert!((m[0][1] - 1.0).abs() < 1e-3, "ch0<->ch1 should be ~1.0, got {}", m[0][1]);
        assert_eq!(m[0][2], 0.0, "ch0<->ch2 (silent) should be 0.0");
    }

    #[test]
    fn generators_are_deterministic() {
        assert_eq!(silence_8ch(1), silence_8ch(1));
        assert_eq!(tone_mono_8ch(1, 440.0, 0, 5000), tone_mono_8ch(1, 440.0, 0, 5000));
    }
}
