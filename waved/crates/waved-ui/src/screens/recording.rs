//! Recording screen — call-pill-style: two overlapping channel meters.
//!
//! ## Layout
//!
//! ```text
//!         REC
//!        00:00
//!  ┌────[ pill ]────┐
//!    green + orange (channels overlay in place)
//!  └────────────────┘
//!     Stop    Mute
//! ```
//!
//! The pill is a fixed strip of `WAVE_BARS` bars that pulse **in place** —
//! no scrolling, no travelling pulse. Two layers share the same x slots:
//!
//! - **Green** is driven by mic VU (`AudioState.vu_level`).
//! - **Orange** is driven by reference / far-end VU
//!   (`AudioState.incoming_vu_level`).
//!
//! Each bar's height = the channel's current normalised level × a fixed
//! per-bar weight (Gaussian envelope, taller at the centre). An EMA on
//! the displayed height smooths jitter so the pill *breathes* with the
//! signal instead of flickering. Both layers can be visible at once when
//! both ends are active; a layer fades to invisible when its channel
//! drops below its noise gate.

use crate::bindings::*;
use crate::theme::*;
use std::ffi::CString;
use std::sync::Arc;
use std::time::Instant;
use waved_state::{AudioState, Command, MeetingState, StateBus};

/// Bars in the pill. 35 × 6 − 2 = 208 px wide.
const WAVE_BARS: usize = 35;
/// Per-bar amplitude envelope. Apple-pill style: bars are roughly
/// uniform across the strip with small random-looking variance, not a
/// bell curve. Most weights sit in [0.70, 1.00] — when the channel is
/// at full level, the pill reads as a flat "loud" block with character,
/// not a centre-tall mound.
#[rustfmt::skip]
const WAVE_BAR_WEIGHTS: [f32; WAVE_BARS] = [
    0.85, 0.72, 0.95, 0.82, 1.00, 0.78, 0.92,
    0.85, 0.98, 0.80, 1.00, 0.90, 0.75, 0.95,
    0.88, 1.00, 0.83, 0.92, 0.78, 1.00, 0.86,
    0.95, 0.82, 0.90, 1.00, 0.75, 0.88, 0.92,
    1.00, 0.80, 0.95, 0.85, 1.00, 0.78, 0.90,
];
/// EMA decay for the displayed bar height. With a 60 ms push cadence,
/// this leaves each bar enough persistence to *animate* between
/// targets rather than snap, but not so much smoothing that the
/// per-bar variation collapses to a mean.
const WAVE_HEIGHT_EMA: f32 = 0.50;
/// Per-bar per-push random jitter range. Multiplier ∈ `[1−JITTER, 1]`.
/// 0.55 gives clearly different bar heights without chaotic flicker.
const WAVE_JITTER: f32 = 0.55;
const WAVE_BAR_W: i32 = 4;
const WAVE_BAR_GAP: i32 = 2;
/// Total bar height; bars are centre-aligned vertically so half extends
/// up and half down. 100 px gives 50 px of swing each way — clearly
/// visible on the 304-px display.
const WAVE_MAX_H: i32 = 100;
const WAVE_MIN_H: i32 = 2;
/// Push cadence. ~16 Hz lets the eye follow each per-bar transition;
/// faster looks like noise, slower feels sluggish.
const WAVE_PUSH_MS: u128 = 60;
const WAVE_ACTIVE_OPA: u8 = 210;

