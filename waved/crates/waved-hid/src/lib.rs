#[cfg(feature = "simulate")]
pub mod simulate;

pub mod events;
pub mod key_writer;
pub mod node;
pub mod pclink;
pub mod report;
pub mod stats;
pub mod reader;
pub mod watch;

use tracing::{info, warn};
use waved_state::ButtonType;

use events::HidEvent;

/// Map a hardware `ButtonType` (from waved-mcu) to a `HidEvent`. The MCU
/// only emits four button kinds; richer events (PlayPause, Next/Prev,
/// PhoneMute) come from the state watchers in `watch.rs`.
pub fn button_to_event(b: ButtonType) -> HidEvent {
    match b {
        ButtonType::VolumeUp      => HidEvent::ConsumerVolumeStep { up: true },
        ButtonType::VolumeDown    => HidEvent::ConsumerVolumeStep { up: false },
        ButtonType::MuteToggle    => HidEvent::ConsumerMute,
        ButtonType::MuteLongPress => HidEvent::ConsumerMute,
    }
}

pub struct HidService;

#[derive(Debug, Clone)]
pub struct HidServiceConfig {
    pub device: String,
    pub press_release_gap_ms: u64,
    /// USB storage's hold on the node (`StateBus::hid_hold`); `None`: no
    /// gadget switch to follow.
    pub hold: Option<waved_state::GadgetHold>,
}

impl HidService {
    /// Drain `HidEvent`s from `events_rx` (external sources: watchers) AND
    /// bridge `DeviceEvent::ButtonPress(ButtonType)` from `bus_rx` into
    /// the same stream. Write each event as a press-release pair to
    /// `/dev/hidg0`. A failed open or write never stops it: the keys wait
    /// for the node, which is opened again once a second
    /// (`key_writer::KeyWriter`; the gadget switch of USB storage replaces
    /// the node). While USB storage holds the node (`cfg.hold`) the handle
    /// is closed and the keys wait.
    pub async fn run(
        cfg: HidServiceConfig,
        bus_rx: tokio::sync::broadcast::Receiver<waved_state::DeviceEvent>,
        events_rx: tokio::sync::mpsc::Receiver<events::HidEvent>,
        stats: std::sync::Arc<stats::HidStats>,
    ) {
        let keys = key_writer::KeyWriter::new(
            node::DevNode::writer(&cfg.device),
            &cfg.device,
            std::time::Duration::from_millis(cfg.press_release_gap_ms),
            std::sync::Arc::clone(&stats),
            node::REOPEN_EVERY,
        )
        .with_hold(cfg.hold.as_ref().map(|hold| hold.register("the HID key writer")));
        Self::run_with(keys, bus_rx, events_rx, stats).await
    }

    // Note: `KeyWriter::flush` writes the press and the release (2 bytes
    // each) with blocking std::io calls on the async executor thread. For
    // `/dev/hidg0` specifically this is acceptable because: (a) the writes
    // go to a kernel-buffered character device, with a fast-return path;
    // (b) the host-side bulk pipe is rarely backpressured
    // (Consumer/Telephony events are sparse compared to UAC2 streams);
    // (c) we `tokio::time::sleep(gap)` between press and release, yielding
    // the executor in the middle of the pair. If host-side stalls become
    // observed (look for `write_errors` climbing in stats), wrap the two
    // writes in a single `spawn_blocking` future.
    async fn run_with<N: node::Node>(
        mut keys: key_writer::KeyWriter<N>,
        mut bus_rx: tokio::sync::broadcast::Receiver<waved_state::DeviceEvent>,
        mut events_rx: tokio::sync::mpsc::Receiver<events::HidEvent>,
        stats: std::sync::Arc<stats::HidStats>,
    ) {
        info!("hid_srv started");
        // USB storage's hold wakes the writer by itself: an idle writer must
        // close the node too.
        let mut hold_rx = keys.take_hold_signal();
        keys.open_at_start();

        // MCU sends button-down then button-up as separate frames; debounce
        // mute buttons so an even number of frames doesn't cancel itself out.
        let mute_debounce = std::time::Duration::from_millis(300);
        let mut last_mute_btn: Option<std::time::Instant> = None;

        loop {
            // While keys wait for the node: when to try it again.
            let retry = keys.retry_at();
            let wake = tokio::time::Instant::from_std(
                retry.unwrap_or_else(|| std::time::Instant::now() + std::time::Duration::from_secs(3600)),
            );
            tokio::select! {
                bus = bus_rx.recv() => match bus {
                    Ok(waved_state::DeviceEvent::ButtonPress(btn)) => {
                        info!("hid: button {:?}", btn);
                        let is_mute = matches!(btn, ButtonType::MuteToggle | ButtonType::MuteLongPress);
                        if is_mute {
                            let now = std::time::Instant::now();
                            if last_mute_btn.is_some_and(|t| now.duration_since(t) < mute_debounce) {
                                stats.glue_drops.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                continue;
                            }
                            last_mute_btn = Some(now);
                        }
                        keys.push(button_to_event(btn));
                        keys.flush().await;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("hid: device-event bus closed");
                        return;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("hid_srv lagged by {n} events");
                    }
                },
                ev = events_rx.recv() => match ev {
                    Some(ev) => {
                        keys.push(ev);
                        keys.flush().await;
                    }
                    None => {
                        info!("hid: mpsc closed");
                        return;
                    }
                },
                _ = tokio::time::sleep_until(wake), if retry.is_some() => keys.flush().await,
                changed = hold_changed(&mut hold_rx), if hold_rx.is_some() => {
                    if changed {
                        keys.check_hold();
                        keys.flush().await;
                    } else {
                        hold_rx = None;
                    }
                }
            }
        }
    }
}

