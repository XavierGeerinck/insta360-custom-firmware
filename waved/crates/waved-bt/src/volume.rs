//! The phone's volume on the Wave's speaker (`0xD5`), mapped as stock maps
//! it. docs/BLUETOOTH_PROTOCOL.md, "Volume".
//!
//! **Stock 1.2.2.6** (libins_entry.so.c):
//!
//! - `0xD5 [kind] [scale] [level]` → `sync_phone_volume` (74219): scale 0
//!   means 15 steps, 1 means 16; the level becomes 0–100 through
//!   `map_volume_100` (74146) and is stored as `BT_MUSIC_VOLUME` (kind 0,
//!   status 0x89) or `BT_PHONE_VOLUME` (kind 1, 0x8a). Any other scale maps
//!   to 0.
//! - The audio server listens to both (`g_audio_client_register_param`,
//!   `.data` 0x284d08 of `libins_entry.so`): 0x89 →
//!   `audio_listen_bt_spk_volume_event` (73324), 0x8a →
//!   `audio_listen_bt_mic_volume_event` (73395).
//! - The music volume becomes the speaker volume:
//!   `audio_server_spk_volume_policy` (71893) with source 2 (Bluetooth) and
//!   type 1 (set), taken only while the speaker plays the phone or the dongle
//!   (input path `BT_IN` or `DONGLE_IN`). 0 mutes the speaker, anything else
//!   unmutes it; the volume is saved like any other.
//! - The call volume is only logged ("bt mic volume is %d").
//!
//! **waved** maps both kinds with stock's `map_volume_100`, while the phone's
//! audio plays on the speaker (`0xB2` bit 1 or 2, waved's equivalent of
//! stock's `BT_IN` path): a phone's volume keys work in a call too (stock
//! ignores them there). A level of 0 sets the volume to 0 without muting;
//! a level above 0 unmutes, as stock does, at no less than waved-audio's
//! 10 % unmute floor ([`speaker_commands`]). The phone's changes never reach
//! the computer: `Speaker` marks the volume and the unmute for the HID
//! watches before sending them. An unknown scale is ignored rather than
//! muting. Nothing goes back to the phone when the Wave's own volume changes
//! (see the doc: stock's `0xD0` volume events are unverified on this module).

use crate::event::VolumeKind;
use waved_state::{BluetoothState, Command as BusCommand};

/// Stock `map_volume_100(volume, max_volume, 100)`: the level as a percent of
/// `max`, then rounded down to stock's steps: 0, 5 (1–10), 10 (11–19), 20,
/// 30 … 90, 100. A `max` of 0 gives 0.
pub fn map_volume_100(level: u8, max: u8) -> u8 {
    let level = u32::from(level.min(max));
    let percent = if max == 0 { 0 } else { level * 100 / u32::from(max) };
    match percent {
        0 => 0,
        1..=10 => 5,
        11..=19 => 10,
        20..=99 => (percent / 10 * 10) as u8,
        _ => 100,
    }
}

/// The speaker volume (0–100) a `0xD5` report asks for, or `None` when it
/// does not apply: Bluetooth off, the phone's audio not on the speaker, an
/// unknown kind or scale.
pub fn speaker_target(kind: VolumeKind, level: u8, max: u8, s: &BluetoothState) -> Option<u8> {
    let playing = s.phone_audio || s.call_audio;
    match kind {
        VolumeKind::Music | VolumeKind::Call if s.bt_on && playing && max > 0 => {
            Some(map_volume_100(level, max))
        }
        _ => None,
    }
}

/// waved-audio's unmute floor (`AudioService::set_speaker_muted`): unmuting
/// a speaker below 10 % raises it to 10 %.
pub const UNMUTE_FLOOR: u8 = 10;

