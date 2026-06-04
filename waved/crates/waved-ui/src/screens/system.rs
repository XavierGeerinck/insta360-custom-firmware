//! System Settings list — pspapp page 0xe / `ui_setting_system_on_create`.
//!
//! Reference: libui.so `ui_setting_system_on_create` at 0x0016f948.
//!
//! Original layout:
//!   - Container with solid black bg (style 0x1b)
//!   - Arc scrollbar component on the right side
//!   - Scrollable FLEX_COLUMN list of 13 rows (bluetooth, wifi, password, usb,
//!     mic, height, common, dongle, original audio, auto sleep, language,
//!     power off, ai_privilege_terms) built via `ui_txt_btn_285_create`
//!     (285×76, white bg_opa=0x19, radius=0x12)
//!   - Title component at TOP_MID via `ui_comp_screen_title_create`
//!   - Small 56×64 back button at TOP_LEFT y=70 via `ui_icon_back_button_ctrl_create`
//!     (bg_image = btn_back.png)
//!
//! We only implement the rows whose underlying features this firmware supports.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{Command, StateBus};

extern "C" {
    static lv_font_montserrat_14: lv_font_t;
    static lv_font_montserrat_16: lv_font_t;
}

const SEL_PRESSED: u32 = 32; // LV_STATE_PRESSED | LV_PART_MAIN

// ── Row actions ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum RowAction {
    OpenAudioMode,
    PowerOff,
    Bookmark, // placeholder for rows whose feature isn't implemented
    OpenWifiProvisioning,
}

const ROWS: &[(&str, RowAction)] = &[
    ("Audio Mode",      RowAction::OpenAudioMode),
    ("Noise Reduction", RowAction::Bookmark),
    ("Mic",             RowAction::Bookmark),
    ("Bluetooth",       RowAction::Bookmark),
    ("Wi-Fi",           RowAction::OpenWifiProvisioning),
    ("USB Mode",        RowAction::Bookmark),
    ("Language",        RowAction::Bookmark),
    ("Auto Sleep",      RowAction::Bookmark),
    ("Power Off",       RowAction::PowerOff),
];

// ── Event callbacks ──────────────────────────────────────────────────────────

unsafe extern "C" fn back_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() { let _ = (*tx).send(Command::CloseSettings); }
    }
}

unsafe extern "C" fn row_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) != lv_event_code_t_LV_EVENT_CLICKED { return; }
    let data = lv_event_get_user_data(e)
        as *const (tokio::sync::broadcast::Sender<Command>, RowAction);
    if data.is_null() { return; }
    let (tx, action) = &*data;
    let cmd = match action {
        RowAction::OpenAudioMode        => Command::OpenAudioMode,
        RowAction::PowerOff             => Command::Shutdown,
        RowAction::Bookmark             => Command::AddBookmark,
        RowAction::OpenWifiProvisioning => Command::OpenWifiProvisioning,
    };
    let _ = tx.send(cmd);
}

// ── Screen ───────────────────────────────────────────────────────────────────

pub struct SystemScreen {
    bus: Arc<StateBus>,
}

impl SystemScreen {
    pub fn new(bus: Arc<StateBus>) -> Self { Self { bus } }
}

