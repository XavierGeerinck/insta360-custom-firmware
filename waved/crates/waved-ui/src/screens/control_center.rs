//! Control Center overlay — slides down from top over active screen.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{AudioMode, AudioState, BluetoothState, Command, MotorPosition, NrLevel, StateBus, WifiState};

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_14: lv_font_t;
}

const NR_LEVELS: [NrLevel; 4] = [NrLevel::Off, NrLevel::Low, NrLevel::Med, NrLevel::High];
const MODES: [AudioMode; 5] = [
    AudioMode::Omni, AudioMode::Stereo, AudioMode::Cardioid,
    AudioMode::Supercardioid, AudioMode::Figure8,
];
const MODE_LABELS: [&str; 5] = ["Omni", "Stereo", "Card.", "S-Card.", "Fig-8"];
const NR_LABELS: [&str; 4] = ["Off", "Low", "Med", "High"];

pub struct ControlCenter {
    bus: Arc<StateBus>,
    panel: *mut lv_obj_t,
    visible: bool,
    // Widget pointers for state updates
    mode_btns: [*mut lv_obj_t; 5],
    nr_btns: [*mut lv_obj_t; 4],
    lbl_wifi: *mut lv_obj_t,
    lbl_bt: *mut lv_obj_t,
    active_mode: AudioMode,
    active_nr: NrLevel,
}

unsafe extern "C" fn mode_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let data = lv_event_get_user_data(e)
            as *const (tokio::sync::broadcast::Sender<Command>, usize);
        if !data.is_null() {
            let (tx, idx) = &*data;
            let _ = tx.send(Command::SetAudioMode(MODES[*idx].clone()));
        }
    }
}

unsafe extern "C" fn nr_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let data = lv_event_get_user_data(e)
            as *const (tokio::sync::broadcast::Sender<Command>, usize);
        if !data.is_null() {
            let (tx, idx) = &*data;
            let _ = tx.send(Command::SetNrLevel(NR_LEVELS[*idx].clone()));
        }
    }
}

unsafe extern "C" fn motor_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let data = lv_event_get_user_data(e)
            as *const (tokio::sync::broadcast::Sender<Command>, MotorPosition);
        if !data.is_null() {
            let (tx, pos) = &*data;
            let _ = tx.send(Command::SetMotorPosition(pos.clone()));
        }
    }
}

/// Power Off button long-pressed → shutdown.
unsafe extern "C" fn poweroff_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_LONG_PRESSED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::Shutdown);
        }
    }
}

/// Swipe up on control center panel → close it.
unsafe extern "C" fn swipe_up_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_GESTURE {
        let dir = lv_indev_get_gesture_dir(lv_indev_active());
        if dir == lv_dir_t_LV_DIR_TOP {
            let tx = lv_event_get_user_data(e)
                as *const tokio::sync::broadcast::Sender<Command>;
            if !tx.is_null() {
                // ToggleSpeakerMute is reused as the "toggle control center" UI signal.
                // handle_command() intercepts this to hide the overlay.
                let _ = (*tx).send(Command::ToggleSpeakerMute);
            }
        }
    }
}