/// Adaptive noise-gate parameters.
///
/// The incoming values are RMS/VU-style magnitudes in [0,1]. Each stream
/// tracks its own noise floor as an EMA and displays only the excess above
/// `noise_floor × WAVE_GATE_RATIO`. The rendered bar is centre-aligned, so
/// visually it spans `-norm..+norm` even though the signal magnitude is
/// stored as an unsigned RMS value.
/// Tuned to the device: idle ambient peaks at 0.002–0.005 (observed in
/// the diagnostic log), so the gate must sit comfortably above that or
/// every breath / fan / footstep flickers the pill.
const WAVE_GATE_RATIO: f32 = 4.0;
/// EMA decay while the current level is "quiet" (≤ floor × 1.5). Slow
/// enough that brief lulls don't drag the floor — and the gate — down,
/// which would re-trigger bars on the next ambient blip.
const WAVE_FLOOR_DOWN_ALPHA: f32 = 0.005;
/// EMA decay during loud moments — kept tiny so a sustained burst can't
/// drag the floor up and gate the speech itself.
const WAVE_FLOOR_UP_ALPHA: f32 = 0.0001;
/// Floor seed at the rough centre of the user's ambient distribution
/// (≈ 0.002 RMS-equivalent on this device). Combined with the gate
/// ratio this puts the initial gate at ≈ 0.008, well above idle peaks,
/// so the pill is calm from the first push instead of needing a
/// multi-minute calibration window.
const WAVE_FLOOR_SEED: f32 = 0.002;
/// Excess (level − gate) at which the bar saturates to full height.
/// Expressed as a multiple of the gate so loud rooms still scale.
const WAVE_EXCESS_FULL_RATIO: f32 = 6.0;

const CLR_WAVE_GREEN: u32 = 0x34c759;
const CLR_WAVE_ORANGE: u32 = 0xff9500;

/// Excess-above-gate as a [0,1] height fraction. sqrt boost so quiet
/// bursts already render as a chunky bar.
fn level_to_height_norm(level: f32, floor: f32) -> f32 {
    let gate = floor * WAVE_GATE_RATIO;
    if level <= gate {
        return 0.0;
    }
    let denom = (floor * WAVE_EXCESS_FULL_RATIO).max(1e-7);
    let norm = ((level - gate) / denom).clamp(0.0, 1.0);
    norm.sqrt()
}

fn update_noise_floor(floor: f32, level: f32) -> f32 {
    let alpha = if level <= floor * 1.5 {
        WAVE_FLOOR_DOWN_ALPHA
    } else {
        WAVE_FLOOR_UP_ALPHA
    };
    (floor * (1.0 - alpha) + level * alpha).max(1e-7)
}

fn wave_height(norm: f32) -> i32 {
    if norm <= 0.0 {
        WAVE_MIN_H
    } else {
        ((norm * WAVE_MAX_H as f32) as i32).max(WAVE_MIN_H)
    }
}

/// Deterministic [0, 1] hash of (bar index, push counter). Cheap PCG-ish
/// finalizer — no allocations, no `rand` dep. Seeded by a non-zero
/// constant so neither bar=0 nor t=0 collapses the hash to zero.
fn jitter_unit(bar: usize, t: u32) -> f32 {
    let mut x: u32 = (bar as u32)
        .wrapping_add(0xC0FFEE5E)
        .wrapping_mul(0x9E3779B1)
        .wrapping_add(t.wrapping_mul(0x85EBCA77))
        .wrapping_add(0xDEADBEEF);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EBCA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2AE35);
    x ^= x >> 16;
    (x as f32) / (u32::MAX as f32)
}

// ── Button event callbacks ────────────────────────────────────────────────────

unsafe extern "C" fn stop_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e) as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::StopRecording);
        }
    }
}

unsafe extern "C" fn mute_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e) as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::ToggleMicMute);
        }
    }
}

extern "C" {
    static lv_font_montserrat_12: lv_font_t;
    static lv_font_montserrat_20: lv_font_t;
}

pub fn format_elapsed(secs: u32) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m:02}:{s:02}")
}

// ── Struct ────────────────────────────────────────────────────────────────────

