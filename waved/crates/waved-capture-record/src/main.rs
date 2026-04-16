//! waved-capture-record: captures N seconds of i16 PCM (mic channels only,
//! or mic + reference when AudioConfig enables dual-stream) to a file and
//! writes a sidecar .meta.json describing the capture.

use clap::Parser;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use waved_audio::audio_config::AudioConfig;
use waved_audio::callbacks::AudioFrame;
use waved_audio::capture::alsa::{AlsaPcmSource, AlsaStreamConfig};
use waved_audio::capture::alsa_dual_stream::AlsaDualStreamSource;
use waved_audio::capture::single_stream::SingleStreamFrameSource;
use waved_audio::capture::{run_capture_loop, CaptureStats};

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

fn write_meta(
    pcm_path: &Path,
    mic_channels: u32,
    reference_channels: u32,
    reference_path: Option<&Path>,
    sample_rate: u32,
    duration_s: u32,
    frames: u64,
    description: &str,
) -> std::io::Result<()> {
    let meta_path = pcm_path.with_extension("meta.json");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let meta = serde_json::json!({
        "schema": 1,
        "format": "i16_le_interleaved",
        "sample_rate": sample_rate,
        "mic_channels": mic_channels,
        "reference_channels": reference_channels,
        "reference_path": reference_path.map(|p| p.display().to_string()),
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

    let (cfg, _) = AudioConfig::load_or_default(Path::new("/etc/waved-audio-config.json"));

    // Source: dual-stream if reference enabled, single otherwise.
    enum Src {
        Dual(AlsaDualStreamSource),
        Single(SingleStreamFrameSource<AlsaPcmSource>),
    }
    let (src, has_ref) = if cfg.reference_enabled() {
        (Src::Dual(AlsaDualStreamSource::from_capture_config(&cfg.capture, cfg.period_frames, cfg.buffer_periods)?), true)
    } else {
        let mic = AlsaPcmSource::open(&AlsaStreamConfig {
            device: &cfg.capture.mic_device,
            channels: cfg.capture.mic_channels,
            sample_rate: cfg.capture.mic_sample_rate,
            format: cfg.capture.mic_format,
            period_frames: cfg.period_frames,
            buffer_periods: cfg.buffer_periods,
        })?;
        (Src::Single(SingleStreamFrameSource::new(mic, cfg.period_frames, cfg.capture.mic_channels as usize)), false)
    };

    let stats = CaptureStats::new();
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = mpsc::channel::<AudioFrame>(256);
    let stats_c = Arc::clone(&stats);
    let stop_c = Arc::clone(&stop);
    let capture = std::thread::spawn(move || match src {
        Src::Dual(s) => run_capture_loop(s, tx, stop_c, stats_c),
        Src::Single(s) => run_capture_loop(s, tx, stop_c, stats_c),
    });

    let mut pcm_file = std::fs::File::create(&args.out)?;
    let ref_path = args.out.with_extension("ref.pcm");
    let mut ref_file: Option<std::fs::File> = if has_ref {
        Some(std::fs::File::create(&ref_path)?)
    } else {
        None
    };

    let mut frames = 0u64;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(args.duration as u64);
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Some(frame)) => {
                frames += 1;
                for &s in &frame.data { pcm_file.write_all(&s.to_le_bytes())?; }
                if let (Some(rf), Some(refbuf)) = (ref_file.as_mut(), frame.reference.as_ref()) {
                    for &s in refbuf { rf.write_all(&s.to_le_bytes())?; }
                }
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    stop.store(true, Ordering::Relaxed);
    capture.join().map_err(|_| "capture join")?;
    pcm_file.flush()?;
    drop(pcm_file);
    if let Some(mut rf) = ref_file {
        rf.flush()?;
        drop(rf);
    }
    let ref_channels = if has_ref { cfg.capture.reference_channels } else { 0 };
    let ref_path_opt = if has_ref { Some(ref_path.as_path()) } else { None };
    write_meta(
        &args.out,
        cfg.capture.mic_channels,
        ref_channels,
        ref_path_opt,
        cfg.capture.mic_sample_rate,
        args.duration,
        frames,
        &args.description,
    )?;
    let summary = serde_json::json!({
        "frames": frames,
        "path": args.out.display().to_string(),
        "xruns": stats.xrun_count.load(Ordering::Relaxed),
    });
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}
