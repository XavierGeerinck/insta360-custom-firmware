//! A simulated MCU for the host. `SimMcu` answers like the MCU on
//! `/dev/ttyS4` (docs/MCU_PROTOCOL.md), frame for frame: heartbeats with a
//! battery level, an `mcu_notify` for every command, a lift that travels to
//! its target reporting WORK, positions and IDLE (or stalls, or is
//! obstructed, when scripted), and the power cut after ALL_SHUTDOWN.
//! `SimMcuService` runs the real `McuService` on it, so the `simulate` build
//! exercises the same protocol, lift and power-off code as the device.
//!
//! What the simulator assumes and the device pass must confirm: the MCU
//! acknowledges MOTOR_CTRL with a cmd 3 `mcu_notify`, reports WORK when a
//! move starts and IDLE when it ends, reports positions as cmd 4 sub 1, and
//! that the start height (MOTOR_CTRL sub 4) does not move the lift.

use crate::mavlink::{encode, Message, Parser};
use crate::power_off::Transport;
use crate::protocol::{cmd, Outgoing, CONTROL, HEARTBEAT, HEARTBEAT_REPLY, NOTIFY};
use crate::McuService;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};
use tracing::info;
use waved_state::{BatteryState, Command, DeviceEvent, McuState};

/// How the next moves end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LiftScript {
    /// The lift reaches its target.
    #[default]
    Reaches,
    /// The motor stalls half-way (motor state BLOCK).
    Stalls,
    /// The sensor sees something half-way (SENSOR_TRIGGER).
    Obstructed,
}

#[derive(Clone, Copy, Debug)]
pub struct SimConfig {
    pub heartbeat_ms: u64,
    /// Lift travel speed.
    pub mm_per_s: u32,
    /// The lift's height when the simulated MCU starts (it raised the lift
    /// to its start height at power-on).
    pub start_mm: i16,
    pub battery_level: u16,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self { heartbeat_ms: 1_000, mm_per_s: 25, start_mm: 40, battery_level: 75 }
    }
}

/// The motor states the MCU reports (cmd 4 sub 2).
const IDLE: u8 = 0;
const WORK: u8 = 1;
const BLOCK: u8 = 2;
const SENSOR_TRIGGER: u8 = 4;

struct Travel {
    from_mm: i16,
    to_mm: i16,
    started_ms: u64,
    /// When the move ends: at the target, or half-way when scripted.
    ends_ms: u64,
    end_state: u8,
    /// Positions already reported, in 10 mm steps.
    reported: Vec<i16>,
}

struct Sim {
    config: SimConfig,
    inbox: VecDeque<Message>,
    seq: u8,
    next_heartbeat_ms: u64,
    battery_level: u16,
    usb_ma: u16,
    lift_mm: i16,
    motor: u8,
    travel: Option<Travel>,
    script: LiftScript,
    start_height: Option<u8>,
    power_cut: bool,
    received: Vec<Outgoing>,
    /// Every LED_CTRL: pattern, sub-pattern, LED count, colour, target.
    leds: Vec<[u8; 5]>,
}

/// The MCU end of the link, behind `Transport`.
pub struct SimMcu {
    started: Instant,
    sim: Mutex<Sim>,
}

impl SimMcu {
    pub fn new(config: SimConfig) -> Self {
        Self {
            started: Instant::now(),
            sim: Mutex::new(Sim {
                config,
                inbox: VecDeque::new(),
                seq: 0,
                next_heartbeat_ms: 0,
                battery_level: config.battery_level,
                usb_ma: 0,
                lift_mm: config.start_mm,
                motor: IDLE,
                travel: None,
                script: LiftScript::Reaches,
                start_height: None,
                power_cut: false,
                received: Vec::new(),
                leds: Vec::new(),
            }),
        }
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// The level the next heartbeats report, and whether USB powers the unit.
    pub fn set_battery(&self, level: u16, on_usb: bool) {
        let mut sim = self.sim.lock().unwrap();
        sim.battery_level = level;
        sim.usb_ma = if on_usb { 500 } else { 0 };
    }

    /// How the next moves end.
    pub fn script_lift(&self, script: LiftScript) {
        self.sim.lock().unwrap().script = script;
    }

    /// Where the lift is now, in mm.
    pub fn lift_mm(&self) -> i16 {
        let now = self.now_ms();
        let mut sim = self.sim.lock().unwrap();
        sim.advance(now);
        sim.lift_mm
    }

    /// The start-height code last stored (MOTOR_CTRL sub 4: 0 = 50, 1 = 40, 2 = 30 mm).
    pub fn start_height(&self) -> Option<u8> {
        self.sim.lock().unwrap().start_height
    }

    /// ALL_SHUTDOWN arrived: the MCU would cut the power now.
    pub fn power_cut(&self) -> bool {
        self.sim.lock().unwrap().power_cut
    }

    /// Every LED_CTRL waved sent: pattern, sub-pattern, LED count, colour, target.
    pub fn leds(&self) -> Vec<[u8; 5]> {
        self.sim.lock().unwrap().leds.clone()
    }

    /// Every command waved sent, heartbeat replies excluded.
    pub fn received(&self) -> Vec<Outgoing> {
        self.sim.lock().unwrap().received.clone()
    }
}

impl Sim {
    fn push(&mut self, msgid: u32, payload: &[u8]) {
        let frame = encode(self.seq, msgid, payload);
        self.seq = self.seq.wrapping_add(1);
        // Through the real parser, as the link would.
        let mut parser = Parser::default();
        if let Some(m) = frame.into_iter().find_map(|b| parser.feed(b)) {
            self.inbox.push_back(m);
        }
    }

