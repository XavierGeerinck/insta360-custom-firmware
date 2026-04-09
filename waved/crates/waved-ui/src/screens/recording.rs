//! Recording screen: red sphere, elapsed timer, VU meter, bookmark + stop + mute.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use crate::screens::home::{VU_BAR_W, VU_BAR_GAP, VU_BANDS, vu_bar_height};
use waved_state::{AudioState, AudioMode, MeetingState, StateBus};

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_14: lv_font_t;
    static lv_font_montserrat_20: lv_font_t;
}

pub fn format_duration(secs: u32) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

pub struct RecordingScreen {
    _bus: Arc<StateBus>,
    sphere: *mut lv_obj_t,
    lbl_timer: *mut lv_obj_t,
    vu_bars: [*mut lv_obj_t; VU_BANDS],
    lbl_mode_badge: *mut lv_obj_t,
    lbl_storage: *mut lv_obj_t,
}

impl RecordingScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            _bus: bus,
            sphere: std::ptr::null_mut(),
            lbl_timer: std::ptr::null_mut(),
            vu_bars: [std::ptr::null_mut(); VU_BANDS],
            lbl_mode_badge: std::ptr::null_mut(),
            lbl_storage: std::ptr::null_mut(),
        }
    }

    pub unsafe fn update_state(&self, audio: &AudioState, meeting: &MeetingState) {
        // Timer
        let dur = format_duration(meeting.duration_secs);
        let s = CString::new(dur).unwrap();
        lv_label_set_text(self.lbl_timer, s.as_ptr());

        // VU bars
        for (i, &bar) in self.vu_bars.iter().enumerate() {
            if bar.is_null() { continue; }
            lv_obj_set_height(bar, vu_bar_height(audio.vu_level[i]).max(2));
        }

        // Mode badge
        let mode_name = match audio.mode {
            AudioMode::Omni => "OMNI",
            AudioMode::Stereo => "STEREO",
            AudioMode::Cardioid => "CARD.",
            AudioMode::Supercardioid => "S-CARD.",
            AudioMode::Figure8 => "FIG-8",
            AudioMode::NoiseReduction => "NR",
        };
        let badge = CString::new(format!("{} · NR:{:?}", mode_name, audio.nr_level)).unwrap();
        lv_label_set_text(self.lbl_mode_badge, badge.as_ptr());
    }
}

impl super::Screen for RecordingScreen {
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

            // Red sphere
            let sphere = lv_obj_create(screen);
            lv_obj_set_size(sphere, 180, 180);
            lv_obj_align(sphere, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, 30);
            lv_obj_set_style_radius(sphere, 90, 0);
            lv_obj_set_style_bg_color(sphere, rgb(0x2a0a0a), 0);
            lv_obj_set_style_bg_grad_color(sphere, rgb(0x5a1010), 0);
            lv_obj_set_style_bg_grad_dir(sphere, lv_grad_dir_t_LV_GRAD_DIR_VER, 0);
            lv_obj_set_style_shadow_color(sphere, rgb(CLR_RED), 0);
            lv_obj_set_style_shadow_width(sphere, 30, 0);
            lv_obj_set_style_shadow_opa(sphere, 120, 0);
            lv_obj_set_style_border_width(sphere, 0, 0);
            self.sphere = sphere;

            let lbl_rec = lv_label_create(sphere);
            lv_label_set_text(lbl_rec, c"● RECORDING".as_ptr());
            lv_obj_set_style_text_color(lbl_rec, rgb(CLR_RED), 0);
            lv_obj_set_style_text_font(lbl_rec, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_rec, lv_align_t_LV_ALIGN_TOP_MID, 0, 12);

