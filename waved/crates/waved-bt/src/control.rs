//! Bus commands for the module (I1, I3): what each one sends, and Wave's
//! 2-minute pairing window. docs/BLUETOOTH_PROTOCOL.md, "Verified on the
//! device"; plan `docs/superpowers/plans/2026-10-13-m3-i1-i3-i4-bt-control.md`.
//!
//! The module has no command to leave pairing mode (stock has none either)
//! and its own timeout is unknown, so the window is the host's: it ends after
//! [`PAIRING_WINDOW_MS`], when a phone connects, when the module clears
//! `0xB2` bit 3 after setting it, or on `BtStopPairing` (the user left the
//! pairing screen). Nothing is sent when it ends.
//!
//! Dropping a phone (`BtDisconnect`, `BtPair`) first turns the module's
//! reconnect to the last phone off (`0xD8 00`), as stock has it whenever a
//! phone is connected (it sends `0xD8 00` on each connect, 75955).

use crate::command::{ButtonEvent, Command};
use waved_state::{BluetoothState, BtCall, Command as BusCommand};

/// How long Wave shows itself as pairing after `BtPair`.
pub const PAIRING_WINDOW_MS: u64 = 120_000;

/// What a bus command asks of the module.
#[derive(Debug, Default, PartialEq)]
pub struct Response {
    /// Frames to send, in order.
    pub frames: Vec<Command>,
    /// `BluetoothState` changed (notify watchers).
    pub changed: bool,
}

#[derive(Debug)]
struct Window {
    until_ms: u64,
    /// A phone was connected when last seen: only a new connection ends the
    /// window, not the old phone still dropping.
    connected: bool,
    /// The module reported pairing mode (`0xB2` bit 3) since the window opened.
    module_pairing: bool,
}

#[derive(Debug, Default)]
pub struct Control {
    window: Option<Window>,
}

impl Control {
    /// A command from the bus. `now_ms` is monotonic; `unix_time` stamps
    /// button events as stock does (`time(NULL)`).
    pub fn command(
        &mut self,
        command: &BusCommand,
        now_ms: u64,
        unix_time: u32,
        s: &mut BluetoothState,
    ) -> Response {
        let button = |event| Command::Button { event, unix_time };
        let mut out = Response::default();
        match command {
            // Stock's "Connect New Device" (libui 115166), back to back, with
            // the module's reconnect off as stock always has it by then.
            BusCommand::BtPair => {
                out.frames = vec![
                    Command::AutoReconnect(false),
                    Command::Disconnect,
                    Command::EnterPairing,
                ];
                self.window = Some(Window {
                    until_ms: now_ms + PAIRING_WINDOW_MS,
                    connected: s.connected,
                    module_pairing: false,
                });
                out.changed = self.publish(now_ms, s);
            }
            // Stock sends `0xD8 00` as each phone connects (75955): its
            // disconnects never race the module's reconnect. Wave sends it
            // right before.
            BusCommand::BtDisconnect => {
                out.frames = vec![Command::AutoReconnect(false), Command::Disconnect];
            }
            // No command leaves pairing mode: only the window ends.
            BusCommand::BtStopPairing => {
                if self.window.take().is_some() {
                    out.changed = set_left(s, 0);
                }
            }
            // Event 3 doubles as a music key: only while a call rings.
            BusCommand::BtAnswerCall if s.call.is_ringing() => {
                out.frames = vec![button(ButtonEvent::AnswerCall)];
            }
            BusCommand::BtDeclineCall if s.call != BtCall::Idle => {
                out.frames = vec![button(ButtonEvent::EndCall)];
            }
            _ => {}
        }
        out
    }

    /// After each module event has been folded into `s`: a phone that
    /// connected, or the module leaving pairing mode, ends the window.
    /// Returns whether `s` changed.
    pub fn after_event(&mut self, s: &mut BluetoothState) -> bool {
        let Some(window) = self.window.as_mut() else {
            return false;
        };
        let connected_now = s.connected && !window.connected;
        window.connected = s.connected;
        let module_left = window.module_pairing && !s.pairing;
        window.module_pairing |= s.pairing;
        if connected_now || module_left {
            self.window = None;
            return set_left(s, 0);
        }
        false
    }

