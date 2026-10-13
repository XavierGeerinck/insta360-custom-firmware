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
//! Stock runs this once per boot (pspapp starts once). waved starts far more
//! often: S60waved kills the first waved of every boot and restarts any
//! waved that exits, so stock's sequence at every start would save the
//! module's flash (`SaveFlash`, and `SetName` / `D8` may write it too) at
//! every restart, without bound in a crash loop. So waved writes only what
//! differs, at most once per boot for each value ([`Stamp`] in
//! [`STAMP_PATH`], a tmpfs emptied at boot):
//!
//! 1. `OpenBt` / `CloseBt`, as the switch is when the step comes (they write
//!    nothing on stock: its switch never saves the module).
//! 2. `GetVersion`, `GetBuildTime`, `GetStatus`, then `GetName` until `0xC6`:
//!    the module's name and whether a phone is connected, read first.
//! 3. `SetName` until `0x8C`, only when the module's name differs and this
//!    boot has tried that name fewer than [`MAX_RENAMES`] times.
//! 4. `AutoReconnect`: `D8 01` while the switch is on, `D8 00` while off (stock
//!    sends its saved setting either way; waved has no Auto Reconnect
//!    setting), as the switch is when the step comes. Not when this boot
//!    already sent that value, and not while a phone is connected (stock
//!    turns the reconnect off, `D8 00`, as a phone connects, 75955).
//! 5. `SaveFlash`, only when this start sent 3 or 4; then `GetName` again
//!    after a rename.
//! 6. `PtaMode(1)` (the Wave always has its Wi-Fi chip, ATBM6031), then
//!    `GetPhoneMac`.
//!
//! The 5 s poll names the module again while its name holds `XXXXXX`, as
//! stock's heartbeat does, within the same [`MAX_RENAMES`]. Stock's 10 s of
//! `bluetooth_set_volume` before `bt_info_init` (75110-75117, `0xD0 01`
//! volume events) is left out (see `volume`).

use crate::command::{id, Command, NAME_LEN};
use std::collections::VecDeque;
use waved_state::BluetoothState;

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

/// Where waved notes what it wrote to the module this boot. `/tmp` is a
/// tmpfs, emptied at every boot (deploy/S60waved).
pub const STAMP_PATH: &str = "/tmp/waved-bt-module";

/// `SetName` attempts per boot for one name: at start and from the 5 s poll
/// while the module is unnamed (stock renames every 5 s without end).
pub const MAX_RENAMES: u8 = 3;

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

/// What waved wrote to the module this boot ([`STAMP_PATH`]): one line per
/// field, the name in hex (a device name may hold any character).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    /// The name `SetName` sent, and how many times this boot.
    pub name: Option<String>,
    pub renames: u8,
    /// What `AutoReconnect` (`D8`, then `SaveFlash`) sent this boot.
    pub reconnect: Option<bool>,
}

impl Stamp {
    /// The stamp file's text; a line it does not know is left out.
    pub fn parse(text: &str) -> Self {
        let mut stamp = Self::default();
        for line in text.lines() {
            match line.split_once(' ') {
                Some(("name", hex)) => stamp.name = unhex(hex).and_then(|b| String::from_utf8(b).ok()),
                Some(("renames", n)) => stamp.renames = n.trim().parse().unwrap_or(0),
                Some(("reconnect", "1")) => stamp.reconnect = Some(true),
                Some(("reconnect", "0")) => stamp.reconnect = Some(false),
                _ => {}
            }
        }
        stamp
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        if let Some(name) = &self.name {
            let hex: String = name.bytes().map(|b| format!("{b:02x}")).collect();
            out.push_str(&format!("name {hex}\nrenames {}\n", self.renames));
        }
        if let Some(on) = self.reconnect {
            out.push_str(&format!("reconnect {}\n", u8::from(on)));
        }
        out
    }

    /// `SetName` attempts this boot for `name`.
    fn renames_of(&self, name: &str) -> u8 {
        if self.name.as_deref() == Some(name) {
            self.renames
        } else {
            0
        }
    }

