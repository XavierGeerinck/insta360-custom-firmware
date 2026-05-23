# waved-wakeup-probe Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a one-shot CLI benchmark (`waved-wakeup-probe`) that measures DSP wake-from-silence latency on the Insta360 Wave by injecting a deterministic speech-shaped burst after N seconds of silence and reporting latency + leading-edge loss.

**Architecture:** New crate `waved-wakeup-probe` under `waved/crates/`. Standalone binary that constructs its own `waved_audio::AudioService` (no ALSA, no IPC), feeds frames synchronously via `process_frame()`, and cross-correlates the captured output against the known burst to derive metrics. Spec: `docs/superpowers/specs/2026-05-23-waved-wakeup-probe-design.md`.

**Tech Stack:** Rust, `clap` (CLI), `tokio` (channels — required by `AudioService::new`), `serde_json` (optional JSON output), `waved-audio` (the pipeline-under-test). Build target `aarch64-unknown-linux-gnu` via `cross`.

---

## File Structure

```
waved/crates/waved-wakeup-probe/
├── Cargo.toml             # new
└── src/
    └── main.rs            # CLI, burst gen, xcorr, drive loop, report (~400 LoC target)
```

Single-file crate. The spec budgets ~200 LoC but with full tests in the same file we'll be closer to 400.

**Workspace touch:** `waved/Cargo.toml` does not need editing — `members = ["crates/*"]` picks the new crate up automatically.

---

## Task 1: Crate scaffold + CLI args

**Files:**
- Create: `waved/crates/waved-wakeup-probe/Cargo.toml`
- Create: `waved/crates/waved-wakeup-probe/src/main.rs`

- [ ] **Step 1: Create the Cargo manifest**

Write `waved/crates/waved-wakeup-probe/Cargo.toml`:

```toml
[package]
name = "waved-wakeup-probe"
version.workspace = true
edition.workspace = true

[dependencies]
waved-audio = { path = "../waved-audio" }
waved-state = { path = "../waved-state" }
clap = { version = "4", features = ["derive"] }
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "sync"] }
tracing.workspace = true
tracing-subscriber.workspace = true
serde = { workspace = true, features = ["derive"] }
serde_json.workspace = true
```

- [ ] **Step 2: Write the CLI skeleton**

Write `waved/crates/waved-wakeup-probe/src/main.rs`:

```rust
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    let args = Args::parse();
    eprintln!("waved-wakeup-probe: {args:?}");
    Ok(())
}
```

- [ ] **Step 3: Verify the crate builds**

Run: `cd waved && cargo build -p waved-wakeup-probe`
Expected: clean build, no warnings, produces `target/debug/waved-wakeup-probe`.

- [ ] **Step 4: Verify it runs and parses args**

Run: `cd waved && cargo run -p waved-wakeup-probe -- --help`
Expected: usage output with all 5 flags (`--silence-seconds`, `--burst-ms`, `--warmup-bursts`, `--cold-runs`, `--json`).

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-wakeup-probe/
git commit -m "$(cat <<'EOF'
feat(wakeup-probe): scaffold waved-wakeup-probe crate with CLI skeleton

Empty binary with clap args matching the design spec. Builds cleanly,
parses --help correctly. Next tasks fill in burst gen, xcorr, drive loop.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Deterministic speech-shaped burst generator

**Files:**
- Modify: `waved/crates/waved-wakeup-probe/src/main.rs`

Add a `generate_burst(burst_ms, sample_rate, seed) -> Vec<f32>` function that produces white noise → band-pass filter (200 Hz–4 kHz) → raised-cosine envelope. Deterministic per seed.

- [ ] **Step 1: Write the first failing test (determinism)**

Append to `waved/crates/waved-wakeup-probe/src/main.rs`:

```rust
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
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd waved && cargo test -p waved-wakeup-probe burst_generation_is_deterministic`
Expected: FAIL — `cannot find function generate_burst`.

- [ ] **Step 3: Implement burst generator**

Add above the `#[tokio::main]` line in `main.rs`:

```rust
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
    let edge = ((10 * sample_rate) / 1000) as usize;
    let edge = edge.min(n / 2);
    for i in 0..edge {
        let t = i as f32 / edge as f32;          // 0..1
        let env = 0.5 * (1.0 - (std::f32::consts::PI * (1.0 - t)).cos());
        samples[i] *= env;
        samples[n - 1 - i] *= env;
    }

    samples
}
```

