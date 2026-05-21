#[cfg(feature = "simulate")]
pub mod simulate;

pub mod events;
pub mod report;
pub mod stats;

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
}

impl HidService {
    /// Drain `HidEvent`s from `events_rx` (external sources: watchers) AND
    /// bridge `DeviceEvent::ButtonPress(ButtonType)` from `bus_rx` into
    /// the same stream. Write each event as a press-release pair to
    /// `/dev/hidg0`.
    pub async fn run(
        cfg: HidServiceConfig,
        mut bus_rx: tokio::sync::broadcast::Receiver<waved_state::DeviceEvent>,
        mut events_rx: tokio::sync::mpsc::Receiver<events::HidEvent>,
        stats: std::sync::Arc<stats::HidStats>,
    ) {
        info!("hid_srv started");

        let device_path = cfg.device.clone();
        let mut dev: Option<std::fs::File> = match tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(&device_path)
        })
        .await
        {
            Ok(Ok(f)) => {
                stats.device_present.store(true, std::sync::atomic::Ordering::Relaxed);
                Some(f)
            }
            Ok(Err(e)) => {
                warn!("hid: cannot open {}: {} — HID writes disabled", cfg.device, e);
                stats.device_present.store(false, std::sync::atomic::Ordering::Relaxed);
                None
            }
            Err(e) => {
                warn!("hid: open task join failed: {} — HID writes disabled", e);
                stats.device_present.store(false, std::sync::atomic::Ordering::Relaxed);
                None
            }
        };

        let gap = std::time::Duration::from_millis(cfg.press_release_gap_ms);

        loop {
            tokio::select! {
                bus = bus_rx.recv() => match bus {
                    Ok(waved_state::DeviceEvent::ButtonPress(btn)) => {
                        let ev = button_to_event(btn);
                        if let Some(f) = dev.as_mut() {
                            if !Self::write_event(f, ev, gap, &stats).await {
                                dev = None;
                            }
                        } else {
                            stats.write_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
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
                        if let Some(f) = dev.as_mut() {
                            if !Self::write_event(f, ev, gap, &stats).await {
                                dev = None;
                            }
                        } else {
                            stats.write_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    None => {
                        info!("hid: mpsc closed");
                        return;
                    }
                },
            }
        }
    }

    async fn write_event(
        f: &mut std::fs::File,
        ev: events::HidEvent,
        gap: std::time::Duration,
        stats: &stats::HidStats,
    ) -> bool {
        use std::io::Write;
        use std::sync::atomic::Ordering;

        let (press, release) = report::event_to_press_release(ev);
        if let Err(e) = f.write_all(&press) {
            warn!("hid: write press failed: {} — disabling HID until restart", e);
            stats.write_errors.fetch_add(1, Ordering::Relaxed);
            stats.device_present.store(false, Ordering::Relaxed);
            return false;
        }
        tokio::time::sleep(gap).await;
        if let Err(e) = f.write_all(&release) {
            warn!("hid: write release failed: {} — disabling HID until restart", e);
            stats.write_errors.fetch_add(1, Ordering::Relaxed);
            stats.device_present.store(false, Ordering::Relaxed);
            return false;
        }
        match ev {
            events::HidEvent::ConsumerVolumeStep { .. } => { stats.events_volume_step.fetch_add(1, Ordering::Relaxed); }
            events::HidEvent::ConsumerMute              => { stats.events_mute.fetch_add(1, Ordering::Relaxed); }
            events::HidEvent::ConsumerPlayPause         => { stats.events_play_pause.fetch_add(1, Ordering::Relaxed); }
            events::HidEvent::ConsumerNext              => { stats.events_next.fetch_add(1, Ordering::Relaxed); }
            events::HidEvent::ConsumerPrev              => { stats.events_prev.fetch_add(1, Ordering::Relaxed); }
            events::HidEvent::TelephonyPhoneMute        => { stats.events_phone_mute.fetch_add(1, Ordering::Relaxed); }
        }
        true
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
}
