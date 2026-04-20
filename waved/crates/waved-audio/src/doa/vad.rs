//! Voice activity gate. Simple energy-threshold wrapper; the "real" VAD
//! is Speex's `speech_prob` but we expose it here as a trait so tests
//! can stub in deterministic behavior.

pub trait Vad {
    fn is_speech(&self, frame: &[f32]) -> bool;
}

/// Threshold-based VAD — "is speech" if frame RMS exceeds `threshold`.
/// Threshold 0.003 (~ -50 dBFS) is the default that rejects most room tone.
pub struct EnergyVad {
    threshold: f32,
}

impl EnergyVad {
    pub fn new(threshold: f32) -> Self {
        Self { threshold }
    }
}

impl Default for EnergyVad {
    fn default() -> Self {
        Self::new(0.003)
    }
}

impl Vad for EnergyVad {
    fn is_speech(&self, frame: &[f32]) -> bool {
        if frame.is_empty() {
            return false;
        }
        let sq: f32 = frame.iter().map(|x| x * x).sum();
        let rms = (sq / frame.len() as f32).sqrt();
        rms > self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_not_speech() {
        let v = EnergyVad::default();
        assert!(!v.is_speech(&vec![0.0; 480]));
    }

    #[test]
    fn loud_tone_is_speech() {
        let v = EnergyVad::default();
        let frame: Vec<f32> = (0..480).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        assert!(v.is_speech(&frame));
    }
}
