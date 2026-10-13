//! The phone call (I3): when it rings, when it is up and every sign that it
//! is over. docs/BLUETOOTH_PROTOCOL.md, "Calls".
//!
//! **Stock (1.2.2.6)** leaves its call pages only on `0xD1 01` (acted on
//! while a number is stored, libins_entry.so.c 77006 → libui
//! `process_ntf_bt_callpickup_event` 85228), on the phone dropping (libui
//! `process_ntf_bt_connect_state` 85054 sends itself a hang-up while a
//! Bluetooth call shows) and on the user's own buttons: Decline / Hang up
//! leave at once (libui `hangup_btn_click_event_cb` 27920 and 28702) and
//! Answer opens the calling page at once (`pickup_btn_click_event_cb` 27958),
//! without waiting for the module. It has no ring timeout, and an empty
//! `0xB1` does not close its incoming-call page (`bt_call_in_event_cb`
//! 102913 ignores it). `0xB2` and `0xA6` never touch its call state: they
//! only route audio (`audio_listen_bt_work_status_event` 73172; `0xA6` is
//! the module's audio mode, 2 = phone, 3 = dongle, which stock asks for and
//! waits for when it opens a path, `audio_server_select_in_path_policy`
//! 70553, at 70907).
//!
//! **On the device** (2026-10-13, module 0.0.0.28) a call answered on the
//! phone sent neither `0xD1` nor an empty `0xB1`, and the Wave kept offering
//! Answer and Decline after the call. So waved also takes:
//!
//! - the module's own log lines (`0xF0`) about the call, fixed messages in
//!   its firmware (0.0.0.28 and 0.0.0.33 alike): `HFP  Call active` (up),
//!   `HFP call end` and `HFP RFCOMM_CLOSED` (over);
//! - the call audio (`0xB2` bit 2, the SCO link): once it has been open
//!   during this call and stays closed for [`CALL_AUDIO_GRACE_MS`], the Wave
//!   is no longer in the call;
//! - a ring with no news for [`RING_TIMEOUT_MS`] since the last `0xB1`: the
//!   call is up if its audio is open then, else it is over.
//!
//! Answer, Decline and Hang up on the Wave act at once, as stock's buttons
//! do. A declined number that the module repeats right after does not ring
//! again ([`DECLINED_QUIET_MS`]).

use crate::event::Event;
use waved_state::{BluetoothState, BtCall};

/// A ring with no `0xB1` for this long is over (stock has no timeout).
/// Networks hand an unanswered call to voicemail after 20 to 30 s, and the
/// module may repeat the number while it rings, so 60 s since the last
/// `0xB1` only fires for a ring that ended without a word.
pub const RING_TIMEOUT_MS: u64 = 60_000;

/// The call audio (`0xB2` bit 2) closed for this long ends the call. Long
/// enough for the SCO link to reopen when the phone changes codec.
pub const CALL_AUDIO_GRACE_MS: u64 = 3_000;

/// After Decline or Hang up on the Wave, the same number does not ring again
/// until the module has been quiet about it for this long.
pub const DECLINED_QUIET_MS: u64 = 10_000;

/// What a module log line says about a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallHint {
    /// `HFP  Call active`: the phone reports a call up (answered anywhere).
    Up,
    /// `HFP call end`, `HFP RFCOMM_CLOSED`: no call, or no call link at all.
    Over,
}

/// The call news in a module log line (`0xF0` text), if any. Matches the
/// start of the line, as the module prints it (`@`, `#` and spaces before
/// the message are its own markers).
pub fn call_hint(line: &str) -> Option<CallHint> {
    let line = line.trim_start_matches(|c: char| c == '@' || c == '#' || c.is_whitespace());
    if line.starts_with("HFP  Call active") {
        Some(CallHint::Up)
    } else if line.starts_with("HFP call end") || line.starts_with("HFP RFCOMM_CLOSED") {
        Some(CallHint::Over)
    } else {
        None
    }
}

/// What a call event did to `BluetoothState::call`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CallChange {
    pub changed: bool,
    /// The info line for a change no frame names by itself (a timeout, the
    /// call audio, a module log line); `None` when the frame's own line says it.
    pub why: Option<&'static str>,
}

impl CallChange {
    fn because(why: &'static str) -> Self {
        Self { changed: true, why: Some(why) }
    }
}

#[derive(Debug)]
struct Declined {
    number: String,
    quiet_until_ms: u64,
}

