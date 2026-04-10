//! Persistent status bar on lv_layer_top() — survives screen changes.
//!
//! Uses the original pspapp 20×20 PNG icons for battery, wifi, bluetooth.
//! Height: 32px. Width: 400px.

use crate::bindings::*;
use crate::theme::*;

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
}

// ── Icon path helpers ─────────────────────────────────────────────────────────

fn battery_icon(pct: u8) -> &'static [u8] {
    match pct {
        0..=5   => b"A:/etc/pspapp/lvgl/icon/status/battery_0_5.png\0",
        6..=10  => b"A:/etc/pspapp/lvgl/icon/status/battery_6_10.png\0",
        11..=20 => b"A:/etc/pspapp/lvgl/icon/status/battery_11_20.png\0",
        21..=30 => b"A:/etc/pspapp/lvgl/icon/status/battery_21_30.png\0",
        31..=40 => b"A:/etc/pspapp/lvgl/icon/status/battery_31_40.png\0",
        41..=50 => b"A:/etc/pspapp/lvgl/icon/status/battery_41_50.png\0",
        51..=60 => b"A:/etc/pspapp/lvgl/icon/status/battery_51_60.png\0",
        61..=70 => b"A:/etc/pspapp/lvgl/icon/status/battery_61_70.png\0",
        71..=80 => b"A:/etc/pspapp/lvgl/icon/status/battery_71_80.png\0",
        81..=90 => b"A:/etc/pspapp/lvgl/icon/status/battery_81_90.png\0",
        _       => b"A:/etc/pspapp/lvgl/icon/status/battery_91_100.png\0",
    }
}

fn wifi_icon(connected: bool) -> &'static [u8] {
    if connected {
        b"A:/etc/pspapp/lvgl/icon/status/wifi_signal_50_100.png\0"
    } else {
        b"A:/etc/pspapp/lvgl/icon/status/wifi_unconnected.png\0"
    }
}

fn bt_icon(connected: bool) -> &'static [u8] {
    if connected {
        b"A:/etc/pspapp/lvgl/icon/status/bluetooth_green.png\0"
    } else {
        b"A:/etc/pspapp/lvgl/icon/status/bluetooth.png\0"
    }
}

// ── Struct ────────────────────────────────────────────────────────────────────

pub struct StatusBar {
    img_battery: *mut lv_obj_t,
    img_wifi:    *mut lv_obj_t,
    img_bt:      *mut lv_obj_t,
    lbl_mic:     *mut lv_obj_t,   // small text "M" when muted
    lbl_time:    *mut lv_obj_t,
}

impl StatusBar {
    pub unsafe fn create() -> Self {
        let top_layer = lv_layer_top();

        let root = lv_obj_create(top_layer);
        lv_obj_set_size(root, 400, 32);
        lv_obj_set_pos(root, 0, 0);
        lv_obj_set_style_bg_color(root, rgb(CLR_BG), 0);
        lv_obj_set_style_bg_opa(root, 200, 0);
        lv_obj_set_style_border_width(root, 0, 0);
        lv_obj_set_style_radius(root, 0, 0);
        lv_obj_set_style_pad_top(root, 0, 0);
        lv_obj_set_style_pad_bottom(root, 0, 0);
        lv_obj_set_style_pad_left(root, 0, 0);
        lv_obj_set_style_pad_right(root, 0, 0);

        // Icons and labels clustered around the screen centre (x=200).
        // At the top of the circular display the visible width is ~270 px, so
        // keeping everything within ±60 px of centre (x=140..260) is safe.
        //
        // Layout (left → right, centred at x=200):
        //   battery  wifi  bt  muted  time
        //   -46      -22   +2  +26    +52
        let mk_icon = |x_off: i32, initial_src: &[u8]| {
            let img = lv_image_create(root);
            lv_image_set_src(img, initial_src.as_ptr() as *const _);
            lv_obj_align(img, lv_align_t_LV_ALIGN_CENTER, x_off, 0);
            lv_obj_set_style_border_width(img, 0, 0);
            img
        };

        // Initialize at 0%; will update to real level once MCU sends battery data.
        let img_battery = mk_icon(-46, battery_icon(0));
        let img_wifi    = mk_icon(-22, wifi_icon(false));
        let img_bt      = mk_icon(  2, bt_icon(false));

        // Small "M" label when mic is muted (hidden by default)
        let lbl_mic = lv_label_create(root);
        lv_label_set_text(lbl_mic, c"".as_ptr());
        lv_obj_set_style_text_color(lbl_mic, rgb(0xff4444), 0);
        lv_obj_set_style_text_font(lbl_mic, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_mic, lv_align_t_LV_ALIGN_CENTER, 26, 0);

        // Time — centred cluster, just right of the muted indicator
        let lbl_time = lv_label_create(root);
        lv_label_set_text(lbl_time, c"00:00".as_ptr());
        lv_obj_set_style_text_color(lbl_time, rgb(CLR_TEXT), 0);
        lv_obj_set_style_text_font(lbl_time, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_time, lv_align_t_LV_ALIGN_CENTER, 52, 0);

        Self { img_battery, img_wifi, img_bt, lbl_mic, lbl_time }
    }

    pub unsafe fn update(
        &self,
        battery_pct: u8,
        wifi_connected: bool,
        bt_connected: bool,
        mic_muted: bool,
    ) {
        use std::ffi::CString;

        lv_image_set_src(self.img_battery, battery_icon(battery_pct).as_ptr() as *const _);
        lv_image_set_src(self.img_wifi,    wifi_icon(wifi_connected).as_ptr() as *const _);
        lv_image_set_src(self.img_bt,      bt_icon(bt_connected).as_ptr() as *const _);

        let mic_text = if mic_muted { c"M".as_ptr() } else { c"".as_ptr() };
        lv_label_set_text(self.lbl_mic, mic_text);

        // Wall clock
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let h = (now / 3600) % 24;
        let m = (now / 60) % 60;
        let time_s = CString::new(format!("{h:02}:{m:02}")).unwrap();
        lv_label_set_text(self.lbl_time, time_s.as_ptr());
    }
}
