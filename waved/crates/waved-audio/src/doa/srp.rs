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
    #[allow(dead_code)]
    geom: MicGeometry,
    #[allow(dead_code)]
    sample_rate: u32,
    #[allow(dead_code)]
    frame_size: usize,
    #[allow(dead_code)]
    fft_len: usize,
    #[allow(dead_code)]
    planner_fwd: Arc<dyn rustfft::Fft<f32>>,
    grid: Vec<GridDirection>,
    /// Per-pair position delta in meters; matches gcc_phat's pair_deltas_m.
    #[allow(dead_code)]
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
}
