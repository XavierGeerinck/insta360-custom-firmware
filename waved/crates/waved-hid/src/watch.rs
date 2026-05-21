//! State-watcher glue tasks: subscribe to AudioState and emit HidEvents
//! per integer-step delta of speaker_volume and per edge of speaker_muted.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::warn;

use waved_state::AudioState;

use crate::events::HidEvent;
use crate::stats::HidStats;

/// Subscribe to `audio_rx` and emit one `ConsumerVolumeStep` per integer
/// step delta of `speaker_volume`. Rate-capped at `volume_step_rate_hz`
/// events per second (paced via `tokio::time::sleep`, so a normal burst
/// is slowed, not dropped). If the downstream mpsc is full, excess
/// events ARE dropped and `stats.glue_drops` is incremented.
///
/// Returns the spawned task handle for the caller to keep alive
/// (dropping it has no effect; the task itself runs until the watch
/// channel closes).
pub fn spawn_volume_watch(
    mut audio_rx: watch::Receiver<AudioState>,
    events: mpsc::Sender<HidEvent>,
    stats: Arc<HidStats>,
    volume_step_rate_hz: u32,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last = audio_rx.borrow().speaker_volume as i32;
        let min_gap = if volume_step_rate_hz == 0 {
            Duration::from_millis(0)
        } else {
            Duration::from_millis(1000 / volume_step_rate_hz as u64)
        };
        while audio_rx.changed().await.is_ok() {
            let now = audio_rx.borrow().speaker_volume as i32;
            let delta = now - last;
            if delta == 0 {
                continue;
            }
            let up = delta > 0;
            for _ in 0..delta.unsigned_abs() {
                if events.try_send(HidEvent::ConsumerVolumeStep { up }).is_err() {
                    stats.glue_drops.fetch_add(1, Ordering::Relaxed);
                    warn!("hid: volume_watch dropped event (queue full)");
                }
                if min_gap > Duration::ZERO {
                    tokio::time::sleep(min_gap).await;
                }
            }
            last = now;
        }
    })
}

/// Subscribe to `audio_rx` and emit `ConsumerMute` on each transition of
/// `speaker_muted`. Additionally emits `TelephonyPhoneMute` when the
/// snapshot at the moment of the edge has `call_off_hook == true`.
pub fn spawn_mute_watch(
    mut audio_rx: watch::Receiver<AudioState>,
    events: mpsc::Sender<HidEvent>,
    stats: Arc<HidStats>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last = audio_rx.borrow().speaker_muted;
        while audio_rx.changed().await.is_ok() {
            let (snap_muted, snap_off_hook) = {
                let g = audio_rx.borrow();
                (g.speaker_muted, g.call_off_hook)
            };
            if snap_muted == last {
                continue;
            }
            last = snap_muted;
            if events.try_send(HidEvent::ConsumerMute).is_err() {
                stats.glue_drops.fetch_add(1, Ordering::Relaxed);
                warn!("hid: mute_watch dropped ConsumerMute (queue full)");
            }
            if snap_off_hook && events.try_send(HidEvent::TelephonyPhoneMute).is_err() {
                stats.glue_drops.fetch_add(1, Ordering::Relaxed);
                warn!("hid: mute_watch dropped TelephonyPhoneMute (queue full)");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{mpsc, watch};

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_emits_one_event_per_step_up() {
        let mut s = AudioState::default();
        s.speaker_volume = 50;
        let (tx, rx) = watch::channel(s);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_volume_watch(rx, ev_tx, HidStats::new(), 1000);
        tokio::task::yield_now().await; // let the task reach .changed()
        tx.send_modify(|s| s.speaker_volume = 53);
        let a = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        let b = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        let c = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        assert_eq!(a, HidEvent::ConsumerVolumeStep { up: true });
        assert_eq!(b, HidEvent::ConsumerVolumeStep { up: true });
        assert_eq!(c, HidEvent::ConsumerVolumeStep { up: true });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_emits_down_events() {
        let mut s = AudioState::default();
        s.speaker_volume = 50;
        let (tx, rx) = watch::channel(s);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_volume_watch(rx, ev_tx, HidStats::new(), 1000);
        tokio::task::yield_now().await; // let the task reach .changed()
        tx.send_modify(|s| s.speaker_volume = 48);
        let a = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        let b = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        assert_eq!(a, HidEvent::ConsumerVolumeStep { up: false });
        assert_eq!(b, HidEvent::ConsumerVolumeStep { up: false });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_ignores_unrelated_state_change() {
        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_volume_watch(rx, ev_tx, HidStats::new(), 1000);
        tokio::task::yield_now().await; // let the task reach .changed()
        tx.send_modify(|s| s.mic_muted = !s.mic_muted);
        let t = tokio::time::timeout(Duration::from_millis(50), ev_rx.recv()).await;
        assert!(t.is_err(), "expected no event on unrelated state change");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mute_watch_emits_consumer_mute_on_toggle() {
        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_mute_watch(rx, ev_tx, HidStats::new());
        tokio::task::yield_now().await; // let the task reach .changed()
        tx.send_modify(|s| s.speaker_muted = true);
        let ev = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        assert_eq!(ev, HidEvent::ConsumerMute);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mute_watch_emits_both_when_off_hook() {
        let mut s = AudioState::default();
        s.call_off_hook = true;
        let (tx, rx) = watch::channel(s);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_mute_watch(rx, ev_tx, HidStats::new());
        tokio::task::yield_now().await; // let the task reach .changed()
        tx.send_modify(|s| s.speaker_muted = true);
        let a = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        let b = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        let got = vec![a, b];
        assert!(got.contains(&HidEvent::ConsumerMute));
        assert!(got.contains(&HidEvent::TelephonyPhoneMute));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mute_watch_ignores_no_op_changes() {
        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_mute_watch(rx, ev_tx, HidStats::new());
        tokio::task::yield_now().await; // let the task reach .changed()
        tx.send_modify(|s| s.speaker_muted = false);
        let t = tokio::time::timeout(Duration::from_millis(50), ev_rx.recv()).await;
        assert!(t.is_err(), "expected no event without an edge");
    }
}