- [ ] **Step 4: Run determinism test to verify it passes**

Run: `cd waved && cargo test -p waved-wakeup-probe burst_generation_is_deterministic`
Expected: PASS.

- [ ] **Step 5: Add envelope test**

Append to the `#[cfg(test)]` block:

```rust
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
```

- [ ] **Step 6: Run new test to verify it passes**

Run: `cd waved && cargo test -p waved-wakeup-probe burst_envelope`
Expected: PASS.

- [ ] **Step 7: Add peak-level test**

Append to `#[cfg(test)]`:

```rust
    #[test]
    fn burst_peak_is_near_minus_20_dbfs() {
        let b = generate_burst(200, 48_000, 42);
        let peak = b.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        // Envelope on the edges can attenuate the very-edge peaks slightly
        // below 0.1; the bulk of the burst should still hit ≈ 0.1.
        assert!(peak > 0.05 && peak < 0.11, "peak should be ≈0.1 (≈-20 dBFS), got {peak}");
    }
```

- [ ] **Step 8: Run all burst tests**

Run: `cd waved && cargo test -p waved-wakeup-probe burst_`
Expected: 3/3 PASS.

- [ ] **Step 9: Commit**

```bash
git add waved/crates/waved-wakeup-probe/src/main.rs
git commit -m "$(cat <<'EOF'
feat(wakeup-probe): deterministic speech-shaped burst generator

Tiny xorshift64* PRNG + 2nd-order biquad BPF (200 Hz–4 kHz) + raised-cosine
envelope on first/last 10 ms. Output peak normalized to ≈ −20 dBFS. Zero
external deps. 3 unit tests cover determinism, envelope shape, level.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Cross-correlation offset finder

**Files:**
- Modify: `waved/crates/waved-wakeup-probe/src/main.rs`

Add `find_burst_offset(output, burst, min_score_ratio) -> Option<usize>` that returns the sample index in `output` where `burst` best aligns. Returns `None` when no detection meets the score threshold.

- [ ] **Step 1: Write the failing test**

Append to `#[cfg(test)]`:

```rust
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
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd waved && cargo test -p waved-wakeup-probe xcorr_`
Expected: FAIL — `cannot find function find_burst_offset`.

- [ ] **Step 3: Implement xcorr finder**

Add to `main.rs` (above the `#[tokio::main]` line):

```rust
/// Find the sample offset in `output` where `burst` best aligns via
/// brute-force cross-correlation. Returns `None` if the peak normalized
/// score is below `min_score_ratio` × the burst's auto-correlation peak
/// (= sum of squares).
///
/// O(len(output) * len(burst)). For our sizes (≤ ~3M × 9600), this runs
/// in well under a second on the device. No need for FFT.
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
```

- [ ] **Step 4: Run xcorr tests**

Run: `cd waved && cargo test -p waved-wakeup-probe xcorr_`
Expected: 2/2 PASS.

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-wakeup-probe/src/main.rs
git commit -m "$(cat <<'EOF'
feat(wakeup-probe): cross-correlation offset finder

Brute-force xcorr — O(len(output) * len(burst)) — runs comfortably under
a second for our sizes (no need for FFT). Normalizes against the burst's
auto-correlation so the score threshold is dimensionless.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Leading-edge-loss calculator

**Files:**
- Modify: `waved/crates/waved-wakeup-probe/src/main.rs`

Add `leading_edge_loss_ms(burst, output_burst, sample_rate) -> f32` that walks 10 ms windows of input vs output (both aligned to t=0 of the burst arrival), finds the longest leading run where output 10 ms RMS is below 50 % of the input window's 10 ms RMS, and returns that run's length in ms.

- [ ] **Step 1: Write the failing tests**

Append to `#[cfg(test)]`:

```rust
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
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd waved && cargo test -p waved-wakeup-probe leading_edge_loss`
Expected: FAIL — `cannot find function leading_edge_loss_ms`.

- [ ] **Step 3: Implement leading-edge-loss**

Add to `main.rs`:

```rust
/// Returns the length (ms) of the leading-edge run in `output_burst` where
/// rolling 10 ms RMS is below 50 % of the corresponding window in
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
```

- [ ] **Step 4: Run leading-edge tests**

