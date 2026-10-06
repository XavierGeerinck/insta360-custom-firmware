//! Fixed make-up gain between residual-echo suppression and noise suppression.
//!
//! The beam carries conversational speech at about −44 dBFS ~1 m away (device
//! pass 2026-10-06, after the echo canceller's 48 kHz fix). DeepFilterNet
//! treats quiet speech as noise: offline it removed 10.5 dB of a −60 dBFS
//! sentence at NR Low and 20 dB at High, but nothing at −40 dBFS or louder.
//! The default +18 dB puts speech near −26 dBFS before the suppressor; the
//! user's mic level and the recording limiters still follow downstream. A soft knee above half scale
//! keeps loud, close speech from clipping the i16 stage.

/// Linear gain for a make-up gain in dB.
pub fn db_to_linear(db: u8) -> f32 {
    10f32.powf(f32::from(db) / 20.0)
}

/// Where the soft knee starts, as a fraction of full scale.
const KNEE: f32 = 0.5;

/// Apply `gain` in place. Below the knee the gain is exact; above it the
/// output bends smoothly towards full scale and never exceeds it. A gain of
/// 1.0 leaves the samples untouched.
pub fn apply(samples: &mut [i16], gain: f32) {
    if gain == 1.0 {
        return;
    }
    for sample in samples.iter_mut() {
        let x = f32::from(*sample) / 32768.0 * gain;
        let magnitude = x.abs();
        let y = if magnitude <= KNEE {
            magnitude
        } else {
            KNEE + (1.0 - KNEE) * ((magnitude - KNEE) / (1.0 - KNEE)).tanh()
        };
        *sample = (y.copysign(x) * 32767.0).round() as i16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_speech_gets_the_exact_gain() {
        let gain = db_to_linear(24);
        assert!((gain - 15.85).abs() < 0.01);
        let mut samples = [100i16, -100, 500, -1000, 0];
        apply(&mut samples, gain);
        assert_eq!(samples, [1585, -1585, 7924, -15848, 0]);
    }

    #[test]
    fn loud_speech_bends_below_full_scale_without_clipping() {
        let gain = db_to_linear(24);
        let input: Vec<i16> = (-32768..=32767).step_by(97).map(|s| s as i16).collect();
        let mut out = input.clone();
        apply(&mut out, gain);
        for pair in input.iter().zip(&out).collect::<Vec<_>>().windows(2) {
            let ((a_in, a_out), (b_in, b_out)) = (pair[0], pair[1]);
            assert!(b_in > a_in && b_out >= a_out, "monotonic at {a_in}..{b_in}");
        }
        assert!(out.iter().all(|&s| s > i16::MIN), "never past full scale");
        let mut peak = [i16::MAX, i16::MIN];
        apply(&mut peak, gain);
        assert!(peak[0] > 32000 && peak[1] < -32000, "{peak:?}");
    }

    #[test]
    fn zero_db_is_identity() {
        let input = [32767i16, -32768, 20000, -5, 0];
        let mut out = input;
        apply(&mut out, db_to_linear(0));
        assert_eq!(out, input);
    }
}
