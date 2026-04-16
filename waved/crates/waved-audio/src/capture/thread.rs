//! CaptureThread: generic over PcmSource. Reads raw 10-ch i32 from the
//! source, downmixes to 8-ch i16 via ChannelMap, timestamps, and emits
//! AudioFrames on an mpsc. Drops the newest frame on backpressure (policy
//! chosen to preserve contiguous history for future AEC reference alignment).

use super::{ChannelMap, PcmSource, downmix_interleaved};
use crate::callbacks::AudioFrame;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;
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

/// Runs the capture loop, consuming the source to EOF or until stop_flag.
///
/// `period_frames` is the number of frames read per iteration (ALSA period
/// size; ~5.3ms at 48kHz with 256). Output frames on `frame_tx` are one
/// period each, interleaved 8-ch i16 after downmix.
pub fn run_capture_loop<P: PcmSource>(
    mut source: P,
    map: ChannelMap,
    period_frames: usize,
    frame_tx: mpsc::Sender<AudioFrame>,
    stop_flag: Arc<AtomicBool>,
    stats: Arc<CaptureStats>,
) {
    let total_channels = map.total_channels as usize;
    let mic_channels = map.physical_mics.len();
    let mut raw = vec![0i32; period_frames * total_channels];
    let mut out = vec![0i16; period_frames * mic_channels];
    let start = Instant::now();
    let sample_rate = source.sample_rate();
    let mut consecutive_errors = 0u32;
    info!(
        "capture loop starting: period={period_frames} frames, mics={mic_channels}, sr={sample_rate}"
    );

    while !stop_flag.load(Ordering::Relaxed) {
        match source.read(&mut raw) {
            Ok(0) => {
                info!("capture source EOF");
                return;
            }
            Ok(n) if n == raw.len() => {
                consecutive_errors = 0;
                if let Err(e) = downmix_interleaved(&raw, &mut out, &map) {
                    error!("downmix failed: {e}");
                    continue;
                }
                let frame = AudioFrame {
                    data: out.clone(),
                    sample_rate,
                    channels: mic_channels as u16,
                    timestamp_ms: start.elapsed().as_millis() as u64,
                };
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
            Ok(partial) => {
                warn!("partial capture read: {partial}/{} samples", raw.len());
                consecutive_errors += 1;
                if consecutive_errors > 5 {
                    error!("too many partial reads, stopping capture");
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                let n = stats.xrun_count.fetch_add(1, Ordering::Relaxed) + 1;
                warn!("xrun #{n} ({e})");
                // AlsaPcmSource is expected to call snd_pcm_prepare internally
                // and return the next read cleanly. We just continue.
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
    use super::super::channel_map::ChannelMap;

    fn tiny_map() -> ChannelMap {
        ChannelMap::from_json(
            r#"{"version":1,"total_channels":2,"physical_mics":[0],"reference_channels":[1]}"#
        ).unwrap()
    }

    #[tokio::test]
    async fn emits_one_frame_per_batch() {
        let src = MockPcmSource::new(2, 48000)
            .with_batch(vec![10 << 16, 99 << 16, 20 << 16, 99 << 16]); // 2 frames
        let (tx, mut rx) = mpsc::channel(8);
        let stats = CaptureStats::new();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let stats = Arc::clone(&stats);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                run_capture_loop(src, tiny_map(), 2, tx, stop, stats);
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
        // Queue 100 small batches of 1 frame each into a cap=2 channel.
        let mut src = MockPcmSource::new(2, 48000);
        for _ in 0..100 {
            src = src.with_batch(vec![0, 0]);
        }
        let (tx, _rx) = mpsc::channel(2);
        // Don't drain rx — force backpressure.
        let stats = CaptureStats::new();
        let stop = Arc::new(AtomicBool::new(false));
        {
            let stats = Arc::clone(&stats);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                run_capture_loop(src, tiny_map(), 1, tx, stop, stats);
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
        let src = MockPcmSource::new(2, 48000)
            .with_batch(vec![1 << 16, 99 << 16])
            .with_error(ErrorKind::BrokenPipe, "xrun")
            .with_batch(vec![2 << 16, 99 << 16]);
        let (tx, mut rx) = mpsc::channel(8);
        let stats = CaptureStats::new();
        let stop = Arc::new(AtomicBool::new(false));
        {
            let stats = Arc::clone(&stats);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                run_capture_loop(src, tiny_map(), 1, tx, stop, stats);
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