    /// Count one more `SetName` attempt for `name`, if the boot has any left.
    fn try_rename(&mut self, name: &str) -> bool {
        let tries = self.renames_of(name);
        if tries >= MAX_RENAMES {
            return false;
        }
        self.name = Some(name.to_string());
        self.renames = tries + 1;
        true
    }
}

fn unhex(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim();
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
enum Step {
    Send(Command),
    /// Send until the module answers with `reply`, [`TRIES`] times at most.
    Until { command: Command, reply: u8 },
    /// `OpenBt` or `CloseBt`, as the switch is when the step comes.
    Switch,
    /// `SetName` until `0x8C`, when the module's name (read by now) differs.
    Name,
    /// `AutoReconnect`, as the switch is when the step comes, when needed.
    Reconnect,
    /// `SaveFlash`, when this start wrote a name or the reconnect switch.
    Save,
}

#[derive(Debug)]
struct Waiting {
    command: Command,
    reply: u8,
    tries: u8,
    resend_at_ms: u64,
}

/// The start-up as a schedule: [`Startup::poll`] gives the frames due,
/// [`Startup::heard`] takes each frame the module sends,
/// [`Startup::take_stamp`] what to note before they go out.
#[derive(Debug, Default)]
pub struct Startup {
    /// What `SetName` sends.
    name: String,
    /// What this boot wrote to the module, this start included.
    stamp: Stamp,
    /// `stamp` changed since the last [`Startup::take_stamp`].
    stamp_changed: bool,
    /// This start sent `SetName` / `D8`: `SaveFlash` is due.
    wrote: bool,
    /// This start sent `SetName`: read the name again after `SaveFlash`.
    renamed: bool,
    steps: VecDeque<Step>,
    waiting: Option<Waiting>,
}

impl Startup {
    /// The start-up for `settings.name`, after what `stamp` says this boot
    /// already wrote. The switch comes from `BluetoothState::bt_on` at each
    /// step, not from `settings`.
    pub fn new(settings: &Settings, stamp: Stamp) -> Self {
        let steps = [
            Step::Switch,
            Step::Send(Command::GetVersion),
            Step::Send(Command::GetBuildTime),
            Step::Send(Command::GetStatus),
            Step::Until { command: Command::GetName, reply: id::NAME },
            Step::Name,
            Step::Reconnect,
            Step::Save,
            Step::Send(Command::PtaMode { wifi: true }),
            Step::Send(Command::GetPhoneMac),
        ];
        Self {
            name: settings.name.clone(),
            stamp,
            steps: steps.into(),
            ..Self::default()
        }
    }

    /// The stamp to note before this poll's frames go out, when it changed.
    pub fn take_stamp(&mut self) -> Option<Stamp> {
        std::mem::take(&mut self.stamp_changed).then(|| self.stamp.clone())
    }

    /// The 5 s poll: stock names the module again while it still has its own
    /// default name (bt_heartbeat_thread 75122-75127), without `SaveFlash`;
    /// waved within [`MAX_RENAMES`] a boot.
    pub fn rename(&mut self, s: &BluetoothState) -> Vec<Command> {
        if !s.advertised_name.contains(UNNAMED) || !self.stamp.try_rename(&self.name) {
            return Vec::new();
        }
        self.stamp_changed = true;
        vec![Command::SetName(self.name.clone()), Command::GetName]
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

    /// The frames due at `now_ms`, in order. `s` is the state as it is now:
    /// the switch (`bt_on`), the module's name and a connected phone.
    pub fn poll(&mut self, now_ms: u64, s: &BluetoothState) -> Vec<Command> {
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
                Step::Switch => out.push(if s.bt_on { Command::OpenBt } else { Command::CloseBt }),
                Step::Name => {
                    let differs = s.advertised_name.trim_end() != self.name;
                    if differs && self.stamp.try_rename(&self.name) {
                        self.stamp_changed = true;
                        self.wrote = true;
                        self.renamed = true;
                        let command = Command::SetName(self.name.clone());
                        self.steps.push_front(Step::Until { command, reply: id::NAME_SET });
                    }
                }
                Step::Reconnect => {
                    if let Some(on) = self.reconnect(s) {
                        out.push(Command::AutoReconnect(on));
                    }
                }
                Step::Save => {
                    if self.wrote {
                        out.push(Command::SaveFlash);
                    }
                    if self.renamed {
                        self.steps.push_front(Step::Until { command: Command::GetName, reply: id::NAME });
                    }
                }
            }
        }
        out
    }

    /// `D8` as the switch is now, unless a phone is connected or this boot
    /// already sent that value.
    fn reconnect(&mut self, s: &BluetoothState) -> Option<bool> {
        let on = s.bt_on;
        if (on && s.connected) || self.stamp.reconnect == Some(on) {
            return None;
        }
        self.stamp.reconnect = Some(on);
        self.stamp_changed = true;
        self.wrote = true;
        Some(on)
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

    fn on() -> BluetoothState {
        BluetoothState { bt_on: true, ..Default::default() }
    }

    fn named(name: &str) -> BluetoothState {
        BluetoothState { advertised_name: name.into(), ..on() }
    }

    const NAME: &str = "Insta360 Wave BT 0A1B2C";
    const SET_NAME_STOCK: &str =
        "75 45 18 8B 49 6E 73 74 61 33 36 30 20 57 61 76 65 20 42 54 20 30 41 31 42 32 43 1D";

    fn startup(stamp: Stamp) -> Startup {
        Startup::new(&Settings { enabled: true, name: NAME.into() }, stamp)
    }

    #[test]
    fn the_first_start_of_a_boot_byte_for_byte() {
        // An unnamed module, no phone: stock's frames, the reads first.
        let mut s = startup(Stamp::default());
        assert_eq!(hex(&s.poll(0, &on())), [
            "75 45 02 CC 00 CE", // OpenBt
            "75 45 01 91 92",    // GetVersion
            "75 45 01 93 94",    // GetBuildTime
            "75 45 01 A9 AA",    // GetStatus
            "75 45 01 C5 C6",    // GetName
        ]);
        assert!(s.poll(500, &on()).is_empty(), "waiting for 0xC6");
        s.heard(id::NAME);
        let unnamed = named("Insta360 Wave BT XXXXXX");
        assert_eq!(hex(&s.poll(600, &unnamed)), [SET_NAME_STOCK]);
        s.heard(id::NAME_SET);
        assert_eq!(hex(&s.poll(700, &unnamed)), [
            "75 45 02 D8 01 DB", // AutoReconnect on
            "75 45 02 E0 00 E2", // SaveFlash
            "75 45 01 C5 C6",    // GetName: the new name
        ]);
        s.heard(id::NAME);
        assert_eq!(hex(&s.poll(800, &named(NAME))), ["75 45 03 EA 01 00 EE", "75 45 01 C7 C8"]);
        assert!(s.done());
        assert!(s.poll(10_000, &named(NAME)).is_empty());
        let stamp = s.take_stamp().expect("noted");
        assert_eq!(stamp, Stamp { name: Some(NAME.into()), renames: 1, reconnect: Some(true) });
        assert_eq!(s.take_stamp(), None, "once");
    }

    #[test]
    fn a_later_start_of_the_boot_only_reads() {
        let stamp = Stamp { name: Some(NAME.into()), renames: 1, reconnect: Some(true) };
        let mut s = startup(stamp);
        s.poll(0, &on());
        s.heard(id::NAME);
        assert_eq!(hex(&s.poll(100, &named(NAME))), ["75 45 03 EA 01 00 EE", "75 45 01 C7 C8"]);
        assert_eq!(s.take_stamp(), None);
    }

    #[test]
    fn switched_off_it_closes_bluetooth_and_stops_reconnecting() {
        let off = BluetoothState { advertised_name: NAME.into(), ..Default::default() };
        let mut s = startup(Stamp::default());
        assert_eq!(hex(&s.poll(0, &off))[0], "75 45 01 CB CC");
        s.heard(id::NAME);
        assert_eq!(&hex(&s.poll(1, &off))[..2], ["75 45 02 D8 00 DA", "75 45 02 E0 00 E2"]);
    }

    #[test]
    fn each_step_reads_the_switch_as_it_is_then() {
        let mut s = startup(Stamp::default());
        assert_eq!(s.poll(0, &on())[0], Command::OpenBt);
        s.heard(id::NAME);
        // Switched off while the start-up waited for the name.
        let off = BluetoothState { advertised_name: NAME.into(), ..Default::default() };
        let frames = s.poll(100, &off);
        assert_eq!(frames[0], Command::AutoReconnect(false));
        assert!(!frames.contains(&Command::AutoReconnect(true)) && !frames.contains(&Command::OpenBt));
    }

    #[test]
    fn get_name_is_tried_ten_times_a_second_apart_then_given_up() {
        // Stock 79443-79449: send, 100 ms for 0xC6, sleep 1 s, up to 10 tries.
        // The module never said its name: it is renamed (within the boot's tries).
        let mut s = startup(Stamp::default());
        let mut sent = s.poll(0, &on()).iter().filter(|c| **c == Command::GetName).count();
        let mut now = 0;
        for _ in 0..20 {
            now += RETRY_MS;
            let frames = s.poll(now, &on());
            if frames.first() != Some(&Command::GetName) {
                assert_eq!(frames.first(), Some(&Command::SetName(NAME.into())));
                break;
            }
            sent += 1;
            assert_eq!(frames.len(), 1);
        }
        assert_eq!(sent, usize::from(TRIES));
    }

    #[test]
    fn a_reply_to_something_else_does_not_end_the_wait() {
        let mut s = startup(Stamp::default());
        s.poll(0, &on());
        s.heard(id::NAME_SET);
        s.heard(0xB2);
        assert_eq!(s.poll(RETRY_MS, &on()), [Command::GetName], "tried again");
    }

    #[test]
    fn the_unnamed_module_is_renamed_within_the_boots_tries() {
        let mut s = startup(Stamp { name: Some(NAME.into()), renames: MAX_RENAMES - 1, reconnect: Some(true) });
        s.poll(0, &on());
        s.heard(id::NAME);
        let unnamed = named("Insta360 Wave BT XXXXXX");
        assert_eq!(s.poll(100, &unnamed), [Command::SetName(NAME.into())], "the last try");
        assert_eq!(s.take_stamp().map(|st| st.renames), Some(MAX_RENAMES));
        s.heard(id::NAME_SET);
        s.poll(200, &unnamed);
        s.heard(id::NAME);
        s.poll(300, &unnamed);
        assert!(s.done());
        assert!(s.rename(&unnamed).is_empty(), "no tries left");
        // A new name has its own tries.
        let mut s = Startup::new(&Settings { enabled: true, name: "Wave".into() }, s.stamp.clone());
        assert_eq!(s.rename(&unnamed), [Command::SetName("Wave".into()), Command::GetName]);
        assert_eq!(s.take_stamp().map(|st| (st.name, st.renames)), Some((Some("Wave".into()), 1)));
    }

    #[test]
    fn the_stamp_survives_its_file() {
        for stamp in [
            Stamp::default(),
            Stamp { name: Some("Salle de réunion\n2".into()), renames: 2, reconnect: Some(false) },
            Stamp { name: None, renames: 0, reconnect: Some(true) },
        ] {
            assert_eq!(Stamp::parse(&stamp.render()), stamp);
        }
        assert_eq!(Stamp::parse(""), Stamp::default());
        assert_eq!(Stamp::parse("name zz\nrenames x\nreconnect 7\nother 1\n"), Stamp::default());
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