pub struct RecordingScreen {
    bus: Arc<StateBus>,
    lbl_timer: *mut lv_obj_t,
    img_mic: *mut lv_obj_t,
    outgoing_bars: [*mut lv_obj_t; WAVE_BARS],
    incoming_bars: [*mut lv_obj_t; WAVE_BARS],
    /// Per-slot bar height normalised to [0,1], computed at push time
    /// against the floor estimate then in effect. Storing the rendered
    /// height (not raw level) keeps each historical bar visually stable
    /// even when the noise floor drifts later — otherwise old quiet
    /// samples can become "loud" when the floor descends, which inverts
    /// the visual relationship to speech.
    outgoing_history: [f32; WAVE_BARS],
    incoming_history: [f32; WAVE_BARS],
    last_push: Option<Instant>,
    /// Per-frequency-band peaks since the last push. Audio service
    /// publishes log-spaced FFT magnitudes via `vu_freq_bands`; tracking
    /// max-per-band over each push window gives each bar its own
    /// frequency-driven timeline.
    outgoing_peak_band: [f32; waved_state::FREQ_BAND_COUNT],
    incoming_peak_band: [f32; waved_state::FREQ_BAND_COUNT],
    /// Monotonic counter advanced once per push. Seeds the per-bar
    /// jitter hash so the random multipliers shuffle every frame.
    push_counter: u32,
    /// EMA-tracked noise floors for each stream (single scalar — applied
    /// uniformly to every band, since all 8 bands share the same noise
    /// regime).
    outgoing_floor: f32,
    incoming_floor: f32,
    last_log: Option<Instant>,
    prev_muted: bool,
}

