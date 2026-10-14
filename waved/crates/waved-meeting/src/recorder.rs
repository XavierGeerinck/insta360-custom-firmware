use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::fs;
use tracing::{info, warn};
use uuid::Uuid;
use waved_codec::{create_writer, AudioFormat, RecordingWriter};
use waved_state::RecordingFormat;

/// Seconds of pre-record audio kept in [`PreBuffer`] during idle.
/// Bumping this is a one-line change; see the design doc for the memory tradeoff.
pub(crate) const PREBUFFER_SECONDS: u32 = 30;

/// Ring buffer of post-DSP mono `f32` samples, capped at
/// `seconds * sample_rate * channels`. When full, `push` evicts the oldest
/// samples. Used by `MeetingService::run` to keep the last N seconds of
/// audio available for splicing onto the front of a new recording.
pub(crate) struct PreBuffer {
    samples: VecDeque<f32>,
    capacity: usize,
    /// When paused, `push` is a no-op so the buffer freezes. Mirrors stock's
    /// `preRecordPause`/`preRecordResume`: the pre-roll is paused while a
    /// recording is active (the take's own writer owns the audio) and resumed
    /// once idle, so a take's audio never bleeds into the next pre-roll.
    paused: bool,
    /// The user's pre-recording setting (D3). Off means nothing is buffered.
    enabled: bool,
}

impl PreBuffer {
    pub(crate) fn new(seconds: u32, sample_rate: u32, channels: u16) -> Self {
        let capacity = (seconds as usize) * (sample_rate as usize) * (channels as usize);
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
            paused: false,
            enabled: true,
        }
    }

    /// Freeze the pre-roll: subsequent `push` calls are dropped until `resume`.
    pub(crate) fn pause(&mut self) {
        self.paused = true;
    }

    /// Re-arm the pre-roll so `push` accumulates again.
    pub(crate) fn resume(&mut self) {
        self.paused = false;
    }

    /// Turn pre-recording on or off. Off drops everything buffered at once,
    /// because the setting is a privacy control, and frees the ring's memory.
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.samples = VecDeque::new();
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[cfg(test)]
    pub(crate) fn is_paused(&self) -> bool {
        self.paused
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    pub(crate) fn clear(&mut self) {
        self.samples.clear();
    }

    pub(crate) fn push(&mut self, samples: &[f32]) {
        if self.capacity == 0 || self.paused || !self.enabled {
            return;
        }
        // If the incoming slice itself exceeds capacity, only the last
        // `capacity` samples can possibly survive — skip the front of the
        // slice entirely instead of pushing-and-evicting one-by-one.
        let start = samples.len().saturating_sub(self.capacity);
        let useful = &samples[start..];

        // Evict from the front to make room for `useful.len()` new samples.
        let need = (self.samples.len() + useful.len()).saturating_sub(self.capacity);
        if need > 0 {
            self.samples.drain(..need);
        }
        self.samples.extend(useful.iter().copied());
    }

    pub(crate) fn drain_all(&mut self) -> Vec<f32> {
        self.samples.drain(..).collect()
    }
}

pub const RECORDING_SAMPLE_RATE: u32 = waved_codec::SAMPLE_RATE;

/// The file format of a new take with this setting.
pub fn audio_format(format: RecordingFormat) -> AudioFormat {
    match format {
        RecordingFormat::Opus => AudioFormat::Opus,
        RecordingFormat::Flac => AudioFormat::Flac,
    }
}

/// Hours of recording `free_bytes` holds in `format`, for the start log
/// (there is no "time left" display).
pub fn hours_left(free_bytes: u64, format: AudioFormat) -> f64 {
    free_bytes as f64 / format.estimated_bytes_per_hour() as f64
}

/// A backlog (the pre-roll spliced in at Record) drains at this many times
/// the audio arriving: each append encodes its own samples plus twice as
/// many from the backlog. On an A55 that keeps encoding under ~a third of
/// each frame's time; 30 s of pre-roll is on disk ~15 s after Record.
const CATCH_UP_FACTOR: usize = 2;
/// Pre-roll written at once by `append_pre_roll` (100 ms), so a medium that
/// cannot be written fails the start, as with WAV.
const PRE_ROLL_FIRST_WRITE: usize = 4_800;
/// The most backlog an abort (or a drop) still writes: 3 s of audio. An
/// abort runs while the meeting task waits, before the fault is
/// acknowledged; encoding a whole 30 s pre-roll would take seconds on an A55
/// and overflow the frame channel (~680 ms). The rest of a damaged take's
/// backlog is dropped, and logged.
const ABORT_BACKLOG_SAMPLES: usize = 3 * RECORDING_SAMPLE_RATE as usize;
/// How often the take's file is pushed to the medium (fdatasync), off the
/// meeting task. A power cut loses at most this plus the writer's last
/// second.
const SYNC_INTERVAL: Duration = Duration::from_secs(2);

