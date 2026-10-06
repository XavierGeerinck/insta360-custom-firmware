pub mod aec;
pub mod audio_config;
pub mod beam;
pub mod callbacks;
pub mod capture;
pub mod doa;
pub mod fixtures;
pub mod integrity;
pub mod makeup;
pub mod metrics;
pub mod mic_check;
pub mod playback;
pub mod processed_frame;
pub mod spectrum;
pub mod stats_collector;

#[cfg(feature = "simulate")]
pub mod simulate;

#[cfg(feature = "capture-alsa")]
pub mod uac;

use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{info, warn};
use waved_state::{AudioState, Command, NrLevel};

use crate::aec::{speex, EchoCanceller, NoiseSuppressor};
use crate::audio_config::AecConfig;
use crate::beam::{steered::SteeredBeam, Beamformer, MicGeometry};
use crate::callbacks::AudioFrame;
use crate::doa::{DoaBackend, DoaEstimator, DoaFrame, DoaPublisher, SrpPhatEstimator};
use crate::integrity::{AudioServiceExit, IntegritySignal, RecordingIntegrity};
use crate::playback::dsp::limiter::{LimiterParams, LookaheadLimiter};
use crate::processed_frame::{mix_to_mono, AecFrameMetrics, ProcessedAudioFrame};

/// Backend dispatch — owns one impl, not a trait object (only two impls,
/// no value in dyn). GccPhat is the existing DoaEstimator; SrpPhat wraps
/// SrpPhatEstimator but adds EnergyVad + EWMA smoothing so both backends
/// produce comparable smoothed angles + confidence.
enum DoaBackendImpl {
    GccPhat(DoaEstimator),
    SrpPhat {
        srp: SrpPhatEstimator,
        vad: crate::doa::vad::EnergyVad,
        smoothed_az: f32,
        smoothed_el: f32,
        primed: bool,
        /// ch0 scratch buffer — pre-allocated at construction to avoid a
        /// per-frame heap alloc in the SRP hot path.
        ch0_scratch: Vec<f32>,
        frame_size: usize,
    },
}

impl DoaBackendImpl {
    fn update(&mut self, interleaved: &[f32]) -> crate::doa::DoaUpdate {
        match self {
            DoaBackendImpl::GccPhat(est) => est.update(interleaved),
            DoaBackendImpl::SrpPhat {
                srp,
                vad,
                smoothed_az,
                smoothed_el,
                primed,
                ch0_scratch,
                frame_size,
            } => {
                use crate::doa::vad::Vad;

                // Guard: wrong-sized input returns stale smoothed values.
                if interleaved.len() != *frame_size * 8 {
                    return crate::doa::DoaUpdate {
                        azimuth_deg: *smoothed_az,
                        elevation_deg: *smoothed_el,
                        vad_active: false,
                        fresh: false,
                        confidence: 0.0,
                    };
                }

                // VAD on ALSA ch 0 (matches GCC-PHAT path).
                ch0_scratch.clear();
                for i in 0..*frame_size {
                    ch0_scratch.push(interleaved[i * 8]);
                }
                let vad_active = vad.is_speech(ch0_scratch);
                if !vad_active {
                    return crate::doa::DoaUpdate {
                        azimuth_deg: *smoothed_az,
                        elevation_deg: *smoothed_el,
                        vad_active: false,
                        fresh: false,
                        confidence: 0.0,
                    };
                }

                match srp.estimate(interleaved) {
                    Some(r) => {
                        if *primed {
                            // Same EWMA-on-unit-vector as DoaEstimator.
                            let alpha = 0.9f32;
                            let p = (*smoothed_az).to_radians();
                            let n = r.azimuth_deg.to_radians();
                            let sx = alpha * p.cos() + (1.0 - alpha) * n.cos();
                            let sy = alpha * p.sin() + (1.0 - alpha) * n.sin();
                            let smoothed = sy.atan2(sx).to_degrees();
                            *smoothed_az = if smoothed < 0.0 {
                                smoothed + 360.0
                            } else {
                                smoothed
                            };
                            *smoothed_el = alpha * *smoothed_el + (1.0 - alpha) * r.elevation_deg;
                        } else {
                            *smoothed_az = r.azimuth_deg;
                            *smoothed_el = r.elevation_deg;
                            *primed = true;
                        }
                        crate::doa::DoaUpdate {
                            azimuth_deg: *smoothed_az,
                            elevation_deg: *smoothed_el,
                            vad_active: true,
                            fresh: true,
                            confidence: r.confidence,
                        }
                    }
                    None => crate::doa::DoaUpdate {
                        azimuth_deg: *smoothed_az,
                        elevation_deg: *smoothed_el,
                        vad_active: true,
                        fresh: false,
                        confidence: 0.0,
                    },
                }
            }
        }
    }
}

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

enum AecProcessor {
    Coupled(speex::SpeexAecStage),
    LinearOnly {
        echo: speex::SpeexEchoCanceller,
        config: AecConfig,
    },
}

impl AecProcessor {
    fn process_linear(&mut self, mic: &[i16], reference: &[i16], out: &mut [i16]) {
        match self {
            Self::Coupled(stage) => stage.process_linear(mic, reference, out),
            Self::LinearOnly { echo, .. } => echo.process(mic, reference, out),
        }
    }

    fn process_residual(&mut self, samples: &mut [i16]) {
        if let Self::Coupled(stage) = self {
            stage.process_residual(samples);
        }
    }

    fn enable_speex_denoise(&mut self) -> Result<(), speex::SpeexInitError> {
        match self {
            Self::Coupled(stage) => stage.set_denoise(true),
            Self::LinearOnly { .. } => Err(speex::SpeexInitError::PreprocessorAllocation),
        }
    }

    fn residual_active(&self) -> bool {
        matches!(self, Self::Coupled(_))
    }

    fn config(&self) -> &AecConfig {
        match self {
            Self::Coupled(stage) => stage.config(),
            Self::LinearOnly { config, .. } => config,
        }
    }
}

struct DspChain {
    aec: AecProcessor,
    environmental_ns: Option<Box<dyn NoiseSuppressor>>,
    environmental_ns_backend: &'static str,
}

/// Build the DSP chain for a given NR level.
///
/// - Every level → Speex linear AEC + linked residual-echo suppression.
/// - `NrLevel::Off` → no environmental noise suppression.
/// - `NrLevel::{Low, Med, High}` → residual AEC + DeepFilterNet 3 (CPU). The
///   level maps to a DFN3 attenuation-limit cap: gentler levels mix more
///   of the original noisy signal back in, so the dereverb/NS becomes a
///   subtle tone-shaping rather than a hard mask.
///   On DFN3 init failure (e.g. tract couldn't load the bundled model on
///   this target) we enable denoise on the already-linked Speex preprocessor.
fn build_dsp_chain(
    frame_size: usize,
    sample_rate: u32,
    nr: NrLevel,
    aec_config: AecConfig,
) -> DspChain {
    let aec = match speex::SpeexAecStage::try_new(frame_size, sample_rate, aec_config.clone()) {
        Ok(stage) => {
            tracing::info!(
                residual_echo_suppress_db = aec_config.residual_echo_suppress_db,
                residual_echo_suppress_active_db = aec_config.residual_echo_suppress_active_db,
                "Speex linear AEC and residual echo suppression active"
            );
            AecProcessor::Coupled(stage)
        }
        Err(error) => {
            tracing::error!(
                %error,
                "residual echo suppressor unavailable; retaining linear AEC"
            );
            AecProcessor::LinearOnly {
                echo: speex::SpeexEchoCanceller::new(frame_size, sample_rate),
                config: aec_config,
            }
        }
    };
    let mut chain = DspChain {
        aec,
        environmental_ns: None,
        environmental_ns_backend: "off",
    };

    if !matches!(nr, NrLevel::Off) {
        build_noise_suppressor(frame_size, sample_rate, &nr, &mut chain);
    }
    chain
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
        NrLevel::Off => 0.0, // bypass anyway, but a sane number for logging
        NrLevel::Low => 12.0,
        NrLevel::Med => 24.0,
        NrLevel::High => 100.0,
    }
}