Run: `cd waved && cargo test -p waved-wakeup-probe leading_edge_loss`
Expected: 2/2 PASS.

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-wakeup-probe/src/main.rs
git commit -m "$(cat <<'EOF'
feat(wakeup-probe): leading-edge-loss calculator

Walks 10 ms RMS windows of input vs output, returns the length (ms) of the
leading-edge run where output RMS is below 50% of input RMS. Targets the
'first ~100 ms eaten' bug class — surfaces how much of the burst's onset
was suppressed by NS/AEC state recovery.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Per-burst result + aggregate report

**Files:**
- Modify: `waved/crates/waved-wakeup-probe/src/main.rs`

Define `BurstResult` (per-run measurement, `None` = MISSED) and `Phase` (aggregate stats: median, p95, peak, missed count). Add `aggregate(phase_label, results) -> Phase`.

- [ ] **Step 1: Write the failing tests**

Append to `#[cfg(test)]`:

```rust
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
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd waved && cargo test -p waved-wakeup-probe aggregate_`
Expected: FAIL — `cannot find struct BurstResult` / `cannot find function aggregate`.

- [ ] **Step 3: Implement BurstResult + Phase + aggregate**

Add to `main.rs`:

```rust
use serde::Serialize;

/// One burst's measurements. All-`None` means MISSED (xcorr score too low).
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
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    loss.sort_by(|a, b| a.partial_cmp(b).unwrap());
    peak.sort_by(|a, b| a.partial_cmp(b).unwrap());

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
```

- [ ] **Step 4: Run aggregate tests**

Run: `cd waved && cargo test -p waved-wakeup-probe aggregate_`
Expected: 2/2 PASS.

- [ ] **Step 5: Run all tests to confirm nothing broke**

Run: `cd waved && cargo test -p waved-wakeup-probe`
Expected: 7/7 PASS (3 burst + 2 xcorr + 2 leading-edge + 2 aggregate = 9 — note correct count when running).

Wait, that's 9 (3+2+2+2). Confirm count is 9.

- [ ] **Step 6: Commit**

```bash
git add waved/crates/waved-wakeup-probe/src/main.rs
git commit -m "$(cat <<'EOF'
feat(wakeup-probe): BurstResult + aggregate (median + p95 + missed exclusion)

Per-burst measurement struct (None = MISSED), phase aggregator that filters
MISSED from stats and reports them separately in `missed`. Nearest-rank
percentile — fine for N ≤ 30. 2 unit tests cover exclusion + all-missed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: AudioService construction + run_burst drive loop

**Files:**
- Modify: `waved/crates/waved-wakeup-probe/src/main.rs`

Construct a real `AudioService` and write a `run_burst(...)` helper that drives one (silence + burst + tail) cycle through `process_frame()` and returns a `BurstResult`. This is the integration layer — no unit tests because it needs the real Processor.

**Critical detail from spec § 5:** `process_frame` calls `audio_tx.send_modify(...)`, which panics if there are no live `watch::Receiver`s. We hold `_audio_rx` for the lifetime of the run. Same for the meeting `mpsc::Receiver`.

- [ ] **Step 1: Add imports + build_service helper**

Add to `main.rs` (above `#[tokio::main]`):

```rust
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

use waved_audio::audio_config::AudioConfig;
use waved_audio::beam::geometry::MicGeometry;
use waved_audio::callbacks::AudioFrame;
use waved_audio::processed_frame::ProcessedAudioFrame;
use waved_audio::AudioService;
use waved_state::AudioState;

/// Owns the receivers we must keep alive — `process_frame` calls
/// `audio_tx.send_modify` which panics on a dead receiver list.
struct ServiceHandle {
    svc: AudioService,
    _audio_rx: watch::Receiver<AudioState>,
    _meeting_rx: mpsc::Receiver<ProcessedAudioFrame>,
}

fn build_service(frame_size: usize, sample_rate: u32) -> ServiceHandle {
    let initial = AudioState::default();
    let (audio_tx, audio_rx) = watch::channel(initial);
    let audio_tx = Arc::new(audio_tx);
    let (frame_tx, frame_rx) = mpsc::channel::<AudioFrame>(4);
    let (meeting_tx, meeting_rx) = mpsc::channel::<ProcessedAudioFrame>(4);
    let svc = AudioService::new(
        audio_tx,
        frame_tx,
        frame_rx,
        meeting_tx,
        frame_size,
        sample_rate,
        MicGeometry::default(),
    );
    ServiceHandle { svc, _audio_rx: audio_rx, _meeting_rx: meeting_rx }
}
```

