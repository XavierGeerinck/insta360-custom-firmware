//! Bluetooth phone audio (I2): the Telink module's TDM-A link, `hw:0,0`
//! (48 kHz, 2 ch, S16_LE both ways, the SoC is the clock master). See
//! `docs/AUDIO_HARDWARE.md` "Bluetooth audio (I2)" and
//! `docs/BLUETOOTH_PROTOCOL.md` "Audio (I2)".
//!
//! - **Phone → speaker** (`downstream`): while `BluetoothState::phone_audio`
//!   (`0xB2` bit 1), `hw:0,0` capture feeds `PlaybackService::play_phone`,
//!   which mixes it with the USB host's audio before the speaker DSP, so the
//!   volume, EQ, limiter and the echo canceller's reference cover it, and
//!   recordings get it as the far end.
//! - **Microphone → phone** (`uplink`): while `call_audio` (bit 2), the
//!   processed uplink (the USB microphone's signal, mono) goes to `hw:0,0`
//!   playback on both channels.
//!
//! The logic runs against the [`CaptureStream`] / [`PlaybackStream`] traits;
//! `alsa` (feature `capture-alsa`) supplies the real streams and threads.

pub mod downstream;
pub mod uplink;

#[cfg(feature = "capture-alsa")]
pub mod alsa;

#[cfg(test)]
mod fake;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use waved_state::BluetoothState;

use crate::playback::{PlaybackError, PlaybackService};

/// Which `hw:0,0` streams the Bluetooth state asks for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Paths {
    /// Capture open: phone audio plays on the speaker (`phone_audio`).
    pub speaker: bool,
    /// Playback open: in a call, or during phone audio with the clock switch.
    pub playback: bool,
    /// The uplink goes to the open playback (a call); silence otherwise.
    pub mic: bool,
}

impl Paths {
    /// `playback_clock` (`bt.playback_clock` in the audio config) keeps
    /// `hw:0,0` playback running whenever phone audio plays, writing silence
    /// outside a call, in case TDM-A capture needs it for its clocks.
    pub fn from_state(state: &BluetoothState, playback_clock: bool) -> Self {
        let speaker = state.phone_audio;
        let mic = state.call_audio;
        Self { speaker, mic, playback: mic || (playback_clock && speaker) }
    }
}

/// Why a stream call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamError {
    /// Nothing to read or no room yet (EAGAIN); try again shortly.
    Again,
    /// An overrun or underrun (EPIPE): count it, then `recover`.
    Xrun,
    /// Anything else: close the stream and open it again later.
    Fatal(String),
}

/// `hw:0,0` capture: audio from the phone.
pub trait CaptureStream {
    /// Read interleaved samples into `buf`; returns the frames read.
    fn read(&mut self, buf: &mut [i16]) -> Result<usize, StreamError>;
    /// Restart after an xrun.
    fn recover(&mut self) -> Result<(), String>;
}

/// `hw:0,0` playback: audio to the phone.
pub trait PlaybackStream {
    /// Write all of `buf` (interleaved); returns the frames written.
    fn write(&mut self, buf: &[i16]) -> Result<usize, StreamError>;
    /// Restart after an xrun.
    fn recover(&mut self) -> Result<(), String>;
}

/// The speaker path as the downstream sees it (`PlaybackService` in
/// production).
pub trait PhoneSpeaker {
    /// Queue one speaker period; `Err(Full)` drops it, any other error means
    /// the speaker is gone.
    fn submit(&self, period: Vec<i16>) -> Result<(), PlaybackError>;
    /// Periods queued and not played yet.
    fn queued(&self) -> usize;
    fn channels(&self) -> usize;
    fn period_frames(&self) -> usize;
}

impl PhoneSpeaker for PlaybackService {
    fn submit(&self, period: Vec<i16>) -> Result<(), PlaybackError> {
        self.play_phone(period)
    }
    fn queued(&self) -> usize {
        self.phone_queue_depth()
    }
    fn channels(&self) -> usize {
        PlaybackService::channels(self) as usize
    }
    fn period_frames(&self) -> usize {
        PlaybackService::period_frames(self)
    }
}

impl<T: PhoneSpeaker + ?Sized> PhoneSpeaker for Arc<T> {
    fn submit(&self, period: Vec<i16>) -> Result<(), PlaybackError> {
        (**self).submit(period)
    }
    fn queued(&self) -> usize {
        (**self).queued()
    }
    fn channels(&self) -> usize {
        (**self).channels()
    }
    fn period_frames(&self) -> usize {
        (**self).period_frames()
    }
}

/// Append `src` (interleaved, `from` channels) to `out` as `to` channels: a
/// copy when they match, the average for mono, otherwise channel `c` takes
/// source channel `c % from`.
pub fn remix_into(src: &[i16], from: usize, to: usize, out: &mut Vec<i16>) {
    let (from, to) = (from.max(1), to.max(1));
    if from == to {
        out.extend_from_slice(src);
        return;
    }
    for frame in src.chunks_exact(from) {
        if to == 1 {
            let sum: i32 = frame.iter().map(|&s| i32::from(s)).sum();
            out.push((sum / from as i32) as i16);
        } else {
            out.extend((0..to).map(|c| frame[c % from]));
        }
    }
}

