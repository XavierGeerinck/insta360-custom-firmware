//! CaptureThread: generic over AudioFrameSource. Pulls complete AudioFrames
//! from the source and emits them on an mpsc. Drops the newest frame on
//! backpressure (policy chosen to preserve contiguous history for future AEC
//! reference alignment).

use super::AudioFrameSource;
use crate::callbacks::AudioFrame;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::mpsc;
use tracing::{info, warn, error};

pub struct CaptureStats {
    pub frames_emitted: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub xrun_count: AtomicU64,
}

impl CaptureStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            frames_emitted: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            xrun_count: AtomicU64::new(0),
        })
    }
}

/// Best-effort SCHED_FIFO bump for the current thread. Requires CAP_SYS_NICE
/// (or root). Silent on platforms where libc isn't POSIX realtime.
#[cfg(target_os = "linux")]
fn set_realtime_priority() {
    let param = libc::sched_param { sched_priority: 20 };
    // SAFETY: sched_setscheduler reads `param` by pointer; scope is this call.
    let rc = unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) };
    if rc != 0 {
        let errno = std::io::Error::last_os_error();
        warn!("capture thread SCHED_FIFO failed: {errno}; falling back to SCHED_OTHER");
    } else {
        info!("capture thread scheduler: SCHED_FIFO priority 20");
    }
}

#[cfg(not(target_os = "linux"))]
fn set_realtime_priority() {}

/// Runs the capture loop, consuming the source to EOF or until stop_flag.
///
/// Repeatedly calls `source.next_frame()` to pull complete AudioFrames
/// (which already have timestamps and sample conversion done internally).
/// Output frames on `frame_tx` as they arrive from the source.
pub fn run_capture_loop<S: AudioFrameSource>(
    mut source: S,
    frame_tx: mpsc::Sender<AudioFrame>,
    stop_flag: Arc<AtomicBool>,
    stats: Arc<CaptureStats>,
) {
    let sample_rate = source.sample_rate();
    let mut consecutive_errors = 0u32;
    set_realtime_priority();
    info!("capture loop starting: sr={sample_rate}");

    while !stop_flag.load(Ordering::Relaxed) {
        match source.next_frame() {
            Ok(None) => {
                info!("capture source EOF");
                return;
            }
            Ok(Some(frame)) => {
                consecutive_errors = 0;
                match frame_tx.try_send(frame) {
                    Ok(()) => { stats.frames_emitted.fetch_add(1, Ordering::Relaxed); }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        let n = stats.frames_dropped.fetch_add(1, Ordering::Relaxed) + 1;
                        if n % 50 == 1 {
                            warn!("capture backpressure: {n} frames dropped");
                        }
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        info!("frame_tx closed, stopping capture");
                        return;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                let n = stats.xrun_count.fetch_add(1, Ordering::Relaxed) + 1;
                // Logging is on the capture thread: rate-limit so the stderr
                // lock doesn't starve us into more xruns.
                if n <= 5 || n.is_power_of_two() || n % 500 == 0 {
                    warn!("xrun #{n} ({e})");
                }
                // Source is expected to recover internally; we just continue.
            }
            Err(e) => {
                consecutive_errors += 1;
                error!("capture read error: {e}");
                if consecutive_errors > 3 {
                    error!("too many read errors, stopping capture");
                    return;
                }
            }
        }
    }
    info!("capture loop stopped (stop flag set)");
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::mock::MockPcmSource;
    use super::super::single_stream::SingleStreamFrameSource;

    #[tokio::test]
    async fn emits_one_frame_per_batch() {
        let mock = MockPcmSource::new(1, 48000)
            .with_batch(vec![10 << 16, 20 << 16]); // 2 samples, 1 channel = period_frames=2
        let src = SingleStreamFrameSource::new(mock, 2, 1); // period_frames=2, channels=1
        let (tx, mut rx) = mpsc::channel(8);
        let stats = CaptureStats::new();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let stats = Arc::clone(&stats);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                run_capture_loop(src, tx, stop, stats);
            })
        };
        let frame = rx.recv().await.expect("one frame");
        assert_eq!(frame.channels, 1);
        assert_eq!(frame.data, vec![10i16, 20i16]);
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        assert_eq!(stats.frames_emitted.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn drops_frames_on_backpressure() {
        // Queue 100 single-sample batches
        let mut mock = MockPcmSource::new(1, 48000);
        for _ in 0..100 {
            mock = mock.with_batch(vec![0 << 16]);
        }
        let src = SingleStreamFrameSource::new(mock, 1, 1); // period_frames=1, channels=1
        let (tx, _rx) = mpsc::channel(2);
        // Don't drain _rx — force backpressure.
        let stats = CaptureStats::new();
        let stop = Arc::new(AtomicBool::new(false));
        {
            let stats = Arc::clone(&stats);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                run_capture_loop(src, tx, stop, stats);
            }).join().unwrap();
        }
        let emitted = stats.frames_emitted.load(Ordering::Relaxed);
        let dropped = stats.frames_dropped.load(Ordering::Relaxed);
        assert!(emitted <= 2, "should emit at most cap=2 frames, got {emitted}");
        assert!(dropped >= 95, "should drop most, got {dropped}");
        let _ = stop;
    }

    #[tokio::test]
    async fn xrun_increments_counter_and_continues() {
        use std::io::ErrorKind;
        let mock = MockPcmSource::new(1, 48000)
            .with_batch(vec![1 << 16])
            .with_error(ErrorKind::BrokenPipe, "xrun")
            .with_batch(vec![2 << 16]);
        let src = SingleStreamFrameSource::new(mock, 1, 1); // period_frames=1, channels=1
        let (tx, mut rx) = mpsc::channel(8);
        let stats = CaptureStats::new();
        let stop = Arc::new(AtomicBool::new(false));
        {
            let stats = Arc::clone(&stats);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                run_capture_loop(src, tx, stop, stats);
            }).join().unwrap();
        }
        // Should have emitted 2 frames (before and after the xrun).
        let frame1 = rx.try_recv().expect("frame 1");
        let frame2 = rx.try_recv().expect("frame 2");
        assert_eq!(frame1.data, vec![1i16]);
        assert_eq!(frame2.data, vec![2i16]);
        assert_eq!(stats.xrun_count.load(Ordering::Relaxed), 1);
        assert_eq!(stats.frames_emitted.load(Ordering::Relaxed), 2);
    }
}
