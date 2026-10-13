//! The status LEDs (docs/MCU_PROTOCOL.md, "Status LEDs"), without any I/O.
//!
//! Stock's `devstatus_server` (1.2.2.6) turns device states into events from
//! a fixed table (colour, pattern, LED count, priority, how long it lasts)
//! and shows the winning event with LED_CTRL. This module keeps stock's
//! table and scheduler, says which waved states raise which event
//! (`Leds::update`), and resends LED_CTRL like stock (3 tries).

use crate::mavlink::Message;
use crate::protocol::{self, LedCtrl, Outgoing};
use tracing::{debug, info};
use waved_state::{Command, PowerOffPhase};

/// Stock's events that waved raises (`ios_devstatus_event_en`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedEvent {
    Idle,
    Shutdown,
    BtBroadcast,
    BtPair,
    Recovery,
    UpDown,
    Ota,
    MicMute,
    MicUnmute,
    SpeakerVolume,
    MeetingWait,
    MeetingTalk,
    TmpErr,
    Battery5,
    Battery20,
    ExceptionMotor,
    ExceptionStorage,
    ExceptionOtaFail,
    AiMeeting,
    AiMeetingExit,
}

impl LedEvent {
    /// Stock's name, for the log.
    pub fn name(self) -> &'static str {
        match self {
            LedEvent::Idle => "IDLE",
            LedEvent::Shutdown => "SHUTDOWN",
            LedEvent::BtBroadcast => "BT_BROADCAST",
            LedEvent::BtPair => "BT_PAIR",
            LedEvent::Recovery => "RECOVERY",
            LedEvent::UpDown => "UPDOWN",
            LedEvent::Ota => "OTA",
            LedEvent::MicMute => "MIC_MUTE",
            LedEvent::MicUnmute => "MIC_UNMUTE",
            LedEvent::SpeakerVolume => "SPEAKER_VOLUME",
            LedEvent::MeetingWait => "MEETING_WAIT",
            LedEvent::MeetingTalk => "MEETING_TALK",
            LedEvent::TmpErr => "TMP_ERR",
            LedEvent::Battery5 => "BATTERY_5_PERCENT",
            LedEvent::Battery20 => "BATTERY_20_PERCENT",
            LedEvent::ExceptionMotor => "EXCEPTION_MOTOR",
            LedEvent::ExceptionStorage => "EXCEPTION_STORAGE",
            LedEvent::ExceptionOtaFail => "EXCEPTION_OTA_FAIL",
            LedEvent::AiMeeting => "AI_MEETING",
            LedEvent::AiMeetingExit => "AI_MEETING_EXIT",
        }
    }
}

/// Stock's colours (`ios_devstatus_led_en`).
pub mod colour {
    pub const RED: u8 = 0;
    pub const GREEN: u8 = 1;
    pub const BLUE: u8 = 2;
    pub const CYAN: u8 = 3;
    pub const YELLOW: u8 = 4;
}

/// Stock's patterns (`ios_devstatus_led_first_lvl_status_en`).
pub mod pattern {
    pub const STEADY: u8 = 0;
    pub const BLINK: u8 = 1;
    pub const PULSE: u8 = 2;
    pub const CHASE: u8 = 3;
    pub const HUE_CHANGE: u8 = 5;
    pub const FALL: u8 = 7;
}

/// The LEDs on the ring.
pub const LED_COUNT: u8 = 12;

/// Stock's priorities (`ios_devstatus_event_prior_en`): SS > P0 > P1 > P2 > IDLE.
const IDLE: u8 = 0;
const P2: u8 = 1;
const P1: u8 = 2;
const P0: u8 = 3;
const SS: u8 = 4;

/// After the power-off pattern is shown, only it and the motor exception
/// change the LEDs for this long (`devstatus_event_set`).
pub const SHUTDOWN_LOCK_MS: u64 = 10_000;

/// One row of stock's `g_ios_event_status_cfg_array`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    priority: u8,
    /// The event switches itself off after this long; 0 = until cleared.
    hold_ms: u64,
    /// `None` is stock's KEEP: the last colour sent.
    colour: Option<u8>,
    pattern: u8,
    sub: u8,
    leds: u8,
    target: u8,
}

