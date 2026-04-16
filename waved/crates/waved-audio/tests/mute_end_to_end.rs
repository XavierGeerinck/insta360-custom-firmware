//! Toggling mic mute via Command::ToggleMicMute must zero the PCM that
//! reaches the meeting service.
//!
//! Run with: cargo test -p waved-audio --features capture-replay --test mute_end_to_end

#![cfg(feature = "capture-replay")]

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{broadcast, mpsc, watch};
use waved_audio::callbacks::AudioFrame;
use waved_audio::capture::{run_capture_loop, CaptureStats};
use waved_audio::capture::replay::{ReplayFormat, ReplayPcmSource};
use waved_audio::fixtures::tone_mono_8ch;
use waved_audio::AudioService;
use waved_state::{AudioState, Command};

fn write_pcm(data: &[i16]) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    for &s in data { f.write_all(&s.to_le_bytes()).unwrap(); }
    f.flush().unwrap();
    f
}

#[tokio::test]
async fn mute_toggles_zero_meeting_frames() {
    let fixture = tone_mono_8ch(10, 1000.0, 0, 15_000);
    let file = write_pcm(&fixture);

    let (audio_tx, _audio_rx) = watch::channel(AudioState::default());
    let audio_tx = Arc::new(audio_tx);
    // Small meeting_tx capacity so the capture loop slows down when it fills.
    // try_send drops frames when full, so we drain concurrently below.
    let (frame_tx, frame_rx) = mpsc::channel::<AudioFrame>(32);
    let (meeting_tx, mut meeting_rx) = mpsc::channel::<AudioFrame>(32);
    let (cmd_tx, cmd_rx) = broadcast::channel::<Command>(8);
    let svc = AudioService::new(Arc::clone(&audio_tx), frame_tx.clone(), frame_rx, meeting_tx);
    tokio::spawn(async move { svc.run(cmd_rx).await });

    let stats = CaptureStats::new();
    let stop = Arc::new(AtomicBool::new(false));
    let src = ReplayPcmSource::open(file.path(), ReplayFormat::I16_EIGHT, 48000).unwrap();
    let stop_c = Arc::clone(&stop);
    let stats_c = Arc::clone(&stats);
    let _join = std::thread::spawn(move || {
        run_capture_loop(src, 256, 8, frame_tx, stop_c, stats_c);
    });

    // Collect unmuted frames: wait for at least 10 non-zero frames before muting.
    let mut unmuted_nonzero = 0usize;
    loop {
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            meeting_rx.recv(),
        ).await {
            Ok(Some(f)) => {
                if f.data.iter().any(|&s| s != 0) {
                    unmuted_nonzero += 1;
                }
                if unmuted_nonzero >= 10 { break; }
            }
            Ok(None) => panic!("meeting_rx closed before we got unmuted frames"),
            Err(_) => panic!("timed out waiting for unmuted frames"),
        }
    }
    assert!(unmuted_nonzero >= 10, "expected at least 10 unmuted non-zero frames");

    // Toggle mute and collect subsequent frames.
    cmd_tx.send(Command::ToggleMicMute).unwrap();

    // Drain any in-flight unmuted frames (in the channel before mute took effect),
    // then wait for at least 5 all-zero (muted) frames.
    let mut muted_zero_frames = 0usize;
    let mut seen_nonzero_after_mute = true; // skip initial in-flight unmuted frames
    loop {
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            meeting_rx.recv(),
        ).await {
            Ok(Some(f)) => {
                let all_zero = f.data.iter().all(|&s| s == 0);
                if seen_nonzero_after_mute {
                    // Still draining in-flight unmuted frames
                    if all_zero { seen_nonzero_after_mute = false; muted_zero_frames += 1; }
                } else {
                    if all_zero {
                        muted_zero_frames += 1;
                    } else {
                        // A non-zero frame arrived after we started seeing zeros — unexpected
                        panic!("non-zero frame arrived after mute was applied");
                    }
                }
                if muted_zero_frames >= 5 { break; }
            }
            Ok(None) => panic!("meeting_rx closed before we got muted frames"),
            Err(_) => panic!("timed out waiting for muted frames"),
        }
    }
    assert!(muted_zero_frames >= 5, "expected at least 5 all-zero muted frames");

    stop.store(true, Ordering::Relaxed);
}