/// The next change of USB storage's hold; false once its sender is gone.
async fn hold_changed(rx: &mut Option<tokio::sync::watch::Receiver<waved_state::GadgetHoldSignal>>) -> bool {
    match rx {
        Some(rx) => rx.changed().await.is_ok(),
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod integ_tests {
    use super::*;
    use std::io::Read;
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::sync::{broadcast, mpsc};

    #[tokio::test(flavor = "current_thread")]
    async fn writer_emits_volume_up_for_button_press() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hidg0");
        std::fs::File::create(&path).unwrap();

        let (btn_tx, btn_rx) = broadcast::channel(8);
        let (ev_tx, ev_rx) = mpsc::channel(16);

        let cfg = HidServiceConfig {
            device: path.to_string_lossy().into_owned(),
            press_release_gap_ms: 1,
            hold: None,
        };
        let stats = stats::HidStats::new();
        tokio::spawn(HidService::run(cfg, btn_rx, ev_rx, std::sync::Arc::clone(&stats)));

        btn_tx.send(waved_state::DeviceEvent::ButtonPress(waved_state::ButtonType::VolumeUp)).unwrap();
        // Keep mpsc sender alive so the task doesn't exit via mpsc-closed branch.
        let _keep = ev_tx;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut buf = Vec::new();
        std::fs::File::open(&path).unwrap().read_to_end(&mut buf).unwrap();
        assert_eq!(buf, vec![0x01, 0b0000_0001, 0x01, 0x00]);
        assert!(stats.device_present.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(stats.events_volume_step.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn writer_emits_consumer_mute_from_external_event() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hidg0");
        std::fs::File::create(&path).unwrap();

        let (_btn_tx, btn_rx) = broadcast::channel::<waved_state::DeviceEvent>(1);
        let (ev_tx, ev_rx) = mpsc::channel(16);

        let cfg = HidServiceConfig {
            device: path.to_string_lossy().into_owned(),
            press_release_gap_ms: 1,
            hold: None,
        };
        let stats = stats::HidStats::new();
        tokio::spawn(HidService::run(cfg, btn_rx, ev_rx, std::sync::Arc::clone(&stats)));

        ev_tx.send(events::HidEvent::ConsumerMute).await.unwrap();
        // Keep the broadcast sender alive so the task doesn't exit via bus-closed branch.
        let _keep = _btn_tx;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut buf = Vec::new();
        std::fs::File::open(&path).unwrap().read_to_end(&mut buf).unwrap();
        assert_eq!(buf, vec![0x01, 0b0000_0100, 0x01, 0x00]);
        assert_eq!(stats.events_mute.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    /// Polls `path` until it holds `want` (or 4 s pass) and returns what it holds.
    async fn wait_for_contents(path: &std::path::Path, want: &[u8]) -> Vec<u8> {
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        loop {
            let got = std::fs::read(path).unwrap_or_default();
            if got == want || std::time::Instant::now() > deadline {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_key_after_a_gadget_switch_reaches_the_new_node() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hidg0");
        std::fs::File::create(&path).unwrap();
        let (_btn_tx, btn_rx) = broadcast::channel::<waved_state::DeviceEvent>(1);
        let (ev_tx, ev_rx) = mpsc::channel(16);
        let cfg = HidServiceConfig { device: path.to_string_lossy().into_owned(), press_release_gap_ms: 1, hold: None };
        let stats = stats::HidStats::new();
        tokio::spawn(HidService::run(cfg, btn_rx, ev_rx, std::sync::Arc::clone(&stats)));

        ev_tx.send(events::HidEvent::ConsumerMute).await.unwrap();
        assert_eq!(wait_for_contents(&path, &[0x01, 0x04, 0x01, 0x00]).await, vec![0x01, 0x04, 0x01, 0x00]);

        // The switch: the old node goes, a new one comes at the same path.
        std::fs::remove_file(&path).unwrap();
        std::fs::File::create(&path).unwrap();
        ev_tx.send(events::HidEvent::ConsumerVolumeStep { up: true }).await.unwrap();
        assert_eq!(
            wait_for_contents(&path, &[0x01, 0x01, 0x01, 0x00]).await,
            vec![0x01, 0x01, 0x01, 0x00],
            "Louder goes to the new node"
        );

        // A key while the node is missing waits for the next one.
        std::fs::remove_file(&path).unwrap();
        ev_tx.send(events::HidEvent::ConsumerVolumeStep { up: false }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::File::create(&path).unwrap();
        assert_eq!(
            wait_for_contents(&path, &[0x01, 0x02, 0x01, 0x00]).await,
            vec![0x01, 0x02, 0x01, 0x00],
            "Quieter waited for the node"
        );
        assert!(stats.device_present.load(std::sync::atomic::Ordering::Relaxed));
    }

    /// Polls `what` until it holds (or 2 s pass).
    async fn eventually(what: impl Fn() -> bool) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !what() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        what()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_key_service_closes_the_node_for_a_hold_and_writes_after_it() {
        use crate::node::fake::FakeNode;
        let node = FakeNode::present();
        let hold = waved_state::GadgetHold::new();
        let stats = stats::HidStats::new();
        let keys = key_writer::KeyWriter::new(
            node.clone(),
            "/dev/hidg0",
            Duration::ZERO,
            std::sync::Arc::clone(&stats),
            node::REOPEN_EVERY,
        )
        .with_hold(Some(hold.register("keys")));
        let (_btn_tx, btn_rx) = broadcast::channel::<waved_state::DeviceEvent>(1);
        let (ev_tx, ev_rx) = mpsc::channel(16);
        tokio::spawn(HidService::run_with(keys, btn_rx, ev_rx, stats));
        assert!(eventually(|| node.open_handles() == 1).await, "opened at start");

        // No key is sent: the hold alone wakes it.
        hold.raise();
        assert!(eventually(|| hold.all_closed()).await, "confirmed");
        assert_eq!(node.open_handles(), 0);
        ev_tx.send(events::HidEvent::ConsumerMute).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(node.opens(), 1, "the key waits, nothing opened while held");

        node.unbind();
        node.bind();
        hold.lower();
        let (press, release) = report::event_to_press_release(events::HidEvent::ConsumerMute);
        assert!(eventually(|| node.written_to(1) == [press.to_vec(), release.to_vec()]).await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_configured_hold_reaches_the_key_writer() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hidg0");
        std::fs::File::create(&path).unwrap();
        let hold = waved_state::GadgetHold::new();
        let (_btn_tx, btn_rx) = broadcast::channel::<waved_state::DeviceEvent>(1);
        let (_ev_tx, ev_rx) = mpsc::channel::<events::HidEvent>(16);
        let cfg = HidServiceConfig {
            device: path.to_string_lossy().into_owned(),
            press_release_gap_ms: 1,
            hold: Some(hold.clone()),
        };
        tokio::spawn(HidService::run(cfg, btn_rx, ev_rx, stats::HidStats::new()));
        assert!(eventually(|| hold.users() == 1).await);
        hold.raise();
        assert!(eventually(|| hold.all_closed()).await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_key_reaches_a_node_that_was_missing_at_start() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hidg0");
        let (_btn_tx, btn_rx) = broadcast::channel::<waved_state::DeviceEvent>(1);
        let (ev_tx, ev_rx) = mpsc::channel(16);
        let cfg = HidServiceConfig { device: path.to_string_lossy().into_owned(), press_release_gap_ms: 1, hold: None };
        let stats = stats::HidStats::new();
        tokio::spawn(HidService::run(cfg, btn_rx, ev_rx, std::sync::Arc::clone(&stats)));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!stats.device_present.load(std::sync::atomic::Ordering::Relaxed));

        std::fs::File::create(&path).unwrap();
        ev_tx.send(events::HidEvent::ConsumerMute).await.unwrap();
        assert_eq!(wait_for_contents(&path, &[0x01, 0x04, 0x01, 0x00]).await, vec![0x01, 0x04, 0x01, 0x00]);
        assert!(stats.device_present.load(std::sync::atomic::Ordering::Relaxed));
    }
}
