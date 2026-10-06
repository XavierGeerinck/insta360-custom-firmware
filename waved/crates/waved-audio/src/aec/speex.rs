//! speexdsp-backed `EchoCanceller` + `NoiseSuppressor` implementations.
//!
//! `SpeexEchoCanceller` wraps `SpeexEchoState` (mdf.c, the adaptive echo
//! canceller). It is constructed with a fixed frame size + sample rate; the
//! filter tail is derived from `FILTER_TAIL_MS`. The reference signal may be
//! either mono (same length as `mic`) or interleaved stereo (twice the length),
//! in which case `process()` downmixes L+R → mono internally. This keeps the
//! stereo-to-mono policy in one place and spares callers from allocating an
//! intermediate buffer per frame.
//!
//! `SpeexNoiseSuppressor` wraps `SpeexPreprocessState`. When associated with
//! an echo canceller (via `SPEEX_PREPROCESS_SET_ECHO_STATE`) the preprocessor
//! also performs residual-echo suppression, cleaning up what the adaptive
//! filter couldn't subtract perfectly.

use std::ffi::c_int;
use std::fmt;
use std::os::raw::c_void;

use crate::audio_config::AecConfig;

use super::{sys, EchoCanceller, NoiseSuppressor, FILTER_TAIL_MS};

const RESIDUAL_FILTER_TAIL_MS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeexInitError {
    InvalidParameters,
    InvalidTuning,
    EchoAllocation,
    PreprocessorAllocation,
    Control(&'static str),
}

impl fmt::Display for SpeexInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidParameters => write!(f, "invalid Speex frame size or sample rate"),
            Self::InvalidTuning => write!(f, "invalid Speex residual echo tuning"),
            Self::EchoAllocation => write!(f, "speex echo-state allocation failed"),
            Self::PreprocessorAllocation => {
                write!(f, "speex preprocessor-state allocation failed")
            }
            Self::Control(name) => write!(f, "speex control {name} failed"),
        }
    }
}

impl std::error::Error for SpeexInitError {}

pub struct SpeexEchoCanceller {
    state: *mut sys::SpeexEchoState,
    frame_size: usize,
    ref_mono_buf: Vec<i16>,
}

// SAFETY: the state pointer is not shared; we own it exclusively and the
// speexdsp state machine has no internal global state.
unsafe impl Send for SpeexEchoCanceller {}

impl SpeexEchoCanceller {
    pub fn new(frame_size: usize, sample_rate: u32) -> Self {
        Self::try_new(frame_size, sample_rate).expect("speex echo canceller construction failed")
    }

    pub fn try_new(frame_size: usize, sample_rate: u32) -> Result<Self, SpeexInitError> {
        Self::try_new_with_tail_ms(frame_size, sample_rate, FILTER_TAIL_MS)
    }

    fn try_new_with_tail_ms(
        frame_size: usize,
        sample_rate: u32,
        tail_ms: usize,
    ) -> Result<Self, SpeexInitError> {
        let tail_samples = sample_rate as usize * tail_ms / 1000;
        // SAFETY: speex_echo_state_init allocates internally; the returned
        // pointer is freed in Drop. Null return means malloc failure.
        let state =
            unsafe { sys::speex_echo_state_init(frame_size as c_int, tail_samples as c_int) };
        if state.is_null() {
            return Err(SpeexInitError::EchoAllocation);
        }
        Ok(Self {
            state,
            frame_size,
            ref_mono_buf: vec![0; frame_size],
        })
    }

    /// Exposed so `SpeexNoiseSuppressor` can link its preprocessor to the same
    /// echo state for residual-echo suppression.
    pub(crate) fn raw_state(&self) -> *mut sys::SpeexEchoState {
        self.state
    }
}

impl Drop for SpeexEchoCanceller {
    fn drop(&mut self) {
        if !self.state.is_null() {
            // SAFETY: we own the pointer and no other code path will touch it
            // after Drop returns.
            unsafe { sys::speex_echo_state_destroy(self.state) };
        }
    }
}

