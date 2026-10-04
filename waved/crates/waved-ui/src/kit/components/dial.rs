//! The room dial: a ring for the table showing idle, live, muted, recording
//! and paused, a wedge toward whoever talks, and the volume gauge and progress
//! ring. Tapping it mutes on Home and Recording.

use crate::bindings::*;
use crate::kit::event::on_click;
use crate::kit::obj::{color, Obj};
use crate::kit::tokens::{lvgl_angle, polar, LINE, LIVE, REC, WARN};
use std::cell::Cell;

const MAIN: lv_style_selector_t = LV_PART_MAIN as lv_style_selector_t;
const INDICATOR: lv_style_selector_t = LV_PART_INDICATOR as lv_style_selector_t;
const TALKER_WIDTH: i32 = 7;
const TALKER_SPAN: f32 = 44.0;
/// The mockup dashes the ring `4 8` with round caps: a 12 px pitch is 9° on
/// r 78, and a 4 px stroke is 3°.
const DASHES: usize = 40;
const DASH_SPAN: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingStyle {
    Idle,
    Live,
    Muted,
    Recording,
    RecordingMuted,
    Paused,
}

/// What the ring object is showing right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Ring,
    Gauge,
    Progress,
}

pub struct Dial {
    parent: Obj,
    cx: i32,
    cy: i32,
    r: i32,
    width: i32,
    hit: Option<Obj>,
    ring: Obj,
    talker: Obj,
    dot: Obj,
    dashes: Vec<Obj>,
    style: RingStyle,
    mode: Mode,
    /// The last talker direction, drawn whenever the ring is showing.
    azimuth: Cell<Option<f32>>,
}

impl Dial {
    /// A ring of radius `r` and stroke `width` centred on (cx, cy).
    pub fn new(parent: Obj, cx: i32, cy: i32, r: i32, width: i32) -> Self {
        let ring = arc(parent, cx, cy, r, width);
        let talker = arc(parent, cx, cy, r, TALKER_WIDTH).hidden(true);
        let dot = Obj::plain(parent).size(8, 8).round().hidden(true);
        let mut dial = Self {
            parent,
            cx,
            cy,
            r,
            width,
            hit: None,
            ring,
            talker,
            dot,
            dashes: Vec::new(),
            style: RingStyle::Idle,
            mode: Mode::Ring,
            azimuth: Cell::new(None),
        };
        dial.set_ring(RingStyle::Idle);
        dial
    }

    /// Make the whole disc one tap target.
    pub fn on_tap(mut self, tap: impl FnMut() + 'static) -> Self {
        let size = 2 * self.r + 8;
        let disc = Obj::plain(self.parent)
            .pos(self.cx - size / 2, self.cy - size / 2)
            .size(size, size)
            .round()
            .clickable(true);
        on_click(disc, tap);
        self.hit = Some(disc);
        self
    }

    pub fn set_ring(&mut self, style: RingStyle) {
        self.style = style;
        self.mode = Mode::Ring;
        let (hex, opa) = match style {
            RingStyle::Idle => (LINE, 255),
            RingStyle::Live => (LIVE, 102),
            RingStyle::Muted | RingStyle::RecordingMuted => (REC, 255),
            RingStyle::Recording => (REC, 89),
            RingStyle::Paused => (WARN, 0),
        };
        if !self.talks() {
            self.azimuth.set(None);
        }
        self.place_ring(self.width);
        full_ring(self.ring);
        arc_style(self.ring, MAIN, hex, opa, self.width);
        arc_style(self.ring, INDICATOR, hex, 0, self.width);
        self.show_dashes(style == RingStyle::Paused);
        self.draw_talker();
    }

    /// Point the wedge toward a talker (device degrees), or clear it. The
    /// direction is kept while the gauge or progress shows and redrawn when
    /// the ring returns; styles that do not follow talkers ignore it.
    pub fn set_talker(&self, azimuth_deg: Option<f32>) {
        self.azimuth.set(azimuth_deg.filter(|_| self.talks()));
        self.draw_talker();
    }

