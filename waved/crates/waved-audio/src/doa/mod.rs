//! Direction-of-arrival estimation.
pub mod gcc_phat;
pub mod solve;
pub mod srp;
pub mod stream;
pub mod vad;

pub use solve::{DoaAngles, PairObservation};
pub use srp::SrpPhatEstimator;
pub use stream::{DoaBackend, DoaFrame, DoaPublisher};

use crate::beam::MicGeometry;
use gcc_phat::GccPhat;
use rustfft::num_complex::Complex;

/// Speed of sound in air (m/s). Used to compute expected per-pair τ for
/// the diagnostic dump.
const SPEED_OF_SOUND_M_S: f32 = 343.0;

/// A frame counts as speech when the correlation band is this far above the
/// tracked noise floor. The old fixed −70 dBFS threshold sat at this room's
/// floor, so the gaps between words counted as speech and gave random
/// directions.
const VAD_MARGIN_DB: f32 = 9.0;
/// The floor follows the band level down at once and up at this rate, so a
/// steady source becomes "noise" in seconds while words, with their gaps,
/// never lift it.
const FLOOR_RISE_DB_PER_S: f32 = 2.3;
/// The floor starts here and never drops below it (digital silence).
const FLOOR_MIN_DB: f32 = -100.0;

/// Physical mics the pairs use.
const PAIR_MICS: [usize; 6] = [0, 1, 2, 3, 4, 6];

/// Fixed set of pairs for the 8-mic array. Two top-ring horizontal
/// diagonals give azimuth; two *pure-Z* vertical pairs (same-XY top/bot
/// mics) give elevation.
///
/// We originally used the full 3D diagonals (0,6) and (1,7) for elevation
/// because they have the longest baseline. On-device 4-compass validation
/// (see docs/AUDIO_HARDWARE.md § "Cross-ring pair acoustic shadow") showed
/// those two pairs give mutually inconsistent τ — they disagree on
/// elevation by 15-25° at every azimuth. Root cause is body-diffraction:
/// when the source is in one hemisphere, one of the two bottom mics in
/// each diagonal pair is in the Wave body's acoustic shadow, so its
/// signal is smeared and GCC-PHAT lands on the wrong lag.
///
/// Pure-Z pairs sidestep the issue: both mics in a pair share XY, so they
/// see the same acoustic side of the device — any shadow applies to both
/// equally and cancels out of the time-difference estimate.
///
/// Indexed by PHYSICAL index (not ALSA); the estimator permutes internally.
const PAIR_INDICES: [(usize, usize); 4] = [
    (0, 2), // top 45°  ↔ top 225° (+X+Y horizontal diagonal)
    (1, 3), // top 135° ↔ top 315° (-X+Y horizontal diagonal)
    (0, 4), // top 45°  ↔ bot 45°  (pure-Z, +X+Y column)
    (2, 6), // top 225° ↔ bot 225° (pure-Z, -X-Y column)
];

/// Result of a single estimator update.
#[derive(Debug, Clone, Copy)]
pub struct DoaUpdate {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    pub vad_active: bool,
    /// False when solver couldn't recover a direction (degenerate input / no speech).
    pub fresh: bool,
    /// Mean coherence of the pairs, [0.0, 1.0]: how much of the band agrees
    /// on one delay per pair. 0 when !vad_active or the solver returned None.
    /// The UI's talker wedge and diarization trust a direction at ≥ 0.30.
    pub confidence: f32,
}