impl EchoCanceller for SpeexEchoCanceller {
    fn process(&mut self, mic: &[i16], reference: &[i16], out: &mut [i16]) {
        assert_eq!(mic.len(), self.frame_size, "mic length mismatch");
        assert_eq!(out.len(), self.frame_size, "out length mismatch");

        let reference_mono: &[i16] = if reference.len() == self.frame_size {
            reference
        } else if reference.len() == self.frame_size * 2 {
            for i in 0..self.frame_size {
                let l = reference[2 * i] as i32;
                let r = reference[2 * i + 1] as i32;
                self.ref_mono_buf[i] = ((l + r) / 2) as i16;
            }
            &self.ref_mono_buf
        } else {
            panic!(
                "reference length {} must be {} (mono) or {} (stereo)",
                reference.len(),
                self.frame_size,
                self.frame_size * 2
            );
        };

        // SAFETY: all three pointers point to buffers of frame_size i16 samples;
        // speex_echo_cancellation reads rec+play and writes out exactly that many.
        unsafe {
            sys::speex_echo_cancellation(
                self.state,
                mic.as_ptr(),
                reference_mono.as_ptr(),
                out.as_mut_ptr(),
            );
        }
    }

    fn frame_size(&self) -> usize {
        self.frame_size
    }
}

struct SpeexPreprocessor {
    state: *mut sys::SpeexPreprocessState,
    frame_size: usize,
}

unsafe impl Send for SpeexPreprocessor {}

impl SpeexPreprocessor {
    fn try_new(
        frame_size: usize,
        sample_rate: u32,
        echo: Option<&SpeexEchoCanceller>,
        denoise_enabled: bool,
        aec_config: &AecConfig,
    ) -> Result<Self, SpeexInitError> {
        // SAFETY: state init allocates internally; null means malloc failure.
        let state =
            unsafe { sys::speex_preprocess_state_init(frame_size as c_int, sample_rate as c_int) };
        if state.is_null() {
            return Err(SpeexInitError::PreprocessorAllocation);
        }

        let mut this = Self { state, frame_size };
        // Speex's DENOISE flag gates *all* spectral gains, including linked
        // residual echo suppression. Keep it enabled and make residual-only
        // mode neutral to environmental noise by setting noise attenuation
        // to 0 dB.
        this.set_ctl(sys::SPEEX_PREPROCESS_SET_DENOISE, 1, "SET_DENOISE")?;
        this.set_ctl(
            sys::SPEEX_PREPROCESS_SET_NOISE_SUPPRESS,
            if denoise_enabled { -15 } else { 0 },
            "SET_NOISE_SUPPRESS",
        )?;

        if let Some(ec) = echo {
            let echo_state = ec.raw_state();
            // SAFETY: the composite `SpeexAecStage` declares the
            // preprocessor before the echo state, so it is destroyed first.
            // Legacy `SpeexNoiseSuppressor` callers retain the existing
            // requirement that the echo canceller outlive the suppressor.
            let rc = unsafe {
                sys::speex_preprocess_ctl(
                    this.state,
                    sys::SPEEX_PREPROCESS_SET_ECHO_STATE,
                    echo_state as *mut c_void,
                )
            };
            if rc != 0 {
                return Err(SpeexInitError::Control("SET_ECHO_STATE"));
            }
        }

        this.set_ctl(
            sys::SPEEX_PREPROCESS_SET_ECHO_SUPPRESS,
            -(aec_config.residual_echo_suppress_db as c_int),
            "SET_ECHO_SUPPRESS",
        )?;
        this.set_ctl(
            sys::SPEEX_PREPROCESS_SET_ECHO_SUPPRESS_ACTIVE,
            -(aec_config.residual_echo_suppress_active_db as c_int),
            "SET_ECHO_SUPPRESS_ACTIVE",
        )?;

        // Prime Speex's overlap/add analysis state before any live microphone
        // frame arrives. Without this constructor-time silent frame the first
        // processed analysis frame is zeroed. The native preprocessor still
        // contributes the one-analysis-frame shift asserted in the tests.
        let mut silence = vec![0i16; frame_size];
        this.process(&mut silence);

        Ok(this)
    }

    fn set_denoise(&mut self, enabled: bool) -> Result<(), SpeexInitError> {
        self.set_ctl(
            sys::SPEEX_PREPROCESS_SET_NOISE_SUPPRESS,
            if enabled { -15 } else { 0 },
            "SET_NOISE_SUPPRESS",
        )
    }

