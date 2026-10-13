//! Stats collector: taps the frame stream via a broadcast channel, computes
//! per-channel metrics every N frames, writes /tmp/audio_stats.json with
//! atomic rename. Never in the capture hot path.

use crate::callbacks::AudioFrame;
use crate::capture::{CaptureStats, ChannelMapSource};
use crate::metrics::{clipping_rate, correlation_matrix, dc_offset, rms_per_channel};
use crate::playback::DspMetrics;
use crate::processed_frame::{AecFrameMetrics, BeamMetrics, ProcessedAudioFrame};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tracing::{info, warn};
use waved_state::{AudioMode, AudioState};

#[derive(Serialize, Debug, Clone)]
pub struct AudioStats {
    pub last_updated_ms: u64,
    pub channel_map_source: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames_emitted: u64,
    pub frames_dropped: u64,
    pub xrun_count: u64,
    pub raw_frames_dropped: u64,
    pub processed_frames_dropped: u64,
    pub rms_per_channel: Vec<f32>,
    pub clipping_rate_per_channel: Vec<f32>,
    pub dc_offset_per_channel: Vec<f32>,
    pub correlation_matrix: Vec<Vec<f32>>,
    pub last_write_error: Option<String>,
    pub reference_rms: Vec<f32>,
    pub alignment: String,
    /// Legacy name for the final processed/uplink RMS after noise suppression,
    /// user microphone gain, and limiting. Kept for compatibility.
    pub mic_post_aec_rms: Option<f32>,
    /// Unambiguous name for the same final processed/uplink RMS.
    pub mic_uplink_rms: Option<f32>,
    /// Same-frame metrics captured inside the DSP pipeline. Unlike the legacy
    /// raw/processed broadcasts, every stage in this snapshot belongs to the
    /// same audio frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aec: Option<AecFrameMetrics>,
    /// Where the beam points and its blend towards the plain average, from
    /// the latest processed frame. Absent until a beamformed frame arrives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beam: Option<BeamMetrics>,
    pub doa_horizontal: f32,
    pub doa_pitch: f32,
    pub doa_active: bool,
    pub doa_backend_fallback: bool,
    pub mode: AudioMode,
    /// The preset the playback thread runs (`voice` / `media`); the last
    /// `SetPlaybackPreset` before playback starts or without playback.
    pub playback_preset: String,
    /// Speaker scene (`playback::scene`): `music` / `meeting`; null without
    /// the scene follower.
    pub speaker_scene: Option<String>,
    /// The playback preset follows the scene (false = pinned).
    pub speaker_scene_auto: bool,
    /// Amplifier `Set EQ MODE` last written (0 bypass, 1 meeting, 2 music);
    /// null until written, after a failed write, or without the follower.
    pub amp_eq_mode: Option<u8>,
    /// The amp mode follows the scene (false = pinned).
    pub amp_eq_mode_auto: bool,
    pub mbagc_active_gain_db: [f32; 4],
    pub limiter_reduction_db: f32,
    /// Linear mic gain currently applied post-DSP. 1.0 = unity, 2.0 = +6 dB,
    /// 8.0 = +18 dB (maximum). Tracks `AudioState.mic_volume / 100`, capped at 8.0.
    pub mic_gain_linear: f32,
    /// Same as `mic_gain_linear` but expressed in dB.
    /// -120.0 when `mic_volume` is 0 (sentinel for -inf; kept finite for JSON).
    pub mic_gain_db: f32,
    /// Peak attenuation (dB, negative) the recording limiter applied during
    /// the last collection block. 0.0 when no limiting was needed.
    pub recording_limiter_reduction_db: f32,
    /// Start-up microphone check: unknown / ok / degraded / not_responding.
    pub mic_health: String,
    /// ALSA channels that are silent (bit n = ALSA channel n).
    pub mic_dead_mask: u8,
    /// ALSA channels that are much quieter than the rest.
    pub mic_low_mask: u8,
    #[cfg(feature = "capture-alsa")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uac: Option<crate::uac::UacStatsSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hid: Option<waved_hid::stats::HidStatsSnapshot>,
    /// Bluetooth phone audio on TDM-A (`crate::bt_audio`); absent when the
    /// paths are not running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bt_audio: Option<crate::bt_audio::BtAudioStatsSnapshot>,
}

