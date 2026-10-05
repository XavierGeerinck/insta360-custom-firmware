//! LVGL display and pointer registration for the Wave panel. With
//! `runtime/gesture.rs` and `runtime::load`, the only runtime code that calls
//! LVGL directly.

use crate::bindings::*;
use crate::display::{self, FramebufferDisplay};
use crate::kit::obj::Obj;
use crate::runtime::touch::TouchSample;
use std::cell::Cell;

pub struct Device {
    pointer: Pointer,
}

impl Device {
    /// `lv_init`, the framebuffer display and the touch pointer. Call once,
    /// on the UI thread.
    pub fn init(fb: FramebufferDisplay) -> Self {
        // SAFETY: called once before any other LVGL call on this thread; the
        // framebuffer is leaked, so LVGL's user-data pointer stays valid for
        // the life of the process.
        unsafe {
            lv_init();
            let disp = lv_display_create(display::SCREEN_W as i32, display::SCREEN_H as i32);
            lv_display_set_color_format(disp, lv_color_format_t_LV_COLOR_FORMAT_RGB565);
            lv_display_set_flush_cb(disp, Some(flush_cb));
            // The buffers are heap allocations owned by `fb`; moving the box
            // below does not move them.
            let buf1 = fb.buf1.as_ptr() as *mut _;
            let buf2 = fb.buf2.as_ptr() as *mut _;
            let len = fb.buf1.len() as u32;
            lv_display_set_user_data(disp, Box::into_raw(Box::new(fb)) as *mut _);
            lv_display_set_buffers(
                disp,
                buf1,
                buf2,
                len,
                lv_display_render_mode_t_LV_DISPLAY_RENDER_MODE_PARTIAL,
            );
        }
        let pointer = Pointer::create();
        release_top_layer();
        Self { pointer }
    }

    /// What LVGL reads at its next pointer poll.
    pub fn report(&self, sample: TouchSample) {
        self.pointer.report(sample);
    }

    /// A gesture took this press, or the screen changed under it: LVGL
    /// ignores the rest of the press (PRESS_LOST, no click).
    pub fn cancel_press(&self) {
        self.pointer.cancel_press();
    }

    /// The loaded screen (null before the first one).
    pub fn active_screen(&self) -> Obj {
        // SAFETY: LVGL returns the live active screen or null.
        unsafe { Obj::from_raw(lv_screen_active()) }
    }

    pub fn tick(&self, ms: u32) {
        // SAFETY: plain LVGL call on the UI thread.
        unsafe { lv_tick_inc(ms) };
    }

    pub fn run_timers(&self) {
        // SAFETY: plain LVGL call on the UI thread; handlers only push to channels.
        unsafe { lv_timer_handler() };
    }
}

/// LVGL's pointer input device, fed the last reported touch sample.
pub(crate) struct Pointer {
    indev: *mut lv_indev_t,
    touch: &'static Cell<TouchSample>,
}

impl Pointer {
    /// Register a pointer with the default display. Call on the UI thread
    /// once a display exists.
    pub(crate) fn create() -> Self {
        // Leaked, so LVGL's user-data pointer stays valid as long as the indev.
        let touch: &'static Cell<TouchSample> =
            Box::leak(Box::new(Cell::new(TouchSample::default())));
        // SAFETY: LVGL is initialised with a default display (caller's
        // contract); the read callback only reads the leaked cell.
        unsafe {
            let indev = lv_indev_create();
            lv_indev_set_type(indev, lv_indev_type_t_LV_INDEV_TYPE_POINTER);
            lv_indev_set_read_cb(indev, Some(touch_read_cb));
            lv_indev_set_user_data(indev, touch as *const Cell<TouchSample> as *mut _);
            Self { indev, touch }
        }
    }

    pub(crate) fn report(&self, sample: TouchSample) {
        self.touch.set(sample);
    }

    pub(crate) fn cancel_press(&self) {
        // SAFETY: the pointer device created in `create`.
        unsafe { lv_indev_wait_release(self.indev) };
    }

    /// Read and process the pointer now, as LVGL's read timer would (tests
    /// run no timers).
    #[cfg(test)]
    pub(crate) fn read(&self) {
        // SAFETY: the pointer device created in `create`.
        unsafe { lv_indev_read(self.indev) };
    }
}

/// Tests create a pointer each: remove it so no later test sees it (the
/// touch cell, a few bytes, stays leaked). The device's pointer lives as
/// long as the process.
#[cfg(test)]
impl Drop for Pointer {
    fn drop(&mut self) {
        // SAFETY: the pointer device created in `create`, deleted once.
        unsafe { lv_indev_delete(self.indev) };
    }
}

/// `lv_layer_top()` must never catch taps, or nothing below it is reachable
/// (LVGL 9.2 already clears the flag; this keeps it cleared).
pub(crate) fn release_top_layer() {
    // SAFETY: the default display's top layer is live once a display exists.
    unsafe { lv_obj_remove_flag(lv_layer_top(), lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE) };
}

/// LVGL flush callback: writes the dirty area to the framebuffer.
///
/// # Safety
/// Only LVGL calls this, with the display created in `Device::init` (user
/// data null or a leaked `FramebufferDisplay`), a valid `area`, and `px_map`
/// pointing at the area's RGB565 pixels.
unsafe extern "C" fn flush_cb(disp: *mut lv_display_t, area: *const lv_area_t, px_map: *mut u8) {
    // SAFETY: per the function's contract.
    unsafe {
        let fb = lv_display_get_user_data(disp) as *mut FramebufferDisplay;
        if !fb.is_null() {
            (*fb).flush(&*area, px_map as *const u8);
        }
        lv_display_flush_ready(disp);
    }
}

/// LVGL pointer read callback: hands LVGL the latest reported sample.
///
/// # Safety
/// Only LVGL calls this, with the indev created in `Device::init` (user data
/// null or the leaked `Cell<TouchSample>`) and a valid `data`.
unsafe extern "C" fn touch_read_cb(indev: *mut lv_indev_t, data: *mut lv_indev_data_t) {
    // SAFETY: per the function's contract.
    unsafe {
        let touch = lv_indev_get_user_data(indev) as *const Cell<TouchSample>;
        let sample = if touch.is_null() {
            TouchSample::default()
        } else {
            (*touch).get()
        };
        (*data).point.x = sample.x;
        (*data).point.y = sample.y;
        (*data).state = if sample.pressed {
            lv_indev_state_t_LV_INDEV_STATE_PRESSED
        } else {
            lv_indev_state_t_LV_INDEV_STATE_RELEASED
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::lvgl;

    #[test]
    fn the_top_layer_never_takes_taps() {
        lvgl(|_lv| {
            // SAFETY: the headless display's top layer.
            let top = unsafe { Obj::from_raw(lv_layer_top()) };
            top.clickable(true);
            release_top_layer();
            assert!(!top.is_clickable(), "a clickable top layer swallows every tap (BUG-3)");
        });
    }

    #[test]
    fn the_default_font_is_the_kit_body_font() {
        lvgl(|_lv| {
            // SAFETY: returns a static font descriptor.
            let default = unsafe { lv_font_default() };
            assert_eq!(default, crate::kit::fonts::Font::Body.ptr(), "no Montserrat left");
        });
    }
}
