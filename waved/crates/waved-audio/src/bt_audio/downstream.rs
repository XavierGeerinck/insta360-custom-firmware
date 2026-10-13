//! Phone → speaker: `hw:0,0` capture into `PlaybackService::play_phone`
//! while `BluetoothState::phone_audio` is set.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, info, warn};
use waved_state::BluetoothState;

use super::{remix_into, BtAudioStats, CaptureStream, Paths, PhoneSpeaker, StreamError};
use crate::playback::PlaybackError;
use crate::repeat_filter::RepeatFilter;

/// How the downstream reads and queues.
#[derive(Debug, Clone)]
pub struct DownstreamParams {
    /// Capture channels (2 on TDM-A).
    pub channels: usize,
    /// Frames per read (the capture period).
    pub read_frames: usize,
    /// A speaker period is dropped once this many phone periods are queued.
    pub drop_at: usize,
    /// Silent periods queued when the capture opens: a jitter cushion.
    pub prefill_periods: usize,
}

/// What one [`PhoneDownstream::step`] did; the runner sleeps accordingly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Phone audio is off and the capture is closed.
    Idle,
    /// Audio moved (or an xrun was recovered): go again at once.
    Moved,
    /// No data yet.
    Waiting,
    /// The open failed or the stream broke: retry later.
    Failed,
    /// The speaker path is gone: stop.
    Stopped,
}

pub struct PhoneDownstream<C, O, S> {
    open: O,
    speaker: S,
    stats: Arc<BtAudioStats>,
    params: DownstreamParams,
    pcm: Option<C>,
    /// One read, in capture channels.
    buf: Vec<i16>,
    /// Read but not yet a whole speaker period, in speaker channels.
    pending: Vec<i16>,
    failures: RepeatFilter,
    /// Phone audio is on and its start was logged; reopens log quietly.
    session: bool,
}

