//! End-to-end: ReplayPcmSource → CaptureThread → AudioService → meeting_tx + stats_tee.
//!
//! Run with: cargo test -p waved-audio --features capture-replay --test replay_smoke

#![cfg(feature = "capture-replay")]

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{broadcast, mpsc, watch};
use waved_audio::capture::{run_capture_loop, CaptureStats};
use waved_audio::capture::replay::{ReplayFormat, ReplayPcmSource};
use waved_audio::fixtures::{silence_8ch, tone_mono_8ch};
use waved_audio::AudioService;
use waved_state::{AudioState, Command};

fn write_i16_pcm(data: &[i16]) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    for &s in data { f.write_all(&s.to_le_bytes()).unwrap(); }
    f.flush().unwrap();
    f
}

#[tokio::test]
async fn replay_silence_produces_zero_vu() {
    let data = silence_8ch(1); // 48000 frames * 8 ch
    let file = write_i16_pcm(&data);
    let (audio_tx, mut audio_rx) = watch::channel(AudioState::default());
    let (frame_tx, frame_rx) = mpsc::channel(32);
    let (meeting_tx, _meeting_rx) = mpsc::channel(32);
    let (cmd_tx, cmd_rx) = broadcast::channel::<Command>(8);
    let svc = AudioService::new(Arc::new(audio_tx), frame_tx.clone(), frame_rx, meeting_tx);
    tokio::spawn(async move { svc.run(cmd_rx).await });

    // Spawn capture thread manually
    let stats = CaptureStats::new();
    let stop = Arc::new(AtomicBool::new(false));
    let src = ReplayPcmSource::open(file.path(), ReplayFormat::I16_EIGHT, 48000).unwrap();
    let stop_c = Arc::clone(&stop);
    let stats_c = Arc::clone(&stats);
    let join = std::thread::spawn(move || {
        run_capture_loop(src, 256, 8, frame_tx, stop_c, stats_c);
    });

    // Wait for VU updates
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let vu = audio_rx.borrow_and_update().vu_level;
    assert!(vu.iter().all(|&v| v == 0.0), "silence should yield zero VU, got {vu:?}");
    stop.store(true, Ordering::Relaxed);
    join.join().unwrap();
    let _ = cmd_tx; // keep alive
}

#[tokio::test]
async fn replay_tone_elevates_vu_on_tone_channel() {
    let data = tone_mono_8ch(1, 1000.0, 0, 20_000);
    let file = write_i16_pcm(&data);
    let (audio_tx, mut audio_rx) = watch::channel(AudioState::default());
    let (frame_tx, frame_rx) = mpsc::channel(32);
    let (meeting_tx, _meeting_rx) = mpsc::channel(32);
    let (_cmd_tx, cmd_rx) = broadcast::channel::<Command>(8);
    let svc = AudioService::new(Arc::new(audio_tx), frame_tx.clone(), frame_rx, meeting_tx);
    tokio::spawn(async move { svc.run(cmd_rx).await });

    let stats = CaptureStats::new();
    let stop = Arc::new(AtomicBool::new(false));
    let src = ReplayPcmSource::open(file.path(), ReplayFormat::I16_EIGHT, 48000).unwrap();
    let stop_c = Arc::clone(&stop);
    let stats_c = Arc::clone(&stats);
    let join = std::thread::spawn(move || {
        run_capture_loop(src, 256, 8, frame_tx, stop_c, stats_c);
    });

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let vu = audio_rx.borrow_and_update().vu_level;
    assert!(vu[0] > 0.1, "tone channel 0 should have elevated VU, got {}", vu[0]);
    stop.store(true, Ordering::Relaxed);
    join.join().unwrap();
}
