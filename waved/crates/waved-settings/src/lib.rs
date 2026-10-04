//! Owns the user-facing settings: seeds live state from config.toml at boot
//! and persists changes made on the device back to it.

mod change;

pub use change::{apply_to_config, apply_to_settings, Change};

use waved_config::WavedConfig;
use waved_state::{SettingsState, StateBus};

/// User-facing settings derived from config. `pin_enabled` mirrors whether a
/// device password is set.
pub fn settings_from_config(config: &WavedConfig) -> SettingsState {
    SettingsState {
        brightness_pct: config.display.brightness_pct,
        dim_after_s: config.display.dim_after_s,
        off_after_s: config.display.off_after_s,
        time_zone: config.time.zone.clone(),
        clock_24h: config.time.clock_24h,
        pre_recording: config.recording.pre_recording,
        ask_timeout_s: config.recording.ask_timeout_s,
        upload_mode: config.api.upload_mode,
        tones_enabled: config.tones.enabled,
        first_run_done: config.device.first_run_done,
        pin_enabled: !config.security.device_password.is_empty(),
    }
}

/// Copy persisted values into live state. Call before any service is spawned:
/// AudioService reads mode and noise reduction from the watch when it is built.
pub fn seed_state(bus: &StateBus, config: &WavedConfig) {
    bus.audio.send_modify(|audio| {
        audio.mode = config.audio.mode.clone();
        audio.nr_level = config.audio.nr_level.clone();
        audio.speaker_volume = config.audio.speaker_volume;
        audio.mic_volume = config.audio.mic_volume;
        audio.beam_aim_deg = config.audio.beam_aim_deg;
    });
    bus.settings.send_replace(settings_from_config(config));
}

#[cfg(test)]
mod tests {
    use super::*;
    use waved_state::{AudioMode, NrLevel, UploadMode};

    fn configured() -> WavedConfig {
        let mut config = WavedConfig::default();
        config.audio.mode = AudioMode::Cardioid;
        config.audio.nr_level = NrLevel::High;
        config.audio.speaker_volume = 40;
        config.audio.mic_volume = 300;
        config.audio.beam_aim_deg = 270.0;
        config.display.brightness_pct = 55;
        config.time.zone = "Europe/Brussels".into();
        config.recording.pre_recording = false;
        config.api.upload_mode = UploadMode::Ask;
        config.device.first_run_done = true;
        config.security.device_password = "4821".into();
        config
    }

    #[test]
    fn settings_mirror_the_config() {
        let settings = settings_from_config(&configured());
        assert_eq!(settings.brightness_pct, 55);
        assert_eq!(settings.time_zone, "Europe/Brussels");
        assert!(!settings.pre_recording);
        assert_eq!(settings.upload_mode, UploadMode::Ask);
        assert!(settings.first_run_done && settings.pin_enabled);
    }

    #[test]
    fn seed_copies_audio_and_settings_into_the_bus() {
        let bus = StateBus::new();
        seed_state(&bus, &configured());
        let audio = bus.audio.borrow().clone();
        assert_eq!(audio.mode, AudioMode::Cardioid);
        assert_eq!(audio.nr_level, NrLevel::High);
        assert_eq!((audio.speaker_volume, audio.mic_volume), (40, 300));
        assert_eq!(audio.beam_aim_deg, 270.0);
        assert_eq!(bus.settings.borrow().brightness_pct, 55);
    }
}
