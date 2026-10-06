//! Stereo alignment (C15).
//!
//! Stereo mode rebuilds its pair around the processed mid
//! (`processed_frame::rebuild_stereo`), adding the side `(L − R)/2` from the
//! raw beam. The mid has been through the echo canceller, the Speex residual
//! stage and, with NR on, DeepFilterNet, which delay it
//! (`AudioService::mid_latency_samples`). `SideDelay` holds the raw pair back
//! by the same number of samples so side and mid line up again.

use std::collections::VecDeque;

/// A two-channel delay line whose delay may change between periods.
#[derive(Debug, Default)]
pub struct SideDelay {
    left: VecDeque<f32>,
    right: VecDeque<f32>,
}

impl SideDelay {
    /// Delay one period of the pair by `delay` samples. Returns as many
    /// samples as came in. When the delay grows (a suppressor's queue
    /// filling) the gap is silence; when it shrinks the oldest samples are
    /// dropped.
    pub fn process(&mut self, left: &[f32], right: &[f32], delay: usize) -> (Vec<f32>, Vec<f32>) {
        let n = left.len().min(right.len());
        let out_l = Self::step(&mut self.left, &left[..n], delay);
        let out_r = Self::step(&mut self.right, &right[..n], delay);
        (out_l, out_r)
    }

    /// Forget the history (mode change): the next period starts from silence.
    pub fn reset(&mut self) {
        self.left.clear();
        self.right.clear();
    }

    fn step(buf: &mut VecDeque<f32>, input: &[f32], delay: usize) -> Vec<f32> {
        buf.extend(input);
        let want = input.len() + delay;
        while buf.len() < want {
            buf.push_front(0.0);
        }
        while buf.len() > want {
            buf.pop_front();
        }
        buf.drain(..input.len()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(start: usize, n: usize) -> Vec<f32> {
        (start..start + n).map(|i| i as f32).collect()
    }

    #[test]
    fn zero_delay_passes_the_pair_through() {
        let mut d = SideDelay::default();
        let (l, r) = d.process(&[1.0, 2.0], &[3.0, 4.0], 0);
        assert_eq!((l, r), (vec![1.0, 2.0], vec![3.0, 4.0]));
    }

    #[test]
    fn delays_across_periods() {
        let mut d = SideDelay::default();
        let mut out = Vec::new();
        for k in 0..4 {
            let x = ramp(1 + k * 5, 5);
            let (l, r) = d.process(&x, &x, 7);
            assert_eq!(l, r);
            out.extend(l);
        }
        let expected: Vec<f32> = std::iter::repeat_n(0.0, 7).chain(ramp(1, 13)).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn a_growing_delay_inserts_silence_and_a_shrinking_one_drops() {
        let mut d = SideDelay::default();
        assert_eq!(d.process(&[1.0, 2.0], &[0.0; 2], 1).0, vec![0.0, 1.0]);
        // Grows by 2: two zeros before the held-back sample.
        assert_eq!(d.process(&[3.0, 4.0], &[0.0; 2], 3).0, vec![0.0, 0.0]);
        assert_eq!(d.process(&[5.0, 6.0], &[0.0; 2], 3).0, vec![2.0, 3.0]);
        // Shrinks to 0: the backlog is dropped.
        assert_eq!(d.process(&[7.0, 8.0], &[0.0; 2], 0).0, vec![7.0, 8.0]);
    }

    #[test]
    fn reset_starts_from_silence() {
        let mut d = SideDelay::default();
        d.process(&[1.0, 2.0], &[1.0, 2.0], 2);
        d.reset();
        assert_eq!(d.process(&[5.0, 6.0], &[5.0, 6.0], 2).0, vec![0.0, 0.0]);
    }
}
