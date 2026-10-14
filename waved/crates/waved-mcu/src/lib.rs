//! The MCU link (docs/MCU_PROTOCOL.md): `/dev/ttyS4`, MAVLink v2. Answers
//! heartbeats, publishes the battery, turns the power key into
//! `DeviceEvent::PowerKey`, moves the lift (Mic height), drives the status
//! LEDs from waved's state (`leds.rs`, with `with_leds`) and powers the
//! device off. `/dev/ttyS1`, where this crate used to send, is the Bluetooth
//! module. `simulate::SimMcu` stands in for the MCU on the host.

pub mod handler;
pub mod leds;
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
    AudioState, BatteryState, BluetoothState, Command, DeviceEvent, FilesState, LiftState,
    McuState, MeetingState, MotorPosition, OtaPhase, OtaState, PowerOffPhase, StateBus,
};

use crate::handler::{Action, Handler};
use crate::leds::{LedInputs, Leds};
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

/// What the status LEDs follow besides the MCU's own state (`leds.rs`).
pub struct LedSources {
    pub audio: watch::Receiver<AudioState>,
    pub meeting: watch::Receiver<MeetingState>,
    pub bt: watch::Receiver<BluetoothState>,
    pub ota: watch::Receiver<OtaState>,
    pub files: watch::Receiver<FilesState>,
}

impl LedSources {
    pub fn from_bus(bus: &StateBus) -> Self {
        Self {
            audio: bus.audio.subscribe(),
            meeting: bus.meeting.subscribe(),
            bt: bus.bt.subscribe(),
            ota: bus.ota.subscribe(),
            files: bus.files.subscribe(),
        }
    }

    fn inputs(&self, mcu: &McuState, battery: &BatteryState) -> LedInputs {
        let audio = self.audio.borrow();
        let meeting = self.meeting.borrow();
        let bt = self.bt.borrow();
        let ota = self.ota.borrow();
        LedInputs {
            mic_muted: audio.mic_muted,
            speaker_volume: audio.speaker_volume,
            speaker_muted: audio.speaker_muted,
            meeting: meeting.recording || meeting.paused,
            bt_pairing: bt.pairing,
            bt_connected: bt.connected,
            call_ringing: bt.call.is_ringing(),
            call_active: bt.call.is_active(),
            updating: matches!(ota.phase, OtaPhase::Installing | OtaPhase::Restarting),
            update_failed: matches!(ota.phase, OtaPhase::Failed { .. }),
            low_space: self.files.borrow().low_space,
            too_hot: battery.temperature >= leds::TOO_HOT_C,
            lift_moving: mcu.lift.is_moving(),
            power_off: mcu.power_off,
            battery_level: battery.level,
            charging: battery.is_charging,
        }
    }
}

/// Whether waved drives the status LEDs: `WAVED_LEDS=0` leaves them alone.
pub fn leds_enabled() -> bool {
    std::env::var("WAVED_LEDS").map_or(true, |v| v.trim() != "0")
}

/// Runs the MCU link: `Link` on the device, `simulate::SimMcu` on the host.
pub struct McuService<T: Transport = Link> {
    link: T,
    battery_tx: Arc<watch::Sender<BatteryState>>,
    mcu_tx: Arc<watch::Sender<McuState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
    /// The status LEDs, when `with_leds` turned them on.
    leds: Option<(LedSources, Leds)>,
}

impl<T: Transport> McuService<T> {
    pub fn new(
        link: T,
        battery_tx: Arc<watch::Sender<BatteryState>>,
        mcu_tx: Arc<watch::Sender<McuState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        Self { link, battery_tx, mcu_tx, events_tx, leds: None }
    }

    /// Drive the status LEDs from waved's state, as stock does.
    pub fn with_leds(mut self, sources: LedSources) -> Self {
        info!("mcu: driving the status LEDs");
        self.leds = Some((sources, Leds::new()));
        self
    }

    /// Runs until the command channel closes.
    pub fn run(mut self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("mcu_srv started");
        let start = Instant::now();
        let now = || start.elapsed().as_millis() as u64;
        let mut handler = Handler::default();
        loop {
            if let Some(m) = self.link.recv(Duration::from_millis(50)) {
                if let Some((_, leds)) = self.leds.as_mut() {
                    leds.on_message(&m);
                }
                for action in handler.on_message(&m, now()) {
                    self.apply(action, now());
                }
            }
            for action in handler.tick(now()) {
                self.apply(action, now());
            }
            self.follow_leds(now());
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

    /// The status LEDs: what changed since the last loop, time limits, resends.
    fn follow_leds(&mut self, now_ms: u64) {
        let Some((sources, leds)) = self.leds.as_mut() else {
            return;
        };
        let inputs = sources.inputs(&self.mcu_tx.borrow(), &self.battery_tx.borrow());
        let mut out = leds.update(&inputs, now_ms);
        out.extend(leds.tick(now_ms));
        for frame in out {
            self.link.send(&frame);
        }
    }

    fn apply(&mut self, action: Action, now_ms: u64) {
        if let (Action::Event(DeviceEvent::LowBattery(level)), Some((_, leds))) = (&action, self.leds.as_mut()) {
            for frame in leds.on_low_battery(*level, now_ms) {
                self.link.send(&frame);
            }
        }
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

    fn handle(&mut self, cmd: &Command, handler: &mut Handler, now_ms: u64) {
        if let Some((_, leds)) = self.leds.as_mut() {
            // Power off and factory reset show before anything stops.
            for frame in leds.on_command(cmd, now_ms) {
                self.link.send(&frame);
            }
        }
        match command_plan(cmd) {
            Plan::Nothing => {}
            Plan::Lift(position) => {
                for action in handler.move_lift(position, now_ms) {
                    self.apply(action, now_ms);
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
                    self.apply(action, now_ms);
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
