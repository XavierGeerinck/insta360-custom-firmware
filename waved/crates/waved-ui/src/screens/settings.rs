//! "More" screen — 1:1 recreation of pspapp `ui_setting_screen_on_create`.
//!
//! Reference: libui.so `ui_setting_screen_on_create` at 0x0016ead8 and
//! `ui_setting_screen_refresh` at 0x0016e8f4 (no-PUC branch).
//!
//! Layout in 400×304 space:
//!
//!   Files      pos(10,  68)  via ui_icon_txt_circule_button_create, icon=btn_folder.png
//!   AudioMode  pos(142, 128) via ui_icon_txt_circule_button_create, icon=btn_audio_mode.png
//!   Settings   pos(274, 68)  via ui_icon_txt_circule_button_create, icon=btn_settings.png
//!   Back       TOP_MID y=264 via ui_icon_circule_button_ctrl_create + size 120, arrow_down.png
//!
//! Button factory (ui_icon_txt_circule_button_create → decomp 0x001fb86c):
//!   lv_btn_create, size 116×116 (0x74), radius=116, border_side=0
//!   bg_color=white, bg_opa=44 (0x2c)          ← semi-transparent white circle
//!   Icon child: lv_img 40×40 at TOP_MID y=23
//!   Label child: style 0x51 → white text, center align, font idx 10
//!     width=100, align_to(icon, OUT_BOTTOM_MID, 0, 2)
//!   Pressed-state overlay child: 116×116 white bg_opa=25, hidden by default.
//!   We simplify the press overlay by bumping bg_opa on LV_STATE_PRESSED (32).
//!
//! Back button factory (ui_icon_circule_button_ctrl_create → decomp 0x001fb31c):
//!   lv_btn_create, initial 90×90 → set_size to 120×120 (0x78)
//!   Transparent bg by default (only arrow_down child is visible)
//!   Pressed-state overlay child: 120×120 white bg_opa=25, hidden by default.
//!   Arrow child: arrow_down.png at TOP_MID y=16.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{Command, StateBus};

extern "C" {
    static lv_font_montserrat_14: lv_font_t;
}

// LVGL style selector for pressed state (LV_STATE_PRESSED = 32, LV_PART_MAIN = 0).
const SEL_PRESSED: u32 = 32;

// ── Event callbacks ──────────────────────────────────────────────────────────

unsafe extern "C" fn back_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() { let _ = (*tx).send(Command::CloseSettings); }
    }
}

unsafe extern "C" fn audio_mode_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() { let _ = (*tx).send(Command::OpenAudioMode); }
    }
}

unsafe extern "C" fn settings_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() { let _ = (*tx).send(Command::OpenSystemSettings); }
    }
}

// Files is not yet implemented — emit a bookmark toast as tap confirmation.
unsafe extern "C" fn noop_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() { let _ = (*tx).send(Command::AddBookmark); }
    }
}

// ── Screen ────────────────────────────────────────────────────────────────────

pub struct SettingsScreen {
    bus: Arc<StateBus>,
}

impl SettingsScreen {
    pub fn new(bus: Arc<StateBus>) -> Self { Self { bus } }
}

