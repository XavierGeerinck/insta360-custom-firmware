//! Home screen — MAIN (ID 10) matching the original pspapp layout.
//!
//! Reference: decompiled ui_Main_screen_on_create (libui.so.c line 14310).
//!
//! Original positions (in user-visible / LVGL space, full-screen content 400×304):
//!   Vol  (bg_main_spk_btn.png 76×76):   TOP_LEFT  x=20,  y=122  → centre (58, 160)
//!   Mic  (bg_main_mic_btn.png 76×76):   TOP_LEFT  x=304, y=122  → centre (342, 160)
//!   Rec  (bg_main_record_btn.png 180×76):TOP_MID   x=0,   y=160  → centre (200, 198)
//!   Connection pill: top-right, below status bar
//!   Bottom bar: Mode | Webcam | Files | Settings  at y≈288
//!
//! Our LV_ALIGN_CENTER offsets from screen centre (200, 152):
//!   Vol: (-142, +8)   Mic: (+142, +8)   Rec: (0, +46)

use crate::bindings::*;
use crate::theme::*;
use std::ffi::CString;
use std::sync::Arc;
use waved_state::{AudioState, BluetoothState, Command, StateBus, WifiState};

// ── Button event callbacks ────────────────────────────────────────────────────

/// Mic button clicked → toggle mic mute.
unsafe extern "C" fn mic_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e) as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::ToggleMicMute);
        }
    }
}

/// Record button clicked → start recording.
unsafe extern "C" fn rec_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e) as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::StartRecording);
        }
    }
}

/// Vol button → open Speaker Volume screen (pspapp page 0xd).
unsafe extern "C" fn vol_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e) as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::OpenVolume);
        }
    }
}

/// Bottom "More" button → navigate to Settings screen (page 0xb equivalent).
unsafe extern "C" fn more_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e) as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::OpenSettings);
        }
    }
}

/// Noise Reduction shortcut → navigate to NR screen (SP#4).
unsafe extern "C" fn nr_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e) as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::OpenNoiseReduction);
        }
    }
}

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_14: lv_font_t;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn connection_text(bt: bool, wifi: bool) -> &'static str {
    if bt {
        "BT Connected"
    } else if wifi {
        "WiFi Connected"
    } else {
        "Not Connected"
    }
}

// ── Struct ────────────────────────────────────────────────────────────────────

pub struct HomeScreen {
    bus: Arc<StateBus>,
    /// Small connection status label below the status bar.
    lbl_status: *mut lv_obj_t,
    /// Mute label inside the mic button — "Mute" / "MUTED" in red.
    lbl_mute: *mut lv_obj_t,
    /// Mic icon inside the mic button — swapped when muted.
    img_mic: *mut lv_obj_t,
    // Change detection
    prev_bt: bool,
    prev_wifi: bool,
    prev_muted: bool,
}

