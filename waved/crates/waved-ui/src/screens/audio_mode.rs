//! Mic Mode picker — 5 modes shown as selectable cards.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{AudioMode, StateBus};

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_16: lv_font_t;
}

pub fn mode_label(mode: &AudioMode) -> &'static str {
    match mode {
        AudioMode::Omni          => "Omni",
        AudioMode::Stereo        => "Stereo",
        AudioMode::Cardioid      => "Cardioid",
        AudioMode::Supercardioid => "Super-card.",
        AudioMode::Figure8       => "Figure-8",
        AudioMode::NoiseReduction => "NR",
    }
}

const MODES: [AudioMode; 5] = [
    AudioMode::Omni,
    AudioMode::Stereo,
    AudioMode::Cardioid,
    AudioMode::Supercardioid,
    AudioMode::Figure8,
];

pub struct AudioModeScreen {
    _bus: Arc<StateBus>,
    current_mode: AudioMode,
}

impl AudioModeScreen {
    pub fn new(bus: Arc<StateBus>, current_mode: AudioMode) -> Self {
        Self { _bus: bus, current_mode }
    }
}

impl super::Screen for AudioModeScreen {
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
            lv_label_set_text(title, c"Mic Mode".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_TEXT), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_16, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 38);

            let card_w = 100i32;
            let card_h = 80i32;
            let gap = 10i32;

            for (i, mode) in MODES.iter().enumerate() {
                let row = i / 3;
                let col = i % 3;
                let total_row_w = if row == 0 { 3 * card_w + 2 * gap } else { 2 * card_w + gap };
                let x_start = (400 - total_row_w) / 2;
                let x = x_start + col as i32 * (card_w + gap);
                let y = 64 + row as i32 * (card_h + gap);

                let card = lv_obj_create(screen);
                lv_obj_set_size(card, card_w, card_h);
                lv_obj_set_pos(card, x, y);
                lv_obj_set_style_bg_color(card, rgb(CLR_SURFACE), 0);
                lv_obj_set_style_radius(card, 8, 0);

                if *mode == self.current_mode {
                    lv_obj_set_style_border_color(card, rgb(CLR_ACCENT), 0);
                    lv_obj_set_style_border_width(card, 2, 0);
                } else {
                    lv_obj_set_style_border_width(card, 0, 0);
                }

                let lbl = lv_label_create(card);
                let label_str = CString::new(mode_label(mode)).unwrap();
                lv_label_set_text(lbl, label_str.as_ptr());
                lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT), 0);
                lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
                lv_obj_align(lbl, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -8);
            }

            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waved_state::AudioMode;

    #[test]
    fn all_modes_have_labels() {
        let modes = [
            AudioMode::Omni,
            AudioMode::Stereo,
            AudioMode::Cardioid,
            AudioMode::Supercardioid,
            AudioMode::Figure8,
        ];
        for mode in &modes {
            assert!(!mode_label(mode).is_empty(), "no label for {mode:?}");
        }
    }
}
