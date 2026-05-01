//! Audio Mode selection carousel — matches ui_setting_beamforming_screen_on_create.
//!
//! Layout (400×304):
//!   Carousel 400×210 at TOP_MID y=12  — FLEX_FLOW_ROW, SCROLL_SNAP_CENTER, SCROLL_ONE
//!   5 cards 307×167 each, white bg opa=25, border 2px 0x32e59e, radius=16
//!     Card children:
//!       Radio icon 36×36 at TOP_RIGHT
//!       Mode image 124×124 at pos(16,21)
//!       Mode label width=142 align_to image OUT_RIGHT_TOP +10+10
//!   Indicator strip 160×24 at BOTTOM_MID y=-75
//!     5 check/uncheck icons spaced with OUT_RIGHT_MID +18
//!   Done button (bg_image=btn_done_white.png) 64px at BOTTOM_MID y=-8 → CloseSettings

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use crate::bindings::*;
use crate::theme::*;
use waved_state::{AudioMode, AudioState, Command, StateBus};

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
}

// ── Mode data ─────────────────────────────────────────────────────────────────

const CARD_IMGS: &[&[u8]] = &[
    b"A:/etc/pspapp/lvgl/icon/beamforming/bg_bf_mode_all.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bg_bf_mode_heart.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bg_bf_mode_super_heart.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bg_bf_mode_eight.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bg_bf_mode_stereo.png\0",
];

const LABELS: &[&str] = &["All-around", "Heart", "Super Heart", "Figure 8", "Stereo"];

const INDICATOR_CHECK: &[&[u8]] = &[
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_all_check.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_heart_check.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_super_heart_check.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_eight_check.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_stereo_check.png\0",
];

const INDICATOR_UNCHECK: &[&[u8]] = &[
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_all_uncheck.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_heart_uncheck.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_super_heart_uncheck.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_eight_uncheck.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_stereo_uncheck.png\0",
];

/// Convert our DOA azimuth (signed, counter-clockwise positive, +X = forward)
/// to an `lv_image_set_rotation` argument (clockwise, 0.1° units).
///
/// Quantises to whole degrees so EWMA jitter does not retrigger redraws.
pub(crate) fn az_to_lvgl_rotation(az_deg: f32) -> i32 {
    let wrapped = az_deg.rem_euclid(360.0);
    let clockwise = (360.0 - wrapped).rem_euclid(360.0);
    let whole_deg = clockwise.round() as i32 % 360;
    whole_deg * 10
}

/// Return the index of the centre closest to `midline`. Ties go to the
/// lower index (deterministic). Panics on empty input — caller should
/// always have ≥1 card.
pub(crate) fn nearest_index_to_midline(centres: &[i32], midline: i32) -> usize {
    assert!(!centres.is_empty(), "no carousel children");
    let mut best_idx = 0usize;
    let mut best_dist = (centres[0] - midline).abs();
    for (i, &c) in centres.iter().enumerate().skip(1) {
        let d = (c - midline).abs();
        if d < best_dist {
            best_dist = d;
            best_idx = i;
        }
    }
    best_idx
}

fn mode_to_idx(mode: &AudioMode) -> usize {
    match mode {
        AudioMode::Omni          => 0,
        AudioMode::Cardioid      => 1,
        AudioMode::Supercardioid => 2,
        AudioMode::Figure8       => 3,
        AudioMode::Stereo        => 4,
    }
}

// ── Callbacks ─────────────────────────────────────────────────────────────────

macro_rules! mode_cb {
    ($name:ident, $mode:expr) => {
        unsafe extern "C" fn $name(e: *mut lv_event_t) {
            if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
                let tx = lv_event_get_user_data(e)
                    as *const tokio::sync::broadcast::Sender<Command>;
                if !tx.is_null() {
                    let _ = (*tx).send(Command::SetAudioMode($mode));
                }
            }
        }
    };
}

