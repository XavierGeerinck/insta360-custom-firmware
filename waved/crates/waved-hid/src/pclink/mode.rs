//! J4: stock mic modes 1–6 ↔ the six waved audio modes (D2).
//! docs/WAVE_CONTROLLER_PROTOCOL.md §8.
//!
//! | Stock | Stock meaning (SK algorithm) | waved |
//! |-------|------------------------------|-------|
//! | 1 | default meeting mode | Follow talker (`Steered`) |
//! | 2 | beam omnidirectional | Whole room (`Omni`) |
//! | 3 | beam cardioid | Focused (`Cardioid`) |
//! | 4 | beam super-cardioid | Narrow (`Supercardioid`) |
//! | 5 | beam figure-eight | Two sides (`Figure8`) |
//! | 6 | beam stereo | Stereo |

use waved_state::AudioMode;

/// The waved mode for stock mic mode `code`; `None` outside 1–6. A Follow
/// talker beam starts from `aim_deg`, the current fixed aim.
pub fn to_audio_mode(code: u8, aim_deg: f32) -> Option<AudioMode> {
    Some(match code {
        1 => AudioMode::Steered { initial_az_deg: aim_deg },
        2 => AudioMode::Omni,
        3 => AudioMode::Cardioid,
        4 => AudioMode::Supercardioid,
        5 => AudioMode::Figure8,
        6 => AudioMode::Stereo,
        _ => return None,
    })
}

/// The stock mic mode the app sees for `mode`.
pub fn to_stock(mode: &AudioMode) -> u8 {
    match mode {
        AudioMode::Steered { .. } => 1,
        AudioMode::Omni => 2,
        AudioMode::Cardioid => 3,
        AudioMode::Supercardioid => 4,
        AudioMode::Figure8 => 5,
        AudioMode::Stereo => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stock_mode_maps_and_round_trips() {
        let expected = [
            (1, AudioMode::Steered { initial_az_deg: 90.0 }),
            (2, AudioMode::Omni),
            (3, AudioMode::Cardioid),
            (4, AudioMode::Supercardioid),
            (5, AudioMode::Figure8),
            (6, AudioMode::Stereo),
        ];
        for (code, mode) in expected {
            assert_eq!(to_audio_mode(code, 90.0), Some(mode.clone()), "code {code}");
            assert_eq!(to_stock(&mode), code);
        }
    }

    #[test]
    fn codes_outside_1_to_6_are_refused() {
        for code in [0, 7, 8, 0xFF] {
            assert_eq!(to_audio_mode(code, 0.0), None, "code {code}");
        }
    }

    #[test]
    fn follow_talker_reports_1_whatever_its_aim() {
        assert_eq!(to_stock(&AudioMode::Steered { initial_az_deg: 271.0 }), 1);
    }
}
