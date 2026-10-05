//! The radial audio-mode picker (B2, audio-mode.png), owned by workstream B.
//! There are six 48 px round buttons on r 132 around (200, 134) and a live
//! preview of the pickup pattern (r 50) drawn on an `lv_canvas`. Directional
//! modes are aimed by dragging inside the preview. The screen decides what
//! each mode looks like and what the preview shows.

use crate::bindings::*;
use crate::kit::event::{on, on_click};
use crate::kit::obj::{color, Obj};
use crate::kit::tokens::{polar, Rect, BG, LINE, LIVE, RAISED, SURFACE, TEXT, TEXT_2, TEXT_3};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Pickup-pattern gain at `theta_deg` from the peak: |a + (1 − a)·cos θ|.
/// a = 1 is omni, 0.5 cardioid, 0.366 supercardioid, 0 figure-8.
pub fn pattern_gain(a: f32, theta_deg: f32) -> f32 {
    (a + (1.0 - a) * theta_deg.to_radians().cos()).abs()
}

/// The point `r` px from (cx, cy) at a device angle (0° up, clockwise).
pub fn polar_f(cx: f32, cy: f32, r: f32, device_deg: f32) -> (f32, f32) {
    let t = device_deg.to_radians();
    (cx + r * t.sin(), cy - r * t.cos())
}

/// Outline of a lobe with peak radius `r` around (cx, cy), its peak turned
/// `rot` degrees clockwise from the top, one point every `step` degrees
/// (closed; the first point is not repeated).
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
pub fn aim_angle(cx: i32, cy: i32, x: i32, y: i32, dead: i32) -> Option<f32> {
    let (dx, dy) = ((x - cx) as f32, (y - cy) as f32);
    if dx.hypot(dy) < dead as f32 {
        return None;
    }
    let degrees = dx.atan2(-dy).to_degrees().rem_euclid(360.0);
    Some(if degrees >= 360.0 { 0.0 } else { degrees })
}

/// `rgb` (0xRRGGBB) at `alpha`/255 over an RGB565 pixel.
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

/// Centre of the button ring and of the preview.
pub const CENTRE: (i32, i32) = (200, 134);
/// The buttons sit on this circle...
pub const RING_R: f32 = 132.0;
/// ...at these device angles: the left column top to bottom, then the right.
pub const ANGLES: [f32; 6] = [300.0, 270.0, 240.0, 60.0, 90.0, 120.0];
pub const BUTTON: i32 = 48;
/// The preview's outer ring (the inner ring is half of it).
pub const PREVIEW_R: f32 = 50.0;
/// The preview canvas and the drag area: the pattern plus the r 8 handle and its 3 px rim.
pub const PREVIEW_BOX: Rect = Rect::new(138, 72, 124, 124);
/// Presses this close to the centre have no direction.
pub const DEAD_ZONE: i32 = 8;
/// A glyph canvas, centred in its button.
const GLYPH: i32 = 40;
/// The pattern fill: Live at 16 % (`rgba(63,214,177,.16)`).
const FILL_OPA: u8 = 41;
const OUTLINE_W: i32 = 3;
const RING_W: i32 = 2;
const RAY_W: i32 = 2;
const GLYPH_W: i32 = 2;
const DOT_R: f32 = 4.0;
const HANDLE_R: f32 = 8.0;
const HANDLE_RIM: f32 = 3.0;
/// Degrees between outline points: 3° for the preview, 6° for the glyphs.
const PREVIEW_STEP: f32 = 3.0;
const GLYPH_STEP: f32 = 6.0;

/// One pickup lobe: shape `a`, peak radius `r` px, peak turned `rot` degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lobe {
    pub a: f32,
    pub r: f32,
    pub rot: f32,
}

/// A button's drawing: solid lobes, plus an optional dashed Text 3 lobe
/// behind them (Follow talker's "where it was").
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub lobes: Vec<Lobe>,
    pub ghost: Option<Lobe>,
}