impl HomeScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            lbl_status: std::ptr::null_mut(),
            lbl_mute: std::ptr::null_mut(),
            img_mic: std::ptr::null_mut(),
            prev_bt: false,
            prev_wifi: false,
            prev_muted: false,
        }
    }

    /// Called every tick; only updates LVGL objects when state changes.
    pub unsafe fn update_state(
        &mut self,
        audio: &AudioState,
        bt: &BluetoothState,
        wifi: &WifiState,
    ) {
        // Connection status label
        if bt.connected != self.prev_bt || wifi.connected != self.prev_wifi {
            let text = connection_text(bt.connected, wifi.connected);
            let s = CString::new(text).unwrap();
            lv_label_set_text(self.lbl_status, s.as_ptr());
            let color = if bt.connected || wifi.connected {
                rgb(0x00c85a) // green
            } else {
                rgb(CLR_TEXT_DIM)
            };
            lv_obj_set_style_text_color(self.lbl_status, color, 0);
            self.prev_bt = bt.connected;
            self.prev_wifi = wifi.connected;
        }

        // Mic mute state
        if audio.mic_muted != self.prev_muted {
            if audio.mic_muted {
                lv_label_set_text(self.lbl_mute, c"MUTED".as_ptr());
                lv_obj_set_style_text_color(self.lbl_mute, rgb(CLR_RED), 0);
                lv_image_set_src(
                    self.img_mic,
                    b"A:/etc/pspapp/lvgl/icon/volume/mic_off_red.png\0".as_ptr() as *const _,
                );
            } else {
                lv_label_set_text(self.lbl_mute, c"Mute".as_ptr());
                lv_obj_set_style_text_color(self.lbl_mute, rgb(CLR_TEXT_DIM), 0);
                lv_image_set_src(
                    self.img_mic,
                    b"A:/etc/pspapp/lvgl/icon/volume/mic_on.png\0".as_ptr() as *const _,
                );
            }
            self.prev_muted = audio.mic_muted;
        }
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

            // ── Full-screen background (bg_main.png, 400×304) ────────────────
            // Set as screen bg_image_src (same as pspapp style_manager 0x1b) so
            // the image fills the screen content area without child-object
            // alignment timing issues.
            lv_obj_set_style_bg_image_src(
                screen,
                b"A:/etc/pspapp/lvgl/icon/msc/bg_main.png\0".as_ptr() as *const _,
                0,
            );
            lv_obj_set_style_bg_opa(screen, 255, 0);

            // ── Connection status label ───────────────────────────────────────
            // Positioned just below the 32 px status bar (y ≈ 40).
            // Shows "Not Connected" / "BT Connected" / "WiFi Connected".
            // Original: small pill at TOP_RIGHT of content; we use a centred label.
            let lbl_status = lv_label_create(screen);
            lv_label_set_text(lbl_status, c"Not Connected".as_ptr());
            lv_obj_set_style_text_color(lbl_status, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(lbl_status, &lv_font_montserrat_12, 0);
            // CENTER y_ofs = 40 − 152 = −112
            lv_obj_align(lbl_status, lv_align_t_LV_ALIGN_CENTER, 0, -112);
            self.lbl_status = lbl_status;

            // ── Noise Reduction shortcut pill (100×32 at center offset 0,-65) ─
            // Sits between the connection label (y=40) and the vol/mic row (y=160).
            // Tap → Command::OpenNoiseReduction → NoiseReductionScreen.
            let btn_nr = lv_button_create(screen);
            lv_obj_set_size(btn_nr, 120, 32);
            lv_obj_align(btn_nr, lv_align_t_LV_ALIGN_CENTER, 0, -65);
            lv_obj_set_style_radius(btn_nr, 16, 0);
            lv_obj_set_style_bg_color(btn_nr, rgb(CLR_SURFACE), 0);
            lv_obj_set_style_bg_opa(btn_nr, 200, 0);
            lv_obj_set_style_border_width(btn_nr, 1, 0);
            lv_obj_set_style_border_color(btn_nr, rgb(CLR_ACCENT), 0);
            let lbl_nr = lv_label_create(btn_nr);
            lv_label_set_text(lbl_nr, c"Noise Reduction".as_ptr());
            lv_obj_set_style_text_color(lbl_nr, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_text_font(lbl_nr, &lv_font_montserrat_12, 0);
            lv_obj_center(lbl_nr);
            let nr_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(
                btn_nr,
                Some(nr_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                nr_tx as *mut _,
            );

            // ── Vol / Speaker button (76×76) ──────────────────────────────────
            // Original: TOP_LEFT x=20, y=122 → centre (58, 160)
            // CENTER offset: (58−200, 160−152) = (−142, +8)
            let btn_spk = lv_image_create(screen);
            lv_image_set_src(
                btn_spk,
                c"A:/etc/pspapp/lvgl/icon/msc/bg_main_spk_btn.png".as_ptr() as *const _,
            );
            lv_obj_align(btn_spk, lv_align_t_LV_ALIGN_CENTER, -142, 8);
            lv_obj_set_style_border_width(btn_spk, 0, 0);
            // Remove ADV_HITTEST so the full bounding box is clickable (not just opaque pixels).
            lv_obj_remove_flag(btn_spk, lv_obj_flag_t_LV_OBJ_FLAG_ADV_HITTEST);

            let img_spk = lv_image_create(btn_spk);
            lv_image_set_src(
                img_spk,
                c"A:/etc/pspapp/lvgl/icon/volume/spk_on.png".as_ptr() as *const _,
            );
            lv_obj_align(img_spk, lv_align_t_LV_ALIGN_TOP_MID, 0, 10);

            let lbl_vol = lv_label_create(btn_spk);
            lv_label_set_text(lbl_vol, c"Vol".as_ptr());
            lv_obj_set_style_text_color(lbl_vol, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(lbl_vol, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_vol, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -8);

            // Make vol button clickable — opens Control Center.
            lv_obj_add_flag(btn_spk, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
            let vol_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(
                btn_spk,
                Some(vol_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                vol_tx as *mut _,
            );

            // ── Record button (180×76) ────────────────────────────────────────
            // Original: TOP_MID x=0, y=160 → centre (200, 198)
            // CENTER offset: (0, 198−152) = (0, +46)
            let btn_rec = lv_image_create(screen);
            lv_image_set_src(
                btn_rec,
                c"A:/etc/pspapp/lvgl/icon/msc/bg_main_record_btn.png".as_ptr() as *const _,
            );
            lv_obj_align(btn_rec, lv_align_t_LV_ALIGN_CENTER, 0, 46);
            lv_obj_set_style_border_width(btn_rec, 0, 0);
            lv_obj_remove_flag(btn_rec, lv_obj_flag_t_LV_OBJ_FLAG_ADV_HITTEST);

            let lbl_rec = lv_label_create(btn_rec);
            lv_label_set_text(lbl_rec, c"> REC".as_ptr());
            lv_obj_set_style_text_color(lbl_rec, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(lbl_rec, &lv_font_montserrat_14, 0);
            lv_obj_center(lbl_rec);

            // Make rec button clickable and dispatch StartRecording on press.
            lv_obj_add_flag(btn_rec, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
            let rec_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(
                btn_rec,
                Some(rec_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                rec_tx as *mut _,
            );

            // ── Mic button (76×76) ────────────────────────────────────────────
            // Original: TOP_LEFT x=304, y=122 → centre (342, 160)
            // CENTER offset: (342−200, 160−152) = (+142, +8)
            let btn_mic = lv_image_create(screen);
            lv_image_set_src(
                btn_mic,
                c"A:/etc/pspapp/lvgl/icon/msc/bg_main_mic_btn.png".as_ptr() as *const _,
            );
            lv_obj_align(btn_mic, lv_align_t_LV_ALIGN_CENTER, 142, 8);
            lv_obj_set_style_border_width(btn_mic, 0, 0);
            lv_obj_remove_flag(btn_mic, lv_obj_flag_t_LV_OBJ_FLAG_ADV_HITTEST);

            let img_mic = lv_image_create(btn_mic);
            lv_image_set_src(
                img_mic,
                c"A:/etc/pspapp/lvgl/icon/volume/mic_on.png".as_ptr() as *const _,
            );
            lv_obj_align(img_mic, lv_align_t_LV_ALIGN_TOP_MID, 0, 10);
            self.img_mic = img_mic;

            let lbl_mute = lv_label_create(btn_mic);
            lv_label_set_text(lbl_mute, c"Mute".as_ptr());
            lv_obj_set_style_text_color(lbl_mute, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(lbl_mute, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_mute, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -8);
            self.lbl_mute = lbl_mute;

            // Make mic button clickable and dispatch ToggleMicMute on press.
            lv_obj_add_flag(btn_mic, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
            let mic_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(
                btn_mic,
                Some(mic_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                mic_tx as *mut _,
            );

            // ── Bottom "More" indicator (TOP_MID y≈264) → toggle Control Center ──
            // Transparent 120×40 hit area with arrow + "More" label.
            // arrow_down.png is stored upside-down in the asset pack (pspapp convention),
            // so rotate 180° (1800 tenths-degrees) to render pointing downward on screen.
            let btn_more = lv_obj_create(screen);
            lv_obj_set_size(btn_more, 120, 40);
            // center of screen (200,152) + (0,+110) → center at y=262, top at y=242
            lv_obj_align(btn_more, lv_align_t_LV_ALIGN_CENTER, 0, 110);
            lv_obj_set_style_bg_opa(btn_more, 0, 0);
            lv_obj_set_style_border_width(btn_more, 0, 0);
            lv_obj_set_style_pad_top(btn_more, 0, 0);
            lv_obj_set_style_pad_bottom(btn_more, 0, 0);
            lv_obj_set_style_pad_left(btn_more, 0, 0);
            lv_obj_set_style_pad_right(btn_more, 0, 0);
            lv_obj_add_flag(btn_more, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

            let img_arrow = lv_image_create(btn_more);
            lv_image_set_src(
                img_arrow,
                b"A:/etc/pspapp/lvgl/icon/btn/arrow_down.png\0".as_ptr() as *const _,
            );
            lv_image_set_rotation(img_arrow, 1800);
            lv_obj_align(img_arrow, lv_align_t_LV_ALIGN_TOP_MID, 0, 0);

            let lbl_more = lv_label_create(btn_more);
            lv_label_set_text(lbl_more, c"More".as_ptr());
            lv_obj_set_style_text_color(lbl_more, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(lbl_more, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_more, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, 0);

            let more_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(
                btn_more,
                Some(more_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                more_tx as *mut _,
            );

            screen
        }
    }
}
