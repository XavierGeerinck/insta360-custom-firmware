//! Steered-beam integration test. #[ignore]-gated because it constructs a
//! real AudioService and drives a synthesised 5-second clip through it —
//! slow vs unit tests, but the only way to verify end-to-end DoA → beam
//! tracking.
//!
//! Run with: cargo test -p waved-audio --test steered_beam_handoff -- --ignored

use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use waved_audio::AudioService;
use waved_audio::beam::MicGeometry;
use waved_audio::callbacks::AudioFrame;
use waved_state::{AudioMode, AudioState};

/// Synthesise an 8-channel i16 interleaved plane wave arriving from the given
/// azimuth/elevation. The per-channel delay is derived from the mic geometry so
/// GCC-PHAT can recover the direction.
///
/// Amplitude 0.5 × 32768 ≈ −6 dBFS → well above the 0.0003 (−70 dBFS) VAD
/// threshold.
fn synth_plane_wave(az_deg: f32, el_deg: f32, sr: u32, frames: usize, geom: &MicGeometry) -> Vec<i16> {
    let az = az_deg.to_radians();
    let el = el_deg.to_radians();
    let d = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
    let freq = 1000.0f32;
    let omega = 2.0 * std::f32::consts::PI * freq / sr as f32;
    const C: f32 = 343.0;
    let mut out = vec![0i16; frames * 8];
    for alsa_ch in 0..8 {
        let p = geom.position_for_alsa(alsa_ch);
        // Propagation delay: dot(position, direction) / c → delay in samples.
        let dot_mm = p.x * d[0] + p.y * d[1] + p.z * d[2];
        let tau_samples = (-(dot_mm * 1e-3) / C) * sr as f32;
        for i in 0..frames {
            let s = ((i as f32 - tau_samples) * omega).sin() * 0.5;
            out[i * 8 + alsa_ch] = (s * 16000.0) as i16;
        }
    }
    out
}

#[tokio::test]
#[ignore]
async fn steered_beam_follows_doa_during_handoff() {
    let geom = MicGeometry::default();
    let sr = 48000u32;
    let period = 480usize;

    let mut initial = AudioState::default();
    initial.mode = AudioMode::Steered { initial_az_deg: 0.0 };
    let (audio_tx_inner, _) = watch::channel(initial);
    let audio_tx = Arc::new(audio_tx_inner);
    let (frame_tx, frame_rx) = mpsc::channel(8);
    let (meeting_tx, _meeting_rx) = mpsc::channel(8);
    let mut svc = AudioService::new(
        audio_tx,
        frame_tx,
        frame_rx,
        meeting_tx,
        period,
        sr,
        geom.clone(),
    );
    let mut rx = svc.subscribe_doa();

    // Phase 1: 2 seconds of source at az=315° (= -45°)
    let n_periods = (2 * sr as usize) / period;
    let s1 = synth_plane_wave(315.0, 0.0, sr, period, &geom);
    for _ in 0..n_periods {
        let frame = AudioFrame {
            data: s1.clone(),
            channels: 8,
            reference: None,
            reference_channels: 0,
            sample_rate: sr,
            timestamp_ms: 0,
        };
        svc.process_frame(&frame);
    }

    // Drain DoA receiver — keep the most recent non-NaN frame.
    // The broadcast channel capacity is 64; 200 frames of phase 1 will cause
    // RecvError::Lagged. On Lagged, tokio advances the receiver to the oldest
    // available slot, so we continue draining rather than break to collect the
    // frames still buffered in the channel.
    let mut last_phase1 = None;
    loop {
        match rx.try_recv() {
            Ok(f) => {
                if !f.azimuth_deg.is_nan() {
                    last_phase1 = Some(f);
                }
            }
            // Lagged: receiver was too slow, some frames were dropped.
            // Continue — there are still buffered frames to read.
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                continue;
            }
            // Empty or closed — done.
            Err(_) => break,
        }
    }

    let p1 = last_phase1.expect("phase 1 should have produced at least one VAD-active DoA frame");
    assert!(
        !p1.azimuth_deg.is_nan(),
        "phase 1 azimuth is NaN — synthetic tone did not clear VAD threshold"
    );
    // Wrap-safe ±10° tolerance around 315°.
    let p1_err = ((p1.azimuth_deg - 315.0 + 360.0) % 360.0).min((315.0 - p1.azimuth_deg + 360.0) % 360.0);
    assert!(
        p1_err < 10.0,
        "phase 1 az {:.1}° should be within ±10° of 315° (err {:.1}°)",
        p1.azimuth_deg, p1_err
    );

    // Phase 2: 1 second silence — let SteeredBeam hold direction, don't assert DoA.
    let silence = vec![0i16; period * 8];
    for _ in 0..(sr as usize / period) {
        let frame = AudioFrame {
            data: silence.clone(),
            channels: 8,
            reference: None,
            reference_channels: 0,
            sample_rate: sr,
            timestamp_ms: 0,
        };
        svc.process_frame(&frame);
    }
    // Drain silence frames (all will be NaN). Continue on Lagged.
    loop {
        match rx.try_recv() {
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }

    // Phase 3: 2 seconds of source at az=90°
    let s3 = synth_plane_wave(90.0, 0.0, sr, period, &geom);
    let n_p3 = (2 * sr as usize) / period;
    for _ in 0..n_p3 {
        let frame = AudioFrame {
            data: s3.clone(),
            channels: 8,
            reference: None,
            reference_channels: 0,
            sample_rate: sr,
            timestamp_ms: 0,
        };
        svc.process_frame(&frame);
    }

    let mut last_phase3 = None;
    loop {
        match rx.try_recv() {
            Ok(f) => {
                if !f.azimuth_deg.is_nan() {
                    last_phase3 = Some(f);
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }

    let p3 = last_phase3.expect("phase 3 should have produced at least one VAD-active DoA frame");
    assert!(
        !p3.azimuth_deg.is_nan(),
        "phase 3 azimuth is NaN — synthetic tone did not clear VAD threshold"
    );
    let p3_err = (p3.azimuth_deg - 90.0).abs();
    assert!(
        p3_err < 10.0,
        "phase 3 az {:.1}° should track to 90° within ±10° (err {:.1}°)",
        p3.azimuth_deg, p3_err
    );
}
