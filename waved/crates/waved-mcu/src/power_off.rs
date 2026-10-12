//! Stock's power-off sequence (docs/MCU_PROTOCOL.md, "Stock power off"):
//! start the MCU's power-off timer, retract the lift to 0 mm, wait for it to
//! stop, turn charging on and tell the MCU to cut power. An obstructed lift
//! stops the timer and leaves the device on.

use crate::mavlink::Message;
use crate::protocol::{
    all_shutdown, charging_on, heartbeat, heartbeat_reply, motor_state, motor_state_query,
    motor_to_mm, notify_cmd, power_off_timer, MotorState, Outgoing,
};
use std::time::{Duration, Instant};

/// The link, as the sequence needs it.
pub trait Transport {
    fn send(&self, out: &Outgoing);
    /// The next message, or `None` after `timeout`.
    fn recv(&self, timeout: Duration) -> Option<Message>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The MCU was told to cut power.
    Sent,
    /// The lift was obstructed; the device stays on.
    Obstructed,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Longest wait for the lift to stop.
    pub motor_ms: u64,
    /// Interval between motor-state queries.
    pub poll_ms: u64,
    /// Wait for a command's acknowledgement (stock: 100 ms, 500 ms for power).
    pub ack_ms: u64,
    /// Pause after the lift stopped, before cutting power.
    pub settle_ms: u64,
}

impl Limits {
    pub const STOCK: Limits = Limits { motor_ms: 20_000, poll_ms: 200, ack_ms: 500, settle_ms: 1_000 };
}

pub fn run(t: &dyn Transport, limits: Limits) -> Outcome {
    send_acked(t, &power_off_timer(true), limits);
    send_acked(t, &motor_to_mm(0), limits);
    let deadline = Instant::now() + Duration::from_millis(limits.motor_ms);
    while Instant::now() < deadline {
        t.send(&motor_state_query());
        match wait_motor_state(t, Duration::from_millis(limits.poll_ms.max(1))) {
            Some(MotorState::Idle) => break,
            Some(MotorState::Obstructed) => {
                send_acked(t, &power_off_timer(false), limits);
                return Outcome::Obstructed;
            }
            _ => std::thread::sleep(Duration::from_millis(limits.poll_ms)),
        }
    }
    std::thread::sleep(Duration::from_millis(limits.settle_ms));
    send_acked(t, &charging_on(), limits);
    send_acked(t, &all_shutdown(), limits);
    Outcome::Sent
}

/// Send, then wait for the `mcu_notify` with the same cmd; up to 3 tries,
/// like stock. Heartbeats that arrive meanwhile are answered.
fn send_acked(t: &dyn Transport, out: &Outgoing, limits: Limits) {
    let command = u32::from_le_bytes([out.payload[0], out.payload[1], out.payload[2], out.payload[3]]);
    for _ in 0..3 {
        t.send(out);
        let deadline = Instant::now() + Duration::from_millis(limits.ack_ms.max(1));
        while Instant::now() < deadline {
            let Some(m) = t.recv(Duration::from_millis(limits.ack_ms.max(1))) else { break };
            if heartbeat(&m).is_some() {
                t.send(&heartbeat_reply());
            } else if notify_cmd(&m) == Some(command) {
                return;
            }
        }
    }
}

fn wait_motor_state(t: &dyn Transport, timeout: Duration) -> Option<MotorState> {
    let m = t.recv(timeout)?;
    if heartbeat(&m).is_some() {
        t.send(&heartbeat_reply());
        return None;
    }
    motor_state(&m)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::{encode, Parser};
    use crate::protocol::{cmd, NOTIFY};
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// Answers each command like the MCU: an ack with the same cmd, and
    /// motor-state replies from `motor` (one per query).
    struct FakeMcu {
        sent: RefCell<Vec<Outgoing>>,
        inbox: RefCell<VecDeque<Message>>,
        motor: RefCell<VecDeque<u8>>,
        silent: bool,
    }

    fn notify(command: u32, params: &[u8]) -> Message {
        let mut p = command.to_le_bytes().to_vec();
        p.extend_from_slice(params);
        let mut parser = Parser::default();
        encode(0, NOTIFY, &p).into_iter().find_map(|b| parser.feed(b)).unwrap()
    }

    impl Transport for FakeMcu {
        fn send(&self, out: &Outgoing) {
            self.sent.borrow_mut().push(out.clone());
            if self.silent {
                return;
            }
            let command = u32::from_le_bytes(out.payload[..4].try_into().unwrap());
            if command == cmd::MOTOR_CHECK {
                let state = self.motor.borrow_mut().pop_front().unwrap_or(0);
                self.inbox.borrow_mut().push_back(notify(cmd::MOTOR_CHECK, &[2, state]));
            } else {
                self.inbox.borrow_mut().push_back(notify(command, &[]));
            }
        }
        fn recv(&self, _timeout: Duration) -> Option<Message> {
            self.inbox.borrow_mut().pop_front()
        }
    }

    fn fake(motor: &[u8], silent: bool) -> FakeMcu {
        FakeMcu {
            sent: RefCell::new(Vec::new()),
            inbox: RefCell::new(VecDeque::new()),
            motor: RefCell::new(motor.iter().copied().collect()),
            silent,
        }
    }

    const FAST: Limits = Limits { motor_ms: 50, poll_ms: 1, ack_ms: 1, settle_ms: 0 };

    #[test]
    fn the_stock_sequence_ends_in_all_shutdown() {
        let mcu = fake(&[1, 1, 0], false); // working, working, idle
        assert_eq!(run(&mcu, FAST), Outcome::Sent);
        let sent = mcu.sent.borrow();
        let first = &sent[..2];
        assert_eq!(first, [power_off_timer(true), motor_to_mm(0)]);
        assert_eq!(&sent[sent.len() - 2..], [charging_on(), all_shutdown()]);
        assert!(sent.iter().filter(|o| **o == motor_state_query()).count() >= 3);
    }

    #[test]
    fn an_obstructed_motor_aborts_the_power_off() {
        let mcu = fake(&[1, 4], false); // working, obstructed
        assert_eq!(run(&mcu, FAST), Outcome::Obstructed);
        let sent = mcu.sent.borrow();
        assert!(sent.contains(&power_off_timer(false)));
        assert!(!sent.contains(&all_shutdown()));
    }

    #[test]
    fn power_off_without_replies_still_sends_the_shutdown() {
        let mcu = fake(&[], true);
        assert_eq!(run(&mcu, FAST), Outcome::Sent);
        assert_eq!(mcu.sent.borrow().last(), Some(&all_shutdown()));
    }
}