impl ControlCenter {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            panel: std::ptr::null_mut(),
            visible: false,
            mode_btns: [std::ptr::null_mut(); 5],
            nr_btns: [std::ptr::null_mut(); 4],
            lbl_wifi: std::ptr::null_mut(),
            lbl_bt: std::ptr::null_mut(),
            active_mode: AudioMode::default(),
            active_nr: NrLevel::default(),
        }
    }

    /// Create the overlay panel on lv_layer_top(). Starts hidden (y = -272).
    pub unsafe fn init(&mut self) {
        let top = lv_layer_top();
        let panel = lv_obj_create(top);
        lv_obj_set_size(panel, 400, 272); // visible area = 304 - 32 (status bar)
        lv_obj_set_pos(panel, 0, -272);
        lv_obj_set_style_bg_color(panel, rgb(0x050f0d), 0);
        lv_obj_set_style_bg_opa(panel, 245, 0);
        lv_obj_set_style_border_width(panel, 0, 0);
        lv_obj_set_style_radius(panel, 0, 0);
        lv_obj_set_style_pad_top(panel, 12, 0);
        lv_obj_set_style_pad_bottom(panel, 12, 0);
        lv_obj_set_style_pad_left(panel, 12, 0);
        lv_obj_set_style_pad_right(panel, 12, 0);
        // Enable scrolling for content taller than the panel
        lv_obj_add_flag(panel, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
        self.panel = panel;

        // ── Header ────────────────────────────────────────────────────────────────
        let hdr = lv_label_create(panel);
        lv_label_set_text(hdr, c"CONTROL CENTER".as_ptr());
        lv_obj_set_style_text_color(hdr, rgb(CLR_ACCENT), 0);
        lv_obj_set_style_text_font(hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 0);

        // ── Audio Mode section (Y=20) ─────────────────────────────────────────────
        let mode_box = lv_obj_create(panel);
        lv_obj_set_size(mode_box, 376, 56);
        lv_obj_align(mode_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 20);
        lv_obj_set_style_bg_color(mode_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(mode_box, 8, 0);
        lv_obj_set_style_border_width(mode_box, 0, 0);

        let lbl_mode_hdr = lv_label_create(mode_box);
        lv_label_set_text(lbl_mode_hdr, c"AUDIO MODE".as_ptr());
        lv_obj_set_style_text_color(lbl_mode_hdr, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(lbl_mode_hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_mode_hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 6);

        let btn_w = (376 - 24 - 4 * 4) / 5;
        for (i, &lbl_text) in MODE_LABELS.iter().enumerate() {
            let btn = lv_button_create(mode_box);
            lv_obj_set_size(btn, btn_w, 24);
            lv_obj_set_pos(btn, 8 + i as i32 * (btn_w + 4), 26);
            lv_obj_set_style_radius(btn, 5, 0);
            lv_obj_set_style_bg_color(btn, rgb(0x071410), 0);
            lv_obj_set_style_border_width(btn, 1, 0);
            lv_obj_set_style_border_color(btn, rgb(CLR_TEXT_DIM), 0);
            let lbl = lv_label_create(btn);
            let s = CString::new(lbl_text).unwrap();
            lv_label_set_text(lbl, s.as_ptr());
            lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
            lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT_DIM), 0);
            lv_obj_center(lbl);
            let data = Box::into_raw(Box::new((self.bus.commands.clone(), i)));
            lv_obj_add_event_cb(
                btn,
                Some(mode_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                data as *mut _,
            );
            self.mode_btns[i] = btn;
        }
        self.highlight_active_mode();

        // ── Noise Reduction section (Y=84) ────────────────────────────────────────
        let nr_box = lv_obj_create(panel);
        lv_obj_set_size(nr_box, 376, 56);
        lv_obj_align(nr_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 84);
        lv_obj_set_style_bg_color(nr_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(nr_box, 8, 0);
        lv_obj_set_style_border_width(nr_box, 0, 0);

        let lbl_nr_hdr = lv_label_create(nr_box);
        lv_label_set_text(lbl_nr_hdr, c"NOISE REDUCTION".as_ptr());
        lv_obj_set_style_text_color(lbl_nr_hdr, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(lbl_nr_hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_nr_hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 6);

        let nr_btn_w = (376 - 24 - 3 * 4) / 4;
        for (i, &lbl_text) in NR_LABELS.iter().enumerate() {
            let btn = lv_button_create(nr_box);
            lv_obj_set_size(btn, nr_btn_w, 24);
            lv_obj_set_pos(btn, 8 + i as i32 * (nr_btn_w + 4), 26);
            lv_obj_set_style_radius(btn, 5, 0);
            lv_obj_set_style_bg_color(btn, rgb(0x071410), 0);
            lv_obj_set_style_border_width(btn, 1, 0);
            lv_obj_set_style_border_color(btn, rgb(CLR_TEXT_DIM), 0);
            let lbl = lv_label_create(btn);
            let s = CString::new(lbl_text).unwrap();
            lv_label_set_text(lbl, s.as_ptr());
            lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
            lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT_DIM), 0);
            lv_obj_center(lbl);
            let data = Box::into_raw(Box::new((self.bus.commands.clone(), i)));
            lv_obj_add_event_cb(
                btn,
                Some(nr_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                data as *mut _,
            );
            self.nr_btns[i] = btn;
        }
        self.highlight_active_nr();

        // ── Mic Arm section (Y=148) ───────────────────────────────────────────────
        let arm_box = lv_obj_create(panel);
        lv_obj_set_size(arm_box, 376, 56);
        lv_obj_align(arm_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 148);
        lv_obj_set_style_bg_color(arm_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(arm_box, 8, 0);
        lv_obj_set_style_border_width(arm_box, 0, 0);

        let lbl_arm_hdr = lv_label_create(arm_box);
        lv_label_set_text(lbl_arm_hdr, c"MIC ARM".as_ptr());
        lv_obj_set_style_text_color(lbl_arm_hdr, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(lbl_arm_hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_arm_hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 6);

        let btn_extend = lv_button_create(arm_box);
        lv_obj_set_size(btn_extend, 180, 24);
        lv_obj_set_pos(btn_extend, 8, 26);
        lv_obj_set_style_radius(btn_extend, 5, 0);
        lv_obj_set_style_bg_color(btn_extend, rgb(CLR_ACCENT), 0);
        lv_obj_set_style_border_width(btn_extend, 0, 0);
        let lbl_ext = lv_label_create(btn_extend);
        lv_label_set_text(lbl_ext, c"UP Extend".as_ptr());
        lv_obj_set_style_text_font(lbl_ext, &lv_font_montserrat_12, 0);
        lv_obj_set_style_text_color(lbl_ext, rgb(CLR_BG), 0);
        lv_obj_center(lbl_ext);
        let extend_data = Box::into_raw(Box::new((self.bus.commands.clone(), MotorPosition::High)));
        lv_obj_add_event_cb(
            btn_extend,
            Some(motor_btn_cb),
            lv_event_code_t_LV_EVENT_CLICKED,
            extend_data as *mut _,
        );

        let btn_retract = lv_button_create(arm_box);
        lv_obj_set_size(btn_retract, 180, 24);
        lv_obj_set_pos(btn_retract, 196, 26);
        lv_obj_set_style_radius(btn_retract, 5, 0);
        lv_obj_set_style_bg_color(btn_retract, rgb(0x071410), 0);
        lv_obj_set_style_border_width(btn_retract, 1, 0);
        lv_obj_set_style_border_color(btn_retract, rgb(CLR_TEXT_DIM), 0);
        let lbl_ret = lv_label_create(btn_retract);
        lv_label_set_text(lbl_ret, c"DN Retract".as_ptr());
        lv_obj_set_style_text_font(lbl_ret, &lv_font_montserrat_12, 0);
        lv_obj_set_style_text_color(lbl_ret, rgb(CLR_TEXT_DIM), 0);
        lv_obj_center(lbl_ret);
        let retract_data = Box::into_raw(Box::new((self.bus.commands.clone(), MotorPosition::Low)));
        lv_obj_add_event_cb(
            btn_retract,
            Some(motor_btn_cb),
            lv_event_code_t_LV_EVENT_CLICKED,
            retract_data as *mut _,
        );

        // ── WiFi section (Y=212) ──────────────────────────────────────────────────
        let wifi_box = lv_obj_create(panel);
        lv_obj_set_size(wifi_box, 376, 56);
        lv_obj_align(wifi_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 212);
        lv_obj_set_style_bg_color(wifi_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(wifi_box, 8, 0);
        lv_obj_set_style_border_width(wifi_box, 0, 0);

        let lbl_wifi_hdr = lv_label_create(wifi_box);
        lv_label_set_text(lbl_wifi_hdr, c"WIFI".as_ptr());
        lv_obj_set_style_text_color(lbl_wifi_hdr, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(lbl_wifi_hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_wifi_hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 6);

        let lbl_wifi = lv_label_create(wifi_box);
        lv_label_set_text(lbl_wifi, c"Not connected".as_ptr());
        lv_obj_set_style_text_color(lbl_wifi, rgb(CLR_TEXT), 0);
        lv_obj_set_style_text_font(lbl_wifi, &lv_font_montserrat_14, 0);
        lv_obj_align(lbl_wifi, lv_align_t_LV_ALIGN_BOTTOM_LEFT, 8, -6);
        self.lbl_wifi = lbl_wifi;

        // ── Bluetooth section (Y=276) ─────────────────────────────────────────────
        let bt_box = lv_obj_create(panel);
        lv_obj_set_size(bt_box, 376, 56);
        lv_obj_align(bt_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 276);
        lv_obj_set_style_bg_color(bt_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(bt_box, 8, 0);
        lv_obj_set_style_border_width(bt_box, 0, 0);

        let lbl_bt_hdr = lv_label_create(bt_box);
        lv_label_set_text(lbl_bt_hdr, c"BLUETOOTH".as_ptr());
        lv_obj_set_style_text_color(lbl_bt_hdr, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(lbl_bt_hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_bt_hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 6);

        let lbl_bt = lv_label_create(bt_box);
        lv_label_set_text(lbl_bt, c"Not connected".as_ptr());
        lv_obj_set_style_text_color(lbl_bt, rgb(CLR_TEXT), 0);
        lv_obj_set_style_text_font(lbl_bt, &lv_font_montserrat_14, 0);
        lv_obj_align(lbl_bt, lv_align_t_LV_ALIGN_BOTTOM_LEFT, 8, -6);
        self.lbl_bt = lbl_bt;

        // ── About section (Y=340) ─────────────────────────────────────────────────
        let about_box = lv_obj_create(panel);
        lv_obj_set_size(about_box, 376, 56);
        lv_obj_align(about_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 340);
        lv_obj_set_style_bg_color(about_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(about_box, 8, 0);
        lv_obj_set_style_border_width(about_box, 0, 0);

        let lbl_about_hdr = lv_label_create(about_box);
        lv_label_set_text(lbl_about_hdr, c"ABOUT".as_ptr());
        lv_obj_set_style_text_color(lbl_about_hdr, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(lbl_about_hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_about_hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 6);

        let lbl_version = lv_label_create(about_box);
        lv_label_set_text(lbl_version, c"waved v0.1.0".as_ptr());
        lv_obj_set_style_text_color(lbl_version, rgb(CLR_TEXT), 0);
        lv_obj_set_style_text_font(lbl_version, &lv_font_montserrat_14, 0);
        lv_obj_align(lbl_version, lv_align_t_LV_ALIGN_BOTTOM_LEFT, 8, -6);

        // ── Power Off section (Y=404) ─────────────────────────────────────────────
        let poweroff_box = lv_obj_create(panel);
        lv_obj_set_size(poweroff_box, 376, 56);
        lv_obj_align(poweroff_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 404);
        lv_obj_set_style_bg_color(poweroff_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(poweroff_box, 8, 0);
        lv_obj_set_style_border_width(poweroff_box, 0, 0);

        let btn_poweroff = lv_button_create(poweroff_box);
        lv_obj_set_size(btn_poweroff, 376 - 16, 32);
        lv_obj_set_pos(btn_poweroff, 8, 12);
        lv_obj_set_style_radius(btn_poweroff, 8, 0);
        lv_obj_set_style_bg_color(btn_poweroff, rgb(CLR_RED), 0);
        lv_obj_set_style_border_width(btn_poweroff, 0, 0);
        let lbl_poff = lv_label_create(btn_poweroff);
        lv_label_set_text(lbl_poff, c"Hold to Power Off".as_ptr());
        lv_obj_set_style_text_font(lbl_poff, &lv_font_montserrat_12, 0);
        lv_obj_set_style_text_color(lbl_poff, rgb(CLR_WHITE), 0);
        lv_obj_center(lbl_poff);
        let poweroff_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
        lv_obj_add_event_cb(
            btn_poweroff,
            Some(poweroff_btn_cb),
            lv_event_code_t_LV_EVENT_LONG_PRESSED,
            poweroff_tx as *mut _,
        );

        // ── "swipe up to close" hint (Y=468) ─────────────────────────────────────
        let hint = lv_label_create(panel);
        lv_label_set_text(hint, c"swipe up to close".as_ptr());
        lv_obj_set_style_text_color(hint, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(hint, &lv_font_montserrat_12, 0);
        lv_obj_align(hint, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 468);

        // ── Swipe up gesture → close ──────────────────────────────────────────────
        // Register on lv_layer_top() (non-scrollable parent) to avoid conflict with
        // the panel's scroll behavior. Enable LV_OBJ_FLAG_GESTURE_BUBBLE on panel so
        // gestures originating on the panel propagate up to layer_top.
        lv_obj_add_flag(panel, lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE);
        let gesture_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
        lv_obj_add_event_cb(
            top,
            Some(swipe_up_cb),
            lv_event_code_t_LV_EVENT_GESTURE,
            gesture_tx as *mut _,
        );
    }

    unsafe fn highlight_active_nr(&self) {
        for (i, &btn) in self.nr_btns.iter().enumerate() {
            if btn.is_null() { continue; }
            let is_active = NR_LEVELS[i] == self.active_nr;
            let border_clr = if is_active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
            let text_clr   = if is_active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
            lv_obj_set_style_border_color(btn, border_clr, 0);
            let lbl = lv_obj_get_child(btn, 0);
            if !lbl.is_null() {
                lv_obj_set_style_text_color(lbl, text_clr, 0);
            }
        }
    }

    unsafe fn highlight_active_mode(&self) {
        // NOTE: AudioMode::NoiseReduction is not in MODES — when active, no button
        // is highlighted. This is intentional: NR is a processing overlay, not a
        // pickup pattern. The NR Level section controls its intensity.
        for (i, &btn) in self.mode_btns.iter().enumerate() {
            if btn.is_null() { continue; }
            let is_active = MODES[i] == self.active_mode;
            let border_clr = if is_active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
            lv_obj_set_style_border_color(btn, border_clr, 0);
            let lbl = lv_obj_get_child(btn, 0);
            if !lbl.is_null() {
                let text_clr = if is_active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
                lv_obj_set_style_text_color(lbl, text_clr, 0);
            }
        }
    }

    pub unsafe fn update_state(
        &mut self,
        audio: &AudioState,
        wifi: &WifiState,
        bt: &BluetoothState,
    ) {
        // Audio mode highlight
        if audio.mode != self.active_mode {
            self.active_mode = audio.mode.clone();
            self.highlight_active_mode();
        }
        // NR highlight
        if audio.nr_level != self.active_nr {
            self.active_nr = audio.nr_level.clone();
            self.highlight_active_nr();
        }
        // WiFi label
        if !self.lbl_wifi.is_null() {
            let wifi_text = if wifi.connected {
                CString::new(format!("{} \u{25CF}", wifi.ssid)).unwrap()
            } else {
                CString::new("Not connected").unwrap()
            };
            lv_label_set_text(self.lbl_wifi, wifi_text.as_ptr());
        }
        // Bluetooth label
        if !self.lbl_bt.is_null() {
            let bt_text = if bt.connected {
                CString::new(format!("{} \u{25CF}", bt.device_name)).unwrap()
            } else {
                CString::new("Not connected").unwrap()
            };
            lv_label_set_text(self.lbl_bt, bt_text.as_ptr());
        }
    }

    pub unsafe fn show(&mut self) {
        if self.visible { return; }
        self.visible = true;
        let mut anim = lv_anim_t::default();
        lv_anim_init(&mut anim);
        lv_anim_set_var(&mut anim, self.panel as *mut _);
        lv_anim_set_values(&mut anim, -272, 32);
        lv_anim_set_duration(&mut anim, 200u32);
        lv_anim_set_exec_cb(&mut anim, Some(anim_y_cb));
        lv_anim_start(&anim);
    }

    pub unsafe fn hide(&mut self) {
        if !self.visible { return; }
        self.visible = false;
        let mut anim = lv_anim_t::default();
        lv_anim_init(&mut anim);
        lv_anim_set_var(&mut anim, self.panel as *mut _);
        lv_anim_set_values(&mut anim, 32, -272);
        lv_anim_set_duration(&mut anim, 200u32);
        lv_anim_set_exec_cb(&mut anim, Some(anim_y_cb));
        lv_anim_start(&anim);
    }

    pub fn is_visible(&self) -> bool { self.visible }
}

unsafe extern "C" fn anim_y_cb(obj: *mut core::ffi::c_void, value: i32) {
    lv_obj_set_y(obj as *mut lv_obj_t, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nr_levels_array_matches_buttons() {
        assert_eq!(NR_LEVELS.len(), 4);
    }

    #[test]
    fn control_center_starts_hidden() {
        let bus = Arc::new(waved_state::StateBus::new());
        let cc = ControlCenter::new(bus);
        assert!(!cc.is_visible());
    }
}
