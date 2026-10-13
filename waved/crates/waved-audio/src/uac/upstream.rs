//! UAC2 upstream sink — mic → host.
//!
//! Subscribes to `AudioService::subscribe_processed()` (post-AEC,
//! post-beamform, post-NS mono `f32`). Writes the frame's uplink pair
//! (`ProcessedAudioFrame::uplink_interleaved_i16`): the rebuilt Stereo pair
//! in Stereo mode, the mono signal on both channels otherwise. The pair goes
//! out as interleaved S16 to `hw:1,0` playback (kernel `f_uac2` IN terminal —
//! what the host reads as the microphone).
//!
//! While the computer does not read the microphone the PCM stays open and
//! untouched, so a card removal alone would never close it, and the f_uac2
//! unbind waits for it: the thread follows USB storage's hold on `hw:1`
//! (`crate::uac_hold`), looked at on every frame.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::processed_frame::ProcessedAudioFrame;
use crate::uac_hold::{self, HoldFollower, Pumped};
use super::UacStats;
use crate::cushion::Cushion;

#[derive(Debug, Clone)]
pub struct UacUpstreamConfig {
    pub device: String,
    pub channels: u32,         // 2
    pub sample_rate: u32,      // 48000
    pub period_frames: usize,  // 1024 — must match ProcessedAudioFrame size
    pub buffer_periods: u32,   // 4
    /// USB storage's hold on `hw:1` (`StateBus::uac_hold`); `None`: no
    /// gadget switch to follow.
    pub hold: Option<waved_state::GadgetHold>,
}

pub struct UacUpstreamSink {
    handle: Option<JoinHandle<()>>,
}

impl UacUpstreamSink {
    pub fn spawn(
        cfg: UacUpstreamConfig,
        rx: broadcast::Receiver<ProcessedAudioFrame>,
        stats: Arc<UacStats>,
    ) -> Self {
        // Registered before the thread runs: a switch from now on waits for it.
        let hold = HoldFollower::new(cfg.hold.as_ref().map(|hold| hold.register("the UAC upstream")));
        let handle = std::thread::Builder::new()
            .name("uac-upstream".into())
            .spawn(move || {
                set_realtime_priority(18);
                run_upstream(cfg, rx, stats, hold);
            })
            .expect("spawn uac-upstream thread");
        Self { handle: Some(handle) }
    }

    pub fn join(mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run_upstream(
    cfg: UacUpstreamConfig,
    mut rx: broadcast::Receiver<ProcessedAudioFrame>,
    stats: Arc<UacStats>,
    hold: HoldFollower,
) {
    info!("uac-upstream starting: {}", cfg.device);
    uac_hold::run(
        "uac-upstream",
        &hold,
        Duration::from_secs(1),
        || open_playback(&cfg),
        |pcm| match pump_loop(pcm, &cfg, &mut rx, &stats, &hold) {
            Ok(pumped) => pumped,
            Err(e) => Pumped::Stopped(Some(e)),
        },
        std::thread::sleep,
    );
    info!("uac-upstream: the processed stream ended");
}

fn open_playback(cfg: &UacUpstreamConfig) -> Result<PCM, alsa::Error> {
    let pcm = PCM::new(&cfg.device, Direction::Playback, false)?;
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
    Ok(pcm)
}

fn pump_loop(
    pcm: &PCM,
    cfg: &UacUpstreamConfig,
    rx: &mut broadcast::Receiver<ProcessedAudioFrame>,
    stats: &Arc<UacStats>,
    hold: &HoldFollower,
) -> Result<Pumped<alsa::Error>, alsa::Error> {
    let mut stereo: Vec<i16> = Vec::with_capacity(cfg.period_frames * cfg.channels as usize);
    let silence = vec![0i16; cfg.period_frames * cfg.channels as usize];
    let mut cushion = Cushion::new(cfg.buffer_periods);
    loop {
        match rx.blocking_recv() {
            Ok(frame) => {
                // Every frame (about 21 ms), active or not: USB storage is
                // about to unbind; the caller closes the PCM.
                if hold.is_held() {
                    return Ok(Pumped::Held);
                }
                // Pre-attach: kernel buffer fills and writei blocks/errs, which
                // backs up the broadcast and inflates upstream_drops. Drain the
                // frame and skip the write until a host opens the endpoint.
                if !stats.upstream_active.load(Ordering::Relaxed) {
                    continue;
                }
                frame.uplink_interleaved_i16(&mut stereo);
                let io = pcm.io_i16()?;
                for _ in 0..cushion.take() {
                    // An error here shows up on the frame's write below.
                    let _ = io.writei(&silence);
                }
                match io.writei(&stereo) {
                    Ok(_) => {}
                    Err(e) => match e.errno() {
                        libc::EBADFD => {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        libc::EPIPE => {
                            stats.upstream_underruns.fetch_add(1, Ordering::Relaxed);
                            pcm.prepare()?;
                            cushion.restarted();
                        }
                        _ => return Err(e),
                    },
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                stats.upstream_drops.fetch_add(n, Ordering::Relaxed);
                warn!("uac-upstream lagged {n} frames");
            }
            Err(broadcast::error::RecvError::Closed) => return Ok(Pumped::Done),
        }
    }
}

#[cfg(target_os = "linux")]
fn set_realtime_priority(prio: i32) {
    let param = libc::sched_param { sched_priority: prio };
    let rc = unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) };
    if rc != 0 {
        let errno = std::io::Error::last_os_error();
        warn!("uac-upstream SCHED_FIFO failed: {errno}");
    }
}

#[cfg(not(target_os = "linux"))]
fn set_realtime_priority(_prio: i32) {}

