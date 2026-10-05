//! Headless LVGL for unit tests. Renders 400×304 frames on the host and
//! compares them with committed golden PNGs in `crates/waved-ui/golden/`.
//!
//! LVGL is a process-wide singleton: every test goes through [`lvgl`], which
//! initializes it once and serializes access with a mutex.

use crate::bindings::*;
use crate::kit::event::{resume_handler_panic, send_swipe, Swipe};
use crate::kit::obj::Obj;
use crate::kit::tokens::{
    Rect, MIN_TARGET, PANEL_CX, PANEL_CY, PANEL_R, SCREEN_H, SCREEN_W, TAP_LIMIT_R, TAP_MIN_Y,
};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

struct Headless {
    disp: *mut lv_display_t,
    buf: *mut u16,
    screen: *mut lv_obj_t,
}

// SAFETY: only used while holding the mutex, by one test at a time.
unsafe impl Send for Headless {}

static LVGL: OnceLock<Mutex<Headless>> = OnceLock::new();

unsafe extern "C" fn flush_cb(disp: *mut lv_display_t, _area: *const lv_area_t, _px: *mut u8) {
    // FULL render mode: the draw buffer already holds the whole frame.
    lv_display_flush_ready(disp);
}

fn init() -> Mutex<Headless> {
    let pixels = (SCREEN_W * SCREEN_H) as usize;
    // u32 storage keeps the 4-byte alignment LVGL expects for draw buffers.
    let storage: &'static mut [u32] = Box::leak(vec![0u32; pixels / 2].into_boxed_slice());
    // SAFETY: LVGL is initialized exactly once, here, before any other call.
    unsafe {
        lv_init();
        let disp = lv_display_create(SCREEN_W, SCREEN_H);
        lv_display_set_color_format(disp, lv_color_format_t_LV_COLOR_FORMAT_RGB565);
        lv_display_set_buffers(
            disp,
            storage.as_mut_ptr().cast(),
            std::ptr::null_mut(),
            (pixels * 2) as u32,
            lv_display_render_mode_t_LV_DISPLAY_RENDER_MODE_FULL,
        );
        lv_display_set_flush_cb(disp, Some(flush_cb));
        Mutex::new(Headless {
            disp,
            buf: storage.as_mut_ptr().cast(),
            screen: std::ptr::null_mut(),
        })
    }
}

/// Exclusive access to the headless LVGL instance for one test.
pub struct Lvgl {
    inner: MutexGuard<'static, Headless>,
}

/// Run `test` with exclusive access to LVGL.
pub fn lvgl<R>(test: impl FnOnce(&mut Lvgl) -> R) -> R {
    let inner = LVGL
        .get_or_init(init)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test(&mut Lvgl { inner })
}

impl Lvgl {
    /// A fresh black screen, loaded and active. Deletes the previous test screen.
    pub fn screen(&mut self) -> Obj {
        let root = Obj::screen();
        // SAFETY: both screens are valid; the old one is no longer active.
        unsafe {
            lv_screen_load(root.raw());
            if !self.inner.screen.is_null() && lv_obj_is_valid(self.inner.screen) {
                lv_obj_delete(self.inner.screen);
            }
        }
        self.inner.screen = root.raw();
        root
    }

    /// Render the active screen.
    pub fn render(&mut self) -> Frame {
        // SAFETY: the buffer holds SCREEN_W * SCREEN_H RGB565 pixels.
        unsafe {
            lv_refr_now(self.inner.disp);
        }
        resume_handler_panic();
        // SAFETY: the buffer holds SCREEN_W * SCREEN_H RGB565 pixels.
        unsafe {
            let px = std::slice::from_raw_parts(self.inner.buf, (SCREEN_W * SCREEN_H) as usize);
            Frame::from_rgb565(px)
        }
    }

    /// Fire the events a finger tap produces.
    pub fn click(&mut self, obj: Obj) {
        for code in [
            lv_event_code_t_LV_EVENT_PRESSED,
            lv_event_code_t_LV_EVENT_RELEASED,
            lv_event_code_t_LV_EVENT_CLICKED,
        ] {
            self.send(obj, code);
        }
    }

    pub fn send(&mut self, obj: Obj, code: lv_event_code_t) {
        // SAFETY: obj belongs to the active test screen.
        unsafe {
            lv_obj_send_event(obj.raw(), code, std::ptr::null_mut());
        }
        resume_handler_panic();
    }