impl<C, O, S> PhoneDownstream<C, O, S>
where
    C: CaptureStream,
    O: FnMut() -> Result<C, String>,
    S: PhoneSpeaker,
{
    pub fn new(open: O, speaker: S, stats: Arc<BtAudioStats>, params: DownstreamParams) -> Self {
        let buf = vec![0; params.read_frames.max(1) * params.channels.max(1)];
        Self {
            open,
            speaker,
            stats,
            params,
            pcm: None,
            buf,
            pending: Vec::new(),
            failures: RepeatFilter::default(),
            session: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.pcm.is_some()
    }

    /// One pass: open or close the capture to match `want` (phone audio),
    /// then read once and queue every whole speaker period.
    pub fn step(&mut self, want: bool) -> Step {
        if !want {
            if self.pcm.is_some() {
                self.close();
                info!("bt-downstream: phone audio off; hw:0,0 capture closed");
            }
            self.session = false;
            self.failures.clear();
            return Step::Idle;
        }
        if self.pcm.is_none() {
            if let Some(step) = self.open() {
                return step;
            }
        }
        let Some(pcm) = self.pcm.as_mut() else {
            return Step::Failed;
        };
        match pcm.read(&mut self.buf) {
            Ok(0) | Err(StreamError::Again) => Step::Waiting,
            Ok(frames) => {
                self.stats.capture_frames.fetch_add(frames as u64, Ordering::Relaxed);
                if self.failures.clear() {
                    info!("bt-downstream: phone audio flowing again");
                }
                let samples = (frames * self.params.channels.max(1)).min(self.buf.len());
                remix_into(
                    &self.buf[..samples],
                    self.params.channels,
                    self.speaker.channels(),
                    &mut self.pending,
                );
                self.queue_periods()
            }
            Err(StreamError::Xrun) => {
                self.stats.capture_xruns.fetch_add(1, Ordering::Relaxed);
                match pcm.recover() {
                    Ok(()) => Step::Moved,
                    Err(e) => {
                        self.close();
                        self.fail(format!("recovery failed: {e}"));
                        Step::Failed
                    }
                }
            }
            Err(StreamError::Fatal(e)) => {
                self.close();
                self.fail(format!("read stopped: {e}"));
                Step::Failed
            }
        }
    }

    fn period_samples(&self) -> usize {
        (self.speaker.period_frames() * self.speaker.channels()).max(1)
    }

    /// Open the capture and queue the cushion; `Some(step)` when the step
    /// ends here.
    fn open(&mut self) -> Option<Step> {
        match (self.open)() {
            Ok(pcm) => {
                self.pcm = Some(pcm);
                self.pending.clear();
                self.stats.speaker_active.store(true, Ordering::Relaxed);
                if self.session {
                    debug!("bt-downstream: hw:0,0 capture reopened");
                } else {
                    self.session = true;
                    info!("bt-downstream: phone audio on; hw:0,0 capture open");
                }
                let silence = vec![0i16; self.period_samples()];
                for _ in 0..self.params.prefill_periods {
                    match self.speaker.submit(silence.clone()) {
                        Ok(()) | Err(PlaybackError::Full) => {}
                        Err(_) => return Some(Step::Stopped),
                    }
                }
                None
            }
            Err(e) => {
                self.stats.open_failures.fetch_add(1, Ordering::Relaxed);
                self.fail(format!("open failed: {e}"));
                Some(Step::Failed)
            }
        }
    }

    fn close(&mut self) {
        self.pcm = None;
        self.pending.clear();
        self.stats.speaker_active.store(false, Ordering::Relaxed);
    }

    fn fail(&mut self, failure: String) {
        if self.failures.is_new(&failure) {
            warn!("bt-downstream {failure}; retrying every second, quietly until it changes");
        } else {
            debug!("bt-downstream {failure}");
        }
    }

    /// Queue every whole speaker period read so far. Drift guard as on the
    /// USB bridge: drop a period while the phone queue is `drop_at` deep.
    fn queue_periods(&mut self) -> Step {
        let period = self.period_samples();
        while self.pending.len() >= period {
            let chunk: Vec<i16> = self.pending.drain(..period).collect();
            if self.speaker.queued() >= self.params.drop_at {
                self.stats.speaker_drops.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            match self.speaker.submit(chunk) {
                Ok(()) => {}
                Err(PlaybackError::Full) => {
                    self.stats.speaker_drops.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => return Step::Stopped,
            }
        }
        Step::Moved
    }
}

/// Sleeps between steps.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Phone audio off: how often to look at the Bluetooth state.
    pub idle_poll: Duration,
    /// No data yet.
    pub wait: Duration,
    /// After a failed open or a broken stream.
    pub retry: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            idle_poll: Duration::from_millis(50),
            wait: Duration::from_millis(5),
            retry: Duration::from_secs(1),
        }
    }
}