/// What the preview shows: lobes as (a, rot) at `PREVIEW_R`, filled together
/// and outlined in Live, and the aim handle with its dashed ray when aimable.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub lobes: Vec<(f32, f32)>,
    pub handle: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AimEvent {
    /// The finger is dragging the aim to this angle.
    Moving(f32),
    /// The finger lifted with the aim at this angle.
    Done(f32),
}

/// The 48 px box of the button at `angle` on the ring.
pub fn button_rect(angle: f32) -> Rect {
    let (x, y) = polar(CENTRE.0, CENTRE.1, RING_R, angle);
    Rect::new(x - BUTTON / 2, y - BUTTON / 2, BUTTON, BUTTON)
}

pub struct ModePicker {
    inner: Rc<Inner>,
}

struct Inner {
    buttons: Vec<Obj>,
    glyphs: Vec<Canvas>,
    specs: Vec<Glyph>,
    preview: Canvas,
    area: Obj,
    selected: Cell<usize>,
    shown: RefCell<Option<Preview>>,
    /// The last valid angle of the current drag, for a release in the dead zone.
    last_aim: Cell<Option<f32>>,
    /// Preview repaints, for tests.
    #[cfg(test)]
    paints: Cell<u32>,
    on_select: RefCell<Box<dyn FnMut(usize)>>,
    on_aim: RefCell<Box<dyn FnMut(AimEvent)>>,
}

impl ModePicker {
    /// The picker on `parent`, which must be the screen root (positions are
    /// in screen coordinates). `glyphs` follow `ANGLES`.
    pub fn new(
        parent: Obj,
        glyphs: [Glyph; 6],
        selected: usize,
        on_select: impl FnMut(usize) + 'static,
        on_aim: impl FnMut(AimEvent) + 'static,
    ) -> Self {
        let preview = Canvas::new(parent, PREVIEW_BOX, false);
        // The drag area is a disc over the preview. Press-lock keeps the drag
        // going when the finger slides out; keep_gestures, so a sideways drag
        // never becomes the global swipe-right Back.
        let area = Obj::plain(parent)
            .rect(PREVIEW_BOX)
            .round()
            .flag(lv_obj_flag_t_LV_OBJ_FLAG_PRESS_LOCK, true)
            .keep_gestures();
        let mut buttons = Vec::with_capacity(6);
        let mut canvases = Vec::with_capacity(6);
        for angle in ANGLES {
            let button = Obj::plain(parent)
                .rect(button_rect(angle))
                .round()
                .bg(SURFACE)
                .pressed_bg(RAISED)
                .pressed_border(2, LIVE)
                .clickable(true);
            let glyph = Canvas::new(button, Rect::new(0, 0, GLYPH, GLYPH), true);
            // Centred, so the 2 px selection border does not shift the glyph.
            glyph.obj.center();
            buttons.push(button);
            canvases.push(glyph);
        }
        let inner = Rc::new(Inner {
            buttons,
            glyphs: canvases,
            specs: Vec::from(glyphs),
            preview,
            area,
            selected: Cell::new(selected.min(5)),
            shown: RefCell::new(None),
            last_aim: Cell::new(None),
            #[cfg(test)]
            paints: Cell::new(0),
            on_select: RefCell::new(Box::new(on_select)),
            on_aim: RefCell::new(Box::new(on_aim)),
        });
        for (index, &button) in inner.buttons.iter().enumerate() {
            let shared = inner.clone();
            on_click(button, move || {
                shared.select(index);
                (shared.on_select.borrow_mut())(index);
            });
        }
        let shared = inner.clone();
        on(area, lv_event_code_t_LV_EVENT_PRESSED, move |_| {
            // A new drag: an aim left from a press that never ended is stale.
            shared.last_aim.set(None);
            shared.drag_from_indev(false);
        });
        for (code, done) in [
            (lv_event_code_t_LV_EVENT_PRESSING, false),
            (lv_event_code_t_LV_EVENT_RELEASED, true),
        ] {
            let shared = inner.clone();
            on(area, code, move |_| shared.drag_from_indev(done));
        }
        // A lost press (the input was reset or the area went away under the
        // finger) ends the drag like a release, at the last aim.
        let shared = inner.clone();
        on(area, lv_event_code_t_LV_EVENT_PRESS_LOST, move |_| {
            shared.finish(None)
        });
        inner.select(inner.selected.get());
        Self { inner }
    }

