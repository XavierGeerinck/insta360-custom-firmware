//! waved-wakeup-probe: one-shot benchmark of DSP wake-from-silence latency.
//!
//! Constructs its own `AudioService`, injects a deterministic speech-shaped
//! burst after N seconds of silence, and reports per-burst latency + leading
//! edge loss vs a warmup baseline. Run on device only (needs DFN3 model
//! init and the real Processor — does not touch ALSA).
//!
//! See `docs/superpowers/specs/2026-05-23-waved-wakeup-probe-design.md`.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(about = "DSP wake-from-silence benchmark for waved")]
struct Args {
    /// Idle duration before each cold burst (seconds).
    #[arg(long, default_value_t = 60)]
    silence_seconds: u32,
    /// Burst duration (milliseconds).
    #[arg(long, default_value_t = 200)]
    burst_ms: u32,
    /// Warm bursts to establish baseline.
    #[arg(long, default_value_t = 5)]
    warmup_bursts: u32,
    /// Cold (post-silence) bursts to measure.
    #[arg(long, default_value_t = 10)]
    cold_runs: u32,
    /// Optional path for JSON report output.
    #[arg(long)]
    json: Option<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    let args = Args::parse();
    eprintln!("waved-wakeup-probe: {args:?}");
    Ok(())
}