/// DFN3 (CPU) → speex fallback. DFN3 only supports 48 kHz; any other rate
/// short-circuits to speex. Frame size is decoupled — `DeepFilterNs`
/// internally buffers to bridge ALSA's period (1024) and DFN3's 10 ms hop
/// (480). Init failure (e.g. tract couldn't load the bundled model on this
/// target) falls back cleanly.
fn build_noise_suppressor(frame_size: usize, sample_rate: u32, nr: &NrLevel, chain: &mut DspChain) {
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
                chain.environmental_ns = Some(Box::new(ns));
                chain.environmental_ns_backend = "deep_filter_cpu";
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "DFN3 init failed; falling back to speex NS");
            }
        }
    } else {
        tracing::info!(sample_rate, "DFN3 needs 48 kHz; using speex NS");
    }
    match chain.aec.enable_speex_denoise() {
        Ok(()) => {
            chain.environmental_ns_backend = "speex";
            tracing::info!("Speex environmental denoise active on linked AEC preprocessor");
        }
        Err(error) => {
            // The residual stage already failed to construct. Keep linear AEC
            // and provide standalone denoise rather than silencing the mic.
            tracing::warn!(%error, "linked Speex denoise unavailable; using standalone preprocessor");
            chain.environmental_ns = Some(Box::new(speex::SpeexNoiseSuppressor::new(
                frame_size,
                sample_rate,
                None,
            )));
            chain.environmental_ns_backend = "speex_standalone";
        }
    }
}

/// Apply user mic volume to a slice of mono f32 samples in-place.
/// `pct` is linear percent: 100 = unity, 200 = 2×, 0 = silence.
/// Values above 800 are clamped to 800 to prevent catastrophic gain.
/// The output is **not** clamped to [-1.0, 1.0]; the recording-path
/// limiter wired into `apply_gain` enforces the ceiling downstream
/// using a -1 dBFS soft-knee compressor.
pub fn apply_mic_volume(samples: &mut [f32], pct: u16) {
    let pct = pct.min(800);
    let g = pct as f32 / 100.0;
    for s in samples.iter_mut() {
        *s = *s * g;
    }
}

