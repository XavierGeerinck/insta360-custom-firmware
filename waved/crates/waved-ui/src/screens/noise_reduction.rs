//! Noise Reduction strength selector — SP#4.
//!
//! Replaces the `aec/speex` NS with DeepFilterNet 3 (CPU). This screen lets
//! the user pick `Off / Low / Med / High`, which maps to DFN3's
//! `RuntimeParams::with_atten_lim` cap inside `waved_audio::build_dsp_chain`.
//!
//! Layout (400×304):
//!   Title         "NOISE REDUCTION" centered, y=18
//!   4 buttons     376×48 stacked, x=12, y=54 + i*60   (last bottom: 54+3*60+48 = 282)
//!   Done button   64×64 BOTTOM_MID y=-8 — sends `Command::CloseSettings`
//!
//! Layout chosen over a carousel (cf. audio_mode.rs) because NR is a
//! one-dimensional strength choice with 4 values; a vertical list reads
//! faster and doesn't need card art we don't have.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{AudioState, Command, NrLevel, StateBus};

extern "C" {
    static lv_font_montserrat_14: lv_font_t;
}

const LABELS: [&str; 4] = ["Off", "Low", "Medium", "High"];
const SUBLABELS: [&str; 4] = [
    "no neural NS",
    "-12 dB cap",
    "-24 dB cap",
    "full attenuation",
];

fn level_to_idx(level: &NrLevel) -> usize {
    match level {
        NrLevel::Off  => 0,
        NrLevel::Low  => 1,
        NrLevel::Med  => 2,
        NrLevel::High => 3,
    }
}

// ── Callbacks ─────────────────────────────────────────────────────────────────

macro_rules! level_cb {
    ($name:ident, $level:expr) => {
        unsafe extern "C" fn $name(e: *mut lv_event_t) {
            if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
                let tx = lv_event_get_user_data(e)
                    as *const tokio::sync::broadcast::Sender<Command>;
                if !tx.is_null() {
                    let _ = (*tx).send(Command::SetNrLevel($level));
                }
            }
        }
    };
}

level_cb!(level0_cb, NrLevel::Off);
level_cb!(level1_cb, NrLevel::Low);
level_cb!(level2_cb, NrLevel::Med);
level_cb!(level3_cb, NrLevel::High);

const CARD_CBS: &[unsafe extern "C" fn(*mut lv_event_t)] = &[
    level0_cb, level1_cb, level2_cb, level3_cb,
];

unsafe extern "C" fn done_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::CloseSettings);
        }
    }
}

// ── Screen ────────────────────────────────────────────────────────────────────

pub struct NoiseReductionScreen {
    bus:        Arc<StateBus>,
    btns:       [*mut lv_obj_t; 4],
    radios:     [*mut lv_obj_t; 4],
    prev_level: Option<NrLevel>,
}

impl NoiseReductionScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            btns:       [std::ptr::null_mut(); 4],
            radios:     [std::ptr::null_mut(); 4],
            prev_level: None,
        }
    }

    /// Refresh radio + border colour to reflect the live `nr_level`.
    pub unsafe fn update_state(&mut self, audio: &AudioState) {
        if self.prev_level.as_ref() == Some(&audio.nr_level) {
            return;
        }
        let idx = level_to_idx(&audio.nr_level);
        for i in 0..4usize {
            if self.btns[i].is_null() { continue; }
            let active = i == idx;
            let clr = if active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
            lv_obj_set_style_border_color(self.btns[i], clr, 0);
            if !self.radios[i].is_null() {
                let src: &[u8] = if active {
                    b"A:/etc/pspapp/lvgl/icon/btn/radio_checked.png\0"
                } else {
                    b"A:/etc/pspapp/lvgl/icon/btn/radio_unchecked.png\0"
                };
                lv_image_set_src(self.radios[i], src.as_ptr() as *const _);
            }
        }
        self.prev_level = Some(audio.nr_level.clone());
    }
}

