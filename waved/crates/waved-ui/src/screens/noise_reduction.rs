//! Noise Reduction strength selector — SP#4.
//!
//! Mirrors `screens::audio_mode` UI pattern:
//!   * 400×210 carousel at TOP_MID y=12, FLEX_FLOW_ROW + SCROLL_SNAP_CENTER
//!   * 4 cards 307×167 (Off / Low / Medium / High) — text-only since no
//!     NR-specific asset pack exists. Card shows the level name (font 14
//!     accent) + a dB-cap sublabel (font 12 dim). Radio at TOP_RIGHT.
//!   * Indicator strip at BOTTOM_MID y=-75 — 4 small dots, active filled
//!     accent.
//!   * Done button 64×64 at BOTTOM_MID y=-8 → `Command::CloseSettings`.
//!
//! Tap a card → `Command::SetNrLevel(level)`. The strength → DFN3
//! attenuation cap mapping lives in `waved_audio::nr_to_atten_lim_db`
//! (Off→bypass, Low→12 dB, Med→24 dB, High→100 dB).

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{AudioState, Command, NrLevel, StateBus};

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_14: lv_font_t;
}

const LABELS:    [&str; 4] = ["Off",          "Low",        "Medium",     "High"];
const SUBLABELS: [&str; 4] = ["no neural NS", "-12 dB cap", "-24 dB cap", "full attenuation"];

fn level_to_idx(level: &NrLevel) -> usize {
    match level {
        NrLevel::Off  => 0,
        NrLevel::Low  => 1,
        NrLevel::Med  => 2,
        NrLevel::High => 3,
    }
}

// ── Per-card click callbacks (mirrors audio_mode.rs `mode_cb!` macro) ─────────

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
    cards:      [*mut lv_obj_t; 4],
    radios:     [*mut lv_obj_t; 4],
    name_lbls:  [*mut lv_obj_t; 4],
    indicators: [*mut lv_obj_t; 4],
    prev_level: Option<NrLevel>,
}

impl NoiseReductionScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            cards:      [std::ptr::null_mut(); 4],
            radios:     [std::ptr::null_mut(); 4],
            name_lbls:  [std::ptr::null_mut(); 4],
            indicators: [std::ptr::null_mut(); 4],
            prev_level: None,
        }
    }

    /// Refresh radio + border + label colour + indicator dots.
    pub unsafe fn update_state(&mut self, audio: &AudioState) {
        if self.prev_level.as_ref() == Some(&audio.nr_level) {
            return;
        }
        let idx = level_to_idx(&audio.nr_level);

        for i in 0..4usize {
            let active = i == idx;

            // Card border: full on selected, none otherwise.
            if !self.cards[i].is_null() {
                let side = if active {
                    lv_border_side_t_LV_BORDER_SIDE_FULL
                } else {
                    lv_border_side_t_LV_BORDER_SIDE_NONE
                };
                lv_obj_set_style_border_side(self.cards[i], side as u32, 0);
            }

            // Radio: checked / unchecked.
            if !self.radios[i].is_null() {
                let src: &[u8] = if active {
                    b"A:/etc/pspapp/lvgl/icon/btn/radio_checked.png\0"
                } else {
                    b"A:/etc/pspapp/lvgl/icon/btn/radio_unchecked.png\0"
                };
                lv_image_set_src(self.radios[i], src.as_ptr() as *const _);
            }

            // Name label: accent on active, white on inactive.
            if !self.name_lbls[i].is_null() {
                let clr = if active { rgb(CLR_ACCENT) } else { rgb(CLR_WHITE) };
                lv_obj_set_style_text_color(self.name_lbls[i], clr, 0);
            }

            // Indicator dot: filled accent on active, dim outline otherwise.
            if !self.indicators[i].is_null() {
                if active {
                    lv_obj_set_style_bg_color(self.indicators[i], rgb(CLR_ACCENT), 0);
                    lv_obj_set_style_bg_opa(self.indicators[i], 255, 0);
                    lv_obj_set_style_border_color(self.indicators[i], rgb(CLR_ACCENT), 0);
                } else {
                    lv_obj_set_style_bg_opa(self.indicators[i], 0, 0);
                    lv_obj_set_style_border_color(self.indicators[i], rgb(CLR_TEXT_DIM), 0);
                }
            }
        }
        self.prev_level = Some(audio.nr_level.clone());
    }
}

