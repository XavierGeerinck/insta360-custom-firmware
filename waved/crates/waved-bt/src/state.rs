//! Folds decoded module events into `BluetoothState`, the way stock keeps
//! `BT_CONNECT_STATUS`, the phone's name and MAC (`bt_msg_recv_thread`).

use crate::event::{format_mac, Event, Link, Mac};
use waved_state::{BluetoothState, DeviceEvent};

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
}

impl Tracker {
    pub fn apply(&mut self, event: &Event, s: &mut BluetoothState) -> Applied {
        let mut out = Applied::default();
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
            Event::ModuleInfo { mac } => {
                let mac = format_mac(mac);
                if s.mac != mac {
                    s.mac = mac;
                    out.changed = true;
                }
            }
            _ => {}
        }
        out
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
            Event::CallNumber("0471".into()),
            Event::Heartbeat { phone: None },
            Event::Other { cmd: 0xA9, data: vec![0] },
        ] {
            assert_eq!(t.apply(&e, &mut s), Applied::default(), "{e:?}");
        }
    }
}