pub struct DoaEstimator {
    geom: MicGeometry,
    sample_rate: u32,
    frame_size: usize,
    gcc: GccPhat,
    /// Zero-padded spectra of the physical mics, refilled every frame.
    spectra: [Vec<Complex<f32>>; 8],
    /// Tracked band level of the room without speech (dB).
    noise_floor_db: f32,
    floor_rise_db: f32,
    smoothed_az: f32,
    smoothed_el: f32,
    alpha: f32,
    /// False until the first fresh solution seeds smoothed_az/el directly.
    primed: bool,
    /// Max lag between paired mics in samples. Computed from geometry.
    max_lag_samples: usize,
    /// Precomputed pair position deltas (meters) keyed by pair index.
    pair_deltas_m: [(f32, f32, f32); 4],
    /// When Some, emit one `debug_dump` log line every N frames with the raw
    /// per-pair τ and the τ expected given the solved direction. Set via
    /// `WAVED_DOA_DEBUG=<N>` at startup (0/unset = off). See lib.rs wiring.
    debug_every_n: Option<u32>,
    debug_frame_counter: u32,
}

impl DoaEstimator {
    pub fn new(geom: MicGeometry, sample_rate: u32, frame_size: usize) -> Self {
        // Max pairwise distance across our geometry: top-ring across is 2*R = 51 mm;
        // top-to-side diagonal is sqrt((R + R/√2)^2 + (R/√2)^2 + 78.2^2) ≈ 90 mm.
        // τ_max = 90e-3 / 343 ≈ 262 μs → 12.6 samples at 48 kHz. Add headroom.
        let max_lag_samples = 20;

        let mut pair_deltas_m = [(0.0f32, 0.0, 0.0); 4];
        for (k, &(i, j)) in PAIR_INDICES.iter().enumerate() {
            let pi = &geom.positions_mm[i];
            let pj = &geom.positions_mm[j];
            pair_deltas_m[k] = (
                (pi.x - pj.x) * 1e-3,
                (pi.y - pj.y) * 1e-3,
                (pi.z - pj.z) * 1e-3,
            );
        }

        let debug_every_n = std::env::var("WAVED_DOA_DEBUG")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&n| n > 0);

