//! The talker gate: which direction on the dial counts as "someone is
//! talking".
//!
//! The audio service's DoA VAD flag (`audio.doa_active`) reads open in almost
//! every sample of a quiet room, so screens gate on `audio.doa_confidence`
//! instead, and add hysteresis on top: a talker shows after `ON_MS` of
//! confident samples and goes after `OFF_MS` without one. The runtime hands
//! screens a snapshot every loop iteration (about every 10 ms), far faster
//! than the audio service moves its direction, so the delays are in time,
//! not in samples.

/// Same threshold as `waved_audio::beam::steered::CONFIDENCE_THRESHOLD`,
/// the level at which Follow talker itself turns the beam.
pub const CONFIDENCE_MIN: f32 = 0.30;
/// Confident for this long before a talker shows.
pub const ON_MS: u64 = 150;
/// Not confident for this long before the talker goes.
pub const OFF_MS: u64 = 100;

/// Samples further apart than this do not count as one stretch.
const MAX_GAP_MS: u64 = 250;

/// Whether this snapshot's direction is trustworthy enough to follow.
pub fn is_confident(confidence: f32) -> bool {
    confidence >= CONFIDENCE_MIN
}

/// Hysteresis for one screen. `step` takes the direction while the sample is
/// confident (`None` otherwise) and returns the direction to show, held
/// through short dropouts.
#[derive(Clone, Copy, Debug, Default)]
pub struct TalkerGate {
    shown: bool,
    /// The last confident direction (device or screen degrees, as given).
    held: f32,
    /// Since when the samples have agreed with `confident`, and the last one.
    run: Option<(bool, u64)>,
    last_ms: u64,
}

impl TalkerGate {
    pub fn step(&mut self, direction: Option<f32>, now_ms: u64) -> Option<f32> {
        let confident = direction.is_some();
        // A screen that was not stepped for a while (parked, or covered by a
        // gauge) has no history at all: nothing shows until a fresh stretch of
        // confident samples has passed the on-delay.
        if now_ms.saturating_sub(self.last_ms) > MAX_GAP_MS {
            self.run = None;
            self.shown = false;
        }
        self.last_ms = now_ms;
        let since = match self.run {
            Some((state, since)) if state == confident => since,
            _ => {
                self.run = Some((confident, now_ms));
                now_ms
            }
        };
        let steady = now_ms.saturating_sub(since);
        match direction {
            Some(d) => {
                self.held = d;
                if steady >= ON_MS {
                    self.shown = true;
                }
            }
            None if steady >= OFF_MS => self.shown = false,
            None => {}
        }
        self.shown.then_some(self.held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steps the gate every 10 ms like the runtime loop, `samples` times.
    fn run(gate: &mut TalkerGate, t: &mut u64, direction: Option<f32>, ms: u64) -> Option<f32> {
        let mut shown = None;
        for _ in 0..ms / 10 {
            *t += 10;
            shown = gate.step(direction, *t);
        }
        shown
    }

    #[test]
    fn confidence_is_the_steered_threshold() {
        assert!(!is_confident(0.29));
        assert!(is_confident(0.30));
    }

    #[test]
    fn a_talker_shows_after_the_on_delay_and_not_before() {
        let (mut gate, mut t) = (TalkerGate::default(), 1_000);
        assert_eq!(run(&mut gate, &mut t, Some(40.0), ON_MS), None, "too soon");
        assert_eq!(run(&mut gate, &mut t, Some(40.0), 10), Some(40.0));
        assert_eq!(
            run(&mut gate, &mut t, Some(55.0), 10),
            Some(55.0),
            "it follows the talker"
        );
    }

    #[test]
    fn room_noise_flicker_never_shows() {
        let (mut gate, mut t) = (TalkerGate::default(), 0);
        for _ in 0..200 {
            // Confident for 100 ms, then 20 ms of nothing: never 150 ms in a row.
            assert_eq!(run(&mut gate, &mut t, Some(90.0), 100), None);
            assert_eq!(run(&mut gate, &mut t, None, 20), None);
        }
    }

    #[test]
    fn a_steady_talker_keeps_the_wedge_through_a_dropout() {
        let (mut gate, mut t) = (TalkerGate::default(), 0);
        run(&mut gate, &mut t, Some(40.0), 500);
        assert_eq!(
            run(&mut gate, &mut t, None, 50),
            Some(40.0),
            "held at the last direction"
        );
        assert_eq!(run(&mut gate, &mut t, Some(42.0), 10), Some(42.0));
        // A longer silence takes it away, and it needs the on-delay again.
        assert_eq!(run(&mut gate, &mut t, None, OFF_MS + 10), None);
        assert_eq!(run(&mut gate, &mut t, Some(42.0), ON_MS), None);
    }

    #[test]
    fn a_gap_in_updates_is_not_a_confident_stretch() {
        // A parked screen is not stepped; the first sample after the gap starts the count.
        let mut gate = TalkerGate::default();
        gate.step(Some(10.0), 0);
        assert_eq!(gate.step(Some(10.0), 60_000), None);
        assert_eq!(gate.step(Some(10.0), 60_000 + ON_MS), Some(10.0));
    }

    #[test]
    fn a_wedge_showing_before_a_gap_does_not_come_back_without_the_on_delay() {
        let (mut gate, mut t) = (TalkerGate::default(), 0);
        assert_eq!(
            run(&mut gate, &mut t, Some(40.0), 500),
            Some(40.0),
            "showing"
        );
        // The screen is parked for a minute, then updated again.
        t += 60_000;
        assert_eq!(
            gate.step(Some(40.0), t),
            None,
            "a confident sample alone shows nothing"
        );
        assert_eq!(run(&mut gate, &mut t, Some(40.0), ON_MS - 10), None);
        assert_eq!(
            run(&mut gate, &mut t, Some(40.0), 20),
            Some(40.0),
            "after the on-delay"
        );
        // And a non-confident first sample after a gap does not show a stale wedge either.
        let mut gate = TalkerGate::default();
        run(&mut gate, &mut t, Some(40.0), 500);
        t += 60_000;
        assert_eq!(gate.step(None, t), None);
    }
}
