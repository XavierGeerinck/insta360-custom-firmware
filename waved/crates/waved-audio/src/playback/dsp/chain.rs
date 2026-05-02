//! Orchestrator for the playback DSP chain. Order: HPF → EQ → MBAGC → Limiter.
//! Input/output is interleaved S16 (the format `PlaybackService` already uses);
//! conversion to/from f32 happens here.

use super::eq::{BiquadParams, ParametricEq, N_BANDS, N_CHANNELS};
use super::hpf::HighPassFilter;
use super::limiter::{LimiterParams, LookaheadLimiter};
use super::mbagc::{MbagcParams, MultibandAgc};

/// Spec §2.8: 5 ms raised-cosine envelope on preset switch to mask the
/// coefficient discontinuity. At 48 kHz that's 240 frames.
const PRESET_RAMP_MS: f32 = 5.0;

pub struct PlaybackDspChain {
    hpf: HighPassFilter,
    eq: ParametricEq,
    mbagc: MultibandAgc,
    limiter: LookaheadLimiter,
    f32_buf: Vec<f32>,
    sample_rate_hz: f32,
    /// Last interleaved output frame (per channel) — captured before each
    /// `set_preset` so the cosine ramp can crossfade from the held value
    /// into the new chain's output.
    last_output: [f32; N_CHANNELS],
    /// Total frames in the active ramp window (0 when no ramp in progress).
    ramp_total: usize,
    /// Frames remaining in the active ramp window. When >0, the next
    /// process_i16 crossfades `last_output` → new chain output.
    ramp_remaining: usize,
}

impl PlaybackDspChain {
    pub fn new(
        hpf_cutoff_hz: f32,
        eq_params: &[[BiquadParams; N_BANDS]; N_CHANNELS],
        mbagc_params: MbagcParams,
        limiter_params: LimiterParams,
        sample_rate_hz: f32,
    ) -> Self {
        Self {
            hpf: HighPassFilter::new(N_CHANNELS, hpf_cutoff_hz, sample_rate_hz),
            eq: ParametricEq::new(eq_params, sample_rate_hz),
            mbagc: MultibandAgc::new(mbagc_params, sample_rate_hz),
            limiter: LookaheadLimiter::new(limiter_params, N_CHANNELS, sample_rate_hz),
            f32_buf: Vec::new(),
            sample_rate_hz,
            last_output: [0.0; N_CHANNELS],
            ramp_total: 0,
            ramp_remaining: 0,
        }
    }

    pub fn set_preset(
        &mut self,
        hpf_cutoff_hz: f32,
        eq_params: &[[BiquadParams; N_BANDS]; N_CHANNELS],
        mbagc_params: MbagcParams,
        limiter_params: LimiterParams,
    ) {
        self.hpf.set_cutoff(hpf_cutoff_hz);
        self.eq.set_params(eq_params);
        self.mbagc.set_params(mbagc_params);
        // Limiter rebuilt from scratch: ceiling/release/lookahead can change.
        // Discarding the lookahead ring restarts the warm-up window — acceptable
        // because preset switch is treated as a transition, not a continuous edit.
        self.limiter = LookaheadLimiter::new(limiter_params, N_CHANNELS, self.sample_rate_hz);
        // Reset MBAGC envelopes to neutral on switch so stale targets don't bleed.
        self.mbagc.reset();
        // Arm the cosine ramp. `last_output` was captured at the end of the
        // most recent process_i16, so the next batch will crossfade from it
        // into whatever the new chain produces. The ramp is shorter than a
        // typical batch (5 ms = 240 frames at 48 kHz; batches are usually
        // 1024) so it completes inside one call.
        let ramp = (PRESET_RAMP_MS * 1e-3 * self.sample_rate_hz).round() as usize;
        self.ramp_total = ramp;
        self.ramp_remaining = ramp;
    }

    pub fn last_mbagc_gain_db(&self) -> [f32; super::mbagc::N_BANDS] {
        self.mbagc.last_gain_db()
    }

    pub fn last_limiter_reduction_db(&self) -> f32 {
        self.limiter.last_reduction_db()
    }

