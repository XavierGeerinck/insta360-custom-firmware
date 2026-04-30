//! Direct-form-1 biquad. State is `x[n-1], x[n-2], y[n-1], y[n-2]`.
//! Matches the form used by stock `dynamic_eq_config_2.json` (b0..b2, a1, a2 with a0=1).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadCoeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl BiquadCoeffs {
    pub const PASSTHROUGH: BiquadCoeffs = BiquadCoeffs {
        b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0,
    };
}

#[derive(Debug, Clone)]
pub struct Biquad {
    coeffs: BiquadCoeffs,
    x1: f32, x2: f32,
    y1: f32, y2: f32,
}

impl Biquad {
    pub fn new(coeffs: BiquadCoeffs) -> Self {
        Self { coeffs, x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }

    /// Update filter coefficients without resetting the state — used by the
    /// preset switch in `PlaybackDspChain::set_preset` so the filter doesn't
    /// re-introduce its startup transient on every preset change.
    pub fn set_coeffs(&mut self, coeffs: BiquadCoeffs) {
        self.coeffs = coeffs;
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0; self.x2 = 0.0;
        self.y1 = 0.0; self.y2 = 0.0;
    }

    #[inline]
    pub fn process_sample(&mut self, x0: f32) -> f32 {
        let c = &self.coeffs;
        // Direct-form-1: y[n] = b0*x[n] + b1*x[n-1] + b2*x[n-2] - a1*y[n-1] - a2*y[n-2]
        let y0 = c.b0 * x0 + c.b1 * self.x1 + c.b2 * self.x2
                          - c.a1 * self.y1 - c.a2 * self.y2;
        self.x2 = self.x1; self.x1 = x0;
        self.y2 = self.y1; self.y1 = y0;
        y0
    }

    pub fn process_block(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.process_sample(*s);
        }
    }
}

/// Butterworth 2nd-order high-pass — RBJ cookbook formulas with Q = 1/√2.
pub fn design_butterworth_hpf(cutoff_hz: f32, sample_rate_hz: f32) -> BiquadCoeffs {
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let w0 = 2.0 * std::f32::consts::PI * cutoff_hz / sample_rate_hz;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q);

    let b0 = (1.0 + cos_w0) * 0.5;
    let b1 = -(1.0 + cos_w0);
    let b2 = (1.0 + cos_w0) * 0.5;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha;

    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// RBJ peaking EQ.
pub fn design_peaking_eq(center_hz: f32, gain_db: f32, q: f32, sample_rate_hz: f32) -> BiquadCoeffs {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * center_hz / sample_rate_hz;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q);

    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos_w0;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha / a;

    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// RBJ low shelf.
