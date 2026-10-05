//! Microphone health check — open-firmware analogue of stock's
//! `Insta360_MicCheckAlgo(duration_ms)`.
//!
//! The stock library runs a duration-bounded self-test that flags a dead or
//! degraded element in the 8-mic ring. We do the same with a pure, allocation-
//! light accumulator: feed it interleaved multi-channel frames for a while
//! (ambient room noise or a played stimulus is enough), then ask for a
//! verdict. Each channel's RMS is compared to the **median** RMS across the
//! array — a robust reference that tolerates a couple of bad elements without
//! skewing the baseline.
//!
//! It deliberately makes no judgement when the whole array is silent: with no
//! signal there's nothing to distinguish a dead mic from a quiet room, so all
//! channels report [`MicStatus::Ok`] (no false positives). Drive it with some
//! signal present for a meaningful result.

use waved_state::MicHealth;

/// Per-channel verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicStatus {
    /// Channel level is in family with the rest of the array.
    Ok,
    /// Channel is alive but markedly quieter than its peers (partial fault,
    /// obstruction, or gain mismatch).
    Low,
    /// Channel is effectively silent while the array has signal — dead element.
    Dead,
}

/// Outcome of a mic-check run.
#[derive(Debug, Clone)]
pub struct MicCheckResult {
    /// Per-channel status, length == `channels`.
    pub per_channel: Vec<MicStatus>,
    /// Per-channel RMS (linear, full-scale = 1.0) over the accumulated window.
    pub rms: Vec<f32>,
    /// Median RMS across channels — the reference the verdict is measured against.
    pub median_rms: f32,
    /// True when every channel is [`MicStatus::Ok`].
    pub healthy: bool,
    /// True when the array had too little signal to judge (verdict is all-Ok by
    /// default in that case).
    pub inconclusive: bool,
}

/// Streaming per-channel energy accumulator. Construct, push frames, verdict.
pub struct MicCheck {
    channels: usize,
    sumsq: Vec<f64>,
    count: u64,
}

impl MicCheck {
    /// A channel below `DEAD_FACTOR × median` (with the array carrying signal)
    /// is treated as a dead element.
    const DEAD_FACTOR: f32 = 0.1;
    /// A channel below `LOW_FACTOR × median` (but above dead) is degraded.
    const LOW_FACTOR: f32 = 0.5;
    /// Median RMS below this (linear full-scale) means "no usable signal" — the
    /// run is inconclusive rather than a wall of false "dead" verdicts.
    const SIGNAL_FLOOR: f32 = 1e-4;

    pub fn new(channels: usize) -> Self {
        Self {
            channels,
            sumsq: vec![0.0; channels],
            count: 0,
        }
    }

    /// Accumulate one block of interleaved samples (`ch0,ch1,…,chN,ch0,…`).
    /// A trailing partial frame (len not a multiple of `channels`) is ignored.
    pub fn push_interleaved(&mut self, interleaved: &[f32]) {
        let frames = interleaved.len() / self.channels;
        for f in 0..frames {
            let base = f * self.channels;
            for ch in 0..self.channels {
                let s = interleaved[base + ch] as f64;
                self.sumsq[ch] += s * s;
            }
        }
        self.count += frames as u64;
    }

    /// Samples accumulated per channel so far — use to decide when enough of the
    /// requested duration has elapsed.
    pub fn samples_per_channel(&self) -> u64 {
        self.count
    }

    /// Reset the accumulator to start a fresh run.
    pub fn reset(&mut self) {
        self.sumsq.iter_mut().for_each(|s| *s = 0.0);
        self.count = 0;
    }

    /// Per-channel RMS over the accumulated window.
    fn rms_per_channel(&self) -> Vec<f32> {
        if self.count == 0 {
            return vec![0.0; self.channels];
        }
        self.sumsq
            .iter()
            .map(|&ss| (ss / self.count as f64).sqrt() as f32)
            .collect()
    }

