//! Simulated MCU service for testing without hardware.
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tracing::info;
use waved_state::{BatteryState, Command, DeviceEvent, McuState, MotorPosition};

pub struct SimMcuService {
    battery_tx: Arc<watch::Sender<BatteryState>>,
    mcu_tx: Arc<watch::Sender<McuState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
    button_rx: Receiver<waved_state::ButtonType>,
}

pub struct SimMcuHandle {
    button_tx: Sender<waved_state::ButtonType>,
    battery_tx: Arc<watch::Sender<BatteryState>>,
}

impl SimMcuHandle {
    pub fn press_button(&self, btn: waved_state::ButtonType) {
        let _ = self.button_tx.send(btn);
    }
    pub fn set_battery(&self, level: u8, is_charging: bool) {
        self.battery_tx.send_modify(|s| { s.level = level; s.is_charging = is_charging; });
    }
}

impl SimMcuService {
    pub fn new(
        battery_tx: Arc<watch::Sender<BatteryState>>,
        mcu_tx: Arc<watch::Sender<McuState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> (Self, SimMcuHandle) {
        let (button_tx, button_rx) = bounded(64);
        battery_tx.send_modify(|s| { s.level = 75; s.voltage = 3800; s.temperature = 25; s.is_charging = false; });
        info!("[SIM] MCU service created (battery=75%, temp=25C)");
        let handle = SimMcuHandle { button_tx, battery_tx: Arc::clone(&battery_tx) };
        (Self { battery_tx, mcu_tx, events_tx, button_rx }, handle)
    }

    pub fn run(self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("[SIM] mcu_srv started");
        loop {
            crossbeam_channel::select! {
                recv(self.button_rx) -> msg => {
                    match msg {
                        Ok(btn) => {
                            info!("[SIM] button press: {:?}", btn);
                            let _ = self.events_tx.send(DeviceEvent::ButtonPress(btn));
                        }
                        Err(_) => return,
                    }
                }
                default(std::time::Duration::from_millis(50)) => {
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        match cmd {
                            Command::SetMotorPosition(pos) => {
                                info!("[SIM] motor -> {:?}", pos);
                                self.mcu_tx.send_modify(|s| s.motor_position = pos);
                            }
                            Command::PowerOffNow => {
                                info!("[SIM] power off");
                                self.mcu_tx.send_modify(|s| s.power_off = waved_state::PowerOffPhase::Sent);
                            }
                            Command::Shutdown => { info!("[SIM] shutdown requested"); return; }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}
