//! 4-band MBAGC. Linkwitz-Riley 4th-order crossovers split into
//! [low, mid-low, mid-high, high]; per-band envelope-follower drives a
//! per-band gain; bands sum back to the stereo bus.

use serde::{Deserialize, Serialize};

use super::biquad::{Biquad, BiquadCoeffs, design_butterworth_hpf, design_butterworth_lpf};

pub const N_BANDS: usize = 4;
pub const N_CROSSOVERS: usize = 3;
pub const N_CHANNELS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MbagcParams {
    pub enabled: bool,
    pub crossover_freq_hz: [f32; N_CROSSOVERS],
    /// Per-band target peak level in dBFS. Stock numbers map straight in.
    pub target_dbfs: [f32; N_BANDS],
    pub max_gain_db: [f32; N_BANDS],
    pub knee_db: [f32; N_BANDS],
    pub release_ms: [f32; N_BANDS],
}

impl MbagcParams {
    pub const PASSTHROUGH: MbagcParams = MbagcParams {
        enabled: false,
        crossover_freq_hz: [300.0, 4000.0, 7000.0],
        target_dbfs: [0.0; N_BANDS],
        max_gain_db: [0.0; N_BANDS],
        knee_db: [0.0; N_BANDS],
        release_ms: [200.0; N_BANDS],
    };
}

pub struct MultibandAgc {
    enabled: bool,
    /// Per-band, per-channel state. Empty when disabled (bypass).
    /// LR4 = two cascaded Butterworth biquads (LR2² → 4th order).
    bands: Vec<BandState>,
    sample_rate_hz: f32,
    params: MbagcParams,
    /// Last-known per-band gain (dB) for stats.
    last_gain_db: [f32; N_BANDS],
}

/// Per-band per-channel filter cascade and envelope state.
struct BandState {
    /// Per-channel filter cascade producing this band.
    /// (low-pass-1, low-pass-2, high-pass-1, high-pass-2) per channel.
    filters: [(Biquad, Biquad, Biquad, Biquad); N_CHANNELS],
    /// Combined peak envelope (driven by max(|L|, |R|) per band).
    /// Single value because the gain is shared across L and R to preserve stereo image.
    envelope: f32,
    /// `exp(-1 / (release_ms × 1e-3 × sample_rate_hz))` — cached at preset switch
    /// instead of recomputed per sample.
    release_alpha: f32,
}

impl MultibandAgc {
    pub fn new(params: MbagcParams, sample_rate_hz: f32) -> Self {
        let mut me = Self {
            enabled: params.enabled,
            bands: Vec::new(),
            sample_rate_hz,
            params,
            last_gain_db: [0.0; N_BANDS],
        };
        if me.enabled {
            me.rebuild_filters();
        }
        me
    }

    fn rebuild_filters(&mut self) {
        self.bands.clear();
        for band in 0..N_BANDS {
            let lp_freq = if band < N_BANDS - 1 {
                Some(self.params.crossover_freq_hz[band])
            } else { None };
            let hp_freq = if band > 0 {
                Some(self.params.crossover_freq_hz[band - 1])
            } else { None };
            let lp_coeffs = lp_freq
                .map(|f| design_butterworth_lpf(f, self.sample_rate_hz))
                .unwrap_or(BiquadCoeffs::PASSTHROUGH);
            let hp_coeffs = hp_freq
                .map(|f| design_butterworth_hpf(f, self.sample_rate_hz))
                .unwrap_or(BiquadCoeffs::PASSTHROUGH);
            let make_filters = || (
                Biquad::new(lp_coeffs), Biquad::new(lp_coeffs),
                Biquad::new(hp_coeffs), Biquad::new(hp_coeffs),
            );
            let release_alpha = (-1.0_f32
                / (self.params.release_ms[band] * 1e-3 * self.sample_rate_hz)).exp();
            self.bands.push(BandState {
                filters: [make_filters(), make_filters()],
                envelope: 0.0,
                release_alpha,
            });
        }
    }

    pub fn set_params(&mut self, params: MbagcParams) {
        self.params = params;
        self.enabled = params.enabled;
        if self.enabled {
            self.rebuild_filters();
        } else {
            self.bands.clear();
        }
    }

    pub fn reset(&mut self) {
        for band in &mut self.bands {
            for ch in 0..N_CHANNELS {
                band.filters[ch].0.reset();
                band.filters[ch].1.reset();
                band.filters[ch].2.reset();
                band.filters[ch].3.reset();
            }
            band.envelope = 0.0;
        }
        self.last_gain_db = [0.0; N_BANDS];
    }

    /// Returns the most recent per-band gain (dB). Used by stats_collector.
    pub fn last_gain_db(&self) -> [f32; N_BANDS] {
        self.last_gain_db
    }

    /// Pull one sample of band `band_idx` from per-channel input via the LR4
    /// cascade. Returns `(left_band, right_band)`. The cascade is HP-then-LP;
    /// either side is `BiquadCoeffs::PASSTHROUGH` for the lowest/highest band.
    fn band_sample(&mut self, band_idx: usize, l_in: f32, r_in: f32) -> (f32, f32) {
        let band = &mut self.bands[band_idx];
        let process_chain = |x: f32, fil: &mut (Biquad, Biquad, Biquad, Biquad)| -> f32 {
            // HP cascade (lower edge — passthrough for band 0)
            let mut y = fil.2.process_sample(x);
            y = fil.3.process_sample(y);
            // LP cascade (upper edge — passthrough for last band)
            y = fil.0.process_sample(y);
            y = fil.1.process_sample(y);
            y
        };
        let l = process_chain(l_in, &mut band.filters[0]);
        let r = process_chain(r_in, &mut band.filters[1]);
        (l, r)
    }

