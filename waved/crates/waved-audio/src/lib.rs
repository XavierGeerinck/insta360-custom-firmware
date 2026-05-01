pub mod aec;
pub mod audio_config;
pub mod beam;
pub mod callbacks;
pub mod doa;
pub mod capture;
pub mod playback;
pub mod processed_frame;
pub mod stats_collector;
pub mod spectrum;
pub mod fixtures;
pub mod metrics;

#[cfg(feature = "simulate")]
pub mod simulate;

#[cfg(feature = "capture-alsa")]
pub mod uac;

use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{info, warn};
use waved_state::{AudioState, Command, NrLevel};

use crate::aec::{speex, EchoCanceller, NoiseSuppressor};
use crate::beam::{Beamformer, MicGeometry};
use crate::callbacks::AudioFrame;
use crate::doa::DoaEstimator;
use crate::processed_frame::{mix_to_mono, ProcessedAudioFrame};

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

/// Build the DSP chain for a given NR level.
///
/// - `NrLevel::Off` → Speex AEC + no NS.
/// - `NrLevel::{Low, Med, High}` → Speex AEC + DeepFilterNet 3 (CPU). The
///   level maps to a DFN3 attenuation-limit cap: gentler levels mix more
///   of the original noisy signal back in, so the dereverb/NS becomes a
///   subtle tone-shaping rather than a hard mask.
///   On DFN3 init failure (e.g. tract couldn't load the bundled model on
///   this target) we fall back to `SpeexNoiseSuppressor`. SP#4 keeps the
///   speex path compiled in as a safety net.
fn build_dsp_chain(
    frame_size: usize,
    sample_rate: u32,
    nr: NrLevel,
) -> (Box<dyn EchoCanceller>, Option<Box<dyn NoiseSuppressor>>) {
    let ec = speex::SpeexEchoCanceller::new(frame_size, sample_rate);
    let ns: Option<Box<dyn NoiseSuppressor>> = if matches!(nr, NrLevel::Off) {
        None
    } else {
        Some(build_noise_suppressor(frame_size, sample_rate, &nr, &ec))
    };
    (Box::new(ec), ns)
}

/// DFN3 attenuation cap per NR level (in dB).
///
/// 100 dB ≈ "no cap" — DFN3 is free to attenuate as much as it wants.
/// 24 dB ≈ moderate — caps the most aggressive masking; output sits ~−24 dB
/// below the noisy input at most, leaves a touch of room tone audible.
/// 12 dB ≈ light — barely-there NS, useful when you want naturalness >
/// noise-floor.
fn nr_to_atten_lim_db(nr: &NrLevel) -> f32 {
    match nr {
        NrLevel::Off  => 0.0,   // bypass anyway, but a sane number for logging
        NrLevel::Low  => 12.0,
        NrLevel::Med  => 24.0,
        NrLevel::High => 100.0,
    }
}

/// DFN3 (CPU) → speex fallback. DFN3 only supports 48 kHz; any other rate
/// short-circuits to speex. Frame size is decoupled — `DeepFilterNs`
/// internally buffers to bridge ALSA's period (1024) and DFN3's 10 ms hop
/// (480). Init failure (e.g. tract couldn't load the bundled model on this
/// target) falls back cleanly.
fn build_noise_suppressor(
    frame_size: usize,
    sample_rate: u32,
    nr: &NrLevel,
    ec: &speex::SpeexEchoCanceller,
) -> Box<dyn NoiseSuppressor> {
    if sample_rate == 48_000 {
        let atten_lim_db = nr_to_atten_lim_db(nr);
        match aec::deep_filter::DeepFilterNs::with_atten_lim_db(atten_lim_db) {
            Ok(ns) => {
                tracing::info!(
                    backend = "deep_filter_cpu",
                    frame_size,
                    nr = ?nr,
                    atten_lim_db,
                    "neural NS active"
                );
                return Box::new(ns);
            }
            Err(e) => {
                tracing::warn!(error = %e, "DFN3 init failed; falling back to speex NS");
            }
        }
    } else {
        tracing::info!(sample_rate, "DFN3 needs 48 kHz; using speex NS");
    }
    Box::new(speex::SpeexNoiseSuppressor::new(
        frame_size,
        sample_rate,
        Some(ec),
    ))
}

