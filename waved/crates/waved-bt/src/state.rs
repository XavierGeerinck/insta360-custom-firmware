//! Folds decoded module events into `BluetoothState`, the way stock keeps
//! `BT_CONNECT_STATUS`, the phone's name and MAC (`bt_msg_recv_thread`), plus
//! the module's own name. The call is `call::Calls`'s; a phone that drops
//! takes it along.
//!
//! **Switched off** (stock's Bluetooth switch, `CloseBt`): the phone counts
//! as gone at once (`bt_ctrl_cmd_send` 74413-74444) and nothing brings it
//! back until the switch is on: stock ignores `0xB2` bit 0 (75805) and `0xB4`
//! (76303, 76327) while off. waved also ignores `0xAA` (stock does not,
//! 75696: on the device the module kept answering `0xAA 01` after `CloseBt`),
//! the audio bits and the phone's name.

use crate::event::{format_mac, Event, Link, Mac};
use waved_state::{BluetoothState, BtCall, DeviceEvent};

/// What applying one event did.
#[derive(Debug, Default, PartialEq)]
pub struct Applied {
    /// The state changed (notify watchers).
    pub changed: bool,
    /// A connect / disconnect to broadcast.
    pub event: Option<DeviceEvent>,
}

/// Link reports in a row that must say "no phone" before stock believes it.
const ABSENT_REPORTS: u8 = 3;

/// Memory that does not belong in `BluetoothState`.
#[derive(Debug, Default)]
pub struct Tracker {
    phone_mac: Option<Mac>,
    absent_reports: u8,
    /// Bluetooth is switched off.
    off: bool,
}

impl Tracker {
    /// The Bluetooth switch. Off counts the phone as gone at once (stock
    /// sets `BT_CONNECT_STATUS` = 1 with `CloseBt`, 74422).
    pub fn set_enabled(&mut self, on: bool, s: &mut BluetoothState) -> Applied {
        self.off = !on;
        self.absent_reports = 0;
        let mut out = Applied::default();
        if self.off {
            set_connected(s, false, &mut out);
            set_flag(&mut s.phone_audio, false, &mut out);
            set_flag(&mut s.call_audio, false, &mut out);
        }
        out
    }

    pub fn apply(&mut self, event: &Event, s: &mut BluetoothState) -> Applied {
        let mut out = Applied::default();
        if self.off {
            // Only the module's own news counts: its name, MAC and pairing
            // bit. Nothing connects, rings or plays.
            match event {
                Event::WorkStatus { status, .. } => set_flag(&mut s.pairing, status.pairing(), &mut out),
                Event::ModuleInfo { mac } => set_module_mac(s, mac, &mut out),
                Event::Name(name) => set_advertised_name(s, name, &mut out),
                _ => {}
            }
            return out;
        }
        match event {
            Event::Status { connected } => set_connected(s, *connected, &mut out),
            Event::WorkStatus { status, phone } => {
                if let Some(p) = phone {
                    self.phone_mac = p.mac.or(self.phone_mac);
                    set_name(s, &p.name, &mut out);
                }
                set_connected(s, status.phone_connected(), &mut out);
                set_flag(&mut s.pairing, status.pairing(), &mut out);
                set_flag(&mut s.phone_audio, status.phone_speaker(), &mut out);
                set_flag(&mut s.call_audio, status.phone_mic(), &mut out);
            }
            Event::Heartbeat { phone: Some(p) } => {
                self.phone_mac = p.mac.or(self.phone_mac);
                set_name(s, &p.name, &mut out);
            }
            Event::LinkReport { phone, .. } => match phone {
                Link::Connected(_) => {
                    self.absent_reports = 0;
                    set_connected(s, true, &mut out);
                }
                Link::Absent => {
                    self.absent_reports = self.absent_reports.saturating_add(1);
                    if self.absent_reports >= ABSENT_REPORTS {
                        self.absent_reports = 0;
                        set_connected(s, false, &mut out);
                    }
                }
                Link::Unknown(_) => {}
            },
            Event::ConnectRequest { mac, name } => {
                self.phone_mac = Some(*mac);
                set_name(s, name, &mut out);
            }
            Event::DeviceDisconnected { mac } => {
                if self.phone_mac == Some(*mac) {
                    set_connected(s, false, &mut out);
                }
            }
            Event::PhoneMac(mac) => self.phone_mac = Some(*mac),
            Event::ModuleInfo { mac } => set_module_mac(s, mac, &mut out),
            Event::Name(name) => set_advertised_name(s, name, &mut out),
            _ => {}
        }
        out
    }
}

