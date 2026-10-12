//! Replay a raw device recording through `AudioService`, as the device would
//! process it, to tune the uplink and the direction estimate on the host.
//!
//! Record on the device with waved stopped (it holds the capture device):
//!     adb shell arecord -D hw:0,2 -c 8 -r 48000 -f S32_LE -d 10 /tmp/raw.wav
//! then:
//!     cargo run --release -p waved-audio --example replay_raw -- raw.wav uplink.wav [omni|cardioid|supercardioid|steered] > doa.tsv
//!
//! `uplink.wav` is the processed mono uplink (16-bit); stdout is one TSV row
//! per DoA frame: time, confidence, VAD, azimuth, elevation, Whole room's
//! omni mix, and the uplink level at each stage. The input must be the
//! 44-byte-header WAV `arecord` writes (S32, 8 channels, 48 kHz).
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use waved_audio::callbacks::AudioFrame;
use waved_audio::AudioService;
use waved_state::{AudioMode, AudioState, NrLevel};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).unwrap();
    let data = &bytes[44..];
    let mode = match args.get(3).map(String::as_str).unwrap_or("omni") {
        "cardioid" => AudioMode::Cardioid,
        "supercardioid" => AudioMode::Supercardioid,
        "steered" => AudioMode::Steered { initial_az_deg: 0.0 },
        _ => AudioMode::Omni,
    };
    let frame_size = 1024usize;
    let state = AudioState { mode, nr_level: NrLevel::Off, ..AudioState::default() };
    let audio_tx = Arc::new(watch::channel(state).0);
    let (frame_tx, frame_rx) = mpsc::channel(4);
    let (meeting_tx, _meeting_rx) = mpsc::channel(4);
    let mut svc = AudioService::new(audio_tx.clone(), frame_tx, frame_rx, meeting_tx, frame_size, 48_000, Default::default());
    let mut doa = svc.subscribe_doa();
    let samples: Vec<i16> = data.chunks_exact(4).map(|b| (i32::from_le_bytes([b[0], b[1], b[2], b[3]]) >> 16) as i16).collect();
    let mut out: Vec<i16> = Vec::new();
    println!("t_ms conf vad az el omni_mix pre_aec post_lin post_res uplink");
    for (i, chunk) in samples.chunks_exact(frame_size * 8).enumerate() {
        let frame = AudioFrame { data: chunk.to_vec(), reference: None, sample_rate: 48_000, channels: 8, reference_channels: 0, timestamp_ms: (i * frame_size * 1000 / 48_000) as u64 };
        let p = svc.process_frame(&frame);
        let m = &p.aec_metrics;
        while let Ok(d) = doa.try_recv() {
            let b = m.beam.as_ref();
            println!("{} {:.3} {} {:.0} {:.0} {:.2} {:.1} {:.1} {:.1} {:.1}", frame.timestamp_ms, d.confidence, d.vad as u8, d.azimuth_deg, d.elevation_deg, b.map(|b| b.omni_mix).unwrap_or(-1.0), m.mic_pre_aec_rms, m.mic_post_linear_aec_rms, m.mic_post_residual_echo_rms, m.mic_uplink_rms);
        }
        out.extend(p.mic_mono_f32.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
    }
    let mut w = Vec::new();
    let n = (out.len() * 2) as u32;
    w.extend_from_slice(b"RIFF"); w.extend_from_slice(&(36 + n).to_le_bytes()); w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes()); w.extend_from_slice(&1u16.to_le_bytes()); w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&48_000u32.to_le_bytes()); w.extend_from_slice(&96_000u32.to_le_bytes()); w.extend_from_slice(&2u16.to_le_bytes()); w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data"); w.extend_from_slice(&n.to_le_bytes());
    for s in out { w.extend_from_slice(&s.to_le_bytes()); }
    std::fs::write(&args[2], w).unwrap();
}
