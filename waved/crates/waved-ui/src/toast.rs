//! Timed notification overlay. Shows a message for `duration_ms`, then hides.

use crate::bindings::*;
use crate::theme::*;
use std::ffi::CString;

pub struct Toast {
    obj: *mut lv_obj_t,
    lbl: *mut lv_obj_t,
    hide_at_ms: Option<u64>,
}

impl Toast {
    pub unsafe fn create() -> Self {
        let top = lv_layer_top();
        let obj = lv_obj_create(top);
        lv_obj_set_size(obj, 200, 36);
        lv_obj_align(obj, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -40);
        lv_obj_set_style_bg_color(obj, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(obj, 18, 0);
        lv_obj_set_style_border_width(obj, 0, 0);
        lv_obj_add_flag(obj, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);

        let lbl = lv_label_create(obj);
        lv_obj_set_style_text_color(lbl, rgb(CLR_ACCENT), 0);
        lv_label_set_text(lbl, c"".as_ptr());
        lv_obj_center(lbl);

        Self {
            obj,
            lbl,
            hide_at_ms: None,
        }
    }

    pub unsafe fn show(&mut self, msg: &str, duration_ms: u64) {
        let s = CString::new(msg).unwrap_or_else(|_| CString::new("?").unwrap());
        lv_label_set_text(self.lbl, s.as_ptr());
        lv_obj_remove_flag(self.obj, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.hide_at_ms = Some(now + duration_ms);
    }

    pub unsafe fn tick(&mut self) {
        if let Some(hide_at) = self.hide_at_ms {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if now >= hide_at {
                lv_obj_add_flag(self.obj, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                self.hide_at_ms = None;
            }
        }
    }
}
