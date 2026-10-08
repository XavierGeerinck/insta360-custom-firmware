//! Least-squares direction-of-arrival solver.
//!
//! Given TDOAs τ_ij between mic pairs with known positions, find the unit
//! direction vector d̂ that best satisfies  τ_ij · c = (p_i - p_j) · d̂  for
//! all pairs. Solved in 3D via normal equations; result is normalized to
//! unit length and converted to (azimuth, elevation) in AudioState convention.

use crate::beam::Position3D;

const SPEED_OF_SOUND_M_S: f32 = 343.0;

pub struct PairObservation {
    /// Position difference p_i - p_j, in METERS.
    pub delta_m: Position3D,
    /// TDOA in seconds (positive = mic i delayed vs mic j).
    pub tdoa_sec: f32,
}

/// Result: (azimuth_deg 0..360, elevation_deg -90..90 with 0=horizon).
#[derive(Debug, Clone, Copy)]
pub struct DoaAngles {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
}

/// Solve the overdetermined system for the source direction.
/// Returns None if observations are degenerate (e.g. all pairs colinear).
pub fn solve_direction(obs: &[PairObservation]) -> Option<DoaAngles> {
    if obs.len() < 3 {
        return None;
    }

    // Build normal equations: A d = b where A is 3x3, b is 3x1.
    // Each observation contributes a rank-1 update:
    //   A += Δ Δ^T,  b += (τ c) Δ
    let mut a = [[0.0f32; 3]; 3];
    let mut b_vec = [0.0f32; 3];
    for o in obs {
        let d = [o.delta_m.x, o.delta_m.y, o.delta_m.z];
        let tc = o.tdoa_sec * SPEED_OF_SOUND_M_S;
        for i in 0..3 {
            b_vec[i] += tc * d[i];
            for j in 0..3 {
                a[i][j] += d[i] * d[j];
            }
        }
    }

    let det_a = det3(&a);
    if is_singular(&a, det_a) {
        return None;
    }

    let a_x = replace_col(&a, 0, &b_vec);
    let a_y = replace_col(&a, 1, &b_vec);
    let a_z = replace_col(&a, 2, &b_vec);
    let dx = det3(&a_x) / det_a;
    let dy = det3(&a_y) / det_a;
    let dz = det3(&a_z) / det_a;

    let norm = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-9);
    let dhat = (dx / norm, dy / norm, dz / norm);

    let az = dhat.1.atan2(dhat.0).to_degrees();
    let el = dhat.2.clamp(-1.0, 1.0).asin().to_degrees();
    let az = if az < 0.0 { az + 360.0 } else { az };

    Some(DoaAngles {
        azimuth_deg: az,
        elevation_deg: el,
    })
}

/// Threshold below which the sum-of-pair-sharpnesses indicates the GCC-PHAT
/// surfaces had no real peak (e.g. silence-on-VAD-edge, all pairs shadowed).
/// Returning `None` lets the caller emit a vad=false / NaN-angles DoaFrame
/// instead of pretending to know.
pub const MIN_SHARPNESS_THRESHOLD: f32 = 1e-4;

/// Per-pair-weighted variant of `solve_direction`. Each observation contributes
/// to the normal equations weighted by `weights[k]`:
///   A += w_k · (Δ Δ^T),   b += w_k · (τ c) Δ
///
/// Use this from the DoA pipeline with the per-pair sharpness values
/// (`weights[k] = sharpness_k`). The solution and the singularity check are
/// both scale-invariant in `weights`, so their units do not matter.
///
/// Returns `None` if:
/// - fewer than 3 observations
/// - `Σ weights < MIN_SHARPNESS_THRESHOLD` (degenerate / no real peaks)
/// - the weighted 3x3 normal matrix is singular (see `is_singular`)
pub fn solve_direction_weighted(
    obs: &[PairObservation],
    weights: &[f32],
) -> Option<DoaAngles> {
    if obs.len() < 3 {
        return None;
    }
    if obs.len() != weights.len() {
        return None;
    }

    let weight_sum: f32 = weights.iter().sum();
    if weight_sum < MIN_SHARPNESS_THRESHOLD {
        return None;
    }

    let mut a = [[0.0f32; 3]; 3];
    let mut b_vec = [0.0f32; 3];
    for (o, &w) in obs.iter().zip(weights.iter()) {
        let d = [o.delta_m.x, o.delta_m.y, o.delta_m.z];
        let tc = o.tdoa_sec * SPEED_OF_SOUND_M_S;
        for i in 0..3 {
            b_vec[i] += w * tc * d[i];
            for j in 0..3 {
                a[i][j] += w * d[i] * d[j];
            }
        }
    }

    let det_a = det3(&a);
    if is_singular(&a, det_a) {
        return None;
    }

    let a_x = replace_col(&a, 0, &b_vec);
    let a_y = replace_col(&a, 1, &b_vec);
    let a_z = replace_col(&a, 2, &b_vec);
    let dx = det3(&a_x) / det_a;
    let dy = det3(&a_y) / det_a;
    let dz = det3(&a_z) / det_a;

    let norm = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-9);
    let dhat = (dx / norm, dy / norm, dz / norm);

    let az = dhat.1.atan2(dhat.0).to_degrees();
    let el = dhat.2.clamp(-1.0, 1.0).asin().to_degrees();
    let az = if az < 0.0 { az + 360.0 } else { az };

    Some(DoaAngles {
        azimuth_deg: az,
        elevation_deg: el,
    })
}

