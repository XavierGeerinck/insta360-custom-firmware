//! Orchestrator for the playback DSP chain. Order: HPF → EQ → MBAGC → Limiter,
//! each stage optional (`PlaybackPresetTuning`). Input/output is interleaved
//! S16 (the format `PlaybackService` already uses); conversion to/from f32
//! happens here. With every stage off the chain does not touch the samples.
//!
//! A preset switch fades the running chain out, swaps the stages (fresh
//! filter state) at silence and fades the new chain in, so a switch in the
//! middle of music or a call does not click. The AEC reference is tapped
//! after the chain, so it carries the same fade.

use super::eq::{self, ParametricEq, N_CHANNELS};
use super::hpf::HighPassFilter;
use super::limiter::LookaheadLimiter;
use super::mbagc::MultibandAgc;
use crate::playback::preset::PlaybackPresetTuning;

/// Fade length on each side of a preset switch.
const PRESET_RAMP_MS: f32 = 5.0;

/// Where a preset switch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fade {
    /// No switch in progress.
    Idle,
    /// Fading the old stages out; `pos` frames of the ramp done.
    Out { pos: usize },
    /// Fading the new stages in; `pos` frames done, counting `hold` silent
    /// frames first (the new limiter's look-ahead fill).
    In { pos: usize, hold: usize },
}

struct Stages {
    hpf: Option<HighPassFilter>,
    eq: Option<ParametricEq>,
    mbagc: MultibandAgc,
    limiter: Option<LookaheadLimiter>,
}

impl Stages {
    fn new(tuning: &PlaybackPresetTuning, sample_rate_hz: f32) -> Self {
        Self {
            hpf: tuning.hpf_cutoff_hz.map(|hz| HighPassFilter::new(N_CHANNELS, hz, sample_rate_hz)),
            eq: eq::is_active(&tuning.eq).then(|| ParametricEq::new(&tuning.eq, sample_rate_hz)),
            mbagc: MultibandAgc::new(tuning.mbagc, sample_rate_hz),
            limiter: tuning.limiter.map(|p| LookaheadLimiter::new(p, N_CHANNELS, sample_rate_hz)),
        }
    }

    fn is_identity(&self) -> bool {
        self.hpf.is_none() && self.eq.is_none() && !self.mbagc.is_enabled() && self.limiter.is_none()
    }

    /// Frames of silence the limiter emits before its look-ahead is full.
    fn warm_up_frames(&self) -> usize {
        self.limiter.as_ref().map_or(0, LookaheadLimiter::lookahead_frames)
    }

    fn process(&mut self, buf: &mut [f32]) {
        if let Some(hpf) = &mut self.hpf {
            hpf.process_interleaved(buf);
        }
        if let Some(eq) = &mut self.eq {
            eq.process_interleaved(buf);
        }
        self.mbagc.process_interleaved(buf);
        if let Some(limiter) = &mut self.limiter {
            limiter.process_interleaved(buf);
        }
    }
}

pub struct PlaybackDspChain {
    stages: Stages,
    /// Tuning waiting for the fade-out to finish.
    pending: Option<PlaybackPresetTuning>,
    fade: Fade,
    ramp_frames: usize,
    f32_buf: Vec<f32>,
    sample_rate_hz: f32,
}

/// Raised cosine, 0 → 1 as `pos` goes 0 → `len`.
fn rise(pos: usize, len: usize) -> f32 {
    if len == 0 {
        return 1.0;
    }
    let t = (pos as f32 / len as f32).min(1.0);
    0.5 - 0.5 * (std::f32::consts::PI * t).cos()
}

impl PlaybackDspChain {
    pub fn new(tuning: &PlaybackPresetTuning, sample_rate_hz: f32) -> Self {
        Self {
            stages: Stages::new(tuning, sample_rate_hz),
            pending: None,
            fade: Fade::Idle,
            ramp_frames: (PRESET_RAMP_MS * 1e-3 * sample_rate_hz).round() as usize,
            f32_buf: Vec::new(),
            sample_rate_hz,
        }
    }

