//! Recording screen — matches pspapp ui_airecord_screen_on_create layout.
//!
//! Original button positions (absolute x,y in 400×304 space):
//!   Speaker (76×76):  pos(20,  121) → bg_airecord_spk.png  → OpenSettings
//!   Stop    (90×90):  pos(102, 186) → bg_airecord_stop.png → StopRecording
//!   Play    (90×90):  pos(208, 186) → bg_airecord_play.png → AddBookmark
//!   Mic     (76×76):  pos(304, 121) → bg_airecord_mic.png  → ToggleMicMute
//!   Status label: TOP_MID y=54
//!   Elapsed timer:    TOP_MID y=112

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{AudioState, Command, MeetingState, StateBus};

// ── Button event callbacks ────────────────────────────────────────────────────

/// Speaker button → open Settings screen (original: page 0xd volume control).
unsafe extern "C" fn spk_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::OpenSettings);
        }
    }
}

/// Stop button → stop recording.
unsafe extern "C" fn stop_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::StopRecording);
        }
    }
}

/// Play/Mark button → add bookmark.
unsafe extern "C" fn mark_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::AddBookmark);
        }
    }
}

/// Mic button → toggle mic mute.
unsafe extern "C" fn mute_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::ToggleMicMute);
        }
    }
}

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_20: lv_font_t;
}

/// Format seconds as MM:SS (original pspapp airecord style).
pub fn format_elapsed(secs: u32) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m:02}:{s:02}")
}

// ── Struct ────────────────────────────────────────────────────────────────────

pub struct RecordingScreen {
    bus:        Arc<StateBus>,
    lbl_timer:  *mut lv_obj_t,
    img_mic:    *mut lv_obj_t,
    prev_muted: bool,
}

impl RecordingScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            lbl_timer:  std::ptr::null_mut(),
            img_mic:    std::ptr::null_mut(),
            prev_muted: false,
        }
    }

    /// Called every tick; only updates LVGL objects when state changes.
    pub unsafe fn update_state(&mut self, audio: &AudioState, meeting: &MeetingState) {
        // Elapsed timer — update every tick (duration increments each second)
        let s = CString::new(format_elapsed(meeting.duration_secs)).unwrap();
        lv_label_set_text(self.lbl_timer, s.as_ptr());

        // Mic icon — swap on mute change
        if audio.mic_muted != self.prev_muted {
            if !self.img_mic.is_null() {
                let src: &[u8] = if audio.mic_muted {
                    b"A:/etc/pspapp/lvgl/icon/volume/mic_off_red.png\0"
                } else {
                    b"A:/etc/pspapp/lvgl/icon/volume/mic_on.png\0"
                };
                lv_image_set_src(self.img_mic, src.as_ptr() as *const _);
            }
            self.prev_muted = audio.mic_muted;
        }
    }
}

// ── Screen trait impl ─────────────────────────────────────────────────────────

