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
use waved_audio::capture::single_stream::SingleStreamFrameSource;
use waved_audio::capture::{run_capture_loop, CaptureStats, ChannelMapSource};
use waved_audio::processed_frame::ProcessedAudioFrame;
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

    const PERIOD_FRAMES: usize = 256;

    let (audio_tx, _audio_rx) = watch::channel(AudioState::default());
    let audio_tx = Arc::new(audio_tx);
    let (frame_tx, frame_rx) = mpsc::channel::<AudioFrame>(64);
    let (meeting_tx, mut meeting_rx) = mpsc::channel::<ProcessedAudioFrame>(256);
    let (_cmd_tx, cmd_rx) = broadcast::channel::<Command>(8);

    let svc = AudioService::new(
        Arc::clone(&audio_tx),
        frame_tx.clone(),
        frame_rx,
        meeting_tx,
        PERIOD_FRAMES,
        args.sample_rate,
        waved_audio::beam::MicGeometry::default(),
    );
    let stats_tee = svc.subscribe_frames();
    tokio::spawn(async move { svc.run(cmd_rx).await });

    let stats = CaptureStats::new();
    let stop = Arc::new(AtomicBool::new(false));
    let format = ReplayFormat::I16Interleaved { channels: args.channels };
    let src = ReplayPcmSource::open(&args.input, format, args.sample_rate)?;
    let src = SingleStreamFrameSource::new(src, PERIOD_FRAMES, args.channels);
    let stats_c = Arc::clone(&stats);
    let stop_c = Arc::clone(&stop);
    let capture = std::thread::spawn(move || {
        run_capture_loop(src, frame_tx, stop_c, stats_c);
    });

    // Drain meeting_rx to EOF — also keep the latest raw frame for stats.
    let mut last_processed: Option<ProcessedAudioFrame> = None;
    loop {
        if capture.is_finished() {
            while let Ok(frame) = meeting_rx.try_recv() {
                last_processed = Some(frame);
            }
            break;
        }
        match tokio::time::timeout(std::time::Duration::from_millis(50), meeting_rx.recv()).await {
            Ok(Some(frame)) => {
                last_processed = Some(frame);
            }
            Ok(None) => break,
            Err(_) => {}
        }
    }
    stop.store(true, Ordering::Relaxed);
    let _ = capture.join();

    let last = last_processed.ok_or("no frames produced — empty input?")?;
    // Reconstruct an AudioFrame-shaped view for the stats helper from the
    // raw mic that ProcessedAudioFrame carries.
    let raw_view = AudioFrame::mic_only(
        last.raw_mic_i16.clone(),
        last.raw_mic_channels,
        last.sample_rate,
        last.timestamp_ms,
    );
    drop(stats_tee);
    let stats_json = compute_stats(&raw_view, &stats, ChannelMapSource::Fallback, None, "replay");
    println!("{}", serde_json::to_string_pretty(&stats_json)?);
    Ok(())
}
