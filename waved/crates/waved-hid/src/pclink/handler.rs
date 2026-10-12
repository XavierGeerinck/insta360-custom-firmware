//! One ack per command, and the bus commands it implies
//! (docs/WAVE_CONTROLLER_PROTOCOL.md §5–§8).
//!
//! Pure: the device state comes in as a [`Snapshot`], the PIN check and the
//! clock through traits, and the result is the ack bytes plus `Command`s.

use waved_state::Command;

use super::cmd::{self, unsupported};
use super::mode;
use super::packet::{Message, DEFAULT_TOKEN};
use super::snapshot::{meet, Snapshot};
use super::status::{self, put_str, LANGUAGE_NAME};

/// A pending on-device PIN prompt reads "not verified" after this long.
pub const PIN_PROMPT_TIMEOUT_MS: u64 = 60_000;

/// Outcome of one PIN check (K2: `waved_config::PinAttempts::check`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinOutcome {
    Accepted,
    Wrong,
    /// Too many wrong tries; waiting.
    Locked,
}

/// The device PIN check (D10); on the device, [`ConfigPins`].
pub trait PinVerifier: Send {
    fn check(&mut self, candidate: &str, now_ms: u64) -> PinOutcome;
}

/// Refuses every PIN (tests, or a build without a config).
pub struct DenyAllPins;

impl PinVerifier for DenyAllPins {
    fn check(&mut self, _candidate: &str, _now_ms: u64) -> PinOutcome {
        PinOutcome::Wrong
    }
}

/// The device PIN from config.toml (K2), with its own five-tries wait. The
/// on-device keypad keeps a separate count.
pub struct ConfigPins {
    security: waved_config::SecurityConfig,
    attempts: waved_config::PinAttempts,
}

impl ConfigPins {
    pub fn new(security: waved_config::SecurityConfig) -> Self {
        Self { security, attempts: waved_config::PinAttempts::new() }
    }
}

impl PinVerifier for ConfigPins {
    fn check(&mut self, candidate: &str, now_ms: u64) -> PinOutcome {
        match self.attempts.check(&self.security, candidate, now_ms) {
            waved_config::PinCheck::Accepted => PinOutcome::Accepted,
            waved_config::PinCheck::Wrong { .. } => PinOutcome::Wrong,
            waved_config::PinCheck::Locked { .. } => PinOutcome::Locked,
        }
    }
}

/// Wall-clock time the app sets, in the device's time zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WallTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// What the PC link needs from the rest of waved beyond bus commands.
pub trait PcLinkHost: Send {
    /// Set the system clock (and RTC); `false` when this build can't, which
    /// the app sees as result 1.
    fn set_time(&mut self, time: WallTime) -> bool;
}

/// No clock control: SET_SYS_TIME answers result 1.
pub struct NoClock;

impl PcLinkHost for NoClock {
    fn set_time(&mut self, _time: WallTime) -> bool {
        false
    }
}

/// MEETING_PC_STATUS: what the app says the computer's meeting software does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PcStatus {
    pub platform: u8,
    pub status: u8,
    pub software_running: bool,
    pub input_is_wave: bool,
    pub output_is_wave: bool,
}

/// Everything one command may read or call.
pub struct Context<'a> {
    pub snapshot: &'a Snapshot,
    pub now_ms: u64,
    pub pins: &'a mut dyn PinVerifier,
    pub host: &'a mut dyn PcLinkHost,
    /// An on-device PIN prompt is wired in (`Reply::prompt_pin` reaches it).
    pub prompt_available: bool,
}

/// The answer to one command.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    /// `None`: no ack at all (no stock handler, wrong token, too short).
    pub ack: Option<Vec<u8>>,
    /// The session token the ack's header carries.
    pub token: u32,
    pub commands: Vec<Command>,
    /// Show the on-device PIN prompt; report back with `pin_prompt_finished`.
    pub prompt_pin: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PinState {
    Idle,
    Pending { since_ms: u64 },
    Verified,
    Failed,
}

pub struct Handler {
    token: u32,
    pin: PinState,
    pc_status: Option<PcStatus>,
}

impl Default for Handler {
    fn default() -> Self {
        Self { token: DEFAULT_TOKEN, pin: PinState::Idle, pc_status: None }
    }
}

