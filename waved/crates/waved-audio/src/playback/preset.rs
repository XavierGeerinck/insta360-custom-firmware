//! Playback preset definitions.
//!
//! `PlaybackPreset::Voice` — conferencing tuning, matches stock
//! `dynamic_eq_config_2.json` byte-for-byte (HPF + EQ + MBAGC + limiter).
//! `PlaybackPreset::Media` — music tuning derived from Voice with MBAGC
//! bypassed and EQ flattened.

use serde::Deserialize;
use std::sync::OnceLock;

pub use waved_state::PlaybackPreset;

use super::dsp::eq::{BiquadKind, BiquadParams, N_BANDS, N_CHANNELS};
use super::dsp::limiter::LimiterParams;
use super::dsp::mbagc::{MbagcParams, N_BANDS as MBAGC_N_BANDS};

/// Concrete tuning for one preset. Drives `PlaybackDspChain::set_preset`.
#[derive(Debug, Clone)]
pub struct PlaybackPresetTuning {
    pub hpf_cutoff_hz: f32,
    pub eq: [[BiquadParams; N_BANDS]; N_CHANNELS],
    pub mbagc: MbagcParams,
    pub limiter: LimiterParams,
}

/// Stock JSON shape — only the fields we care about. Rest are tolerated.
#[derive(Debug, Deserialize)]
struct StockConfig {
    asp_param_eq: AspParamEq,
}

#[derive(Debug, Deserialize)]
struct AspParamEq {
    cut_off_freq: f32,
    multi_band_num: usize,
    agc_settings: Vec<AgcSetting>,
    eq_filter_num: usize,
    eq_settings: Vec<EqSetting>,
}

#[derive(Debug, Deserialize)]
struct AgcSetting {
    crossover_freq: Vec<f32>,
    mbagc_target: Vec<f32>,
    mbagc_max_gain: Vec<f32>,
    mbagc_gain_knee: Vec<f32>,
    mbagc_release_time: Vec<f32>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct EqSetting {
    /// Stock encodes type per band: 0=peaking, 1=low shelf, 2=high shelf.
    #[serde(rename = "type")]
    kinds: Vec<u8>,
    center_freq: Vec<f32>,
    gain_dB: Vec<f32>,
    Q: Vec<f32>,
}

const STOCK_VOICE_JSON: &str = include_str!("../../data/dynamic_eq_config_2.json");

static VOICE_PRESET: OnceLock<PlaybackPresetTuning> = OnceLock::new();

/// Returns the embedded voice preset. Parses the vendored stock JSON on first
/// call. Panics with a descriptive message if the JSON is malformed — caught
/// in unit tests, never at runtime if we ship.
pub fn voice_preset() -> &'static PlaybackPresetTuning {
    VOICE_PRESET.get_or_init(|| parse_stock_json(STOCK_VOICE_JSON)
        .expect("vendored stock JSON must parse — check crates/waved-audio/data/"))
}

fn parse_stock_json(s: &str) -> Result<PlaybackPresetTuning, String> {
    let cfg: StockConfig = serde_json::from_str(s).map_err(|e| format!("parse: {e}"))?;
    let p = cfg.asp_param_eq;

    if p.multi_band_num != MBAGC_N_BANDS {
        return Err(format!("multi_band_num expected {}, got {}", MBAGC_N_BANDS, p.multi_band_num));
    }
    if p.eq_filter_num != N_BANDS {
        return Err(format!("eq_filter_num expected {}, got {}", N_BANDS, p.eq_filter_num));
    }

    let agc = p.agc_settings.first().ok_or("agc_settings empty")?;
    if agc.crossover_freq.len() != 3 {
        return Err(format!("expected 3 crossovers, got {}", agc.crossover_freq.len()));
    }
    let mbagc = MbagcParams {
        enabled: true,
        crossover_freq_hz: [agc.crossover_freq[0], agc.crossover_freq[1], agc.crossover_freq[2]],
        target_dbfs: convert_target_to_dbfs(&agc.mbagc_target)?,
        max_gain_db: [agc.mbagc_max_gain[0], agc.mbagc_max_gain[1], agc.mbagc_max_gain[2], agc.mbagc_max_gain[3]],
        knee_db: [agc.mbagc_gain_knee[0], agc.mbagc_gain_knee[1], agc.mbagc_gain_knee[2], agc.mbagc_gain_knee[3]],
        release_ms: [agc.mbagc_release_time[0], agc.mbagc_release_time[1], agc.mbagc_release_time[2], agc.mbagc_release_time[3]],
    };

    let eq_set = p.eq_settings.first().ok_or("eq_settings empty")?;
    let mut bands = [[BiquadParams::BYPASS; N_BANDS]; N_CHANNELS];
    for band in 0..N_BANDS {
        let kind = match eq_set.kinds.get(band).copied().unwrap_or(0) {
            0 => BiquadKind::Peaking,
            1 => BiquadKind::LowShelf,
            2 => BiquadKind::HighShelf,
            other => return Err(format!("unknown eq type code {other}")),
        };
        let params = BiquadParams {
            kind,
            center_freq_hz: eq_set.center_freq[band],
            gain_db: eq_set.gain_dB[band],
            q: eq_set.Q[band],
        };
        bands[0][band] = params;
        bands[1][band] = params;
    }

    Ok(PlaybackPresetTuning {
        hpf_cutoff_hz: p.cut_off_freq,
        eq: bands,
        mbagc,
        limiter: LimiterParams::DEFAULT,
    })
}