pub struct AudioService {
    audio_tx: Arc<watch::Sender<AudioState>>,
    /// Kept alive so the channel stays open for future C SDK callback registration.
    _frame_tx: mpsc::Sender<AudioFrame>,
    frame_rx: mpsc::Receiver<AudioFrame>,
    meeting_tx: mpsc::Sender<ProcessedAudioFrame>,
    stats_tee: broadcast::Sender<AudioFrame>,
    processed_tx: broadcast::Sender<ProcessedAudioFrame>,
    frame_size: usize,
    sample_rate: u32,
    echo_canceller: Box<dyn EchoCanceller>,
    noise_suppressor: Option<Box<dyn NoiseSuppressor>>,
    mic_mono_scratch: Vec<i16>,
    ref_mono_scratch: Vec<i16>,
    beamformer: Beamformer,
    doa: DoaEstimator,
    /// When `Some((az, el))`, the beamformer steers to this fixed direction
    /// instead of tracking the live DOA. Settable via `Command::SetDoaOverride`.
    /// Used by the on-device FBR acceptance test.
    doa_override: Option<(f32, f32)>,
    mic_8ch_f32_scratch: Vec<f32>,
    /// FFT-based magnitude-spectrum binner for the recording-screen pill.
    /// Single-threaded, reused across frames so the FFT plan and Hann
    /// window are computed once.
    mic_spectrum: spectrum::Spectrum,
    ref_spectrum: spectrum::Spectrum,
    /// Mono downmix scratch for the mic spectrum input.
    mic_spectrum_input: Vec<i16>,
}

impl AudioService {
    pub fn new(
        audio_tx: Arc<watch::Sender<AudioState>>,
        frame_tx: mpsc::Sender<AudioFrame>,
        frame_rx: mpsc::Receiver<AudioFrame>,
        meeting_tx: mpsc::Sender<ProcessedAudioFrame>,
        frame_size: usize,
        sample_rate: u32,
        geometry: MicGeometry,
    ) -> Self {
        let (stats_tee, _) = broadcast::channel(16);
        let (processed_tx, _) = broadcast::channel(16);
        let (initial_nr, initial_mode) = {
            let s = audio_tx.borrow();
            (s.nr_level.clone(), s.mode.clone())
        };
        let (echo_canceller, noise_suppressor) =
            build_dsp_chain(frame_size, sample_rate, initial_nr);
        let mut beamformer = Beamformer::new(geometry.clone(), sample_rate);
        beamformer.set_mode(initial_mode);
        let doa = DoaEstimator::new(geometry, sample_rate, frame_size);
        Self {
            audio_tx,
            _frame_tx: frame_tx,
            frame_rx,
            meeting_tx,
            stats_tee,
            processed_tx,
            frame_size,
            sample_rate,
            echo_canceller,
            noise_suppressor,
            mic_mono_scratch: vec![0; frame_size],
            ref_mono_scratch: vec![0; frame_size],
            beamformer,
            doa,
            doa_override: None,
            mic_8ch_f32_scratch: vec![0.0; frame_size * 8],
            mic_spectrum: spectrum::Spectrum::new(sample_rate),
            ref_spectrum: spectrum::Spectrum::new(sample_rate),
            mic_spectrum_input: vec![0i16; frame_size],
        }
    }

    /// Test hook: set DOA directly, bypassing DoaEstimator.
    #[cfg(test)]
    pub fn set_doa(&mut self, az_deg: f32, el_deg: f32) {
        self.beamformer.set_doa(az_deg, el_deg);
    }

    /// Subscribe to the raw frame stream for diagnostics (stats collector,
    /// debug tools). Raw 8-ch mic — stats want per-channel RMS, which is
    /// meaningless on the post-AEC mono signal.
    pub fn subscribe_frames(&self) -> broadcast::Receiver<AudioFrame> {
        self.stats_tee.subscribe()
    }

    /// Subscribe to the processed frame stream (post-AEC mono + reference +
    /// raw mic for sidecar). Used by future stats consumers that want ERLE
    /// and by any downstream uplink path beyond meeting recording.
    pub fn subscribe_processed(&self) -> broadcast::Receiver<ProcessedAudioFrame> {
        self.processed_tx.subscribe()
    }