fn set_module_mac(s: &mut BluetoothState, mac: &Mac, out: &mut Applied) {
    let mac = format_mac(mac);
    if s.mac != mac {
        s.mac = mac;
        out.changed = true;
    }
}

/// The module's own name; an empty reply keeps what we know.
fn set_advertised_name(s: &mut BluetoothState, name: &str, out: &mut Applied) {
    if !name.is_empty() && s.advertised_name != name {
        s.advertised_name = name.to_string();
        out.changed = true;
    }
}

fn set_flag(flag: &mut bool, value: bool, out: &mut Applied) {
    if *flag != value {
        *flag = value;
        out.changed = true;
    }
}

fn set_name(s: &mut BluetoothState, name: &str, out: &mut Applied) {
    if !name.is_empty() && s.device_name != name {
        s.device_name = name.to_string();
        out.changed = true;
    }
}

fn set_connected(s: &mut BluetoothState, connected: bool, out: &mut Applied) {
    if s.connected == connected {
        return;
    }
    s.connected = connected;
    out.changed = true;
    out.event = Some(if connected {
        DeviceEvent::BtConnected(s.device_name.clone())
    } else {
        s.device_name.clear();
        // No phone, no call and no phone audio, whichever frame said it
        // dropped (waved-audio keeps hw:0,0 open while the audio bits are
        // set). Pairing mode is the module's own and stays.
        s.call = BtCall::Idle;
        s.phone_audio = false;
        s.call_audio = false;
        DeviceEvent::BtDisconnected
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Phone, WorkStatus};

    const MAC: Mac = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];

    fn phone(name: &str) -> Option<Phone> {
        Some(Phone { name: name.into(), mac: Some(MAC) })
    }

    fn report(phone: Link<i8>) -> Event {
        Event::LinkReport { dongle: Link::Absent, phone, counters: None }
    }

    #[test]
    fn work_status_bits_set_audio_and_pairing() {
        // Seen on the device 2026-10-13: 0x08 after EnterPairing, 0x01 once a
        // phone connected. Bits 1 and 2 are stock's audio bits (I2).
        let mut t = Tracker::default();
        let mut s = BluetoothState::default();
        let a = t.apply(&Event::WorkStatus { status: WorkStatus(0x08), phone: None }, &mut s);
        assert!(a.changed && s.pairing && !s.connected && !s.phone_audio && !s.call_audio);
        t.apply(&Event::WorkStatus { status: WorkStatus(0x03), phone: None }, &mut s);
        assert!(s.connected && s.phone_audio && !s.call_audio && !s.pairing);
        t.apply(&Event::WorkStatus { status: WorkStatus(0x07), phone: None }, &mut s);
        assert!(s.phone_audio && s.call_audio);
        let same = t.apply(&Event::WorkStatus { status: WorkStatus(0x07), phone: None }, &mut s);
        assert!(!same.changed);
    }

    #[test]
    fn work_status_connects_with_the_name() {
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        let a = t.apply(&Event::WorkStatus { status: WorkStatus(0x01), phone: phone("Pixel") }, &mut s);
        assert!(a.changed);
        assert_eq!(a.event, Some(DeviceEvent::BtConnected("Pixel".into())));
        assert!(s.connected);
        assert_eq!(s.device_name, "Pixel");

        // The same again changes nothing.
        let a = t.apply(&Event::WorkStatus { status: WorkStatus(0x01), phone: phone("Pixel") }, &mut s);
        assert_eq!(a, Applied::default());
    }

    #[test]
    fn status_disconnects_and_forgets_the_name() {
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        t.apply(&Event::WorkStatus { status: WorkStatus(0x01), phone: phone("Pixel") }, &mut s);
        let a = t.apply(&Event::Status { connected: false }, &mut s);
        assert!(a.changed);
        assert_eq!(a.event, Some(DeviceEvent::BtDisconnected));
        assert!(!s.connected);
        assert!(s.device_name.is_empty());
    }

    #[test]
    fn status_connects_without_a_name() {
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        let a = t.apply(&Event::Status { connected: true }, &mut s);
        assert_eq!(a.event, Some(DeviceEvent::BtConnected(String::new())));
        // A later heartbeat names the phone without a second connect event.
        let a = t.apply(&Event::Heartbeat { phone: phone("Pixel") }, &mut s);
        assert!(a.changed);
        assert_eq!(a.event, None);
        assert_eq!(s.device_name, "Pixel");
    }

    #[test]
    fn three_absent_link_reports_disconnect() {
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        t.apply(&Event::Status { connected: true }, &mut s);
        assert_eq!(t.apply(&report(Link::Absent), &mut s), Applied::default());
        assert_eq!(t.apply(&report(Link::Absent), &mut s), Applied::default());
        let a = t.apply(&report(Link::Absent), &mut s);
        assert_eq!(a.event, Some(DeviceEvent::BtDisconnected));
        assert!(!s.connected);
    }

    #[test]
    fn a_connected_link_report_resets_the_count_and_connects() {
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        t.apply(&Event::Status { connected: true }, &mut s);
        t.apply(&report(Link::Absent), &mut s);
        t.apply(&report(Link::Absent), &mut s);
        t.apply(&report(Link::Connected(-50)), &mut s);
        t.apply(&report(Link::Absent), &mut s);
        assert!(s.connected, "count restarted");

        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        let a = t.apply(&report(Link::Connected(-50)), &mut s);
        assert_eq!(a.event, Some(DeviceEvent::BtConnected(String::new())));
        assert_eq!(t.apply(&report(Link::Unknown(7)), &mut s), Applied::default());
    }

    #[test]
    fn device_disconnected_only_for_our_phone() {
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        t.apply(&Event::WorkStatus { status: WorkStatus(0x01), phone: phone("Pixel") }, &mut s);
        let other = [0xAA; 6];
        assert_eq!(t.apply(&Event::DeviceDisconnected { mac: other }, &mut s), Applied::default());
        assert!(s.connected);
        let a = t.apply(&Event::DeviceDisconnected { mac: MAC }, &mut s);
        assert_eq!(a.event, Some(DeviceEvent::BtDisconnected));
    }

    #[test]
    fn phone_mac_reply_identifies_the_phone() {
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        t.apply(&Event::Status { connected: true }, &mut s);
        t.apply(&Event::PhoneMac(MAC), &mut s);
        let a = t.apply(&Event::DeviceDisconnected { mac: MAC }, &mut s);
        assert_eq!(a.event, Some(DeviceEvent::BtDisconnected));
    }

    #[test]
    fn connect_request_names_the_phone_but_does_not_connect() {
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        let a = t.apply(&Event::ConnectRequest { mac: MAC, name: "Pixel".into() }, &mut s);
        assert!(a.changed);
        assert_eq!(a.event, None);
        assert!(!s.connected);
        assert_eq!(s.device_name, "Pixel");
    }

    #[test]
    fn module_info_sets_our_mac() {
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        let a = t.apply(&Event::ModuleInfo { mac: MAC }, &mut s);
        assert!(a.changed);
        assert_eq!(s.mac, "11:22:33:44:55:66");
        assert_eq!(t.apply(&Event::ModuleInfo { mac: MAC }, &mut s), Applied::default());
    }

    #[test]
    fn other_events_change_nothing() {
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        for e in [
            Event::Version("0.0.0.33".into()),
            Event::Heartbeat { phone: None },
            Event::Other { cmd: 0xA9, data: vec![0] },
            Event::PhoneBookEntry,
            Event::Log("app_set_bt_last_addr A1B2C3D4E557".into()),
        ] {
            assert_eq!(t.apply(&e, &mut s), Applied::default(), "{e:?}");
        }
    }

    #[test]
    fn the_module_names_itself() {
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        let a = t.apply(&Event::Name("Insta360 Wave BT 0A1B2C".into()), &mut s);
        assert!(a.changed);
        assert_eq!(a.event, None);
        assert_eq!(s.advertised_name, "Insta360 Wave BT 0A1B2C");
        assert!(s.device_name.is_empty(), "the phone's name is another field");
        assert_eq!(t.apply(&Event::Name("Insta360 Wave BT 0A1B2C".into()), &mut s), Applied::default());
        // An empty reply keeps what we know.
        assert_eq!(t.apply(&Event::Name(String::new()), &mut s), Applied::default());
        assert_eq!(s.advertised_name, "Insta360 Wave BT 0A1B2C");
    }

    #[test]
    fn the_phone_dropping_ends_the_call() {
        // The call itself is call::Calls's; a phone that drops takes it along.
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        t.apply(&Event::WorkStatus { status: WorkStatus(0x07), phone: phone("Pixel") }, &mut s);
        s.call = BtCall::Active { number: "0471".into() };
        let a = t.apply(&Event::WorkStatus { status: WorkStatus(0x00), phone: None }, &mut s);
        assert_eq!(a.event, Some(DeviceEvent::BtDisconnected));
        assert_eq!(s.call, BtCall::Idle);

        t.apply(&Event::Status { connected: true }, &mut s);
        s.call = BtCall::Ringing { number: "0471".into() };
        t.apply(&Event::Status { connected: false }, &mut s);
        assert_eq!(s.call, BtCall::Idle);
    }

    #[test]
    fn call_frames_are_not_the_trackers() {
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        for e in [
            Event::CallNumber("0471".into()),
            Event::CallCleared,
            Event::Phone(crate::event::PhoneEvent::CallActive),
        ] {
            assert_eq!(t.apply(&e, &mut s), Applied::default(), "{e:?}");
        }
        assert_eq!(s.call, BtCall::Idle);
    }

    #[test]
    fn switching_off_drops_the_phone_at_once() {
        // Stock: CloseBt sets BT_CONNECT_STATUS = 1 at once (74422).
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        t.apply(&Event::WorkStatus { status: WorkStatus(0x07), phone: phone("Pixel") }, &mut s);
        s.call = BtCall::Active { number: "0471".into() };
        let a = t.set_enabled(false, &mut s);
        assert_eq!(a.event, Some(DeviceEvent::BtDisconnected));
        assert!(!s.connected && !s.phone_audio && !s.call_audio);
        assert_eq!(s.call, BtCall::Idle);
        assert!(s.device_name.is_empty());
        // Already off: nothing more to say.
        assert_eq!(t.set_enabled(false, &mut s), Applied::default());
    }

    #[test]
    fn while_off_nothing_brings_the_phone_back() {
        // Stock ignores 0xB2 bit 0 (75805) and 0xB4 (76303, 76327) while
        // off. waved ignores 0xAA too: on the device the module kept
        // answering 0xAA 01 after CloseBt (2026-10-13).
        let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
        t.set_enabled(false, &mut s);
        for e in [
            Event::WorkStatus { status: WorkStatus(0x07), phone: phone("Pixel") },
            Event::Status { connected: true },
            report(Link::Connected(-40)),
            Event::Heartbeat { phone: phone("Pixel") },
            Event::ConnectRequest { mac: MAC, name: "Pixel".into() },
        ] {
            let a = t.apply(&e, &mut s);
            assert_eq!(a.event, None, "{e:?}");
            assert!(!s.connected && !s.phone_audio && !s.call_audio, "{e:?}");
            assert!(s.device_name.is_empty(), "{e:?}");
        }
        // The module's own news still counts: its name, MAC and pairing bit.
        t.apply(&Event::Name("Wave Boardroom".into()), &mut s);
        t.apply(&Event::ModuleInfo { mac: MAC }, &mut s);
        t.apply(&Event::WorkStatus { status: WorkStatus(0x08), phone: None }, &mut s);
        assert_eq!(s.advertised_name, "Wave Boardroom");
        assert_eq!(s.mac, "11:22:33:44:55:66");
        assert!(s.pairing);
        // Back on: the module's next report counts again.
        assert_eq!(t.set_enabled(true, &mut s), Applied::default(), "on changes nothing by itself");
        let a = t.apply(&Event::Status { connected: true }, &mut s);
        assert_eq!(a.event, Some(DeviceEvent::BtConnected(String::new())));
    }

    #[test]
    fn a_dropped_phone_takes_its_audio_bits_along() {
        // waved-audio keeps hw:0,0 open while these are set (I2): every path
        // that decides the phone is gone must clear them, not only an 0xB2.
        // Pairing mode is the module's, not the phone's: it stays.
        let drops: [(&str, Vec<Event>); 3] = [
            ("0xE1", vec![Event::DeviceDisconnected { mac: MAC }]),
            ("0xAA", vec![Event::Status { connected: false }]),
            ("0xB4 x3", vec![report(Link::Absent), report(Link::Absent), report(Link::Absent)]),
        ];
        for (name, events) in drops {
            let (mut t, mut s) = (Tracker::default(), BluetoothState::default());
            t.apply(&Event::WorkStatus { status: WorkStatus(0x0F), phone: phone("Pixel") }, &mut s);
            assert!(s.connected && s.phone_audio && s.call_audio && s.pairing);
            let mut last = Applied::default();
            for e in &events {
                last = t.apply(e, &mut s);
            }
            assert_eq!(last.event, Some(DeviceEvent::BtDisconnected), "{name}");
            assert!(!s.connected && !s.phone_audio && !s.call_audio, "{name}");
            assert!(s.pairing, "{name}: the module's pairing mode is its own");
        }
    }
}
