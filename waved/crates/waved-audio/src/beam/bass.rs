//! Bass compensation for the first-order differential patterns (C18).
//!
//! `front(t) − back(t − τ)` responds like `2·sin(ω·D/2)`, with `D` the
//! acoustic front-to-back delay plus the back delay `τ`, in samples. Below
//! its first peak that is a 6 dB/octave high-pass: a Cardioid (D ≈ 10.1
//! samples at 48 kHz) lost 15.7 dB at 125 Hz against 1 kHz.
//!
//! `BassShelf` is a first-order low shelf, an integrator with a floor:
//! - its upper corner (the zero) sits where the pattern stops rising,
//!   `ω·D = ZERO_OMEGA_D`, so it follows the pattern's own roll-off;
//! - below the lower corner (`FLOOR_HZ`, the pole) the gain flattens at
//!   `MAX_BOOST_DB`, so mic self-noise is never lifted by more than that;
//! - between the two it falls at 6 dB/octave, cancelling the pattern's slope.
//!
//! The gain is highest at DC (+12 dB) and falls to `12 dB − depth` above the
//! zero, so the flattened pattern sits below the plain average in the voice
//! band; `presence_makeup` adds the flat gain that brings its talker back to
//! Whole room's level (design table in `docs/AUDIO_HARDWARE.md`).

use std::f32::consts::PI;

/// Most the shelf ever boosts (at DC), in dB.
pub const MAX_BOOST_DB: f32 = 12.0;
/// Lower corner: below it the shelf stops rising.
pub const FLOOR_HZ: f32 = 85.0;
/// Upper corner, as `ω·D` (radians): the differential response stops
/// following `ω·D` at about 2 rad (−1.5 dB there).
pub const ZERO_OMEGA_D: f32 = 2.2;

/// One-pole, one-zero low shelf (transposed direct form II). `Default` is a
/// pass-through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BassShelf {
    b0: f32,
    b1: f32,
    a1: f32,
    state: f32,
}

impl Default for BassShelf {
    fn default() -> Self {
        Self { b0: 1.0, b1: 0.0, a1: 0.0, state: 0.0 }
    }
}

impl BassShelf {
    /// A shelf for a pattern whose front-to-back delay is `d_samples`.
    pub fn for_delay(d_samples: f32, sample_rate: u32) -> Self {
        let mut s = Self::default();
        s.retune(d_samples, sample_rate);
        s
    }

    /// Redesign for a new delay (the beam was re-aimed), keeping the state
    /// so a moving beam does not click.
    pub fn retune(&mut self, d_samples: f32, sample_rate: u32) {
        let fs = sample_rate as f32;
        if d_samples.is_nan() || d_samples <= 0.5 || fs <= 0.0 {
            // No usable front-to-back spacing: no differential roll-off.
            (self.b0, self.b1, self.a1) = (1.0, 0.0, 0.0);
            return;
        }
        let zero_hz = (ZERO_OMEGA_D * fs / (2.0 * PI * d_samples)).clamp(FLOOR_HZ, 0.45 * fs);
        // Bilinear transform of g·(s + ωz)/(s + ωp), corners pre-warped.
        let k = 2.0 * fs;
        let warp = |hz: f32| k * (PI * hz / fs).tan();
        let (wp, wz) = (warp(FLOOR_HZ), warp(zero_hz));
        let max_boost = 10f32.powf(MAX_BOOST_DB / 20.0);
        // DC gain g·ωz/ωp = max_boost; g is the gain above the zero.
        let g = max_boost * wp / wz;
        self.b0 = g * (k + wz) / (k + wp);
        self.b1 = g * (wz - k) / (k + wp);
        self.a1 = (wp - k) / (k + wp);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.state;
        self.state = self.b1 * x - self.a1 * y;
        y
    }

    pub fn reset(&mut self) {
        self.state = 0.0;
    }

    /// Magnitude response at `hz`, in dB.
    pub fn gain_db(&self, hz: f32, sample_rate: u32) -> f32 {
        let w = 2.0 * PI * hz / sample_rate as f32;
        let (c, s) = (w.cos(), w.sin());
        // (b0 + b1·e^{−jω}) / (1 + a1·e^{−jω})
        let num = (self.b0 + self.b1 * c).hypot(self.b1 * s);
        let den = (1.0 + self.a1 * c).hypot(self.a1 * s);
        20.0 * (num / den).log10()
    }
}

/// The voice presence band a pattern's make-up gain levels, in Hz.
pub const PRESENCE_BAND_HZ: (f32, f32) = (1_000.0, 3_000.0);
/// Most make-up gain a pattern gets, in dB (a degenerate spacing must not
/// turn into a huge gain).
pub const MAX_MAKEUP_DB: f32 = 24.0;

