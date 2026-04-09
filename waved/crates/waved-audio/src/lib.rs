pub mod callbacks;
pub mod ffi;

#[cfg(feature = "simulate")]
pub mod simulate;

use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{error, info, warn};
use waved_state::{AudioMode, AudioState, Command};

/// Compute N-band VU levels from i16 PCM samples.
/// Returns values in [0.0, 1.0] where 1.0 = i16::MAX RMS.
pub fn compute_vu(samples: &[i16], bands: usize) -> Vec<f32> {
    if samples.is_empty() || bands == 0 {
        return vec![0.0; bands];
    }
    let chunk = (samples.len() / bands).max(1);
    (0..bands)
        .map(|i| {
            let start = i * chunk;
            let end = ((i + 1) * chunk).min(samples.len());
            let rms = (samples[start..end]
                .iter()
                .map(|&s| (s as f64) * (s as f64))
                .sum::<f64>()
                / (end - start) as f64)
                .sqrt();
            (rms / i16::MAX as f64).min(1.0) as f32
        })
        .collect()
}

pub struct AudioService {
    audio_tx: Arc<watch::Sender<AudioState>>,
    /// Kept alive so the channel stays open for future C SDK callback registration.
    _frame_tx: mpsc::Sender<callbacks::AudioFrame>,
    frame_rx: mpsc::Receiver<callbacks::AudioFrame>,
    meeting_tx: mpsc::Sender<callbacks::AudioFrame>,
}

impl AudioService {
    /// * `frame_tx`/`frame_rx`: channel that C SDK callbacks will write PCM frames to.
    /// * `meeting_tx`: forwards each frame to MeetingService after VU computation.
    pub fn new(
        audio_tx: Arc<watch::Sender<AudioState>>,
        frame_tx: mpsc::Sender<callbacks::AudioFrame>,
        frame_rx: mpsc::Receiver<callbacks::AudioFrame>,
        meeting_tx: mpsc::Sender<callbacks::AudioFrame>,
    ) -> Self {
        Self { audio_tx, _frame_tx: frame_tx, frame_rx, meeting_tx }
    }

    pub async fn run(mut self, mut cmd_rx: broadcast::Receiver<Command>) {
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
            tokio::select! {
                frame = self.frame_rx.recv() => {
                    if let Some(frame) = frame {
                        let vu = compute_vu(&frame.data, 8);
                        self.audio_tx.send_modify(|s| s.vu_level.copy_from_slice(&vu));
                        let _ = self.meeting_tx.try_send(frame);
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_vu_level_silent_is_zero() {
        let samples = vec![0i16; 512];
        let vu = compute_vu(&samples, 8);
        assert!(vu.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn compute_vu_level_full_scale_is_one() {
        let samples = vec![i16::MAX; 512];
        let vu = compute_vu(&samples, 8);
        // Each band should be close to 1.0
        for v in &vu {
            assert!(*v > 0.9, "expected ~1.0, got {v}");
        }
    }
}
