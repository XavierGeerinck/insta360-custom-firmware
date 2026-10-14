//! Deterministic test audio and comparisons, shared by the tests and
//! `examples/encode_bench.rs`. Not part of the recording path.

use crate::SAMPLE_RATE;

/// A speech-like signal: voiced syllables (a harmonic series on a gliding
/// pitch, shaped by two moving formant-like peaks) at about 4 per second,
/// unvoiced noise bursts, short pauses, and a room noise floor around
/// −60 dBFS. Peaks near −6 dBFS. The same `seed` gives the same samples.
pub fn speech_like(seconds: f64, seed: u64) -> Vec<i16> {
    let n = (seconds * f64::from(SAMPLE_RATE)).round() as usize;
    let rate = f64::from(SAMPLE_RATE);
    let mut rng = XorShift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0f64;
    let syllable = (rate * 0.25) as usize;
    let mut kind = 0u64;
    for i in 0..n {
        if i % syllable == 0 {
            kind = rng.next() % 8;
        }
        let t = i as f64 / rate;
        let within = (i % syllable) as f64 / syllable as f64;
        let envelope = (std::f64::consts::PI * within).sin().powi(2);
        let f0 = 120.0 + 60.0 * (t * 0.7).sin() + 25.0 * (t * 3.1).sin();
        phase += f0 / rate;
        let formant1 = 500.0 + 300.0 * (t * 1.3).sin();
        let formant2 = 1_500.0 + 700.0 * (t * 0.9).cos();
        let voiced = match kind {
            0 => 0.0, // pause
            1 => 0.0, // unvoiced burst below
            _ => {
                let mut sum = 0.0;
                for k in 1..=30 {
                    let f = f0 * k as f64;
                    if f > 7_500.0 {
                        break;
                    }
                    let gain = peak(f, formant1, 250.0) + 0.6 * peak(f, formant2, 400.0) + 0.05;
                    sum += gain * (2.0 * std::f64::consts::PI * phase * k as f64).sin() / k as f64;
                }
                0.35 * sum
            }
        };
        let noise = rng.uniform() - 0.5;
        let burst = if kind == 1 { 0.15 * noise } else { 0.0 };
        let floor = 0.002 * (rng.uniform() - 0.5);
        let value = envelope * (voiced + burst) + floor;
        out.push((value.clamp(-1.0, 1.0) * 32_767.0) as i16);
    }
    out
}

fn peak(f: f64, centre: f64, width: f64) -> f64 {
    let x = (f - centre) / width;
    1.0 / (1.0 + x * x)
}

/// Signal-to-noise ratio of `decoded` against `reference` in dB, over the
/// common length.
pub fn snr_db(reference: &[i16], decoded: &[i16]) -> f64 {
    let n = reference.len().min(decoded.len());
    let (mut signal, mut noise) = (0.0f64, 0.0f64);
    for i in 0..n {
        let r = f64::from(reference[i]);
        let d = f64::from(decoded[i]);
        signal += r * r;
        noise += (r - d) * (r - d);
    }
    10.0 * (signal / noise.max(1e-9)).log10()
}

/// The lag in −`max_lag..=max_lag` where `decoded` best matches
/// `reference` (normalised cross-correlation): 0 means sample-aligned.
pub fn best_lag(reference: &[i16], decoded: &[i16], max_lag: i64) -> i64 {
    let n = reference.len().min(decoded.len()) as i64;
    let mut best = (f64::MIN, 0i64);
    for lag in -max_lag..=max_lag {
        let mut sum = 0.0f64;
        let mut energy = 0.0f64;
        for i in max_lag..(n - max_lag) {
            let d = f64::from(decoded[(i + lag) as usize]);
            sum += f64::from(reference[i as usize]) * d;
            energy += d * d;
        }
        let score = sum / energy.max(1.0).sqrt();
        if score > best.0 {
            best = (score, lag);
        }
    }
    best.1
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}
