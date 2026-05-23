//! Subscribes to AudioService::subscribe_doa() and prints DoaFrame JSON to
//! stdout. Smoke-test for F3's future consumer path; not part of waved.
//!
//! Usage on device:
//!   adb push target/aarch64-unknown-linux-gnu/release/waved-doa-tap /data/waved/
//!   adb shell /data/waved/waved-doa-tap --duration-secs 10

use clap::Parser;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, watch};
use waved_audio::beam::MicGeometry;
use waved_audio::callbacks::AudioFrame;
use waved_audio::AudioService;
use waved_state::AudioState;

#[derive(Parser, Debug)]
struct Args {
    /// How long to listen (seconds).
    #[clap(long, default_value_t = 5)]
    duration_secs: u64,
    /// DoA backend to construct AudioService with. "gcc_phat" | "srp_phat".
    #[clap(long, default_value = "gcc_phat")]
    backend: String,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let backend = match args.backend.as_str() {
        "srp_phat" => waved_audio::doa::DoaBackend::SrpPhat,
        _ => waved_audio::doa::DoaBackend::GccPhat,
    };
    let (audio_tx_inner, _) = watch::channel(AudioState::default());
    let audio_tx = Arc::new(audio_tx_inner);
    let (frame_tx, frame_rx) = mpsc::channel::<AudioFrame>(64);
    let (meeting_tx, _meeting_rx) = mpsc::channel(64);
    let svc = AudioService::new_with_backend(
        audio_tx,
        frame_tx,
        frame_rx,
        meeting_tx,
        1024,
        48000,
        MicGeometry::default(),
        backend,
    );
    let mut rx = svc.subscribe_doa();

    // Run AudioService in the background — it will produce DoaFrames
    // whenever real audio arrives on frame_rx. Without a capture source
    // wired in, this only emits frames when something else calls
    // process_frame; on device with the real waved daemon already running,
    // operators should stop waved first and let this be the consumer.
    let (_cmd_tx, cmd_rx) = broadcast::channel::<waved_state::Command>(8);
    tokio::spawn(svc.run(cmd_rx));

    let deadline = Instant::now() + Duration::from_secs(args.duration_secs);
    let mut count = 0u64;
    while Instant::now() < deadline {
        tokio::select! {
            r = rx.recv() => {
                match r {
                    Ok(f) => {
                        println!("{}", serde_json::to_string(&f).unwrap());
                        count += 1;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("lagged {n} frames");
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                // Heartbeat so we can tell if no frames are flowing.
                if count == 0 {
                    eprintln!("(no frames yet — is waved running and feeding this AudioService?)");
                }
            }
        }
    }
    eprintln!("doa-tap: {count} frames in {} s", args.duration_secs);
    Ok(())
}
