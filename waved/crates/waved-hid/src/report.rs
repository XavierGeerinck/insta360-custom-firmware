//! Pure HID-report serialization — `HidEvent → (press_bytes, release_bytes)`.
//!
//! Report IDs and bit mapping match the **stock** descriptor from
//! `fw_v1.1.6.6/rootfs/etc/init.d/S89usbgadget:199`:
//!
//!   Report 0x01 (Consumer): bit0 VolumeUp, bit1 VolumeDown, bit2 Mute,
//!                           bit3 PlayPause, bit4 Next, bit5 Prev.
//!   Report 0x02 (Telephony): bit0 HookSwitch, bit1 PhoneMute, ...

use super::events::HidEvent;

pub const REPORT_ID_CONSUMER:  u8 = 0x01;
pub const REPORT_ID_TELEPHONY: u8 = 0x02;

pub const CONSUMER_VOLUME_UP:   u8 = 1 << 0;
pub const CONSUMER_VOLUME_DOWN: u8 = 1 << 1;
pub const CONSUMER_MUTE:        u8 = 1 << 2;
pub const CONSUMER_PLAY_PAUSE:  u8 = 1 << 3;
pub const CONSUMER_NEXT:        u8 = 1 << 4;
pub const CONSUMER_PREV:        u8 = 1 << 5;

pub const TELEPHONY_PHONE_MUTE: u8 = 1 << 1;

/// Serialize one `HidEvent` to `(press, release)` buffers, each including
/// the leading Report ID byte.
pub fn event_to_press_release(ev: HidEvent) -> ([u8; 2], [u8; 2]) {
    match ev {
        HidEvent::ConsumerVolumeStep { up: true }  => ([REPORT_ID_CONSUMER, CONSUMER_VOLUME_UP],   [REPORT_ID_CONSUMER, 0x00]),
        HidEvent::ConsumerVolumeStep { up: false } => ([REPORT_ID_CONSUMER, CONSUMER_VOLUME_DOWN], [REPORT_ID_CONSUMER, 0x00]),
        HidEvent::ConsumerMute                     => ([REPORT_ID_CONSUMER, CONSUMER_MUTE],        [REPORT_ID_CONSUMER, 0x00]),
        HidEvent::ConsumerPlayPause                => ([REPORT_ID_CONSUMER, CONSUMER_PLAY_PAUSE],  [REPORT_ID_CONSUMER, 0x00]),
        HidEvent::ConsumerNext                     => ([REPORT_ID_CONSUMER, CONSUMER_NEXT],        [REPORT_ID_CONSUMER, 0x00]),
        HidEvent::ConsumerPrev                     => ([REPORT_ID_CONSUMER, CONSUMER_PREV],        [REPORT_ID_CONSUMER, 0x00]),
        HidEvent::TelephonyPhoneMute               => ([REPORT_ID_TELEPHONY, TELEPHONY_PHONE_MUTE], [REPORT_ID_TELEPHONY, 0x00]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_up_uses_consumer_report_bit0() {
        let (p, r) = event_to_press_release(HidEvent::ConsumerVolumeStep { up: true });
        assert_eq!(p, [0x01, 0b0000_0001]);
        assert_eq!(r, [0x01, 0x00]);
    }

    #[test]
    fn volume_down_uses_consumer_report_bit1() {
        let (p, r) = event_to_press_release(HidEvent::ConsumerVolumeStep { up: false });
        assert_eq!(p, [0x01, 0b0000_0010]);
        assert_eq!(r, [0x01, 0x00]);
    }

    #[test]
    fn consumer_mute_bit2() {
        let (p, r) = event_to_press_release(HidEvent::ConsumerMute);
        assert_eq!(p, [0x01, 0b0000_0100]);
        assert_eq!(r, [0x01, 0x00]);
    }

    #[test]
    fn telephony_phone_mute_bit1_on_report_2() {
        let (p, r) = event_to_press_release(HidEvent::TelephonyPhoneMute);
        assert_eq!(p, [0x02, 0b0000_0010]);
        assert_eq!(r, [0x02, 0x00]);
    }

    #[test]
    fn play_pause_next_prev() {
        assert_eq!(event_to_press_release(HidEvent::ConsumerPlayPause).0, [0x01, 0b0000_1000]);
        assert_eq!(event_to_press_release(HidEvent::ConsumerNext).0,      [0x01, 0b0001_0000]);
        assert_eq!(event_to_press_release(HidEvent::ConsumerPrev).0,      [0x01, 0b0010_0000]);
    }
}
