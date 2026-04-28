//! Noise Reduction strength selector — SP#4.
//!
//! 4 circular buttons in a row (Off / Low / Med / High). Tap the one you
//! want — selected circle fills with accent colour, others stay dim.
//! The dB-cap sublabel under the row tells you what each level does.
//!
//! ## Optimistic UI
//!
//! Audio-service rebuild of the DSP chain takes ~1.5 s (tract session
//! creation), so we don't wait for the AudioState round-trip to update the
//! visual. The click callback paints the new selection immediately and
//! marks the level as `pending`. While `pending` is `Some`, `update_state`
//! ignores `audio.nr_level` (still showing the pre-tap value) and keeps
//! the optimistic paint. Once the audio thread catches up
//! (`audio.nr_level == pending`), `pending` clears and normal sync
//! resumes.
//!
//! Layout (400×304):
//!   y=44   Title  "NOISE REDUCTION"  (clears 32 px status bar + 12 px gap)
//!   y=88   Row of 4 circles (76 px Ø, 16 px gap, 24 px side margin)
//!   y=178  Sublabel for the active level
//!   y=-20  Done button (BOTTOM_MID), check_circle.png

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

const DETAIL: [&str; 4] = [
    "Off - speex AEC only, no neural NS",
    "Low - -12 dB cap, room tone preserved",
    "Med - -24 dB cap, moderate suppression",
    "High - full DFN3 attenuation",
];

fn idx_to_level(idx: usize) -> NrLevel {
    match idx {
        0 => NrLevel::Off,
        1 => NrLevel::Low,
        2 => NrLevel::Med,
        _ => NrLevel::High,
    }
}

fn level_to_idx(level: &NrLevel) -> usize {
    match level {
        NrLevel::Off  => 0,
        NrLevel::Low  => 1,
        NrLevel::Med  => 2,
        NrLevel::High => 3,
    }
}

// ── Click-callback user_data ──────────────────────────────────────────────────
//
// Carries the broadcast Sender, the back-pointer to the screen for optimistic
// repaint, and the level index. The screen lives on the UI-thread stack for
// the process lifetime and never moves, so the raw pointer is always valid
// inside the callback.

struct ClickData {
    tx:     tokio::sync::broadcast::Sender<Command>,
    screen: *mut NoiseReductionScreen,
    idx:    usize,
}

unsafe extern "C" fn level_click_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) != lv_event_code_t_LV_EVENT_CLICKED {
        return;
    }
    let data = lv_event_get_user_data(e) as *const ClickData;
    if data.is_null() { return; }
    let d = &*data;

    // Optimistic UI — paint before round-tripping through the audio service.
    if !d.screen.is_null() {
        (*d.screen).paint_optimistic(d.idx);
    }

    let _ = d.tx.send(Command::SetNrLevel(idx_to_level(d.idx)));
}

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
    /// Last level we painted. Tracks `audio.nr_level` once `pending` is
    /// `None`; while `pending` is `Some`, this mirrors `pending`.
    prev_level: Option<NrLevel>,
    /// Set when the user taps a button — survives across UI ticks until
    /// the audio service confirms (`audio.nr_level == pending`). Prevents
    /// the optimistic paint from being overwritten during the ~1.5 s
    /// tract-rebuild window.
    pending: Option<NrLevel>,
}

impl NoiseReductionScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            circles:    [std::ptr::null_mut(); 4],
            inner_lbls: [std::ptr::null_mut(); 4],
            detail_lbl: std::ptr::null_mut(),
            prev_level: None,
            pending:    None,
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

    /// Repaint all circles + the detail label for a given selected idx.
    /// Caller is responsible for setting `prev_level` / `pending`.
    unsafe fn paint(&self, idx: usize) {
        for i in 0..4usize {
            if self.circles[i].is_null() { continue; }
            Self::paint_circle(self.circles[i], self.inner_lbls[i], i == idx);
        }
        if !self.detail_lbl.is_null() {
            let s = CString::new(DETAIL[idx]).unwrap();
            lv_label_set_text(self.detail_lbl, s.as_ptr());
        }
    }

    /// Optimistic-paint hook called from the click callback. Marks the
    /// level as pending; subsequent `update_state` ticks will hold this
    /// paint until the audio thread confirms.
    unsafe fn paint_optimistic(&mut self, idx: usize) {
        self.paint(idx);
        let level = idx_to_level(idx);
        self.prev_level = Some(level.clone());
        self.pending    = Some(level);
    }

    /// Called every UI tick. While `pending` is `Some`, ignores
    /// `audio.nr_level` and keeps the optimistic paint until the audio
    /// state catches up.
    pub unsafe fn update_state(&mut self, audio: &AudioState) {
        if let Some(p) = self.pending.clone() {
            if audio.nr_level == p {
                // Audio caught up — clear pending and resume normal sync.
                self.pending    = None;
                self.prev_level = Some(audio.nr_level.clone());
            }
            // Either way: don't repaint. Visual already shows `pending`.
            return;
        }
        if self.prev_level.as_ref() == Some(&audio.nr_level) {
            return;
        }
        self.paint(level_to_idx(&audio.nr_level));
        self.prev_level = Some(audio.nr_level.clone());
    }
}

impl super::Screen for NoiseReductionScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            self.prev_level = None;
            self.pending    = None;
            self.circles    = [std::ptr::null_mut(); 4];
            self.inner_lbls = [std::ptr::null_mut(); 4];
            let screen_ptr: *mut Self = self as *mut Self;

            let screen = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_top(screen, 0, 0);
            lv_obj_set_style_pad_bottom(screen, 0, 0);
            lv_obj_set_style_pad_left(screen, 0, 0);
            lv_obj_set_style_pad_right(screen, 0, 0);

            // ── Title ─ y=44 clears the 32 px status bar + 12 px gap ────────
            let title = lv_label_create(screen);
            lv_label_set_text(title, c"NOISE REDUCTION".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_14, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 44);

            // ── Row of 4 circles ─────────────────────────────────────────────
            const SIZE: i32 = 76;
            const GAP:  i32 = 16;
            const STEP: i32 = SIZE + GAP;
            let x0: i32 = (400 - (4 * SIZE + 3 * GAP)) / 2;
            let y0: i32 = 88;

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

                let click_data = Box::into_raw(Box::new(ClickData {
                    tx:     self.bus.commands.clone(),
                    screen: screen_ptr,
                    idx:    i,
                }));
                lv_obj_add_event_cb(
                    circle,
                    Some(level_click_cb),
                    lv_event_code_t_LV_EVENT_CLICKED,
                    click_data as *mut _,
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

            // ── Done button ─────────────────────────────────────────────────
            // `check_circle.png` has a visible green-on-white check; the
            // earlier `btn_done_white.png` rendered as a white blob on the
            // dark screen background. y=-20 lifts it off the bottom edge.
            let btn_done = lv_obj_create(screen);
            lv_obj_set_size(btn_done, 64, 64);
            lv_obj_align(btn_done, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -20);
            lv_obj_set_style_bg_image_src(
                btn_done,
                b"A:/etc/pspapp/lvgl/icon/btn/check_circle.png\0".as_ptr() as *const _,
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
