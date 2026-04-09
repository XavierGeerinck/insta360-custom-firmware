//! Settings menu screen — navigates to sub-screens.

use std::ffi::CString;
use crate::bindings::*;
use crate::theme::*;

extern "C" {
    static lv_font_montserrat_16: lv_font_t;
}

pub struct SettingsScreen;

impl SettingsScreen {
    pub fn new() -> Self { Self }
}

impl super::Screen for SettingsScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            let screen = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_top(screen, 0, 0);
            lv_obj_set_style_pad_bottom(screen, 0, 0);
            lv_obj_set_style_pad_left(screen, 0, 0);
            lv_obj_set_style_pad_right(screen, 0, 0);

            let title = lv_label_create(screen);
            lv_label_set_text(title, c"Settings".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_TEXT), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_16, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 38);

            // Settings list
            let list = lv_list_create(screen);
            lv_obj_set_size(list, 380, 200);
            lv_obj_align(list, lv_align_t_LV_ALIGN_TOP_MID, 0, 62);
            lv_obj_set_style_bg_color(list, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(list, 0, 0);

            let items = [
                "Audio Mode",
                "WiFi",
                "Bluetooth",
                "Volume",
                "Mic Arm Height",
                "About",
                "Power Off",
            ];
            for item in &items {
                let s = CString::new(*item).unwrap();
                let btn = lv_list_add_button(list, std::ptr::null(), s.as_ptr());
                lv_obj_set_style_bg_color(btn, rgb(CLR_SURFACE), 0);
            }

            screen
        }
    }
}
