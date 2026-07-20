use crate::bindings::*;
use crate::theme::*;
use std::ffi::CString;
use std::sync::Arc;
use waved_config::{UploadMode, WavedConfig};
use waved_state::{Command, MeetingState, StateBus};

extern "C" {
    static lv_font_montserrat_14: lv_font_t;
    static lv_font_montserrat_16: lv_font_t;
}

const PRESSED: u32 = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebhookRenderModel {
    pub endpoint: String,
    pub settings: String,
    pub delivery: String,
    pub test: String,
}

pub fn render_model(config: &WavedConfig, meeting: &MeetingState) -> WebhookRenderModel {
    let endpoint = if config.api.endpoint.is_empty() {
        "Not configured".into()
    } else {
        endpoint_host(&config.api.endpoint)
    };
    let mode = match config.api.upload_mode {
        UploadMode::Automatic => "Automatic",
        UploadMode::Ask => "Ask after recording",
        UploadMode::LocalOnly => "Local only",
    };
    let auth = if config.api.auth_token.is_empty() {
        "No bearer token"
    } else {
        "Bearer configured"
    };
    let mut delivery = format!(
        "{} queued  •  {} failed",
        meeting.pending_uploads, meeting.failed_uploads
    );
    let recording_blocked = meeting.recording_blocked_reason;
    if meeting.storage_blocked {
        delivery.push_str("  •  Storage full");
    }
    if let Some(fault) = recording_blocked {
        delivery.push_str(&format!("\nRecording unavailable: {}", fault.diagnostic()));
    }
    if !meeting.storage_blocked && recording_blocked.is_none() {
        if let Some(success) = &meeting.last_upload_success_at {
            delivery.push_str(&format!("\nLast sent {}", compact_timestamp(success)));
        } else if let Some(error) = &meeting.last_upload_error {
            delivery.push_str(&format!("\n{error}"));
        }
    }
    WebhookRenderModel {
        endpoint,
        settings: format!("{mode}  •  {auth}"),
        delivery,
        test: meeting
            .last_webhook_test
            .clone()
            .unwrap_or_else(|| "No setup test yet".into()),
    }
}

fn endpoint_host(endpoint: &str) -> String {
    endpoint
        .split_once("://")
        .map_or(endpoint, |(_, remainder)| remainder)
        .split('/')
        .next()
        .unwrap_or(endpoint)
        .chars()
        .take(42)
        .collect()
}

fn compact_timestamp(timestamp: &str) -> &str {
    timestamp.get(..16).unwrap_or(timestamp)
}

unsafe extern "C" fn back_cb(event: *mut lv_event_t) {
    if lv_event_get_code(event) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(event) as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::CloseSettings);
        }
    }
}

unsafe extern "C" fn retry_cb(event: *mut lv_event_t) {
    if lv_event_get_code(event) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(event) as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::RetryFailedUploads);
        }
    }
}

pub struct WebhookScreen {
    bus: Arc<StateBus>,
    config: WavedConfig,
    endpoint_label: *mut lv_obj_t,
    settings_label: *mut lv_obj_t,
    delivery_label: *mut lv_obj_t,
    test_label: *mut lv_obj_t,
    previous: Option<WebhookRenderModel>,
}

impl WebhookScreen {
    pub fn new(bus: Arc<StateBus>, config: WavedConfig) -> Self {
        Self {
            bus,
            config,
            endpoint_label: std::ptr::null_mut(),
            settings_label: std::ptr::null_mut(),
            delivery_label: std::ptr::null_mut(),
            test_label: std::ptr::null_mut(),
            previous: None,
        }
    }

    /// # Safety
    ///
    /// Must run on the LVGL UI thread while this screen's label objects are
    /// alive.
    pub unsafe fn update_state(&mut self, meeting: &MeetingState) {
        let model = render_model(&self.config, meeting);
        if self.previous.as_ref() == Some(&model) || self.endpoint_label.is_null() {
            return;
        }
        set_text(self.endpoint_label, &model.endpoint);
        set_text(self.settings_label, &model.settings);
        set_text(self.delivery_label, &model.delivery);
        set_text(self.test_label, &format!("Test: {}", model.test));
        self.previous = Some(model);
    }
}

unsafe fn set_text(label: *mut lv_obj_t, value: &str) {
    let text = CString::new(value.replace('\0', "")).unwrap();
    lv_label_set_text(label, text.as_ptr());
}

