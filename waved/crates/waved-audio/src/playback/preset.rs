//! Playback preset definitions.
//!
//! Stock's speaker path runs no DSP at all: input → volume → `hw:0,1`; the
//! only speaker tuning is the amplifier's own `Set EQ MODE`
//! (`crate::playback::scene`, `docs/AUDIO_HARDWARE.md` "Stock speaker tuning").
//!
//! - `PlaybackPreset::Media` — stock's speaker path: identity. No HPF, EQ,
//!   MBAGC or limiter; the samples pass through untouched.
//! - `PlaybackPreset::Voice` — waved's call tuning: stock's
//!   `dynamic_eq_config_2.json` with its switches honoured (no HPF, no MBAGC,
//!   the four peaking filters) plus the limiter, because the EQ boosts. Stock
//!   ships that file but loads it nowhere.

use serde::Deserialize;
use std::sync::OnceLock;

pub use waved_state::PlaybackPreset;

use super::dsp::eq::{self, BiquadKind, BiquadParams, N_BANDS, N_CHANNELS};
use super::dsp::limiter::LimiterParams;
use super::dsp::mbagc::{MbagcParams, N_BANDS as MBAGC_N_BANDS};

/// Concrete tuning for one preset. Drives `PlaybackDspChain`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackPresetTuning {
    /// High-pass cutoff; `None` = no high-pass (stock `hpf_switch: 0`).
    pub hpf_cutoff_hz: Option<f32>,
    pub eq: [[BiquadParams; N_BANDS]; N_CHANNELS],
    pub mbagc: MbagcParams,
    /// `None` = no limiter: nothing in the tuning adds gain.
    pub limiter: Option<LimiterParams>,
}

impl PlaybackPresetTuning {
    /// Every stage off: the chain hands samples through untouched.
    pub const IDENTITY: PlaybackPresetTuning = PlaybackPresetTuning {
        hpf_cutoff_hz: None,
        eq: [[BiquadParams::BYPASS; N_BANDS]; N_CHANNELS],
        mbagc: MbagcParams::PASSTHROUGH,
        limiter: None,
    };

    /// True when no stage changes the signal.
    pub fn is_identity(&self) -> bool {
        self.hpf_cutoff_hz.is_none()
            && !eq::is_active(&self.eq)
            && !self.mbagc.enabled
            && self.limiter.is_none()
    }
}

/// Stock JSON shape — only the fields we care about. Rest are tolerated.
#[derive(Debug, Deserialize)]
struct StockConfig {
    asp_param_eq: AspParamEq,
}

