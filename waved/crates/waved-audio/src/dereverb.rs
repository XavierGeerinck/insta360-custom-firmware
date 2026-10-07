//! Late-reverberation suppression for the uplink voice (C17).
//!
//! The 8-mic sum keeps the room's diffuse reverb, which makes the far end
//! hear a "roomy" talker. This is Lebart's statistical model: the late
//! reverb in each STFT bin is the smoothed power from `DELAY_MS` ago,
//! decayed at the room's T60, and a spectral gain removes it. It is cheap
//! (two 1024-point FFTs per 5.3 ms hop) and needs no model.
//!
//! Prototyped offline on a 32 s room recording and picked by ear
//! (2026-10-07, clip 4): T60 0.6 s, 50 ms delay, −15 dB floor. The prototype
//! ran 1536/384 framing; 1024/256 measured within 0.15 dB in every octave
//! band with half the latency, so the time constants below are the
//! prototype's, rescaled to the shorter hop.

use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

const FFT_SIZE: usize = 1024;
const HOP: usize = 256;
/// Room reverberation time the suppressor assumes.
const T60_S: f32 = 0.6;
/// Reverb older than this counts as late.
const DELAY_MS: f32 = 50.0;
/// The gain never drops below this (power, dB).
const FLOOR_DB: f32 = -15.0;
/// Per-hop smoothing of the bin power and of the gain, from the prototype's
/// 0.7 and 0.5 at a 384-sample hop.
const POWER_SMOOTHING_AT_384: f32 = 0.7;
const GAIN_SMOOTHING_AT_384: f32 = 0.5;
/// Sum of the squared periodic Hann window over the overlapping frames at
/// 75 % overlap; the synthesis divides by it.
const WOLA_NORM: f32 = 1.5;

pub struct Dereverb {
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// The last `FFT_SIZE` input samples.
    frame: Vec<f32>,
    /// Input not yet a full hop.
    pending: Vec<f32>,
    overlap: Vec<f32>,
    /// Finished output waiting to be handed out.
    ready: std::collections::VecDeque<f32>,
    spectrum: Vec<Complex32>,
    smoothed_power: Vec<f32>,
    /// `smoothed_power` from the last `delay_hops` hops, oldest at `history_pos`.
    history: Vec<Vec<f32>>,
    history_pos: usize,
    gain: Vec<f32>,
    decay: f32,
    floor: f32,
    power_alpha: f32,
    gain_alpha: f32,
    latency: usize,
}

impl Dereverb {
    /// `frame_size` is the period the caller passes to `process`; when it is
    /// not a whole number of hops, one hop of extra delay keeps the output
    /// from running dry.
    pub fn new(frame_size: usize, sample_rate: u32) -> Self {
        let mut planner = FftPlanner::new();
        let hop_s = HOP as f32 / sample_rate as f32;
        let delay_hops = ((DELAY_MS / 1000.0 / hop_s).round() as usize).max(1);
        let delta = 3.0 * std::f32::consts::LN_10 / T60_S;
        let bins = FFT_SIZE / 2 + 1;
        let prime = if frame_size.is_multiple_of(HOP) { 0 } else { HOP };
        Self {
            forward: planner.plan_fft_forward(FFT_SIZE),
            inverse: planner.plan_fft_inverse(FFT_SIZE),
            window: (0..FFT_SIZE)
                .map(|i| {
                    0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos()
                })
                .collect(),
            frame: vec![0.0; FFT_SIZE],
            pending: Vec::with_capacity(HOP),
            overlap: vec![0.0; FFT_SIZE],
            ready: std::iter::repeat_n(0.0, prime).collect(),
            spectrum: vec![Complex32::new(0.0, 0.0); FFT_SIZE],
            smoothed_power: vec![0.0; bins],
            history: vec![vec![0.0; bins]; delay_hops],
            history_pos: 0,
            gain: vec![1.0; bins],
            decay: (-2.0 * delta * delay_hops as f32 * hop_s).exp(),
            floor: 10f32.powf(FLOOR_DB / 10.0),
            power_alpha: POWER_SMOOTHING_AT_384.powf(HOP as f32 / 384.0),
            gain_alpha: GAIN_SMOOTHING_AT_384.powf(HOP as f32 / 384.0),
            latency: FFT_SIZE - HOP + prime,
        }
    }

    /// How far the output lags the input, in samples.
    pub fn latency_samples(&self) -> usize {
        self.latency
    }

    /// Process in place; the output is the input `latency_samples()` ago
    /// with the late reverb removed.
    pub fn process(&mut self, samples: &mut [f32]) {
        for &x in samples.iter() {
            self.pending.push(x);
            if self.pending.len() == HOP {
                self.frame.copy_within(HOP.., 0);
                self.frame[FFT_SIZE - HOP..].copy_from_slice(&self.pending);
                self.pending.clear();
                self.process_hop();
            }
        }
        for out in samples.iter_mut() {
            *out = self.ready.pop_front().unwrap_or(0.0);
        }
    }