/// Stock's row for `event` (docs/MCU_PROTOCOL.md, "The event table").
fn entry(event: LedEvent) -> Entry {
    use colour::*;
    use pattern::*;
    let row = |priority, hold_ms, colour, pattern, sub, leds, target| Entry {
        priority,
        hold_ms,
        colour,
        pattern,
        sub,
        leds,
        target,
    };
    // Sub-patterns: steady 0 = 100 %; blink and pulse 0 = 1 Hz, 1 = 3 Hz;
    // chase 0 = come-back; hue change 1 = reverse; fall 1 (stock's value).
    match event {
        LedEvent::Idle => row(IDLE, 0, Some(GREEN), STEADY, 0, LED_COUNT, 15),
        LedEvent::Shutdown => row(SS, 30_000, Some(GREEN), FALL, 1, LED_COUNT, 15),
        LedEvent::BtBroadcast => row(P1, 60_000, Some(BLUE), CHASE, 0, 1, 15),
        LedEvent::BtPair => row(IDLE, 1_000, Some(GREEN), STEADY, 0, LED_COUNT, 15),
        LedEvent::Recovery => row(P2, 10_000, Some(CYAN), PULSE, 0, LED_COUNT, 15),
        LedEvent::UpDown => row(P2, 3_000, Some(CYAN), PULSE, 0, LED_COUNT, 15),
        // Stock's LED count for OTA is 11 × 100 % / 100 + 1 = 12: it always
        // passes 100 (`ota_thread`).
        LedEvent::Ota => row(P2, 0, Some(BLUE), PULSE, 0, LED_COUNT, 15),
        LedEvent::MicMute => row(SS, 0, Some(RED), STEADY, 0, LED_COUNT, 15),
        LedEvent::MicUnmute => row(SS, 1_000, Some(GREEN), STEADY, 0, LED_COUNT, 15),
        // The LED count is the volume's (`Scheduler::speaker_volume`).
        LedEvent::SpeakerVolume => row(SS, 500, None, STEADY, 0, 0, 15),
        LedEvent::MeetingWait => row(SS, 60_000, Some(CYAN), CHASE, 0, 1, 15),
        LedEvent::MeetingTalk => row(SS, 0, Some(CYAN), STEADY, 0, LED_COUNT, 15),
        LedEvent::TmpErr => row(SS, 0, Some(YELLOW), BLINK, 0, LED_COUNT, 15),
        LedEvent::Battery5 => row(P0, 8_000, Some(RED), PULSE, 0, LED_COUNT, 15),
        LedEvent::Battery20 => row(P0, 8_000, Some(YELLOW), PULSE, 0, LED_COUNT, 15),
        LedEvent::ExceptionMotor => row(SS, 3_000, Some(YELLOW), BLINK, 1, LED_COUNT, 3),
        LedEvent::ExceptionStorage => row(SS, 3_000, Some(YELLOW), BLINK, 1, LED_COUNT, 3),
        LedEvent::ExceptionOtaFail => row(SS, 3_000, Some(YELLOW), BLINK, 1, LED_COUNT, 3),
        LedEvent::AiMeeting => row(P0, 0, Some(CYAN), STEADY, 0, LED_COUNT, 15),
        LedEvent::AiMeetingExit => row(P0, 1_000, Some(CYAN), HUE_CHANGE, 1, LED_COUNT, 1),
    }
}

/// An active event: when it was raised (the order of activations breaks
/// ties) and its LED count.
struct Active {
    event: LedEvent,
    since_ms: u64,
    order: u64,
    leds: u8,
}

/// Stock's scheduler (`devstatus_handle_schedule`, `devstatu_led_ctrl`):
/// the active event with the highest priority wins, the latest activation
/// breaks a tie, time-limited events switch themselves off, an unchanged
/// winner is not resent (except the speaker volume and the update, whose
/// LED count changes), KEEP reuses the last colour.
///
/// One deviation: during stock's 10 s power-off lock, requests other than
/// SHUTDOWN and EXCEPTION_MOTOR are recorded but not shown (stock drops
/// them), so an aborted power off comes back to the current state.
pub struct Scheduler {
    active: Vec<Active>,
    shown: Option<LedEvent>,
    shown_ms: u64,
    shown_leds: u8,
    /// Stock's `last_led_dev`; it starts at 0 (red).
    last_colour: u8,
    meeting: bool,
    frozen: bool,
    order: u64,
    /// Events requested since the last schedule.
    requests: Vec<LedEvent>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    /// IDLE is always active, as stock's timer thread keeps it.
    pub fn new() -> Self {
        Self {
            active: vec![Active { event: LedEvent::Idle, since_ms: 0, order: 0, leds: LED_COUNT }],
            shown: None,
            shown_ms: 0,
            shown_leds: 0,
            last_colour: colour::RED,
            meeting: false,
            frozen: false,
            order: 0,
            requests: Vec::new(),
        }
    }

    /// Activate or clear `event` without showing anything yet.
    pub fn activate(&mut self, event: LedEvent, on: bool, now_ms: u64) {
        self.requests.push(event);
        self.active.retain(|a| a.event != event);
        if on {
            self.order += 1;
            let leds = entry(event).leds;
            self.active.push(Active { event, since_ms: now_ms, order: self.order, leds });
        }
    }

    /// Raise the speaker volume (0..=100; 0 when muted) without showing it
    /// yet: `12 × volume / 100` LEDs (`devstatus_event_set`).
    pub fn volume(&mut self, percent: u16, now_ms: u64) {
        self.activate(LedEvent::SpeakerVolume, true, now_ms);
        let percent = u32::from(percent.min(100));
        let leds = (percent * u32::from(LED_COUNT) / 100) as u8;
        if let Some(active) = self.active.last_mut() {
            active.leds = leds;
        }
        // Unlike stock, IDLE shows the volume too (the owner's choice,
        // 2026-10-13), rounded up so any sound lights an LED.
        let idle_leds = (percent * u32::from(LED_COUNT)).div_ceil(100) as u8;
        if let Some(idle) = self.active.iter_mut().find(|a| a.event == LedEvent::Idle) {
            idle.leds = idle_leds;
        }
    }

    /// Activate or clear `event`; what to show now, if it changed.
    pub fn set(&mut self, event: LedEvent, on: bool, now_ms: u64) -> Option<(LedEvent, LedCtrl)> {
        self.activate(event, on, now_ms);
        self.schedule(now_ms)
    }

    /// Show the speaker volume (0..=100; 0 when muted) for stock's 500 ms.
    pub fn speaker_volume(&mut self, percent: u16, now_ms: u64) -> Option<(LedEvent, LedCtrl)> {
        self.volume(percent, now_ms);
        self.schedule(now_ms)
    }

    /// A meeting records or is paused (mic unmute is cyan then).
    pub fn set_meeting(&mut self, meeting: bool) {
        self.meeting = meeting;
    }

    /// Stock stops scheduling while an update installs (`ios_devstatus_switch(0)`).
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    /// Time limits; what to show now, if it changed.
    pub fn tick(&mut self, now_ms: u64) -> Option<(LedEvent, LedCtrl)> {
        self.schedule(now_ms)
    }

