//! What waved tells the module when it starts: stock's `bt_info_init`
//! (libins_entry.so.c 79400-79456), and the name it gives the module.
//! docs/BLUETOOTH_PROTOCOL.md, "How stock runs the module".
//!
//! | Step | Stock | Line |
//! |------|-------|------|
//! | `SetName`, until `0x8C` (up to 10 tries, 1 s apart) | the same | 79416-79421 |
//! | `OpenBt` (`CC 00`) when the switch is on, else `CloseBt` (`CB`) | the same | 79422-79429 |
//! | `AutoReconnect` (`D8`) | the saved Auto Reconnect switch | 79438 |
//! | `SaveFlash` (`E0 00`) | the same | 79439 |
//! | `GetVersion`, `GetBuildTime`, `GetStatus` | the same | 79440-79442 |
//! | `GetName`, until `0xC6` (up to 10 tries) | the same | 79443-79449 |
//! | `PtaMode` (`EA 01 00`) | 1 when a Wi-Fi chip exists | 79450-79456 |
//! | `GetPhoneMac` | — (waved: the MAC that `0xE1` names) | |
//!
//! waved has no Auto Reconnect setting: `D8 01` (reconnect to the last
//! phone) while the switch is on, `D8 00` while off (stock sends its saved
//! setting either way). The Wave always has its Wi-Fi chip (ATBM6031), so
//! `PtaMode` is always 1. Stock's 10 s of `bluetooth_set_volume` before
//! `bt_info_init` (75110-75117, `0xD0 01` volume events) is left out (see
//! `volume`).

use crate::command::{id, Command, NAME_LEN};
use std::collections::VecDeque;

/// Stock waits 100 ms for the reply (`timeout_set`), then sleeps 1 s before
/// trying again (79420, 79447).
pub const RETRY_MS: u64 = 1_100;
/// Stock tries `SetName` and `GetName` up to 10 times.
pub const TRIES: u8 = 10;

/// The module's name before 1.2.2.6 gives it the serial's digits, and the
/// part stock always sends before them (`s_Insta360_Wave_BT_002f8d78`).
pub const STOCK_PREFIX: &str = "Insta360 Wave BT ";
/// The module's own default name contains this until it is named; stock's
/// heartbeat names it again while it does (75122-75127).
pub const UNNAMED: &str = "XXXXXX";

/// What the start-up needs from config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// `[bluetooth] enabled`.
    pub enabled: bool,
    /// What `SetName` sends ([`module_name`]).
    pub name: String,
}

/// The last six hex digits of `/proc/cpuinfo`'s `Serial` line, as stock
/// takes them (`extract_uuid`, 74100; no change of case).
pub fn serial_suffix(cpuinfo: &str) -> Option<String> {
    let line = cpuinfo
        .lines()
        .find(|line| line.trim_start().starts_with("Serial"))?;
    let (_, value) = line.split_once(':')?;
    let digits: String = value
        .trim()
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    (digits.len() >= 6).then(|| digits[digits.len() - 6..].to_string())
}

