//! ConfirmationScreen — Save / Send / Discard after recording stop.
//!
//! Layout (mockup A): title at top, duration + size, then path, then
//! endpoint, then a row of three 64×64 icon buttons at the bottom. See
//! `docs/superpowers/specs/2026-04-30-waved-recording-save-discard-design.md`
//! §7 for visual specifications.

use crate::bindings::*;
use crate::theme::*;
use std::ffi::CString;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;
use waved_state::{Command, MeetingState, PendingDecision, StateBus};

const BTN_SIZE: i32 = 64;
const BTN_GAP: i32 = 16;
/// Vertical offset from BOTTOM_MID for the button row's centre. -52 puts the
/// button centre at y=220 (top edge y=188, bottom edge y=252) — sits clearly
/// above the bottom of the readable circle and gives the labels above more
/// breathing room than the original -28 placement.
const BTN_Y_OFFSET: i32 = -52;

const CLR_TITLE_GREEN: u32 = 0x34c759;
/// Per-button background colours. Match brightness so opaque white-on-tint
/// icons read uniformly. See mockup A in the design doc.
const CLR_BTN_SAVE: u32 = 0x2563eb; // blue
const CLR_BTN_SEND: u32 = 0x16a34a; // green
const CLR_BTN_DISCARD: u32 = 0xdc2626; // red

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_14: lv_font_t;
}

/// Format a recording's audio path for the confirmation screen.
///
/// Takes the absolute path (e.g. `/mnt/udisk/recordings/4f3a8e2c-.../audio.wav`),
/// truncates the immediate parent dir name to 8 chars + `...`, and keeps the
/// filename. Returns e.g. `4f3a8e2c.../audio.wav`.
///
/// If the path has no parent dir or the parent name is 8 chars or fewer,
/// it's returned without truncation.
///
/// Uses ASCII `...` rather than Unicode `…` (U+2026) because the on-device
/// Montserrat 12 build lacks the ellipsis glyph and renders it as tofu.
pub fn format_path(audio_path: &Path) -> String {
    let filename = audio_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = audio_path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent_disp = if parent.chars().count() > 8 {
        let truncated: String = parent.chars().take(8).collect();
        format!("{truncated}...")
    } else {
        parent
    };
    if parent_disp.is_empty() {
        filename
    } else {
        format!("{parent_disp}/{filename}")
    }
}

/// Format an endpoint URL for display.
///
/// Strips `http://` or `https://`. If the remaining string is ≤ 32 chars,
/// returns it unchanged. Otherwise ellipsizes the *path* part, keeping the
/// host:port and the last path segment intact:
///
///   `https://example.com/api/v2/very/long/path/upload`
///   → `example.com/.../upload`
///
/// Uses ASCII `...` rather than Unicode `…` (see `format_path`).
pub fn format_endpoint(url: &str) -> String {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    if without_scheme.chars().count() <= 32 {
        return without_scheme.to_string();
    }
    let (host, path) = match without_scheme.split_once('/') {
        Some((h, p)) => (h, p),
        None => return without_scheme.to_string(),
    };
    let last_seg = path.rsplit('/').next().unwrap_or("");
    if last_seg.is_empty() {
        without_scheme.to_string()
    } else {
        format!("{host}/.../{last_seg}")
    }
}

/// "duration MM:SS · X.Y MB"
pub fn format_duration_size(duration_secs: u32, size_bytes: u64) -> String {
    let m = duration_secs / 60;
    let s = duration_secs % 60;
    let mb = size_bytes as f64 / 1_048_576.0;
    format!("duration {m:02}:{s:02} · {mb:.1} MB")
}

/// Three button kinds. Each click callback dereferences a heap-allocated
/// `(Sender<Command>, Uuid)` tuple and emits the right command.
struct ButtonCtx {
    tx: tokio::sync::broadcast::Sender<Command>,
    id: Uuid,
}

unsafe extern "C" fn save_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) != lv_event_code_t_LV_EVENT_CLICKED {
        return;
    }
    let ctx = lv_event_get_user_data(e) as *const ButtonCtx;
    if ctx.is_null() {
        return;
    }
    let _ = (*ctx).tx.send(Command::SaveRecording((*ctx).id));
}

unsafe extern "C" fn send_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) != lv_event_code_t_LV_EVENT_CLICKED {
        return;
    }
    let ctx = lv_event_get_user_data(e) as *const ButtonCtx;
    if ctx.is_null() {
        return;
    }
    let _ = (*ctx).tx.send(Command::SendRecording((*ctx).id));
}