impl super::Screen for NoiseReductionScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            self.prev_level = None;
            self.btns   = [std::ptr::null_mut(); 4];
            self.radios = [std::ptr::null_mut(); 4];

            let screen = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_top(screen, 0, 0);
            lv_obj_set_style_pad_bottom(screen, 0, 0);
            lv_obj_set_style_pad_left(screen, 0, 0);
            lv_obj_set_style_pad_right(screen, 0, 0);

            // ── Title ────────────────────────────────────────────────────────
            let title = lv_label_create(screen);
            lv_label_set_text(title, c"NOISE REDUCTION".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_14, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 18);

            // ── 4 buttons stacked ────────────────────────────────────────────
            for (i, &label) in LABELS.iter().enumerate() {
                let btn = lv_button_create(screen);
                lv_obj_set_size(btn, 376, 48);
                lv_obj_set_pos(btn, 12, 54 + (i as i32) * 60);
                lv_obj_set_style_radius(btn, 10, 0);
                lv_obj_set_style_bg_color(btn, rgb(CLR_SURFACE), 0);
                lv_obj_set_style_bg_opa(btn, 200, 0);
                lv_obj_set_style_border_width(btn, 2, 0);
                lv_obj_set_style_border_color(btn, rgb(CLR_TEXT_DIM), 0);
                lv_obj_set_style_pad_top(btn, 0, 0);
                lv_obj_set_style_pad_bottom(btn, 0, 0);
                lv_obj_set_style_pad_left(btn, 12, 0);
                lv_obj_set_style_pad_right(btn, 12, 0);
                lv_obj_remove_flag(btn, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

                // Radio icon at LEFT_MID.
                let radio = lv_image_create(btn);
                lv_obj_set_size(radio, 28, 28);
                lv_image_set_src(
                    radio,
                    b"A:/etc/pspapp/lvgl/icon/btn/radio_unchecked.png\0".as_ptr() as *const _,
                );
                lv_obj_align(radio, lv_align_t_LV_ALIGN_LEFT_MID, 0, 0);
                lv_obj_remove_flag(radio, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
                self.radios[i] = radio;

                // Main label.
                let lbl = lv_label_create(btn);
                let s = CString::new(label).unwrap();
                lv_label_set_text(lbl, s.as_ptr());
                lv_obj_set_style_text_color(lbl, rgb(CLR_WHITE), 0);
                lv_obj_set_style_text_font(lbl, &lv_font_montserrat_14, 0);
                lv_obj_align(lbl, lv_align_t_LV_ALIGN_LEFT_MID, 44, -8);

                // Sub-label (smaller, dim).
                let sub = lv_label_create(btn);
                let ss = CString::new(SUBLABELS[i]).unwrap();
                lv_label_set_text(sub, ss.as_ptr());
                lv_obj_set_style_text_color(sub, rgb(CLR_TEXT_DIM), 0);
                lv_obj_align(sub, lv_align_t_LV_ALIGN_LEFT_MID, 44, 8);

                let tx = Box::into_raw(Box::new(self.bus.commands.clone()));
                lv_obj_add_event_cb(
                    btn,
                    Some(CARD_CBS[i]),
                    lv_event_code_t_LV_EVENT_CLICKED,
                    tx as *mut _,
                );
                self.btns[i] = btn;
            }

            // ── Done button ─────────────────────────────────────────────────
            // Same factory as audio_mode.rs.
            let btn_done = lv_obj_create(screen);
            lv_obj_set_size(btn_done, 64, 64);
            lv_obj_align(btn_done, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -8);
            lv_obj_set_style_bg_image_src(
                btn_done,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_done_white.png\0".as_ptr() as *const _,
                0,
            );
            lv_obj_set_style_bg_opa(btn_done, 255, 0);
            lv_obj_set_style_border_width(btn_done, 0, 0);
            lv_obj_set_style_radius(btn_done, 32, 0);
            lv_obj_set_style_pad_top(btn_done, 0, 0);
            lv_obj_set_style_pad_bottom(btn_done, 0, 0);
            lv_obj_set_style_pad_left(btn_done, 0, 0);
            lv_obj_set_style_pad_right(btn_done, 0, 0);
            lv_obj_remove_flag(btn_done, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            lv_obj_add_flag(btn_done, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
            let done_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(
                btn_done,
                Some(done_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                done_tx as *mut _,
            );

            // Initial highlight; subsequent ticks go through update_state.
            let snapshot = self.bus.audio.borrow().clone();
            self.update_state(&snapshot);

            screen
        }
    }
}