/// Create a 116×116 icon+text circular button, matching pspapp
/// `ui_icon_txt_circule_button_create`.
unsafe fn make_icon_txt_btn(
    parent: *mut lv_obj_t,
    icon_path: &[u8],
    label_text: &str,
    x: i32, y: i32,
    cb: unsafe extern "C" fn(*mut lv_event_t),
    tx: *mut ::std::os::raw::c_void,
) -> *mut lv_obj_t {
    let btn = lv_button_create(parent);
    lv_obj_set_size(btn, 116, 116);
    lv_obj_set_pos(btn, x, y);
    lv_obj_set_style_radius(btn, 116, 0);
    lv_obj_set_style_border_width(btn, 0, 0);
    lv_obj_set_style_bg_color(btn, rgb(CLR_WHITE), 0);
    lv_obj_set_style_bg_opa(btn, 44, 0);               // 0x2c base
    lv_obj_set_style_bg_opa(btn, 95,  SEL_PRESSED);    // brighter on press
    lv_obj_set_style_pad_top(btn, 0, 0);
    lv_obj_set_style_pad_bottom(btn, 0, 0);
    lv_obj_set_style_pad_left(btn, 0, 0);
    lv_obj_set_style_pad_right(btn, 0, 0);
    lv_obj_set_style_shadow_width(btn, 0, 0);
    lv_obj_remove_flag(btn, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

    // Icon: 40×40 at TOP_MID y=23
    let icon = lv_image_create(btn);
    lv_image_set_src(icon, icon_path.as_ptr() as *const _);
    lv_obj_set_size(icon, 40, 40);
    lv_obj_align(icon, lv_align_t_LV_ALIGN_TOP_MID, 0, 23);
    lv_obj_remove_flag(icon, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

    // Label: width=100, white text, center-aligned, below icon.
    let lbl = lv_label_create(btn);
    let s = CString::new(label_text).unwrap();
    lv_label_set_text(lbl, s.as_ptr());
    lv_obj_set_width(lbl, 100);
    lv_obj_set_style_text_color(lbl, rgb(CLR_WHITE), 0);
    lv_obj_set_style_text_font(lbl, &lv_font_montserrat_14, 0);
    lv_obj_set_style_text_align(lbl, lv_text_align_t_LV_TEXT_ALIGN_CENTER, 0);
    lv_obj_align_to(lbl, icon, lv_align_t_LV_ALIGN_OUT_BOTTOM_MID, 0, 2);
    lv_obj_remove_flag(lbl, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

    lv_obj_add_event_cb(btn, Some(cb), lv_event_code_t_LV_EVENT_CLICKED, tx);
    btn
}

/// Create a 120×120 transparent circular back button with arrow_down icon,
/// matching pspapp `ui_icon_circule_button_ctrl_create` + size 0x78.
unsafe fn make_back_btn(
    parent: *mut lv_obj_t,
    tx: *mut ::std::os::raw::c_void,
) -> *mut lv_obj_t {
    let btn = lv_button_create(parent);
    lv_obj_set_size(btn, 120, 120);
    lv_obj_align(btn, lv_align_t_LV_ALIGN_TOP_MID, 0, 264);
    lv_obj_set_style_radius(btn, 120, 0);
    lv_obj_set_style_border_width(btn, 0, 0);
    // Transparent by default — press-state shows a faint white circle.
    lv_obj_set_style_bg_opa(btn, 0, 0);
    lv_obj_set_style_bg_color(btn, rgb(CLR_WHITE), SEL_PRESSED);
    lv_obj_set_style_bg_opa(btn, 40, SEL_PRESSED);
    lv_obj_set_style_pad_top(btn, 0, 0);
    lv_obj_set_style_pad_bottom(btn, 0, 0);
    lv_obj_set_style_pad_left(btn, 0, 0);
    lv_obj_set_style_pad_right(btn, 0, 0);
    lv_obj_set_style_shadow_width(btn, 0, 0);
    lv_obj_remove_flag(btn, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

    let arrow = lv_image_create(btn);
    lv_image_set_src(arrow,
        b"A:/etc/pspapp/lvgl/icon/btn/arrow_down.png\0".as_ptr() as *const _);
    lv_obj_align(arrow, lv_align_t_LV_ALIGN_TOP_MID, 0, 16);
    lv_obj_remove_flag(arrow, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

    lv_obj_add_event_cb(btn, Some(back_btn_cb),
        lv_event_code_t_LV_EVENT_CLICKED, tx);
    btn
}

impl super::Screen for SettingsScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            // Root — solid black (style 0x1b from pspapp: bg_opa=0xff, bg_color=0).
            let screen = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_bg_opa(screen, 255, 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_top(screen, 0, 0);
            lv_obj_set_style_pad_bottom(screen, 0, 0);
            lv_obj_set_style_pad_left(screen, 0, 0);
            lv_obj_set_style_pad_right(screen, 0, 0);
            lv_obj_remove_flag(screen, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            lv_obj_set_scrollbar_mode(screen, lv_scrollbar_mode_t_LV_SCROLLBAR_MODE_OFF);

            // Back button (120×120 transparent, arrow_down at top).
            let back_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            let _back = make_back_btn(screen, back_tx as *mut _);

            // Files — pos(10, 68) → placeholder (AddBookmark toast).
            let files_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            let _files = make_icon_txt_btn(
                screen,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_folder.png\0",
                "Files",
                10, 68,
                noop_btn_cb,
                files_tx as *mut _,
            );

            // Audio Mode — pos(142, 128) → OpenAudioMode.
            let audio_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            let _audio = make_icon_txt_btn(
                screen,
                b"A:/etc/pspapp/lvgl/icon/beamforming/btn_audio_mode.png\0",
                "Audio Mode",
                142, 128,
                audio_mode_btn_cb,
                audio_tx as *mut _,
            );

            // Settings — pos(274, 68) → OpenSystemSettings (pspapp page 0xe).
            let sys_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            let _settings = make_icon_txt_btn(
                screen,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_settings.png\0",
                "Settings",
                274, 68,
                settings_btn_cb,
                sys_tx as *mut _,
            );

            screen
        }
    }
}