unsafe extern "C" fn discard_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) != lv_event_code_t_LV_EVENT_CLICKED {
        return;
    }
    let ctx = lv_event_get_user_data(e) as *const ButtonCtx;
    if ctx.is_null() {
        return;
    }
    let _ = (*ctx).tx.send(Command::DiscardRecording((*ctx).id));
}

pub struct ConfirmationScreen {
    bus: Arc<StateBus>,
    /// Cached at navigate-time so the LVGL thread doesn't read the watch
    /// mid-frame. Cleared on each `set_decision` call.
    decision: Option<PendingDecision>,
}

impl ConfirmationScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            decision: None,
        }
    }

    /// Called by the UI loop just before `create()` whenever the
    /// `meeting.pending_decision` watch transitions to `Some(...)`. The
    /// decision is then read from `self.decision` while building the
    /// LVGL widget tree.
    pub fn set_decision(&mut self, d: PendingDecision) {
        self.decision = Some(d);
    }

    /// `update_state` doesn't repaint anything — the screen is static
    /// once built. We only re-check whether the user changed
    /// `pending_decision` mid-frame (shouldn't happen, but defensive).
    pub unsafe fn update_state(&mut self, _meeting: &MeetingState) {}
}

impl super::Screen for ConfirmationScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        let decision = match self.decision.clone() {
            Some(d) => d,
            // Defensive: rendering with no decision means the nav fired
            // wrong. Show an empty screen so we don't crash.
            None => unsafe {
                let s = lv_obj_create(std::ptr::null_mut());
                lv_obj_set_size(s, 400, 304);
                lv_obj_set_style_bg_color(s, rgb(CLR_BG), 0);
                return s;
            },
        };

        unsafe {
            let screen = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_top(screen, 0, 0);
            lv_obj_set_style_pad_bottom(screen, 0, 0);
            lv_obj_set_style_pad_left(screen, 0, 0);
            lv_obj_set_style_pad_right(screen, 0, 0);

            // ── Title ────────────────────────────────────────────────────────
            let title = lv_label_create(screen);
            lv_label_set_text(title, c"SAVE RECORDING".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_TITLE_GREEN), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_14, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 38);

            // ── Duration + size ─────────────────────────────────────────────
            let line = format_duration_size(decision.duration_secs, decision.size_bytes);
            let cs = CString::new(line).unwrap();
            let lbl_dur = lv_label_create(screen);
            lv_label_set_text(lbl_dur, cs.as_ptr());
            lv_obj_set_style_text_color(lbl_dur, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(lbl_dur, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_dur, lv_align_t_LV_ALIGN_TOP_MID, 0, 78);

            // ── Path ─────────────────────────────────────────────────────────
            let path_disp = format!("[F] {}", format_path(&decision.audio_path));
            let cs = CString::new(path_disp).unwrap();
            let lbl_path = lv_label_create(screen);
            lv_label_set_text(lbl_path, cs.as_ptr());
            lv_obj_set_style_text_color(lbl_path, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(lbl_path, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_path, lv_align_t_LV_ALIGN_TOP_MID, 0, 110);

            // ── Endpoint ────────────────────────────────────────────────────
            let ep_disp = format!("[N] {}", format_endpoint(&decision.endpoint));
            let cs = CString::new(ep_disp).unwrap();
            let lbl_ep = lv_label_create(screen);
            lv_label_set_text(lbl_ep, cs.as_ptr());
            lv_obj_set_style_text_color(lbl_ep, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(lbl_ep, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_ep, lv_align_t_LV_ALIGN_TOP_MID, 0, 132);

            // ── Three icon buttons ──────────────────────────────────────────
            // Buttons are 64×64 circles. Centred on x via three fixed offsets
            // that give a BTN_GAP px gap between adjacent buttons.
            // Y is BOTTOM_MID with BTN_Y_OFFSET.
            let x_offset = BTN_SIZE + BTN_GAP;
            self.add_button(
                screen,
                -x_offset,
                CLR_BTN_SAVE,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_save.png\0",
                save_btn_cb,
                decision.id,
            );
            self.add_button(
                screen,
                0,
                CLR_BTN_SEND,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_send.png\0",
                send_btn_cb,
                decision.id,
            );
            self.add_button(
                screen,
                x_offset,
                CLR_BTN_DISCARD,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_discard.png\0",
                discard_btn_cb,
                decision.id,
            );

            screen
        }
    }
}

