//! PlaybackService — owns an ALSA playback PCM, feeds it continuously so the
//! hardware clock never stalls, and publishes every written batch as the AEC
//! reference signal.
//!
//! Architecture:
//! - Caller submits audio via [`PlaybackService::play_batch`] (interleaved i16).
//! - A dedicated std::thread at SCHED_FIFO drains the mpsc, writes to ALSA,
//!   and pushes the same samples on a broadcast so capture can pair them with
//!   mic frames.
//! - When the mpsc is empty, the thread writes silence so TDMOUT keeps ticking
//!   (this is what makes AEC reference timing well-defined).
//!
//! Why software reference (not `hw:0,3` loopback): the LOOPBACK-A device on
//! this SoC can only source from PDMIN/TDMIN inputs — none of the mixer combos
//! we probed accept TDMOUT as a source without driver-level setup we don't
//! control. Software tap gives us sub-millisecond reference alignment, which
//! is well inside any AEC filter's tap length.
//!
//! Latency note: end-to-end speaker latency is dominated by ALSA's playback
//! buffer (period_frames × buffer_periods), not by the reference-delivery
//! mechanism. Tune `PlaybackConfig::period_frames` / `buffer_periods` to trade
//! stability for latency.

pub mod dsp;
pub mod preset;

use crate::audio_config::SampleFormat;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{info, warn};
use waved_state::AudioState;

#[derive(Debug, Clone)]
pub struct PlaybackConfig {
    pub device: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub format: SampleFormat,
    pub period_frames: usize,
    pub buffer_periods: u32,
    /// Initial preset on spawn. Default `PlaybackPreset::Voice`.
    pub initial_preset: crate::playback::preset::PlaybackPreset,
}

#[derive(Debug, Error)]
pub enum PlaybackError {
    #[cfg(feature = "capture-alsa")]
    #[error("alsa error: {0}")]
    Alsa(#[from] alsa::Error),
    #[error("config invalid: {0}")]
    Invalid(String),
    #[error("playback thread stopped")]
    Stopped,
}

#[derive(Debug, Default)]
pub struct PlaybackStats {
    pub frames_written: AtomicU64,
    pub frames_silence: AtomicU64,
    pub underruns: AtomicU64,
}

/// DSP runtime metrics exposed for `/tmp/audio_stats.json` consumers.
/// Single writer (playback thread), single reader (stats collector) — Relaxed
/// ordering is sufficient.
#[derive(Debug, Default)]
pub struct DspMetrics {
    /// Per-band MBAGC gain (dB), packed `[b0, b1, b2, b3]` as fixed-point
    /// i32 = `dB × 100`. Atomic updates — readers may see partial state for
    /// a single 500 ms stats tick, but values rarely move that fast.
    pub mbagc_gain_db_x100: [AtomicI32; 4],
    /// Limiter peak attenuation over the most recent block (`dB × 100`,
    /// negative = attenuating).
    pub limiter_reduction_db_x100: AtomicI32,
    /// Recording-path limiter peak attenuation over the most recent block
    /// (`dB × 100`, negative = attenuating). Written by the audio service
    /// after `apply_gain`; 0 when no limiting was needed.
    pub recording_limiter_reduction_db_x100: AtomicI32,
}

pub struct PlaybackService {
    cmd_tx: mpsc::Sender<Vec<i16>>,
    preset_tx: mpsc::Sender<crate::playback::preset::PlaybackPreset>,
    ref_tx: broadcast::Sender<Vec<i16>>,
    channels: u32,
    period_frames: usize,
    sample_rate: u32,
    stats: Arc<PlaybackStats>,
    dsp_metrics: Arc<DspMetrics>,
}

impl PlaybackService {
    /// Submit a batch for playback. The batch will be written to ALSA at the
    /// next period boundary; silence fills the gap if this is called sparsely.
    /// Returns immediately; returns Err if the playback thread has exited.
    pub fn play_batch(&self, samples: Vec<i16>) -> Result<(), PlaybackError> {
        self.cmd_tx
            .try_send(samples)
            .map_err(|_| PlaybackError::Stopped)
    }

    /// Submit a preset switch. Drops silently if the playback thread has exited.
    pub fn set_preset(&self, preset: crate::playback::preset::PlaybackPreset) {
        let _ = self.preset_tx.try_send(preset);
    }

    /// Subscribe to the reference signal — a copy of every batch written to
    /// ALSA (including silence), in wall-clock order.
    pub fn subscribe_reference(&self) -> broadcast::Receiver<Vec<i16>> {
        self.ref_tx.subscribe()
    }

