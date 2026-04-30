//! Magnitude-spectrum binning for the recording-screen pill.
//!
//! Turns a block of mono i16 audio into `FREQ_BAND_COUNT` log-spaced
//! magnitude bins suitable for driving a per-bar visualisation.
//!
//! ## Pipeline
//!
//! 1. Mix multi-channel input down to mono i16 → cast to f32 in [-1, 1].
//! 2. Apply a Hann window (reduces spectral leakage so a bin reflects
//!    its frequency rather than smearing across neighbours).
//! 3. Forward FFT (rustfft, complex-in/complex-out — imaginary part of
//!    the input is zero-padded). 1024-pt FFT → 512 useful bins.
//! 4. Log-spaced bin grouping over 100 Hz – 8 kHz (the band where speech
//!    sits). Each output band is the max of its FFT bin range —
//!    perceptually closer to "is anything happening here" than mean.
//! 5. Soft-normalise to [0, 1] using a fixed reference. Saturating clamp.
//!
//! The same `Spectrum` instance can be reused per frame; it caches the
//! window, the FFT plan, and the bin-mapping table.

use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;
use waved_state::FREQ_BAND_COUNT;

/// FFT size — matches the ALSA period. Larger windows improve frequency
/// resolution but add latency; this size is the natural choice.
const FFT_SIZE: usize = 1024;
/// Magnitude divisor that maps a strong i16-domain band to ≈ 1.0.
/// Picked empirically: a -10 dBFS sine over the band's bins lands near
/// full scale on the pill.
const MAG_REF: f32 = 80.0;

pub struct Spectrum {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// FFT scratch buffer (complex-typed, length FFT_SIZE).
    scratch: Vec<Complex<f32>>,
    /// Pre-computed (start_bin, end_bin) ranges for each output band.
    band_ranges: [(usize, usize); FREQ_BAND_COUNT],
    sample_rate: u32,
}

impl Spectrum {
    /// `sample_rate` is the input PCM rate (e.g. 48 000). FFT size is
    /// fixed at 1024, so the bin width is `sample_rate / 1024` Hz
    /// (≈ 47 Hz at 48 kHz).
    pub fn new(sample_rate: u32) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let window = hann_window(FFT_SIZE);
        let scratch = vec![Complex::new(0.0, 0.0); FFT_SIZE];
        let band_ranges = log_band_ranges(sample_rate);
        Self { fft, window, scratch, band_ranges, sample_rate }
    }

    /// Run one FFT pass and write the `FREQ_BAND_COUNT` magnitudes into
    /// `bands`. `input` is i16 PCM, any length — short inputs are
    /// zero-padded, long inputs are truncated to `FFT_SIZE`.
    pub fn compute(&mut self, input: &[i16], bands: &mut [f32; FREQ_BAND_COUNT]) {
        // 1. Window + cast to f32.
        let n = input.len().min(FFT_SIZE);
        for i in 0..FFT_SIZE {
            let s = if i < n {
                (input[i] as f32) / 32768.0
            } else {
                0.0
            };
            self.scratch[i] = Complex::new(s * self.window[i], 0.0);
        }

        // 2. Forward FFT.
        self.fft.process(&mut self.scratch);

        // 3. For each output band, take the max magnitude in its bin
        //    range. Max (not mean) is what makes a tonal burst at a
        //    single frequency light up its band rather than smearing.
        for (band_idx, &(lo, hi)) in self.band_ranges.iter().enumerate() {
            let mut peak = 0.0f32;
            for bin in lo..hi {
                let c = self.scratch[bin];
                let mag = (c.re * c.re + c.im * c.im).sqrt();
                if mag > peak {
                    peak = mag;
                }
            }
            bands[band_idx] = (peak / MAG_REF).clamp(0.0, 1.0);
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

fn hann_window(n: usize) -> Vec<f32> {
    let mut w = vec![0.0; n];
    let denom = (n as f32 - 1.0).max(1.0);
    for i in 0..n {
        w[i] = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / denom).cos();
    }
    w
}

/// Log-spaced bin ranges over [100 Hz, 8 kHz]. Each pair is
/// (start_bin_inclusive, end_bin_exclusive); ranges are non-overlapping
/// and contiguous.
fn log_band_ranges(sample_rate: u32) -> [(usize, usize); FREQ_BAND_COUNT] {
    const LOW_HZ: f32 = 100.0;
    const HIGH_HZ: f32 = 8_000.0;
    let bin_hz = sample_rate as f32 / FFT_SIZE as f32;
    let max_bin = FFT_SIZE / 2;

    let lo_log = LOW_HZ.ln();
    let hi_log = HIGH_HZ.ln();
    let mut out = [(0usize, 0usize); FREQ_BAND_COUNT];
    let mut prev_bin = 1usize; // skip DC
    for i in 0..FREQ_BAND_COUNT {
        let frac = (i + 1) as f32 / FREQ_BAND_COUNT as f32;
        let hz = (lo_log + (hi_log - lo_log) * frac).exp();
        let bin = ((hz / bin_hz) as usize).clamp(prev_bin + 1, max_bin);
        out[i] = (prev_bin, bin);
        prev_bin = bin;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_ranges_cover_speech_band() {
        let r = log_band_ranges(48_000);
        // First range must start above DC and below the band start.
        assert!(r[0].0 >= 1);
        // Ranges are contiguous and non-overlapping.
        for i in 1..FREQ_BAND_COUNT {
            assert_eq!(r[i - 1].1, r[i].0);
        }
        // Final range stays within the FFT's positive half.
        assert!(r[FREQ_BAND_COUNT - 1].1 <= FFT_SIZE / 2);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut s = Spectrum::new(48_000);
        let input = [0i16; FFT_SIZE];
        let mut bands = [0.0f32; FREQ_BAND_COUNT];
        s.compute(&input, &mut bands);
        for b in &bands {
            assert!(*b < 1e-3, "got {b}");
        }
    }

    #[test]
    fn tone_lights_up_one_band() {
        // 1 kHz sine — should energise a band somewhere in the middle
        // of the log range, with at least a 5× ratio over the rest.
        let mut s = Spectrum::new(48_000);
        let mut input = [0i16; FFT_SIZE];
        for i in 0..FFT_SIZE {
            let t = i as f32 / 48_000.0;
            input[i] = ((t * 1000.0 * 2.0 * std::f32::consts::PI).sin() * 16_000.0) as i16;
        }
        let mut bands = [0.0f32; FREQ_BAND_COUNT];
        s.compute(&input, &mut bands);
        let max = bands.iter().cloned().fold(0.0f32, f32::max);
        let max_idx = bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        // Compute ratio against the median to skip the tone's bin.
        let mut sorted = bands.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[FREQ_BAND_COUNT / 2];
        assert!(
            max > median * 5.0,
            "tone should dominate one band: max={max} median={median} idx={max_idx}",
        );
    }
}
