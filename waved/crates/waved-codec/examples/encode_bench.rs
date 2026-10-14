//! Host benchmark for the recording encoders: speed and size per hour.
//!
//!     cargo run --release -p waved-codec --example encode_bench [minutes]
//!
//! Encodes speech-like test audio (`testsignal::speech_like`, room floor
//! around −60 dBFS) and the same with a louder −45 dBFS floor, through the
//! same writers and file buffering the recorder uses, in 10 ms chunks. The
//! A55 estimate in docs/AUDIO_HARDWARE.md scales these numbers. Times are
//! this thread's CPU time, so a busy host does not skew them.

use std::fs::File;
use waved_codec::testsignal::speech_like;
use waved_codec::{create_writer, open_reader, read_to_end, AudioFormat, SAMPLE_RATE};

/// CPU seconds this thread has used: unlike wall time, other load on the
/// host does not count.
fn cpu_seconds() -> f64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid timespec for the call to fill.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut now) };
    assert_eq!(result, 0, "clock_gettime");
    now.tv_sec as f64 + now.tv_nsec as f64 * 1e-9
}

fn noisier(samples: &[i16], seed: u64) -> Vec<i16> {
    let mut state = seed | 1;
    samples
        .iter()
        .map(|&sample| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let noise = ((state >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 2.0 * 184.0;
            (f64::from(sample) + noise).clamp(-32_768.0, 32_767.0) as i16
        })
        .collect()
}

fn main() {
    let minutes: f64 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(10.0);
    let minute = speech_like(60.0, 42);
    let repeats = (minutes.ceil() as usize).max(1);
    let quiet: Vec<i16> = minute
        .iter()
        .copied()
        .cycle()
        .take(minute.len() * repeats)
        .collect();
    let loud = noisier(&quiet, 7);
    let seconds = quiet.len() as f64 / f64::from(SAMPLE_RATE);
    let dir = tempfile::tempdir().expect("temp dir");
    println!("{seconds:.0} s of audio per run, 10 ms writes");
    println!(
        "{:<26} {:>10} {:>12} {:>12} {:>12}",
        "format / floor", "encode cpu s", "x realtime", "MB per hour", "decode x rt"
    );
    for (label, signal) in [("-60 dBFS", &quiet), ("-45 dBFS", &loud)] {
        for format in [AudioFormat::Opus, AudioFormat::Flac, AudioFormat::Wav] {
            let path = dir.path().join(format.file_name());
            let started = cpu_seconds();
            let mut writer = create_writer(format, File::create(&path).unwrap()).unwrap();
            for chunk in signal.chunks(480) {
                writer.write(chunk).unwrap();
            }
            writer.finish().unwrap();
            let encode = cpu_seconds() - started;
            let bytes = std::fs::metadata(&path).unwrap().len() as f64;
            let started = cpu_seconds();
            let mut reader = open_reader(format, &path).unwrap();
            let decoded = read_to_end(reader.as_mut()).unwrap();
            let decode = cpu_seconds() - started;
            assert_eq!(decoded.len(), signal.len());
            println!(
                "{:<26} {:>10.2} {:>12.0} {:>12.1} {:>12.0}",
                format!("{format:?} / {label}"),
                encode,
                seconds / encode,
                bytes * 3_600.0 / seconds / 1e6,
                seconds / decode
            );
        }
    }
}