    fn notify(&mut self, command: u32, params: &[u8]) {
        let mut payload = command.to_le_bytes().to_vec();
        payload.extend_from_slice(params);
        self.push(NOTIFY, &payload);
    }

    fn heartbeat(&mut self) {
        let mut p = vec![0u8; 32];
        p[4..6].copy_from_slice(&self.battery_level.to_le_bytes());
        p[6..8].copy_from_slice(&3_800u16.to_le_bytes());
        p[8..10].copy_from_slice(&25i16.to_le_bytes());
        p[14..16].copy_from_slice(&self.usb_ma.to_le_bytes());
        self.push(HEARTBEAT, &p);
    }

    fn motor_state(&mut self, state: u8) {
        self.motor = state;
        self.notify(cmd::MOTOR_CHECK, &[2, state]);
    }

    fn position(&mut self, mm: i16) {
        let [lo, hi] = mm.to_le_bytes();
        self.notify(cmd::MOTOR_CHECK, &[1, lo, hi]);
    }

    /// Heartbeats that fell due and the lift's progress up to `now_ms`.
    fn advance(&mut self, now_ms: u64) {
        while now_ms >= self.next_heartbeat_ms {
            self.heartbeat();
            self.next_heartbeat_ms += self.config.heartbeat_ms;
        }
        let Some(travel) = self.travel.as_mut() else {
            return;
        };
        let elapsed = now_ms.min(travel.ends_ms).saturating_sub(travel.started_ms);
        let moved = (elapsed * u64::from(self.config.mm_per_s) / 1_000) as i16;
        let mm = if travel.to_mm >= travel.from_mm {
            (travel.from_mm + moved).min(travel.to_mm)
        } else {
            (travel.from_mm - moved).max(travel.to_mm)
        };
        // A position report for every 10 mm mark passed since the last call.
        let (lo, hi) = (travel.from_mm.min(mm), travel.from_mm.max(mm));
        let mut steps: Vec<i16> = (0..=5)
            .map(|k| k * 10)
            .filter(|step| (lo..=hi).contains(step) && *step != travel.from_mm)
            .filter(|step| !travel.reported.contains(step))
            .collect();
        if travel.to_mm < travel.from_mm {
            steps.reverse();
        }
        travel.reported.extend(&steps);
        let ended = now_ms >= travel.ends_ms;
        let end_state = travel.end_state;
        self.lift_mm = mm;
        for step in steps {
            self.position(step);
        }
        if ended {
            self.travel = None;
            self.motor_state(end_state);
        }
    }

    fn on_command(&mut self, out: &Outgoing, now_ms: u64) {
        let command = u32::from_le_bytes([out.payload[0], out.payload[1], out.payload[2], out.payload[3]]);
        let sub = out.payload[4];
        if command == cmd::MOTOR_CTRL && sub == 4 {
            self.start_height = Some(out.payload[5]);
        }
        if command == cmd::LED_CTRL {
            let p = &out.payload[4..9];
            self.leds.push([p[0], p[1], p[2], p[3], p[4]]);
        }
        if command == cmd::MOTOR_CHECK {
            // A motor-state query: the reply carries the state.
            let state = self.motor;
            self.motor_state(state);
            return;
        }
        self.notify(command, &[]);
        if command == cmd::MOTOR_CTRL && sub == 0 {
            let to_mm = i16::from_le_bytes([out.payload[9], out.payload[10]]);
            self.start_move(to_mm, now_ms);
        }
        if command == cmd::POWER_CTRL && sub == 0 && out.payload[5] == 4 {
            info!("[SIM] MCU: all shutdown, cutting power");
            self.power_cut = true;
        }
    }

