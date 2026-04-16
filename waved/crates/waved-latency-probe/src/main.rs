//! waved-latency-probe: plays a 5 kHz / 20 ms chirp via aplay, captures the
//! mic, cross-correlates to find the arrival peak, prints the delay in ms.
//!
//! Run on device only (needs real ALSA).

use clap::Parser;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::sync::mpsc;
use waved_audio::callbacks::AudioFrame;
use waved_audio::capture::alsa::AlsaPcmSource;
use waved_audio::capture::{run_capture_loop, CaptureStats};
use waved_audio::fixtures::chirp_5khz_burst;
use waved_audio::metrics::cross_correlate_peak;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long, default_value = "/tmp/chirp.wav")]
    chirp_path: String,
    #[arg(long, default_value_t = 2.0)]
    capture_seconds: f32,
    #[arg(long, default_value_t = 0.5)]
    playback_delay_seconds: f32,
    /// Number of mic channels on hw:0,2.
    #[arg(long, default_value_t = 8)]
    mic_channels: u32,
    /// Minimum cross-correlation score to consider the chirp "heard".
    /// cross_correlate_peak returns an unnormalized dot-product; for a 960-sample
    /// reference at amplitude ~20000, a score of 1e9 requires ~50 RMS of mic signal
    /// aligned with the chirp — a reasonable floor for "speaker was audible".
    #[arg(long, default_value_t = 1e9_f32)]
    min_score: f32,
}

fn write_wav(path: &Path, samples: &[i16], sample_rate: u32) -> std::io::Result<()> {
    // Minimal canonical PCM WAV (mono, 16-bit).
    let mut f = std::fs::File::create(path)?;
    let data_bytes = (samples.len() * 2) as u32;
    let chunk_size = 36 + data_bytes;
    f.write_all(b"RIFF")?;
    f.write_all(&chunk_size.to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;    // PCM
    f.write_all(&1u16.to_le_bytes())?;    // mono
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&(sample_rate * 2).to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;    // block align
    f.write_all(&16u16.to_le_bytes())?;   // bits/sample
    f.write_all(b"data")?;
    f.write_all(&data_bytes.to_le_bytes())?;
    for &s in samples { f.write_all(&s.to_le_bytes())?; }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    let args = Args::parse();

    // Extract just channel 0 from the 8-channel fixture as a mono reference.
    let chirp_8ch = chirp_5khz_burst(1);
    let mono_ref: Vec<i16> = chirp_8ch.iter().step_by(8).copied().collect();
    // Keep only the first 20ms (960 samples @ 48kHz) of the reference — after that it's silence.
    let reference: Vec<i16> = mono_ref.into_iter().take(960).collect();
    write_wav(Path::new(&args.chirp_path), &reference, 48000)?;

    let mic_channels = args.mic_channels as usize;
    let stats = CaptureStats::new();
    let stop = Arc::new(AtomicBool::new(false));
    let src = AlsaPcmSource::open("hw:0,2", 48000, args.mic_channels, 256, 4)?;
    let (tx, mut rx) = mpsc::channel::<AudioFrame>(256);
    let stop_c = Arc::clone(&stop);
    let stats_c = Arc::clone(&stats);
    let capture = std::thread::spawn(move || {
        run_capture_loop(src, 256, mic_channels, tx, stop_c, stats_c);
    });

    // Wait, then kick off playback. aplay handle kept to reap the child after capture.
    tokio::time::sleep(Duration::from_secs_f32(args.playback_delay_seconds)).await;
    let mut aplay = Command::new("aplay")
        .arg("-q")
        .arg(&args.chirp_path)
        .spawn()
        .map_err(|e| format!("aplay spawn: {e}"))?;

    // Collect frames for the configured duration, then stop the capture thread.
    tokio::time::sleep(Duration::from_secs_f32(args.capture_seconds)).await;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    capture.join().map_err(|_| "capture join")?;
    let _ = aplay.wait(); // reap child; ignore exit status

    let mut mono: Vec<i16> = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        // Take channel 0 of each frame.
        for s in frame.data.iter().step_by(mic_channels) { mono.push(*s); }
    }
    if mono.len() < reference.len() {
        return Err(format!("not enough samples captured ({})", mono.len()).into());
    }
    let (peak_idx, score) = cross_correlate_peak(&mono, &reference);
    let lead_samples = (args.playback_delay_seconds * 48000.0) as usize;
    if peak_idx < lead_samples {
        eprintln!("warning: peak before expected playback start — false positive?");
    }
    let delay_samples = peak_idx as i64 - lead_samples as i64;
    let delay_ms = delay_samples as f32 / 48.0;
    if score < args.min_score {
        return Err(format!("chirp not heard (peak score {score:.2e} < min {:.2e}) — speaker muted? mic dead?", args.min_score).into());
    }
    println!("{{\"delay_ms\": {:.2}, \"peak_sample\": {}, \"score\": {:.2e}}}", delay_ms, peak_idx, score);
    Ok(())
}