    fn process_hop(&mut self) {
        for ((bin, &x), &w) in self.spectrum.iter_mut().zip(&self.frame).zip(&self.window) {
            *bin = Complex32::new(x * w, 0.0);
        }
        self.forward.process(&mut self.spectrum);

        let late = &mut self.history[self.history_pos];
        for (((bin, smoothed), gain), late) in self
            .spectrum
            .iter()
            .zip(&mut self.smoothed_power)
            .zip(&mut self.gain)
            .zip(late.iter_mut())
        {
            *smoothed = self.power_alpha * *smoothed + (1.0 - self.power_alpha) * bin.norm_sqr();
            let reverb = self.decay * *late;
            let target = (1.0 - reverb / (*smoothed + 1e-20)).max(self.floor).sqrt();
            *gain = self.gain_alpha * *gain + (1.0 - self.gain_alpha) * target;
            *late = *smoothed;
        }
        self.history_pos = (self.history_pos + 1) % self.history.len();

        // Real input: the upper bins mirror the lower ones.
        for k in 0..FFT_SIZE {
            let g = self.gain[k.min(FFT_SIZE - k)];
            self.spectrum[k] *= g / FFT_SIZE as f32;
        }
        self.inverse.process(&mut self.spectrum);

        for ((acc, bin), &w) in self.overlap.iter_mut().zip(&self.spectrum).zip(&self.window) {
            *acc += bin.re * w / WOLA_NORM;
        }
        self.ready.extend(self.overlap[..HOP].iter().copied());
        self.overlap.copy_within(HOP.., 0);
        self.overlap[FFT_SIZE - HOP..].fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(len: usize, seed: u32) -> Vec<f32> {
        let mut seed = seed;
        (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect()
    }

    fn energy(samples: &[f32]) -> f32 {
        samples.iter().map(|s| s * s).sum()
    }

    fn run(dereverb: &mut Dereverb, input: &[f32], frame: usize) -> Vec<f32> {
        let mut out = input.to_vec();
        for chunk in out.chunks_mut(frame) {
            dereverb.process(chunk);
        }
        out
    }

    #[test]
    fn reconstructs_the_input_exactly_before_any_reverb_is_estimated() {
        // Until `DELAY_MS` of history exists the gain is 1, so the STFT must
        // hand back the input, delayed by the reported latency.
        for frame in [1024, 1000] {
            let mut dereverb = Dereverb::new(frame, 48_000);
            let input = noise(4 * frame, 7);
            let out = run(&mut dereverb, &input, frame);
            let lag = dereverb.latency_samples();
            for i in 0..1024 {
                assert!(
                    (out[i + lag] - input[i]).abs() < 1e-4,
                    "frame {frame}: sample {i} came back as {} not {}",
                    out[i + lag],
                    input[i]
                );
            }
        }
    }

    #[test]
    fn production_framing_adds_one_window_less_one_hop() {
        assert_eq!(Dereverb::new(1024, 48_000).latency_samples(), 768);
    }

    #[test]
    fn removes_a_reverberant_tail_but_keeps_the_direct_sound() {
        let sr = 48_000;
        let burst = sr as usize / 5;
        let tail = sr as usize;
        let decay_per_sample = (-3.0 * std::f32::consts::LN_10 / T60_S / sr as f32).exp();
        let source = noise(burst + tail, 11);
        let mut input = source.clone();
        let mut level = 1.0;
        for x in &mut input[burst..] {
            level *= decay_per_sample;
            *x *= level;
        }
        let mut dereverb = Dereverb::new(1024, sr);
        let out = run(&mut dereverb, &input, 1024);
        let lag = dereverb.latency_samples();

        let window = |v: &[f32], from: usize, len: usize| energy(&v[from..from + len]);
        let direct_loss_db = 10.0
            * (window(&input, burst / 2, burst / 2) / window(&out, burst / 2 + lag, burst / 2))
                .log10();
        let late = burst + sr as usize / 10;
        let tail_cut_db = 10.0
            * (window(&input, late, sr as usize / 5) / window(&out, late + lag, sr as usize / 5))
                .log10();
        assert!(direct_loss_db < 2.5, "direct sound lost {direct_loss_db:.1} dB");
        assert!(tail_cut_db > 6.0, "late tail only lost {tail_cut_db:.1} dB");
    }

    #[test]
    fn silence_stays_silent() {
        let mut dereverb = Dereverb::new(1024, 48_000);
        let out = run(&mut dereverb, &vec![0.0; 8192], 1024);
        assert!(out.iter().all(|&s| s == 0.0));
    }
}