    /// What to show now, if it changed.
    pub fn schedule(&mut self, now_ms: u64) -> Option<(LedEvent, LedCtrl)> {
        let requests = std::mem::take(&mut self.requests);
        if self.frozen {
            return None;
        }
        self.active.retain(|a| {
            let hold = entry(a.event).hold_ms;
            hold == 0 || now_ms < a.since_ms + hold
        });
        let lock_open = requests.iter().any(|e| matches!(e, LedEvent::Shutdown | LedEvent::ExceptionMotor));
        let locked = self.shown == Some(LedEvent::Shutdown)
            && now_ms.saturating_sub(self.shown_ms) <= SHUTDOWN_LOCK_MS
            && !lock_open;
        if locked {
            return None;
        }
        let top = self.active.iter().max_by_key(|a| (entry(a.event).priority, a.order))?;
        let changing = matches!(top.event, LedEvent::SpeakerVolume | LedEvent::Ota);
        let same_leds = self.shown_leds == top.leds;
        if self.shown == Some(top.event) && same_leds && !(changing && requests.contains(&top.event)) {
            return None;
        }
        let (event, leds) = (top.event, top.leds);
        let row = entry(event);
        let colour = if self.meeting && event == LedEvent::MicUnmute {
            // `devstatu_led_ctrl`: cyan while recording, not kept as the last colour.
            colour::CYAN
        } else if let Some(c) = row.colour {
            self.last_colour = c;
            c
        } else {
            self.last_colour
        };
        self.shown = Some(event);
        self.shown_ms = now_ms;
        self.shown_leds = leds;
        Some((event, LedCtrl { pattern: row.pattern, sub: row.sub, leds, colour, target: row.target }))
    }
}

/// The waved states the LEDs follow, read each loop.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LedInputs {
    pub mic_muted: bool,
    pub speaker_volume: u16,
    pub speaker_muted: bool,
    /// A meeting records or is paused.
    pub meeting: bool,
    pub bt_pairing: bool,
    pub bt_connected: bool,
    pub call_ringing: bool,
    pub call_active: bool,
    /// An update installs (`OtaPhase::Installing` / `Restarting`).
    pub updating: bool,
    pub update_failed: bool,
    pub low_space: bool,
    /// Battery temperature at or above stock's 75 °C.
    pub too_hot: bool,
    pub lift_moving: bool,
    pub power_off: PowerOffPhase,
    pub battery_level: u8,
    pub charging: bool,
}

/// Battery temperature (°C) from which stock shows TMP_ERR.
pub const TOO_HOT_C: i8 = 75;

/// LED_CTRL resends: stock waits 100 ms per try (`mcu_timeout[1]`); the
/// service loop runs every 50 ms.
pub const ACK_MS: u64 = 200;
pub const TRIES: u8 = 3;

/// The low-battery pulses re-arm at this level (waved-mcu re-arms
/// `LowBattery` there) or on charging.
const LOW_REARM_LEVEL: u8 = 13;
const LOW_RED_LEVEL: u8 = 5;

/// The latest LED_CTRL until the MCU answers it: a newer one replaces it.
#[derive(Default)]
struct Outbox {
    in_flight: Option<(Outgoing, u64, u8)>,
    /// Whether the MCU answered LED_CTRL, once known (logged once).
    answers: Option<bool>,
}

impl Outbox {
    fn send(&mut self, out: Outgoing, now_ms: u64) -> Vec<Outgoing> {
        self.in_flight = Some((out.clone(), now_ms, 1));
        vec![out]
    }

    fn on_ack(&mut self, params: &[u8]) {
        if self.in_flight.take().is_some() && self.answers.is_none() {
            info!("mcu: led: the MCU answers LED_CTRL (params {params:02x?})");
            self.answers = Some(true);
        }
    }

    fn tick(&mut self, now_ms: u64) -> Vec<Outgoing> {
        let Some((out, sent_ms, tries)) = self.in_flight.as_mut() else {
            return Vec::new();
        };
        if now_ms < *sent_ms + ACK_MS {
            return Vec::new();
        }
        if *tries < TRIES {
            *tries += 1;
            *sent_ms = now_ms;
            return vec![out.clone()];
        }
        self.in_flight = None;
        if self.answers.is_none() {
            info!("mcu: led: no reply to LED_CTRL after {TRIES} tries (not logged again)");
            self.answers = Some(false);
        } else {
            debug!("mcu: led: no reply to LED_CTRL after {TRIES} tries");
        }
        Vec::new()
    }
}

/// The LEDs: follows `LedInputs` and the commands, schedules like stock,
/// sends LED_CTRL.
pub struct Leds {
    scheduler: Scheduler,
    last: Option<LedInputs>,
    /// The power-off pattern is up.
    shutting: bool,
    /// Low-battery pulses given: 0 none, 1 yellow, 2 red.
    battery_band: u8,
    outbox: Outbox,
}

impl Default for Leds {
    fn default() -> Self {
        Self::new()
    }
}

impl Leds {
    pub fn new() -> Self {
        Self { scheduler: Scheduler::new(), last: None, shutting: false, battery_band: 0, outbox: Outbox::default() }
    }

    /// The states changed (or not): what to send.
    pub fn update(&mut self, inputs: &LedInputs, now_ms: u64) -> Vec<Outgoing> {
        if self.last.as_ref() == Some(inputs) {
            return Vec::new();
        }
        let last = self.last.replace(inputs.clone());
        let update_starts = inputs.updating && !last.as_ref().is_some_and(|l| l.updating);
        self.follow(last.as_ref(), inputs, now_ms);
        let shown = self.scheduler.schedule(now_ms);
        if update_starts {
            // Stock shows OTA, then stops scheduling until the update ends.
            self.scheduler.set_frozen(true);
        }
        self.emit(shown, now_ms)
    }

