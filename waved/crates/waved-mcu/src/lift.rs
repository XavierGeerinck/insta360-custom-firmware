//! The mic lift (Mic height, B5), without any I/O: what to send for a chosen
//! height, and what the MCU's motor reports mean for `McuState::lift`.
//!
//! The lift moves only when the user picks a height (and for power off,
//! `power_off.rs`). A move sends stock's MOTOR_CTRL move and then the MCU's
//! start height (so the lift rises to the same height at the next power-on),
//! each resent up to 3 times without an acknowledgement, like stock. While
//! it runs, the motor state is polled. A stall (BLOCK) or an obstruction
//! (SENSOR_TRIGGER) ends the move; nothing retries by itself, but the user's
//! next tap on a stalled height uses stock's block-control speeds.

use crate::handler::Action;
use crate::mavlink::Message;
use crate::protocol::{self, MotorState, Outgoing, Speeds};
use std::collections::VecDeque;
use tracing::{info, warn};
use waved_state::{DeviceEvent, LiftState, MotorPosition};

/// Interval between motor-state queries while a move runs.
pub const POLL_MS: u64 = 500;
/// An IDLE this soon after the move was sent, before any sign of motion, is
/// the motor not having started yet.
pub const SETTLE_MS: u64 = 1_000;
/// Longest move (stock waits 20 s for the retract at power off).
pub const MOVE_TIMEOUT_MS: u64 = 20_000;
/// Wait for an acknowledgement before resending (stock: 100 ms for cmd 3;
/// the service loop runs every 50 ms).
pub const ACK_MS: u64 = 200;
/// Sends of one command (stock: 3).
pub const TRIES: u8 = 3;

#[derive(Default)]
pub struct Lift {
    state: LiftState,
    active: Option<Move>,
    outbox: Outbox,
    /// The height whose last move stalled.
    stalled: Option<MotorPosition>,
}

struct Move {
    position: MotorPosition,
    started_ms: u64,
    next_poll_ms: u64,
    /// The MCU reported WORK or a position since the move was sent.
    seen_motion: bool,
}

impl Lift {
    /// The user chose `position`: move there and make it the start height.
    pub fn request(&mut self, position: MotorPosition, now_ms: u64) -> Vec<Action> {
        let Some(mm) = protocol::height_mm(position) else {
            return Vec::new();
        };
        let speeds = if self.stalled == Some(position) {
            Speeds::BLOCKED
        } else {
            Speeds::for_target(mm)
        };
        self.stalled = None;
        info!("mcu: lift to {mm} mm ({position:?}, speeds {}/{})", speeds.high, speeds.low);
        // A new choice replaces whatever the last one still had to send.
        self.outbox = Outbox::default();
        let mut sends = self.outbox.push(protocol::motor_move(mm, speeds), now_ms);
        if let Some(start) = protocol::start_height(mm) {
            sends.extend(self.outbox.push(start, now_ms));
        }
        self.active = Some(Move {
            position,
            started_ms: now_ms,
            next_poll_ms: now_ms + POLL_MS,
            seen_motion: false,
        });
        let mut actions: Vec<Action> = sends.into_iter().map(Action::Send).collect();
        actions.extend(self.set(LiftState::Moving));
        actions
    }

    /// A message from the MCU; `None` when it is not about the lift.
    pub fn on_message(&mut self, m: &Message, now_ms: u64) -> Option<Vec<Action>> {
        let command = protocol::notify_cmd(m)?;
        if command == protocol::cmd::MOTOR_CTRL {
            let sends = self.outbox.on_ack(command, now_ms);
            return Some(sends.into_iter().map(Action::Send).collect());
        }
        if let Some(state) = protocol::motor_state(m) {
            return Some(self.on_state(state, now_ms));
        }
        if let Some(mm) = protocol::lift_position(m) {
            info!("mcu: lift at {mm} mm");
            if let Some(active) = self.active.as_mut() {
                active.seen_motion = true;
            }
            return Some(self.move_received(now_ms));
        }
        None
    }

