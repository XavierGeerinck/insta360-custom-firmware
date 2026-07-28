//! Optional post-AEC microphone capture for diagnostics.
//!
//! Disabled unless `WAVED_MIC_CAPTURE_PATH` is set to a non-empty path.

#![cfg_attr(feature = "simulate", allow(dead_code))]

use std::ffi::OsString;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use tokio::sync::broadcast;
use tracing::{info, warn};
use waved_audio::processed_frame::ProcessedAudioFrame;

const PATH_ENV: &str = "WAVED_MIC_CAPTURE_PATH";

pub fn path_from_env() -> Option<PathBuf> {
    path_from_value(std::env::var_os(PATH_ENV))
}

fn path_from_value(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

pub fn spawn(path: PathBuf, rx: broadcast::Receiver<ProcessedAudioFrame>) {
    tokio::spawn(async move {
        let file = match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) => {
                warn!(path = %path.display(), %error, "diagnostic mic capture create failed");
                return;
            }
        };

        info!(path = %path.display(), "diagnostic mic capture enabled");
        if let Err(error) = record_frames(BufWriter::new(file), rx).await {
            warn!(path = %path.display(), %error, "diagnostic mic capture stopped");
        }
    });
}

async fn record_frames<W: Write>(
    mut writer: W,
    mut rx: broadcast::Receiver<ProcessedAudioFrame>,
) -> io::Result<()> {
    loop {
        match rx.recv().await {
            Ok(frame) => write_frame(&mut writer, &frame)?,
            Err(broadcast::error::RecvError::Lagged(count)) => {
                warn!(count, "diagnostic mic capture lagged frames");
            }
            Err(broadcast::error::RecvError::Closed) => return writer.flush(),
        }
    }
}

fn write_frame(writer: &mut impl Write, frame: &ProcessedAudioFrame) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(frame.mic_mono_f32.len() * 2);
    for &sample in &frame.mic_mono_f32 {
        let sample = (sample * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    writer.write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn missing_value_disables_capture() {
        assert_eq!(path_from_value(None), None);
    }

    #[test]
    fn empty_value_disables_capture() {
        assert_eq!(path_from_value(Some(OsString::new())), None);
    }

    #[test]
    fn non_empty_value_preserves_path() {
        assert_eq!(
            path_from_value(Some(OsString::from("/mnt/udisk/debug/mic.pcm"))),
            Some(PathBuf::from("/mnt/udisk/debug/mic.pcm")),
        );
    }

    struct FailingWriter {
        attempts: Arc<AtomicUsize>,
    }

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            Err(io::Error::other("forced failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn test_frame() -> ProcessedAudioFrame {
        ProcessedAudioFrame {
            mic_mono_f32: vec![0.25, -0.25],
            mic_stereo_f32: None,
            reference_f32: vec![0.0; 2],
            sample_rate: 48_000,
            timestamp_ms: 0,
            raw_mic_i16: Vec::new(),
            raw_mic_channels: 8,
            aec_metrics: Default::default(),
        }
    }

    #[tokio::test]
    async fn writer_failure_terminates_after_one_attempt() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let writer = FailingWriter {
            attempts: Arc::clone(&attempts),
        };
        let (tx, rx) = broadcast::channel(4);
        tx.send(test_frame()).unwrap();
        tx.send(test_frame()).unwrap();

        let error = record_frames(writer, rx).await.unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }
}