- [ ] **Step 2: Add run_burst helper**

Add to `main.rs`:

```rust
/// Drive one (silence + burst + 200 ms tail, frame-padded) cycle through
/// the AudioService. Returns the measured BurstResult.
///
/// `silence_samples` may be 0 (warm phase — no silence between bursts).
fn run_burst(
    handle: &mut ServiceHandle,
    silence_samples: usize,
    burst: &[f32],
    sample_rate: u32,
    frame_size: usize,
    mic_channels: u16,
    min_score_ratio: f32,
) -> BurstResult {
    let tail_samples = (sample_rate as usize * 200) / 1000; // 200 ms tail
    let total = silence_samples + burst.len() + tail_samples;
    // Pad up to a frame boundary so chunks_exact aligns.
    let total_padded = total.div_ceil(frame_size) * frame_size;

    let mut input = Vec::with_capacity(total_padded);
    input.extend(std::iter::repeat(0.0_f32).take(silence_samples));
    input.extend_from_slice(burst);
    input.extend(std::iter::repeat(0.0_f32).take(total_padded - silence_samples - burst.len()));

    let mut output: Vec<f32> = Vec::with_capacity(input.len());
    let mc = mic_channels as usize;
    let mut interleaved: Vec<i16> = vec![0; frame_size * mc];
    for chunk in input.chunks_exact(frame_size) {
        // Duplicate mono float into 8-ch interleaved i16.
        for (i, &s) in chunk.iter().enumerate() {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            for ch in 0..mc {
                interleaved[i * mc + ch] = v;
            }
        }
        let frame = AudioFrame::mic_only(
            interleaved.clone(),
            mic_channels,
            sample_rate,
            0,
        );
        let processed = handle.svc.process_frame(&frame);
        output.extend_from_slice(&processed.mic_mono_f32);
    }

    let arrival = match find_burst_offset(&output, burst, min_score_ratio) {
        Some(idx) => idx,
        None => return BurstResult { latency_ms: None, edge_loss_ms: None, peak_db: None },
    };
    // arrival is relative to the start of `output` (which is aligned 1:1
    // with `input`). Subtract silence_samples to get the burst-relative delay.
    let lat_samples = arrival as i64 - silence_samples as i64;
    let latency_ms = (lat_samples.max(0) as f32) * 1000.0 / sample_rate as f32;

    let out_burst_end = (arrival + burst.len()).min(output.len());
    let out_burst = &output[arrival..out_burst_end];
    let edge_loss_ms = leading_edge_loss_ms(burst, out_burst, sample_rate);

    let peak = out_burst.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    let peak_db = if peak > 0.0 { 20.0 * peak.log10() } else { -120.0 };

    BurstResult {
        latency_ms: Some(latency_ms),
        edge_loss_ms: Some(edge_loss_ms),
        peak_db: Some(peak_db),
    }
}
```

- [ ] **Step 3: Verify the crate still builds**

Run: `cd waved && cargo build -p waved-wakeup-probe`
Expected: clean build, no warnings.

- [ ] **Step 4: Run all existing tests to confirm nothing broke**

Run: `cd waved && cargo test -p waved-wakeup-probe`
Expected: 9/9 PASS (3 burst + 2 xcorr + 2 leading-edge + 2 aggregate).

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-wakeup-probe/src/main.rs
git commit -m "$(cat <<'EOF'
feat(wakeup-probe): wire AudioService + run_burst drive loop

build_service() constructs an AudioService with default MicGeometry, holding
the watch/mpsc receivers for the lifetime of the run (process_frame panics
without live receivers). run_burst() drives one silence+burst+tail cycle
through process_frame, then xcorrs the captured mic_mono_f32 against the
known burst to derive latency / leading-edge-loss / peak_db.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Main loop + stdout report + optional JSON output

**Files:**
- Modify: `waved/crates/waved-wakeup-probe/src/main.rs`

Replace the stub `main()` body with the actual orchestration: build_service once, run warmup phase, run cold phase, aggregate each, print stdout report, optionally write JSON.

- [ ] **Step 1: Add Report struct and serialize test**