    fn on_state(&mut self, state: MotorState, now_ms: u64) -> Vec<Action> {
        match state {
            MotorState::Working => {
                if let Some(active) = self.active.as_mut() {
                    active.seen_motion = true;
                }
                // Without a move of ours this is the MCU moving the lift
                // itself (the power key's 2 s hold retracts it).
                let mut actions = self.move_received(now_ms);
                actions.extend(self.set(LiftState::Moving));
                actions
            }
            MotorState::Idle => match &self.active {
                Some(active) if active.seen_motion || now_ms >= active.started_ms + SETTLE_MS => {
                    let position = active.position;
                    self.active = None;
                    let mut actions = self.move_received(now_ms);
                    actions.extend(self.set(LiftState::Idle));
                    actions.push(Action::Height(position));
                    actions
                }
                Some(_) => Vec::new(),
                None if self.state.is_moving() => self.set(LiftState::Idle),
                None => Vec::new(),
            },
            MotorState::Blocked => {
                warn!("mcu: the lift stalled");
                self.stalled = self.active.take().map(|active| active.position);
                let mut actions = self.move_received(now_ms);
                actions.extend(self.set(LiftState::Blocked));
                actions
            }
            MotorState::Obstructed => {
                warn!("mcu: the lift's sensor saw an obstruction");
                self.active = None;
                self.stalled = None;
                let mut actions = self.move_received(now_ms);
                let changed = self.set(LiftState::Obstructed);
                if !changed.is_empty() {
                    actions.extend(changed);
                    actions.push(Action::Event(DeviceEvent::MotorObstruction));
                }
                actions
            }
            MotorState::WaitData => Vec::new(),
        }
    }

    /// A motor report shows the MCU has the move: stop resending it (a
    /// resent move after a stall would be a retry nobody asked for).
    fn move_received(&mut self, now_ms: u64) -> Vec<Action> {
        self.outbox.confirm_move(now_ms).into_iter().map(Action::Send).collect()
    }

    /// Resends, motor-state polls and the move's time limit.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Action> {
        let mut actions: Vec<Action> =
            self.outbox.tick(now_ms).into_iter().map(Action::Send).collect();
        let Some(active) = self.active.as_mut() else {
            return actions;
        };
        if now_ms >= active.started_ms + MOVE_TIMEOUT_MS {
            warn!("mcu: the lift did not report idle within {} s", MOVE_TIMEOUT_MS / 1_000);
            self.active = None;
            actions.extend(self.set(LiftState::Idle));
        } else if now_ms >= active.next_poll_ms {
            active.next_poll_ms = now_ms + POLL_MS;
            actions.push(Action::Send(protocol::motor_state_query()));
        }
        actions
    }

    /// End any move with `state`: the power-off sequence took the link over.
    pub fn force(&mut self, state: LiftState) -> Vec<Action> {
        self.active = None;
        self.outbox = Outbox::default();
        self.set(state)
    }

    fn set(&mut self, state: LiftState) -> Vec<Action> {
        if self.state == state {
            return Vec::new();
        }
        self.state = state;
        vec![Action::Lift(state)]
    }
}

/// Commands that wait for the MCU's `mcu_notify` with the same cmd, one at a
/// time, each sent at most `TRIES` times.
#[derive(Default)]
struct Outbox {
    queue: VecDeque<Outgoing>,
    in_flight: Option<InFlight>,
}

struct InFlight {
    out: Outgoing,
    sent_ms: u64,
    tries: u8,
}

fn command_of(out: &Outgoing) -> u32 {
    u32::from_le_bytes([out.payload[0], out.payload[1], out.payload[2], out.payload[3]])
}

impl Outbox {
    fn push(&mut self, out: Outgoing, now_ms: u64) -> Vec<Outgoing> {
        self.queue.push_back(out);
        self.next(now_ms)
    }