impl super::Screen for RecordingScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            self.prev_muted = false;

            let screen = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_top(screen, 0, 0);
            lv_obj_set_style_pad_bottom(screen, 0, 0);
            lv_obj_set_style_pad_left(screen, 0, 0);
            lv_obj_set_style_pad_right(screen, 0, 0);

            // ── Full-screen background (bg_airecord.png, 400×304) ────────────
            // Set as screen bg_image_src so the image fills the content area.
            lv_obj_set_style_bg_image_src(
                screen,
                b"A:/etc/pspapp/lvgl/icon/msc/bg_airecord.png\0".as_ptr() as *const _,
                0,
            );
            lv_obj_set_style_bg_opa(screen, 255, 0);

            // ── Recording status label (● RECORDING, red, TOP_MID y=54) ──────
            let lbl_status = lv_label_create(screen);
            lv_label_set_text(lbl_status, c"● RECORDING".as_ptr());
            lv_obj_set_style_text_color(lbl_status, rgb(CLR_RED), 0);
            lv_obj_set_style_text_font(lbl_status, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_status, lv_align_t_LV_ALIGN_TOP_MID, 0, 54);

            // ── Elapsed timer "00:00" (white, large, TOP_MID y=112) ──────────
            let lbl_timer = lv_label_create(screen);
            lv_label_set_text(lbl_timer, c"00:00".as_ptr());
            lv_obj_set_style_text_color(lbl_timer, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(lbl_timer, &lv_font_montserrat_20, 0);
            lv_obj_align(lbl_timer, lv_align_t_LV_ALIGN_TOP_MID, 0, 112);
            self.lbl_timer = lbl_timer;

            // ── Speaker button (76×76 at pos 20,121) → OpenSettings ──────────
            let btn_spk = lv_image_create(screen);
            lv_image_set_src(btn_spk,
                b"A:/etc/pspapp/lvgl/icon/msc/bg_airecord_spk.png\0".as_ptr() as *const _);
            lv_obj_set_size(btn_spk, 76, 76);
            lv_obj_set_pos(btn_spk, 20, 121);
            lv_obj_remove_flag(btn_spk, lv_obj_flag_t_LV_OBJ_FLAG_ADV_HITTEST);
            lv_obj_add_flag(btn_spk, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
            let spk_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(btn_spk, Some(spk_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED, spk_tx as *mut _);

            // ── Stop button (90×90 at pos 102,186) → StopRecording ───────────
            let btn_stop = lv_image_create(screen);
            lv_image_set_src(btn_stop,
                b"A:/etc/pspapp/lvgl/icon/msc/bg_airecord_stop.png\0".as_ptr() as *const _);
            lv_obj_set_size(btn_stop, 90, 90);
            lv_obj_set_pos(btn_stop, 102, 186);
            lv_obj_remove_flag(btn_stop, lv_obj_flag_t_LV_OBJ_FLAG_ADV_HITTEST);
            lv_obj_add_flag(btn_stop, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
            let stop_icon = lv_image_create(btn_stop);
            lv_image_set_src(stop_icon,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_stop.png\0".as_ptr() as *const _);
            lv_obj_center(stop_icon);
            let stop_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(btn_stop, Some(stop_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED, stop_tx as *mut _);

            // ── Play/Mark button (90×90 at pos 208,186) → AddBookmark ────────
            let btn_play = lv_image_create(screen);
            lv_image_set_src(btn_play,
                b"A:/etc/pspapp/lvgl/icon/msc/bg_airecord_play.png\0".as_ptr() as *const _);
            lv_obj_set_size(btn_play, 90, 90);
            lv_obj_set_pos(btn_play, 208, 186);
            lv_obj_remove_flag(btn_play, lv_obj_flag_t_LV_OBJ_FLAG_ADV_HITTEST);
            lv_obj_add_flag(btn_play, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
            let play_icon = lv_image_create(btn_play);
            lv_image_set_src(play_icon,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_play.png\0".as_ptr() as *const _);
            lv_obj_center(play_icon);
            let mark_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(btn_play, Some(mark_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED, mark_tx as *mut _);

            // ── Mic button (76×76 at pos 304,121) → ToggleMicMute ────────────
            let btn_mic = lv_image_create(screen);
            lv_image_set_src(btn_mic,
                b"A:/etc/pspapp/lvgl/icon/msc/bg_airecord_mic.png\0".as_ptr() as *const _);
            lv_obj_set_size(btn_mic, 76, 76);
            lv_obj_set_pos(btn_mic, 304, 121);
            lv_obj_remove_flag(btn_mic, lv_obj_flag_t_LV_OBJ_FLAG_ADV_HITTEST);
            lv_obj_add_flag(btn_mic, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
            let img_mic = lv_image_create(btn_mic);
            lv_image_set_src(img_mic,
                b"A:/etc/pspapp/lvgl/icon/volume/mic_on.png\0".as_ptr() as *const _);
            lv_obj_center(img_mic);
            self.img_mic = img_mic;
            let mute_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(btn_mic, Some(mute_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED, mute_tx as *mut _);

            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_elapsed_zero() {
        assert_eq!(format_elapsed(0), "00:00");
    }

    #[test]
    fn format_elapsed_90s() {
        assert_eq!(format_elapsed(90), "01:30");
    }

    #[test]
    fn format_elapsed_overflow() {
        assert_eq!(format_elapsed(3661), "61:01");
    }
}
