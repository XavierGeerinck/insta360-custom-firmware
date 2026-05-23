//! waved-wakeup-probe: one-shot benchmark of DSP wake-from-silence latency.
//!
//! Constructs its own `AudioService`, injects a deterministic speech-shaped
//! burst after N seconds of silence, and reports per-burst latency + leading
//! edge loss vs a warmup baseline. Run on device only (needs DFN3 model
//! init and the real Processor — does not touch ALSA).
//!
//! See `docs/superpowers/specs/2026-05-23-waved-wakeup-probe-design.md`.

use clap::Parser;
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(about = "DSP wake-from-silence benchmark for waved")]
struct Args {
    /// Idle duration before each cold burst (seconds).
    #[arg(long, default_value_t = 60)]
    silence_seconds: u32,
    /// Burst duration (milliseconds).
    #[arg(long, default_value_t = 200)]
    burst_ms: u32,
    /// Warm bursts to establish baseline.
    #[arg(long, default_value_t = 5)]
    warmup_bursts: u32,
    /// Cold (post-silence) bursts to measure.
    #[arg(long, default_value_t = 10)]
    cold_runs: u32,
    /// Optional path for JSON report output.
    #[arg(long)]
    json: Option<String>,
}

/// Tiny xorshift64* PRNG. Deterministic per seed, no external deps. ~2 ns/call.
struct Xorshift64Star(u64);
impl Xorshift64Star {
    fn new(seed: u64) -> Self { Self(seed.max(1)) }
    fn next_f32(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        let bits = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        // Map upper 24 bits to [-1, 1).
        let u = ((bits >> 40) as i32) - (1 << 23);
        u as f32 / (1 << 23) as f32
    }
}

