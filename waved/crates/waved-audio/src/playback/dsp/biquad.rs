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
}
