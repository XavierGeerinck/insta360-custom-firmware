//! Microphone → phone: the processed uplink to `hw:0,0` playback while
//! `BluetoothState::call_audio` is set.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};
use waved_state::BluetoothState;

use super::{BtAudioStats, Paths, PlaybackStream, StreamError};
use crate::cushion::Cushion;
use crate::processed_frame::ProcessedAudioFrame;
use crate::repeat_filter::RepeatFilter;

/// About one second of processed frames at the 1024-frame period: how long
/// a failed open waits before the next try.
pub const RETRY_FRAMES: u32 = 47;

#[derive(Debug, Clone)]
pub struct UplinkParams {
    /// Playback channels (2 on TDM-A); the mono uplink goes on each.
    pub channels: usize,
    /// The ALSA buffer in periods; sizes the start-up cushion.
    pub buffer_periods: u32,
    /// Frames to skip after a failed open before trying again.
    pub retry_frames: u32,
    /// `bt.playback_clock`: run playback during phone audio too.
    pub playback_clock: bool,
}

/// What one [`PhoneUplink::frame`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UplinkStep {
    /// Not in a call: playback closed.
    Idle,
    /// The frame (uplink or silence) went out.
    Wrote,
    /// The frame was not written (waiting to retry an open, or no room).
    Skipped,
    /// An xrun was recovered; the frame was lost.
    Recovered,
    /// The open failed or the stream broke.
    Failed,
}

pub struct PhoneUplink<P, O> {
    open: O,
    stats: Arc<BtAudioStats>,
    params: UplinkParams,
    pcm: Option<P>,
    cushion: Cushion,
    out: Vec<i16>,
    silence: Vec<i16>,
    failures: RepeatFilter,
    /// Frames left to skip before the next open after a failure.
    cooldown: u32,
    /// The playback is wanted and its start was logged; reopens log quietly.
    session: bool,
}

