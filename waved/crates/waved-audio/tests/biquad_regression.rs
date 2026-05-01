//! Bit-exact regression: our `Biquad` direct-form-1 + RBJ peaking-EQ designer
//! must reproduce the reference impulse response (computed in Python f64) to
//! within 1e-6.
//!
//! If this test fires, EITHER the biquad form changed OR a designer coefficient
//! changed sign — both silently break stock-tuning compatibility, so catch them
//! before they reach the EQ chain.

use waved_audio::playback::dsp::biquad::{Biquad, design_peaking_eq};

/// 16-sample impulse response of `design_peaking_eq(1000, 6, 1, 48000)`,
/// computed in Python f64 with the same direct-form-1 recurrence.
const REFERENCE_IMPULSE_RESPONSE: [f64; 16] = [
    1.043953087,
    0.08330519665,
    0.07386603172,
    0.06405252458,
    0.05405823533,
    0.04406258457,
    0.03422916789,
    0.0247044781,
    0.01561701995,
    0.007076797303,
    -0.000824851879,
    -0.008015100673,
    -0.01443918927,
    -0.02005972477,
    -0.02485575883,
    -0.02882167778,
];

#[test]
fn peaking_impulse_response_matches_reference() {
    let coeffs = design_peaking_eq(1000.0, 6.0, 1.0, 48_000.0);
    let mut b = Biquad::new(coeffs);
    let mut buf = [0.0_f32; 16];
    buf[0] = 1.0;
    b.process_block(&mut buf);

    for (i, (got, exp)) in buf.iter().zip(REFERENCE_IMPULSE_RESPONSE.iter()).enumerate() {
        let g = *got as f64;
        let diff = (g - *exp).abs();
        assert!(
            diff < 1e-6,
            "sample {i}: got {g:.10}, expected {exp:.10}, diff {diff:.3e}"
        );
    }
}
