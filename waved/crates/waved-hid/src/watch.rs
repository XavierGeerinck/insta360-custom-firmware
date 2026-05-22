//! State-watcher glue tasks: subscribe to AudioState and emit HidEvents
//! per integer-step delta of speaker_volume and per edge of speaker_muted.

use std::sync::atomic::{AtomicI32, Ordering};
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
/// `host_pending_speaker_vol` is the StateBus echo-suppress marker. When
/// `uac/control.rs` dispatches a host-originated `SetSpeakerVolume(v)`,
/// it stamps `v` here *before* the state mutation. If we wake up and the
/// new state matches the marker, the change came from Mac itself — we
/// just advance our cursor and skip emitting events, preventing the
/// F11 → mixer → state → HID → Mac F11 feedback loop. Sentinel `-1`
/// means "no pending host echo, treat as local".
///
/// Returns the spawned task handle for the caller to keep alive
/// (dropping it has no effect; the task itself runs until the watch
/// channel closes).
pub fn spawn_volume_watch(
    mut audio_rx: watch::Receiver<AudioState>,
    events: mpsc::Sender<HidEvent>,
    stats: Arc<HidStats>,
    volume_step_rate_hz: u32,
    host_pending_speaker_vol: Arc<AtomicI32>,
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
            // Echo suppression: if the host stamped this exact value, the
            // change originated from Mac (via uac/control.rs). Advance the
            // cursor but don't emit HID — otherwise we ping-pong the value
            // back to Mac, which decrements/increments again, and so on.
            let pending = host_pending_speaker_vol.load(Ordering::Acquire);
            if pending == now {
                host_pending_speaker_vol.store(-1, Ordering::Release);
                last = now;
                continue;
            }
            let up = delta > 0;
            // Mac's consumer volume scale is ~16 steps (6.25% each) over 0-100%.
            // Without scaling a 50-unit Wave drag fires 50 HID steps → 312% Mac
            // jump (capped at 100%). Scale: Wave 0-100 → Mac 0-16 steps, minimum 1.
            let mac_steps = ((delta.unsigned_abs() as f32 * 16.0 / 100.0).round() as u32).max(1);
            for _ in 0..mac_steps {
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

/// Subscribe to `audio_rx` and emit `TelephonyPhoneMute` on each transition
/// of `mic_muted`. This is what Teams/Zoom/Meet listen to for "user is muted"
/// indicators on the conferencing UI. Fired unconditionally — the host
/// app decides what to do with it (in-call apps act on it; non-call
/// apps ignore it).
pub fn spawn_mic_mute_watch(
    mut audio_rx: watch::Receiver<AudioState>,
    events: mpsc::Sender<HidEvent>,
    stats: Arc<HidStats>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last = audio_rx.borrow().mic_muted;
        while audio_rx.changed().await.is_ok() {
            let snap = audio_rx.borrow().mic_muted;
            if snap == last {
                continue;
            }
            last = snap;
            if events.try_send(HidEvent::TelephonyPhoneMute).is_err() {
                stats.glue_drops.fetch_add(1, Ordering::Relaxed);
                warn!("hid: mic_mute_watch dropped TelephonyPhoneMute (queue full)");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{mpsc, watch};

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_emits_scaled_steps_up() {
        // delta=50 → round(50*16/100)=8 HID steps (not 50).
        let mut s = AudioState::default();
        s.speaker_volume = 0;
        let (tx, rx) = watch::channel(s);
        let (ev_tx, mut ev_rx) = mpsc::channel(64);
        let _h = spawn_volume_watch(rx, ev_tx, HidStats::new(), 1000, Arc::new(AtomicI32::new(-1)));
        tokio::task::yield_now().await;
        tx.send_modify(|s| s.speaker_volume = 50);
        let mut count = 0u32;
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await {
            assert_eq!(ev, HidEvent::ConsumerVolumeStep { up: true });
            count += 1;
        }
        assert_eq!(count, 8, "50 Wave units → 8 Mac HID steps");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_emits_one_step_for_small_delta() {
        // delta=3 → round(3*16/100)=0 → clamped to 1 step.
        let mut s = AudioState::default();
        s.speaker_volume = 50;
        let (tx, rx) = watch::channel(s);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_volume_watch(rx, ev_tx, HidStats::new(), 1000, Arc::new(AtomicI32::new(-1)));
        tokio::task::yield_now().await;
        tx.send_modify(|s| s.speaker_volume = 53);
        let ev = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        assert_eq!(ev, HidEvent::ConsumerVolumeStep { up: true });
        // No second event within timeout.
        assert!(tokio::time::timeout(Duration::from_millis(50), ev_rx.recv()).await.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_emits_down_events() {
        // delta=-13 → round(13*16/100)=round(2.08)=2 steps down.
        let mut s = AudioState::default();
        s.speaker_volume = 50;
        let (tx, rx) = watch::channel(s);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_volume_watch(rx, ev_tx, HidStats::new(), 1000, Arc::new(AtomicI32::new(-1)));
        tokio::task::yield_now().await;
        tx.send_modify(|s| s.speaker_volume = 37);
        let a = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        let b = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        assert_eq!(a, HidEvent::ConsumerVolumeStep { up: false });
        assert_eq!(b, HidEvent::ConsumerVolumeStep { up: false });
        assert!(tokio::time::timeout(Duration::from_millis(50), ev_rx.recv()).await.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_suppresses_host_originated_change() {
        // Simulates uac/control.rs stamping a host-set value before the
        // state mutation. The task must advance its cursor but NOT emit
        // any ConsumerVolumeStep — otherwise Mac F11 would loop back to
        // itself.
        let mut s = AudioState::default();
        s.speaker_volume = 60;
        let (tx, rx) = watch::channel(s);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let pending = Arc::new(AtomicI32::new(-1));
        let _h = spawn_volume_watch(rx, ev_tx, HidStats::new(), 1000, Arc::clone(&pending));
        tokio::task::yield_now().await;
        // Host stamps the marker, then updates state.
        pending.store(40, Ordering::Release);
        tx.send_modify(|s| s.speaker_volume = 40);
        // No HID event should fire.
        let t = tokio::time::timeout(Duration::from_millis(100), ev_rx.recv()).await;
        assert!(t.is_err(), "host-originated change must not emit HID events");
        // Marker must be consumed (reset to -1) so a subsequent local
        // change isn't accidentally suppressed.
        assert_eq!(pending.load(Ordering::Acquire), -1);
        // A subsequent local change (no marker) emits normally.
        tx.send_modify(|s| s.speaker_volume = 50);
        let ev = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        assert_eq!(ev, HidEvent::ConsumerVolumeStep { up: true });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_ignores_unrelated_state_change() {
        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_volume_watch(rx, ev_tx, HidStats::new(), 1000, Arc::new(AtomicI32::new(-1)));
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

    #[tokio::test(flavor = "current_thread")]
    async fn mic_mute_watch_emits_phone_mute_on_toggle() {
        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_mic_mute_watch(rx, ev_tx, HidStats::new());
        tokio::task::yield_now().await;
        tx.send_modify(|s| s.mic_muted = true);
        let ev = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        assert_eq!(ev, HidEvent::TelephonyPhoneMute);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mic_mute_watch_fires_regardless_of_off_hook() {
        // Unlike spawn_mute_watch's PhoneMute branch, mic_mute fires
        // unconditionally — Teams listens whether or not an off-hook
        // LED report has arrived from the host yet.
        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_mic_mute_watch(rx, ev_tx, HidStats::new());
        tokio::task::yield_now().await;
        tx.send_modify(|s| s.mic_muted = true);
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap(),
            HidEvent::TelephonyPhoneMute,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mic_mute_watch_ignores_speaker_mute() {
        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_mic_mute_watch(rx, ev_tx, HidStats::new());
        tokio::task::yield_now().await;
        tx.send_modify(|s| s.speaker_muted = !s.speaker_muted);
        let t = tokio::time::timeout(Duration::from_millis(50), ev_rx.recv()).await;
        assert!(t.is_err(), "mic_mute_watch must not fire on speaker_muted toggle");
    }
}
