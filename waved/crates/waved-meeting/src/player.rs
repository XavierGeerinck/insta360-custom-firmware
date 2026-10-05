//! Local playback of recordings through the speaker (C8): play, pause, seek
//! and marks for Recording detail. It pauses by itself while a call is active
//! or a recording runs, so the speaker never talks over either.
//!
//! Periods go to `PlaybackService::play_local`, which mixes them with any
//! host audio before the speaker DSP, so volume, the limiter and the AEC
//! reference cover playback too. `position_ms` is the next period to queue:
//! at most two periods (~43 ms) ahead of what is audible.

use crate::metadata::marks_from_metadata;
use crate::recorder::RECORDING_SAMPLE_RATE;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tracing::{info, warn};
use uuid::Uuid;
use waved_state::{AudioState, Command, MeetingState, PlaybackState};

/// Periods kept queued ahead of the speaker: enough to ride out a late tick,
/// few enough that Pause and Seek feel immediate.
const QUEUE_AHEAD: usize = 2;
/// Position updates on the bus at most this often while playing.
const PUBLISH_STEP_MS: u32 = 250;

/// Where decoded periods go. The speaker's PlaybackService in production,
/// a fake in tests.
pub trait SpeakerOutput: Send + Sync {
    /// Queue one interleaved period; false when the speaker is gone or full.
    fn submit_local(&self, interleaved: Vec<i16>) -> bool;
    /// Periods queued and not played yet.
    fn local_queued(&self) -> usize;
    fn output_channels(&self) -> usize;
    fn frames_per_period(&self) -> usize;
}

impl SpeakerOutput for waved_audio::playback::PlaybackService {
    fn submit_local(&self, interleaved: Vec<i16>) -> bool {
        self.play_local(interleaved).is_ok()
    }
    fn local_queued(&self) -> usize {
        self.local_queue_depth()
    }
    fn output_channels(&self) -> usize {
        self.channels() as usize
    }
    fn frames_per_period(&self) -> usize {
        self.period_frames()
    }
}

/// One recording opened for playback.
pub struct Track {
    id: Uuid,
    reader: hound::WavReader<BufReader<std::fs::File>>,
    position: u32,
    total: u32,
    marks_ms: Vec<u32>,
}

