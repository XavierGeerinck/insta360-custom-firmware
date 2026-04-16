//! Audio measurement primitives — pure, infallible, no I/O.
//!
//! All functions return defined values for degenerate inputs (empty slice,
//! zero-variance channel) instead of NaN/Inf or panicking. This is the
//! documented contract; the stats collector and bench tooling rely on it.

pub fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() { return 0.0; }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

pub fn rms_per_channel(interleaved: &[i16], channels: usize) -> Vec<f32> {
    if channels == 0 || interleaved.is_empty() {
        return vec![0.0; channels];
    }
    (0..channels)
        .map(|ch| {
            let chan: Vec<i16> = interleaved.iter()
                .skip(ch)
                .step_by(channels)
                .copied()
                .collect();
            rms(&chan)
        })
        .collect()
}

pub fn clipping_rate(samples: &[i16], threshold: i16) -> f32 {
    if samples.is_empty() { return 0.0; }
    let clipped = samples.iter()
        .filter(|&&s| s.saturating_abs() >= threshold)
        .count();
    clipped as f32 / samples.len() as f32
}

pub fn dc_offset(samples: &[i16]) -> f32 {
    if samples.is_empty() { return 0.0; }
    let sum: i64 = samples.iter().map(|&s| s as i64).sum();
    sum as f32 / samples.len() as f32
}

/// Pearson correlation of two equal-length channels. Returns 0.0 when
/// either channel has near-zero variance (dead mic).
pub fn correlation(a: &[i16], b: &[i16]) -> f32 {
    if a.len() != b.len() || a.is_empty() { return 0.0; }
    let n = a.len() as f64;
    let mean_a: f64 = a.iter().map(|&x| x as f64).sum::<f64>() / n;
    let mean_b: f64 = b.iter().map(|&x| x as f64).sum::<f64>() / n;
    let mut num = 0f64;
    let mut var_a = 0f64;
    let mut var_b = 0f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let dx = x as f64 - mean_a;
        let dy = y as f64 - mean_b;
        num += dx * dy;
        var_a += dx * dx;
        var_b += dy * dy;
    }
    if var_a < f64::EPSILON || var_b < f64::EPSILON { return 0.0; }
    (num / (var_a * var_b).sqrt()) as f32
}

pub fn correlation_matrix(interleaved: &[i16], channels: usize) -> Vec<Vec<f32>> {
    let chans: Vec<Vec<i16>> = (0..channels)
        .map(|ch| interleaved.iter().skip(ch).step_by(channels).copied().collect())
        .collect();
    (0..channels)
        .map(|i| (0..channels).map(|j| correlation(&chans[i], &chans[j])).collect())
        .collect()
}

/// Cross-correlation at a given lag. Used by the latency probe to find
/// where the reference chirp appears in the captured mic signal.
pub fn cross_correlate_peak(signal: &[i16], reference: &[i16]) -> (usize, f32) {
    if reference.is_empty() || signal.len() < reference.len() {
        return (0, 0.0);
    }
    let max_lag = signal.len() - reference.len();
    let mut best_lag = 0usize;
    let mut best_score = f64::NEG_INFINITY;
    for lag in 0..=max_lag {
        let mut sum = 0f64;
        for (i, &r) in reference.iter().enumerate() {
            sum += signal[lag + i] as f64 * r as f64;
        }
        if sum > best_score {
            best_score = sum;
            best_lag = lag;
        }
    }
    (best_lag, best_score as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_empty_is_zero() {
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn rms_silence_is_zero() {
        assert_eq!(rms(&vec![0i16; 512]), 0.0);
    }

    #[test]
    fn rms_full_scale_is_max() {
        let r = rms(&vec![i16::MAX; 512]);
        assert!((r - i16::MAX as f32).abs() < 1.0);
    }

    #[test]
    fn rms_per_channel_splits_interleaved() {
        // ch0 = all i16::MAX, ch1 = all 0
        let mut data = Vec::new();
        for _ in 0..100 { data.push(i16::MAX); data.push(0); }
        let rms = rms_per_channel(&data, 2);
        assert!((rms[0] - i16::MAX as f32).abs() < 1.0);
        assert_eq!(rms[1], 0.0);
    }

    #[test]
    fn clipping_rate_all_clipped() {
        let r = clipping_rate(&vec![i16::MAX; 100], i16::MAX);
        assert!((r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn clipping_rate_none_clipped() {
        assert_eq!(clipping_rate(&vec![100i16; 100], i16::MAX), 0.0);
    }

    #[test]
    fn clipping_rate_half() {
        let mut data = vec![i16::MAX; 50];
        data.extend(vec![0i16; 50]);
        let r = clipping_rate(&data, i16::MAX);
        assert!((r - 0.5).abs() < 1e-6);
    }

    #[test]
    fn dc_offset_centered() {
        let data: Vec<i16> = (-100..100).collect();
        assert!(dc_offset(&data).abs() < 1.0);
    }

    #[test]
    fn dc_offset_biased() {
        let r = dc_offset(&vec![500i16; 100]);
        assert!((r - 500.0).abs() < 1.0);
    }

    #[test]
    fn correlation_identical_is_one() {
        let signal: Vec<i16> = (0..512).map(|i| ((i as f32 * 0.1).sin() * 10000.0) as i16).collect();
        let r = correlation(&signal, &signal);
        assert!((r - 1.0).abs() < 1e-4);
    }

    #[test]
    fn correlation_dead_channel_is_zero() {
        let signal: Vec<i16> = (0..512).map(|i| ((i as f32 * 0.1).sin() * 10000.0) as i16).collect();
        let silence = vec![0i16; 512];
        assert_eq!(correlation(&signal, &silence), 0.0);
    }

    #[test]
    fn correlation_inverted_is_negative_one() {
        let signal: Vec<i16> = (0..512).map(|i| ((i as f32 * 0.1).sin() * 10000.0) as i16).collect();
        let inv: Vec<i16> = signal.iter().map(|&s| -s).collect();
        let r = correlation(&signal, &inv);
        assert!((r + 1.0).abs() < 1e-4);
    }

    #[test]
    fn correlation_matrix_diagonal_is_one() {
        let mut data = Vec::new();
        for i in 0..256 {
            data.push(((i as f32 * 0.1).sin() * 10000.0) as i16);
            data.push(((i as f32 * 0.2).cos() * 10000.0) as i16);
        }
        let m = correlation_matrix(&data, 2);
        assert!((m[0][0] - 1.0).abs() < 1e-4);
        assert!((m[1][1] - 1.0).abs() < 1e-4);
        assert!((m[0][1] - m[1][0]).abs() < 1e-4); // symmetric
    }

    #[test]
    fn cross_correlate_finds_embedded_reference() {
        let reference: Vec<i16> = (0..100).map(|i| ((i as f32).sin() * 1000.0) as i16).collect();
        let mut signal = vec![0i16; 200];
        signal.extend(&reference);
        signal.extend(vec![0i16; 200]);
        let (lag, score) = cross_correlate_peak(&signal, &reference);
        assert_eq!(lag, 200);
        assert!(score > 0.0);
    }

    #[test]
    fn cross_correlate_empty_reference_is_zero_lag() {
        let signal = vec![100i16; 10];
        assert_eq!(cross_correlate_peak(&signal, &[]), (0, 0.0));
    }
}
