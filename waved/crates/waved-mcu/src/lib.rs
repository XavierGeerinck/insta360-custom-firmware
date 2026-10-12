//! The MCU link (docs/MCU_PROTOCOL.md): `/dev/ttyS4`, MAVLink v2. Answers
//! heartbeats, publishes the battery, turns the power key into
//! `DeviceEvent::PowerKey`, moves the lift and powers the device off.
//! `/dev/ttyS1`, where this crate used to send, is the Bluetooth module.

pub mod handler;
pub mod link;
pub mod mavlink;
pub mod power_off;
pub mod protocol;

#[cfg(feature = "simulate")]
pub mod simulate;

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};
use tracing::{info, warn};
use waved_state::{BatteryState, Command, DeviceEvent, McuState, MotorPosition, PowerOffPhase};

use crate::handler::{Action, Handler};
use crate::link::Link;
use crate::power_off::{Limits, Outcome, Transport};
use crate::protocol::Outgoing;

pub const MCU_PORT: &str = "/dev/ttyS4";
pub const MCU_BAUD: u32 = 115_200;

/// What the service does for a bus command.
#[derive(Debug, PartialEq)]
enum Plan {
    Nothing,
    Send(Outgoing),
    PowerOff,
}

fn command_plan(cmd: &Command) -> Plan {
    match cmd {
        Command::PowerOff => Plan::PowerOff,
        Command::SetMotorPosition(position) => match position {
            MotorPosition::Low => Plan::Send(protocol::motor_to_mm(0)),
            MotorPosition::Medium => Plan::Send(protocol::motor_to_mm(30)),
            MotorPosition::High => Plan::Send(protocol::motor_to_mm(50)),
            MotorPosition::Moving => Plan::Nothing,
        },
        // Shutdown stops the services on every exit; it is not a power-off.
        _ => Plan::Nothing,
    }
}

pub struct McuService {
    link: Link,
    battery_tx: Arc<watch::Sender<BatteryState>>,
    mcu_tx: Arc<watch::Sender<McuState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl McuService {
    pub fn new(
        link: Link,
        battery_tx: Arc<watch::Sender<BatteryState>>,
        mcu_tx: Arc<watch::Sender<McuState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        Self { link, battery_tx, mcu_tx, events_tx }
    }

    pub fn run(self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("mcu_srv started on {MCU_PORT}");
        let start = Instant::now();
        let mut handler = Handler::default();
        loop {
            let now = start.elapsed().as_millis() as u64;
            if let Some(m) = self.link.recv(Duration::from_millis(50)) {
                for action in handler.on_message(&m, now) {
                    self.apply(action);
                }
            }
            if handler.link_lost(now) {
                warn!("mcu: no heartbeat for {} ms", handler::LINK_LOST_MS);
            }
            loop {
                match cmd_rx.try_recv() {
                    Ok(cmd) => self.handle(&cmd),
                    Err(broadcast::error::TryRecvError::Empty) => break,
                    Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                    Err(broadcast::error::TryRecvError::Closed) => return,
                }
            }
        }
    }

    fn apply(&self, action: Action) {
        match action {
            Action::Send(out) => self.link.send(&out),
            Action::Battery(state) => {
                self.battery_tx.send_if_modified(|s| {
                    let changed = *s != state;
                    *s = state;
                    changed
                });
            }
            Action::Event(event) => {
                let _ = self.events_tx.send(event);
            }
        }
    }

    fn handle(&self, cmd: &Command) {
        match command_plan(cmd) {
            Plan::Nothing => {}
            Plan::Send(out) => self.link.send(&out),
            Plan::PowerOff => {
                info!("mcu: powering off");
                self.mcu_tx.send_modify(|s| s.power_off = PowerOffPhase::InProgress);
                let phase = match power_off::run(&self.link, Limits::STOCK) {
                    Outcome::Sent => PowerOffPhase::Sent,
                    Outcome::Obstructed => {
                        warn!("mcu: the lift was obstructed, staying on");
                        let _ = self.events_tx.send(DeviceEvent::MotorObstruction);
                        PowerOffPhase::Aborted
                    }
                };
                self.mcu_tx.send_modify(|s| s.power_off = phase);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_is_not_power_off() {
        // Every exit sends Shutdown (deploys, watchdog restarts): it must
        // never reach the MCU as a power-off.
        assert_eq!(command_plan(&Command::Shutdown), Plan::Nothing);
        assert_eq!(command_plan(&Command::PowerOff), Plan::PowerOff);
        assert_eq!(command_plan(&Command::SetMotorPosition(MotorPosition::High)), Plan::Send(protocol::motor_to_mm(50)));
        assert_eq!(command_plan(&Command::SetMotorPosition(MotorPosition::Low)), Plan::Send(protocol::motor_to_mm(0)));
        assert_eq!(command_plan(&Command::SetMotorPosition(MotorPosition::Moving)), Plan::Nothing);
    }
}
