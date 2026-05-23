//! SRP-PHAT direction-of-arrival estimator.
//!
//! Grid search over candidate directions, accumulating PHAT-weighted
//! cross-spectrum energy across the 4 mic pairs from doa/mod.rs.
//! Voice-band limited (200–4000 Hz). Trades 5-10× CPU vs GCC-PHAT for
//! more robust localisation in reverberant rooms.
//!
//! Selected via `AudioConfig.doa_backend = "srp_phat"`. Default backend
//! remains GCC-PHAT; SRP is opt-in pending on-device acceptance (§5.2 of
//! the F2 spec — gate: SRP CPU delta < 5%).

use crate::beam::MicGeometry;
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use std::sync::Arc;

/// Speed of sound in air (m/s). Matches the constant in doa::solve.
const SPEED_OF_SOUND_M_S: f32 = 343.0;

/// Voice-band bin range — out-of-band PHAT bins are skipped, halving
/// per-direction work and sharpening speech localisation.
const VOICE_BAND_LO_HZ: f32 = 200.0;
const VOICE_BAND_HI_HZ: f32 = 4000.0;

/// Candidate-direction grid resolution.
const AZ_STEP_DEG: f32 = 5.0;
const EL_STEP_DEG: f32 = 10.0;

/// Same 4 pairs as gcc_phat — see doa/mod.rs PAIR_INDICES rationale.
const PAIR_INDICES: [(usize, usize); 4] = [(0, 2), (1, 3), (0, 4), (2, 6)];

#[derive(Debug, Clone, Copy)]
pub struct GridDirection {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    /// Cartesian unit vector (x, y, z) precomputed at construction time.
    pub dhat: [f32; 3],
}

pub struct SrpPhatEstimator {
    geom: MicGeometry,
    sample_rate: u32,
    frame_size: usize,
    fft_len: usize,
    planner_fwd: Arc<dyn rustfft::Fft<f32>>,
    grid: Vec<GridDirection>,
    /// Per-pair position delta in meters; matches gcc_phat's pair_deltas_m.
    pair_deltas_m: [(f32, f32, f32); 4],
    /// Min/max FFT bin to include in PHAT accumulation, voice-band masked.
    bin_lo: usize,
    bin_hi: usize,
}

impl SrpPhatEstimator {
    pub fn new(geom: MicGeometry, sample_rate: u32, frame_size: usize) -> Self {
        let fft_len = (frame_size * 2).next_power_of_two();
        let mut planner = FftPlanner::<f32>::new();
        let planner_fwd = planner.plan_fft_forward(fft_len);

        let grid = build_grid();

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

        let bin_hz = sample_rate as f32 / fft_len as f32;
        let bin_lo = (VOICE_BAND_LO_HZ / bin_hz).floor() as usize;
        let bin_hi = ((VOICE_BAND_HI_HZ / bin_hz).ceil() as usize).min(fft_len / 2);

        Self {
            geom,
            sample_rate,
            frame_size,
            fft_len,
            planner_fwd,
            grid,
            pair_deltas_m,
            bin_lo,
            bin_hi,
        }
    }