    fn start_move(&mut self, to_mm: i16, now_ms: u64) {
        self.advance(now_ms);
        let from_mm = self.lift_mm;
        let distance = u64::from(from_mm.abs_diff(to_mm));
        let full_ms = distance * 1_000 / u64::from(self.config.mm_per_s.max(1));
        let (ends_ms, end_state) = match self.script {
            LiftScript::Reaches => (now_ms + full_ms, IDLE),
            LiftScript::Stalls => (now_ms + full_ms / 2, BLOCK),
            LiftScript::Obstructed => (now_ms + full_ms / 2, SENSOR_TRIGGER),
        };
        info!("[SIM] MCU: lift {from_mm} -> {to_mm} mm ({:?})", self.script);
        self.travel = Some(Travel { from_mm, to_mm, started_ms: now_ms, ends_ms, end_state, reported: Vec::new() });
        self.motor_state(WORK);
    }
}

impl Transport for SimMcu {
    fn send(&self, out: &Outgoing) {
        let now = self.now_ms();
        let mut sim = self.sim.lock().unwrap();
        if sim.power_cut {
            return;
        }
        sim.advance(now);
        match out.msgid {
            HEARTBEAT_REPLY => {}
            CONTROL => {
                sim.received.push(out.clone());
                sim.on_command(out, now);
            }
            _ => sim.received.push(out.clone()),
        }
    }

