//! Wi-Fi management and provisioning screen. Normal entry renders the live
//! connection state without disturbing the station. The Connect action enters
//! the existing QR/captive-portal provisioning flow.
//!
//! Back button cancels provisioning and returns to System settings.

use crate::bindings::*;
use crate::theme::*;
use std::ffi::CString;
use std::sync::Arc;
use waved_state::{Command, ProvisioningState, StateBus, WifiState};

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

unsafe extern "C" fn connect_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e) as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::OpenWifiProvisioning);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RenderModel {
    Management(String),
    Message { text: String, retry: bool },
    AccessPoint { payload: String, fallback: String },
}

pub struct WifiProvisioningScreen {
    bus: Arc<StateBus>,
    body: *mut lv_obj_t,
    status_panel: *mut lv_obj_t,
    status_label: *mut lv_obj_t,
    connect_btn: *mut lv_obj_t,
    qr_panel: *mut lv_obj_t,
    qr: *mut lv_obj_t,
    fallback: *mut lv_obj_t,
    provisioning_session: bool,
    awaiting_provisioning: bool,
    last_render: Option<RenderModel>,
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

fn qrcode_update_succeeded(result: lv_result_t) -> bool {
    result == lv_result_t_LV_RESULT_OK
}

fn signal_quality(rssi: i8) -> &'static str {
    if rssi >= -55 {
        "Excellent"
    } else if rssi >= -67 {
        "Good"
    } else if rssi >= -75 {
        "Fair"
    } else {
        "Weak"
    }
}

fn management_text(wifi: &WifiState) -> String {
    if wifi.connected {
        format!(
            "Connected\n{}\nSignal: {} ({} dBm)\nIP: {}",
            wifi.ssid,
            signal_quality(wifi.signal_strength),
            wifi.signal_strength,
            wifi.ip_addr
        )
    } else {
        "Not connected\nNo Wi-Fi network".to_string()
    }
}

fn render_model(
    state: &ProvisioningState,
    wifi: &WifiState,
    provisioning_session: bool,
    awaiting_provisioning: bool,
) -> RenderModel {
    if awaiting_provisioning {
        return RenderModel::Message {
            text: "Scanning\u{2026}".to_string(),
            retry: false,
        };
    }

    match state {
        ProvisioningState::Scanning => RenderModel::Message {
            text: "Scanning\u{2026}".to_string(),
            retry: false,
        },
        ProvisioningState::ApActive {
            ssid,
            passphrase,
            url,
        } => RenderModel::AccessPoint {
            payload: wifi_qr_payload(ssid, passphrase),
            fallback: format!(
                "Network\n{ssid}\nPassword\n{passphrase}\nOpen\n{}",
                url.trim_start_matches("http://").trim_end_matches('/')
            ),
        },
        ProvisioningState::Connecting { ssid } => RenderModel::Message {
            text: format!("Connecting to\n{ssid}\u{2026}"),
            retry: false,
        },
        ProvisioningState::Saving => RenderModel::Message {
            text: "Saving settings\u{2026}".to_string(),
            retry: false,
        },
        ProvisioningState::Reconnecting => RenderModel::Message {
            text: "Reconnecting to\nsaved Wi-Fi\u{2026}".to_string(),
            retry: false,
        },
        ProvisioningState::Restarting => RenderModel::Message {
            text: "Settings saved\nRestarting\u{2026}".to_string(),
            retry: false,
        },
        ProvisioningState::Failed { reason } if provisioning_session => RenderModel::Message {
            text: format!("Connection failed\n{reason}"),
            retry: true,
        },
        ProvisioningState::Idle
        | ProvisioningState::Connected { .. }
        | ProvisioningState::Failed { .. } => RenderModel::Management(management_text(wifi)),
    }
}

