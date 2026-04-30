//! Stats collector: taps the frame stream via a broadcast channel, computes
//! per-channel metrics every N frames, writes /tmp/audio_stats.json with
//! atomic rename. Never in the capture hot path.

use crate::callbacks::AudioFrame;
use crate::capture::{ChannelMapSource, CaptureStats};
use crate::metrics::{clipping_rate, correlation_matrix, dc_offset, rms_per_channel};
use crate::playback::DspMetrics;
use crate::processed_frame::ProcessedAudioFrame;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
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
    pub rms_per_channel: Vec<f32>,
    pub clipping_rate_per_channel: Vec<f32>,
    pub dc_offset_per_channel: Vec<f32>,
    pub correlation_matrix: Vec<Vec<f32>>,
    pub last_write_error: Option<String>,
    pub reference_rms: Vec<f32>,
    pub alignment: String,
    /// RMS of the post-AEC mono signal, scaled to i16 units so it's directly
    /// comparable to `rms_per_channel`. `None` when no processed frame has
    /// been seen yet (AEC disabled or collector just started).
    pub mic_post_aec_rms: Option<f32>,
    pub doa_horizontal: f32,
    pub doa_pitch: f32,
    pub mode: AudioMode,
    pub playback_preset: String,
    pub mbagc_active_gain_db: [f32; 4],
    pub limiter_reduction_db: f32,
}

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
) {
    info!("stats_collector running, out={}", out_path.display());
    let mut interval = tokio::time::interval(period);
    let mut latest_frame: Option<AudioFrame> = None;
    let mut latest_post_aec_rms: Option<f32> = None;
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
                Ok(p) => { latest_post_aec_rms = Some(processed_mono_rms(&p)); }
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
                    let s = compute_stats_with_processed(
                        frame, &stats, map_source, last_write_error.clone(), &alignment,
                        latest_post_aec_rms, &state, dsp_metrics.as_deref(),
                    );
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

/// RMS of a ProcessedAudioFrame's mono post-AEC signal, scaled to the same
/// i16 unit basis as `rms_per_channel` so the ERLE calculation is a single
/// `20 * log10(raw / post)` with both operands in the same domain.
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
        frame, cap_stats, map_source, last_write_error, alignment, None, &AudioState::default(), None,
    )
}

pub fn compute_stats_with_processed(
    frame: &AudioFrame,
    cap_stats: &CaptureStats,
    map_source: ChannelMapSource,
    last_write_error: Option<String>,
    alignment: &str,
    mic_post_aec_rms: Option<f32>,
    audio_state: &AudioState,
    dsp_metrics: Option<&DspMetrics>,
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
        if ref_ch > 0 { rms_per_channel(ref_data, ref_ch) } else { vec![] }
    } else {
        vec![]
    };
    let playback_preset = match audio_state.playback_preset {
        waved_state::PlaybackPreset::Voice => "voice".to_string(),
        waved_state::PlaybackPreset::Media => "media".to_string(),
    };
    let (mbagc_active_gain_db, limiter_reduction_db) = if let Some(m) = dsp_metrics {
        let gains = [
            m.mbagc_gain_db_x100[0].load(Ordering::Relaxed) as f32 / 100.0,
            m.mbagc_gain_db_x100[1].load(Ordering::Relaxed) as f32 / 100.0,
            m.mbagc_gain_db_x100[2].load(Ordering::Relaxed) as f32 / 100.0,
            m.mbagc_gain_db_x100[3].load(Ordering::Relaxed) as f32 / 100.0,
        ];
        let limiter = m.limiter_reduction_db_x100.load(Ordering::Relaxed) as f32 / 100.0;
        (gains, limiter)
    } else {
        ([0.0_f32; 4], 0.0_f32)
    };

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
        rms_per_channel: per_ch_rms,
        clipping_rate_per_channel: per_ch_clip,
        dc_offset_per_channel: per_ch_dc,
        correlation_matrix: corr,
        last_write_error,
        reference_rms: ref_rms,
        alignment: alignment.to_string(),
        mic_post_aec_rms,
        doa_horizontal: audio_state.doa_horizontal,
        doa_pitch: audio_state.doa_pitch,
        mode: audio_state.mode.clone(),
        playback_preset,
        mbagc_active_gain_db,
        limiter_reduction_db,
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
        assert!((s.reference_rms[0] - 16384.0).abs() < 1.0, "expected ~16384, got {}", s.reference_rms[0]);
        assert_eq!(s.alignment, "kernel-linked");
    }

    #[test]
    fn processed_mono_rms_matches_expected_i16_scale() {
        // 0.5 amplitude sine → RMS = 0.5/sqrt(2) * i16::MAX = ~11585.
        let n = 48000;
        let step = 2.0 * std::f32::consts::PI * 1000.0 / 48000.0;
        let mic_mono_f32: Vec<f32> = (0..n).map(|i| (i as f32 * step).sin() * 0.5).collect();
        let p = ProcessedAudioFrame {
            mic_mono_f32,
            mic_stereo_f32: None,
            reference_f32: vec![],
            sample_rate: 48000,
            timestamp_ms: 0,
            raw_mic_i16: vec![],
            raw_mic_channels: 0,
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
            &frame, &cs, ChannelMapSource::File, None, "none", Some(123.4), &AudioState::default(), None,
        );
        assert_eq!(s.mic_post_aec_rms, Some(123.4));
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
            ..Default::default()
        };
        let s = compute_stats_with_processed(
            &frame, &cs, ChannelMapSource::File, None, "none", None, &state, None,
        );
        assert_eq!(s.mode, AudioMode::Cardioid);
        assert!((s.doa_horizontal - 47.5).abs() < 1e-6);
        assert!((s.doa_pitch + 12.0).abs() < 1e-6);
    }

    #[test]
    fn reference_rms_empty_when_no_reference() {
        let frame = AudioFrame::mic_only(vec![0i16; 8 * 256], 8, 48000, 0);
        let cs = CaptureStats::new();
        let s = compute_stats(&frame, &cs, ChannelMapSource::File, None, "none");
        assert!(s.reference_rms.is_empty());
        assert_eq!(s.alignment, "none");
    }
}
