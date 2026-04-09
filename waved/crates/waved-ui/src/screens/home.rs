//! Home screen: teal sphere, VU meter, 3 buttons, bottom bar.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{AudioState, AudioMode, MeetingState, StateBus};

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_14: lv_font_t;
}

pub const VU_MAX_H: i32 = 20;
pub const VU_BAR_W: i32 = 3;
pub const VU_BAR_GAP: i32 = 2;
pub const VU_BANDS: usize = 8;

pub fn clamp_vu(v: f32) -> f32 { v.clamp(0.0, 1.0) }

pub fn vu_bar_height(level: f32) -> i32 {
    (clamp_vu(level) * VU_MAX_H as f32) as i32
}

pub struct HomeScreen {
    bus: Arc<StateBus>,
    connected_label: String,
    sphere: *mut lv_obj_t,
    lbl_status: *mut lv_obj_t,
    vu_bars: [*mut lv_obj_t; VU_BANDS],
    btn_mode: *mut lv_obj_t,
    lbl_mode: *mut lv_obj_t,
    btn_rec: *mut lv_obj_t,
    btn_mute: *mut lv_obj_t,
    lbl_nr: *mut lv_obj_t,
    lbl_upload: *mut lv_obj_t,
}

impl HomeScreen {
    pub fn new(bus: Arc<StateBus>, connected_label: &str) -> Self {
        Self {
            bus,
            connected_label: connected_label.to_owned(),
            sphere: std::ptr::null_mut(),
            lbl_status: std::ptr::null_mut(),
            vu_bars: [std::ptr::null_mut(); VU_BANDS],
            btn_mode: std::ptr::null_mut(),
            lbl_mode: std::ptr::null_mut(),
            btn_rec: std::ptr::null_mut(),
            btn_mute: std::ptr::null_mut(),
            lbl_nr: std::ptr::null_mut(),
            lbl_upload: std::ptr::null_mut(),
        }
    }

    /// Update all state-driven labels. Call each LVGL tick.
    pub unsafe fn update_state(&self, audio: &AudioState, meeting: &MeetingState) {
        // VU bars
        for (i, &bar) in self.vu_bars.iter().enumerate() {
            if bar.is_null() { continue; }
            let h = vu_bar_height(audio.vu_level[i]);
            lv_obj_set_height(bar, h.max(2));
        }

        // Mic mode label
        let mode_str = mode_short_name(&audio.mode);
        let s = CString::new(mode_str).unwrap();
        lv_label_set_text(self.lbl_mode, s.as_ptr());

        // NR level
        let nr_str = CString::new(format!("NR:{:?}", audio.nr_level)).unwrap();
        lv_label_set_text(self.lbl_nr, nr_str.as_ptr());

        // Upload queue count
        let upload_str = if meeting.pending_uploads == 0 {
            CString::new("☁ 0").unwrap()
        } else {
            CString::new(format!("☁ {}", meeting.pending_uploads)).unwrap()
        };
        lv_label_set_text(self.lbl_upload, upload_str.as_ptr());
    }
}

fn mode_short_name(mode: &AudioMode) -> &'static str {
    match mode {
        AudioMode::Omni => "OMNI",
        AudioMode::Stereo => "STEREO",
        AudioMode::Cardioid => "CARD.",
        AudioMode::Supercardioid => "S-CARD.",
        AudioMode::Figure8 => "FIG-8",
        AudioMode::NoiseReduction => "NR",
    }
}

impl super::Screen for HomeScreen {
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