mode_cb!(mode0_cb, AudioMode::Omni);
mode_cb!(mode1_cb, AudioMode::Cardioid);
mode_cb!(mode2_cb, AudioMode::Supercardioid);
mode_cb!(mode3_cb, AudioMode::Figure8);
mode_cb!(mode4_cb, AudioMode::Stereo);

const CARD_CBS: &[unsafe extern "C" fn(*mut lv_event_t)] = &[
    mode0_cb, mode1_cb, mode2_cb, mode3_cb, mode4_cb,
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

/// LVGL `SCROLL_END` handler on the carousel. user_data is a raw
/// `Arc<AtomicUsize>` (via `Arc::into_raw`) sharing the centered-card
/// index with `AudioModeScreen::update_state`.
unsafe extern "C" fn carousel_scroll_end_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) != lv_event_code_t_LV_EVENT_SCROLL_END {
        return;
    }
    let carousel = lv_event_get_target(e) as *mut lv_obj_t;
    let cell_ptr = lv_event_get_user_data(e) as *const AtomicUsize;
    if carousel.is_null() || cell_ptr.is_null() {
        return;
    }

    let count = lv_obj_get_child_count(carousel) as usize;
    if count == 0 {
        return;
    }
    // `lv_obj_get_x(child)` returns position in the carousel's content
    // frame, already including scroll_x (LVGL pad=0 here). The viewport
    // midline in the same frame is `scroll_x + carousel_width/2` because
    // the carousel spans the full 400px parent (x1 == 0). If the carousel
    // were ever resized or off-centred, this formula would need to add
    // the carousel's parent-relative offset.
    let scroll_x = lv_obj_get_scroll_x(carousel);
    let viewport_mid = scroll_x + lv_obj_get_width(carousel) / 2;

    let mut centres = Vec::with_capacity(count);
    for i in 0..count {
        let child = lv_obj_get_child(carousel, i as i32);
        if child.is_null() { return; }
        let cx = lv_obj_get_x(child) + lv_obj_get_width(child) / 2;
        centres.push(cx);
    }
    let idx = nearest_index_to_midline(&centres, viewport_mid);
    tracing::info!(target: "waved_ui::audio_mode", "carousel centered idx = {}", idx);
    (*cell_ptr).store(idx, Ordering::Relaxed);
}

// ── Struct ────────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
struct DoaCacheKey {
    centered: usize,
    rotation: i32,
    active:   bool,
}

pub struct AudioModeScreen {
    bus:          Arc<StateBus>,
    cards:        [*mut lv_obj_t; 5],
    radios:       [*mut lv_obj_t; 5],
    indicators:   [*mut lv_obj_t; 5],
    doa_arrows:   [*mut lv_obj_t; 5],
    centered_idx: Arc<AtomicUsize>,
    prev_mode:    Option<AudioMode>,
    prev_doa:     Option<DoaCacheKey>,
}

