//! Simulated audio service — generates silence frames at real-time rate.
use crate::callbacks::AudioFrame;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{info, warn};
use waved_state::{AudioMode, AudioState, Command};

const SAMPLES_PER_FRAME: usize = 960;
const CHANNELS: u16 = 2;
const FRAME_INTERVAL_MS: u64 = 20;

pub struct SimAudioService {
    audio_tx: Arc<watch::Sender<AudioState>>,
    frame_tx: mpsc::Sender<AudioFrame>,
}

impl SimAudioService {
    pub fn new(audio_tx: Arc<watch::Sender<AudioState>>, frame_tx: mpsc::Sender<AudioFrame>) -> Self {
        info!("[SIM] audio service created (silence generator)");
        Self { audio_tx, frame_tx }
    }

    pub async fn run(self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("[SIM] audio_srv starting (generating silence frames)");
        let mut frame_count: u64 = 0;
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(FRAME_INTERVAL_MS));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let frame = AudioFrame {
                        data: vec![0i16; SAMPLES_PER_FRAME * CHANNELS as usize],
                        sample_rate: 48000, channels: CHANNELS,
                        timestamp_ms: frame_count * FRAME_INTERVAL_MS,
                    };
                    let vu = crate::compute_vu(&frame.data, 8);
                    self.audio_tx.send_modify(|s| s.vu_level.copy_from_slice(&vu));
                    let _ = self.frame_tx.send(frame).await;
                    frame_count += 1;
                }
                cmd_result = cmd_rx.recv() => {
                    match cmd_result {
                        Ok(Command::SetAudioMode(mode)) => {
                            info!("[SIM] audio mode -> {:?}", mode);
                            self.audio_tx.send_modify(|s| s.mode = mode);
                        }
                        Ok(Command::ToggleMicMute) => { self.audio_tx.send_modify(|s| s.mic_muted = !s.mic_muted); }
                        Ok(Command::ToggleSpeakerMute) => { self.audio_tx.send_modify(|s| s.speaker_muted = !s.speaker_muted); }
                        Ok(Command::SetSpeakerVolume(v)) => { self.audio_tx.send_modify(|s| s.speaker_volume = v); }
                        Ok(Command::SetMicVolume(v)) => { self.audio_tx.send_modify(|s| s.mic_volume = v); }
                        Ok(Command::Shutdown) => return,
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Closed) => return,
                        Err(broadcast::error::RecvError::Lagged(n)) => { warn!("[SIM] audio_srv lagged {n} commands"); }
                    }
                }
            }
        }
    }
}
