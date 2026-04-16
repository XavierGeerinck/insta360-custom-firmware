//! MockPcmSource for unit tests. Feeds pre-built frame batches and can
//! simulate read errors (EPIPE) at a programmed frame index.

use super::PcmSource;
use std::collections::VecDeque;
use std::io::{Error, ErrorKind};

pub struct MockPcmSource {
    channels: usize,
    sample_rate: u32,
    batches: VecDeque<std::io::Result<Vec<i32>>>,
}

impl MockPcmSource {
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        Self { channels, sample_rate, batches: VecDeque::new() }
    }

    pub fn with_batch(mut self, batch: Vec<i32>) -> Self {
        self.batches.push_back(Ok(batch));
        self
    }

    pub fn with_error(mut self, kind: ErrorKind, msg: &'static str) -> Self {
        self.batches.push_back(Err(Error::new(kind, msg)));
        self
    }
}

impl PcmSource for MockPcmSource {
    fn read(&mut self, buf: &mut [i32]) -> std::io::Result<usize> {
        match self.batches.pop_front() {
            Some(Ok(batch)) => {
                let n = batch.len().min(buf.len());
                buf[..n].copy_from_slice(&batch[..n]);
                Ok(n)
            }
            Some(Err(e)) => Err(e),
            None => Ok(0), // EOF
        }
    }

    fn channels(&self) -> usize { self.channels }
    fn sample_rate(&self) -> u32 { self.sample_rate }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_returns_batches_in_order() {
        let mut src = MockPcmSource::new(2, 48000)
            .with_batch(vec![1, 2, 3, 4])
            .with_batch(vec![5, 6, 7, 8]);
        let mut buf = vec![0i32; 4];
        assert_eq!(src.read(&mut buf).unwrap(), 4);
        assert_eq!(buf, vec![1, 2, 3, 4]);
        assert_eq!(src.read(&mut buf).unwrap(), 4);
        assert_eq!(buf, vec![5, 6, 7, 8]);
        assert_eq!(src.read(&mut buf).unwrap(), 0); // EOF
    }

    #[test]
    fn mock_propagates_errors() {
        let mut src = MockPcmSource::new(2, 48000)
            .with_batch(vec![1, 2])
            .with_error(ErrorKind::BrokenPipe, "simulated xrun");
        let mut buf = vec![0i32; 2];
        assert!(src.read(&mut buf).is_ok());
        let err = src.read(&mut buf).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::BrokenPipe);
    }
}