    fn set_echo_suppression(&mut self, config: Option<&AecConfig>) -> Result<(), SpeexInitError> {
        let (far_end_db, active_db) = config
            .map(|config| {
                (
                    -(config.residual_echo_suppress_db as c_int),
                    -(config.residual_echo_suppress_active_db as c_int),
                )
            })
            .unwrap_or((0, 0));
        self.set_ctl(
            sys::SPEEX_PREPROCESS_SET_ECHO_SUPPRESS,
            far_end_db,
            "SET_ECHO_SUPPRESS",
        )?;
        self.set_ctl(
            sys::SPEEX_PREPROCESS_SET_ECHO_SUPPRESS_ACTIVE,
            active_db,
            "SET_ECHO_SUPPRESS_ACTIVE",
        )
    }

    fn set_ctl(
        &mut self,
        request: c_int,
        mut value: c_int,
        name: &'static str,
    ) -> Result<(), SpeexInitError> {
        // SAFETY: `state` is owned by self and value is a live c_int for the
        // duration of the synchronous control call.
        let rc = unsafe {
            sys::speex_preprocess_ctl(self.state, request, &mut value as *mut c_int as *mut c_void)
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(SpeexInitError::Control(name))
        }
    }

    fn process(&mut self, samples: &mut [i16]) {
        assert_eq!(samples.len(), self.frame_size, "samples length mismatch");
        // SAFETY: preprocess_run reads+writes frame_size samples via the ptr.
        unsafe {
            sys::speex_preprocess_run(self.state, samples.as_mut_ptr());
        }
    }
}

impl Drop for SpeexPreprocessor {
    fn drop(&mut self) {
        if !self.state.is_null() {
            // SAFETY: we own the pointer.
            unsafe { sys::speex_preprocess_state_destroy(self.state) };
        }
    }
}

/// Coupled linear AEC and residual-echo suppressor.
///
/// Field order is deliberate: Rust drops fields in declaration order, so the
/// preprocessor that holds a borrowed residual echo-state pointer is destroyed
/// before that echo canceller. The independent full-frame linear echo
/// canceller is dropped last.
pub struct SpeexAecStage {
    preprocessor: SpeexPreprocessor,
    residual_echo: SpeexEchoCanceller,
    linear_echo: SpeexEchoCanceller,
    config: AecConfig,
    denoise_enabled: bool,
    frame_size: usize,
    subframe_size: usize,
    shadow_out_scratch: Vec<i16>,
    reference_active: bool,
    echo_suppression_enabled: bool,
}

unsafe impl Send for SpeexAecStage {}

impl SpeexAecStage {
    pub fn try_new(
        frame_size: usize,
        sample_rate: u32,
        config: AecConfig,
    ) -> Result<Self, SpeexInitError> {
        if frame_size == 0 || sample_rate == 0 {
            return Err(SpeexInitError::InvalidParameters);
        }
        if config.residual_echo_suppress_db > 60
            || config.residual_echo_suppress_active_db > 60
            || config.residual_echo_suppress_active_db > config.residual_echo_suppress_db
        {
            return Err(SpeexInitError::InvalidTuning);
        }
        // Preserve the proven caller-sized linear canceller. A small shadow
        // estimator supplies the linked residual preprocessor with <=5 ms
        // analysis frames. Its shorter tail covers the local speaker path
        // without duplicating the primary 200 ms adaptive filter cost.
        let max_subframe = ((sample_rate as usize) / 200).max(1).min(frame_size);
        let subframe_size = (1..=max_subframe)
            .rev()
            .find(|candidate| frame_size.is_multiple_of(*candidate))
            .expect("1 divides every non-zero frame size");
        let linear_echo = SpeexEchoCanceller::try_new(frame_size, sample_rate)?;
        let residual_echo = SpeexEchoCanceller::try_new_with_tail_ms(
            subframe_size,
            sample_rate,
            RESIDUAL_FILTER_TAIL_MS,
        )?;
        let preprocessor = SpeexPreprocessor::try_new(
            subframe_size,
            sample_rate,
            Some(&residual_echo),
            false,
            &config,
        )?;
        Ok(Self {
            preprocessor,
            residual_echo,
            linear_echo,
            config,
            denoise_enabled: false,
            frame_size,
            subframe_size,
            shadow_out_scratch: vec![0; subframe_size],
            reference_active: false,
            echo_suppression_enabled: true,
        })
    }