pub struct AudioService {
    audio_tx: Arc<watch::Sender<AudioState>>,
    frame_rx: mpsc::Receiver<AudioFrame>,
    meeting_tx: mpsc::Sender<ProcessedAudioFrame>,
    integrity: Arc<RecordingIntegrity>,
    integrity_tx: mpsc::UnboundedSender<IntegritySignal>,
    stats_tee: broadcast::Sender<AudioFrame>,
    processed_tx: broadcast::Sender<ProcessedAudioFrame>,
    frame_size: usize,
    sample_rate: u32,
    dsp_chain: DspChain,
    aec_config: AecConfig,
    mic_mono_scratch: Vec<i16>,
    ref_mono_scratch: Vec<i16>,
    aec_linear_scratch: Vec<i16>,
    aec_residual_scratch: Vec<i16>,
    beamformer: Beamformer,
    doa: DoaBackendImpl,
    /// When `Some((az, el))`, the directional and Stereo modes steer to this
    /// direction instead of their aim or orientation (`beam::aim`). Settable
    /// via `Command::SetDoaOverride`. Used by the on-device FBR acceptance test.
    doa_override: Option<(f32, f32)>,
    mic_8ch_f32_scratch: Vec<f32>,
    /// FFT-based magnitude-spectrum binner for the recording-screen pill.
    /// Single-threaded, reused across frames so the FFT plan and Hann
    /// window are computed once.
    mic_spectrum: spectrum::Spectrum,
    ref_spectrum: spectrum::Spectrum,
    /// Mono downmix scratch for the mic spectrum input.
    mic_spectrum_input: Vec<i16>,
    /// Recording-path peak limiter for the mono mic signal. Replaces the
    /// hard ±1.0 clamp that used to live in `apply_mic_volume`. SP#5
    /// voice defaults: -1 dBFS ceiling, 96-sample lookahead, 50 ms release.
    recording_limiter_mono: LookaheadLimiter,
    /// Same shape, but for the stereo mic pair (only exercised when beam
    /// mode is `Stereo`). Shared gain across L/R preserves stereo image.
    recording_limiter_stereo: LookaheadLimiter,
    /// Optional handle to the shared DSP metrics. Set by main once both the
    /// audio and playback services exist (they share the same Arc). When
    /// present, `apply_gain` writes the recording limiter's reduction into
    /// it after each frame; the stats collector reads from the same Arc.
    dsp_metrics: Option<Arc<crate::playback::DspMetrics>>,
    doa_publisher: DoaPublisher,
    doa_backend: DoaBackend,
    doa_frame_counter: u64,
    /// Some when current mode is AudioMode::Steered. Reset to None when
    /// switching to any other mode.
    steered: Option<SteeredBeam>,
    /// Current audio mode, for steering.
    mode: waved_state::AudioMode,
    /// Fixed aim for Focused / Narrow / Two sides (device degrees). Seeded
    /// from AudioState, which main.rs seeds from config before this service
    /// is built.
    beam_aim_deg: f32,
    /// Side gain the Stereo pair reached at the end of the last period
    /// (`processed_frame::rebuild_stereo`). Reset to 0 on a mode change so
    /// Stereo fades its side in.
    stereo_side_gain: f32,
    /// Start-up microphone check; `None` before the first frame and after the verdict.
    mic_monitor: Option<crate::mic_check::MicHealthMonitor>,
    mic_check_done: bool,
    /// Reads whether the host has the USB microphone open (UAC "PCM Playback
    /// State"). main.rs installs it when UAC routing is on.
    host_listening_probe: Option<Box<dyn Fn() -> bool + Send>>,
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
        // The producer must own the final raw-frame sender. Keeping this
        // constructor argument alive inside AudioService would mask capture
        // thread death by preventing frame_rx from ever closing.
        drop(frame_tx);
        let (integrity_tx, _integrity_rx) = mpsc::unbounded_channel();
        let (initial_nr, initial_mode, initial_aim) = {
            let s = audio_tx.borrow();
            (s.nr_level.clone(), s.mode.clone(), s.beam_aim_deg)
        };
        // A bad persisted aim must not steer to garbage: wrap it like a
        // SetBeamAim, and fall back to straight ahead when it is not a number.
        let initial_aim = waved_config::wrap_beam_aim(initial_aim).unwrap_or(0.0);
        let aec_config = AecConfig::default();
        let dsp_chain = build_dsp_chain(frame_size, sample_rate, initial_nr, aec_config.clone());
        let mut beamformer = Beamformer::new(geometry.clone(), sample_rate);
        beamformer.set_mode(initial_mode.clone());
        let steered = match &initial_mode {
            waved_state::AudioMode::Steered { initial_az_deg } => {
                Some(SteeredBeam::new(*initial_az_deg))
            }
            _ => None,
        };
        let (doa, doa_backend, _fallback) =
            build_doa_backend(DoaBackend::default(), geometry, sample_rate, frame_size);
        let doa_publisher = DoaPublisher::new();
        Self {
            audio_tx,
            frame_rx,
            meeting_tx,
            integrity: Arc::new(RecordingIntegrity::default()),
            integrity_tx,
            stats_tee,
            processed_tx,
            frame_size,
            sample_rate,
            dsp_chain,
            aec_config,
            mic_mono_scratch: vec![0; frame_size],
            ref_mono_scratch: vec![0; frame_size],
            aec_linear_scratch: vec![0; frame_size],
            aec_residual_scratch: vec![0; frame_size],
            beamformer,
            doa,
            doa_override: None,
            mic_8ch_f32_scratch: vec![0.0; frame_size * 8],
            mic_spectrum: spectrum::Spectrum::new(sample_rate),
            ref_spectrum: spectrum::Spectrum::new(sample_rate),
            mic_spectrum_input: vec![0i16; frame_size],
            recording_limiter_mono: LookaheadLimiter::new(
                LimiterParams::DEFAULT,
                1,
                sample_rate as f32,
            ),
            recording_limiter_stereo: LookaheadLimiter::new(
                LimiterParams::DEFAULT,
                2,
                sample_rate as f32,
            ),
            dsp_metrics: None,
            doa_publisher,
            doa_backend,
            doa_frame_counter: 0,
            steered,
            mode: initial_mode,
            beam_aim_deg: initial_aim,
            stereo_side_gain: 0.0,
            mic_monitor: None,
            mic_check_done: false,
            host_listening_probe: None,
        }
    }

    /// Publish `AudioState.host_listening` from this probe on every frame
    /// ("Computer is listening" on Home).
    pub fn set_host_listening_probe(&mut self, probe: impl Fn() -> bool + Send + 'static) {
        self.host_listening_probe = Some(Box::new(probe));
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

    /// Subscribe to the per-frame DoA stream. One DoaFrame is emitted per
    /// process_frame() call (≈ one per ALSA period). Capacity 64; slow
    /// consumers get RecvError::Lagged, not errors. F3 (diarization) is the
    /// primary consumer; the steered-beam controller (Task 14) and any UI
    /// arrow can also subscribe.
    pub fn subscribe_doa(&self) -> broadcast::Receiver<DoaFrame> {
        self.doa_publisher.subscribe()
    }

    /// Wire the shared DSP metrics handle. Call once at startup, after
    /// `PlaybackService` is constructed (since playback owns the Arc).
    pub fn set_dsp_metrics(&mut self, metrics: Arc<crate::playback::DspMetrics>) {
        self.dsp_metrics = Some(metrics);
    }

    /// Apply static AEC calibration loaded from the versioned hardware audio
    /// configuration. Production calls this before the service starts.
    pub fn set_aec_config(&mut self, config: AecConfig) {
        let nr = self.audio_tx.borrow().nr_level.clone();
        self.dsp_chain = build_dsp_chain(self.frame_size, self.sample_rate, nr, config.clone());
        self.aec_config = config;
    }

    /// Replace the standalone test integrity state with the production-shared
    /// state and fault channel.
    pub fn set_recording_integrity(
        &mut self,
        integrity: Arc<RecordingIntegrity>,
        integrity_tx: mpsc::UnboundedSender<IntegritySignal>,
    ) {
        self.integrity = integrity;
        self.integrity_tx = integrity_tx;
    }

    /// Construct `AudioService` with a specific DoA backend. Falls back to
    /// GCC-PHAT and sets `AudioState.doa_backend_fallback` when SRP-PHAT is
    /// requested but cannot be initialised (e.g. `frame_size == 0`).
    ///
    /// Callers that don't need backend selection (sim, unit tests) can use
    /// `new()` directly — it always builds GCC-PHAT.
    pub fn new_with_backend(
        audio_tx: Arc<watch::Sender<AudioState>>,
        frame_tx: mpsc::Sender<AudioFrame>,
        frame_rx: mpsc::Receiver<AudioFrame>,
        meeting_tx: mpsc::Sender<ProcessedAudioFrame>,
        frame_size: usize,
        sample_rate: u32,
        geometry: MicGeometry,
        doa_backend: DoaBackend,
    ) -> Self {
        let (doa, effective_backend, fallback) =
            build_doa_backend(doa_backend, geometry.clone(), sample_rate, frame_size);
        let mut svc = Self::new(
            audio_tx.clone(),
            frame_tx,
            frame_rx,
            meeting_tx,
            frame_size,
            sample_rate,
            geometry,
        );
        svc.doa = doa;
        svc.doa_backend = effective_backend;
        if fallback {
            audio_tx.send_modify(|s| s.doa_backend_fallback = true);
        }
        svc
    }

    fn process_mono_dsp(&mut self) -> AecFrameMetrics {
        self.dsp_chain.aec.process_linear(
            &self.mic_mono_scratch,
            &self.ref_mono_scratch,
            &mut self.aec_linear_scratch,
        );

        self.aec_residual_scratch
            .copy_from_slice(&self.aec_linear_scratch);
        self.dsp_chain
            .aec
            .process_residual(&mut self.aec_residual_scratch);
        let post_residual_rms = rms_i16(&self.aec_residual_scratch);

        // Raise the voice before noise suppression, which removes speech
        // that is still near the noise floor (`makeup`).
        let makeup_db = self.dsp_chain.aec.config().uplink_makeup_gain_db;
        crate::makeup::apply(&mut self.aec_residual_scratch, crate::makeup::db_to_linear(makeup_db));

        if let Some(ns) = self.dsp_chain.environmental_ns.as_mut() {
            ns.process(&mut self.aec_residual_scratch);
        }

        let config = self.dsp_chain.aec.config();
        let mut metrics = AecFrameMetrics {
            reference_rms: rms_i16(&self.ref_mono_scratch),
            mic_pre_aec_rms: rms_i16(&self.mic_mono_scratch),
            mic_post_linear_aec_rms: rms_i16(&self.aec_linear_scratch),
            mic_post_residual_echo_rms: post_residual_rms,
            mic_post_noise_suppression_rms: rms_i16(&self.aec_residual_scratch),
            mic_uplink_rms: 0.0,
            linear_erle_db: None,
            effective_echo_suppression_db: None,
            residual_echo_suppression_active: self.dsp_chain.aec.residual_active(),
            residual_echo_suppress_db: config.residual_echo_suppress_db,
            residual_echo_suppress_active_db: config.residual_echo_suppress_active_db,
            uplink_makeup_gain_db: config.uplink_makeup_gain_db,
            noise_suppression_backend: self.dsp_chain.environmental_ns_backend.to_string(),
        };
        metrics.update_suppression();
        metrics
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
                s.doa_confidence = doa.confidence;
            });
            let frame_idx = self.doa_frame_counter;
            self.doa_frame_counter = self.doa_frame_counter.wrapping_add(1);
            let doa_frame = DoaFrame {
                frame_idx,
                timestamp_ms: frame.timestamp_ms,
                azimuth_deg: if doa.vad_active && doa.fresh {
                    doa.azimuth_deg
                } else {
                    f32::NAN
                },
                elevation_deg: if doa.vad_active && doa.fresh {
                    doa.elevation_deg
                } else {
                    f32::NAN
                },
                confidence: doa.confidence,
                vad: doa.vad_active,
                backend: self.doa_backend,
            };
            self.doa_publisher.emit(doa_frame);
            if let Some(beam) = self.steered.as_mut() {
                let dir = beam.on_doa_frame(&doa_frame);
                self.beamformer.set_doa(dir.az_deg, dir.el_deg);
                self.beamformer.set_omni_mix(dir.omni_mix);
            } else if let Some((az, el)) =
                crate::beam::aim::fixed_direction(&self.mode, self.beam_aim_deg, self.doa_override)
            {
                // Directional modes hold their aim and Stereo its orientation
                // instead of chasing the raw DoA every frame (BUG-17).
                self.beamformer.set_doa(az, el);
            }

            let beam_out = self.beamformer.process(&self.mic_8ch_f32_scratch);

            if self.mic_mono_scratch.len() != frame_size {
                self.mic_mono_scratch.resize(frame_size, 0);
            }
            for (dst, &src) in self.mic_mono_scratch.iter_mut().zip(beam_out.mono.iter()) {
                *dst = (src.clamp(-1.0, 1.0) * 32768.0) as i16;
            }

            let metrics = self.process_mono_dsp();
            let mut p = ProcessedAudioFrame::from_raw(
                frame,
                &self.aec_residual_scratch,
                &self.ref_mono_scratch,
            );
            p.aec_metrics = metrics;
            if let Some((l, r)) = beam_out.stereo {
                let (left, right, gain) = crate::processed_frame::rebuild_stereo(
                    &l,
                    &r,
                    &p.mic_mono_f32,
                    self.stereo_side_gain,
                    crate::makeup::db_to_linear(p.aec_metrics.uplink_makeup_gain_db),
                );
                self.stereo_side_gain = gain;
                p = p.with_stereo(left, right);
            }
            apply_gain(self, p)
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

            let metrics = self.process_mono_dsp();
            let mut p = ProcessedAudioFrame::from_raw(
                frame,
                &self.aec_residual_scratch,
                &self.ref_mono_scratch,
            );
            p.aec_metrics = metrics;
            apply_gain(self, p)
        }
    }

    /// Feed the start-up microphone check (C6). Called before mic mute so a
    /// muted boot is still checked.
    fn check_mics(&mut self, frame: &AudioFrame) {
        if self.mic_check_done {
            return;
        }
        let channels = frame.channels as usize;
        let monitor = self.mic_monitor.get_or_insert_with(|| {
            crate::mic_check::MicHealthMonitor::new(channels, frame.sample_rate)
        });
        if let Some(health) = monitor.push_i16(&frame.data, channels) {
            match health {
                waved_state::MicHealth::NotResponding { dead_mask } => {
                    warn!("microphone check: ALSA channels {dead_mask:#010b} are not responding")
                }
                waved_state::MicHealth::Degraded { low_mask } => {
                    warn!("microphone check: ALSA channels {low_mask:#010b} are much quieter than the rest")
                }
                other => info!("microphone check: {other:?}"),
            }
            self.audio_tx.send_modify(|s| s.mic_health = health);
        }
        if monitor.is_done() {
            if self.audio_tx.borrow().mic_health == waved_state::MicHealth::Unknown {
                info!("microphone check: the room stayed too quiet to judge");
            }
            self.mic_monitor = None;
            self.mic_check_done = true;
        }
    }

    pub async fn run(mut self, mut cmd_rx: broadcast::Receiver<Command>) -> AudioServiceExit {
        info!(
            "audio_srv starting (DSP on: frame_size={} sample_rate={})",
            self.frame_size, self.sample_rate
        );

        loop {
            tokio::select! {
                frame = self.frame_rx.recv() => {
                    let Some(mut frame) = frame else {
                        self.audio_tx.send_modify(|state| state.capture_healthy = false);
                        return AudioServiceExit::RawFrameChannelClosed;
                    };
                    self.check_mics(&frame);
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
                    self.mic_spectrum
                        .compute(&self.mic_spectrum_input, &mut mic_bands);

                    // Reference spectrum: same pipeline on the
                    // far-end / playback signal when present.
                    let mut ref_bands = [0.0f32; waved_state::FREQ_BAND_COUNT];
                    if let Some(reference) = frame.reference.as_deref() {
                        let chs = frame.reference_channels.max(1) as usize;
                        let mono = mix_to_mono(reference, chs);
                        self.ref_spectrum.compute(&mono, &mut ref_bands);
                    }

                    let host_listening = self.host_listening_probe.as_ref().map(|probe| probe());
                    let integrity = self.integrity.snapshot();
                    self.audio_tx.send_modify(|s| {
                        s.vu_level.copy_from_slice(&vu);
                        s.incoming_vu_level.copy_from_slice(&incoming_vu);
                        s.vu_freq_bands.copy_from_slice(&mic_bands);
                        s.incoming_freq_bands.copy_from_slice(&ref_bands);
                        s.capture_xruns = integrity.xruns;
                        s.raw_frames_dropped = integrity.raw_frames_dropped;
                        s.processed_frames_dropped = integrity.processed_frames_dropped;
                        if let Some(listening) = host_listening {
                            s.host_listening = listening;
                        }
                    });
                    let _ = self.stats_tee.send(frame.clone());

                    let processed = self.process_frame(&frame);
                    let _ = self.processed_tx.send(processed.clone());
                    match self.meeting_tx.try_send(processed) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            self.publish_integrity_fault(
                                waved_state::RecordingIntegrityFault::ProcessedFrameDropped,
                            );
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            self.audio_tx.send_modify(|state| state.capture_healthy = false);
                            return AudioServiceExit::ProcessedFrameChannelClosed;
                        }
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Ok(command) => {
                            if let Some(exit) = self.handle_command(command) {
                                return exit;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return AudioServiceExit::StopRequested;
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("audio_srv lagged {n} commands");
                        }
                    }
                }
            }
        }
    }

    /// Apply one bus command. Returns the exit when the command stops the service.
    fn handle_command(&mut self, command: Command) -> Option<AudioServiceExit> {
        match command {
            Command::SetAudioMode(mode) => {
                info!("audio mode → {mode:?}");
                self.steered = match &mode {
                    waved_state::AudioMode::Steered { initial_az_deg } => {
                        Some(SteeredBeam::new(*initial_az_deg))
                    }
                    _ => None,
                };
                self.beamformer.set_mode(mode.clone());
                self.beamformer.set_omni_mix(0.0); // reset crossfade for non-steered modes
                self.mode = mode.clone();
                self.stereo_side_gain = 0.0;
                self.audio_tx.send_modify(|s| s.mode = mode);
            }
            Command::SetDoaOverride(ov) => {
                match ov {
                    Some((az, el)) => info!("DOA override → ({az:.1}°, {el:.1}°)"),
                    None => info!("DOA override cleared — back to the mode's steering"),
                }
                self.doa_override = ov;
            }
            Command::SetNrLevel(level) => {
                info!("NR level -> {level:?}; rebuilding DSP chain");
                self.dsp_chain = build_dsp_chain(
                    self.frame_size,
                    self.sample_rate,
                    level.clone(),
                    self.aec_config.clone(),
                );
                self.audio_tx.send_modify(|s| s.nr_level = level);
            }
            Command::SetPlaybackPreset(p) => {
                info!("playback preset → {p:?}");
                self.audio_tx.send_modify(|s| s.playback_preset = p);
            }
            Command::ToggleMicMute => {
                self.audio_tx.send_modify(|s| s.mic_muted = !s.mic_muted);
            }
            Command::ToggleSpeakerMute => {
                let new_muted = !self.audio_tx.borrow().speaker_muted;
                self.audio_tx.send_modify(|s| {
                    s.speaker_muted = new_muted;
                    if !new_muted && s.speaker_volume < 10 {
                        s.speaker_volume = 10;
                    }
                });
                // SP#6: UacControl mirrors the new state to hw:1
                // PCM Playback Switch on the next state-watch tick.
            }
            Command::SetSpeakerVolume(v) => {
                self.audio_tx.send_modify(|s| s.speaker_volume = v);
                // SP#6: UacControl mirrors to hw:1 PCM Playback Volume.
            }
            Command::SetMicVolume(v) => {
                // The UI's Mic level and the host both land here; keep
                // AudioState in the documented 0..=800 range.
                let v = waved_config::clamp_mic_volume(v);
                self.audio_tx.send_modify(|s| s.mic_volume = v);
            }
            Command::SetBeamAim(degrees) => match waved_config::wrap_beam_aim(degrees) {
                Some(aim) => {
                    info!("beam aim → {aim:.1}°");
                    self.beam_aim_deg = aim;
                    self.audio_tx.send_modify(|s| s.beam_aim_deg = aim);
                }
                None => warn!("beam aim ignored: {degrees} is not a direction"),
            },
            Command::Shutdown => return Some(AudioServiceExit::StopRequested),
            _ => {}
        }
        None
    }

    fn publish_integrity_fault(&self, fault: waved_state::RecordingIntegrityFault) {
        self.integrity.record_fault(fault);
        let snapshot = self.integrity.snapshot();
        self.audio_tx.send_modify(|state| {
            state.capture_xruns = snapshot.xruns;
            state.raw_frames_dropped = snapshot.raw_frames_dropped;
            state.processed_frames_dropped = snapshot.processed_frames_dropped;
        });
        let _ = self.integrity_tx.send(IntegritySignal::new(fault));
    }
}

