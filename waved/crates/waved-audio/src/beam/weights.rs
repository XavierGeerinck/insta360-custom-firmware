//! Hemispheric front/back weights for differential beamforming.
//!
//! For a steering direction d̂, each mic is split into the "front" hemisphere
//! (p̂·d̂ > 0) or the "back" hemisphere (p̂·d̂ < 0) and given a weight
//! proportional to |p̂·d̂|. The two weighted sums are then subtracted with a
//! back delay of Δ = (c_f − c_b)·d̂/c (centroid-to-centroid time-of-flight)
//! to form a first-order differential cardioid with a null at −d̂.
//!
//! Pattern is selected by scaling that back delay:
//!   * Cardioid        scale = 1.0      (null at 180°)
//!   * Supercardioid   scale = 0.577    (null at 125°)
//!   * Hypercardioid   scale = 0.333    (null at 109°)
//!   * Figure-8        scale = 0.0      (nulls at ±90°)
//!
//! Why this instead of DAS + cardioid weights: at 1 kHz the array is ~0.15 λ
//! across, so DAS alignment alone buys only ~3 dB of directivity. Non-negative
//! cardioid weights can't null an in-phase rear source either. Differential
//! beamforming exploits the natural inter-sensor TOA difference and gets a
//! real null at the cost of a first-order high-pass roll-off.
//!
//! Conventions:
//! - Azimuth φ: 0° = +X (front), 90° = +Y (left), CCW looking down.
//! - Elevation: 0° = horizon, +90° = zenith.

use super::geometry::{MicGeometry, Position3D};

const SPEED_OF_SOUND_M_S: f32 = 343.0;

/// Unit vector in Cartesian for (azimuth, elevation) in degrees with
/// elevation measured from the horizon.
pub fn direction_vector(azimuth_deg: f32, elevation_deg: f32) -> Position3D {
    let theta = (90.0 - elevation_deg).to_radians();
    let phi = azimuth_deg.to_radians();
    Position3D::new(
        theta.sin() * phi.cos(),
        theta.sin() * phi.sin(),
        theta.cos(),
    )
}

/// Per-channel weights and the front→back centroid delay for a given DOA.
///
/// `w_front` / `w_back` are indexed by ALSA channel (already through
/// `alsa_to_physical`) and each sums to 1.0 if at least one mic lies in that
/// hemisphere. `delta_samples` is the one-way time-of-flight between the
/// front and back weighted centroids projected onto d̂, expressed in
/// fractional samples at `sample_rate`. It is ≥ 0.
pub struct HemisphericWeights {
    pub w_front: [f32; 8],
    pub w_back: [f32; 8],
    pub delta_samples: f32,
}

pub fn hemispheric_weights(
    geom: &MicGeometry,
    doa_az_deg: f32,
    doa_el_deg: f32,
    sample_rate: u32,
) -> HemisphericWeights {
    let d = direction_vector(doa_az_deg, doa_el_deg);
    let mut w_front = [0.0f32; 8];
    let mut w_back = [0.0f32; 8];
    let mut sum_f = 0.0f32;
    let mut sum_b = 0.0f32;

    for alsa_ch in 0..8 {
        let p = geom.position_for_alsa(alsa_ch);
        let r = (p.x * p.x + p.y * p.y + p.z * p.z).sqrt().max(1e-6);
        let p_hat_dot_d = (p.x * d.x + p.y * d.y + p.z * d.z) / r;
        if p_hat_dot_d > 0.0 {
            w_front[alsa_ch] = p_hat_dot_d;
            sum_f += p_hat_dot_d;
        } else if p_hat_dot_d < 0.0 {
            w_back[alsa_ch] = -p_hat_dot_d;
            sum_b += -p_hat_dot_d;
        }
    }

    if sum_f > 1e-6 {
        for w in w_front.iter_mut() {
            *w /= sum_f;
        }
    }
    if sum_b > 1e-6 {
        for w in w_back.iter_mut() {
            *w /= sum_b;
        }
    }

    let mut c_f_proj = 0.0f32;
    let mut c_b_proj = 0.0f32;
    for alsa_ch in 0..8 {
        let p = geom.position_for_alsa(alsa_ch);
        let proj_mm = p.x * d.x + p.y * d.y + p.z * d.z;
        c_f_proj += w_front[alsa_ch] * proj_mm;
        c_b_proj += w_back[alsa_ch] * proj_mm;
    }
    let delta_sec = (c_f_proj - c_b_proj) * 1e-3 / SPEED_OF_SOUND_M_S;
    let delta_samples = (delta_sec * sample_rate as f32).max(0.0);

    HemisphericWeights {
        w_front,
        w_back,
        delta_samples,
    }
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
    fn direction_vector_zenith_is_plus_z() {
        let d = direction_vector(0.0, 90.0);
        assert!(d.z > 0.999);
    }

    #[test]
    fn hemispheric_weights_sum_to_one_each_side() {
        let geom = MicGeometry::default();
        let hw = hemispheric_weights(&geom, 0.0, 0.0, 48000);
        let sf: f32 = hw.w_front.iter().sum();
        let sb: f32 = hw.w_back.iter().sum();
        assert!((sf - 1.0).abs() < 1e-4, "front sum {sf} != 1");
        assert!((sb - 1.0).abs() < 1e-4, "back sum {sb} != 1");
    }

    #[test]
    fn hemispheric_partition_respects_projection_sign() {
        let geom = MicGeometry::default();
        let hw = hemispheric_weights(&geom, 0.0, 0.0, 48000);
        // Steering az=0° (+X forward). Any mic with +x goes to the front
        // hemisphere and w_back must be zero; any mic with -x goes to the
        // back hemisphere and w_front must be zero. Holds regardless of the
        // alsa_to_physical permutation.
        for alsa_ch in 0..8 {
            let p = geom.position_for_alsa(alsa_ch);
            if p.x > 0.0 {
                assert!(
                    hw.w_front[alsa_ch] > 0.0 && hw.w_back[alsa_ch] == 0.0,
                    "alsa ch {alsa_ch} (x={}) should be front-only",
                    p.x
                );
            } else if p.x < 0.0 {
                assert!(
                    hw.w_back[alsa_ch] > 0.0 && hw.w_front[alsa_ch] == 0.0,
                    "alsa ch {alsa_ch} (x={}) should be back-only",
                    p.x
                );
            }
        }
    }

    #[test]
    fn hemispheric_delta_positive_on_axis() {
        let geom = MicGeometry::default();
        let hw = hemispheric_weights(&geom, 0.0, 0.0, 48000);
        // Expect roughly 2R/c at 48 kHz ≈ 7.14 samples, but pulled down
        // by the side-ring mics off-axis contribution.
        assert!(
            hw.delta_samples > 3.0 && hw.delta_samples < 10.0,
            "delta_samples {} out of expected range",
            hw.delta_samples
        );
    }

    #[test]
    fn hemispheric_weights_symmetric_under_doa_flip() {
        let geom = MicGeometry::default();
        let fwd = hemispheric_weights(&geom, 0.0, 0.0, 48000);
        let rev = hemispheric_weights(&geom, 180.0, 0.0, 48000);
        for ch in 0..8 {
            assert!(
                (fwd.w_front[ch] - rev.w_back[ch]).abs() < 1e-4,
                "front/back flip mismatch ch {ch}"
            );
        }
    }
}