    pub fn process_linear(&mut self, mic: &[i16], reference: &[i16], out: &mut [i16]) {
        assert_eq!(mic.len(), self.frame_size, "mic length mismatch");
        assert_eq!(out.len(), self.frame_size, "out length mismatch");
        let reference_power: f64 = reference
            .iter()
            .map(|&sample| {
                let sample = sample as f64;
                sample * sample
            })
            .sum();
        self.reference_active = reference_power / reference.len().max(1) as f64 >= 100.0;

        self.linear_echo.process(mic, reference, out);

        if reference.len() == self.frame_size {
            for index in 0..self.frame_size / self.subframe_size {
                let start = index * self.subframe_size;
                let end = start + self.subframe_size;
                self.residual_echo.process(
                    &mic[start..end],
                    &reference[start..end],
                    &mut self.shadow_out_scratch,
                );
            }
        } else if reference.len() == self.frame_size * 2 {
            for index in 0..self.frame_size / self.subframe_size {
                let mono_start = index * self.subframe_size;
                let mono_end = mono_start + self.subframe_size;
                let stereo_start = mono_start * 2;
                let stereo_end = stereo_start + self.subframe_size * 2;
                self.residual_echo.process(
                    &mic[mono_start..mono_end],
                    &reference[stereo_start..stereo_end],
                    &mut self.shadow_out_scratch,
                );
            }
        } else {
            panic!(
                "reference length {} must be {} (mono) or {} (stereo)",
                reference.len(),
                self.frame_size,
                self.frame_size * 2
            );
        }
    }

    pub fn process_residual(&mut self, samples: &mut [i16]) {
        assert_eq!(samples.len(), self.frame_size, "samples length mismatch");
        if self.reference_active != self.echo_suppression_enabled {
            self.preprocessor
                .set_echo_suppression(self.reference_active.then_some(&self.config))
                .expect("validated Speex echo suppression control failed at runtime");
            self.echo_suppression_enabled = self.reference_active;
        }
        for subframe in samples.chunks_exact_mut(self.subframe_size) {
            self.preprocessor.process(subframe);
        }
    }

    pub fn set_denoise(&mut self, enabled: bool) -> Result<(), SpeexInitError> {
        self.preprocessor.set_denoise(enabled)?;
        self.denoise_enabled = enabled;
        Ok(())
    }

    pub fn denoise_enabled(&self) -> bool {
        self.denoise_enabled
    }

    pub fn config(&self) -> &AecConfig {
        &self.config
    }

    pub fn linear_frame_size(&self) -> usize {
        self.linear_echo.frame_size()
    }

    pub fn analysis_frame_size(&self) -> usize {
        self.preprocessor.frame_size
    }
}

pub struct SpeexNoiseSuppressor {
    preprocessor: SpeexPreprocessor,
}

unsafe impl Send for SpeexNoiseSuppressor {}

impl SpeexNoiseSuppressor {
    pub fn new(frame_size: usize, sample_rate: u32, echo: Option<&SpeexEchoCanceller>) -> Self {
        let preprocessor =
            SpeexPreprocessor::try_new(frame_size, sample_rate, echo, true, &AecConfig::default())
                .expect("speex noise suppressor construction failed");
        Self { preprocessor }
    }
}

impl NoiseSuppressor for SpeexNoiseSuppressor {
    fn process(&mut self, samples: &mut [i16]) {
        self.preprocessor.process(samples);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms_f64(samples: &[i16]) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
        (sum_sq / samples.len() as f64).sqrt()
    }

    fn sine(freq: f32, sample_rate: u32, n: usize, amp: f32) -> Vec<i16> {
        let step = 2.0 * std::f32::consts::PI * freq / sample_rate as f32;
        (0..n)
            .map(|i| {
                let s = (i as f32 * step).sin() * amp * i16::MAX as f32;
                s.clamp(i16::MIN as f32, i16::MAX as f32) as i16
            })
            .collect()
    }