    /// Deliver a swipe in `direction` to `obj`'s swipe handlers, as the
    /// runtime does once it recognises one (`event::send_swipe`).
    pub fn swipe(&mut self, obj: Obj, direction: Swipe) {
        send_swipe(obj, direction);
        resume_handler_panic();
    }

    /// Advance LVGL's clock and run due timers.
    pub fn advance(&mut self, ms: u32) {
        // SAFETY: plain LVGL calls under the test lock.
        unsafe {
            lv_tick_inc(ms);
            lv_timer_handler();
        }
        resume_handler_panic();
    }
}

/// A rendered frame as RGB888.
pub struct Frame {
    rgb: Vec<u8>,
}

impl Frame {
    fn from_rgb565(px: &[u16]) -> Self {
        let mut rgb = Vec::with_capacity(px.len() * 3);
        for &value in px {
            let r = u32::from((value >> 11) & 0x1f);
            let g = u32::from((value >> 5) & 0x3f);
            let b = u32::from(value & 0x1f);
            rgb.extend_from_slice(&[
                (r * 255 / 31) as u8,
                (g * 255 / 63) as u8,
                (b * 255 / 31) as u8,
            ]);
        }
        Self { rgb }
    }

    pub fn pixel(&self, x: i32, y: i32) -> [u8; 3] {
        let i = ((y * SCREEN_W + x) * 3) as usize;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }

    pub fn to_png(&self) -> Vec<u8> {
        encode(&self.rgb)
    }

    /// The frame with everything outside the visible circle dimmed, for review.
    /// (Row 0 is the flat top of that circle, so nothing above it needs dimming.)
    fn masked(&self) -> Vec<u8> {
        let mut rgb = self.rgb.clone();
        for y in 0..SCREEN_H {
            for x in 0..SCREEN_W {
                if (x - PANEL_CX).pow(2) + (y - PANEL_CY).pow(2) > PANEL_R * PANEL_R {
                    let i = ((y * SCREEN_W + x) * 3) as usize;
                    for channel in &mut rgb[i..i + 3] {
                        *channel /= 4;
                    }
                }
            }
        }
        rgb
    }
}

fn encode(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, SCREEN_W as u32, SCREEN_H as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(rgb).expect("png data");
    }
    out
}

fn decode(bytes: &[u8]) -> Vec<u8> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("png info");
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("png frame");
    buf.truncate(info.buffer_size());
    buf
}

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Compare `frame` with `golden/<name>.png`.
///
/// `UPDATE_GOLDEN=1` writes the golden instead. `UI_PREVIEW=1` also writes a
/// panel-masked copy to `target/ui-preview/` for side-by-side review.
pub fn assert_golden(name: &str, frame: &Frame) {
    let path = crate_dir().join("golden").join(format!("{name}.png"));
    if std::env::var_os("UI_PREVIEW").is_some() {
        let dir = crate_dir().join("../../target/ui-preview");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.png")), encode(&frame.masked())).unwrap();
    }
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, frame.to_png()).unwrap();
        return;
    }
    let expected = std::fs::read(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden {}: run with UPDATE_GOLDEN=1, then compare it with docs/mockups/png",
            path.display()
        )
    });
    if decode(&expected) != frame.rgb {
        let dir = crate_dir().join("../../target/golden-diff");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.actual.png")), frame.to_png()).unwrap();
        panic!("{name} differs from its golden; see target/golden-diff/{name}.actual.png");
    }
}

/// Clickable objects under `root` whose drawn box (corners rounded by its
/// style radius) reaches past the tap limit or above `TAP_MIN_Y`, or that are
/// smaller than 44 px (counting any extended click area). Hidden subtrees are skipped.
pub fn target_problems(root: Obj) -> Vec<String> {
    root.update_layout();
    let mut problems = Vec::new();
    visit(root, &mut |obj| {
        if obj == root || obj.is_hidden() || !obj.is_clickable() {
            return;
        }
        let a = obj.coords();
        let visible = Rect::new(a.x1, a.y1, a.x2 - a.x1 + 1, a.y2 - a.y1 + 1);
        // SAFETY: live object under the test lock.
        let radius = unsafe {
            lv_obj_get_style_prop(obj.raw(), LV_PART_MAIN, LV_STYLE_RADIUS as lv_style_prop_t).num
        };
        if visible.y < TAP_MIN_Y {
            problems.push(format!("{visible:?} reaches above y {TAP_MIN_Y}"));
        } else if !visible.within_tap_limit_rounded(radius) {
            problems.push(format!(
                "{visible:?} (radius {radius}) reaches {:.1} px, past the r{TAP_LIMIT_R} tap limit",
                visible.farthest(radius)
            ));
        }
        let c = obj.click_area();
        let (w, h) = (c.x2 - c.x1 + 1, c.y2 - c.y1 + 1);
        if w < MIN_TARGET || h < MIN_TARGET {
            problems.push(format!(
                "{visible:?} is a {w}x{h} target, below {MIN_TARGET} px"
            ));
        }
    });
    problems
}