fn to_i16(sample: f32) -> i16 {
    (sample * 32768.0).clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

/// Calls `fdatasync` on the take's file every [`SYNC_INTERVAL`] from its own
/// thread, so the meeting task never waits on flash. A failure is kept and
/// fails the next write: storage that cannot be synced is not trusted.
struct Syncer {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    failure: Arc<Mutex<Option<io::Error>>>,
}

impl Syncer {
    fn start(file: File, interval: Duration) -> io::Result<Self> {
        let (stop, stopped) = mpsc::channel::<()>();
        let failure = Arc::new(Mutex::new(None));
        let failed = Arc::clone(&failure);
        let thread = std::thread::Builder::new()
            .name("recording-sync".into())
            .spawn(move || loop {
                match stopped.recv_timeout(interval) {
                    Err(RecvTimeoutError::Timeout) => {
                        if let Err(error) = file.sync_data() {
                            *failed
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
                            return;
                        }
                    }
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                }
            })?;
        Ok(Self {
            stop: Some(stop),
            thread: Some(thread),
            failure,
        })
    }

    fn failure(&self) -> Option<io::Error> {
        self.failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    fn stop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Syncer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// One pause in a take. `offset_ms` is where the pause began on the
/// recording timeline (pre-roll excluded, like bookmark offsets);
/// `duration_ms` is how long it lasted on the wall clock. Paused audio is
/// not in the file, so the file is shorter than the wall-clock take by the
/// sum of the durations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PauseSpan {
    pub offset_ms: u64,
    pub duration_ms: u64,
}

pub struct ActiveRecording {
    pub id: Uuid,
    pub dir: PathBuf,
    pub audio_mode: String,
    pub started_at: Option<String>,
    pub clock_synchronized: bool,
    format: AudioFormat,
    integrity_epoch: u64,
    /// Samples taken into the take (the timer); `written` of them are in
    /// the encoder, the rest wait in `backlog`.
    total_samples: u64,
    pre_roll_samples: u64,
    pauses: Vec<PauseSpan>,
    /// Set while paused: when the pause began and its recording-timeline offset.
    paused_since: Option<(std::time::Instant, u64)>,
    /// The take's encoder. `Some` while recording, `None` once finalized.
    /// Kept hot so each `ProcessedAudioFrame` appends with no per-frame
    /// open/seek overhead; it encodes on the meeting task.
    writer: Option<Box<dyn RecordingWriter>>,
    /// Samples taken but not encoded yet: the pre-roll, draining.
    backlog: VecDeque<i16>,
    written: u64,
    /// A write failed: the writer is not trusted with the backlog any more.
    write_failed: bool,
    syncer: Option<Syncer>,
    #[cfg(test)]
    fail_writes_after_samples: Option<u64>,
}

pub struct FinalizedRecording {
    pub id: Uuid,
    pub dir: PathBuf,
    pub format: AudioFormat,
    pub audio_path: PathBuf,
    pub audio_mode: String,
    pub started_at: Option<String>,
    pub clock_synchronized: bool,
    pub total_samples: u64,
    pub pre_roll_samples: u64,
    pub pauses: Vec<PauseSpan>,
    pub size_bytes: u64,
}

impl FinalizedRecording {
    pub fn audio_duration_ms(&self) -> u64 {
        samples_to_ms(self.total_samples)
    }

    pub fn pre_roll_duration_ms(&self) -> u64 {
        samples_to_ms(self.pre_roll_samples)
    }

    pub fn recording_duration_ms(&self) -> u64 {
        samples_to_ms(self.total_samples.saturating_sub(self.pre_roll_samples))
    }
}

impl ActiveRecording {
    pub async fn start(
        base_dir: &Path,
        audio_mode: &str,
        format: AudioFormat,
    ) -> std::io::Result<Self> {
        Self::start_with_integrity_epoch(base_dir, audio_mode, 0, format).await
    }

    pub async fn start_with_integrity_epoch(
        base_dir: &Path,
        audio_mode: &str,
        integrity_epoch: u64,
        format: AudioFormat,
    ) -> std::io::Result<Self> {
        let id = Uuid::new_v4();
        let dir = base_dir.join(id.to_string());
        fs::create_dir_all(&dir).await?;

        // Create the file synchronously: opening it and writing the stream
        // header is cheap; the hot path is `append_samples`, also sync.
        // 16-bit PCM in: the capture/AEC pipeline is 16-bit (see
        // docs/AUDIO_HARDWARE.md "Recording bit depth").
        let file = File::create(dir.join(format.partial_file_name()))?;
        let sync_handle = file.try_clone()?;
        let writer = create_writer(format, file)?;
        let syncer = Syncer::start(sync_handle, SYNC_INTERVAL)?;

        info!(
            "recording started: {} at {} ({format:?})",
            id,
            dir.display()
        );
        let now = chrono::Utc::now();
        let clock_synchronized = now.timestamp() >= 1_577_836_800;
        Ok(Self {
            id,
            dir,
            audio_mode: audio_mode.to_string(),
            started_at: clock_synchronized.then(|| now.to_rfc3339()),
            clock_synchronized,
            format,
            integrity_epoch,
            total_samples: 0,
            pre_roll_samples: 0,
            pauses: Vec::new(),
            paused_since: None,
            writer: Some(writer),
            backlog: VecDeque::new(),
            written: 0,
            write_failed: false,
            syncer: Some(syncer),
            #[cfg(test)]
            fail_writes_after_samples: None,
        })
    }

    pub fn format(&self) -> AudioFormat {
        self.format
    }

    pub fn duration_secs(&self) -> u32 {
        (self.total_samples.saturating_sub(self.pre_roll_samples) / RECORDING_SAMPLE_RATE as u64)
            as u32
    }

    pub fn is_paused(&self) -> bool {
        self.paused_since.is_some()
    }

    pub fn pauses(&self) -> &[PauseSpan] {
        &self.pauses
    }

    /// Start a pause. Returns false when already paused.
    pub fn pause_at(&mut self, now: std::time::Instant) -> bool {
        if self.paused_since.is_some() {
            return false;
        }
        let offset_ms = samples_to_ms(self.total_samples.saturating_sub(self.pre_roll_samples));
        self.paused_since = Some((now, offset_ms));
        true
    }

    /// End the current pause. Returns false when not paused.
    pub fn resume_at(&mut self, now: std::time::Instant) -> bool {
        let Some((since, offset_ms)) = self.paused_since.take() else {
            return false;
        };
        let duration_ms = now.saturating_duration_since(since).as_millis() as u64;
        self.pauses.push(PauseSpan {
            offset_ms,
            duration_ms,
        });
        true
    }

    /// The finalized file: `audio.<ext>`.
    pub fn audio_path(&self) -> PathBuf {
        self.dir.join(self.format.file_name())
    }

    /// The file while recording, and after an interruption.
    pub fn partial_audio_path(&self) -> PathBuf {
        self.dir.join(self.format.partial_file_name())
    }

    pub fn integrity_epoch(&self) -> u64 {
        self.integrity_epoch
    }

    /// Accept faults that happened while paused, when nothing was written.
    pub(crate) fn rebase_integrity_epoch(&mut self, epoch: u64) {
        self.integrity_epoch = epoch;
    }

    /// Splice the pre-roll in front of the take. It joins the backlog and
    /// drains with the frames that follow; the first 100 ms are written now.
    pub fn append_pre_roll(&mut self, samples: &[f32]) -> std::io::Result<u64> {
        let accepted = self.accept(samples);
        self.pre_roll_samples = self.pre_roll_samples.saturating_add(accepted);
        self.drain(PRE_ROLL_FIRST_WRITE)?;
        Ok(accepted)
    }

    /// Append a block of f32 mono samples in [-1.0, 1.0], saturating to S16,
    /// and encode it with up to twice as many backlog samples. Any write
    /// error is returned so the meeting service can fail the take closed and
    /// preserve the partial file for diagnosis.
    pub fn append_samples(&mut self, samples: &[f32]) -> std::io::Result<u64> {
        if self.is_paused() {
            // Paused audio never reaches the file (C1); a backlog still drains.
            self.drain(samples.len() * CATCH_UP_FACTOR)?;
            return Ok(0);
        }
        let accepted = self.accept(samples);
        self.drain(samples.len() * (1 + CATCH_UP_FACTOR))?;
        Ok(accepted)
    }

    fn accept(&mut self, samples: &[f32]) -> u64 {
        self.backlog
            .extend(samples.iter().map(|&sample| to_i16(sample)));
        self.total_samples = self.total_samples.saturating_add(samples.len() as u64);
        samples.len() as u64
    }

    /// Encode up to `budget` backlog samples, oldest first.
    fn drain(&mut self, budget: usize) -> std::io::Result<()> {
        if let Some(error) = self.syncer.as_ref().and_then(Syncer::failure) {
            self.write_failed = true;
            return Err(error);
        }
        let wanted = budget.min(self.backlog.len());
        if wanted == 0 {
            return Ok(());
        }
        // Tests stop writing after a set number of samples and fail.
        #[cfg(test)]
        let (count, injected) = match self.fail_writes_after_samples {
            Some(limit) if (limit.saturating_sub(self.written) as usize) < wanted => {
                (limit.saturating_sub(self.written) as usize, true)
            }
            _ => (wanted, false),
        };
        #[cfg(not(test))]
        let count = wanted;
        let Some(writer) = self.writer.as_mut() else {
            return Err(std::io::Error::other("recording writer unavailable"));
        };
        let (front, back) = self.backlog.as_slices();
        let from_front = count.min(front.len());
        let result = writer
            .write(&front[..from_front])
            .and_then(|()| writer.write(&back[..count - from_front]));
        if let Err(error) = result {
            self.write_failed = true;
            return Err(error);
        }
        self.backlog.drain(..count);
        self.written += count as u64;
        #[cfg(test)]
        if injected {
            self.write_failed = true;
            return Err(std::io::Error::other("injected recording write failure"));
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn fail_writes_after(&mut self, samples: u64) {
        self.fail_writes_after_samples = Some(samples);
    }

    #[cfg(test)]
    pub(crate) fn backlog_len(&self) -> usize {
        self.backlog.len()
    }

    /// Write up to `backlog_limit` samples still waiting (unless writing
    /// already failed), close the stream and stop syncing. The first error
    /// wins.
    fn close(&mut self, backlog_limit: usize) -> std::io::Result<()> {
        let mut first_error = None;
        if !self.write_failed {
            let dropped = self.backlog.len().saturating_sub(backlog_limit);
            first_error = self.drain(backlog_limit).err();
            if dropped > 0 {
                warn!(
                    "recording {}: {} ms of backlog left out of the closed partial",
                    self.id,
                    samples_to_ms(dropped as u64)
                );
            }
        }
        match self.writer.take() {
            Some(mut writer) => {
                if let Err(error) = writer.finish() {
                    first_error.get_or_insert(error);
                }
            }
            None => {
                first_error.get_or_insert(std::io::Error::other("recording already closed"));
            }
        }
        if let Some(mut syncer) = self.syncer.take() {
            syncer.stop();
            if let Some(error) = syncer.failure() {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Make an interrupted take inspectable without exposing it as a complete
    /// or uploadable artifact: the stream is closed (a playable file) under
    /// its `.partial` name.
    pub fn abort(mut self) -> std::io::Result<PathBuf> {
        let close_error = self.close(ABORT_BACKLOG_SAMPLES).err();
        let partial = self.partial_audio_path();
        let file_sync_error = File::open(&partial).and_then(|file| file.sync_all()).err();
        let dir_sync_error = File::open(&self.dir).and_then(|dir| dir.sync_all()).err();
        if let Some(error) = close_error.or(file_sync_error).or(dir_sync_error) {
            return Err(error);
        }
        Ok(partial)
    }

    /// Close and sync the file, then atomically expose it as `audio.<ext>`.
    pub fn finalize(mut self) -> std::io::Result<FinalizedRecording> {
        if self.is_paused() {
            // Stop while paused: close the span at the moment of Stop.
            self.resume_at(std::time::Instant::now());
        }
        // On the blocking pool, while the meeting task keeps draining frames.
        self.close(usize::MAX)?;

        let partial = self.partial_audio_path();
        let final_path = self.audio_path();
        File::open(&partial)?.sync_all()?;
        std::fs::rename(&partial, &final_path)?;
        File::open(&self.dir)?.sync_all()?;
        let size_bytes = std::fs::metadata(&final_path)?.len();
        Ok(FinalizedRecording {
            id: self.id,
            dir: self.dir.clone(),
            format: self.format,
            audio_path: final_path,
            audio_mode: self.audio_mode.clone(),
            started_at: self.started_at.clone(),
            clock_synchronized: self.clock_synchronized,
            total_samples: self.total_samples,
            pre_roll_samples: self.pre_roll_samples,
            pauses: std::mem::take(&mut self.pauses),
            size_bytes,
        })
    }
}

impl Drop for ActiveRecording {
    fn drop(&mut self) {
        // Safety net: make an interrupted partial file inspectable, but never
        // rename it to the finalized filename or make it uploadable.
        if self.writer.is_some() {
            if let Err(error) = self.close(ABORT_BACKLOG_SAMPLES) {
                warn!("partial recording close error: {error}");
            }
        }
    }
}

fn samples_to_ms(samples: u64) -> u64 {
    samples.saturating_mul(1_000) / RECORDING_SAMPLE_RATE as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use waved_codec::testsignal::{snr_db, speech_like};
    use waved_codec::{open_reader, read_to_end};

    fn decoded(format: AudioFormat, path: &Path) -> Vec<i16> {
        let mut reader = open_reader(format, path).unwrap();
        read_to_end(reader.as_mut()).unwrap()
    }

    fn finalized_samples(finalized: &FinalizedRecording) -> Vec<i16> {
        decoded(finalized.format, &finalized.audio_path)
    }

    /// Speech-like audio as the f32 the pipeline delivers; `to_i16` gives
    /// back exactly the i16 it came from.
    fn speech_f32(seconds: f64, seed: u64) -> (Vec<f32>, Vec<i16>) {
        let samples = speech_like(seconds, seed);
        let f32s = samples.iter().map(|&s| f32::from(s) / 32768.0).collect();
        (f32s, samples)
    }

    #[test]
    fn each_setting_records_its_format() {
        assert_eq!(audio_format(RecordingFormat::Opus), AudioFormat::Opus);
        assert_eq!(audio_format(RecordingFormat::Flac), AudioFormat::Flac);
    }

    #[test]
    fn hours_left_uses_the_format_byte_rate() {
        let free = 12_000_000_000;
        assert!(hours_left(free, AudioFormat::Opus) > 800.0);
        assert!((hours_left(free, AudioFormat::Wav) - 34.7).abs() < 0.1);
        assert!(hours_left(free, AudioFormat::Flac) > hours_left(free, AudioFormat::Wav));
    }

    #[tokio::test]
    async fn active_recording_creates_only_a_partial_file() {
        for format in AudioFormat::ALL {
            let dir = tempfile::tempdir().unwrap();
            let rec = ActiveRecording::start(dir.path(), "Omni", format)
                .await
                .unwrap();
            assert_eq!(rec.format(), format);
            assert!(rec.dir.join(format.partial_file_name()).exists());
            assert!(!rec.dir.join(format.file_name()).exists());
            assert!(!rec.dir.join("metadata.json").exists());
            assert_eq!(rec.audio_path(), rec.dir.join(format.file_name()));
            assert_eq!(
                rec.partial_audio_path(),
                rec.dir.join(format.partial_file_name())
            );
        }
    }

    #[tokio::test]
    async fn every_format_plays_the_take_back_at_its_exact_length() {
        let (input, samples) = speech_f32(3.0, 1);
        for format in AudioFormat::ALL {
            let dir = tempfile::tempdir().unwrap();
            let mut rec = ActiveRecording::start(dir.path(), "Omni", format)
                .await
                .unwrap();
            rec.append_pre_roll(&input[..48_000]).unwrap();
            for frame in input[48_000..].chunks(480) {
                rec.append_samples(frame).unwrap();
            }
            let finalized = rec.finalize().unwrap();
            assert_eq!(finalized.format, format);
            assert_eq!(finalized.audio_path, finalized.dir.join(format.file_name()));
            assert!(!finalized.dir.join(format.partial_file_name()).exists());
            assert_eq!(finalized.total_samples, 144_000);
            assert_eq!(finalized.pre_roll_samples, 48_000);
            assert_eq!(
                finalized.size_bytes,
                std::fs::metadata(&finalized.audio_path).unwrap().len()
            );
            let back = finalized_samples(&finalized);
            assert_eq!(back.len(), samples.len(), "{format:?}");
            match format {
                AudioFormat::Opus => assert!(snr_db(&samples, &back) > 8.0),
                AudioFormat::Wav | AudioFormat::Flac => assert_eq!(back, samples, "{format:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_killed_take_keeps_what_reached_the_file() {
        // kill -9 leaves exactly what the process handed to the OS: read the
        // partial while the take is still open.
        let (input, samples) = speech_f32(3.0, 2);
        for format in [AudioFormat::Opus, AudioFormat::Flac] {
            let dir = tempfile::tempdir().unwrap();
            let mut rec = ActiveRecording::start(dir.path(), "Omni", format)
                .await
                .unwrap();
            for frame in input.chunks(480) {
                rec.append_samples(frame).unwrap();
            }
            let partial = rec.partial_audio_path();
            let back = decoded(format, &partial);
            assert!(
                back.len() >= 91_000,
                "{format:?}: {} samples survive a kill after 3 s",
                back.len()
            );
            if format == AudioFormat::Flac {
                assert_eq!(back, samples[..back.len()]);
            }
            drop(rec);
            assert_eq!(
                decoded(format, &partial).len(),
                samples.len(),
                "closed on drop"
            );
        }
    }

    #[tokio::test]
    async fn the_pre_roll_drains_with_the_frames_that_follow() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Omni", AudioFormat::Opus)
            .await
            .unwrap();
        let (input, _) = speech_f32(31.0, 3);
        rec.append_pre_roll(&input[..1_440_000]).unwrap();
        assert_eq!(
            rec.backlog_len(),
            1_440_000 - 4_800,
            "only 100 ms written at once"
        );
        for frame in input[1_440_000..].chunks(480) {
            rec.append_samples(frame).unwrap();
        }
        // 100 frames of 10 ms, each taking two frames' worth off the backlog.
        assert_eq!(rec.backlog_len(), 1_440_000 - 4_800 - 100 * 960);
        assert_eq!(
            rec.duration_secs(),
            1,
            "the timer counts the take, not the file"
        );
        let finalized = rec.finalize().unwrap();
        assert_eq!(finalized.total_samples, 1_488_000);
        assert_eq!(finalized_samples(&finalized).len(), 1_488_000);
    }

    #[test]
    fn prebuffer_new_is_empty() {
        let pb = PreBuffer::new(2, 48_000, 1);
        assert_eq!(pb.len(), 0);
        assert!(!pb.is_paused());
    }

    #[test]
    fn prebuffer_pause_drops_pushes_resume_rearms() {
        let mut pb = PreBuffer::new(2, 48_000, 1);
        pb.push(&[0.1, 0.2, 0.3]);
        assert_eq!(pb.len(), 3);

        pb.pause();
        assert!(pb.is_paused());
        pb.push(&[0.4; 1000]); // dropped while paused
        assert_eq!(pb.len(), 3, "paused buffer must not accumulate");

        pb.resume();
        assert!(!pb.is_paused());
        pb.push(&[0.5, 0.6]);
        assert_eq!(pb.len(), 5, "resumed buffer accumulates again");
    }

    #[test]
    fn prebuffer_clear_resets_len() {
        let mut pb = PreBuffer::new(2, 48_000, 1);
        pb.push(&[0.1_f32; 100]);
        assert_eq!(pb.len(), 100);
        pb.clear();
        assert_eq!(pb.len(), 0);
    }

    #[test]
    fn prebuffer_push_then_drain_round_trips_under_capacity() {
        let mut pb = PreBuffer::new(1, 100, 1); // capacity 100
        pb.push(&[0.1, 0.2, 0.3, 0.4, 0.5]);
        let drained = pb.drain_all();
        assert_eq!(drained, vec![0.1, 0.2, 0.3, 0.4, 0.5]);
        assert_eq!(pb.len(), 0, "drain_all should empty the buffer");
    }

    #[test]
    fn prebuffer_evicts_oldest_when_overflowing() {
        let mut pb = PreBuffer::new(1, 4, 1); // capacity 4
        pb.push(&[1.0, 2.0, 3.0]); // [1,2,3]
        pb.push(&[4.0, 5.0, 6.0]); // overflow → [3,4,5,6]
        let drained = pb.drain_all();
        assert_eq!(drained, vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn prebuffer_push_slice_larger_than_capacity_keeps_tail() {
        let mut pb = PreBuffer::new(1, 4, 1); // capacity 4
        pb.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        let drained = pb.drain_all();
        assert_eq!(drained, vec![6.0, 7.0, 8.0, 9.0]);
    }

    #[test]
    fn prebuffer_drain_all_on_empty_returns_empty_vec() {
        let mut pb = PreBuffer::new(1, 100, 1);
        let drained = pb.drain_all();
        assert!(drained.is_empty());
        assert_eq!(pb.len(), 0);
    }

    #[tokio::test]
    async fn prebuffer_splice_then_live_produces_correct_audio() {
        // PreBuffer capacity of 2 s @ 48 kHz mono = 96_000 samples.
        let mut pb = PreBuffer::new(2, RECORDING_SAMPLE_RATE, 1);

        // Push 3 s of 0.25 — first 1 s should be evicted, last 2 s retained.
        let three_seconds: Vec<f32> = vec![0.25_f32; 3 * RECORDING_SAMPLE_RATE as usize];
        pb.push(&three_seconds);
        assert_eq!(pb.len(), 2 * RECORDING_SAMPLE_RATE as usize);

        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Test", AudioFormat::Flac)
            .await
            .unwrap();

        // Splice prebuffer prefix, then write 1 s of live audio at 0.75.
        rec.append_pre_roll(&pb.drain_all()).unwrap();
        rec.append_samples(&vec![0.75_f32; RECORDING_SAMPLE_RATE as usize])
            .unwrap();
        let finalized = rec.finalize().unwrap();
        let samples = finalized_samples(&finalized);

        let prefix_len = 2 * RECORDING_SAMPLE_RATE as usize; // 96_000
        let live_len = RECORDING_SAMPLE_RATE as usize; // 48_000
        assert_eq!(samples.len(), prefix_len + live_len);

        // 0.25 * 32768 = 8192 = 0x2000
        assert!(
            samples[..prefix_len].iter().all(|&s| s == 0x2000),
            "expected all 0x2000 in prefix; first mismatch = {}",
            samples[..prefix_len]
                .iter()
                .position(|&s| s != 0x2000)
                .map(|i| format!("idx {i} = {}", samples[i]))
                .unwrap_or_else(|| "none".into())
        );

        // 0.75 * 32768 = 24576 = 0x6000
        assert!(
            samples[prefix_len..].iter().all(|&s| s == 0x6000),
            "expected all 0x6000 in live section"
        );

        // Drain the buffer was used; prebuffer is empty.
        assert_eq!(pb.len(), 0);
    }

    #[tokio::test]
    async fn prebuffer_splice_when_not_yet_full_produces_short_prefix() {
        // 2 s capacity, only 1 s pushed before recording starts.
        let mut pb = PreBuffer::new(2, RECORDING_SAMPLE_RATE, 1);
        pb.push(&vec![0.5_f32; RECORDING_SAMPLE_RATE as usize]); // 1 s

        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Test", AudioFormat::Flac)
            .await
            .unwrap();
        rec.append_pre_roll(&pb.drain_all()).unwrap();
        rec.append_samples(&vec![0.5_f32; RECORDING_SAMPLE_RATE as usize])
            .unwrap(); // 1 s live
        let finalized = rec.finalize().unwrap();
        let samples = finalized_samples(&finalized);

        // 1 s prefix + 1 s live = 2 s total. No padding, no duplication.
        assert_eq!(samples.len(), 2 * RECORDING_SAMPLE_RATE as usize);
        // 0.5 * 32768 = 16384 = 0x4000
        assert!(samples.iter().all(|&s| s == 0x4000));
    }

    #[tokio::test]
    async fn injected_write_failure_is_reported_and_abort_preserves_the_partial() {
        for format in AudioFormat::ALL {
            let dir = tempfile::tempdir().unwrap();
            let mut rec =
                ActiveRecording::start_with_integrity_epoch(dir.path(), "Test", 7, format)
                    .await
                    .unwrap();
            rec.fail_writes_after(2);

            let error = rec.append_samples(&[0.25, 0.5, 0.75]).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::Other);
            assert_eq!(rec.integrity_epoch(), 7);

            let partial = rec.abort().unwrap();
            assert!(partial.exists());
            assert_eq!(partial, partial.with_file_name(format.partial_file_name()));
            assert!(!partial.with_file_name(format.file_name()).exists());
            assert_eq!(
                decoded(format, &partial).len(),
                2,
                "{format:?}: what was written before the failure, closed"
            );
        }
    }

    #[tokio::test]
    async fn injected_pre_roll_failure_is_returned_without_exposing_a_final_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Test", AudioFormat::Flac)
            .await
            .unwrap();
        rec.fail_writes_after(1);

        rec.append_pre_roll(&[0.1, 0.2]).unwrap_err();
        let partial = rec.abort().unwrap();

        assert!(partial.exists());
        assert!(!partial.with_file_name("audio.flac").exists());
        assert_eq!(decoded(AudioFormat::Flac, &partial).len(), 1);
    }

    #[tokio::test]
    async fn an_aborted_opus_take_is_a_playable_partial() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Test", AudioFormat::Opus)
            .await
            .unwrap();
        let (input, _) = speech_f32(1.1, 4);
        rec.append_pre_roll(&input[..48_000]).unwrap();
        for frame in input[48_000..].chunks(480) {
            rec.append_samples(frame).unwrap();
        }
        assert!(rec.backlog_len() > 0, "part of the pre-roll still waits");
        // An integrity fault (not a write error): the backlog is written too.
        let partial = rec.abort().unwrap();
        assert_eq!(decoded(AudioFormat::Opus, &partial).len(), 52_800);
    }

    #[tokio::test]
    async fn an_abort_writes_at_most_three_seconds_of_backlog() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Test", AudioFormat::Flac)
            .await
            .unwrap();
        // 10 s of pre-roll: 100 ms are written at once, the rest waits.
        rec.append_pre_roll(&vec![0.1; 480_000]).unwrap();
        let partial = rec.abort().unwrap();
        assert_eq!(decoded(AudioFormat::Flac, &partial).len(), 4_800 + 144_000);
    }

    #[test]
    fn prebuffer_disabled_drops_buffered_audio_and_ignores_pushes() {
        let mut pb = PreBuffer::new(1, 100, 1);
        pb.push(&[0.1; 50]);
        pb.set_enabled(false);
        assert!(!pb.is_enabled());
        assert_eq!(pb.len(), 0, "turning it off drops what was buffered");
        pb.push(&[0.2; 10]);
        assert_eq!(pb.len(), 0, "nothing is buffered while off");
        pb.set_enabled(true);
        pb.push(&[0.3; 10]);
        assert_eq!(pb.drain_all(), vec![0.3; 10]);
    }

    #[tokio::test]
    async fn paused_audio_never_reaches_the_file() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Omni", AudioFormat::Flac)
            .await
            .unwrap();
        let t0 = Instant::now();
        rec.append_samples(&vec![0.25; 48_000]).unwrap();
        assert!(rec.pause_at(t0));
        assert!(!rec.pause_at(t0), "a second pause is a no-op");
        assert!(rec.is_paused());
        assert_eq!(rec.append_samples(&vec![0.5; 48_000]).unwrap(), 0);
        assert_eq!(rec.duration_secs(), 1, "the timer is frozen while paused");
        assert!(rec.resume_at(t0 + Duration::from_secs(5)));
        assert!(
            !rec.resume_at(t0 + Duration::from_secs(6)),
            "not paused any more"
        );
        rec.append_samples(&vec![0.75; 48_000]).unwrap();
        assert_eq!(rec.duration_secs(), 2);

        let finalized = rec.finalize().unwrap();
        assert_eq!(
            finalized.pauses,
            vec![PauseSpan {
                offset_ms: 1_000,
                duration_ms: 5_000
            }]
        );
        let samples = finalized_samples(&finalized);
        assert_eq!(samples.len(), 96_000);
        // 0.5 * 32768 = 0x4000: the paused value must not appear.
        assert!(samples.iter().all(|&s| s != 0x4000));
    }

    #[tokio::test]
    async fn stop_while_paused_closes_the_span() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = ActiveRecording::start(dir.path(), "Omni", AudioFormat::Opus)
            .await
            .unwrap();
        rec.append_pre_roll(&vec![0.1; 4_800]).unwrap();
        rec.append_samples(&vec![0.1; 24_000]).unwrap();
        rec.pause_at(std::time::Instant::now());
        let finalized = rec.finalize().unwrap();
        assert_eq!(finalized.pauses.len(), 1);
        assert_eq!(
            finalized.pauses[0].offset_ms, 500,
            "the offset excludes the pre-roll"
        );
    }
}