/// Stock targets are i16-amplitude peak values (e.g. 8203 ≈ 0.25 fullscale).
/// Convert to dBFS (relative to 32768 = 0 dBFS).
fn convert_target_to_dbfs(raw: &[f32]) -> Result<[f32; MBAGC_N_BANDS], String> {
    if raw.len() != MBAGC_N_BANDS {
        return Err(format!("expected {} mbagc_target values, got {}", MBAGC_N_BANDS, raw.len()));
    }
    let mut out = [0.0_f32; MBAGC_N_BANDS];
    for (i, &v) in raw.iter().enumerate() {
        let normalized = (v / 32768.0).max(1e-6);
        out[i] = 20.0 * normalized.log10();
    }
    Ok(out)
}

/// Media preset = Voice with MBAGC bypassed and EQ flattened.
pub fn media_preset_from_voice(voice: &PlaybackPresetTuning) -> PlaybackPresetTuning {
    PlaybackPresetTuning {
        hpf_cutoff_hz: voice.hpf_cutoff_hz,
        eq: [[BiquadParams::BYPASS; N_BANDS]; N_CHANNELS],
        mbagc: MbagcParams { enabled: false, ..voice.mbagc },
        limiter: voice.limiter,
    }
}

pub fn tuning_for(preset: PlaybackPreset) -> PlaybackPresetTuning {
    let voice = voice_preset();
    match preset {
        PlaybackPreset::Voice => voice.clone(),
        PlaybackPreset::Media => media_preset_from_voice(voice),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_preset_loads_without_panic() {
        let v = voice_preset();
        assert!(v.hpf_cutoff_hz >= 20.0 && v.hpf_cutoff_hz <= 200.0,
                "voice HPF cutoff {} out of expected range", v.hpf_cutoff_hz);
        assert!(v.mbagc.enabled, "voice preset MBAGC should be enabled");
        assert_eq!(v.mbagc.crossover_freq_hz, [300.0, 4000.0, 7000.0]);
    }

    #[test]
    fn voice_preset_target_dbfs_matches_stock() {
        // Stock numbers: [8203, 16406, 1160, 580] / 32768 → dBFS
        let v = voice_preset();
        let expected = [
            20.0_f32 * (8203.0_f32 / 32768.0).log10(),
            20.0_f32 * (16406.0_f32 / 32768.0).log10(),
            20.0_f32 * (1160.0_f32 / 32768.0).log10(),
            20.0_f32 * (580.0_f32 / 32768.0).log10(),
        ];
        for (i, (a, b)) in v.mbagc.target_dbfs.iter().zip(expected.iter()).enumerate() {
            assert!((a - b).abs() < 0.01, "band {i}: got {a}, expected {b}");
        }
    }

    #[test]
    fn media_preset_disables_mbagc_and_flattens_eq() {
        let v = voice_preset();
        let m = media_preset_from_voice(v);
        assert!(!m.mbagc.enabled);
        assert_eq!(m.eq[0][0].kind, BiquadKind::Bypass);
        assert_eq!(m.eq[0][3].kind, BiquadKind::Bypass);
        assert_eq!(m.hpf_cutoff_hz, v.hpf_cutoff_hz);  // HPF stays
    }
}
