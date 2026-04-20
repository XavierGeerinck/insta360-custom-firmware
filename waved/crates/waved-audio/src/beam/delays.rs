//! Fractional-sample delay line via linear interpolation.
//!
//! Each channel gets its own ring buffer. Max delay our geometry demands
//! is ~7 samples at 48 kHz; we size the buffer at 32 samples to give
//! comfortable headroom and stay cache-friendly.

const BUFFER_LEN: usize = 32;

/// Single-channel fractional delay. The current sample is pushed via
/// `write(x)`; the delayed sample at delay `d` (in samples, fractional)
/// is read via `read(d)`. `d` must be in [0.0, BUFFER_LEN - 1].
pub struct DelayLine {
    buf: [f32; BUFFER_LEN],
    write_idx: usize,
}

impl DelayLine {
    pub fn new() -> Self {
        Self {
            buf: [0.0; BUFFER_LEN],
            write_idx: 0,
        }
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write_idx = 0;
    }

    pub fn write(&mut self, sample: f32) {
        self.buf[self.write_idx] = sample;
        self.write_idx = (self.write_idx + 1) % BUFFER_LEN;
    }

    /// Read delayed sample. `delay_samples` may be fractional.
    /// Returns 0.0 if delay exceeds buffer capacity.
    pub fn read(&self, delay_samples: f32) -> f32 {
        if delay_samples < 0.0 || delay_samples >= (BUFFER_LEN - 1) as f32 {
            return 0.0;
        }
        let int_d = delay_samples.floor() as usize;
        let frac = delay_samples - int_d as f32;

        let idx_a = (self.write_idx + BUFFER_LEN - 1 - int_d) % BUFFER_LEN;
        let idx_b = (self.write_idx + BUFFER_LEN - 2 - int_d) % BUFFER_LEN;

        let a = self.buf[idx_a];
        let b = self.buf[idx_b];
        a * (1.0 - frac) + b * frac
    }
}

impl Default for DelayLine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_delay_reads_most_recent_sample() {
        let mut d = DelayLine::new();
        d.write(1.0);
        d.write(2.0);
        d.write(3.0);
        assert!((d.read(0.0) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn integer_delay_reads_older_sample() {
        let mut d = DelayLine::new();
        for i in 0..10 {
            d.write(i as f32);
        }
        assert!((d.read(2.0) - 7.0).abs() < 1e-6);
        assert!((d.read(5.0) - 4.0).abs() < 1e-6);
    }

    #[test]
    fn fractional_delay_interpolates_linearly() {
        let mut d = DelayLine::new();
        d.write(10.0);
        d.write(20.0);
        let y = d.read(0.5);
        assert!((y - 15.0).abs() < 1e-6, "expected 15.0, got {y}");
    }

    #[test]
    fn out_of_range_delay_reads_zero() {
        let mut d = DelayLine::new();
        d.write(1.0);
        assert_eq!(d.read(-0.1), 0.0);
        assert_eq!(d.read(100.0), 0.0);
    }

    #[test]
    fn reset_clears_buffer() {
        let mut d = DelayLine::new();
        for i in 0..5 {
            d.write(i as f32);
        }
        d.reset();
        assert_eq!(d.read(0.0), 0.0);
        assert_eq!(d.read(2.0), 0.0);
    }
}