    /// Switch to `tuning`: the running stages fade out, then the new ones
    /// fade in. A switch during a fade-in turns it around at the current
    /// level; during a fade-out the target is replaced.
    pub fn set_preset(&mut self, tuning: &PlaybackPresetTuning) {
        self.pending = Some(tuning.clone());
        self.fade = match self.fade {
            Fade::Idle => Fade::Out { pos: 0 },
            out @ Fade::Out { .. } => out,
            // Silent hold: swap right away. Rising: continue down from the
            // same gain (the fade-out is the fade-in mirrored).
            Fade::In { pos, hold } if pos < hold => Fade::Out { pos: self.ramp_frames },
            Fade::In { pos, hold } => Fade::Out { pos: self.ramp_frames - (pos - hold).min(self.ramp_frames) },
        };
    }

    /// True while a preset switch is fading.
    pub fn is_switching(&self) -> bool {
        self.fade != Fade::Idle
    }

    pub fn last_mbagc_gain_db(&self) -> [f32; super::mbagc::N_BANDS] {
        self.stages.mbagc.last_gain_db()
    }

    /// 0 when no limiter runs.
    pub fn last_limiter_reduction_db(&self) -> f32 {
        self.stages.limiter.as_ref().map_or(0.0, LookaheadLimiter::last_reduction_db)
    }

