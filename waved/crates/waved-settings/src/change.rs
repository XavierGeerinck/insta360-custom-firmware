//! One persisted setting, mapped from a bus command and clamped to its range.

use waved_config::WavedConfig;
use waved_state::{AudioMode, Command, NrLevel, SettingsState, UploadMode};

#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Brightness(u8),
    DimAfter(u32),
    TimeZone(String),
    Clock24h(bool),
    PreRecording(bool),
    UploadMode(UploadMode),
    Tones(bool),
    FirstRunDone,
    AudioMode(AudioMode),
    NrLevel(NrLevel),
    SpeakerVolume(u16),
    MicVolume(u16),
    BeamAim(f32),
}

impl Change {
    /// The setting a command changes, clamped to its valid range. `None` for
    /// commands that don't touch settings, invalid time zones and non-finite aims.
    pub fn from_command(command: &Command) -> Option<Self> {
        Some(match command {
            Command::SetBrightness(value) => Change::Brightness((*value).clamp(5, 100)),
            Command::SetDimAfter(0) => Change::DimAfter(0),
            Command::SetDimAfter(value) => Change::DimAfter((*value).clamp(15, 3_600)),
            Command::SetTimeZone(zone) if waved_config::valid_time_zone(zone) => {
                Change::TimeZone(zone.clone())
            }
            Command::SetClock24h(value) => Change::Clock24h(*value),
            Command::SetPreRecording(value) => Change::PreRecording(*value),
            Command::SetUploadMode(mode) => Change::UploadMode(*mode),
            Command::SetAlertTones(value) => Change::Tones(*value),
            Command::MarkFirstRunDone => Change::FirstRunDone,
            Command::SetAudioMode(mode) => Change::AudioMode(mode.clone()),
            Command::SetNrLevel(level) => Change::NrLevel(level.clone()),
            Command::SetSpeakerVolume(value) => Change::SpeakerVolume((*value).min(100)),
            Command::SetMicVolume(value) => Change::MicVolume((*value).min(800)),
            Command::SetBeamAim(degrees) if degrees.is_finite() => {
                let wrapped = degrees.rem_euclid(360.0);
                Change::BeamAim(if wrapped >= 360.0 { 0.0 } else { wrapped })
            }
            _ => return None,
        })
    }
}

pub fn apply_to_config(config: &mut WavedConfig, change: &Change) {
    match change {
        Change::Brightness(value) => config.display.brightness_pct = *value,
        Change::DimAfter(value) => {
            config.display.dim_after_s = *value;
            let off = config.display.off_after_s;
            if *value != 0 && off != 0 && off < *value {
                config.display.off_after_s = (*value).max(60);
            }
        }
        Change::TimeZone(zone) => config.time.zone.clone_from(zone),
        Change::Clock24h(value) => config.time.clock_24h = *value,
        Change::PreRecording(value) => config.recording.pre_recording = *value,
        Change::UploadMode(mode) => config.api.upload_mode = *mode,
        Change::Tones(value) => config.tones.enabled = *value,
        Change::FirstRunDone => config.device.first_run_done = true,
        Change::AudioMode(mode) => config.audio.mode = mode.clone(),
        Change::NrLevel(level) => config.audio.nr_level = level.clone(),
        Change::SpeakerVolume(value) => config.audio.speaker_volume = *value,
        Change::MicVolume(value) => config.audio.mic_volume = *value,
        Change::BeamAim(degrees) => config.audio.beam_aim_deg = *degrees,
    }
}

/// Audio changes are owned by AudioService (AudioState) and leave settings alone.
pub fn apply_to_settings(settings: &mut SettingsState, change: &Change) {
    match change {
        Change::Brightness(value) => settings.brightness_pct = *value,
        Change::DimAfter(value) => {
            settings.dim_after_s = *value;
            if *value != 0 && settings.off_after_s != 0 && settings.off_after_s < *value {
                settings.off_after_s = (*value).max(60);
            }
        }
        Change::TimeZone(zone) => settings.time_zone.clone_from(zone),
        Change::Clock24h(value) => settings.clock_24h = *value,
        Change::PreRecording(value) => settings.pre_recording = *value,
        Change::UploadMode(mode) => settings.upload_mode = *mode,
        Change::Tones(value) => settings.tones_enabled = *value,
        Change::FirstRunDone => settings.first_run_done = true,
        Change::AudioMode(_)
        | Change::NrLevel(_)
        | Change::SpeakerVolume(_)
        | Change::MicVolume(_)
        | Change::BeamAim(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_range_commands_are_clamped_or_ignored() {
        assert_eq!(Change::from_command(&Command::SetBrightness(0)), Some(Change::Brightness(5)));
        assert_eq!(Change::from_command(&Command::SetBrightness(250)), Some(Change::Brightness(100)));
        assert_eq!(Change::from_command(&Command::SetSpeakerVolume(250)), Some(Change::SpeakerVolume(100)));
        assert_eq!(Change::from_command(&Command::SetMicVolume(5_000)), Some(Change::MicVolume(800)));
        assert_eq!(Change::from_command(&Command::SetDimAfter(5)), Some(Change::DimAfter(15)));
        assert_eq!(Change::from_command(&Command::SetDimAfter(0)), Some(Change::DimAfter(0)));
        assert_eq!(Change::from_command(&Command::SetBeamAim(-60.0)), Some(Change::BeamAim(300.0)));
        assert_eq!(Change::from_command(&Command::SetBeamAim(720.0)), Some(Change::BeamAim(0.0)));
        assert_eq!(Change::from_command(&Command::SetBeamAim(f32::NAN)), None);
        assert_eq!(Change::from_command(&Command::SetTimeZone("../etc".into())), None);
        assert_eq!(Change::from_command(&Command::ToggleMicMute), None);
    }

    #[test]
    fn longer_dimming_pushes_screen_off_later() {
        let mut config = WavedConfig::default();
        config.display.off_after_s = 300;
        apply_to_config(&mut config, &Change::DimAfter(600));
        assert_eq!(config.display.dim_after_s, 600);
        assert_eq!(config.display.off_after_s, 600, "never off before dimming");
        assert!(waved_config::validate_config(&config).is_ok());
    }

    #[test]
    fn audio_changes_touch_config_but_not_settings() {
        let mut config = WavedConfig::default();
        let mut settings = SettingsState::default();
        let change = Change::SpeakerVolume(30);
        apply_to_config(&mut config, &change);
        apply_to_settings(&mut settings, &change);
        assert_eq!(config.audio.speaker_volume, 30);
        assert_eq!(settings, SettingsState::default());
    }

    #[test]
    fn settings_changes_reach_both() {
        let mut config = WavedConfig::default();
        let mut settings = SettingsState::default();
        for change in [
            Change::UploadMode(UploadMode::LocalOnly),
            Change::PreRecording(false),
            Change::FirstRunDone,
        ] {
            apply_to_config(&mut config, &change);
            apply_to_settings(&mut settings, &change);
        }
        assert_eq!(config.api.upload_mode, UploadMode::LocalOnly);
        assert_eq!(settings.upload_mode, UploadMode::LocalOnly);
        assert!(!config.recording.pre_recording && !settings.pre_recording);
        assert!(config.device.first_run_done && settings.first_run_done);
    }
}