    /// Which events a change raises (stock's `devstatus_server_event_handle_cb`
    /// and the UI's calls). The first snapshot sets the lasting states only.
    fn follow(&mut self, last: Option<&LedInputs>, now: &LedInputs, t: u64) {
        let s = &mut self.scheduler;
        if now.charging || now.battery_level >= LOW_REARM_LEVEL {
            self.battery_band = 0;
        }
        let busy = |p: PowerOffPhase| matches!(p, PowerOffPhase::InProgress | PowerOffPhase::Sent);
        if busy(now.power_off) && !last.is_some_and(|l| busy(l.power_off)) && !self.shutting {
            self.shutting = true;
            s.activate(LedEvent::Shutdown, true, t);
        }
        let Some(last) = last else {
            s.set_meeting(now.meeting);
            let lasting = [
                (now.mic_muted, LedEvent::MicMute),
                (now.meeting, LedEvent::AiMeeting),
                (now.bt_pairing, LedEvent::BtBroadcast),
                (now.call_ringing && !now.call_active, LedEvent::MeetingWait),
                (now.call_active, LedEvent::MeetingTalk),
                (now.updating, LedEvent::Ota),
                (now.too_hot, LedEvent::TmpErr),
                (now.lift_moving, LedEvent::UpDown),
            ];
            for (on, event) in lasting {
                if on {
                    s.activate(event, true, t);
                }
            }
            return;
        };
        if now.power_off == PowerOffPhase::Aborted && last.power_off != PowerOffPhase::Aborted && self.shutting {
            // The lift was obstructed during the retract: the unit stays on.
            self.shutting = false;
            s.activate(LedEvent::Shutdown, false, t);
            s.activate(LedEvent::UpDown, false, t);
            s.activate(LedEvent::ExceptionMotor, true, t);
        }
        if now.mic_muted != last.mic_muted {
            let (on, off) = if now.mic_muted {
                (LedEvent::MicMute, LedEvent::MicUnmute)
            } else {
                (LedEvent::MicUnmute, LedEvent::MicMute)
            };
            s.activate(on, true, t);
            s.activate(off, false, t);
        }
        if now.speaker_volume != last.speaker_volume || now.speaker_muted != last.speaker_muted {
            s.volume(if now.speaker_muted { 0 } else { now.speaker_volume }, t);
        }
        if now.meeting != last.meeting {
            s.set_meeting(now.meeting);
            if now.meeting {
                s.activate(LedEvent::AiMeetingExit, false, t);
                s.activate(LedEvent::AiMeeting, true, t);
            } else {
                s.activate(LedEvent::AiMeeting, false, t);
                s.activate(LedEvent::AiMeetingExit, true, t);
            }
        }
        if now.bt_pairing != last.bt_pairing {
            s.activate(LedEvent::BtBroadcast, now.bt_pairing, t);
        }
        if now.bt_connected && !last.bt_connected {
            s.activate(LedEvent::BtBroadcast, false, t);
            s.activate(LedEvent::BtPair, true, t);
        }
        if now.call_active && !last.call_active {
            s.activate(LedEvent::MeetingTalk, true, t);
            s.activate(LedEvent::MeetingWait, false, t);
        } else if now.call_ringing && !now.call_active && !last.call_ringing {
            s.activate(LedEvent::MeetingWait, true, t);
        } else if !now.call_ringing && !now.call_active && (last.call_ringing || last.call_active) {
            s.activate(LedEvent::MeetingTalk, false, t);
            s.activate(LedEvent::MeetingWait, false, t);
        }
        if now.updating != last.updating {
            if now.updating {
                s.activate(LedEvent::ExceptionOtaFail, false, t);
                s.activate(LedEvent::Ota, true, t);
            } else {
                s.set_frozen(false);
                s.activate(LedEvent::Ota, false, t);
                if now.update_failed {
                    s.activate(LedEvent::ExceptionOtaFail, true, t);
                }
            }
        }
        if now.low_space && !last.low_space {
            s.activate(LedEvent::ExceptionStorage, true, t);
        }
        if now.too_hot != last.too_hot {
            s.activate(LedEvent::TmpErr, now.too_hot, t);
        }
        if now.lift_moving != last.lift_moving {
            s.activate(LedEvent::UpDown, now.lift_moving, t);
        }
    }

    /// Power off and factory reset show before the services stop.
    pub fn on_command(&mut self, command: &Command, now_ms: u64) -> Vec<Outgoing> {
        match command {
            Command::PowerOff | Command::PowerOffNow if !self.shutting => {
                self.shutting = true;
                self.scheduler.activate(LedEvent::Shutdown, true, now_ms);
            }
            Command::FactoryReset => self.scheduler.activate(LedEvent::Recovery, true, now_ms),
            _ => return Vec::new(),
        }
        let shown = self.scheduler.schedule(now_ms);
        self.emit(shown, now_ms)
    }

    /// waved-mcu's `LowBattery`: stock pulses yellow at its 20 % and 10 %
    /// tips and red at 5 %, once per band.
    pub fn on_low_battery(&mut self, level: u8, now_ms: u64) -> Vec<Outgoing> {
        let band = if level <= LOW_RED_LEVEL { 2 } else { 1 };
        if band <= self.battery_band {
            return Vec::new();
        }
        self.battery_band = band;
        let (on, off) = if band == 2 {
            (LedEvent::Battery5, LedEvent::Battery20)
        } else {
            (LedEvent::Battery20, LedEvent::Battery5)
        };
        self.scheduler.activate(on, true, now_ms);
        self.scheduler.activate(off, false, now_ms);
        let shown = self.scheduler.schedule(now_ms);
        self.emit(shown, now_ms)
    }

