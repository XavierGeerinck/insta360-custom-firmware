//! DoA-tracking beam direction smoothing + reactivation logic.
//!
//! Holds last high-confidence direction during silence (beam doesn't swing
//! when the speaker pauses). Reverts to omni after long silence so a
//! returning speaker isn't initially captured by a stale-pointed cardioid.
//! See F2 spec §3.3 for full state machine.

use crate::doa::DoaFrame;

/// EWMA factor on direction (alpha = 0.15 → ≈140 ms time constant at 21 ms cadence).
const ALPHA_DIR: f32 = 0.15;
/// Confidence below which a frame is treated as "no source".
pub const CONFIDENCE_THRESHOLD: f32 = 0.30;
/// Frames of continuous low-confidence before omni reversion starts
/// (240 frames × 21 ms ≈ 5 s).
const REVERT_SILENCE_FRAMES: u32 = 240;
/// Cosine-fade length between cardioid↔omni (50 × 21 ms ≈ 1 s).
const OMNI_RAMP_FRAMES: u32 = 50;

#[derive(Debug, Clone, Copy)]
pub struct SteeredDirection {
    pub az_deg: f32,
    pub el_deg: f32,
    /// 0.0 = pure cardioid at (az, el); 1.0 = pure omni.
    pub omni_mix: f32,
}

pub struct SteeredBeam {
    /// Current smoothed direction (the one the cardioid weights track).
    current_dir: [f32; 3],
    /// Last received high-confidence target.
    target_dir: [f32; 3],
    silence_frames: u32,
    omni_mix: f32,
}

impl SteeredBeam {
    pub fn new(initial_az_deg: f32) -> Self {
        let dhat = unit_vec_from_az_el(initial_az_deg, 0.0);
        Self {
            current_dir: dhat,
            target_dir: dhat,
            silence_frames: 0,
            omni_mix: 0.0,
        }
    }

    /// Feed one DoA frame. Returns the smoothed beam direction + omni_mix.
    pub fn on_doa_frame(&mut self, frame: &DoaFrame) -> SteeredDirection {
        if frame.vad
            && frame.confidence > CONFIDENCE_THRESHOLD
            && !frame.azimuth_deg.is_nan()
        {
            self.target_dir = unit_vec_from_az_el(frame.azimuth_deg, frame.elevation_deg);
            self.current_dir = slerp(self.current_dir, self.target_dir, ALPHA_DIR);
            self.silence_frames = 0;
            // Ramp back from omni → cardioid one step per high-conf frame.
            self.omni_mix = (self.omni_mix - 1.0 / OMNI_RAMP_FRAMES as f32).max(0.0);
        } else {
            self.silence_frames = self.silence_frames.saturating_add(1);
            if self.silence_frames > REVERT_SILENCE_FRAMES {
                self.omni_mix = (self.omni_mix + 1.0 / OMNI_RAMP_FRAMES as f32).min(1.0);
            }
        }
        let (az, el) = az_el_from_unit_vec(self.current_dir);
        SteeredDirection {
            az_deg: az,
            el_deg: el,
            omni_mix: self.omni_mix,
        }
    }
}

fn unit_vec_from_az_el(az_deg: f32, el_deg: f32) -> [f32; 3] {
    let az = az_deg.to_radians();
    let el = el_deg.to_radians();
    [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()]
}

fn az_el_from_unit_vec(d: [f32; 3]) -> (f32, f32) {
    let az = d[1].atan2(d[0]).to_degrees();
    let az = if az < 0.0 { az + 360.0 } else { az };
    let el = d[2].clamp(-1.0, 1.0).asin().to_degrees();
    (az, el)
}

