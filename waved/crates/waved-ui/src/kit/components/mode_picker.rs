//! The radial audio-mode picker (B2, audio-mode.png), owned by workstream B.
//! There are six 48 px round buttons on r 132 around (200, 134) and a live
//! preview of the pickup pattern (r 50) drawn on an `lv_canvas`. Directional
//! modes are aimed by dragging inside the preview. The screen decides what
//! each mode looks like and what the preview shows.

// The geometry helpers below are used by the ModePicker component (Task 3);
// until it lands only the tests call them.

/// Pickup-pattern gain at `theta_deg` from the peak: |a + (1 − a)·cos θ|.
/// a = 1 is omni, 0.5 cardioid, 0.366 supercardioid, 0 figure-8.
#[cfg_attr(not(test), allow(dead_code))] // used by ModePicker (Task 3)
pub fn pattern_gain(a: f32, theta_deg: f32) -> f32 {
    (a + (1.0 - a) * theta_deg.to_radians().cos()).abs()
}

/// The point `r` px from (cx, cy) at a device angle (0° up, clockwise).
#[cfg_attr(not(test), allow(dead_code))] // used by ModePicker (Task 3)
pub fn polar_f(cx: f32, cy: f32, r: f32, device_deg: f32) -> (f32, f32) {
    let t = device_deg.to_radians();
    (cx + r * t.sin(), cy - r * t.cos())
}

/// Outline of a lobe with peak radius `r` around (cx, cy), its peak turned
/// `rot` degrees clockwise from the top, one point every `step` degrees
/// (closed; the first point is not repeated).
#[cfg_attr(not(test), allow(dead_code))] // used by ModePicker (Task 3)
pub fn lobe_points(cx: f32, cy: f32, r: f32, a: f32, rot: f32, step: f32) -> Vec<(f32, f32)> {
    let count = (360.0 / step).round() as usize;
    (0..count)
        .map(|i| {
            let theta = i as f32 * step;
            polar_f(cx, cy, r * pattern_gain(a, theta), theta + rot)
        })
        .collect()
}

/// Whether (x, y) lies inside the lobe drawn by `lobe_points`.
#[cfg_attr(not(test), allow(dead_code))] // used by ModePicker (Task 3)
pub fn inside_lobe(cx: f32, cy: f32, r: f32, a: f32, rot: f32, x: f32, y: f32) -> bool {
    let (dx, dy) = (x - cx, y - cy);
    let distance = dx.hypot(dy);
    if distance < 0.5 {
        return a > 0.0; // a figure-8 has no pickup at its centre
    }
    let theta = dx.atan2(-dy).to_degrees() - rot;
    distance <= r * pattern_gain(a, theta)
}

/// The device angle in [0, 360) of (x, y) seen from (cx, cy); `None` within
/// `dead` px of the centre, where a press has no direction.
#[cfg_attr(not(test), allow(dead_code))] // used by ModePicker (Task 3)
pub fn aim_angle(cx: i32, cy: i32, x: i32, y: i32, dead: i32) -> Option<f32> {
    let (dx, dy) = ((x - cx) as f32, (y - cy) as f32);
    if dx.hypot(dy) < dead as f32 {
        return None;
    }
    let degrees = dx.atan2(-dy).to_degrees().rem_euclid(360.0);
    Some(if degrees >= 360.0 { 0.0 } else { degrees })
}

/// `rgb` (0xRRGGBB) at `alpha`/255 over an RGB565 pixel.
#[cfg_attr(not(test), allow(dead_code))] // used by ModePicker (Task 3)
pub fn blend565(dst: u16, rgb: u32, alpha: u8) -> u16 {
    let a = u32::from(alpha);
    let dst = u32::from(dst);
    let (dr, dg, db) = ((dst >> 11) & 0x1f, (dst >> 5) & 0x3f, dst & 0x1f);
    let (sr, sg, sb) = ((rgb >> 19) & 0x1f, (rgb >> 10) & 0x3f, (rgb >> 3) & 0x1f);
    let mix = |s: u32, d: u32| (s * a + d * (255 - a) + 127) / 255;
    ((mix(sr, dr) << 11) | (mix(sg, dg) << 5) | mix(sb, db)) as u16
}

type Segment = ((f32, f32), (f32, f32));