    /// Run the DSP pipeline on a single `AudioFrame`. Separate method so the
    /// async `run` loop can invoke it with a bounded borrow of `self` (calling
    /// a free function with split borrows inside `tokio::select!` triggers a
    /// `'static` inference issue on the trait-object references). Also used
    /// directly by unit tests.
    pub fn process_frame(&mut self, frame: &AudioFrame) -> ProcessedAudioFrame {
        let frame_size = self.frame_size;

        // Reference is the same shape in either branch — prepare it once.
        if self.ref_mono_scratch.len() != frame_size {
            self.ref_mono_scratch.resize(frame_size, 0);
        }
        match frame.reference.as_ref() {
            Some(r) if frame.reference_channels > 0 => {
                let mono_ref = mix_to_mono(r, frame.reference_channels as usize);
                let copy_len = mono_ref.len().min(frame_size);
                self.ref_mono_scratch[..copy_len].copy_from_slice(&mono_ref[..copy_len]);
                if copy_len < frame_size {
                    self.ref_mono_scratch[copy_len..].fill(0);
                }
            }
            _ => self.ref_mono_scratch.fill(0),
        }

        if frame.channels as usize == 8 && frame.data.len() == frame_size * 8 {
            // 8-ch beamforming path.
            if self.mic_8ch_f32_scratch.len() != frame_size * 8 {
                self.mic_8ch_f32_scratch.resize(frame_size * 8, 0.0);
            }
            for (i, &s) in frame.data.iter().enumerate() {
                self.mic_8ch_f32_scratch[i] = (s as f32) / 32768.0;
            }

            let doa = self.doa.update(&self.mic_8ch_f32_scratch);
            self.audio_tx.send_modify(|s| {
                s.doa_horizontal = doa.azimuth_deg;
                s.doa_pitch = doa.elevation_deg;
                s.doa_active = doa.vad_active;
            });
            let (steer_az, steer_el) = self
                .doa_override
                .unwrap_or((doa.azimuth_deg, doa.elevation_deg));
            self.beamformer.set_doa(steer_az, steer_el);

            let beam_out = self.beamformer.process(&self.mic_8ch_f32_scratch);

            if self.mic_mono_scratch.len() != frame_size {
                self.mic_mono_scratch.resize(frame_size, 0);
            }
            for (dst, &src) in self.mic_mono_scratch.iter_mut().zip(beam_out.mono.iter()) {
                *dst = (src.clamp(-1.0, 1.0) * 32768.0) as i16;
            }

            let mut aec_out = vec![0i16; frame_size];
            self.echo_canceller
                .process(&self.mic_mono_scratch, &self.ref_mono_scratch, &mut aec_out);
            if let Some(ns) = self.noise_suppressor.as_mut() {
                ns.process(&mut aec_out);
            }

            let mut p = ProcessedAudioFrame::from_raw(frame, &aec_out, &self.ref_mono_scratch);
            if let Some((l, r)) = beam_out.stereo {
                p = p.with_stereo(l, r);
            }
            p
        } else {
            // Legacy path: used by host tests that inject <8ch mic data.
            let mono_mic = mix_to_mono(&frame.data, frame.channels as usize);
            if self.mic_mono_scratch.len() != frame_size {
                self.mic_mono_scratch.resize(frame_size, 0);
            }
            let copy_len = mono_mic.len().min(frame_size);
            self.mic_mono_scratch[..copy_len].copy_from_slice(&mono_mic[..copy_len]);
            if copy_len < frame_size {
                self.mic_mono_scratch[copy_len..].fill(0);
            }

            let mut aec_out = vec![0i16; frame_size];
            self.echo_canceller
                .process(&self.mic_mono_scratch, &self.ref_mono_scratch, &mut aec_out);
            if let Some(ns) = self.noise_suppressor.as_mut() {
                ns.process(&mut aec_out);
            }

            ProcessedAudioFrame::from_raw(frame, &aec_out, &self.ref_mono_scratch)
        }
    }

