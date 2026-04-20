//! Physical mic array geometry for beamforming.
//!
//! Coordinate system: +X forward, +Y left, +Z up. Origin at center of
//! top-ring plane. Units: millimeters.
//!
//! Default matches the teardown-measured Insta360 Wave geometry: top ring
//! of 4 mics at z=0 (azimuths 0/90/180/270°), side ring of 4 mics at
//! z=-78.2 mm (azimuths 45/135/225/315°), all at r=25.5 mm.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position3D {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Position3D {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// Dot product (both in mm) returns mm². Used in steering-vector math
    /// after converting to meters.
    pub fn dot(&self, other: &Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }
}

/// 8 physical mic positions + ALSA channel → physical index permutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicGeometry {
    /// Positions indexed by physical index (0..8).
    pub positions_mm: [Position3D; 8],
    /// `alsa_to_physical[alsa_ch] = physical_index`. Produced by the
    /// `waved-mic-map` tap harness. Defaults to identity until overridden.
    pub alsa_to_physical: [u8; 8],
}

impl Default for MicGeometry {
    fn default() -> Self {
        Self::insta360_wave()
    }
}

impl MicGeometry {
    /// Teardown-measured geometry: r=25.5 mm, top ring z=0 at 0/90/180/270°,
    /// side ring z=-78.2 mm at 45/135/225/315°.
    pub fn insta360_wave() -> Self {
        const R: f32 = 25.5;
        const Z_SIDE: f32 = -78.2;
        let side_xy = R * std::f32::consts::FRAC_1_SQRT_2;

        Self {
            positions_mm: [
                Position3D::new(R, 0.0, 0.0),
                Position3D::new(0.0, R, 0.0),
                Position3D::new(-R, 0.0, 0.0),
                Position3D::new(0.0, -R, 0.0),
                Position3D::new(side_xy, side_xy, Z_SIDE),
                Position3D::new(-side_xy, side_xy, Z_SIDE),
                Position3D::new(-side_xy, -side_xy, Z_SIDE),
                Position3D::new(side_xy, -side_xy, Z_SIDE),
            ],
            alsa_to_physical: [0, 1, 2, 3, 4, 5, 6, 7],
        }
    }

    /// Resolve an ALSA channel index to the corresponding physical mic position.
    pub fn position_for_alsa(&self, alsa_ch: usize) -> &Position3D {
        let physical = self.alsa_to_physical[alsa_ch] as usize;
        &self.positions_mm[physical]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_wave_geometry_top_ring_at_z_zero() {
        let g = MicGeometry::default();
        for i in 0..4 {
            assert_eq!(g.positions_mm[i].z, 0.0, "top mic {i} should be at z=0");
        }
    }

    #[test]
    fn default_wave_geometry_side_ring_below_top() {
        let g = MicGeometry::default();
        for i in 4..8 {
            assert_eq!(g.positions_mm[i].z, -78.2, "side mic {i} should be at z=-78.2");
        }
    }

    #[test]
    fn default_wave_geometry_radius_25_5() {
        let g = MicGeometry::default();
        for (i, p) in g.positions_mm.iter().enumerate() {
            let r = (p.x * p.x + p.y * p.y).sqrt();
            assert!((r - 25.5).abs() < 0.01, "mic {i} radius {r} != 25.5");
        }
    }

    #[test]
    fn default_wave_geometry_top_azimuths_cardinal() {
        let g = MicGeometry::default();
        assert!((g.positions_mm[0].x - 25.5).abs() < 0.01);
        assert!(g.positions_mm[0].y.abs() < 0.01);
        assert!(g.positions_mm[1].x.abs() < 0.01);
        assert!((g.positions_mm[1].y - 25.5).abs() < 0.01);
    }

    #[test]
    fn default_alsa_to_physical_is_identity() {
        let g = MicGeometry::default();
        for i in 0..8 {
            assert_eq!(g.alsa_to_physical[i], i as u8);
        }
    }

    #[test]
    fn position_for_alsa_applies_permutation() {
        let mut g = MicGeometry::default();
        g.alsa_to_physical = [3, 2, 1, 0, 7, 6, 5, 4];
        let p = g.position_for_alsa(0);
        assert!((p.y + 25.5).abs() < 0.01);
    }
}
