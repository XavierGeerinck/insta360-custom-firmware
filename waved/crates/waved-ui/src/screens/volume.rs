//! Speaker Volume screen — 1:1 recreation of pspapp `ui_volume_screen_on_create`.
//!
//! Reference: libui.so `ui_volume_screen_on_create` at 0x0016dc74 and
//! refresh logic `FUN_0016d3e4` at 0x0016d3e4.
//!
//! Layout (400×304 space):
//!   Title        pos(24, 45),  size 352×34   — "Speaker Volume", style 0x35
//!   Volume bar   size 140×36,  OUT_BOTTOM_MID of title +0,+8
//!   Volume label size 200×36,  OUT_BOTTOM_MID of bar +0,+0 ("Off" / nn / "Max")
//!   Back  btn    76×76  at pos(20,  121), bg_img=btn_goback.png
//!   Down  btn    90×90  at pos(102, 186), bg_img=spk_down.png
//!   Up    btn    90×90  at pos(208, 186), bg_img=spk_up.png
//!   Mute  btn    76×76  at pos(304, 121), bg_img=spk_on.png / spk_off.png
//!
//! Button semantics (from pspapp FUN_0016d8ec/FUN_0016da1c/FUN_0016db44):
//!   Down:  new = ((vol - 10) / 10) * 10; if new < 1 → 0 + mute, else unmute
//!   Up:    new = ((vol + 10) / 10) * 10; clamp to 100; unmute
//!   Mute:  if not muted → mute. Else unmute; if vol==0 set vol=10
//!
//! Bar image (refresh): spk_muted or vol==0 → `bar_vol_0.png`. Otherwise
//! `bar_vol_{10..100}.png` stepped by 10. Label: "Off" at 0/muted, "Max" at 100,
//! else numeric.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{AudioState, Command, StateBus};

extern "C" {
    static lv_font_montserrat_14: lv_font_t;
    static lv_font_montserrat_20: lv_font_t;
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Pspapp bar image selection. Returns a nul-terminated path slice.
fn bar_asset(vol: u16, muted: bool) -> &'static [u8] {
    if muted || vol == 0 {
        return b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_0.png\0";
    }
    match vol {
        1..=10   => b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_10.png\0",
        11..=20  => b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_20.png\0",
        21..=30  => b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_30.png\0",
        31..=40  => b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_40.png\0",
        41..=50  => b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_50.png\0",
        51..=60  => b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_60.png\0",
        61..=70  => b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_70.png\0",
        71..=80  => b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_80.png\0",
        81..=90  => b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_90.png\0",
        _        => b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_100.png\0",
    }
}

/// Volume-down step matching pspapp: round down to nearest 10, below 1 mute.
/// Returns (new_vol, should_mute).
pub fn step_down(vol: u16) -> (u16, bool) {
    if vol == 0 { return (0, true); }
    let new = (vol.saturating_sub(10) / 10) * 10;
    if new < 1 { (0, true) } else { (new, false) }
}

/// Volume-up step: round up to nearest 10, clamp to 100.
pub fn step_up(vol: u16) -> u16 {
    let new = ((vol + 10) / 10) * 10;
    if new > 100 { 100 } else { new }
}

// ── Event callbacks ─────────────────────────────────────────────────────────

/// Callback user_data holds a snapshot of the StateBus pointer (via Arc) —
/// we dereference each click to read fresh state.
struct BusData(Arc<StateBus>);

unsafe extern "C" fn back_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let data = lv_event_get_user_data(e) as *const BusData;
        if !data.is_null() {
            let bus = &(&*data).0;
            let _ = bus.commands.send(Command::CloseSettings);
        }
    }
}

unsafe extern "C" fn down_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) != lv_event_code_t_LV_EVENT_CLICKED { return; }
    let data = lv_event_get_user_data(e) as *const BusData;
    if data.is_null() { return; }
    let bus = &(&*data).0;
    let vol = bus.audio.borrow().speaker_volume;
    let (new_vol, mute) = step_down(vol);
    let _ = bus.commands.send(Command::SetSpeakerVolume(new_vol));
    if mute != bus.audio.borrow().speaker_muted {
        let _ = bus.commands.send(Command::ToggleSpeakerMute);
    }
}

