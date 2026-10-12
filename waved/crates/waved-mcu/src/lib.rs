pub mod battery;
pub mod mavlink;
pub mod protocol;
pub mod buttons;
pub mod motor;

#[cfg(feature = "simulate")]
pub mod simulate;

use crossbeam_channel::Receiver;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};
use waved_state::{BatteryState, Command, DeviceEvent, McuState, MotorPosition};
use waved_uart::{Frame, UartMux};

pub struct McuService {
    mcu_rx: Receiver<Frame>,
    uart: Arc<UartMux>,
    battery_tx: Arc<watch::Sender<BatteryState>>,
    mcu_tx: Arc<watch::Sender<McuState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl McuService {
    pub fn new(
        mcu_rx: Receiver<Frame>,
        uart: Arc<UartMux>,
        battery_tx: Arc<watch::Sender<BatteryState>>,
        mcu_tx: Arc<watch::Sender<McuState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        Self {
            mcu_rx,
            uart,
            battery_tx,
            mcu_tx,
            events_tx,
        }
    }

    pub fn run(self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("mcu_srv started");

        loop {
            crossbeam_channel::select! {
                recv(self.mcu_rx) -> msg => {
                    match msg {
                        Ok(frame) => self.handle_frame(&frame),
                        Err(_) => {
                            warn!("mcu channel closed, stopping");
                            return;
                        }
                    }
                }
                default(std::time::Duration::from_millis(50)) => {
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        self.handle_command(&cmd);
                    }
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
            0x10..=0x1F => {
                if let Some(state) = battery::parse_battery_frame(payload) {
                    debug!("battery: {}% {}mV charging={}", state.level, state.voltage, state.is_charging);
                    if state.level < 10 {
                        let _ = self.events_tx.send(DeviceEvent::LowBattery(state.level));
                    }
                    let _ = self.battery_tx.send(state);
                }
            }
            0x40..=0x4F => {
                if let Some(btn) = buttons::parse_button_frame(&frame.data) {
                    debug!("button: {:?}", btn);
                    let _ = self.events_tx.send(DeviceEvent::ButtonPress(btn));
                }
            }
            0x50 => {
                let _ = self.events_tx.send(DeviceEvent::UsbConnected);
                self.mcu_tx.send_modify(|s| s.usb_connected = true);
            }
            0x51 => {
                let _ = self.events_tx.send(DeviceEvent::UsbDisconnected);
                self.mcu_tx.send_modify(|s| s.usb_connected = false);
            }
            0x60 => {
                let _ = self.events_tx.send(DeviceEvent::MotorObstruction);
            }
            _ => {
                debug!("unknown MCU command: {:#04x}", cmd_id);
            }
        }
    }

    fn handle_command(&self, cmd: &Command) {
        match cmd {
            Command::SetMotorPosition(pos) => {
                let raw = motor::build_motor_command(pos);
                if !raw.is_empty() {
                    self.uart.send(raw, true);
                    self.mcu_tx.send_modify(|s| s.motor_position = MotorPosition::Moving);
                }
            }
            Command::Shutdown => {
                info!("shutdown requested, retracting motor first");
                let retract = motor::build_motor_command(&MotorPosition::Low);
                if !retract.is_empty() {
                    self.uart.send(retract, true);
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
                let shutdown = motor::build_shutdown_command();
                self.uart.send(shutdown, true);
            }
            _ => {}
        }
    }
}
