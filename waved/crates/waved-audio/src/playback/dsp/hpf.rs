//! Butterworth 2nd-order high-pass, one biquad per channel.

use super::biquad::{Biquad, design_butterworth_hpf};

pub struct HighPassFilter {
    biquads: Vec<Biquad>,
    sample_rate_hz: f32,
    cutoff_hz: f32,
}

impl HighPassFilter {
    pub fn new(channels: usize, cutoff_hz: f32, sample_rate_hz: f32) -> Self {
        let coeffs = design_butterworth_hpf(cutoff_hz, sample_rate_hz);
        Self {
            biquads: (0..channels).map(|_| Biquad::new(coeffs)).collect(),
            sample_rate_hz,
            cutoff_hz,
        }
    }

    pub fn set_cutoff(&mut self, cutoff_hz: f32) {
        self.cutoff_hz = cutoff_hz;
        let c = design_butterworth_hpf(cutoff_hz, self.sample_rate_hz);
        for b in &mut self.biquads {
            b.set_coeffs(c);
        }
    }

    pub fn reset(&mut self) {
        for b in &mut self.biquads {
            b.reset();
        }
    }

    /// Process interleaved f32 samples in place. `buf.len()` must be a
    /// multiple of `channels` (set at construction).
    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        let ch = self.biquads.len();
        if ch == 0 { return; }
        debug_assert_eq!(buf.len() % ch, 0, "buf length must be a multiple of channels");
        for frame in buf.chunks_exact_mut(ch) {
            for (c, sample) in frame.iter_mut().enumerate() {
                *sample = self.biquads[c].process_sample(*sample);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_kilohertz_attenuates_subsonic() {
        let mut hpf = HighPassFilter::new(2, 80.0, 48_000.0);
        // Stereo interleaved: L = 1 kHz sine, R = 30 Hz sine
        let mut buf: Vec<f32> = (0..48_000)
            .flat_map(|i| {
                let t = i as f32 / 48_000.0;
                let l = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
                let r = (2.0 * std::f32::consts::PI * 30.0 * t).sin();
                [l, r]
            })
            .collect();
        hpf.process_interleaved(&mut buf);

        // Compute per-channel RMS over the second half (after settling).
        let mut l_sq = 0.0_f32;
        let mut r_sq = 0.0_f32;
        for i in 24_000..48_000 {
            l_sq += buf[i * 2] * buf[i * 2];
            r_sq += buf[i * 2 + 1] * buf[i * 2 + 1];
        }
        let l_rms = (l_sq / 24_000.0).sqrt();
        let r_rms = (r_sq / 24_000.0).sqrt();
        let l_db = 20.0 * l_rms.log10();
        let r_db = 20.0 * r_rms.log10();
        // 1 kHz passes at near unity (sin RMS = 1/√2 ≈ -3 dB)
        assert!((l_db - (-3.0)).abs() < 1.0, "expected 1 kHz ≈ -3 dB, got {l_db:.2}");
        // 30 Hz attenuated >12 dB below 1 kHz
        assert!(r_db < l_db - 12.0, "expected ≥ 12 dB attenuation, got {} dB delta", l_db - r_db);
    }
}
