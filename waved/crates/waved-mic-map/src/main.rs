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
const DURATION_SEC: usize = 24;
const TAP_SLOT_SEC: f32 = 3.0;
const NUM_TAPS: usize = 8;
// Peak-detect window (frames). ~20 ms of audio is long enough to integrate
// a tap transient but short enough to isolate it from neighbours.
const PEAK_WIN_FRAMES: usize = 960;
// Minimum gap between accepted peaks (frames). ~1 s keeps one loud tap from
// being counted as two.
const MIN_PEAK_SEP_FRAMES: usize = 48000;

const PHYSICAL_LABELS: [&str; 8] = [
    "TOP 45°  (front-left)",
    "TOP 135° (back-left)",
    "TOP 225° (back-right)",
    "TOP 315° (front-right)",
    "BOT 45°  (front-left)",
    "BOT 135° (back-left)",
    "BOT 225° (back-right)",
    "BOT 315° (front-right)",
];

fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--health") {
        return health_check();
    }

    println!();
    println!("waved-mic-map — tap-harness for ALSA→physical mic mapping");
    println!();
    println!("Instructions:");
    println!("  You will tap each of the 8 mic holes in the order below.");
    println!("  Each tap should be a short, sharp pencil-tap on the hole.");
    println!("  You have 3 seconds per slot — wait for the announcement, then tap firmly.");
    println!("  Analysis uses peak detection, so exact timing inside the slot doesn't matter.");
    println!("  Recording begins in 3 s...");
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

    // Compute broadband energy per PEAK_WIN_FRAMES window (summed across channels).
    // Then pick the top NUM_TAPS peaks in time order with MIN_PEAK_SEP_FRAMES spacing.
    // For each picked peak, the ALSA channel with highest energy inside that window
    // is the mic the operator tapped.
    let num_windows = total_frames / PEAK_WIN_FRAMES;
    let mut window_energy = vec![0.0f64; num_windows];
    let mut window_ch_energy = vec![[0.0f64; 8]; num_windows];
    for w in 0..num_windows {
        let base = w * PEAK_WIN_FRAMES;
        for f in 0..PEAK_WIN_FRAMES {
            for ch in 0..CHANNELS as usize {
                let v = recording[(base + f) * CHANNELS as usize + ch] as f64 / i32::MAX as f64;
                let sq = v * v;
                window_ch_energy[w][ch] += sq;
                window_energy[w] += sq;
            }
        }
    }

    // Greedy peak picking: take global max, zero out ±MIN_PEAK_SEP windows around it,
    // repeat until we have NUM_TAPS peaks. Then sort by time (frame index).
    let min_sep_windows = MIN_PEAK_SEP_FRAMES / PEAK_WIN_FRAMES;
    let mut energies_work = window_energy.clone();
    let mut peak_windows: Vec<usize> = Vec::with_capacity(NUM_TAPS);
    for _ in 0..NUM_TAPS {
        let (best_w, &best_e) = energies_work
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        if best_e <= 0.0 {
            break;
        }
        peak_windows.push(best_w);
        let lo = best_w.saturating_sub(min_sep_windows);
        let hi = (best_w + min_sep_windows + 1).min(num_windows);
        for w in lo..hi {
            energies_work[w] = 0.0;
        }
    }
    peak_windows.sort_unstable();

    if peak_windows.len() < NUM_TAPS {
        eprintln!(
            "  ERROR: only detected {} tap(s) (needed {NUM_TAPS}). Tap harder / closer to the mic.",
            peak_windows.len()
        );
        std::process::exit(2);
    }

    // slot_best_ch[slot] = ALSA channel with highest energy in that slot's peak window.
    let mut slot_best_ch = [0usize; NUM_TAPS];
    for (slot, &w) in peak_windows.iter().enumerate() {
        let ch_e = &window_ch_energy[w];
        let (best_ch, &best_ch_e) = ch_e
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        slot_best_ch[slot] = best_ch;
        let total_e = window_energy[w];
        let t_sec = (w * PEAK_WIN_FRAMES) as f32 / SAMPLE_RATE as f32;
        println!(
            "  slot {slot} ({}) @ t={t_sec:.2}s: ALSA ch {best_ch} energy={best_ch_e:.3} (total {total_e:.3})",
            PHYSICAL_LABELS[slot]
        );
    }

    // Assign ALSA → physical from the winner of each slot, first-come-first-served
    // (so duplicates don't silently overwrite earlier assignments). Track which
    // slots lost out to a duplicate and which ALSA channels never peaked.
    let mut alsa_to_physical = [u8::MAX; 8];
    let mut duplicate_slots: Vec<usize> = Vec::new();
    for (slot, &ch) in slot_best_ch.iter().enumerate() {
        if alsa_to_physical[ch] == u8::MAX {
            alsa_to_physical[ch] = slot as u8;
        } else {
            duplicate_slots.push(slot);
        }
    }
    let unassigned_channels: Vec<usize> = (0..8)
        .filter(|&ch| alsa_to_physical[ch] == u8::MAX)
        .collect();

    let mut ok = duplicate_slots.is_empty() && unassigned_channels.is_empty();

    // Unambiguous-inference shortcut: if exactly one slot was duplicated and
    // exactly one channel never peaked, the missing channel must belong to
    // the losing slot. This recovers from the common case where the operator
    // hit the same pinhole twice (or one mic is too quiet to peak).
    if duplicate_slots.len() == 1 && unassigned_channels.len() == 1 {
        let slot = duplicate_slots[0];
        let ch = unassigned_channels[0];
        println!(
            "  inferred: ALSA ch {ch} ↔ slot {slot} ({}) [only remaining pairing]",
            PHYSICAL_LABELS[slot]
        );
        alsa_to_physical[ch] = slot as u8;
        duplicate_slots.clear();
        ok = true;
    }

    for slot in &duplicate_slots {
        eprintln!(
            "  WARNING: slot {slot} ({}) lost to a duplicate — re-tap that hole.",
            PHYSICAL_LABELS[*slot]
        );
    }
    for ch in &unassigned_channels {
        eprintln!("  WARNING: ALSA ch {ch} never peaked — dead mic or missed tap?");
    }
    // Any slot still unfilled in alsa_to_physical defaults to 0 (noise).
    for v in alsa_to_physical.iter_mut() {
        if *v == u8::MAX {
            *v = 0;
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

/// Diagnostic: record 3 s of ambient + any nearby speech, print per-channel RMS
/// (both absolute 0..1 and dBFS). Used to sanity-check that all 8 mics are
/// alive before attempting the tap-mapping procedure.
fn health_check() -> std::io::Result<()> {
    const HEALTH_SEC: usize = 3;
    println!();
    println!("waved-mic-map --health — recording {HEALTH_SEC} s of ambient audio...");
    println!("  (speak / make noise near the device so dead mics stand out)");
    println!();

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

    let total_frames = SAMPLE_RATE as usize * HEALTH_SEC;
    let mut recording = vec![0i32; total_frames * CHANNELS as usize];
    let mut filled = 0usize;
    while filled < total_frames {
        let chunk = PERIOD_FRAMES as usize;
        let end = (filled + chunk).min(total_frames);
        let s0 = filled * CHANNELS as usize;
        let s1 = end * CHANNELS as usize;
        let n = io
            .readi(&mut recording[s0..s1])
            .map_err(|e| std::io::Error::other(format!("readi: {e}")))?;
        filled += n;
    }

    let mut sumsq = [0.0f64; 8];
    let mut peak = [0.0f64; 8];
    for f in 0..total_frames {
        for ch in 0..CHANNELS as usize {
            let v = recording[f * CHANNELS as usize + ch] as f64 / i32::MAX as f64;
            sumsq[ch] += v * v;
            if v.abs() > peak[ch] {
                peak[ch] = v.abs();
            }
        }
    }
    println!("  per-channel results:");
    for ch in 0..CHANNELS as usize {
        let rms = (sumsq[ch] / total_frames as f64).sqrt();
        let dbfs = if rms > 0.0 { 20.0 * rms.log10() } else { -200.0 };
        let peak_db = if peak[ch] > 0.0 {
            20.0 * peak[ch].log10()
        } else {
            -200.0
        };
        let status = if dbfs < -80.0 {
            "DEAD?"
        } else if dbfs < -60.0 {
            "quiet"
        } else {
            "ok"
        };
        println!(
            "    ALSA ch {ch}: rms={rms:.5} ({dbfs:6.1} dBFS)  peak={:.5} ({peak_db:6.1} dBFS)  [{status}]",
            peak[ch]
        );
    }
    Ok(())
}
