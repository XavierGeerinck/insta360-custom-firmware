//! Design tokens from `docs/mockups/README.md` (revision 2). Colours are 0xRRGGBB.

pub const BG: u32 = 0x000000;
pub const SURFACE: u32 = 0x141c1e;
pub const RAISED: u32 = 0x1f292b;
pub const LINE: u32 = 0x2a3639;
pub const TEXT: u32 = 0xf2f5f4;
pub const TEXT_2: u32 = 0x9aa7a6;
pub const TEXT_3: u32 = 0x5d6a6a;
pub const LIVE: u32 = 0x3fd6b1;
pub const ON_LIVE: u32 = 0x00140f;
pub const REC: u32 = 0xff4b3e;
pub const WARN: u32 = 0xffb547;
pub const WHITE: u32 = 0xffffff;

pub const SCREEN_W: i32 = 400;
pub const SCREEN_H: i32 = 304;
/// The visible image is a circle of r 198 around (200, 106), cut flat at
/// framebuffer row 0 (docs/DISPLAY_HARDWARE.md, "Visible Area").
pub const PANEL_CX: i32 = 200;
pub const PANEL_CY: i32 = 106;
pub const PANEL_R: i32 = 198;
/// No tappable pixel may lie farther than this from the panel centre...
pub const TAP_LIMIT_R: i32 = 190;
/// ...or above this row (6 px in from the flat top edge).
pub const TAP_MIN_Y: i32 = 6;
pub const MIN_TARGET: i32 = 44;
pub const MIN_PRIMARY: i32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Distance from the panel centre to the farthest pixel of this rect
    /// drawn with corner `radius` (clamped to half the shorter side, so
    /// `LV_RADIUS_CIRCLE` works). Radius 0 measures the corners of the box.
    pub fn farthest(&self, radius: i32) -> f32 {
        let r = radius.clamp(0, self.w.min(self.h) / 2);
        let xs = [self.x + r, self.x + self.w - 1 - r];
        let ys = [self.y + r, self.y + self.h - 1 - r];
        xs.iter()
            .flat_map(|x| ys.iter().map(move |y| (x - PANEL_CX, y - PANEL_CY)))
            .map(|(dx, dy)| ((dx * dx + dy * dy) as f32).sqrt() + r as f32)
            .fold(0.0, f32::max)
    }

    /// The whole box is within `TAP_LIMIT_R` of the panel centre and at or
    /// below `TAP_MIN_Y`.
    pub fn within_tap_limit(&self) -> bool {
        self.within_tap_limit_rounded(0)
    }

    /// Like `within_tap_limit`, for a box drawn with corner `radius`: only
    /// its drawn pixels count, so a rounded corner may cut past the limit.
    pub fn within_tap_limit_rounded(&self, radius: i32) -> bool {
        self.y >= TAP_MIN_Y && self.farthest(radius) <= TAP_LIMIT_R as f32
    }
}

// Placements from the component table in docs/mockups/README.md.
/// The status cluster on sub-screens.
pub const STATUS: Rect = Rect::new(80, 8, 240, 20);
/// The status pill on Home and Recording (radius 14)...
pub const STATUS_PILL: Rect = Rect::new(80, 6, 240, 28);
/// ...and its hit area, which ends where the dial's tap circle begins.
pub const STATUS_PILL_HIT: Rect = Rect::new(80, 6, 240, 44);
/// Messages take the status slot: h 28, centred, this wide at most.
pub const MESSAGE_MAX_W: i32 = 240;
pub const TITLE_Y: i32 = 38;
pub const BACK: Rect = Rect::new(144, 244, 112, 44);
pub const SIDE_LEFT: Rect = Rect::new(26, 91, 60, 60);
pub const SIDE_RIGHT: Rect = Rect::new(314, 91, 60, 60);
pub const SIDE_LABEL_Y: i32 = 155;

