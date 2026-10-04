//! Valid ranges for the settings the device itself changes. Both a config file
//! from another version or a hand edit (on load) and commands from the host or
//! the UI (in waved-settings) are clamped with these rules.

use crate::WavedConfig;

/// Screen brightness in percent, 5..=100.
pub fn clamp_brightness(percent: u8) -> u8 {
    percent.clamp(5, 100)
}

/// Seconds before the resting screen: 0 (never) or 15..=3600.
pub fn clamp_dim_after(seconds: u32) -> u32 {
    if seconds == 0 {
        0
    } else {
        seconds.clamp(15, 3_600)
    }
}

/// Seconds before the screen turns off: 0 (never) or 60..=86400, and never
/// before dimming; when it would be, it moves out to match.
pub fn clamp_off_after(dim_after_s: u32, off_after_s: u32) -> u32 {
    if off_after_s == 0 {
        return 0;
    }
    let off = off_after_s.clamp(60, 86_400);
    if dim_after_s != 0 && off < dim_after_s {
        dim_after_s.clamp(60, 86_400)
    } else {
        off
    }
}

/// Seconds before an unanswered "Send this recording?" keeps the file, 15..=600.
pub fn clamp_ask_timeout(seconds: u32) -> u32 {
    seconds.clamp(15, 600)
}

/// Speaker volume in percent, 0..=100.
pub fn clamp_speaker_volume(percent: u16) -> u16 {
    percent.min(100)
}

/// Linear microphone gain in percent, 0..=800.
pub fn clamp_mic_volume(percent: u16) -> u16 {
    percent.min(800)
}

/// Beam aim wrapped into [0, 360). `None` when it is not a finite number.
pub fn wrap_beam_aim(degrees: f32) -> Option<f32> {
    if !degrees.is_finite() {
        return None;
    }
    let wrapped = degrees.rem_euclid(360.0);
    // rem_euclid can return 360.0 (tiny negatives) or -0.0.
    Some(if wrapped <= 0.0 || wrapped >= 360.0 {
        0.0
    } else {
        wrapped
    })
}

/// Clamp every device-side value into its range; a non-finite aim becomes 0.
/// Returns true when anything changed.
pub(crate) fn normalise(config: &mut WavedConfig) -> bool {
    let before = config.clone();
    let display = &mut config.display;
    display.brightness_pct = clamp_brightness(display.brightness_pct);
    display.dim_after_s = clamp_dim_after(display.dim_after_s);
    display.off_after_s = clamp_off_after(display.dim_after_s, display.off_after_s);
    config.recording.ask_timeout_s = clamp_ask_timeout(config.recording.ask_timeout_s);
    let audio = &mut config.audio;
    audio.speaker_volume = clamp_speaker_volume(audio.speaker_volume);
    audio.mic_volume = clamp_mic_volume(audio.mic_volume);
    audio.beam_aim_deg = wrap_beam_aim(audio.beam_aim_deg).unwrap_or(0.0);
    *config != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_off_never_comes_before_dimming() {
        assert_eq!(clamp_off_after(600, 300), 600);
        assert_eq!(clamp_off_after(30, 10), 60);
        assert_eq!(clamp_off_after(0, 10), 60);
        assert_eq!(clamp_off_after(600, 0), 0);
        assert_eq!(clamp_off_after(120, 600), 600);
    }

    #[test]
    fn beam_aim_wraps_to_positive_zero() {
        for degrees in [-0.0_f32, -360.0, 360.0, -1e-9, 720.0] {
            let aim = wrap_beam_aim(degrees).unwrap();
            assert!(aim == 0.0 && aim.is_sign_positive(), "{degrees} -> {aim}");
        }
        assert_eq!(wrap_beam_aim(-60.0), Some(300.0));
        assert_eq!(wrap_beam_aim(f32::NAN), None);
        assert_eq!(wrap_beam_aim(f32::INFINITY), None);
    }

    #[test]
    fn a_valid_config_is_left_alone() {
        let mut config = WavedConfig::default();
        assert!(!normalise(&mut config));
        assert_eq!(config, WavedConfig::default());
    }
}
