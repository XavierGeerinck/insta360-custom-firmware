//! waved-audio-bench: replay a captured PCM through the real AudioService
//! and emit measurement JSON. Deterministic.
//!
//! Usage: waved-audio-bench <input.pcm> [--channels N] [--sample-rate N]

use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{broadcast, mpsc, watch};
use waved_audio::callbacks::AudioFrame;
use waved_audio::capture::replay::{ReplayFormat, ReplayPcmSource};
use waved_audio::capture::{run_capture_loop, CaptureStats, ChannelMapSource};
use waved_audio::stats_collector::compute_stats;
use waved_audio::AudioService;
use waved_state::{AudioState, Command};

#[derive(Parser, Debug)]
#[command(about = "Replay a captured PCM through AudioService and emit measurement JSON")]
struct Args {
    /// Input PCM file (raw interleaved i16 samples).
    input: PathBuf,

    /// Number of interleaved channels in the input file.
    #[arg(long, default_value_t = 8)]
    channels: usize,

    /// Sample rate for the replay.
    #[arg(long, default_value_t = 48_000)]
    sample_rate: u32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("warn")
        .init();
    let args = Args::parse();

    let (audio_tx, _audio_rx) = watch::channel(AudioState::default());
    let audio_tx = Arc::new(audio_tx);
    let (frame_tx, frame_rx) = mpsc::channel::<AudioFrame>(64);
    let (meeting_tx, mut meeting_rx) = mpsc::channel::<AudioFrame>(256);
    let (_cmd_tx, cmd_rx) = broadcast::channel::<Command>(8);

    let svc = AudioService::new(Arc::clone(&audio_tx), frame_tx.clone(), frame_rx, meeting_tx);
    tokio::spawn(async move { svc.run(cmd_rx).await });

    let stats = CaptureStats::new();
    let stop = Arc::new(AtomicBool::new(false));
    let format = ReplayFormat::I16Interleaved { channels: args.channels };
    let src = ReplayPcmSource::open(&args.input, format, args.sample_rate)?;
    let stats_c = Arc::clone(&stats);
    let stop_c = Arc::clone(&stop);
    let channels = args.channels;
    let capture = std::thread::spawn(move || {
        run_capture_loop(src, 256, channels, frame_tx, stop_c, stats_c);
    });

    // Drain meeting_rx to EOF.
    let mut last_frame: Option<AudioFrame> = None;
    loop {
        // capture finishes at EOF and drops frame_tx, which closes meeting_rx
        if capture.is_finished() {
            // drain remaining
            while let Ok(frame) = meeting_rx.try_recv() {
                last_frame = Some(frame);
            }
            break;
        }
        match tokio::time::timeout(std::time::Duration::from_millis(50), meeting_rx.recv()).await {
            Ok(Some(frame)) => {
                last_frame = Some(frame);
            }
            Ok(None) => break,
            Err(_) => {} // timeout, loop again
        }
    }
    stop.store(true, Ordering::Relaxed);
    let _ = capture.join();

    let last = last_frame.ok_or("no frames produced — empty input?")?;
    let stats_json = compute_stats(&last, &stats, ChannelMapSource::Fallback, None);
    println!("{}", serde_json::to_string_pretty(&stats_json)?);
    Ok(())
}