/// Attach the Bluetooth audio counters (`None`: not running).
pub fn with_bt_audio(stats: &mut AudioStats, bt: Option<&crate::bt_audio::BtAudioStats>) {
    stats.bt_audio = bt.map(|b| b.snapshot());
}

#[allow(clippy::too_many_arguments)] // tracking issue: collapse into StatsCollectorConfig once SP#7(d') lands
pub async fn run(
    mut rx: broadcast::Receiver<AudioFrame>,
    mut processed_rx: broadcast::Receiver<ProcessedAudioFrame>,
    audio_state_rx: watch::Receiver<AudioState>,
    stats: Arc<CaptureStats>,
    map_source: ChannelMapSource,
    out_path: PathBuf,
    period: std::time::Duration,
    alignment: String,
    dsp_metrics: Option<Arc<DspMetrics>>,
    #[cfg(feature = "capture-alsa")] uac_stats: Option<Arc<crate::uac::UacStats>>,
    hid_stats: Option<Arc<waved_hid::stats::HidStats>>,
    bt_audio_stats: Option<Arc<crate::bt_audio::BtAudioStats>>,
) {
    info!("stats_collector running, out={}", out_path.display());
    let mut interval = tokio::time::interval(period);
    let mut latest_frame: Option<AudioFrame> = None;
    let mut latest_post_aec_rms: Option<f32> = None;
    let mut latest_aec_metrics: Option<AecFrameMetrics> = None;
    let mut last_write_error: Option<String> = None;
    loop {
        tokio::select! {
            frame = rx.recv() => match frame {
                Ok(f) => { latest_frame = Some(f); }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("stats_collector lagged {n} frames");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    info!("stats_collector: broadcast closed, stopping");
                    return;
                }
            },
            processed = processed_rx.recv() => match processed {
                Ok(p) => {
                    let uplink_rms = processed_mono_rms(&p);
                    let mut metrics = p.aec_metrics.clone();
                    metrics.mic_uplink_rms = uplink_rms;
                    metrics.update_suppression();
                    latest_post_aec_rms = Some(uplink_rms);
                    latest_aec_metrics = Some(metrics);
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("stats_collector lagged {n} processed frames");
                }
                // Closed is not fatal — raw frames can still be collected if
                // the DSP path tears down first.
                Err(broadcast::error::RecvError::Closed) => {}
            },
            _ = interval.tick() => {
                if let Some(frame) = latest_frame.as_ref() {
                    let state = audio_state_rx.borrow().clone();
                    let mut s = compute_stats_with_processed(
                        frame, &stats, map_source, last_write_error.clone(), &alignment,
                        latest_post_aec_rms, latest_aec_metrics.clone(),
                        &state, dsp_metrics.as_deref(),
                        #[cfg(feature = "capture-alsa")]
                        uac_stats.as_deref(),
                        hid_stats.as_deref(),
                    );
                    with_bt_audio(&mut s, bt_audio_stats.as_deref());
                    if let Err(e) = write_atomic(&out_path, &s) {
                        warn!("stats write failed: {e}");
                        last_write_error = Some(format!("{e}"));
                    } else {
                        last_write_error = None;
                    }
                }
            }
        }
    }
}

/// RMS of the final ProcessedAudioFrame uplink signal, after all DSP, user
/// gain, and limiting, scaled to the i16 domain.
pub fn processed_mono_rms(p: &ProcessedAudioFrame) -> f32 {
    if p.mic_mono_f32.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = p
        .mic_mono_f32
        .iter()
        .map(|&v| (v as f64) * (v as f64))
        .sum();
    let rms_f32 = (sum_sq / p.mic_mono_f32.len() as f64).sqrt() as f32;
    rms_f32 * (i16::MAX as f32)
}