/// Field names and meaning from `objASP_Param` (libInstaAudio.so DWARF,
/// `nlp/asp/common.h`). A stage runs only when its switch is exactly 1
/// (`aspDynamicEqInitApi` / `aspDynamicEqProcessApi`, libbva_audio.so); a
/// missing switch reads as 0.
#[derive(Debug, Deserialize)]
struct AspParamEq {
    #[serde(default)]
    hpf_switch: i32,
    #[serde(default)]
    agc_switch: i32,
    #[serde(default)]
    mbagc_switch: i32,
    #[serde(default)]
    iir_eq_switch: i32,
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
    /// `bva::asp::asp_iir_eq_init` type per filter, see [`stock_filter_kind`].
    #[serde(rename = "type")]
    kinds: Vec<i32>,
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

/// Stock's speaker path: nothing between the volume and the amplifier.
pub fn media_preset() -> PlaybackPresetTuning {
    PlaybackPresetTuning::IDENTITY
}

/// Stock filter type code → kind (`asp_iir_eq_init`, libbva_audio.so
/// 0x2a5c0): 0 peaking, 1 first-order low shelf, 2 first-order high shelf,
/// 3 Butterworth low-pass, 4 Butterworth high-pass. Stock turns any other
/// code into a passthrough section; so do we.
pub fn stock_filter_kind(code: i32) -> BiquadKind {
    match code {
        0 => BiquadKind::StockPeaking,
        1 => BiquadKind::LowShelf1,
        2 => BiquadKind::HighShelf1,
        3 => BiquadKind::LowPass,
        4 => BiquadKind::HighPass,
        _ => BiquadKind::Bypass,
    }
}

/// True when a band can lift some frequency above unity.
fn can_boost(p: &BiquadParams) -> bool {
    match p.kind {
        BiquadKind::Bypass | BiquadKind::LowPass | BiquadKind::HighPass => false,
        _ => p.gain_db > 0.0,
    }
}

pub fn parse_stock_json(s: &str) -> Result<PlaybackPresetTuning, String> {
    let cfg: StockConfig = serde_json::from_str(s).map_err(|e| format!("parse: {e}"))?;
    let p = cfg.asp_param_eq;

    if p.agc_switch == 1 {
        return Err("agc_switch 1: the full-band AGC is not implemented".into());
    }

    let mbagc = match parse_mbagc(&p) {
        Ok(m) => MbagcParams { enabled: p.mbagc_switch == 1, ..m },
        Err(e) if p.mbagc_switch == 1 => return Err(e),
        Err(_) => MbagcParams::PASSTHROUGH,
    };

    if p.eq_filter_num > N_BANDS {
        return Err(format!("eq_filter_num {} exceeds the {} bands", p.eq_filter_num, N_BANDS));
    }
    let mut bands = [[BiquadParams::BYPASS; N_BANDS]; N_CHANNELS];
    if p.iir_eq_switch == 1 {
        let eq_set = p.eq_settings.first().ok_or("eq_settings empty")?;
        for band in 0..p.eq_filter_num {
            let field = |v: &Vec<f32>, name: &str| {
                v.get(band).copied().ok_or(format!("{name} has no value for filter {band}"))
            };
            let code = *eq_set.kinds.get(band).ok_or(format!("type has no value for filter {band}"))?;
            let params = match stock_filter_kind(code) {
                BiquadKind::Bypass => BiquadParams::BYPASS,
                kind => BiquadParams {
                    kind,
                    center_freq_hz: field(&eq_set.center_freq, "center_freq")?,
                    gain_db: field(&eq_set.gain_dB, "gain_dB")?,
                    q: field(&eq_set.Q, "Q")?,
                },
            };
            for channel in bands.iter_mut() {
                channel[band] = params;
            }
        }
    }

    let adds_gain = mbagc.enabled || bands.iter().flatten().any(can_boost);
    Ok(PlaybackPresetTuning {
        hpf_cutoff_hz: (p.hpf_switch == 1).then_some(p.cut_off_freq),
        eq: bands,
        mbagc,
        limiter: adds_gain.then_some(LimiterParams::DEFAULT),
    })
}

fn parse_mbagc(p: &AspParamEq) -> Result<MbagcParams, String> {
    if p.multi_band_num != MBAGC_N_BANDS {
        return Err(format!("multi_band_num expected {}, got {}", MBAGC_N_BANDS, p.multi_band_num));
    }
    let agc = p.agc_settings.first().ok_or("agc_settings empty")?;
    let four = |v: &Vec<f32>, name: &str| -> Result<[f32; MBAGC_N_BANDS], String> {
        v.as_slice().try_into().map_err(|_| format!("{name}: expected {MBAGC_N_BANDS} values, got {}", v.len()))
    };
    let crossover: [f32; 3] = agc.crossover_freq.as_slice().try_into()
        .map_err(|_| format!("expected 3 crossovers, got {}", agc.crossover_freq.len()))?;
    Ok(MbagcParams {
        enabled: true,
        crossover_freq_hz: crossover,
        target_dbfs: convert_target_to_dbfs(&agc.mbagc_target)?,
        max_gain_db: four(&agc.mbagc_max_gain, "mbagc_max_gain")?,
        knee_db: four(&agc.mbagc_gain_knee, "mbagc_gain_knee")?,
        release_ms: four(&agc.mbagc_release_time, "mbagc_release_time")?,
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

pub fn tuning_for(preset: PlaybackPreset) -> PlaybackPresetTuning {
    match preset {
        PlaybackPreset::Voice => voice_preset().clone(),
        PlaybackPreset::Media => media_preset(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOCK_MUSIC_JSON: &str = include_str!("../../data/dynamic_eq_config.json");

    /// The voice file with one `"key": value` replaced.
    fn voice_with(key: &str, value: &str) -> String {
        let needle = format!("\"{key}\": ");
        let start = STOCK_VOICE_JSON.find(&needle).expect(key) + needle.len();
        let end = start + STOCK_VOICE_JSON[start..].find(',').unwrap();
        format!("{}{}{}", &STOCK_VOICE_JSON[..start], value, &STOCK_VOICE_JSON[end..])
    }

    #[test]
    fn voice_preset_honours_the_stock_switches() {
        // dynamic_eq_config_2.json: hpf_switch 0, mbagc_switch 0, agc_switch
        // 0, iir_eq_switch 1.
        let v = voice_preset();
        assert_eq!(v.hpf_cutoff_hz, None, "hpf_switch 0 → no high-pass");
        assert!(!v.mbagc.enabled, "mbagc_switch 0 → no MBAGC");
        assert_eq!(v.limiter, Some(LimiterParams::DEFAULT), "the EQ boosts, so the limiter stays");
        assert!(!v.is_identity());
    }

    #[test]
    fn voice_preset_eq_is_stocks_four_peaking_filters() {
        let v = voice_preset();
        let want = [(150.0, -6.0, 1.0), (800.0, 2.0, 2.0), (400.0, -5.0, 1.5), (3500.0, 5.0, 2.0)];
        for channel in &v.eq {
            for (band, (f, g, q)) in channel.iter().zip(want) {
                assert_eq!(*band, BiquadParams { kind: BiquadKind::StockPeaking, center_freq_hz: f, gain_db: g, q });
            }
        }
    }

    #[test]
    fn voice_preset_keeps_the_mbagc_numbers_while_off() {
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
        assert_eq!(v.mbagc.crossover_freq_hz, [300.0, 4000.0, 7000.0]);
    }

    #[test]
    fn stock_music_file_parses_with_the_decoded_types() {
        // dynamic_eq_config.json: type [4, 2, 0], 150 / 8000 / 90 Hz,
        // 0 / +5 / −6 dB, Q 0.7 / 0.7 / 2.0, eq_filter_num 3.
        let t = parse_stock_json(STOCK_MUSIC_JSON).unwrap();
        let want = [
            BiquadParams { kind: BiquadKind::HighPass, center_freq_hz: 150.0, gain_db: 0.0, q: 0.7 },
            BiquadParams { kind: BiquadKind::HighShelf1, center_freq_hz: 8000.0, gain_db: 5.0, q: 0.7 },
            BiquadParams { kind: BiquadKind::StockPeaking, center_freq_hz: 90.0, gain_db: -6.0, q: 2.0 },
            BiquadParams::BYPASS,
        ];
        assert_eq!(t.eq, [want, want]);
        assert_eq!(t.hpf_cutoff_hz, None);
        assert!(!t.mbagc.enabled);
        assert_eq!(t.limiter, Some(LimiterParams::DEFAULT), "+5 dB shelf → limiter");
    }

    #[test]
    fn hpf_switch_one_uses_the_cut_off_freq() {
        let t = parse_stock_json(&voice_with("hpf_switch", "1")).unwrap();
        assert_eq!(t.hpf_cutoff_hz, Some(30.0));
    }

    #[test]
    fn mbagc_switch_one_enables_the_mbagc() {
        let t = parse_stock_json(&voice_with("mbagc_switch", "1")).unwrap();
        assert!(t.mbagc.enabled);
        assert_eq!(t.mbagc.max_gain_db, [6.0, 20.0, 10.0, 10.0]);
    }

    #[test]
    fn agc_switch_one_is_rejected() {
        let err = parse_stock_json(&voice_with("agc_switch", "1")).unwrap_err();
        assert!(err.contains("agc_switch"), "{err}");
    }

    #[test]
    fn iir_eq_switch_zero_flattens_and_drops_the_limiter() {
        let t = parse_stock_json(&voice_with("iir_eq_switch", "0")).unwrap();
        assert!(!eq::is_active(&t.eq));
        assert_eq!(t.limiter, None);
        assert!(t.is_identity(), "every switch off → identity");
    }

    #[test]
    fn unknown_filter_types_pass_through_like_stock() {
        let json = STOCK_MUSIC_JSON.replace("[4,\t\t2,\t\t0]", "[7,\t\t2,\t\t0]");
        assert_ne!(json, STOCK_MUSIC_JSON, "fixture must contain the type array");
        let t = parse_stock_json(&json).unwrap();
        assert_eq!(t.eq[0][0], BiquadParams::BYPASS);
        assert_eq!(t.eq[0][1].kind, BiquadKind::HighShelf1);
    }

    #[test]
    fn more_filters_than_bands_is_rejected() {
        let err = parse_stock_json(&voice_with("eq_filter_num", "5")).unwrap_err();
        assert!(err.contains("eq_filter_num"), "{err}");
    }

    #[test]
    fn media_preset_is_stocks_speaker_path() {
        assert!(media_preset().is_identity());
        assert!(tuning_for(PlaybackPreset::Media).is_identity());
        assert_eq!(tuning_for(PlaybackPreset::Voice), *voice_preset());
    }

    #[test]
    fn stock_codes_map_to_kinds() {
        assert_eq!(stock_filter_kind(0), BiquadKind::StockPeaking);
        assert_eq!(stock_filter_kind(1), BiquadKind::LowShelf1);
        assert_eq!(stock_filter_kind(2), BiquadKind::HighShelf1);
        assert_eq!(stock_filter_kind(3), BiquadKind::LowPass);
        assert_eq!(stock_filter_kind(4), BiquadKind::HighPass);
        assert_eq!(stock_filter_kind(5), BiquadKind::Bypass);
        assert_eq!(stock_filter_kind(-1), BiquadKind::Bypass);
    }
}