impl Handler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn token(&self) -> u32 {
        self.token
    }

    pub fn pc_status(&self) -> Option<PcStatus> {
        self.pc_status
    }

    /// The on-device PIN prompt closed.
    pub fn pin_prompt_finished(&mut self, verified: bool) {
        if matches!(self.pin, PinState::Pending { .. }) {
            self.pin = if verified { PinState::Verified } else { PinState::Failed };
        }
    }

    /// Answer one reassembled command.
    pub fn handle(&mut self, msg: &Message, ctx: &mut Context) -> Reply {
        let id = msg.header.cmd;
        let mut reply = Reply { token: self.token, ..Reply::default() };
        if cmd::ack_spec(id).is_none() {
            tracing::debug!("pclink: command {id:#04x} has no stock handler, no ack");
            return reply;
        }
        // Stock drops these silently when the session token does not match.
        if id > cmd::AUTH_DEV && msg.header.token != self.token {
            tracing::warn!("pclink: command {id:#04x} with a stale session token, dropped");
            return reply;
        }
        if msg.header.encrypted() || msg.overflowed {
            tracing::warn!("pclink: command {id:#04x} encrypted or too long, unsupported");
            reply.ack = unsupported(id);
            return reply;
        }
        reply.ack = self.answer(id, &msg.data, ctx, &mut reply.commands, &mut reply.prompt_pin);
        if id == cmd::UNAUTH_DEV {
            // The ack still carries the old token (stock resets after sending).
            self.token = DEFAULT_TOKEN;
            self.pin = PinState::Idle;
        }
        reply
    }

    /// Answer an out-of-band command (compid 1): the payload of an OB ack.
    pub fn handle_ob(&mut self, cmd: u16) -> Option<Vec<u8>> {
        match cmd {
            // File-transfer resume: we keep no file, so nothing was received.
            cmd::OB_LAST_PKT_SEQ => Some(0i32.to_le_bytes().to_vec()),
            cmd::OB_BACKUP_OFFSET => Some(vec![1]),
            _ => None,
        }
    }

    fn answer(
        &mut self,
        id: u16,
        data: &[u8],
        ctx: &mut Context,
        commands: &mut Vec<Command>,
        prompt_pin: &mut bool,
    ) -> Option<Vec<u8>> {
        let s = ctx.snapshot;
        // Minimum request sizes stock checks; shorter requests get no ack.
        let min_len = match id {
            cmd::VERIFY_DEV_PASSWD => 33,
            cmd::AUTH_DEV => 0x50,
            cmd::MEETING_PC_STATUS => 5,
            cmd::SET_SYS_ALL_DATE_TIME_SETTING => 9,
            cmd::SET_SYS_TIME => 0x18,
            cmd::SET_SYS_LANGUAGE | cmd::SET_SYS_TIMEZONE | cmd::GET_SYS_LANGUAGE_COUNTRY_LIST => 0x40,
            cmd::GET_SYS_COUNTRY_TIMEZONE_LIST => 0x80,
            cmd::MEETING_OP
            | cmd::MEETING_MODE
            | cmd::SET_AUDIO_SPEAKER_VOLUME
            | cmd::SET_AUDIO_SPEAKER_MUTE
            | cmd::SET_AUDIO_MIC_VOLUME
            | cmd::SET_AUDIO_MIC_MODE
            | cmd::SET_AUDIO_MIC_MUTE
            | cmd::SET_SYS_TIME_SHOW_MODE
            | cmd::SET_SYS_TIME_AUTO_SYNC => 1,
            _ => 0,
        };
        if data.len() < min_len || (id == cmd::MEETING_MARK && data.len() != 1) {
            tracing::warn!("pclink: command {id:#04x} with {} bytes, no ack", data.len());
            return None;
        }
        let byte = data.first().copied().unwrap_or(0);
        let ack = match id {
            cmd::GET_DEV_INFO => dev_info(s),
            cmd::GET_DEV_STATUS => status::dev_status(s).to_vec(),
            cmd::WIFI_GETSTATUS => status::wifi_status(s).to_vec(),
            cmd::GET_SYSTEM_BASE_INFO => status::system_base_info(s).to_vec(),
            cmd::GET_WIFI_BASE_INFO => status::wifi_base_info(s).to_vec(),
            cmd::GET_BIND_INFO => vec![0; 0x2D2],
            cmd::AUTH_DEV => {
                // Never bound: stock answers 1 and leaves the token alone.
                let mut a = vec![0u8; 0x55];
                a[0] = 1;
                a[1..5].copy_from_slice(&self.token.to_le_bytes());
                a
            }
            cmd::UNAUTH_DEV | cmd::SET_SYS_REBOOT => vec![0],
            cmd::VERIFY_DEV_PASSWD => vec![self.verify_pin(data, ctx, prompt_pin)],
            cmd::MEETING_OP => vec![meeting_op(byte, s, commands)],
            cmd::MEETING_GETSTATUS => {
                let mut a = vec![0u8; 0x205];
                a[0x201..].copy_from_slice(&s.meet_secs.to_le_bytes());
                a
            }
            cmd::MEETING_MARK => {
                if byte <= 1 && is_recording(s) {
                    commands.push(Command::AddBookmark);
                }
                vec![0]
            }
            cmd::MEETING_MODE => vec![u8::from(byte != 0)],
            cmd::MEETING_GET_INFO => {
                // Cloud upload info: echo the meeting id, result 1.
                let mut a = unsupported(id)?;
                let n = data.len().min(2);
                a[..n].copy_from_slice(&data[..n]);
                a
            }
            cmd::MEETING_PC_STATUS => {
                self.pc_status = Some(PcStatus {
                    platform: data[0],
                    status: data[1],
                    software_running: data[2] != 0,
                    input_is_wave: data[3] != 0,
                    output_is_wave: data[4] != 0,
                });
                vec![0]
            }
            cmd::SET_AUDIO_SPEAKER_VOLUME => {
                let v = byte.min(100);
                commands.push(Command::SetSpeakerVolume(u16::from(v)));
                if (v == 0) != s.speaker_muted {
                    commands.push(Command::ToggleSpeakerMute);
                }
                vec![0]
            }
            cmd::SET_AUDIO_MIC_VOLUME => {
                let v = byte.min(100);
                commands.push(Command::SetMicVolume(waved_config::clamp_mic_volume(u16::from(v))));
                if (v == 0) != s.mic_muted {
                    commands.push(Command::ToggleMicMute);
                }
                vec![0]
            }
            cmd::SET_AUDIO_SPEAKER_MUTE => {
                if (byte != 0) != s.speaker_muted {
                    commands.push(Command::ToggleSpeakerMute);
                }
                vec![0]
            }
            cmd::SET_AUDIO_MIC_MUTE => {
                if (byte != 0) != s.mic_muted {
                    commands.push(Command::ToggleMicMute);
                }
                vec![0]
            }
            cmd::SET_AUDIO_MIC_MODE => match mode::to_audio_mode(byte, s.beam_aim_deg) {
                Some(m) => {
                    commands.push(Command::SetAudioMode(m));
                    vec![0]
                }
                None => vec![1],
            },
            cmd::GET_SYS_LANGUAGE_LIST => {
                let mut a = vec![0u8; 0x501];
                a[0] = 1;
                put_str(&mut a[1..65], LANGUAGE_NAME);
                a
            }
            cmd::GET_SYS_LANGUAGE_COUNTRY_LIST => vec![0; 0x4102],
            cmd::GET_SYS_COUNTRY_TIMEZONE_LIST => {
                let mut a = vec![0u8; 0x1E02];
                a[0] = 1;
                put_str(&mut a[2..66], &s.time_zone);
                a
            }
            cmd::GET_SYS_LANGUAGE => {
                let mut a = vec![0u8; 0x41];
                put_str(&mut a[..64], LANGUAGE_NAME);
                a
            }
            cmd::GET_SYS_COUNTRY => vec![0; 0x41],
            cmd::GET_SYS_CITY => {
                let mut a = vec![0u8; 0x41];
                put_str(&mut a[1..], &s.time_zone);
                a
            }
            cmd::GET_SYS_ALL_SETTING_LIST => {
                let mut a = vec![0u8; 0x6406];
                a[0] = 1;
                put_str(&mut a[1..65], LANGUAGE_NAME);
                a[0x4603] = 1;
                put_str(&mut a[0x4605..0x4645], &s.time_zone);
                a
            }
            cmd::GET_SYS_DEFAULT_SETTING => {
                let mut a = vec![0u8; 0xC1];
                put_str(&mut a[0..64], LANGUAGE_NAME);
                put_str(&mut a[128..192], &s.time_zone);
                a
            }
            cmd::SET_SYS_LANGUAGE => {
                let name = c_string(&data[..64]);
                let mut a = vec![0u8; 0x41];
                put_str(&mut a[..64], &name);
                a[64] = u8::from(name != LANGUAGE_NAME);
                a
            }
            cmd::SET_SYS_TIMEZONE => {
                let name = c_string(&data[..64]);
                if waved_config::valid_time_zone(&name) {
                    commands.push(Command::SetTimeZone(name));
                    vec![0]
                } else {
                    vec![0xFF]
                }
            }
            cmd::SET_SYS_TIME_SHOW_MODE => {
                commands.push(Command::SetClock24h(byte == 1));
                vec![0]
            }
            cmd::SET_SYS_TIME_AUTO_SYNC => vec![u8::from(byte == 0)],
            cmd::SET_SYS_ALL_DATE_TIME_SETTING => {
                commands.push(Command::SetClock24h(data[0] == 1));
                let set = data[1] != 0
                    || ctx.host.set_time(WallTime {
                        year: u16::from_le_bytes([data[2], data[3]]),
                        month: data[4],
                        day: data[5],
                        hour: data[6],
                        minute: data[7],
                        second: data[8],
                    });
                vec![u8::from(!set)]
            }
            cmd::SET_SYS_TIME => {
                let field = |i: usize| u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
                let narrow = |v: u32| u8::try_from(v).unwrap_or(u8::MAX);
                let time = WallTime {
                    year: u16::try_from(field(0)).unwrap_or(u16::MAX),
                    month: narrow(field(4)),
                    day: narrow(field(8)),
                    hour: narrow(field(12)),
                    minute: narrow(field(16)),
                    second: narrow(field(20)),
                };
                vec![u8::from(!ctx.host.set_time(time))]
            }
            // Binding, cloud, OTA, Wi-Fi setup, storage mode, factory reset,
            // EQ, country, camera module: not offered.
            _ => unsupported(id)?,
        };
        Some(ack)
    }

    fn verify_pin(&mut self, data: &[u8], ctx: &mut Context, prompt_pin: &mut bool) -> u8 {
        if let PinState::Pending { since_ms } = self.pin {
            if ctx.now_ms.saturating_sub(since_ms) >= PIN_PROMPT_TIMEOUT_MS {
                self.pin = PinState::Failed;
            }
        }
        if data[0] != 0 {
            return match self.pin {
                PinState::Pending { .. } => 0,
                PinState::Verified => 1,
                PinState::Idle | PinState::Failed => 2,
            };
        }
        let candidate = c_string(&data[1..33]);
        self.pin = if !ctx.snapshot.pin_enabled {
            PinState::Verified
        } else if !candidate.is_empty() {
            match ctx.pins.check(&candidate, ctx.now_ms) {
                PinOutcome::Accepted => PinState::Verified,
                PinOutcome::Wrong | PinOutcome::Locked => PinState::Failed,
            }
        } else if ctx.prompt_available {
            *prompt_pin = true;
            PinState::Pending { since_ms: ctx.now_ms }
        } else {
            PinState::Failed
        };
        0
    }
}