    /// Show `percent` on a −135°…135° gauge in place of the ring, or restore it.
    pub fn set_gauge(&mut self, percent: Option<u8>) {
        let Some(value) = percent else {
            self.set_ring(self.style);
            return;
        };
        self.mode = Mode::Gauge;
        self.draw_talker();
        self.show_dashes(false);
        // Thicker than the ring, still centred on r.
        self.place_ring(self.width + 2);
        // SAFETY: live arc.
        unsafe {
            lv_arc_set_rotation(self.ring.raw(), 0);
            lv_arc_set_bg_angles(self.ring.raw(), lvgl_angle(-135.0), lvgl_angle(135.0));
            lv_arc_set_range(self.ring.raw(), 0, 100);
            lv_arc_set_value(self.ring.raw(), i32::from(value.min(100)));
        }
        arc_style(self.ring, MAIN, LINE, 255, self.width + 2);
        arc_style(self.ring, INDICATOR, LIVE, 255, self.width + 2);
    }

    /// Show progress 0..1 clockwise from the top, or restore the ring.
    pub fn set_progress(&mut self, fraction: Option<f32>) {
        let Some(fraction) = fraction else {
            self.set_ring(self.style);
            return;
        };
        self.mode = Mode::Progress;
        self.draw_talker();
        self.show_dashes(false);
        self.place_ring(self.width);
        full_ring(self.ring);
        // SAFETY: live arc.
        unsafe {
            lv_arc_set_value(self.ring.raw(), (fraction.clamp(0.0, 1.0) * 1000.0).round() as i32);
        }
        arc_style(self.ring, MAIN, LINE, 255, self.width);
        arc_style(self.ring, INDICATOR, LIVE, 255, self.width);
    }

    pub fn hit(&self) -> Option<Obj> {
        self.hit
    }

    pub fn talker_visible(&self) -> bool {
        !self.talker.is_hidden()
    }

    /// Whether the current ring style follows talkers.
    fn talks(&self) -> bool {
        matches!(self.style, RingStyle::Live | RingStyle::Recording)
    }

    /// Size the ring object so a stroke of `width` is centred on r.
    fn place_ring(&self, width: i32) {
        let size = 2 * self.r + width;
        self.ring.pos(self.cx - size / 2, self.cy - size / 2).size(size, size);
    }

    /// Draw the wedge and dot from the stored direction, only while the ring
    /// shows in a style that follows talkers.
    fn draw_talker(&self) {
        let hex = if self.style == RingStyle::Recording { REC } else { LIVE };
        match self.azimuth.get() {
            Some(az) if self.mode == Mode::Ring && self.talks() => {
                // SAFETY: live arc.
                unsafe {
                    lv_arc_set_bg_angles(
                        self.talker.raw(),
                        lvgl_angle(az - TALKER_SPAN / 2.0),
                        lvgl_angle(az + TALKER_SPAN / 2.0),
                    );
                }
                arc_style(self.talker, MAIN, hex, 255, TALKER_WIDTH);
                arc_style(self.talker, INDICATOR, hex, 0, TALKER_WIDTH);
                let (x, y) = polar(self.cx, self.cy, (self.r + 14) as f32, az);
                self.dot.pos(x - 4, y - 4).bg(hex).hidden(false);
                self.talker.hidden(false);
            }
            _ => {
                self.talker.hidden(true);
                self.dot.hidden(true);
            }
        }
    }

    fn show_dashes(&mut self, on: bool) {
        if on && self.dashes.is_empty() {
            let pitch = 360.0 / DASHES as f32;
            self.dashes = (0..DASHES)
                .map(|i| {
                    let dash = arc(self.parent, self.cx, self.cy, self.r, self.width);
                    let start = i as f32 * pitch;
                    // SAFETY: live arc.
                    unsafe {
                        lv_arc_set_bg_angles(dash.raw(), lvgl_angle(start), lvgl_angle(start + DASH_SPAN));
                    }
                    arc_style(dash, MAIN, WARN, 255, self.width);
                    arc_style(dash, INDICATOR, WARN, 0, self.width);
                    dash
                })
                .collect();
        }
        for dash in &self.dashes {
            dash.hidden(!on);
        }
    }
}

