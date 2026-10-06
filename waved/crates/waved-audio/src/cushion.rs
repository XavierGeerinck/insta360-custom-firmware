//! The UAC2 uplink's start-up cushion (used by `uac::upstream`). Kept out of
//! the ALSA-gated `uac` module so it is tested on every host.

/// Periods of silence written ahead of the first frame whenever the stream
/// (re)starts. Without them the host drained the single queued period on any
/// processing hiccup (speech raises the denoiser's cost): 183 underruns, each
/// an audible gap, in 20 s of a streamed call on the device (2026-10-06).
/// Two periods (≈ 43 ms at 1024 frames) absorb that jitter.
pub const PREFILL_PERIODS: u32 = 2;

/// How many periods of silence to write before the next frame: the prefill
/// once after each (re)start, then none. Never more than leaves room for the
/// frame itself in a `buffer_periods` buffer.
#[derive(Debug)]
pub struct Cushion {
    pending: u32,
    max: u32,
}

impl Cushion {
    pub fn new(buffer_periods: u32) -> Self {
        let max = PREFILL_PERIODS.min(buffer_periods.saturating_sub(1));
        Self { pending: max, max }
    }

    /// The stream was prepared again (start-up or after an underrun).
    pub fn restarted(&mut self) {
        self.pending = self.max;
    }

    /// Silence periods to write now; the next call returns 0 until a restart.
    pub fn take(&mut self) -> u32 {
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cushion_prefills_once_per_start() {
        let mut cushion = Cushion::new(4);
        assert_eq!(cushion.take(), PREFILL_PERIODS, "first frame after open");
        assert_eq!(cushion.take(), 0, "steady state writes frames only");
        cushion.restarted();
        assert_eq!(cushion.take(), PREFILL_PERIODS, "again after an underrun");
        assert_eq!(cushion.take(), 0);
    }

    #[test]
    fn the_cushion_leaves_room_for_the_frame() {
        assert_eq!(Cushion::new(2).take(), 1);
        assert_eq!(Cushion::new(1).take(), 0);
        assert_eq!(Cushion::new(0).take(), 0);
    }
}
