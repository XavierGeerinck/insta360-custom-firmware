//! Directions on the dial.
//!
//! The audio side reports **device degrees**: 0° in front of the screen,
//! counter-clockwise, 90° on the listener's left (measured on the device
//! 2026-10-06: front ≈ 355°, left ≈ 92°, behind ≈ 183°, right ≈ 260°).
//! `doa_horizontal` and `beam_aim_deg` both use it.
//!
//! Components draw in **screen degrees**: 0° up, clockwise. The dial is a
//! top-down map seen by the listener in front of the screen: front at the
//! bottom, their left on the left. Convert at the snapshot/command boundary
//! with these two functions; components never see device degrees.

/// Screen degrees for a device direction.
pub fn screen_from_device(device_deg: f32) -> f32 {
    (device_deg + 180.0).rem_euclid(360.0)
}

/// Device degrees for a direction the user points at on screen.
pub fn device_from_screen(screen_deg: f32) -> f32 {
    (screen_deg + 180.0).rem_euclid(360.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_is_at_the_bottom_and_left_on_the_left() {
        assert_eq!(screen_from_device(0.0), 180.0, "front: bottom");
        assert_eq!(screen_from_device(90.0), 270.0, "listener's left: left");
        assert_eq!(screen_from_device(180.0), 0.0, "behind: top");
        assert_eq!(screen_from_device(270.0), 90.0, "listener's right: right");
        // The device-pass report: talking front-left must not show back-right.
        assert_eq!(screen_from_device(45.0), 225.0, "front-left: lower left");
    }

    #[test]
    fn the_conversions_undo_each_other() {
        for deg in [0.0, 12.5, 90.0, 179.0, 181.0, 359.5] {
            assert!((device_from_screen(screen_from_device(deg)) - deg).abs() < 1e-3);
            assert!((screen_from_device(device_from_screen(deg)) - deg).abs() < 1e-3);
        }
        assert_eq!(screen_from_device(-90.0), 90.0);
        assert_eq!(screen_from_device(540.0), 0.0);
    }
}