    pub async fn run(mut self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!(
            "audio_srv starting (DSP on: frame_size={} sample_rate={})",
            self.frame_size, self.sample_rate
        );

        loop {
            tokio::select! {
                frame = self.frame_rx.recv() => {
                    if let Some(mut frame) = frame {
                        let muted = self.audio_tx.borrow().mic_muted;
                        apply_mic_mute(&mut frame.data, muted);
                        let vu = compute_vu(&frame.data, 8);
                        let incoming_vu = frame
                            .reference
                            .as_deref()
                            .map(|reference| compute_vu(reference, 8))
                            .unwrap_or_else(|| vec![0.0; 8]);

                        // Mic spectrum: downmix 8-ch → mono i16, FFT,
                        // log-spaced bin to FREQ_BAND_COUNT magnitudes.
                        let mono = mix_to_mono(&frame.data, frame.channels.max(1) as usize);
                        let n = mono.len().min(self.mic_spectrum_input.len());
                        self.mic_spectrum_input[..n].copy_from_slice(&mono[..n]);
                        if n < self.mic_spectrum_input.len() {
                            self.mic_spectrum_input[n..].fill(0);
                        }
                        let mut mic_bands = [0.0f32; waved_state::FREQ_BAND_COUNT];
                        self.mic_spectrum.compute(&self.mic_spectrum_input, &mut mic_bands);

                        // Reference spectrum: same pipeline on the
                        // far-end / playback signal when present.
                        let mut ref_bands = [0.0f32; waved_state::FREQ_BAND_COUNT];
                        if let Some(reference) = frame.reference.as_deref() {
                            let chs = frame.reference_channels.max(1) as usize;
                            let mono = mix_to_mono(reference, chs);
                            self.ref_spectrum.compute(&mono, &mut ref_bands);
                        }

                        self.audio_tx.send_modify(|s| {
                            s.vu_level.copy_from_slice(&vu);
                            s.incoming_vu_level.copy_from_slice(&incoming_vu);
                            s.vu_freq_bands.copy_from_slice(&mic_bands);
                            s.incoming_freq_bands.copy_from_slice(&ref_bands);
                        });
                        let _ = self.stats_tee.send(frame.clone());

                        let processed = self.process_frame(&frame);
                        let _ = self.processed_tx.send(processed.clone());
                        let _ = self.meeting_tx.try_send(processed);
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Ok(Command::SetAudioMode(mode)) => {
                            info!("audio mode → {mode:?}");
                            self.beamformer.set_mode(mode.clone());
                            self.audio_tx.send_modify(|s| s.mode = mode);
                        }
                        Ok(Command::SetDoaOverride(ov)) => {
                            match ov {
                                Some((az, el)) => info!("DOA override → ({az:.1}°, {el:.1}°)"),
                                None => info!("DOA override cleared — resuming DOA tracking"),
                            }
                            self.doa_override = ov;
                        }
                        Ok(Command::SetNrLevel(level)) => {
                            info!("NR level -> {level:?}; rebuilding DSP chain");
                            let (ec, ns) = build_dsp_chain(self.frame_size, self.sample_rate, level.clone());
                            self.echo_canceller = ec;
                            self.noise_suppressor = ns;
                            self.audio_tx.send_modify(|s| s.nr_level = level);
                        }
                        Ok(Command::SetPlaybackPreset(p)) => {
                            info!("playback preset → {p:?}");
                            self.audio_tx.send_modify(|s| s.playback_preset = p);
                        }
                        Ok(Command::ToggleMicMute) => {
                            self.audio_tx.send_modify(|s| s.mic_muted = !s.mic_muted);
                        }
                        Ok(Command::ToggleSpeakerMute) => {
                            let new_muted = !self.audio_tx.borrow().speaker_muted;
                            self.audio_tx.send_modify(|s| {
                                s.speaker_muted = new_muted;
                                if !new_muted && s.speaker_volume < 10 { s.speaker_volume = 10; }
                            });
                            // SP#6: UacControl mirrors the new state to hw:1
                            // PCM Playback Switch on the next state-watch tick.
                        }
                        Ok(Command::SetSpeakerVolume(v)) => {
                            self.audio_tx.send_modify(|s| s.speaker_volume = v);
                            // SP#6: UacControl mirrors to hw:1 PCM Playback Volume.
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

    /// End-to-end DSP test through `AudioService::process_frame`. Injects a
    /// synthetic `AudioFrame` whose mic is an echo of its reference; asserts
    /// the processed mono RMS is at least 10 dB below the raw mic RMS after
    /// convergence. Uses SpeexEchoCanceller with no NS (NrLevel::Off).
    #[test]
    fn audio_service_process_frame_cancels_echo() {
        let sr: u32 = 48000;
        let frame_size: usize = 480;
        let mic_channels: u16 = 8;
        let ref_channels: u16 = 2;
        let duration_frames: usize = (sr as usize / frame_size) * 2; // ~2 s

        // Stand up a minimal AudioService with NrLevel::Off so NS doesn't
        // confound the ERLE measurement.
        let mut initial_state = AudioState::default();
        initial_state.nr_level = NrLevel::Off;
        let (audio_tx, _audio_rx) = tokio::sync::watch::channel(initial_state);
        let audio_tx = Arc::new(audio_tx);
        let (frame_tx, frame_rx) = tokio::sync::mpsc::channel::<AudioFrame>(4);
        let (meeting_tx, _meeting_rx) = tokio::sync::mpsc::channel::<ProcessedAudioFrame>(4);
        let mut svc = AudioService::new(
            audio_tx,
            frame_tx,
            frame_rx,
            meeting_tx,
            frame_size,
            sr,
            MicGeometry::default(),
        );

        let step = 2.0 * std::f32::consts::PI * 1000.0 / sr as f32;
        let mut raw_rms_sum = 0.0f64;
        let mut raw_rms_n = 0usize;
        let mut out_rms_sum = 0.0f64;
        let mut out_rms_n = 0usize;

        for f in 0..duration_frames {
            let base = f * frame_size;
            let ref_mono: Vec<i16> = (0..frame_size)
                .map(|i| {
                    let v = ((base + i) as f32 * step).sin() * 0.5 * i16::MAX as f32;
                    v as i16
                })
                .collect();
            let mut ref_interleaved = vec![0i16; frame_size * ref_channels as usize];
            for (i, &s) in ref_mono.iter().enumerate() {
                ref_interleaved[2 * i] = s;
                ref_interleaved[2 * i + 1] = s;
            }
            let mic_mono: Vec<i16> = ref_mono.iter().map(|&s| (s as i32 / 2) as i16).collect();
            let mut mic_interleaved = vec![0i16; frame_size * mic_channels as usize];
            for (i, &s) in mic_mono.iter().enumerate() {
                for ch in 0..mic_channels as usize {
                    mic_interleaved[i * mic_channels as usize + ch] = s;
                }
            }

            let audio_frame = AudioFrame::paired(
                mic_interleaved,
                mic_channels,
                ref_interleaved,
                ref_channels,
                sr,
                (f * frame_size * 1000 / sr as usize) as u64,
            );

            let processed = svc.process_frame(&audio_frame);

            if f >= duration_frames / 2 {
                for &s in &mic_mono {
                    raw_rms_sum += (s as f64).powi(2);
                    raw_rms_n += 1;
                }
                for &v in &processed.mic_mono_f32 {
                    let s = (v * 32768.0) as f64;
                    out_rms_sum += s.powi(2);
                    out_rms_n += 1;
                }
            }
        }

        let raw_rms = (raw_rms_sum / raw_rms_n as f64).sqrt();
        let out_rms = (out_rms_sum / out_rms_n as f64).sqrt().max(1.0);
        let attenuation_db = 20.0 * (raw_rms / out_rms).log10();
        assert!(
            attenuation_db >= 10.0,
            "process_frame attenuation {attenuation_db:.1} dB < 10 dB (raw rms {raw_rms:.1}, out rms {out_rms:.1})"
        );
    }

    #[test]
    fn audio_service_applies_beamformer_before_aec() {
        use crate::beam::MicGeometry;
        use waved_state::AudioMode;

        let sr: u32 = 48000;
        let frame_size: usize = 480;
        let mic_channels: u16 = 8;

        let mut initial_state = AudioState::default();
        initial_state.nr_level = NrLevel::Off;
        initial_state.mode = AudioMode::Cardioid;
        let (audio_tx, _audio_rx) = tokio::sync::watch::channel(initial_state);
        let audio_tx = Arc::new(audio_tx);
        let (frame_tx, frame_rx) = tokio::sync::mpsc::channel::<AudioFrame>(4);
        let (meeting_tx, _meeting_rx) = tokio::sync::mpsc::channel::<ProcessedAudioFrame>(4);
        let mut svc = AudioService::new(
            audio_tx.clone(),
            frame_tx,
            frame_rx,
            meeting_tx,
            frame_size,
            sr,
            MicGeometry::default(),
        );

        let geom = MicGeometry::default();
        let omega = 2.0 * std::f32::consts::PI * 1000.0 / sr as f32;
        const C: f32 = 343.0;
        let mut samples = vec![0i16; frame_size * mic_channels as usize];
        for alsa_ch in 0..mic_channels as usize {
            let p = geom.position_for_alsa(alsa_ch);
            let tau_samples = -(p.x * 1e-3) / C * sr as f32;
            for i in 0..frame_size {
                let v = ((i as f32 - tau_samples) * omega).sin() * 0.3 * i16::MAX as f32;
                samples[i * mic_channels as usize + alsa_ch] = v as i16;
            }
        }
        let frame = AudioFrame::mic_only(samples, mic_channels, sr, 0);

        svc.set_doa(0.0, 0.0);
        let processed = svc.process_frame(&frame);
        let rms_front: f32 = {
            let sq: f32 = processed.mic_mono_f32.iter().map(|x| x * x).sum();
            (sq / processed.mic_mono_f32.len() as f32).sqrt()
        };
        assert!(rms_front > 0.05, "beamformer produced silence: rms={rms_front}");
    }
}