impl super::Screen for NoiseReductionScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            self.prev_level = None;
            self.cards      = [std::ptr::null_mut(); 4];
            self.radios     = [std::ptr::null_mut(); 4];
            self.name_lbls  = [std::ptr::null_mut(); 4];
            self.indicators = [std::ptr::null_mut(); 4];

            let screen = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_top(screen, 0, 0);
            lv_obj_set_style_pad_bottom(screen, 0, 0);
            lv_obj_set_style_pad_left(screen, 0, 0);
            lv_obj_set_style_pad_right(screen, 0, 0);

            // ── Carousel (400×210, TOP_MID y=12) ─────────────────────────────
            let carousel = lv_obj_create(screen);
            lv_obj_set_size(carousel, 400, 210);
            lv_obj_align(carousel, lv_align_t_LV_ALIGN_TOP_MID, 0, 12);
            lv_obj_set_style_bg_opa(carousel, 0, 0);
            lv_obj_set_style_border_width(carousel, 0, 0);
            lv_obj_set_style_pad_top(carousel, 0, 0);
            lv_obj_set_style_pad_bottom(carousel, 0, 0);
            lv_obj_set_style_pad_left(carousel, 0, 0);
            lv_obj_set_style_pad_right(carousel, 0, 0);
            lv_obj_set_flex_flow(carousel, lv_flex_flow_t_LV_FLEX_FLOW_ROW);
            lv_obj_set_flex_align(
                carousel,
                lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            );
            lv_obj_set_scroll_snap_x(carousel, lv_scroll_snap_t_LV_SCROLL_SNAP_CENTER);
            lv_obj_set_scrollbar_mode(carousel, lv_scrollbar_mode_t_LV_SCROLLBAR_MODE_OFF);
            lv_obj_set_style_pad_column(carousel, 16, 0);
            lv_obj_add_flag(carousel, lv_obj_flag_t_LV_OBJ_FLAG_SCROLL_ONE);
            lv_obj_set_scroll_dir(carousel, lv_dir_t_LV_DIR_HOR);

            // ── 4 cards ──────────────────────────────────────────────────────
            let current_level = self.bus.audio.borrow().nr_level.clone();
            let initial_idx   = level_to_idx(&current_level);

            for i in 0..4usize {
                let card = lv_obj_create(carousel);
                lv_obj_set_size(card, 307, 167);
                lv_obj_set_style_bg_color(card, rgb(0xffffff), 0);
                lv_obj_set_style_bg_opa(card, 25, 0);
                lv_obj_set_style_border_width(card, 2, 0);
                lv_obj_set_style_border_color(card, rgb(0x32e59e), 0);
                lv_obj_set_style_border_side(
                    card,
                    lv_border_side_t_LV_BORDER_SIDE_NONE as u32,
                    0,
                );
                lv_obj_set_style_radius(card, 16, 0);
                lv_obj_set_style_pad_top(card, 0, 0);
                lv_obj_set_style_pad_bottom(card, 0, 0);
                lv_obj_set_style_pad_left(card, 24, 0);
                lv_obj_set_style_pad_right(card, 24, 0);
                lv_obj_remove_flag(card, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
                lv_obj_add_flag(card, lv_obj_flag_t_LV_OBJ_FLAG_SNAPPABLE);
                lv_obj_add_flag(card, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
                self.cards[i] = card;

                // Radio at TOP_RIGHT.
                let radio = lv_image_create(card);
                lv_obj_set_size(radio, 36, 36);
                lv_obj_align(radio, lv_align_t_LV_ALIGN_TOP_RIGHT, 0, 0);
                lv_image_set_src(
                    radio,
                    b"A:/etc/pspapp/lvgl/icon/btn/radio_unchecked.png\0".as_ptr() as *const _,
                );
                lv_obj_remove_flag(radio, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
                self.radios[i] = radio;

                // Name label, big.
                let lbl = lv_label_create(card);
                let s = CString::new(LABELS[i]).unwrap();
                lv_label_set_text(lbl, s.as_ptr());
                lv_obj_set_style_text_color(lbl, rgb(CLR_WHITE), 0);
                lv_obj_set_style_text_font(lbl, &lv_font_montserrat_14, 0);
                lv_obj_align(lbl, lv_align_t_LV_ALIGN_LEFT_MID, 0, -10);
                self.name_lbls[i] = lbl;

                // Sublabel (dB cap), dim.
                let sub = lv_label_create(card);
                let ss = CString::new(SUBLABELS[i]).unwrap();
                lv_label_set_text(sub, ss.as_ptr());
                lv_obj_set_style_text_color(sub, rgb(CLR_TEXT_DIM), 0);
                lv_obj_set_style_text_font(sub, &lv_font_montserrat_12, 0);
                lv_obj_align(sub, lv_align_t_LV_ALIGN_LEFT_MID, 0, 12);

                // Click → SetNrLevel(...).
                let tx = Box::into_raw(Box::new(self.bus.commands.clone()));
                lv_obj_add_event_cb(
                    card,
                    Some(CARD_CBS[i]),
                    lv_event_code_t_LV_EVENT_CLICKED,
                    tx as *mut _,
                );
            }

            // Snap to the active card on entry.
            if !self.cards[initial_idx].is_null() {
                lv_obj_scroll_to_view(
                    self.cards[initial_idx],
                    lv_anim_enable_t_LV_ANIM_OFF,
                );
            }

            // ── Indicator strip (4 dots, BOTTOM_MID y=-75) ───────────────────
            // Mirrors audio_mode's indicator strip but with simple LV-drawn
            // dots instead of asset images (we don't have NR-specific art).
            let strip = lv_obj_create(screen);
            lv_obj_set_size(strip, 100, 16);
            lv_obj_align(strip, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -75);
            lv_obj_set_style_bg_opa(strip, 0, 0);
            lv_obj_set_style_border_width(strip, 0, 0);
            lv_obj_set_style_pad_top(strip, 0, 0);
            lv_obj_set_style_pad_bottom(strip, 0, 0);
            lv_obj_set_style_pad_left(strip, 0, 0);
            lv_obj_set_style_pad_right(strip, 0, 0);
            lv_obj_remove_flag(strip, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

            let mut prev_dot: *mut lv_obj_t = std::ptr::null_mut();
            for i in 0..4usize {
                let dot = lv_obj_create(strip);
                lv_obj_set_size(dot, 10, 10);
                lv_obj_set_style_radius(dot, 5, 0);
                lv_obj_set_style_bg_opa(dot, 0, 0);
                lv_obj_set_style_border_width(dot, 2, 0);
                lv_obj_set_style_border_color(dot, rgb(CLR_TEXT_DIM), 0);
                lv_obj_set_style_pad_top(dot, 0, 0);
                lv_obj_set_style_pad_bottom(dot, 0, 0);
                lv_obj_set_style_pad_left(dot, 0, 0);
                lv_obj_set_style_pad_right(dot, 0, 0);
                lv_obj_remove_flag(dot, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
                lv_obj_remove_flag(dot, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
                if prev_dot.is_null() {
                    lv_obj_align(dot, lv_align_t_LV_ALIGN_LEFT_MID, 0, 0);
                } else {
                    lv_obj_align_to(dot, prev_dot, lv_align_t_LV_ALIGN_OUT_RIGHT_MID, 18, 0);
                }
                self.indicators[i] = dot;
                prev_dot = dot;
            }

            // ── Done button (64×64, BOTTOM_MID y=-8) — same factory as audio_mode ─
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