Add to `main.rs`:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub config: ReportConfig,
    pub warm: Phase,
    pub cold: Phase,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReportConfig {
    pub silence_seconds: u32,
    pub burst_ms: u32,
    pub warmup_bursts: u32,
    pub cold_runs: u32,
    pub sample_rate: u32,
    pub frame_size: usize,
}
```

Add to `#[cfg(test)]`:

```rust
    #[test]
    fn report_serializes_to_json() {
        let report = Report {
            config: ReportConfig {
                silence_seconds: 60, burst_ms: 200, warmup_bursts: 5,
                cold_runs: 10, sample_rate: 48_000, frame_size: 480,
            },
            warm: aggregate("warm", &[BurstResult {
                latency_ms: Some(4.0), edge_loss_ms: Some(0.0), peak_db: Some(-12.0),
            }]),
            cold: aggregate("cold", &[BurstResult {
                latency_ms: Some(80.0), edge_loss_ms: Some(40.0), peak_db: Some(-11.0),
            }]),
        };
        let s = serde_json::to_string(&report).expect("serialize");
        assert!(s.contains("\"silence_seconds\":60"));
        assert!(s.contains("\"latency_median_ms\":4"));
    }
```

- [ ] **Step 2: Run JSON test to verify it passes**

Run: `cd waved && cargo test -p waved-wakeup-probe report_serializes`
Expected: PASS.

- [ ] **Step 3: Implement print_summary helper**

Add to `main.rs`:

```rust
fn fmt_opt(v: Option<f32>, unit: &str) -> String {
    match v {
        Some(x) => format!("{x:7.2} {unit}"),
        None => format!("{:>10}", "—"),
    }
}

fn print_summary(report: &Report) {
    let c = &report.config;
    println!();
    println!(
        "waved-wakeup-probe — {} warm + {} cold runs ({}s silence, {}ms bursts)",
        c.warmup_bursts, c.cold_runs, c.silence_seconds, c.burst_ms
    );
    println!();
    let pp = |phase: &Phase, field_label: &str, median: Option<f32>, p95: Option<f32>| {
        println!(
            "{:<5} {:<22}  median {}   p95 {}",
            phase.label, field_label,
            fmt_opt(median, "ms"), fmt_opt(p95, "ms"),
        );
    };
    pp(&report.warm, "latency:",            report.warm.latency_median_ms, report.warm.latency_p95_ms);
    pp(&report.cold, "latency:",            report.cold.latency_median_ms, report.cold.latency_p95_ms);
    pp(&report.cold, "leading-edge loss:",  report.cold.edge_loss_median_ms, report.cold.edge_loss_p95_ms);
    println!(
        "warm  peak: {}    cold peak: {}",
        fmt_opt(report.warm.peak_db_median, "dBFS"),
        fmt_opt(report.cold.peak_db_median, "dBFS"),
    );
    println!("missed: {} / {}", report.warm.missed + report.cold.missed, report.warm.total + report.cold.total);
}
```

- [ ] **Step 4: Replace main() with the orchestrator**

Replace the existing `main()` body in `main.rs` with:

```rust
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    let args = Args::parse();

    // Load same audio config waved uses, so frame_size / sample_rate match.
    let (cfg, _) = AudioConfig::load_or_default(Path::new("/etc/waved-audio-config.json"));
    let sample_rate = cfg.capture.mic_sample_rate;
    let frame_size = cfg.period_frames;
    let mic_channels = cfg.capture.mic_channels as u16;

    eprintln!(
        "[setup] sample_rate={sample_rate} frame_size={frame_size} mic_channels={mic_channels}"
    );

    let mut handle = build_service(frame_size, sample_rate);
    let burst = generate_burst(args.burst_ms, sample_rate, 0xDEAD_BEEF_CAFE_F00D);
    let silence_samples = args.silence_seconds as usize * sample_rate as usize;
    const MIN_SCORE_RATIO: f32 = 0.3;

    // Warm phase: bursts back-to-back, no silence between.
    eprintln!("[warm ] {} bursts back-to-back", args.warmup_bursts);
    let mut warm = Vec::with_capacity(args.warmup_bursts as usize);
    for i in 0..args.warmup_bursts {
        let r = run_burst(&mut handle, 0, &burst, sample_rate, frame_size, mic_channels, MIN_SCORE_RATIO);
        eprintln!("  warm[{i}] {:?}", r);
        warm.push(r);
    }

    // Cold phase: silence + burst, repeated.
    eprintln!("[cold ] {} bursts with {}s silence each", args.cold_runs, args.silence_seconds);
    let mut cold = Vec::with_capacity(args.cold_runs as usize);
    for i in 0..args.cold_runs {
        let r = run_burst(&mut handle, silence_samples, &burst, sample_rate, frame_size, mic_channels, MIN_SCORE_RATIO);
        eprintln!("  cold[{i}] {:?}", r);
        cold.push(r);
    }

    let report = Report {
        config: ReportConfig {
            silence_seconds: args.silence_seconds,
            burst_ms: args.burst_ms,
            warmup_bursts: args.warmup_bursts,
            cold_runs: args.cold_runs,
            sample_rate,
            frame_size,
        },
        warm: aggregate("warm", &warm),
        cold: aggregate("cold", &cold),
    };
    print_summary(&report);

    if let Some(path) = args.json.as_deref() {
        let mut f = std::fs::File::create(path)?;
        serde_json::to_writer_pretty(&mut f, &report)?;
        eprintln!("[json] wrote {}", path);
    }

    // Exit non-zero if every cold run missed.
    if !cold.is_empty() && report.cold.missed == report.cold.total {
        return Err("DSP swallowed every cold burst — check NS aggression".into());
    }
    Ok(())
}
```

