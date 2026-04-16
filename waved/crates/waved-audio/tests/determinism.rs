//! Two runs on the same fixture must produce byte-identical metrics JSON.
//!
//! Run with: cargo test -p waved-audio --features capture-replay --test determinism

#![cfg(feature = "capture-replay")]

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::mpsc;
use waved_audio::callbacks::AudioFrame;
use waved_audio::capture::{run_capture_loop, CaptureStats, ChannelMapSource, SingleStreamFrameSource};
use waved_audio::capture::replay::{ReplayFormat, ReplayPcmSource};
use waved_audio::fixtures::tone_mono_8ch;
use waved_audio::stats_collector::compute_stats;

fn write_pcm(data: &[i16]) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    for &s in data { f.write_all(&s.to_le_bytes()).unwrap(); }
    f.flush().unwrap();
    f
}

fn collect_last_frame(fixture: &[i16]) -> (AudioFrame, Arc<CaptureStats>) {
    let file = write_pcm(fixture);
    let (tx, mut rx) = mpsc::channel::<AudioFrame>(256);
    let stats = CaptureStats::new();
    let stop = Arc::new(AtomicBool::new(false));
    let src = ReplayPcmSource::open(file.path(), ReplayFormat::I16_EIGHT, 48000).unwrap();
    let stats_c = Arc::clone(&stats);
    let stop_c = Arc::clone(&stop);
    std::thread::spawn(move || {
        run_capture_loop(SingleStreamFrameSource::new(src, 256, 8), tx, stop_c, stats_c);
    })
    .join()
    .unwrap();
    let mut last = None;
    while let Ok(f) = rx.try_recv() { last = Some(f); }
    (last.expect("at least one frame"), stats)
}

#[test]
fn replaying_same_fixture_twice_yields_identical_stats_json() {
    let fixture = tone_mono_8ch(1, 1000.0, 2, 8000);
    let (f1, s1) = collect_last_frame(&fixture);
    let (f2, s2) = collect_last_frame(&fixture);
    let stats1 = compute_stats(&f1, &s1, ChannelMapSource::File, None, "none");
    let stats2 = compute_stats(&f2, &s2, ChannelMapSource::File, None, "none");
    let mut stats1 = stats1;
    let mut stats2 = stats2;
    stats1.last_updated_ms = 0;
    stats2.last_updated_ms = 0;
    stats1.frames_emitted = 0;
    stats2.frames_emitted = 0;
    let j1 = serde_json::to_string(&stats1).unwrap();
    let j2 = serde_json::to_string(&stats2).unwrap();
    assert_eq!(j1, j2);
}
