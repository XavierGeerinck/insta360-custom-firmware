//! Parametric EQ: 4 biquad bands per channel. Each band has a kind, center
//! frequency, gain, and Q. Cascade order = array order.

use serde::{Deserialize, Serialize};

use super::biquad::{
    Biquad, BiquadCoeffs,
    design_butterworth_hpf, design_butterworth_lpf, design_first_order_high_shelf,
    design_first_order_low_shelf, design_high_shelf, design_low_shelf, design_peaking_eq,
    design_stock_peaking,
};

/// Number of EQ bands per channel.
pub const N_BANDS: usize = 4;
/// Number of channels (always stereo on this hardware).
pub const N_CHANNELS: usize = 2;

/// `Peaking`, `LowShelf` and `HighShelf` are the RBJ cookbook forms. The
/// rest are stock's `bva::asp::asp_iir_eq_init` types (`preset.rs` maps the
/// JSON codes; `docs/AUDIO_HARDWARE.md` "Stock speaker tuning").
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BiquadKind {
    Bypass,
    Peaking,
    LowShelf,
    HighShelf,
    /// Stock type 0: peaking with stock's Q convention.
    StockPeaking,
    /// Stock type 1: first-order low shelf (Q unused).
    LowShelf1,
    /// Stock type 2: first-order high shelf (Q unused).
    HighShelf1,
    /// Stock type 3: Butterworth low-pass (gain and Q unused).
    LowPass,
    /// Stock type 4: Butterworth high-pass (gain and Q unused).
    HighPass,
}