fn is_recording(s: &Snapshot) -> bool {
    matches!(s.meet_status, meet::RECORDING | meet::PAUSED)
}

/// Stock `slave_meeting_operate`: 0 start, 1 finish, 2 pause, 3 resume.
fn meeting_op(op: u8, s: &Snapshot, commands: &mut Vec<Command>) -> u8 {
    const STORAGE_FULL: u8 = 5;
    const UNSUPPORTED: u8 = 7;
    match op {
        0 if !is_recording(s) && s.storage_blocked => return STORAGE_FULL,
        0 if !is_recording(s) => commands.push(Command::StartRecording),
        1 if is_recording(s) => commands.push(Command::StopRecording),
        2 if s.meet_status == meet::RECORDING => commands.push(Command::PauseRecording),
        3 if s.meet_status == meet::PAUSED => commands.push(Command::ResumeRecording),
        0..=3 => {}
        _ => return UNSUPPORTED,
    }
    0
}

/// A NUL-terminated string from a fixed field.
fn c_string(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// `pc_link_get_dev_info_ack_t`, with stock's fallbacks for every serial.
fn dev_info(s: &Snapshot) -> Vec<u8> {
    let mut a = vec![0u8; 0x3A5];
    put_str(&mut a[0x000..0x010], "0000000000000000");
    put_str(&mut a[0x012..0x022], "00000000000");
    a[0x222] = u8::from(s.pin_enabled);
    a[0x223] = u8::from(s.first_run_done);
    a[0x224] = u8::from(s.wifi_connected);
    put_str(&mut a[0x225..0x245], &s.version);
    put_str(&mut a[0x245..0x265], "0.0.0.1");
    put_str(&mut a[0x265..0x285], "0.0.0.1");
    put_str(&mut a[0x285..0x2A4], "v1.0.0_build60");
    a[0x2C4] = 1;
    put_str(&mut a[0x2C5..0x2E5], "0.0.0.2");
    put_str(&mut a[0x2E5..0x305], "insta360");
    put_str(&mut a[0x385..0x3A5], "PSP_0000000000000000");
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pclink::packet::Header;
    use waved_state::AudioMode;

    struct Pins(Vec<(String, u64)>, PinOutcome);

    #[test]
    fn config_pins_use_the_device_pin_and_its_wait() {
        let security = waved_config::SecurityConfig {
            device_password: waved_config::hash_pin("4821"),
            ..Default::default()
        };
        let mut pins = ConfigPins::new(security);
        assert_eq!(pins.check("4821", 0), PinOutcome::Accepted);
        for _ in 0..5 {
            assert_eq!(pins.check("0000", 10), PinOutcome::Wrong);
        }
        assert_eq!(pins.check("4821", 20), PinOutcome::Locked, "five wrong tries start the wait");
        assert_eq!(pins.check("4821", 20 + waved_config::PIN_LOCKOUT_MS), PinOutcome::Accepted);
    }

    impl PinVerifier for Pins {
        fn check(&mut self, candidate: &str, now_ms: u64) -> PinOutcome {
            self.0.push((candidate.to_string(), now_ms));
            self.1
        }
    }

    struct Clock(Option<WallTime>, bool);

    impl PcLinkHost for Clock {
        fn set_time(&mut self, time: WallTime) -> bool {
            self.0 = Some(time);
            self.1
        }
    }

    fn snap() -> Snapshot {
        Snapshot {
            speaker_volume: 40,
            mic_volume: 80,
            mic_mode: 2,
            beam_aim_deg: 45.0,
            battery_level: 64,
            wifi_connected: true,
            wifi_ssid: "Office".into(),
            wifi_ip: "192.168.1.20".into(),
            wifi_rssi_dbm: -55,
            time_zone: "Europe/Brussels".into(),
            clock_24h: true,
            first_run_done: true,
            version: "0.1.0 (abc1234)".into(),
            ..Snapshot::default()
        }
    }

    fn msg(cmd: u16, data: &[u8]) -> Message {
        msg_with_token(cmd, data, DEFAULT_TOKEN)
    }

    fn msg_with_token(cmd: u16, data: &[u8], token: u32) -> Message {
        let header = Header { token, version: 1, flags: 0x24, cmd, seq: 0, payload_len: data.len() as u32 };
        Message { header, data: data.to_vec(), overflowed: false }
    }

    struct Rig {
        handler: Handler,
        snap: Snapshot,
        pins: Pins,
        clock: Clock,
        prompt: bool,
        now: u64,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                handler: Handler::new(),
                snap: snap(),
                pins: Pins(Vec::new(), PinOutcome::Accepted),
                clock: Clock(None, true),
                prompt: true,
                now: 1_000,
            }
        }

        fn send(&mut self, m: Message) -> Reply {
            let mut ctx = Context {
                snapshot: &self.snap,
                now_ms: self.now,
                pins: &mut self.pins,
                host: &mut self.clock,
                prompt_available: self.prompt,
            };
            self.handler.handle(&m, &mut ctx)
        }

        fn ack(&mut self, cmd: u16, data: &[u8]) -> Vec<u8> {
            self.send(msg(cmd, data)).ack.expect("an ack")
        }

        fn commands(&mut self, cmd: u16, data: &[u8]) -> (Vec<u8>, Vec<Command>) {
            let r = self.send(msg(cmd, data));
            (r.ack.expect("an ack"), r.commands)
        }
    }

    fn cstr(field: &[u8]) -> &str {
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        std::str::from_utf8(&field[..end]).unwrap()
    }

    // --- transport-level rules ---

    #[test]
    fn ids_without_a_stock_handler_get_no_ack() {
        let mut rig = Rig::new();
        for id in [0x2C, 0x39, 0x41, 0x80, 0xFF] {
            assert_eq!(rig.send(msg(id, &[0])).ack, None, "{id:#x}");
        }
    }

    #[test]
    fn a_wrong_token_drops_commands_above_0x13_but_not_the_early_ones() {
        let mut rig = Rig::new();
        assert_eq!(rig.send(msg_with_token(cmd::GET_DEV_STATUS, &[], 7)).ack, None);
        assert!(rig.send(msg_with_token(cmd::GET_DEV_INFO, &[], 7)).ack.is_some());
        assert!(rig.send(msg_with_token(cmd::AUTH_DEV, &[0; 80], 7)).ack.is_some());
    }

    #[test]
    fn encrypted_or_oversized_commands_get_the_unsupported_ack() {
        let mut rig = Rig::new();
        let mut m = msg(cmd::SET_AUDIO_MIC_MUTE, &[1]);
        m.header.flags |= 0x10;
        let r = rig.send(m);
        assert_eq!(r.ack, Some(vec![1]));
        assert!(r.commands.is_empty());
        let mut m = msg(cmd::FILE_TRANS, &[0; 12]);
        m.overflowed = true;
        assert_eq!(rig.send(m).ack, Some(vec![1]));
    }

    // --- device info and status ---

    #[test]
    fn get_dev_info_sends_no_serial_and_the_stock_fallbacks() {
        let mut rig = Rig::new();
        rig.snap.pin_enabled = true;
        let a = rig.ack(cmd::GET_DEV_INFO, &[]);
        assert_eq!(a.len(), 0x3A5);
        assert_eq!(cstr(&a[0x000..0x010]), "000000000000000", "15 zeros, as stock's snprintf(16) leaves");
        assert_eq!(a[0x010], 0, "not bound");
        assert_eq!(cstr(&a[0x012..0x022]), "00000000000");
        assert_eq!(cstr(&a[0x022..0x222]), "");
        assert_eq!((a[0x222], a[0x223], a[0x224]), (1, 1, 1), "PIN set, set up, Wi-Fi up");
        assert_eq!(cstr(&a[0x225..0x245]), "0.1.0 (abc1234)");
        assert_eq!(cstr(&a[0x245..0x265]), "0.0.0.1");
        assert_eq!(cstr(&a[0x265..0x285]), "0.0.0.1");
        assert_eq!(cstr(&a[0x285..0x2A4]), "v1.0.0_build60");
        assert_eq!(a[0x2C4], 1, "SK algorithm");
        assert_eq!(cstr(&a[0x2C5..0x2E5]), "0.0.0.2");
        assert_eq!(cstr(&a[0x2E5..0x305]), "insta360");
        assert_eq!(cstr(&a[0x385..0x3A5]), "PSP_0000000000000000");
    }

    #[test]
    fn status_queries_return_the_status_structs() {
        let mut rig = Rig::new();
        let s = rig.snap.clone();
        assert_eq!(rig.ack(cmd::GET_DEV_STATUS, &[]), status::dev_status(&s).to_vec());
        assert_eq!(rig.ack(cmd::WIFI_GETSTATUS, &[]), status::wifi_status(&s).to_vec());
        assert_eq!(rig.ack(cmd::GET_SYSTEM_BASE_INFO, &[]), status::system_base_info(&s).to_vec());
        assert_eq!(rig.ack(cmd::GET_WIFI_BASE_INFO, &[]), status::wifi_base_info(&s).to_vec());
    }

    // --- session ---

    #[test]
    fn auth_dev_on_an_unbound_unit_fails_and_keeps_the_token() {
        let mut rig = Rig::new();
        let r = rig.send(msg(cmd::AUTH_DEV, &[0x31; 80]));
        let a = r.ack.unwrap();
        assert_eq!(a.len(), 0x55);
        assert_eq!(a[0], 1);
        assert_eq!(&a[1..5], &DEFAULT_TOKEN.to_le_bytes());
        assert!(a[5..].iter().all(|&b| b == 0));
        assert_eq!(r.token, DEFAULT_TOKEN);
        assert_eq!(rig.send(msg(cmd::AUTH_DEV, &[0; 79])).ack, None, "short");
    }

    #[test]
    fn unauth_dev_acks_with_the_old_token_then_resets() {
        let mut rig = Rig::new();
        rig.handler.token = 0x1234;
        let r = rig.send(msg_with_token(cmd::UNAUTH_DEV, &[], 0x1234));
        assert_eq!((r.ack, r.token), (Some(vec![0]), 0x1234));
        assert_eq!(rig.handler.token(), DEFAULT_TOKEN);
    }

    #[test]
    fn binding_and_cloud_commands_are_unsupported() {
        let mut rig = Rig::new();
        for id in [cmd::AUTH_UNBIND_DEV, cmd::UNBIND_OFFLINE, cmd::BIND_DEV, cmd::UNBIND_DEV, cmd::MEETING_SCREENSHOT] {
            assert_eq!(rig.ack(id, &[0; 4]), vec![1], "{id:#x}");
        }
        let start = rig.ack(cmd::GET_BIND_START, &[]);
        assert_eq!((start.len(), start[0]), (17, 1));
        assert!(start[1..].iter().all(|&b| b == 0), "no key handed out");
        let info = rig.ack(cmd::MEETING_GET_INFO, &[0x34, 0x12]);
        assert_eq!((info.len(), &info[0..2], info[0x283]), (0x284, &[0x34, 0x12][..], 1));
        assert!(rig.ack(cmd::MEETING_GET_CUR_ALL_INFO, &[]).iter().all(|&b| b == 0));
    }

    #[test]
    fn get_bind_info_reports_not_bound() {
        let mut rig = Rig::new();
        let a = rig.ack(cmd::GET_BIND_INFO, &[]);
        assert_eq!(a.len(), 0x2D2);
        assert!(a.iter().all(|&b| b == 0));
    }

    #[test]
    fn os_ota_wifi_config_and_camera_commands_are_unsupported() {
        let mut rig = Rig::new();
        for id in [
            cmd::WIFI_SCAN, cmd::WIFI_CONNECT, cmd::WIFI_FORGETSSID, cmd::WIFI_SWITCH,
            cmd::WIFI_AUTO_CONNECT_SWITCH, cmd::SET_SYS_ENABLE_UDISK_MODE, cmd::SET_SYS_RECOVERY,
            cmd::OTA_ENABLE_UDISK, cmd::OTA_OP, cmd::OTA_FINISH_SURE, cmd::FILE_TRANS,
            cmd::PUC_ZOOM_CONTROL, cmd::PUC_POS_RESET, cmd::PUC_POS_CONTROL, cmd::PUC_SET_MODE,
            cmd::SET_AUDIO_SPEAKER_MODE, cmd::SET_SYS_COUNTRY,
        ] {
            let r = rig.send(msg(id, &[0; 200]));
            assert_eq!(r.ack, Some(vec![1]), "{id:#x}");
            assert!(r.commands.is_empty(), "{id:#x}");
        }
        assert_eq!(rig.ack(cmd::OTA_GETSTATUS, &[]), vec![1, 0, 0, 0]);
        let list = rig.ack(cmd::WIFI_LIST, &[]);
        assert_eq!((list.len(), list[0], list[1]), (0x10C2, 1, 0));
        let disc = rig.ack(cmd::WIFI_DISCONNECT, &[0; 65]);
        assert_eq!((disc.len(), disc[0x40]), (0x41, 1));
    }

    #[test]
    fn reboot_is_acked_and_does_nothing_like_stock() {
        let mut rig = Rig::new();
        assert_eq!(rig.commands(cmd::SET_SYS_REBOOT, &[]), (vec![0], vec![]));
    }

    // --- PIN (VERIFY_DEV_PASSWD) ---

    fn verify(op: u8, pin: &str) -> Vec<u8> {
        let mut d = vec![0u8; 33];
        d[0] = op;
        d[1..1 + pin.len()].copy_from_slice(pin.as_bytes());
        d
    }

    #[test]
    fn without_a_pin_the_request_is_verified_at_once() {
        let mut rig = Rig::new();
        assert_eq!(rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(0, "")), vec![0]);
        assert_eq!(rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(1, "")), vec![1]);
        assert!(rig.pins.0.is_empty(), "nothing to check");
    }

    #[test]
    fn a_pin_sent_by_the_app_goes_through_the_pin_check() {
        let mut rig = Rig::new();
        rig.snap.pin_enabled = true;
        assert_eq!(rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(0, "4821")), vec![0]);
        assert_eq!(rig.pins.0, vec![("4821".to_string(), 1_000)]);
        assert_eq!(rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(1, "")), vec![1]);
        rig.pins.1 = PinOutcome::Wrong;
        rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(0, "0000"));
        assert_eq!(rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(1, "")), vec![2]);
        rig.pins.1 = PinOutcome::Locked;
        rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(0, "4821"));
        assert_eq!(rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(1, "")), vec![2]);
    }

    #[test]
    fn an_empty_pin_asks_for_the_device_prompt_and_polls_its_result() {
        let mut rig = Rig::new();
        rig.snap.pin_enabled = true;
        let r = rig.send(msg(cmd::VERIFY_DEV_PASSWD, &verify(0, "")));
        assert_eq!((r.ack, r.prompt_pin), (Some(vec![0]), true));
        assert_eq!(rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(1, "")), vec![0], "still showing");
        rig.handler.pin_prompt_finished(true);
        assert_eq!(rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(1, "")), vec![1]);
        rig.send(msg(cmd::VERIFY_DEV_PASSWD, &verify(0, "")));
        rig.handler.pin_prompt_finished(false);
        assert_eq!(rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(1, "")), vec![2]);
    }

    #[test]
    fn a_prompt_nobody_answers_fails_after_60_s_and_none_wired_fails_at_once() {
        let mut rig = Rig::new();
        rig.snap.pin_enabled = true;
        rig.send(msg(cmd::VERIFY_DEV_PASSWD, &verify(0, "")));
        rig.now += PIN_PROMPT_TIMEOUT_MS;
        assert_eq!(rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(1, "")), vec![2]);
        rig.prompt = false;
        let r = rig.send(msg(cmd::VERIFY_DEV_PASSWD, &verify(0, "")));
        assert!(!r.prompt_pin);
        assert_eq!(rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(1, "")), vec![2]);
    }

    #[test]
    fn never_asked_reads_not_verified_and_short_requests_get_no_ack() {
        let mut rig = Rig::new();
        rig.snap.pin_enabled = true;
        assert_eq!(rig.ack(cmd::VERIFY_DEV_PASSWD, &verify(1, "")), vec![2]);
        assert_eq!(rig.send(msg(cmd::VERIFY_DEV_PASSWD, &[0; 32])).ack, None);
    }

    // --- meeting ---

    #[test]
    fn meeting_op_starts_stops_pauses_and_resumes() {
        let mut rig = Rig::new();
        assert_eq!(rig.commands(cmd::MEETING_OP, &[0]), (vec![0], vec![Command::StartRecording]));
        rig.snap.meet_status = meet::RECORDING;
        assert_eq!(rig.commands(cmd::MEETING_OP, &[0]), (vec![0], vec![]), "already recording");
        assert_eq!(rig.commands(cmd::MEETING_OP, &[2]), (vec![0], vec![Command::PauseRecording]));
        rig.snap.meet_status = meet::PAUSED;
        assert_eq!(rig.commands(cmd::MEETING_OP, &[3]), (vec![0], vec![Command::ResumeRecording]));
        assert_eq!(rig.commands(cmd::MEETING_OP, &[1]), (vec![0], vec![Command::StopRecording]));
        rig.snap.meet_status = meet::IDLE;
        assert_eq!(rig.commands(cmd::MEETING_OP, &[1]), (vec![0], vec![]), "nothing to stop");
        assert_eq!(rig.commands(cmd::MEETING_OP, &[9]), (vec![7], vec![]), "stock's unsupported code");
        assert_eq!(rig.send(msg(cmd::MEETING_OP, &[])).ack, None);
    }

    #[test]
    fn meeting_start_with_storage_blocked_answers_5() {
        let mut rig = Rig::new();
        rig.snap.storage_blocked = true;
        assert_eq!(rig.commands(cmd::MEETING_OP, &[0]), (vec![5], vec![]));
    }

    #[test]
    fn meeting_status_and_marks() {
        let mut rig = Rig::new();
        rig.snap.meet_status = meet::RECORDING;
        rig.snap.meet_secs = 300;
        let a = rig.ack(cmd::MEETING_GETSTATUS, &[]);
        assert_eq!((a.len(), a[0]), (0x205, 0));
        assert_eq!(cstr(&a[1..0x201]), "");
        assert_eq!(&a[0x201..], &300u32.to_le_bytes());
        assert_eq!(rig.commands(cmd::MEETING_MARK, &[0]), (vec![0], vec![Command::AddBookmark]));
        assert_eq!(rig.commands(cmd::MEETING_MARK, &[1]), (vec![0], vec![Command::AddBookmark]));
        assert_eq!(rig.commands(cmd::MEETING_MARK, &[2]), (vec![0], vec![]));
        assert_eq!(rig.send(msg(cmd::MEETING_MARK, &[0, 0])).ack, None, "stock wants exactly 1 byte");
        rig.snap.meet_status = meet::IDLE;
        assert_eq!(rig.commands(cmd::MEETING_MARK, &[0]), (vec![0], vec![]), "no recording to mark");
    }

    #[test]
    fn meeting_mode_takes_plain_recording_only() {
        let mut rig = Rig::new();
        assert_eq!(rig.ack(cmd::MEETING_MODE, &[0]), vec![0]);
        assert_eq!(rig.ack(cmd::MEETING_MODE, &[1]), vec![1]);
    }

    #[test]
    fn pc_status_is_kept() {
        let mut rig = Rig::new();
        assert_eq!(rig.ack(cmd::MEETING_PC_STATUS, &[2, 1, 1, 1, 0]), vec![0]);
        assert_eq!(
            rig.handler.pc_status(),
            Some(PcStatus { platform: 2, status: 1, software_running: true, input_is_wave: true, output_is_wave: false })
        );
        assert_eq!(rig.send(msg(cmd::MEETING_PC_STATUS, &[1; 4])).ack, None);
    }

    // --- audio ---

    #[test]
    fn speaker_volume_sets_and_follows_the_zero_means_mute_rule() {
        let mut rig = Rig::new();
        assert_eq!(rig.commands(cmd::SET_AUDIO_SPEAKER_VOLUME, &[55]), (vec![0], vec![Command::SetSpeakerVolume(55)]));
        assert_eq!(
            rig.commands(cmd::SET_AUDIO_SPEAKER_VOLUME, &[0]),
            (vec![0], vec![Command::SetSpeakerVolume(0), Command::ToggleSpeakerMute])
        );
        rig.snap.speaker_muted = true;
        assert_eq!(
            rig.commands(cmd::SET_AUDIO_SPEAKER_VOLUME, &[200]),
            (vec![0], vec![Command::SetSpeakerVolume(100), Command::ToggleSpeakerMute])
        );
    }

    #[test]
    fn mic_volume_maps_one_to_one_and_mutes_at_zero() {
        let mut rig = Rig::new();
        assert_eq!(rig.commands(cmd::SET_AUDIO_MIC_VOLUME, &[70]), (vec![0], vec![Command::SetMicVolume(70)]));
        assert_eq!(
            rig.commands(cmd::SET_AUDIO_MIC_VOLUME, &[0]),
            (vec![0], vec![Command::SetMicVolume(0), Command::ToggleMicMute])
        );
    }

    #[test]
    fn mutes_toggle_only_when_the_state_differs() {
        let mut rig = Rig::new();
        assert_eq!(rig.commands(cmd::SET_AUDIO_MIC_MUTE, &[1]), (vec![0], vec![Command::ToggleMicMute]));
        assert_eq!(rig.commands(cmd::SET_AUDIO_MIC_MUTE, &[0]), (vec![0], vec![]));
        assert_eq!(rig.commands(cmd::SET_AUDIO_SPEAKER_MUTE, &[1]), (vec![0], vec![Command::ToggleSpeakerMute]));
        rig.snap.speaker_muted = true;
        assert_eq!(rig.commands(cmd::SET_AUDIO_SPEAKER_MUTE, &[1]), (vec![0], vec![]));
        assert_eq!(rig.send(msg(cmd::SET_AUDIO_MIC_MUTE, &[])).ack, None);
    }

    #[test]
    fn mic_mode_uses_the_j4_mapping() {
        let mut rig = Rig::new();
        assert_eq!(
            rig.commands(cmd::SET_AUDIO_MIC_MODE, &[1]),
            (vec![0], vec![Command::SetAudioMode(AudioMode::Steered { initial_az_deg: 45.0 })])
        );
        assert_eq!(rig.commands(cmd::SET_AUDIO_MIC_MODE, &[5]), (vec![0], vec![Command::SetAudioMode(AudioMode::Figure8)]));
        assert_eq!(rig.commands(cmd::SET_AUDIO_MIC_MODE, &[7]), (vec![1], vec![]));
    }

    // --- language, region, time ---

    #[test]
    fn language_queries_offer_english() {
        let mut rig = Rig::new();
        let list = rig.ack(cmd::GET_SYS_LANGUAGE_LIST, &[]);
        assert_eq!((list.len(), list[0]), (0x501, 1));
        assert_eq!(cstr(&list[1..65]), LANGUAGE_NAME);
        let lang = rig.ack(cmd::GET_SYS_LANGUAGE, &[]);
        assert_eq!((cstr(&lang[..64]), lang[64]), ("English", 0));
        let country = rig.ack(cmd::GET_SYS_COUNTRY, &[]);
        assert_eq!((cstr(&country[..64]), country[64]), ("", 0));
        let city = rig.ack(cmd::GET_SYS_CITY, &[]);
        assert_eq!((city[0], cstr(&city[1..])), (0, "Europe/Brussels"));
        let def = rig.ack(cmd::GET_SYS_DEFAULT_SETTING, &[]);
        assert_eq!((cstr(&def[0..64]), cstr(&def[64..128]), cstr(&def[128..192]), def[192]), ("English", "", "Europe/Brussels", 0));
    }

    #[test]
    fn region_lists() {
        let mut rig = Rig::new();
        let countries = rig.ack(cmd::GET_SYS_LANGUAGE_COUNTRY_LIST, &[0; 64]);
        assert_eq!(countries.len(), 0x4102);
        assert!(countries.iter().all(|&b| b == 0), "no countries");
        let zones = rig.ack(cmd::GET_SYS_COUNTRY_TIMEZONE_LIST, &[0; 128]);
        assert_eq!((zones.len(), &zones[0..2]), (0x1E02, &[1, 0][..]));
        assert_eq!(cstr(&zones[2..66]), "Europe/Brussels");
        assert_eq!(rig.send(msg(cmd::GET_SYS_COUNTRY_TIMEZONE_LIST, &[0; 127])).ack, None);
        let all = rig.ack(cmd::GET_SYS_ALL_SETTING_LIST, &[]);
        assert_eq!(all.len(), 0x6406);
        assert_eq!((all[0], cstr(&all[1..65])), (1, "English"));
        assert_eq!(&all[0x501..0x503], &[0, 0]);
        assert_eq!(&all[0x4603..0x4605], &[1, 0]);
        assert_eq!(cstr(&all[0x4605..0x4645]), "Europe/Brussels");
        assert_eq!(all[0x6405], 0);
    }

    fn name64(name: &str) -> Vec<u8> {
        let mut d = vec![0u8; 64];
        d[..name.len()].copy_from_slice(name.as_bytes());
        d
    }

    #[test]
    fn set_language_accepts_english_only_and_echoes_it() {
        let mut rig = Rig::new();
        let ok = rig.ack(cmd::SET_SYS_LANGUAGE, &name64("English"));
        assert_eq!((cstr(&ok[..64]), ok[64]), ("English", 0));
        let no = rig.ack(cmd::SET_SYS_LANGUAGE, &name64("Deutsch"));
        assert_eq!((cstr(&no[..64]), no[64]), ("Deutsch", 1));
        assert_eq!(rig.send(msg(cmd::SET_SYS_LANGUAGE, &[0; 63])).ack, None);
    }

    #[test]
    fn set_timezone_takes_iana_names_and_answers_ff_otherwise() {
        let mut rig = Rig::new();
        assert_eq!(
            rig.commands(cmd::SET_SYS_TIMEZONE, &name64("America/New_York")),
            (vec![0], vec![Command::SetTimeZone("America/New_York".into())])
        );
        assert_eq!(rig.commands(cmd::SET_SYS_TIMEZONE, &name64("(UTC+08:00) Beijing")), (vec![0xFF], vec![]));
    }

    #[test]
    fn time_show_mode_and_auto_sync() {
        let mut rig = Rig::new();
        assert_eq!(rig.commands(cmd::SET_SYS_TIME_SHOW_MODE, &[1]), (vec![0], vec![Command::SetClock24h(true)]));
        assert_eq!(rig.commands(cmd::SET_SYS_TIME_SHOW_MODE, &[0]), (vec![0], vec![Command::SetClock24h(false)]));
        assert_eq!(rig.commands(cmd::SET_SYS_TIME_AUTO_SYNC, &[1]), (vec![0], vec![]));
        assert_eq!(rig.commands(cmd::SET_SYS_TIME_AUTO_SYNC, &[0]), (vec![1], vec![]), "waved always syncs");
    }

    #[test]
    fn set_time_goes_to_the_host() {
        let mut rig = Rig::new();
        let mut d = Vec::new();
        for v in [2026u32, 10, 12, 14, 30, 5] {
            d.extend(v.to_le_bytes());
        }
        assert_eq!(rig.ack(cmd::SET_SYS_TIME, &d), vec![0]);
        assert_eq!(rig.clock.0, Some(WallTime { year: 2026, month: 10, day: 12, hour: 14, minute: 30, second: 5 }));
        rig.clock.1 = false;
        assert_eq!(rig.ack(cmd::SET_SYS_TIME, &d), vec![1]);
        assert_eq!(rig.send(msg(cmd::SET_SYS_TIME, &d[..23])).ack, None);
    }

    #[test]
    fn all_date_time_sets_the_mode_and_the_clock_when_auto_sync_is_off() {
        let mut rig = Rig::new();
        let mut d = vec![0u8, 0];
        d.extend(2026u16.to_le_bytes());
        d.extend([10, 12, 8, 0, 59]);
        assert_eq!(rig.commands(cmd::SET_SYS_ALL_DATE_TIME_SETTING, &d), (vec![0], vec![Command::SetClock24h(false)]));
        assert_eq!(rig.clock.0, Some(WallTime { year: 2026, month: 10, day: 12, hour: 8, minute: 0, second: 59 }));
        rig.clock.0 = None;
        d[0] = 1;
        d[1] = 1;
        assert_eq!(rig.commands(cmd::SET_SYS_ALL_DATE_TIME_SETTING, &d), (vec![0], vec![Command::SetClock24h(true)]));
        assert_eq!(rig.clock.0, None, "auto sync: the clock is left to NTP");
    }

    // --- out of band ---

    #[test]
    fn ob_commands_answer_with_zero_seq_and_a_refused_offset() {
        let mut h = Handler::new();
        assert_eq!(h.handle_ob(cmd::OB_LAST_PKT_SEQ), Some(vec![0, 0, 0, 0]));
        assert_eq!(h.handle_ob(cmd::OB_BACKUP_OFFSET), Some(vec![1]));
        assert_eq!(h.handle_ob(0x12), None);
    }
}