    pub fn set_selected(&self, index: usize) {
        if index != self.inner.selected.get() {
            self.inner.select(index.min(5));
        }
    }

    pub fn selected(&self) -> usize {
        self.inner.selected.get()
    }

    /// Redraw the preview when `preview` differs from what it shows.
    pub fn set_preview(&self, preview: &Preview) {
        if self.inner.shown.borrow().as_ref() == Some(preview) {
            return;
        }
        self.inner.paint_preview(preview);
        *self.inner.shown.borrow_mut() = Some(preview.clone());
    }

    /// Directional modes take drags; the others ignore them.
    pub fn set_aimable(&self, on: bool) {
        self.inner.area.clickable(on);
        self.inner.last_aim.set(None);
    }

    /// What a finger at (x, y) does: the input handler calls this with the
    /// indev point, and tests call it directly.
    pub fn drag_to(&self, x: i32, y: i32, done: bool) {
        self.inner.drag_to(x, y, done);
    }

    pub fn button(&self, index: usize) -> Obj {
        self.inner.buttons[index]
    }

    pub fn aim_area(&self) -> Obj {
        self.inner.area
    }
}

impl Inner {
    fn select(&self, index: usize) {
        self.selected.set(index);
        for (i, (button, glyph)) in self.buttons.iter().zip(&self.glyphs).enumerate() {
            let on = i == index;
            if on {
                button.bg(RAISED).border(2, LIVE);
            } else {
                button.bg(SURFACE).border(0, LIVE);
            }
            paint_glyph(glyph, &self.specs[i], on);
        }
    }

    fn drag_to(&self, x: i32, y: i32, done: bool) {
        if !self.area.is_clickable() {
            return;
        }
        let angle = aim_angle(CENTRE.0, CENTRE.1, x, y, DEAD_ZONE);
        if done {
            self.finish(angle);
        } else if let Some(angle) = angle {
            self.last_aim.set(Some(angle));
            (self.on_aim.borrow_mut())(AimEvent::Moving(angle));
        }
    }

    /// End the drag at `angle`, or at the last valid aim when it has none.
    fn finish(&self, angle: Option<f32>) {
        if !self.area.is_clickable() {
            return;
        }
        if let Some(angle) = angle.or(self.last_aim.get()) {
            (self.on_aim.borrow_mut())(AimEvent::Done(angle));
        }
        self.last_aim.set(None);
    }

    fn drag_from_indev(&self, done: bool) {
        // SAFETY: called from an LVGL event on the UI thread; the active
        // input device (if any) is valid for the duration of the event.
        let indev = unsafe { lv_indev_active() };
        if indev.is_null() {
            return;
        }
        let mut point = lv_point_t::default();
        // SAFETY: live indev, point is a valid out parameter.
        unsafe { lv_indev_get_point(indev, &mut point) };
        self.drag_to(point.x, point.y, done);
    }

    fn paint_preview(&self, preview: &Preview) {
        #[cfg(test)]
        self.paints.set(self.paints.get() + 1);
        let canvas = &self.preview;
        let centre = (CENTRE.0 as f32, CENTRE.1 as f32);
        canvas.clear();
        canvas.draw(|pen| {
            pen.ring(centre, PREVIEW_R, RING_W, LINE);
            pen.ring(centre, PREVIEW_R / 2.0, RING_W, LINE);
        });
        canvas.tint(LIVE, FILL_OPA, |x, y| {
            preview
                .lobes
                .iter()
                .any(|&(a, rot)| inside_lobe(centre.0, centre.1, PREVIEW_R, a, rot, x, y))
        });
        canvas.draw(|pen| {
            for &(a, rot) in &preview.lobes {
                let points = lobe_points(centre.0, centre.1, PREVIEW_R, a, rot, PREVIEW_STEP);
                pen.outline(&points, OUTLINE_W, LIVE);
            }
            if let Some(aim) = preview.handle {
                let tip = polar_f(centre.0, centre.1, PREVIEW_R, aim);
                for (from, to) in dash_segments(&[centre, tip], false, 3.0, 4.0) {
                    pen.line(from, to, RAY_W, TEXT_2);
                }
                pen.disc(centre, DOT_R, TEXT);
                pen.disc(tip, HANDLE_R + HANDLE_RIM, BG);
                pen.disc(tip, HANDLE_R, TEXT);
            } else {
                pen.disc(centre, DOT_R, TEXT);
            }
        });
    }
}