    /// Process interleaved S16 stereo in place. `buf.len()` must be a multiple of 2.
    pub fn process_i16(&mut self, buf: &mut [i16]) {
        if buf.is_empty() { return; }
        debug_assert_eq!(buf.len() % N_CHANNELS, 0,
            "buf length must be a multiple of N_CHANNELS");
        // Resize scratch f32 buffer if needed.
        if self.f32_buf.len() != buf.len() {
            self.f32_buf.resize(buf.len(), 0.0);
        }
        // Convert i16 → f32 in [-1, 1).
        for (dst, &src) in self.f32_buf.iter_mut().zip(buf.iter()) {
            *dst = src as f32 / 32768.0;
        }
        // Run the chain.
        self.hpf.process_interleaved(&mut self.f32_buf);
        self.eq.process_interleaved(&mut self.f32_buf);
        self.mbagc.process_interleaved(&mut self.f32_buf);
        self.limiter.process_interleaved(&mut self.f32_buf);

        // Preset cosine ramp: blend `last_output` → new chain output across
        // the next `ramp_total` frames. env(t) goes 0→1 as t goes 0→1.
        if self.ramp_remaining > 0 && self.ramp_total > 0 {
            let total = self.ramp_total as f32;
            let n_frames = self.f32_buf.len() / N_CHANNELS;
            let take = self.ramp_remaining.min(n_frames);
            let consumed_before = self.ramp_total - self.ramp_remaining;
            for i in 0..take {
                let t = (consumed_before + i) as f32 / total;
                let env = 0.5 - 0.5 * (std::f32::consts::PI * t).cos();
                let inv = 1.0 - env;
                for ch in 0..N_CHANNELS {
                    let idx = i * N_CHANNELS + ch;
                    self.f32_buf[idx] = inv * self.last_output[ch] + env * self.f32_buf[idx];
                }
            }
            self.ramp_remaining -= take;
        }

        // Capture the final frame for the next preset switch's hold value.
        let n = self.f32_buf.len();
        if n >= N_CHANNELS {
            for ch in 0..N_CHANNELS {
                self.last_output[ch] = self.f32_buf[n - N_CHANNELS + ch];
            }
        }

        // Convert back to S16 with hard clamp. The limiter keeps us in range,
        // but clamp guards against any final sample > ±1 due to numerical drift.
        for (dst, &src) in buf.iter_mut().zip(self.f32_buf.iter()) {
            *dst = (src.clamp(-1.0, 1.0) * 32767.0) as i16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_eq() -> [[BiquadParams; N_BANDS]; N_CHANNELS] {
        [[BiquadParams::BYPASS; N_BANDS]; N_CHANNELS]
    }

    #[test]
    fn mid_playback_preset_switch_clears_state() {
        // Drive the limiter into deep reduction with a hot signal, capture the
        // post-block reduction, then call set_preset and confirm the rebuild
        // returns the limiter to a clean reduction_db ~ 0 on the next batch.
        let mut chain = PlaybackDspChain::new(
            80.0,
            &flat_eq(),
            MbagcParams::PASSTHROUGH,
            LimiterParams::DEFAULT,
            48_000.0,
        );
        // Hot square wave at ±32767 — drives the limiter to its ceiling.
        let mut hot: Vec<i16> = (0..2048).flat_map(|i| {
            let s = if i % 2 == 0 { 32767 } else { -32767 };
            [s, s]
        }).collect();
        chain.process_i16(&mut hot);
        let pre_switch_reduction_db = chain.last_limiter_reduction_db();
        assert!(
            pre_switch_reduction_db < -0.5,
            "expected hot signal to engage limiter, got {pre_switch_reduction_db:.2} dB",
        );

        // Switch presets mid-playback. set_preset rebuilds the limiter from
        // scratch, which discards the held peak.
        chain.set_preset(
            80.0,
            &flat_eq(),
            MbagcParams::PASSTHROUGH,
            LimiterParams::DEFAULT,
        );
        // After rebuild, last_reduction_db must reset to 0 immediately —
        // the new limiter has no history.
        assert_eq!(
            chain.last_limiter_reduction_db(), 0.0,
            "limiter rebuild must zero last_reduction_db",
        );

        // First batch after switch is during the cosine ramp + limiter
        // warm-up. Run a soft signal so the limiter doesn't immediately
        // re-engage, and verify the chain is alive (output amplitude > 0
        // somewhere past the warm-up window).
        let amp = 0.05_f32;
        let mut soft: Vec<i16> = (0..2048).flat_map(|i| {
            let s = amp * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin();
            let v = (s * 32767.0) as i16;
            [v, v]
        }).collect();
        chain.process_i16(&mut soft);
        // Reduction should remain ~0 because the soft signal is well below
        // the ceiling.
        assert!(
            chain.last_limiter_reduction_db().abs() < 0.5,
            "soft signal post-switch should not engage limiter, got {:.2} dB",
            chain.last_limiter_reduction_db(),
        );
        // Output past the lookahead window must be non-zero (chain is alive).
        let max_past_warmup = soft.iter().skip(400).map(|&s| s.abs()).max().unwrap_or(0);
        assert!(max_past_warmup > 100, "chain output looks dead post-switch: {max_past_warmup}");
    }

    #[test]
    fn passthrough_chain_preserves_low_amplitude_signal() {
        // EQ flat, MBAGC bypassed, HPF on (80 Hz). 1 kHz signal sits well in
        // the passband. After the limiter's 96-sample lookahead warm-up,
        // RMS energy should match the input within a couple of dB.
        let mut chain = PlaybackDspChain::new(
            80.0,
            &flat_eq(),
            MbagcParams::PASSTHROUGH,
            LimiterParams::DEFAULT,
            48_000.0,
        );
        let amp = 0.1_f32; // -20 dBFS, well under limiter ceiling
        let mut buf: Vec<i16> = (0..2048)
            .flat_map(|i| {
                let s = amp * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin();
                let v = (s * 32767.0) as i16;
                [v, v]
            })
            .collect();
        chain.process_i16(&mut buf);
        // Sine RMS at amp=0.1 → -20 dBFS amplitude → -23 dBFS RMS. After warm-up.
        let rms_squared: f64 = buf[400..].iter().map(|&s| (s as f64).powi(2)).sum::<f64>()
                             / (buf.len() - 400) as f64;
        let rms = rms_squared.sqrt() / 32768.0;
        let rms_db = 20.0 * (rms as f32).log10();
        assert!(rms_db > -25.0 && rms_db < -21.0,
                "expected ~-23 dBFS, got {rms_db:.2} dBFS");
    }
}