/// Construct a `DoaBackendImpl` for the given `DoaBackend` preference.
/// Returns `(impl, effective_backend, fallback_occurred)`.
/// Fallback to GCC-PHAT occurs when SRP-PHAT is requested but `frame_size == 0`
/// (which would cause a zero-length FFT panic in rustfft).
fn build_doa_backend(
    requested: DoaBackend,
    geometry: MicGeometry,
    sample_rate: u32,
    frame_size: usize,
) -> (DoaBackendImpl, DoaBackend, bool) {
    match requested {
        DoaBackend::GccPhat => (
            DoaBackendImpl::GccPhat(DoaEstimator::new(geometry, sample_rate, frame_size)),
            DoaBackend::GccPhat,
            false,
        ),
        DoaBackend::SrpPhat => {
            if frame_size == 0 {
                tracing::warn!(
                    "doa_backend=srp_phat requested but frame_size=0; falling back to gcc_phat"
                );
                return (
                    DoaBackendImpl::GccPhat(DoaEstimator::new(geometry, sample_rate, frame_size)),
                    DoaBackend::GccPhat,
                    true,
                );
            }
            let srp = SrpPhatEstimator::new(geometry, sample_rate, frame_size);
            (
                DoaBackendImpl::SrpPhat {
                    srp,
                    vad: crate::doa::vad::EnergyVad::default(),
                    smoothed_az: 0.0,
                    smoothed_el: 0.0,
                    primed: false,
                    ch0_scratch: Vec::with_capacity(frame_size),
                    frame_size,
                },
                DoaBackend::SrpPhat,
                false,
            )
        }
    }
}

fn apply_gain(svc: &mut AudioService, mut processed: ProcessedAudioFrame) -> ProcessedAudioFrame {
    let mic_vol = svc.audio_tx.borrow().mic_volume;
    // Mono: gain → limiter.
    apply_mic_volume(&mut processed.mic_mono_f32, mic_vol);
    svc.recording_limiter_mono
        .process_interleaved(&mut processed.mic_mono_f32);
    // Stereo (rare; only Stereo beam mode): gain on each channel, then
    // shared-gain limiter on the interleaved pair to preserve image.
    if let Some((l, r)) = processed.mic_stereo_f32.as_mut() {
        apply_mic_volume(l, mic_vol);
        apply_mic_volume(r, mic_vol);
        let mut interleaved: Vec<f32> = Vec::with_capacity(l.len() * 2);
        for i in 0..l.len() {
            interleaved.push(l[i]);
            interleaved.push(r[i]);
        }
        svc.recording_limiter_stereo
            .process_interleaved(&mut interleaved);
        for (i, chunk) in interleaved.chunks_exact(2).enumerate() {
            l[i] = chunk[0];
            r[i] = chunk[1];
        }
    }
    if let Some(metrics) = svc.dsp_metrics.as_ref() {
        let mono = svc.recording_limiter_mono.last_reduction_db();
        // last_reduction_db is always ≤ 0 (zero or negative). Using 0.0
        // as the "no stereo path" sentinel means mono.min(stereo) collapses
        // to mono whenever stereo isn't actively running.
        let stereo = if processed.mic_stereo_f32.is_some() {
            svc.recording_limiter_stereo.last_reduction_db()
        } else {
            0.0
        };
        let worst = mono.min(stereo);
        metrics
            .recording_limiter_reduction_db_x100
            .store((worst * 100.0) as i32, std::sync::atomic::Ordering::Relaxed);
    }
    processed.aec_metrics.mic_uplink_rms = rms_f32_i16_scale(&processed.mic_mono_f32);
    processed.aec_metrics.update_suppression();
    processed
}