impl<P, O> PhoneUplink<P, O>
where
    P: PlaybackStream,
    O: FnMut() -> Result<P, String>,
{
    pub fn new(open: O, stats: Arc<BtAudioStats>, params: UplinkParams) -> Self {
        let cushion = Cushion::new(params.buffer_periods);
        Self {
            open,
            stats,
            params,
            pcm: None,
            cushion,
            out: Vec::new(),
            silence: Vec::new(),
            failures: RepeatFilter::default(),
            cooldown: 0,
            session: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.pcm.is_some()
    }

    pub fn playback_clock(&self) -> bool {
        self.params.playback_clock
    }

    /// `n` processed frames never arrived (the broadcast lagged); they count
    /// as uplink drops while the playback is open.
    pub fn lost(&mut self, n: u64) {
        if self.is_open() {
            self.stats.mic_drops.fetch_add(n, Ordering::Relaxed);
        }
    }

    /// One processed frame: open or close the playback to match `paths`,
    /// then write the uplink (a call) or silence (the clock switch).
    pub fn frame(&mut self, paths: Paths, frame: &ProcessedAudioFrame) -> UplinkStep {
        if !paths.playback {
            if self.pcm.is_some() {
                self.close();
                info!("bt-uplink: call audio off; hw:0,0 playback closed");
            }
            self.session = false;
            self.cooldown = 0;
            self.failures.clear();
            return UplinkStep::Idle;
        }
        if self.pcm.is_none() {
            if self.cooldown > 0 {
                self.cooldown -= 1;
                return UplinkStep::Skipped;
            }
            match (self.open)() {
                Ok(pcm) => {
                    self.pcm = Some(pcm);
                    self.cushion.restarted();
                    self.stats.playback_open.store(true, Ordering::Relaxed);
                    if self.session {
                        debug!("bt-uplink: hw:0,0 playback reopened");
                    } else {
                        self.session = true;
                        let why = if paths.mic { "call audio on" } else { "clock for phone audio" };
                        info!("bt-uplink: {why}; hw:0,0 playback open");
                    }
                }
                Err(e) => {
                    self.stats.open_failures.fetch_add(1, Ordering::Relaxed);
                    self.fail(format!("open failed: {e}"));
                    return UplinkStep::Failed;
                }
            }
        }

        let channels = self.params.channels.max(1);
        if paths.mic {
            frame.mono_interleaved_i16(channels, &mut self.out);
        } else {
            self.out.clear();
            self.out.resize(frame.mic_mono_f32.len() * channels, 0);
        }
        self.stats.mic_active.store(paths.mic, Ordering::Relaxed);
        if self.silence.len() != self.out.len() {
            self.silence = vec![0; self.out.len()];
        }

        let Some(pcm) = self.pcm.as_mut() else {
            return UplinkStep::Failed;
        };
        for _ in 0..self.cushion.take() {
            // An error here shows up on the frame's write below.
            let _ = pcm.write(&self.silence);
        }
        match pcm.write(&self.out) {
            Ok(frames) => {
                self.stats.playback_frames.fetch_add(frames as u64, Ordering::Relaxed);
                if self.failures.clear() {
                    info!("bt-uplink: writing to the phone again");
                }
                UplinkStep::Wrote
            }
            Err(StreamError::Again) => {
                self.stats.mic_drops.fetch_add(1, Ordering::Relaxed);
                UplinkStep::Skipped
            }
            Err(StreamError::Xrun) => {
                self.stats.playback_xruns.fetch_add(1, Ordering::Relaxed);
                match pcm.recover() {
                    Ok(()) => {
                        self.cushion.restarted();
                        UplinkStep::Recovered
                    }
                    Err(e) => self.broke(format!("recovery failed: {e}")),
                }
            }
            Err(StreamError::Fatal(e)) => self.broke(format!("write stopped: {e}")),
        }
    }

    fn broke(&mut self, failure: String) -> UplinkStep {
        self.close();
        self.fail(failure);
        UplinkStep::Failed
    }

    fn close(&mut self) {
        self.pcm = None;
        self.stats.playback_open.store(false, Ordering::Relaxed);
        self.stats.mic_active.store(false, Ordering::Relaxed);
    }

    fn fail(&mut self, failure: String) {
        self.cooldown = self.params.retry_frames;
        if self.failures.is_new(&failure) {
            warn!("bt-uplink {failure}; retrying every second, quietly until it changes");
        } else {
            debug!("bt-uplink {failure}");
        }
    }
}

/// Run the uplink until the processed stream closes. Paced by the processed
/// frames (one per capture period), so the Bluetooth state is re-read every
/// period; the audio worker never waits on it (broadcast).
pub fn run<P, O>(
    mut uplink: PhoneUplink<P, O>,
    mut frames: broadcast::Receiver<ProcessedAudioFrame>,
    state: watch::Receiver<BluetoothState>,
) where
    P: PlaybackStream,
    O: FnMut() -> Result<P, String>,
{
    loop {
        match frames.blocking_recv() {
            Ok(frame) => {
                let paths = Paths::from_state(&state.borrow(), uplink.playback_clock());
                uplink.frame(paths, &frame);
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                uplink.lost(n);
                debug!("bt-uplink lagged {n} frames");
            }
            Err(broadcast::error::RecvError::Closed) => {
                info!("bt-uplink: the processed stream is gone; stopping");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bt_audio::fake::{FakeDevice, FakeStream};
    use crate::bt_audio::StreamError;
    use crate::callbacks::AudioFrame;
    use crate::cushion::PREFILL_PERIODS;
    use std::sync::atomic::Ordering;

    type Up = PhoneUplink<FakeStream, Box<dyn FnMut() -> Result<FakeStream, String> + Send>>;

    const CALL: Paths = Paths { speaker: true, playback: true, mic: true };
    const MUSIC_WITH_CLOCK: Paths = Paths { speaker: true, playback: true, mic: false };
    const NONE: Paths = Paths { speaker: false, playback: false, mic: false };

    fn setup(retry_frames: u32) -> (Up, FakeDevice, Arc<BtAudioStats>) {
        let device = FakeDevice::new(2);
        let stats = BtAudioStats::new();
        let up = PhoneUplink::new(
            Box::new(device.playback_opener()) as Box<dyn FnMut() -> Result<FakeStream, String> + Send>,
            Arc::clone(&stats),
            UplinkParams { channels: 2, buffer_periods: 4, retry_frames, playback_clock: false },
        );
        (up, device, stats)
    }

    /// A processed frame whose mono uplink is `mono` (i16 values) and whose
    /// Stereo pair is something else.
    fn frame(mono: &[i16]) -> ProcessedAudioFrame {
        let raw = AudioFrame::mic_only(vec![0; mono.len()], 1, 48_000, 0);
        let n = mono.len();
        ProcessedAudioFrame::from_raw(&raw, mono, &vec![0; n]).with_stereo(vec![0.9; n], vec![-0.9; n])
    }

    #[test]
    fn nothing_opens_outside_a_call() {
        let (mut up, device, stats) = setup(2);
        assert_eq!(up.frame(NONE, &frame(&[1, 2])), UplinkStep::Idle);
        let music = Paths { speaker: true, playback: false, mic: false };
        assert_eq!(up.frame(music, &frame(&[1, 2])), UplinkStep::Idle);
        assert_eq!(device.opens(), 0);
        assert!(!stats.playback_open.load(Ordering::Relaxed));
    }

    #[test]
    fn a_call_opens_the_playback_and_sends_the_cushion_then_the_mono_uplink() {
        let (mut up, device, stats) = setup(2);
        assert_eq!(up.frame(CALL, &frame(&[16_384, -8_192])), UplinkStep::Wrote);
        assert!(up.is_open());
        let writes = device.writes();
        let cushion = PREFILL_PERIODS as usize;
        assert_eq!(writes.len(), cushion + 1);
        assert!(writes[..cushion].iter().all(|w| w == &vec![0; 4]), "silent cushion periods");
        assert_eq!(writes[cushion], vec![16_383, 16_383, -8_191, -8_191], "mono on both channels");
        assert!(stats.playback_open.load(Ordering::Relaxed));
        assert!(stats.mic_active.load(Ordering::Relaxed));
        assert_eq!(stats.playback_frames.load(Ordering::Relaxed), 2);

        assert_eq!(up.frame(CALL, &frame(&[0, 0])), UplinkStep::Wrote);
        assert_eq!(device.writes().len(), cushion + 2, "the cushion only once");
    }

    #[test]
    fn the_call_ending_closes_the_playback() {
        let (mut up, device, stats) = setup(2);
        up.frame(CALL, &frame(&[1, 2]));
        assert_eq!(up.frame(NONE, &frame(&[1, 2])), UplinkStep::Idle);
        assert!(!up.is_open());
        assert_eq!(device.closes(), 1);
        assert!(!stats.playback_open.load(Ordering::Relaxed));
        assert!(!stats.mic_active.load(Ordering::Relaxed));

        up.frame(CALL, &frame(&[1, 2]));
        assert_eq!(device.opens(), 2);
        let cushion = PREFILL_PERIODS as usize;
        assert_eq!(device.writes().len(), 2 * (cushion + 1), "a fresh cushion per call");
    }

    #[test]
    fn the_clock_switch_writes_silence_while_music_plays() {
        let (mut up, device, stats) = setup(2);
        assert_eq!(up.frame(MUSIC_WITH_CLOCK, &frame(&[9_000, 9_000])), UplinkStep::Wrote);
        assert_eq!(device.writes().last(), Some(&vec![0; 4]), "never the mic outside a call");
        assert!(stats.playback_open.load(Ordering::Relaxed));
        assert!(!stats.mic_active.load(Ordering::Relaxed));
        // The call starts on the open stream: no reopen, the mic goes out.
        up.frame(CALL, &frame(&[16_384, 16_384]));
        assert_eq!(device.opens(), 1);
        assert_eq!(device.writes().last(), Some(&vec![16_383; 4]));
        assert!(stats.mic_active.load(Ordering::Relaxed));
    }

    #[test]
    fn an_xrun_recovers_and_cushions_again() {
        let (mut up, device, stats) = setup(2);
        up.frame(CALL, &frame(&[1, 2]));
        let before = device.writes().len();
        device.push_write_result(Err(StreamError::Xrun));
        assert_eq!(up.frame(CALL, &frame(&[3, 4])), UplinkStep::Recovered);
        assert_eq!(stats.playback_xruns.load(Ordering::Relaxed), 1);
        assert_eq!(device.recovers(), 1);
        up.frame(CALL, &frame(&[5, 6]));
        let cushion = PREFILL_PERIODS as usize;
        assert_eq!(device.writes().len(), before + cushion + 1, "a cushion after the restart");
        assert_eq!(device.opens(), 1);
    }

    #[test]
    fn no_room_drops_the_frame_and_counts_it() {
        let (mut up, device, stats) = setup(2);
        up.frame(CALL, &frame(&[1, 2]));
        device.push_write_result(Err(StreamError::Again));
        assert_eq!(up.frame(CALL, &frame(&[3, 4])), UplinkStep::Skipped);
        assert_eq!(stats.mic_drops.load(Ordering::Relaxed), 1);
        assert!(up.is_open());
    }

    #[test]
    fn an_open_failure_waits_before_the_next_try() {
        let (mut up, device, stats) = setup(2);
        device.fail_next_open("Device or resource busy");
        assert_eq!(up.frame(CALL, &frame(&[1, 2])), UplinkStep::Failed);
        assert_eq!(stats.open_failures.load(Ordering::Relaxed), 1);
        assert_eq!(up.frame(CALL, &frame(&[1, 2])), UplinkStep::Skipped);
        assert_eq!(up.frame(CALL, &frame(&[1, 2])), UplinkStep::Skipped);
        assert_eq!(up.frame(CALL, &frame(&[1, 2])), UplinkStep::Wrote);
        assert_eq!(device.opens(), 1);
    }

    #[test]
    fn a_broken_stream_closes_and_waits() {
        let (mut up, device, stats) = setup(1);
        up.frame(CALL, &frame(&[1, 2]));
        device.push_write_result(Err(StreamError::Fatal("EIO".into())));
        assert_eq!(up.frame(CALL, &frame(&[1, 2])), UplinkStep::Failed);
        assert_eq!(device.closes(), 1);
        assert!(!stats.playback_open.load(Ordering::Relaxed));
        assert_eq!(up.frame(CALL, &frame(&[1, 2])), UplinkStep::Skipped);
        assert_eq!(up.frame(CALL, &frame(&[1, 2])), UplinkStep::Wrote);
        assert_eq!(device.opens(), 2);
    }

    #[test]
    fn a_failed_recovery_closes_and_waits() {
        let (mut up, device, _) = setup(1);
        up.frame(CALL, &frame(&[1, 2]));
        device.push_write_result(Err(StreamError::Xrun));
        device.fail_next_recover("EBADFD");
        assert_eq!(up.frame(CALL, &frame(&[1, 2])), UplinkStep::Failed);
        assert_eq!(device.closes(), 1);
    }

    #[test]
    fn the_runner_writes_during_a_call_and_ends_with_the_audio() {
        let (up, device, _) = setup(2);
        let (frames_tx, frames_rx) = broadcast::channel(16);
        let (state_tx, state_rx) = watch::channel(BluetoothState {
            phone_audio: true,
            call_audio: true,
            ..BluetoothState::default()
        });
        frames_tx.send(frame(&[16_384, 16_384])).unwrap();
        frames_tx.send(frame(&[0, 0])).unwrap();
        drop(frames_tx);
        run(up, frames_rx, state_rx);
        let writes = device.writes();
        let cushion = PREFILL_PERIODS as usize;
        assert_eq!(writes.len(), cushion + 2);
        assert_eq!(writes[cushion], vec![16_383; 4]);
        drop(state_tx);
    }
}