/// Spherical-linear interpolation between two unit vectors. Falls back to
/// linear + renorm at near-antipodal (dot ≈ -1, slerp undefined) — paranoia
/// guard since real DoA never jumps 180° frame-to-frame.
fn slerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
    if dot > 0.9995 {
        // Vectors nearly parallel — linear interp is fine and avoids 0/sin(0).
        return normalize([
            a[0] * (1.0 - t) + b[0] * t,
            a[1] * (1.0 - t) + b[1] * t,
            a[2] * (1.0 - t) + b[2] * t,
        ]);
    }
    if dot < -0.9995 {
        // Antipodal — slerp ill-defined; linear + renorm.
        return normalize([
            a[0] * (1.0 - t) + b[0] * t,
            a[1] * (1.0 - t) + b[1] * t,
            a[2] * (1.0 - t) + b[2] * t,
        ]);
    }
    let theta_0 = dot.acos();
    let theta = theta_0 * t;
    let sin_theta = theta.sin();
    let sin_theta_0 = theta_0.sin();
    let s1 = ((1.0 - t) * theta_0).sin() / sin_theta_0;
    let s2 = sin_theta / sin_theta_0;
    [a[0] * s1 + b[0] * s2, a[1] * s1 + b[1] * s2, a[2] * s1 + b[2] * s2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
    [v[0] / n, v[1] / n, v[2] / n]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doa::{DoaBackend, DoaFrame};

    fn high_conf_frame(az: f32, el: f32) -> DoaFrame {
        DoaFrame {
            frame_idx: 0,
            timestamp_ms: 0,
            azimuth_deg: az,
            elevation_deg: el,
            confidence: 0.8,
            vad: true,
            backend: DoaBackend::GccPhat,
        }
    }

    fn silence_frame() -> DoaFrame {
        DoaFrame::silent(0, 0, DoaBackend::GccPhat)
    }

    #[test]
    fn steered_beam_holds_direction_during_silence() {
        let mut beam = SteeredBeam::new(0.0);
        // Lock onto az=45° over a few frames.
        for _ in 0..30 {
            beam.on_doa_frame(&high_conf_frame(45.0, 0.0));
        }
        let locked = beam.on_doa_frame(&high_conf_frame(45.0, 0.0));
        // Now 100 silent frames — direction must not change.
        for _ in 0..100 {
            let s = beam.on_doa_frame(&silence_frame());
            assert!((s.az_deg - locked.az_deg).abs() < 0.5,
                "az drifted during silence: {} vs {}", s.az_deg, locked.az_deg);
        }
    }

    #[test]
    fn steered_beam_reverts_to_omni_after_long_silence() {
        let mut beam = SteeredBeam::new(0.0);
        for _ in 0..30 {
            beam.on_doa_frame(&high_conf_frame(90.0, 0.0));
        }
        // 240 silent frames triggers the ramp, then 50 more to reach omni.
        for _ in 0..(REVERT_SILENCE_FRAMES + OMNI_RAMP_FRAMES + 5) {
            beam.on_doa_frame(&silence_frame());
        }
        let s = beam.on_doa_frame(&silence_frame());
        assert!((s.omni_mix - 1.0).abs() < 1e-3, "omni_mix should reach 1.0, got {}", s.omni_mix);
    }

    #[test]
    fn steered_beam_smooths_jitter() {
        let mut beam = SteeredBeam::new(45.0);
        // Feed 30 frames jittering ±10° around az=45°.
        for i in 0..30 {
            let jitter = if i % 2 == 0 { 10.0 } else { -10.0 };
            beam.on_doa_frame(&high_conf_frame(45.0 + jitter, 0.0));
        }
        // Re-read by feeding one more centered frame and inspecting:
        let final_result = beam.on_doa_frame(&high_conf_frame(45.0, 0.0));
        assert!((final_result.az_deg - 45.0).abs() < 5.0,
            "smoothed az {} not within ±5° of 45 under ±10° jitter", final_result.az_deg);
    }

    #[test]
    fn steered_beam_slerp_short_arc_across_180_boundary() {
        let mut beam = SteeredBeam::new(170.0);
        beam.on_doa_frame(&high_conf_frame(170.0, 0.0));
        beam.on_doa_frame(&high_conf_frame(170.0, 0.0));
        // Target = az=190°. Short arc goes through 180°, not through 0°.
        let s = beam.on_doa_frame(&high_conf_frame(190.0, 0.0));
        // Smoothed should be between 170 and 190 (going up through 180),
        // NOT around 80-90 (which would mean it went the long way).
        assert!(s.az_deg > 165.0 && s.az_deg < 195.0,
            "slerp went the long way: az={}", s.az_deg);
    }

    #[test]
    fn steered_beam_ramp_back_from_omni() {
        let mut beam = SteeredBeam::new(0.0);
        // Get to omni_mix = 1.0 via long silence.
        for _ in 0..(REVERT_SILENCE_FRAMES + OMNI_RAMP_FRAMES + 10) {
            beam.on_doa_frame(&silence_frame());
        }
        let omni = beam.on_doa_frame(&silence_frame());
        assert!((omni.omni_mix - 1.0).abs() < 1e-3);
        // High-confidence frame should start ramping back to cardioid.
        let next = beam.on_doa_frame(&high_conf_frame(90.0, 0.0));
        let expected_step = 1.0 / OMNI_RAMP_FRAMES as f32;
        assert!((next.omni_mix - (1.0 - expected_step)).abs() < 1e-3,
            "omni_mix should decrement one step (~{:.3}), got {}", 1.0 - expected_step, next.omni_mix);
    }
}