unsafe fn make_label(
    parent: *mut lv_obj_t,
    y: i32,
    color: u32,
    font: *const lv_font_t,
) -> *mut lv_obj_t {
    let label = lv_label_create(parent);
    lv_obj_set_pos(label, 55, y);
    lv_obj_set_width(label, 290);
    lv_obj_set_style_text_align(label, lv_text_align_t_LV_TEXT_ALIGN_CENTER, 0);
    lv_obj_set_style_text_color(label, rgb(color), 0);
    lv_obj_set_style_text_font(label, font, 0);
    label
}

impl super::Screen for WebhookScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            self.previous = None;
            let screen = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_bg_opa(screen, 255, 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_remove_flag(screen, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

            let title = lv_label_create(screen);
            lv_label_set_text(title, c"Webhook".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_16, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 20);

            let back = lv_button_create(screen);
            lv_obj_set_size(back, 56, 64);
            lv_obj_align(back, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 8);
            lv_obj_set_style_bg_opa(back, 0, 0);
            lv_obj_set_style_bg_color(back, rgb(CLR_WHITE), PRESSED);
            lv_obj_set_style_bg_opa(back, 40, PRESSED);
            lv_obj_set_style_border_width(back, 0, 0);
            lv_obj_set_style_shadow_width(back, 0, 0);
            lv_obj_set_style_bg_image_src(
                back,
                c"A:/etc/pspapp/lvgl/icon/btn/btn_back.png".as_ptr() as *const _,
                0,
            );
            let back_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(
                back,
                Some(back_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                back_tx as *mut _,
            );

            self.endpoint_label = make_label(screen, 72, CLR_WHITE, &lv_font_montserrat_16);
            self.settings_label = make_label(screen, 103, CLR_TEXT_DIM, &lv_font_montserrat_14);
            self.delivery_label = make_label(screen, 140, CLR_WHITE, &lv_font_montserrat_14);
            self.test_label = make_label(screen, 190, CLR_TEXT_DIM, &lv_font_montserrat_14);

            let retry = lv_button_create(screen);
            lv_obj_set_size(retry, 190, 42);
            lv_obj_align(retry, lv_align_t_LV_ALIGN_TOP_MID, 0, 242);
            lv_obj_set_style_radius(retry, 16, 0);
            lv_obj_set_style_bg_color(retry, rgb(CLR_WHITE), 0);
            lv_obj_set_style_bg_opa(retry, 35, 0);
            lv_obj_set_style_bg_opa(retry, 85, PRESSED);
            lv_obj_set_style_border_width(retry, 0, 0);
            lv_obj_set_style_shadow_width(retry, 0, 0);
            let retry_label = lv_label_create(retry);
            lv_label_set_text(retry_label, c"Retry failed".as_ptr());
            lv_obj_set_style_text_color(retry_label, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(retry_label, &lv_font_montserrat_14, 0);
            lv_obj_center(retry_label);
            let retry_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(
                retry,
                Some(retry_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                retry_tx as *mut _,
            );

            let meeting = self.bus.meeting.borrow().clone();
            self.update_state(&meeting);
            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_model_never_exposes_token_or_endpoint_path() {
        let mut config = WavedConfig::default();
        config.api.endpoint = "https://hooks.example.test/private/wave".into();
        config.api.auth_token = "super-secret".into();
        let model = render_model(&config, &MeetingState::default());
        assert_eq!(model.endpoint, "hooks.example.test");
        assert!(model.settings.contains("Bearer configured"));
        assert!(!format!("{model:?}").contains("super-secret"));
        assert!(!model.endpoint.contains("private"));
    }

    #[test]
    fn queue_faults_and_storage_are_visible() {
        let meeting = MeetingState {
            pending_uploads: 3,
            failed_uploads: 2,
            storage_blocked: true,
            ..Default::default()
        };
        let model = render_model(&WavedConfig::default(), &meeting);
        assert!(model.delivery.contains("3 queued"));
        assert!(model.delivery.contains("2 failed"));
        assert!(model.delivery.contains("Storage full"));
    }

    #[test]
    fn recording_block_uses_only_the_fixed_integrity_diagnostic() {
        let meeting = MeetingState {
            recording_blocked_reason: Some(waved_state::RecordingIntegrityFault::WavWriteFailed),
            ..Default::default()
        };
        let model = render_model(&WavedConfig::default(), &meeting);
        assert!(model.delivery.contains("Recording unavailable"));
        assert!(model.delivery.contains("recording storage write failed"));
    }
}