pub fn compute_stats(
    frame: &AudioFrame,
    cap_stats: &CaptureStats,
    map_source: ChannelMapSource,
    last_write_error: Option<String>,
    alignment: &str,
) -> AudioStats {
    compute_stats_with_processed(
        frame,
        cap_stats,
        map_source,
        last_write_error,
        alignment,
        None,
        None,
        &AudioState::default(),
        None,
        #[cfg(feature = "capture-alsa")]
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)] // tracking issue: collapse into StatsCollectorConfig once SP#7(d') lands
pub fn compute_stats_with_processed(
    frame: &AudioFrame,
    cap_stats: &CaptureStats,
    map_source: ChannelMapSource,
    last_write_error: Option<String>,
    alignment: &str,
    mic_post_aec_rms: Option<f32>,
    aec_metrics: Option<AecFrameMetrics>,
    audio_state: &AudioState,
    dsp_metrics: Option<&DspMetrics>,
    #[cfg(feature = "capture-alsa")] uac_stats: Option<&crate::uac::UacStats>,
    hid_stats: Option<&waved_hid::stats::HidStats>,
) -> AudioStats {
    let ch = frame.channels as usize;
    let per_ch_rms = rms_per_channel(&frame.data, ch);
    let per_ch_clip = (0..ch)
        .map(|c| {
            let chan: Vec<i16> = frame.data.iter().skip(c).step_by(ch).copied().collect();
            clipping_rate(&chan, i16::MAX)
        })
        .collect();
    let per_ch_dc = (0..ch)
        .map(|c| {
            let chan: Vec<i16> = frame.data.iter().skip(c).step_by(ch).copied().collect();
            dc_offset(&chan)
        })
        .collect();
    let corr = correlation_matrix(&frame.data, ch);
    let ref_rms = if let Some(ref_data) = &frame.reference {
        let ref_ch = frame.reference_channels as usize;
        if ref_ch > 0 {
            rms_per_channel(ref_data, ref_ch)
        } else {
            vec![]
        }
    } else {
        vec![]
    };
    let running = dsp_metrics.and_then(|m| m.active_preset());
    let playback_preset = match running.unwrap_or(audio_state.playback_preset) {
        waved_state::PlaybackPreset::Voice => "voice".to_string(),
        waved_state::PlaybackPreset::Media => "media".to_string(),
    };
    let speaker_scene = dsp_metrics.and_then(|m| m.scene()).map(|s| s.as_str().to_string());
    let speaker_scene_auto = dsp_metrics.is_some_and(|m| m.scene_auto());
    let amp_eq_mode = dsp_metrics.and_then(|m| m.amp_eq_mode()).map(|m| m.value() as u8);
    let amp_eq_mode_auto = dsp_metrics.is_some_and(|m| m.amp_eq_mode_auto());
    let (mbagc_active_gain_db, limiter_reduction_db, recording_limiter_reduction_db) =
        if let Some(m) = dsp_metrics {
            let gains = [
                m.mbagc_gain_db_x100[0].load(Ordering::Relaxed) as f32 / 100.0,
                m.mbagc_gain_db_x100[1].load(Ordering::Relaxed) as f32 / 100.0,
                m.mbagc_gain_db_x100[2].load(Ordering::Relaxed) as f32 / 100.0,
                m.mbagc_gain_db_x100[3].load(Ordering::Relaxed) as f32 / 100.0,
            ];
            let limiter = m.limiter_reduction_db_x100.load(Ordering::Relaxed) as f32 / 100.0;
            let rec_limiter = m
                .recording_limiter_reduction_db_x100
                .load(Ordering::Relaxed) as f32
                / 100.0;
            (gains, limiter, rec_limiter)
        } else {
            ([0.0_f32; 4], 0.0_f32, 0.0_f32)
        };

    #[cfg(feature = "capture-alsa")]
    let uac = uac_stats.map(|s| s.snapshot());

    let hid = hid_stats.map(|s| s.snapshot());

    let mic_gain_linear = (audio_state.mic_volume.min(800) as f32) / 100.0;
    let mic_gain_db = if mic_gain_linear > 0.0 {
        20.0 * mic_gain_linear.log10()
    } else {
        -120.0
    };
    let (mic_health, mic_dead_mask, mic_low_mask) = mic_health_fields(audio_state.mic_health);

    AudioStats {
        last_updated_ms: frame.timestamp_ms,
        channel_map_source: match map_source {
            ChannelMapSource::File => "file".into(),
            ChannelMapSource::Fallback => "fallback".into(),
        },
        sample_rate: frame.sample_rate,
        channels: frame.channels,
        frames_emitted: cap_stats.frames_emitted.load(Ordering::Relaxed),
        frames_dropped: cap_stats.frames_dropped.load(Ordering::Relaxed),
        xrun_count: cap_stats.xrun_count.load(Ordering::Relaxed),
        raw_frames_dropped: audio_state.raw_frames_dropped,
        processed_frames_dropped: audio_state.processed_frames_dropped,
        rms_per_channel: per_ch_rms,
        clipping_rate_per_channel: per_ch_clip,
        dc_offset_per_channel: per_ch_dc,
        correlation_matrix: corr,
        last_write_error,
        reference_rms: ref_rms,
        alignment: alignment.to_string(),
        mic_post_aec_rms,
        mic_uplink_rms: aec_metrics
            .as_ref()
            .map(|m| m.mic_uplink_rms)
            .or(mic_post_aec_rms),
        beam: aec_metrics.as_ref().and_then(|m| m.beam),
        aec: aec_metrics,
        doa_horizontal: audio_state.doa_horizontal,
        doa_pitch: audio_state.doa_pitch,
        doa_active: audio_state.doa_active,
        doa_backend_fallback: audio_state.doa_backend_fallback,
        mode: audio_state.mode.clone(),
        playback_preset,
        speaker_scene,
        speaker_scene_auto,
        amp_eq_mode,
        amp_eq_mode_auto,
        mbagc_active_gain_db,
        limiter_reduction_db,
        mic_gain_linear,
        mic_gain_db,
        recording_limiter_reduction_db,
        mic_health,
        mic_dead_mask,
        mic_low_mask,
        #[cfg(feature = "capture-alsa")]
        uac,
        hid,
        bt_audio: None,
    }
}