/// Run the downstream until the Bluetooth state or the speaker goes away.
pub fn run<C, O, S>(
    mut downstream: PhoneDownstream<C, O, S>,
    state: watch::Receiver<BluetoothState>,
    timing: Timing,
) where
    C: CaptureStream,
    O: FnMut() -> Result<C, String>,
    S: PhoneSpeaker,
{
    loop {
        if state.has_changed().is_err() {
            info!("bt-downstream: the Bluetooth state is gone; stopping");
            return;
        }
        let want = Paths::from_state(&state.borrow(), false).speaker;
        let pause = match downstream.step(want) {
            Step::Moved => continue,
            Step::Idle => timing.idle_poll,
            Step::Waiting => timing.wait,
            Step::Failed => timing.retry,
            Step::Stopped => {
                info!("bt-downstream: the speaker path is gone; stopping");
                return;
            }
        };
        std::thread::sleep(pause);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bt_audio::fake::{FakeDevice, FakeSpeaker, FakeStream};
    use crate::bt_audio::StreamError;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    type Down = PhoneDownstream<FakeStream, Box<dyn FnMut() -> Result<FakeStream, String> + Send>, Arc<FakeSpeaker>>;

    /// Speaker periods of 4 stereo frames (8 samples); reads of `read_frames`.
    fn setup(read_frames: usize) -> (Down, FakeDevice, Arc<FakeSpeaker>, Arc<BtAudioStats>) {
        let device = FakeDevice::new(2);
        let speaker = FakeSpeaker::new(2, 4);
        let stats = BtAudioStats::new();
        let down = PhoneDownstream::new(
            Box::new(device.capture_opener()) as Box<dyn FnMut() -> Result<FakeStream, String> + Send>,
            Arc::clone(&speaker),
            Arc::clone(&stats),
            DownstreamParams { channels: 2, read_frames, drop_at: 4, prefill_periods: 1 },
        );
        (down, device, speaker, stats)
    }

    fn ramp(from: i16, n: usize) -> Vec<i16> {
        (0..n as i16).map(|i| from + i).collect()
    }

    #[test]
    fn nothing_opens_while_phone_audio_is_off() {
        let (mut down, device, speaker, stats) = setup(4);
        assert_eq!(down.step(false), Step::Idle);
        assert_eq!(down.step(false), Step::Idle);
        assert_eq!(device.opens(), 0);
        assert!(speaker.periods().is_empty());
        assert!(!stats.speaker_active.load(Ordering::Relaxed));
    }

    #[test]
    fn phone_audio_opens_the_capture_and_queues_a_silent_period_first() {
        let (mut down, device, speaker, stats) = setup(4);
        device.push_read(Ok(ramp(1, 8)));
        assert_eq!(down.step(true), Step::Moved);
        assert!(down.is_open());
        assert_eq!(device.opens(), 1);
        assert_eq!(speaker.periods(), vec![vec![0; 8], ramp(1, 8)]);
        assert!(stats.speaker_active.load(Ordering::Relaxed));
        assert_eq!(stats.capture_frames.load(Ordering::Relaxed), 4);
        assert_eq!(down.step(true), Step::Waiting, "no data yet");
        assert_eq!(device.opens(), 1, "stays open");
    }

    #[test]
    fn a_shorter_capture_period_is_rechunked_into_speaker_periods() {
        // Stock reads 480-frame periods; the speaker plays 1024. Here 3 and 4.
        let (mut down, device, speaker, _) = setup(3);
        for n in 0..3 {
            device.push_read(Ok(ramp(1 + 6 * n, 6)));
        }
        for _ in 0..3 {
            down.step(true);
        }
        let periods = speaker.periods();
        assert_eq!(periods.len(), 3, "the cushion and two whole periods");
        assert_eq!(periods[1], ramp(1, 8));
        assert_eq!(periods[2], ramp(9, 8));
    }

    #[test]
    fn the_bit_clearing_closes_the_capture_and_a_new_session_starts_clean() {
        let (mut down, device, speaker, stats) = setup(3);
        device.push_read(Ok(ramp(1, 6)));
        down.step(true);
        assert_eq!(down.step(false), Step::Idle);
        assert!(!down.is_open());
        assert_eq!(device.closes(), 1);
        assert!(!stats.speaker_active.load(Ordering::Relaxed));

        // The 2 frames left over from the last session are not replayed.
        device.push_read(Ok(ramp(100, 6)));
        device.push_read(Ok(ramp(106, 6)));
        down.step(true);
        down.step(true);
        assert_eq!(device.opens(), 2);
        let periods = speaker.periods();
        assert_eq!(periods.len(), 3, "a cushion per session, one whole period");
        assert_eq!(periods[1], vec![0; 8]);
        assert_eq!(periods[2], ramp(100, 8));
    }

    #[test]
    fn a_full_speaker_queue_drops_periods_and_counts_them() {
        let (mut down, device, speaker, stats) = setup(4);
        device.push_read(Ok(ramp(1, 8)));
        down.step(true);
        *speaker.queued.lock().unwrap() = 4;
        device.push_read(Ok(ramp(9, 8)));
        down.step(true);
        *speaker.queued.lock().unwrap() = 0;
        *speaker.full.lock().unwrap() = true;
        device.push_read(Ok(ramp(17, 8)));
        assert_eq!(down.step(true), Step::Moved, "a full queue is not the end");
        assert_eq!(speaker.periods().len(), 2);
        assert_eq!(stats.speaker_drops.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn capture_channels_are_matched_to_the_speaker() {
        let device = FakeDevice::new(1);
        let speaker = FakeSpeaker::new(2, 2);
        let mut down = PhoneDownstream::new(
            device.capture_opener(),
            Arc::clone(&speaker),
            BtAudioStats::new(),
            DownstreamParams { channels: 1, read_frames: 2, drop_at: 4, prefill_periods: 0 },
        );
        device.push_read(Ok(vec![5, -5]));
        down.step(true);
        assert_eq!(speaker.periods(), vec![vec![5, 5, -5, -5]]);
    }

    #[test]
    fn an_xrun_is_counted_and_recovered_in_place() {
        let (mut down, device, speaker, stats) = setup(4);
        device.push_read(Err(StreamError::Xrun));
        device.push_read(Ok(ramp(1, 8)));
        assert_eq!(down.step(true), Step::Moved);
        assert_eq!(stats.capture_xruns.load(Ordering::Relaxed), 1);
        assert_eq!(device.recovers(), 1);
        assert_eq!(down.step(true), Step::Moved);
        assert_eq!(device.opens(), 1, "recovered, not reopened");
        assert_eq!(speaker.periods().last(), Some(&ramp(1, 8)));
    }

    #[test]
    fn a_failed_recovery_closes_the_capture_to_reopen_it() {
        let (mut down, device, _, stats) = setup(4);
        device.push_read(Err(StreamError::Xrun));
        device.fail_next_recover("EBADFD");
        assert_eq!(down.step(true), Step::Failed);
        assert!(!down.is_open());
        assert_eq!(device.closes(), 1);
        assert!(!stats.speaker_active.load(Ordering::Relaxed));
        device.push_read(Ok(ramp(1, 8)));
        assert_eq!(down.step(true), Step::Moved);
        assert_eq!(device.opens(), 2);
    }

    #[test]
    fn an_open_failure_is_counted_and_retried() {
        let (mut down, device, speaker, stats) = setup(4);
        device.fail_next_open("Device or resource busy");
        assert_eq!(down.step(true), Step::Failed);
        assert_eq!(stats.open_failures.load(Ordering::Relaxed), 1);
        assert!(!stats.speaker_active.load(Ordering::Relaxed));
        assert!(speaker.periods().is_empty());
        device.push_read(Ok(ramp(1, 8)));
        assert_eq!(down.step(true), Step::Moved);
        assert_eq!(device.opens(), 1);
    }

    #[test]
    fn a_broken_stream_closes_and_reopens() {
        let (mut down, device, _, _) = setup(4);
        device.push_read(Err(StreamError::Fatal("EIO".into())));
        assert_eq!(down.step(true), Step::Failed);
        assert_eq!(device.closes(), 1);
        device.push_read(Ok(ramp(1, 8)));
        assert_eq!(down.step(true), Step::Moved);
        assert_eq!(device.opens(), 2);
    }

    #[test]
    fn a_stopped_speaker_ends_the_downstream() {
        let (mut down, device, speaker, _) = setup(4);
        *speaker.stopped.lock().unwrap() = true;
        device.push_read(Ok(ramp(1, 8)));
        assert_eq!(down.step(true), Step::Stopped);
    }

    fn wait_until(what: &str, cond: impl Fn() -> bool) {
        let start = Instant::now();
        while !cond() {
            assert!(start.elapsed() < Duration::from_secs(5), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn the_runner_follows_the_bluetooth_state_and_ends_with_it() {
        let (down, device, speaker, stats) = setup(4);
        for n in 0..4 {
            device.push_read(Ok(ramp(1 + 8 * n, 8)));
        }
        let (tx, rx) = watch::channel(BluetoothState::default());
        let timing = Timing {
            idle_poll: Duration::from_millis(1),
            wait: Duration::from_millis(1),
            retry: Duration::from_millis(1),
        };
        let runner = std::thread::spawn(move || run(down, rx, timing));
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(device.opens(), 0, "idle while no phone audio");

        tx.send_modify(|s| s.phone_audio = true);
        wait_until("four phone periods", || speaker.periods().len() == 5);
        assert!(stats.speaker_active.load(Ordering::Relaxed));

        tx.send_modify(|s| s.phone_audio = false);
        wait_until("the capture to close", || device.closes() == 1);
        drop(tx);
        runner.join().unwrap();
    }
}
