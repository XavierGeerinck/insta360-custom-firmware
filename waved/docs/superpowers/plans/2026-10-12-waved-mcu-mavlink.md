# waved-mcu on the real MCU link (`/dev/ttyS4`, MAVLink) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** waved talks to the MCU where it really is, so the power key, the battery readout and power off work.

**Architecture:** `waved-mcu` gets its own serial link on `/dev/ttyS4` (115200) speaking the MCU's MAVLink v2 dialect: a pure framing module, a pure protocol module (decode heartbeats and notifications, build commands), a pure handler that turns messages into actions, and a power-off sequence written against a small transport trait so all of it is testable on the host. The old `DC CD` frames on `/dev/ttyS1` (which is the Bluetooth module) are removed from `waved-mcu`; the `ttyS1` mux stays for Bluetooth. A new `Command::PowerOff` separates "switch the device off" from `Command::Shutdown` ("stop the services"), which every process exit sends.

**Tech Stack:** Rust (workspace crates `waved-mcu`, `waved-state`, `waved-ui`, `waved`), `serialport` 4, `crossbeam-channel`, `tokio` watch/broadcast.

**Spec:** `waved/docs/MCU_PROTOCOL.md` (protocol, verified frames) and the user's decisions of 2026-10-12: the power key is screen + power (click = screen off/on, hold 2 s = "Power off?"; double click does nothing); the motor moves only for power off (retract to 0 mm), no Mic height setting yet.

## Global Constraints

- Serial: `/dev/ttyS4`, 115200 baud, 8N1, raw. Never open `/dev/ttyS4` from anything but `waved-mcu`.
- Framing: MAVLink v2, `sysid = compid = 0`, `incompat = compat = 0`, per-sender `seq`, trailing payload zeros trimmed (≥ 1 byte kept), CRC-16/MCRF4XX over everything after `FD` plus `crc_extra` (0 → 0x45, 1 → 0x20, 100 → 0x27, 101 → 0x3A).
- Never send `mcu_control` cmd 0x0D (MISC_CTRL) type 0 (arms an MCU watchdog that reboots the SoC).
- `Command::Shutdown` must never power the device off (it is sent on every exit, including `killall waved` during deploys).
- Copy rules (`waved-ui` `screens_v2::messages::copy`): no new UI strings in this plan.
- Tests: `cargo test -p waved-mcu --lib`, `cargo test -p waved-ui --lib`, `cargo test -p waved-state --lib` on the host; `scripts/linux-test.sh test -p waved --features ui,capture-alsa` and `scripts/linux-test.sh check -p waved --features ui,capture-alsa` for the binary.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; commit on `feat/display-redesign`.

## Review Focus

1. **A deploy or watchdog restart must not switch the device off.** `Command::Shutdown` reaching `McuService` sends nothing to the MCU — test in Task 6 (`shutdown_is_not_power_off`).
2. **Garbage on the line** (a frame cut mid-way when waved starts, a corrupted CRC, bytes from before the MCU booted) must neither panic nor lose the next good frame — test in Task 1 (`a_bad_frame_does_not_eat_the_next_one`).
3. **A glitched battery reading** (level 255, or 100 → 0 in one heartbeat) must not raise `LowBattery` or power off — test in Task 4 (`glitched_levels_are_ignored`).
4. **The camera blocked while retracting at power off** aborts the power off and leaves the device on — test in Task 5 (`an_obstructed_motor_aborts_the_power_off`).
5. **A silent MCU** (unplugged link, different MCU firmware) leaves waved running; power off gives up after its timeout instead of hanging the exit — tests in Task 4 (`a_silent_link_is_reported_once`) and Task 5 (`power_off_without_replies_still_sends_the_shutdown`).

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/waved-mcu/src/mavlink.rs` (new) | MAVLink v2 framing: CRC, `encode`, streaming `Parser`, `Message` |
| `crates/waved-mcu/src/protocol.rs` (new) | Message ids and command numbers; decode heartbeat / button / motor state; build the commands waved sends |
| `crates/waved-mcu/src/handler.rs` (new) | Pure: message → actions (heartbeat reply, battery state with glitch filter, `LowBattery`, power key events, link-lost) |
| `crates/waved-mcu/src/power_off.rs` (new) | Pure sequence over a `Transport` trait: timer start, retract, wait idle, charging on, all shutdown |
| `crates/waved-mcu/src/link.rs` (new) | `/dev/ttyS4` I/O: reader thread → `Message` channel; writer with `seq`; implements `Transport` |
| `crates/waved-mcu/src/lib.rs` (rewrite) | `McuService`: runs the handler and commands on the link |
| `crates/waved-mcu/src/{battery,buttons,motor}.rs` (delete) | Old `DC CD` frames for `ttyS1` |
| `crates/waved-mcu/Cargo.toml` | drop `waved-uart`, add `serialport` |
| `crates/waved-state/src/lib.rs` | `Command::PowerOff`; `McuState.power_off: PowerOffPhase` |
| `crates/waved-ui/src/screens_v2/confirm.rs` | Power off confirmation sends `Command::PowerOff` |
| `crates/waved/src/main.rs` | open the link; drain the `ttyS1` mux's MCU channel; `PowerOff` lifecycle; critical battery → `PowerOff` |
| `crates/waved-mcu/src/simulate.rs` | `PowerOff` marks the phase `Sent` |
| `docs/MCU_PROTOCOL.md`, `crates/waved-system/README.md`, `../TODO.md` | waved's status |

---

### Task 1: MAVLink framing

**Files:**
- Create: `crates/waved-mcu/src/mavlink.rs`
- Modify: `crates/waved-mcu/src/lib.rs` (add `pub mod mavlink;`)

**Interfaces:**
- Produces: `pub struct Message { pub seq: u8, pub msgid: u32, pub payload: Vec<u8> }` (payload zero-filled to the message's full length); `pub fn encode(seq: u8, msgid: u32, payload: &[u8]) -> Vec<u8>`; `pub struct Parser` with `pub fn feed(&mut self, byte: u8) -> Option<Message>`; `pub fn full_len(msgid: u32) -> Option<usize>`.

- [ ] **Step 1: Write the failing tests** — append to the new `mavlink.rs` (the module body comes in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    // Captured on the device, 2026-10-12 (docs/MCU_PROTOCOL.md).
    const HEARTBEAT: &str = "FD 10 00 00 1C 00 00 00 00 00 00 00 00 00 64 00 52 10 24 00 00 00 00 00 00 02 62 A9";
    const PRESSED: &str = "FD 01 00 00 1D 00 00 65 00 00 0E 31 C7";
    const CLICKED: &str = "FD 06 00 00 1E 00 00 65 00 00 0E 00 00 00 00 01 06 66";

    fn parse_all(bytes: &[u8]) -> Vec<Message> {
        let mut parser = Parser::default();
        bytes.iter().filter_map(|&b| parser.feed(b)).collect()
    }

    #[test]
    fn captured_frames_decode_with_zero_filled_payloads() {
        let msgs = parse_all(&[hex(HEARTBEAT), hex(PRESSED), hex(CLICKED)].concat());
        assert_eq!(msgs.len(), 3);
        assert_eq!((msgs[0].msgid, msgs[0].seq, msgs[0].payload.len()), (0, 0x1C, 32));
        assert_eq!(&msgs[0].payload[4..6], &[0x64, 0x00]);
        assert_eq!((msgs[1].msgid, msgs[1].payload.len()), (101, 36));
        assert!(msgs[1].payload[1..].iter().all(|&b| b == 0), "trimmed payload is zero-filled");
        assert_eq!(msgs[2].payload[5], 1);
    }

    #[test]
    fn encode_reproduces_the_stock_frames() {
        assert_eq!(encode(0, 1, &[0; 32]), hex("FD 01 00 00 00 00 00 01 00 00 00 FC 8F"));
        let mut build_time = 0x0Fu32.to_le_bytes().to_vec();
        build_time.resize(36, 0);
        assert_eq!(encode(0, 100, &build_time), hex("FD 01 00 00 00 00 00 64 00 00 0F 6C FF"));
    }

    #[test]
    fn encode_then_parse_round_trips() {
        let payload: Vec<u8> = (1..=10).collect();
        let msgs = parse_all(&encode(200, 100, &payload));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].seq, 200);
        assert_eq!(&msgs[0].payload[..10], &payload[..]);
    }

    #[test]
    fn a_bad_frame_does_not_eat_the_next_one() {
        let mut corrupt = hex(CLICKED);
        corrupt[15] ^= 0x40; // the CRC no longer matches
        let mut line = vec![0x4B, 0x00, 0xFD]; // noise and a cut-off start
        line.extend(corrupt);
        line.extend(hex(PRESSED));
        let msgs = parse_all(&line);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].seq, 0x1D);
    }

    #[test]
    fn unknown_message_ids_are_skipped() {
        let mut parser = Parser::default();
        // A well-formed v2 header with msgid 7, which this link never carries.
        for b in hex("FD 01 00 00 00 00 00 07 00 00 00 00 00") {
            assert!(parser.feed(b).is_none());
        }
        assert!(parse_all(&hex(PRESSED)).len() == 1);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p waved-mcu --lib mavlink`
