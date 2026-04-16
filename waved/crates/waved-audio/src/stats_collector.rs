//! Stats collector: taps the frame stream via a broadcast channel, computes
//! per-channel metrics every N frames, writes /tmp/audio_stats.json with
//! atomic rename. Never in the capture hot path.

use crate::callbacks::AudioFrame;
use crate::capture::{ChannelMapSource, CaptureStats};
use crate::metrics::{clipping_rate, correlation_matrix, dc_offset, rms_per_channel};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::broadcast;
use tracing::{info, warn};

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
}

pub async fn run(
    mut rx: broadcast::Receiver<AudioFrame>,
    stats: Arc<CaptureStats>,
    map_source: ChannelMapSource,
    out_path: PathBuf,
    period: std::time::Duration,
) {
    info!("stats_collector running, out={}", out_path.display());
    let mut interval = tokio::time::interval(period);
    let mut latest_frame: Option<AudioFrame> = None;
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
            _ = interval.tick() => {
                if let Some(frame) = latest_frame.as_ref() {
                    let s = compute_stats(frame, &stats, map_source, last_write_error.clone());
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

pub fn compute_stats(
    frame: &AudioFrame,
    cap_stats: &CaptureStats,
    map_source: ChannelMapSource,
    last_write_error: Option<String>,
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
        let s = compute_stats(&frame, &cs, ChannelMapSource::File, None);
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
        let s = compute_stats(&frame, &cs, ChannelMapSource::Fallback, None);
        assert_eq!(s.channel_map_source, "fallback");
    }

    #[test]
    fn stats_correlation_on_paired_tone() {
        let frame = frame_from(correlation_pair_8ch(1, 1000.0, 10_000)[..8 * 1024].to_vec());
        let cs = CaptureStats::new();
        let s = compute_stats(&frame, &cs, ChannelMapSource::File, None);
        assert!((s.correlation_matrix[0][1] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn write_atomic_creates_json() {
        let frame = frame_from(silence_8ch(1)[..8 * 256].to_vec());
        let cs = CaptureStats::new();
        let s = compute_stats(&frame, &cs, ChannelMapSource::File, None);
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
        let s = compute_stats(&frame, &cs, ChannelMapSource::File, None);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.json");
        write_atomic(&path, &s).unwrap();
        let tmp_path = path.with_extension("json.tmp");
        assert!(!tmp_path.exists(), "tmp should be renamed away");
        assert!(path.exists());
    }
}
