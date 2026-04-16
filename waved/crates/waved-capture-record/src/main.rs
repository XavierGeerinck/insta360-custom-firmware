//! waved-capture-record: captures N seconds of 8-ch i16 PCM (after downmix)
//! to a file and writes a sidecar .meta.json describing the capture.

use clap::Parser;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use waved_audio::callbacks::AudioFrame;
use waved_audio::capture::alsa::AlsaPcmSource;
use waved_audio::capture::{run_capture_loop, CaptureStats, ChannelMap};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long, default_value_t = 30)]
    duration: u32,
    #[arg(long)]
    out: PathBuf,
    /// Free-text description of the scenario (typed into meta).
    #[arg(long, default_value = "")]
    description: String,
}

fn write_meta(pcm_path: &Path, duration_s: u32, frames: u64, description: &str) -> std::io::Result<()> {
    let meta_path = pcm_path.with_extension("meta.json");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let meta = serde_json::json!({
        "schema": 1,
        "format": "i16_le_interleaved_8ch",
        "sample_rate": 48000,
        "channels": 8,
        "duration_seconds": duration_s,
        "frames_captured": frames,
        "captured_at_unix": now,
        "description": description,
    });
    std::fs::write(meta_path, serde_json::to_string_pretty(&meta)?)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    let args = Args::parse();

    let map = ChannelMap::default_embedded();
    let stats = CaptureStats::new();
    let stop = Arc::new(AtomicBool::new(false));
    let src = AlsaPcmSource::open("hw:0,0", 48000, map.total_channels as u32, 256, 4)?;
    let (tx, mut rx) = mpsc::channel::<AudioFrame>(256);
    let stats_c = Arc::clone(&stats);
    let stop_c = Arc::clone(&stop);
    let map_c = map.clone();
    let capture = std::thread::spawn(move || {
        run_capture_loop(src, map_c, 256, tx, stop_c, stats_c);
    });

    let mut file = std::fs::File::create(&args.out)?;
    let mut frames = 0u64;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(args.duration as u64);
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Some(frame)) => {
                frames += 1;
                for &s in &frame.data {
                    file.write_all(&s.to_le_bytes())?;
                }
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    stop.store(true, Ordering::Relaxed);
    capture.join().map_err(|_| "capture join")?;
    file.flush()?;
    write_meta(&args.out, args.duration, frames, &args.description)?;
    println!(
        "{{\"frames\": {frames}, \"path\": \"{}\", \"xruns\": {}}}",
        args.out.display(),
        stats.xrun_count.load(Ordering::Relaxed),
    );
    Ok(())
}