unsafe extern "C" fn up_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) != lv_event_code_t_LV_EVENT_CLICKED { return; }
    let data = lv_event_get_user_data(e) as *const BusData;
    if data.is_null() { return; }
    let bus = &(&*data).0;
    let vol = bus.audio.borrow().speaker_volume;
    let new_vol = step_up(vol);
    let _ = bus.commands.send(Command::SetSpeakerVolume(new_vol));
    if bus.audio.borrow().speaker_muted {
        let _ = bus.commands.send(Command::ToggleSpeakerMute);
    }
}

unsafe extern "C" fn mute_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) != lv_event_code_t_LV_EVENT_CLICKED { return; }
    let data = lv_event_get_user_data(e) as *const BusData;
    if data.is_null() { return; }
    let bus = &(&*data).0;
    let (muted, vol) = {
        let a = bus.audio.borrow();
        (a.speaker_muted, a.speaker_volume)
    };
    if !muted {
        let _ = bus.commands.send(Command::ToggleSpeakerMute);
    } else {
        if vol == 0 {
            let _ = bus.commands.send(Command::SetSpeakerVolume(10));
        }
        let _ = bus.commands.send(Command::ToggleSpeakerMute);
    }
}

// ── Screen ───────────────────────────────────────────────────────────────────

pub struct VolumeScreen {
    bus:       Arc<StateBus>,
    img_bar:   *mut lv_obj_t,
    lbl_vol:   *mut lv_obj_t,
    btn_mute:  *mut lv_obj_t,
    prev_vol:  Option<u16>,
    prev_mute: Option<bool>,
}

impl VolumeScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            img_bar:   std::ptr::null_mut(),
            lbl_vol:   std::ptr::null_mut(),
            btn_mute:  std::ptr::null_mut(),
            prev_vol:  None,
            prev_mute: None,
        }
    }

    /// Sync bar image + label + speaker icon to current AudioState.
    pub unsafe fn update_state(&mut self, audio: &AudioState) {
        let vol = audio.speaker_volume;
        let muted = audio.speaker_muted;
        if self.prev_vol == Some(vol) && self.prev_mute == Some(muted) { return; }

        if !self.img_bar.is_null() {
            lv_image_set_src(self.img_bar, bar_asset(vol, muted).as_ptr() as *const _);
        }

        if !self.lbl_vol.is_null() {
            let text = if muted || vol == 0 {
                "Off".to_string()
            } else if vol >= 100 {
                "Max".to_string()
            } else {
                vol.to_string()
            };
            let s = CString::new(text).unwrap();
            lv_label_set_text(self.lbl_vol, s.as_ptr());
        }

        if !self.btn_mute.is_null() {
            let src: &[u8] = if muted {
                b"A:/etc/pspapp/lvgl/icon/volume/spk_off.png\0"
            } else {
                b"A:/etc/pspapp/lvgl/icon/volume/spk_on.png\0"
            };
            lv_obj_set_style_bg_image_src(self.btn_mute, src.as_ptr() as *const _, 0);
        }

        self.prev_vol  = Some(vol);
        self.prev_mute = Some(muted);
    }
}

