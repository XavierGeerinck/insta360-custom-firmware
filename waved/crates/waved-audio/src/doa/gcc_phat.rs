//! GCC-PHAT pairwise time-delay estimation for the DoA pairs.
//!
//! Given two mic signals x1, x2 with the same source delayed by τ samples
//! between them, returns τ (fractional) via the phase-transform-weighted
//! cross-correlation peak.
//!
//! Three choices make this work on speech, not just broadband noise (device
//! pass, 2026-10-08, see docs/AUDIO_HARDWARE.md § "Array calibration"):
//! only 300 Hz–6 kHz enters the correlation; each pair's cross-spectrum is
//! averaged over recent frames before the phase transform; and the frame
//! edges are tapered. The earlier mask kept bins within 20 dB of the loudest
//! one, which on speech meant a few low harmonics that a 51 mm array can
//! barely resolve, and one frame of speech carries too little of the band to
//! agree with itself. Without the taper, the bins between a voice's
//! harmonics hold the leakage of the frame edges, which every channel cuts
//! at the same instant, so it pulls the delay towards 0. On counted speech
//! from the front at 1 m the median per-frame azimuth error went from 63° to
//! 5°; broadband noise from four sides stayed at 5–9°.
use rustfft::{num_complex::Complex, FftPlanner};

/// The correlation band. Below it the array is too small to resolve the
/// delay; above it speech carries little energy at a distance.
const BAND_LOW_HZ: f32 = 300.0;
const BAND_HIGH_HZ: f32 = 6_000.0;
/// Weight of the running average in each pair's cross-spectrum, per
/// 1024-sample frame (21 ms at 48 kHz); rescaled for other frame sizes.
const SMOOTHING_AT_1024: f32 = 0.7;
/// Fraction of the frame tapered (half at each end) by the Tukey window.
const TAPER: f32 = 0.25;
/// Partial phase transform: each bin is divided by |cross|^β. β = 1 is plain
/// PHAT (every bin counts the same); 0.8 lets the bins the talker fills count
/// a little more than the empty ones (speech from the left 26° → 22° median
/// error, noise from the left 8° → 5°).
const PHAT_BETA: f32 = 0.8;

/// One pair's delay estimate.
#[derive(Debug, Clone, Copy)]
pub struct PairDelay {
    /// τ in samples such that x2[n] ≈ x1[n − τ].
    pub tau_samples: f32,
    /// Curvature of the correlation peak (> 0 for a concave peak, 0 on a
    /// flat or convex one). The DoA solver weights the pairs by it.
    pub sharpness: f32,
    /// Height of the correlation peak, normalised so a fully coherent pair
    /// reads 1 and unrelated signals read ~0.1.
    pub coherence: f32,
}

pub struct GccPhat {
    fft_len: usize,
    planner_fwd: std::sync::Arc<dyn rustfft::Fft<f32>>,
    planner_inv: std::sync::Arc<dyn rustfft::Fft<f32>>,
    /// First and last FFT bin of the correlation band.
    band: (usize, usize),
    /// Tukey window over one frame.
    window: Vec<f32>,
    smoothing: f32,
    /// Running cross-spectrum per pair, over the band bins.
    cross: Vec<Vec<Complex<f32>>>,
    corr: Vec<Complex<f32>>,
}

impl GccPhat {
    /// `pairs` is how many mic pairs the caller tracks; each keeps its own
    /// cross-spectrum average.
    pub fn new(frame_size: usize, sample_rate: u32, pairs: usize) -> Self {
        // Zero-pad to 2x to avoid circular-correlation artifacts.
        // (At least 16 points, so a degenerate frame size still builds.)
        let fft_len = (frame_size * 2).next_power_of_two().max(16);
        let mut planner = FftPlanner::<f32>::new();
        let bin_hz = sample_rate as f32 / fft_len as f32;
        let top = fft_len / 2 - 1;
        let low = ((BAND_LOW_HZ / bin_hz).ceil() as usize).clamp(1, top);
        let high = ((BAND_HIGH_HZ / bin_hz).floor() as usize).clamp(low, top);
        Self {
            fft_len,
            planner_fwd: planner.plan_fft_forward(fft_len),
            planner_inv: planner.plan_fft_inverse(fft_len),
            band: (low, high),
            window: tukey(frame_size, TAPER),
            smoothing: SMOOTHING_AT_1024.powf(frame_size as f32 / 1024.0),
            cross: vec![vec![Complex::new(0.0, 0.0); high - low + 1]; pairs],
            corr: vec![Complex::new(0.0, 0.0); fft_len],
        }
    }