    /// Simulate an acoustic echo path: a delayed, attenuated copy of the
    /// reference mixes into the mic. No near-end signal — this is pure echo.
    fn simulate_echo(reference: &[i16], gain: f32, delay_samples: usize) -> Vec<i16> {
        let mut mic = vec![0i16; reference.len()];
        for i in delay_samples..reference.len() {
            let echo = reference[i - delay_samples] as f32 * gain;
            mic[i] = echo.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
        mic
    }

    #[test]
    fn cancels_synthetic_echo_above_20db() {
        let sr: u32 = 48000;
        let frame: usize = 480; // 10 ms

        // 2 s of 1 kHz tone at 0.5 amplitude.
        let reference = sine(1000.0, sr, 2 * sr as usize, 0.5);
        // Echo: 30 % gain, 20-sample delay (~0.4 ms, well under tail length).
        let mic = simulate_echo(&reference, 0.3, 20);

        let mut canceller = SpeexEchoCanceller::new(frame, sr);
        let mut out = vec![0i16; mic.len()];

        let n_frames = mic.len() / frame;
        for i in 0..n_frames {
            let s = i * frame;
            let e = s + frame;
            canceller.process(&mic[s..e], &reference[s..e], &mut out[s..e]);
        }

        // Let the adaptive filter converge: skip the first ~1 s of output.
        let converged_start = sr as usize;
        let input_rms = rms_f64(&mic[converged_start..]);
        let out_rms = rms_f64(&out[converged_start..]).max(1.0);
        let erle_db = 20.0 * (input_rms / out_rms).log10();

        assert!(
            erle_db > 20.0,
            "expected ERLE > 20 dB after convergence, got {erle_db:.1} dB (in {input_rms:.1} out {out_rms:.1})"
        );
    }

    #[test]
    fn noise_suppressor_attenuates_noise_above_8db() {
        let sr: u32 = 48000;
        let frame: usize = 480;

        // 2 s of low-amplitude white noise, deterministic seed.
        let mut rng_state: u32 = 0xC0FFEE;
        let mut noise: Vec<i16> = (0..2 * sr as usize)
            .map(|_| {
                rng_state = rng_state.wrapping_mul(1664525).wrapping_add(1013904223);
                let v = ((rng_state >> 16) as i32 - 32768) / 8; // ~±4096 peak
                v as i16
            })
            .collect();

        let input_rms = rms_f64(&noise);
        let mut ns = SpeexNoiseSuppressor::new(frame, sr, None);

        let n_frames = noise.len() / frame;
        for i in 0..n_frames {
            let s = i * frame;
            let e = s + frame;
            ns.process(&mut noise[s..e]);
        }

        // Skip the first 500 ms — the noise estimator converges gradually.
        let converged_start = sr as usize / 2;
        let output_rms = rms_f64(&noise[converged_start..]).max(1.0);
        let atten_db = 20.0 * (input_rms / output_rms).log10();

        assert!(
            atten_db >= 8.0,
            "expected noise attenuation >= 8 dB, got {atten_db:.1} dB (in {input_rms:.1} out {output_rms:.1})"
        );
    }

    #[test]
    fn noise_suppressor_links_to_echo_state_without_panic() {
        let sr: u32 = 48000;
        let frame: usize = 480;
        let ec = SpeexEchoCanceller::new(frame, sr);
        let mut ns = SpeexNoiseSuppressor::new(frame, sr, Some(&ec));
        let mut samples = sine(1000.0, sr, frame, 0.3);
        ns.process(&mut samples);
        // Smoke test: didn't panic, didn't produce garbage length.
        assert_eq!(samples.len(), frame);
    }

    #[test]
    fn accepts_stereo_reference_and_downmixes() {
        let sr: u32 = 48000;
        let frame: usize = 480;

        let mono = sine(1000.0, sr, frame, 0.5);
        // Interleaved stereo with identical L+R → mono downmix equals mono
        // (within rounding).
        let mut stereo = vec![0i16; frame * 2];
        for (i, &s) in mono.iter().enumerate() {
            stereo[2 * i] = s;
            stereo[2 * i + 1] = s;
        }

        let mut canceller = SpeexEchoCanceller::new(frame, sr);
        let mic = simulate_echo(&mono, 0.3, 20);
        let mut out = vec![0i16; frame];
        // Just asserting the call doesn't panic with a stereo reference.
        canceller.process(&mic, &stereo, &mut out);
    }

    #[test]
    fn residual_stage_is_transparent_with_silent_reference() {
        let sr = 48_000;
        let frame = 480;
        let near = sine(440.0, sr, sr as usize, 0.25);
        let silent = vec![0i16; near.len()];
        let mut stage = SpeexAecStage::try_new(frame, sr, AecConfig::default()).unwrap();
        let mut linear = vec![0i16; near.len()];
        let mut out = vec![0i16; near.len()];

        for i in 0..near.len() / frame {
            let range = i * frame..(i + 1) * frame;
            stage.process_linear(
                &near[range.clone()],
                &silent[range.clone()],
                &mut linear[range.clone()],
            );
            out[range.clone()].copy_from_slice(&linear[range.clone()]);
            stage.process_residual(&mut out[range]);
        }

        let start = sr as usize / 2;
        let loss_db = 20.0 * (rms_f64(&linear[start..]) / rms_f64(&out[start..]).max(1.0)).log10();
        assert!(
            loss_db <= 0.5,
            "silent-reference near-end loss must stay <= 0.5 dB, got {loss_db:.2} dB"
        );
    }

    #[test]
    fn residual_stage_delay_is_one_bounded_analysis_frame() {
        let sr = 48_000;
        let frame = 1024;
        let frames = 12;
        let mut seed = 0x51a7_e123_u32;
        let near: Vec<i16> = (0..frames * frame)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (((seed >> 16) as i32 - 32_768) / 4) as i16
            })
            .collect();
        let silent = vec![0i16; near.len()];
        let mut stage = SpeexAecStage::try_new(frame, sr, AecConfig::default()).unwrap();
        let mut linear = vec![0i16; near.len()];
        let mut out = vec![0i16; near.len()];