Expected: compile errors (`Message`, `Parser`, `encode` not defined).

- [ ] **Step 3: Write the implementation** — top of `crates/waved-mcu/src/mavlink.rs`:

```rust
//! MAVLink v2 framing for the MCU link on `/dev/ttyS4` (docs/MCU_PROTOCOL.md).
//!
//! `FD len incompat compat seq sysid compid msgid[3 LE] payload crc[2 LE]`.
//! Trailing payload zeros are trimmed on the wire and zero-filled here; the
//! CRC is CRC-16/MCRF4XX over everything after `FD`, then the message's
//! `crc_extra`.

const MAGIC: u8 = 0xFD;
/// Bytes before the payload, magic included.
const HEADER_LEN: usize = 10;

/// A decoded message. `payload` is zero-filled to the message's full length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub seq: u8,
    pub msgid: u32,
    pub payload: Vec<u8>,
}

/// Full payload length and `crc_extra` of the messages the MCU link carries.
fn spec(msgid: u32) -> Option<(usize, u8)> {
    match msgid {
        0 => Some((32, 0x45)),
        1 => Some((32, 0x20)),
        100 => Some((36, 0x27)),
        101 => Some((36, 0x3A)),
        102 => Some((140, 0xA9)),
        103 => Some((68, 0x15)),
        _ => None,
    }
}

/// The full payload length of `msgid`, if this link carries it.
pub fn full_len(msgid: u32) -> Option<usize> {
    spec(msgid).map(|(len, _)| len)
}

fn crc_accumulate(crc: u16, byte: u8) -> u16 {
    let mut t = byte ^ (crc as u8);
    t ^= t << 4;
    (crc >> 8) ^ (u16::from(t) << 8) ^ (u16::from(t) << 3) ^ (u16::from(t) >> 4)
}

fn checksum(after_magic: &[u8], extra: u8) -> u16 {
    let crc = after_magic.iter().fold(0xFFFF, |crc, &b| crc_accumulate(crc, b));
    crc_accumulate(crc, extra)
}

/// One frame for `msgid`, with the payload zero-padded to its full length
/// and then trimmed as MAVLink v2 sends it.
///
/// Panics on a `msgid` this link does not carry (a programming error).
pub fn encode(seq: u8, msgid: u32, payload: &[u8]) -> Vec<u8> {
    let (full, extra) = spec(msgid).expect("msgid carried by the MCU link");
    let mut body = payload.to_vec();
    body.resize(full, 0);
    while body.len() > 1 && body.last() == Some(&0) {
        body.pop();
    }
    let mut frame = vec![MAGIC, body.len() as u8, 0, 0, seq, 0, 0];
    frame.extend_from_slice(&msgid.to_le_bytes()[..3]);
    frame.extend_from_slice(&body);
    let crc = checksum(&frame[1..], extra);
    frame.extend_from_slice(&crc.to_le_bytes());
    frame
}

/// Reassembles frames from the byte stream, one byte at a time.
#[derive(Default)]
pub struct Parser {
    buf: Vec<u8>,
}

impl Parser {
    /// Feed one received byte; returns a message when it completes one.
    pub fn feed(&mut self, byte: u8) -> Option<Message> {
        if self.buf.is_empty() && byte != MAGIC {
            return None;
        }
        self.buf.push(byte);
        // A header that can't be one of ours (e.g. a cut-off frame whose
        // next `FD` would read as length 253) is dropped up to the next magic.
        while self.buf.len() >= HEADER_LEN && !header_ok(&self.buf) {
            self.resync();
        }
        if self.buf.len() < HEADER_LEN {
            return None;
        }
        let total = HEADER_LEN + usize::from(self.buf[1]) + 2;
        if self.buf.len() < total {
            return None;
        }
        let frame: Vec<u8> = self.buf.drain(..total).collect();
        match decode(&frame) {
            Some(message) => Some(message),
            None => {
                self.buf = frame;
                self.resync();
                None
            }
        }
    }

    /// Drop the leading magic and everything up to the next one.
    fn resync(&mut self) {
        match self.buf[1..].iter().position(|&b| b == MAGIC) {
            Some(at) => {
                self.buf.drain(..=at);
            }
            None => self.buf.clear(),
        }
    }
}

/// Flags zero, a message this link carries, and a length within it.
fn header_ok(h: &[u8]) -> bool {
    let msgid = u32::from_le_bytes([h[7], h[8], h[9], 0]);
    h[2] == 0 && h[3] == 0 && spec(msgid).is_some_and(|(full, _)| usize::from(h[1]) <= full)
}

fn decode(frame: &[u8]) -> Option<Message> {
    let len = usize::from(frame[1]);
    let msgid = u32::from_le_bytes([frame[7], frame[8], frame[9], 0]);
    let (full, extra) = spec(msgid)?;
    let crc = u16::from_le_bytes([frame[HEADER_LEN + len], frame[HEADER_LEN + len + 1]]);
    if checksum(&frame[1..HEADER_LEN + len], extra) != crc {
        return None;
    }
    let mut payload = frame[HEADER_LEN..HEADER_LEN + len].to_vec();
    payload.resize(full.max(len), 0);
    Some(Message { seq: frame[4], msgid, payload })
}
```