    /// Tapered, zero-padded spectrum of one channel's frame, for `estimate`.
    pub fn spectrum(&self, x: &[f32], out: &mut Vec<Complex<f32>>) {
        debug_assert_eq!(x.len(), self.window.len());
        out.clear();
        out.extend(x.iter().zip(&self.window).map(|(&v, &w)| Complex::new(v * w, 0.0)));
        out.resize(self.fft_len, Complex::new(0.0, 0.0));
        self.planner_fwd.process(out);
    }

    /// Mean power of the band in a `spectrum` of `frame_size` samples, in dB
    /// relative to a full-scale signal.
    pub fn band_level_db(&self, spectrum: &[Complex<f32>], frame_size: usize) -> f32 {
        let (low, high) = self.band;
        let energy: f32 = spectrum[low..=high].iter().map(|c| c.norm_sqr()).sum();
        // Parseval over both halves of the spectrum, per input sample.
        let mean_square = 2.0 * energy / (self.fft_len as f32 * frame_size as f32);
        10.0 * (mean_square + 1e-30).log10()
    }

    /// Fold this frame into `pair`'s cross-spectrum and estimate its delay.
    /// `s1`, `s2` come from `spectrum`.
    pub fn estimate(
        &mut self,
        pair: usize,
        s1: &[Complex<f32>],
        s2: &[Complex<f32>],
        max_lag_samples: usize,
    ) -> PairDelay {
        let n = self.fft_len;
        let (low, high) = self.band;
        let alpha = self.smoothing;
        // s2 * conj(s1) so the IFFT peak appears at +τ for x2[n] ≈ x1[n − τ].
        for (k, avg) in (low..=high).zip(self.cross[pair].iter_mut()) {
            *avg = *avg * alpha + s2[k] * s1[k].conj() * (1.0 - alpha);
        }
        // Phase transform over the band, mirrored for a real correlation.
        self.corr.fill(Complex::new(0.0, 0.0));
        let mut total = 0.0f32;
        for (k, avg) in (low..=high).zip(self.cross[pair].iter()) {
            let mag = avg.norm();
            if mag > 1e-30 {
                let weighted = avg / mag.powf(PHAT_BETA);
                total += weighted.norm();
                self.corr[k] = weighted;
                self.corr[n - k] = weighted.conj();
            }
        }
        if total <= 0.0 {
            return PairDelay { tau_samples: 0.0, sharpness: 0.0, coherence: 0.0 };
        }
        self.planner_inv.process(&mut self.corr);
        // A fully coherent pair puts every bin's weight into one lag.
        let scale = 1.0 / (2.0 * total);
        let at = |lag: i32| {
            let idx = if lag >= 0 { lag as usize } else { (n as i32 + lag) as usize };
            self.corr[idx].re * scale
        };

        // Search for peak within ±max_lag_samples.
        let max_lag = max_lag_samples.min(n / 2 - 1) as i32;
        let mut best_lag = 0i32;
        let mut best_val = f32::NEG_INFINITY;
        for lag in -max_lag..=max_lag {
            let v = at(lag);
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
        let y_l = at(best_lag - 1);
        let y_c = best_val;
        let y_r = at(best_lag + 1);
        let denom = y_l - 2.0 * y_c + y_r;
        let frac = if denom < -1e-12 {
            (0.5 * (y_l - y_r) / denom).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        PairDelay {
            tau_samples: best_lag as f32 + frac,
            sharpness: (-denom).max(0.0),
            coherence: best_val,
        }
    }
}

/// Tukey window: flat, with `taper / 2` of the length at each end rising as
/// a half cosine.
fn tukey(len: usize, taper: f32) -> Vec<f32> {
    let edge = ((taper * len as f32) / 2.0).max(1.0);
    (0..len)
        .map(|i| {
            let from_end = (i.min(len - 1 - i)) as f32;
            if from_end >= edge {
                1.0
            } else {
                0.5 - 0.5 * (std::f32::consts::PI * from_end / edge).cos()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame through a fresh single-pair estimator.
    fn once(x1: &[f32], x2: &[f32], max_lag: usize) -> PairDelay {
        let mut gcc = GccPhat::new(x1.len(), 48_000, 1);
        let (mut s1, mut s2) = (Vec::new(), Vec::new());
        gcc.spectrum(x1, &mut s1);
        gcc.spectrum(x2, &mut s2);
        gcc.estimate(0, &s1, &s2, max_lag)
    }

    /// A broadband source over the correlation band, delayed by any
    /// (fractional) number of samples. Pure tones are not used: one tone is
    /// one bin of the ~240 in the band, so the rest is window leakage, and
    /// the DoA VAD treats a steady tone as room noise anyway.
    fn broadband(n: usize, delay: f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = (i as f32 - delay) / 48_000.0;
                (0..60)
                    .map(|k| {
                        let f = 320.0 + 95.0 * k as f32;
                        (2.0 * std::f32::consts::PI * f * t + (k * k) as f32 * 0.7).sin()
                    })
                    .sum::<f32>()
                    / 60.0
            })
            .collect()
    }

    #[test]
    fn zero_delay_detects_zero() {
        let x1 = broadband(480, 0.0);
        let tau = once(&x1, &x1, 16).tau_samples;
        assert!(tau.abs() < 0.5, "expected τ~0, got {tau}");
    }

    #[test]
    fn positive_delay_detected() {
        // x2 lags x1 by 5 samples. So x2[n] = x1[n-5]. τ in our convention = +5.
        let tau = once(&broadband(480, 0.0), &broadband(480, 5.0), 16).tau_samples;
        assert!((tau - 5.0).abs() < 0.5, "expected τ≈5, got {tau}");
    }

    #[test]
    fn negative_delay_detected() {
        let tau = once(&broadband(480, 3.0), &broadband(480, 0.0), 16).tau_samples;
        assert!((tau + 3.0).abs() < 0.5, "expected τ≈-3, got {tau}");
    }

    #[test]
    fn fractional_delay_subsample_precision() {
        let tau = once(&broadband(1024, 0.0), &broadband(1024, 2.5), 16).tau_samples;
        assert!((tau - 2.5).abs() < 0.3, "expected τ≈2.5, got {tau}");
    }

    #[test]
    fn noise_pair_is_bounded_not_hundreds_of_samples() {
        // Uncorrelated noise sources produce a flat/lumpy correlation surface
        // where parabolic interpolation can blow up. The estimator must stay
        // within ±(max_lag + 0.5) even in the worst case. Regression test for
        // on-device outliers like τ=+1836 observed on pair (1,7).
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
        let tau = once(&x1, &x2, max_lag).tau_samples;
        assert!(
            tau.abs() <= max_lag as f32 + 0.5,
            "noise pair must stay bounded, got τ={tau}"
        );
    }

    #[test]
    fn peak_sharpness_is_positive_for_concave_peak() {
        let sharpness = once(&broadband(480, 0.0), &broadband(480, 5.0), 16).sharpness;
        assert!(sharpness > 0.0, "concave peak should have positive sharpness, got {sharpness}");
    }

    #[test]
    fn peak_sharpness_discriminates_broadband_signal_from_noise() {
        // Replaces the plan's `peak_sharpness_near_zero_for_noise` (with `< 1.0`
        // bound). That bound was incorrect: PHAT-normalised noise still produces
        // IFFT peaks. The discrimination ratio below is what
        // the weighted-LS approach actually depends on — see plan §3.1.
        //
        // The weighted-LS DoA solver (SP#3 270° fix) weights each mic pair by
        // sharpness so that body-shadowed pairs (flat correlation surface) are
        // down-weighted relative to clean pairs (sharp concave peak).  For that
        // weighting to be meaningful, signal sharpness must be substantially
        // larger than noise sharpness. Over the full band this was > 5×; with
        // the correlation limited to 300 Hz–6 kHz every coherent peak is
        // broader (curvature follows the band's mean ω²) and the ratio is ~4×.
        // The bar is 3×: the weights stay far from degenerate, and the
        // weighting is validated on the device recordings (docs/AUDIO_HARDWARE.md
        // § "Array calibration").
        //
        // SIGNAL: same broadband noise on both channels, delayed by τ=5 samples.
        // This is the closest analog to a real plane-wave hitting two mics: same
        // source content, different arrival time.
        // NOISE: independent broadband noise per channel. No correlation.
        //
        // Both run as production does, ten consecutive frames through one
        // estimator with the cross-spectra averaging between them. (Averaging
        // does not make unrelated noise flatter: after the phase transform its
        // bins keep random phases however long they are averaged.)
        let mut state = 0x9E3779B1u32;
        let mut rnd = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32) - 0.5
        };
        let mut gcc = GccPhat::new(480, 48_000, 2);
        let (mut s1, mut s2) = (Vec::new(), Vec::new());
        let (mut sig, mut noise) = (None, None);
        for _ in 0..10 {
            let src: Vec<f32> = (0..480 + 16).map(|_| rnd()).collect();
            gcc.spectrum(&src[5..485], &mut s1);
            gcc.spectrum(&src[0..480], &mut s2); // x2 lags x1 by 5 → τ=+5
            sig = Some(gcc.estimate(0, &s1, &s2, 20));
            let a: Vec<f32> = (0..480).map(|_| rnd()).collect();
            let b: Vec<f32> = (0..480).map(|_| rnd()).collect();
            gcc.spectrum(&a, &mut s1);
            gcc.spectrum(&b, &mut s2);
            noise = Some(gcc.estimate(1, &s1, &s2, 20));
        }
        let (sig, noise) = (sig.unwrap(), noise.unwrap());
        let (sig_tau, sig_sharpness, noise_sharpness) =
            (sig.tau_samples, sig.sharpness, noise.sharpness);

        eprintln!(
            "broadband: sig_tau={sig_tau:.2} (expected ≈ 5), sig_sharpness={sig_sharpness:.2}, \
             noise_sharpness={noise_sharpness:.2}, ratio={:.2}",
            sig_sharpness / noise_sharpness.max(1e-9)
        );

        // Sanity: signal tau must be recovered.
        assert!((sig_tau - 5.0).abs() < 0.5, "signal τ {sig_tau} should be ≈ 5");
        // Discrimination: signal sharpness must dominate noise sharpness by ≥ 3×.
        assert!(
            sig_sharpness > 3.0 * noise_sharpness,
            "weighted-LS unsound: sig_sharpness {sig_sharpness} vs noise_sharpness {noise_sharpness} (ratio {:.2}× < 3×)",
            sig_sharpness / noise_sharpness.max(1e-9)
        );
    }

    fn xorshift(seed: u32) -> impl FnMut() -> f32 {
        let mut state = seed;
        move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32) - 0.5
        }
    }

    #[test]
    fn coherence_reads_near_one_for_a_delayed_copy_and_low_for_unrelated_noise() {
        let mut rnd = xorshift(0x1234_5678);
        let src: Vec<f32> = (0..1024 + 8).map(|_| rnd()).collect();
        let copy = once(&src[4..1028], &src[0..1024], 20);
        assert!((copy.tau_samples - 4.0).abs() < 0.3, "τ {}", copy.tau_samples);
        assert!(copy.coherence > 0.9, "copy coherence {}", copy.coherence);

        let a: Vec<f32> = (0..1024).map(|_| rnd()).collect();
        let b: Vec<f32> = (0..1024).map(|_| rnd()).collect();
        let unrelated = once(&a, &b, 20);
        assert!(unrelated.coherence < 0.3, "noise coherence {}", unrelated.coherence);
    }

    #[test]
    fn averaging_keeps_unrelated_noise_incoherent_over_many_frames() {
        let mut rnd = xorshift(0x0bad_cafe);
        let mut gcc = GccPhat::new(1024, 48_000, 1);
        let (mut s1, mut s2) = (Vec::new(), Vec::new());
        let mut last = None;
        for _ in 0..50 {
            let a: Vec<f32> = (0..1024).map(|_| rnd()).collect();
            let b: Vec<f32> = (0..1024).map(|_| rnd()).collect();
            gcc.spectrum(&a, &mut s1);
            gcc.spectrum(&b, &mut s2);
            last = Some(gcc.estimate(0, &s1, &s2, 20));
        }
        let c = last.unwrap().coherence;
        assert!(c < 0.3, "averaged noise coherence {c}");
    }

    #[test]
    fn a_source_with_its_energy_in_low_harmonics_still_gives_the_delay() {
        // Voiced speech: a 150 Hz fundamental whose harmonics fall at
        // 12 dB/octave. The delay must come from the whole band, not the
        // first few harmonics.
        let harmonic = |delay: f32| -> Vec<f32> {
            (0..1024)
                .map(|i| {
                    let t = (i as f32 - delay) / 48_000.0;
                    (1..40)
                        .map(|k| {
                            let f = 150.0 * k as f32;
                            (2.0 * std::f32::consts::PI * f * t + k as f32).sin() / (k * k) as f32
                        })
                        .sum()
                })
                .collect()
        };
        let d = once(&harmonic(0.0), &harmonic(3.0), 20);
        assert!((d.tau_samples - 3.0).abs() < 0.3, "τ {}", d.tau_samples);
        assert!(d.coherence > 0.8, "coherence {}", d.coherence);
    }
}