impl ConfirmationScreen {
    unsafe fn add_button(
        &self,
        parent: *mut lv_obj_t,
        x_offset: i32,
        bg_color: u32,
        icon_path_nul: &[u8],
        cb: unsafe extern "C" fn(*mut lv_event_t),
        id: Uuid,
    ) {
        // Circular button — styled lv_obj rather than an image background.
        // bg_airecord_stop.png is sized for the 90×90 record-screen button;
        // at 64×64 LVGL renders it at native size and clips, producing a
        // square. Rolling our own circle via radius=BTN_SIZE/2 avoids that
        // and lets us colour-code per action.
        let btn = lv_obj_create(parent);
        lv_obj_set_size(btn, BTN_SIZE, BTN_SIZE);
        lv_obj_align(btn, lv_align_t_LV_ALIGN_BOTTOM_MID, x_offset, BTN_Y_OFFSET);
        lv_obj_set_style_radius(btn, BTN_SIZE / 2, 0);
        lv_obj_set_style_bg_color(btn, rgb(bg_color), 0);
        lv_obj_set_style_bg_opa(btn, 255, 0);
        lv_obj_set_style_border_width(btn, 0, 0);
        lv_obj_set_style_pad_top(btn, 0, 0);
        lv_obj_set_style_pad_bottom(btn, 0, 0);
        lv_obj_set_style_pad_left(btn, 0, 0);
        lv_obj_set_style_pad_right(btn, 0, 0);
        lv_obj_remove_flag(btn, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
        lv_obj_add_flag(btn, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);

        // Foreground icon centred.
        let icon = lv_image_create(btn);
        lv_image_set_src(icon, icon_path_nul.as_ptr() as *const _);
        lv_obj_center(icon);

        // Per-button context (sender + uuid). Leaked deliberately —
        // matches the existing recording-screen pattern (see
        // recording.rs:490) where button context outlives the screen.
        let ctx = Box::into_raw(Box::new(ButtonCtx {
            tx: self.bus.commands.clone(),
            id,
        }));
        lv_obj_add_event_cb(
            btn,
            Some(cb),
            lv_event_code_t_LV_EVENT_CLICKED,
            ctx as *mut _,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn format_path_truncates_uuid_parent_dir() {
        let p =
            PathBuf::from("/mnt/udisk/recordings/4f3a8e2c-1234-5678-9abc-def012345678/audio.wav");
        assert_eq!(format_path(&p), "4f3a8e2c.../audio.wav");
    }

    #[test]
    fn format_path_keeps_short_parent_name_intact() {
        let p = PathBuf::from("/tmp/abc/audio.wav");
        assert_eq!(format_path(&p), "abc/audio.wav");
    }

    #[test]
    fn format_path_no_parent_returns_filename_only() {
        let p = PathBuf::from("audio.wav");
        assert_eq!(format_path(&p), "audio.wav");
    }

    #[test]
    fn format_endpoint_strips_https_scheme() {
        assert_eq!(
            format_endpoint("https://localhost:8080/meetings/upload"),
            "localhost:8080/meetings/upload"
        );
    }

    #[test]
    fn format_endpoint_strips_http_scheme() {
        assert_eq!(
            format_endpoint("http://localhost:8080/x"),
            "localhost:8080/x"
        );
    }

    #[test]
    fn format_endpoint_short_url_unchanged() {
        assert_eq!(format_endpoint("localhost:8080/x"), "localhost:8080/x");
    }

    #[test]
    fn format_endpoint_long_path_is_ellipsised_keeping_last_segment() {
        let r = format_endpoint("https://meetings.example.com/api/v2/very/long/path/upload");
        assert_eq!(r, "meetings.example.com/.../upload");
    }

    #[test]
    fn format_endpoint_no_scheme_no_path_unchanged() {
        assert_eq!(format_endpoint("localhost:8080"), "localhost:8080");
    }

    #[test]
    fn format_duration_size_basic() {
        assert_eq!(
            format_duration_size(74, 12_582_912),
            "duration 01:14 · 12.0 MB"
        );
    }

    #[test]
    fn format_duration_size_zero() {
        assert_eq!(format_duration_size(0, 0), "duration 00:00 · 0.0 MB");
    }
}
