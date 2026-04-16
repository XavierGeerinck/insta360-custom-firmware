//! Control Center overlay — slides down from top over active screen.
//!
//! Main view: 3 circular shortcut buttons (Audio Mode, Noise Reduction, Mic Arm)
//! styled like the home-screen Vol/Mic buttons.  Tapping any button opens a
//! full-panel sub-page for that setting; sub-pages have a "< Back" bar to
//! return to the main view.
//!
//! Page navigation is handled via raw pointer callbacks within the UI thread:
//! `ControlCenter` lives on the UI thread stack for the process lifetime and
//! never moves, so `*mut ControlCenter` stored in `NavData` is always valid
//! when a callback fires (during `lv_timer_handler`, not during `init`).

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
// Mode order matches original pspapp beamforming screen: all, heart, super_heart, eight, stereo.
const MODES: [AudioMode; 5] = [
    AudioMode::Omni, AudioMode::Cardioid, AudioMode::Supercardioid,
    AudioMode::Figure8, AudioMode::Stereo,
];
const MODE_LABELS: [&str; 5] = ["Omni", "Cardioid", "Supercardioid", "Figure-8", "Stereo"];
const NR_LABELS:   [&str; 4] = ["Off", "Low", "Med", "High"];

// Beamforming assets — order matches MODES (and original pspapp strip order):
// Omni(all), Cardioid(heart), Supercardioid(super_heart), Figure8(eight), Stereo(stereo).
const BF_BG_ICONS: [&[u8]; 5] = [
    b"A:/etc/pspapp/lvgl/icon/beamforming/bg_bf_mode_all.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bg_bf_mode_heart.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bg_bf_mode_super_heart.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bg_bf_mode_eight.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bg_bf_mode_stereo.png\0",
];
// Indicator-strip check/uncheck variants (160×24 strip at bottom of audio page).
const BF_ICONS_CHECK: [&[u8]; 5] = [
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_all_check.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_heart_check.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_super_heart_check.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_eight_check.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_stereo_check.png\0",
];
const BF_ICONS_UNCHECK: [&[u8]; 5] = [
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_all_uncheck.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_heart_uncheck.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_super_heart_uncheck.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_eight_uncheck.png\0",
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_stereo_uncheck.png\0",
];

// ── Page identifier ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum CcPage { Main, Audio, NR, Arm }

// ── Navigation callback user_data ─────────────────────────────────────────────

struct NavData {
    cc:   *mut ControlCenter,
    page: CcPage,
}

// ── Action callbacks ──────────────────────────────────────────────────────────

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

/// Close CC overlay (intercepted by handle_command → cc.hide()).
unsafe extern "C" fn close_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::ToggleSpeakerMute);
        }
    }
}

/// Navigate to a CC sub-page (or back to main).
unsafe extern "C" fn nav_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let data = lv_event_get_user_data(e) as *const NavData;
        if !data.is_null() {
            (*(*data).cc).show_page((*data).page);
        }
    }
}

unsafe extern "C" fn swipe_up_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_GESTURE {
        let dir = lv_indev_get_gesture_dir(lv_indev_active());
        if dir == lv_dir_t_LV_DIR_TOP {
            let tx = lv_event_get_user_data(e)
                as *const tokio::sync::broadcast::Sender<Command>;
            if !tx.is_null() {
                let _ = (*tx).send(Command::ToggleSpeakerMute);
            }
        }
    }
}

// ── ControlCenter ─────────────────────────────────────────────────────────────

pub struct ControlCenter {
    bus:       Arc<StateBus>,
    panel:     *mut lv_obj_t,
    visible:   bool,
    // Page containers — full-size children of panel, toggled via HIDDEN flag.
    main_page: *mut lv_obj_t,
    sub_audio: *mut lv_obj_t,
    sub_nr:    *mut lv_obj_t,
    sub_arm:   *mut lv_obj_t,
    // Sub-page option buttons — border/text colour reflects active state.
    mode_btns: [*mut lv_obj_t; 5],
    nr_btns:   [*mut lv_obj_t; 4],
    // Indicator strip icons in the audio sub-page header (check/uncheck swapped on mode change).
    indicator_icons: [*mut lv_obj_t; 5],
    active_mode: AudioMode,
    active_nr:   NrLevel,
}

