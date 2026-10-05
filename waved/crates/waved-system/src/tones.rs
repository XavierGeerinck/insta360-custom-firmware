//! Recording start and stop tones (TODO E5), so everyone in the room hears
//! when recording begins and ends. Synthesised here and played through a
//! `ToneSink`; the integrator's sink feeds waved-audio's playback path.

use std::f32::consts::PI;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::debug;
use waved_state::{MeetingState, SettingsState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    RecordingStarted,
    RecordingStopped,
}

/// The playback path's format: interleaved i16 at `sample_rate`, submitted
/// one period of `period_frames` frames at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: usize,
    pub period_frames: usize,
}

pub trait ToneSink: Send + Sync {
    fn format(&self) -> PcmFormat;
    /// Queue one period. False when playback stopped or its queue is full.
    fn submit(&self, period: Vec<i16>) -> bool;
}

/// Peak level as a fraction of full scale (about −8 dBFS before the speaker volume).
pub const TONE_PEAK: f32 = 0.4;
const FADE_MS: u64 = 8;
const LOW_HZ: f32 = 659.25; // E5
const HIGH_HZ: f32 = 880.0; // A5

impl Tone {
    /// (frequency in Hz, length in ms): rising for start, falling for stop.
    pub fn notes(self) -> [(f32, u32); 2] {
        match self {
            Tone::RecordingStarted => [(LOW_HZ, 110), (HIGH_HZ, 160)],
            Tone::RecordingStopped => [(HIGH_HZ, 110), (LOW_HZ, 160)],
        }
    }
}

/// Interleaved samples, every channel the same, padded with silence to whole periods.
pub fn render(tone: Tone, format: PcmFormat) -> Vec<i16> {
    let rate = u64::from(format.sample_rate.max(1));
    let mut mono = Vec::new();
    for (hz, ms) in tone.notes() {
        let len = (rate * u64::from(ms) / 1_000) as usize;
        let fade = ((rate * FADE_MS / 1_000) as usize).max(1);
        for i in 0..len {
            let edge = i.min(len - 1 - i);
            let gain = if edge < fade {
                0.5 - 0.5 * (PI * edge as f32 / fade as f32).cos()
            } else {
                1.0
            };
            let phase = 2.0 * PI * hz * i as f32 / rate as f32;
            mono.push((phase.sin() * gain * TONE_PEAK * f32::from(i16::MAX)) as i16);
        }
    }
    let period = format.period_frames.max(1);
    let channels = format.channels.max(1);
    let frames = mono.len().div_ceil(period) * period;
    let mut out = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        out.extend(std::iter::repeat_n(mono.get(i).copied().unwrap_or(0), channels));
    }
    out
}

/// The tone a change of the recording flag calls for. Pausing is not a change.
pub fn tone_for(was_recording: bool, recording: bool) -> Option<Tone> {
    match (was_recording, recording) {
        (false, true) => Some(Tone::RecordingStarted),
        (true, false) => Some(Tone::RecordingStopped),
        _ => None,
    }
}

/// Plays the start and stop tones while `tones_enabled`. Runs until the
/// meeting sender is dropped.
pub async fn run(
    mut meeting: watch::Receiver<MeetingState>,
    settings: watch::Receiver<SettingsState>,
    sink: Arc<dyn ToneSink>,
) {
    let mut was_recording = meeting.borrow_and_update().recording;
    while meeting.changed().await.is_ok() {
        let recording = meeting.borrow_and_update().recording;
        let Some(tone) = tone_for(was_recording, recording) else {
            continue;
        };
        was_recording = recording;
        if settings.borrow().tones_enabled {
            play(tone, sink.as_ref()).await;
        }
    }
}

