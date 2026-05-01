//! UAC2 downstream bridge — host audio → speaker.
//!
//! Reads from `hw:1,0` (kernel `f_uac2` capture endpoint that mirrors what
//! the host writes to the UAC2 OUT terminal) and submits each period to the
//! `PlaybackService`. The playback loop's SP#5 DSP chain processes the
//! samples and broadcasts them as the AEC reference, so AEC sees host audio
//! exactly as it reaches the speaker.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use tracing::{info, warn};

use crate::playback::PlaybackService;
use super::UacStats;

#[derive(Debug, Clone)]
pub struct UacBridgeConfig {
    pub device: String,         // e.g. "hw:1,0"
    pub channels: u32,          // 2
    pub sample_rate: u32,       // 48000
    pub period_frames: usize,   // 1024
    pub buffer_periods: u32,    // 4
}

pub struct UacBridge {
    handle: Option<JoinHandle<()>>,
}

impl UacBridge {
    /// Spawn the downstream thread. Returns a handle whose `Drop` joins the
    /// thread (the thread exits when `playback.play_batch` errors with
    /// `Stopped`, i.e. when `PlaybackService` is dropped).
    pub fn spawn(
        cfg: UacBridgeConfig,
        playback: Arc<PlaybackService>,
        stats: Arc<UacStats>,
    ) -> Self {
        let handle = std::thread::Builder::new()
            .name("uac-downstream".into())
            .spawn(move || {
                set_realtime_priority(18);
                run_downstream(cfg, playback, stats);
            })
            .expect("spawn uac-downstream thread");
        Self { handle: Some(handle) }
    }

    pub fn join(mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run_downstream(
    cfg: UacBridgeConfig,
    playback: Arc<PlaybackService>,
    stats: Arc<UacStats>,
) {
    info!("uac-downstream starting: {}", cfg.device);
    loop {
        match open_capture(&cfg) {
            Ok(pcm) => {
                if let Err(e) = drain_loop(&pcm, &cfg, &playback, &stats) {
                    warn!("uac-downstream drain stopped: {e}");
                }
                stats.downstream_active.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                warn!("uac-downstream open failed: {e}; retrying in 1 s");
            }
        }
        // Either ALSA closed or open failed — back off and retry.
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn open_capture(cfg: &UacBridgeConfig) -> Result<PCM, alsa::Error> {
    let pcm = PCM::new(&cfg.device, Direction::Capture, false)?;
    {
        let hwp = HwParams::any(&pcm)?;
        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_format(Format::s16())?;
        hwp.set_channels(cfg.channels)?;
        hwp.set_rate(cfg.sample_rate, ValueOr::Nearest)?;
        hwp.set_period_size(cfg.period_frames as i64, ValueOr::Nearest)?;
        hwp.set_buffer_size((cfg.period_frames * cfg.buffer_periods as usize) as i64)?;
        pcm.hw_params(&hwp)?;
    }
    pcm.prepare()?;
    pcm.start()?;
    Ok(pcm)
}

fn drain_loop(
    pcm: &PCM,
    cfg: &UacBridgeConfig,
    playback: &Arc<PlaybackService>,
    stats: &Arc<UacStats>,
) -> Result<(), alsa::Error> {
    let frame_samples = cfg.channels as usize;
    let period_samples = cfg.period_frames * frame_samples;
    let mut buf: Vec<i16> = vec![0; period_samples];
    let drop_threshold = cfg.buffer_periods as usize;

    loop {
        let io = pcm.io_i16()?;
        match io.readi(&mut buf) {
            Ok(0) => {
                stats.downstream_active.store(false, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(frames) => {
                stats.downstream_active.store(true, Ordering::Relaxed);
                if playback.queue_depth() >= drop_threshold {
                    stats.downstream_drops.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let to_send = frames * frame_samples;
                let _ = playback.play_batch(buf[..to_send].to_vec());
            }
            Err(e) => match e.errno() {
                libc::EAGAIN => {
                    stats.downstream_active.store(false, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(50));
                }
                libc::EPIPE => {
                    stats.downstream_underruns.fetch_add(1, Ordering::Relaxed);
                    pcm.prepare()?;
                }
                _ => return Err(e),
            },
        }
    }
}

#[cfg(target_os = "linux")]
fn set_realtime_priority(prio: i32) {
    let param = libc::sched_param { sched_priority: prio };
    let rc = unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) };
    if rc != 0 {
        let errno = std::io::Error::last_os_error();
        warn!("uac-downstream SCHED_FIFO failed: {errno}");
    } else {
        info!("uac-downstream scheduler: SCHED_FIFO priority {prio}");
    }
}

#[cfg(not(target_os = "linux"))]
fn set_realtime_priority(_prio: i32) {}
