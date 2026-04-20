//! Per-preset beamformer weight vectors.
//!
//! Inputs: target direction (azimuth φ, elevation θ) and MicGeometry.
//! Output: `[f32; 8]` weights applied AFTER delay alignment. Weights sum
//! is 1 for unity unit-gain on-axis for Omni/Cardioid/Supercardioid; for
//! Figure-8 the sum is 0 (pressure-gradient pattern).
//!
//! Conventions:
//! - Azimuth φ: 0° = +X (front), 90° = +Y (left), CCW looking down.
//! - Elevation θ: measured from +Z (zenith). θ=90° is horizon. Our "pitch"
//!   in AudioState is `90 - θ` (0 = horizon).

use super::geometry::{MicGeometry, Position3D};
use waved_state::AudioMode;

/// Unit vector in Cartesian for (azimuth φ, elevation θ) where θ is
/// measured from +Z. Inputs in degrees (elevation: 0 = horizon).
pub fn direction_vector(azimuth_deg: f32, elevation_deg: f32) -> Position3D {
    let theta = (90.0 - elevation_deg).to_radians();
    let phi = azimuth_deg.to_radians();
    Position3D::new(
        theta.sin() * phi.cos(),
        theta.sin() * phi.sin(),
        theta.cos(),
    )
}

/// Compute weights for a given preset, DOA, and geometry.
///
/// Returns a [f32; 8] indexed by *physical* mic index (not ALSA channel);
/// the beamformer maps ALSA → physical before applying.
pub fn weights_for(mode: AudioMode, doa_az_deg: f32, doa_el_deg: f32, geom: &MicGeometry) -> [f32; 8] {
    match mode {
        AudioMode::Omni => [1.0 / 8.0; 8],
        AudioMode::NoiseReduction => [1.0 / 8.0; 8],
        AudioMode::Cardioid => cardioid_weights(doa_az_deg, doa_el_deg, geom, 0.5),
        AudioMode::Supercardioid => cardioid_weights(doa_az_deg, doa_el_deg, geom, 0.37),
        AudioMode::Figure8 => figure8_weights(doa_az_deg, doa_el_deg, geom),
        AudioMode::Stereo => cardioid_weights(doa_az_deg - 45.0, doa_el_deg, geom, 0.5),
    }
}

/// First-order cardioid: w_i = alpha + (1-alpha) * (p_i_hat · d_hat)
/// where p_i_hat is the unit vector to mic i and d_hat is DOA unit vector.
/// alpha controls pattern tightness: 0.5 = cardioid, 0.37 = supercardioid,
/// 0.0 = figure-8 (handled separately).
fn cardioid_weights(az_deg: f32, el_deg: f32, geom: &MicGeometry, alpha: f32) -> [f32; 8] {
    let d = direction_vector(az_deg, el_deg);
    let mut w = [0.0f32; 8];
    let mut sum = 0.0f32;
    for (i, p) in geom.positions_mm.iter().enumerate() {
        let r = (p.x * p.x + p.y * p.y + p.z * p.z).sqrt().max(1e-6);
        let p_hat = Position3D::new(p.x / r, p.y / r, p.z / r);
        let cos_theta = p_hat.dot(&d);
        w[i] = alpha + (1.0 - alpha) * cos_theta;
        sum += w[i];
    }
    let norm = 1.0 / sum.max(1e-6);
    for v in w.iter_mut() {
        *v *= norm;
    }
    w
}

/// Figure-8: front vs back differential. Pressure-gradient pattern has
/// zero sum; we don't normalize.
fn figure8_weights(az_deg: f32, el_deg: f32, geom: &MicGeometry) -> [f32; 8] {
    let d = direction_vector(az_deg, el_deg);
    let mut w = [0.0f32; 8];
    for (i, p) in geom.positions_mm.iter().enumerate() {
        let r = (p.x * p.x + p.y * p.y + p.z * p.z).sqrt().max(1e-6);
        let p_hat = Position3D::new(p.x / r, p.y / r, p.z / r);
        w[i] = p_hat.dot(&d);
    }
    let max = w.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
    if max > 1e-6 {
        for v in w.iter_mut() {
            *v /= max * 2.0;
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_vector_front_is_plus_x() {
        let d = direction_vector(0.0, 0.0);
        assert!((d.x - 1.0).abs() < 1e-5);
        assert!(d.y.abs() < 1e-5);
        assert!(d.z.abs() < 1e-5);
    }

    #[test]
    fn direction_vector_left_is_plus_y() {
        let d = direction_vector(90.0, 0.0);
        assert!(d.x.abs() < 1e-5);
        assert!((d.y - 1.0).abs() < 1e-5);
    }

    #[test]
    fn omni_weights_sum_to_one() {
        let geom = MicGeometry::default();
        let w = weights_for(AudioMode::Omni, 0.0, 0.0, &geom);
        let sum: f32 = w.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cardioid_peak_weight_at_front_mic_for_doa_zero() {
        let geom = MicGeometry::default();
        let w = weights_for(AudioMode::Cardioid, 0.0, 0.0, &geom);
        let max_idx = w.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        assert_eq!(max_idx, 0, "expected front mic (0) to peak, got {max_idx}");
    }

    #[test]
    fn cardioid_rear_mic_has_near_zero_weight_for_doa_zero() {
        let geom = MicGeometry::default();
        let w = weights_for(AudioMode::Cardioid, 0.0, 0.0, &geom);
        assert!(w[2].abs() < 0.01, "back mic weight {} should be near 0", w[2]);
    }

    #[test]
    fn cardioid_sums_to_one_after_normalize() {
        let geom = MicGeometry::default();
        let w = weights_for(AudioMode::Cardioid, 45.0, 0.0, &geom);
        let sum: f32 = w.iter().sum();
        assert!((sum - 1.0).abs() < 1e-4, "sum {sum} != 1");
    }

    #[test]
    fn figure8_front_and_back_opposite_sign() {
        let geom = MicGeometry::default();
        let w = weights_for(AudioMode::Figure8, 0.0, 0.0, &geom);
        assert!(w[0] > 0.0, "front weight should be positive");
        assert!(w[2] < 0.0, "back weight should be negative");
        assert!((w[0] + w[2]).abs() < 1e-4, "front+back should cancel");
    }
}