Add `pub mod mavlink;` to `crates/waved-mcu/src/lib.rs` (next to the existing `pub mod` lines).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p waved-mcu --lib mavlink`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-mcu/src/mavlink.rs crates/waved-mcu/src/lib.rs
git commit -m "feat(mcu): MAVLink v2 framing for the MCU link

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Protocol: decode what the MCU sends, build what waved sends

**Files:**
- Create: `crates/waved-mcu/src/protocol.rs`
- Modify: `crates/waved-mcu/src/lib.rs` (add `pub mod protocol;`)

**Interfaces:**
- Consumes: `mavlink::{Message, encode}` (Task 1).
- Produces:
  - `pub const HEARTBEAT: u32 = 0; pub const HEARTBEAT_REPLY: u32 = 1; pub const CONTROL: u32 = 100; pub const NOTIFY: u32 = 101;`
  - `pub struct Heartbeat { pub battery_current_ma: i32, pub level: u16, pub voltage_mv: u16, pub temperature_c: i16, pub usb_current_ma: u16 }` and `pub fn heartbeat(m: &Message) -> Option<Heartbeat>`
  - `pub enum Button { Pressed, Clicked, DoubleClicked, Held1s, Released1To2s, Held2s, Released2To5s }` and `pub fn button(m: &Message) -> Option<Button>`
  - `pub enum MotorState { Idle, Working, Blocked, WaitData, Obstructed }` and `pub fn motor_state(m: &Message) -> Option<MotorState>`
  - `pub fn notify_cmd(m: &Message) -> Option<u32>`
  - `pub struct Outgoing { pub msgid: u32, pub payload: Vec<u8> }` and builders `heartbeat_reply()`, `motor_to_mm(mm: i16)`, `motor_state_query()`, `power_off_timer(start: bool)`, `charging_on()`, `all_shutdown()`, each `-> Outgoing`; `pub mod cmd { MOTOR_CTRL = 3, MOTOR_CHECK = 4, POWER_CTRL = 5, CHARGE_CTRL = 7, MISC_CHECK = 0x0E }`.

- [ ] **Step 1: Write the failing tests** (bottom of `protocol.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::{encode, Parser};

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    fn parse(s: &str) -> Message {
        let mut parser = Parser::default();
        hex(s).into_iter().find_map(|b| parser.feed(b)).expect("a frame")
    }

    fn wire(out: &Outgoing) -> Vec<u8> {
        encode(0, out.msgid, &out.payload)
    }

    #[test]
    fn a_captured_heartbeat_is_the_battery() {
        let hb = heartbeat(&parse("FD 10 00 00 1C 00 00 00 00 00 00 00 00 00 64 00 52 10 24 00 00 00 00 00 00 02 62 A9")).unwrap();
        assert_eq!(
            hb,
            Heartbeat { battery_current_ma: 0, level: 100, voltage_mv: 4178, temperature_c: 36, usb_current_ma: 512 }
        );
    }

    #[test]
    fn captured_key_gestures_decode() {
        let cases = [
            ("FD 01 00 00 1D 00 00 65 00 00 0E 31 C7", Button::Pressed),
            ("FD 06 00 00 1E 00 00 65 00 00 0E 00 00 00 00 01 06 66", Button::Clicked),
            ("FD 06 00 00 21 00 00 65 00 00 0E 00 00 00 00 02 35 96", Button::DoubleClicked),
            ("FD 06 00 00 25 00 00 65 00 00 0E 00 00 00 00 04 51 D4", Button::Held1s),
            ("FD 06 00 00 26 00 00 65 00 00 0E 00 00 00 00 05 7E C3", Button::Released1To2s),
        ];
        for (frame, expected) in cases {
            assert_eq!(button(&parse(frame)), Some(expected), "{frame}");
        }
    }

    #[test]
    fn other_notifications_are_not_buttons() {
        // MISC_CHECK sub 1 (battery status) and a heartbeat.
        let mut battery = 0x0Eu32.to_le_bytes().to_vec();
        battery.extend([1, 1]);
        let mut parser = Parser::default();
        let m = encode(3, NOTIFY, &battery).into_iter().find_map(|b| parser.feed(b)).unwrap();
        assert_eq!(button(&m), None);
        assert_eq!(button(&parse("FD 10 00 00 1C 00 00 00 00 00 00 00 00 00 64 00 52 10 24 00 00 00 00 00 00 02 62 A9")), None);
    }

    #[test]
    fn commands_match_the_stock_frames() {
        assert_eq!(wire(&heartbeat_reply()), hex("FD 01 00 00 00 00 00 01 00 00 00 FC 8F"));
        assert_eq!(
            wire(&motor_to_mm(50)),
            hex("FD 11 00 00 00 00 00 64 00 00 03 00 00 00 00 02 00 00 00 32 00 A0 0F B8 0B 32 19 B9 86")
        );
        assert_eq!(&wire(&motor_to_mm(0))[19..], &hex("00 00 A0 0F B8 0B 28 14 79 53")[..]);
        assert_eq!(wire(&power_off_timer(true)), hex("FD 06 00 00 00 00 00 64 00 00 05 00 00 00 01 01 4D C7"));
        assert_eq!(&wire(&all_shutdown())[10..16], &hex("05 00 00 00 00 04")[..]);
        assert_eq!(&wire(&charging_on())[10..16], &hex("07 00 00 00 02 01")[..]);
        assert_eq!(&wire(&motor_state_query())[10..15], &hex("04 00 00 00 02")[..]);
    }

    #[test]
    fn motor_state_notifications_decode() {
        for (code, state) in [(0u8, MotorState::Idle), (2, MotorState::Blocked), (4, MotorState::Obstructed)] {
            let mut payload = cmd::MOTOR_CHECK.to_le_bytes().to_vec();
            payload.extend([2, code]);
            let mut parser = Parser::default();
            let m = encode(9, NOTIFY, &payload).into_iter().find_map(|b| parser.feed(b)).unwrap();
            assert_eq!(motor_state(&m), Some(state));
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p waved-mcu --lib protocol`
Expected: compile errors (items not defined).

- [ ] **Step 3: Write the implementation** — top of `crates/waved-mcu/src/protocol.rs`:

```rust
//! The MCU's messages (docs/MCU_PROTOCOL.md): decoding what it sends and
//! building the commands waved sends. `mcu_control` / `mcu_notify` payloads
//! are `u32 cmd` then `params[32]`, most with a sub-command in `params[0]`.

use crate::mavlink::Message;

pub const HEARTBEAT: u32 = 0;
pub const HEARTBEAT_REPLY: u32 = 1;
pub const CONTROL: u32 = 100;
pub const NOTIFY: u32 = 101;

/// `mcu_control` / `mcu_notify` command numbers waved uses.
pub mod cmd {
    pub const MOTOR_CTRL: u32 = 3;
    pub const MOTOR_CHECK: u32 = 4;
    pub const POWER_CTRL: u32 = 5;
    pub const CHARGE_CTRL: u32 = 7;
    pub const MISC_CHECK: u32 = 0x0E;
}

/// The battery fields of a heartbeat (msgid 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heartbeat {
    pub battery_current_ma: i32,
    pub level: u16,
    pub voltage_mv: u16,
    pub temperature_c: i16,
    pub usb_current_ma: u16,
}

pub fn heartbeat(m: &Message) -> Option<Heartbeat> {
    if m.msgid != HEARTBEAT || m.payload.len() < 16 {
        return None;
    }
    let p = &m.payload;
    Some(Heartbeat {
        battery_current_ma: i32::from_le_bytes([p[0], p[1], p[2], p[3]]),
        level: u16::from_le_bytes([p[4], p[5]]),
        voltage_mv: u16::from_le_bytes([p[6], p[7]]),
        temperature_c: i16::from_le_bytes([p[8], p[9]]),
        usb_current_ma: u16::from_le_bytes([p[14], p[15]]),
    })
}

/// The `cmd` of an `mcu_notify`.
pub fn notify_cmd(m: &Message) -> Option<u32> {
    (m.msgid == NOTIFY && m.payload.len() >= 4)
        .then(|| u32::from_le_bytes([m.payload[0], m.payload[1], m.payload[2], m.payload[3]]))
}

/// What the MCU reports about the power key (`mcu_notify` cmd 0x0E, sub 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Pressed,
    Clicked,
    DoubleClicked,
    /// Still held at 1 s.
    Held1s,
    /// Released between 1 and 2 s.
    Released1To2s,
    /// Still held at 2 s.
    Held2s,
    /// Released between 2 and 5 s.
    Released2To5s,
}

