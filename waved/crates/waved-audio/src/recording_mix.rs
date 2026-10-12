//! The recording's track: the room plus the far end of a call.
//!
//! The uplink cancels whatever the Wave's speaker plays, which is right for
//! the call (the far end must not hear itself) but left meeting recordings
//! with only the room's side (device pass, 2026-10-12). The recording now
//! adds what the computer sent to the speaker, as it was sent: before the
//! speaker EQ and volume, so the remote side does not follow the volume
//! knob.
//!
//! The far end arrives from the playback thread one period at a time. The
//! echo canceller's reference keeps only the freshest period, which is fine
//! for cancellation; a recording needs every sample once, so `FarEndFeed`
//! queues them and delays the queue to line up with the processed room.

use std::collections::VecDeque;

use tokio::sync::broadcast;

use crate::playback::dsp::limiter::{LimiterParams, LookaheadLimiter};

/// The queue may run this many periods ahead of its target before the
/// oldest samples are dropped (the two threads share a clock, so this only
/// absorbs scheduling jitter).
const MAX_AHEAD_PERIODS: usize = 4;

/// The far end as mono samples, one capture period at a time.
pub struct FarEndFeed {
    rx: broadcast::Receiver<Vec<i16>>,
    channels: usize,
    queue: VecDeque<f32>,
    /// Samples of delay the queue holds in steady state.
    delay: usize,
    period: usize,
}

impl FarEndFeed {
    /// `channels` is the interleaving of the playback periods; `period` the
    /// capture period in frames; `delay` how far to hold the far end back
    /// (the uplink's processing latency).
    pub fn new(rx: broadcast::Receiver<Vec<i16>>, channels: usize, period: usize, delay: usize) -> Self {
        Self {
            rx,
            channels: channels.max(1),
            queue: std::iter::repeat_n(0.0, delay).collect(),
            delay,
            period: period.max(1),
        }
    }

    /// Fill `out` with the next far-end samples, oldest first.
    pub fn take(&mut self, out: &mut [f32]) {
        loop {
            match self.rx.try_recv() {
                Ok(batch) => {
                    for frame in batch.chunks_exact(self.channels) {
                        let sum: f32 = frame.iter().map(|&s| f32::from(s)).sum();
                        self.queue.push_back(sum / (self.channels as f32 * 32_768.0));
                    }
                }
                // Lost periods are lost; the queue carries on without them.
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        let limit = self.delay + out.len() + MAX_AHEAD_PERIODS * self.period;
        if self.queue.len() > limit {
            let excess = self.queue.len() - (self.delay + out.len());
            self.queue.drain(..excess);
        }
        for sample in out.iter_mut() {
            // Short of samples: silence, never a repeat.
            *sample = self.queue.pop_front().unwrap_or(0.0);
        }
    }
}

/// Adds the far end to the room and keeps the sum under the ceiling.
pub struct RecordingMix {
    feed: FarEndFeed,
    far: Vec<f32>,
    limiter: LookaheadLimiter,
}

impl RecordingMix {
    pub fn new(feed: FarEndFeed, sample_rate: u32) -> Self {
        Self {
            feed,
            far: Vec::new(),
            limiter: LookaheadLimiter::new(LimiterParams::DEFAULT, 1, sample_rate as f32),
        }
    }

    /// The recording track for this period of the processed room.
    pub fn mix(&mut self, room: &[f32]) -> Vec<f32> {
        self.far.resize(room.len(), 0.0);
        self.feed.take(&mut self.far);
        let mut out: Vec<f32> = room.iter().zip(&self.far).map(|(r, f)| r + f).collect();
        self.limiter.process_interleaved(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(values: &[i16]) -> Vec<i16> {
        values.iter().flat_map(|&v| [v, v]).collect()
    }

    #[test]
    fn the_far_end_comes_out_once_in_order_and_delayed() {
        let (tx, rx) = broadcast::channel(16);
        let mut feed = FarEndFeed::new(rx, 2, 4, 3);
        tx.send(stereo(&[100, 200, 300, 400])).unwrap();
        tx.send(stereo(&[500, 600, 700, 800])).unwrap();
        let mut out = [0.0; 4];
        feed.take(&mut out);
        let scale = |v: i16| f32::from(v) / 32_768.0;
        assert_eq!(out, [0.0, 0.0, 0.0, scale(100)]);
        feed.take(&mut out);
        assert_eq!(out, [scale(200), scale(300), scale(400), scale(500)]);
    }

    #[test]
    fn a_late_period_leaves_silence_not_a_repeat() {
        let (tx, rx) = broadcast::channel(16);
        let mut feed = FarEndFeed::new(rx, 1, 2, 0);
        tx.send(vec![1000, 1000]).unwrap();
        let mut out = [0.0; 2];
        feed.take(&mut out);
        assert!(out.iter().all(|&s| s > 0.0));
        feed.take(&mut out);
        assert_eq!(out, [0.0, 0.0]);
    }

    #[test]
    fn running_ahead_drops_the_oldest_and_keeps_the_delay_bounded() {
        let (tx, rx) = broadcast::channel(64);
        let mut feed = FarEndFeed::new(rx, 1, 2, 2);
        for n in 0..20 {
            tx.send(vec![n, n]).unwrap();
        }
        let mut out = [0.0; 2];
        feed.take(&mut out);
        // Back to the target delay: the newest samples, two behind the end.
        assert_eq!(out, [18.0 / 32_768.0; 2]);
    }

    #[test]
    fn a_closed_playback_leaves_the_far_end_silent() {
        let (tx, rx) = broadcast::channel::<Vec<i16>>(4);
        drop(tx);
        let mut feed = FarEndFeed::new(rx, 2, 4, 0);
        let mut out = [1.0; 4];
        feed.take(&mut out);
        assert_eq!(out, [0.0; 4]);
    }

    #[test]
    fn the_mix_carries_both_sides_and_stays_under_the_ceiling() {
        let (tx, rx) = broadcast::channel(64);
        let period = 480;
        let mut mix = RecordingMix::new(FarEndFeed::new(rx, 1, period, 0), 48_000);
        let room = vec![0.2f32; period];
        let mut total = Vec::new();
        for _ in 0..10 {
            tx.send(vec![(0.3 * 32_768.0) as i16; period]).unwrap();
            total.extend(mix.mix(&room));
        }
        let settled = &total[2 * period..];
        assert!(settled.iter().all(|&s| (s - 0.5).abs() < 0.01), "room + far end");

        for _ in 0..10 {
            tx.send(vec![i16::MAX; period]).unwrap();
            total.extend(mix.mix(&vec![0.9; period]));
        }
        let ceiling = 10f32.powf(LimiterParams::DEFAULT.ceiling_dbfs / 20.0);
        assert!(total.iter().all(|&s| s.abs() <= ceiling + 1e-4), "peak {}", total.iter().fold(0.0f32, |m, &s| m.max(s.abs())));
    }
}