/// Flat gain that brings a pattern with front-to-back delay `d_samples`,
/// bass-compensated by `shelf`, to 0 dB for its talker on average over the
/// presence band, the level Whole room gives a talker it is aligned on. So
/// switching mode does not change the talker's level.
///
/// Derived from the pattern's own response: the differential is
/// `2·|sin(π·f·D/fs)|` on axis (matches the simulated beam to 0.1 dB, see
/// `beam::tests`), times the shelf; the gain is minus its mean in dB over
/// 21 evenly spaced points from 1 to 3 kHz.
pub fn presence_makeup(d_samples: f32, sample_rate: u32, shelf: &BassShelf) -> f32 {
    if d_samples.is_nan() || d_samples <= 0.5 || sample_rate == 0 {
        return 1.0;
    }
    const POINTS: usize = 21;
    let (lo, hi) = PRESENCE_BAND_HZ;
    let fs = sample_rate as f32;
    let mean_db = (0..POINTS)
        .map(|i| {
            let hz = lo + (hi - lo) * i as f32 / (POINTS - 1) as f32;
            let pattern = 2.0 * (PI * hz * d_samples / fs).sin().abs();
            // A null inside the band counts as −20 dB, not −∞.
            20.0 * pattern.max(0.1).log10() + shelf.gain_db(hz, sample_rate)
        })
        .sum::<f32>()
        / POINTS as f32;
    10f32.powf((-mean_db).min(MAX_MAKEUP_DB) / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Front-to-back delays of the patterns at 48 kHz, front, el 0
    /// (Figure8, Supercardioid, Cardioid, Stereo side / diagonal aim).
    const DELAYS: [f32; 4] = [5.05, 7.96, 10.09, 14.27];

    #[test]
    fn boost_never_exceeds_the_cap() {
        for d in DELAYS {
            let s = BassShelf::for_delay(d, 48_000);
            let mut hz = 1.0;
            while hz < 24_000.0 {
                let db = s.gain_db(hz, 48_000);
                assert!(db <= MAX_BOOST_DB + 1e-3, "D {d}: {hz} Hz boosted {db:.2} dB");
                hz *= 1.1;
            }
            assert!(s.gain_db(50.0, 48_000) <= MAX_BOOST_DB);
            assert!((s.gain_db(0.0, 48_000) - MAX_BOOST_DB).abs() < 0.01, "DC sits at the cap");
        }
    }

    #[test]
    fn slope_cancels_the_differential_roll_off() {
        // Between the corners the shelf falls ~6 dB/octave.
        let s = BassShelf::for_delay(10.09, 48_000);
        let octave = s.gain_db(250.0, 48_000) - s.gain_db(500.0, 48_000);
        assert!((4.5..=6.1).contains(&octave), "250→500 Hz: {octave:.2} dB");
    }

    #[test]
    fn wider_spacing_turns_over_lower() {
        // A longer front-to-back delay rolls the pattern off lower, so its
        // shelf's top corner is lower and it cuts less in the mids.
        let short = BassShelf::for_delay(5.05, 48_000);
        let long = BassShelf::for_delay(14.27, 48_000);
        assert!(long.gain_db(2_000.0, 48_000) > short.gain_db(2_000.0, 48_000));
    }

    #[test]
    fn measured_gain_matches_the_design() {
        let mut s = BassShelf::for_delay(10.09, 48_000);
        let omega = 2.0 * PI * 200.0 / 48_000.0;
        let y: Vec<f32> = (0..48_000).map(|i| s.process((i as f32 * omega).sin())).collect();
        let rms = (y[24_000..].iter().map(|v| v * v).sum::<f32>() / 24_000.0).sqrt();
        let db = 20.0 * (rms / std::f32::consts::FRAC_1_SQRT_2).log10();
        assert!((db - s.gain_db(200.0, 48_000)).abs() < 0.05, "{db:.2} dB");
    }

    #[test]
    fn reset_clears_the_state_but_keeps_the_tuning() {
        let mut s = BassShelf::for_delay(10.09, 48_000);
        for _ in 0..100 {
            s.process(1.0);
        }
        let tuned = s;
        s.reset();
        assert_eq!(s.process(0.0), 0.0);
        assert_eq!(s.gain_db(100.0, 48_000), tuned.gain_db(100.0, 48_000));
    }

    #[test]
    fn makeup_levels_the_presence_band() {
        for d in DELAYS {
            let shelf = BassShelf::for_delay(d, 48_000);
            let g = presence_makeup(d, 48_000, &shelf);
            let db_at = |hz: f32| {
                20.0 * (g * 2.0 * (PI * hz * d / 48_000.0).sin().abs()).log10()
                    + shelf.gain_db(hz, 48_000)
            };
            let mean = [1_000.0, 1_500.0, 2_000.0, 2_500.0, 3_000.0]
                .iter()
                .map(|&hz| db_at(hz))
                .sum::<f32>()
                / 5.0;
            // Five points against the 21 of the derivation; the widest
            // spacing falls steeply into its 3.4 kHz null at the top.
            assert!(mean.abs() <= 1.0, "D {d}: presence mean {mean:+.2} dB");
            assert!(g > 1.0 && 20.0 * g.log10() <= MAX_MAKEUP_DB);
        }
        assert_eq!(presence_makeup(0.0, 48_000, &BassShelf::default()), 1.0);
    }

    #[test]
    fn no_spacing_is_a_pass_through() {
        let mut s = BassShelf::for_delay(0.0, 48_000);
        assert_eq!(s.process(0.25), 0.25);
        assert!(s.gain_db(50.0, 48_000).abs() < 1e-4);
    }
}
