//! `NoiseSuppressor` impl backed by DeepFilterNet 3 (CPU, tract-NEON).
//!
//! ## Hop-size buffering
//!
//! DFN3's hop is fixed at 480 samples (10 ms @ 48 kHz). Our ALSA capture
//! period is 1024 samples (~21 ms), so we can't feed the model directly.
//! We accumulate input in `in_acc`, drain whole hops into DFN3, and queue
//! the enhanced output in `out_acc`. Each `process` call copies up to
//! `samples.len()` enhanced samples back to the caller; if the queue is
//! still warming up (first call or two after construction) the gap is
//! zero-padded, producing a brief audio mute at session start.
//!
//! ## Latency
//!
//! Steady-state added latency ≈ one ALSA period (≈ 21 ms) on top of
//! DFN3's intrinsic ≈ 20 ms algorithmic delay → ≈ 40 ms total. Acceptable
//! for conferencing (Teams/Zoom tolerate 150 ms easily).
//!
//! ## Failure handling
//!
//! Per-frame inference errors fall through to pass-through (uses the raw
//! `samples` as the output for that hop). Init failure is the caller's
//! problem — `init_processing` catches it and falls back to
//! `SpeexNoiseSuppressor`.

use std::collections::VecDeque;

use super::NoiseSuppressor;
use waved_npu::{Dfn3CpuInfer, NpuInfer};

pub struct DeepFilterNs {
    infer: Dfn3CpuInfer,
    hop_size: usize,
    in_acc: Vec<f32>,
    out_acc: VecDeque<i16>,
}

impl DeepFilterNs {
    /// Construct with the bundled DFN3 low-latency model. 48 kHz only.
    pub fn new() -> anyhow::Result<Self> {
        let infer = Dfn3CpuInfer::new()?;
        let hop_size = infer.hop_size();
        Ok(Self {
            infer,
            hop_size,
            in_acc: Vec::with_capacity(hop_size * 4),
            out_acc: VecDeque::with_capacity(hop_size * 4),
        })
    }

    /// DFN3 hop, exposed for tests + diagnostics. The caller's frame size
    /// is independent of this — buffering inside `process` bridges the
    /// difference.
    pub fn hop_size(&self) -> usize {
        self.hop_size
    }
}

impl NoiseSuppressor for DeepFilterNs {
    fn process(&mut self, samples: &mut [i16]) {
        // 1. Append i16→f32 of new input to in_acc.
        self.in_acc.reserve(samples.len());
        for &s in samples.iter() {
            self.in_acc.push((s as f32) / 32768.0);
        }

        // 2. Drain whole hops through DFN3 into out_acc.
        while self.in_acc.len() >= self.hop_size {
            let hop: Vec<f32> = self.in_acc.drain(..self.hop_size).collect();
            match self.infer.run(&hop) {
                Ok(enh) => {
                    for &v in enh.iter() {
                        let q = (v * 32768.0).clamp(i16::MIN as f32, i16::MAX as f32);
                        self.out_acc.push_back(q as i16);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "DFN3 inference failed; passing hop through");
                    // Pass-through: re-encode the raw f32 hop back to i16.
                    for &v in hop.iter() {
                        let q = (v * 32768.0).clamp(i16::MIN as f32, i16::MAX as f32);
                        self.out_acc.push_back(q as i16);
                    }
                }
            }
        }

        // 3. Drain up to samples.len() enhanced samples back to caller.
        //    During warm-up the queue may be short; zero-pad the gap so
        //    output is always exactly samples.len().
        for slot in samples.iter_mut() {
            *slot = self.out_acc.pop_front().unwrap_or(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(samples: &[i16]) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        let s: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>()
            / samples.len() as f64;
        s.sqrt()
    }

    #[test]
    fn hop_size_is_dfn3_native() {
        let ns = DeepFilterNs::new().expect("model loads");
        assert_eq!(ns.hop_size(), 480, "DFN3 hop is 10 ms @ 48 kHz");
    }

    #[test]
    fn process_works_at_alsa_period_size_1024() {
        let mut ns = DeepFilterNs::new().expect("model loads");
        // Mimic the on-device frame size (ALSA period 1024) — DFN3 hops are
        // 480, so per call we fire 2 hops and carry the residue.
        let mut buf = vec![0i16; 1024];
        for _ in 0..8 {
            ns.process(&mut buf);
        }
        // Output buffer length is preserved.
        assert_eq!(buf.len(), 1024);
    }

    #[test]
    fn process_works_at_native_hop_480() {
        let mut ns = DeepFilterNs::new().expect("model loads");
        let mut buf = vec![0i16; ns.hop_size()];
        ns.process(&mut buf);
        assert_eq!(buf.len(), 480);
    }

    #[test]
    fn process_works_at_arbitrary_chunk_sizes() {
        // Caller passes any size; we buffer internally. Verify no panic.
        let mut ns = DeepFilterNs::new().expect("model loads");
        for sz in [100usize, 256, 480, 512, 1024, 2048] {
            let mut buf = vec![0i16; sz];
            ns.process(&mut buf);
            assert_eq!(buf.len(), sz);
        }
    }

    #[test]
    fn silence_in_silence_out_after_warmup() {
        let mut ns = DeepFilterNs::new().expect("model loads");
        let mut buf = vec![0i16; 1024];
        // Prime: a few periods to clear the warmup zero-pad.
        for _ in 0..4 {
            ns.process(&mut buf);
        }
        // Steady-state silence input → silence output.
        ns.process(&mut buf);
        let peak = buf.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        assert!(peak < 64, "silence steady-state should stay near zero, got peak {}", peak);
    }

    #[test]
    fn white_noise_is_attenuated_after_warmup() {
        let mut ns = DeepFilterNs::new().expect("model loads");
        let frame = 1024usize;
        let mut x: u32 = 0xdeadbeef;
        let make = |x: &mut u32| -> Vec<i16> {
            (0..frame)
                .map(|_| {
                    *x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    let f = ((*x >> 16) as i32 as f32) / 32768.0 * 0.3;
                    (f * 32768.0) as i16
                })
                .collect()
        };
        let in_rms = rms(&make(&mut x));
        let mut last = vec![0i16; frame];
        // Many frames so the system reaches steady state.
        for _ in 0..32 {
            last = make(&mut x);
            ns.process(&mut last);
        }
        let out_rms = rms(&last);
        assert!(
            out_rms < in_rms * 0.7,
            "expected ≥ 3 dB attenuation; in={:.0} out={:.0}",
            in_rms,
            out_rms
        );
    }
}