    pub fn stats(&self) -> Arc<PlaybackStats> {
        Arc::clone(&self.stats)
    }

    pub fn dsp_metrics(&self) -> Arc<DspMetrics> {
        Arc::clone(&self.dsp_metrics)
    }

    pub fn channels(&self) -> u32 { self.channels }
    pub fn period_frames(&self) -> usize { self.period_frames }
    pub fn sample_rate(&self) -> u32 { self.sample_rate }

    /// Number of `play_batch` periods currently queued in the mpsc buffer.
    /// Used by the UAC2 downstream bridge for drift detection — drop a
    /// period when this grows past `buffer_periods`.
    pub fn queue_depth(&self) -> usize {
        self.cmd_tx.max_capacity() - self.cmd_tx.capacity()
    }
}

/// Run the playback loop synchronously on the current thread until `cmd_rx`
/// closes or an unrecoverable ALSA error occurs. Split out from the
/// spawn site for testability — mock sinks exercise the same loop.
pub fn run_playback_loop<S: PlaybackSink>(
    mut sink: S,
    mut cmd_rx: mpsc::Receiver<Vec<i16>>,
    mut preset_rx: mpsc::Receiver<crate::playback::preset::PlaybackPreset>,
    ref_tx: broadcast::Sender<Vec<i16>>,
    stats: Arc<PlaybackStats>,
    period_samples: usize,
    sample_rate: u32,
    initial_preset: crate::playback::preset::PlaybackPreset,
    dsp_metrics: Arc<DspMetrics>,
    state_rx: watch::Receiver<AudioState>,
) {
    use crate::playback::dsp::PlaybackDspChain;
    use crate::playback::preset::tuning_for;

    let silence: Vec<i16> = vec![0; period_samples];
    info!("playback loop starting: samples/period={period_samples}");

    let mut tuning = tuning_for(initial_preset);
    let mut chain = PlaybackDspChain::new(
        tuning.hpf_cutoff_hz,
        &tuning.eq,
        tuning.mbagc,
        tuning.limiter,
        sample_rate as f32,
    );

    loop {
        // Drain any pending preset switches; only the most recent matters.
        while let Ok(p) = preset_rx.try_recv() {
            tuning = tuning_for(p);
            chain.set_preset(
                tuning.hpf_cutoff_hz,
                &tuning.eq,
                tuning.mbagc,
                tuning.limiter,
            );
            info!("playback preset switched → {:?}", p);
        }

        let (batch, is_silence) = match cmd_rx.try_recv() {
            Ok(b) => (b, false),
            Err(mpsc::error::TryRecvError::Empty) => (silence.clone(), true),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                info!("playback cmd channel closed, stopping");
                return;
            }
        };
        // Pad/trim to period. Callers should normally submit period-sized
        // batches; we defensively reshape so the sink always gets whole periods.
        let mut normalized = if batch.len() == period_samples {
            batch
        } else if batch.len() < period_samples {
            let mut v = batch;
            v.resize(period_samples, 0);
            v
        } else {
            // Oversized: truncate and drop the tail.
            warn!("playback batch truncated: {} > {}", batch.len(), period_samples);
            batch.into_iter().take(period_samples).collect()
        };

        // SP#5: DSP applied before the ALSA write. AEC reference broadcast
        // taps the post-DSP buffer so the cancellation reference matches what
        // actually reaches the speaker.
        chain.process_i16(&mut normalized);

        // Snapshot post-DSP state for /tmp/audio_stats.json consumers.
        let mbagc = chain.last_mbagc_gain_db();
        for i in 0..4 {
            dsp_metrics.mbagc_gain_db_x100[i].store(
                (mbagc[i] * 100.0) as i32, Ordering::Relaxed,
            );
        }
        dsp_metrics.limiter_reduction_db_x100.store(
            (chain.last_limiter_reduction_db() * 100.0) as i32, Ordering::Relaxed,
        );

        // User volume + mute. Read once per batch from the watch channel; mute
        // forces a hard 0, otherwise scale by speaker_volume / 100. Linear for
        // now — macOS already log-tapers the slider before sending UAC2 SET_CUR
        // values, so a second log curve here would feel double-attenuated.
        // The AEC reference broadcast happens AFTER scaling so it reflects the
        // actual signal hitting the mics.
        let st = state_rx.borrow();
        let muted = st.speaker_muted;
        let volume_pct = st.speaker_volume.min(100);
        drop(st);
        if muted {
            for s in normalized.iter_mut() {
                *s = 0;
            }
        } else if volume_pct < 100 {
            let gain = volume_pct as f32 / 100.0;
            for s in normalized.iter_mut() {
                *s = ((*s as f32) * gain) as i16;
            }
        }

        match sink.write(&normalized) {
            Ok(()) => {
                if is_silence {
                    stats.frames_silence.fetch_add((period_samples / sink.channels() as usize) as u64, Ordering::Relaxed);
                } else {
                    stats.frames_written.fetch_add((period_samples / sink.channels() as usize) as u64, Ordering::Relaxed);
                }
                let _ = ref_tx.send(normalized);
            }
            Err(WriteError::Underrun) => {
                stats.underruns.fetch_add(1, Ordering::Relaxed);
                sink.recover();
            }
            Err(WriteError::Fatal(e)) => {
                warn!("playback fatal write: {e}");
                return;
            }
        }
    }
}