    /// A message from the MCU: a cmd 1 `mcu_notify` answers LED_CTRL.
    pub fn on_message(&mut self, m: &Message) {
        if protocol::notify_cmd(m) == Some(protocol::cmd::LED_CTRL) {
            self.outbox.on_ack(&m.payload[4..]);
        }
    }

    /// Time limits and resends.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Outgoing> {
        let shown = self.scheduler.tick(now_ms);
        let sent = self.emit(shown, now_ms);
        if !sent.is_empty() {
            return sent;
        }
        self.outbox.tick(now_ms)
    }

    fn emit(&mut self, shown: Option<(LedEvent, LedCtrl)>, now_ms: u64) -> Vec<Outgoing> {
        let Some((event, led)) = shown else {
            return Vec::new();
        };
        info!(
            "mcu: led {} (pattern {} sub {} leds {} colour {} target {})",
            event.name(),
            led.pattern,
            led.sub,
            led.leds,
            led.colour,
            led.target
        );
        self.outbox.send(protocol::led_ctrl(led), now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::{encode, Parser};
    use crate::protocol::{cmd, NOTIFY};

    fn shown(out: Option<(LedEvent, LedCtrl)>) -> Option<LedEvent> {
        out.map(|(event, _)| event)
    }

    fn ctrl(pattern: u8, sub: u8, leds: u8, colour: u8, target: u8) -> LedCtrl {
        LedCtrl { pattern, sub, leds, colour, target }
    }

    fn started() -> Scheduler {
        let mut s = Scheduler::new();
        assert_eq!(shown(s.tick(0)), Some(LedEvent::Idle));
        s
    }

    #[test]
    fn stocks_table_is_copied_byte_for_byte() {
        // g_ios_event_status_cfg_array (1.2.2.6 .data 0x288658 = 3.0.0.1 0x285658).
        use colour::*;
        use pattern::*;
        let rows = [
            (LedEvent::Idle, IDLE, 0, Some(GREEN), STEADY, 0, 12, 15),
            (LedEvent::Shutdown, SS, 30_000, Some(GREEN), FALL, 1, 12, 15),
            (LedEvent::BtBroadcast, P1, 60_000, Some(BLUE), CHASE, 0, 1, 15),
            (LedEvent::BtPair, IDLE, 1_000, Some(GREEN), STEADY, 0, 12, 15),
            (LedEvent::Recovery, P2, 10_000, Some(CYAN), PULSE, 0, 12, 15),
            (LedEvent::UpDown, P2, 3_000, Some(CYAN), PULSE, 0, 12, 15),
            (LedEvent::Ota, P2, 0, Some(BLUE), PULSE, 0, 12, 15),
            (LedEvent::MicMute, SS, 0, Some(RED), STEADY, 0, 12, 15),
            (LedEvent::MicUnmute, SS, 1_000, Some(GREEN), STEADY, 0, 12, 15),
            (LedEvent::SpeakerVolume, SS, 500, None, STEADY, 0, 0, 15),
            (LedEvent::MeetingWait, SS, 60_000, Some(CYAN), CHASE, 0, 1, 15),
            (LedEvent::MeetingTalk, SS, 0, Some(CYAN), STEADY, 0, 12, 15),
            (LedEvent::TmpErr, SS, 0, Some(YELLOW), BLINK, 0, 12, 15),
            (LedEvent::Battery20, P0, 8_000, Some(YELLOW), PULSE, 0, 12, 15),
            (LedEvent::Battery5, P0, 8_000, Some(RED), PULSE, 0, 12, 15),
            (LedEvent::ExceptionMotor, SS, 3_000, Some(YELLOW), BLINK, 1, 12, 3),
            (LedEvent::ExceptionStorage, SS, 3_000, Some(YELLOW), BLINK, 1, 12, 3),
            (LedEvent::ExceptionOtaFail, SS, 3_000, Some(YELLOW), BLINK, 1, 12, 3),
            (LedEvent::AiMeeting, P0, 0, Some(CYAN), STEADY, 0, 12, 15),
            (LedEvent::AiMeetingExit, P0, 1_000, Some(CYAN), HUE_CHANGE, 1, 12, 1),
        ];
        for (event, priority, hold_ms, colour, pattern, sub, leds, target) in rows {
            assert_eq!(entry(event), Entry { priority, hold_ms, colour, pattern, sub, leds, target }, "{event:?}");
        }
        assert_eq!(LedEvent::MicMute.name(), "MIC_MUTE");
        assert_eq!(LedEvent::Battery20.name(), "BATTERY_20_PERCENT");
    }

    #[test]
    fn idle_is_steady_green_on_all_twelve() {
        let mut s = Scheduler::new();
        assert_eq!(s.tick(0), Some((LedEvent::Idle, ctrl(0, 0, 12, colour::GREEN, 15))));
        assert_eq!(s.tick(50), None, "shown once");
    }

    #[test]
    fn idle_shows_the_volume_level() {
        // The owner's choice (2026-10-13): the green ring always shows the
        // volume, never a full bar. Rounded up, so any sound lights an LED.
        let mut s = started();
        s.speaker_volume(50, 100);
        assert_eq!(s.tick(700), Some((LedEvent::Idle, ctrl(0, 0, 6, colour::GREEN, 15))));
        s.speaker_volume(5, 1_000);
        assert_eq!(s.tick(1_600), Some((LedEvent::Idle, ctrl(0, 0, 1, colour::GREEN, 15))));
        s.speaker_volume(0, 2_000);
        assert_eq!(s.tick(2_600), Some((LedEvent::Idle, ctrl(0, 0, 0, colour::GREEN, 15))));
        assert_eq!(s.tick(2_700), None, "shown once");
    }

    #[test]
    fn a_higher_priority_wins_over_a_later_lower_one() {
        let mut s = started();
        assert_eq!(shown(s.set(LedEvent::MicMute, true, 100)), Some(LedEvent::MicMute));
        assert_eq!(s.set(LedEvent::BtBroadcast, true, 200), None, "P1 under SS");
        assert_eq!(shown(s.set(LedEvent::MicMute, false, 300)), Some(LedEvent::BtBroadcast));
    }

    #[test]
    fn the_latest_activation_breaks_a_tie() {
        let mut s = started();
        s.set(LedEvent::MicMute, true, 100);
        assert_eq!(shown(s.set(LedEvent::MeetingTalk, true, 200)), Some(LedEvent::MeetingTalk));
        assert_eq!(shown(s.set(LedEvent::MeetingTalk, false, 300)), Some(LedEvent::MicMute));
        // Same millisecond: the later call wins.
        assert_eq!(shown(s.set(LedEvent::TmpErr, true, 300)), Some(LedEvent::TmpErr));
    }

    #[test]
    fn time_limited_events_switch_themselves_off() {
        let mut s = started();
        let blink = s.set(LedEvent::ExceptionStorage, true, 1_000);
        assert_eq!(blink, Some((LedEvent::ExceptionStorage, ctrl(1, 1, 12, colour::YELLOW, 3))));
        assert_eq!(s.tick(3_999), None);
        assert_eq!(shown(s.tick(4_000)), Some(LedEvent::Idle));
    }

    #[test]
    fn an_unchanged_winner_is_not_resent() {
        let mut s = started();
        assert!(s.set(LedEvent::MicMute, true, 0).is_some());
        assert_eq!(s.set(LedEvent::MicMute, true, 10), None);
        assert_eq!(s.set(LedEvent::BtBroadcast, false, 20), None, "clearing an inactive event");
    }

    #[test]
    fn the_speaker_volume_shows_its_level_in_the_last_colour() {
        let mut s = started();
        // 12 × v / 100 LEDs (`devstatus_event_set`), resent on every change.
        assert_eq!(s.speaker_volume(50, 0), Some((LedEvent::SpeakerVolume, ctrl(0, 0, 6, colour::GREEN, 15))));
        assert_eq!(s.speaker_volume(100, 100), Some((LedEvent::SpeakerVolume, ctrl(0, 0, 12, colour::GREEN, 15))));
        assert_eq!(s.speaker_volume(5, 200).unwrap().1.leds, 0);
        assert_eq!(s.tick(699), None);
        assert_eq!(shown(s.tick(700)), Some(LedEvent::Idle), "500 ms after the last change");
        // While the mic is muted the level shows in red.
        s.set(LedEvent::MicMute, true, 1_000);
        assert_eq!(s.speaker_volume(25, 1_100), Some((LedEvent::SpeakerVolume, ctrl(0, 0, 3, colour::RED, 15))));
        assert_eq!(shown(s.tick(1_600)), Some(LedEvent::MicMute));
    }

    #[test]
    fn mic_unmute_while_a_meeting_records_is_cyan() {
        let mut s = started();
        s.set_meeting(true);
        assert_eq!(s.set(LedEvent::AiMeeting, true, 0).unwrap().1.colour, colour::CYAN);
        assert_eq!(s.set(LedEvent::MicMute, true, 100).unwrap().1.colour, colour::RED);
        // Stock activates the unmute, then clears the mute.
        let (event, led) = s.set(LedEvent::MicUnmute, true, 200).expect("unmute shown");
        assert_eq!((event, led), (LedEvent::MicUnmute, ctrl(0, 0, 12, colour::CYAN, 15)));
        assert_eq!(s.set(LedEvent::MicMute, false, 200), None, "still the unmute");
        // The special colour does not become the last colour (`devstatu_led_ctrl`).
        assert_eq!(s.speaker_volume(50, 300).unwrap().1.colour, colour::RED);
        s.set_meeting(false);
        s.set(LedEvent::MicMute, true, 2_000);
        assert_eq!(s.set(LedEvent::MicUnmute, true, 2_100).unwrap().1.colour, colour::GREEN);
    }

    #[test]
    fn only_the_motor_exception_interrupts_the_power_off_pattern_for_10_s() {
        let mut s = started();
        assert_eq!(s.set(LedEvent::Shutdown, true, 0), Some((LedEvent::Shutdown, ctrl(7, 1, 12, colour::GREEN, 15))));
        assert_eq!(s.set(LedEvent::MicMute, true, 1_000), None, "locked");
        assert_eq!(s.tick(2_000), None);
        assert_eq!(shown(s.set(LedEvent::ExceptionMotor, true, 3_000)), Some(LedEvent::ExceptionMotor));
        // The power off was aborted: once the exception ends, the current state shows.
        assert_eq!(s.set(LedEvent::Shutdown, false, 3_500), None, "the exception still wins");
        assert_eq!(shown(s.tick(6_000)), Some(LedEvent::MicMute), "recorded during the lock");
    }

    #[test]
    fn the_lock_ends_after_10_s() {
        let mut s = started();
        s.set(LedEvent::Shutdown, true, 0);
        s.set(LedEvent::MicMute, true, 1_000);
        assert_eq!(s.tick(SHUTDOWN_LOCK_MS), None);
        assert_eq!(shown(s.tick(SHUTDOWN_LOCK_MS + 1)), Some(LedEvent::MicMute), "SS, activated later");
    }

    #[test]
    fn nothing_changes_while_an_update_installs() {
        let mut s = started();
        assert_eq!(shown(s.set(LedEvent::Ota, true, 0)), Some(LedEvent::Ota));
        s.set_frozen(true);
        assert_eq!(s.set(LedEvent::MicMute, true, 100), None);
        assert_eq!(s.tick(200), None);
        s.set_frozen(false);
        assert_eq!(shown(s.tick(300)), Some(LedEvent::MicMute));
    }

    // --- Leds: states to events, resends ---

    fn led_of(out: &Outgoing) -> LedCtrl {
        assert_eq!(&out.payload[..4], &cmd::LED_CTRL.to_le_bytes());
        let p = &out.payload[4..];
        ctrl(p[0], p[1], p[2], p[3], p[4])
    }

    fn leds_of(outs: &[Outgoing]) -> Vec<LedCtrl> {
        outs.iter().map(led_of).collect()
    }

    fn ack() -> Message {
        let mut parser = Parser::default();
        let payload = cmd::LED_CTRL.to_le_bytes();
        encode(0, NOTIFY, &payload).into_iter().find_map(|b| parser.feed(b)).unwrap()
    }

    const IDLE_LED: LedCtrl = LedCtrl { pattern: 0, sub: 0, leds: 12, colour: colour::GREEN, target: 15 };
    const RED_LED: LedCtrl = LedCtrl { pattern: 0, sub: 0, leds: 12, colour: colour::RED, target: 15 };

    /// `Leds` after the first snapshot `base`, its frame answered.
    fn following(base: &LedInputs) -> (Leds, Vec<LedCtrl>) {
        let mut leds = Leds::new();
        let first = leds_of(&leds.update(base, 0));
        leds.on_message(&ack());
        (leds, first)
    }

    /// The LED states sent after `change` at `now_ms` (each answered).
    fn after(leds: &mut Leds, inputs: &LedInputs, now_ms: u64) -> Vec<LedCtrl> {
        let out = leds_of(&leds.update(inputs, now_ms));
        leds.on_message(&ack());
        out
    }

    fn tick(leds: &mut Leds, now_ms: u64) -> Vec<LedCtrl> {
        let out = leds_of(&leds.tick(now_ms));
        leds.on_message(&ack());
        out
    }

    #[test]
    fn the_first_snapshot_shows_the_state_without_pulses() {
        let base = LedInputs { mic_muted: true, speaker_volume: 40, ..LedInputs::default() };
        let (_, first) = following(&base);
        assert_eq!(first, [RED_LED], "mute, no volume flash, no unmute");
        let (_, idle) = following(&LedInputs { speaker_volume: 40, ..LedInputs::default() });
        assert_eq!(idle, [IDLE_LED]);
    }

    #[test]
    fn mute_is_red_and_unmute_green_for_a_second() {
        let mut inputs = LedInputs { speaker_volume: 40, ..LedInputs::default() };
        let (mut leds, _) = following(&inputs);
        assert!(after(&mut leds, &inputs, 50).is_empty(), "nothing changed");
        inputs.mic_muted = true;
        assert_eq!(after(&mut leds, &inputs, 100), [RED_LED]);
        inputs.mic_muted = false;
        assert_eq!(after(&mut leds, &inputs, 200), [IDLE_LED], "MIC_UNMUTE: steady green");
        assert!(tick(&mut leds, 1_199).is_empty());
        assert_eq!(tick(&mut leds, 1_200), [IDLE_LED], "back to IDLE");
    }

    #[test]
    fn a_volume_change_flashes_the_level() {
        let mut inputs = LedInputs { speaker_volume: 40, ..LedInputs::default() };
        let (mut leds, _) = following(&inputs);
        inputs.speaker_volume = 50;
        assert_eq!(after(&mut leds, &inputs, 100), [ctrl(0, 0, 6, colour::GREEN, 15)]);
        inputs.speaker_muted = true;
        assert_eq!(after(&mut leds, &inputs, 200), [ctrl(0, 0, 0, colour::GREEN, 15)], "muted: none lit");
    }

    #[test]
    fn a_recording_is_cyan_and_its_end_changes_hue_for_a_second() {
        let mut inputs = LedInputs::default();
        let (mut leds, _) = following(&inputs);
        inputs.meeting = true;
        assert_eq!(after(&mut leds, &inputs, 100), [ctrl(0, 0, 12, colour::CYAN, 15)]);
        inputs.mic_muted = true;
        assert_eq!(after(&mut leds, &inputs, 200), [RED_LED]);
        inputs.mic_muted = false;
        assert_eq!(after(&mut leds, &inputs, 300), [ctrl(0, 0, 12, colour::CYAN, 15)], "unmute in cyan");
        assert_eq!(tick(&mut leds, 1_300), [ctrl(0, 0, 12, colour::CYAN, 15)], "AI_MEETING again");
        inputs.meeting = false;
        assert_eq!(after(&mut leds, &inputs, 2_000), [ctrl(5, 1, 12, colour::CYAN, 1)]);
        assert_eq!(tick(&mut leds, 3_000), [IDLE_LED]);
    }

    #[test]
    fn bluetooth_pairing_connecting_and_calls() {
        let mut inputs = LedInputs::default();
        let (mut leds, _) = following(&inputs);
        inputs.bt_pairing = true;
        assert_eq!(after(&mut leds, &inputs, 100), [ctrl(3, 0, 1, colour::BLUE, 15)], "blue chase");
        inputs.bt_pairing = false;
        inputs.bt_connected = true;
        assert_eq!(after(&mut leds, &inputs, 200), [IDLE_LED], "BT_PAIR: green for a second");
        inputs.call_ringing = true;
        assert_eq!(after(&mut leds, &inputs, 300), [ctrl(3, 0, 1, colour::CYAN, 15)], "cyan chase");
        inputs.call_ringing = false;
        inputs.call_active = true;
        assert_eq!(after(&mut leds, &inputs, 400), [ctrl(0, 0, 12, colour::CYAN, 15)]);
        inputs.call_active = false;
        assert_eq!(after(&mut leds, &inputs, 500), [IDLE_LED]);
    }

    #[test]
    fn power_off_falls_green_and_an_obstruction_blinks_yellow() {
        let mut inputs = LedInputs::default();
        let (mut leds, _) = following(&inputs);
        let fall = ctrl(7, 1, 12, colour::GREEN, 15);
        assert_eq!(leds_of(&leds.on_command(&Command::PowerOff, 100)), [fall]);
        leds.on_message(&ack());
        inputs.power_off = PowerOffPhase::InProgress;
        assert!(after(&mut leds, &inputs, 200).is_empty(), "already shown");
        inputs.power_off = PowerOffPhase::Aborted;
        assert_eq!(after(&mut leds, &inputs, 300), [ctrl(1, 1, 12, colour::YELLOW, 3)]);
        assert_eq!(tick(&mut leds, 3_300), [IDLE_LED]);
        // A power off that starts without the command (PowerOffNow) shows too.
        let (mut direct, _) = following(&LedInputs::default());
        let busy = LedInputs { power_off: PowerOffPhase::InProgress, ..LedInputs::default() };
        assert_eq!(after(&mut direct, &busy, 100), [fall]);
    }

    #[test]
    fn low_battery_pulses_once_per_band() {
        let mut inputs = LedInputs { battery_level: 20, ..LedInputs::default() };
        let (mut leds, _) = following(&inputs);
        let yellow = ctrl(2, 0, 12, colour::YELLOW, 15);
        assert_eq!(leds_of(&leds.on_low_battery(9, 100)), [yellow]);
        leds.on_message(&ack());
        assert!(leds.on_low_battery(8, 200).is_empty(), "same band");
        assert_eq!(leds_of(&leds.on_low_battery(5, 300)), [ctrl(2, 0, 12, colour::RED, 15)]);
        leds.on_message(&ack());
        assert_eq!(tick(&mut leds, 8_300), [IDLE_LED], "8 s each");
        // Charging re-arms both.
        inputs.charging = true;
        inputs.battery_level = 6;
        after(&mut leds, &inputs, 9_000);
        assert_eq!(leds_of(&leds.on_low_battery(9, 9_100)), [yellow]);
    }

    #[test]
    fn an_update_freezes_the_ring_until_it_ends() {
        let mut inputs = LedInputs::default();
        let (mut leds, _) = following(&inputs);
        inputs.updating = true;
        assert_eq!(after(&mut leds, &inputs, 100), [ctrl(2, 0, 12, colour::BLUE, 15)]);
        inputs.mic_muted = true;
        assert!(after(&mut leds, &inputs, 200).is_empty(), "frozen");
        inputs.updating = false;
        inputs.update_failed = true;
        assert_eq!(after(&mut leds, &inputs, 300), [ctrl(1, 1, 12, colour::YELLOW, 3)], "OTA failed");
        assert_eq!(tick(&mut leds, 3_300), [RED_LED], "the mute from during the update");
    }

    #[test]
    fn the_other_states_raise_stocks_events() {
        type Change = fn(&mut LedInputs);
        let cases: [(Change, LedCtrl); 3] = [
            (|i| i.lift_moving = true, ctrl(2, 0, 12, colour::CYAN, 15)),
            (|i| i.low_space = true, ctrl(1, 1, 12, colour::YELLOW, 3)),
            (|i| i.too_hot = true, ctrl(1, 0, 12, colour::YELLOW, 15)),
        ];
        for (change, expected) in cases {
            let mut inputs = LedInputs::default();
            let (mut leds, _) = following(&inputs);
            change(&mut inputs);
            assert_eq!(after(&mut leds, &inputs, 100), [expected]);
        }
        let (mut leds, _) = following(&LedInputs::default());
        assert_eq!(leds_of(&leds.on_command(&Command::FactoryReset, 100)), [ctrl(2, 0, 12, colour::CYAN, 15)]);
        assert!(leds.on_command(&Command::Shutdown, 200).is_empty(), "every exit sends Shutdown");
    }

    #[test]
    fn led_ctrl_is_resent_until_the_mcu_answers() {
        let mut leds = Leds::new();
        assert_eq!(leds_of(&leds.update(&LedInputs::default(), 0)), [IDLE_LED]);
        assert!(leds.tick(ACK_MS - 1).is_empty());
        assert_eq!(leds_of(&leds.tick(ACK_MS)), [IDLE_LED], "second try");
        leds.on_message(&ack());
        assert!(leds.tick(3 * ACK_MS).is_empty(), "answered");
    }

    #[test]
    fn three_tries_then_it_gives_up() {
        let mut leds = Leds::new();
        leds.update(&LedInputs::default(), 0);
        assert_eq!(leds.tick(ACK_MS).len(), 1);
        assert_eq!(leds.tick(2 * ACK_MS).len(), 1);
        assert!(leds.tick(3 * ACK_MS).is_empty(), "{TRIES} tries");
        assert!(leds.tick(10 * ACK_MS).is_empty());
    }

    #[test]
    fn a_newer_state_replaces_one_still_waiting() {
        let mut leds = Leds::new();
        let mut inputs = LedInputs::default();
        leds.update(&inputs, 0);
        inputs.mic_muted = true;
        assert_eq!(leds_of(&leds.update(&inputs, 100)), [RED_LED], "sent at once");
        assert_eq!(leds_of(&leds.tick(100 + ACK_MS)), [RED_LED], "only the newest is resent");
    }
}