impl RecordingScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            lbl_timer: std::ptr::null_mut(),
            img_mic: std::ptr::null_mut(),
            outgoing_bars: [std::ptr::null_mut(); WAVE_BARS],
            incoming_bars: [std::ptr::null_mut(); WAVE_BARS],
            outgoing_history: [0.0; WAVE_BARS],
            incoming_history: [0.0; WAVE_BARS],
            last_push: None,
            outgoing_peak_band: [0.0; waved_state::FREQ_BAND_COUNT],
            incoming_peak_band: [0.0; waved_state::FREQ_BAND_COUNT],
            push_counter: 0,
            outgoing_floor: WAVE_FLOOR_SEED,
            incoming_floor: WAVE_FLOOR_SEED,
            last_log: None,
            prev_muted: false,
        }
    }

    /// Bring each bar one EMA step toward its target. Target is driven
    /// by the channel's *overall* peak (max of the 8 sub-bands) so all
    /// bars scale together with the actual sound intensity. Visual
    /// variance comes from a per-bar envelope weight × a per-frame
    /// jitter hash of (bar_idx, push_counter) — same shape, decoupled
    /// motion. Mapping a per-band index to bars previously created a
    /// visible repeating pattern (every 8 bars) that read as a fixed
    /// "5-channel meter" instead of a continuous pill.
    fn step_heights(
        &mut self,
        outgoing_band: &[f32; waved_state::FREQ_BAND_COUNT],
        incoming_band: &[f32; waved_state::FREQ_BAND_COUNT],
        outgoing_floor: f32,
        incoming_floor: f32,
    ) {
        self.push_counter = self.push_counter.wrapping_add(1);
        let alpha = WAVE_HEIGHT_EMA;
        let t = self.push_counter;

        for i in 0..WAVE_BARS {
            // Map this bar to a frequency band. Linear stretch — bars
            // 0..WAVE_BARS-1 cover bands 0..FREQ_BAND_COUNT-1.
            let band_idx = (i * waved_state::FREQ_BAND_COUNT) / WAVE_BARS;
            let band_idx = band_idx.min(waved_state::FREQ_BAND_COUNT - 1);
            let out_band_norm = level_to_height_norm(outgoing_band[band_idx], outgoing_floor);
            let in_band_norm  = level_to_height_norm(incoming_band[band_idx], incoming_floor);

            let w = WAVE_BAR_WEIGHTS[i];
            // Per-bar per-push jitter ∈ [1−JITTER, 1]. Only applied when
            // the channel is non-zero — silent bars must stay flat.
            let out_jit = if out_band_norm > 0.0 {
                1.0 - WAVE_JITTER + WAVE_JITTER * jitter_unit(i, t)
            } else { 1.0 };
            let in_jit = if in_band_norm > 0.0 {
                1.0 - WAVE_JITTER + WAVE_JITTER * jitter_unit(i, t.wrapping_add(0xA5A5))
            } else { 1.0 };
            let out_target = (out_band_norm * w * out_jit).clamp(0.0, 1.0);
            let in_target  = (in_band_norm * w * in_jit).clamp(0.0, 1.0);
            self.outgoing_history[i] =
                self.outgoing_history[i] * alpha + out_target * (1.0 - alpha);
            self.incoming_history[i] =
                self.incoming_history[i] * alpha + in_target * (1.0 - alpha);
        }
    }

    unsafe fn paint_bars(&self, muted: bool) {
        for i in 0..WAVE_BARS {
            let x = i as i32 * (WAVE_BAR_W + WAVE_BAR_GAP);

            let outgoing_bar = self.outgoing_bars[i];
            if !outgoing_bar.is_null() {
                let norm = self.outgoing_history[i];
                let colour = if muted { CLR_TEXT_DIM } else { CLR_WAVE_GREEN };
                lv_obj_set_size(outgoing_bar, WAVE_BAR_W, wave_height(norm));
                lv_obj_align(outgoing_bar, lv_align_t_LV_ALIGN_LEFT_MID, x, 0);
                lv_obj_set_style_bg_color(outgoing_bar, rgb(colour), 0);
                lv_obj_set_style_bg_opa(
                    outgoing_bar,
                    if norm > 0.0 { WAVE_ACTIVE_OPA } else { 0 },
                    0,
                );
                lv_obj_invalidate(outgoing_bar);
            }

            let incoming_bar = self.incoming_bars[i];
            if !incoming_bar.is_null() {
                let norm = self.incoming_history[i];
                lv_obj_set_size(incoming_bar, WAVE_BAR_W, wave_height(norm));
                lv_obj_align(incoming_bar, lv_align_t_LV_ALIGN_LEFT_MID, x, 0);
                lv_obj_set_style_bg_color(incoming_bar, rgb(CLR_WAVE_ORANGE), 0);
                lv_obj_set_style_bg_opa(
                    incoming_bar,
                    if norm > 0.0 { WAVE_ACTIVE_OPA } else { 0 },
                    0,
                );
                lv_obj_invalidate(incoming_bar);
            }
        }
    }

    pub unsafe fn update_state(&mut self, audio: &AudioState, meeting: &MeetingState) {
        let s = CString::new(format_elapsed(meeting.duration_secs)).unwrap();
        lv_label_set_text(self.lbl_timer, s.as_ptr());

        // Track per-frequency-band peaks since the last push.
        for b in 0..waved_state::FREQ_BAND_COUNT {
            let out_b = if audio.mic_muted { 0.0 } else { audio.vu_freq_bands[b] };
            if out_b > self.outgoing_peak_band[b] {
                self.outgoing_peak_band[b] = out_b;
            }
            let in_b = audio.incoming_freq_bands[b];
            if in_b > self.incoming_peak_band[b] {
                self.incoming_peak_band[b] = in_b;
            }
        }

        // Floor follower runs on the max-across-bands — single floor per
        // stream is fine; bands share the same noise regime.
        let outgoing_level = audio.vu_level.iter().cloned().fold(0.0f32, f32::max);
        let outgoing_level = if audio.mic_muted { 0.0 } else { outgoing_level };
        let incoming_level = audio
            .incoming_vu_level
            .iter()
            .cloned()
            .fold(0.0f32, f32::max);
        self.outgoing_floor = update_noise_floor(self.outgoing_floor, outgoing_level);
        self.incoming_floor = update_noise_floor(self.incoming_floor, incoming_level);

        let now = Instant::now();
        let should_push = match self.last_push {
            Some(t) => now.duration_since(t).as_millis() >= WAVE_PUSH_MS,
            None => true,
        };
        if should_push {
            let out_band = self.outgoing_peak_band;
            let in_band = self.incoming_peak_band;
            self.step_heights(&out_band, &in_band, self.outgoing_floor, self.incoming_floor);
            self.outgoing_peak_band = [0.0; waved_state::FREQ_BAND_COUNT];
            self.incoming_peak_band = [0.0; waved_state::FREQ_BAND_COUNT];
            self.last_push = Some(now);
            self.paint_bars(audio.mic_muted);

            let log_due = match self.last_log {
                Some(t) => now.duration_since(t).as_millis() >= 1000,
                None => true,
            };
            if log_due {
                let out_max = out_band.iter().cloned().fold(0.0f32, f32::max);
                let in_max = in_band.iter().cloned().fold(0.0f32, f32::max);
                tracing::info!(
                    "rec_screen: out_max={:.5} out_floor={:.5} in_max={:.5} in_floor={:.5} muted={}",
                    out_max, self.outgoing_floor, in_max, self.incoming_floor, audio.mic_muted,
                );
                self.last_log = Some(now);
            }
        }

        if audio.mic_muted != self.prev_muted {
            if !self.img_mic.is_null() {
                let src: &[u8] = if audio.mic_muted {
                    b"A:/etc/pspapp/lvgl/icon/volume/mic_off_red.png\0"
                } else {
                    b"A:/etc/pspapp/lvgl/icon/volume/mic_on.png\0"
                };
                lv_image_set_src(self.img_mic, src.as_ptr() as *const _);
            }
            self.prev_muted = audio.mic_muted;
            self.paint_bars(audio.mic_muted);
        }
    }
}