    fn recv(&self, timeout: Duration) -> Option<Message> {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let now = self.now_ms();
                let mut sim = self.sim.lock().unwrap();
                if !sim.power_cut {
                    sim.advance(now);
                    if let Some(m) = sim.inbox.pop_front() {
                        return Some(m);
                    }
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            std::thread::sleep(left.min(Duration::from_millis(5)));
        }
    }
}

/// The `simulate` build's MCU: the real `McuService` on a `SimMcu`.
pub struct SimMcuService {
    service: McuService<Arc<SimMcu>>,
    events_tx: broadcast::Sender<DeviceEvent>,
    button_rx: Receiver<waved_state::ButtonType>,
}

pub struct SimMcuHandle {
    button_tx: Sender<waved_state::ButtonType>,
    mcu: Arc<SimMcu>,
}

impl SimMcuHandle {
    pub fn press_button(&self, btn: waved_state::ButtonType) {
        let _ = self.button_tx.send(btn);
    }
    pub fn set_battery(&self, level: u8, is_charging: bool) {
        self.mcu.set_battery(u16::from(level), is_charging);
    }
    /// How the next lift moves end.
    pub fn script_lift(&self, script: LiftScript) {
        self.mcu.script_lift(script);
    }
    pub fn mcu(&self) -> &Arc<SimMcu> {
        &self.mcu
    }
}

impl SimMcuService {
    pub fn new(
        battery_tx: Arc<watch::Sender<BatteryState>>,
        mcu_tx: Arc<watch::Sender<McuState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> (Self, SimMcuHandle) {
        Self::with_config(SimConfig::default(), battery_tx, mcu_tx, events_tx)
    }

    pub fn with_config(
        config: SimConfig,
        battery_tx: Arc<watch::Sender<BatteryState>>,
        mcu_tx: Arc<watch::Sender<McuState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> (Self, SimMcuHandle) {
        let (button_tx, button_rx) = bounded(64);
        let mcu = Arc::new(SimMcu::new(config));
        info!("[SIM] MCU: battery {}%, lift at {} mm", config.battery_level, config.start_mm);
        let service = McuService::new(Arc::clone(&mcu), battery_tx, mcu_tx, events_tx.clone());
        (Self { service, events_tx, button_rx }, SimMcuHandle { button_tx, mcu })
    }

    /// Drive the status LEDs (`McuService::with_leds`).
    pub fn with_leds(mut self, sources: crate::LedSources) -> Self {
        self.service = self.service.with_leds(sources);
        self
    }

    /// Runs the MCU service until the command channel closes; simulated
    /// volume buttons go out as `DeviceEvent::ButtonPress`.
    pub fn run(self, cmd_rx: broadcast::Receiver<Command>) {
        let (button_rx, events_tx) = (self.button_rx, self.events_tx);
        std::thread::Builder::new()
            .name("sim-buttons".into())
            .spawn(move || {
                for btn in button_rx.iter() {
                    info!("[SIM] button press: {:?}", btn);
                    let _ = events_tx.send(DeviceEvent::ButtonPress(btn));
                }
            })
            .expect("spawn sim-buttons");
        self.service.run(cmd_rx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waved_state::{LiftState, MotorPosition, PowerOffPhase, StateBus};

    const WAIT: Duration = Duration::from_secs(10);

    /// The real service on a fast simulated MCU, on its own thread.
    struct Rig {
        bus: StateBus,
        mcu: Arc<SimMcu>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    fn rig() -> Rig {
        rig_with(SimConfig { heartbeat_ms: 200, mm_per_s: 100, start_mm: 40, battery_level: 64 }, false)
    }

    /// The service with the status LEDs on.
    fn rig_with_leds() -> Rig {
        rig_with(SimConfig { heartbeat_ms: 200, mm_per_s: 100, start_mm: 40, battery_level: 64 }, true)
    }

    fn rig_with(config: SimConfig, leds: bool) -> Rig {
        let bus = StateBus::new();
        let (service, handle) = SimMcuService::with_config(
            config,
            Arc::clone(&bus.battery),
            Arc::clone(&bus.mcu),
            bus.events.clone(),
        );
        let service = if leds { service.with_leds(crate::LedSources::from_bus(&bus)) } else { service };
        let cmd_rx = bus.commands.subscribe();
        let thread = std::thread::spawn(move || service.run(cmd_rx));
        Rig { bus, mcu: Arc::clone(handle.mcu()), thread: Some(thread) }
    }

    impl Rig {
        fn send(&self, command: Command) {
            self.bus.commands.send(command).unwrap();
        }

        fn wait_for(&self, what: &str, ok: impl Fn(&McuState) -> bool) -> McuState {
            let deadline = Instant::now() + WAIT;
            loop {
                let state = self.bus.mcu.borrow().clone();
                if ok(&state) {
                    return state;
                }
                assert!(Instant::now() < deadline, "timed out waiting for {what}: {state:?}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Rig {
        /// Waits until the simulated MCU has received `count` LED_CTRL frames.
        fn wait_leds(&self, count: usize) -> Vec<[u8; 5]> {
            let deadline = Instant::now() + WAIT;
            loop {
                let leds = self.mcu.leds();
                if leds.len() >= count {
                    return leds;
                }
                assert!(Instant::now() < deadline, "timed out waiting for {count} LED frames: {leds:?}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    const IDLE_LED: [u8; 5] = [0, 0, 12, 1, 15];
    const MUTE_LED: [u8; 5] = [0, 0, 12, 0, 15];

    #[test]
    fn the_status_leds_follow_the_mic_mute() {
        let rig = rig_with_leds();
        assert_eq!(rig.wait_leds(1), [IDLE_LED], "IDLE at start");
        rig.bus.audio.send_modify(|a| a.mic_muted = true);
        assert_eq!(rig.wait_leds(2)[1], MUTE_LED);
        rig.bus.audio.send_modify(|a| a.mic_muted = false);
        // MIC_UNMUTE (green for 1 s), then IDLE; the simulated MCU answers every
        // frame, so nothing is resent.
        assert_eq!(rig.wait_leds(4), [IDLE_LED, MUTE_LED, IDLE_LED, IDLE_LED]);
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(rig.mcu.leds().len(), 4);
    }

    #[test]
    fn power_off_shows_its_pattern_before_the_power_is_cut() {
        let rig = rig_with_leds();
        rig.wait_leds(1);
        rig.send(Command::PowerOff);
        assert_eq!(rig.wait_leds(2)[1], [7, 1, 12, 1, 15], "SHUTDOWN: green fall");
        rig.send(Command::PowerOffNow);
        rig.wait_for("power off sent", |s| s.power_off == PowerOffPhase::Sent);
        assert!(rig.mcu.power_cut());
    }

    #[test]
    fn a_low_battery_pulses_the_ring_yellow() {
        let config = SimConfig { heartbeat_ms: 100, mm_per_s: 100, start_mm: 40, battery_level: 9 };
        let rig = rig_with(config, true);
        let leds = rig.wait_leds(2);
        assert_eq!(leds[1], [2, 0, 12, 4, 15], "BATTERY_20_PERCENT: yellow pulse");
    }

    #[test]
    fn without_leds_no_led_ctrl_is_sent() {
        let rig = rig();
        rig.bus.audio.send_modify(|a| a.mic_muted = true);
        rig.send(Command::SetMotorPosition(MotorPosition::High));
        rig.wait_for("idle at high", |s| s.lift == LiftState::Idle && s.motor_position == MotorPosition::High);
        assert!(rig.mcu.leds().is_empty());
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            // Dropping the bus closes the command channel, which ends the service.
            drop(std::mem::take(&mut self.bus));
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    #[test]
    fn mic_height_moves_the_simulated_lift_and_stores_the_start_height() {
        let rig = rig();
        rig.send(Command::SetMotorPosition(MotorPosition::High));
        rig.wait_for("moving", |s| s.lift == LiftState::Moving);
        let done = rig.wait_for("idle at high", |s| s.lift == LiftState::Idle && s.motor_position == MotorPosition::High);
        assert_eq!(done.power_off, PowerOffPhase::Idle);
        assert_eq!(rig.mcu.lift_mm(), 50);
        assert_eq!(rig.mcu.start_height(), Some(0), "50 mm is start-height code 0");
        rig.send(Command::SetMotorPosition(MotorPosition::Low));
        rig.wait_for("idle at low", |s| s.lift == LiftState::Idle && s.motor_position == MotorPosition::Low);
        assert_eq!(rig.mcu.lift_mm(), 30, "Low is 30 mm, never 0");
        assert_eq!(rig.mcu.start_height(), Some(2));
    }

    #[test]
    fn an_obstruction_stops_the_lift_with_the_event() {
        let rig = rig();
        let mut events = rig.bus.events.subscribe();
        rig.mcu.script_lift(LiftScript::Obstructed);
        rig.send(Command::SetMotorPosition(MotorPosition::High));
        let stopped = rig.wait_for("obstructed", |s| s.lift == LiftState::Obstructed);
        assert_ne!(stopped.motor_position, MotorPosition::High, "never reached");
        let deadline = Instant::now() + WAIT;
        loop {
            match events.try_recv() {
                Ok(DeviceEvent::MotorObstruction) => break,
                Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => {
                    assert!(Instant::now() < deadline, "no MotorObstruction");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        // Cleared, the next tap moves it.
        rig.mcu.script_lift(LiftScript::Reaches);
        rig.send(Command::SetMotorPosition(MotorPosition::High));
        rig.wait_for("idle at high", |s| s.lift == LiftState::Idle && s.motor_position == MotorPosition::High);
    }

    #[test]
    fn a_stall_ends_blocked_and_the_retry_pushes_harder() {
        let rig = rig();
        rig.mcu.script_lift(LiftScript::Stalls);
        rig.send(Command::SetMotorPosition(MotorPosition::High));
        rig.wait_for("blocked", |s| s.lift == LiftState::Blocked);
        rig.mcu.script_lift(LiftScript::Reaches);
        rig.send(Command::SetMotorPosition(MotorPosition::High));
        rig.wait_for("idle at high", |s| s.lift == LiftState::Idle && s.motor_position == MotorPosition::High);
        let block_speed_move = crate::protocol::motor_move(50, crate::protocol::Speeds::BLOCKED);
        assert!(rig.mcu.received().contains(&block_speed_move));
    }

    #[test]
    fn the_battery_comes_from_the_simulated_heartbeats() {
        let rig = rig();
        let deadline = Instant::now() + WAIT;
        while rig.bus.battery.borrow().level != 64 {
            assert!(Instant::now() < deadline, "no battery level");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!rig.bus.battery.borrow().is_charging);
        rig.mcu.set_battery(64, true);
        while !rig.bus.battery.borrow().is_charging {
            assert!(Instant::now() < deadline, "never charging");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn power_off_retracts_the_simulated_lift_and_cuts_the_power() {
        let rig = rig();
        rig.send(Command::PowerOffNow);
        rig.wait_for("power off sent", |s| s.power_off == PowerOffPhase::Sent);
        assert_eq!(rig.mcu.lift_mm(), 0);
        assert!(rig.mcu.power_cut());
    }

    #[test]
    fn an_obstructed_retract_leaves_the_unit_on() {
        let rig = rig();
        rig.mcu.script_lift(LiftScript::Obstructed);
        rig.send(Command::PowerOffNow);
        let aborted = rig.wait_for("aborted", |s| s.power_off == PowerOffPhase::Aborted);
        assert_eq!(aborted.lift, LiftState::Obstructed);
        assert!(!rig.mcu.power_cut());
    }
}