    /// Update per-band envelope state and compute the linear gain for this
    /// sample. Envelope is peak-with-exponential-release; gain is floored at 0
    /// (lift-only, no cut) and ceiled at `max_gain_db`.
    fn band_gain_linear(&mut self, band_idx: usize, peak: f32) -> f32 {
        let alpha = self.bands[band_idx].release_alpha;
        let env = self.bands[band_idx].envelope;
        let new_env = if peak > env { peak } else { env * alpha };
        self.bands[band_idx].envelope = new_env;
        let env = new_env.max(1e-12);

        let env_dbfs = 20.0 * env.log10();
        let target = self.params.target_dbfs[band_idx];
        let max_gain = self.params.max_gain_db[band_idx];

        let raw = target - env_dbfs;
        let gain_db = raw.min(max_gain).max(0.0);  // lift-only
        self.last_gain_db[band_idx] = gain_db;
        10.0_f32.powf(gain_db / 20.0)
    }

    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        if !self.enabled { return; }
        debug_assert_eq!(buf.len() % N_CHANNELS, 0, "buf length must be a multiple of N_CHANNELS");
        for frame in buf.chunks_exact_mut(N_CHANNELS) {
            let l_in = frame[0];
            let r_in = frame[1];
            let mut l_out = 0.0_f32;
            let mut r_out = 0.0_f32;
            for band in 0..N_BANDS {
                let (lb, rb) = self.band_sample(band, l_in, r_in);
                let peak = lb.abs().max(rb.abs());
                let g = self.band_gain_linear(band, peak);
                l_out += lb * g;
                r_out += rb * g;
            }
            frame[0] = l_out;
            frame[1] = r_out;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_mbagc_is_bypass() {
        let mut m = MultibandAgc::new(MbagcParams::PASSTHROUGH, 48_000.0);
        let mut buf = vec![0.5_f32, -0.5, 0.25, -0.25];
        let copy = buf.clone();
        m.process_interleaved(&mut buf);
        assert_eq!(buf, copy);
    }

    #[test]
    fn soft_input_lifted_toward_target() {
        // Voice preset's mid-low band is targeted at -6 dBFS (16406 / 32768).
        // A −20 dBFS sine in the mid-low band should be lifted toward target.
        let mut params = MbagcParams::PASSTHROUGH;
        params.enabled = true;
        params.target_dbfs = [-12.0, -6.0, -29.0, -35.0];
        params.max_gain_db = [6.0, 20.0, 10.0, 10.0];
        params.knee_db = [5.0, 5.0, 5.0, 5.0];
        params.release_ms = [200.0, 200.0, 200.0, 200.0];
        let mut m = MultibandAgc::new(params, 48_000.0);

        // 1 kHz sine at −20 dBFS, stereo, 2 seconds (96_000 frames).
        let amp = 10.0_f32.powf(-20.0 / 20.0); // 0.1
        let mut buf: Vec<f32> = (0..96_000)
            .flat_map(|i| {
                let s = amp * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin();
                [s, s]
            })
            .collect();
        m.process_interleaved(&mut buf);

        // RMS over second half of L channel: indices 96_000.. step_by 2 = 48_000 samples.
        let l_sq: f32 = buf[96_000..].iter().step_by(2).map(|x| x * x).sum();
        let l_rms = (l_sq / 48_000_f32).sqrt();
        let l_db = 20.0 * l_rms.log10() - 20.0 * (1.0_f32 / 2.0_f32.sqrt()).log10();
        // mid-low band target is -6 dBFS, max gain 20 dB. Expect within ±3 dB of target.
        assert!((l_db - (-6.0)).abs() < 3.0, "expected ≈ -6 dBFS, got {l_db:.2} dBFS");
    }

    #[test]
    fn hot_input_attenuated_below_max_gain() {
        // 0 dBFS input — gain calculation says we should attenuate (gain < 0),
        // but the gain is floored at 0.0 (only lift, never cut). Output stays ≈ 0 dBFS.
        let mut params = MbagcParams::PASSTHROUGH;
        params.enabled = true;
        params.target_dbfs = [-12.0, -6.0, -29.0, -35.0];
        params.max_gain_db = [6.0, 20.0, 10.0, 10.0];
        params.knee_db = [5.0, 5.0, 5.0, 5.0];
        params.release_ms = [200.0, 200.0, 200.0, 200.0];
        let mut m = MultibandAgc::new(params, 48_000.0);

        let mut buf: Vec<f32> = (0..96_000)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin();
                [s, s]
            })
            .collect();
        m.process_interleaved(&mut buf);

        let l_sq: f32 = buf[96_000..].iter().step_by(2).map(|x| x * x).sum();
        let l_rms = (l_sq / 48_000_f32).sqrt();
        let l_db = 20.0 * l_rms.log10() - 20.0 * (1.0_f32 / 2.0_f32.sqrt()).log10();
        // Should stay ≈ 0 dBFS (no cut). Within 1 dB.
        assert!(l_db.abs() < 1.0, "expected ≈ 0 dBFS, got {l_db:.2} dBFS");
    }
}