    /// The countdown; call it often (the service does every 100 ms).
    /// Returns whether `s` changed.
    pub fn tick(&mut self, now_ms: u64, s: &mut BluetoothState) -> bool {
        self.publish(now_ms, s)
    }

    /// Seconds left, rounded up, into `s`; closes an expired window.
    fn publish(&mut self, now_ms: u64, s: &mut BluetoothState) -> bool {
        let left_ms = match &self.window {
            Some(window) => window.until_ms.saturating_sub(now_ms),
            None => return false,
        };
        if left_ms == 0 {
            self.window = None;
        }
        set_left(s, left_ms.div_ceil(1_000) as u16)
    }
}

fn set_left(s: &mut BluetoothState, seconds: u16) -> bool {
    let changed = s.pairing_left_s != seconds;
    s.pairing_left_s = seconds;
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u32 = 0x6720_0000;

    fn hex(frames: &[Command]) -> Vec<String> {
        frames
            .iter()
            .map(|c| c.encode().iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "))
            .collect()
    }

    fn send(c: &mut Control, command: BusCommand, now_ms: u64, s: &mut BluetoothState) -> Vec<String> {
        hex(&c.command(&command, now_ms, T, s).frames)
    }

    /// `AutoReconnect(false)`: stop the module reconnecting to the last phone.
    /// Stock sends it each time a phone connects (75955), so its "Connect a
    /// new device" always runs with it off.
    const RECONNECT_OFF: &str = "75 45 02 D8 00 DA";
    const DISCONNECT: &str = "75 45 02 A3 00 A5";
    const ENTER_PAIRING: &str = "75 45 02 A7 00 A9";

    #[test]
    fn pair_drops_the_phone_then_enters_pairing_for_two_minutes() {
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        let out = c.command(&BusCommand::BtPair, 1_000, T, &mut s);
        assert_eq!(hex(&out.frames), [RECONNECT_OFF, DISCONNECT, ENTER_PAIRING]);
        assert!(out.changed);
        assert_eq!(s.pairing_left_s, 120);
    }

    #[test]
    fn pairing_another_phone_while_one_is_connected_is_the_same_sequence() {
        // Stock's "Connect New Device" (ui 22184 → bluetooth_set_connect(0),
        // 115166): Disconnect, then EnterPairing.
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        s.connected = true;
        let out = c.command(&BusCommand::BtPair, 0, T, &mut s);
        assert_eq!(hex(&out.frames), [RECONNECT_OFF, DISCONNECT, ENTER_PAIRING]);
        assert_eq!(s.pairing_left_s, 120);
    }

    #[test]
    fn stop_pairing_closes_the_window_and_sends_nothing() {
        // No command leaves pairing mode (stock has none): the window is the
        // host's, and it ends here.
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        c.command(&BusCommand::BtPair, 0, T, &mut s);
        c.tick(20_000, &mut s);
        assert_eq!(s.pairing_left_s, 100);
        let out = c.command(&BusCommand::BtStopPairing, 20_000, T, &mut s);
        assert!(out.frames.is_empty());
        assert!(out.changed);
        assert_eq!(s.pairing_left_s, 0);
        assert!(!c.tick(21_000, &mut s), "closed");
        s.connected = true;
        assert!(!c.after_event(&mut s));
        assert_eq!(s.pairing_left_s, 0);
        // Without a window it changes nothing.
        assert_eq!(c.command(&BusCommand::BtStopPairing, 30_000, T, &mut s), Response::default());
    }

    #[test]
    fn the_window_counts_down_and_ends_after_two_minutes() {
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        c.command(&BusCommand::BtPair, 1_000, T, &mut s);
        assert!(!c.tick(1_500, &mut s), "still 120 s, rounded up");
        assert!(c.tick(2_000, &mut s));
        assert_eq!(s.pairing_left_s, 119);
        c.tick(1_000 + 18_000, &mut s);
        assert_eq!(s.pairing_left_s, 102, "1:42");
        c.tick(1_000 + PAIRING_WINDOW_MS - 1, &mut s);
        assert_eq!(s.pairing_left_s, 1);
        assert!(c.tick(1_000 + PAIRING_WINDOW_MS, &mut s));
        assert_eq!(s.pairing_left_s, 0);
        assert!(!c.tick(1_000 + PAIRING_WINDOW_MS + 5_000, &mut s), "closed");
        assert_eq!(s.pairing_left_s, 0);
    }

    #[test]
    fn a_phone_connecting_ends_the_window() {
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        c.command(&BusCommand::BtPair, 0, T, &mut s);
        assert!(!c.after_event(&mut s));
        s.connected = true;
        assert!(c.after_event(&mut s));
        assert_eq!(s.pairing_left_s, 0);
        assert!(!c.tick(10_000, &mut s));
        assert_eq!(s.pairing_left_s, 0);
    }

    #[test]
    fn the_old_phone_still_connected_does_not_end_it() {
        // Pair while a phone is connected: Disconnect first, so the old phone
        // stays connected for a moment. Only a new connection ends the window.
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        s.connected = true;
        c.command(&BusCommand::BtPair, 0, T, &mut s);
        assert!(!c.after_event(&mut s));
        assert_eq!(s.pairing_left_s, 120);
        s.connected = false;
        assert!(!c.after_event(&mut s));
        s.connected = true;
        assert!(c.after_event(&mut s));
        assert_eq!(s.pairing_left_s, 0);
    }

    #[test]
    fn the_module_leaving_pairing_mode_ends_it() {
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        c.command(&BusCommand::BtPair, 0, T, &mut s);
        // Bit 3 not seen yet: a status without it (the Disconnect) keeps the window.
        assert!(!c.after_event(&mut s));
        s.pairing = true;
        assert!(!c.after_event(&mut s));
        assert_eq!(s.pairing_left_s, 120);
        s.pairing = false;
        assert!(c.after_event(&mut s));
        assert_eq!(s.pairing_left_s, 0);
    }

    #[test]
    fn pairing_again_restarts_the_window() {
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        c.command(&BusCommand::BtPair, 0, T, &mut s);
        c.tick(60_000, &mut s);
        assert_eq!(s.pairing_left_s, 60);
        c.command(&BusCommand::BtPair, 60_000, T, &mut s);
        assert_eq!(s.pairing_left_s, 120);
    }

    #[test]
    fn without_a_window_events_and_ticks_change_nothing() {
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        s.connected = true;
        s.pairing = true;
        assert!(!c.after_event(&mut s));
        assert!(!c.tick(1_000, &mut s));
        assert_eq!(s.pairing_left_s, 0);
    }

    #[test]
    fn disconnect_stops_the_reconnect_first() {
        // Device 2026-10-13: after a bare Disconnect the iPhone was back
        // within a second. Stock never disconnects with the reconnect on: it
        // turns it off as each phone connects.
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        s.connected = true;
        let out = c.command(&BusCommand::BtDisconnect, 0, T, &mut s);
        assert_eq!(hex(&out.frames), [RECONNECT_OFF, DISCONNECT]);
        assert!(!out.changed);
    }

    #[test]
    fn answer_only_while_ringing() {
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        assert!(send(&mut c, BusCommand::BtAnswerCall, 0, &mut s).is_empty(), "nothing rings");
        s.call = BtCall::Ringing { number: "0471".into() };
        assert_eq!(send(&mut c, BusCommand::BtAnswerCall, 0, &mut s), ["75 45 07 D0 03 00 00 20 67 00 61"]);
        s.call = BtCall::Active { number: "0471".into() };
        assert!(send(&mut c, BusCommand::BtAnswerCall, 0, &mut s).is_empty(), "already up");
    }

    #[test]
    fn decline_while_ringing_and_hang_up_while_active() {
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        assert!(send(&mut c, BusCommand::BtDeclineCall, 0, &mut s).is_empty(), "no call");
        s.call = BtCall::Ringing { number: "0471".into() };
        assert_eq!(send(&mut c, BusCommand::BtDeclineCall, 0, &mut s), ["75 45 07 D0 00 00 00 20 67 00 5E"]);
        s.call = BtCall::Active { number: "0471".into() };
        assert_eq!(send(&mut c, BusCommand::BtDeclineCall, 0, &mut s), ["75 45 07 D0 00 00 00 20 67 00 5E"]);
    }

    #[test]
    fn other_commands_send_nothing() {
        let (mut c, mut s) = (Control::default(), BluetoothState::default());
        for command in [BusCommand::StartRecording, BusCommand::ToggleMicMute, BusCommand::Shutdown] {
            assert_eq!(c.command(&command, 0, T, &mut s), Response::default(), "{command:?}");
        }
    }
}
