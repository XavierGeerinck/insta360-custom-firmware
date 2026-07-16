//! WiFi provisioning screen — renders ProvisioningState:
//!  - Scanning           → "Scanning…"
//!  - ApActive           → scan Wi-Fi QR, with text credentials as fallback
//!  - Connecting         → "Connecting to <ssid>…"
//!  - Connected / Failed → result line
//!
//! Back button cancels provisioning and returns to System settings.

use crate::bindings::*;
use crate::theme::*;
use std::ffi::CString;
use std::sync::Arc;
use waved_state::{Command, ProvisioningState, StateBus};

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_14: lv_font_t;
    static lv_font_montserrat_16: lv_font_t;
}

unsafe extern "C" fn back_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e) as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::CancelWifiProvisioning);
            let _ = (*tx).send(Command::CloseSettings);
        }
    }
}

pub struct WifiProvisioningScreen {
    bus: Arc<StateBus>,
    body: *mut lv_obj_t,
    qr_panel: *mut lv_obj_t,
    qr: *mut lv_obj_t,
    fallback: *mut lv_obj_t,
    last_state: Option<ProvisioningState>,
}

fn wifi_qr_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | ';' | ',' | ':' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn wifi_qr_payload(ssid: &str, passphrase: &str) -> String {
    format!(
        "WIFI:T:WPA;S:{};P:{};H:false;;",
        wifi_qr_escape(ssid),
        wifi_qr_escape(passphrase)
    )
}

