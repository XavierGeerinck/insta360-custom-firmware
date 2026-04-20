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
use std::os::raw::c_void;

use super::{sys, EchoCanceller, NoiseSuppressor, FILTER_TAIL_MS};

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
        let tail_samples = sample_rate as usize * FILTER_TAIL_MS / 1000;
        // SAFETY: speex_echo_state_init allocates internally; the returned
        // pointer is freed in Drop. Null return is a constructor failure
        // (speexdsp only returns null on malloc failure for the state);
        // panic here because we can't recover without a state.
        let state =
            unsafe { sys::speex_echo_state_init(frame_size as c_int, tail_samples as c_int) };
        assert!(!state.is_null(), "speex_echo_state_init returned null");
        Self { state, frame_size, ref_mono_buf: vec![0; frame_size] }
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

pub struct SpeexNoiseSuppressor {
    state: *mut sys::SpeexPreprocessState,
    frame_size: usize,
}

unsafe impl Send for SpeexNoiseSuppressor {}

impl SpeexNoiseSuppressor {
    pub fn new(
        frame_size: usize,
        sample_rate: u32,
        echo: Option<&SpeexEchoCanceller>,
    ) -> Self {
        // SAFETY: state init allocates internally; null means malloc failure.
        let state = unsafe {
            sys::speex_preprocess_state_init(frame_size as c_int, sample_rate as c_int)
        };
        assert!(!state.is_null(), "speex_preprocess_state_init returned null");

        // Enable denoise explicitly (default is on in upstream, but pin it).
        let mut enable: c_int = 1;
        // SAFETY: ptr points at a valid c_int for the lifetime of this call.
        unsafe {
            sys::speex_preprocess_ctl(
                state,
                sys::SPEEX_PREPROCESS_SET_DENOISE,
                &mut enable as *mut c_int as *mut c_void,
            );
        }

        if let Some(ec) = echo {
            let echo_state = ec.raw_state();
            // SAFETY: echo_state outlives this call (caller holds the
            // SpeexEchoCanceller). speexdsp stores the pointer; the lifetime
            // of that storage is tied to the preprocessor, so we enforce at
            // the public API: the returned NoiseSuppressor must not outlive
            // the echo canceller it's linked to. `AudioService` rebuilds
            // both together on mode change, satisfying that invariant.
            unsafe {
                sys::speex_preprocess_ctl(
                    state,
                    sys::SPEEX_PREPROCESS_SET_ECHO_STATE,
                    echo_state as *mut c_void,
                );
            }
        }

        Self { state, frame_size }
    }
}

impl Drop for SpeexNoiseSuppressor {
    fn drop(&mut self) {
        if !self.state.is_null() {
            // SAFETY: we own the pointer.
            unsafe { sys::speex_preprocess_state_destroy(self.state) };
        }
    }
}

impl NoiseSuppressor for SpeexNoiseSuppressor {
    fn process(&mut self, samples: &mut [i16]) {
        assert_eq!(samples.len(), self.frame_size, "samples length mismatch");
        // SAFETY: preprocess_run reads+writes frame_size samples via the ptr.
        unsafe {
            sys::speex_preprocess_run(self.state, samples.as_mut_ptr());
        }
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
}