fn paint_glyph(canvas: &Canvas, glyph: &Glyph, selected: bool) {
    let centre = (GLYPH as f32 / 2.0, GLYPH as f32 / 2.0);
    canvas.clear();
    canvas.draw(|pen| {
        if let Some(ghost) = glyph.ghost {
            let points = lobe_points(centre.0, centre.1, ghost.r, ghost.a, ghost.rot, GLYPH_STEP);
            for (from, to) in dash_segments(&points, true, 2.0, 3.0) {
                pen.line(from, to, GLYPH_W, TEXT_3);
            }
        }
        let hex = if selected { LIVE } else { TEXT_2 };
        for lobe in &glyph.lobes {
            let points = lobe_points(centre.0, centre.1, lobe.r, lobe.a, lobe.rot, GLYPH_STEP);
            pen.outline(&points, GLYPH_W, hex);
        }
    });
}

/// An `lv_canvas` with a buffer that lives exactly as long as the object:
/// RGB565 and opaque for the preview, ARGB8888 and transparent for glyphs.
/// The buffer is sized and indexed by LVGL's row stride
/// (`lv_draw_buf_width_to_stride`), so a stride alignment set in lv_conf.h
/// can't overrun it.
struct Canvas {
    obj: Obj,
    w: i32,
    h: i32,
    /// Bytes per buffer row, as LVGL computes it for this width and format.
    stride: usize,
    /// Screen position of the buffer's top-left, to map screen points.
    origin: (i32, i32),
    pixels: *mut u32,
    argb: bool,
}

impl Canvas {
    fn new(parent: Obj, rect: Rect, argb: bool) -> Self {
        let format = if argb {
            lv_color_format_t_LV_COLOR_FORMAT_ARGB8888
        } else {
            lv_color_format_t_LV_COLOR_FORMAT_RGB565
        };
        // SAFETY: a pure calculation from LVGL's draw-buffer settings (LVGL is initialised).
        let stride = unsafe { lv_draw_buf_width_to_stride(rect.w as u32, format) } as usize;
        let words = (stride * rect.h as usize).div_ceil(4);
        let pixels: *mut [u32] = Box::into_raw(vec![0u32; words].into_boxed_slice());
        // SAFETY: parent is a live object (callers created it just before).
        let obj = unsafe { Obj::from_raw(lv_canvas_create(parent.raw())) };
        // SAFETY: live canvas; the buffer holds rect.h rows of `stride` bytes,
        // the stride lv_canvas_set_buffer computes for this width and format.
        unsafe {
            lv_obj_remove_style(
                obj.raw(),
                std::ptr::null(),
                (LV_PART_ANY | LV_STATE_ANY) as lv_style_selector_t,
            );
            lv_canvas_set_buffer(obj.raw(), pixels.cast(), rect.w, rect.h, format);
        }
        let obj = obj
            .clickable(false)
            .flag(lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE, false)
            .pos(rect.x, rect.y);
        let mut freed = false;
        on(obj, lv_event_code_t_LV_EVENT_DELETE, move |_| {
            if !freed {
                freed = true;
                // SAFETY: allocated above and freed once, when the canvas is deleted.
                unsafe { drop(Box::from_raw(pixels)) };
            }
        });
        Self {
            obj,
            w: rect.w,
            h: rect.h,
            stride,
            origin: (rect.x, rect.y),
            pixels: pixels.cast(),
            argb,
        }
    }

