//! Toggling mic mute via Command::ToggleMicMute must zero the PCM that
//! reaches the meeting service.
//!
//! Run with: cargo test -p waved-audio --features capture-replay --test mute_end_to_end

#![cfg(feature = "capture-replay")]

use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};
use waved_audio::callbacks::AudioFrame;
use waved_audio::processed_frame::ProcessedAudioFrame;
use waved_audio::AudioService;
use waved_state::{AudioState, Command};

/// Feeds frames directly into `AudioService` via the frame channel (blocking
/// `send().await` — no try_send drops), then verifies `Command::ToggleMicMute`
/// zeros subsequent frames arriving at the meeting receiver.
///
/// The feature gate is kept so this test compiles with the same knob as the
/// replay harness that validates the full capture pipeline.
#[tokio::test]
async fn mute_toggles_zero_meeting_frames() {
    const FRAME_SIZE: usize = 256;
    const MIC_CHANNELS: u16 = 8;
    const SR: u32 = 48_000;

    let (audio_tx, _audio_rx) = watch::channel(AudioState::default());
    let audio_tx = Arc::new(audio_tx);
    let (frame_tx, frame_rx) = mpsc::channel::<AudioFrame>(4);
    let (meeting_tx, mut meeting_rx) = mpsc::channel::<ProcessedAudioFrame>(8);
    let (cmd_tx, cmd_rx) = broadcast::channel::<Command>(8);
    let svc = AudioService::new(
        Arc::clone(&audio_tx),
        frame_tx.clone(),
        frame_rx,
        meeting_tx,
        FRAME_SIZE,
        SR,
        waved_audio::beam::MicGeometry::default(),
    );
    tokio::spawn(async move { svc.run(cmd_rx).await });

    // Build a single 1kHz tone frame on channel 0 (other channels silent).
    // Re-used for every send — no fixture file needed.
    let tone_frame: Vec<i16> = {
        let mut buf = vec![0i16; FRAME_SIZE * MIC_CHANNELS as usize];
        let omega = 2.0 * std::f32::consts::PI * 1000.0 / SR as f32;
        for i in 0..FRAME_SIZE {
            let v = ((i as f32) * omega).sin() * 15_000.0;
            buf[i * MIC_CHANNELS as usize] = v as i16;
        }
        buf
    };

    // Feed 10 unmuted frames; confirm they arrive with non-zero raw mic.
    let mut unmuted_nonzero = 0usize;
    for _ in 0..20 {
        frame_tx
            .send(AudioFrame::mic_only(tone_frame.clone(), MIC_CHANNELS, SR, 0))
            .await
            .unwrap();
        if let Ok(Some(f)) = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            meeting_rx.recv(),
        )
        .await
        {
            if f.raw_mic_i16.iter().any(|&s| s != 0) {
                unmuted_nonzero += 1;
            }
        }
        if unmuted_nonzero >= 10 {
            break;
        }
    }
    assert!(
        unmuted_nonzero >= 10,
        "expected at least 10 unmuted non-zero frames, got {unmuted_nonzero}"
    );

    // Toggle mute and wait for AudioService to observe it (yield the runtime
    // once — SetMicMute is a watch write that the run loop picks up on its
    // next select iteration).
    cmd_tx.send(Command::ToggleMicMute).unwrap();
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }

    let mut muted_zero_frames = 0usize;
    for _ in 0..20 {
        frame_tx
            .send(AudioFrame::mic_only(tone_frame.clone(), MIC_CHANNELS, SR, 0))
            .await
            .unwrap();
        match tokio::time::timeout(
            std::time::Duration::from_secs(1),
            meeting_rx.recv(),
        )
        .await
        {
            Ok(Some(f)) => {
                if f.raw_mic_i16.iter().all(|&s| s == 0) {
                    muted_zero_frames += 1;
                }
            }
            Ok(None) => panic!("meeting_rx closed before we got muted frames"),
            Err(_) => panic!("timed out waiting for muted frames"),
        }
        if muted_zero_frames >= 5 {
            break;
        }
    }
    assert!(
        muted_zero_frames >= 5,
        "expected at least 5 all-zero muted frames, got {muted_zero_frames}"
    );
}
