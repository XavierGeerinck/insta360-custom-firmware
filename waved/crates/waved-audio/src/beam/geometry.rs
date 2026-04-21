//! Physical mic array geometry for beamforming.
//!
//! Coordinate system: +X forward, +Y left, +Z up. Origin at center of
//! top-ring plane. Units: millimeters.
//!
//! Default matches the physical Insta360 Wave layout: two stacked rings of
//! 4 mics each, all at the same 4 diagonal azimuths (45/135/225/315°),
//! at r=25.5 mm. Top ring at z=0, bottom ring at z=-78.2 mm.

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
    /// Physical Insta360 Wave layout: r=25.5 mm, top ring z=0 and bottom
    /// ring z=-78.2 mm, both rings sharing the same 4 diagonal azimuths
    /// (45°, 135°, 225°, 315°). Physical indices 0..=3 are top, 4..=7 are
    /// bottom, both in the same azimuth order.
    pub fn insta360_wave() -> Self {
        const R: f32 = 25.5;
        const Z_BOT: f32 = -78.2;
        let xy = R * std::f32::consts::FRAC_1_SQRT_2;

        Self {
            positions_mm: [
                Position3D::new(xy, xy, 0.0),           // 0: TOP 45°  (front-left)
                Position3D::new(-xy, xy, 0.0),          // 1: TOP 135° (back-left)
                Position3D::new(-xy, -xy, 0.0),         // 2: TOP 225° (back-right)
                Position3D::new(xy, -xy, 0.0),          // 3: TOP 315° (front-right)
                Position3D::new(xy, xy, Z_BOT),         // 4: BOT 45°  (front-left)
                Position3D::new(-xy, xy, Z_BOT),        // 5: BOT 135° (back-left)
                Position3D::new(-xy, -xy, Z_BOT),       // 6: BOT 225° (back-right)
                Position3D::new(xy, -xy, Z_BOT),        // 7: BOT 315° (front-right)
            ],
            // Measured on device PSP_0000000000000000 with waved-mic-map
            // tap harness (2026-04-21). Each ALSA channel maps to the
            // physical mic position at index `alsa_to_physical[alsa_ch]`.
            alsa_to_physical: [5, 4, 7, 6, 1, 0, 3, 2],
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
    fn default_wave_geometry_bottom_ring_below_top() {
        let g = MicGeometry::default();
        for i in 4..8 {
            assert_eq!(g.positions_mm[i].z, -78.2, "bottom mic {i} should be at z=-78.2");
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
    fn default_wave_geometry_top_and_bottom_share_diagonal_azimuths() {
        let g = MicGeometry::default();
        let xy = 25.5 * std::f32::consts::FRAC_1_SQRT_2;
        // Index i in top ring (0..4) has same (x,y) as index i+4 in bottom ring.
        for i in 0..4 {
            assert!((g.positions_mm[i].x - g.positions_mm[i + 4].x).abs() < 1e-3);
            assert!((g.positions_mm[i].y - g.positions_mm[i + 4].y).abs() < 1e-3);
        }
        // And each (x,y) is at +/- xy on each axis (diagonal).
        for p in g.positions_mm.iter() {
            assert!((p.x.abs() - xy).abs() < 1e-3, "|x|={} != xy={xy}", p.x.abs());
            assert!((p.y.abs() - xy).abs() < 1e-3, "|y|={} != xy={xy}", p.y.abs());
        }
    }

    #[test]
    fn default_alsa_to_physical_is_permutation() {
        // The default mapping comes from the tap-harness measurement; it is
        // a permutation (every physical index 0..8 referenced exactly once),
        // not identity.
        let g = MicGeometry::default();
        let mut seen = [false; 8];
        for &p in g.alsa_to_physical.iter() {
            let p = p as usize;
            assert!(p < 8, "physical index {p} out of range");
            assert!(!seen[p], "physical index {p} assigned twice");
            seen[p] = true;
        }
    }

    #[test]
    fn position_for_alsa_applies_permutation() {
        let mut g = MicGeometry::default();
        // Send ALSA ch 0 → physical 3 (TOP 315°, which is at +xy,-xy,0).
        g.alsa_to_physical = [3, 2, 1, 0, 7, 6, 5, 4];
        let xy = 25.5 * std::f32::consts::FRAC_1_SQRT_2;
        let p = g.position_for_alsa(0);
        assert!((p.x - xy).abs() < 0.01);
        assert!((p.y + xy).abs() < 0.01);
    }
}
