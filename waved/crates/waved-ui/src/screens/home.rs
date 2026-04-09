//! Home screen: original pspapp PNG assets (bg_main + button images + mode icons).

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{AudioState, AudioMode, MeetingState, StateBus};

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_14: lv_font_t;
}

// ── Asset path helpers ────────────────────────────────────────────────────────

/// 32×32 beamforming mode icon from the original pspapp asset set.
fn mode_icon_path(mode: &AudioMode) -> &'static [u8] {
    match mode {
        AudioMode::Omni | AudioMode::NoiseReduction =>
            b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_all.png\0",
        AudioMode::Stereo =>
            b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_stereo.png\0",
        AudioMode::Cardioid =>
            b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_heart.png\0",
        AudioMode::Supercardioid =>
            b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_super_heart.png\0",
        AudioMode::Figure8 =>
            b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_eight.png\0",
    }
}

fn mode_short_name(mode: &AudioMode) -> &'static str {
    match mode {
        AudioMode::Omni          => "OMNI",
        AudioMode::Stereo        => "STEREO",
        AudioMode::Cardioid      => "CARD.",
        AudioMode::Supercardioid => "S-CARD",
        AudioMode::Figure8       => "FIG-8",
        AudioMode::NoiseReduction=> "NR",
    }
}

// ── Struct ────────────────────────────────────────────────────────────────────

pub struct HomeScreen {
    _bus: Arc<StateBus>,
    img_mode_icon: *mut lv_obj_t,
    lbl_mode: *mut lv_obj_t,
    lbl_mute: *mut lv_obj_t,
    lbl_nr: *mut lv_obj_t,
    lbl_upload: *mut lv_obj_t,
}

impl HomeScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            _bus: bus,
            img_mode_icon: std::ptr::null_mut(),
            lbl_mode:      std::ptr::null_mut(),
            lbl_mute:      std::ptr::null_mut(),
            lbl_nr:        std::ptr::null_mut(),
            lbl_upload:    std::ptr::null_mut(),
        }
    }

    /// Drive LVGL label/image updates from the latest state snapshot.
    pub unsafe fn update_state(&self, audio: &AudioState, meeting: &MeetingState) {
        // Beamforming mode icon
        if !self.img_mode_icon.is_null() {
            lv_image_set_src(
                self.img_mode_icon,
                mode_icon_path(&audio.mode).as_ptr() as *const _,
            );
        }

        // Mode name label
        let mode_s = CString::new(mode_short_name(&audio.mode)).unwrap();
        lv_label_set_text(self.lbl_mode, mode_s.as_ptr());

        // Mute button label
        let mute_s = if audio.mic_muted { c"MUTED".as_ptr() } else { c"Mute".as_ptr() };
        lv_label_set_text(self.lbl_mute, mute_s);

        // Bottom bar labels
        let nr_s = CString::new(format!("NR:{:?}", audio.nr_level)).unwrap();
        lv_label_set_text(self.lbl_nr, nr_s.as_ptr());

        let up_s = CString::new(format!("C:{}", meeting.pending_uploads)).unwrap();
        lv_label_set_text(self.lbl_upload, up_s.as_ptr());
    }
}

// ── Screen trait impl ─────────────────────────────────────────────────────────

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

            // ── Background (400×304, the glowing teal sphere) ─────────────
            let bg = lv_image_create(screen);
            lv_image_set_src(
                bg,
                c"A:/etc/pspapp/lvgl/icon/msc/bg_main.png".as_ptr() as *const _,
            );
            lv_obj_align(bg, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 0);
            lv_obj_set_style_border_width(bg, 0, 0);

            // ── Button row ────────────────────────────────────────────────
            // Three buttons, bottoms flush 40 px above the screen bottom
            // (32 px bottom bar + 8 px gap).
            // Widths:  76 · 12 gap · 180 · 12 gap · 76  = 356 px
            // Centers from BOTTOM_MID: −140, 0, +140

            // Left: Mode selector (76×76 circle)
            let btn_mode = lv_image_create(screen);
            lv_image_set_src(
                btn_mode,
                c"A:/etc/pspapp/lvgl/icon/msc/bg_main_circule_btn.png".as_ptr() as *const _,
            );
            lv_obj_align(btn_mode, lv_align_t_LV_ALIGN_BOTTOM_MID, -140, -40);
            lv_obj_set_style_border_width(btn_mode, 0, 0);

            // Beamforming icon centred in the upper half of the mode button
            let img_mode_icon = lv_image_create(btn_mode);
            lv_image_set_src(
                img_mode_icon,
                c"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_all.png".as_ptr() as *const _,
            );
            lv_obj_align(img_mode_icon, lv_align_t_LV_ALIGN_TOP_MID, 0, 10);
            self.img_mode_icon = img_mode_icon;

            // Mode name label in the lower part of the mode button
            let lbl_mode = lv_label_create(btn_mode);
            lv_label_set_text(lbl_mode, c"OMNI".as_ptr());
            lv_obj_set_style_text_color(lbl_mode, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_text_font(lbl_mode, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_mode, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -8);
            self.lbl_mode = lbl_mode;

            // Centre: REC pill (180×76)
            let btn_rec = lv_image_create(screen);
            lv_image_set_src(
                btn_rec,
                c"A:/etc/pspapp/lvgl/icon/msc/bg_main_record_btn.png".as_ptr() as *const _,
            );
            lv_obj_align(btn_rec, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -40);
            lv_obj_set_style_border_width(btn_rec, 0, 0);

            let lbl_rec = lv_label_create(btn_rec);
            lv_label_set_text(lbl_rec, c"> REC".as_ptr());
            lv_obj_set_style_text_color(lbl_rec, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(lbl_rec, &lv_font_montserrat_14, 0);
            lv_obj_center(lbl_rec);

            // Right: Mute button (76×76 circle, slightly different shade)
            let btn_mute = lv_image_create(screen);
            lv_image_set_src(
                btn_mute,
                c"A:/etc/pspapp/lvgl/icon/msc/bg_main_mic_btn.png".as_ptr() as *const _,
            );
            lv_obj_align(btn_mute, lv_align_t_LV_ALIGN_BOTTOM_MID, 140, -40);
            lv_obj_set_style_border_width(btn_mute, 0, 0);

            // Mic icon in upper half of mute button
            let img_mic = lv_image_create(btn_mute);
            lv_image_set_src(
                img_mic,
                c"A:/etc/pspapp/lvgl/icon/volume/mic_on.png".as_ptr() as *const _,
            );
            lv_obj_align(img_mic, lv_align_t_LV_ALIGN_TOP_MID, 0, 10);

            let lbl_mute = lv_label_create(btn_mute);
            lv_label_set_text(lbl_mute, c"Mute".as_ptr());
            lv_obj_set_style_text_color(lbl_mute, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(lbl_mute, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_mute, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -8);
            self.lbl_mute = lbl_mute;

            // ── Bottom bar (400×32) ───────────────────────────────────────
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
                lv_label_set_text(lbl, text.as_ptr() as *const _);
                lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
                lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT_DIM), 0);
                lv_obj_align(lbl, lv_align_t_LV_ALIGN_LEFT_MID, x, 0);
                lbl
            };

            self.lbl_nr     = mk(b"NR:Off\0",   8);
            let _            = mk(b"Files\0",   108);
            self.lbl_upload = mk(b"C:0\0",      208);
            let _            = mk(b"Settings\0",300);

            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_icon_path_not_empty() {
        assert!(!mode_icon_path(&AudioMode::Omni).is_empty());
        assert!(!mode_icon_path(&AudioMode::Figure8).is_empty());
    }
}