// ── Screen trait impl ─────────────────────────────────────────────────────────

impl super::Screen for RecordingScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            self.outgoing_history = [0.0; WAVE_BARS];
            self.incoming_history = [0.0; WAVE_BARS];
            self.last_push = None;
            self.outgoing_peak_band = [0.0; waved_state::FREQ_BAND_COUNT];
            self.incoming_peak_band = [0.0; waved_state::FREQ_BAND_COUNT];
            self.push_counter = 0;
            self.outgoing_floor = WAVE_FLOOR_SEED;
            self.incoming_floor = WAVE_FLOOR_SEED;
            self.last_log = None;
            self.prev_muted = false;

            let screen = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_top(screen, 0, 0);
            lv_obj_set_style_pad_bottom(screen, 0, 0);
            lv_obj_set_style_pad_left(screen, 0, 0);
            lv_obj_set_style_pad_right(screen, 0, 0);

            // ── REC + timer (centred, top) ───────────────────────────────────
            let lbl_status = lv_label_create(screen);
            lv_label_set_text(lbl_status, c"REC".as_ptr());
            lv_obj_set_style_text_color(lbl_status, rgb(CLR_RED), 0);
            lv_obj_set_style_text_font(lbl_status, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_status, lv_align_t_LV_ALIGN_TOP_MID, 0, 40);

            let lbl_timer = lv_label_create(screen);
            lv_label_set_text(lbl_timer, c"00:00".as_ptr());
            lv_obj_set_style_text_color(lbl_timer, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(lbl_timer, &lv_font_montserrat_20, 0);
            lv_obj_align(lbl_timer, lv_align_t_LV_ALIGN_TOP_MID, 0, 58);
            self.lbl_timer = lbl_timer;

            // ── Single waveform strip ────────────────────────────────────────
            // 35 × (4+2) − 2 = 208 px wide × 100 px tall; centred at
            // TOP_MID y=88 → spans y=[88, 188]. Buttons start at y=200,
            // 12 px clearance below the strip.
            let strip_w = WAVE_BARS as i32 * (WAVE_BAR_W + WAVE_BAR_GAP) - WAVE_BAR_GAP;
            let strip = lv_obj_create(screen);
            lv_obj_set_size(strip, strip_w, WAVE_MAX_H);
            lv_obj_align(strip, lv_align_t_LV_ALIGN_TOP_MID, 0, 88);
            lv_obj_set_style_bg_opa(strip, 0, 0);
            lv_obj_set_style_border_width(strip, 0, 0);
            lv_obj_set_style_pad_top(strip, 0, 0);
            lv_obj_set_style_pad_bottom(strip, 0, 0);
            lv_obj_set_style_pad_left(strip, 0, 0);
            lv_obj_set_style_pad_right(strip, 0, 0);
            lv_obj_remove_flag(strip, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

            for i in 0..WAVE_BARS {
                let outgoing_bar = lv_obj_create(strip);
                let x = i as i32 * (WAVE_BAR_W + WAVE_BAR_GAP);
                lv_obj_set_size(outgoing_bar, WAVE_BAR_W, WAVE_MIN_H);
                lv_obj_align(outgoing_bar, lv_align_t_LV_ALIGN_LEFT_MID, x, 0);
                lv_obj_set_style_bg_color(outgoing_bar, rgb(CLR_WAVE_GREEN), 0);
                lv_obj_set_style_bg_opa(outgoing_bar, 0, 0);
                lv_obj_set_style_border_width(outgoing_bar, 0, 0);
                lv_obj_set_style_pad_top(outgoing_bar, 0, 0);
                lv_obj_set_style_pad_bottom(outgoing_bar, 0, 0);
                lv_obj_set_style_pad_left(outgoing_bar, 0, 0);
                lv_obj_set_style_pad_right(outgoing_bar, 0, 0);
                lv_obj_set_style_radius(outgoing_bar, WAVE_BAR_W / 2, 0);
                lv_obj_remove_flag(outgoing_bar, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
                lv_obj_remove_flag(outgoing_bar, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
                self.outgoing_bars[i] = outgoing_bar;

                let incoming_bar = lv_obj_create(strip);
                lv_obj_set_size(incoming_bar, WAVE_BAR_W, WAVE_MIN_H);
                lv_obj_align(incoming_bar, lv_align_t_LV_ALIGN_LEFT_MID, x, 0);
                lv_obj_set_style_bg_color(incoming_bar, rgb(CLR_WAVE_ORANGE), 0);
                lv_obj_set_style_bg_opa(incoming_bar, 0, 0);
                lv_obj_set_style_border_width(incoming_bar, 0, 0);
                lv_obj_set_style_pad_top(incoming_bar, 0, 0);
                lv_obj_set_style_pad_bottom(incoming_bar, 0, 0);
                lv_obj_set_style_pad_left(incoming_bar, 0, 0);
                lv_obj_set_style_pad_right(incoming_bar, 0, 0);
                lv_obj_set_style_radius(incoming_bar, WAVE_BAR_W / 2, 0);
                lv_obj_remove_flag(incoming_bar, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
                lv_obj_remove_flag(incoming_bar, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
                self.incoming_bars[i] = incoming_bar;
            }

            // ── Stop button (90×90) → StopRecording ──────────────────────────
            let btn_stop = lv_image_create(screen);
            lv_image_set_src(
                btn_stop,
                b"A:/etc/pspapp/lvgl/icon/msc/bg_airecord_stop.png\0".as_ptr() as *const _,
            );
            lv_obj_set_size(btn_stop, 90, 90);
            lv_obj_align(btn_stop, lv_align_t_LV_ALIGN_BOTTOM_MID, -55, -14);
            lv_obj_remove_flag(btn_stop, lv_obj_flag_t_LV_OBJ_FLAG_ADV_HITTEST);
            lv_obj_add_flag(btn_stop, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
            let stop_icon = lv_image_create(btn_stop);
            lv_image_set_src(
                stop_icon,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_stop.png\0".as_ptr() as *const _,
            );
            lv_obj_center(stop_icon);
            let stop_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(
                btn_stop,
                Some(stop_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                stop_tx as *mut _,
            );

            // ── Mic button (76×76) → ToggleMicMute ───────────────────────────
            let btn_mic = lv_image_create(screen);
            lv_image_set_src(
                btn_mic,
                b"A:/etc/pspapp/lvgl/icon/msc/bg_airecord_mic.png\0".as_ptr() as *const _,
            );
            lv_obj_set_size(btn_mic, 76, 76);
            lv_obj_align(btn_mic, lv_align_t_LV_ALIGN_BOTTOM_MID, 60, -21);
            lv_obj_remove_flag(btn_mic, lv_obj_flag_t_LV_OBJ_FLAG_ADV_HITTEST);
            lv_obj_add_flag(btn_mic, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
            let img_mic = lv_image_create(btn_mic);
            lv_image_set_src(
                img_mic,
                b"A:/etc/pspapp/lvgl/icon/volume/mic_on.png\0".as_ptr() as *const _,
            );
            lv_obj_center(img_mic);
            self.img_mic = img_mic;
            let mute_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(
                btn_mic,
                Some(mute_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED,
                mute_tx as *mut _,
            );

            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_elapsed_zero() {
        assert_eq!(format_elapsed(0), "00:00");
    }

    #[test]
    fn format_elapsed_90s() {
        assert_eq!(format_elapsed(90), "01:30");
    }

    #[test]
    fn format_elapsed_overflow() {
        assert_eq!(format_elapsed(3661), "61:01");
    }

    #[test]
    fn height_norm_below_gate_is_zero() {
        let floor = 0.001;
        assert_eq!(level_to_height_norm(floor, floor), 0.0);
        assert_eq!(level_to_height_norm(floor * 1.6, floor), 0.0);
    }

    #[test]
    fn height_norm_far_above_gate_saturates() {
        let floor = 0.001;
        let level = floor * 20.0;
        assert!((level_to_height_norm(level, floor) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn step_heights_drives_bars_high_for_loud_band() {
        // 8 bands all loud → all bars settle near full height (each
        // bar's weight ≤ 1.0 caps it; centre saturates at its weight).
        let bus = Arc::new(StateBus::new());
        let mut s = RecordingScreen::new(bus);
        let loud = [1.0f32; waved_state::FREQ_BAND_COUNT];
        let silence = [0.0f32; waved_state::FREQ_BAND_COUNT];
        let floor = 0.001;
        for _ in 0..30 {
            s.step_heights(&loud, &silence, floor, floor);
        }
        let max_h = s
            .outgoing_history
            .iter()
            .cloned()
            .fold(0.0f32, f32::max);
        assert!(max_h > 0.7, "expected at least one bar to saturate, max={max_h}");
    }

    #[test]
    fn step_heights_decays_toward_silence() {
        let bus = Arc::new(StateBus::new());
        let mut s = RecordingScreen::new(bus);
        let loud = [1.0f32; waved_state::FREQ_BAND_COUNT];
        let silence = [0.0f32; waved_state::FREQ_BAND_COUNT];
        let floor = 0.001;
        for _ in 0..30 {
            s.step_heights(&loud, &loud, floor, floor);
        }
        for _ in 0..30 {
            s.step_heights(&silence, &silence, floor, floor);
        }
        for h in &s.outgoing_history {
            assert!(*h < 0.05, "expected near-zero, got {h}");
        }
        for h in &s.incoming_history {
            assert!(*h < 0.05, "expected near-zero, got {h}");
        }
    }

    #[test]
    fn step_heights_keeps_channels_independent() {
        let bus = Arc::new(StateBus::new());
        let mut s = RecordingScreen::new(bus);
        let loud = [1.0f32; waved_state::FREQ_BAND_COUNT];
        let silence = [0.0f32; waved_state::FREQ_BAND_COUNT];
        let floor = 0.001;
        for _ in 0..30 {
            s.step_heights(&loud, &silence, floor, floor);
        }
        let any_in_active = s.incoming_history.iter().any(|h| *h > 0.05);
        assert!(!any_in_active, "incoming should stay quiet when only outgoing is driven");
    }

    #[test]
    fn step_heights_jitter_produces_per_bar_variance() {
        // Sustained loud signal — per-bar weights × per-frame jitter
        // should land bars at clearly different heights even though
        // they all share the same overall peak.
        let bus = Arc::new(StateBus::new());
        let mut s = RecordingScreen::new(bus);
        let loud = [0.05f32; waved_state::FREQ_BAND_COUNT];
        let silence = [0.0f32; waved_state::FREQ_BAND_COUNT];
        let floor = 0.001;
        for _ in 0..50 {
            s.step_heights(&loud, &silence, floor, floor);
        }
        let max = s.outgoing_history.iter().cloned().fold(0.0f32, f32::max);
        let min = s
            .outgoing_history
            .iter()
            .cloned()
            .fold(f32::INFINITY, f32::min);
        assert!(max - min > 0.1, "expected variance, max={max} min={min}");
    }
}
