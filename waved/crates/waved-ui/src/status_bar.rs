//! Persistent status bar on lv_layer_top() — survives screen changes.
//!
//! Shows: battery %, WiFi signal, BT connected, mic muted, time.
//! Height: 32px. Width: 400px.

use crate::bindings::*;
use crate::theme::*;

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
}

pub struct StatusBar {
    root: *mut lv_obj_t,
    lbl_battery: *mut lv_obj_t,
    lbl_wifi: *mut lv_obj_t,
    lbl_bt: *mut lv_obj_t,
    lbl_mic: *mut lv_obj_t,
    lbl_time: *mut lv_obj_t,
}

impl StatusBar {
    /// Create the status bar on the top LVGL layer. Call once after lv_init().
    pub unsafe fn create() -> Self {
        let top_layer = lv_layer_top();

        let root = lv_obj_create(top_layer);
        lv_obj_set_size(root, 400, 32);
        lv_obj_set_pos(root, 0, 0);
        lv_obj_set_style_bg_color(root, rgb(CLR_BG), 0);
        lv_obj_set_style_bg_opa(root, 220, 0);
        lv_obj_set_style_border_width(root, 0, 0);
        lv_obj_set_style_radius(root, 0, 0);
        lv_obj_set_style_pad_top(root, 0, 0);
        lv_obj_set_style_pad_bottom(root, 0, 0);
        lv_obj_set_style_pad_left(root, 0, 0);
        lv_obj_set_style_pad_right(root, 0, 0);

        let mk_label = |parent: *mut lv_obj_t, x: i32, align: lv_align_t| {
            let lbl = lv_label_create(parent);
            lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT), 0);
            lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl, align, x, 0);
            lbl
        };

        let lbl_battery = mk_label(root, 4,   lv_align_t_LV_ALIGN_LEFT_MID);
        let lbl_wifi    = mk_label(root, 50,  lv_align_t_LV_ALIGN_LEFT_MID);
        let lbl_bt      = mk_label(root, 80,  lv_align_t_LV_ALIGN_LEFT_MID);
        let lbl_mic     = mk_label(root, 110, lv_align_t_LV_ALIGN_LEFT_MID);
        let lbl_time    = mk_label(root, -4,  lv_align_t_LV_ALIGN_RIGHT_MID);

        Self { root, lbl_battery, lbl_wifi, lbl_bt, lbl_mic, lbl_time }
    }

    /// Update labels from current state. Call each LVGL tick.
    pub unsafe fn update(
        &self,
        battery_pct: u8,
        wifi_connected: bool,
        bt_connected: bool,
        mic_muted: bool,
    ) {
        use std::ffi::CString;

        let batt_str = CString::new(format!("{battery_pct}%")).unwrap();
        lv_label_set_text(self.lbl_battery, batt_str.as_ptr());

        let wifi_str = CString::new(if wifi_connected { "W" } else { "--" }).unwrap();
        lv_label_set_text(self.lbl_wifi, wifi_str.as_ptr());

        let bt_str = CString::new(if bt_connected { "BT" } else { "" }).unwrap();
        lv_label_set_text(self.lbl_bt, bt_str.as_ptr());

        let mic_str = CString::new(if mic_muted { "M" } else { "" }).unwrap();
        lv_label_set_text(self.lbl_mic, mic_str.as_ptr());

        // Wall clock from system time
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let h = (now / 3600) % 24;
        let m = (now / 60) % 60;
        let time_str = CString::new(format!("{h:02}:{m:02}")).unwrap();
        lv_label_set_text(self.lbl_time, time_str.as_ptr());
    }
}