fn mic_health_fields(health: waved_state::MicHealth) -> (String, u8, u8) {
    use waved_state::MicHealth;
    match health {
        MicHealth::Unknown => ("unknown".into(), 0, 0),
        MicHealth::Ok => ("ok".into(), 0, 0),
        MicHealth::Degraded { low_mask } => ("degraded".into(), 0, low_mask),
        MicHealth::NotResponding { dead_mask } => ("not_responding".into(), dead_mask, 0),
    }
}

fn write_atomic(path: &Path, stats: &AudioStats) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(stats)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::CaptureStats;
    use crate::fixtures::{correlation_pair_8ch, silence_8ch};

    fn frame_from(data: Vec<i16>) -> AudioFrame {
        AudioFrame::mic_only(data, 8, 48000, 42)
    }

    #[test]
    fn stats_on_silence_frame() {
        let frame = frame_from(silence_8ch(1)[..8 * 256].to_vec());
        let cs = CaptureStats::new();
        cs.frames_emitted.store(7, Ordering::Relaxed);
        let s = compute_stats(&frame, &cs, ChannelMapSource::File, None, "none");
        assert_eq!(s.channels, 8);
        assert_eq!(s.frames_emitted, 7);
        assert!(s.rms_per_channel.iter().all(|&r| r == 0.0));
        assert!(s.clipping_rate_per_channel.iter().all(|&c| c == 0.0));
        assert_eq!(s.channel_map_source, "file");
    }

    #[test]
    fn stats_flag_fallback_channel_map() {
        let frame = frame_from(silence_8ch(1)[..8 * 256].to_vec());
        let cs = CaptureStats::new();
        let s = compute_stats(&frame, &cs, ChannelMapSource::Fallback, None, "none");
        assert_eq!(s.channel_map_source, "fallback");
    }

    #[test]
    fn stats_correlation_on_paired_tone() {
        let frame = frame_from(correlation_pair_8ch(1, 1000.0, 10_000)[..8 * 1024].to_vec());
        let cs = CaptureStats::new();
        let s = compute_stats(&frame, &cs, ChannelMapSource::File, None, "none");
        assert!((s.correlation_matrix[0][1] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn write_atomic_creates_json() {
        let frame = frame_from(silence_8ch(1)[..8 * 256].to_vec());
        let cs = CaptureStats::new();
        let s = compute_stats(&frame, &cs, ChannelMapSource::File, None, "none");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.json");
        write_atomic(&path, &s).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"channels\": 8"));
        assert!(raw.contains("\"channel_map_source\": \"file\""));
    }

    #[test]
    fn write_atomic_cleans_up_tmp_on_success() {
        let frame = frame_from(silence_8ch(1)[..8 * 256].to_vec());
        let cs = CaptureStats::new();
        let s = compute_stats(&frame, &cs, ChannelMapSource::File, None, "none");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.json");
        write_atomic(&path, &s).unwrap();
        let tmp_path = path.with_extension("json.tmp");
        assert!(!tmp_path.exists(), "tmp should be renamed away");
        assert!(path.exists());
    }

    #[test]
    fn reference_rms_populated_when_reference_present() {
        let mic = vec![0i16; 8 * 256];
        let ref_data = vec![16384i16; 2 * 256];
        let frame = AudioFrame::paired(mic, 8, ref_data, 2, 48000, 0);
        let cs = CaptureStats::new();
        let s = compute_stats(&frame, &cs, ChannelMapSource::File, None, "kernel-linked");
        assert_eq!(s.reference_rms.len(), 2);
        assert!(
            (s.reference_rms[0] - 16384.0).abs() < 1.0,
            "expected ~16384, got {}",
            s.reference_rms[0]
        );
        assert_eq!(s.alignment, "kernel-linked");
    }

    #[test]
    fn processed_mono_rms_matches_expected_i16_scale() {
        // 0.5 amplitude sine → RMS = 0.5/sqrt(2) * i16::MAX = ~11585.
        let n = 48000;
        let step = 2.0 * std::f32::consts::PI * 1000.0 / 48000.0;
        let mic_mono_f32: Vec<f32> = (0..n).map(|i| (i as f32 * step).sin() * 0.5).collect();
        let p = ProcessedAudioFrame {
            recording_f32: mic_mono_f32.clone(),
            mic_mono_f32,
            mic_stereo_f32: None,
            reference_f32: vec![],
            sample_rate: 48000,
            timestamp_ms: 0,
            raw_mic_i16: vec![],
            raw_mic_channels: 0,
            aec_metrics: Default::default(),
        };
        let rms = processed_mono_rms(&p);
        let expected = 0.5 / 2f32.sqrt() * i16::MAX as f32;
        assert!(
            (rms - expected).abs() < expected * 0.01,
            "expected ~{expected:.1}, got {rms:.1}"
        );
    }

    #[test]
    fn mic_post_aec_rms_threads_through_compute_stats_with_processed() {
        let frame = frame_from(silence_8ch(1)[..8 * 256].to_vec());
        let cs = CaptureStats::new();
        let s = compute_stats_with_processed(
            &frame,
            &cs,
            ChannelMapSource::File,
            None,
            "none",
            Some(123.4),
            None,
            &AudioState::default(),
            None,
            #[cfg(feature = "capture-alsa")]
            None,
            None,
        );
        assert_eq!(s.mic_post_aec_rms, Some(123.4));
        assert_eq!(s.mic_uplink_rms, Some(123.4));
    }

    #[test]
    fn same_frame_aec_metrics_override_legacy_uplink_name() {
        let frame = frame_from(silence_8ch(1)[..8 * 256].to_vec());
        let cs = CaptureStats::new();
        let mut metrics = AecFrameMetrics {
            reference_rms: 100.0,
            mic_pre_aec_rms: 1000.0,
            mic_post_linear_aec_rms: 100.0,
            mic_post_residual_echo_rms: 20.0,
            mic_post_noise_suppression_rms: 20.0,
            mic_uplink_rms: 40.0,
            ..Default::default()
        };
        metrics.update_suppression();
        let s = compute_stats_with_processed(
            &frame,
            &cs,
            ChannelMapSource::File,
            None,
            "software-reference",
            Some(39.0),
            Some(metrics),
            &AudioState::default(),
            None,
            #[cfg(feature = "capture-alsa")]
            None,
            None,
        );
        assert_eq!(s.mic_post_aec_rms, Some(39.0));
        assert_eq!(s.mic_uplink_rms, Some(40.0));
        let aec = s.aec.expect("same-frame AEC metrics");
        assert!((aec.linear_erle_db.unwrap() - 20.0).abs() < 0.01);
        assert!((aec.effective_echo_suppression_db.unwrap() - 27.9588).abs() < 0.01);
    }

    #[test]
    fn audio_state_doa_and_mode_appear_in_stats() {
        use waved_state::AudioMode;
        let frame = frame_from(silence_8ch(1)[..8 * 256].to_vec());
        let cs = CaptureStats::new();
        let state = AudioState {
            mode: AudioMode::Cardioid,
            doa_horizontal: 47.5,
            doa_pitch: -12.0,
            doa_active: true,
            raw_frames_dropped: 3,
            processed_frames_dropped: 4,
            ..Default::default()
        };
        let s = compute_stats_with_processed(
            &frame,
            &cs,
            ChannelMapSource::File,
            None,
            "none",
            None,
            None,
            &state,
            None,
            #[cfg(feature = "capture-alsa")]
            None,
            None,
        );
        assert_eq!(s.mode, AudioMode::Cardioid);
        assert!((s.doa_horizontal - 47.5).abs() < 1e-6);
        assert!((s.doa_pitch + 12.0).abs() < 1e-6);
        assert!(s.doa_active);
        assert_eq!(s.raw_frames_dropped, 3);
        assert_eq!(s.processed_frames_dropped, 4);
    }

    #[test]
    fn reference_rms_empty_when_no_reference() {
        let frame = AudioFrame::mic_only(vec![0i16; 8 * 256], 8, 48000, 0);
        let cs = CaptureStats::new();
        let s = compute_stats(&frame, &cs, ChannelMapSource::File, None, "none");
        assert!(s.reference_rms.is_empty());
        assert_eq!(s.alignment, "none");
    }

    #[test]
    fn audio_stats_includes_mic_gain_when_volume_set() {
        use waved_state::AudioState;
        let frame = AudioFrame::mic_only(vec![0i16; 8 * 256], 8, 48_000, 0);
        let cs = CaptureStats::new();
        let mut state = AudioState::default();
        state.mic_volume = 200; // 2× linear, +6 dB
        let stats = compute_stats_with_processed(
            &frame,
            &cs,
            ChannelMapSource::File,
            None,
            "none",
            Some(123.4),
            None,
            &state,
            None,
            #[cfg(feature = "capture-alsa")]
            None,
            None,
        );
        assert!(
            (stats.mic_gain_linear - 2.0).abs() < 1e-6,
            "expected 2.0, got {}",
            stats.mic_gain_linear
        );
        assert!(
            (stats.mic_gain_db - 6.0205994).abs() < 0.01,
            "expected ~6.02 dB, got {}",
            stats.mic_gain_db
        );
    }

    #[test]
    #[allow(non_snake_case)]
    fn audio_stats_mic_gain_zero_volume_emits_minus_inf_dB_as_neg_120() {
        use waved_state::AudioState;
        let frame = AudioFrame::mic_only(vec![0i16; 8 * 256], 8, 48_000, 0);
        let cs = CaptureStats::new();
        let mut state = AudioState::default();
        state.mic_volume = 0;
        let stats = compute_stats_with_processed(
            &frame,
            &cs,
            ChannelMapSource::File,
            None,
            "none",
            None,
            None,
            &state,
            None,
            #[cfg(feature = "capture-alsa")]
            None,
            None,
        );
        assert_eq!(stats.mic_gain_linear, 0.0);
        // Convention: clamp -inf to -120 dB so JSON stays finite.
        assert_eq!(stats.mic_gain_db, -120.0);
    }

    #[test]
    fn audio_stats_recording_limiter_reduction_db_zero_when_metrics_absent() {
        use waved_state::AudioState;
        let frame = AudioFrame::mic_only(vec![0i16; 8 * 256], 8, 48_000, 0);
        let cs = CaptureStats::new();
        let state = AudioState::default();
        let stats = compute_stats_with_processed(
            &frame,
            &cs,
            ChannelMapSource::File,
            None,
            "none",
            Some(123.4),
            None,
            &state,
            None,
            #[cfg(feature = "capture-alsa")]
            None,
            None,
        );
        assert_eq!(stats.recording_limiter_reduction_db, 0.0);
    }

    #[test]
    fn audio_stats_recording_limiter_reduction_db_reads_from_dsp_metrics() {
        use std::sync::atomic::Ordering;
        use waved_state::AudioState;
        let frame = AudioFrame::mic_only(vec![0i16; 8 * 256], 8, 48_000, 0);
        let cs = CaptureStats::new();
        let state = AudioState::default();
        let m = crate::playback::DspMetrics::default();
        // Limiter pulled signal down by 3.5 dB → store as -350 (×100).
        m.recording_limiter_reduction_db_x100
            .store(-350, Ordering::Relaxed);
        let stats = compute_stats_with_processed(
            &frame,
            &cs,
            ChannelMapSource::File,
            None,
            "none",
            None,
            None,
            &state,
            Some(&m),
            #[cfg(feature = "capture-alsa")]
            None,
            None,
        );
        assert!(
            (stats.recording_limiter_reduction_db - (-3.5)).abs() < 1e-6,
            "got {}",
            stats.recording_limiter_reduction_db
        );
    }

    fn stats_with_metrics(state: &waved_state::AudioState, m: Option<&crate::playback::DspMetrics>) -> AudioStats {
        let frame = AudioFrame::mic_only(vec![0i16; 8 * 256], 8, 48_000, 0);
        let cs = CaptureStats::new();
        compute_stats_with_processed(
            &frame,
            &cs,
            ChannelMapSource::File,
            None,
            "none",
            None,
            None,
            state,
            m,
            #[cfg(feature = "capture-alsa")]
            None,
            None,
        )
    }

    #[test]
    fn speaker_scene_and_amp_mode_appear_in_stats() {
        use crate::playback::scene::SpeakerScene;
        use waved_state::{AmpEqMode, AudioState, PlaybackPreset};
        let state = AudioState::default(); // playback_preset: voice (the last command)
        let m = crate::playback::DspMetrics::default();
        m.set_active_preset(PlaybackPreset::Media);
        m.set_scene(SpeakerScene::Music, true);
        m.set_amp_eq_mode(Some(AmpEqMode::Music), false);
        let stats = stats_with_metrics(&state, Some(&m));
        assert_eq!(stats.playback_preset, "media", "the preset the playback thread runs");
        assert_eq!(stats.speaker_scene.as_deref(), Some("music"));
        assert!(stats.speaker_scene_auto);
        assert_eq!(stats.amp_eq_mode, Some(2));
        assert!(!stats.amp_eq_mode_auto);
        let json = serde_json::to_value(&stats).unwrap();
        assert_eq!(json["speaker_scene"], "music");
        assert_eq!(json["amp_eq_mode"], 2);

        m.set_scene(SpeakerScene::Meeting, false);
        m.set_amp_eq_mode(None, true);
        let stats = stats_with_metrics(&state, Some(&m));
        assert_eq!(stats.speaker_scene.as_deref(), Some("meeting"));
        assert!(!stats.speaker_scene_auto);
        let json = serde_json::to_value(&stats).unwrap();
        assert!(json["amp_eq_mode"].is_null(), "not written or failed → null");
    }

    #[test]
    fn stats_without_a_scene_follower_fall_back() {
        let mut state = waved_state::AudioState::default();
        state.playback_preset = waved_state::PlaybackPreset::Media;
        let stats = stats_with_metrics(&state, None);
        assert_eq!(stats.playback_preset, "media");
        assert_eq!(stats.speaker_scene, None);
        assert_eq!(stats.amp_eq_mode, None);
        let m = crate::playback::DspMetrics::default();
        let stats = stats_with_metrics(&state, Some(&m));
        assert_eq!(stats.playback_preset, "media", "playback not started → last command");
        assert_eq!(stats.speaker_scene, None);
    }

    #[test]
    fn beam_tracker_appears_in_stats() {
        let frame = frame_from(silence_8ch(1)[..8 * 256].to_vec());
        let cs = CaptureStats::new();
        let beam = crate::processed_frame::BeamMetrics {
            az_deg: 92.5,
            el_deg: 12.0,
            omni_mix: 0.25,
        };
        let metrics = AecFrameMetrics {
            beam: Some(beam),
            ..Default::default()
        };
        let s = compute_stats_with_processed(
            &frame,
            &cs,
            ChannelMapSource::File,
            None,
            "none",
            None,
            Some(metrics),
            &AudioState::default(),
            None,
            #[cfg(feature = "capture-alsa")]
            None,
            None,
        );
        assert_eq!(s.beam, Some(beam));
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["beam"]["az_deg"], 92.5);
        assert_eq!(json["beam"]["el_deg"], 12.0);
        assert_eq!(json["beam"]["omni_mix"], 0.25);
        assert!(json["aec"].get("beam").is_none(), "published once, at the top");
    }

    #[test]
    fn beam_is_omitted_without_a_beamformed_frame() {
        let frame = frame_from(silence_8ch(1)[..8 * 256].to_vec());
        let s = compute_stats(&frame, &CaptureStats::new(), ChannelMapSource::File, None, "none");
        assert_eq!(s.beam, None);
        assert!(serde_json::to_value(&s).unwrap().get("beam").is_none());
    }

    #[test]
    fn mic_health_appears_in_stats() {
        let frame = frame_from(silence_8ch(1)[..8 * 256].to_vec());
        let cs = CaptureStats::new();
        let state = AudioState {
            mic_health: waved_state::MicHealth::NotResponding {
                dead_mask: 0b1000_0001,
            },
            ..Default::default()
        };
        let s = compute_stats_with_processed(
            &frame,
            &cs,
            ChannelMapSource::File,
            None,
            "none",
            None,
            None,
            &state,
            None,
            #[cfg(feature = "capture-alsa")]
            None,
            None,
        );
        assert_eq!(s.mic_health, "not_responding");
        assert_eq!(s.mic_dead_mask, 0b1000_0001);
        assert_eq!(s.mic_low_mask, 0);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"mic_health\":\"not_responding\""));
    }
}