            let lbl_timer = lv_label_create(sphere);
            lv_label_set_text(lbl_timer, c"00:00:00".as_ptr());
            lv_obj_set_style_text_color(lbl_timer, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(lbl_timer, &lv_font_montserrat_20, 0);
            lv_obj_align(lbl_timer, lv_align_t_LV_ALIGN_TOP_MID, 0, 30);
            self.lbl_timer = lbl_timer;

            // VU bars
            let vu_total_w = VU_BANDS as i32 * (VU_BAR_W + VU_BAR_GAP) - VU_BAR_GAP;
            for i in 0..VU_BANDS {
                let bar = lv_obj_create(sphere);
                lv_obj_set_size(bar, VU_BAR_W, 2);
                let x = -vu_total_w / 2 + i as i32 * (VU_BAR_W + VU_BAR_GAP);
                lv_obj_set_pos(bar, 90 + x, 60);
                lv_obj_set_style_bg_color(bar, rgb(CLR_RED), 0);
                lv_obj_set_style_radius(bar, 2, 0);
                lv_obj_set_style_border_width(bar, 0, 0);
                self.vu_bars[i] = bar;
            }

            // Mode badge above sphere
            let lbl_mode = lv_label_create(screen);
            lv_label_set_text(lbl_mode, c"OMNI · NR:Off".as_ptr());
            lv_obj_set_style_text_color(lbl_mode, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_text_font(lbl_mode, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_mode, lv_align_t_LV_ALIGN_TOP_MID, 0, 38);
            self.lbl_mode_badge = lbl_mode;

            // 3 buttons
            let btn_y = -(32 + 15 + 38);

            let btn_mark = lv_button_create(screen);
            lv_obj_set_size(btn_mark, 60, 38);
            lv_obj_align(btn_mark, lv_align_t_LV_ALIGN_BOTTOM_MID, -80, btn_y);
            lv_obj_set_style_radius(btn_mark, 19, 0);
            lv_obj_set_style_bg_color(btn_mark, rgb(0x071a14), 0);
            let lbl_mark = lv_label_create(btn_mark);
            lv_label_set_text(lbl_mark, c"Mark".as_ptr());
            lv_obj_set_style_text_color(lbl_mark, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(lbl_mark, &lv_font_montserrat_12, 0);
            lv_obj_center(lbl_mark);

            let btn_stop = lv_button_create(screen);
            lv_obj_set_size(btn_stop, 80, 32);
            lv_obj_align(btn_stop, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, btn_y + 3);
            lv_obj_set_style_radius(btn_stop, 16, 0);
            lv_obj_set_style_bg_color(btn_stop, rgb(0x8b0000), 0);
            let lbl_stop = lv_label_create(btn_stop);
            lv_label_set_text(lbl_stop, c"■ STOP".as_ptr());
            lv_obj_set_style_text_color(lbl_stop, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(lbl_stop, &lv_font_montserrat_14, 0);
            lv_obj_center(lbl_stop);

            let btn_mute = lv_button_create(screen);
            lv_obj_set_size(btn_mute, 60, 38);
            lv_obj_align(btn_mute, lv_align_t_LV_ALIGN_BOTTOM_MID, 80, btn_y);
            lv_obj_set_style_radius(btn_mute, 19, 0);
            lv_obj_set_style_bg_color(btn_mute, rgb(0x071a14), 0);
            let lbl_mute = lv_label_create(btn_mute);
            lv_label_set_text(lbl_mute, c"Mute".as_ptr());
            lv_obj_set_style_text_color(lbl_mute, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(lbl_mute, &lv_font_montserrat_12, 0);
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

            let mk = |text: &[u8], x: i32| {
                let lbl = lv_label_create(bbar);
                lv_label_set_text(lbl, text.as_ptr() as *const i8);
                lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
                lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT_DIM), 0);
                lv_obj_align(lbl, lv_align_t_LV_ALIGN_LEFT_MID, x, 0);
                lbl
            };
            let _ = mk(b"OMNI\0", 8);
            let _ = mk(b"NR:Off\0", 88);
            let lbl_storage = mk(b"-- GB\0", 188);
            let _ = mk(b"Settings\0", 300);
            self.lbl_storage = lbl_storage;

            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_zero() {
        assert_eq!(format_duration(0), "00:00:00");
    }

    #[test]
    fn format_duration_one_hour() {
        assert_eq!(format_duration(3661), "01:01:01");
    }
}