/// The name phones see: `device.name`, as the mockup shows it ("Shows up as
/// Wave Boardroom"), cut to [`NAME_LEN`] bytes; with no device name, stock's
/// `Insta360 Wave BT <six hex digits of the serial>`. `SetName` pads it with
/// NULs to stock's 23 bytes.
pub fn module_name(device_name: &str, serial: Option<&str>) -> String {
    let name = device_name.trim();
    if !name.is_empty() {
        let mut end = name.len().min(NAME_LEN);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        return name[..end].trim_end().to_string();
    }
    match serial {
        Some(serial) => format!("{STOCK_PREFIX}{serial}"),
        None => STOCK_PREFIX.trim_end().to_string(),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Step {
    Send(Command),
    /// Send until the module answers with `reply`, [`TRIES`] times at most.
    Until { command: Command, reply: u8 },
}

#[derive(Debug)]
struct Waiting {
    command: Command,
    reply: u8,
    tries: u8,
    resend_at_ms: u64,
}

/// Stock's start-up as a schedule: [`Startup::poll`] gives the frames due,
/// [`Startup::heard`] takes each frame the module sends.
#[derive(Debug, Default)]
pub struct Startup {
    steps: VecDeque<Step>,
    waiting: Option<Waiting>,
}

impl Startup {
    pub fn stock(settings: &Settings) -> Self {
        let on = settings.enabled;
        let steps = [
            Step::Until { command: Command::SetName(settings.name.clone()), reply: id::NAME_SET },
            Step::Send(if on { Command::OpenBt } else { Command::CloseBt }),
            Step::Send(Command::AutoReconnect(on)),
            Step::Send(Command::SaveFlash),
            Step::Send(Command::GetVersion),
            Step::Send(Command::GetBuildTime),
            Step::Send(Command::GetStatus),
            Step::Until { command: Command::GetName, reply: id::NAME },
            Step::Send(Command::PtaMode { wifi: true }),
            Step::Send(Command::GetPhoneMac),
        ];
        Self { steps: steps.into(), waiting: None }
    }

    /// Every step sent and every wait over.
    pub fn done(&self) -> bool {
        self.steps.is_empty() && self.waiting.is_none()
    }

    /// The module sent a frame with `cmd`: a reply we wait for ends the wait.
    pub fn heard(&mut self, cmd: u8) {
        if self.waiting.as_ref().is_some_and(|w| w.reply == cmd) {
            self.waiting = None;
        }
    }

    /// The frames due at `now_ms`, in order.
    pub fn poll(&mut self, now_ms: u64) -> Vec<Command> {
        let mut out = Vec::new();
        if let Some(waiting) = self.waiting.as_mut() {
            if now_ms < waiting.resend_at_ms {
                return out;
            }
            if waiting.tries < TRIES {
                waiting.tries += 1;
                waiting.resend_at_ms = now_ms + RETRY_MS;
                out.push(waiting.command.clone());
                return out;
            }
            // Stock goes on after the last try.
            self.waiting = None;
        }
        while let Some(step) = self.steps.pop_front() {
            match step {
                Step::Send(command) => out.push(command),
                Step::Until { command, reply } => {
                    out.push(command.clone());
                    self.waiting = Some(Waiting { command, reply, tries: 1, resend_at_ms: now_ms + RETRY_MS });
                    break;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(frames: &[Command]) -> Vec<String> {
        frames
            .iter()
            .map(|c| c.encode().iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "))
            .collect()
    }

    const SET_NAME_STOCK: &str =
        "75 45 18 8B 49 6E 73 74 61 33 36 30 20 57 61 76 65 20 42 54 20 30 41 31 42 32 43 1D";

    fn stock(enabled: bool) -> Startup {
        Startup::stock(&Settings { enabled, name: "Insta360 Wave BT 0A1B2C".into() })
    }

    #[test]
    fn the_stock_sequence_byte_for_byte() {
        let mut s = stock(true);
        assert_eq!(hex(&s.poll(0)), [SET_NAME_STOCK]);
        assert!(s.poll(500).is_empty(), "waiting for 0x8C");
        s.heard(0x8C);
        assert_eq!(hex(&s.poll(600)), [
            "75 45 02 CC 00 CE", // OpenBt
            "75 45 02 D8 01 DB", // AutoReconnect on
            "75 45 02 E0 00 E2", // SaveFlash
            "75 45 01 91 92",    // GetVersion
            "75 45 01 93 94",    // GetBuildTime
            "75 45 01 A9 AA",    // GetStatus
            "75 45 01 C5 C6",    // GetName
        ]);
        s.heard(0xC6);
        assert_eq!(hex(&s.poll(700)), ["75 45 03 EA 01 00 EE", "75 45 01 C7 C8"]);
        assert!(s.done());
        assert!(s.poll(10_000).is_empty());
    }

    #[test]
    fn switched_off_it_closes_bluetooth_and_stops_reconnecting() {
        let mut s = stock(false);
        s.poll(0);
        s.heard(0x8C);
        let frames = hex(&s.poll(1));
        assert_eq!(&frames[..2], ["75 45 01 CB CC", "75 45 02 D8 00 DA"]);
    }

    #[test]
    fn set_name_is_tried_ten_times_a_second_apart_then_given_up() {
        // Stock 79416-79421: send, 100 ms for 0x8C, sleep 1 s, up to 10 tries.
        let mut s = stock(true);
        let mut sent = s.poll(0).len();
        let mut now = 0;
        for _ in 0..20 {
            now += RETRY_MS;
            let frames = s.poll(now);
            if frames.first() != Some(&Command::SetName("Insta360 Wave BT 0A1B2C".into())) {
                // Given up: on to OpenBt and the rest.
                assert_eq!(frames.first(), Some(&Command::OpenBt));
                break;
            }
            sent += 1;
            assert_eq!(frames.len(), 1);
        }
        assert_eq!(sent, usize::from(TRIES));
    }

    #[test]
    fn a_reply_to_something_else_does_not_end_the_wait() {
        let mut s = stock(true);
        s.poll(0);
        s.heard(0xC6);
        s.heard(0xB2);
        assert_eq!(s.poll(RETRY_MS), [Command::SetName("Insta360 Wave BT 0A1B2C".into())], "tried again");
    }

    #[test]
    fn the_serial_gives_stocks_six_digits() {
        let cpuinfo = "processor\t: 0\nHardware\t: Amlogic\nSerial\t\t: 290a0b0c0d0e0f10a1b2c3d4\nRevision\t: 0000\n";
        assert_eq!(serial_suffix(cpuinfo).as_deref(), Some("b2c3d4"));
        assert_eq!(serial_suffix("Serial : 12ab\n"), None, "too short");
        assert_eq!(serial_suffix("Hardware : x\n"), None);
    }

    #[test]
    fn the_device_name_names_the_module() {
        assert_eq!(module_name("Wave Boardroom", Some("0A1B2C")), "Wave Boardroom");
        assert_eq!(module_name("  Board room  ", None), "Board room");
        // No device name: stock's name.
        assert_eq!(module_name("", Some("0A1B2C")), "Insta360 Wave BT 0A1B2C");
        assert_eq!(module_name(" ", None), "Insta360 Wave BT");
        // At most 23 bytes, never half a character.
        let long = module_name("The big conference room on floor three", None);
        assert_eq!(long, "The big conference room");
        let utf8 = module_name("Salle de réunion – étage 3", None);
        assert!(utf8.len() <= NAME_LEN && "Salle de réunion – étage 3".starts_with(&utf8), "{utf8}");
        // SetName pads to stock's 23 bytes.
        let frame = Command::SetName(module_name("Wave Boardroom", None)).frame();
        assert_eq!(frame.data.len(), NAME_LEN);
        assert_eq!(&frame.data[..14], b"Wave Boardroom");
        assert!(frame.data[14..].iter().all(|&b| b == 0));
    }
}