impl WifiProvisioningScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            body: std::ptr::null_mut(),
            qr_panel: std::ptr::null_mut(),
            qr: std::ptr::null_mut(),
            fallback: std::ptr::null_mut(),
            last_state: None,
        }
    }

    fn state_text(state: &ProvisioningState) -> String {
        match state {
            ProvisioningState::Idle => "Wi-Fi".to_string(),
            ProvisioningState::Scanning => "Scanning\u{2026}".to_string(),
            ProvisioningState::ApActive {
                ssid,
                passphrase,
                url,
            } => {
                format!("Join Wi-Fi:\n{ssid}\nPass: {passphrase}\nThen open:\n{url}")
            }
            ProvisioningState::Connecting { ssid } => format!("Connecting to\n{ssid}\u{2026}"),
            ProvisioningState::Connected { ssid } => format!("Connected to\n{ssid}"),
            ProvisioningState::Failed { reason } => format!("Failed:\n{reason}"),
        }
    }

    /// CString from the (user-controlled) state text. An SSID may legally
    /// contain a NUL byte; fall back rather than panic the UI thread.
    fn state_cstring(state: &ProvisioningState) -> CString {
        CString::new(Self::state_text(state)).unwrap_or_else(|_| CString::new("(invalid)").unwrap())
    }

    unsafe fn show_text_state(&self, state: &ProvisioningState) {
        lv_obj_add_flag(self.qr_panel, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        lv_obj_add_flag(self.fallback, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        lv_obj_remove_flag(self.body, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);

        let text = Self::state_cstring(state);
        lv_label_set_text(self.body, text.as_ptr());
        lv_obj_align(self.body, lv_align_t_LV_ALIGN_CENTER, 0, -2);
    }

    /// Refresh the QR or status text from the latest ProvisioningState.
    ///
    /// # Safety
    ///
    /// Must run on the LVGL UI thread while the objects created by `create`
    /// remain alive.
    pub unsafe fn update_state(&mut self, state: &ProvisioningState) {
        if self.body.is_null()
            || self.qr_panel.is_null()
            || self.qr.is_null()
            || self.fallback.is_null()
            || self.last_state.as_ref() == Some(state)
        {
            return;
        }
        self.last_state = Some(state.clone());

        let ProvisioningState::ApActive {
            ssid,
            passphrase,
            url,
        } = state
        else {
            self.show_text_state(state);
            return;
        };

        let payload = wifi_qr_payload(ssid, passphrase);
        if lv_qrcode_update(self.qr, payload.as_ptr().cast(), payload.len() as u32) != 0 {
            self.show_text_state(state);
            return;
        }

        let fallback = CString::new(format!(
            "Network\n{ssid}\nPassword\n{passphrase}\nOpen\n{}",
            url.trim_start_matches("http://").trim_end_matches('/')
        ))
        .unwrap_or_else(|_| CString::new("Use manual setup").unwrap());
        lv_label_set_text(self.fallback, fallback.as_ptr());
        lv_obj_align(self.fallback, lv_align_t_LV_ALIGN_CENTER, 112, -5);
        lv_obj_add_flag(self.body, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        lv_obj_remove_flag(self.qr_panel, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        lv_obj_remove_flag(self.fallback, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
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

            // Keep all content below the persistent status overlay and inside
            // the circular panel's narrow top/bottom edges.
            let title = lv_label_create(screen);
            lv_label_set_text(title, c"Wi-Fi Setup".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_16, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 45);

            // Body label is constrained to the central safe area. Set text
            // before alignment so LVGL uses its real multi-line height.
            let body = lv_label_create(screen);
            lv_label_set_long_mode(body, lv_label_long_mode_t_LV_LABEL_LONG_WRAP);
            lv_obj_set_width(body, 260);
            lv_obj_set_style_text_align(body, lv_text_align_t_LV_TEXT_ALIGN_CENTER, 0);
            lv_obj_set_style_text_color(body, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(body, &lv_font_montserrat_14, 0);
            lv_label_set_text(body, c"Scanning…".as_ptr());
            lv_obj_align(body, lv_align_t_LV_ALIGN_CENTER, 0, -2);
            self.body = body;

            // A white parent supplies the QR quiet zone. The QR is shifted
            // left so manual credentials remain readable alongside it.
            let qr_panel = lv_obj_create(screen);
            lv_obj_set_size(qr_panel, 160, 160);
            lv_obj_align(qr_panel, lv_align_t_LV_ALIGN_CENTER, -32, -5);
            lv_obj_set_style_bg_color(qr_panel, rgb(CLR_WHITE), 0);
            lv_obj_set_style_bg_opa(qr_panel, 255, 0);
            lv_obj_set_style_border_width(qr_panel, 0, 0);
            lv_obj_set_style_radius(qr_panel, 6, 0);
            lv_obj_set_style_pad_top(qr_panel, 0, 0);
            lv_obj_set_style_pad_bottom(qr_panel, 0, 0);
            lv_obj_set_style_pad_left(qr_panel, 0, 0);
            lv_obj_set_style_pad_right(qr_panel, 0, 0);
            lv_obj_remove_flag(qr_panel, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

            let qr = lv_qrcode_create(qr_panel);
            lv_qrcode_set_size(qr, 140);
            lv_qrcode_set_dark_color(qr, rgb(CLR_BG));
            lv_qrcode_set_light_color(qr, rgb(CLR_WHITE));
            lv_obj_center(qr);

            let fallback = lv_label_create(screen);
            lv_label_set_long_mode(fallback, lv_label_long_mode_t_LV_LABEL_LONG_WRAP);
            lv_obj_set_width(fallback, 120);
            lv_obj_set_style_text_align(fallback, lv_text_align_t_LV_TEXT_ALIGN_LEFT, 0);
            lv_obj_set_style_text_color(fallback, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(fallback, &lv_font_montserrat_12, 0);
            lv_obj_align(fallback, lv_align_t_LV_ALIGN_CENTER, 112, -5);

            lv_obj_add_flag(qr_panel, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
            lv_obj_add_flag(fallback, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
            self.qr_panel = qr_panel;
            self.qr = qr;
            self.fallback = fallback;

            // Bottom-center back control stays reachable on the round panel
            // and does not compete with the status bar.
            let btn_back = lv_button_create(screen);
            lv_obj_set_size(btn_back, 120, 42);
            lv_obj_align(btn_back, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -10);
            lv_obj_set_style_radius(btn_back, 21, 0);
            lv_obj_set_style_bg_color(btn_back, rgb(CLR_WHITE), 0);
            lv_obj_set_style_bg_opa(btn_back, 35, 0);
            lv_obj_set_style_border_width(btn_back, 0, 0);
            lv_obj_set_style_shadow_width(btn_back, 0, 0);
            lv_obj_set_style_pad_top(btn_back, 0, 0);
            lv_obj_set_style_pad_bottom(btn_back, 0, 0);
            lv_obj_set_style_pad_left(btn_back, 0, 0);
            lv_obj_set_style_pad_right(btn_back, 0, 0);

            let back_label = lv_label_create(btn_back);
            lv_label_set_text(back_label, c"Back".as_ptr());
            lv_obj_set_style_text_color(back_label, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(back_label, &lv_font_montserrat_14, 0);
            lv_obj_center(back_label);
            lv_obj_remove_flag(back_label, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

            let back_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(
                btn_back,
                Some(back_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                back_tx as *mut _,
            );

            let initial = match &*self.bus.provisioning.borrow() {
                ProvisioningState::Idle => ProvisioningState::Scanning,
                state => state.clone(),
            };
            self.last_state = None;
            self.update_state(&initial);

            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ap_state_includes_every_operator_value() {
        let text = WifiProvisioningScreen::state_text(&ProvisioningState::ApActive {
            ssid: "Wave-1A2B".into(),
            passphrase: "12ab34cd".into(),
            url: "http://192.168.2.1".into(),
        });
        assert!(text.contains("Wave-1A2B"));
        assert!(text.contains("12ab34cd"));
        assert!(text.contains("http://192.168.2.1"));
    }

    #[test]
    fn wifi_qr_payload_uses_the_standard_join_schema() {
        assert_eq!(
            wifi_qr_payload("Wave-1A2B", "12ab34cd"),
            "WIFI:T:WPA;S:Wave-1A2B;P:12ab34cd;H:false;;"
        );
    }

    #[test]
    fn wifi_qr_payload_escapes_reserved_characters() {
        assert_eq!(
            wifi_qr_payload("Wave:guest;lab", "p,ass\\\"word"),
            "WIFI:T:WPA;S:Wave\\:guest\\;lab;P:p\\,ass\\\\\\\"word;H:false;;"
        );
    }
}