/// 2nd-order biquad — applied twice (HPF at 200, LPF at 4000) gives a
/// speech-band BPF. Cookbook coefficients (Robert Bristow-Johnson).
struct Biquad { b0: f32, b1: f32, b2: f32, a1: f32, a2: f32, z1: f32, z2: f32 }
impl Biquad {
    fn highpass(sr: f32, f0: f32, q: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * f0 / sr;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 + cos_w0) / 2.0) / a0,
            b1: -(1.0 + cos_w0) / a0,
            b2: ((1.0 + cos_w0) / 2.0) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0, z2: 0.0,
        }
    }
    fn lowpass(sr: f32, f0: f32, q: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * f0 / sr;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cos_w0) / 2.0) / a0,
            b1: (1.0 - cos_w0) / a0,
            b2: ((1.0 - cos_w0) / 2.0) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0, z2: 0.0,
        }
    }
    fn process(&mut self, x: f32) -> f32 {
        // Transposed direct form II.
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Deterministic speech-shaped noise burst. White noise → band-pass 200 Hz–4 kHz
/// → raised-cosine envelope on first/last 10 ms. Output peak ≈ −20 dBFS.
pub fn generate_burst(burst_ms: u32, sample_rate: u32, seed: u64) -> Vec<f32> {
    let n = (burst_ms as usize * sample_rate as usize) / 1000;
    let sr = sample_rate as f32;
    let mut rng = Xorshift64Star::new(seed);
    let mut hp = Biquad::highpass(sr, 200.0, 0.707);
    let mut lp = Biquad::lowpass(sr, 4000.0, 0.707);

    // Filtered noise.
    let mut samples: Vec<f32> = (0..n)
        .map(|_| lp.process(hp.process(rng.next_f32())))
        .collect();

    // Normalize to −20 dBFS peak (0.1 amplitude).
    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    if peak > 0.0 {
        let scale = 0.1 / peak;
        for s in samples.iter_mut() { *s *= scale; }
    }

    // Raised-cosine envelope on first/last 10 ms.
    // env(t) = 0.5 * (1 - cos(π*t)), t in [0,1] → 0 at t=0, 1 at t=1.
    let edge = ((10 * sample_rate) / 1000) as usize;
    let edge = edge.min(n / 2);
    for i in 0..edge {
        let t = i as f32 / edge as f32;          // 0..1
        let env = 0.5 * (1.0 - (std::f32::consts::PI * t).cos());
        samples[i] *= env;
        samples[n - 1 - i] *= env;
    }

    samples
}

/// Find the sample offset in `output` where `burst` best aligns via
/// brute-force cross-correlation. Returns `None` if the peak normalized
/// score is below `min_score_ratio` × the burst's auto-correlation peak
/// (= sum of squares).
///
/// O(len(output) * len(burst)). At default cold-run sizing
/// (`output ≈ 2.9 M`, `burst ≈ 9 600`) this is ~28 G mul-adds per call —
/// expect roughly 10–20 s per burst on the Wave's Cortex-A53. Acceptable
/// for a one-shot benchmark; if Task 6 finds it intolerable, window the
/// search (the burst arrival is known to be near `silence_samples`) or
/// switch to an FFT-based xcorr.
pub fn find_burst_offset(output: &[f32], burst: &[f32], min_score_ratio: f32) -> Option<usize> {
    if burst.is_empty() || output.len() < burst.len() {
        return None;
    }
    let auto: f32 = burst.iter().map(|s| s * s).sum();
    if auto <= 0.0 {
        return None;
    }
    let mut best_score = f32::MIN;
    let mut best_offset = 0_usize;
    let last = output.len() - burst.len();
    for off in 0..=last {
        let win = &output[off..off + burst.len()];
        let score: f32 = win.iter().zip(burst.iter()).map(|(a, b)| a * b).sum();
        if score > best_score {
            best_score = score;
            best_offset = off;
        }
    }
    let ratio = best_score / auto;
    if ratio >= min_score_ratio { Some(best_offset) } else { None }
}

/// Returns the length (ms) of the leading-edge run in `output_burst` where
/// non-overlapping 10 ms RMS is below 50 % of the corresponding window in
/// `input_burst`. Both slices are assumed time-aligned (output already
/// shifted to start at the burst arrival sample).
pub fn leading_edge_loss_ms(input_burst: &[f32], output_burst: &[f32], sample_rate: u32) -> f32 {
    let window = (sample_rate as usize * 10) / 1000; // 10 ms
    let n_windows = input_burst.len().min(output_burst.len()) / window;
    let mut leading = 0_usize;
    for w in 0..n_windows {
        let lo = w * window;
        let hi = lo + window;
        let in_rms = rms(&input_burst[lo..hi]);
        let out_rms = rms(&output_burst[lo..hi]);
        if out_rms < 0.5 * in_rms {
            leading = w + 1;
        } else {
            break;
        }
    }
    (leading * window) as f32 * 1000.0 / sample_rate as f32
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() { return 0.0; }
    let sumsq: f32 = samples.iter().map(|s| s * s).sum();
    (sumsq / samples.len() as f32).sqrt()
}

/// One burst's measurements. By convention, all fields are either populated
/// together (successful detection) or all `None` (MISSED — xcorr score below
/// threshold). `aggregate()` uses `latency_ms.is_none()` as the MISSED
/// indicator and filters each metric independently — so a partially-populated
/// BurstResult would silently desync the `missed` counter from per-metric
/// sample counts. Don't construct one.
#[derive(Debug, Clone, Serialize)]
pub struct BurstResult {
    pub latency_ms: Option<f32>,
    pub edge_loss_ms: Option<f32>,
    pub peak_db: Option<f32>,
}

/// Aggregate stats for one phase (warm or cold). Medians/p95 are computed
/// over the non-MISSED subset; `missed` counts the exclusions.
#[derive(Debug, Clone, Serialize)]
pub struct Phase {
    pub label: String,
    pub total: usize,
    pub missed: usize,
    pub latency_median_ms: Option<f32>,
    pub latency_p95_ms: Option<f32>,
    pub edge_loss_median_ms: Option<f32>,
    pub edge_loss_p95_ms: Option<f32>,
    pub peak_db_median: Option<f32>,
}

fn percentile(sorted: &[f32], p: f32) -> Option<f32> {
    if sorted.is_empty() { return None; }
    // Nearest-rank percentile (no interpolation — fine for our N≤30).
    let idx = ((p * sorted.len() as f32).ceil() as usize).saturating_sub(1);
    Some(sorted[idx.min(sorted.len() - 1)])
}

pub fn aggregate(label: &str, results: &[BurstResult]) -> Phase {
    let total = results.len();
    let missed = results.iter().filter(|r| r.latency_ms.is_none()).count();

    let mut lat: Vec<f32> = results.iter().filter_map(|r| r.latency_ms).collect();
    let mut loss: Vec<f32> = results.iter().filter_map(|r| r.edge_loss_ms).collect();
    let mut peak: Vec<f32> = results.iter().filter_map(|r| r.peak_db).collect();
    lat.sort_by(|a, b| a.total_cmp(b));
    loss.sort_by(|a, b| a.total_cmp(b));
    peak.sort_by(|a, b| a.total_cmp(b));

    Phase {
        label: label.to_string(),
        total,
        missed,
        latency_median_ms: percentile(&lat, 0.5),
        latency_p95_ms: percentile(&lat, 0.95),
        edge_loss_median_ms: percentile(&loss, 0.5),
        edge_loss_p95_ms: percentile(&loss, 0.95),
        peak_db_median: percentile(&peak, 0.5),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    let args = Args::parse();
    eprintln!("waved-wakeup-probe: {args:?}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_generation_is_deterministic() {
        let a = generate_burst(200, 48_000, 0xDEADBEEF);
        let b = generate_burst(200, 48_000, 0xDEADBEEF);
        assert_eq!(a, b, "same seed must produce identical samples");
        assert_eq!(a.len(), 9600, "200 ms @ 48 kHz = 9600 samples");
    }

    #[test]
    fn burst_envelope_has_raised_cosine_edges() {
        let b = generate_burst(200, 48_000, 1);
        // First and last samples are 0 (cos(π) = -1, so env(0) = 0).
        assert!(b[0].abs() < 1e-6, "first sample should be ~0, got {}", b[0]);
        assert!(b.last().unwrap().abs() < 1e-6, "last sample should be ~0");
        // Middle samples have nonzero amplitude.
        let mid = b[b.len() / 2];
        assert!(mid.abs() > 0.01, "middle sample should be audible, got {mid}");
    }

    #[test]
    fn burst_peak_is_near_minus_20_dbfs() {
        let b = generate_burst(200, 48_000, 42);
        let peak = b.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        // Envelope on the edges can attenuate the very-edge peaks slightly
        // below 0.1; the bulk of the burst should still hit ≈ 0.1.
        assert!(peak > 0.05 && peak < 0.11, "peak should be ≈0.1 (≈-20 dBFS), got {peak}");
    }

    #[test]
    fn xcorr_finds_known_offset() {
        let burst = generate_burst(50, 48_000, 7);
        let known_offset = 5000_usize;
        let mut output = vec![0.0_f32; known_offset + burst.len() + 1000];
        output[known_offset..known_offset + burst.len()].copy_from_slice(&burst);
        let offset = find_burst_offset(&output, &burst, 0.3).expect("burst must be found");
        // Allow ±1 sample slop for argmax ties.
        let delta = (offset as i64 - known_offset as i64).abs();
        assert!(delta <= 1, "expected offset ≈{known_offset}, got {offset} (delta {delta})");
    }

    #[test]
    fn xcorr_returns_none_when_burst_absent() {
        let burst = generate_burst(50, 48_000, 11);
        let output = vec![0.0_f32; 10_000]; // pure silence
        assert!(find_burst_offset(&output, &burst, 0.3).is_none());
    }

    #[test]
    fn leading_edge_loss_zero_when_output_is_input() {
        let burst = generate_burst(200, 48_000, 99);
        let loss = leading_edge_loss_ms(&burst, &burst, 48_000);
        assert!(loss < 1.0, "passthrough should have ≈0 loss, got {loss} ms");
    }

    #[test]
    fn leading_edge_loss_detects_suppressed_prefix() {
        let burst = generate_burst(200, 48_000, 99);
        // Zero out the first 50 ms = 2400 samples.
        let mut out = burst.clone();
        for s in out.iter_mut().take(2400) { *s = 0.0; }
        let loss = leading_edge_loss_ms(&burst, &out, 48_000);
        // Expect approximately 50 ms loss (within one window of slack).
        assert!((40.0..=60.0).contains(&loss), "expected ≈50 ms, got {loss}");
    }

    #[test]
    fn aggregate_excludes_missed_from_medians() {
        let results = vec![
            BurstResult { latency_ms: Some(10.0), edge_loss_ms: Some(5.0), peak_db: Some(-12.0) },
            BurstResult { latency_ms: None,        edge_loss_ms: None,       peak_db: None },
            BurstResult { latency_ms: Some(20.0), edge_loss_ms: Some(15.0), peak_db: Some(-14.0) },
            BurstResult { latency_ms: Some(30.0), edge_loss_ms: Some(25.0), peak_db: Some(-13.0) },
        ];
        let agg = aggregate("test", &results);
        assert_eq!(agg.missed, 1);
        assert_eq!(agg.total, 4);
        assert_eq!(agg.latency_median_ms, Some(20.0));
        assert_eq!(agg.edge_loss_median_ms, Some(15.0));
        assert_eq!(agg.peak_db_median, Some(-13.0));
    }

    #[test]
    fn aggregate_all_missed_returns_none_stats() {
        let results = vec![
            BurstResult { latency_ms: None, edge_loss_ms: None, peak_db: None },
            BurstResult { latency_ms: None, edge_loss_ms: None, peak_db: None },
        ];
        let agg = aggregate("test", &results);
        assert_eq!(agg.missed, 2);
        assert_eq!(agg.latency_median_ms, None);
        assert_eq!(agg.latency_p95_ms, None);
    }
}
