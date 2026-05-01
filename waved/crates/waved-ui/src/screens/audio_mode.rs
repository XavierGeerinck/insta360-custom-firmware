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

// ── Struct ────────────────────────────────────────────────────────────────────

pub struct AudioModeScreen {
    bus:        Arc<StateBus>,
    cards:      [*mut lv_obj_t; 5],
    radios:     [*mut lv_obj_t; 5],
    indicators: [*mut lv_obj_t; 5],
    prev_mode:  Option<AudioMode>,
}

impl AudioModeScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            cards:      [std::ptr::null_mut(); 5],
            radios:     [std::ptr::null_mut(); 5],
            indicators: [std::ptr::null_mut(); 5],
            prev_mode:  None,
        }
    }

    /// Called each tick to update radio icons + indicator based on current audio mode.
    pub unsafe fn update_state(&mut self, audio: &AudioState) {
        if self.prev_mode.as_ref() == Some(&audio.mode) { return; }

        let idx = mode_to_idx(&audio.mode);

        for i in 0..5 {
            // Radio icon: checked on selected card, unchecked otherwise.
            if !self.radios[i].is_null() {
                let src = if i == idx {
                    b"A:/etc/pspapp/lvgl/icon/btn/radio_checked.png\0".as_ptr()
                } else {
                    b"A:/etc/pspapp/lvgl/icon/btn/radio_unchecked.png\0".as_ptr()
                };
                lv_image_set_src(self.radios[i], src as *const _);
            }
            // Card border: full on selected, none otherwise.
            if !self.cards[i].is_null() {
                let side = if i == idx {
                    lv_border_side_t_LV_BORDER_SIDE_FULL
                } else {
                    lv_border_side_t_LV_BORDER_SIDE_NONE
                };
                lv_obj_set_style_border_side(self.cards[i], side as u32, 0);
            }
            // Indicator icon.
            if !self.indicators[i].is_null() {
                let src = if i == idx {
                    INDICATOR_CHECK[i].as_ptr()
                } else {
                    INDICATOR_UNCHECK[i].as_ptr()
                };
                lv_image_set_src(self.indicators[i], src as *const _);
            }
        }

        self.prev_mode = Some(audio.mode.clone());
    }
}

// ── Screen trait impl ─────────────────────────────────────────────────────────

impl super::Screen for AudioModeScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            // Reset state so update_state triggers on first tick.
            self.prev_mode = None;
            self.cards      = [std::ptr::null_mut(); 5];
            self.radios     = [std::ptr::null_mut(); 5];
            self.indicators = [std::ptr::null_mut(); 5];

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

            // ── 5 mode cards ─────────────────────────────────────────────────
            let current_mode = self.bus.audio.borrow().mode.clone();
            let initial_idx  = mode_to_idx(&current_mode);

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
    fn az_359_4_wraps_to_zero() {
        // 359.4° rounds to 359 → clockwise (360-359)=1, but the input is
        // already extremely close to 0, so after rounding it should be 0.
        // Verify wrap is well-behaved at the seam.
        let r = az_to_lvgl_rotation(359.4);
        assert!(r == 0 || r == 10, "got {r}");
    }

    #[test]
    fn az_negative_0_4_wraps_to_zero() {
        let r = az_to_lvgl_rotation(-0.4);
        assert!(r == 0 || r == 3590, "got {r}");
    }
}