async fn play(tone: Tone, sink: &dyn ToneSink) {
    let format = sink.format();
    let samples = render(tone, format);
    let period = format.period_frames.max(1) * format.channels.max(1);
    let pace = Duration::from_micros(format.period_frames as u64 * 1_000_000 / u64::from(format.sample_rate.max(1)));
    for chunk in samples.chunks(period) {
        if !sink.submit(chunk.to_vec()) {
            debug!("{tone:?} tone cut short: playback stopped or its queue is full");
            return;
        }
        tokio::time::sleep(pace).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const FORMAT: PcmFormat = PcmFormat {
        sample_rate: 48_000,
        channels: 2,
        period_frames: 480,
    };

    fn crossings(samples: &[i16]) -> usize {
        samples.windows(2).filter(|pair| (pair[0] < 0) != (pair[1] < 0)).count()
    }

    fn left(samples: &[i16]) -> Vec<i16> {
        samples.iter().step_by(2).copied().collect()
    }

    #[test]
    fn start_rises_and_stop_falls() {
        let (first, second) = (5_280, 5_280 + 7_680); // 110 ms and 160 ms at 48 kHz
        let rate = |samples: &[i16], from: usize, to: usize| crossings(&samples[from..to]) * 1_000 / (to - from);
        let start = left(&render(Tone::RecordingStarted, FORMAT));
        assert!(rate(&start, 0, first) < rate(&start, first, second));
        let stop = left(&render(Tone::RecordingStopped, FORMAT));
        assert!(rate(&stop, 0, first) > rate(&stop, first, second));
    }

    #[test]
    fn tones_stay_under_the_peak_start_silent_and_fill_whole_periods() {
        let samples = render(Tone::RecordingStarted, FORMAT);
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(f32::from(peak) <= TONE_PEAK * f32::from(i16::MAX) + 1.0, "{peak}");
        assert!(peak > 10_000, "audible: {peak}");
        assert_eq!(samples[0], 0);
        assert_eq!(samples.len() % (480 * 2), 0);
        assert_eq!(samples.len(), 27 * 480 * 2, "270 ms is 27 periods of 10 ms");
        assert!(samples.chunks(2).all(|frame| frame[0] == frame[1]), "both channels carry the tone");
    }

    #[test]
    fn only_a_change_of_the_recording_flag_plays_a_tone() {
        assert_eq!(tone_for(false, true), Some(Tone::RecordingStarted));
        assert_eq!(tone_for(true, false), Some(Tone::RecordingStopped));
        assert_eq!(tone_for(true, true), None);
        assert_eq!(tone_for(false, false), None);
    }

    struct FakeSink {
        periods: Mutex<Vec<Vec<i16>>>,
        accept: bool,
    }

    impl ToneSink for FakeSink {
        fn format(&self) -> PcmFormat {
            FORMAT
        }
        fn submit(&self, period: Vec<i16>) -> bool {
            self.periods.lock().unwrap().push(period);
            self.accept
        }
    }

    fn sink(accept: bool) -> Arc<FakeSink> {
        Arc::new(FakeSink {
            periods: Mutex::default(),
            accept,
        })
    }

    fn settle() -> tokio::time::Sleep {
        tokio::time::sleep(Duration::from_secs(1))
    }

    #[tokio::test(start_paused = true)]
    async fn the_service_plays_start_and_stop_and_honours_the_setting() {
        let sink = sink(true);
        let (meeting, meeting_rx) = watch::channel(MeetingState::default());
        let (settings, settings_rx) = watch::channel(SettingsState::default());
        tokio::spawn(run(meeting_rx, settings_rx, sink.clone()));
        tokio::task::yield_now().await; // let `run` read the starting state first
        meeting.send_modify(|m| m.recording = true);
        settle().await;
        assert_eq!(sink.periods.lock().unwrap().concat(), render(Tone::RecordingStarted, FORMAT));
        meeting.send_modify(|m| m.recording = false);
        settle().await;
        assert_eq!(sink.periods.lock().unwrap().len(), 2 * 27);
        settings.send_modify(|s| s.tones_enabled = false);
        meeting.send_modify(|m| m.recording = true);
        settle().await;
        meeting.send_modify(|m| m.recording = false);
        settle().await;
        assert_eq!(sink.periods.lock().unwrap().len(), 2 * 27, "tones off: silent");
    }

    #[tokio::test(start_paused = true)]
    async fn pausing_plays_nothing() {
        let sink = sink(true);
        let (meeting, meeting_rx) = watch::channel(MeetingState::default());
        let (_settings, settings_rx) = watch::channel(SettingsState::default());
        tokio::spawn(run(meeting_rx, settings_rx, sink.clone()));
        tokio::task::yield_now().await; // let `run` read the starting state first
        meeting.send_modify(|m| m.recording = true);
        settle().await;
        meeting.send_modify(|m| m.paused = true);
        settle().await;
        meeting.send_modify(|m| m.paused = false);
        settle().await;
        assert_eq!(sink.periods.lock().unwrap().len(), 27);
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_queue_cuts_the_tone_short() {
        let sink = sink(false);
        let (meeting, meeting_rx) = watch::channel(MeetingState::default());
        let (_settings, settings_rx) = watch::channel(SettingsState::default());
        tokio::spawn(run(meeting_rx, settings_rx, sink.clone()));
        tokio::task::yield_now().await; // let `run` read the starting state first
        meeting.send_modify(|m| m.recording = true);
        settle().await;
        assert_eq!(sink.periods.lock().unwrap().len(), 1);
    }
}