pub fn design_low_shelf(corner_hz: f32, gain_db: f32, q: f32, sample_rate_hz: f32) -> BiquadCoeffs {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * corner_hz / sample_rate_hz;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let alpha = sin_w0 / (2.0 * q);
    let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

    let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
    let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
    let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
    let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
    let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
    let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha;

    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// RBJ high shelf.
pub fn design_high_shelf(corner_hz: f32, gain_db: f32, q: f32, sample_rate_hz: f32) -> BiquadCoeffs {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * corner_hz / sample_rate_hz;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let alpha = sin_w0 / (2.0 * q);
    let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

    let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
    let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
    let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
    let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
    let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
    let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha;

    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_biquad_is_passthrough() {
        let mut b = Biquad::new(BiquadCoeffs::PASSTHROUGH);
        let mut buf = [0.0f32; 8];
        buf[0] = 1.0;
        b.process_block(&mut buf);
        assert!((buf[0] - 1.0).abs() < 1e-9);
        for i in 1..8 {
            assert!(buf[i].abs() < 1e-9, "expected zero at i={i}, got {}", buf[i]);
        }
    }

    #[test]
    fn one_sample_delay_biquad() {
        let mut b = Biquad::new(BiquadCoeffs { b0: 0.0, b1: 1.0, b2: 0.0, a1: 0.0, a2: 0.0 });
        let mut buf = [1.0_f32, 2.0, 3.0, 4.0];
        b.process_block(&mut buf);
        assert!(buf[0].abs() < 1e-9);
        assert!((buf[1] - 1.0).abs() < 1e-9);
        assert!((buf[2] - 2.0).abs() < 1e-9);
        assert!((buf[3] - 3.0).abs() < 1e-9);
    }

    /// Leaky integrator (b0=0.5, a1=-0.5) — exponential decay on an impulse.
    /// Reference values computed in Python f64 on the same direct-form-1 formula.
    /// This exercises the recursive (a-coefficient) path that the FIR-only tests
    /// above do not cover. Catches sign/aliasing errors in the y[n-1], y[n-2] update.
    #[test]
    fn iir_impulse_response_decays_exponentially() {
        let mut b = Biquad::new(BiquadCoeffs { b0: 0.5, b1: 0.0, b2: 0.0, a1: -0.5, a2: 0.0 });
        let mut buf = [1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0];
        b.process_block(&mut buf);
        let expected = [0.5_f32, 0.25, 0.125, 0.0625, 0.03125, 0.015625];
        for (i, (got, exp)) in buf.iter().zip(expected.iter()).enumerate() {
            assert!((got - exp).abs() < 1e-6, "sample {i}: got {got}, expected {exp}");
        }
    }

    #[test]
    fn butterworth_hpf_attenuates_below_cutoff() {
        // 80 Hz HPF at 48 kHz: 40 Hz should be attenuated by ~12 dB or more.
        let coeffs = design_butterworth_hpf(80.0, 48_000.0);
        let mut b = Biquad::new(coeffs);
        // 40 Hz sine, 1 second
        let mut buf: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 40.0 * i as f32 / 48_000.0).sin())
            .collect();
        b.process_block(&mut buf);
        // RMS over second half (after settling)
        let rms = {
            let n = 24_000;
            let s: f32 = buf[24_000..].iter().map(|x| x * x).sum();
            (s / n as f32).sqrt()
        };
        let attn_db = 20.0 * rms.log10();
        assert!(attn_db <= -12.0, "expected ≤ -12 dB at 40 Hz, got {attn_db:.2} dB");
    }

    #[test]
    fn peaking_eq_matches_target_at_center() {
        // +6 dB peaking at 1 kHz, Q=1, sample rate 48 kHz.
        let coeffs = design_peaking_eq(1000.0, 6.0, 1.0, 48_000.0);
        let mut b = Biquad::new(coeffs);
        let mut buf: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin())
            .collect();
        b.process_block(&mut buf);
        let rms = {
            let s: f32 = buf[24_000..].iter().map(|x| x * x).sum();
            (s / 24_000_f32).sqrt()
        };
        let gain_db = 20.0 * rms.log10() - 20.0 * (1.0_f32 / 2.0_f32.sqrt()).log10();
        assert!((gain_db - 6.0).abs() < 0.5, "expected +6 dB at 1 kHz, got {gain_db:.2} dB");
    }

    #[test]
    fn low_shelf_lifts_low_frequencies() {
        let coeffs = design_low_shelf(200.0, 6.0, 1.0, 48_000.0);
        let mut b = Biquad::new(coeffs);
        let mut buf: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 60.0 * i as f32 / 48_000.0).sin())
            .collect();
        b.process_block(&mut buf);
        let rms = {
            let s: f32 = buf[24_000..].iter().map(|x| x * x).sum();
            (s / 24_000_f32).sqrt()
        };
        let gain_db = 20.0 * rms.log10() - 20.0 * (1.0_f32 / 2.0_f32.sqrt()).log10();
        assert!(gain_db > 4.0, "expected ≥ 4 dB lift at 60 Hz, got {gain_db:.2} dB");
    }

    #[test]
    fn high_shelf_lifts_high_frequencies() {
        let coeffs = design_high_shelf(4000.0, 6.0, 1.0, 48_000.0);
        let mut b = Biquad::new(coeffs);
        let mut buf: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 8000.0 * i as f32 / 48_000.0).sin())
            .collect();
        b.process_block(&mut buf);
        let rms = {
            let s: f32 = buf[24_000..].iter().map(|x| x * x).sum();
            (s / 24_000_f32).sqrt()
        };
        let gain_db = 20.0 * rms.log10() - 20.0 * (1.0_f32 / 2.0_f32.sqrt()).log10();
        assert!(gain_db > 4.0, "expected ≥ 4 dB lift at 8 kHz, got {gain_db:.2} dB");
    }
}
