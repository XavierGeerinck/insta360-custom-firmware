//! Output event type for the HID gadget — single ingress point that all
//! sources (MCU buttons, audio-state watchers) push to.

/// What we want the host to receive. The writer task serializes each one
/// into a press-release pair on `/dev/hidg0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidEvent {
    // Report 0x01 — Consumer page (press = bit set, release = bit cleared)
    ConsumerVolumeStep { up: bool },
    ConsumerMute,
    ConsumerPlayPause,
    ConsumerNext,
    ConsumerPrev,

    // Report 0x02 — Telephony page
    TelephonyPhoneMute,
}