- [ ] **Step 5: Build to verify**

Run: `cd waved && cargo build -p waved-wakeup-probe`
Expected: clean build, no warnings.

- [ ] **Step 6: Run all tests to confirm nothing broke**

Run: `cd waved && cargo test -p waved-wakeup-probe`
Expected: 10/10 PASS (9 from before + 1 new JSON test).

- [ ] **Step 7: Commit**

```bash
git add waved/crates/waved-wakeup-probe/src/main.rs
git commit -m "$(cat <<'EOF'
feat(wakeup-probe): wire main orchestrator + stdout + JSON output

Loads audio config (matches waved's frame_size/sample_rate), runs warmup
phase (back-to-back bursts) then cold phase (silence + burst), aggregates
both, prints the spec § 4.4 stdout table. --json writes the same Report
struct as pretty JSON. Exits non-zero if every cold burst was MISSED.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Integration test (`#[ignore]`-gated, on-device only)

**Files:**
- Modify: `waved/crates/waved-wakeup-probe/src/main.rs`

A test that constructs a real `AudioService`, runs one warmup + one cold burst with a short (1 s) silence, and asserts warm latency ≤ cold latency. Excluded from default test runs because real DSP timing depends on hardware.

- [ ] **Step 1: Add the test**

Append to `#[cfg(test)]`:

```rust
    #[test]
    #[ignore = "needs real device — run with `cargo test -- --ignored` on Wave"]
    fn integration_warm_less_than_or_equal_cold() {
        let sample_rate: u32 = 48_000;
        let frame_size: usize = 480;
        let mic_channels: u16 = 8;
        let mut handle = build_service(frame_size, sample_rate);
        let burst = generate_burst(200, sample_rate, 0xCAFE_BABE_u64);

        let warm = run_burst(&mut handle, 0, &burst, sample_rate, frame_size, mic_channels, 0.3);
        let cold_silence = sample_rate as usize; // 1 s
        let cold = run_burst(&mut handle, cold_silence, &burst, sample_rate, frame_size, mic_channels, 0.3);

        eprintln!("warm={warm:?}");
        eprintln!("cold={cold:?}");
        let w = warm.latency_ms.expect("warm must detect");
        let c = cold.latency_ms.expect("cold must detect");
        // Generous bound — we're confirming the loop runs end-to-end and the
        // cold-vs-warm relationship holds. Tight bounds would be flaky.
        assert!(w <= c + 5.0, "warm ({w} ms) should be ≤ cold ({c} ms) + 5 ms slack");
    }
```

- [ ] **Step 2: Verify default test run still passes (test should be skipped)**