/// Split a path into dashes of `on` px separated by `off` px, measured along
/// the path so the rhythm carries across corners.
#[cfg_attr(not(test), allow(dead_code))] // used by ModePicker (Task 3)
pub fn dash_segments(points: &[(f32, f32)], closed: bool, on: f32, off: f32) -> Vec<Segment> {
    let mut out = Vec::new();
    if points.len() < 2 {
        return out;
    }
    let edges = if closed {
        points.len()
    } else {
        points.len() - 1
    };
    let period = on + off;
    let mut walked = 0.0f32;
    for (&a, &b) in points.iter().zip(points.iter().cycle().skip(1)).take(edges) {
        let length = (b.0 - a.0).hypot(b.1 - a.1);
        let at = |t: f32| {
            (
                a.0 + (b.0 - a.0) * t / length,
                a.1 + (b.1 - a.1) * t / length,
            )
        };
        let mut s = 0.0f32;
        while s < length {
            let phase = (walked + s) % period;
            let (drawn, left) = if phase < on {
                (true, on - phase)
            } else {
                (false, period - phase)
            };
            let end = (s + left.max(1e-3)).min(length);
            if drawn {
                out.push((at(s), at(end)));
            }
            s = end;
        }
        walked += length;
    }
    out
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }

    #[test]
    fn patterns_follow_the_first_order_formula() {
        // r(θ) = |a + (1 − a)·cos θ| (index.html, data-polar).
        assert!((pattern_gain(1.0, 123.0) - 1.0).abs() < 1e-6); // omni
        assert!((pattern_gain(0.5, 0.0) - 1.0).abs() < 1e-6); // cardioid front
        assert!(pattern_gain(0.5, 180.0).abs() < 1e-6); // cardioid null
        assert!((pattern_gain(0.0, 180.0) - 1.0).abs() < 1e-6); // figure-8 back lobe
        assert!(pattern_gain(0.0, 90.0).abs() < 1e-6); // figure-8 side null
    }

    #[test]
    fn lobe_points_turn_with_the_aim() {
        let points = lobe_points(200.0, 134.0, 50.0, 0.5, 300.0, 3.0);
        assert_eq!(points.len(), 120);
        // θ = 0 is the peak, pointing at the aim: the handle spot in audio-mode.png.
        assert!(close(points[0], polar_f(200.0, 134.0, 50.0, 300.0)));
        // θ = 180° is the cardioid's null: the centre.
        assert!(close(points[60], (200.0, 134.0)));
        assert!(close(polar_f(200.0, 134.0, 50.0, 90.0), (250.0, 134.0)));
    }

    #[test]
    fn inside_lobe_matches_the_outline() {
        let (cx, cy) = (200.0, 134.0);
        assert!(inside_lobe(cx, cy, 50.0, 0.5, 0.0, 200.0, 94.0)); // 40 px in front
        assert!(!inside_lobe(cx, cy, 50.0, 0.5, 0.0, 200.0, 174.0)); // 40 px behind
        assert!(inside_lobe(cx, cy, 50.0, 0.0, 0.0, 200.0, 174.0)); // figure-8 back
        assert!(!inside_lobe(cx, cy, 50.0, 0.0, 0.0, 240.0, 134.0)); // figure-8 side
        assert!(!inside_lobe(cx, cy, 50.0, 0.0, 0.0, cx, cy)); // figure-8 centre is a null
        assert!(inside_lobe(cx, cy, 50.0, 1.0, 0.0, 230.0, 160.0));
        assert!(!inside_lobe(cx, cy, 50.0, 1.0, 0.0, 250.0, 180.0));
    }

    #[test]
    fn aim_angle_wraps_and_ignores_the_centre() {
        let a = |x, y| aim_angle(200, 134, x, y, 8);
        assert_eq!(a(200, 84), Some(0.0));
        assert_eq!(a(250, 134), Some(90.0));
        assert_eq!(a(200, 184), Some(180.0));
        assert_eq!(a(150, 134), Some(270.0));
        let just_left_of_top = a(199, 34).unwrap();
        assert!(
            (359.0..360.0).contains(&just_left_of_top),
            "{just_left_of_top}"
        );
        assert_eq!(a(200, 134), None);
        assert_eq!(a(205, 130), None); // 6.4 px out: inside the dead zone
        assert!(a(208, 134).is_some());
    }

    #[test]
    fn blending_over_rgb565() {
        assert_eq!(blend565(0x0000, 0xffffff, 255), 0xffff);
        assert_eq!(blend565(0xffff, 0x000000, 0), 0xffff);
        // Live at 16 % over black, the preview fill.
        assert_eq!(blend565(0x0000, 0x3fd6b1, 41), (1 << 11) | (9 << 5) | 4);
    }

    #[test]
    fn dashes_split_a_path_by_length() {
        let dashes = dash_segments(&[(0.0, 0.0), (20.0, 0.0)], false, 3.0, 4.0);
        let spans: Vec<(f32, f32)> = dashes.iter().map(|(a, b)| (a.0, b.0)).collect();
        assert_eq!(spans, vec![(0.0, 3.0), (7.0, 10.0), (14.0, 17.0)]);
        // The pattern carries over corners and around a closed path.
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let total: f32 = dash_segments(&square, true, 2.0, 3.0)
            .iter()
            .map(|(a, b)| (b.0 - a.0).hypot(b.1 - a.1))
            .sum();
        assert!((total - 16.0).abs() < 0.01, "{total}"); // 40 px at 2 on / 3 off
        assert!(dash_segments(&[(1.0, 1.0)], true, 2.0, 3.0).is_empty());
    }
}