    /// Treat an in-flight move (MOTOR_CTRL sub 0) as acknowledged.
    fn confirm_move(&mut self, now_ms: u64) -> Vec<Outgoing> {
        match &self.in_flight {
            Some(sent) if command_of(&sent.out) == protocol::cmd::MOTOR_CTRL && sent.out.payload[4] == 0 => {
                self.in_flight = None;
                self.next(now_ms)
            }
            _ => Vec::new(),
        }
    }

    fn on_ack(&mut self, command: u32, now_ms: u64) -> Vec<Outgoing> {
        match &self.in_flight {
            Some(sent) if command_of(&sent.out) == command => {
                self.in_flight = None;
                self.next(now_ms)
            }
            _ => Vec::new(),
        }
    }

    fn tick(&mut self, now_ms: u64) -> Vec<Outgoing> {
        let Some(sent) = self.in_flight.as_mut() else {
            return Vec::new();
        };
        if now_ms < sent.sent_ms + ACK_MS {
            return Vec::new();
        }
        if sent.tries < TRIES {
            sent.tries += 1;
            sent.sent_ms = now_ms;
            return vec![sent.out.clone()];
        }
        warn!("mcu: no reply to cmd {} after {TRIES} tries", command_of(&sent.out));
        self.in_flight = None;
        self.next(now_ms)
    }

