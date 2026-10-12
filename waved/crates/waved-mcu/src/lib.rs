//! The MCU link (docs/MCU_PROTOCOL.md): `/dev/ttyS4`, MAVLink v2. Answers
//! heartbeats, publishes the battery, turns the power key into
//! `DeviceEvent::PowerKey`, moves the lift (Mic height) and powers the
//! device off. `/dev/ttyS1`, where this crate used to send, is the Bluetooth
//! module. `simulate::SimMcu` stands in for the MCU on the host.

pub mod handler;
pub mod lift;
pub mod link;
pub mod mavlink;
pub mod power_off;
pub mod protocol;

#[cfg(any(test, feature = "simulate"))]
pub mod simulate;

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};
use tracing::{info, warn};
use waved_state::{
    BatteryState, Command, DeviceEvent, LiftState, McuState, MotorPosition, PowerOffPhase,
};

use crate::handler::{Action, Handler};
use crate::link::Link;
use crate::power_off::{Limits, Outcome, Transport};

pub const MCU_PORT: &str = "/dev/ttyS4";
pub const MCU_BAUD: u32 = 115_200;

/// What the service does for a bus command.
#[derive(Debug, PartialEq)]
enum Plan {
    Nothing,
    /// Mic height: move the lift (`lift.rs`).
    Lift(MotorPosition),
    PowerOff,
}

fn command_plan(cmd: &Command) -> Plan {
    match cmd {
        Command::PowerOffNow => Plan::PowerOff,
        Command::SetMotorPosition(MotorPosition::Moving) => Plan::Nothing,
        Command::SetMotorPosition(position) => Plan::Lift(*position),
        // Shutdown stops the services on every exit; it is not a power-off.
        _ => Plan::Nothing,
    }
}

/// Runs the MCU link: `Link` on the device, `simulate::SimMcu` on the host.
pub struct McuService<T: Transport = Link> {
    link: T,
    battery_tx: Arc<watch::Sender<BatteryState>>,
    mcu_tx: Arc<watch::Sender<McuState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl<T: Transport> McuService<T> {
    pub fn new(
        link: T,
        battery_tx: Arc<watch::Sender<BatteryState>>,
        mcu_tx: Arc<watch::Sender<McuState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        Self { link, battery_tx, mcu_tx, events_tx }
    }

    /// Runs until the command channel closes.
    pub fn run(self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("mcu_srv started");
        let start = Instant::now();
        let now = || start.elapsed().as_millis() as u64;
        let mut handler = Handler::default();
        loop {
            if let Some(m) = self.link.recv(Duration::from_millis(50)) {
                for action in handler.on_message(&m, now()) {
                    self.apply(action);
                }
            }
            for action in handler.tick(now()) {
                self.apply(action);
            }
            if handler.link_lost(now()) {
                warn!("mcu: no heartbeat for {} ms", handler::LINK_LOST_MS);
            }
            loop {
                match cmd_rx.try_recv() {
                    Ok(cmd) => self.handle(&cmd, &mut handler, now()),
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
            Action::Lift(lift) => {
                self.mcu_tx.send_if_modified(|s| std::mem::replace(&mut s.lift, lift) != lift);
            }
            Action::Height(position) => {
                self.mcu_tx.send_if_modified(|s| {
                    std::mem::replace(&mut s.motor_position, position) != position
                });
            }
        }
    }

    fn handle(&self, cmd: &Command, handler: &mut Handler, now_ms: u64) {
        match command_plan(cmd) {
            Plan::Nothing => {}
            Plan::Lift(position) => {
                for action in handler.move_lift(position, now_ms) {
                    self.apply(action);
                }
            }
            Plan::PowerOff => {
                info!("mcu: powering off");
                self.mcu_tx.send_modify(|s| s.power_off = PowerOffPhase::InProgress);
                let (phase, lift) = match power_off::run(&self.link, Limits::STOCK) {
                    Outcome::Sent => (PowerOffPhase::Sent, LiftState::Idle),
                    Outcome::Obstructed => {
                        warn!("mcu: the lift was obstructed, staying on");
                        let _ = self.events_tx.send(DeviceEvent::MotorObstruction);
                        (PowerOffPhase::Aborted, LiftState::Obstructed)
                    }
                };
                for action in handler.lift_after_power_off(lift) {
                    self.apply(action);
                }
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
        // PowerOff is the intent; main sends PowerOffNow once the services
        // stopped, and only that runs the MCU sequence.
        assert_eq!(command_plan(&Command::PowerOff), Plan::Nothing);
        assert_eq!(command_plan(&Command::PowerOffNow), Plan::PowerOff);
    }

    #[test]
    fn a_mic_height_moves_the_lift_and_never_to_0_mm() {
        // Low is stock's 30 mm stop; 0 mm (retracted) is only for power off.
        assert_eq!(command_plan(&Command::SetMotorPosition(MotorPosition::High)), Plan::Lift(MotorPosition::High));
        assert_eq!(command_plan(&Command::SetMotorPosition(MotorPosition::Low)), Plan::Lift(MotorPosition::Low));
        assert_eq!(protocol::height_mm(MotorPosition::Low), Some(30));
        assert_eq!(command_plan(&Command::SetMotorPosition(MotorPosition::Moving)), Plan::Nothing);
    }
}