/// A non-interactive arc whose stroke is centred on radius `r`, angles in
/// device-to-LVGL terms (rotation 0, full background, range 0..1000).
fn arc(parent: Obj, cx: i32, cy: i32, r: i32, width: i32) -> Obj {
    let size = 2 * r + width;
    let arc = Obj::arc(parent).pos(cx - size / 2, cy - size / 2).size(size, size);
    // SAFETY: live arc.
    unsafe {
        lv_arc_set_rotation(arc.raw(), 0);
        lv_arc_set_bg_angles(arc.raw(), 0, 360);
        lv_arc_set_range(arc.raw(), 0, 1000);
        lv_arc_set_value(arc.raw(), 0);
    }
    arc
}

/// Full circle with the indicator starting at the top.
fn full_ring(arc: Obj) {
    // SAFETY: live arc.
    unsafe {
        lv_arc_set_rotation(arc.raw(), 270);
        lv_arc_set_bg_angles(arc.raw(), 0, 360);
        lv_arc_set_range(arc.raw(), 0, 1000);
        lv_arc_set_value(arc.raw(), 0);
    }
}

fn arc_style(arc: Obj, part: lv_style_selector_t, hex: u32, opa: u8, width: i32) {
    // SAFETY: live arc.
    unsafe {
        lv_obj_set_style_arc_color(arc.raw(), color(hex), part);
        lv_obj_set_style_arc_opa(arc.raw(), opa, part);
        lv_obj_set_style_arc_width(arc.raw(), width, part);
        lv_obj_set_style_arc_rounded(arc.raw(), true, part);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl, Lvgl};
    use crate::kit::tokens::{DIAL_CX, DIAL_CY, DIAL_R};
    use std::cell::Cell;
    use std::rc::Rc;

    fn home_dial(lv: &mut Lvgl) -> (Obj, Dial) {
        let root = lv.screen();
        (root, Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3))
    }

    #[test]
    fn ring_styles_follow_the_dial_table() {
        lvgl(|lv| {
            let (_root, mut dial) = home_dial(lv);
            assert_golden("dial_idle", &lv.render());
            dial.set_ring(RingStyle::Live);
            dial.set_talker(Some(307.0));
            assert_golden("dial_live_talker", &lv.render());
            dial.set_ring(RingStyle::Muted);
            assert!(!dial.talker_visible(), "muted hides the talker wedge");
            assert_golden("dial_muted", &lv.render());
            dial.set_ring(RingStyle::Recording);
            dial.set_talker(Some(60.0));
            assert_golden("dial_recording_talker", &lv.render());
            dial.set_ring(RingStyle::Paused);
            assert_golden("dial_paused", &lv.render());
        });
    }

    #[test]
    fn gauge_and_progress_give_the_ring_back() {
        lvgl(|lv| {
            let (_root, mut dial) = home_dial(lv);
            dial.set_ring(RingStyle::Live);
            let ring = lv.render().to_png();
            dial.set_gauge(Some(60));
            assert_golden("dial_gauge_60", &lv.render());
            dial.set_gauge(None);
            assert!(lv.render().to_png() == ring, "the ring is back after the gauge");
            dial.set_progress(Some(0.64));
            assert_golden("dial_progress_64", &lv.render());
            dial.set_progress(None);
            assert!(lv.render().to_png() == ring, "the ring is back after progress");
        });
    }

    #[test]
    fn the_wedge_stays_off_the_gauge_and_progress_and_comes_back() {
        lvgl(|lv| {
            let (_root, mut dial) = home_dial(lv);
            dial.set_ring(RingStyle::Live);
            dial.set_talker(Some(307.0));
            let with_wedge = lv.render().to_png();
            for mode in 0..2 {
                if mode == 0 {
                    dial.set_gauge(Some(60));
                } else {
                    dial.set_progress(Some(0.64));
                }
                assert!(!dial.talker_visible(), "the gauge and progress hide the wedge");
                let shown = lv.render().to_png();
                dial.set_talker(Some(300.0));
                assert!(!dial.talker_visible(), "a talker update must not draw over the gauge");
                assert!(lv.render().to_png() == shown, "the frame is unchanged by the update");
                if mode == 0 {
                    dial.set_gauge(None);
                } else {
                    dial.set_progress(None);
                }
                assert!(dial.talker_visible(), "the wedge returns with the ring");
                dial.set_talker(Some(307.0));
                assert!(lv.render().to_png() == with_wedge, "the ring is back with its wedge");
            }
        });
    }

    #[test]
    fn changing_the_style_recolours_the_wedge_at_once() {
        lvgl(|lv| {
            let (_root, mut dial) = home_dial(lv);
            dial.set_ring(RingStyle::Live);
            dial.set_talker(Some(90.0));
            let [r, g, _] = lv.render().pixel(DIAL_CX + DIAL_R, DIAL_CY);
            assert!(g > 180 && r < 100, "teal wedge in Live, got r{r} g{g}");
            dial.set_ring(RingStyle::Recording);
            assert!(dial.talker_visible());
            let [r, g, _] = lv.render().pixel(DIAL_CX + DIAL_R, DIAL_CY);
            assert!(r > 200 && g < 120, "red wedge right after Live to Recording, got r{r} g{g}");
            dial.set_ring(RingStyle::Muted);
            assert!(!dial.talker_visible());
            dial.set_ring(RingStyle::Live);
            assert!(!dial.talker_visible(), "a muted spell forgets the old direction");
        });
    }

    #[test]
    fn a_wedge_crossing_ninety_degrees_draws_on_the_ring() {
        lvgl(|lv| {
            let (_root, mut dial) = home_dial(lv);
            dial.set_ring(RingStyle::Live);
            let ring = lv.render().pixel(DIAL_CX + DIAL_R, DIAL_CY);
            dial.set_talker(Some(90.0));
            assert!(dial.talker_visible());
            let [r, g, b] = lv.render().pixel(DIAL_CX + DIAL_R, DIAL_CY);
            assert!(g > 180 && r < 100 && [r, g, b] != ring, "full teal at 3 o'clock, got {r},{g},{b}");
        });
    }

    #[test]
    fn the_gauge_stroke_is_centred_on_the_ring_radius() {
        lvgl(|lv| {
            let (_root, mut dial) = home_dial(lv);
            // The lit columns along the 3 o'clock row.
            let spans = |lv: &mut Lvgl| -> Vec<i32> {
                let frame = lv.render();
                (-6..=6)
                    .filter(|dx| frame.pixel(DIAL_CX + DIAL_R + dx, DIAL_CY) != [0, 0, 0])
                    .collect()
            };
            dial.set_ring(RingStyle::Idle);
            let ring = spans(lv);
            dial.set_gauge(Some(100));
            let gauge = spans(lv);
            assert_eq!(ring.len(), 3);
            assert_eq!(gauge.len(), 5);
            // One pixel thicker on each side: the same centre line.
            assert_eq!((gauge[0], gauge[4]), (ring[0] - 1, ring[2] + 1), "ring {ring:?} gauge {gauge:?}");
        });
    }

    #[test]
    fn tapping_mutes_only_when_enabled() {
        lvgl(|lv| {
            let root = lv.screen();
            let taps = Rc::new(Cell::new(0));
            let counter = taps.clone();
            let dial = Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3).on_tap(move || counter.set(counter.get() + 1));
            assert_targets_ok(root);
            lv.click(dial.hit().unwrap());
            assert_eq!(taps.get(), 1);
            let (_root, plain) = home_dial(lv);
            assert!(plain.hit().is_none());
        });
    }
}