            // Sphere: 180px circle, partially off-screen at bottom
            let sphere = lv_obj_create(screen);
            lv_obj_set_size(sphere, 180, 180);
            lv_obj_align(sphere, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, 30);
            lv_obj_set_style_radius(sphere, 90, 0);
            lv_obj_set_style_bg_color(sphere, rgb(0x071a14), 0);
            lv_obj_set_style_bg_grad_color(sphere, rgb(0x1a5c4a), 0);
            lv_obj_set_style_bg_grad_dir(sphere, lv_grad_dir_t_LV_GRAD_DIR_VER, 0);
            lv_obj_set_style_shadow_color(sphere, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_shadow_width(sphere, 30, 0);
            lv_obj_set_style_shadow_opa(sphere, 100, 0);
            lv_obj_set_style_border_width(sphere, 0, 0);
            self.sphere = sphere;

            // Status label inside sphere
            let lbl = lv_label_create(sphere);
            lv_obj_set_style_text_color(lbl, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
            lv_label_set_text(lbl, c"● Connected".as_ptr());
            lv_obj_align(lbl, lv_align_t_LV_ALIGN_TOP_MID, 0, 20);
            self.lbl_status = lbl;

            // VU bars inside sphere
            let vu_total_w = VU_BANDS as i32 * (VU_BAR_W + VU_BAR_GAP) - VU_BAR_GAP;
            for i in 0..VU_BANDS {
                let bar = lv_obj_create(sphere);
                lv_obj_set_size(bar, VU_BAR_W, 2);
                let x = -vu_total_w / 2 + i as i32 * (VU_BAR_W + VU_BAR_GAP);
                lv_obj_set_pos(bar, 90 + x, 60);
                lv_obj_set_style_bg_color(bar, rgb(CLR_ACCENT), 0);
                lv_obj_set_style_radius(bar, 2, 0);
                lv_obj_set_style_border_width(bar, 0, 0);
                self.vu_bars[i] = bar;
            }

            // Button row
            let btn_y = -(32 + 15 + 38);

            // Mic Mode button
            let btn_mode = lv_button_create(screen);
            lv_obj_set_size(btn_mode, 60, 38);
            lv_obj_align(btn_mode, lv_align_t_LV_ALIGN_BOTTOM_MID, -80, btn_y);
            lv_obj_set_style_radius(btn_mode, 19, 0);
            lv_obj_set_style_bg_color(btn_mode, rgb(0x071a14), 0);
            lv_obj_set_style_shadow_color(btn_mode, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_shadow_width(btn_mode, 8, 0);
            self.btn_mode = btn_mode;

            let lbl_mode = lv_label_create(btn_mode);
            lv_label_set_text(lbl_mode, c"OMNI".as_ptr());
            lv_obj_set_style_text_font(lbl_mode, &lv_font_montserrat_12, 0);
            lv_obj_set_style_text_color(lbl_mode, rgb(CLR_ACCENT), 0);
            lv_obj_center(lbl_mode);
            self.lbl_mode = lbl_mode;

            // REC pill
            let btn_rec = lv_button_create(screen);
            lv_obj_set_size(btn_rec, 80, 32);
            lv_obj_align(btn_rec, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, btn_y + 3);
            lv_obj_set_style_radius(btn_rec, 16, 0);
            lv_obj_set_style_bg_color(btn_rec, rgb(0x007a5e), 0);
            lv_obj_set_style_shadow_color(btn_rec, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_shadow_width(btn_rec, 12, 0);
            self.btn_rec = btn_rec;

            let lbl_rec = lv_label_create(btn_rec);
            lv_label_set_text(lbl_rec, c"● REC".as_ptr());
            lv_obj_set_style_text_font(lbl_rec, &lv_font_montserrat_14, 0);
            lv_obj_set_style_text_color(lbl_rec, rgb(CLR_WHITE), 0);
            lv_obj_center(lbl_rec);

            // Mute button
            let btn_mute = lv_button_create(screen);
            lv_obj_set_size(btn_mute, 60, 38);
            lv_obj_align(btn_mute, lv_align_t_LV_ALIGN_BOTTOM_MID, 80, btn_y);
            lv_obj_set_style_radius(btn_mute, 19, 0);
            lv_obj_set_style_bg_color(btn_mute, rgb(0x071a14), 0);
            self.btn_mute = btn_mute;

            let lbl_mute = lv_label_create(btn_mute);
            lv_label_set_text(lbl_mute, c"Mute".as_ptr());
            lv_obj_set_style_text_font(lbl_mute, &lv_font_montserrat_12, 0);
            lv_obj_set_style_text_color(lbl_mute, rgb(CLR_TEXT_DIM), 0);
            lv_obj_center(lbl_mute);

            // Bottom bar
            let bbar = lv_obj_create(screen);
            lv_obj_set_size(bbar, 400, 32);
            lv_obj_align(bbar, lv_align_t_LV_ALIGN_BOTTOM_LEFT, 0, 0);
            lv_obj_set_style_bg_color(bbar, rgb(0x080808), 0);
            lv_obj_set_style_border_width(bbar, 0, 0);
            lv_obj_set_style_pad_top(bbar, 0, 0);
            lv_obj_set_style_pad_bottom(bbar, 0, 0);
            lv_obj_set_style_pad_left(bbar, 0, 0);
            lv_obj_set_style_pad_right(bbar, 0, 0);

            let mk_bbar_lbl = |text: &[u8], x: i32| {
                let lbl = lv_label_create(bbar);
                lv_label_set_text(lbl, text.as_ptr() as *const i8);
                lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
                lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT_DIM), 0);
                lv_obj_align(lbl, lv_align_t_LV_ALIGN_LEFT_MID, x, 0);
                lbl
            };

            let lbl_nr       = mk_bbar_lbl(b"NR:Off\0", 8);
            let _files_lbl   = mk_bbar_lbl(b"Files\0", 108);
            let lbl_upload   = mk_bbar_lbl(b"\xe2\x98\x81 0\0", 208); // ☁ 0 in UTF-8
            let _settings_lbl = mk_bbar_lbl(b"Settings\0", 300);

            self.lbl_nr = lbl_nr;
            self.lbl_upload = lbl_upload;

            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vu_level_clamps_above_one() {
        assert!(clamp_vu(1.5) <= 1.0);
    }

    #[test]
    fn vu_bar_height_scales_correctly() {
        assert_eq!(vu_bar_height(0.0), 0);
        assert_eq!(vu_bar_height(1.0), VU_MAX_H);
        assert_eq!(vu_bar_height(0.5), VU_MAX_H / 2);
    }
}