pub enum WriteError {
    /// EPIPE / xrun. Caller should call recover() and retry next period.
    Underrun,
    /// Unrecoverable error. Playback thread exits.
    Fatal(String),
}

/// Abstracts over ALSA and mock sinks for unit testing.
pub trait PlaybackSink: Send {
    fn write(&mut self, samples: &[i16]) -> Result<(), WriteError>;
    fn recover(&mut self);
    fn channels(&self) -> u32;
}

#[cfg(feature = "capture-alsa")]
pub use alsa_impl::{AlsaPlaybackSink, spawn_playback};

#[cfg(feature = "capture-alsa")]
mod alsa_impl {
    use super::*;
    use alsa::pcm::{Access, Format, HwParams, PCM};
    use alsa::{Direction, ValueOr};

    pub struct AlsaPlaybackSink {
        pcm: PCM,
        channels: u32,
    }

    fn configure_playback(pcm: &PCM, cfg: &PlaybackConfig) -> Result<(), alsa::Error> {
        let hwp = HwParams::any(pcm)?;
        hwp.set_access(Access::RWInterleaved)?;
        let fmt = match cfg.format {
            SampleFormat::S16_LE => Format::s16(),
            SampleFormat::S24_LE => Format::s24_3(),
            SampleFormat::S32_LE => Format::s32(),
        };
        hwp.set_format(fmt)?;
        hwp.set_channels(cfg.channels)?;
        hwp.set_rate(cfg.sample_rate, ValueOr::Nearest)?;
        hwp.set_period_size(cfg.period_frames as i64, ValueOr::Nearest)?;
        hwp.set_buffer_size((cfg.period_frames * cfg.buffer_periods as usize) as i64)?;
        pcm.hw_params(&hwp)?;
        pcm.prepare()?;
        Ok(())
    }

    impl AlsaPlaybackSink {
        pub fn open(cfg: &PlaybackConfig) -> Result<Self, PlaybackError> {
            let pcm = PCM::new(&cfg.device, Direction::Playback, false)?;
            configure_playback(&pcm, cfg)?;
            info!(
                "alsa playback opened: device={} ch={} rate={} period={} buffer={}",
                cfg.device,
                cfg.channels,
                cfg.sample_rate,
                cfg.period_frames,
                cfg.period_frames * cfg.buffer_periods as usize
            );
            Ok(Self {
                pcm,
                channels: cfg.channels,
            })
        }
    }

    impl PlaybackSink for AlsaPlaybackSink {
        fn write(&mut self, samples: &[i16]) -> Result<(), WriteError> {
            let io = self.pcm.io_i16().map_err(|e| WriteError::Fatal(e.to_string()))?;
            // writei may be partial; loop until the full batch is drained.
            let mut written = 0;
            while written < samples.len() {
                match io.writei(&samples[written..]) {
                    Ok(0) => break,
                    Ok(frames) => written += frames * self.channels as usize,
                    Err(e) => {
                        if e.errno() == libc::EPIPE {
                            drop(io);
                            return Err(WriteError::Underrun);
                        }
                        return Err(WriteError::Fatal(e.to_string()));
                    }
                }
            }
            Ok(())
        }

        fn recover(&mut self) {
            let _ = self.pcm.prepare();
        }

        fn channels(&self) -> u32 {
            self.channels
        }
    }