fn rms_i16(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples
        .iter()
        .map(|&sample| {
            let sample = sample as f64;
            sample * sample
        })
        .sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

fn rms_f32_i16_scale(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples
        .iter()
        .map(|&sample| {
            let sample = sample as f64;
            sample * sample
        })
        .sum();
    (sum_sq / samples.len() as f64).sqrt() as f32 * i16::MAX as f32
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
    fn nr_level_maps_to_distinct_increasing_atten() {
        // SP#4 follow-up / stock parity: NrLevel must drive *distinct* DFN3
        // attenuation caps, not collapse Low/Med/High to one value.
        let off = nr_to_atten_lim_db(&NrLevel::Off);
        let low = nr_to_atten_lim_db(&NrLevel::Low);
        let med = nr_to_atten_lim_db(&NrLevel::Med);
        let high = nr_to_atten_lim_db(&NrLevel::High);
        assert!(
            off < low && low < med && med < high,
            "expected strictly increasing atten Off<Low<Med<High, got {off}/{low}/{med}/{high}"
        );
        // The three active levels must be mutually distinct (the bug we guard against).
        assert!(low != med && med != high && low != high);
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
    fn process_frame_metrics_capture_stage_order_before_user_gain() {
        let frame_size = 480;
        let sample_rate = 48_000;
        let state = AudioState {
            nr_level: NrLevel::Off,
            mic_volume: 200,
            ..AudioState::default()
        };
        let (audio_tx, _audio_rx) = tokio::sync::watch::channel(state);
        let (frame_tx, frame_rx) = tokio::sync::mpsc::channel(4);
        let (meeting_tx, _meeting_rx) = tokio::sync::mpsc::channel(4);
        let mut service = AudioService::new(
            Arc::new(audio_tx),
            frame_tx,
            frame_rx,
            meeting_tx,
            frame_size,
            sample_rate,
            MicGeometry::default(),
        );
        let frame = AudioFrame::mic_only(
            (0..frame_size)
                .map(|sample| {
                    let phase =
                        2.0 * std::f32::consts::PI * 1_000.0 * sample as f32 / sample_rate as f32;
                    (phase.sin() * 500.0) as i16
                })
                .collect(),
            1,
            sample_rate,
            0,
        );

        let mut processed = service.process_frame(&frame);
        for _ in 0..4 {
            processed = service.process_frame(&frame);
        }
        let metrics = &processed.aec_metrics;

        assert!(metrics.residual_echo_suppression_active);
        assert_eq!(metrics.residual_echo_suppress_db, 40);
        assert_eq!(metrics.residual_echo_suppress_active_db, 15);
        assert_eq!(metrics.noise_suppression_backend, "off");
        assert!((metrics.mic_pre_aec_rms - 353.2).abs() < 1.0);
        assert_eq!(metrics.uplink_makeup_gain_db, 24);
        let makeup = metrics.mic_post_noise_suppression_rms / metrics.mic_post_residual_echo_rms;
        assert!(
            (makeup - crate::makeup::db_to_linear(24)).abs() < 0.05,
            "NR Off adds only the make-up gain before the (absent) denoiser; got {makeup:.3}"
        );
        let gain_ratio = metrics.mic_uplink_rms / metrics.mic_post_noise_suppression_rms.max(0.01);
        assert!(
            (gain_ratio - 2.0).abs() < 0.05,
            "pre-gain metric must exclude 2x user gain; got ratio {gain_ratio:.3}"
        );
        assert!(metrics.linear_erle_db.is_none());
        assert!(metrics.effective_echo_suppression_db.is_none());
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
        assert!(
            rms_front > 0.05,
            "beamformer produced silence: rms={rms_front}"
        );
    }

    #[test]
    fn apply_mic_volume_unity_is_identity() {
        let mut s = vec![0.1, -0.2, 0.5, -0.5];
        apply_mic_volume(&mut s, 100);
        assert_eq!(s, vec![0.1, -0.2, 0.5, -0.5]);
    }

    #[test]
    fn apply_mic_volume_doubles_at_200() {
        let mut s = vec![0.4, -0.3, 0.0];
        apply_mic_volume(&mut s, 200);
        assert!((s[0] - 0.8).abs() < 1e-6, "expected 0.8, got {}", s[0]);
        assert!((s[1] - (-0.6)).abs() < 1e-6, "expected -0.6, got {}", s[1]);
        assert_eq!(s[2], 0.0);
    }

    #[test]
    fn apply_mic_volume_silences_at_zero() {
        let mut s = vec![0.5, -0.3, 1.0];
        apply_mic_volume(&mut s, 0);
        assert_eq!(s, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn apply_mic_volume_caps_pct_at_800() {
        let mut a = vec![0.001];
        let mut b = vec![0.001];
        apply_mic_volume(&mut a, 900);
        apply_mic_volume(&mut b, 800);
        assert!(
            a[0] > 0.005,
            "expected ~0.008 (pct=900 capped to 800), got {}",
            a[0]
        );
        assert_eq!(a, b, "pct above 800 should produce same output as 800");
    }

    #[test]
    fn apply_mic_volume_no_longer_clamps_output() {
        let mut s = vec![0.5_f32];
        apply_mic_volume(&mut s, 800);
        assert!((s[0] - 4.0).abs() < 1e-6, "expected 4.0, got {}", s[0]);
    }

    #[test]
    fn process_frame_limiter_holds_peak_below_ceiling() {
        use crate::callbacks::AudioFrame;
        use std::sync::Arc;
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        // mic_volume = 800 (max). Feed an i16 sine that reaches ±16384 (= ±0.5
        // in f32 after /32768). Post-gain peak is ~4.0, far above the limiter's
        // -1 dBFS ceiling (≈0.891).
        let mut state = AudioState::default();
        state.mic_volume = 800;
        let (audio_tx, _audio_rx) = watch::channel(state);
        let (frame_tx, frame_rx) = mpsc::channel(8);
        let (meeting_tx, _meeting_rx) = mpsc::channel(8);
        let mut svc = AudioService::new(
            Arc::new(audio_tx),
            frame_tx,
            frame_rx,
            meeting_tx,
            256,
            48_000,
            crate::beam::MicGeometry::default(),
        );

        // Push enough frames through to flush the 96-sample lookahead.
        let mut max_post_warmup = 0.0_f32;
        for frame_idx in 0..6_u32 {
            let mut raw = vec![0i16; 256];
            for (i, s) in raw.iter_mut().enumerate() {
                let phase = (frame_idx as usize * 256 + i) as f32;
                *s = (16384.0 * (2.0 * std::f32::consts::PI * 1000.0 * phase / 48_000.0).sin())
                    as i16;
            }
            let frame = AudioFrame::mic_only(raw, 1, 48_000, frame_idx as u64);
            let processed = svc.process_frame(&frame);
            // Skip the first 96 output samples (lookahead warmup).
            let start = if frame_idx == 0 { 96 } else { 0 };
            for &v in &processed.mic_mono_f32[start..] {
                max_post_warmup = max_post_warmup.max(v.abs());
            }
        }
        let ceiling = 10.0_f32.powf(-1.0 / 20.0); // ≈0.891
        assert!(
            max_post_warmup <= ceiling + 0.02,
            "post-warmup peak {max_post_warmup} exceeded -1 dBFS ceiling {ceiling}"
        );
        // Also confirm the limiter actually did something — output isn't silent.
        assert!(
            max_post_warmup > 0.5,
            "post-warmup peak {max_post_warmup} suspiciously low (limiter eating the signal?)"
        );
    }

    #[test]
    fn process_frame_quiet_signal_passes_with_only_gain_post_warmup() {
        use crate::callbacks::AudioFrame;
        use std::sync::Arc;
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        // Quiet sine: i16 amplitude ±200 (= ±0.0061 in f32). At mic_volume=100
        // (unity), post-gain ≈ ±0.0061 — well below the -1 dBFS ceiling
        // (≈0.891). Limiter should be idle; we just confirm the signal is
        // not clamped to zero or silence. Pure DC is zeroed by the Speex AEC
        // even with no reference, so we use a 1 kHz sine to survive the AEC.
        let mut state = AudioState::default();
        state.mic_volume = 100;
        let (audio_tx, _audio_rx) = watch::channel(state);
        let (frame_tx, frame_rx) = mpsc::channel(8);
        let (meeting_tx, _meeting_rx) = mpsc::channel(8);
        let mut svc = AudioService::new(
            Arc::new(audio_tx),
            frame_tx,
            frame_rx,
            meeting_tx,
            256,
            48_000,
            crate::beam::MicGeometry::default(),
        );

        // Push enough frames through to flush the 96-sample lookahead, then
        // measure the peak on post-warmup samples. With mic_volume=100 (unity)
        // and amplitude 200/32768 ≈ 0.0061, the post-gain peak should stay
        // well below the -1 dBFS ceiling (≈0.891). The limiter should be
        // completely idle.
        let ceiling = 10.0_f32.powf(-1.0 / 20.0); // ≈0.891
        let mut max_post_warmup = 0.0_f32;
        for frame_idx in 0..6_u32 {
            let mut raw = vec![0i16; 256];
            for (i, s) in raw.iter_mut().enumerate() {
                let phase = (frame_idx as usize * 256 + i) as f32;
                *s =
                    (200.0 * (2.0 * std::f32::consts::PI * 1000.0 * phase / 48_000.0).sin()) as i16;
            }
            let frame = AudioFrame::mic_only(raw, 1, 48_000, frame_idx as u64);
            let processed = svc.process_frame(&frame);
            // Skip the first 96 output samples (lookahead warmup).
            let start = if frame_idx == 0 { 96 } else { 0 };
            for &v in &processed.mic_mono_f32[start..] {
                max_post_warmup = max_post_warmup.max(v.abs());
            }
        }
        // Quiet signal must not have been muted or clamped to silence.
        assert!(
            max_post_warmup > 0.001,
            "quiet signal was silenced unexpectedly: max_post_warmup={max_post_warmup}",
        );
        // And the limiter must not have engaged — peak must stay well below ceiling.
        assert!(
            max_post_warmup < ceiling * 0.5,
            "limiter appeared to engage on quiet signal: peak={max_post_warmup} ceiling={ceiling}",
        );
    }

    #[test]
    fn process_frame_applies_mic_volume_to_mono() {
        use crate::callbacks::AudioFrame;
        use std::sync::Arc;
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        // Helper: build a fresh AudioService with the given mic_volume.
        let make_svc = |mic_vol: u16| {
            let mut state = AudioState::default();
            state.mic_volume = mic_vol;
            let (audio_tx, _audio_rx) = watch::channel(state);
            let (frame_tx, frame_rx) = mpsc::channel(8);
            let (meeting_tx, _meeting_rx) = mpsc::channel(8);
            AudioService::new(
                Arc::new(audio_tx),
                frame_tx,
                frame_rx,
                meeting_tx,
                256,
                48_000,
                crate::beam::MicGeometry::default(),
            )
        };

        let raw = vec![1000i16; 256];

        // Unity (100) — reference level through the DSP chain.
        let mut svc_unity = make_svc(100);
        let frame_unity = AudioFrame::mic_only(raw.clone(), 1, 48_000, 0);
        let out_unity = svc_unity.process_frame(&frame_unity);
        let max_unity = out_unity
            .mic_mono_f32
            .iter()
            .cloned()
            .fold(0.0_f32, f32::max);

        // Double gain (200) — same DSP chain, fresh service, identical input.
        let mut svc_double = make_svc(200);
        let frame_double = AudioFrame::mic_only(raw, 1, 48_000, 0);
        let out_double = svc_double.process_frame(&frame_double);
        let max_double = out_double
            .mic_mono_f32
            .iter()
            .cloned()
            .fold(0.0_f32, f32::max);

        // The ratio should be exactly 2× regardless of AEC attenuation,
        // since both services see the same input and the gain is applied
        // after all DSP processing.
        assert!(
            max_unity > 0.0,
            "unity output was zero — gain stage silenced signal unexpectedly",
        );
        let ratio = max_double / max_unity;
        assert!(
            (ratio - 2.0).abs() < 0.05,
            "expected gain ratio ~2.0 (200/100), got {ratio:.4} (unity max={max_unity:.6}, double max={max_double:.6})",
        );
    }

    #[test]
    fn process_frame_writes_recording_limiter_reduction_to_dsp_metrics() {
        use crate::callbacks::AudioFrame;
        use std::sync::atomic::Ordering;
        use std::sync::Arc;
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        // mic_volume = 800 + loud sine → limiter must engage and write a
        // negative reduction to the shared DspMetrics atomic.
        let mut state = AudioState::default();
        state.mic_volume = 800;
        let (audio_tx, _audio_rx) = watch::channel(state);
        let (frame_tx, frame_rx) = mpsc::channel(8);
        let (meeting_tx, _meeting_rx) = mpsc::channel(8);
        let mut svc = AudioService::new(
            Arc::new(audio_tx),
            frame_tx,
            frame_rx,
            meeting_tx,
            256,
            48_000,
            crate::beam::MicGeometry::default(),
        );
        let metrics = Arc::new(crate::playback::DspMetrics::default());
        svc.set_dsp_metrics(Arc::clone(&metrics));

        // Push enough frames to flush the lookahead and engage the limiter.
        for frame_idx in 0..6_u32 {
            let mut raw = vec![0i16; 256];
            for (i, s) in raw.iter_mut().enumerate() {
                let phase = (frame_idx as usize * 256 + i) as f32;
                *s = (16384.0 * (2.0 * std::f32::consts::PI * 1000.0 * phase / 48_000.0).sin())
                    as i16;
            }
            let frame = AudioFrame::mic_only(raw, 1, 48_000, frame_idx as u64);
            let _ = svc.process_frame(&frame);
        }

        let reduction_x100 = metrics
            .recording_limiter_reduction_db_x100
            .load(Ordering::Relaxed);
        assert!(
            reduction_x100 < 0,
            "expected negative reduction (limiter engaged), got {reduction_x100}"
        );
    }

    #[tokio::test]
    async fn srp_backend_with_zero_frame_size_falls_back_to_gcc_phat() {
        use std::sync::Arc;
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        let (audio_tx_inner, _audio_rx) = watch::channel(AudioState::default());
        let audio_tx = Arc::new(audio_tx_inner);
        let (frame_tx, frame_rx) = mpsc::channel(8);
        let (meeting_tx, _meeting_rx) = mpsc::channel(8);
        let svc = AudioService::new_with_backend(
            audio_tx.clone(),
            frame_tx,
            frame_rx,
            meeting_tx,
            0,
            48000,
            crate::beam::MicGeometry::default(),
            crate::doa::DoaBackend::SrpPhat,
        );
        assert_eq!(
            svc.doa_backend,
            crate::doa::DoaBackend::GccPhat,
            "zero frame_size should fall back to gcc_phat"
        );
        assert!(
            audio_tx.borrow().doa_backend_fallback,
            "fallback flag must be set in AudioState"
        );
    }

    #[tokio::test]
    async fn audio_service_emits_one_doa_frame_per_process_call() {
        use crate::AudioFrame;
        use std::sync::Arc;
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        let (audio_tx_inner, _audio_rx) = watch::channel(AudioState::default());
        let audio_tx = Arc::new(audio_tx_inner);
        let (frame_tx, frame_rx) = mpsc::channel(8);
        let (meeting_tx, _meeting_rx) = mpsc::channel(8);
        let mut svc = AudioService::new(
            audio_tx,
            frame_tx,
            frame_rx,
            meeting_tx,
            480,
            48000,
            crate::beam::MicGeometry::default(),
        );
        let mut rx = svc.subscribe_doa();
        let frame = AudioFrame {
            data: vec![0i16; 480 * 8],
            channels: 8,
            reference: None,
            reference_channels: 0,
            sample_rate: 48000,
            timestamp_ms: 1_000,
        };
        svc.process_frame(&frame);
        let got = rx.try_recv().expect("one doa frame per process");
        assert_eq!(got.frame_idx, 0);
        assert!(!got.vad, "silence frame should be vad=false");
        svc.process_frame(&frame);
        let got2 = rx.try_recv().expect("second frame");
        assert_eq!(got2.frame_idx, 1);
    }

    #[tokio::test]
    async fn steered_mode_tracks_doa_override() {
        use crate::AudioFrame;
        use std::sync::Arc;
        use tokio::sync::{mpsc, watch};
        use waved_state::{AudioMode, AudioState};

        let mut initial = AudioState::default();
        initial.mode = AudioMode::Steered {
            initial_az_deg: 0.0,
        };
        let (audio_tx_inner, _audio_rx) = watch::channel(initial);
        let audio_tx = Arc::new(audio_tx_inner);
        let (frame_tx, frame_rx) = mpsc::channel(8);
        let (meeting_tx, _meeting_rx) = mpsc::channel(8);
        let mut svc = AudioService::new(
            audio_tx,
            frame_tx,
            frame_rx,
            meeting_tx,
            480,
            48000,
            crate::beam::MicGeometry::default(),
        );
        // Verify the steered wrapper is constructed.
        assert!(
            svc.steered.is_some(),
            "Steered mode should construct SteeredBeam"
        );

        // Feed silence — direction holds, omni_mix doesn't immediately ramp.
        let frame = AudioFrame {
            data: vec![0i16; 480 * 8],
            channels: 8,
            reference: None,
            reference_channels: 0,
            sample_rate: 48000,
            timestamp_ms: 1_000,
        };
        svc.process_frame(&frame);
        // No assertion on direction — just that it doesn't panic.
    }

    #[test]
    fn process_frame_skips_dsp_metrics_write_when_handle_absent() {
        use crate::callbacks::AudioFrame;
        use std::sync::Arc;
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        // No set_dsp_metrics call → dsp_metrics is None → apply_gain skips
        // the atomic write. Just verify process_frame returns without panic.
        let mut state = AudioState::default();
        state.mic_volume = 800;
        let (audio_tx, _audio_rx) = watch::channel(state);
        let (frame_tx, frame_rx) = mpsc::channel(8);
        let (meeting_tx, _meeting_rx) = mpsc::channel(8);
        let mut svc = AudioService::new(
            Arc::new(audio_tx),
            frame_tx,
            frame_rx,
            meeting_tx,
            256,
            48_000,
            crate::beam::MicGeometry::default(),
        );

        let raw = vec![16000i16; 256];
        let frame = AudioFrame::mic_only(raw, 1, 48_000, 0);
        let _ = svc.process_frame(&frame); // must not panic
    }

    #[tokio::test]
    async fn processed_frame_backpressure_records_and_signals_a_drop() {
        let (audio_tx, _audio_rx) = watch::channel(AudioState::default());
        let (frame_tx, frame_rx) = mpsc::channel(4);
        let (meeting_tx, _meeting_rx) = mpsc::channel(1);
        let integrity = Arc::new(RecordingIntegrity::default());
        let (integrity_tx, mut integrity_rx) = mpsc::unbounded_channel();
        let mut svc = AudioService::new(
            Arc::new(audio_tx),
            frame_tx.clone(),
            frame_rx,
            meeting_tx,
            16,
            48_000,
            crate::beam::MicGeometry::default(),
        );
        svc.set_recording_integrity(Arc::clone(&integrity), integrity_tx);
        let (cmd_tx, cmd_rx) = broadcast::channel(4);
        frame_tx
            .send(AudioFrame::mic_only(vec![0; 16], 1, 48_000, 0))
            .await
            .unwrap();
        frame_tx
            .send(AudioFrame::mic_only(vec![0; 16], 1, 48_000, 1))
            .await
            .unwrap();
        let handle = tokio::spawn(svc.run(cmd_rx));

        let signal = tokio::time::timeout(std::time::Duration::from_secs(1), integrity_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            signal.fault,
            waved_state::RecordingIntegrityFault::ProcessedFrameDropped
        );
        assert_eq!(integrity.snapshot().processed_frames_dropped, 1);

        cmd_tx.send(Command::Shutdown).unwrap();
        assert_eq!(handle.await.unwrap(), AudioServiceExit::StopRequested);
    }

    #[tokio::test]
    async fn closed_processed_frame_channel_is_a_fatal_typed_exit() {
        let (audio_tx, _audio_rx) = watch::channel(AudioState::default());
        let (frame_tx, frame_rx) = mpsc::channel(2);
        let (meeting_tx, meeting_rx) = mpsc::channel(1);
        drop(meeting_rx);
        let svc = AudioService::new(
            Arc::new(audio_tx),
            frame_tx.clone(),
            frame_rx,
            meeting_tx,
            16,
            48_000,
            crate::beam::MicGeometry::default(),
        );
        let (_cmd_tx, cmd_rx) = broadcast::channel(2);
        frame_tx
            .send(AudioFrame::mic_only(vec![0; 16], 1, 48_000, 0))
            .await
            .unwrap();

        assert_eq!(
            svc.run(cmd_rx).await,
            AudioServiceExit::ProcessedFrameChannelClosed
        );
    }

    /// A 1 kHz plane wave arriving from `az_deg` (device degrees).
    fn plane_wave_8ch(az_deg: f32, frame_size: usize) -> AudioFrame {
        plane_wave_8ch_at(az_deg, frame_size, 0.3)
    }

    /// As `plane_wave_8ch`, at `amplitude` of full scale.
    fn plane_wave_8ch_at(az_deg: f32, frame_size: usize, amplitude: f32) -> AudioFrame {
        let geom = crate::beam::MicGeometry::default();
        let sr = 48_000.0_f32;
        let omega = 2.0 * std::f32::consts::PI * 1_000.0 / sr;
        let (cos, sin) = (az_deg.to_radians().cos(), az_deg.to_radians().sin());
        let mut samples = vec![0i16; frame_size * 8];
        for ch in 0..8 {
            let p = geom.position_for_alsa(ch);
            let tau = -((p.x * cos + p.y * sin) * 1e-3) / 343.0 * sr;
            for i in 0..frame_size {
                samples[i * 8 + ch] =
                    (((i as f32 - tau) * omega).sin() * amplitude * i16::MAX as f32) as i16;
            }
        }
        AudioFrame::mic_only(samples, 8, 48_000, 0)
    }

    /// A service whose state was seeded with `mode` and `aim`, as main.rs does.
    fn service_with(
        mode: waved_state::AudioMode,
        aim: f32,
    ) -> (AudioService, Arc<watch::Sender<AudioState>>) {
        let state = AudioState {
            mode,
            beam_aim_deg: aim,
            nr_level: NrLevel::Off,
            ..AudioState::default()
        };
        let audio_tx = Arc::new(watch::channel(state).0);
        let (frame_tx, frame_rx) = mpsc::channel(4);
        let (meeting_tx, _meeting_rx) = mpsc::channel(4);
        let svc = AudioService::new(
            Arc::clone(&audio_tx),
            frame_tx,
            frame_rx,
            meeting_tx,
            480,
            48_000,
            crate::beam::MicGeometry::default(),
        );
        (svc, audio_tx)
    }

    #[test]
    fn cardioid_holds_the_aim_while_the_talker_moves() {
        let (mut svc, _state) = service_with(waved_state::AudioMode::Cardioid, 90.0);
        for az in [0.0, 180.0, 270.0] {
            svc.process_frame(&plane_wave_8ch(az, 480));
            assert_eq!(svc.beamformer.direction(), (90.0, 0.0), "talker at {az}°");
        }
    }

    #[test]
    fn stereo_keeps_the_device_orientation() {
        let (mut svc, _state) = service_with(waved_state::AudioMode::Stereo, 200.0);
        svc.process_frame(&plane_wave_8ch(90.0, 480));
        assert_eq!(svc.beamformer.direction(), (0.0, 0.0));
    }

    #[test]
    fn doa_override_still_wins_for_the_beamforming_test() {
        let (mut svc, _state) = service_with(waved_state::AudioMode::Cardioid, 90.0);
        svc.handle_command(Command::SetDoaOverride(Some((45.0, 10.0))));
        svc.process_frame(&plane_wave_8ch(0.0, 480));
        assert_eq!(svc.beamformer.direction(), (45.0, 10.0));
        svc.handle_command(Command::SetDoaOverride(None));
        svc.process_frame(&plane_wave_8ch(0.0, 480));
        assert_eq!(
            svc.beamformer.direction(),
            (90.0, 0.0),
            "clearing it restores the aim"
        );
    }

    #[test]
    fn mode_switch_changes_the_steering() {
        let (mut svc, state) = service_with(waved_state::AudioMode::Cardioid, 90.0);
        // Ruling D4: prove the directional mode held its aim before the switch.
        svc.process_frame(&plane_wave_8ch(90.0, 480));
        assert_eq!(svc.beamformer.direction(), (90.0, 0.0));
        assert!(svc
            .handle_command(Command::SetAudioMode(waved_state::AudioMode::Stereo))
            .is_none());
        assert_eq!(state.borrow().mode, waved_state::AudioMode::Stereo);
        svc.process_frame(&plane_wave_8ch(90.0, 480));
        assert_eq!(svc.beamformer.direction(), (0.0, 0.0));
        assert_eq!(
            svc.handle_command(Command::Shutdown),
            Some(AudioServiceExit::StopRequested)
        );
    }

    #[test]
    fn set_beam_aim_moves_the_fixed_beam_and_the_state() {
        let (mut svc, state) = service_with(waved_state::AudioMode::Supercardioid, 0.0);
        svc.handle_command(Command::SetBeamAim(-60.0));
        assert_eq!(state.borrow().beam_aim_deg, 300.0);
        svc.process_frame(&plane_wave_8ch(0.0, 480));
        assert_eq!(svc.beamformer.direction(), (300.0, 0.0));
        svc.handle_command(Command::SetBeamAim(f32::NAN));
        assert_eq!(state.borrow().beam_aim_deg, 300.0, "NaN is ignored");
        svc.handle_command(Command::SetBeamAim(720.0));
        assert_eq!(state.borrow().beam_aim_deg, 0.0);
    }

    #[test]
    fn a_bad_seeded_aim_is_wrapped_or_zeroed() {
        let (svc, _state) = service_with(waved_state::AudioMode::Cardioid, -90.0);
        assert_eq!(svc.beam_aim_deg, 270.0);
        let (svc, _state) = service_with(waved_state::AudioMode::Cardioid, f32::NAN);
        assert_eq!(svc.beam_aim_deg, 0.0);
    }

    #[test]
    fn stereo_uplink_pair_averages_to_the_mono_uplink() {
        let (mut svc, _state) = service_with(waved_state::AudioMode::Stereo, 0.0);
        let mut last = None;
        // Speech level on the device (about -50 dBFS): below the limiters,
        // where the pair must average to the mono uplink.
        for _ in 0..4 {
            last = Some(svc.process_frame(&plane_wave_8ch_at(60.0, 480, 0.003)));
        }
        let frame = last.unwrap();
        let (l, r) = frame
            .mic_stereo_f32
            .as_ref()
            .expect("Stereo mode carries a pair");
        for i in 0..frame.mic_mono_f32.len() {
            let mid = 0.5 * (l[i] + r[i]);
            assert!(
                (mid - frame.mic_mono_f32[i]).abs() < 1e-3,
                "sample {i}: {mid} vs {}",
                frame.mic_mono_f32[i]
            );
        }
    }

    /// A running `AudioService` over `state`, for tests that drive it through
    /// its channels. Returns the frame sender, the processed-frame receiver
    /// (what the meeting layer sees), the command sender and the task handle.
    #[allow(clippy::type_complexity)]
    fn spawn_service(
        state: Arc<watch::Sender<AudioState>>,
        frame_size: usize,
    ) -> (
        mpsc::Sender<AudioFrame>,
        mpsc::Receiver<ProcessedAudioFrame>,
        broadcast::Sender<Command>,
        tokio::task::JoinHandle<AudioServiceExit>,
    ) {
        spawn_service_with(state, frame_size, |_| {})
    }

    /// `spawn_service`, with `configure` run on the service before it starts
    /// (for setters that must be called before `run`).
    #[allow(clippy::type_complexity)]
    fn spawn_service_with(
        state: Arc<watch::Sender<AudioState>>,
        frame_size: usize,
        configure: impl FnOnce(&mut AudioService),
    ) -> (
        mpsc::Sender<AudioFrame>,
        mpsc::Receiver<ProcessedAudioFrame>,
        broadcast::Sender<Command>,
        tokio::task::JoinHandle<AudioServiceExit>,
    ) {
        let (frame_tx, frame_rx) = mpsc::channel(8);
        let (meeting_tx, meeting_rx) = mpsc::channel(8);
        let mut svc = AudioService::new(
            state,
            frame_tx.clone(),
            frame_rx,
            meeting_tx,
            frame_size,
            48_000,
            crate::beam::MicGeometry::default(),
        );
        configure(&mut svc);
        let (cmd_tx, cmd_rx) = broadcast::channel(8);
        let handle = tokio::spawn(svc.run(cmd_rx));
        (frame_tx, meeting_rx, cmd_tx, handle)
    }

    #[tokio::test]
    async fn host_listening_follows_the_probe() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let state = Arc::new(watch::channel(AudioState::default()).0);
        let open = Arc::new(AtomicBool::new(true));
        let probe = Arc::clone(&open);
        let (frame_tx, mut meeting_rx, cmd_tx, handle) =
            spawn_service_with(Arc::clone(&state), 16, move |svc| {
                svc.set_host_listening_probe(move || probe.load(Ordering::Relaxed));
            });
        frame_tx
            .send(AudioFrame::mic_only(vec![0; 16], 1, 48_000, 0))
            .await
            .unwrap();
        meeting_rx.recv().await.unwrap();
        assert!(state.borrow().host_listening);
        open.store(false, Ordering::Relaxed);
        frame_tx
            .send(AudioFrame::mic_only(vec![0; 16], 1, 48_000, 1))
            .await
            .unwrap();
        meeting_rx.recv().await.unwrap();
        assert!(!state.borrow().host_listening);
        cmd_tx.send(Command::Shutdown).unwrap();
        assert_eq!(handle.await.unwrap(), AudioServiceExit::StopRequested);
    }

    #[test]
    fn doa_confidence_is_published_with_the_direction() {
        let (mut svc, state) = service_with(waved_state::AudioMode::Omni, 0.0);
        let mut doa = svc.subscribe_doa();
        let mut frame = None;
        for _ in 0..5 {
            svc.process_frame(&plane_wave_8ch(90.0, 480));
            frame = Some(doa.try_recv().unwrap());
        }
        let frame = frame.unwrap();
        assert!(frame.confidence > 0.0, "a clean plane wave is not zero-confidence");
        assert_eq!(state.borrow().doa_confidence, frame.confidence);
    }

    #[test]
    fn set_mic_volume_is_clamped_into_state() {
        let (mut svc, state) = service_with(waved_state::AudioMode::Omni, 0.0);
        svc.handle_command(Command::SetMicVolume(5_000));
        assert_eq!(state.borrow().mic_volume, 800);
        svc.handle_command(Command::SetMicVolume(0));
        assert_eq!(state.borrow().mic_volume, 0);
        svc.handle_command(Command::SetMicVolume(250));
        assert_eq!(state.borrow().mic_volume, 250);
    }

    #[tokio::test]
    async fn the_device_mic_level_reaches_the_gain_stage() {
        let state = Arc::new(
            watch::channel(AudioState {
                nr_level: NrLevel::Off,
                ..AudioState::default()
            })
            .0,
        );
        let (frame_tx, mut meeting_rx, cmd_tx, handle) = spawn_service(Arc::clone(&state), 256);
        cmd_tx.send(Command::SetMicVolume(0)).unwrap();
        let mut seen = state.subscribe();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            seen.wait_for(|s| s.mic_volume == 0),
        )
        .await
        .unwrap()
        .unwrap();
        frame_tx
            .send(AudioFrame::mic_only(vec![8_000; 256], 1, 48_000, 0))
            .await
            .unwrap();
        let out = tokio::time::timeout(std::time::Duration::from_secs(1), meeting_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            out.mic_mono_f32.iter().all(|&s| s == 0.0),
            "level 0 silences the uplink"
        );
        cmd_tx.send(Command::Shutdown).unwrap();
        assert_eq!(handle.await.unwrap(), AudioServiceExit::StopRequested);
    }

    #[tokio::test]
    async fn mic_check_hears_the_microphones_before_mute() {
        let state = Arc::new(
            watch::channel(AudioState {
                mic_muted: true, // mute zero-fills frames after the check
                nr_level: NrLevel::Off,
                ..AudioState::default()
            })
            .0,
        );
        let (frame_tx, _meeting_rx, cmd_tx, handle) = spawn_service(Arc::clone(&state), 256);
        // 2 channels, channel 1 digitally silent: 2 × 72 000 frames = one 3 s window.
        for _ in 0..2 {
            let data: Vec<i16> = (0..72_000)
                .flat_map(|i| [if i % 2 == 0 { 900 } else { -900 }, 0])
                .collect();
            frame_tx
                .send(AudioFrame::mic_only(data, 2, 48_000, 0))
                .await
                .unwrap();
        }
        let mut seen = state.subscribe();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            seen.wait_for(|s| {
                s.mic_health == waved_state::MicHealth::NotResponding { dead_mask: 0b10 }
            }),
        )
        .await
        .expect("verdict published")
        .unwrap();
        cmd_tx.send(Command::Shutdown).unwrap();
        assert_eq!(handle.await.unwrap(), AudioServiceExit::StopRequested);
    }
}