pub fn assert_targets_ok(root: Obj) {
    let problems = target_problems(root);
    assert!(
        problems.is_empty(),
        "tap target problems:\n{}",
        problems.join("\n")
    );
}

fn visit(obj: Obj, f: &mut dyn FnMut(Obj)) {
    f(obj);
    if obj.is_hidden() {
        return;
    }
    for i in 0..obj.child_count() {
        visit(obj.child(i as i32), f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::tokens::Rect;

    #[test]
    fn renders_a_black_screen() {
        lvgl(|lv| {
            lv.screen();
            assert_eq!(lv.render().pixel(200, 152), [0, 0, 0]);
        });
    }

    #[test]
    fn renders_a_coloured_box() {
        lvgl(|lv| {
            let root = lv.screen();
            Obj::plain(root)
                .rect(Rect::new(10, 20, 30, 40))
                .bg(0xff0000);
            let frame = lv.render();
            assert_eq!(frame.pixel(25, 40), [255, 0, 0]);
            assert_eq!(frame.pixel(5, 5), [0, 0, 0]);
        });
    }

    #[test]
    fn unstyled_theme_objects_render_dark() {
        lvgl(|lv| {
            let root = lv.screen();
            // SAFETY: a raw themed object, deliberately without kit styling.
            unsafe {
                let card = lv_obj_create(root.raw());
                lv_obj_set_pos(card, 100, 100);
                lv_obj_set_size(card, 100, 60);
            }
            let [r, g, b] = lv.render().pixel(150, 130);
            assert!(
                r < 100 && g < 100 && b < 100,
                "theme card is dark, got {r},{g},{b}"
            );
        });
    }

    #[test]
    fn target_check_flags_far_and_small_targets() {
        lvgl(|lv| {
            let root = lv.screen();
            // Past r 190: the revision 1 corner button and milestone 1's Back.
            Obj::plain(root)
                .rect(Rect::new(8, 8, 56, 64))
                .clickable(true);
            Obj::plain(root)
                .rect(Rect::new(144, 252, 112, 44))
                .radius(22)
                .clickable(true);
            // Above the first tappable row.
            Obj::plain(root)
                .rect(Rect::new(150, 2, 100, 44))
                .clickable(true);
            // Too small.
            Obj::plain(root)
                .rect(Rect::new(180, 140, 30, 30))
                .clickable(true);
            // Fine: a central box, and the third radio row whose box corner is
            // past the limit but whose rounded corner is not.
            Obj::plain(root)
                .rect(Rect::new(150, 120, 100, 60))
                .clickable(true);
            Obj::plain(root)
                .rect(Rect::new(52, 180, 296, 52))
                .radius(20)
                .clickable(true);
            let problems = target_problems(root);
            assert_eq!(problems.len(), 4, "{problems:#?}");
            assert!(problems[2].contains("above y 6"), "{problems:#?}");
        });
    }

    #[test]
    fn the_review_mask_dims_outside_the_measured_circle() {
        lvgl(|lv| {
            let root = lv.screen();
            Obj::plain(root)
                .rect(Rect::new(0, 0, SCREEN_W, SCREEN_H))
                .bg(0xffffff);
            let masked = lv.render().masked();
            let at = |x: i32, y: i32| masked[((y * SCREEN_W + x) * 3) as usize];
            // The flat top edge is visible from x 33 to 367.
            assert_eq!((at(200, 0), at(40, 0), at(360, 0)), (255, 255, 255));
            assert_eq!((at(20, 0), at(380, 0)), (63, 63));
            // r 198 around (200, 106).
            assert_eq!((at(2, 106), at(1, 106)), (255, 63));
            assert_eq!((at(200, 303), at(120, 290)), (255, 63));
        });
    }

    #[test]
    fn png_round_trips() {
        lvgl(|lv| {
            let root = lv.screen();
            Obj::plain(root).rect(Rect::new(0, 0, 50, 50)).bg(0x3fd6b1);
            let frame = lv.render();
            assert_eq!(decode(&frame.to_png()), frame.rgb);
        });
    }
}