/// Counters for `/tmp/audio_stats.json` (`bt_audio`). Each path writes its
/// own subset.
#[derive(Debug, Default)]
pub struct BtAudioStats {
    /// `hw:0,0` capture is open: phone audio → speaker.
    pub speaker_active: AtomicBool,
    /// `hw:0,0` playback is open.
    pub playback_open: AtomicBool,
    /// The uplink is going to the phone (a call).
    pub mic_active: AtomicBool,
    /// Frames read from the phone.
    pub capture_frames: AtomicU64,
    /// Uplink frames written to the phone (silence included outside a call).
    pub playback_frames: AtomicU64,
    pub capture_xruns: AtomicU64,
    pub playback_xruns: AtomicU64,
    /// Speaker periods dropped because the phone queue was full.
    pub speaker_drops: AtomicU64,
    /// Uplink frames lost while the playback was open (late broadcast or
    /// no room).
    pub mic_drops: AtomicU64,
    /// Failed opens of either stream.
    pub open_failures: AtomicU64,
}

impl BtAudioStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn snapshot(&self) -> BtAudioStatsSnapshot {
        let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
        BtAudioStatsSnapshot {
            speaker_active: self.speaker_active.load(Ordering::Relaxed),
            playback_open: self.playback_open.load(Ordering::Relaxed),
            mic_active: self.mic_active.load(Ordering::Relaxed),
            capture_frames: get(&self.capture_frames),
            playback_frames: get(&self.playback_frames),
            capture_xruns: get(&self.capture_xruns),
            playback_xruns: get(&self.playback_xruns),
            speaker_drops: get(&self.speaker_drops),
            mic_drops: get(&self.mic_drops),
            open_failures: get(&self.open_failures),
        }
    }
}

/// What the stats writer emits as `bt_audio`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BtAudioStatsSnapshot {
    pub speaker_active: bool,
    pub playback_open: bool,
    pub mic_active: bool,
    pub capture_frames: u64,
    pub playback_frames: u64,
    pub capture_xruns: u64,
    pub playback_xruns: u64,
    pub speaker_drops: u64,
    pub mic_drops: u64,
    pub open_failures: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(phone_audio: bool, call_audio: bool) -> BluetoothState {
        BluetoothState { connected: true, phone_audio, call_audio, ..BluetoothState::default() }
    }

    #[test]
    fn the_paths_follow_the_audio_bits() {
        assert_eq!(Paths::from_state(&state(false, false), false), Paths::default());
        assert_eq!(
            Paths::from_state(&state(true, false), false),
            Paths { speaker: true, playback: false, mic: false },
            "music: the speaker only"
        );
        assert_eq!(
            Paths::from_state(&state(true, true), false),
            Paths { speaker: true, playback: true, mic: true },
            "a call: both ways"
        );
    }

    #[test]
    fn the_clock_switch_runs_playback_with_music_but_sends_the_mic_only_in_a_call() {
        assert_eq!(
            Paths::from_state(&state(true, false), true),
            Paths { speaker: true, playback: true, mic: false }
        );
        assert_eq!(Paths::from_state(&state(false, false), true), Paths::default());
        assert_eq!(
            Paths::from_state(&state(true, true), true),
            Paths { speaker: true, playback: true, mic: true }
        );
    }

    #[test]
    fn remix_copies_averages_or_spreads_channels() {
        let mut out = Vec::new();
        remix_into(&[1, 2, 3, 4], 2, 2, &mut out);
        assert_eq!(out, vec![1, 2, 3, 4]);
        out.clear();
        remix_into(&[100, 300, -100, -301], 2, 1, &mut out);
        assert_eq!(out, vec![200, -200]);
        out.clear();
        remix_into(&[5, -5], 1, 2, &mut out);
        assert_eq!(out, vec![5, 5, -5, -5]);
    }

    #[test]
    fn stats_start_at_zero_and_show_writes() {
        let s = BtAudioStats::default();
        assert_eq!(
            s.snapshot(),
            BtAudioStatsSnapshot {
                speaker_active: false,
                playback_open: false,
                mic_active: false,
                capture_frames: 0,
                playback_frames: 0,
                capture_xruns: 0,
                playback_xruns: 0,
                speaker_drops: 0,
                mic_drops: 0,
                open_failures: 0,
            }
        );
        s.speaker_active.store(true, Ordering::Relaxed);
        s.capture_frames.fetch_add(1024, Ordering::Relaxed);
        s.mic_drops.fetch_add(2, Ordering::Relaxed);
        let snap = s.snapshot();
        assert!(snap.speaker_active && !snap.mic_active);
        assert_eq!((snap.capture_frames, snap.mic_drops), (1024, 2));
        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["capture_frames"], 1024);
    }
}
