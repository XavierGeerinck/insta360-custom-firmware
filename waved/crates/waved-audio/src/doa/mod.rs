//! Direction-of-arrival estimation.
pub mod gcc_phat;
pub mod solve;
pub mod vad;

pub use solve::{DoaAngles, PairObservation};

use crate::beam::MicGeometry;
use gcc_phat::GccPhat;
use vad::{EnergyVad, Vad};

/// Speed of sound in air (m/s). Used to compute expected per-pair τ for
/// the diagnostic dump.
const SPEED_OF_SOUND_M_S: f32 = 343.0;

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
}

pub struct DoaEstimator {
    geom: MicGeometry,
    sample_rate: u32,
    frame_size: usize,
    gcc: GccPhat,
    vad: EnergyVad,
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
            gcc: GccPhat::new(frame_size),
            vad: EnergyVad::default(),
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
    /// If VAD gates the frame (below threshold) or the solver fails, the
    /// previous smoothed value is returned unchanged.
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

        // VAD on channel-0 physical (reasonable proxy for overall energy).
        let vad_active = self.vad.is_speech(&per_ch[0]);
        if !vad_active {
            return DoaUpdate {
                azimuth_deg: self.smoothed_az,
                elevation_deg: self.smoothed_el,
                vad_active: false,
                fresh: false,
            };
        }

        // Compute TDOAs for each pair; build PairObservations.
        let mut obs = Vec::with_capacity(4);
        let mut raw_tau_samples = [0.0f32; 4];
        for (k, &(i, j)) in PAIR_INDICES.iter().enumerate() {
            let (tau_samples, _sharpness) = self
                .gcc
                .estimate_tdoa(&per_ch[i], &per_ch[j], self.max_lag_samples);
            raw_tau_samples[k] = tau_samples;
            // (sharpness wired into solve in Task 3)
            let (dx, dy, dz) = self.pair_deltas_m[k];
            obs.push(PairObservation {
                delta_m: crate::beam::Position3D::new(dx, dy, dz),
                tdoa_sec: tau_samples / self.sample_rate as f32,
            });
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
                    line.push_str(&format!(
                        "({},{})raw={:+.2} exp={:+.2}  ",
                        i, j, raw_tau_samples[k], expected_tau_samples,
                    ));
                }
                tracing::info!(
                    target: "waved_audio::doa::debug",
                    "doa_debug az={:.1} el={:.1} rms0={:.5} | {}",
                    self.smoothed_az, self.smoothed_el, rms0, line,
                );
            }
        }

        match solve::solve_direction(&obs) {
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
                }
            }
            None => DoaUpdate {
                azimuth_deg: self.smoothed_az,
                elevation_deg: self.smoothed_el,
                vad_active: true,
                fresh: false,
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

    #[test]
    fn smoothing_wraps_around_360_zero_boundary() {
        let smoothed = smooth_angle_deg(359.0, 1.0, 0.5);
        let diff = smoothed.min(360.0 - smoothed);
        assert!(diff < 1.0, "smoothed {} should be near 0", smoothed);
    }
}