/// The bus commands that bring the speaker from `volume` / `muted` to
/// `target`: the volume, then the unmute, so each changes the speaker once
/// and `Speaker` can mark both for the HID watches. A muted speaker unmutes
/// at [`UNMUTE_FLOOR`] at least: a 5 % target from muted ends at 10 %
/// (unmuting first and then setting 5 % would pass through 10 %, a change
/// no marker covers, and the computer would get a volume key). Nothing when
/// it is already there.
pub fn speaker_commands(target: u8, volume: u16, muted: bool) -> Vec<BusCommand> {
    let unmute = target > 0 && muted;
    let target = u16::from(if unmute { target.max(UNMUTE_FLOOR) } else { target });
    let mut out = Vec::new();
    if volume != target {
        out.push(BusCommand::SetSpeakerVolume(target));
    }
    if unmute {
        out.push(BusCommand::SetSpeakerMute(false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_map_to_stocks_steps() {
        // 15 steps (scale 0): what an AVRCP / HFP volume of n / 15 becomes.
        let fifteen: Vec<u8> = (0..=15).map(|n| map_volume_100(n, 15)).collect();
        assert_eq!(fifteen, [0, 5, 10, 20, 20, 30, 40, 40, 50, 60, 60, 70, 80, 80, 90, 100]);
        let sixteen: Vec<u8> = (0..=16).map(|n| map_volume_100(n, 16)).collect();
        assert_eq!(sixteen, [0, 5, 10, 10, 20, 30, 30, 40, 50, 50, 60, 60, 70, 80, 80, 90, 100]);
        // Stock clamps to the maximum and gives 0 for an unknown scale.
        assert_eq!(map_volume_100(200, 15), 100);
        assert_eq!(map_volume_100(7, 0), 0);
        // The edges of each step.
        assert_eq!(map_volume_100(10, 100), 5);
        assert_eq!(map_volume_100(11, 100), 10);
        assert_eq!(map_volume_100(19, 100), 10);
        assert_eq!(map_volume_100(20, 100), 20);
        assert_eq!(map_volume_100(99, 100), 90);
    }

    fn playing() -> BluetoothState {
        BluetoothState { bt_on: true, connected: true, phone_audio: true, ..Default::default() }
    }

    #[test]
    fn music_and_call_volumes_apply_while_the_phone_plays() {
        let s = playing();
        assert_eq!(speaker_target(VolumeKind::Music, 8, 15, &s), Some(50));
        assert_eq!(speaker_target(VolumeKind::Call, 15, 15, &s), Some(100));
        let call = BluetoothState { call_audio: true, phone_audio: false, ..playing() };
        assert_eq!(speaker_target(VolumeKind::Call, 4, 16, &call), Some(20));
        assert_eq!(speaker_target(VolumeKind::Music, 0, 16, &call), Some(0));
    }

    #[test]
    fn nothing_applies_when_the_speaker_does_not_play_the_phone() {
        // Stock: "not bt or dongle path, not need sync volume".
        let idle = BluetoothState { phone_audio: false, ..playing() };
        assert_eq!(speaker_target(VolumeKind::Music, 8, 15, &idle), None);
        let off = BluetoothState { bt_on: false, ..playing() };
        assert_eq!(speaker_target(VolumeKind::Music, 8, 15, &off), None);
        assert_eq!(speaker_target(VolumeKind::Other(2), 8, 15, &playing()), None);
        // An unknown scale (decoded as max 0) would mute on stock: ignored.
        assert_eq!(speaker_target(VolumeKind::Music, 8, 0, &playing()), None);
    }

    #[test]
    fn the_speaker_gets_the_volume_then_is_unmuted() {
        use BusCommand::{SetSpeakerMute, SetSpeakerVolume};
        assert_eq!(speaker_commands(50, 80, false), [SetSpeakerVolume(50)]);
        assert!(speaker_commands(50, 50, false).is_empty(), "already there");
        assert_eq!(speaker_commands(60, 60, true), [SetSpeakerMute(false)]);
        assert_eq!(speaker_commands(60, 30, true), [SetSpeakerVolume(60), SetSpeakerMute(false)]);
        // Unmuting below 10 % raises the volume to 10 % (waved-audio): from
        // muted, 5 % unmutes at 10 %, one change of each.
        assert_eq!(speaker_commands(5, 5, true), [SetSpeakerVolume(10), SetSpeakerMute(false)]);
        assert_eq!(speaker_commands(5, 10, true), [SetSpeakerMute(false)]);
        assert_eq!(speaker_commands(5, 40, false), [SetSpeakerVolume(5)], "unmuted: 5 % as asked");
        // 0 sets the volume to 0 and leaves the mute alone.
        assert_eq!(speaker_commands(0, 40, false), [SetSpeakerVolume(0)]);
        assert_eq!(speaker_commands(0, 40, true), [SetSpeakerVolume(0)]);
        assert!(speaker_commands(0, 0, false).is_empty());
    }
}
