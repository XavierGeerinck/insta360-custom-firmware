pub mod callbacks;
pub mod ffi;

#[cfg(feature = "simulate")]
pub mod simulate;

use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{error, info, warn};
use waved_state::{AudioMode, AudioState, Command};

pub struct AudioService {
    audio_tx: Arc<watch::Sender<AudioState>>,
    _frame_tx: mpsc::Sender<callbacks::AudioFrame>,
}

impl AudioService {
    pub fn new(
        audio_tx: Arc<watch::Sender<AudioState>>,
        frame_tx: mpsc::Sender<callbacks::AudioFrame>,
    ) -> Self {
        Self { audio_tx, _frame_tx: frame_tx }
    }

    pub async fn run(self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("audio_srv starting");

        let init_result = tokio::task::spawn_blocking(|| ffi::init_audio()).await;
        match init_result {
            Ok(Ok(())) => info!("TEA SDK initialized"),
            Ok(Err(e)) => {
                error!("TEA SDK init failed: {e}");
                return;
            }
            Err(e) => {
                error!("TEA SDK init thread panicked: {e}");
                return;
            }
        }

        loop {
            match cmd_rx.recv().await {
                Ok(Command::SetAudioMode(mode)) => {
                    let mode_num = match mode {
                        AudioMode::NoiseReduction => 1,
                        AudioMode::Omni => 2,
                        AudioMode::Cardioid => 3,
                        AudioMode::Supercardioid => 4,
                        AudioMode::Figure8 => 5,
                        AudioMode::Stereo => 6,
                    };
                    let _ = tokio::task::spawn_blocking(move || ffi::set_work_mode(mode_num)).await;
                    self.audio_tx.send_modify(|s| s.mode = mode);
                }
                Ok(Command::ToggleMicMute) => {
                    self.audio_tx.send_modify(|s| s.mic_muted = !s.mic_muted);
                }
                Ok(Command::ToggleSpeakerMute) => {
                    self.audio_tx.send_modify(|s| s.speaker_muted = !s.speaker_muted);
                }
                Ok(Command::SetSpeakerVolume(v)) => {
                    self.audio_tx.send_modify(|s| s.speaker_volume = v);
                }
                Ok(Command::SetMicVolume(v)) => {
                    self.audio_tx.send_modify(|s| s.mic_volume = v);
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("audio_srv lagged {n} commands");
                }
            }
        }
    }
}