#[cfg(test)]
mod tests_hid_wiring {
    use super::*;
    use waved_hid::stats::HidStats;

    #[test]
    fn audio_stats_serializes_hid_when_present() {
        let stats = HidStats::default();
        stats
            .events_volume_step
            .fetch_add(5, std::sync::atomic::Ordering::Relaxed);
        let snap = stats.snapshot();

        let mut audio = sample_audio_stats();
        audio.hid = Some(snap);

        let json = serde_json::to_value(&audio).unwrap();
        assert_eq!(json["hid"]["events_emitted"]["volume_step"], 5);
    }

    #[test]
    fn audio_stats_carry_bt_audio_only_when_wired() {
        let mut audio = sample_audio_stats();
        with_bt_audio(&mut audio, None);
        let json = serde_json::to_value(&audio).unwrap();
        assert!(json.get("bt_audio").is_none());

        let bt = crate::bt_audio::BtAudioStats::default();
        bt.speaker_active.store(true, std::sync::atomic::Ordering::Relaxed);
        bt.capture_frames.fetch_add(48_000, std::sync::atomic::Ordering::Relaxed);
        bt.capture_xruns.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        with_bt_audio(&mut audio, Some(&bt));
        let json = serde_json::to_value(&audio).unwrap();
        assert_eq!(json["bt_audio"]["speaker_active"], true);
        assert_eq!(json["bt_audio"]["mic_active"], false);
        assert_eq!(json["bt_audio"]["capture_frames"], 48_000);
        assert_eq!(json["bt_audio"]["capture_xruns"], 1);
    }