impl super::Screen for VolumeScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            // Reset memoization so first tick refreshes labels/images.
            self.prev_vol  = None;
            self.prev_mute = None;

            // Root — solid black, matches pspapp style 0x1b on the page container.
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

            // Title "Speaker Volume" — pos(24, 45), 352×34, white, centered.
            let title = lv_label_create(screen);
            lv_label_set_text(title, c"Speaker Volume".as_ptr());
            lv_obj_set_size(title, 352, 34);
            lv_obj_set_pos(title, 24, 45);
            lv_obj_set_style_text_color(title, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_14, 0);
            lv_obj_set_style_text_align(title, lv_text_align_t_LV_TEXT_ALIGN_CENTER, 0);

            // Volume bar — 140×36 below title +8 (centered on title).
            let img_bar = lv_image_create(screen);
            lv_obj_set_size(img_bar, 140, 36);
            lv_image_set_src(img_bar,
                b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_0.png\0".as_ptr() as *const _);
            lv_obj_align_to(img_bar, title,
                lv_align_t_LV_ALIGN_OUT_BOTTOM_MID, 0, 8);
            self.img_bar = img_bar;

            // Volume label — 200×36 below bar.
            let lbl_vol = lv_label_create(screen);
            lv_label_set_text(lbl_vol, c"Off".as_ptr());
            lv_obj_set_size(lbl_vol, 200, 36);
            lv_obj_set_style_text_color(lbl_vol, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(lbl_vol, &lv_font_montserrat_20, 0);
            lv_obj_set_style_text_align(lbl_vol, lv_text_align_t_LV_TEXT_ALIGN_CENTER, 0);
            lv_obj_align_to(lbl_vol, img_bar,
                lv_align_t_LV_ALIGN_OUT_BOTTOM_MID, 0, 0);
            self.lbl_vol = lbl_vol;

            // Back button — 76×76 at (20, 121), bg_img=btn_goback.png.
            let back_data = Box::into_raw(Box::new(BusData(Arc::clone(&self.bus))));
            let btn_back = make_icon_button(
                screen, 76, 76, 20, 121,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_goback.png\0",
                back_cb,
                back_data as *mut _,
            );
            let _ = btn_back;

            // Down button — 90×90 at (102, 186), bg_img=spk_down.png.
            let down_data = Box::into_raw(Box::new(BusData(Arc::clone(&self.bus))));
            let _btn_down = make_icon_button(
                screen, 90, 90, 102, 186,
                b"A:/etc/pspapp/lvgl/icon/volume/spk_down.png\0",
                down_cb,
                down_data as *mut _,
            );

            // Up button — 90×90 at (208, 186), bg_img=spk_up.png.
            let up_data = Box::into_raw(Box::new(BusData(Arc::clone(&self.bus))));
            let _btn_up = make_icon_button(
                screen, 90, 90, 208, 186,
                b"A:/etc/pspapp/lvgl/icon/volume/spk_up.png\0",
                up_cb,
                up_data as *mut _,
            );

            // Mute toggle — 76×76 at (304, 121). Initial icon set by update_state.
            let mute_data = Box::into_raw(Box::new(BusData(Arc::clone(&self.bus))));
            let btn_mute = make_icon_button(
                screen, 76, 76, 304, 121,
                b"A:/etc/pspapp/lvgl/icon/volume/spk_on.png\0",
                mute_cb,
                mute_data as *mut _,
            );
            self.btn_mute = btn_mute;

            screen
        }
    }
}

/// Create an icon button: circular `lv_button` with a `bg_image_src`, zero
/// padding/border, transparent bg (shows only the icon PNG).
unsafe fn make_icon_button(
    parent: *mut lv_obj_t,
    w: i32, h: i32, x: i32, y: i32,
    icon_path: &[u8],
    cb: unsafe extern "C" fn(*mut lv_event_t),
    user_data: *mut ::std::os::raw::c_void,
) -> *mut lv_obj_t {
    let btn = lv_button_create(parent);
    lv_obj_set_size(btn, w, h);
    lv_obj_set_pos(btn, x, y);
    lv_obj_set_style_radius(btn, w.max(h), 0);
    lv_obj_set_style_bg_opa(btn, 0, 0);
    lv_obj_set_style_border_width(btn, 0, 0);
    lv_obj_set_style_shadow_width(btn, 0, 0);
    lv_obj_set_style_bg_image_src(btn, icon_path.as_ptr() as *const _, 0);
    lv_obj_set_style_pad_top(btn, 0, 0);
    lv_obj_set_style_pad_bottom(btn, 0, 0);
    lv_obj_set_style_pad_left(btn, 0, 0);
    lv_obj_set_style_pad_right(btn, 0, 0);
    lv_obj_remove_flag(btn, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
    lv_obj_add_event_cb(btn, Some(cb), lv_event_code_t_LV_EVENT_CLICKED, user_data);
    btn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_down_rounds_to_ten() {
        assert_eq!(step_down(55), (40, false));
        assert_eq!(step_down(20), (10, false));
        assert_eq!(step_down(10), (0, true));
        assert_eq!(step_down(5), (0, true));
        assert_eq!(step_down(0), (0, true));
    }

    #[test]
    fn step_up_rounds_to_ten_and_clamps() {
        assert_eq!(step_up(0), 10);
        assert_eq!(step_up(25), 30);
        assert_eq!(step_up(95), 100);
        assert_eq!(step_up(100), 100);
    }

    #[test]
    fn bar_asset_muted_is_zero() {
        assert_eq!(bar_asset(50, true), b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_0.png\0");
    }

    #[test]
    fn bar_asset_bucketing() {
        assert_eq!(bar_asset(0, false),  b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_0.png\0");
        assert_eq!(bar_asset(10, false), b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_10.png\0");
        assert_eq!(bar_asset(11, false), b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_20.png\0");
        assert_eq!(bar_asset(100,false), b"A:/etc/pspapp/lvgl/icon/volume/bar_vol_100.png\0");
    }
}