impl WifiProvisioningScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            body: std::ptr::null_mut(),
            status_panel: std::ptr::null_mut(),
            status_label: std::ptr::null_mut(),
            connect_btn: std::ptr::null_mut(),
            qr_panel: std::ptr::null_mut(),
            qr: std::ptr::null_mut(),
            fallback: std::ptr::null_mut(),
            provisioning_session: false,
            awaiting_provisioning: false,
            last_render: None,
        }
    }

    pub fn open_management(&mut self) {
        self.provisioning_session = false;
        self.awaiting_provisioning = false;
        self.last_render = None;
    }

    pub fn begin_provisioning(&mut self) {
        self.provisioning_session = true;
        self.awaiting_provisioning = true;
        self.last_render = None;
    }

    fn safe_cstring(text: &str) -> CString {
        CString::new(text).unwrap_or_else(|_| CString::new("(invalid)").unwrap())
    }

    unsafe fn hide_provisioning_content(&self) {
        lv_obj_add_flag(self.qr_panel, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        lv_obj_add_flag(self.fallback, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        lv_obj_add_flag(self.body, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
    }

    unsafe fn show_management(&self, text: &str) {
        self.hide_provisioning_content();
        lv_obj_remove_flag(self.status_panel, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        lv_obj_remove_flag(self.connect_btn, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);

        let text = Self::safe_cstring(text);
        lv_label_set_text(self.status_label, text.as_ptr());
        lv_obj_center(self.status_label);
    }

    unsafe fn show_message(&self, text: &str, retry: bool) {
        lv_obj_add_flag(self.status_panel, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        lv_obj_add_flag(self.qr_panel, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        lv_obj_add_flag(self.fallback, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        lv_obj_remove_flag(self.body, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        if retry {
            lv_obj_remove_flag(self.connect_btn, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        } else {
            lv_obj_add_flag(self.connect_btn, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        }

        let text = Self::safe_cstring(text);
        lv_label_set_text(self.body, text.as_ptr());
        lv_obj_align(self.body, lv_align_t_LV_ALIGN_CENTER, 0, -18);
    }

    /// Refresh the QR or status text from the latest ProvisioningState.
    ///
    /// # Safety
    ///
    /// Must run on the LVGL UI thread while the objects created by `create`
    /// remain alive.
    pub unsafe fn update_state(&mut self, state: &ProvisioningState, wifi: &WifiState) {
        if self.body.is_null()
            || self.status_panel.is_null()
            || self.status_label.is_null()
            || self.connect_btn.is_null()
            || self.qr_panel.is_null()
            || self.qr.is_null()
            || self.fallback.is_null()
        {
            return;
        }

        if matches!(
            state,
            ProvisioningState::Scanning
                | ProvisioningState::ApActive { .. }
                | ProvisioningState::Connecting { .. }
        ) {
            self.provisioning_session = true;
            self.awaiting_provisioning = false;
        } else if !self.awaiting_provisioning
            && matches!(
                state,
                ProvisioningState::Idle | ProvisioningState::Connected { .. }
            )
        {
            self.provisioning_session = false;
        }

        let model = render_model(
            state,
            wifi,
            self.provisioning_session,
            self.awaiting_provisioning,
        );
        if self.last_render.as_ref() == Some(&model) {
            return;
        }
        self.last_render = Some(model.clone());

        match model {
            RenderModel::Management(text) => self.show_management(&text),
            RenderModel::Message { text, retry } => self.show_message(&text, retry),
            RenderModel::AccessPoint { payload, fallback } => {
                if !qrcode_update_succeeded(lv_qrcode_update(
                    self.qr,
                    payload.as_ptr().cast(),
                    payload.len() as u32,
                )) {
                    self.show_message("Use the manual setup details", false);
                    return;
                }

                let fallback = Self::safe_cstring(&fallback);
                lv_label_set_text(self.fallback, fallback.as_ptr());
                lv_obj_align(self.fallback, lv_align_t_LV_ALIGN_CENTER, 112, -5);
                lv_obj_add_flag(self.body, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                lv_obj_add_flag(self.status_panel, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                lv_obj_add_flag(self.connect_btn, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                lv_obj_remove_flag(self.qr_panel, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                lv_obj_remove_flag(self.fallback, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
            }
        }
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
            lv_label_set_text(title, c"Wi-Fi".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_16, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 45);

            // Live connection card shown during normal, non-disruptive menu
            // use. The compact centered copy stays within the round safe area.
            let status_panel = lv_obj_create(screen);
            lv_obj_set_size(status_panel, 270, 106);
            lv_obj_align(status_panel, lv_align_t_LV_ALIGN_TOP_MID, 0, 72);
            lv_obj_set_style_bg_color(status_panel, rgb(CLR_SURFACE), 0);
            lv_obj_set_style_bg_opa(status_panel, 255, 0);
            lv_obj_set_style_border_color(status_panel, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_border_width(status_panel, 1, 0);
            lv_obj_set_style_radius(status_panel, 18, 0);
            lv_obj_set_style_pad_top(status_panel, 6, 0);
            lv_obj_set_style_pad_bottom(status_panel, 6, 0);
            lv_obj_set_style_pad_left(status_panel, 10, 0);
            lv_obj_set_style_pad_right(status_panel, 10, 0);
            lv_obj_remove_flag(status_panel, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

            let status_label = lv_label_create(status_panel);
            lv_label_set_long_mode(status_label, lv_label_long_mode_t_LV_LABEL_LONG_CLIP);
            lv_obj_set_size(status_label, 242, 80);
            lv_obj_set_style_text_align(status_label, lv_text_align_t_LV_TEXT_ALIGN_CENTER, 0);
            lv_obj_set_style_text_color(status_label, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(status_label, &lv_font_montserrat_14, 0);
            lv_label_set_text(status_label, c"Not connected\nNo Wi-Fi network".as_ptr());
            lv_obj_center(status_label);
            self.status_panel = status_panel;
            self.status_label = status_label;

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

            // Opening this menu never drops the current link. Only this
            // explicit action starts AP provisioning.
            let connect_btn = lv_button_create(screen);
            lv_obj_set_size(connect_btn, 250, 42);
            lv_obj_align(connect_btn, lv_align_t_LV_ALIGN_CENTER, 0, 62);
            lv_obj_set_style_radius(connect_btn, 21, 0);
            lv_obj_set_style_bg_color(connect_btn, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_bg_opa(connect_btn, 255, 0);
            lv_obj_set_style_border_width(connect_btn, 0, 0);
            lv_obj_set_style_shadow_width(connect_btn, 0, 0);
            lv_obj_set_style_pad_top(connect_btn, 0, 0);
            lv_obj_set_style_pad_bottom(connect_btn, 0, 0);
            lv_obj_set_style_pad_left(connect_btn, 0, 0);
            lv_obj_set_style_pad_right(connect_btn, 0, 0);

            let connect_label = lv_label_create(connect_btn);
            lv_label_set_text(connect_label, c"Connect to another network".as_ptr());
            lv_obj_set_style_text_color(connect_label, rgb(CLR_BG), 0);
            lv_obj_set_style_text_font(connect_label, &lv_font_montserrat_14, 0);
            lv_obj_center(connect_label);
            lv_obj_remove_flag(connect_label, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

            let connect_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(
                connect_btn,
                Some(connect_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                connect_tx as *mut _,
            );
            self.connect_btn = connect_btn;

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

            let initial_provisioning = self.bus.provisioning.borrow().clone();
            let initial_wifi = self.bus.wifi.borrow().clone();
            self.last_render = None;
            self.update_state(&initial_provisioning, &initial_wifi);

            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ap_state_includes_every_operator_value() {
        let model = render_model(
            &ProvisioningState::ApActive {
                ssid: "Wave-1A2B".into(),
                passphrase: "12ab34cd".into(),
                url: "http://192.168.2.1".into(),
            },
            &WifiState::default(),
            true,
            false,
        );
        let RenderModel::AccessPoint { payload, fallback } = model else {
            panic!("expected access point model");
        };
        assert!(payload.contains("Wave-1A2B"));
        assert!(payload.contains("12ab34cd"));
        assert!(fallback.contains("192.168.2.1"));
    }

    #[test]
    fn connected_management_includes_live_network_details() {
        let wifi = WifiState {
            connected: true,
            ssid: "Office 5G".into(),
            ip_addr: "192.168.1.148".into(),
            signal_strength: -61,
        };
        let text = management_text(&wifi);
        assert!(text.contains("Connected"));
        assert!(text.contains("Office 5G"));
        assert!(text.contains("Good (-61 dBm)"));
        assert!(text.contains("192.168.1.148"));
    }

    #[test]
    fn disconnected_management_is_explicit() {
        assert_eq!(
            management_text(&WifiState::default()),
            "Not connected\nNo Wi-Fi network"
        );
    }

    #[test]
    fn signal_quality_thresholds_are_stable() {
        assert_eq!(signal_quality(-55), "Excellent");
        assert_eq!(signal_quality(-56), "Good");
        assert_eq!(signal_quality(-67), "Good");
        assert_eq!(signal_quality(-68), "Fair");
        assert_eq!(signal_quality(-75), "Fair");
        assert_eq!(signal_quality(-76), "Weak");
    }

    #[test]
    fn stale_failure_is_hidden_when_menu_is_reopened() {
        let failed = ProvisioningState::Failed {
            reason: "old failure".into(),
        };
        assert!(matches!(
            render_model(&failed, &WifiState::default(), false, false),
            RenderModel::Management(_)
        ));
        assert!(matches!(
            render_model(&failed, &WifiState::default(), true, false),
            RenderModel::Message { retry: true, .. }
        ));
    }

    #[test]
    fn connect_action_shows_scanning_before_service_state_arrives() {
        assert_eq!(
            render_model(
                &ProvisioningState::Connected {
                    ssid: "Old network".into()
                },
                &WifiState::default(),
                true,
                true,
            ),
            RenderModel::Message {
                text: "Scanning\u{2026}".into(),
                retry: false,
            }
        );
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

    #[test]
    fn qrcode_result_accepts_only_lvgl_success() {
        assert!(qrcode_update_succeeded(lv_result_t_LV_RESULT_OK));
        assert!(!qrcode_update_succeeded(lv_result_t_LV_RESULT_INVALID));
    }
}
