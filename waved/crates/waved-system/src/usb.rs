//! USB host connection from the device controller's state.
//!
//! The MCU's USB frames (0x50 / 0x51) never arrive on this hardware (device
//! pass 2026-10-06: no MCU frames at all in 25 s of debug logging), so
//! `McuState.usb_connected` stayed false while plugged into a computer. The
//! USB device controller knows: `/sys/class/udc/<controller>/state` reads
//! `configured` once a host has enumerated the gadget.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use waved_state::McuState;

pub const UDC_DIR: &str = "/sys/class/udc";
const POLL: Duration = Duration::from_secs(1);

/// Whether a controller state means a host is connected.
pub fn host_connected(state: &str) -> bool {
    state.trim() == "configured"
}

/// Whether any controller under `udc_dir` reports a configured host.
pub fn any_host_connected(udc_dir: &Path) -> bool {
    std::fs::read_dir(udc_dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            std::fs::read_to_string(entry.path().join("state")).is_ok_and(|s| host_connected(&s))
        })
}

/// Poll the controllers every second and publish changes to `usb_connected`.
pub async fn run(udc_dir: PathBuf, mcu_tx: Arc<watch::Sender<McuState>>) {
    let mut interval = tokio::time::interval(POLL);
    loop {
        interval.tick().await;
        let connected = any_host_connected(&udc_dir);
        mcu_tx.send_if_modified(|mcu| {
            let changed = mcu.usb_connected != connected;
            mcu.usb_connected = connected;
            changed
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_configured_means_a_host() {
        assert!(host_connected("configured\n"));
        for state in ["not attached\n", "attached", "powered", "default", "addressed", "suspended", ""] {
            assert!(!host_connected(state), "{state:?}");
        }
    }

    #[test]
    fn any_controller_with_a_host_counts() {
        let dir = std::env::temp_dir().join(format!("waved-udc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!any_host_connected(&dir), "no udc directory: not connected");
        for (name, state) in [("a.udc", "not attached\n"), ("b.udc", "configured\n")] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            std::fs::write(dir.join(name).join("state"), state).unwrap();
        }
        assert!(any_host_connected(&dir));
        std::fs::write(dir.join("b.udc").join("state"), "suspended\n").unwrap();
        assert!(!any_host_connected(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn run_publishes_the_controller_state() {
        let dir = std::env::temp_dir().join(format!("waved-udc-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("x.udc")).unwrap();
        std::fs::write(dir.join("x.udc").join("state"), "configured\n").unwrap();
        let mcu_tx = Arc::new(watch::channel(McuState::default()).0);
        let mut rx = mcu_tx.subscribe();
        tokio::spawn(run(dir.clone(), Arc::clone(&mcu_tx)));
        rx.changed().await.unwrap();
        assert!(rx.borrow().usb_connected);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
