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
pub const PANEL_CX: i32 = 200;
pub const PANEL_CY: i32 = 152;
pub const PANEL_R: i32 = 190;
/// No tappable pixel may lie farther than this from the panel centre.
pub const TAP_LIMIT_R: i32 = 182;
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

    /// Squared distance from the panel centre to the farthest corner.
    pub fn max_dist2(&self) -> i32 {
        let xs = [self.x, self.x + self.w - 1];
        let ys = [self.y, self.y + self.h - 1];
        xs.iter()
            .flat_map(|x| ys.iter().map(move |y| (x - PANEL_CX).pow(2) + (y - PANEL_CY).pow(2)))
            .max()
            .unwrap_or(0)
    }

    pub fn within_tap_limit(&self) -> bool {
        self.max_dist2() <= TAP_LIMIT_R * TAP_LIMIT_R
    }
}

// Placements from the component table in docs/mockups/README.md.
pub const STATUS: Rect = Rect::new(100, 8, 200, 20);
pub const STATUS_PILL: Rect = Rect::new(100, 7, 200, 28);
pub const TITLE_Y: i32 = 38;
pub const BACK: Rect = Rect::new(144, 252, 112, 44);
pub const PRIMARY: Rect = Rect::new(108, 216, 184, 56);
pub const SIDE_LEFT: Rect = Rect::new(28, 102, 60, 60);
pub const SIDE_RIGHT: Rect = Rect::new(312, 102, 60, 60);
pub const SIDE_LABEL_Y: i32 = 168;
pub const ROW2: [Rect; 2] = [Rect::new(86, 222, 108, 48), Rect::new(206, 222, 108, 48)];
pub const ROW3: [Rect; 3] = [
    Rect::new(72, 222, 80, 48),
    Rect::new(160, 222, 80, 48),
    Rect::new(248, 222, 80, 48),
];
pub const DIAL_CX: i32 = 200;
pub const DIAL_CY: i32 = 132;
pub const DIAL_R: i32 = 78;
/// Left edge of the 20 px icon at the top of the dial's text stack (Home's mic).
pub const DIAL_ICON_X: i32 = 190;
/// Top of that icon.
pub const DIAL_ICON_Y: i32 = 74;
/// Top of the dial's headline ("Mic on").
pub const DIAL_TITLE_Y: i32 = 100;
/// Top of the dial's status line ("In a call").
pub const DIAL_LINE_Y: i32 = 134;
/// Top of the dial's hint ("Tap to mute").
pub const DIAL_HINT_Y: i32 = 172;
/// Top of the content under a sub-screen title (the first radio row).
pub const CONTENT_TOP: i32 = 70;
pub const MESSAGE_MAX_W: i32 = 230;

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

    #[test]
    fn every_layout_rect_respects_the_tap_limit() {
        let rects = [
            STATUS_PILL, BACK, PRIMARY, SIDE_LEFT, SIDE_RIGHT, ROW2[0], ROW2[1], ROW3[0], ROW3[1], ROW3[2],
        ];
        for rect in rects {
            assert!(rect.within_tap_limit(), "{rect:?}");
        }
    }

    #[test]
    fn targets_meet_the_minimum_size() {
        for rect in [BACK, SIDE_LEFT, SIDE_RIGHT, ROW2[0], ROW2[1], ROW3[0], ROW3[1], ROW3[2]] {
            assert!(rect.w >= MIN_TARGET && rect.h >= MIN_TARGET, "{rect:?}");
        }
        const { assert!(PRIMARY.h >= MIN_PRIMARY) };
    }

    #[test]
    fn the_old_back_button_fails_the_tap_limit() {
        assert!(!Rect::new(8, 8, 56, 64).within_tap_limit());
    }

    #[test]
    fn device_angles_map_to_lvgl_and_points() {
        assert_eq!(lvgl_angle(0.0), 270);
        assert_eq!(lvgl_angle(90.0), 0);
        assert_eq!(lvgl_angle(-90.0), 180);
        assert_eq!(lvgl_angle(450.0), 0);
        assert_eq!(polar(200, 132, 78.0, 0.0), (200, 54));
        assert_eq!(polar(200, 132, 78.0, 90.0), (278, 132));
        assert_eq!(polar(200, 132, 78.0, 180.0), (200, 210));
    }
}