        Self {
            geom,
            sample_rate,
            frame_size,
            gcc: GccPhat::new(frame_size, sample_rate, PAIR_INDICES.len()),
            spectra: std::array::from_fn(|_| Vec::new()),
            noise_floor_db: FLOOR_MIN_DB,
            floor_rise_db: FLOOR_RISE_DB_PER_S * frame_size as f32 / sample_rate as f32,
            smoothed_az: 0.0,
            smoothed_el: 0.0,
            alpha: 0.9,
            primed: false,
            max_lag_samples,
            pair_deltas_m,
            debug_every_n,
            debug_frame_counter: 0,
        }
    }

    /// Process an 8-channel f32 interleaved frame, return current smoothed DOA.
    /// If VAD gates the frame or the solver fails, the previous smoothed value
    /// is returned unchanged. The pairs' cross-spectra take in every frame, so
    /// the first frames of a word already carry the ones before it.
    pub fn update(&mut self, interleaved: &[f32]) -> DoaUpdate {
        assert_eq!(interleaved.len(), self.frame_size * 8);

        // Extract per-physical-mic frame via ALSA permutation.
        let mut per_ch: [Vec<f32>; 8] =
            std::array::from_fn(|_| Vec::with_capacity(self.frame_size));
        for i in 0..self.frame_size {
            for alsa_ch in 0..8 {
                let physical = self.geom.alsa_to_physical[alsa_ch] as usize;
                per_ch[physical].push(interleaved[i * 8 + alsa_ch]);
            }
        }

        for &mic in &PAIR_MICS {
            self.gcc.spectrum(&per_ch[mic], &mut self.spectra[mic]);
        }

        // Compute TDOAs, sharpness and coherence for each pair.
        let mut obs = Vec::with_capacity(4);
        let mut raw_tau_samples = [0.0f32; 4];
        let mut sharpnesses = [0.0f32; 4];
        let mut coherence_sum = 0.0f32;
        for (k, &(i, j)) in PAIR_INDICES.iter().enumerate() {
            let d = self
                .gcc
                .estimate(k, &self.spectra[i], &self.spectra[j], self.max_lag_samples);
            raw_tau_samples[k] = d.tau_samples;
            sharpnesses[k] = d.sharpness;
            coherence_sum += d.coherence;
            let (dx, dy, dz) = self.pair_deltas_m[k];
            obs.push(PairObservation {
                delta_m: crate::beam::Position3D::new(dx, dy, dz),
                tdoa_sec: d.tau_samples / self.sample_rate as f32,
            });
        }

        // VAD on physical mic 0's band level against the tracked floor.
        let level_db = self.gcc.band_level_db(&self.spectra[0], self.frame_size);
        self.noise_floor_db = if level_db < self.noise_floor_db {
            level_db
        } else {
            self.noise_floor_db + self.floor_rise_db
        }
        .max(FLOOR_MIN_DB);
        let vad_active = level_db > self.noise_floor_db + VAD_MARGIN_DB;
        if !vad_active {
            return DoaUpdate {
                azimuth_deg: self.smoothed_az,
                elevation_deg: self.smoothed_el,
                vad_active: false,
                fresh: false,
                confidence: 0.0,
            };
        }

        // Optional diagnostic: one line every N frames with raw τ per pair,
        // expected τ for the current smoothed DOA, and RMS of channel 0.
        // Enables offline cross-check of hardware TDoAs against geometry.
        if let Some(n) = self.debug_every_n {
            self.debug_frame_counter = self.debug_frame_counter.wrapping_add(1);
            if self.debug_frame_counter % n == 0 {
                let az = self.smoothed_az.to_radians();
                let el = self.smoothed_el.to_radians();
                let dhat = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
                let sr = self.sample_rate as f32;
                let rms0 = {
                    let sq: f32 = per_ch[0].iter().map(|x| x * x).sum();
                    (sq / per_ch[0].len() as f32).sqrt()
                };
                let mut line = String::new();
                for k in 0..4 {
                    let (i, j) = PAIR_INDICES[k];
                    let (dx, dy, dz) = self.pair_deltas_m[k];
                    let expected_tau_sec =
                        (dx * dhat[0] + dy * dhat[1] + dz * dhat[2]) / SPEED_OF_SOUND_M_S;
                    let expected_tau_samples = expected_tau_sec * sr;
                    // Per-pair implied elevation: if this pair carried 100% of the
                    // direction, what el would it imply assuming our smoothed az?
                    // For pure-Z pairs (dz != 0, dx ≈ dy ≈ 0) this isolates the
                    // elevation contribution; for XY-diagonal pairs it's noisy but
                    // still useful for spotting outliers.
                    let baseline_len = (dx*dx + dy*dy + dz*dz).sqrt().max(1e-9);
                    let tau_meters = raw_tau_samples[k] / sr * SPEED_OF_SOUND_M_S;
                    let cos_angle_to_baseline = (tau_meters / baseline_len).clamp(-1.0, 1.0);
                    let el_implied_deg = if dz.abs() > 0.5 * baseline_len {
                        // Pure-Z-ish pair: cos_angle_to_baseline ≈ sin(el) (signed)
                        cos_angle_to_baseline.asin().to_degrees()
                    } else {
                        // XY-ish pair: no clean elevation; report NaN sentinel.
                        f32::NAN
                    };
                    line.push_str(&format!(
                        "({},{})raw={:+.2} exp={:+.2} sh={:.2} el_p={:+.1}  ",
                        i, j, raw_tau_samples[k], expected_tau_samples,
                        sharpnesses[k], el_implied_deg,
                    ));
                }
                tracing::info!(
                    target: "waved_audio::doa::debug",
                    "doa_debug az={:.1} el={:.1} rms0={:.5} | {}",
                    self.smoothed_az, self.smoothed_el, rms0, line,
                );
            }
        }

        let confidence = (coherence_sum / PAIR_INDICES.len() as f32).clamp(0.0, 1.0);

        // The solver weights the pairs by sharpness (scale-free); its
        // MIN_SHARPNESS_THRESHOLD guard still fires when no pair has a peak.
        match solve::solve_direction_weighted(&obs, &sharpnesses) {
            Some(ang) => {
                if self.primed {
                    self.smoothed_az =
                        smooth_angle_deg(self.smoothed_az, ang.azimuth_deg, self.alpha);
                    self.smoothed_el =
                        self.alpha * self.smoothed_el + (1.0 - self.alpha) * ang.elevation_deg;
                } else {
                    self.smoothed_az = ang.azimuth_deg;
                    self.smoothed_el = ang.elevation_deg;
                    self.primed = true;
                }
                DoaUpdate {
                    azimuth_deg: self.smoothed_az,
                    elevation_deg: self.smoothed_el,
                    vad_active: true,
                    fresh: true,
                    confidence,
                }
            }
            None => DoaUpdate {
                azimuth_deg: self.smoothed_az,
                elevation_deg: self.smoothed_el,
                vad_active: true,
                fresh: false,
                confidence: 0.0,
            },
        }
    }
}

