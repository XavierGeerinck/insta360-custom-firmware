//! Voice shaping on the uplink mid, after echo control and before the
//! make-up gain: late-reverb suppression (`crate::dereverb`), then an EQ.
//!
//! At the mics the voice is bass-heavy (the fundamental near 110 Hz sits
//! ~16 dB over 400-630 Hz) and the 8-mic sum loses treble to the diffuse
//! reverb, so calls sounded muffled and roomy. Stock firmware also shapes
//! its mic path (`/etc/dynamic_eq_config.json`: 150 Hz high-pass, −6 dB at
//! 90 Hz, +5 dB from 8 kHz). This chain is the stronger version picked by
//! ear from a room recording on 2026-10-07 (clip 4): clearer and closer than
//! the stock curve. It cuts about 6 dB of mostly bass energy, which the
//! closing `LEVEL_DB` restores so the voice reaches the denoiser and the
//! far end at the level it did before.

use crate::dereverb::Dereverb;
use crate::playback::dsp::biquad::{
    design_high_shelf, design_highpass, design_low_shelf, design_peaking_eq, Biquad,
};

/// Level restored after the EQ, in dB.
const LEVEL_DB: f32 = 5.0;

/// One EQ section. Sections whose corner is near Nyquist are skipped.
enum Section {
    /// Butterworth high-pass section: cutoff, Q.
    HighPass(f32, f32),
    LowShelf(f32, f32, f32),
    Peaking(f32, f32, f32),
    HighShelf(f32, f32, f32),
}

/// Corner Hz, gain dB, Q.
const VOICE_EQ: [Section; 6] = [
    // 4th-order Butterworth at 160 Hz: the fundamental's boom and rumble.
    Section::HighPass(160.0, 0.541_196_1),
    Section::HighPass(160.0, 1.306_563),
    // Less chest below 300 Hz.
    Section::LowShelf(300.0, -5.0, 0.7),
    // Presence: consonants carry intelligibility here.
    Section::Peaking(3_000.0, 5.0, 1.0),
    // Give back the treble the 8-mic sum cancels, then air (as stock).
    Section::HighShelf(2_000.0, 7.0, 0.7),
    Section::HighShelf(8_000.0, 5.0, 0.7),
];

/// The voice EQ and the level it restores. The Stereo side runs its own
/// copy so both channels carry the same voice curve.
pub struct VoiceEq {
    sections: Vec<Biquad>,
    gain: f32,
}

impl VoiceEq {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f32;
        let sections = VOICE_EQ
            .iter()
            .filter_map(|section| {
                let corner = match *section {
                    Section::HighPass(f, _)
                    | Section::LowShelf(f, ..)
                    | Section::Peaking(f, ..)
                    | Section::HighShelf(f, ..) => f,
                };
                if corner >= 0.45 * sr {
                    return None;
                }
                Some(Biquad::new(match *section {
                    Section::HighPass(f, q) => design_highpass(f, q, sr),
                    Section::LowShelf(f, g, q) => design_low_shelf(f, g, q, sr),
                    Section::Peaking(f, g, q) => design_peaking_eq(f, g, q, sr),
                    Section::HighShelf(f, g, q) => design_high_shelf(f, g, q, sr),
                }))
            })
            .collect();
        Self {
            sections,
            gain: 10f32.powf(LEVEL_DB / 20.0),
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        for section in &mut self.sections {
            section.process_block(samples);
        }
        for sample in samples.iter_mut() {
            *sample *= self.gain;
        }
    }

    /// Forget the filter history (the input restarts from silence).
    pub fn reset(&mut self) {
        for section in &mut self.sections {
            section.reset();
        }
    }
}

pub struct UplinkVoice {
    dereverb: Dereverb,
    eq: VoiceEq,
    scratch: Vec<f32>,
}

impl UplinkVoice {
    pub fn new(frame_size: usize, sample_rate: u32) -> Self {
        Self {
            dereverb: Dereverb::new(frame_size, sample_rate),
            eq: VoiceEq::new(sample_rate),
            scratch: vec![0.0; frame_size],
        }
    }

    /// How far the output lags the input, in samples.
    pub fn latency_samples(&self) -> usize {
        self.dereverb.latency_samples()
    }

    pub fn process(&mut self, samples: &mut [i16]) {
        self.scratch.resize(samples.len(), 0.0);
        for (dst, &src) in self.scratch.iter_mut().zip(samples.iter()) {
            *dst = f32::from(src) / 32768.0;
        }
        self.dereverb.process(&mut self.scratch);
        self.eq.process(&mut self.scratch);
        for (dst, &src) in samples.iter_mut().zip(&self.scratch) {
            *dst = (src * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steady-state level of a sine through the stage, in dB re the input.
    fn gain_db(freq: f32, sample_rate: u32) -> f32 {
        let frame = 1024;
        let mut voice = UplinkVoice::new(frame, sample_rate);
        let amp = 0.05;
        let mut input: Vec<i16> = (0..sample_rate as usize * 2)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (amp * (2.0 * std::f32::consts::PI * freq * t).sin() * 32768.0) as i16
            })
            .collect();
        let reference = input.clone();
        for chunk in input.chunks_mut(frame) {
            voice.process(chunk);
        }
        let rms = |v: &[i16]| {
            (v.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        let half = input.len() / 2;
        20.0 * (rms(&input[half..]) / rms(&reference[half..])).log10() as f32
    }

    #[test]
    fn cuts_the_boom_and_lifts_the_consonants_relative_to_the_vowels() {
        let mid = gain_db(1_000.0, 48_000);
        let boom = gain_db(110.0, 48_000);
        let presence = gain_db(4_000.0, 48_000);
        let air = gain_db(10_000.0, 48_000);
        assert!(mid - boom > 15.0, "110 Hz only {:.1} dB under 1 kHz", mid - boom);
        assert!(presence - mid > 6.0, "4 kHz only {:.1} dB over 1 kHz", presence - mid);
        assert!(air - mid > 8.0, "10 kHz only {:.1} dB over 1 kHz", air - mid);
    }

    #[test]
    fn keeps_the_vowel_band_near_the_restored_level() {
        // A steady tone also loses the dereverb's stationary share (about
        // 1.7 dB), so the 1 kHz gain sits a little under LEVEL_DB + the EQ.
        let mid = gain_db(1_000.0, 48_000);
        assert!((3.0..=9.0).contains(&mid), "1 kHz gain {mid:.1} dB");
    }

    #[test]
    fn skips_sections_above_nyquist_at_low_rates() {
        let voice = UplinkVoice::new(320, 16_000);
        assert_eq!(voice.eq.sections.len(), 5);
        assert!(gain_db(1_000.0, 16_000).is_finite());
    }

    #[test]
    fn reports_the_dereverb_latency() {
        assert_eq!(UplinkVoice::new(1024, 48_000).latency_samples(), 768);
    }
}