    /// Convenience: open the ALSA device, spawn a SCHED_FIFO thread running
    /// the playback loop, and return a `PlaybackService` handle. The thread
    /// exits when `play_batch` senders all drop.
    pub fn spawn_playback(
        cfg: PlaybackConfig,
        state_rx: watch::Receiver<AudioState>,
    ) -> Result<PlaybackService, PlaybackError> {
        let sink = AlsaPlaybackSink::open(&cfg)?;
        let (cmd_tx, cmd_rx) = mpsc::channel::<Vec<i16>>(64);
        let (preset_tx, preset_rx) = mpsc::channel::<crate::playback::preset::PlaybackPreset>(8);
        let (ref_tx, _) = broadcast::channel::<Vec<i16>>(32);
        let stats = Arc::new(PlaybackStats::default());
        let dsp_metrics = Arc::new(DspMetrics::default());
        let period_samples = cfg.period_frames * cfg.channels as usize;
        let sample_rate = cfg.sample_rate;
        let initial_preset = cfg.initial_preset;

        let ref_tx_thread = ref_tx.clone();
        let stats_thread = Arc::clone(&stats);
        let dsp_metrics_thread = Arc::clone(&dsp_metrics);
        std::thread::Builder::new()
            .name("audio-playback".into())
            .spawn(move || {
                super::set_realtime_priority();
                run_playback_loop(
                    sink, cmd_rx, preset_rx, ref_tx_thread, stats_thread,
                    period_samples, sample_rate, initial_preset, dsp_metrics_thread,
                    state_rx,
                );
            })
            .map_err(|e| PlaybackError::Invalid(format!("spawn playback thread: {e}")))?;

        Ok(PlaybackService {
            cmd_tx,
            preset_tx,
            ref_tx,
            channels: cfg.channels,
            period_frames: cfg.period_frames,
            sample_rate: cfg.sample_rate,
            stats,
            dsp_metrics,
        })
    }
}

/// Best-effort SCHED_FIFO bump for the current thread (see capture::thread).
/// Only referenced from the alsa spawn path so guarded by the same cfg.
#[cfg(all(target_os = "linux", feature = "capture-alsa"))]
fn set_realtime_priority() {
    let param = libc::sched_param { sched_priority: 20 };
    // SAFETY: param lives for the duration of this call.
    let rc = unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) };
    if rc != 0 {
        let errno = std::io::Error::last_os_error();
        warn!("playback SCHED_FIFO failed: {errno}");
    } else {
        info!("playback thread scheduler: SCHED_FIFO priority 20");
    }
}