/// Bottom round buttons are centred on this arc around (PANEL_CX, PANEL_CY).
pub const BOTTOM_ARC_R: i32 = 135;
/// Width of every round-button label, bottom and side (micro 12, centred under its button).
pub const ROUND_LABEL_W: i32 = 90;
/// The main spot at the bottom of the arc: Record and Stop (one control),
/// also Send and Set up Wi-Fi. Icon 26 px.
pub const BOTTOM_MAIN: Rect = Rect::new(168, 209, 64, 64);
pub const BOTTOM_MAIN_LABEL: (i32, i32) = (155, 279);
/// Stop's hold ring: r 35 around the main spot's centre (just outside its
/// r 32 circle), stroke 3. Not tappable.
pub const BOTTOM_HOLD_R: i32 = 35;
pub const BOTTOM_HOLD_WIDTH: i32 = 3;
/// The companions at ±40° on the arc: Mark / Pause (Resume), Keep / Delete. Icon 22 px.
pub const BOTTOM_LEFT: Rect = Rect::new(85, 181, 56, 56);
pub const BOTTOM_RIGHT: Rect = Rect::new(259, 181, 56, 56);
pub const BOTTOM_LEFT_LABEL: (i32, i32) = (68, 241);
pub const BOTTOM_RIGHT_LABEL: (i32, i32) = (242, 241);
/// The equal pair on the companion centres: Decline / Answer. Icon 26 px.
pub const BOTTOM_PAIR_LEFT: Rect = Rect::new(81, 177, 64, 64);
pub const BOTTOM_PAIR_RIGHT: Rect = Rect::new(255, 177, 64, 64);
pub const BOTTOM_PAIR_LEFT_LABEL: (i32, i32) = (68, 245);
pub const BOTTOM_PAIR_RIGHT_LABEL: (i32, i32) = (242, 245);
/// The wide pill centred on the main spot (radius 26): Stop USB storage.
pub const BOTTOM_PILL_WIDE: Rect = Rect::new(108, 215, 184, 52);
/// The pill pair mirrored around x 200 (radius 25): confirmation Cancel / action.
pub const BOTTOM_PILL_PAIR: [Rect; 2] = [Rect::new(70, 196, 124, 50), Rect::new(206, 196, 124, 50)];

pub const DIAL_CX: i32 = 200;
pub const DIAL_CY: i32 = 121;
pub const DIAL_R: i32 = 70;
/// Left edge of the 20 px icon at the top of the dial's text stack (Home's mic).
pub const DIAL_ICON_X: i32 = 190;
/// Top of that icon.
pub const DIAL_ICON_Y: i32 = 73;
/// Top of the dial's headline ("Mic on").
pub const DIAL_TITLE_Y: i32 = 95;
/// Top of the dial's status line ("In a call").
pub const DIAL_LINE_Y: i32 = 127;
/// Top of the dial's hint ("Tap to mute").
pub const DIAL_HINT_Y: i32 = 153;
/// Recording's text stack inside the dial (tops of the kit labels): the
/// "Recording" chip...
pub const REC_CHIP_Y: i32 = 76;
/// ...the 48 px timer, placed by its digits' baseline (y 138, as in
/// recording.png), not by its box: the Num48 box is 43 tall with 9 px under
/// the baseline, so its top is 104...
pub const REC_TIMER_Y: i32 = 104;
/// ...the talker line ("Speaker 2 talking") and the hint ("Tap to mute").
pub const REC_LINE_Y: i32 = 140;
pub const REC_HINT_Y: i32 = 161;
/// The chip's red dot and the gap before its word.
pub const REC_DOT: i32 = 7;
pub const REC_DOT_GAP: i32 = 5;
/// Top of the content under a sub-screen title (62 for a sub-line directly
/// under the title).
pub const CONTENT_TOP: i32 = 70;
/// Top of the first radio row under a sub-screen title (rows at 68 / 124 / 180).
pub const RADIO_TOP: i32 = 68;

pub const MESSAGE_MS_SHORT: u64 = 2_000;
pub const MESSAGE_MS_LONG: u64 = 4_000;
pub const GAUGE_MS: u64 = 1_500;
pub const HOLD_TO_STOP_MS: u64 = 1_000;

/// Device angles: 0° is the top of the screen, clockwise (the DoA convention).
/// LVGL arcs measure 0° at 3 o'clock, clockwise.
pub fn lvgl_angle(device_deg: f32) -> i32 {
    ((device_deg - 90.0).rem_euclid(360.0).round() as i32) % 360
}