    /// Process interleaved S16 stereo in place. `buf.len()` must be a multiple of 2.
    pub fn process_i16(&mut self, buf: &mut [i16]) {
        if buf.is_empty() || (self.fade == Fade::Idle && self.stages.is_identity()) {
            return;
        }
        debug_assert_eq!(buf.len() % N_CHANNELS, 0,
            "buf length must be a multiple of N_CHANNELS");
        let mut f32_buf = std::mem::take(&mut self.f32_buf);
        f32_buf.clear();
        f32_buf.extend(buf.iter().map(|&s| s as f32 / 32768.0));

        let n_frames = f32_buf.len() / N_CHANNELS;
        let mut frame = 0;
        while frame < n_frames {
            match self.fade {
                Fade::Idle => {
                    self.stages.process(&mut f32_buf[frame * N_CHANNELS..]);
                    frame = n_frames;
                }
                Fade::Out { pos } => {
                    let take = (self.ramp_frames - pos).min(n_frames - frame);
                    let span = &mut f32_buf[frame * N_CHANNELS..(frame + take) * N_CHANNELS];
                    self.stages.process(span);
                    for (i, f) in span.chunks_exact_mut(N_CHANNELS).enumerate() {
                        let g = rise(self.ramp_frames - (pos + i + 1), self.ramp_frames);
                        f.iter_mut().for_each(|s| *s *= g);
                    }
                    frame += take;
                    if pos + take < self.ramp_frames {
                        self.fade = Fade::Out { pos: pos + take };
                    } else {
                        // Silent now: swap in fresh stages.
                        if let Some(tuning) = self.pending.take() {
                            self.stages = Stages::new(&tuning, self.sample_rate_hz);
                        }
                        self.fade = Fade::In { pos: 0, hold: self.stages.warm_up_frames() };
                    }
                }
                Fade::In { pos, hold } => {
                    let total = hold + self.ramp_frames;
                    let take = (total - pos).min(n_frames - frame);
                    let span = &mut f32_buf[frame * N_CHANNELS..(frame + take) * N_CHANNELS];
                    self.stages.process(span);
                    for (i, f) in span.chunks_exact_mut(N_CHANNELS).enumerate() {
                        let p = pos + i;
                        let g = if p < hold { 0.0 } else { rise(p - hold + 1, self.ramp_frames) };
                        f.iter_mut().for_each(|s| *s *= g);
                    }
                    frame += take;
                    self.fade = if pos + take < total { Fade::In { pos: pos + take, hold } } else { Fade::Idle };
                }
            }
        }

        // Back to S16. Same scale both ways, so a stage set that changes
        // nothing round-trips exactly; the clamp catches numerical overshoot.
        for (dst, &src) in buf.iter_mut().zip(f32_buf.iter()) {
            *dst = (src * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
        }
        self.f32_buf = f32_buf;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::dsp::limiter::LimiterParams;
    use crate::playback::preset::{media_preset, voice_preset};

    const FS: f32 = 48_000.0;
    /// 5 ms at 48 kHz.
    const RAMP: usize = 240;
    const LOOKAHEAD: usize = LimiterParams::DEFAULT.lookahead_samples;

    /// `frames` stereo frames of a constant.
    fn dc(frames: usize, value: i16) -> Vec<i16> {
        vec![value; frames * 2]
    }

    fn sine(frames: usize, freq: f32, amp: f32) -> Vec<i16> {
        (0..frames)
            .flat_map(|i| {
                let v = (amp * (2.0 * std::f32::consts::PI * freq * i as f32 / FS).sin() * 32767.0) as i16;
                [v, v]
            })
            .collect()
    }

    fn left(buf: &[i16]) -> Vec<i16> {
        buf.iter().step_by(2).copied().collect()
    }

    #[test]
    fn media_chain_is_bit_exact() {
        let mut chain = PlaybackDspChain::new(&media_preset(), FS);
        let mut buf: Vec<i16> = (0..4096).map(|i| ((i * 7919) % 65536) as u16 as i16).collect();
        buf[0] = i16::MIN;
        buf[1] = i16::MAX;
        let before = buf.clone();
        chain.process_i16(&mut buf);
        assert_eq!(buf, before);
        assert_eq!(chain.last_limiter_reduction_db(), 0.0);
    }

    #[test]
    fn s16_round_trip_through_f32_is_exact() {
        // An active chain whose stages leave this signal alone: a limiter
        // far above the signal. Past the look-ahead the output is the input,
        // delayed, sample for sample.
        let mut tuning = media_preset();
        tuning.limiter = Some(LimiterParams { ceiling_dbfs: 0.0, ..LimiterParams::DEFAULT });
        let mut chain = PlaybackDspChain::new(&tuning, FS);
        let input = sine(2048, 997.0, 0.5);
        let mut buf = input.clone();
        chain.process_i16(&mut buf);
        assert_eq!(&buf[LOOKAHEAD * 2..], &input[..input.len() - LOOKAHEAD * 2]);
    }

    #[test]
    fn voice_chain_shapes_the_signal() {
        let mut chain = PlaybackDspChain::new(voice_preset(), FS);
        let input = sine(4800, 3500.0, 0.1);
        let mut buf = input.clone();
        chain.process_i16(&mut buf);
        // +5 dB at 3.5 kHz once the filters and the look-ahead settled.
        let rms = |b: &[i16]| (b.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / b.len() as f64).sqrt();
        let gain_db = 20.0 * (rms(&buf[4800..]) / rms(&input[4800..])).log10();
        assert!((gain_db - 5.0).abs() < 0.5, "got {gain_db:.2} dB at 3.5 kHz");
    }

    #[test]
    fn a_switch_fades_out_swaps_at_silence_and_fades_in() {
        let mut chain = PlaybackDspChain::new(&media_preset(), FS);
        chain.set_preset(&media_preset());
        assert!(chain.is_switching());
        let mut buf = dc(1024, 16384);
        chain.process_i16(&mut buf);
        let l = left(&buf);
        assert!(l[0] > 16300, "starts at full level: {}", l[0]);
        assert_eq!(l[RAMP - 1], 0, "silent at the swap");
        for w in l[..RAMP].windows(2) {
            assert!(w[1] <= w[0], "fade-out must not rise: {w:?}");
        }
        for w in l[RAMP..2 * RAMP].windows(2) {
            assert!(w[1] >= w[0], "fade-in must not fall: {w:?}");
        }
        assert!(l[2 * RAMP - 1] >= 16383, "back to full level: {}", l[2 * RAMP - 1]);
        assert!(l[2 * RAMP..].iter().all(|&s| s == 16384));
        assert!(!chain.is_switching());
    }

    #[test]
    fn a_switch_into_a_limited_tuning_stays_silent_through_the_look_ahead() {
        let mut chain = PlaybackDspChain::new(&media_preset(), FS);
        chain.set_preset(voice_preset());
        let mut buf = sine(2048, 1000.0, 0.3);
        chain.process_i16(&mut buf);
        let l = left(&buf);
        assert!(l[RAMP..RAMP + LOOKAHEAD].iter().all(|&s| s == 0), "limiter fill must be silent");
        let after = &l[2 * RAMP + LOOKAHEAD..];
        assert!(after.iter().any(|&s| s.abs() > 5000), "new chain audible after the fade-in");
    }

    #[test]
    fn a_switch_during_a_fade_in_turns_around_without_a_jump() {
        let mut chain = PlaybackDspChain::new(&media_preset(), FS);
        chain.set_preset(&media_preset());
        let mut first = dc(RAMP + RAMP / 2, 16384);
        chain.process_i16(&mut first);
        let last = *first.last().unwrap();
        chain.set_preset(&media_preset());
        let mut second = dc(RAMP, 16384);
        chain.process_i16(&mut second);
        let next = second[0];
        assert!((last - next).abs() < 16384 / 50, "jump from {last} to {next}");
        assert_eq!(left(&second)[RAMP / 2 - 1], 0, "silent at the swap");
    }

    #[test]
    fn output_does_not_depend_on_how_the_stream_is_cut() {
        let input = sine(4096, 440.0, 0.4);
        let run = |batch: usize| {
            let mut chain = PlaybackDspChain::new(voice_preset(), FS);
            let mut out = Vec::new();
            for (i, chunk) in input.chunks(batch * 2).enumerate() {
                if i == 3 {
                    chain.set_preset(&media_preset());
                }
                let mut b = chunk.to_vec();
                chain.process_i16(&mut b);
                out.extend(b);
            }
            out
        };
        // The switch lands at frame 3·batch, so compare cuts that share it.
        assert_eq!(run(64), {
            let mut chain = PlaybackDspChain::new(voice_preset(), FS);
            let mut out = Vec::new();
            let (head, tail) = input.split_at(3 * 64 * 2);
            let mut h = head.to_vec();
            chain.process_i16(&mut h);
            out.extend(h);
            chain.set_preset(&media_preset());
            let mut t = tail.to_vec();
            chain.process_i16(&mut t);
            out.extend(t);
            out
        });
    }

    #[test]
    fn limiter_still_engages_on_a_hot_voice_signal() {
        let mut chain = PlaybackDspChain::new(voice_preset(), FS);
        let mut hot: Vec<i16> = (0..2048).flat_map(|i| {
            let s = if i % 2 == 0 { 32767 } else { -32767 };
            [s, s]
        }).collect();
        chain.process_i16(&mut hot);
        assert!(chain.last_limiter_reduction_db() < -0.5,
            "expected the limiter to engage, got {:.2} dB", chain.last_limiter_reduction_db());
        let peak = hot.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak <= 29_300, "ceiling −1 dBFS, got peak {peak}");
    }

    #[test]
    fn a_switch_replaces_the_limiter() {
        let mut chain = PlaybackDspChain::new(voice_preset(), FS);
        let mut hot: Vec<i16> = (0..2048).flat_map(|i| {
            let s = if i % 2 == 0 { 32767 } else { -32767 };
            [s, s]
        }).collect();
        chain.process_i16(&mut hot);
        assert!(chain.last_limiter_reduction_db() < -0.5);
        chain.set_preset(&media_preset());
        let mut quiet = dc(1024, 100);
        chain.process_i16(&mut quiet);
        assert_eq!(chain.last_limiter_reduction_db(), 0.0, "media has no limiter");
    }
}
