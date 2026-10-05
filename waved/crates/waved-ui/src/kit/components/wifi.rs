//! Wi-Fi pieces that need raw LVGL: the drawn signal arcs (wifi.png) and the
//! QR panel (wifi-setup.png). Owned by workstream B (B7).

use crate::bindings::*;
use crate::kit::components::dial::{arc, arc_style, INDICATOR, MAIN};
use crate::kit::obj::{color, Obj};
use crate::kit::tokens::{lvgl_angle, Rect, BG, LINE, LIVE, WHITE};

/// Arc radii, innermost first.
pub const SIGNAL_RADII: [i32; 4] = [9, 22, 35, 48];
/// Each arc runs from -SIGNAL_SPAN to +SIGNAL_SPAN device degrees (0 = up).
const SIGNAL_SPAN: f32 = 42.0;
const SIGNAL_WIDTH: i32 = 5;
const QR_RADIUS: i32 = 8;

/// Four concentric arcs opening upward around (cx, cy), the Wi-Fi strength glyph.
pub struct SignalArcs {
    arcs: Vec<Obj>,
}

impl SignalArcs {
    /// All four arcs in Line; call `set_bars` to light them.
    pub fn new(parent: Obj, cx: i32, cy: i32) -> Self {
        let arcs = SIGNAL_RADII
            .iter()
            .map(|&r| {
                let obj = arc(parent, cx, cy, r, SIGNAL_WIDTH);
                // SAFETY: live arc.
                unsafe {
                    lv_arc_set_bg_angles(
                        obj.raw(),
                        lvgl_angle(-SIGNAL_SPAN),
                        lvgl_angle(SIGNAL_SPAN),
                    );
                }
                arc_style(obj, MAIN, LINE, 255, SIGNAL_WIDTH);
                arc_style(obj, INDICATOR, LINE, 0, SIGNAL_WIDTH);
                obj
            })
            .collect();
        Self { arcs }
    }

    /// Light the inner `lit` arcs (0-4) in Live; the rest are Line. This is the
    /// one place that decides which arcs a bar count lights (wifi.png greys the
    /// innermost arc instead; see the plan's open points).
    pub fn set_bars(&self, lit: u8) {
        for (index, obj) in self.arcs.iter().enumerate() {
            let hex = if index < usize::from(lit) { LIVE } else { LINE };
            arc_style(*obj, MAIN, hex, 255, SIGNAL_WIDTH);
        }
    }

    /// The arc at `index` in `SIGNAL_RADII` order (innermost first).
    pub fn arc(&self, index: usize) -> Obj {
        self.arcs[index]
    }
}

/// A white rounded panel holding a QR code with a quiet zone.
pub struct Qr {
    panel: Obj,
    code: Obj,
}

impl Qr {
    /// A white rounded panel at `rect` with the code inset `quiet` px.
    pub fn new(parent: Obj, rect: Rect, quiet: i32) -> Self {
        let panel = Obj::plain(parent).rect(rect).bg(WHITE).radius(QR_RADIUS);
        // SAFETY: panel is live; the qrcode object belongs to it.
        let code = unsafe { Obj::from_raw(lv_qrcode_create(panel.raw())) };
        // SAFETY: live qrcode.
        unsafe {
            lv_obj_remove_style(
                code.raw(),
                std::ptr::null(),
                (LV_PART_ANY | LV_STATE_ANY) as lv_style_selector_t,
            );
            lv_qrcode_set_size(code.raw(), rect.w - 2 * quiet);
            lv_qrcode_set_dark_color(code.raw(), color(BG));
            lv_qrcode_set_light_color(code.raw(), color(WHITE));
        }
        let code = code.clickable(false).center();
        Self { panel, code }
    }

    /// Encode `data`; false when it doesn't fit in a QR code.
    pub fn set_data(&self, data: &str) -> bool {
        // SAFETY: live qrcode; LVGL reads `data.len()` bytes and copies the result.
        let result =
            unsafe { lv_qrcode_update(self.code.raw(), data.as_ptr().cast(), data.len() as u32) };
        result == lv_result_t_LV_RESULT_OK
    }

    /// The panel (show or hide it as a whole).
    pub fn obj(&self) -> Obj {
        self.panel
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, lvgl};
    use crate::kit::tokens::polar;

    #[test]
    fn signal_arcs_light_from_the_inside() {
        lvgl(|lv| {
            let root = lv.screen();
            let arcs = SignalArcs::new(root, 200, 126);
            arcs.set_bars(3);
            let frame = lv.render();
            assert_golden("wifi_signal_3", &frame);
            let teal = |[r, g, b]: [u8; 3]| g > 150 && g > r + 60 && b > 100;
            for (radius, lit) in [(9, true), (22, true), (35, true), (48, false)] {
                let (x, y) = polar(200, 126, radius as f32, 0.0);
                assert_eq!(teal(frame.pixel(x, y)), lit, "r {radius}");
            }
            // Nothing below the arcs' ends (±42°).
            let (x, y) = polar(200, 126, 48.0, 60.0);
            assert_eq!(frame.pixel(x, y), [0, 0, 0]);
        });
    }

    #[test]
    fn the_qr_panel_encodes_and_rejects_oversized_data() {
        lvgl(|lv| {
            let root = lv.screen();
            let qr = Qr::new(root, Rect::new(68, 68, 120, 120), 8);
            assert!(qr.set_data("WIFI:T:WPA;S:Wave-7F2A;P:k3x9p2vw;H:false;;"));
            let frame = lv.render();
            assert_golden("wifi_qr", &frame);
            assert_eq!(frame.pixel(72, 128), [255, 255, 255], "quiet zone");
            assert!(!qr.set_data(&"x".repeat(5_000)));
            assert!(!qr.obj().is_clickable());
        });
    }
}