impl Track {
    pub fn open(recordings_dir: &Path, id: Uuid) -> std::io::Result<Self> {
        let dir = recordings_dir.join(id.to_string());
        let reader = hound::WavReader::open(dir.join("audio.wav")).map_err(to_io)?;
        let spec = reader.spec();
        if spec.channels != 1
            || spec.sample_rate != RECORDING_SAMPLE_RATE
            || spec.bits_per_sample != 16
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported recording format {spec:?}"),
            ));
        }
        let total = reader.duration();
        Ok(Self {
            id,
            reader,
            position: 0,
            total,
            marks_ms: marks_from_metadata(&dir.join("metadata.json")),
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn duration_ms(&self) -> u32 {
        frames_to_ms(self.total)
    }

    pub fn position_ms(&self) -> u32 {
        frames_to_ms(self.position)
    }

    pub fn marks_ms(&self) -> &[u32] {
        &self.marks_ms
    }

    pub fn at_end(&self) -> bool {
        self.position >= self.total
    }

    /// Move the next period to `ms` from the start, clamped to the end.
    pub fn seek_ms(&mut self, ms: u32) -> std::io::Result<()> {
        let frame = ms_to_frames(ms).min(self.total);
        self.reader.seek(frame)?;
        self.position = frame;
        Ok(())
    }

    /// The next period with the mono sample copied to every channel; the
    /// last period is zero-padded. `None` once at the end.
    pub fn next_period(
        &mut self,
        frames: usize,
        channels: usize,
    ) -> std::io::Result<Option<Vec<i16>>> {
        if self.at_end() {
            return Ok(None);
        }
        let mut out = vec![0i16; frames * channels];
        let mut read = 0usize;
        for (index, sample) in self.reader.samples::<i16>().take(frames).enumerate() {
            let sample = sample.map_err(to_io)?;
            out[index * channels..(index + 1) * channels].fill(sample);
            read += 1;
        }
        self.position = if read < frames {
            self.total
        } else {
            (self.position + read as u32).min(self.total)
        };
        Ok(Some(out))
    }
}

fn frames_to_ms(frames: u32) -> u32 {
    (u64::from(frames) * 1_000 / u64::from(RECORDING_SAMPLE_RATE)) as u32
}

fn ms_to_frames(ms: u32) -> u32 {
    (u64::from(ms) * u64::from(RECORDING_SAMPLE_RATE) / 1_000).min(u64::from(u32::MAX)) as u32
}

fn to_io(error: hound::Error) -> std::io::Error {
    match error {
        hound::Error::IoError(io) => io,
        other => std::io::Error::new(std::io::ErrorKind::InvalidData, other),
    }
}

fn is_blocked(audio: &AudioState, meeting: &MeetingState) -> bool {
    audio.call_off_hook || meeting.recording
}

pub struct RecordingPlayer {
    recordings_dir: PathBuf,
    output: Arc<dyn SpeakerOutput>,
    state_tx: Arc<watch::Sender<PlaybackState>>,
    track: Option<Track>,
    playing: bool,
    blocked: bool,
    published_ms: u32,
}

impl RecordingPlayer {
    pub fn new(
        recordings_dir: PathBuf,
        output: Arc<dyn SpeakerOutput>,
        state_tx: Arc<watch::Sender<PlaybackState>>,
    ) -> Self {
        Self {
            recordings_dir,
            output,
            state_tx,
            track: None,
            playing: false,
            blocked: false,
            published_ms: 0,
        }
    }

    pub async fn run(
        mut self,
        mut commands: broadcast::Receiver<Command>,
        mut audio: watch::Receiver<AudioState>,
        mut meeting: watch::Receiver<MeetingState>,
    ) {
        let period = Duration::from_micros(
            self.output.frames_per_period() as u64 * 1_000_000 / u64::from(RECORDING_SAMPLE_RATE),
        );
        let mut tick = tokio::time::interval(period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let blocked = is_blocked(&audio.borrow(), &meeting.borrow());
        self.set_blocked(blocked);
        info!("recording player ready");
        loop {
            tokio::select! {
                received = commands.recv() => match received {
                    Ok(Command::Shutdown) | Err(broadcast::error::RecvError::Closed) => return,
                    Ok(command) => self.handle(&command),
                    Err(broadcast::error::RecvError::Lagged(n)) => warn!("player lagged {n} commands"),
                },
                Ok(()) = audio.changed() => {
                    let blocked = is_blocked(&audio.borrow_and_update(), &meeting.borrow());
                    self.set_blocked(blocked);
                }
                Ok(()) = meeting.changed() => {
                    let blocked = is_blocked(&audio.borrow(), &meeting.borrow_and_update());
                    self.set_blocked(blocked);
                }
                _ = tick.tick(), if self.playing => self.pump(),
            }
        }
    }

    /// A call or a recording started (true) or ended (false). Playback
    /// pauses and stays paused; the user resumes it.
    pub fn set_blocked(&mut self, blocked: bool) {
        if blocked && !self.blocked && self.playing {
            info!("playback paused: a call or recording started");
            self.playing = false;
            self.publish(true);
        }
        self.blocked = blocked;
    }

    pub fn handle(&mut self, command: &Command) {
        match command {
            Command::PlayRecording(id) => self.play(*id),
            Command::PausePlayback => {
                if self.playing {
                    self.playing = false;
                    self.publish(true);
                }
            }
            Command::ResumePlayback => {
                if let Some(id) = self.track.as_ref().map(Track::id) {
                    self.play(id);
                }
            }
            Command::SeekPlayback(ms) => self.seek(*ms),
            Command::StopPlayback => self.stop(),
            Command::DeleteRecording(id) | Command::DiscardRecording(id)
                if self.track.as_ref().is_some_and(|track| track.id() == *id) =>
            {
                self.stop()
            }
            _ => {}
        }
    }

    fn play(&mut self, id: Uuid) {
        if self.blocked {
            warn!("play ignored: a call or recording is active");
            return;
        }
        if self.track.as_ref().map(Track::id) != Some(id) {
            match Track::open(&self.recordings_dir, id) {
                Ok(track) => self.track = Some(track),
                Err(error) => {
                    warn!("cannot play recording {id}: {error}");
                    self.stop();
                    return;
                }
            }
        }
        let track = self.track.as_mut().expect("track loaded above");
        if track.at_end() {
            if let Err(error) = track.seek_ms(0) {
                warn!("cannot rewind recording {id}: {error}");
                self.stop();
                return;
            }
        }
        self.playing = true;
        self.publish(true);
        self.pump();
    }

    fn seek(&mut self, ms: u32) {
        let Some(track) = self.track.as_mut() else {
            return;
        };
        if let Err(error) = track.seek_ms(ms) {
            warn!("seek to {ms} ms failed: {error}");
            return;
        }
        self.publish(true);
    }

    fn stop(&mut self) {
        self.track = None;
        self.playing = false;
        self.publish(true);
    }

    /// Keep `QUEUE_AHEAD` periods queued while playing.
    pub fn pump(&mut self) {
        if !self.playing {
            return;
        }
        let frames = self.output.frames_per_period();
        let channels = self.output.output_channels();
        while self.output.local_queued() < QUEUE_AHEAD {
            let Some(track) = self.track.as_mut() else {
                self.playing = false;
                break;
            };
            match track.next_period(frames, channels) {
                Ok(Some(period)) => {
                    if !self.output.submit_local(period) {
                        warn!("the speaker refused playback; stopping");
                        self.playing = false;
                        break;
                    }
                }
                Ok(None) => {
                    self.playing = false;
                    break;
                }
                Err(error) => {
                    warn!("playback read failed: {error}");
                    self.playing = false;
                    break;
                }
            }
        }
        let finished = !self.playing;
        self.publish(finished);
    }

    fn publish(&mut self, force: bool) {
        let state = match self.track.as_ref() {
            Some(track) => PlaybackState {
                recording: Some(track.id()),
                playing: self.playing,
                position_ms: track.position_ms(),
                duration_ms: track.duration_ms(),
                marks_ms: track.marks_ms().to_vec(),
            },
            None => PlaybackState::default(),
        };
        if !force && state.position_ms.abs_diff(self.published_ms) < PUBLISH_STEP_MS {
            return;
        }
        self.published_ms = state.position_ms;
        self.state_tx.send_if_modified(|current| {
            if *current == state {
                return false;
            }
            *current = state;
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct FakeOutput {
        submitted: Mutex<Vec<Vec<i16>>>,
        queued: AtomicUsize,
        frames: usize,
    }

    impl FakeOutput {
        fn new(frames: usize) -> Arc<Self> {
            Arc::new(Self {
                submitted: Mutex::new(Vec::new()),
                queued: AtomicUsize::new(0),
                frames,
            })
        }
        /// The speaker played everything queued.
        fn drain(&self) {
            self.queued.store(0, Ordering::SeqCst);
        }
        fn submitted(&self) -> Vec<Vec<i16>> {
            self.submitted.lock().unwrap().clone()
        }
    }

    impl SpeakerOutput for FakeOutput {
        fn submit_local(&self, interleaved: Vec<i16>) -> bool {
            self.submitted.lock().unwrap().push(interleaved);
            self.queued.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn local_queued(&self) -> usize {
            self.queued.load(Ordering::SeqCst)
        }
        fn output_channels(&self) -> usize {
            2
        }
        fn frames_per_period(&self) -> usize {
            self.frames
        }
    }

    fn ramp(n: usize) -> Vec<i16> {
        (0..n).map(|i| (i % 30_000) as i16).collect()
    }

    fn fixture(dir: &Path, samples: &[i16], pre_roll_ms: u64, bookmarks_ms: &[u64]) -> Uuid {
        let id = Uuid::new_v4();
        let rec = dir.join(id.to_string());
        std::fs::create_dir_all(&rec).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(rec.join("audio.wav"), spec).unwrap();
        for &sample in samples {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let metadata = serde_json::json!({
            "schema_version": 1,
            "event": "meeting.recording.completed",
            "meeting_id": id.to_string(),
            "device": {"id": "wave-test", "name": "Wave", "software_version": "0"},
            "recording": {
                "started_at": null, "clock_synchronized": false,
                "recording_duration_ms": 0, "audio_duration_ms": 0,
                "pre_roll_duration_ms": pre_roll_ms, "audio_mode": "Omni"
            },
            "audio": {
                "filename": "x.wav", "content_type": "audio/wav", "codec": "pcm_s16le",
                "sample_rate_hz": 48000, "channels": 1, "bit_depth": 16,
                "size_bytes": 0, "sha256": ""
            },
            "bookmarks": bookmarks_ms.iter().map(|ms| serde_json::json!({"offset_ms": ms})).collect::<Vec<_>>(),
            "speaker_segments": []
        });
        std::fs::write(
            rec.join("metadata.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        id
    }

    fn player(
        dir: &Path,
        output: &Arc<FakeOutput>,
    ) -> (RecordingPlayer, watch::Receiver<PlaybackState>) {
        let (tx, rx) = watch::channel(PlaybackState::default());
        let output: Arc<dyn SpeakerOutput> = Arc::clone(output) as Arc<dyn SpeakerOutput>;
        (
            RecordingPlayer::new(dir.to_path_buf(), output, Arc::new(tx)),
            rx,
        )
    }

    #[test]
    fn play_streams_mono_to_both_channels_two_periods_ahead() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(1_000), 0, &[]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        let periods = out.submitted();
        assert_eq!(periods.len(), 2, "two periods queued ahead");
        let expected: Vec<i16> = (0..100).flat_map(|i| [i as i16, i as i16]).collect();
        assert_eq!(periods[0], expected);
        let s = state.borrow().clone();
        assert_eq!(s.recording, Some(id));
        assert!(s.playing);
        assert_eq!(s.duration_ms, 20);
    }

    #[test]
    fn end_of_file_parks_at_the_end_and_play_restarts() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(250), 0, &[]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        out.drain();
        p.pump();
        let periods = out.submitted();
        assert_eq!(periods.len(), 3);
        assert_eq!(periods[2][..2], [200, 200]);
        assert!(
            periods[2][100..].iter().all(|&s| s == 0),
            "the last period is padded"
        );
        let s = state.borrow().clone();
        assert!(!s.playing);
        assert_eq!(s.position_ms, s.duration_ms);
        out.drain();
        p.handle(&Command::PlayRecording(id));
        assert_eq!(
            out.submitted()[3][..2],
            [0, 0],
            "Play at the end starts over"
        );
    }

    #[test]
    fn seek_moves_the_next_period_and_clamps_past_the_end() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(48_000), 0, &[]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        p.handle(&Command::SeekPlayback(500));
        assert_eq!(state.borrow().position_ms, 500);
        out.drain();
        p.pump();
        assert_eq!(out.submitted()[2][..2], [24_000, 24_000]);
        p.handle(&Command::SeekPlayback(10_000));
        assert_eq!(state.borrow().position_ms, state.borrow().duration_ms);
    }

    #[test]
    fn marks_count_from_the_start_of_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(1_000), 1_000, &[500, 2_000]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        assert_eq!(state.borrow().marks_ms, vec![1_500, 3_000]);
        assert!(crate::metadata::marks_from_metadata(&tmp.path().join("missing.json")).is_empty());
    }

    #[test]
    fn a_call_or_recording_pauses_and_blocks_playback() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(48_000), 0, &[]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        p.set_blocked(true);
        assert!(!state.borrow().playing);
        p.handle(&Command::ResumePlayback);
        assert!(!state.borrow().playing, "blocked while the call lasts");
        p.set_blocked(false);
        p.handle(&Command::ResumePlayback);
        assert!(state.borrow().playing);
    }

    #[test]
    fn deleting_the_playing_recording_stops_playback() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(48_000), 0, &[]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        p.handle(&Command::DeleteRecording(Uuid::new_v4()));
        assert!(
            state.borrow().playing,
            "another recording's delete changes nothing"
        );
        p.handle(&Command::DeleteRecording(id));
        assert_eq!(*state.borrow(), PlaybackState::default());
    }

    #[test]
    fn a_missing_recording_clears_the_state() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(48_000), 0, &[]);
        let out = FakeOutput::new(100);
        let (mut p, state) = player(tmp.path(), &out);
        p.handle(&Command::PlayRecording(id));
        assert_eq!(
            state.borrow().recording,
            Some(id),
            "a valid recording plays first"
        );
        assert!(state.borrow().playing);
        let before = out.submitted().len();
        out.drain();
        p.handle(&Command::PlayRecording(Uuid::new_v4()));
        assert_eq!(*state.borrow(), PlaybackState::default());
        assert_eq!(
            out.submitted().len(),
            before,
            "nothing queued for a missing recording"
        );
    }

    #[tokio::test]
    async fn run_pauses_when_a_call_starts() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(480_000), 0, &[]);
        let out = FakeOutput::new(1_024);
        let (p, mut state) = player(tmp.path(), &out);
        let (commands, commands_rx) = broadcast::channel(8);
        let (audio, audio_rx) = watch::channel(AudioState::default());
        let (_meeting, meeting_rx) = watch::channel(MeetingState::default());
        let task = tokio::spawn(p.run(commands_rx, audio_rx, meeting_rx));
        commands.send(Command::PlayRecording(id)).unwrap();
        tokio::time::timeout(Duration::from_secs(1), state.wait_for(|s| s.playing))
            .await
            .unwrap()
            .unwrap();
        audio.send_modify(|a| a.call_off_hook = true);
        tokio::time::timeout(Duration::from_secs(1), state.wait_for(|s| !s.playing))
            .await
            .unwrap()
            .unwrap();
        commands.send(Command::Shutdown).unwrap();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn run_pauses_and_refuses_play_while_recording() {
        let tmp = tempfile::tempdir().unwrap();
        let id = fixture(tmp.path(), &ramp(480_000), 0, &[]);
        let out = FakeOutput::new(1_024);
        let (p, mut state) = player(tmp.path(), &out);
        let (commands, commands_rx) = broadcast::channel(8);
        let (_audio, audio_rx) = watch::channel(AudioState::default());
        let (meeting, meeting_rx) = watch::channel(MeetingState::default());
        let task = tokio::spawn(p.run(commands_rx, audio_rx, meeting_rx));
        commands.send(Command::PlayRecording(id)).unwrap();
        tokio::time::timeout(Duration::from_secs(1), state.wait_for(|s| s.playing))
            .await
            .unwrap()
            .unwrap();
        meeting.send_modify(|m| m.recording = true);
        tokio::time::timeout(Duration::from_secs(1), state.wait_for(|s| !s.playing))
            .await
            .unwrap()
            .unwrap();
        commands.send(Command::ResumePlayback).unwrap();
        // Commands are handled in order: once the seek shows, Resume was handled.
        commands.send(Command::SeekPlayback(1_000)).unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            state.wait_for(|s| s.position_ms == 1_000),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!state.borrow().playing, "no playback while recording");
        meeting.send_modify(|m| m.recording = false);
        // The end of the take and Resume reach the player on different
        // channels, in either order; Resume works once the end is seen.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while !state.borrow().playing {
            assert!(tokio::time::Instant::now() < deadline, "Resume refused after the take");
            commands.send(Command::ResumePlayback).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        commands.send(Command::Shutdown).unwrap();
        task.await.unwrap();
    }
}