/// True when any band of `params` changes the signal.
pub fn is_active(params: &[[BiquadParams; N_BANDS]; N_CHANNELS]) -> bool {
    params.iter().flatten().any(|p| p.kind != BiquadKind::Bypass)
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BiquadParams {
    pub kind: BiquadKind,
    pub center_freq_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

impl BiquadParams {
    /// All non-`kind` fields are unused when `kind == Bypass` (`coeffs()`
    /// returns `BiquadCoeffs::PASSTHROUGH` without reading them). Set to 0.0
    /// so a serialised preset doesn't carry misleading tuning numbers.
    pub const BYPASS: BiquadParams = BiquadParams {
        kind: BiquadKind::Bypass,
        center_freq_hz: 0.0,
        gain_db: 0.0,
        q: 0.0,
    };

    pub fn coeffs(&self, sample_rate_hz: f32) -> BiquadCoeffs {
        match self.kind {
            BiquadKind::Bypass    => BiquadCoeffs::PASSTHROUGH,
            BiquadKind::Peaking   => design_peaking_eq(self.center_freq_hz, self.gain_db, self.q, sample_rate_hz),
            BiquadKind::LowShelf  => design_low_shelf(self.center_freq_hz, self.gain_db, self.q, sample_rate_hz),
            BiquadKind::HighShelf => design_high_shelf(self.center_freq_hz, self.gain_db, self.q, sample_rate_hz),
            BiquadKind::StockPeaking => design_stock_peaking(self.center_freq_hz, self.gain_db, self.q, sample_rate_hz),
            BiquadKind::LowShelf1 => design_first_order_low_shelf(self.center_freq_hz, self.gain_db, sample_rate_hz),
            BiquadKind::HighShelf1 => design_first_order_high_shelf(self.center_freq_hz, self.gain_db, sample_rate_hz),
            BiquadKind::LowPass => design_butterworth_lpf(self.center_freq_hz, sample_rate_hz),
            BiquadKind::HighPass => design_butterworth_hpf(self.center_freq_hz, sample_rate_hz),
        }
    }
}

pub struct ParametricEq {
    /// `[ch][band]` biquad state.
    biquads: [[Biquad; N_BANDS]; N_CHANNELS],
    sample_rate_hz: f32,
}

impl ParametricEq {
    pub fn new(params: &[[BiquadParams; N_BANDS]; N_CHANNELS], sample_rate_hz: f32) -> Self {
        let make_band = |ch: usize, band: usize| Biquad::new(params[ch][band].coeffs(sample_rate_hz));
        Self {
            biquads: [
                [make_band(0, 0), make_band(0, 1), make_band(0, 2), make_band(0, 3)],
                [make_band(1, 0), make_band(1, 1), make_band(1, 2), make_band(1, 3)],
            ],
            sample_rate_hz,
        }
    }

    pub fn set_params(&mut self, params: &[[BiquadParams; N_BANDS]; N_CHANNELS]) {
        for ch in 0..N_CHANNELS {
            for band in 0..N_BANDS {
                self.biquads[ch][band].set_coeffs(params[ch][band].coeffs(self.sample_rate_hz));
            }
        }
    }

    pub fn reset(&mut self) {
        for ch in 0..N_CHANNELS {
            for band in 0..N_BANDS {
                self.biquads[ch][band].reset();
            }
        }
    }

    /// Process interleaved stereo f32 in place. `buf.len()` must be a multiple of `N_CHANNELS`.
    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        debug_assert_eq!(buf.len() % N_CHANNELS, 0, "buf length must be a multiple of N_CHANNELS");
        for frame in buf.chunks_exact_mut(N_CHANNELS) {
            for ch in 0..N_CHANNELS {
                let mut x = frame[ch];
                for band in 0..N_BANDS {
                    x = self.biquads[ch][band].process_sample(x);
                }
                frame[ch] = x;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> [[BiquadParams; N_BANDS]; N_CHANNELS] {
        [[BiquadParams::BYPASS; N_BANDS]; N_CHANNELS]
    }

    #[test]
    fn stock_kinds_use_the_decoded_designers() {
        use super::super::biquad::{
            design_butterworth_hpf, design_butterworth_lpf, design_first_order_high_shelf,
            design_first_order_low_shelf, design_stock_peaking,
        };
        let p = |kind| BiquadParams { kind, center_freq_hz: 500.0, gain_db: 4.0, q: 0.7 };
        let fs = 48_000.0;
        assert_eq!(p(BiquadKind::StockPeaking).coeffs(fs), design_stock_peaking(500.0, 4.0, 0.7, fs));
        assert_eq!(p(BiquadKind::LowShelf1).coeffs(fs), design_first_order_low_shelf(500.0, 4.0, fs));
        assert_eq!(p(BiquadKind::HighShelf1).coeffs(fs), design_first_order_high_shelf(500.0, 4.0, fs));
        assert_eq!(p(BiquadKind::LowPass).coeffs(fs), design_butterworth_lpf(500.0, fs));
        assert_eq!(p(BiquadKind::HighPass).coeffs(fs), design_butterworth_hpf(500.0, fs));
    }

    #[test]
    fn biquad_kinds_round_trip_through_json() {
        for kind in [
            BiquadKind::Bypass,
            BiquadKind::Peaking,
            BiquadKind::LowShelf,
            BiquadKind::HighShelf,
            BiquadKind::StockPeaking,
            BiquadKind::LowShelf1,
            BiquadKind::HighShelf1,
            BiquadKind::LowPass,
            BiquadKind::HighPass,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(serde_json::from_str::<BiquadKind>(&json).unwrap(), kind, "{json}");
        }
        assert_eq!(serde_json::to_string(&BiquadKind::StockPeaking).unwrap(), "\"stock_peaking\"");
    }

    #[test]
    fn eq_reports_whether_it_does_anything() {
        let mut params = flat();
        assert!(!is_active(&params));
        params[1][2] = BiquadParams { kind: BiquadKind::HighPass, center_freq_hz: 100.0, gain_db: 0.0, q: 0.7 };
        assert!(is_active(&params));
    }

    #[test]
    fn bypass_eq_is_passthrough() {
        let mut eq = ParametricEq::new(&flat(), 48_000.0);
        let mut buf: Vec<f32> = (0..960).map(|i| (i as f32 * 0.01).sin()).collect();
        let original = buf.clone();
        eq.process_interleaved(&mut buf);
        for (a, b) in buf.iter().zip(original.iter()) {
            assert!((a - b).abs() < 1e-6, "bypass should not modify samples");
        }
    }

    #[test]
    fn peaking_band_lifts_target_frequency() {
        // Generate 48_000 stereo frames (= 96_000 interleaved samples) of a 1 kHz sine.
        // Then take the second half on the L channel only: indices 48_000.. step_by 2 = 24_000 L samples.
        let mut params = flat();
        for ch in 0..N_CHANNELS {
            params[ch][0] = BiquadParams { kind: BiquadKind::Peaking, center_freq_hz: 1000.0, gain_db: 6.0, q: 1.0 };
        }
        let mut eq = ParametricEq::new(&params, 48_000.0);
        let mut buf: Vec<f32> = (0..48_000)
            .flat_map(|i| {
                let t = i as f32 / 48_000.0;
                let s = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
                [s, s]
            })
            .collect();
        eq.process_interleaved(&mut buf);

        // Second half of L channel: indices 48_000, 48_002, ... 95_998 → 24_000 samples.
        let l_rms = {
            let s: f32 = buf[48_000..].iter().step_by(2).map(|x| x * x).sum();
            (s / 24_000_f32).sqrt()
        };
        let l_db = 20.0 * l_rms.log10() - 20.0 * (1.0_f32 / 2.0_f32.sqrt()).log10();
        assert!((l_db - 6.0).abs() < 0.5, "expected +6 dB lift at 1 kHz, got {l_db:.2} dB");
    }
}