/// The call's clock and evidence. `BluetoothState::call` holds the state;
/// `Tracker` clears it when the phone drops.
#[derive(Debug, Default)]
pub struct Calls {
    /// Bluetooth is switched off: no call can ring.
    off: bool,
    /// While ringing: when the ring is over without news.
    ring_until_ms: Option<u64>,
    /// The call audio was open during this call.
    audio_seen: bool,
    /// The call audio closed at this time (after it was open).
    audio_gone_at_ms: Option<u64>,
    declined: Option<Declined>,
}

impl Calls {
    /// The Bluetooth switch: while off nothing rings.
    pub fn set_enabled(&mut self, on: bool) {
        self.off = !on;
        if self.off {
            self.reset();
            self.declined = None;
        }
    }

    /// A module event, after `Tracker` folded it into `s` (so `s.call_audio`
    /// and a dropped phone are already there).
    pub fn event(&mut self, event: &Event, now_ms: u64, s: &mut BluetoothState) -> CallChange {
        if self.off {
            return CallChange::default();
        }
        let out = match event {
            Event::CallNumber(number) => self.number(number, now_ms, s),
            Event::CallCleared | Event::Phone(crate::event::PhoneEvent::CallEnded) => {
                CallChange { changed: self.end(s), why: None }
            }
            Event::Phone(crate::event::PhoneEvent::CallActive) => {
                CallChange { changed: self.up(s), why: None }
            }
            Event::WorkStatus { .. } => {
                self.follow_audio(now_ms, s);
                CallChange::default()
            }
            Event::Log(line) => match call_hint(line) {
                Some(CallHint::Up) if self.up(s) => CallChange::because("bt: the module reports the call up"),
                Some(CallHint::Over) if self.end(s) => CallChange::because("bt: the module reports the call over"),
                _ => CallChange::default(),
            },
            _ => CallChange::default(),
        };
        if s.call == BtCall::Idle {
            self.reset();
        }
        out
    }

    /// The clock: a ring that went quiet, call audio that stayed closed.
    pub fn tick(&mut self, now_ms: u64, s: &mut BluetoothState) -> CallChange {
        if self
            .declined
            .as_ref()
            .is_some_and(|d| now_ms >= d.quiet_until_ms)
        {
            self.declined = None;
        }
        if s.call == BtCall::Idle {
            self.reset();
            return CallChange::default();
        }
        if s.call.is_ringing() && self.ring_until_ms.is_some_and(|until| now_ms >= until) {
            // Rang a minute without a word: if the call audio is open the
            // call was answered (on the phone, audio here), else it is over.
            return if s.call_audio && self.up(s) {
                CallChange::because("bt: no news of the ring for 60 s, call audio open: call up")
            } else {
                self.end(s);
                CallChange::because("bt: no news of the ring for 60 s: ring over")
            };
        }
        let audio_gone = self
            .audio_gone_at_ms
            .is_some_and(|at| now_ms.saturating_sub(at) >= CALL_AUDIO_GRACE_MS);
        if audio_gone && !s.call_audio {
            self.end(s);
            return CallChange::because("bt: the call audio closed: call over here");
        }
        CallChange::default()
    }

    /// Answer on the Wave: the call is up at once, as on stock's calling
    /// page. Returns whether `s` changed (only while ringing).
    pub fn answered(&mut self, s: &mut BluetoothState) -> bool {
        self.up(s)
    }

    /// Decline or Hang up on the Wave: no call at once, and the same number
    /// repeated right after does not ring again. Returns whether `s` changed.
    pub fn hung_up(&mut self, now_ms: u64, s: &mut BluetoothState) -> bool {
        let Some(number) = s.call.number().map(str::to_string) else {
            return false;
        };
        self.declined = Some(Declined { number, quiet_until_ms: now_ms + DECLINED_QUIET_MS });
        self.end(s)
    }

    fn number(&mut self, number: &str, now_ms: u64, s: &mut BluetoothState) -> CallChange {
        if let Some(declined) = self.declined.as_mut() {
            if declined.number == number {
                declined.quiet_until_ms = now_ms + DECLINED_QUIET_MS;
                return CallChange::default();
            }
            self.declined = None;
        }
        match &s.call {
            // A repeat during a call is not a new ring (call waiting is not
            // handled).
            BtCall::Active { .. } => CallChange::default(),
            BtCall::Ringing { number: ringing } if ringing == number => {
                self.ring_until_ms = Some(now_ms + RING_TIMEOUT_MS);
                CallChange::default()
            }
            _ => {
                s.call = BtCall::Ringing { number: number.to_string() };
                self.ring_until_ms = Some(now_ms + RING_TIMEOUT_MS);
                self.audio_seen = s.call_audio;
                self.audio_gone_at_ms = None;
                CallChange { changed: true, why: None }
            }
        }
    }