impl AudioModeScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            cards:        [std::ptr::null_mut(); 5],
            radios:       [std::ptr::null_mut(); 5],
            indicators:   [std::ptr::null_mut(); 5],
            doa_arrows:   [std::ptr::null_mut(); 5],
            centered_idx: Arc::new(AtomicUsize::new(0)),
            prev_mode:    None,
            prev_doa:     None,
        }
    }

    /// Called each tick to update radio icons, indicator strip, border, and DOA arrow.
    pub unsafe fn update_state(&mut self, audio: &AudioState) {
        let selected_idx = mode_to_idx(&audio.mode);

        // ── Selection visuals: border, radio icon, indicator strip ─────────────
        if self.prev_mode.as_ref() != Some(&audio.mode) {
            for i in 0..5 {
                if !self.radios[i].is_null() {
                    let src = if i == selected_idx {
                        b"A:/etc/pspapp/lvgl/icon/btn/radio_checked.png\0".as_ptr()
                    } else {
                        b"A:/etc/pspapp/lvgl/icon/btn/radio_unchecked.png\0".as_ptr()
                    };
                    lv_image_set_src(self.radios[i], src as *const _);
                }
                if !self.cards[i].is_null() {
                    let side = if i == selected_idx {
                        lv_border_side_t_LV_BORDER_SIDE_FULL
                    } else {
                        lv_border_side_t_LV_BORDER_SIDE_NONE
                    };
                    lv_obj_set_style_border_side(self.cards[i], side as u32, 0);
                }
                if !self.indicators[i].is_null() {
                    let src = if i == selected_idx {
                        INDICATOR_CHECK[i].as_ptr()
                    } else {
                        INDICATOR_UNCHECK[i].as_ptr()
                    };
                    lv_image_set_src(self.indicators[i], src as *const _);
                }
            }

            // Slide carousel to the newly-selected card so centered tracks
            // selection on every tap. Skipped on the first tick (prev_mode
            // None means we just entered the screen and create() already
            // aligned). During the slide animation, centered_idx still
            // reflects the old card — the DOA arrow stays on it until
            // SCROLL_END fires and the next tick rebuilds the cache key.
            if self.prev_mode.is_some() && !self.cards[selected_idx].is_null() {
                lv_obj_scroll_to_view(
                    self.cards[selected_idx],
                    lv_anim_enable_t_LV_ANIM_ON,
                );
            }

            self.prev_mode = Some(audio.mode.clone());
        }

        // ── DOA arrow on the carousel-centered card ────────────────────────────
        let centered = self.centered_idx.load(Ordering::Relaxed);
        let rotation = az_to_lvgl_rotation(audio.doa_horizontal);
        let key = DoaCacheKey { centered, rotation, active: audio.doa_active };

        if self.prev_doa.as_ref() != Some(&key) {
            for i in 0..5 {
                let arrow = self.doa_arrows[i];
                if arrow.is_null() { continue; }
                let show = i == centered && audio.doa_active;
                if show {
                    lv_image_set_rotation(arrow, rotation);
                    lv_obj_remove_flag(arrow, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                } else {
                    lv_obj_add_flag(arrow, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                }
            }
            self.prev_doa = Some(key);
        }
    }
}

// ── Screen trait impl ─────────────────────────────────────────────────────────

impl super::Screen for AudioModeScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            // Reset state so update_state triggers on first tick.
            self.prev_mode = None;
            self.prev_doa  = None;
            self.cards        = [std::ptr::null_mut(); 5];
            self.radios       = [std::ptr::null_mut(); 5];
            self.indicators   = [std::ptr::null_mut(); 5];
            self.doa_arrows   = [std::ptr::null_mut(); 5];

            let screen = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_top(screen, 0, 0);
            lv_obj_set_style_pad_bottom(screen, 0, 0);
            lv_obj_set_style_pad_left(screen, 0, 0);
            lv_obj_set_style_pad_right(screen, 0, 0);

            // ── Carousel container (400×210, TOP_MID y=12) ───────────────────
            // flex row, snap to center, one-at-a-time scroll, no scrollbar.
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

            let cell_raw = Arc::into_raw(Arc::clone(&self.centered_idx)) as *mut std::ffi::c_void;
            lv_obj_add_event_cb(
                carousel,
                Some(carousel_scroll_end_cb),
                lv_event_code_t_LV_EVENT_SCROLL_END,
                cell_raw,
            );

            // ── 5 mode cards ─────────────────────────────────────────────────
            let current_mode = self.bus.audio.borrow().mode.clone();
            let initial_idx  = mode_to_idx(&current_mode);
            self.centered_idx.store(initial_idx, Ordering::Relaxed);

            for i in 0..5usize {
                // Card: 307×167, white bg opa=25, border 2px 0x32e59e, radius=16.
                let card = lv_obj_create(carousel);
                lv_obj_set_size(card, 307, 167);
                lv_obj_set_style_bg_color(card, rgb(0xffffff), 0);
                lv_obj_set_style_bg_opa(card, 25, 0);
                lv_obj_set_style_border_width(card, 2, 0);
                lv_obj_set_style_border_color(card, rgb(0x32e59e), 0);
                // Initially no border (shown on selected card via update_state).
                lv_obj_set_style_border_side(card, lv_border_side_t_LV_BORDER_SIDE_NONE as u32, 0);
                lv_obj_set_style_radius(card, 16, 0);
                lv_obj_set_style_pad_top(card, 0, 0);
                lv_obj_set_style_pad_bottom(card, 0, 0);
                lv_obj_set_style_pad_left(card, 0, 0);
                lv_obj_set_style_pad_right(card, 0, 0);
                lv_obj_remove_flag(card, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
                lv_obj_add_flag(card, lv_obj_flag_t_LV_OBJ_FLAG_SNAPPABLE);
                lv_obj_add_flag(card, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
                self.cards[i] = card;

                // Radio icon: 36×36 at TOP_RIGHT.
                let radio = lv_image_create(card);
                lv_obj_set_size(radio, 36, 36);
                lv_obj_align(radio, lv_align_t_LV_ALIGN_TOP_RIGHT, 0, 0);
                lv_image_set_src(radio,
                    b"A:/etc/pspapp/lvgl/icon/btn/radio_unchecked.png\0".as_ptr() as *const _);
                lv_obj_remove_flag(radio, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
                self.radios[i] = radio;

                // Mode background image: 124×124 at pos(16,21).
                let mode_img = lv_image_create(card);
                lv_obj_set_size(mode_img, 124, 124);
                lv_obj_set_pos(mode_img, 16, 21);
                lv_image_set_src(mode_img, CARD_IMGS[i].as_ptr() as *const _);
                lv_obj_remove_flag(mode_img, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

                // DOA arrow overlay — child of the polar-pattern bg image. Initially
                // hidden; shown/rotated by update_state on the centered card only.
                let arrow = lv_image_create(mode_img);
                lv_obj_set_size(arrow, 124, 124);
                lv_obj_set_pos(arrow, 0, 0);
                lv_image_set_src(
                    arrow,
                    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_doa.png\0".as_ptr() as *const _,
                );
                lv_image_set_pivot(arrow, 62, 62);
                lv_obj_remove_flag(arrow, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
                lv_obj_add_flag(arrow, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                self.doa_arrows[i] = arrow;

                // Mode name label: width=142, aligned OUT_RIGHT_TOP of mode_img +10,+10.
                let lbl = lv_label_create(card);
                lv_obj_set_style_text_color(lbl, rgb(0xffffff), 0);
                lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
                lv_obj_set_width(lbl, 142);
                let s = std::ffi::CString::new(LABELS[i]).unwrap();
                lv_label_set_text(lbl, s.as_ptr());
                lv_obj_align_to(lbl, mode_img,
                    lv_align_t_LV_ALIGN_OUT_RIGHT_TOP, 10, 10);
                lv_obj_remove_flag(lbl, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

                // Card click → SetAudioMode.
                let tx = Box::into_raw(Box::new(self.bus.commands.clone()));
                lv_obj_add_event_cb(card, Some(CARD_CBS[i]),
                    lv_event_code_t_LV_EVENT_CLICKED, tx as *mut _);
            }

            // Scroll carousel to show the initially-selected card.
            if !self.cards[initial_idx].is_null() {
                lv_obj_scroll_to_view(
                    self.cards[initial_idx],
                    lv_anim_enable_t_LV_ANIM_OFF,
                );
            }

            // ── Indicator strip (160×24, BOTTOM_MID y=-75) ───────────────────
            let indicator = lv_obj_create(screen);
            lv_obj_set_size(indicator, 160, 24);
            lv_obj_align(indicator, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -75);
            lv_obj_set_style_bg_opa(indicator, 0, 0);
            lv_obj_set_style_border_width(indicator, 0, 0);
            lv_obj_set_style_pad_top(indicator, 0, 0);
            lv_obj_set_style_pad_bottom(indicator, 0, 0);
            lv_obj_set_style_pad_left(indicator, 0, 0);
            lv_obj_set_style_pad_right(indicator, 0, 0);
            lv_obj_remove_flag(indicator, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

            // Icon[0]: align LEFT_MID; icons[1-4]: OUT_RIGHT_MID +18 from previous.
            let icon0 = lv_image_create(indicator);
            lv_image_set_src(icon0,
                INDICATOR_UNCHECK[0].as_ptr() as *const _);
            lv_obj_align(icon0, lv_align_t_LV_ALIGN_LEFT_MID, 0, 0);
            self.indicators[0] = icon0;

            let mut prev_icon = icon0;
            for i in 1..5usize {
                let icon = lv_image_create(indicator);
                lv_image_set_src(icon, INDICATOR_UNCHECK[i].as_ptr() as *const _);
                lv_obj_align_to(icon, prev_icon,
                    lv_align_t_LV_ALIGN_OUT_RIGHT_MID, 18, 0);
                self.indicators[i] = icon;
                prev_icon = icon;
            }

            // ── Done button (bg_image btn_done_white.png, 64px, BOTTOM_MID y=-8) ─
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
            lv_obj_add_event_cb(btn_done, Some(done_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED, done_tx as *mut _);

            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn az_zero_maps_to_zero() {
        assert_eq!(az_to_lvgl_rotation(0.0), 0);
    }

    #[test]
    fn az_positive_90_maps_to_2700() {
        // +90° (left in our frame) → LVGL clockwise 270°
        assert_eq!(az_to_lvgl_rotation(90.0), 2700);
    }

    #[test]
    fn az_negative_90_maps_to_900() {
        assert_eq!(az_to_lvgl_rotation(-90.0), 900);
    }

    #[test]
    fn az_180_maps_to_1800() {
        assert_eq!(az_to_lvgl_rotation(180.0), 1800);
    }

    #[test]
    fn az_negative_180_maps_to_1800() {
        assert_eq!(az_to_lvgl_rotation(-180.0), 1800);
    }

    #[test]
    fn az_45_maps_to_3150() {
        assert_eq!(az_to_lvgl_rotation(45.0), 3150);
    }

    #[test]
    fn az_359_4_maps_to_10() {
        // 359.4° → wrapped=359.4 → clockwise=0.6 → round=1 → 10
        assert_eq!(az_to_lvgl_rotation(359.4), 10);
    }

    #[test]
    fn az_negative_0_4_maps_to_0() {
        // -0.4° → wrapped=359.6 → clockwise=0.4 → round=0 → 0
        assert_eq!(az_to_lvgl_rotation(-0.4), 0);
    }

    #[test]
    fn az_0_4_round_clamps_to_zero() {
        // 0.4° → wrapped=0.4 → clockwise=359.6 → round=360 → 360 % 360 = 0
        // The `% 360` guard is what prevents the function returning 3600.
        assert_eq!(az_to_lvgl_rotation(0.4), 0);
    }

    #[test]
    fn nearest_single_child_returns_zero() {
        assert_eq!(nearest_index_to_midline(&[100], 200), 0);
    }

    #[test]
    fn nearest_three_children_picks_closest_to_midline() {
        // Three card centres at 50, 200, 350; midline at 220 → centre #1 is closest.
        assert_eq!(nearest_index_to_midline(&[50, 200, 350], 220), 1);
    }

    #[test]
    fn nearest_picks_lower_index_on_tie() {
        // Two equidistant from midline → return the lower index, deterministically.
        assert_eq!(nearest_index_to_midline(&[100, 300], 200), 0);
    }

    #[test]
    fn nearest_works_when_midline_left_of_all() {
        assert_eq!(nearest_index_to_midline(&[100, 200, 300], 0), 0);
    }

    #[test]
    fn nearest_works_when_midline_right_of_all() {
        assert_eq!(nearest_index_to_midline(&[100, 200, 300], 1000), 2);
    }

    #[test]
    #[should_panic(expected = "no carousel children")]
    fn nearest_panics_on_empty() {
        nearest_index_to_midline(&[], 0);
    }
}