    fn clear(&self) {
        let opa = (if self.argb {
            LV_OPA_TRANSP
        } else {
            LV_OPA_COVER
        }) as lv_opa_t;
        // SAFETY: live canvas with a buffer.
        unsafe { lv_canvas_fill_bg(self.obj.raw(), color(BG), opa) };
    }

    fn draw(&self, paint: impl FnOnce(&mut Pen)) {
        let mut layer = lv_layer_t::default();
        // SAFETY: live canvas; the layer is finished before it goes out of scope.
        unsafe { lv_canvas_init_layer(self.obj.raw(), &mut layer) };
        paint(&mut Pen {
            layer: &mut layer,
            origin: self.origin,
        });
        // SAFETY: same canvas and layer.
        unsafe { lv_canvas_finish_layer(self.obj.raw(), &mut layer) };
    }

    /// Blend `hex` at `opa` over every pixel whose centre (in screen
    /// coordinates) satisfies `inside`. RGB565 canvases only.
    fn tint(&self, hex: u32, opa: u8, inside: impl Fn(f32, f32) -> bool) {
        debug_assert!(!self.argb, "tint works on the RGB565 preview");
        let rows = self.pixels.cast::<u8>();
        for y in 0..self.h {
            // SAFETY: y < h; the buffer holds h rows of `stride` bytes.
            let row = unsafe { rows.add(y as usize * self.stride).cast::<u16>() };
            for x in 0..self.w {
                let sx = (self.origin.0 + x) as f32 + 0.5;
                let sy = (self.origin.1 + y) as f32 + 0.5;
                if inside(sx, sy) {
                    // SAFETY: x < w, and a row holds at least w RGB565 pixels
                    // (stride ≥ 2w, even, so the u16 access is aligned).
                    unsafe {
                        let pixel = row.add(x as usize);
                        *pixel = blend565(*pixel, hex, opa);
                    }
                }
            }
        }
        // SAFETY: live canvas.
        unsafe { lv_obj_invalidate(self.obj.raw()) };
    }
}

/// Draws on a canvas layer in screen coordinates.
struct Pen<'a> {
    layer: &'a mut lv_layer_t,
    origin: (i32, i32),
}

impl Pen<'_> {
    fn point(&self, (x, y): (f32, f32)) -> lv_point_precise_t {
        lv_point_precise_t {
            x: x.round() as i32 - self.origin.0,
            y: y.round() as i32 - self.origin.1,
        }
    }

    fn line(&mut self, from: (f32, f32), to: (f32, f32), width: i32, hex: u32) {
        let mut dsc = lv_draw_line_dsc_t::default();
        // SAFETY: valid descriptor.
        unsafe { lv_draw_line_dsc_init(&mut dsc) };
        dsc.p1 = self.point(from);
        dsc.p2 = self.point(to);
        dsc.width = width;
        dsc.color = color(hex);
        dsc.opa = LV_OPA_COVER as lv_opa_t;
        dsc.set_round_start(1);
        dsc.set_round_end(1);
        // SAFETY: the layer is initialised and alive for this Pen.
        unsafe { lv_draw_line(self.layer, &dsc) };
    }

    /// A closed outline through `points`; round caps join the segments.
    fn outline(&mut self, points: &[(f32, f32)], width: i32, hex: u32) {
        for (&from, &to) in points.iter().zip(points.iter().cycle().skip(1)) {
            self.line(from, to, width, hex);
        }
    }

    fn circle(&mut self, centre: (f32, f32), outer_r: f32, width: i32, hex: u32) {
        let mut dsc = lv_draw_arc_dsc_t::default();
        // SAFETY: valid descriptor.
        unsafe { lv_draw_arc_dsc_init(&mut dsc) };
        let c = self.point(centre);
        dsc.center = lv_point_t { x: c.x, y: c.y };
        dsc.radius = outer_r.round() as u16;
        dsc.width = width;
        dsc.start_angle = 0;
        dsc.end_angle = 360;
        dsc.color = color(hex);
        dsc.opa = LV_OPA_COVER as lv_opa_t;
        // SAFETY: the layer is initialised and alive for this Pen.
        unsafe { lv_draw_arc(self.layer, &dsc) };
    }

    /// A ring whose stroke is centred on `r`.
    fn ring(&mut self, centre: (f32, f32), r: f32, width: i32, hex: u32) {
        self.circle(centre, r + width as f32 / 2.0, width, hex);
    }

    fn disc(&mut self, centre: (f32, f32), r: f32, hex: u32) {
        self.circle(centre, r, r.round() as i32, hex);
    }
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