impl super::Screen for SystemScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            // Root 400×304 black.
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

            // Title "Settings" at TOP_MID y=20, white, montserrat_16.
            let title = lv_label_create(screen);
            lv_label_set_text(title, c"Settings".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_16, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 20);

            // Back button: 56×64 at TOP_LEFT (8, 8), bg_image=btn_back.png,
            // transparent otherwise. Pspapp: ui_icon_back_button_ctrl_create.
            let btn_back = lv_button_create(screen);
            lv_obj_set_size(btn_back, 56, 64);
            lv_obj_align(btn_back, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 8);
            lv_obj_set_style_bg_opa(btn_back, 0, 0);
            lv_obj_set_style_bg_color(btn_back, rgb(CLR_WHITE), SEL_PRESSED);
            lv_obj_set_style_bg_opa(btn_back, 40, SEL_PRESSED);
            lv_obj_set_style_border_width(btn_back, 0, 0);
            lv_obj_set_style_shadow_width(btn_back, 0, 0);
            lv_obj_set_style_radius(btn_back, 8, 0);
            lv_obj_set_style_bg_image_src(
                btn_back,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_back.png\0".as_ptr() as *const _,
                0,
            );
            lv_obj_set_style_pad_top(btn_back, 0, 0);
            lv_obj_set_style_pad_bottom(btn_back, 0, 0);
            lv_obj_set_style_pad_left(btn_back, 0, 0);
            lv_obj_set_style_pad_right(btn_back, 0, 0);
            lv_obj_remove_flag(btn_back, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            let back_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(btn_back, Some(back_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED, back_tx as *mut _);

            // Scrollable list container: 360×220 at (20, 60), flex column, vertical
            // scroll. The circle clips outside the centre; keep content inside x=20..380.
            let list = lv_obj_create(screen);
            lv_obj_set_size(list, 360, 220);
            lv_obj_set_pos(list, 20, 60);
            lv_obj_set_style_bg_opa(list, 0, 0);
            lv_obj_set_style_border_width(list, 0, 0);
            lv_obj_set_style_pad_top(list, 0, 0);
            lv_obj_set_style_pad_bottom(list, 0, 0);
            lv_obj_set_style_pad_left(list, 0, 0);
            lv_obj_set_style_pad_right(list, 0, 0);
            lv_obj_set_style_pad_row(list, 8, 0);
            lv_obj_set_flex_flow(list, lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
            lv_obj_set_flex_align(
                list,
                lv_flex_align_t_LV_FLEX_ALIGN_START,
                lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            );
            lv_obj_set_scroll_dir(list, lv_dir_t_LV_DIR_VER);
            lv_obj_set_scrollbar_mode(list, lv_scrollbar_mode_t_LV_SCROLLBAR_MODE_OFF);

            // Rows: 285×60 white bg_opa=25 rounded rect buttons with centered label.
            // (Original uses 285×76 with radius=18; we squeeze to 60 so ~3.5 rows fit
            // without scrolling on this 220-tall list.)
            for &(label, action) in ROWS {
                let row = lv_button_create(list);
                lv_obj_set_size(row, 285, 60);
                lv_obj_set_style_radius(row, 18, 0);
                lv_obj_set_style_bg_color(row, rgb(CLR_WHITE), 0);
                lv_obj_set_style_bg_opa(row, 25, 0);               // 0x19
                lv_obj_set_style_bg_opa(row, 90, SEL_PRESSED);
                lv_obj_set_style_border_width(row, 0, 0);
                lv_obj_set_style_shadow_width(row, 0, 0);
                lv_obj_set_style_pad_top(row, 0, 0);
                lv_obj_set_style_pad_bottom(row, 0, 0);
                lv_obj_set_style_pad_left(row, 0, 0);
                lv_obj_set_style_pad_right(row, 0, 0);
                lv_obj_remove_flag(row, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

                let lbl = lv_label_create(row);
                let s = CString::new(label).unwrap();
                lv_label_set_text(lbl, s.as_ptr());
                lv_obj_set_style_text_color(lbl, rgb(CLR_WHITE), 0);
                lv_obj_set_style_text_font(lbl, &lv_font_montserrat_14, 0);
                lv_obj_center(lbl);
                lv_obj_remove_flag(lbl, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

                let data = Box::into_raw(Box::new(
                    (self.bus.commands.clone(), action),
                ));
                lv_obj_add_event_cb(row, Some(row_btn_cb),
                    lv_event_code_t_LV_EVENT_CLICKED, data as *mut _);
            }

            screen
        }
    }
}
