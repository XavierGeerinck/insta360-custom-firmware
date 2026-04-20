pub mod audio_config;
pub mod callbacks;
pub mod capture;
pub mod playback;
pub mod stats_collector;
pub mod speaker_mixer;
pub mod fixtures;
pub mod metrics;

#[cfg(feature = "simulate")]
pub mod simulate;

use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{info, warn};
use waved_state::{AudioState, Command};

/// Zero-fill PCM samples in place when `muted`. Used by the capture loop to
/// implement mic mute in software. libInstaAudio's `setMicMute` does the same
/// thing (multiplies the gain ratio by 0 in its DSP chain); we short-circuit
/// at the frame boundary instead, which is equivalent for our use case (VU +
/// meeting recording) and avoids pulling in the full audio-server singleton.
pub fn apply_mic_mute(samples: &mut [i16], muted: bool) {
    if muted {
        samples.fill(0);
    }
}

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
    stats_tee: broadcast::Sender<callbacks::AudioFrame>,
    speaker_control: Option<String>,
}

impl AudioService {
    pub fn new(
        audio_tx: Arc<watch::Sender<AudioState>>,
        frame_tx: mpsc::Sender<callbacks::AudioFrame>,
        frame_rx: mpsc::Receiver<callbacks::AudioFrame>,
        meeting_tx: mpsc::Sender<callbacks::AudioFrame>,
    ) -> Self {
        let (stats_tee, _) = broadcast::channel(16);
        let speaker_control = speaker_mixer::discover_control();
        if speaker_control.is_none() {
            warn!("no amixer speaker control found — speaker mute/volume will be state-only");
        }
        Self { audio_tx, _frame_tx: frame_tx, frame_rx, meeting_tx, stats_tee, speaker_control }
    }

    /// Subscribe to the frame stream for diagnostics (stats collector, debug tools).
    pub fn subscribe_frames(&self) -> broadcast::Receiver<callbacks::AudioFrame> {
        self.stats_tee.subscribe()
    }

    pub async fn run(mut self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("audio_srv starting (consumer-only loop)");

        loop {
            tokio::select! {
                frame = self.frame_rx.recv() => {
                    if let Some(mut frame) = frame {
                        let muted = self.audio_tx.borrow().mic_muted;
                        apply_mic_mute(&mut frame.data, muted);
                        let vu = compute_vu(&frame.data, 8);
                        self.audio_tx.send_modify(|s| s.vu_level.copy_from_slice(&vu));
                        let _ = self.stats_tee.send(frame.clone());
                        let _ = self.meeting_tx.try_send(frame);
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Ok(Command::SetAudioMode(mode)) => {
                            tracing::debug!("SetAudioMode({mode:?}): state-only until SP#3 (beamforming) or SP#4 (NS)");
                            self.audio_tx.send_modify(|s| s.mode = mode);
                        }
                        Ok(Command::ToggleMicMute) => {
                            // Implemented via software PCM zeroing in the frame loop above
                            // (see `apply_mic_mute`). libInstaAudio's setMicMute does the same
                            // thing internally (gain ratio → 0); we skip the full audio-server
                            // init by muting at the frame boundary.
                            self.audio_tx.send_modify(|s| s.mic_muted = !s.mic_muted);
                        }
                        Ok(Command::ToggleSpeakerMute) => {
                            let (new_muted, restore_vol) = {
                                let s = self.audio_tx.borrow();
                                (!s.speaker_muted, s.speaker_volume)
                            };
                            if let Some(ctrl) = self.speaker_control.clone() {
                                let _ = tokio::task::spawn_blocking(move || {
                                    let _ = speaker_mixer::set_mute(&ctrl, new_muted);
                                    if !new_muted {
                                        let _ = speaker_mixer::set_volume(&ctrl, restore_vol.max(10) as u32);
                                    }
                                }).await;
                            }
                            self.audio_tx.send_modify(|s| {
                                s.speaker_muted = new_muted;
                                if !new_muted && s.speaker_volume < 10 { s.speaker_volume = 10; }
                            });
                        }
                        Ok(Command::SetSpeakerVolume(v)) => {
                            if let Some(ctrl) = self.speaker_control.clone() {
                                let _ = tokio::task::spawn_blocking(move || {
                                    let _ = speaker_mixer::set_volume(&ctrl, v as u32);
                                }).await;
                            }
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

    #[test]
    fn apply_mic_mute_zeros_when_muted() {
        let mut samples = vec![1234i16; 16];
        apply_mic_mute(&mut samples, true);
        assert!(samples.iter().all(|&s| s == 0));
    }

    #[test]
    fn apply_mic_mute_preserves_when_unmuted() {
        let mut samples = vec![1234i16; 16];
        apply_mic_mute(&mut samples, false);
        assert!(samples.iter().all(|&s| s == 1234));
    }
}