    /// Compute the verdict from the accumulated energy.
    pub fn verdict(&self) -> MicCheckResult {
        let rms = self.rms_per_channel();
        let median = median(&rms);

        if median < Self::SIGNAL_FLOOR {
            return MicCheckResult {
                per_channel: vec![MicStatus::Ok; self.channels],
                rms,
                median_rms: median,
                healthy: true,
                inconclusive: true,
            };
        }

        let dead_thresh = Self::DEAD_FACTOR * median;
        let low_thresh = Self::LOW_FACTOR * median;
        let per_channel: Vec<MicStatus> = rms
            .iter()
            .map(|&r| {
                if r < dead_thresh {
                    MicStatus::Dead
                } else if r < low_thresh {
                    MicStatus::Low
                } else {
                    MicStatus::Ok
                }
            })
            .collect();

        let healthy = per_channel.iter().all(|&s| s == MicStatus::Ok);
        MicCheckResult {
            per_channel,
            rms,
            median_rms: median,
            healthy,
            inconclusive: false,
        }
    }
}

/// Median of a slice (average of the two middle values for an even count).
/// Returns 0.0 for an empty slice. Does not mutate the input.
fn median(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Runs `MicCheck` over the first seconds of capture and turns it into one
/// `MicHealth` verdict (C6). Feed raw frames before mic mute.
pub struct MicHealthMonitor {
    check: MicCheck,
    channels: usize,
    window_frames: u64,
    attempts_left: u32,
    scratch: Vec<f32>,
    done: bool,
}

impl MicHealthMonitor {
    pub const WINDOW_MS: u64 = 3_000;
    /// Quiet-room windows before giving up (30 s with the default window).
    pub const MAX_ATTEMPTS: u32 = 10;

    pub fn new(channels: usize, sample_rate: u32) -> Self {
        Self {
            check: MicCheck::new(channels.max(1)),
            channels,
            window_frames: u64::from(sample_rate) * Self::WINDOW_MS / 1_000,
            attempts_left: Self::MAX_ATTEMPTS,
            scratch: Vec::new(),
            done: channels == 0,
        }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Accumulate interleaved samples; returns the verdict once, when a
    /// window is conclusive.
    pub fn push_i16(&mut self, interleaved: &[i16], channels: usize) -> Option<MicHealth> {
        if self.done || channels != self.channels {
            return None;
        }
        self.scratch.clear();
        self.scratch
            .extend(interleaved.iter().map(|&s| f32::from(s) / 32_768.0));
        self.check.push_interleaved(&self.scratch);
        if self.check.samples_per_channel() < self.window_frames {
            return None;
        }
        let verdict = self.check.verdict();
        self.check.reset();
        let zero = mask(verdict.rms.iter().map(|&rms| rms == 0.0));
        let dead = mask(verdict.per_channel.iter().map(|&s| s == MicStatus::Dead));
        let low = mask(verdict.per_channel.iter().map(|&s| s == MicStatus::Low));
        if zero != 0 {
            self.done = true;
            return Some(MicHealth::NotResponding {
                dead_mask: zero | dead,
            });
        }
        if verdict.inconclusive {
            self.attempts_left = self.attempts_left.saturating_sub(1);
            self.done = self.attempts_left == 0;
            return None;
        }
        self.done = true;
        Some(if dead != 0 {
            MicHealth::NotResponding { dead_mask: dead }
        } else if low != 0 {
            MicHealth::Degraded { low_mask: low }
        } else {
            MicHealth::Ok
        })
    }
}

/// Bit n set when the n-th flag is true (first 8 channels).
fn mask(flags: impl Iterator<Item = bool>) -> u8 {
    flags
        .take(8)
        .enumerate()
        .fold(0u8, |m, (i, set)| if set { m | (1 << i) } else { m })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Push `frames` of interleaved audio where channel `ch` gets amplitude
    /// `amps[ch]` (DC for simplicity — RMS == |amp|).
    fn feed(mc: &mut MicCheck, amps: &[f32], frames: usize) {
        let ch = amps.len();
        let mut buf = vec![0.0f32; ch * frames];
        for f in 0..frames {
            for c in 0..ch {
                buf[f * ch + c] = amps[c];
            }
        }
        mc.push_interleaved(&buf);
    }

    #[test]
    fn healthy_array_all_ok() {
        let mut mc = MicCheck::new(8);
        feed(&mut mc, &[0.2; 8], 1000);
        let v = mc.verdict();
        assert!(v.healthy);
        assert!(!v.inconclusive);
        assert!(v.per_channel.iter().all(|&s| s == MicStatus::Ok));
    }

    #[test]
    fn dead_channel_flagged() {
        let mut mc = MicCheck::new(8);
        let mut amps = [0.2f32; 8];
        amps[3] = 0.0; // dead element
        feed(&mut mc, &amps, 1000);
        let v = mc.verdict();
        assert_eq!(v.per_channel[3], MicStatus::Dead);
        assert!(!v.healthy);
        // The other seven stay Ok.
        assert_eq!(v.per_channel.iter().filter(|&&s| s == MicStatus::Ok).count(), 7);
    }

    #[test]
    fn low_channel_flagged_between_thresholds() {
        let mut mc = MicCheck::new(8);
        let mut amps = [0.2f32; 8];
        amps[5] = 0.05; // 0.25 × median → above dead (0.1×), below low (0.5×)
        feed(&mut mc, &amps, 1000);
        let v = mc.verdict();
        assert_eq!(v.per_channel[5], MicStatus::Low);
        assert!(!v.healthy);
    }

    #[test]
    fn silent_array_is_inconclusive_not_dead() {
        let mut mc = MicCheck::new(8);
        feed(&mut mc, &[0.0; 8], 1000);
        let v = mc.verdict();
        assert!(v.inconclusive);
        assert!(v.healthy, "silence must not be reported as faults");
        assert!(v.per_channel.iter().all(|&s| s == MicStatus::Ok));
    }

    #[test]
    fn no_samples_is_inconclusive() {
        let mc = MicCheck::new(8);
        let v = mc.verdict();
        assert!(v.inconclusive);
        assert!(v.healthy);
    }

    #[test]
    fn median_even_and_odd() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), 2.5);
        assert_eq!(median(&[]), 0.0);
    }

    use waved_state::MicHealth;

    /// `frames` of interleaved i16 with channel c at ±amps[c] (RMS = |amp|).
    fn frames_i16(amps: &[i16], frames: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(amps.len() * frames);
        for f in 0..frames {
            let sign = if f % 2 == 0 { 1 } else { -1 };
            out.extend(amps.iter().map(|&a| a * sign));
        }
        out
    }

    #[test]
    fn healthy_array_is_ok_after_one_window() {
        let mut m = MicHealthMonitor::new(8, 1_000); // 3 000-frame window
        assert_eq!(m.push_i16(&frames_i16(&[3_000; 8], 2_000), 8), None);
        assert_eq!(
            m.push_i16(&frames_i16(&[3_000; 8], 1_000), 8),
            Some(MicHealth::Ok)
        );
        assert!(m.is_done());
        assert_eq!(
            m.push_i16(&frames_i16(&[0; 8], 3_000), 8),
            None,
            "the verdict is final"
        );
    }

    #[test]
    fn a_dead_element_is_not_responding() {
        let mut amps = [3_000i16; 8];
        amps[3] = 0;
        let mut m = MicHealthMonitor::new(8, 1_000);
        assert_eq!(
            m.push_i16(&frames_i16(&amps, 3_000), 8),
            Some(MicHealth::NotResponding {
                dead_mask: 0b0000_1000
            })
        );
    }

    #[test]
    fn a_quiet_element_is_degraded() {
        let mut amps = [3_000i16; 8];
        amps[5] = 750; // 0.25 × median
        let mut m = MicHealthMonitor::new(8, 1_000);
        assert_eq!(
            m.push_i16(&frames_i16(&amps, 3_000), 8),
            Some(MicHealth::Degraded {
                low_mask: 0b0010_0000
            })
        );
    }

    #[test]
    fn an_all_zero_capture_is_not_responding() {
        let mut m = MicHealthMonitor::new(8, 1_000);
        assert_eq!(
            m.push_i16(&frames_i16(&[0; 8], 3_000), 8),
            Some(MicHealth::NotResponding { dead_mask: 0xFF })
        );
    }

    #[test]
    fn a_quiet_room_retries_then_gives_up() {
        let mut m = MicHealthMonitor::new(8, 1_000);
        for _ in 0..MicHealthMonitor::MAX_ATTEMPTS {
            assert!(!m.is_done());
            assert_eq!(m.push_i16(&frames_i16(&[1; 8], 3_000), 8), None);
        }
        assert!(m.is_done(), "stays Unknown after the last attempt");
    }

    #[test]
    fn frames_with_another_channel_count_are_ignored() {
        let mut m = MicHealthMonitor::new(8, 1_000);
        assert_eq!(m.push_i16(&frames_i16(&[0; 2], 3_000), 2), None);
        assert!(!m.is_done());
    }
}
