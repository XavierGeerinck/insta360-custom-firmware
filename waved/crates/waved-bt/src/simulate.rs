//! Simulated BT service for testing without hardware.
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tracing::info;
use waved_state::{BluetoothState, DeviceEvent};

pub struct SimBtService {
    bt_tx: Arc<watch::Sender<BluetoothState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl SimBtService {
    pub fn new(
        bt_tx: Arc<watch::Sender<BluetoothState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        bt_tx.send_modify(|s| { s.bt_on = true; s.connected = false; s.mac = "AA:BB:CC:DD:EE:FF".to_string(); });
        info!("[SIM] BT service created (on, not connected)");
        Self { bt_tx, events_tx }
    }

    pub fn run(self) {
        info!("[SIM] bt_srv started (idle loop)");
        loop { std::thread::sleep(std::time::Duration::from_secs(1)); }
    }
}