    #[test]
    fn audio_stats_omits_hid_when_none() {
        let audio = sample_audio_stats();
        let json = serde_json::to_value(&audio).unwrap();
        assert!(
            json.get("hid").is_none(),
            "expected hid field absent when None"
        );
    }

    fn sample_audio_stats() -> AudioStats {
        AudioStats {
            last_updated_ms: 0,
            channel_map_source: "fallback".into(),
            sample_rate: 48_000,
            channels: 1,
            frames_emitted: 0,
            frames_dropped: 0,
            xrun_count: 0,
            raw_frames_dropped: 0,
            processed_frames_dropped: 0,
            rms_per_channel: vec![],
            clipping_rate_per_channel: vec![],
            dc_offset_per_channel: vec![],
            correlation_matrix: vec![],
            last_write_error: None,
            reference_rms: vec![],
            alignment: String::new(),
            mic_post_aec_rms: None,
            mic_uplink_rms: None,
            aec: None,
            beam: None,
            doa_horizontal: 0.0,
            doa_pitch: 0.0,
            doa_active: false,
            doa_backend_fallback: false,
            mode: waved_state::AudioMode::default(),
            playback_preset: "voice".into(),
            speaker_scene: None,
            speaker_scene_auto: false,
            amp_eq_mode: None,
            amp_eq_mode_auto: false,
            mbagc_active_gain_db: [0.0; 4],
            limiter_reduction_db: 0.0,
            mic_gain_linear: 1.0,
            mic_gain_db: 0.0,
            recording_limiter_reduction_db: 0.0,
            mic_health: "unknown".into(),
            mic_dead_mask: 0,
            mic_low_mask: 0,
            #[cfg(feature = "capture-alsa")]
            uac: None,
            hid: None,
            bt_audio: None,
        }
    }
}
