//! Touch samples as the runtime loop reads them.

/// One touch reading in LVGL coordinates (already flipped; see `input.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TouchSample {
    pub x: i32,
    pub y: i32,
    pub pressed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_touch_is_a_released_sample() {
        let s = TouchSample::default();
        assert!(!s.pressed);
        assert_eq!(
            s,
            TouchSample {
                x: 0,
                y: 0,
                pressed: false
            }
        );
    }
}
