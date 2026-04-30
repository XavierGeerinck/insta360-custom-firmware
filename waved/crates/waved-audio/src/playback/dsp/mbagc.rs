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
/// `filters` and `envelope` are populated by `rebuild_filters`/`reset` here
/// and consumed by Task 7's `band_sample`/`band_gain_linear`. The allow
/// silences dead_code warnings during the bypass-only window.
#[allow(dead_code)]
struct BandState {
    /// Per-channel filter cascade producing this band.
    /// For each channel: (low-pass-1, low-pass-2, high-pass-1, high-pass-2).
    /// Band 0 (lowest): passes content below crossover[0] — uses LP cascade only.
    /// Band 1..N-1: pass content between crossover[band-1] and crossover[band] —
    ///   apply HP at lower edge, LP at upper edge.
    /// Band N-1 (highest): passes above crossover[N-2] — HP cascade only.
    filters: [(Biquad, Biquad, Biquad, Biquad); N_CHANNELS],
    /// Envelope state per channel (peak with IIR release).
    envelope: [f32; N_CHANNELS],
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
            self.bands.push(BandState {
                filters: [make_filters(), make_filters()],
                envelope: [0.0; N_CHANNELS],
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
                band.envelope[ch] = 0.0;
            }
        }
        self.last_gain_db = [0.0; N_BANDS];
    }

    /// Returns the most recent per-band gain (dB). Used by stats_collector.
    pub fn last_gain_db(&self) -> [f32; N_BANDS] {
        self.last_gain_db
    }

    /// Bypass path — Task 7 replaces this with the envelope+gain implementation.
    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        if !self.enabled { return; }
        // Until Task 7 lands, "enabled" is a no-op so the chain composes.
        let _ = buf;
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
    fn enabled_mbagc_currently_no_op_until_task_7() {
        let mut params = MbagcParams::PASSTHROUGH;
        params.enabled = true;
        let mut m = MultibandAgc::new(params, 48_000.0);
        let mut buf = vec![0.5_f32, -0.5];
        let copy = buf.clone();
        m.process_interleaved(&mut buf);
        // Task 7 will replace this assertion.
        assert_eq!(buf, copy);
    }
}
