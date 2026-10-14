//! Lookahead peak limiter. Holds `lookahead_samples` of input in a ring buffer;
//! emits the delayed sample with attenuation derived from the peak across the
//! ring (current sample + lookahead window). Operates per-frame across all
//! channels — gain is shared so stereo image survives.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LimiterParams {
    pub ceiling_dbfs: f32,
    pub lookahead_samples: usize,
    pub release_ms: f32,
}

impl LimiterParams {
    pub const DEFAULT: LimiterParams = LimiterParams {
        ceiling_dbfs: -1.0,
        lookahead_samples: 96,
        release_ms: 50.0,
    };
}

/// dB/s the held peak reduction recovers toward 0 between batches. Picked
/// to outlive the 500 ms stats cadence — at 6 dB/s, a -10 dB reduction
/// remains visible (~ -7 dB) on the next stats sample.
const PEAK_HOLD_DECAY_DB_PER_S: f32 = 6.0;

pub struct LookaheadLimiter {
    /// Frame ring buffer (one Vec<f32> per frame, length = channels).
    ring: VecDeque<Vec<f32>>,
    channels: usize,
    sample_rate_hz: f32,
    /// Configured lookahead (frames). The emit gate is `ring.len() > lookahead`.
    lookahead: usize,
    ceiling: f32,
    release_alpha: f32,
    /// Current attenuation factor (1.0 = no attenuation).
    current_atten: f32,
    /// Peak-held reduction (dB, ≤ 0). Updated on every batch: takes the
    /// deeper of {block-min reduction, decayed previous peak}. Decays toward
    /// zero at `PEAK_HOLD_DECAY_DB_PER_S` so /tmp/audio_stats.json reflects
    /// recent activity at the 500 ms cadence rather than the last block alone.
    last_reduction_db: f32,
}

impl LookaheadLimiter {
    pub fn new(params: LimiterParams, channels: usize, sample_rate_hz: f32) -> Self {
        let release_alpha = (-1.0_f32 / (params.release_ms * 1e-3 * sample_rate_hz)).exp();
        Self {
            ring: VecDeque::with_capacity(params.lookahead_samples + 1),
            channels,
            sample_rate_hz,
            lookahead: params.lookahead_samples,
            ceiling: 10.0_f32.powf(params.ceiling_dbfs / 20.0),
            release_alpha,
            current_atten: 1.0,
            last_reduction_db: 0.0,
        }
    }

    /// Output delay in frames; the first this many frames after a reset
    /// are silence.
    pub fn lookahead_frames(&self) -> usize {
        self.lookahead
    }

    pub fn last_reduction_db(&self) -> f32 {
        self.last_reduction_db
    }

    pub fn reset(&mut self) {
        self.ring.clear();
        self.current_atten = 1.0;
        self.last_reduction_db = 0.0;
    }

    /// Process interleaved f32 in place. The output is delayed by
    /// `lookahead` frames; the first `lookahead` output frames after a
    /// `reset()` (or first call) are zero-filled silence.
    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        debug_assert_eq!(buf.len() % self.channels, 0,
            "buf length must be a multiple of channels");
        let mut min_atten_this_block = 1.0_f32;
        let mut out: Vec<f32> = Vec::with_capacity(buf.len());

        for frame in buf.chunks_exact(self.channels) {
            // Push the new input frame into the ring.
            let frame_vec: Vec<f32> = frame.to_vec();
            self.ring.push_back(frame_vec);

            if self.ring.len() > self.lookahead {
                // Ring contains lookahead+1 frames: oldest is ready to emit.
                // Peak-search across the entire ring (= current + lookahead window).
                let mut peak = 0.0_f32;
                for f in self.ring.iter() {
                    for &s in f {
                        peak = peak.max(s.abs());
                    }
                }
                // Required attenuation for this output sample.
                let req_atten = if peak > self.ceiling {
                    self.ceiling / peak
                } else {
                    1.0
                };
                // Pull current_atten down instantly (attack), release upward.
                if req_atten < self.current_atten {
                    self.current_atten = req_atten;
                } else {
                    self.current_atten = self.current_atten * self.release_alpha
                                       + req_atten * (1.0 - self.release_alpha);
                }
                min_atten_this_block = min_atten_this_block.min(self.current_atten);

                let oldest = self.ring.pop_front().unwrap();
                for &s in &oldest {
                    out.push(s * self.current_atten);
                }
            } else {
                // Fill phase — ring is still accumulating lookahead.
                // Emit silence so output length matches input length.
                // (Net effect: introduces `lookahead` frames of startup latency.)
                for _ in 0..self.channels {
                    out.push(0.0);
                }
            }
        }
        buf.copy_from_slice(&out);
        let now_db = if min_atten_this_block < 1.0 {
            20.0 * min_atten_this_block.log10()
        } else { 0.0 };
        // Decay the held peak toward 0 by the time spanned by this batch,
        // then keep the deeper of {decayed-held, this-block}.
        let n_frames = (buf.len() / self.channels) as f32;
        let dt_s = n_frames / self.sample_rate_hz;
        let decayed = (self.last_reduction_db + PEAK_HOLD_DECAY_DB_PER_S * dt_s).min(0.0);
        self.last_reduction_db = decayed.min(now_db);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_ceiling_passes_unchanged_after_warmup() {
        let mut lim = LookaheadLimiter::new(LimiterParams::DEFAULT, 2, 48_000.0);
        // 1024 stereo frames at -6 dBFS sine.
        let amp = 10.0_f32.powf(-6.0 / 20.0);
        let mut buf: Vec<f32> = (0..1024)
            .flat_map(|i| {
                let s = amp * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin();
                [s, s]
            })
            .collect();
        let original = buf.clone();
        lim.process_interleaved(&mut buf);
        // After 96-frame warmup (= 192 samples), output[i] should equal
        // original[i - 192] within numerical noise.
        for i in 200..buf.len() {
            assert!(
                (buf[i] - original[i - 192]).abs() < 1e-3,
                "post-warmup sample {i} mismatch: got {} expected {}",
                buf[i], original[i - 192]
            );
        }
    }

    #[test]
    fn full_scale_attenuated_to_ceiling() {
        let mut lim = LookaheadLimiter::new(LimiterParams::DEFAULT, 2, 48_000.0);
        // ±1.0 square wave (alternating per sample).
        let mut buf: Vec<f32> = (0..4096)
            .flat_map(|i| {
                let s = if i % 2 == 0 { 1.0 } else { -1.0 };
                [s, s]
            })
            .collect();
        lim.process_interleaved(&mut buf);
        // After warmup, peaks must stay below the -1 dBFS ceiling.
        let mut max_post_warmup = 0.0_f32;
        for &s in &buf[400..] {
            max_post_warmup = max_post_warmup.max(s.abs());
        }
        let ceiling = 10.0_f32.powf(-1.0 / 20.0);
        assert!(
            max_post_warmup <= ceiling + 1e-3,
            "expected <= {ceiling:.4} (-1 dBFS), got {max_post_warmup:.4}"
        );
        // Sanity: also confirm the limiter did SOMETHING (output isn't silent).
        // A correct limiter on a unit square wave produces output near the ceiling.
        assert!(
            max_post_warmup > 0.5,
            "expected post-warmup output near ceiling, got {max_post_warmup:.4} \
             (looks like the lookahead never emitted real samples)"
        );
    }
}
