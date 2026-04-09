//! Control Center overlay — slides down from top over active screen.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{NrLevel, StateBus};

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_14: lv_font_t;
}

const NR_LEVELS: [NrLevel; 4] = [NrLevel::Off, NrLevel::Low, NrLevel::Med, NrLevel::High];

pub struct ControlCenter {
    _bus: Arc<StateBus>,
    panel: *mut lv_obj_t,
    visible: bool,
    lbl_wifi: *mut lv_obj_t,
    nr_btns: [*mut lv_obj_t; 4],
    active_nr: NrLevel,
}

impl ControlCenter {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            _bus: bus,
            panel: std::ptr::null_mut(),
            visible: false,
            lbl_wifi: std::ptr::null_mut(),
            nr_btns: [std::ptr::null_mut(); 4],
            active_nr: NrLevel::Off,
        }
    }

    /// Create the overlay panel on lv_layer_top(). Starts hidden (y = -240).
    pub unsafe fn init(&mut self) {
        let top = lv_layer_top();
        let panel = lv_obj_create(top);
        lv_obj_set_size(panel, 400, 240);
        lv_obj_set_pos(panel, 0, -240);
        lv_obj_set_style_bg_color(panel, rgb(0x050f0d), 0);
        lv_obj_set_style_bg_opa(panel, 245, 0);
        lv_obj_set_style_border_width(panel, 0, 0);
        lv_obj_set_style_radius(panel, 0, 0);
        lv_obj_set_style_pad_top(panel, 12, 0);
        lv_obj_set_style_pad_bottom(panel, 12, 0);
        lv_obj_set_style_pad_left(panel, 12, 0);
        lv_obj_set_style_pad_right(panel, 12, 0);
        self.panel = panel;

        // Header
        let hdr = lv_label_create(panel);
        lv_label_set_text(hdr, c"CONTROL CENTER".as_ptr());
        lv_obj_set_style_text_color(hdr, rgb(CLR_ACCENT), 0);
        lv_obj_set_style_text_font(hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 0);

        // WiFi section
        let wifi_box = lv_obj_create(panel);
        lv_obj_set_size(wifi_box, 376, 56);
        lv_obj_align(wifi_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 20);
        lv_obj_set_style_bg_color(wifi_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(wifi_box, 8, 0);
        lv_obj_set_style_border_width(wifi_box, 0, 0);

        let lbl_wifi_hdr = lv_label_create(wifi_box);
        lv_label_set_text(lbl_wifi_hdr, c"WIFI".as_ptr());
        lv_obj_set_style_text_color(lbl_wifi_hdr, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(lbl_wifi_hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_wifi_hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 6);

        let lbl_wifi = lv_label_create(wifi_box);
        lv_label_set_text(lbl_wifi, c"Not connected".as_ptr());
        lv_obj_set_style_text_color(lbl_wifi, rgb(CLR_TEXT), 0);
        lv_obj_set_style_text_font(lbl_wifi, &lv_font_montserrat_14, 0);
        lv_obj_align(lbl_wifi, lv_align_t_LV_ALIGN_BOTTOM_LEFT, 8, -6);
        self.lbl_wifi = lbl_wifi;

        // NR section
        let nr_box = lv_obj_create(panel);
        lv_obj_set_size(nr_box, 376, 56);
        lv_obj_align(nr_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 84);
        lv_obj_set_style_bg_color(nr_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(nr_box, 8, 0);
        lv_obj_set_style_border_width(nr_box, 0, 0);

        let lbl_nr_hdr = lv_label_create(nr_box);
        lv_label_set_text(lbl_nr_hdr, c"NOISE REDUCTION".as_ptr());
        lv_obj_set_style_text_color(lbl_nr_hdr, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(lbl_nr_hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_nr_hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 6);

        let nr_label_texts = ["Off", "Low", "Med", "High"];
        for (i, &lbl_text) in nr_label_texts.iter().enumerate() {
            let btn = lv_button_create(nr_box);
            let btn_w = (376 - 24 - 3 * 4) / 4;
            lv_obj_set_size(btn, btn_w, 24);
            lv_obj_set_pos(btn, 8 + i as i32 * (btn_w + 4), 26);
            lv_obj_set_style_radius(btn, 5, 0);
            lv_obj_set_style_bg_color(btn, rgb(0x071410), 0);
            lv_obj_set_style_border_width(btn, 1, 0);
            lv_obj_set_style_border_color(btn, rgb(CLR_TEXT_DIM), 0);

            let lbl = lv_label_create(btn);
            let s = CString::new(lbl_text).unwrap();
            lv_label_set_text(lbl, s.as_ptr());
            lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
            lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT_DIM), 0);
            lv_obj_center(lbl);

            self.nr_btns[i] = btn;
        }
        self.highlight_active_nr();

        // Motor arm section
        let arm_box = lv_obj_create(panel);
        lv_obj_set_size(arm_box, 376, 56);
        lv_obj_align(arm_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 148);
        lv_obj_set_style_bg_color(arm_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(arm_box, 8, 0);
        lv_obj_set_style_border_width(arm_box, 0, 0);

        let lbl_arm_hdr = lv_label_create(arm_box);
        lv_label_set_text(lbl_arm_hdr, c"MIC ARM".as_ptr());
        lv_obj_set_style_text_color(lbl_arm_hdr, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(lbl_arm_hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_arm_hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 6);

        let btn_extend = lv_button_create(arm_box);
        lv_obj_set_size(btn_extend, 180, 24);
        lv_obj_set_pos(btn_extend, 8, 26);
        lv_obj_set_style_radius(btn_extend, 5, 0);
        lv_obj_set_style_bg_color(btn_extend, rgb(CLR_ACCENT), 0);
        lv_obj_set_style_border_width(btn_extend, 0, 0);
        let lbl_ext = lv_label_create(btn_extend);
        lv_label_set_text(lbl_ext, c"UP Extend".as_ptr());
        lv_obj_set_style_text_font(lbl_ext, &lv_font_montserrat_12, 0);
        lv_obj_set_style_text_color(lbl_ext, rgb(CLR_BG), 0);
        lv_obj_center(lbl_ext);

        let btn_retract = lv_button_create(arm_box);
        lv_obj_set_size(btn_retract, 180, 24);
        lv_obj_set_pos(btn_retract, 196, 26);
        lv_obj_set_style_radius(btn_retract, 5, 0);
        lv_obj_set_style_bg_color(btn_retract, rgb(0x071410), 0);
        lv_obj_set_style_border_width(btn_retract, 1, 0);
        lv_obj_set_style_border_color(btn_retract, rgb(CLR_TEXT_DIM), 0);
        let lbl_ret = lv_label_create(btn_retract);
        lv_label_set_text(lbl_ret, c"DN Retract".as_ptr());
        lv_obj_set_style_text_font(lbl_ret, &lv_font_montserrat_12, 0);
        lv_obj_set_style_text_color(lbl_ret, rgb(CLR_TEXT_DIM), 0);
        lv_obj_center(lbl_ret);

        // Swipe hint
        let hint = lv_label_create(panel);
        lv_label_set_text(hint, c"swipe up to close".as_ptr());
        lv_obj_set_style_text_color(hint, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(hint, &lv_font_montserrat_12, 0);
        lv_obj_align(hint, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -4);
    }

    unsafe fn highlight_active_nr(&self) {
        for (i, &btn) in self.nr_btns.iter().enumerate() {
            if btn.is_null() { continue; }
            let is_active = NR_LEVELS[i] == self.active_nr;
            let border_clr = if is_active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
            let text_clr   = if is_active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
            lv_obj_set_style_border_color(btn, border_clr, 0);
            let lbl = lv_obj_get_child(btn, 0);
            if !lbl.is_null() {
                lv_obj_set_style_text_color(lbl, text_clr, 0);
            }
        }
    }

    pub unsafe fn show(&mut self) {
        if self.visible { return; }
        self.visible = true;
        let mut anim = lv_anim_t::default();
        lv_anim_init(&mut anim);
        lv_anim_set_var(&mut anim, self.panel as *mut _);
        lv_anim_set_values(&mut anim, -240, 32);
        lv_anim_set_duration(&mut anim, 200u32);
        lv_anim_set_exec_cb(&mut anim, Some(anim_y_cb));
        lv_anim_start(&anim);
    }

    pub unsafe fn hide(&mut self) {
        if !self.visible { return; }
        self.visible = false;
        let mut anim = lv_anim_t::default();
        lv_anim_init(&mut anim);
        lv_anim_set_var(&mut anim, self.panel as *mut _);
        lv_anim_set_values(&mut anim, 32, -240);
        lv_anim_set_duration(&mut anim, 200u32);
        lv_anim_set_exec_cb(&mut anim, Some(anim_y_cb));
        lv_anim_start(&anim);
    }

    pub fn is_visible(&self) -> bool { self.visible }

    pub unsafe fn update_wifi(&self, ssid: &str, connected: bool) {
        let text = if connected {
            CString::new(format!("{ssid} \u{25CF}")).unwrap()
        } else {
            CString::new("Not connected").unwrap()
        };
        lv_label_set_text(self.lbl_wifi, text.as_ptr());
    }
}

unsafe extern "C" fn anim_y_cb(obj: *mut core::ffi::c_void, value: i32) {
    lv_obj_set_y(obj as *mut lv_obj_t, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nr_levels_array_matches_buttons() {
        assert_eq!(NR_LEVELS.len(), 4);
    }

    #[test]
    fn control_center_starts_hidden() {
        let bus = Arc::new(waved_state::StateBus::new());
        let cc = ControlCenter::new(bus);
        assert!(!cc.is_visible());
    }
}
