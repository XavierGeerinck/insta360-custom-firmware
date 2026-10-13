//! Test doubles for the Bluetooth paths: scripted `hw:0,0` streams and a
//! speaker queue, all observable from the test.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::{CaptureStream, PhoneSpeaker, PlaybackStream, StreamError};
use crate::playback::PlaybackError;

/// What the fake device saw.
#[derive(Debug, Default)]
pub struct DeviceLog {
    pub opens: usize,
    pub closes: usize,
    pub recovers: usize,
    /// Every playback write, in order.
    pub writes: Vec<Vec<i16>>,
}

/// One fake `hw:0,0`: open results, capture reads and playback write results
/// are scripted; an empty script opens fine, reads `Again` and writes all.
#[derive(Clone, Default)]
pub struct FakeDevice {
    pub log: Arc<Mutex<DeviceLog>>,
    pub opens: Arc<Mutex<VecDeque<Result<(), String>>>>,
    pub reads: Arc<Mutex<VecDeque<Result<Vec<i16>, StreamError>>>>,
    pub write_results: Arc<Mutex<VecDeque<Result<(), StreamError>>>>,
    pub recover_results: Arc<Mutex<VecDeque<Result<(), String>>>>,
    pub channels: usize,
}

impl FakeDevice {
    pub fn new(channels: usize) -> Self {
        Self { channels, ..Self::default() }
    }

    pub fn fail_next_open(&self, why: &str) {
        self.opens.lock().unwrap().push_back(Err(why.into()));
    }

    pub fn push_read(&self, read: Result<Vec<i16>, StreamError>) {
        self.reads.lock().unwrap().push_back(read);
    }

    pub fn push_write_result(&self, result: Result<(), StreamError>) {
        self.write_results.lock().unwrap().push_back(result);
    }

    pub fn fail_next_recover(&self, why: &str) {
        self.recover_results.lock().unwrap().push_back(Err(why.into()));
    }

    pub fn opens(&self) -> usize {
        self.log.lock().unwrap().opens
    }

    pub fn closes(&self) -> usize {
        self.log.lock().unwrap().closes
    }

    pub fn recovers(&self) -> usize {
        self.log.lock().unwrap().recovers
    }

    pub fn writes(&self) -> Vec<Vec<i16>> {
        self.log.lock().unwrap().writes.clone()
    }

    fn open(&self) -> Result<FakeStream, String> {
        if let Some(Err(e)) = self.opens.lock().unwrap().pop_front() {
            return Err(e);
        }
        self.log.lock().unwrap().opens += 1;
        Ok(FakeStream { device: self.clone() })
    }

    /// An `open` closure for the capture side.
    pub fn capture_opener(&self) -> impl FnMut() -> Result<FakeStream, String> {
        let device = self.clone();
        move || device.open()
    }

    /// An `open` closure for the playback side.
    pub fn playback_opener(&self) -> impl FnMut() -> Result<FakeStream, String> {
        self.capture_opener()
    }
}

/// An open fake stream; dropping it closes it.
pub struct FakeStream {
    device: FakeDevice,
}

impl CaptureStream for FakeStream {
    fn read(&mut self, buf: &mut [i16]) -> Result<usize, StreamError> {
        match self.device.reads.lock().unwrap().pop_front() {
            None => Err(StreamError::Again),
            Some(Err(e)) => Err(e),
            Some(Ok(samples)) => {
                let n = samples.len().min(buf.len());
                buf[..n].copy_from_slice(&samples[..n]);
                Ok(n / self.device.channels.max(1))
            }
        }
    }

    fn recover(&mut self) -> Result<(), String> {
        self.device.log.lock().unwrap().recovers += 1;
        self.device.recover_results.lock().unwrap().pop_front().unwrap_or(Ok(()))
    }
}

impl PlaybackStream for FakeStream {
    fn write(&mut self, buf: &[i16]) -> Result<usize, StreamError> {
        if let Some(Err(e)) = self.device.write_results.lock().unwrap().pop_front() {
            return Err(e);
        }
        self.device.log.lock().unwrap().writes.push(buf.to_vec());
        Ok(buf.len() / self.device.channels.max(1))
    }

    fn recover(&mut self) -> Result<(), String> {
        CaptureStream::recover(self)
    }
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        self.device.log.lock().unwrap().closes += 1;
    }
}

/// The speaker's phone queue: records every period; `queued`, a full queue
/// and a stopped loop are settable.
#[derive(Default)]
pub struct FakeSpeaker {
    pub periods: Mutex<Vec<Vec<i16>>>,
    pub queued: Mutex<usize>,
    pub full: Mutex<bool>,
    pub stopped: Mutex<bool>,
    pub channels: usize,
    pub period_frames: usize,
}

impl FakeSpeaker {
    pub fn new(channels: usize, period_frames: usize) -> Arc<Self> {
        Arc::new(Self { channels, period_frames, ..Self::default() })
    }

    pub fn periods(&self) -> Vec<Vec<i16>> {
        self.periods.lock().unwrap().clone()
    }
}

impl PhoneSpeaker for FakeSpeaker {
    fn submit(&self, period: Vec<i16>) -> Result<(), PlaybackError> {
        if *self.stopped.lock().unwrap() {
            return Err(PlaybackError::Stopped);
        }
        if *self.full.lock().unwrap() {
            return Err(PlaybackError::Full);
        }
        self.periods.lock().unwrap().push(period);
        Ok(())
    }
    fn queued(&self) -> usize {
        *self.queued.lock().unwrap()
    }
    fn channels(&self) -> usize {
        self.channels
    }
    fn period_frames(&self) -> usize {
        self.period_frames
    }
}
