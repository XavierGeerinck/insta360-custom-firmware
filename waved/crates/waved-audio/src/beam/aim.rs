//! Where the beamformer points in each audio mode (D2, C3).
//!
//! Directions are "device degrees", the same frame as the DoA azimuth. The UI
//! draws `doa_horizontal` clockwise from the top of the screen with no
//! conversion (`waved-ui` `kit/components/dial.rs`), so an aim set by
//! dragging on the Audio mode screen is used as is.

use waved_state::AudioMode;

/// Stereo keeps the device orientation: its two lobes sit at ±45° around the
/// front of the device and never follow talkers.
pub const STEREO_FORWARD: (f32, f32) = (0.0, 0.0);

/// The fixed (azimuth, elevation) for `mode`, or `None` when the mode has no
/// fixed direction. Whole room (Omni) has none, and Follow talker (Steered)
/// is driven by its own controller. `test_override` is the `SetDoaOverride`
/// hook used by scripts/test-beamforming.sh; it wins over aim and orientation.
pub fn fixed_direction(
    mode: &AudioMode,
    beam_aim_deg: f32,
    test_override: Option<(f32, f32)>,
) -> Option<(f32, f32)> {
    match mode {
        AudioMode::Omni | AudioMode::Steered { .. } => None,
        AudioMode::Stereo => Some(test_override.unwrap_or(STEREO_FORWARD)),
        AudioMode::Cardioid | AudioMode::Supercardioid | AudioMode::Figure8 => {
            Some(test_override.unwrap_or((beam_aim_deg, 0.0)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_direction_follows_the_mode_table() {
        use AudioMode::*;
        assert_eq!(fixed_direction(&Omni, 90.0, None), None);
        assert_eq!(
            fixed_direction(
                &Steered {
                    initial_az_deg: 30.0
                },
                90.0,
                None
            ),
            None
        );
        assert_eq!(
            fixed_direction(
                &Steered {
                    initial_az_deg: 30.0
                },
                90.0,
                Some((10.0, 0.0))
            ),
            None,
            "Follow talker has its own controller"
        );
        for mode in [Cardioid, Supercardioid, Figure8] {
            assert_eq!(
                fixed_direction(&mode, 270.0, None),
                Some((270.0, 0.0)),
                "{mode:?}"
            );
            assert_eq!(
                fixed_direction(&mode, 270.0, Some((45.0, 10.0))),
                Some((45.0, 10.0)),
                "the test override wins for {mode:?}"
            );
        }
        assert_eq!(fixed_direction(&Stereo, 270.0, None), Some(STEREO_FORWARD));
        assert_eq!(
            fixed_direction(&Stereo, 270.0, Some((45.0, 0.0))),
            Some((45.0, 0.0))
        );
    }
}