#[cfg(all(not(target_os = "linux"), feature = "capture-alsa"))]
fn set_realtime_priority() {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::preset::PlaybackPreset;
    use std::sync::Mutex;

    /// MockSink with a per-write counter so tests can deterministically stop
    /// the loop after N writes — avoids racy sleeps and broadcast overruns.
    struct MockSink {
        channels: u32,
        written: Arc<Mutex<Vec<Vec<i16>>>>,
        stop_after: usize,
        fail_once: bool,
    }

    impl PlaybackSink for MockSink {
        fn write(&mut self, samples: &[i16]) -> Result<(), WriteError> {
            if self.fail_once {
                self.fail_once = false;
                return Err(WriteError::Underrun);
            }
            let mut w = self.written.lock().unwrap();
            w.push(samples.to_vec());
            if w.len() >= self.stop_after {
                // Shut the loop down cleanly once we've captured enough samples
                // for the test — otherwise the unpaced mock spins fast enough
                // to overflow the broadcast ring.
                return Err(WriteError::Fatal("mock stop".into()));
            }
            Ok(())
        }
        fn recover(&mut self) {}
        fn channels(&self) -> u32 {
            self.channels
        }
    }

    fn run_with_mock(
        stop_after: usize,
        fail_once: bool,
        prefill: Vec<Vec<i16>>,
        period_samples: usize,
        channels: u32,
    ) -> (Vec<Vec<i16>>, Arc<PlaybackStats>, Vec<Vec<i16>>) {
        let written = Arc::new(Mutex::new(Vec::new()));
        let sink = MockSink {
            channels,
            written: Arc::clone(&written),
            stop_after,
            fail_once,
        };
        let (cmd_tx, cmd_rx) = mpsc::channel::<Vec<i16>>(16);
        for b in prefill {
            cmd_tx.try_send(b).unwrap();
        }
        let (preset_tx, preset_rx) = mpsc::channel::<PlaybackPreset>(8);
        // Drop preset_tx immediately so no preset switches happen during the test.
        drop(preset_tx);
        let (ref_tx, mut ref_rx) = broadcast::channel::<Vec<i16>>(64);
        let stats = Arc::new(PlaybackStats::default());
        let stats_thread = Arc::clone(&stats);

        // Tests run with full volume + unmuted so post-DSP plumbing is observable.
        let (_state_tx, state_rx) = watch::channel(AudioState {
            speaker_volume: 100,
            speaker_muted: false,
            ..AudioState::default()
        });
        let handle = std::thread::spawn(move || {
            run_playback_loop(
                sink, cmd_rx, preset_rx, ref_tx, stats_thread,
                period_samples, 48_000, PlaybackPreset::Voice,
                Arc::new(DspMetrics::default()),
                state_rx,
            )
        });
        // Mock returns Fatal when stop_after is reached, which exits the loop;
        // join waits for it.
        handle.join().unwrap();
        drop(cmd_tx);

        let mut ref_batches = Vec::new();
        while let Ok(b) = ref_rx.try_recv() {
            ref_batches.push(b);
        }
        let final_written = written.lock().unwrap().clone();
        (final_written, stats, ref_batches)
    }

    #[test]
    fn silence_when_queue_empty() {
        let (batches, stats, ref_batches) = run_with_mock(3, false, vec![], 8, 2);
        assert!(batches.len() >= 3);
        for b in &batches {
            assert_eq!(b.len(), 8);
            // DSP on silence emits silence (HPF on zero is zero, etc.).
            assert!(b.iter().all(|&s| s == 0));
        }
        assert!(stats.frames_silence.load(Ordering::Relaxed) > 0);
        assert_eq!(stats.frames_written.load(Ordering::Relaxed), 0);
        assert!(!ref_batches.is_empty());
    }

    #[test]
    fn submitted_batch_is_played_and_referenced() {
        let payload = vec![1i16, 2, 3, 4, 5, 6, 7, 8];
        let (batches, stats, ref_batches) =
            run_with_mock(2, false, vec![payload.clone()], 8, 2);
        // Post-SP#5: the limiter's 96-frame lookahead means the first batch
        // emerges as silence during warmup. We can't byte-compare. Instead,
        // verify the plumbing: the batch flowed through DSP, advanced stats,
        // and was published on the broadcast.
        assert_eq!(batches.len(), 2, "two batches written before mock stop");
        assert_eq!(batches[0].len(), payload.len(), "batch length preserved");
        assert!(stats.frames_written.load(Ordering::Relaxed) > 0,
                "frames_written advanced");
        assert!(!ref_batches.is_empty(),
                "broadcast received post-DSP frame(s)");
        // Sanity: broadcast and sink see the same data (same post-DSP buffer).
        assert_eq!(ref_batches[0].len(), batches[0].len());
    }

    #[test]
    fn short_batch_is_padded_with_silence() {
        let (batches, _, _) = run_with_mock(1, false, vec![vec![99, 99]], 8, 2);
        let first = &batches[0];
        assert_eq!(first.len(), 8, "short batch padded to period");
        // Don't assert specific sample values — DSP warmup obscures the first batch.
    }

    #[test]
    fn underrun_increments_stat_and_continues() {
        // First write returns Underrun; loop recovers and writes on subsequent iterations.
        let (batches, stats, _) = run_with_mock(3, true, vec![], 4, 1);
        assert_eq!(stats.underruns.load(Ordering::Relaxed), 1);
        assert!(batches.len() >= 3);
    }

    #[test]
    fn queue_depth_reports_pending_batches() {
        // Build a stand-alone PlaybackService stub by exposing the constructor
        // shape used by spawn_playback. The simplest test is to construct a
        // pair of channels manually and call the same accessor.
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<i16>>(4);
        let (preset_tx, _preset_rx) =
            mpsc::channel::<crate::playback::preset::PlaybackPreset>(2);
        let (ref_tx, _) = broadcast::channel::<Vec<i16>>(2);
        let svc = PlaybackService {
            cmd_tx: cmd_tx.clone(),
            preset_tx,
            ref_tx,
            channels: 2,
            period_frames: 1024,
            sample_rate: 48_000,
            stats: Arc::new(PlaybackStats::default()),
            dsp_metrics: Arc::new(DspMetrics::default()),
        };
        assert_eq!(svc.queue_depth(), 0);
        cmd_tx.try_send(vec![0; 4]).unwrap();
        cmd_tx.try_send(vec![0; 4]).unwrap();
        assert_eq!(svc.queue_depth(), 2);
        // Read one back via the receiver — depth drops by one.
        let _ = cmd_rx.try_recv().ok();
        assert_eq!(svc.queue_depth(), 1);
    }
}