/// Smallest `|det| / (trace / 3)³` a normal matrix may have: well-spread
/// pairs sit near 1, collinear pairs at 0.
const MIN_CONDITION: f32 = 1e-3;

/// Whether the normal matrix is too close to singular to solve. Judged
/// relative to the matrix's own scale: an absolute `|det| < 1e-9` scaled with
/// the cube of the pair weights, so on speech (whose GCC peaks are ~100×
/// flatter than broadband noise) it rejected nearly every frame even when
/// every pair agreed.
fn is_singular(a: &[[f32; 3]; 3], det: f32) -> bool {
    let scale = (a[0][0] + a[1][1] + a[2][2]) / 3.0;
    scale <= 0.0 || det.is_nan() || det.abs() < MIN_CONDITION * scale.powi(3)
}

fn det3(m: &[[f32; 3]; 3]) -> f32 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn replace_col(m: &[[f32; 3]; 3], col: usize, b: &[f32; 3]) -> [[f32; 3]; 3] {
    let mut out = *m;
    for row in 0..3 {
        out[row][col] = b[row];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_obs(
        pairs: &[(Position3D, Position3D)],
        source_az_deg: f32,
        source_el_deg: f32,
    ) -> Vec<PairObservation> {
        let az = source_az_deg.to_radians();
        let el = source_el_deg.to_radians();
        let d = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
        pairs
            .iter()
            .map(|(pi, pj)| {
                let delta = Position3D::new(
                    (pi.x - pj.x) * 1e-3,
                    (pi.y - pj.y) * 1e-3,
                    (pi.z - pj.z) * 1e-3,
                );
                let tc = delta.x * d[0] + delta.y * d[1] + delta.z * d[2];
                PairObservation {
                    delta_m: delta,
                    tdoa_sec: tc / SPEED_OF_SOUND_M_S,
                }
            })
            .collect()
    }

    #[test]
    fn recovers_source_at_zero_azimuth() {
        let pairs = vec![
            (Position3D::new(25.5, 0.0, 0.0), Position3D::new(-25.5, 0.0, 0.0)),
            (Position3D::new(0.0, 25.5, 0.0), Position3D::new(0.0, -25.5, 0.0)),
            (Position3D::new(25.5, 0.0, 0.0), Position3D::new(0.0, 0.0, -78.2)),
        ];
        let obs = make_obs(&pairs, 0.0, 0.0);
        let ang = solve_direction(&obs).expect("well-conditioned");
        assert!(
            ang.azimuth_deg < 2.0 || ang.azimuth_deg > 358.0,
            "az {} not ≈ 0",
            ang.azimuth_deg
        );
        assert!(ang.elevation_deg.abs() < 2.0, "el {} not ≈ 0", ang.elevation_deg);
    }

    #[test]
    fn recovers_source_at_ninety_azimuth() {
        let pairs = vec![
            (Position3D::new(25.5, 0.0, 0.0), Position3D::new(-25.5, 0.0, 0.0)),
            (Position3D::new(0.0, 25.5, 0.0), Position3D::new(0.0, -25.5, 0.0)),
            (Position3D::new(25.5, 0.0, 0.0), Position3D::new(0.0, 0.0, -78.2)),
        ];
        let obs = make_obs(&pairs, 90.0, 0.0);
        let ang = solve_direction(&obs).expect("well-conditioned");
        assert!((ang.azimuth_deg - 90.0).abs() < 2.0, "az {} not ≈ 90", ang.azimuth_deg);
    }

    #[test]
    fn recovers_source_at_elevation() {
        let pairs = vec![
            (Position3D::new(25.5, 0.0, 0.0), Position3D::new(-25.5, 0.0, 0.0)),
            (Position3D::new(0.0, 25.5, 0.0), Position3D::new(0.0, -25.5, 0.0)),
            (Position3D::new(25.5, 0.0, 0.0), Position3D::new(0.0, 0.0, -78.2)),
        ];
        let obs = make_obs(&pairs, 45.0, 30.0);
        let ang = solve_direction(&obs).expect("well-conditioned");
        assert!((ang.azimuth_deg - 45.0).abs() < 2.0);
        assert!((ang.elevation_deg - 30.0).abs() < 2.0);
    }

    #[test]
    fn rejects_degenerate_pairs() {
        let pairs = vec![
            (Position3D::new(10.0, 0.0, 0.0), Position3D::new(-10.0, 0.0, 0.0)),
            (Position3D::new(20.0, 0.0, 0.0), Position3D::new(-20.0, 0.0, 0.0)),
            (Position3D::new(5.0, 0.0, 0.0), Position3D::new(-5.0, 0.0, 0.0)),
        ];
        let obs = make_obs(&pairs, 45.0, 0.0);
        assert!(solve_direction(&obs).is_none());
    }

    #[test]
    fn requires_at_least_three_pairs() {
        let obs = vec![
            PairObservation {
                delta_m: Position3D::new(0.05, 0.0, 0.0),
                tdoa_sec: 0.0,
            },
            PairObservation {
                delta_m: Position3D::new(0.0, 0.05, 0.0),
                tdoa_sec: 0.0,
            },
        ];
        assert!(solve_direction(&obs).is_none());
    }

    #[test]
    fn weighted_ls_equals_unweighted_with_equal_weights() {
        let pairs = vec![
            (Position3D::new(25.5, 0.0, 0.0), Position3D::new(-25.5, 0.0, 0.0)),
            (Position3D::new(0.0, 25.5, 0.0), Position3D::new(0.0, -25.5, 0.0)),
            (Position3D::new(25.5, 0.0, 0.0), Position3D::new(0.0, 0.0, -78.2)),
        ];
        let obs = make_obs(&pairs, 45.0, 15.0);
        let weights = vec![1.0; obs.len()];
        let a = solve_direction(&obs).unwrap();
        let b = solve_direction_weighted(&obs, &weights).unwrap();
        assert!((a.azimuth_deg - b.azimuth_deg).abs() < 0.01);
        assert!((a.elevation_deg - b.elevation_deg).abs() < 0.01);
    }

    #[test]
    fn weighted_ls_zero_weight_pair_ignored() {
        let pairs = vec![
            (Position3D::new(25.5, 0.0, 0.0), Position3D::new(-25.5, 0.0, 0.0)),
            (Position3D::new(0.0, 25.5, 0.0), Position3D::new(0.0, -25.5, 0.0)),
            (Position3D::new(25.5, 0.0, 0.0), Position3D::new(0.0, 0.0, -78.2)),
            // Bogus 4th pair, completely wrong direction
            (Position3D::new(10.0, 10.0, 10.0), Position3D::new(-10.0, -10.0, -10.0)),
        ];
        let mut obs = make_obs(&pairs, 45.0, 0.0);
        // Corrupt the 4th observation's tdoa to simulate a noisy pair
        obs[3].tdoa_sec = 5e-4; // very wrong
        let weights = vec![1.0, 1.0, 1.0, 0.0]; // ignore the bad one
        let ang = solve_direction_weighted(&obs, &weights).unwrap();
        assert!((ang.azimuth_deg - 45.0).abs() < 2.0, "az {} not ≈45 with bad pair zeroed", ang.azimuth_deg);
    }

    #[test]
    fn weighted_ls_solves_with_small_but_valid_weights() {
        // Speech gives GCC peaks ~100× flatter than broadband noise. The
        // weights only set the pairs' relative say, so a frame with every
        // weight at 0.03 must solve as one with every weight at 3.
        let g = crate::beam::MicGeometry::default();
        let pairs: Vec<_> = [(0, 2), (1, 3), (0, 4), (2, 6)]
            .iter()
            .map(|&(i, j)| (g.positions_mm[i], g.positions_mm[j]))
            .collect();
        let obs = make_obs(&pairs, 90.0, 0.0);
        let small = solve_direction_weighted(&obs, &vec![0.03; obs.len()]).expect("small weights solve");
        let large = solve_direction_weighted(&obs, &vec![3.0; obs.len()]).expect("large weights solve");
        assert!((small.azimuth_deg - large.azimuth_deg).abs() < 0.01);
        assert!((small.azimuth_deg - 90.0).abs() < 1.0, "az {}", small.azimuth_deg);
    }

    #[test]
    fn weighted_ls_returns_none_when_all_weights_below_threshold() {
        let pairs = vec![
            (Position3D::new(25.5, 0.0, 0.0), Position3D::new(-25.5, 0.0, 0.0)),
            (Position3D::new(0.0, 25.5, 0.0), Position3D::new(0.0, -25.5, 0.0)),
            (Position3D::new(25.5, 0.0, 0.0), Position3D::new(0.0, 0.0, -78.2)),
        ];
        let obs = make_obs(&pairs, 30.0, 0.0);
        let weights = vec![1e-6, 1e-6, 1e-6]; // all sub-threshold
        assert!(solve_direction_weighted(&obs, &weights).is_none());
    }
}
