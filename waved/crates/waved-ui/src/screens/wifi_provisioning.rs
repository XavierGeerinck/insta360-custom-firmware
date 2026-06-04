//! WiFi provisioning screen — renders ProvisioningState:
//!  - Scanning           → "Scanning…"
//!  - ApActive           → join SSID + passphrase + URL
//!  - Connecting         → "Connecting to <ssid>…"
//!  - Connected / Failed → result line
//! Back button cancels provisioning and returns to System settings.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{Command, ProvisioningState, StateBus};

extern "C" {
    static lv_font_montserrat_14: lv_font_t;
    static lv_font_montserrat_16: lv_font_t;
}

unsafe extern "C" fn back_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::CancelWifiProvisioning);
            let _ = (*tx).send(Command::CloseSettings);
        }
    }
}

pub struct WifiProvisioningScreen {
    bus: Arc<StateBus>,
    body: *mut lv_obj_t,
}

impl WifiProvisioningScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self { bus, body: std::ptr::null_mut() }
    }

    fn state_text(state: &ProvisioningState) -> String {
        match state {
            ProvisioningState::Idle => "Wi-Fi".to_string(),
            ProvisioningState::Scanning => "Scanning\u{2026}".to_string(),
            ProvisioningState::ApActive { ssid, passphrase, url } => {
                format!("Join Wi-Fi:\n{ssid}\nPass: {passphrase}\nThen open:\n{url}")
            }
            ProvisioningState::Connecting { ssid } => format!("Connecting to\n{ssid}\u{2026}"),
            ProvisioningState::Connected { ssid } => format!("Connected to\n{ssid}"),
            ProvisioningState::Failed { reason } => format!("Failed:\n{reason}"),
        }
    }

    /// Refresh the body label from the latest ProvisioningState.
    pub unsafe fn update_state(&mut self, state: &ProvisioningState) {
        if self.body.is_null() { return; }
        let s = CString::new(Self::state_text(state)).unwrap();
        lv_label_set_text(self.body, s.as_ptr());
    }
}

impl super::Screen for WifiProvisioningScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            let screen = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_bg_opa(screen, 255, 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_remove_flag(screen, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

            let title = lv_label_create(screen);
            lv_label_set_text(title, c"Wi-Fi Setup".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_16, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 20);

            let btn_back = lv_button_create(screen);
            lv_obj_set_size(btn_back, 56, 64);
            lv_obj_align(btn_back, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 8);
            lv_obj_set_style_bg_opa(btn_back, 0, 0);
            lv_obj_set_style_border_width(btn_back, 0, 0);
            lv_obj_set_style_bg_image_src(
                btn_back,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_back.png\0".as_ptr() as *const _,
                0,
            );
            let back_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(btn_back, Some(back_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED, back_tx as *mut _);

            // Body label centered, multi-line.
            let body = lv_label_create(screen);
            lv_label_set_long_mode(body, lv_label_long_mode_t_LV_LABEL_LONG_WRAP);
            lv_obj_set_width(body, 280);
            lv_obj_set_style_text_align(body, lv_text_align_t_LV_TEXT_ALIGN_CENTER, 0);
            lv_obj_set_style_text_color(body, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(body, &lv_font_montserrat_14, 0);
            lv_obj_align(body, lv_align_t_LV_ALIGN_CENTER, 0, 10);
            let init = Self::state_text(&self.bus.provisioning.borrow());
            let s = CString::new(init).unwrap();
            lv_label_set_text(body, s.as_ptr());
            self.body = body;

            screen
        }
    }
}
