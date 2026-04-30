//! Orchestrator for the playback DSP chain. Order: HPF → EQ → MBAGC → Limiter.
//! Input/output is interleaved S16 (the format `PlaybackService` already uses);
//! conversion to/from f32 happens here.

use super::eq::{BiquadParams, ParametricEq, N_BANDS, N_CHANNELS};
use super::hpf::HighPassFilter;
use super::limiter::{LimiterParams, LookaheadLimiter};
use super::mbagc::{MbagcParams, MultibandAgc};

pub struct PlaybackDspChain {
    hpf: HighPassFilter,
    eq: ParametricEq,
    mbagc: MultibandAgc,
    limiter: LookaheadLimiter,
    f32_buf: Vec<f32>,
    sample_rate_hz: f32,
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
