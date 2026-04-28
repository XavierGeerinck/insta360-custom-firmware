//! Noise Reduction strength selector — SP#4.
//!
//! 4 circular buttons in a row (Off / Low / Med / High). Tap the one you
//! want — selected circle fills with accent colour, others stay dim. Tap
//! Done to dismiss. The dB-cap sublabel under the row tells you what
//! "Low / Med / High" mean (12 / 24 / 100 dB DFN3 attenuation cap).
//!
//! Layout (400×304):
//!   y=24   Title  "NOISE REDUCTION"
//!   y=ctr  Row of 4 circles (76×76, 16 px gap), horizontally centered
//!   y+96   Sublabel — "Low — light, room tone preserved"
//!   y=-8   Done button (BOTTOM_MID), shared factory with audio_mode.rs

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{AudioState, Command, NrLevel, StateBus};

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_14: lv_font_t;
}

const LABELS: [&str; 4] = ["Off", "Low", "Med", "High"];

/// What we say below the row when a level is selected. Maps NrLevel to
/// the same atten_lim_db cap used by `waved_audio::nr_to_atten_lim_db`.
const DETAIL: [&str; 4] = [
    "Off — speex AEC only, no neural NS",
    "Low — -12 dB cap, room tone preserved",
    "Med — -24 dB cap, moderate suppression",
    "High — full DFN3 attenuation",
];

fn level_to_idx(level: &NrLevel) -> usize {
    match level {
        NrLevel::Off  => 0,
        NrLevel::Low  => 1,
        NrLevel::Med  => 2,
        NrLevel::High => 3,
    }
}

// ── Per-circle click callbacks ────────────────────────────────────────────────

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
    circles:    [*mut lv_obj_t; 4],
    inner_lbls: [*mut lv_obj_t; 4],
    detail_lbl: *mut lv_obj_t,
    prev_level: Option<NrLevel>,
}

impl NoiseReductionScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            circles:    [std::ptr::null_mut(); 4],
            inner_lbls: [std::ptr::null_mut(); 4],
            detail_lbl: std::ptr::null_mut(),
            prev_level: None,
        }
    }

    /// Style a circle for active vs inactive. Active = filled accent +
    /// dark text; inactive = transparent surface + accent outline + accent
    /// text.
    unsafe fn paint_circle(circle: *mut lv_obj_t, label: *mut lv_obj_t, active: bool) {
        if active {
            lv_obj_set_style_bg_color(circle, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_bg_opa(circle, 255, 0);
            lv_obj_set_style_border_color(circle, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_border_width(circle, 3, 0);
            if !label.is_null() {
                // Dark text on accent fill stays readable.
                lv_obj_set_style_text_color(label, rgb(CLR_BG), 0);
            }
        } else {
            lv_obj_set_style_bg_color(circle, rgb(CLR_SURFACE), 0);
            lv_obj_set_style_bg_opa(circle, 96, 0);
            lv_obj_set_style_border_color(circle, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_border_width(circle, 2, 0);
            if !label.is_null() {
                lv_obj_set_style_text_color(label, rgb(CLR_WHITE), 0);
            }
        }
    }

    pub unsafe fn update_state(&mut self, audio: &AudioState) {
        if self.prev_level.as_ref() == Some(&audio.nr_level) {
            return;
        }
        let idx = level_to_idx(&audio.nr_level);

        for i in 0..4usize {
            if self.circles[i].is_null() { continue; }
            Self::paint_circle(self.circles[i], self.inner_lbls[i], i == idx);
        }

        if !self.detail_lbl.is_null() {
            let s = CString::new(DETAIL[idx]).unwrap();
            lv_label_set_text(self.detail_lbl, s.as_ptr());
        }

        self.prev_level = Some(audio.nr_level.clone());
    }
}

impl super::Screen for NoiseReductionScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            self.prev_level = None;
            self.circles    = [std::ptr::null_mut(); 4];
            self.inner_lbls = [std::ptr::null_mut(); 4];

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
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 24);

            // ── Row of 4 circles ─────────────────────────────────────────────
            // 4 × 76 + 3 × 16 = 352 → 24 px margin each side, fits 400.
            const SIZE: i32 = 76;
            const GAP:  i32 = 16;
            const STEP: i32 = SIZE + GAP;
            // Row width 352, centred: first circle x = (400 - 352) / 2 = 24.
            let x0: i32 = (400 - (4 * SIZE + 3 * GAP)) / 2;
            let y0: i32 = 88; // below title, leaves room for sublabel + Done.

            for i in 0..4usize {
                let circle = lv_button_create(screen);
                lv_obj_set_size(circle, SIZE, SIZE);
                lv_obj_set_pos(circle, x0 + i as i32 * STEP, y0);
                lv_obj_set_style_radius(circle, SIZE / 2, 0);
                lv_obj_set_style_pad_top(circle, 0, 0);
                lv_obj_set_style_pad_bottom(circle, 0, 0);
                lv_obj_set_style_pad_left(circle, 0, 0);
                lv_obj_set_style_pad_right(circle, 0, 0);
                lv_obj_set_style_shadow_width(circle, 0, 0);
                lv_obj_remove_flag(circle, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

                let lbl = lv_label_create(circle);
                let s = CString::new(LABELS[i]).unwrap();
                lv_label_set_text(lbl, s.as_ptr());
                lv_obj_set_style_text_font(lbl, &lv_font_montserrat_14, 0);
                lv_obj_center(lbl);

                Self::paint_circle(circle, lbl, false);

                let tx = Box::into_raw(Box::new(self.bus.commands.clone()));
                lv_obj_add_event_cb(
                    circle,
                    Some(CARD_CBS[i]),
                    lv_event_code_t_LV_EVENT_CLICKED,
                    tx as *mut _,
                );

                self.circles[i]    = circle;
                self.inner_lbls[i] = lbl;
            }

            // ── Detail / sublabel under the row ──────────────────────────────
            let detail = lv_label_create(screen);
            lv_label_set_text(detail, c"".as_ptr());
            lv_obj_set_style_text_color(detail, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(detail, &lv_font_montserrat_12, 0);
            lv_obj_set_width(detail, 360);
            lv_obj_set_style_text_align(detail, lv_text_align_t_LV_TEXT_ALIGN_CENTER, 0);
            lv_obj_align(detail, lv_align_t_LV_ALIGN_TOP_MID, 0, y0 + SIZE + 14);
            self.detail_lbl = detail;

            // ── Done button (64×64 BOTTOM_MID y=-8 — same factory as audio_mode) ─
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

            // Apply initial highlight from current state.
            let snapshot = self.bus.audio.borrow().clone();
            self.update_state(&snapshot);

            screen
        }
    }
}