/// Wrap-around-safe EWMA on angles in degrees via unit-vector smoothing.
fn smooth_angle_deg(prev: f32, new: f32, alpha: f32) -> f32 {
    let p = prev.to_radians();
    let n = new.to_radians();
    let sx = alpha * p.cos() + (1.0 - alpha) * n.cos();
    let sy = alpha * p.sin() + (1.0 - alpha) * n.sin();
    let out = sy.atan2(sx).to_degrees();
    if out < 0.0 {
        out + 360.0
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_plane_wave(
        az_deg: f32,
        el_deg: f32,
        sr: u32,
        frames: usize,
        geom: &MicGeometry,
    ) -> Vec<f32> {
        let az = az_deg.to_radians();
        let el = el_deg.to_radians();
        let d = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
        let freq = 1000.0f32;
        let omega = 2.0 * std::f32::consts::PI * freq / sr as f32;
        let mut out = vec![0.0; frames * 8];
        const C: f32 = 343.0;
        for alsa_ch in 0..8 {
            let p = geom.position_for_alsa(alsa_ch);
            let dot_mm = p.x * d[0] + p.y * d[1] + p.z * d[2];
            let tau_sec = -(dot_mm * 1e-3) / C;
            let tau_samples = tau_sec * sr as f32;
            for i in 0..frames {
                out[i * 8 + alsa_ch] = ((i as f32 - tau_samples) * omega).sin() * 0.5;
            }
        }
        out
    }

    #[test]
    fn recovers_doa_at_zero() {
        let geom = MicGeometry::default();
        let mut est = DoaEstimator::new(geom.clone(), 48000, 480);
        let frame = synth_plane_wave(0.0, 0.0, 48000, 480, &geom);
        let mut last = est.update(&frame);
        for _ in 0..20 {
            last = est.update(&frame);
        }
        assert!(last.vad_active, "synthetic tone should register as speech");
        assert!(last.fresh, "should have solved direction");
        let diff = last
            .azimuth_deg
            .min((360.0 - last.azimuth_deg).abs());
        assert!(diff < 5.0, "recovered az {} not ≈ 0", last.azimuth_deg);
    }

    #[test]
    fn recovers_doa_at_ninety() {
        let geom = MicGeometry::default();
        let mut est = DoaEstimator::new(geom.clone(), 48000, 480);
        let frame = synth_plane_wave(90.0, 0.0, 48000, 480, &geom);
        let mut last = est.update(&frame);
        for _ in 0..20 {
            last = est.update(&frame);
        }
        assert!(
            (last.azimuth_deg - 90.0).abs() < 5.0,
            "recovered az {} not ≈ 90",
            last.azimuth_deg
        );
    }

    #[test]
    fn silence_freezes_smoothed_doa() {
        let geom = MicGeometry::default();
        let mut est = DoaEstimator::new(geom.clone(), 48000, 480);
        let loud = synth_plane_wave(180.0, 0.0, 48000, 480, &geom);
        for _ in 0..30 {
            est.update(&loud);
        }
        let silence = vec![0.0f32; 480 * 8];
        let az_before = est.update(&silence).azimuth_deg;
        for _ in 0..50 {
            est.update(&silence);
        }
        let az_after = est.update(&silence).azimuth_deg;
        assert!(
            (az_before - az_after).abs() < 0.01,
            "DOA drifted during silence: {az_before} -> {az_after}"
        );
    }

    /// A voiced source (150 Hz fundamental, harmonics falling 12 dB/octave)
    /// arriving as a plane wave, about −50 dBFS at the mics, over
    /// independent room noise per mic at about −80 dBFS. Frame `index` of a
    /// continuous signal.
    fn voiced_frame(az_deg: f32, index: usize, frame: usize, voiced: bool, geom: &MicGeometry) -> Vec<f32> {
        let az = az_deg.to_radians();
        let d = [az.cos(), az.sin(), 0.0];
        let mut state = 0x2545_F491u32.wrapping_add(index as u32 * 7919);
        let mut out = vec![0.0; frame * 8];
        for alsa_ch in 0..8 {
            let p = geom.position_for_alsa(alsa_ch);
            let tau = -(p.x * d[0] + p.y * d[1] + p.z * d[2]) * 1e-3 / 343.0;
            for i in 0..frame {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let noise = ((state as f32 / u32::MAX as f32) - 0.5) * 3e-4;
                let t = (index * frame + i) as f32 / 48_000.0 - tau;
                let voice: f32 = if voiced {
                    (1..40)
                        .map(|k| {
                            let f = 150.0 * k as f32;
                            (2.0 * std::f32::consts::PI * f * t + k as f32).sin() / (k * k) as f32
                        })
                        .sum::<f32>()
                        * 3e-3
                } else {
                    0.0
                };
                out[i * 8 + alsa_ch] = voice + noise;
            }
        }
        out
    }

    #[test]
    fn a_quiet_voice_after_a_quiet_room_is_confident_and_points_the_right_way() {
        // Regression (device pass 2026-10-08): speech at 1 m got a direction
        // in 15 of 123 frames, so the talker wedge rarely showed.
        let geom = MicGeometry::default();
        let mut est = DoaEstimator::new(geom.clone(), 48_000, 1024);
        let mut index = 0;
        // The noise floor starts low and climbs to the room in a few
        // seconds, so early room noise may pass the VAD; it must never look
        // like a talker.
        for _ in 0..150 {
            let q = est.update(&voiced_frame(90.0, index, 1024, false, &geom));
            assert!(q.confidence < 0.30, "room noise read as a talker at frame {index}: {}", q.confidence);
            index += 1;
        }
        let mut confident = 0;
        let mut last = None;
        for _ in 0..30 {
            let u = est.update(&voiced_frame(90.0, index, 1024, true, &geom));
            index += 1;
            if u.vad_active && u.fresh && u.confidence >= 0.30 {
                confident += 1;
            }
            last = Some(u);
        }
        let last = last.unwrap();
        assert!(confident >= 25, "only {confident} of 30 voiced frames were confident");
        assert!((last.azimuth_deg - 90.0).abs() < 10.0, "az {}", last.azimuth_deg);
    }

    #[test]
    fn smoothing_wraps_around_360_zero_boundary() {
        let smoothed = smooth_angle_deg(359.0, 1.0, 0.5);
        let diff = smoothed.min(360.0 - smoothed);
        assert!(diff < 1.0, "smoothed {} should be near 0", smoothed);
    }

    /// Synthesizes an 8-ch broadband-noise plane wave with one mic's signal
    /// degraded to simulate body-shadow.
    ///
    /// Uses broadband noise as the source (not a pure tone) because GCC-PHAT
    /// sharpness discrimination requires broadband stimulus: with a pure tone,
    /// PHAT normalisation gives all frequency bins unit magnitude, so the noisy
    /// channel can produce a spurious sharp peak at the wrong lag, making
    /// sharpness an unreliable discriminator.
    fn synth_with_shadow(
        az_deg: f32,
        el_deg: f32,
        sr: u32,
        frames: usize,
        geom: &MicGeometry,
        shadowed_alsa_ch: usize,
    ) -> Vec<f32> {
        let az = az_deg.to_radians();
        let el = el_deg.to_radians();
        let d = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
        let mut out = vec![0.0; frames * 8];
        const C: f32 = 343.0;

        // Broadband source signal seeded identically on each call for
        // determinism. Integer-shifted per mic to simulate propagation delay.
        let mut src_state = 0x9E3779B1u32;
        let src_len = frames + 32;
        let mut src_signal: Vec<f32> = (0..src_len)
            .map(|_| {
                src_state ^= src_state << 13;
                src_state ^= src_state >> 17;
                src_state ^= src_state << 5;
                (src_state as f32 / u32::MAX as f32) - 0.5
            })
            .collect();
        for s in &mut src_signal {
            *s *= 0.5;
        }

        let mut noise_state = 0xDEADBEEFu32;
        let mut rnd = || {
            noise_state ^= noise_state << 13;
            noise_state ^= noise_state >> 17;
            noise_state ^= noise_state << 5;
            (noise_state as f32 / u32::MAX as f32) - 0.5
        };

        for alsa_ch in 0..8 {
            let p = geom.position_for_alsa(alsa_ch);
            let dot_mm = p.x * d[0] + p.y * d[1] + p.z * d[2];
            let tau_sec = -(dot_mm * 1e-3) / C;
            let tau_samples = (tau_sec * sr as f32).round() as i32;
            for i in 0..frames {
                let src_idx = (i as i32 - tau_samples).clamp(0, src_len as i32 - 1) as usize;
                let clean = src_signal[src_idx];
                // Shadowed mic: 80% noise, 20% signal (simulates diffraction smear).
                out[i * 8 + alsa_ch] = if alsa_ch == shadowed_alsa_ch {
                    0.4 * rnd() + 0.1 * clean
                } else {
                    clean
                };
            }
        }
        out
    }

    /// Smoke-test the weighted-LS wiring through DoaEstimator::update on a
    /// recoverable scenario: az=270°, one mic on a pure-Z pair (alsa ch 1 →
    /// physical 4, pair (0,4)) replaced by 80% noise. Verifies the new code
    /// path produces a sensible output without panicking and recovers
    /// azimuth within ±5° when the corrupted pair is azimuth-irrelevant.
    ///
    /// **This test does NOT prove weighted-LS helps:** pure-Z pair (0,4)'s
    /// weight does not affect recovered azimuth (the pair only contributes
    /// to A[2][2] and b[2]). The test would pass with weights [1,1,0,1] or
    /// [1,1,1,1] identically. Weighted-LS *mechanism* correctness is verified
    /// by `doa::solve::tests::weighted_ls_zero_weight_pair_ignored`.
    ///
    /// **Production 270° fix:** see SRP-PHAT (Tasks 8-11) and on-device
    /// `scripts/test-doa-improvements.sh` test 1 (Task 17).
    #[test]
    fn update_with_weighted_solver_recovers_az_on_recoverable_scenario() {
        // Shadow alsa ch 1 (physical 4, BOT 45°) — participates in Z-pair (0,4)
        // only. The two horizontal pairs remain clean, so azimuth is fully
        // constrained. The corrupted Z-pair is down-weighted by its low
        // sharpness, keeping the normal matrix well-conditioned.
        let geom = MicGeometry::default();
        let mut est = DoaEstimator::new(geom.clone(), 48000, 480);
        for _ in 0..40 {
            let frame = synth_with_shadow(270.0, 0.0, 48000, 480, &geom, 1);
            est.update(&frame);
        }
        let last = est.update(&synth_with_shadow(270.0, 0.0, 48000, 480, &geom, 1));
        assert!(last.fresh, "should solve");
        let raw = (last.azimuth_deg - 270.0).abs();
        let err = raw.min(360.0 - raw);
        assert!(
            err < 5.0,
            "weighted LS should resolve az=270 ±5°, got {:.1}° (err {:.1}°)",
            last.azimuth_deg, err
        );
    }

}