    fn next(&mut self, now_ms: u64) -> Vec<Outgoing> {
        if self.in_flight.is_some() {
            return Vec::new();
        }
        let Some(out) = self.queue.pop_front() else {
            return Vec::new();
        };
        self.in_flight = Some(InFlight { out: out.clone(), sent_ms: now_ms, tries: 1 });
        vec![out]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::{encode, Parser};
    use crate::protocol::{cmd, motor_move, motor_state_query, start_height, NOTIFY};

    fn notify(command: u32, params: &[u8]) -> Message {
        let mut payload = command.to_le_bytes().to_vec();
        payload.extend_from_slice(params);
        let mut parser = Parser::default();
        encode(0, NOTIFY, &payload).into_iter().find_map(|b| parser.feed(b)).unwrap()
    }

    fn state(code: u8) -> Message {
        notify(cmd::MOTOR_CHECK, &[2, code])
    }

    fn ack() -> Message {
        notify(cmd::MOTOR_CTRL, &[])
    }

    const IDLE: u8 = 0;
    const WORK: u8 = 1;
    const BLOCK: u8 = 2;
    const SENSOR: u8 = 4;

    fn sends(actions: &[Action]) -> Vec<Outgoing> {
        actions.iter().filter_map(|a| match a { Action::Send(o) => Some(o.clone()), _ => None }).collect()
    }

    fn lift_states(actions: &[Action]) -> Vec<LiftState> {
        actions.iter().filter_map(|a| match a { Action::Lift(s) => Some(*s), _ => None }).collect()
    }

    fn on(lift: &mut Lift, m: Message, now: u64) -> Vec<Action> {
        lift.on_message(&m, now).expect("a lift message")
    }

    #[test]
    fn a_choice_sends_the_move_then_the_start_height_and_shows_moving() {
        let mut lift = Lift::default();
        let actions = lift.request(MotorPosition::High, 0);
        assert_eq!(sends(&actions), [motor_move(50, Speeds::RAISE)], "one command in flight");
        assert_eq!(lift_states(&actions), [LiftState::Moving]);
        // The MCU acknowledges the move: the start height follows.
        assert_eq!(sends(&on(&mut lift, ack(), 30)), [start_height(50).unwrap()]);
        assert!(sends(&on(&mut lift, ack(), 60)).is_empty());
        assert!(sends(&lift.tick(400)).iter().all(|o| *o == motor_state_query()), "nothing resent once acknowledged");
    }

    #[test]
    fn an_unanswered_command_is_sent_three_times_then_the_next_one_goes() {
        let mut lift = Lift::default();
        lift.request(MotorPosition::Low, 0);
        let mut moves = 1;
        let mut t = 0;
        let mut start_sent_at = None;
        while t < 2_000 && start_sent_at.is_none() {
            t += 50;
            for out in sends(&lift.tick(t)) {
                if out == motor_move(30, Speeds::RAISE) {
                    moves += 1;
                } else if out == start_height(30).unwrap() {
                    start_sent_at = Some(t);
                }
            }
        }
        assert_eq!(moves, TRIES, "stock sends a command at most 3 times");
        assert_eq!(start_sent_at, Some(3 * ACK_MS), "then gives up on it and sends the next");
    }

    #[test]
    fn working_then_idle_reaches_the_height() {
        let mut lift = Lift::default();
        lift.request(MotorPosition::Medium, 0);
        assert!(lift_states(&on(&mut lift, state(WORK), 100)).is_empty(), "already moving");
        let done = on(&mut lift, state(IDLE), 2_500);
        assert_eq!(lift_states(&done), [LiftState::Idle]);
        assert!(done.contains(&Action::Height(MotorPosition::Medium)));
    }

    #[test]
    fn idle_before_the_motor_started_is_not_arrival() {
        let mut lift = Lift::default();
        lift.request(MotorPosition::High, 0);
        assert!(on(&mut lift, state(IDLE), 300).is_empty(), "the first poll can beat the motor");
        // Already at the height: the motor never works, IDLE after a second ends it.
        let done = on(&mut lift, state(IDLE), SETTLE_MS);
        assert!(done.contains(&Action::Height(MotorPosition::High)));
        // A position report is motion too.
        lift.request(MotorPosition::Low, 5_000);
        let report = on(&mut lift, notify(cmd::MOTOR_CHECK, &[1, 40, 0]), 5_100);
        assert!(lift_states(&report).is_empty() && !report.iter().any(|a| matches!(a, Action::Height(_))));
        assert!(on(&mut lift, state(IDLE), 5_200).contains(&Action::Height(MotorPosition::Low)));
    }

    #[test]
    fn the_motor_state_is_polled_while_a_move_runs() {
        let mut lift = Lift::default();
        lift.request(MotorPosition::High, 0);
        on(&mut lift, ack(), 10);
        on(&mut lift, ack(), 20);
        let polls = |lift: &mut Lift, t: u64| sends(&lift.tick(t)).iter().filter(|o| **o == motor_state_query()).count();
        assert_eq!(polls(&mut lift, 450), 0);
        assert_eq!(polls(&mut lift, 500), 1);
        assert_eq!(polls(&mut lift, 950), 0);
        assert_eq!(polls(&mut lift, 1_000), 1);
        on(&mut lift, state(WORK), 1_100);
        on(&mut lift, state(IDLE), 1_200);
        assert_eq!(polls(&mut lift, 2_000), 0, "no polls once it stopped");
    }

    #[test]
    fn a_move_without_an_end_gives_up_after_its_time_limit() {
        let mut lift = Lift::default();
        lift.request(MotorPosition::High, 0);
        on(&mut lift, state(WORK), 100);
        assert!(lift_states(&lift.tick(MOVE_TIMEOUT_MS - 1)).is_empty());
        let gave_up = lift.tick(MOVE_TIMEOUT_MS);
        assert_eq!(lift_states(&gave_up), [LiftState::Idle]);
        assert!(!gave_up.iter().any(|a| matches!(a, Action::Height(_))), "not known to be there");
    }

    #[test]
    fn a_stall_is_reported_and_only_the_users_retry_pushes_harder() {
        let mut lift = Lift::default();
        lift.request(MotorPosition::High, 0);
        // The move is never acknowledged, but WORK shows the MCU has it: it is
        // not resent (after a stall that would be a retry), the start height goes.
        assert_eq!(sends(&on(&mut lift, state(WORK), 100)), [start_height(50).unwrap()]);
        let stalled = on(&mut lift, state(BLOCK), 900);
        assert_eq!(lift_states(&stalled), [LiftState::Blocked]);
        assert!(sends(&stalled).is_empty(), "no automatic retry");
        for t in [1_200, 1_500, 5_000] {
            assert!(sends(&lift.tick(t)).iter().all(|o| *o != motor_move(50, Speeds::RAISE)));
        }
        assert!(on(&mut lift, state(IDLE), 6_000).is_empty(), "blocked stays until the next move");
        let retry = lift.request(MotorPosition::High, 10_000);
        assert_eq!(sends(&retry), [motor_move(50, Speeds::BLOCKED)]);
        assert_eq!(lift_states(&retry), [LiftState::Moving]);
        // Another height, or the next move after that, uses the normal speeds.
        assert_eq!(sends(&lift.request(MotorPosition::Low, 11_000)), [motor_move(30, Speeds::RAISE)]);
    }

    #[test]
    fn an_obstruction_ends_the_move_with_one_event() {
        let mut lift = Lift::default();
        lift.request(MotorPosition::Medium, 0);
        on(&mut lift, state(WORK), 100);
        let blocked = on(&mut lift, state(SENSOR), 400);
        assert_eq!(lift_states(&blocked), [LiftState::Obstructed]);
        assert!(blocked.contains(&Action::Event(DeviceEvent::MotorObstruction)));
        assert!(on(&mut lift, state(SENSOR), 500).is_empty(), "reported once");
        assert_eq!(
            sends(&lift.request(MotorPosition::Medium, 3_000)),
            [motor_move(40, Speeds::RAISE)],
            "an obstruction is not a stall: normal speeds"
        );
    }

    #[test]
    fn the_mcu_moving_the_lift_itself_shows_moving_then_idle() {
        // A 2 s power-key hold: the MCU retracts the lift on its own.
        let mut lift = Lift::default();
        assert_eq!(lift_states(&on(&mut lift, state(WORK), 0)), [LiftState::Moving]);
        let idle = on(&mut lift, state(IDLE), 3_000);
        assert_eq!(lift_states(&idle), [LiftState::Idle]);
        assert!(!idle.iter().any(|a| matches!(a, Action::Height(_))));
        assert!(on(&mut lift, state(IDLE), 4_000).is_empty());
    }

    #[test]
    fn moving_is_not_a_height_and_other_messages_are_not_the_lifts() {
        let mut lift = Lift::default();
        assert!(lift.request(MotorPosition::Moving, 0).is_empty());
        assert!(lift.on_message(&notify(cmd::MISC_CHECK, &[0, 1]), 0).is_none());
        assert!(lift.on_message(&notify(cmd::POWER_CTRL, &[]), 0).is_none());
    }

    #[test]
    fn a_new_choice_replaces_what_the_last_one_still_had_to_send() {
        let mut lift = Lift::default();
        lift.request(MotorPosition::Low, 0);
        let next = lift.request(MotorPosition::High, 50);
        assert_eq!(sends(&next), [motor_move(50, Speeds::RAISE)]);
        assert!(lift_states(&next).is_empty(), "still moving");
        assert_eq!(sends(&on(&mut lift, ack(), 60)), [start_height(50).unwrap()]);
        assert!(sends(&on(&mut lift, ack(), 70)).is_empty(), "Low's start height was dropped");
    }

    #[test]
    fn power_off_taking_the_link_ends_the_move() {
        let mut lift = Lift::default();
        lift.request(MotorPosition::High, 0);
        assert_eq!(lift.force(LiftState::Obstructed), [Action::Lift(LiftState::Obstructed)]);
        assert!(sends(&lift.tick(1_000)).is_empty(), "nothing left to send or poll");
    }
}