#[cfg(test)]
mod picker_tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// The six glyphs from audio-mode.png (data-polar on each button).
    fn mockup_glyphs() -> [Glyph; 6] {
        let lobe = |a, r, rot| Lobe { a, r, rot };
        let solid = |lobes: Vec<Lobe>| Glyph { lobes, ghost: None };
        [
            solid(vec![lobe(1.0, 14.0, 0.0)]),
            Glyph {
                lobes: vec![lobe(0.5, 13.0, 40.0)],
                ghost: Some(lobe(0.5, 12.0, -50.0)),
            },
            solid(vec![lobe(0.5, 15.0, 0.0)]),
            solid(vec![lobe(0.366, 15.0, 0.0)]),
            solid(vec![lobe(0.0, 14.0, 0.0)]),
            solid(vec![lobe(0.5, 12.0, -45.0), lobe(0.5, 12.0, 45.0)]),
        ]
    }

    fn focused(aim: f32) -> Preview {
        Preview {
            lobes: vec![(0.5, aim)],
            handle: Some(aim),
        }
    }

    #[test]
    fn the_picker_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let picker = ModePicker::new(root, mockup_glyphs(), 2, |_| {}, |_| {});
            picker.set_preview(&focused(300.0));
            picker.set_aimable(true);
            let frame = lv.render();
            assert_golden("picker_focused", &frame);
            assert_targets_ok(root);
            // The handle sits on r 50 at the aim, the centre dot is Text.
            let bright = |[r, g, b]: [u8; 3]| r > 200 && g > 200 && b > 200;
            assert!(bright(frame.pixel(157, 109)), "handle");
            assert!(bright(frame.pixel(200, 134)), "centre dot");
            // Inside the lobe away from strokes: Live at 16 % over black.
            let [r, g, b] = frame.pixel(178, 107);
            assert!(g > r && g > 20 && g < 80 && b > r, "fill {r},{g},{b}");
            // Behind the cardioid (its null): untouched black.
            assert_eq!(frame.pixel(230, 152), [0, 0, 0]);
        });
    }

    #[test]
    fn buttons_sit_on_the_ring_and_report_taps() {
        lvgl(|lv| {
            let root = lv.screen();
            let picked = Rc::new(RefCell::new(Vec::new()));
            let log = picked.clone();
            let picker = ModePicker::new(
                root,
                mockup_glyphs(),
                0,
                move |i| log.borrow_mut().push(i),
                |_| {},
            );
            assert_eq!(button_rect(300.0), Rect::new(62, 44, 48, 48));
            assert_eq!(button_rect(90.0), Rect::new(308, 110, 48, 48));
            lv.click(picker.button(4));
            assert_eq!(*picked.borrow(), vec![4]);
            assert_eq!(picker.selected(), 4);
        });
    }

    #[test]
    fn drags_report_moving_then_done() {
        lvgl(|lv| {
            let root = lv.screen();
            let seen = Rc::new(RefCell::new(Vec::new()));
            let log = seen.clone();
            let picker = ModePicker::new(
                root,
                mockup_glyphs(),
                2,
                |_| {},
                move |e| log.borrow_mut().push(e),
            );
            picker.set_aimable(true);
            picker.drag_to(250, 134, false);
            picker.drag_to(200, 184, true);
            assert_eq!(
                *seen.borrow(),
                vec![AimEvent::Moving(90.0), AimEvent::Done(180.0)]
            );
            // Not aimable: drags are ignored.
            picker.set_aimable(false);
            picker.drag_to(250, 134, true);
            assert_eq!(seen.borrow().len(), 2);
        });
    }

    #[test]
    fn a_release_in_the_dead_zone_keeps_the_last_aim() {
        lvgl(|lv| {
            let root = lv.screen();
            let seen = Rc::new(RefCell::new(Vec::new()));
            let log = seen.clone();
            let picker = ModePicker::new(
                root,
                mockup_glyphs(),
                2,
                |_| {},
                move |e| log.borrow_mut().push(e),
            );
            picker.set_aimable(true);
            picker.drag_to(250, 134, false);
            picker.drag_to(201, 135, true); // released on the centre dot
            assert_eq!(
                *seen.borrow(),
                vec![AimEvent::Moving(90.0), AimEvent::Done(90.0)]
            );
            // A press that never left the centre reports nothing.
            picker.drag_to(200, 134, true);
            assert_eq!(seen.borrow().len(), 2);
        });
    }

    #[test]
    fn a_lost_press_finishes_the_drag_like_a_release() {
        lvgl(|lv| {
            let root = lv.screen();
            let seen = Rc::new(RefCell::new(Vec::new()));
            let log = seen.clone();
            let picker = ModePicker::new(
                root,
                mockup_glyphs(),
                2,
                |_| {},
                move |e| log.borrow_mut().push(e),
            );
            picker.set_aimable(true);
            picker.drag_to(250, 134, false);
            lv.send(picker.aim_area(), lv_event_code_t_LV_EVENT_PRESS_LOST);
            assert_eq!(
                *seen.borrow(),
                vec![AimEvent::Moving(90.0), AimEvent::Done(90.0)]
            );
            // The drag is over: a later release on the centre reports nothing.
            picker.drag_to(200, 134, true);
            assert_eq!(seen.borrow().len(), 2);
        });
    }

    #[test]
    fn a_new_press_forgets_the_last_aim() {
        lvgl(|lv| {
            let root = lv.screen();
            let seen = Rc::new(RefCell::new(Vec::new()));
            let log = seen.clone();
            let picker = ModePicker::new(
                root,
                mockup_glyphs(),
                2,
                |_| {},
                move |e| log.borrow_mut().push(e),
            );
            picker.set_aimable(true);
            picker.drag_to(250, 134, false); // a drag whose release never arrived
            lv.send(picker.aim_area(), lv_event_code_t_LV_EVENT_PRESSED);
            picker.drag_to(200, 134, true); // a tap on the centre dot
            assert_eq!(*seen.borrow(), vec![AimEvent::Moving(90.0)]);
        });
    }

    #[test]
    fn the_aim_area_does_not_bubble_gestures() {
        lvgl(|lv| {
            let root = lv.screen();
            let picker = ModePicker::new(root, mockup_glyphs(), 2, |_| {}, |_| {});
            let area = picker.aim_area();
            assert!(!area.has_flag(lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE));
            assert!(area.has_flag(lv_obj_flag_t_LV_OBJ_FLAG_PRESS_LOCK));
        });
    }

    #[test]
    fn canvas_buffers_follow_lvgls_stride() {
        lvgl(|lv| {
            let root = lv.screen();
            let picker = ModePicker::new(root, mockup_glyphs(), 2, |_| {}, |_| {});
            let inner = &picker.inner;
            for canvas in std::iter::once(&inner.preview).chain(&inner.glyphs) {
                // SAFETY: live canvas with a buffer set.
                let header = unsafe { (*lv_canvas_get_draw_buf(canvas.obj.raw())).header };
                assert_eq!(canvas.stride, header.stride() as usize);
                assert!(canvas.stride >= canvas.w as usize * if canvas.argb { 4 } else { 2 });
            }
        });
    }

    #[test]
    fn an_unchanged_preview_is_not_redrawn() {
        lvgl(|lv| {
            let root = lv.screen();
            let picker = ModePicker::new(root, mockup_glyphs(), 2, |_| {}, |_| {});
            picker.set_preview(&focused(300.0));
            assert_eq!(picker.inner.paints.get(), 1);
            picker.set_preview(&focused(300.0));
            assert_eq!(picker.inner.paints.get(), 1);
            picker.set_preview(&focused(301.0));
            assert_eq!(picker.inner.paints.get(), 2);
        });
    }
}
