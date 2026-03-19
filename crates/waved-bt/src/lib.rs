pub mod commands;

use crossbeam_channel::Receiver;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};
use waved_state::{BluetoothState, DeviceEvent};
use waved_uart::{Frame, UartMux};

pub struct BtService {
    bt_rx: Receiver<Frame>,
    uart: Arc<UartMux>,
    bt_tx: Arc<watch::Sender<BluetoothState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl BtService {
    pub fn new(
        bt_rx: Receiver<Frame>,
        uart: Arc<UartMux>,
        bt_tx: Arc<watch::Sender<BluetoothState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        Self { bt_rx, uart, bt_tx, events_tx }
    }

    pub fn run(self) {
        info!("bt_srv started");

        self.uart.send(commands::build_get_status(), false);
        self.uart.send(commands::build_get_mac(), false);
        self.uart.send(commands::build_get_name(), false);

        loop {
            match self.bt_rx.recv() {
                Ok(frame) => self.handle_frame(&frame),
                Err(_) => {
                    warn!("bt channel closed, stopping");
                    return;
                }
            }
        }
    }

    fn handle_frame(&self, frame: &Frame) {
        if frame.data.is_empty() {
            return;
        }

        let cmd_id = frame.data[0];
        let payload = &frame.data[1..];

        match cmd_id {
            0x01 => {
                let connected = !payload.is_empty() && payload[0] == 1;
                if connected {
                    let name = String::from_utf8_lossy(&payload[1..]).trim_end_matches('\0').to_string();
                    info!("bluetooth connected: {name}");
                    let _ = self.events_tx.send(DeviceEvent::BtConnected(name.clone()));
                    self.bt_tx.send_modify(|s| {
                        s.connected = true;
                        s.device_name = name;
                    });
                }
            }
            0x02 => {
                info!("bluetooth disconnected");
                let _ = self.events_tx.send(DeviceEvent::BtDisconnected);
                self.bt_tx.send_modify(|s| {
                    s.connected = false;
                    s.device_name.clear();
                });
            }
            0x09 => {
                let mac = String::from_utf8_lossy(payload).trim_end_matches('\0').to_string();
                debug!("bt mac: {mac}");
                self.bt_tx.send_modify(|s| s.mac = mac);
            }
            0x0E => {
                let connected = !payload.is_empty() && payload[0] != 0;
                self.bt_tx.send_modify(|s| s.connected = connected);
            }
            _ => {
                debug!("unhandled bt response: cmd={:#04x} len={}", cmd_id, payload.len());
            }
        }
    }
}
