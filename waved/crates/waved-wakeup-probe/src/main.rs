//! waved-wakeup-probe: one-shot benchmark of DSP wake-from-silence latency.
//!
//! Constructs its own `AudioService`, injects a deterministic speech-shaped
//! burst after N seconds of silence, and reports per-burst latency + leading
//! edge loss vs a warmup baseline. Run on device only (needs DFN3 model
//! init and the real Processor — does not touch ALSA).
//!
//! See `docs/superpowers/specs/2026-05-23-waved-wakeup-probe-design.md`.

use clap::Parser;

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
}