pub fn button(m: &Message) -> Option<Button> {
    if notify_cmd(m)? != cmd::MISC_CHECK || m.payload[4] != 0 {
        return None;
    }
    Some(match m.payload[5] {
        0 => Button::Pressed,
        1 => Button::Clicked,
        2 => Button::DoubleClicked,
        4 => Button::Held1s,
        5 => Button::Released1To2s,
        6 => Button::Held2s,
        7 => Button::Released2To5s,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotorState {
    Idle,
    Working,
    Blocked,
    WaitData,
    /// The lift's sensor saw an obstruction.
    Obstructed,
}

/// `mcu_notify` cmd 4 sub 2.
pub fn motor_state(m: &Message) -> Option<MotorState> {
    if notify_cmd(m)? != cmd::MOTOR_CHECK || m.payload[4] != 2 {
        return None;
    }
    Some(match m.payload[5] {
        0 => MotorState::Idle,
        1 => MotorState::Working,
        2 => MotorState::Blocked,
        3 => MotorState::WaitData,
        4 => MotorState::Obstructed,
        _ => return None,
    })
}

/// A message for the MCU, before framing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub msgid: u32,
    pub payload: Vec<u8>,
}

fn control(command: u32, params: &[u8]) -> Outgoing {
    let mut payload = command.to_le_bytes().to_vec();
    payload.extend_from_slice(params);
    payload.resize(36, 0);
    Outgoing { msgid: CONTROL, payload }
}

/// The reply to every heartbeat: 32 zero bytes.
pub fn heartbeat_reply() -> Outgoing {
    Outgoing { msgid: HEARTBEAT_REPLY, payload: vec![0; 32] }
}

/// Move the lift to `mm` (0, 30, 40 or 50) with stock's limits and speeds.
pub fn motor_to_mm(mm: i16) -> Outgoing {
    let (high, low): (i8, i8) = if mm == 0 { (40, 20) } else { (50, 25) };
    let mut params = vec![0u8]; // sub 0: move
    params.extend(2u32.to_le_bytes()); // mode POS
    params.extend(mm.to_le_bytes());
    params.extend(4000i16.to_le_bytes()); // up current limit
    params.extend(3000i16.to_le_bytes()); // down current limit
    params.extend([high as u8, low as u8]);
    control(cmd::MOTOR_CTRL, &params)
}

pub fn motor_state_query() -> Outgoing {
    control(cmd::MOTOR_CHECK, &[2])
}

/// The MCU's power-off timer (POWER_CTRL sub 1): start or stop.
pub fn power_off_timer(start: bool) -> Outgoing {
    control(cmd::POWER_CTRL, &[1, u8::from(start)])
}

/// Charging on (CHARGE_CTRL sub 2), so a powered-off unit charges.
pub fn charging_on() -> Outgoing {
    control(cmd::CHARGE_CTRL, &[2, 1])
}

/// Power everything off (POWER_CTRL sub 0, ALL_SHUTDOWN).
pub fn all_shutdown() -> Outgoing {
    control(cmd::POWER_CTRL, &[0, 4])
}
```

Add `pub mod protocol;` to `lib.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p waved-mcu --lib protocol`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-mcu/src/protocol.rs crates/waved-mcu/src/lib.rs
git commit -m "feat(mcu): decode the MCU's heartbeats and key events, build its commands

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: State contract: `Command::PowerOff` and the power-off phase

**Files:**
- Modify: `crates/waved-state/src/lib.rs` (`Command` enum near line 524, `McuState` near line 432)
- Modify: `crates/waved-ui/src/screens_v2/confirm.rs:91-93` and its test at `:229`

**Interfaces:**
- Produces: `Command::PowerOff`; `pub enum PowerOffPhase { #[default] Idle, InProgress, Sent, Aborted }`; `McuState.power_off: PowerOffPhase`.

- [ ] **Step 1: Write the failing test** — in `crates/waved-state/src/lib.rs` tests:

```rust
    #[test]
    fn power_off_is_its_own_command_and_starts_idle() {
        assert_ne!(Command::PowerOff, Command::Shutdown);
        assert_eq!(McuState::default().power_off, PowerOffPhase::Idle);
    }
```

and in `confirm.rs` change the existing assertion at line 229 to:

```rust
            assert!(matches!(f.commands().as_slice(), [Command::PowerOff]));
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p waved-state --lib power_off; cargo test -p waved-ui --lib confirm`
Expected: compile error (`PowerOff`, `PowerOffPhase` missing).

- [ ] **Step 3: Implement**

In `Command`, after `Shutdown,`:

```rust
    /// Switch the device off (the user confirmed, or the battery is empty):
    /// waved-mcu retracts the lift and has the MCU cut power. Unlike
    /// `Shutdown`, which every exit sends to stop the services.
    PowerOff,
```

`McuState` gains a field and its phase type:

```rust
pub struct McuState {
    pub motor_position: MotorPosition,
    pub usb_connected: bool,
    /// Where a `Command::PowerOff` is; `waved` waits for `Sent` or `Aborted`.
    pub power_off: PowerOffPhase,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PowerOffPhase {
    #[default]
    Idle,
    InProgress,
    /// The MCU was told to cut power.
    Sent,
    /// The lift was obstructed; the device stays on.
    Aborted,
}
```

(Keep `McuState`'s existing derives; `PowerOffPhase` derives `Default` so `#[derive(Default)]` on `McuState` still works.)

In `confirm.rs` `outcome`:

```rust
        ConfirmKind::PowerOff => done(
            Some(Command::PowerOff),
            Some(Message::new(Severity::Info, copy::POWERING_OFF)),
        ),
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p waved-state --lib && cargo test -p waved-ui --lib`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-state/src/lib.rs crates/waved-ui/src/screens_v2/confirm.rs
git commit -m "feat(state): power off is its own command, separate from stopping services

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The handler: heartbeats, battery, the power key, a silent link

**Files:**
- Create: `crates/waved-mcu/src/handler.rs`
- Modify: `crates/waved-mcu/src/lib.rs` (add `pub mod handler;`)

**Interfaces:**
- Consumes: `mavlink::Message`, `protocol::{heartbeat, button, Button, heartbeat_reply, Outgoing}`, `waved_state::{BatteryState, DeviceEvent, PowerKeyPress}`.
- Produces: `pub enum Action { Send(Outgoing), Battery(BatteryState), Event(DeviceEvent) }`; `pub struct Handler` with `pub fn on_message(&mut self, m: &Message, now_ms: u64) -> Vec<Action>` and `pub fn link_lost(&mut self, now_ms: u64) -> bool` (true once per silence of more than `LINK_LOST_MS`).

- [ ] **Step 1: Write the failing tests** (bottom of `handler.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::{encode, Parser};
    use crate::protocol::{cmd, HEARTBEAT, NOTIFY};

    fn msg(msgid: u32, payload: &[u8]) -> Message {
        let mut parser = Parser::default();
        encode(0, msgid, payload).into_iter().find_map(|b| parser.feed(b)).unwrap()
    }

    fn hb(level: u16, usb_ma: u16) -> Message {
        let mut p = vec![0u8; 32];
        p[4..6].copy_from_slice(&level.to_le_bytes());
        p[6..8].copy_from_slice(&4100u16.to_le_bytes());
        p[8..10].copy_from_slice(&30i16.to_le_bytes());
        p[14..16].copy_from_slice(&usb_ma.to_le_bytes());
        msg(HEARTBEAT, &p)
    }

    fn key(code: u8) -> Message {
        let mut p = cmd::MISC_CHECK.to_le_bytes().to_vec();
        p.extend([0, code]);
        msg(NOTIFY, &p)
    }

    fn events(actions: &[Action]) -> Vec<DeviceEvent> {
        actions.iter().filter_map(|a| match a { Action::Event(e) => Some(e.clone()), _ => None }).collect()
    }

    #[test]
    fn every_heartbeat_is_answered_and_published() {
        let mut handler = Handler::default();
        let actions = handler.on_message(&hb(80, 0), 0);
        assert!(actions.contains(&Action::Send(crate::protocol::heartbeat_reply())));
        assert!(actions.iter().any(|a| matches!(a, Action::Battery(b) if b.level == 80 && b.voltage == 4100 && b.temperature == 30 && !b.is_charging)));
        let charging = handler.on_message(&hb(81, 500), 2_000);
        assert!(charging.iter().any(|a| matches!(a, Action::Battery(b) if b.is_charging)));
    }

    #[test]
    fn click_and_two_second_hold_become_power_key_presses() {
        let mut handler = Handler::default();
        assert!(events(&handler.on_message(&key(0), 0)).is_empty(), "key down alone does nothing");
        assert_eq!(events(&handler.on_message(&key(1), 0)), [DeviceEvent::PowerKey(PowerKeyPress::Short)]);
        assert_eq!(events(&handler.on_message(&key(5), 0)), [DeviceEvent::PowerKey(PowerKeyPress::Short)]);
        assert_eq!(events(&handler.on_message(&key(6), 0)), [DeviceEvent::PowerKey(PowerKeyPress::Long)]);
        for ignored in [2u8, 4, 7] {
            assert!(events(&handler.on_message(&key(ignored), 0)).is_empty(), "code {ignored}");
        }
    }

    #[test]
    fn low_battery_is_raised_once_per_new_low_level_and_not_on_usb_power() {
        let mut handler = Handler::default();
        let mut t = 0;
        let mut low = Vec::new();
        for level in [12u16, 11, 9, 9, 9, 8] {
            t += 2_000;
            low.extend(events(&handler.on_message(&hb(level, 0), t)));
        }
        assert_eq!(low, [DeviceEvent::LowBattery(9), DeviceEvent::LowBattery(8)]);
        let mut plugged = Handler::default();
        assert!(events(&plugged.on_message(&hb(4, 1200), 0)).is_empty());
    }

    #[test]
    fn glitched_levels_are_ignored() {
        let mut handler = Handler::default();
        handler.on_message(&hb(100, 0), 0);
        for (t, bad) in [(2_000, 255u16), (4_000, 0)] {
            let actions = handler.on_message(&hb(bad, 0), t);
            assert!(events(&actions).is_empty(), "level {bad}");
            assert!(actions.iter().any(|a| matches!(a, Action::Battery(b) if b.level == 100)), "level {bad} kept 100");
        }
    }

    #[test]
    fn a_silent_link_is_reported_once() {
        let mut handler = Handler::default();
        assert!(!handler.link_lost(60_000), "never heard: not lost, just absent");
        handler.on_message(&hb(50, 0), 0);
        assert!(!handler.link_lost(4_000));
        assert!(handler.link_lost(6_000));
        assert!(!handler.link_lost(8_000), "reported once");
        handler.on_message(&hb(50, 0), 9_000);
        assert!(handler.link_lost(15_000), "again after it came back and went silent");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p waved-mcu --lib handler`
Expected: compile errors (`Handler`, `Action` missing).

- [ ] **Step 3: Write the implementation** — top of `crates/waved-mcu/src/handler.rs`:

```rust
//! What waved does with each message from the MCU, without any I/O: answer
//! heartbeats, publish the battery, turn the key into `PowerKey` presses
//! (click = sleep / wake, 2 s = "Power off?"; user decision 2026-10-12).

use crate::mavlink::Message;
use crate::protocol::{self, Button, Outgoing};
use waved_state::{BatteryState, DeviceEvent, PowerKeyPress};

/// Stock reports the link lost after this long without a heartbeat.
pub const LINK_LOST_MS: u64 = 5_000;
/// Levels at or above this are not a percentage (stock ignores them).
const LEVEL_INVALID: u16 = 111;
/// A jump this large between heartbeats is a glitch (stock drops it).
const LEVEL_MAX_STEP: u16 = 6;
/// `LowBattery` is raised below this level...
const LOW_LEVEL: u16 = 10;
/// ...and only while USB delivers less than this (stock's charging threshold).
const ON_USB_MA: u16 = 900;
/// USB current above this counts as charging.
const CHARGING_MA: u16 = 100;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Send(Outgoing),
    Battery(BatteryState),
    Event(DeviceEvent),
}

#[derive(Default)]
pub struct Handler {
    level: Option<u16>,
    last_low: Option<u16>,
    last_heartbeat_ms: Option<u64>,
    lost_reported: bool,
}

impl Handler {
    pub fn on_message(&mut self, m: &Message, now_ms: u64) -> Vec<Action> {
        if let Some(hb) = protocol::heartbeat(m) {
            return self.on_heartbeat(hb, now_ms);
        }
        let press = match protocol::button(m) {
            Some(Button::Clicked | Button::Released1To2s) => PowerKeyPress::Short,
            Some(Button::Held2s) => PowerKeyPress::Long,
            _ => return Vec::new(),
        };
        vec![Action::Event(DeviceEvent::PowerKey(press))]
    }

    fn on_heartbeat(&mut self, hb: protocol::Heartbeat, now_ms: u64) -> Vec<Action> {
        self.last_heartbeat_ms = Some(now_ms);
        self.lost_reported = false;
        let mut actions = vec![Action::Send(protocol::heartbeat_reply())];
        let plausible = hb.level < LEVEL_INVALID
            && self.level.is_none_or(|last| last.abs_diff(hb.level) < LEVEL_MAX_STEP);
        if plausible {
            self.level = Some(hb.level);
        }
        let Some(level) = self.level else {
            return actions;
        };
        actions.push(Action::Battery(BatteryState {
            level: level.min(100) as u8,
            voltage: hb.voltage_mv,
            temperature: hb.temperature_c.clamp(i16::from(i8::MIN), i16::from(i8::MAX)) as i8,
            is_charging: hb.usb_current_ma > CHARGING_MA && level < 100,
        }));
        if plausible && level < LOW_LEVEL && hb.usb_current_ma < ON_USB_MA && self.last_low != Some(level) {
            self.last_low = Some(level);
            actions.push(Action::Event(DeviceEvent::LowBattery(level as u8)));
        }
        actions
    }

    /// True once when heartbeats stopped for longer than `LINK_LOST_MS`.
    pub fn link_lost(&mut self, now_ms: u64) -> bool {
        let silent = self.last_heartbeat_ms.is_some_and(|t| now_ms.saturating_sub(t) > LINK_LOST_MS);
        if silent && !self.lost_reported {
            self.lost_reported = true;
            return true;
        }
        false
    }
}
```

Note: `DeviceEvent` must be `PartialEq` for the tests; if it is not, add `PartialEq` to its derive in `waved-state` (it holds `Uuid`, `String`, enums — all `PartialEq`) in this task and include `crates/waved-state/src/lib.rs` in the commit. Likewise `BatteryState` needs `PartialEq` for `Action: PartialEq`.

Add `pub mod handler;` to `lib.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p waved-mcu --lib handler && cargo test -p waved-state --lib`
Expected: 5 handler tests pass; waved-state unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-mcu/src/handler.rs crates/waved-mcu/src/lib.rs crates/waved-state/src/lib.rs
git commit -m "feat(mcu): heartbeats become the battery, the key becomes PowerKey presses

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Power off

**Files:**
- Create: `crates/waved-mcu/src/power_off.rs`
- Modify: `crates/waved-mcu/src/lib.rs` (add `pub mod power_off;`)

**Interfaces:**
- Consumes: `protocol::{power_off_timer, motor_to_mm, motor_state_query, charging_on, all_shutdown, motor_state, MotorState, notify_cmd, heartbeat, heartbeat_reply, Outgoing, cmd}`, `mavlink::Message`.
- Produces: `pub trait Transport { fn send(&self, out: &Outgoing); fn recv(&self, timeout: std::time::Duration) -> Option<Message>; }`; `pub enum Outcome { Sent, Obstructed }`; `pub fn run(t: &dyn Transport, limits: Limits) -> Outcome`; `pub struct Limits { pub motor_ms: u64, pub poll_ms: u64, pub ack_ms: u64, pub settle_ms: u64 }` with `pub const STOCK: Limits`.

- [ ] **Step 1: Write the failing tests** (bottom of `power_off.rs`):

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p waved-mcu --lib power_off`
Expected: compile errors.

- [ ] **Step 3: Write the implementation** — top of `crates/waved-mcu/src/power_off.rs`:

```rust
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
```

Add `pub mod power_off;` to `lib.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p waved-mcu --lib power_off`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-mcu/src/power_off.rs crates/waved-mcu/src/lib.rs
git commit -m "feat(mcu): stock's power-off sequence over a testable transport

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: The link and the service on `/dev/ttyS4`

**Files:**
- Create: `crates/waved-mcu/src/link.rs`
- Rewrite: `crates/waved-mcu/src/lib.rs`
- Delete: `crates/waved-mcu/src/battery.rs`, `buttons.rs`, `motor.rs`
- Modify: `crates/waved-mcu/Cargo.toml`, `crates/waved-mcu/src/simulate.rs`, `crates/waved/src/main.rs`

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces: `pub struct Link` with `pub fn open(path: &str, baud: u32) -> Result<Link, serialport::Error>` and `impl power_off::Transport for Link`; `McuService::new(link: Link, battery_tx, mcu_tx, events_tx)` and `run(self, cmd_rx)`; constants `pub const MCU_PORT: &str = "/dev/ttyS4"; pub const MCU_BAUD: u32 = 115_200;`.

- [ ] **Step 1: Write the failing test** — the service's command handling, against a fake link. In `lib.rs` tests:

```rust
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p waved-mcu --lib shutdown_is_not_power_off`
Expected: compile error (`command_plan`, `Plan` missing).

- [ ] **Step 3: Implement**

`crates/waved-mcu/Cargo.toml` dependencies: remove `waved-uart`, add `serialport = { version = "4", default-features = false }`; change the feature to `simulate = []`.

`crates/waved-mcu/src/link.rs`:

```rust
//! The MCU's serial line (`/dev/ttyS4`, 115200): a reader thread turns bytes
//! into messages; sends are framed with this side's sequence number.

use crate::mavlink::{encode, Message, Parser};
use crate::power_off::Transport;
use crate::protocol::Outgoing;
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError};
use serialport::SerialPort;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::time::Duration;
use tracing::{error, warn};

pub struct Link {
    writer: Mutex<(Box<dyn SerialPort>, u8)>,
    messages: Receiver<Message>,
}

impl Link {
    pub fn open(path: &str, baud: u32) -> Result<Self, serialport::Error> {
        let port = serialport::new(path, baud).timeout(Duration::from_millis(100)).open()?;
        let mut reader = port.try_clone()?;
        let (tx, rx) = bounded::<Message>(64);
        std::thread::Builder::new()
            .name("mcu-reader".into())
            .spawn(move || {
                let mut parser = Parser::default();
                let mut buf = [0u8; 64];
                loop {
                    match reader.read(&mut buf) {
                        Ok(n) => {
                            for &b in &buf[..n] {
                                if let Some(m) = parser.feed(b) {
                                    if tx.send(m).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                        Err(e) => {
                            error!("mcu: read failed: {e}");
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
            })
            .expect("spawn mcu-reader");
        Ok(Self { writer: Mutex::new((port, 0)), messages: rx })
    }

    pub fn messages(&self) -> &Receiver<Message> {
        &self.messages
    }
}

impl Transport for Link {
    fn send(&self, out: &Outgoing) {
        let mut guard = self.writer.lock().unwrap();
        let (port, seq) = &mut *guard;
        let frame = encode(*seq, out.msgid, &out.payload);
        *seq = seq.wrapping_add(1);
        if let Err(e) = port.write_all(&frame).and_then(|()| port.flush()) {
            warn!("mcu: write failed: {e}");
        }
    }

    fn recv(&self, timeout: Duration) -> Option<Message> {
        match self.messages.recv_timeout(timeout) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
        }
    }
}
```

`crates/waved-mcu/src/lib.rs` (replaces the whole file):

```rust
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
```

(`BatteryState` needs `PartialEq` — added in Task 4.)

Delete `battery.rs`, `buttons.rs`, `motor.rs`.

`crates/waved-mcu/src/simulate.rs`, in `run`'s command match, add:

```rust
                            Command::PowerOff => {
                                info!("[SIM] power off");
                                self.mcu_tx.send_modify(|s| s.power_off = waved_state::PowerOffPhase::Sent);
                            }
```

`crates/waved/src/main.rs`:
1. Keep `UART_PORT` (`/dev/ttyS1`) for Bluetooth, and replace the block that builds `waved_mcu::McuService` from `mcu_rx` (lines ~76–96) with: take the mux's MCU receiver and drain it on a thread (dropping it would stop the mux's reader, and with it Bluetooth):

```rust
                // ttyS1 is the Bluetooth module; its "MCU" route carries no
                // MCU traffic (docs/MCU_PROTOCOL.md). Keep draining it: a
                // dropped receiver stops the mux's reader, Bluetooth included.
                let mcu_rx = uart.take_mcu_receiver();
                std::thread::Builder::new()
                    .name("ttys1-drain".into())
                    .spawn(move || for _ in mcu_rx.iter() {})
                    .expect("spawn ttys1-drain");
```

and open the real link next to it:

```rust
        match waved_mcu::link::Link::open(waved_mcu::MCU_PORT, waved_mcu::MCU_BAUD) {
            Ok(link) => {
                info!("MCU link opened: {} @ {}", waved_mcu::MCU_PORT, waved_mcu::MCU_BAUD);
                let svc = waved_mcu::McuService::new(
                    link,
                    Arc::clone(&bus.battery),
                    Arc::clone(&bus.mcu),
                    bus.events.clone(),
                );
                let cmd_rx = bus.commands.subscribe();
                std::thread::Builder::new()
                    .name("mcu-srv".into())
                    .spawn(move || svc.run(cmd_rx))
                    .expect("spawn mcu-srv");
            }
            Err(e) => error!("failed to open the MCU link {}: {e}", waved_mcu::MCU_PORT),
        }
```

(Match the surrounding code's names for the bus fields; the old block used `mcu_battery_tx`, `mcu_state_tx`, `mcu_events_tx` cloned from `bus`.)
2. The lifecycle (`run_async`, ~line 795): `Ok(Command::PowerOff) => return ExitCause::PowerOff,` and `Ok(Command::Shutdown) => return ExitCause::Other,`. Rename `ExitCause::Shutdown` to `ExitCause::PowerOff` (doc: "the user powered off, or the battery is empty").
3. After the exit fan-out, replace the fixed `SHUTDOWN_POWER_OFF_WAIT` sleep for `PowerOff` with a wait on the MCU phase:

```rust
    if cause == ExitCause::PowerOff {
        info!("waiting for the MCU to power the device off");
        let mut mcu = bus.mcu.subscribe();
        let done = tokio::time::timeout(POWER_OFF_WAIT, mcu.wait_for(|s| {
            matches!(s.power_off, waved_state::PowerOffPhase::Sent | waved_state::PowerOffPhase::Aborted)
        }))
        .await;
        if done.is_err() {
            warn!("power off did not finish within {} s", POWER_OFF_WAIT.as_secs());
        }
        // The MCU cuts power shortly after ALL_SHUTDOWN; until then, stay.
        tokio::time::sleep(EXIT_WAIT).await;
    } else {
        tokio::time::sleep(EXIT_WAIT).await;
    }
```

with `const POWER_OFF_WAIT: Duration = Duration::from_secs(30);` replacing `SHUTDOWN_POWER_OFF_WAIT` and `exit_wait`; update the test `only_a_shutdown_waits_for_the_mcu_power_off_frame` to assert that `ExitCause::Other` waits `EXIT_WAIT` and drop the function-based assertion if `exit_wait` is removed.
4. The low-battery handler (~line 730): `Command::Shutdown` → `Command::PowerOff`; update `crates/waved/tests/low_battery_shutdown.rs` (`critical_battery_sends_shutdown`) to send and expect `Command::PowerOff`.

- [ ] **Step 4: Run all affected tests**

Run:
```bash
cargo test -p waved-mcu --lib
cargo test -p waved-state --lib && cargo test -p waved-ui --lib
scripts/linux-test.sh test -p waved --features ui,capture-alsa
scripts/linux-test.sh test -p waved --features simulate --test low_battery_shutdown
scripts/linux-test.sh check -p waved --features ui,capture-alsa
cargo clippy -p waved-mcu --lib --tests
```
Expected: all pass; no new clippy findings in `crates/waved-mcu`.

- [ ] **Step 5: Commit**

```bash
git add -A crates/waved-mcu crates/waved/src/main.rs crates/waved/tests/low_battery_shutdown.rs
git commit -m "feat(mcu): talk to the MCU on ttyS4: power key, battery, real power off

waved-mcu used to frame DC CD packets on ttyS1, which is the Bluetooth
module: the lift, the battery and power off never reached the MCU. It now
runs the MAVLink link on ttyS4.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Device verification and docs

**Files:**
- Modify: `docs/MCU_PROTOCOL.md` (a "waved" section), `crates/waved-system/README.md` (power key fact), `../TODO.md` (F7 done; F2 battery partly done), `CLAUDE.md` (the `WAVED_POWER_KEY` sentence)

- [ ] **Step 1: Build and deploy** (from `waved/`; procedure in `docs/DISPLAY_HARDWARE.md#deploy`, wait for waved to exit before the swap).

- [ ] **Step 2: Link and battery.** `adb shell grep -a "MCU link opened\|mcu:" /data/waved/waved.log | tail` shows the link opened and no "no heartbeat" warning after 10 s. The status pill / Menu shows the battery level the MCU reports (100 % on USB).

- [ ] **Step 3: Power key** (with the user): click → screen rests/off; click again → wakes; hold 2 s → "Power off?" appears while held; Cancel returns. Double click does nothing.

- [ ] **Step 4: Power off** (with the user): System → Power off → confirm. Expected: the lift retracts (if raised), the screen goes dark and the unit stays off (no "Starting" again); adb disconnects. **If the unit does not switch off within ~10 s**, stock also suspends the SoC after ALL_SHUTDOWN (`echo mem > /sys/power/state`): add that after `PowerOffPhase::Sent` in `main.rs`, rebuild, and repeat this step.

- [ ] **Step 5: Deploy safety.** `adb shell killall waved` (the watchdog restarts it): the unit stays on.

- [ ] **Step 6: Docs.** In `docs/MCU_PROTOCOL.md` replace the sentence "To watch the link with waved running (waved does not open `ttyS4` yet)" with a note that waved owns `ttyS4` (stop waved first to use `mcutool` or the capture snippet), and add a short "In waved" section naming `crates/waved-mcu/src/{mavlink,protocol,handler,power_off,link}.rs`. Update the power-key paragraph in `crates/waved-system/README.md` and the `WAVED_POWER_KEY` sentence in `CLAUDE.md` to say the key now arrives through `waved-mcu`. In `../TODO.md` tick F7 and note under F2 that the level, voltage, temperature and charging now come from MCU heartbeats.

- [ ] **Step 7: Commit**

```bash
git add docs/MCU_PROTOCOL.md crates/waved-system/README.md CLAUDE.md ../TODO.md
git commit -m "docs: waved owns the MCU link; power key and power off verified

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
