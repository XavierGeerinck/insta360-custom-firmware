//! GCC-PHAT pairwise time-delay estimation.
//!
//! Given two mic signals x1, x2 with the same source delayed by τ samples
//! between them, returns τ (fractional) via the phase-transform-weighted
//! cross-correlation peak.

use rustfft::{num_complex::Complex, FftPlanner};

pub struct GccPhat {
    fft_len: usize,
    planner_fwd: std::sync::Arc<dyn rustfft::Fft<f32>>,
    planner_inv: std::sync::Arc<dyn rustfft::Fft<f32>>,
}

impl GccPhat {
    pub fn new(frame_size: usize) -> Self {
        // Zero-pad to 2x to avoid circular-correlation artifacts.
        let fft_len = (frame_size * 2).next_power_of_two();
        let mut planner = FftPlanner::<f32>::new();
        Self {
            fft_len,
            planner_fwd: planner.plan_fft_forward(fft_len),
            planner_inv: planner.plan_fft_inverse(fft_len),
        }
    }

    /// Estimate TDOA (in samples, fractional) such that x2[n] ≈ x1[n - τ].
    /// `max_lag_samples` bounds the search window (set from geometry: max
    /// physical separation / c / T_s).
    pub fn estimate_tdoa(&self, x1: &[f32], x2: &[f32], max_lag_samples: usize) -> f32 {
        assert_eq!(x1.len(), x2.len());
        let n = self.fft_len;

        let mut b1: Vec<Complex<f32>> = x1.iter().map(|&v| Complex::new(v, 0.0)).collect();
        b1.resize(n, Complex::new(0.0, 0.0));
        let mut b2: Vec<Complex<f32>> = x2.iter().map(|&v| Complex::new(v, 0.0)).collect();
        b2.resize(n, Complex::new(0.0, 0.0));

        self.planner_fwd.process(&mut b1);
        self.planner_fwd.process(&mut b2);

        // PHAT-weighted cross-power: X1 * conj(X2) / |X1 * conj(X2)|.
        // Silent bins (|X1| or |X2| below threshold) are zeroed — without
        // masking, the PHAT 1/|·| factor amplifies numerical noise at empty
        // bins and drowns the true correlation peak.
        let max_mag = b1
            .iter()
            .zip(b2.iter())
            .map(|(a, b)| a.norm().min(b.norm()))
            .fold(0.0f32, f32::max);
        let thresh = (max_mag * 0.1).max(1e-6);
        let mut cross: Vec<Complex<f32>> = b1
            .iter()
            .zip(b2.iter())
            .map(|(a, b)| {
                if a.norm() < thresh || b.norm() < thresh {
                    Complex::new(0.0, 0.0)
                } else {
                    // b * conj(a) so IFFT peak appears at +τ for x2[n]≈x1[n-τ].
                    let num = b * a.conj();
                    num / num.norm().max(1e-10)
                }
            })
            .collect();

        self.planner_inv.process(&mut cross);

        // Search for peak within ±max_lag_samples (wrap-around: positive lags
        // at start of buffer; negative lags at end).
        let max_lag = max_lag_samples.min(n / 2 - 1);
        let mut best_lag = 0i32;
        let mut best_val = f32::NEG_INFINITY;
        for lag in -(max_lag as i32)..=(max_lag as i32) {
            let idx = if lag >= 0 { lag as usize } else { (n as i32 + lag) as usize };
            let v = cross[idx].re;
            if v > best_val {
                best_val = v;
                best_lag = lag;
            }
        }

        // Parabolic interpolation for sub-sample precision.
        //
        // A genuine peak has y_c > y_l and y_c > y_r, which forces
        //   denom = y_l - 2y_c + y_r  <  0 (concave).
        // In that case |frac| ≤ 0.5 and the interpolated peak is between the
        // two neighbours. Rejecting non-concave denom cuts a class of hardware
        // outliers observed on-device (noisy cross-ring pairs producing
        // parabolic-fit values of ±hundreds of samples); clamping to ±0.5
        // bounds the remaining numerical noise near zero-crossings.
        let center = best_lag;
        let idx_c = if center >= 0 { center as usize } else { (n as i32 + center) as usize };
        let idx_l = if idx_c == 0 { n - 1 } else { idx_c - 1 };
        let idx_r = if idx_c == n - 1 { 0 } else { idx_c + 1 };
        let y_l = cross[idx_l].re;
        let y_c = cross[idx_c].re;
        let y_r = cross[idx_r].re;
        let denom = y_l - 2.0 * y_c + y_r;
        let frac = if denom < -1e-9 {
            (0.5 * (y_l - y_r) / denom).clamp(-0.5, 0.5)
        } else {
            0.0
        };

        center as f32 + frac
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, sr: u32, n: usize, phase_samples: f32) -> Vec<f32> {
        let omega = 2.0 * std::f32::consts::PI * freq / sr as f32;
        (0..n).map(|i| ((i as f32 - phase_samples) * omega).sin()).collect()
    }

    #[test]
    fn zero_delay_detects_zero() {
        let gcc = GccPhat::new(480);
        let x1 = tone(1000.0, 48000, 480, 0.0);
        let x2 = x1.clone();
        let tau = gcc.estimate_tdoa(&x1, &x2, 16);
        assert!(tau.abs() < 0.5, "expected τ~0, got {tau}");
    }

    #[test]
    fn positive_delay_detected() {
        let gcc = GccPhat::new(480);
        // x2 lags x1 by 5 samples. So x2[n] = x1[n-5]. τ in our convention = +5.
        let x1 = tone(1000.0, 48000, 480, 0.0);
        let x2 = tone(1000.0, 48000, 480, 5.0);
        let tau = gcc.estimate_tdoa(&x1, &x2, 16);
        assert!((tau - 5.0).abs() < 0.5, "expected τ≈5, got {tau}");
    }

    #[test]
    fn negative_delay_detected() {
        let gcc = GccPhat::new(480);
        let x1 = tone(1000.0, 48000, 480, 3.0);
        let x2 = tone(1000.0, 48000, 480, 0.0);
        let tau = gcc.estimate_tdoa(&x1, &x2, 16);
        assert!((tau + 3.0).abs() < 0.5, "expected τ≈-3, got {tau}");
    }

    #[test]
    fn fractional_delay_subsample_precision() {
        let gcc = GccPhat::new(480);
        let x1 = tone(2000.0, 48000, 480, 0.0);
        let x2 = tone(2000.0, 48000, 480, 2.5);
        let tau = gcc.estimate_tdoa(&x1, &x2, 16);
        assert!((tau - 2.5).abs() < 0.3, "expected τ≈2.5, got {tau}");
    }

    #[test]
    fn noise_pair_is_bounded_not_hundreds_of_samples() {
        // Uncorrelated noise sources produce a flat/lumpy correlation surface
        // where parabolic interpolation can blow up. The estimator must stay
        // within ±(max_lag + 0.5) even in the worst case. Regression test for
        // on-device outliers like τ=+1836 observed on pair (1,7).
        let gcc = GccPhat::new(480);
        let mut state = 0x9E3779B1u32;
        let mut rnd = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32) - 0.5
        };
        let x1: Vec<f32> = (0..480).map(|_| rnd()).collect();
        let x2: Vec<f32> = (0..480).map(|_| rnd()).collect();
        let max_lag = 20usize;
        let tau = gcc.estimate_tdoa(&x1, &x2, max_lag);
        assert!(
            tau.abs() <= max_lag as f32 + 0.5,
            "noise pair must stay bounded, got τ={tau}"
        );
    }
}
