//! waved-aec-probe: play a steady sine tone through PlaybackService for
//! on-device ERLE measurement.
//!
//! Pairs with `scripts/test-aec-on-device.sh`: while this binary is playing,
//! the script samples `/tmp/audio_stats.json` to compare mic_rms (should
//! elevate from room noise to tone-driven echo) against mic_post_aec_rms
//! (should stay near baseline thanks to AEC cancelling the echo).
//!
//! Defaults match the waved playback path: `hw:0,1`, 2-ch, 48 kHz, S16_LE,
//! 1024-frame periods, 4-period buffer.

use clap::Parser;
use std::time::{Duration, Instant};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use waved_audio::audio_config::SampleFormat;
use waved_audio::playback::{spawn_playback, PlaybackConfig};

#[derive(Parser, Debug)]
#[command(about = "Play a sine tone through PlaybackService (SP#2 AEC probe)")]
struct Args {
    /// ALSA playback device. Default matches embedded audio config.
    #[arg(long, default_value = "hw:0,1")]
    device: String,

    /// Tone frequency in Hz.
    #[arg(long, default_value_t = 1000.0)]
    freq: f32,

    /// Duration in seconds.
    #[arg(long, default_value_t = 3.0)]
    duration: f32,

    /// Amplitude in [0.0, 1.0]. Scaled against i16::MAX internally.
    #[arg(long, default_value_t = 0.3)]
    amplitude: f32,

    /// Channel count (2 = stereo, matches default playback config).
    #[arg(long, default_value_t = 2)]
    channels: u32,

    /// Sample rate.
    #[arg(long, default_value_t = 48_000)]
    sample_rate: u32,

    /// Period size for PlaybackService (must match the playing waved instance
    /// to keep timing predictable — defaults to 1024 per embedded config).
    #[arg(long, default_value_t = 1024)]
    period_frames: usize,

    /// Buffer size (in periods).
    #[arg(long, default_value_t = 4)]
    buffer_periods: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("waved_aec_probe=info".parse().unwrap())
                .add_directive("waved_audio=warn".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    if !(args.amplitude >= 0.0 && args.amplitude <= 1.0) {
        return Err(format!("amplitude {} outside [0.0, 1.0]", args.amplitude).into());
    }
    if args.duration <= 0.0 {
        return Err("duration must be > 0".into());
    }

    let cfg = PlaybackConfig {
        device: args.device.clone(),
        channels: args.channels,
        sample_rate: args.sample_rate,
        format: SampleFormat::S16_LE,
        period_frames: args.period_frames,
        buffer_periods: args.buffer_periods,
    };

    info!(
        "playback: device={} {}Hz {}ch period={} buffer={}",
        args.device, args.sample_rate, args.channels, args.period_frames, args.buffer_periods
    );
    info!(
        "tone: {:.1} Hz, {:.2} amplitude, {:.2} s",
        args.freq, args.amplitude, args.duration
    );

    let playback = spawn_playback(cfg)?;
    let period_samples = args.period_frames * args.channels as usize;
    let peak = (args.amplitude * i16::MAX as f32) as i32;
    let step = 2.0 * std::f32::consts::PI * args.freq / args.sample_rate as f32;

    let start = Instant::now();
    let target = Duration::from_secs_f32(args.duration);
    let mut frame_idx: usize = 0;

    // Submit batches steadily; play_batch is non-blocking (try_send).
    // Pace ourselves so we stay ahead of the playback thread without flooding
    // the 64-slot mpsc.
    let batch_period = Duration::from_micros(
        (args.period_frames as u64 * 1_000_000) / args.sample_rate as u64,
    );

    while start.elapsed() < target {
        let mut batch = vec![0i16; period_samples];
        for i in 0..args.period_frames {
            let sample = ((frame_idx + i) as f32 * step).sin();
            let s = (sample * peak as f32) as i16;
            for ch in 0..args.channels as usize {
                batch[i * args.channels as usize + ch] = s;
            }
        }
        frame_idx += args.period_frames;
        if let Err(e) = playback.play_batch(batch) {
            warn!("play_batch failed: {e}; retrying after a period");
        }
        std::thread::sleep(batch_period);
    }

    info!("tone duration reached; flushing with silence for 1 period");
    // One trailing silence batch so the speaker tail decays cleanly before
    // the probe exits (and the PlaybackService is dropped).
    let _ = playback.play_batch(vec![0i16; period_samples]);
    std::thread::sleep(batch_period);

    info!("done: {:.2} s played", start.elapsed().as_secs_f32());
    Ok(())
}
