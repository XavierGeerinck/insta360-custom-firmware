//! waved-audio-bench: replay a captured PCM through the real AudioService
//! and emit measurement JSON. Deterministic.
//!
//! Usage: waved-audio-bench <input.pcm> [--format i16_8|i32_10] [--channel-map path]

use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{broadcast, mpsc, watch};
use waved_audio::callbacks::AudioFrame;
use waved_audio::capture::replay::{ReplayFormat, ReplayPcmSource};
use waved_audio::capture::{run_capture_loop, CaptureStats, ChannelMap, ChannelMapSource};
use waved_audio::stats_collector::compute_stats;
use waved_audio::AudioService;
use waved_state::{AudioState, Command};

#[derive(Parser, Debug)]
#[command(about = "Replay a captured PCM through AudioService and emit measurement JSON")]
struct Args {
    /// Input PCM file (raw interleaved samples).
    input: PathBuf,

    /// Input format.
    #[arg(long, default_value = "i16_8")]
    format: InputFormat,

    /// Path to channel map JSON. Defaults to embedded fallback.
    #[arg(long)]
    channel_map: Option<PathBuf>,

    /// Sample rate for the replay.
    #[arg(long, default_value_t = 48_000)]
    sample_rate: u32,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum InputFormat {
    #[value(name = "i16_8")]
    I168,
    #[value(name = "i32_10")]
    I3210,
}

impl From<&InputFormat> for ReplayFormat {
    fn from(f: &InputFormat) -> Self {
        match f {
            InputFormat::I168 => ReplayFormat::I16Eight,
            InputFormat::I3210 => ReplayFormat::I32Ten,
        }
    }
}

/// Build a channel map that is consistent with the given replay format when no
/// explicit map file is provided.
///
/// - `I16Eight`: the file already contains 8 post-downmix channels; use an
///   8-channel identity map (all mics, no references) so `run_capture_loop`
///   treats every sample as a physical mic channel.
/// - `I32Ten`: raw 10-channel ALSA capture; use the embedded default (mics
///   0-7, refs 8-9).
fn default_map_for_format(fmt: &InputFormat) -> (ChannelMap, ChannelMapSource) {
    match fmt {
        InputFormat::I168 => {
            let map = ChannelMap::from_json(
                r#"{"version":1,"total_channels":8,"physical_mics":[0,1,2,3,4,5,6,7],"reference_channels":[]}"#,
            )
            .expect("hardcoded 8-ch identity map must parse");
            (map, ChannelMapSource::Fallback)
        }
        InputFormat::I3210 => (ChannelMap::default_embedded(), ChannelMapSource::Fallback),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("warn")
        .init();
    let args = Args::parse();

    let (map, map_src) = match args.channel_map.as_deref() {
        Some(p) => ChannelMap::load_or_default(p),
        None => default_map_for_format(&args.format),
    };

    let (audio_tx, _audio_rx) = watch::channel(AudioState::default());
    let audio_tx = Arc::new(audio_tx);
    let (frame_tx, frame_rx) = mpsc::channel::<AudioFrame>(64);
    let (meeting_tx, mut meeting_rx) = mpsc::channel::<AudioFrame>(256);
    let (_cmd_tx, cmd_rx) = broadcast::channel::<Command>(8);

    let svc = AudioService::new(Arc::clone(&audio_tx), frame_tx.clone(), frame_rx, meeting_tx);
    tokio::spawn(async move { svc.run(cmd_rx).await });

    let stats = CaptureStats::new();
    let stop = Arc::new(AtomicBool::new(false));
    let format: ReplayFormat = (&args.format).into();
    let src = ReplayPcmSource::open(&args.input, format, args.sample_rate)?;
    let stats_c = Arc::clone(&stats);
    let stop_c = Arc::clone(&stop);
    let map_c = map.clone();
    let capture = std::thread::spawn(move || {
        run_capture_loop(src, map_c, 256, frame_tx, stop_c, stats_c);
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
    let stats_json = compute_stats(&last, &stats, map_src, None);
    println!("{}", serde_json::to_string_pretty(&stats_json)?);
    Ok(())
}