    pub fn grid_len(&self) -> usize {
        self.grid.len()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SrpResult {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    pub confidence: f32,
}

impl SrpPhatEstimator {
    /// Estimate source direction from one ALSA-interleaved 8-channel frame.
    /// Returns None only when the input is wrong-sized; on silence or noise
    /// it returns Some with low `confidence`.
    pub fn estimate(&mut self, interleaved: &[f32]) -> Option<SrpResult> {
        if interleaved.len() != self.frame_size * 8 {
            return None;
        }

        // Demultiplex 8 ALSA channels into 8 per-physical-mic buffers, then
        // forward-FFT each. Use a fresh local Vec per channel — the previous
        // grid result has no carry-over state.
        let mut per_ch_fft: [Vec<Complex<f32>>; 8] = std::array::from_fn(|_| {
            vec![Complex::new(0.0, 0.0); self.fft_len]
        });
        for i in 0..self.frame_size {
            for alsa_ch in 0..8 {
                let physical = self.geom.alsa_to_physical[alsa_ch] as usize;
                per_ch_fft[physical][i] =
                    Complex::new(interleaved[i * 8 + alsa_ch], 0.0);
            }
        }
        for ch in 0..8 {
            self.planner_fwd.process(&mut per_ch_fft[ch]);
        }

        // For each pair, precompute PHAT-normalized cross-spectrum C_ij(f) =
        // X_i(f)·conj(X_j(f)) / |X_i(f)·conj(X_j(f))|. Zero out-of-band bins.
        let mut cross_per_pair: [Vec<Complex<f32>>; 4] = std::array::from_fn(|_| {
            vec![Complex::new(0.0, 0.0); self.bin_hi - self.bin_lo]
        });
        for (k, &(i, j)) in PAIR_INDICES.iter().enumerate() {
            for (idx, bin) in (self.bin_lo..self.bin_hi).enumerate() {
                let xi = per_ch_fft[i][bin];
                let xj_conj = per_ch_fft[j][bin].conj();
                let num = xi * xj_conj;
                let mag = num.norm();
                cross_per_pair[k][idx] = if mag > 1e-10 {
                    num / mag
                } else {
                    Complex::new(0.0, 0.0)
                };
            }
        }

        // Steered response: sum over pairs of sum over bins of Re[C_ij(f) ·
        // exp(j·2π·f·τ_ij(d̂))]. τ_ij(d̂) = (p_i - p_j) · d̂ / c, in seconds.
        let bin_hz = self.sample_rate as f32 / self.fft_len as f32;
        let two_pi = 2.0 * std::f32::consts::PI;
        let mut best_idx = 0usize;
        let mut best_power = f32::NEG_INFINITY;
        let mut power_sum = 0.0f32;
        let mut powers: Vec<f32> = Vec::with_capacity(self.grid.len());
        for (g_idx, g) in self.grid.iter().enumerate() {
            let mut total = 0.0f32;
            for (k, &(_i, _j)) in PAIR_INDICES.iter().enumerate() {
                let (dx, dy, dz) = self.pair_deltas_m[k];
                let proj = dx * g.dhat[0] + dy * g.dhat[1] + dz * g.dhat[2];
                // Negative sign: wave from d̂ arrives at mic i before mic j
                // when (p_i - p_j)·d̂ > 0, so τ_ij = -(p_i - p_j)·d̂ / c.
                let tau_sec = -proj / SPEED_OF_SOUND_M_S;
                for (idx, bin) in (self.bin_lo..self.bin_hi).enumerate() {
                    let f_hz = bin as f32 * bin_hz;
                    let phase = two_pi * f_hz * tau_sec;
                    let (sp, cp) = phase.sin_cos();
                    let exp_jp = Complex::new(cp, sp);
                    let c = cross_per_pair[k][idx];
                    total += (c * exp_jp).re;
                }
            }
            powers.push(total);
            power_sum += total;
            if total > best_power {
                best_power = total;
                best_idx = g_idx;
            }
        }

        let best = &self.grid[best_idx];
        let _mean_power = power_sum / self.grid.len() as f32;
        // Maximum possible steered response: all PHAT bins coherent across all
        // pairs (magnitude 1 each bin, n_pairs pairs, n_bins bins).
        let max_possible = PAIR_INDICES.len() as f32 * (self.bin_hi - self.bin_lo) as f32;
        // Confidence: normalised peak height. On a clean plane wave all bins
        // phase-align ⇒ power_peak ≈ max_possible. On noise, random phases
        // cancel and the peak is O(√n_bins) ≪ max_possible.
        let confidence = (best_power / max_possible).clamp(0.0, 1.0);

        // Sub-sample parabolic refinement in az and el. Grid is stored
        // elevation-major (n_az_cells azimuths per elevation row), so:
        //   az neighbor:  same el row, az±1 (wrap on row)
        //   el neighbor:  same az col, el±1 (clamp at ±90°; no wrap)
        let n_az = (360.0 / AZ_STEP_DEG) as usize;
        let n_el = ((180.0 / EL_STEP_DEG) as usize) + 1;
        let ei = best_idx / n_az;
        let ai = best_idx % n_az;

        let p_c = powers[best_idx];
        let ai_l = (ai + n_az - 1) % n_az;
        let ai_r = (ai + 1) % n_az;
        let p_az_l = powers[ei * n_az + ai_l];
        let p_az_r = powers[ei * n_az + ai_r];
        let az_denom = p_az_l - 2.0 * p_c + p_az_r;
        let az_frac = if az_denom < -1e-9 {
            (0.5 * (p_az_l - p_az_r) / az_denom).clamp(-0.5, 0.5)
        } else {
            0.0
        };

        let el_frac = if ei == 0 || ei == n_el - 1 {
            0.0 // polar boundary; no neighbor on one side
        } else {
            let p_el_l = powers[(ei - 1) * n_az + ai];
            let p_el_r = powers[(ei + 1) * n_az + ai];
            let el_denom = p_el_l - 2.0 * p_c + p_el_r;
            if el_denom < -1e-9 {
                (0.5 * (p_el_l - p_el_r) / el_denom).clamp(-0.5, 0.5)
            } else {
                0.0
            }
        };

        let mut refined_az = best.azimuth_deg + az_frac * AZ_STEP_DEG;
        if refined_az < 0.0 { refined_az += 360.0; }
        if refined_az >= 360.0 { refined_az -= 360.0; }
        let refined_el = (best.elevation_deg + el_frac * EL_STEP_DEG).clamp(-90.0, 90.0);

        Some(SrpResult {
            azimuth_deg: refined_az,
            elevation_deg: refined_el,
            confidence,
        })
    }
}

fn build_grid() -> Vec<GridDirection> {
    let mut grid = Vec::new();
    let n_az = (360.0 / AZ_STEP_DEG) as i32; // 72
    let n_el = ((180.0 / EL_STEP_DEG) as i32) + 1; // 19, from -90..=+90
    for ei in 0..n_el {
        let el_deg = -90.0 + EL_STEP_DEG * ei as f32;
        let el = el_deg.to_radians();
        for ai in 0..n_az {
            let az_deg = AZ_STEP_DEG * ai as f32;
            let az = az_deg.to_radians();
            let dhat = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
            grid.push(GridDirection {
                azimuth_deg: az_deg,
                elevation_deg: el_deg,
                dhat,
            });
        }
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_plane_wave_srp(
        az_deg: f32, el_deg: f32, sr: u32, frames: usize, geom: &MicGeometry,
    ) -> Vec<f32> {
        let az = az_deg.to_radians();
        let el = el_deg.to_radians();
        let d = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
        let freq = 1500.0f32; // voice-band
        let omega = 2.0 * std::f32::consts::PI * freq / sr as f32;
        let mut out = vec![0.0; frames * 8];
        const C: f32 = 343.0;
        for alsa_ch in 0..8 {
            let p = geom.position_for_alsa(alsa_ch);
            let dot_mm = p.x * d[0] + p.y * d[1] + p.z * d[2];
            let tau_samples = (-(dot_mm * 1e-3) / C) * sr as f32;
            for i in 0..frames {
                out[i * 8 + alsa_ch] = ((i as f32 - tau_samples) * omega).sin() * 0.5;
            }
        }
        out
    }

    #[test]
    fn srp_locates_plane_wave_within_grid_cell() {
        let geom = MicGeometry::default();
        let mut est = SrpPhatEstimator::new(geom.clone(), 48000, 480);
        let frame = synth_plane_wave_srp(45.0, 0.0, 48000, 480, &geom);
        let result = est.estimate(&frame).expect("plane wave should localise");
        let az_err = (result.azimuth_deg - 45.0).abs();
        eprintln!("srp az={:.1}° el={:.1}° confidence={:.3}", result.azimuth_deg, result.elevation_deg, result.confidence);
        assert!(az_err < AZ_STEP_DEG, "SRP az {} not within ±{}°", result.azimuth_deg, AZ_STEP_DEG);
        assert!(result.elevation_deg.abs() < EL_STEP_DEG);
        assert!(result.confidence > 0.05, "should have decent confidence on clean source, got {}", result.confidence);
    }

    #[test]
    fn srp_low_confidence_on_uniform_noise() {
        let geom = MicGeometry::default();
        let mut est = SrpPhatEstimator::new(geom.clone(), 48000, 480);
        let mut state = 0xC0FFEEu32;
        let mut rnd = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32) - 0.5
        };
        // Independent noise per channel — no shared direction.
        let frame: Vec<f32> = (0..480 * 8).map(|_| rnd() * 0.5).collect();
        let result = est.estimate(&frame).expect("noise still returns Some");
        eprintln!("noise confidence={:.3}", result.confidence);
        assert!(result.confidence < 0.3, "noise should give low confidence, got {}", result.confidence);
    }

    #[test]
    fn srp_locates_az_270_within_grid() {
        let geom = MicGeometry::default();
        let mut est = SrpPhatEstimator::new(geom.clone(), 48000, 480);
        let frame = synth_plane_wave_srp(270.0, 0.0, 48000, 480, &geom);
        let result = est.estimate(&frame).expect("should localise");
        let az_err = ((result.azimuth_deg - 270.0).abs()).min(360.0 - (result.azimuth_deg - 270.0).abs());
        eprintln!("srp az270: az={:.1}° el={:.1}° confidence={:.3}", result.azimuth_deg, result.elevation_deg, result.confidence);
        assert!(az_err < AZ_STEP_DEG, "SRP az {} not within ±{}° of 270", result.azimuth_deg, AZ_STEP_DEG);
    }

    #[test]
    fn srp_grid_has_expected_cardinality() {
        // 72 az × 19 el = 1368
        let g = build_grid();
        assert_eq!(g.len(), 1368);
    }

    #[test]
    fn srp_estimator_constructs_without_panic() {
        let geom = MicGeometry::default();
        let est = SrpPhatEstimator::new(geom, 48000, 480);
        assert_eq!(est.grid_len(), 1368);
        // Voice band 200..4000 at fft_len 1024 → bin_hz ≈ 46.875.
        // bin_lo ≈ 4, bin_hi ≈ 86. Sanity-bound.
        assert!(est.bin_lo > 0);
        assert!(est.bin_hi < est.fft_len / 2);
        assert!(est.bin_hi > est.bin_lo + 10);
    }

    #[test]
    fn srp_grid_contains_cardinal_azimuths() {
        let g = build_grid();
        let azs: Vec<f32> = g.iter().map(|d| d.azimuth_deg).collect();
        for &target in &[0.0, 90.0, 180.0, 270.0] {
            assert!(
                azs.iter().any(|&a| (a - target).abs() < 0.01),
                "grid missing az={target}"
            );
        }
    }

    #[test]
    fn srp_refines_to_sub_grid_accuracy() {
        // Source at az=47.5° (between grid cells 45° and 50°). Without
        // refinement the result would snap to 45° or 50°; with parabolic
        // interp it should land closer to truth.
        let geom = MicGeometry::default();
        let mut est = SrpPhatEstimator::new(geom.clone(), 48000, 480);
        let frame = synth_plane_wave_srp(47.5, 0.0, 48000, 480, &geom);
        let result = est.estimate(&frame).expect("should localise");
        let az_err = (result.azimuth_deg - 47.5).abs();
        assert!(az_err < 2.0,
            "refined az {} should be within ±2° of 47.5", result.azimuth_deg);
    }

    #[test]
    fn srp_az_boundary_wraps_correctly() {
        // Source at az=185°. Grid cells are at 180° and 185°; the parabolic-interp
        // right-neighbor of 185° is at 190°, which is still in range. To
        // genuinely exercise the wrap, use a source at az=357.5° where the
        // parabolic right-neighbor of grid cell 355° lands at 360° → wraps to 0°.
        let geom = MicGeometry::default();
        let mut est = SrpPhatEstimator::new(geom.clone(), 48000, 480);
        let frame = synth_plane_wave_srp(357.5, 0.0, 48000, 480, &geom);
        let result = est.estimate(&frame).expect("should localise");
        let az_err = ((result.azimuth_deg - 357.5).abs()).min((360.0 - (result.azimuth_deg - 357.5).abs()));
        assert!(az_err < AZ_STEP_DEG,
            "wrap-around az {} not within ±{}° of 357.5", result.azimuth_deg, AZ_STEP_DEG);
    }
}