    /// Ringing → Active. The call audio is judged afresh from here: a call
    /// answered on the phone with its audio there stays up until the phone
    /// says it ended, as on stock.
    fn up(&mut self, s: &mut BluetoothState) -> bool {
        let BtCall::Ringing { number } = &s.call else {
            return false;
        };
        s.call = BtCall::Active { number: number.clone() };
        self.ring_until_ms = None;
        self.audio_seen = s.call_audio;
        self.audio_gone_at_ms = None;
        true
    }

    fn end(&mut self, s: &mut BluetoothState) -> bool {
        let changed = s.call != BtCall::Idle;
        s.call = BtCall::Idle;
        self.reset();
        changed
    }

    fn follow_audio(&mut self, now_ms: u64, s: &BluetoothState) {
        if s.call == BtCall::Idle {
            return;
        }
        if s.call_audio {
            self.audio_seen = true;
            self.audio_gone_at_ms = None;
        } else if self.audio_seen && self.audio_gone_at_ms.is_none() {
            self.audio_gone_at_ms = Some(now_ms);
        }
    }

    fn reset(&mut self) {
        self.ring_until_ms = None;
        self.audio_seen = false;
        self.audio_gone_at_ms = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{PhoneEvent, WorkStatus};

    const N: &str = "+32470123456";

    fn ring(c: &mut Calls, now: u64, s: &mut BluetoothState) -> CallChange {
        c.event(&Event::CallNumber(N.into()), now, s)
    }

    fn ringing() -> BtCall {
        BtCall::Ringing { number: N.into() }
    }

    fn active() -> BtCall {
        BtCall::Active { number: N.into() }
    }

    /// `0xB2` as `Tracker` folds it: the call audio bit into `s`, then here.
    fn audio(c: &mut Calls, open: bool, now: u64, s: &mut BluetoothState) -> CallChange {
        s.call_audio = open;
        s.phone_audio = open;
        let bits = if open { 0x07 } else { 0x01 };
        c.event(&Event::WorkStatus { status: WorkStatus(bits), phone: None }, now, s)
    }

    fn log(c: &mut Calls, line: &str, now: u64, s: &mut BluetoothState) -> CallChange {
        c.event(&Event::Log(line.into()), now, s)
    }

    #[test]
    fn a_number_rings_and_call_up_makes_it_active() {
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        assert_eq!(ring(&mut c, 0, &mut s), CallChange { changed: true, why: None });
        assert_eq!(s.call, ringing());
        assert_eq!(ring(&mut c, 500, &mut s), CallChange::default(), "a repeat");
        let up = c.event(&Event::Phone(PhoneEvent::CallActive), 1_000, &mut s);
        assert!(up.changed && up.why.is_none(), "the frame's own line says it");
        assert_eq!(s.call, active());
        assert_eq!(ring(&mut c, 2_000, &mut s), CallChange::default(), "no ring during a call");
        assert_eq!(s.call, active());
        assert!(c.event(&Event::Phone(PhoneEvent::CallEnded), 3_000, &mut s).changed);
        assert_eq!(s.call, BtCall::Idle);
    }

    #[test]
    fn call_events_without_a_number_change_nothing() {
        // Stock acts on 0xD1 00 / 01 only while a number is stored (77006).
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        for e in [
            Event::Phone(PhoneEvent::CallActive),
            Event::Phone(PhoneEvent::CallEnded),
            Event::CallCleared,
            Event::Phone(PhoneEvent::RecordStart),
            Event::Log("HFP  Call active".into()),
            Event::Log("HFP call end".into()),
        ] {
            assert_eq!(c.event(&e, 0, &mut s), CallChange::default(), "{e:?}");
        }
        assert_eq!(s.call, BtCall::Idle);
    }

    #[test]
    fn an_empty_number_ends_the_call() {
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        ring(&mut c, 0, &mut s);
        assert!(c.event(&Event::CallCleared, 1, &mut s).changed);
        assert_eq!(s.call, BtCall::Idle);
        ring(&mut c, 2, &mut s);
        c.event(&Event::Phone(PhoneEvent::CallActive), 3, &mut s);
        c.event(&Event::CallCleared, 4, &mut s);
        assert_eq!(s.call, BtCall::Idle);
    }

    #[test]
    fn a_new_number_while_ringing_replaces_it() {
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        ring(&mut c, 0, &mut s);
        assert!(c.event(&Event::CallNumber("0472".into()), 1, &mut s).changed);
        assert_eq!(s.call, BtCall::Ringing { number: "0472".into() });
    }

    #[test]
    fn the_call_audio_bit_alone_starts_no_call() {
        // An in-band ring tone opens the audio link while ringing (decision 9).
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        ring(&mut c, 0, &mut s);
        audio(&mut c, true, 100, &mut s);
        assert_eq!(s.call, ringing());
        assert_eq!(c.tick(30_000, &mut s), CallChange::default());
        assert_eq!(s.call, ringing());
    }

    #[test]
    fn the_module_log_says_the_call_is_up_or_over() {
        // Device 2026-10-13: answered on the iPhone, no 0xD1 and no empty
        // 0xB1. The module's firmware prints these (0.0.0.28 and 0.0.0.33).
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        ring(&mut c, 0, &mut s);
        let up = log(&mut c, "@HFP  Call active", 5_000, &mut s);
        assert_eq!(up, CallChange::because("bt: the module reports the call up"));
        assert_eq!(s.call, active());
        let over = log(&mut c, "HFP call end\r\n", 60_000, &mut s);
        assert!(over.changed && over.why.is_some());
        assert_eq!(s.call, BtCall::Idle);
        // The call link closing ends a ring too.
        ring(&mut c, 70_000, &mut s);
        assert!(log(&mut c, "HFP RFCOMM_CLOSED no hf_dev!!", 71_000, &mut s).changed);
        assert_eq!(s.call, BtCall::Idle);
        // Other lines say nothing about the call.
        ring(&mut c, 80_000, &mut s);
        for line in ["HFP  incoming", "HFP outgoing", "AT RING", "hf statusCall = 1", "RECONN_START!"] {
            assert_eq!(log(&mut c, line, 81_000, &mut s), CallChange::default(), "{line}");
        }
        assert_eq!(s.call, ringing());
    }

    #[test]
    fn a_ring_whose_call_audio_closed_is_over() {
        // In-band ring, then answered on the phone with its audio there (or
        // declined there): the call audio closes and stays closed.
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        ring(&mut c, 0, &mut s);
        audio(&mut c, true, 1_000, &mut s);
        audio(&mut c, false, 8_000, &mut s);
        assert_eq!(c.tick(8_000 + CALL_AUDIO_GRACE_MS - 1, &mut s), CallChange::default());
        assert_eq!(s.call, ringing());
        let over = c.tick(8_000 + CALL_AUDIO_GRACE_MS, &mut s);
        assert_eq!(over, CallChange::because("bt: the call audio closed: call over here"));
        assert_eq!(s.call, BtCall::Idle);
    }

    #[test]
    fn call_audio_that_reopens_in_time_keeps_the_call() {
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        ring(&mut c, 0, &mut s);
        c.answered(&mut s);
        audio(&mut c, true, 1_000, &mut s);
        audio(&mut c, false, 2_000, &mut s);
        audio(&mut c, true, 3_000, &mut s);
        assert_eq!(c.tick(20_000, &mut s), CallChange::default());
        assert_eq!(s.call, active());
        // Hung up on the phone: the audio closes for good.
        audio(&mut c, false, 600_000, &mut s);
        assert!(c.tick(600_000 + CALL_AUDIO_GRACE_MS, &mut s).changed);
        assert_eq!(s.call, BtCall::Idle);
    }

    #[test]
    fn a_call_up_without_its_audio_here_stays_up_as_on_stock() {
        // 0xD1 00 after the audio closed: answered on the phone, audio
        // there. Stock shows its calling page until 0xD1 01.
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        ring(&mut c, 0, &mut s);
        audio(&mut c, true, 500, &mut s);
        audio(&mut c, false, 4_000, &mut s);
        c.event(&Event::Phone(PhoneEvent::CallActive), 5_000, &mut s);
        assert_eq!(s.call, active());
        assert_eq!(c.tick(5_000 + 10 * CALL_AUDIO_GRACE_MS, &mut s), CallChange::default());
        assert_eq!(c.tick(5_000 + 10 * RING_TIMEOUT_MS, &mut s), CallChange::default());
        assert_eq!(s.call, active());
        assert!(c.event(&Event::Phone(PhoneEvent::CallEnded), 900_000, &mut s).changed);
    }

    #[test]
    fn a_ring_with_no_news_ends_after_a_minute() {
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        ring(&mut c, 0, &mut s);
        assert_eq!(c.tick(RING_TIMEOUT_MS - 1, &mut s), CallChange::default());
        let over = c.tick(RING_TIMEOUT_MS, &mut s);
        assert_eq!(over, CallChange::because("bt: no news of the ring for 60 s: ring over"));
        assert_eq!(s.call, BtCall::Idle);
        assert_eq!(c.tick(RING_TIMEOUT_MS * 2, &mut s), CallChange::default());
    }

    #[test]
    fn each_repeat_of_the_number_restarts_the_minute() {
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        ring(&mut c, 0, &mut s);
        ring(&mut c, 50_000, &mut s);
        assert_eq!(c.tick(RING_TIMEOUT_MS + 1, &mut s), CallChange::default());
        assert_eq!(s.call, ringing());
        assert!(c.tick(50_000 + RING_TIMEOUT_MS, &mut s).changed);
        assert_eq!(s.call, BtCall::Idle);
    }

    #[test]
    fn a_minute_of_ringing_with_call_audio_open_is_a_call() {
        // Answered on the phone with the audio sent here: the call audio
        // stays open, the module says nothing.
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        ring(&mut c, 0, &mut s);
        audio(&mut c, true, 10_000, &mut s);
        let up = c.tick(RING_TIMEOUT_MS, &mut s);
        assert_eq!(up, CallChange::because("bt: no news of the ring for 60 s, call audio open: call up"));
        assert_eq!(s.call, active());
        audio(&mut c, false, 300_000, &mut s);
        assert!(c.tick(300_000 + CALL_AUDIO_GRACE_MS, &mut s).changed);
        assert_eq!(s.call, BtCall::Idle);
    }

    #[test]
    fn answer_on_the_wave_is_up_at_once() {
        // Stock's pickup_btn_click_event_cb opens the calling page at once.
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        assert!(!c.answered(&mut s), "nothing rings");
        ring(&mut c, 0, &mut s);
        assert!(c.answered(&mut s));
        assert_eq!(s.call, active());
        assert!(!c.answered(&mut s), "already up");
        // The module's own 0xD1 00 afterwards changes nothing.
        assert_eq!(c.event(&Event::Phone(PhoneEvent::CallActive), 1, &mut s), CallChange::default());
        // No ring timeout once up.
        assert_eq!(c.tick(10 * RING_TIMEOUT_MS, &mut s), CallChange::default());
    }

    #[test]
    fn decline_and_hang_up_on_the_wave_end_it_at_once() {
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        assert!(!c.hung_up(0, &mut s), "no call");
        ring(&mut c, 0, &mut s);
        assert!(c.hung_up(1_000, &mut s));
        assert_eq!(s.call, BtCall::Idle);
        ring(&mut c, 2_000, &mut s);
        assert_eq!(s.call, BtCall::Idle, "the declined number repeated does not ring again");
        c.tick(2_000 + DECLINED_QUIET_MS - 1, &mut s);
        ring(&mut c, 2_000 + DECLINED_QUIET_MS - 1, &mut s);
        assert_eq!(s.call, BtCall::Idle, "each repeat keeps it quiet");
        // Quiet for 10 s: the next call from that number rings.
        let later = 2_000 + 2 * DECLINED_QUIET_MS;
        c.tick(later, &mut s);
        assert!(ring(&mut c, later, &mut s).changed);
        // Another number rings at once.
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        ring(&mut c, 0, &mut s);
        c.answered(&mut s);
        assert!(c.hung_up(1_000, &mut s), "hang up");
        assert!(c.event(&Event::CallNumber("0471".into()), 1_500, &mut s).changed);
    }

    #[test]
    fn nothing_rings_while_bluetooth_is_off() {
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        c.set_enabled(false);
        assert_eq!(ring(&mut c, 0, &mut s), CallChange::default());
        assert_eq!(s.call, BtCall::Idle);
        c.set_enabled(true);
        assert!(ring(&mut c, 1, &mut s).changed);
    }

    #[test]
    fn a_call_cleared_elsewhere_stops_the_clocks() {
        // Tracker clears the call when the phone drops.
        let (mut c, mut s) = (Calls::default(), BluetoothState::default());
        ring(&mut c, 0, &mut s);
        s.call = BtCall::Idle;
        assert_eq!(c.tick(RING_TIMEOUT_MS, &mut s), CallChange::default());
        assert_eq!(s.call, BtCall::Idle);
    }

    #[test]
    fn hints_match_the_start_of_the_line_only() {
        assert_eq!(call_hint("HFP  Call active"), Some(CallHint::Up));
        assert_eq!(call_hint("@ HFP  Call active 1"), Some(CallHint::Up));
        assert_eq!(call_hint("HFP call end"), Some(CallHint::Over));
        assert_eq!(call_hint("HFP RFCOMM_CLOSED"), Some(CallHint::Over));
        assert_eq!(call_hint("HFP Call active"), None, "one space is another message");
        assert_eq!(call_hint("xx HFP call end"), None);
        assert_eq!(call_hint(""), None);
    }
}