Run: `cd waved && cargo test -p waved-wakeup-probe`
Expected: 10 passed, **1 ignored**.

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-wakeup-probe/src/main.rs
git commit -m "$(cat <<'EOF'
test(wakeup-probe): on-device integration test (#[ignore]-gated)

Constructs a real AudioService and runs one warm + one cold burst (1 s
silence), asserts warm latency ≤ cold + 5 ms slack. Skipped by default
because DSP timing depends on hardware — run with `cargo test --
--ignored` on the Wave to verify the end-to-end loop.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: On-device validation (manual; runbook, not a code commit)

**Files:** none — this is the runbook the implementer follows after Task 8.

- [ ] **Step 1: Cross-build for the device**

Run: `cd waved && cross build --release --target aarch64-unknown-linux-gnu -p waved-wakeup-probe`
Expected: produces `target/aarch64-unknown-linux-gnu/release/waved-wakeup-probe`.

- [ ] **Step 2: Deploy via adb**

Run: `adb push waved/target/aarch64-unknown-linux-gnu/release/waved-wakeup-probe /data/waved/`
Expected: `pushed` confirmation.

- [ ] **Step 3: Short smoke run (10 s silence)**

Run: `adb shell "killall waved 2>/dev/null; sleep 1; /data/waved/waved-wakeup-probe --silence-seconds 10 --cold-runs 3"`

(Killing waved first prevents ALSA contention on the DFN3 model state. The S60waved watchdog will respawn it within ~3 s of probe exit — see `waved/CLAUDE.md`.)

Expected stdout: the spec § 4.4 table. Specifically:
- `warm latency` is a small number (single-digit to low-double-digit ms)
- `cold latency ≥ warm latency`
- `missed: 0 / 8` (or however many warm + cold ran)
- Exit status 0

- [ ] **Step 4: Full benchmark run (60 s silence)**

Run: `adb shell "killall waved 2>/dev/null; sleep 1; /data/waved/waved-wakeup-probe --json /tmp/wakeup.json"`

Pull the JSON: `adb pull /tmp/wakeup.json firmwares/measurements/wakeup-$(date +%Y%m%d).json`

- [ ] **Step 5: Record baseline in the commit message**

Run: `git log -1 --oneline` then create a follow-up commit with the baseline:

```bash
mkdir -p firmwares/measurements
# (move the pulled JSON in place if not already)
git add firmwares/measurements/wakeup-*.json
git commit -m "$(cat <<'EOF'
chore(wakeup-probe): baseline measurement on Wave

Warm  median latency: <FILL IN FROM RUN> ms
Cold  median latency: <FILL IN FROM RUN> ms
Cold  median edge loss: <FILL IN FROM RUN> ms

Baseline established 2026-05-23 with --silence-seconds 60 --cold-runs 10.
Future pipeline changes should compare against this number.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Note: the baseline numbers are FILL-INs because they depend on the actual measurement. This is the only place a fill-in is acceptable — it's literal device output, not a design ambiguity.

---

## Self-Review Notes

**Spec coverage check:**
- ✅ § 1 (architecture) → Task 6 (build_service + run_burst)
- ✅ § 2 (file layout) → Task 1 (Cargo.toml + main.rs single file)
- ✅ § 3 (CLI) → Task 1 (clap Args)
- ✅ § 4.1 (stimulus) → Task 2 (generate_burst)
- ✅ § 4.2 (drive loop) → Task 6 (run_burst)
- ✅ § 4.3 (metrics) → Tasks 3 (xcorr) + 4 (edge loss) + 6 (peak_db inside run_burst)
- ✅ § 4.4 (aggregate report) → Task 5 (aggregate) + Task 7 (print_summary, JSON)
- ✅ § 5 (error handling) → Task 7 (non-zero exit when all cold MISSED)
- ✅ § 5 (watch/mpsc receivers) → Task 6 (`ServiceHandle` holds them)
- ✅ § 6.1 (unit tests) → Tasks 2/3/4/5/7
- ✅ § 6.2 (integration test) → Task 8
- ✅ § 6.3 (manual validation) → Task 9

**Type consistency check:** `BurstResult`, `Phase`, `Report`, `ReportConfig`, `ServiceHandle` are defined once each. `aggregate(label, results)` signature is stable across Tasks 5/7. `run_burst(handle, silence_samples, burst, sample_rate, frame_size, mic_channels, min_score_ratio)` signature is stable across Tasks 6/7/8. `generate_burst(burst_ms, sample_rate, seed)` is stable across Tasks 2/6/7/8.

**Test count progression:** Task 2 → 3 tests. Task 3 → +2 = 5. Task 4 → +2 = 7. Task 5 → +2 = 9. Task 7 → +1 = 10. Task 8 → +1 ignored = 10 passed + 1 ignored.

**Placeholder scan:** None. Task 9 step 5 has FILL-IN values for the actual measurement output — by design, since those depend on real device behavior.
