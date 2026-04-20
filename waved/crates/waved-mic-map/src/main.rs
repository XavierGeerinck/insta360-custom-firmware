//! Tap-harness CLI. Records 12 s of 8-ch mic while the operator taps each
//! physical mic hole in a documented order. Segments the recording into
//! eight 1.5 s windows, computes per-ALSA-channel energy per window, and
//! reports the ALSA channel with highest energy per physical-index slot.

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use std::io::Write;
use std::time::Duration;

const SAMPLE_RATE: u32 = 48000;
const CHANNELS: u32 = 8;
const PERIOD_FRAMES: i64 = 1024;
const DURATION_SEC: usize = 12;
const TAP_SLOT_SEC: f32 = 1.5;
const NUM_TAPS: usize = 8;

const PHYSICAL_LABELS: [&str; 8] = [
    "TOP 0° (front)",
    "TOP 90° (left)",
    "TOP 180° (back)",
    "TOP 270° (right)",
    "SIDE 45°",
    "SIDE 135°",
    "SIDE 225°",
    "SIDE 315°",
];

fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    println!();
    println!("waved-mic-map — tap-harness for ALSA→physical mic mapping");
    println!();
    println!("Instructions:");
    println!("  You will tap each of the 8 mic holes in the order below.");
    println!("  Each tap should be a short, sharp pencil-tap on the hole.");
    println!("  You have 1.5 seconds per slot. Recording begins in 3 s...");
    println!();
    for (i, label) in PHYSICAL_LABELS.iter().enumerate() {
        println!("  slot {i}: tap {label}");
    }
    println!();
    for s in (1..=3).rev() {
        println!("  {s}...");
        std::thread::sleep(Duration::from_secs(1));
    }

    let pcm = PCM::new("hw:0,2", Direction::Capture, false)
        .map_err(|e| std::io::Error::other(format!("open hw:0,2: {e}")))?;
    {
        let hwp = HwParams::any(&pcm).unwrap();
        hwp.set_channels(CHANNELS).unwrap();
        hwp.set_rate(SAMPLE_RATE, ValueOr::Nearest).unwrap();
        hwp.set_format(Format::s32()).unwrap();
        hwp.set_access(Access::RWInterleaved).unwrap();
        hwp.set_period_size(PERIOD_FRAMES, ValueOr::Nearest).unwrap();
        pcm.hw_params(&hwp).unwrap();
    }
    pcm.prepare().unwrap();

    let io = pcm
        .io_i32()
        .map_err(|e| std::io::Error::other(format!("io_i32: {e}")))?;
    let total_frames = SAMPLE_RATE as usize * DURATION_SEC;
    let mut recording = vec![0i32; total_frames * CHANNELS as usize];
    let mut filled = 0usize;

    println!("RECORDING... tap each slot in order:");
    let start = std::time::Instant::now();
    let mut last_slot_announced = usize::MAX;
    while filled < total_frames {
        let chunk_frames = PERIOD_FRAMES as usize;
        let end = (filled + chunk_frames).min(total_frames);
        let slice_start = filled * CHANNELS as usize;
        let slice_end = end * CHANNELS as usize;
        let n = io
            .readi(&mut recording[slice_start..slice_end])
            .map_err(|e| std::io::Error::other(format!("readi: {e}")))?;
        filled += n;

        let elapsed_s = start.elapsed().as_secs_f32();
        let slot = (elapsed_s / TAP_SLOT_SEC).floor() as usize;
        if slot < NUM_TAPS && slot != last_slot_announced {
            println!("  -> slot {slot}: tap {}", PHYSICAL_LABELS[slot]);
            last_slot_announced = slot;
        }
    }
    println!("DONE recording, analyzing...");

    // Segment + per-alsa-channel per-slot RMS.
    let slot_frames = (TAP_SLOT_SEC * SAMPLE_RATE as f32) as usize;
    let mut energies = [[0.0f64; 8]; NUM_TAPS]; // [slot][alsa_ch]
    for (slot, slot_energies) in energies.iter_mut().enumerate() {
        let base_frame = slot * slot_frames;
        let end_frame = (base_frame + slot_frames).min(total_frames);
        for f in base_frame..end_frame {
            for ch in 0..CHANNELS as usize {
                let v = recording[f * CHANNELS as usize + ch] as f64 / i32::MAX as f64;
                slot_energies[ch] += v * v;
            }
        }
    }

    // For each physical slot, pick ALSA channel with highest energy.
    // Also validate: no two slots picked the same channel; no channel is dead.
    let mut alsa_to_physical = [0u8; 8];
    let mut used_channels = [false; 8];
    let mut ok = true;
    for (slot, slot_energies) in energies.iter().enumerate() {
        let (best_ch, best_e) = slot_energies
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        println!(
            "  slot {slot} ({}): ALSA ch {best_ch} energy={best_e:.3}",
            PHYSICAL_LABELS[slot]
        );
        if used_channels[best_ch] {
            eprintln!(
                "  WARNING: ALSA ch {best_ch} already assigned to a previous slot — tap error?"
            );
            ok = false;
        }
        used_channels[best_ch] = true;
        alsa_to_physical[best_ch] = slot as u8;
    }
    for (ch, &used) in used_channels.iter().enumerate() {
        if !used {
            eprintln!("  WARNING: ALSA ch {ch} never peaked — dead mic or missed tap?");
            ok = false;
        }
    }

    let json = serde_json::json!({ "alsa_to_physical": alsa_to_physical.to_vec(), "ok": ok });
    let json_str = serde_json::to_string_pretty(&json).unwrap();
    let out_path = "/tmp/mic_map.json";
    let mut f = std::fs::File::create(out_path)?;
    f.write_all(json_str.as_bytes())?;
    println!();
    println!("wrote {out_path}");
    println!("alsa_to_physical = {alsa_to_physical:?}");
    if !ok {
        eprintln!();
        eprintln!("  analysis flagged issues — consider re-running.");
        std::process::exit(2);
    }
    Ok(())
}