/// The point `r` px from (cx, cy) at a device angle.
pub fn polar(cx: i32, cy: i32, r: f32, device_deg: f32) -> (i32, i32) {
    let t = device_deg.to_radians();
    (
        (cx as f32 + r * t.sin()).round() as i32,
        (cy as f32 - r * t.cos()).round() as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tappable layout rect with the corner radius it is drawn with.
    fn tappable() -> Vec<(&'static str, Rect, i32)> {
        let round = |r: Rect| r.w / 2;
        vec![
            ("status pill", STATUS_PILL, 14),
            ("status pill hit area", STATUS_PILL_HIT, 0),
            ("back", BACK, 22),
            ("side left", SIDE_LEFT, round(SIDE_LEFT)),
            ("side right", SIDE_RIGHT, round(SIDE_RIGHT)),
            ("bottom main", BOTTOM_MAIN, round(BOTTOM_MAIN)),
            ("bottom left", BOTTOM_LEFT, round(BOTTOM_LEFT)),
            ("bottom right", BOTTOM_RIGHT, round(BOTTOM_RIGHT)),
            ("pair left", BOTTOM_PAIR_LEFT, round(BOTTOM_PAIR_LEFT)),
            ("pair right", BOTTOM_PAIR_RIGHT, round(BOTTOM_PAIR_RIGHT)),
            ("pill wide", BOTTOM_PILL_WIDE, 26),
            ("pill pair left", BOTTOM_PILL_PAIR[0], 25),
            ("pill pair right", BOTTOM_PILL_PAIR[1], 25),
        ]
    }

    #[test]
    fn every_layout_rect_respects_the_tap_limit() {
        for (name, rect, radius) in tappable() {
            assert!(
                rect.within_tap_limit_rounded(radius),
                "{name} {rect:?} reaches {:.1} px",
                rect.farthest(radius)
            );
        }
        // The bottom buttons stay well inside, the main one within r 167.
        assert!(BOTTOM_MAIN.farthest(32) <= 167.5);
    }

    #[test]
    fn targets_meet_the_minimum_size() {
        for (name, rect, _) in tappable() {
            if name == "status pill" {
                continue; // its hit area provides the height
            }
            assert!(
                rect.w >= MIN_TARGET && rect.h >= MIN_TARGET,
                "{name} {rect:?}"
            );
        }
        const { assert!(STATUS_PILL_HIT.h >= MIN_TARGET) };
        for rect in [
            BOTTOM_MAIN,
            BOTTOM_PAIR_LEFT,
            BOTTOM_PILL_WIDE,
            BOTTOM_PILL_PAIR[0],
        ] {
            assert!(rect.h >= MIN_PRIMARY, "{rect:?}");
        }
    }

    #[test]
    fn the_tap_limit_is_a_circle_cut_at_the_top_edge() {
        // r 190 around (200, 106).
        assert!(Rect::new(10, 106, 10, 1).within_tap_limit());
        assert!(!Rect::new(9, 106, 10, 1).within_tap_limit());
        assert!(Rect::new(200, 287, 1, 10).within_tap_limit());
        assert!(!Rect::new(200, 288, 1, 10).within_tap_limit());
        // The flat top: y 6 is the first tappable row.
        assert!(Rect::new(150, TAP_MIN_Y, 100, 44).within_tap_limit());
        assert!(!Rect::new(150, TAP_MIN_Y - 1, 100, 44).within_tap_limit());
        assert!(!Rect::new(150, TAP_MIN_Y - 1, 100, 44).within_tap_limit_rounded(22));
    }

    #[test]
    fn rounded_corners_count_only_their_drawn_pixels() {
        // The third radio row: its box corner is 193.7 px out, its rounded corner 185.6.
        let row = Rect::new(52, 180, 296, 52);
        assert!(!row.within_tap_limit());
        assert!(row.within_tap_limit_rounded(20));
        // A radius past half the box clamps (LV_RADIUS_CIRCLE).
        assert_eq!(BOTTOM_MAIN.farthest(0x7fff), BOTTOM_MAIN.farthest(32));
        assert_eq!(
            Rect::new(0, 0, 10, 10).farthest(0),
            Rect::new(0, 0, 10, 10).farthest(-3)
        );
    }

    #[test]
    fn the_old_back_button_fails_the_tap_limit() {
        // Revision 1's corner button and milestone 1's Back at y 252.
        assert!(!Rect::new(8, 8, 56, 64).within_tap_limit_rounded(28));
        assert!(!Rect::new(144, 252, 112, 44).within_tap_limit_rounded(22));
        assert!(BACK.within_tap_limit_rounded(22));
    }

    #[test]
    fn bottom_buttons_sit_on_the_arc_mirrored_around_the_centre() {
        let centre = |r: Rect| (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
        let arc_r = |r: Rect| {
            let (x, y) = centre(r);
            (x - PANEL_CX as f32).hypot(y - PANEL_CY as f32)
        };
        for rect in [
            BOTTOM_MAIN,
            BOTTOM_LEFT,
            BOTTOM_RIGHT,
            BOTTOM_PAIR_LEFT,
            BOTTOM_PAIR_RIGHT,
        ] {
            assert!((arc_r(rect) - BOTTOM_ARC_R as f32).abs() < 0.5, "{rect:?}");
        }
        assert_eq!(
            centre(BOTTOM_MAIN),
            (PANEL_CX as f32, (PANEL_CY + BOTTOM_ARC_R) as f32)
        );
        // The companions sit 40° either side of the bottom.
        let (x, y) = centre(BOTTOM_LEFT);
        let deg = (PANEL_CX as f32 - x)
            .atan2(y - PANEL_CY as f32)
            .to_degrees();
        assert!((deg - 40.0).abs() < 0.5, "{deg}");
        assert_eq!(centre(BOTTOM_PAIR_LEFT), centre(BOTTOM_LEFT));
        let mirrored = |l: Rect, r: Rect| l.x + l.w + r.x == SCREEN_W && l.y == r.y && l.w == r.w;
        assert!(mirrored(BOTTOM_LEFT, BOTTOM_RIGHT));
        assert!(mirrored(BOTTOM_PAIR_LEFT, BOTTOM_PAIR_RIGHT));
        assert!(mirrored(BOTTOM_PILL_PAIR[0], BOTTOM_PILL_PAIR[1]));
        assert!(mirrored(SIDE_LEFT, SIDE_RIGHT));
        assert_eq!(centre(BOTTOM_PILL_WIDE).0, centre(BOTTOM_MAIN).0);
        // Stop's hold ring runs just outside the main circle.
        const { assert!(BOTTOM_HOLD_R - BOTTOM_HOLD_WIDTH / 2 > BOTTOM_MAIN.w / 2) };
        // Labels are centred under their buttons.
        for (rect, (x, y)) in [
            (BOTTOM_MAIN, BOTTOM_MAIN_LABEL),
            (BOTTOM_LEFT, BOTTOM_LEFT_LABEL),
            (BOTTOM_RIGHT, BOTTOM_RIGHT_LABEL),
            (BOTTOM_PAIR_LEFT, BOTTOM_PAIR_LEFT_LABEL),
            (BOTTOM_PAIR_RIGHT, BOTTOM_PAIR_RIGHT_LABEL),
        ] {
            assert!((x as f32 + ROUND_LABEL_W as f32 / 2.0 - centre(rect).0).abs() <= 0.5);
            assert!(y > rect.y + rect.h, "{rect:?}: label below the button");
        }
    }

    #[test]
    fn the_dial_sits_between_the_status_pill_and_the_main_button() {
        // The tap circle (r + 1) starts where the status pill's hit area ends.
        assert_eq!(
            DIAL_CY - (DIAL_R + 1),
            STATUS_PILL_HIT.y + STATUS_PILL_HIT.h
        );
        const { assert!(DIAL_CY + DIAL_R + 1 < BOTTOM_MAIN.y) };
        // Even clear space from the ring (stroke 3) to the pill and the main button.
        let above = (DIAL_CY - DIAL_R) as f32 - 1.5 - (STATUS_PILL.y + STATUS_PILL.h) as f32;
        let below = BOTTOM_MAIN.y as f32 - ((DIAL_CY + DIAL_R) as f32 + 1.5);
        assert_eq!((above, below), (15.5, 16.5));
        // The side buttons and the dial share a centre line.
        assert_eq!(SIDE_LEFT.y + SIDE_LEFT.h / 2, DIAL_CY);
        assert_eq!(SIDE_LABEL_Y, SIDE_LEFT.y + SIDE_LEFT.h + 4);
    }

    #[test]
    fn device_angles_map_to_lvgl_and_points() {
        assert_eq!(lvgl_angle(0.0), 270);
        assert_eq!(lvgl_angle(90.0), 0);
        assert_eq!(lvgl_angle(-90.0), 180);
        assert_eq!(lvgl_angle(450.0), 0);
        assert_eq!(polar(DIAL_CX, DIAL_CY, 70.0, 0.0), (200, 51));
        assert_eq!(polar(DIAL_CX, DIAL_CY, 70.0, 90.0), (270, 121));
        assert_eq!(polar(DIAL_CX, DIAL_CY, 70.0, 180.0), (200, 191));
        assert_eq!(polar(PANEL_CX, PANEL_CY, 135.0, 220.0), (113, 209));
    }
}