        for index in 0..frames {
            let range = index * frame..(index + 1) * frame;
            stage.process_linear(
                &near[range.clone()],
                &silent[range.clone()],
                &mut linear[range.clone()],
            );
            out[range.clone()].copy_from_slice(&linear[range.clone()]);
            stage.process_residual(&mut out[range]);
        }

        // Speex preprocessing shifts by one analysis frame. The production
        // stage must correlate at 128 samples, not at the complete
        // 1024-sample capture period.
        let start = 4 * frame;
        let input = &linear[start..];
        let output = &out[start..];
        let correlation = |lag: usize| -> f64 {
            let (a, b) = if lag == 0 {
                (input, output)
            } else {
                (&input[..input.len() - lag], &output[lag..])
            };
            let dot: f64 = a.iter().zip(b).map(|(&x, &y)| x as f64 * y as f64).sum();
            let a_energy: f64 = a.iter().map(|&x| (x as f64).powi(2)).sum();
            let b_energy: f64 = b.iter().map(|&x| (x as f64).powi(2)).sum();
            dot / (a_energy * b_energy).sqrt().max(f64::EPSILON)
        };
        let zero_lag = correlation(0);
        let analysis_lag = correlation(stage.analysis_frame_size());
        let one_frame_lag = correlation(frame).abs();
        assert!(
            analysis_lag > 0.95
                && analysis_lag > zero_lag.abs()
                && analysis_lag > one_frame_lag,
            "unexpected residual delay: zero={zero_lag:.3}, analysis={analysis_lag:.3}, capture={one_frame_lag:.3}"
        );
    }

    #[test]
    fn residual_stage_reduces_converged_echo() {
        let sr = 48_000;
        let frame = 480;
        // Deterministic broadband, speech-like far-end excitation. A single
        // stationary tone lets the linear FIR converge to numerical silence
        // and cannot exercise a residual suppressor meaningfully.
        let mut seed = 0x5eed_u32;
        let mut filtered = 0.0f32;
        let reference: Vec<i16> = (0..3 * sr as usize)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let white = ((seed >> 16) as i32 - 32_768) as f32 / 32_768.0;
                filtered = 0.88 * filtered + 0.12 * white;
                (filtered * 0.8 * 32768.0).clamp(i16::MIN as f32, i16::MAX as f32) as i16
            })
            .collect();
        // A real speaker/microphone path is not perfectly linear. Preserve a
        // correlated cubic component that the adaptive FIR cannot subtract;
        // this is precisely the residual handled by the linked preprocessor.
        let initial_delay = 20;
        let changed_delay = 120;
        let path_change = 2 * sr as usize;
        let mut mic = vec![0i16; reference.len()];
        for i in initial_delay..reference.len() {
            let delay = if i < path_change {
                initial_delay
            } else {
                changed_delay
            };
            if i < delay {
                continue;
            }
            let x = reference[i - delay] as f32 / 32768.0;
            let echo = (0.45 * x + 0.25 * x * x * x) * 32768.0;
            mic[i] = echo.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
        let mut stage = SpeexAecStage::try_new(frame, sr, AecConfig::default()).unwrap();
        let mut linear = vec![0i16; mic.len()];
        let mut residual = vec![0i16; mic.len()];

        for i in 0..mic.len() / frame {
            let range = i * frame..(i + 1) * frame;
            stage.process_linear(
                &mic[range.clone()],
                &reference[range.clone()],
                &mut linear[range.clone()],
            );
            residual[range.clone()].copy_from_slice(&linear[range.clone()]);
            stage.process_residual(&mut residual[range]);
        }

        let start = path_change;
        let end = start + sr as usize / 5;
        let linear_rms = rms_f64(&linear[start..end]);
        let residual_rms = rms_f64(&residual[start..end]);
        let reduction_db = 20.0 * (linear_rms / residual_rms.max(0.01)).log10();
        assert!(
            reduction_db >= 0.1,
            "residual stage should reduce a changed nonlinear echo path, got {reduction_db:.1} dB (linear {linear_rms:.3}, residual {residual_rms:.3})"
        );
    }

    #[test]
    fn residual_stage_preserves_double_talk_near_end() {
        let sr = 48_000;
        let frame = 480;
        let reference = sine(997.0, sr, 3 * sr as usize, 0.35);
        let echo = simulate_echo(&reference, 0.25, 20);
        let near = sine(431.0, sr, reference.len(), 0.25);
        let mic: Vec<i16> = echo
            .iter()
            .zip(&near)
            .map(|(&e, &n)| (e as i32 + n as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16)
            .collect();
        let mut stage = SpeexAecStage::try_new(frame, sr, AecConfig::default()).unwrap();
        let mut linear = vec![0i16; mic.len()];
        let mut residual = vec![0i16; mic.len()];

        for i in 0..mic.len() / frame {
            let range = i * frame..(i + 1) * frame;
            stage.process_linear(
                &mic[range.clone()],
                &reference[range.clone()],
                &mut linear[range.clone()],
            );
            residual[range.clone()].copy_from_slice(&linear[range.clone()]);
            stage.process_residual(&mut residual[range]);
        }

        let start = 2 * sr as usize;
        let loss_db =
            20.0 * (rms_f64(&linear[start..]) / rms_f64(&residual[start..]).max(1.0)).log10();
        assert!(
            loss_db <= 3.0,
            "double-talk near-end loss must stay <= 3 dB, got {loss_db:.2} dB"
        );
    }

    #[test]
    fn residual_stage_can_enable_environmental_denoise_without_rebuild() {
        let mut stage = SpeexAecStage::try_new(480, 48_000, AecConfig::default()).unwrap();
        assert!(!stage.denoise_enabled());
        stage.set_denoise(true).unwrap();
        assert!(stage.denoise_enabled());
    }

    #[test]
    fn production_frame_preserves_linear_aec_and_caps_residual_analysis() {
        let stage = SpeexAecStage::try_new(1024, 48_000, AecConfig::default()).unwrap();
        assert_eq!(stage.linear_frame_size(), 1024);
        assert_eq!(stage.analysis_frame_size(), 128);
        assert!(stage.analysis_frame_size() as f32 / 48_000.0 <= 0.005);
    }

    #[test]
    fn residual_stage_rejects_invalid_direct_tuning() {
        let invalid = AecConfig {
            residual_echo_suppress_db: 15,
            residual_echo_suppress_active_db: 40,
            ..AecConfig::default()
        };
        assert!(matches!(
            SpeexAecStage::try_new(480, 48_000, invalid),
            Err(SpeexInitError::InvalidTuning)
        ));
    }
}
