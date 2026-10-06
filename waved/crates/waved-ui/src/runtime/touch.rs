//! Touch samples as the runtime loop reads them.

/// One touch reading in LVGL coordinates (already flipped; see `input.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TouchSample {
    pub x: i32,
    pub y: i32,
    pub pressed: bool,
}

/// How long after a failed open of the touch device the next try waits.
pub const OPEN_RETRY_MS: u64 = 2_000;

/// When to (re)open the touch device. Without touch the UI still renders
/// (the screen, the power key and the services all work); the loop asks
/// `due` each iteration while the device is closed, tries the open when it
/// says so, and reports the outcome. Pure, so it is host-tested.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenRetry {
    next_ms: u64,
    warned: bool,
}

impl OpenRetry {
    /// Time to try opening (the first time: at once).
    pub fn due(&self, now_ms: u64) -> bool {
        now_ms >= self.next_ms
    }

    /// An open failed: the next try is `OPEN_RETRY_MS` away. True when this
    /// failure should be logged, that is the first of an outage; the 2 s
    /// retries stay quiet.
    pub fn failed(&mut self, now_ms: u64) -> bool {
        self.next_ms = now_ms + OPEN_RETRY_MS;
        !std::mem::replace(&mut self.warned, true)
    }

    /// The device opened: a later outage warns again.
    pub fn opened(&mut self) {
        self.warned = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_open_retries_every_2_s_and_warns_once() {
        let mut retry = OpenRetry::default();
        assert!(retry.due(0), "the first try is at start-up");
        assert!(retry.failed(0), "the first failure is logged");
        assert!(!retry.due(1_999), "not before 2 s");
        assert!(retry.due(2_000));
        assert!(!retry.failed(2_000), "the retry's failure stays quiet");
        assert!(
            !retry.due(3_999),
            "2 s from the last try, not from the first"
        );
        assert!(retry.due(4_000));
        assert!(!retry.failed(4_010));
        assert!(!retry.due(6_009));
        assert!(retry.due(6_010));
    }

    #[test]
    fn opening_ends_the_outage() {
        let mut retry = OpenRetry::default();
        assert!(retry.failed(0));
        retry.opened();
        assert!(retry.failed(9_000), "a new outage is logged again");
    }

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