impl ControlCenter {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            panel:     std::ptr::null_mut(),
            visible:   false,
            main_page: std::ptr::null_mut(),
            sub_audio: std::ptr::null_mut(),
            sub_nr:    std::ptr::null_mut(),
            sub_arm:   std::ptr::null_mut(),
            mode_btns:       [std::ptr::null_mut(); 5],
            nr_btns:         [std::ptr::null_mut(); 4],
            indicator_icons: [std::ptr::null_mut(); 5],
            active_mode: AudioMode::default(),
            active_nr:   NrLevel::default(),
        }
    }

    /// Show one CC page and hide all others.
    unsafe fn show_page(&mut self, page: CcPage) {
        let pages = [
            (self.main_page, CcPage::Main),
            (self.sub_audio, CcPage::Audio),
            (self.sub_nr,    CcPage::NR),
            (self.sub_arm,   CcPage::Arm),
        ];
        for (obj, id) in pages {
            if obj.is_null() { continue; }
            if id == page {
                lv_obj_remove_flag(obj, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
            } else {
                lv_obj_add_flag(obj, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
            }
        }
    }

    /// Create the overlay and all pages. Call once before the UI loop.
    pub unsafe fn init(&mut self) {
        let cc_ptr = self as *mut Self;

        let top = lv_layer_top();
        // lv_layer_top() is CLICKABLE by default in LVGL 9 and sits above
        // the active screen. Removing the flag makes the layer input-transparent
        // so touches reach the screen below when the CC is not shown.
        lv_obj_remove_flag(top, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

        // ── Outer panel ───────────────────────────────────────────────────────────
        let panel = lv_obj_create(top);
        lv_obj_set_size(panel, 400, 272);
        lv_obj_set_pos(panel, 0, -272);   // starts above screen; show() animates to y=32
        lv_obj_set_style_bg_color(panel, rgb(0x050f0d), 0);
        lv_obj_set_style_bg_opa(panel, 245, 0);
        lv_obj_set_style_border_width(panel, 0, 0);
        lv_obj_set_style_radius(panel, 0, 0);
        lv_obj_set_style_pad_top(panel, 0, 0);
        lv_obj_set_style_pad_bottom(panel, 0, 0);
        lv_obj_set_style_pad_left(panel, 0, 0);
        lv_obj_set_style_pad_right(panel, 0, 0);
        lv_obj_remove_flag(panel, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
        self.panel = panel;

        // ── Create all pages (full-size children of panel) ────────────────────────
        self.main_page = self.create_main_page(cc_ptr);
        self.sub_audio = self.create_sub_audio(cc_ptr);
        self.sub_nr    = self.create_sub_nr(cc_ptr);
        self.sub_arm   = self.create_sub_arm(cc_ptr);
        self.show_page(CcPage::Main);

        // ── Swipe up on panel → close CC ─────────────────────────────────────────
        // With non-scrollable pages this gesture fires reliably.
        let gesture_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
        lv_obj_add_event_cb(
            panel,
            Some(swipe_up_cb),
            lv_event_code_t_LV_EVENT_GESTURE,
            gesture_tx as *mut _,
        );
    }

    /// Allocate a blank full-panel page container with 12 px padding on all sides.
    /// Content area per page: 376 × 248 px.
    unsafe fn make_page(&self) -> *mut lv_obj_t {
        let page = lv_obj_create(self.panel);
        lv_obj_set_size(page, 400, 272);
        lv_obj_set_pos(page, 0, 0);
        lv_obj_set_style_bg_color(page, rgb(0x050f0d), 0);
        lv_obj_set_style_bg_opa(page, 255, 0);
        lv_obj_set_style_border_width(page, 0, 0);
        lv_obj_set_style_radius(page, 0, 0);
        lv_obj_set_style_pad_top(page, 12, 0);
        lv_obj_set_style_pad_bottom(page, 12, 0);
        lv_obj_set_style_pad_left(page, 12, 0);
        lv_obj_set_style_pad_right(page, 12, 0);
        lv_obj_remove_flag(page, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
        page
    }

    // ── Main page ─────────────────────────────────────────────────────────────────
    //
    // Layout (content area 376 × 248):
    //   y=0   "CONTROL CENTER" title (left) + CLOSE button (right)
    //   y=40  3 × 76×76 circular buttons: Audio | NR | Arm
    //         x offsets: 37, 150, 263  (37 px equal margins / gaps in 376 px)
    //
    unsafe fn create_main_page(&self, cc_ptr: *mut ControlCenter) -> *mut lv_obj_t {
        let page = self.make_page();

        // Title
        let hdr = lv_label_create(page);
        lv_label_set_text(hdr, c"CONTROL CENTER".as_ptr());
        lv_obj_set_style_text_color(hdr, rgb(CLR_ACCENT), 0);
        lv_obj_set_style_text_font(hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 6);

        // CLOSE button (top-right)
        let btn_close = lv_button_create(page);
        lv_obj_set_size(btn_close, 64, 24);
        lv_obj_align(btn_close, lv_align_t_LV_ALIGN_TOP_RIGHT, 0, 0);
        lv_obj_set_style_radius(btn_close, 6, 0);
        lv_obj_set_style_bg_color(btn_close, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_border_width(btn_close, 1, 0);
        lv_obj_set_style_border_color(btn_close, rgb(CLR_TEXT_DIM), 0);
        let lbl_x = lv_label_create(btn_close);
        lv_label_set_text(lbl_x, c"CLOSE".as_ptr());
        lv_obj_set_style_text_font(lbl_x, &lv_font_montserrat_12, 0);
        lv_obj_set_style_text_color(lbl_x, rgb(CLR_TEXT_DIM), 0);
        lv_obj_center(lbl_x);
        let close_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
        lv_obj_add_event_cb(
            btn_close,
            Some(close_btn_cb),
            lv_event_code_t_LV_EVENT_CLICKED,
            close_tx as *mut _,
        );

        // 3 circular shortcut buttons (76×76, same look as home-screen vol/mic)
        struct BtnSpec {
            bg:    &'static [u8],
            icon:  &'static [u8],
            label: &'static str,
            page:  CcPage,
        }
        let specs = [
            BtnSpec {
                bg:    b"A:/etc/pspapp/lvgl/icon/msc/bg_main_spk_btn.png\0",
                icon:  b"A:/etc/pspapp/lvgl/icon/volume/spk_on.png\0",
                label: "Audio",
                page:  CcPage::Audio,
            },
            BtnSpec {
                bg:    b"A:/etc/pspapp/lvgl/icon/msc/bg_main_mic_btn.png\0",
                icon:  b"A:/etc/pspapp/lvgl/icon/volume/mic_on.png\0",
                label: "NR",
                page:  CcPage::NR,
            },
            BtnSpec {
                bg:    b"A:/etc/pspapp/lvgl/icon/msc/bg_main_mic_btn.png\0",
                icon:  b"A:/etc/pspapp/lvgl/icon/volume/mic_on.png\0",
                label: "Arm",
                page:  CcPage::Arm,
            },
        ];
        // x offsets: (376 - 3×76) / 4 = 37 px margin/gap
        let x_offsets: [i32; 3] = [37, 150, 263];

        for (i, spec) in specs.iter().enumerate() {
            let btn = lv_image_create(page);
            lv_image_set_src(btn, spec.bg.as_ptr() as *const _);
            lv_obj_align(btn, lv_align_t_LV_ALIGN_TOP_LEFT, x_offsets[i], 40);
            lv_obj_set_style_border_width(btn, 0, 0);
            lv_obj_add_flag(btn, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

            let icon = lv_image_create(btn);
            lv_image_set_src(icon, spec.icon.as_ptr() as *const _);
            lv_obj_align(icon, lv_align_t_LV_ALIGN_TOP_MID, 0, 8);

            let lbl = lv_label_create(btn);
            let s = CString::new(spec.label).unwrap();
            lv_label_set_text(lbl, s.as_ptr());
            lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
            lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT_DIM), 0);
            lv_obj_align(lbl, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -4);

            let nav = Box::into_raw(Box::new(NavData { cc: cc_ptr, page: spec.page }));
            lv_obj_add_event_cb(
                btn,
                Some(nav_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                nav as *mut _,
            );
        }

        page
    }

    // ── Sub-page helpers ──────────────────────────────────────────────────────────

    /// Add a full-width "< Back | TITLE" bar at the top of a sub-page.
    /// Clicking anywhere on the bar returns to the main page.
    unsafe fn add_back_header(page: *mut lv_obj_t, title: &str, cc_ptr: *mut ControlCenter) {
        let bar = lv_button_create(page);
        lv_obj_set_size(bar, 376, 28);
        lv_obj_align(bar, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 0);
        lv_obj_set_style_radius(bar, 0, 0);
        lv_obj_set_style_bg_color(bar, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_border_width(bar, 0, 0);

        let lbl_back = lv_label_create(bar);
        lv_label_set_text(lbl_back, c"< Back".as_ptr());
        lv_obj_set_style_text_font(lbl_back, &lv_font_montserrat_12, 0);
        lv_obj_set_style_text_color(lbl_back, rgb(CLR_TEXT_DIM), 0);
        lv_obj_align(lbl_back, lv_align_t_LV_ALIGN_LEFT_MID, 8, 0);

        let lbl_title = lv_label_create(bar);
        let s = CString::new(title).unwrap();
        lv_label_set_text(lbl_title, s.as_ptr());
        lv_obj_set_style_text_font(lbl_title, &lv_font_montserrat_14, 0);
        lv_obj_set_style_text_color(lbl_title, rgb(CLR_ACCENT), 0);
        lv_obj_center(lbl_title);

        let nav = Box::into_raw(Box::new(NavData { cc: cc_ptr, page: CcPage::Main }));
        lv_obj_add_event_cb(bar, Some(nav_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, nav as *mut _);
    }

    // ── Audio Mode sub-page ───────────────────────────────────────────────────────
    //
    // Replicates the original pspapp ui_setting_beamforming_screen layout.
    //
    // Full 400×272 panel (no padding — carousel needs the full width):
    //   y=0    "< Back | AUDIO MODE" bar, 400×28 px
    //   y=32   Horizontal card carousel, 400×167 px (mirrors FUN_0017b178)
    //          ROW flex, scroll_snap_x=CENTER, SCROLL_ONE, 16 px column gap, CENTER align
    //          5 cards × 307×167 px (mirrors FUN_0017afa4):
    //            - bg pattern image (124×124) at pos(16, 21): bg_bf_mode_*.png
    //            - radio button (36×36) at TOP_RIGHT
    //            - mode name label (w=142) at OUT_RIGHT_TOP of bg (+10, +10)
    //            - active: 2 px accent border on all sides; inactive: border hidden
    //   y=203  Indicator strip, 160×24 px, TOP_MID (mirrors FUN_0017a8d0)
    //          Icons chained L→R with 18 px gap: all, heart, super_heart, eight, stereo
    //
    unsafe fn create_sub_audio(&mut self, cc_ptr: *mut ControlCenter) -> *mut lv_obj_t {
        // Zero-padding page so the carousel can use the full 400 px panel width.
        let page = lv_obj_create(self.panel);
        lv_obj_set_size(page, 400, 272);
        lv_obj_set_pos(page, 0, 0);
        lv_obj_set_style_bg_color(page, rgb(0x050f0d), 0);
        lv_obj_set_style_bg_opa(page, 255, 0);
        lv_obj_set_style_border_width(page, 0, 0);
        lv_obj_set_style_radius(page, 0, 0);
        lv_obj_set_style_pad_top(page, 0, 0);
        lv_obj_set_style_pad_bottom(page, 0, 0);
        lv_obj_set_style_pad_left(page, 0, 0);
        lv_obj_set_style_pad_right(page, 0, 0);
        lv_obj_remove_flag(page, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

        // Back header — full panel width.
        let bar = lv_button_create(page);
        lv_obj_set_size(bar, 400, 28);
        lv_obj_align(bar, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 0);
        lv_obj_set_style_radius(bar, 0, 0);
        lv_obj_set_style_bg_color(bar, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_border_width(bar, 0, 0);
        let lbl_back = lv_label_create(bar);
        lv_label_set_text(lbl_back, c"< Back".as_ptr());
        lv_obj_set_style_text_font(lbl_back, &lv_font_montserrat_12, 0);
        lv_obj_set_style_text_color(lbl_back, rgb(CLR_TEXT_DIM), 0);
        lv_obj_align(lbl_back, lv_align_t_LV_ALIGN_LEFT_MID, 8, 0);
        let lbl_title = lv_label_create(bar);
        lv_label_set_text(lbl_title, c"AUDIO MODE".as_ptr());
        lv_obj_set_style_text_font(lbl_title, &lv_font_montserrat_14, 0);
        lv_obj_set_style_text_color(lbl_title, rgb(CLR_ACCENT), 0);
        lv_obj_center(lbl_title);
        let nav = Box::into_raw(Box::new(NavData { cc: cc_ptr, page: CcPage::Main }));
        lv_obj_add_event_cb(bar, Some(nav_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, nav as *mut _);

        // ── Horizontal card carousel (mirrors FUN_0017b178) ───────────────────────
        let carousel = lv_obj_create(page);
        lv_obj_set_size(carousel, 400, 167);
        lv_obj_align(carousel, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 32);
        lv_obj_set_style_bg_opa(carousel, 0, 0);
        lv_obj_set_style_border_width(carousel, 0, 0);
        lv_obj_set_style_pad_top(carousel, 0, 0);
        lv_obj_set_style_pad_bottom(carousel, 0, 0);
        lv_obj_set_style_pad_left(carousel, 0, 0);
        lv_obj_set_style_pad_right(carousel, 0, 0);
        lv_obj_set_style_pad_column(carousel, 16, 0);
        lv_obj_set_flex_flow(carousel, lv_flex_flow_t_LV_FLEX_FLOW_ROW);
        lv_obj_set_flex_align(
            carousel,
            lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
        );
        lv_obj_set_scroll_dir(carousel, lv_dir_t_LV_DIR_HOR);
        lv_obj_set_scroll_snap_x(carousel, lv_scroll_snap_t_LV_SCROLL_SNAP_CENTER);
        lv_obj_add_flag(carousel, lv_obj_flag_t_LV_OBJ_FLAG_SCROLL_ONE);

        // ── 5 mode cards (mirrors FUN_0017afa4 + FUN_0017b25c) ───────────────────
        for (i, &label) in MODE_LABELS.iter().enumerate() {
            let card = lv_obj_create(carousel);
            lv_obj_set_size(card, 307, 167);
            lv_obj_set_style_bg_color(card, rgb(0xffffff), 0);
            lv_obj_set_style_bg_opa(card, 25, 0);
            lv_obj_set_style_border_width(card, 2, 0);
            lv_obj_set_style_border_color(card, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_border_side(card, 0, 0); // hidden until active
            lv_obj_set_style_radius(card, 16, 0);
            lv_obj_set_style_pad_top(card, 0, 0);
            lv_obj_set_style_pad_bottom(card, 0, 0);
            lv_obj_set_style_pad_left(card, 0, 0);
            lv_obj_set_style_pad_right(card, 0, 0);
            lv_obj_remove_flag(card, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            lv_obj_add_flag(card, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

            // Radio button (36×36) at TOP_RIGHT.
            let radio = lv_image_create(card);
            lv_obj_set_size(radio, 36, 36);
            lv_image_set_src(radio, b"A:/etc/pspapp/lvgl/icon/btn/radio_unchecked.png\0".as_ptr() as *const _);
            lv_obj_align(radio, lv_align_t_LV_ALIGN_TOP_RIGHT, 0, 0);

            // Background pattern image (124×124) at pos(16, 21).
            let bg_img = lv_image_create(card);
            lv_obj_set_size(bg_img, 124, 124);
            lv_obj_set_pos(bg_img, 16, 21);
            lv_image_set_src(bg_img, BF_BG_ICONS[i].as_ptr() as *const _);

            // Mode name label (w=142) at OUT_RIGHT_TOP of bg image (+10, +10).
            let lbl = lv_label_create(card);
            let s = CString::new(label).unwrap();
            lv_label_set_text(lbl, s.as_ptr());
            lv_obj_set_style_text_font(lbl, &lv_font_montserrat_14, 0);
            lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_width(lbl, 142);
            lv_obj_align_to(lbl, bg_img, lv_align_t_LV_ALIGN_OUT_RIGHT_TOP, 10, 10);

            let data = Box::into_raw(Box::new((self.bus.commands.clone(), i)));
            lv_obj_add_event_cb(card, Some(mode_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, data as *mut _);
            self.mode_btns[i] = card;
        }

        // ── Indicator strip (mirrors FUN_0017a8d0) ────────────────────────────────
        // 160×24, icons chained L→R with 18 px gap.
        // First icon (i=0) uses LEFT_MID; each subsequent uses OUT_RIGHT_MID of previous.
        let strip = lv_obj_create(page);
        lv_obj_set_size(strip, 160, 24);
        lv_obj_set_style_bg_opa(strip, 0, 0);
        lv_obj_set_style_border_width(strip, 0, 0);
        lv_obj_set_style_pad_top(strip, 0, 0);
        lv_obj_set_style_pad_bottom(strip, 0, 0);
        lv_obj_set_style_pad_left(strip, 0, 0);
        lv_obj_set_style_pad_right(strip, 0, 0);
        lv_obj_remove_flag(strip, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
        lv_obj_align(strip, lv_align_t_LV_ALIGN_TOP_MID, 0, 203);

        let mut prev_icon: *mut lv_obj_t = std::ptr::null_mut();
        for i in 0..5usize {
            let icon = lv_image_create(strip);
            lv_image_set_src(icon, BF_ICONS_UNCHECK[i].as_ptr() as *const _);
            if prev_icon.is_null() {
                lv_obj_align(icon, lv_align_t_LV_ALIGN_LEFT_MID, 0, 0);
            } else {
                lv_obj_align_to(icon, prev_icon, lv_align_t_LV_ALIGN_OUT_RIGHT_MID, 18, 0);
            }
            self.indicator_icons[i] = icon;
            prev_icon = icon;
        }

        self.highlight_active_mode();
        page
    }

    // ── Noise Reduction sub-page ──────────────────────────────────────────────────
    //
    // Layout (content area 376 × 248):
    //   y=0    "< Back | NOISE REDUCTION" bar (28 px)
    //   y=36   4 full-width option buttons (44 px each, 8 px gap → step 52)
    //          last button bottom: 192 + 44 = 236 < 248 ✓
    //
    unsafe fn create_sub_nr(&mut self, cc_ptr: *mut ControlCenter) -> *mut lv_obj_t {
        let page = self.make_page();
        Self::add_back_header(page, "NOISE REDUCTION", cc_ptr);

        for (i, &label) in NR_LABELS.iter().enumerate() {
            let btn = lv_button_create(page);
            lv_obj_set_size(btn, 376, 44);
            lv_obj_align(btn, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 36 + i as i32 * 52);
            lv_obj_set_style_radius(btn, 8, 0);
            lv_obj_set_style_bg_color(btn, rgb(CLR_SURFACE), 0);
            lv_obj_set_style_border_width(btn, 1, 0);
            lv_obj_set_style_border_color(btn, rgb(CLR_TEXT_DIM), 0);
            let lbl = lv_label_create(btn);
            let s = CString::new(label).unwrap();
            lv_label_set_text(lbl, s.as_ptr());
            lv_obj_set_style_text_font(lbl, &lv_font_montserrat_14, 0);
            lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT_DIM), 0);
            lv_obj_center(lbl);
            let data = Box::into_raw(Box::new((self.bus.commands.clone(), i)));
            lv_obj_add_event_cb(btn, Some(nr_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, data as *mut _);
            self.nr_btns[i] = btn;
        }
        self.highlight_active_nr();
        page
    }

    // ── Mic Arm sub-page ──────────────────────────────────────────────────────────
    //
    // Layout (content area 376 × 248):
    //   y=0    "< Back | MIC ARM" bar (28 px)
    //   y=36   "Extend Mic Arm" (accent, 72 px)
    //   y=120  "Retract Mic Arm" (surface, 72 px)
    //
    unsafe fn create_sub_arm(&self, cc_ptr: *mut ControlCenter) -> *mut lv_obj_t {
        let page = self.make_page();
        Self::add_back_header(page, "MIC ARM", cc_ptr);

        let btn_ext = lv_button_create(page);
        lv_obj_set_size(btn_ext, 376, 72);
        lv_obj_align(btn_ext, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 36);
        lv_obj_set_style_radius(btn_ext, 10, 0);
        lv_obj_set_style_bg_color(btn_ext, rgb(CLR_ACCENT), 0);
        lv_obj_set_style_border_width(btn_ext, 0, 0);
        let lbl_ext = lv_label_create(btn_ext);
        lv_label_set_text(lbl_ext, c"Extend Mic Arm".as_ptr());
        lv_obj_set_style_text_font(lbl_ext, &lv_font_montserrat_14, 0);
        lv_obj_set_style_text_color(lbl_ext, rgb(CLR_BG), 0);
        lv_obj_center(lbl_ext);
        let ext_data = Box::into_raw(Box::new((self.bus.commands.clone(), MotorPosition::High)));
        lv_obj_add_event_cb(btn_ext, Some(motor_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, ext_data as *mut _);

        let btn_ret = lv_button_create(page);
        lv_obj_set_size(btn_ret, 376, 72);
        lv_obj_align(btn_ret, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 120);
        lv_obj_set_style_radius(btn_ret, 10, 0);
        lv_obj_set_style_bg_color(btn_ret, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_border_width(btn_ret, 1, 0);
        lv_obj_set_style_border_color(btn_ret, rgb(CLR_TEXT_DIM), 0);
        let lbl_ret = lv_label_create(btn_ret);
        lv_label_set_text(lbl_ret, c"Retract Mic Arm".as_ptr());
        lv_obj_set_style_text_font(lbl_ret, &lv_font_montserrat_14, 0);
        lv_obj_set_style_text_color(lbl_ret, rgb(CLR_TEXT_DIM), 0);
        lv_obj_center(lbl_ret);
        let ret_data = Box::into_raw(Box::new((self.bus.commands.clone(), MotorPosition::Low)));
        lv_obj_add_event_cb(btn_ret, Some(motor_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, ret_data as *mut _);

        page
    }

    // ── State update helpers ──────────────────────────────────────────────────────

    unsafe fn highlight_active_nr(&self) {
        for (i, &btn) in self.nr_btns.iter().enumerate() {
            if btn.is_null() { continue; }
            let active = NR_LEVELS[i] == self.active_nr;
            let clr = if active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
            lv_obj_set_style_border_color(btn, clr, 0);
            let lbl = lv_obj_get_child(btn, 0);
            if !lbl.is_null() { lv_obj_set_style_text_color(lbl, clr, 0); }
        }
    }

    unsafe fn highlight_active_mode(&self) {
        for (i, &card) in self.mode_btns.iter().enumerate() {
            let active = MODES[i] == self.active_mode;

            // Card: show/hide accent border (0xf = all sides, 0 = none).
            if !card.is_null() {
                lv_obj_set_style_border_side(card, if active { 0xf } else { 0 }, 0);

                // Radio button is child 0; swap checked / unchecked image.
                let radio = lv_obj_get_child(card, 0);
                if !radio.is_null() {
                    let src: &[u8] = if active {
                        b"A:/etc/pspapp/lvgl/icon/btn/radio_checked.png\0"
                    } else {
                        b"A:/etc/pspapp/lvgl/icon/btn/radio_unchecked.png\0"
                    };
                    lv_image_set_src(radio, src.as_ptr() as *const _);
                }

                // Label is child 2; accent colour when active.
                let lbl = lv_obj_get_child(card, 2);
                if !lbl.is_null() {
                    let clr = if active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
                    lv_obj_set_style_text_color(lbl, clr, 0);
                }
            }

            // Indicator-strip icon: swap between check and uncheck variant.
            let strip_icon = self.indicator_icons[i];
            if !strip_icon.is_null() {
                let src = if active {
                    BF_ICONS_CHECK[i].as_ptr()
                } else {
                    BF_ICONS_UNCHECK[i].as_ptr()
                };
                lv_image_set_src(strip_icon, src as *const _);
            }
        }
    }

    pub unsafe fn update_state(
        &mut self,
        audio: &AudioState,
        _wifi: &WifiState,
        _bt:   &BluetoothState,
    ) {
        if audio.mode != self.active_mode {
            self.active_mode = audio.mode.clone();
            self.highlight_active_mode();
        }
        if audio.nr_level != self.active_nr {
            self.active_nr = audio.nr_level.clone();
            self.highlight_active_nr();
        }
    }

    // ── Show / hide ───────────────────────────────────────────────────────────────

    /// Show the CC overlay pre-navigated to the Audio sub-page (called from Settings).
    pub unsafe fn show_audio(&mut self) {
        self.show_page(CcPage::Audio);
        if !self.visible {
            self.visible = true;
            let mut anim = lv_anim_t::default();
            lv_anim_init(&mut anim);
            lv_anim_set_var(&mut anim, self.panel as *mut _);
            lv_anim_set_values(&mut anim, -272, 32);
            lv_anim_set_duration(&mut anim, 200u32);
            lv_anim_set_exec_cb(&mut anim, Some(anim_y_cb));
            lv_anim_start(&anim);
        }
    }

    pub unsafe fn show(&mut self) {
        if self.visible { return; }
        self.visible = true;
        self.show_page(CcPage::Main); // always open at the main page
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
