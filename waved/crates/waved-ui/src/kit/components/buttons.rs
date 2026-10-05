//! Buttons: the one Back button, text buttons, and the round buttons. The
//! side buttons (Quieter and Louder) and the bottom actions (Record and Stop,
//! Mark, Pause, Send, Decline, Answer) share one round implementation.

use crate::bindings::{
    lv_anim_delete, lv_anim_init, lv_anim_set_completed_cb, lv_anim_set_duration,
    lv_anim_set_exec_cb, lv_anim_set_values, lv_anim_set_var, lv_anim_start, lv_anim_t,
    lv_arc_set_value, lv_event_code_t_LV_EVENT_DELETE,
    lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT, lv_event_code_t_LV_EVENT_PRESSED,
    lv_event_code_t_LV_EVENT_PRESS_LOST, lv_event_code_t_LV_EVENT_READY,
    lv_event_code_t_LV_EVENT_RELEASED, lv_obj_is_valid, lv_obj_send_event, lv_tick_elaps,
    lv_tick_get,
};
use crate::kit::components::dial::{arc, arc_style, full_ring, INDICATOR, MAIN};
use crate::kit::dispatch::Dispatcher;
use crate::kit::event::{on, on_click};
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::tokens::{
    Rect, BACK, BOTTOM_HOLD_R, BOTTOM_HOLD_WIDTH, BOTTOM_LEFT, BOTTOM_LEFT_LABEL, BOTTOM_MAIN,
    BOTTOM_MAIN_LABEL, BOTTOM_PAIR_LEFT, BOTTOM_PAIR_LEFT_LABEL, BOTTOM_PAIR_RIGHT,
    BOTTOM_PAIR_RIGHT_LABEL, BOTTOM_RIGHT, BOTTOM_RIGHT_LABEL, HOLD_TO_STOP_MS, LINE, LIVE, RAISED,
    REC, ROUND_LABEL_W, SIDE_LABEL_Y, SIDE_LEFT, SIDE_RIGHT, SURFACE, TEXT, TEXT_2, TEXT_3, WARN,
};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;

/// The accent of a button. Every button is a Surface fill that turns Raised
/// while pressed; the accent colours only what is drawn on it, so red is
/// never a button fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Text icon and words; a round button's label is Text 2: Mark, Pause,
    /// Keep, Cancel.
    Neutral,
    /// Teal icon and words: Send, Answer, Set up Wi-Fi.
    Live,
    /// On a round button: a red icon over a grey (Text 2) label. Record, where
    /// only the dot is red so red keeps meaning "recording". On a pill (the
    /// words are inside it): red icon and words, like `RecText`.
    Rec,
    /// On a round button: a red icon over a red label. Stop, Delete, Decline.
    /// On a pill: red icon and words (Power off).
    RecText,
    /// Amber icon and words: Resume.
    WarnText,
    /// Teal icon over a Text 2 label: File detail's Send / Send again.
    LiveIcon,
}

impl Tone {
    /// The colour drawn on the button: its icon, and a pill's words.
    pub fn fg(self) -> u32 {
        match self {
            Tone::Neutral => TEXT,
            Tone::Live | Tone::LiveIcon => LIVE,
            Tone::Rec | Tone::RecText => REC,
            Tone::WarnText => WARN,
        }
    }

    /// The colour of a round button's label under the circle.
    pub fn label(self) -> u32 {
        match self {
            Tone::Neutral | Tone::Rec | Tone::LiveIcon => TEXT_2,
            Tone::Live | Tone::RecText | Tone::WarnText => self.fg(),
        }
    }

    /// The accent a dialog uses for this tone: red for destructive tones,
    /// teal otherwise.
    pub fn accent(self) -> u32 {
        match self {
            Tone::Rec | Tone::RecText => REC,
            Tone::Neutral | Tone::Live | Tone::LiveIcon | Tone::WarnText => LIVE,
        }
    }
}

/// A Surface pill that turns Raised while pressed: Back, text buttons and the
/// pill pair.
pub(super) fn pill(parent: Obj, rect: Rect) -> Obj {
    Obj::plain(parent)
        .rect(rect)
        .bg(SURFACE)
        .radius(rect.h / 2)
        .pressed_bg(RAISED)
        .clickable(true)
}

/// The one Back button: 112×44 at the bottom centre.
pub fn back_button(parent: Obj, dispatch: &Dispatcher) -> Obj {
    let button = pill(parent, BACK).row(2);
    Obj::label(button, icons::LEFT, Font::Icon16, TEXT);
    Obj::label(button, "Back", Font::Body, TEXT);
    let dispatch = dispatch.clone();
    on_click(button, move || dispatch.back());
    button
}

/// A text button in the Back style: "Cancel", "Stop USB storage".
pub fn text_button(parent: Obj, rect: Rect, text: &str, on_tap: impl FnMut() + 'static) -> Obj {
    let button = pill(parent, rect).row(0);
    Obj::label(button, text, Font::Body, TEXT);
    on_click(button, on_tap);
    button
}

/// Where a round button goes: its circle, its icon size and the top-left of
/// its label (`ROUND_LABEL_W` wide, centred under the circle).
#[derive(Clone, Copy)]
struct Spot {
    rect: Rect,
    icon: Font,
    label: (i32, i32),
}

/// The bottom round-button spots on the r 135 arc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BottomSpot {
    /// 64 px at the bottom of the arc: Record and Stop, Send, Set up Wi-Fi.
    Main,
    /// 56 px companions: Mark / Pause (Resume), Keep / Delete.
    Left,
    Right,
    /// 64 px on the companion centres: Decline / Answer.
    PairLeft,
    PairRight,
}

impl BottomSpot {
    fn spot(self) -> Spot {
        let (rect, icon, label) = match self {
            BottomSpot::Main => (BOTTOM_MAIN, Font::Icon26, BOTTOM_MAIN_LABEL),
            BottomSpot::Left => (BOTTOM_LEFT, Font::Icon22, BOTTOM_LEFT_LABEL),
            BottomSpot::Right => (BOTTOM_RIGHT, Font::Icon22, BOTTOM_RIGHT_LABEL),
            BottomSpot::PairLeft => (BOTTOM_PAIR_LEFT, Font::Icon26, BOTTOM_PAIR_LEFT_LABEL),
            BottomSpot::PairRight => (BOTTOM_PAIR_RIGHT, Font::Icon26, BOTTOM_PAIR_RIGHT_LABEL),
        };
        Spot { rect, icon, label }
    }

    pub fn rect(self) -> Rect {
        self.spot().rect
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// 60 px at the edge on the dial's centre line, 26 px icon, label 4 px below.
    fn spot(self) -> Spot {
        let rect = match self {
            Side::Left => SIDE_LEFT,
            Side::Right => SIDE_RIGHT,
        };
        let label = (rect.x + rect.w / 2 - ROUND_LABEL_W / 2, SIDE_LABEL_Y);
        Spot {
            rect,
            icon: Font::Icon26,
            label,
        }
    }
}

/// Tabler's solid media glyphs fill more of their em than the mockup's
/// symbols, so they are drawn at 20 px wherever they appear: a 12–13 px dot
/// or square, as in home.png and recording.png.
const MEDIA_ICONS: [&str; 4] = [icons::RECORD, icons::STOP, icons::PAUSE, icons::PLAY];
const MEDIA_FONT: Font = Font::Icon20;

/// A round Surface button with an icon and a micro label under it. Pressed
/// is Raised with a 2 px teal ring, drawn as a border so it stays inside the
/// circle (and the tap target).
///
/// The label (and Stop's hold ring) are siblings of the circle, placed in
/// screen coordinates, so the parent must be the screen root or a
/// full-screen container without a layout. They are deleted with the button.
pub struct RoundButton {
    parent: Obj,
    rect: Rect,
    obj: Obj,
    icon: Obj,
    label: Obj,
    /// The spot's icon size, for glyphs that are not media glyphs.
    spot_icon: Font,
    /// An explicit `icon_font`, which wins over both.
    icon_override: Cell<Option<Font>>,
    /// The label and the hold ring, deleted with the button.
    siblings: Rc<RefCell<Vec<Obj>>>,
    accent: Cell<Tone>,
    enabled: Cell<bool>,
    /// Stop's hold ring, once `hold` made this a hold button.
    ring: Option<Obj>,
    /// Set when a hold completes; the release that follows is not a tap.
    held: Rc<Cell<bool>>,
}

impl RoundButton {
    /// A bottom action at `spot`. `on_tap` runs on every tap while enabled.
    pub fn new(
        parent: Obj,
        spot: BottomSpot,
        icon: &str,
        label: &str,
        accent: Tone,
        on_tap: impl FnMut() + 'static,
    ) -> Self {
        Self::build(parent, spot.spot(), icon, label, accent).on_tap(on_tap)
    }

    /// A round button at an explicit spot: a circle filling `rect`, a 26 px
    /// icon (media glyphs 20 px, as everywhere) and a `ROUND_LABEL_W`-wide
    /// micro label whose top-left is `label`. Taps only; no hold, no repeat
    /// (File detail's Delete and Send, which `SideButton` would repeat).
    pub fn at(
        parent: Obj,
        rect: Rect,
        label: (i32, i32),
        icon: &str,
        text: &str,
        accent: Tone,
        on_tap: impl FnMut() + 'static,
    ) -> Self {
        let spot = Spot {
            rect,
            icon: Font::Icon26,
            label,
        };
        Self::build(parent, spot, icon, text, accent).on_tap(on_tap)
    }

    /// Run `on_tap` on every tap while enabled.
    fn on_tap(self, mut on_tap: impl FnMut() + 'static) -> Self {
        let (obj, held) = (self.obj, self.held.clone());
        on_click(obj, move || {
            // A disabled button ignores taps even when the event is sent
            // directly, and the release that ends a completed hold is no tap.
            if obj.is_clickable() && !held.get() {
                on_tap();
            }
        });
        self
    }

    /// Home's Record: the main spot, a red dot and a calm "Record" label.
    pub fn record(parent: Obj, on_tap: impl FnMut() + 'static) -> Self {
        Self::new(
            parent,
            BottomSpot::Main,
            icons::RECORD,
            "Record",
            Tone::Rec,
            on_tap,
        )
    }

    /// Recording's Stop: Record's spot, a red square and a red label. A tap
    /// runs `on_tap` (explain the hold); holding for `HOLD_TO_STOP_MS` runs
    /// `on_stop` once.
    pub fn stop(
        parent: Obj,
        on_tap: impl FnMut() + 'static,
        on_stop: impl FnMut() + 'static,
    ) -> Self {
        Self::new(
            parent,
            BottomSpot::Main,
            icons::STOP,
            "Stop",
            Tone::RecText,
            on_tap,
        )
        .hold(HOLD_TO_STOP_MS as u32, on_stop)
    }

    fn build(parent: Obj, spot: Spot, icon: &str, label: &str, accent: Tone) -> Self {
        let obj = Obj::plain(parent)
            .rect(spot.rect)
            .bg(SURFACE)
            .round()
            .pressed_bg(RAISED)
            .pressed_border(2, LIVE)
            .clickable(true);
        let icon = Obj::label(obj, icon, icon_size(icon, spot.icon), accent.fg()).center();
        let (x, y) = spot.label;
        let label = Obj::label(parent, label, Font::Micro, accent.label())
            .centered_text(ROUND_LABEL_W)
            .pos(x, y);
        let siblings = Rc::new(RefCell::new(vec![label]));
        {
            let siblings = siblings.clone();
            on(obj, lv_event_code_t_LV_EVENT_DELETE, move |_| {
                for sibling in siblings.borrow().iter() {
                    // SAFETY: only compares the pointer against live objects.
                    // When the whole screen goes, the siblings (created after
                    // the circle) are still live here, and LVGL's teardown
                    // re-reads its first child after each deletion.
                    if unsafe { lv_obj_is_valid(sibling.raw()) } {
                        sibling.delete();
                    }
                }
            });
        }
        Self {
            parent,
            rect: spot.rect,
            obj,
            icon,
            label,
            spot_icon: spot.icon,
            icon_override: Cell::new(None),
            siblings,
            accent: Cell::new(accent),
            enabled: Cell::new(true),
            ring: None,
            held: Rc::new(Cell::new(false)),
        }
    }

    /// Make this a hold button (Stop): pressing shows a red ring of r 35 that
    /// fills clockwise over `duration_ms` on a Line track; `on_complete` runs
    /// once when it is full. Letting go early cancels and resets the ring, and
    /// that release still counts as a tap (Stop explains the hold). While
    /// held the circle is Raised without the teal ring.
    pub fn hold(mut self, duration_ms: u32, mut on_complete: impl FnMut() + 'static) -> Self {
        let (cx, cy) = (self.rect.x + self.rect.w / 2, self.rect.y + self.rect.h / 2);
        let ring = arc(self.parent, cx, cy, BOTTOM_HOLD_R, BOTTOM_HOLD_WIDTH).hidden(true);
        full_ring(ring);
        arc_style(ring, MAIN, LINE, 255, BOTTOM_HOLD_WIDTH);
        arc_style(ring, INDICATOR, REC, 255, BOTTOM_HOLD_WIDTH);
        self.obj.pressed_border(0, LIVE);

        let (obj, held) = (self.obj, self.held.clone());
        on(obj, lv_event_code_t_LV_EVENT_PRESSED, move |_| {
            if obj.is_clickable() {
                held.set(false);
                start_hold(ring, duration_ms);
            }
        });
        on(obj, lv_event_code_t_LV_EVENT_RELEASED, move |_| {
            stop_hold(ring)
        });
        on(obj, lv_event_code_t_LV_EVENT_PRESS_LOST, move |_| {
            stop_hold(ring)
        });
        // The ring is a sibling (it reaches past the circle), so it goes with
        // the button: a deleted button's fill can never complete later.
        self.siblings.borrow_mut().push(ring);
        let held = self.held.clone();
        on(ring, lv_event_code_t_LV_EVENT_READY, move |_| {
            if !held.replace(true) {
                on_complete();
            }
        });
        self.ring = Some(ring);
        self
    }

    /// Draw the icon in `font` from now on, instead of the automatic size
    /// (the spot's size, or 20 px for the solid media glyphs).
    pub fn icon_font(self, font: Font) -> Self {
        self.icon_override.set(Some(font));
        self.icon.font(font);
        self
    }

    /// Swap the icon in place (Pause and Resume). The size follows the new
    /// glyph unless `icon_font` fixed it.
    pub fn set_icon(&self, icon: &str) {
        let font = self
            .icon_override
            .get()
            .unwrap_or_else(|| icon_size(icon, self.spot_icon));
        self.icon.font(font).set_text(icon);
    }

    pub fn set_label(&self, text: &str) {
        self.label.set_text(text);
    }

    pub fn set_accent(&self, accent: Tone) {
        self.accent.set(accent);
        self.paint();
    }

    /// A disabled button shows its icon and label in Text 3 and ignores taps
    /// (Mark while paused). A hold in progress is cancelled.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.set(enabled);
        self.obj.clickable(enabled);
        if let (false, Some(ring)) = (enabled, self.ring) {
            stop_hold(ring);
        }
        self.paint();
    }

    pub fn obj(&self) -> Obj {
        self.obj
    }

    fn paint(&self) {
        let (icon, label) = if self.enabled.get() {
            let accent = self.accent.get();
            (accent.fg(), accent.label())
        } else {
            (TEXT_3, TEXT_3)
        };
        self.icon.text_color(icon);
        self.label.text_color(label);
    }
}

/// The icon size for `icon` at a spot whose icons are `spot` px.
fn icon_size(icon: &str, spot: Font) -> Font {
    if MEDIA_ICONS.contains(&icon) {
        MEDIA_FONT
    } else {
        spot
    }
}

/// Show the hold ring and fill it from empty over `duration_ms`.
fn start_hold(ring: Obj, duration_ms: u32) {
    ring.hidden(false);
    // SAFETY: live arc. LVGL copies the descriptor, and deletes the animation
    // with the arc (or in `stop_hold`), so the callbacks never see a dead arc.
    unsafe {
        lv_anim_delete(ring.raw().cast(), Some(set_hold_value));
        lv_arc_set_value(ring.raw(), 0);
        let mut anim = lv_anim_t::default();
        lv_anim_init(&mut anim);
        lv_anim_set_var(&mut anim, ring.raw().cast());
        lv_anim_set_exec_cb(&mut anim, Some(set_hold_value));
        lv_anim_set_values(&mut anim, 0, 1000);
        lv_anim_set_duration(&mut anim, duration_ms);
        lv_anim_set_completed_cb(&mut anim, Some(hold_complete));
        lv_anim_start(&anim);
    }
}

/// Cancel the fill, empty the ring and hide it.
fn stop_hold(ring: Obj) {
    // SAFETY: live arc.
    unsafe {
        lv_anim_delete(ring.raw().cast(), Some(set_hold_value));
        lv_arc_set_value(ring.raw(), 0);
    }
    ring.hidden(true);
}

unsafe extern "C" fn set_hold_value(arc: *mut c_void, value: i32) {
    lv_arc_set_value(arc.cast(), value);
}

/// The fill reached the end: tell the ring's READY handler.
unsafe extern "C" fn hold_complete(anim: *mut lv_anim_t) {
    lv_obj_send_event(
        (*anim).var.cast(),
        lv_event_code_t_LV_EVENT_READY,
        std::ptr::null_mut(),
    );
}

/// Repeat interval while a side button is held.
pub const REPEAT_MS: u32 = 300;

/// Quieter and Louder: round buttons at the side spots that step on press
/// and every 300 ms while held.
pub struct SideButton {
    round: RoundButton,
}

impl SideButton {
    /// A 60 px circle at the left or right edge with a 12 px label below.
    /// `on_step` runs on press and every 300 ms while held.
    pub fn new(
        parent: Obj,
        side: Side,
        icon: &str,
        label: &str,
        on_step: impl FnMut() + 'static,
    ) -> Self {
        let round = RoundButton::build(parent, side.spot(), icon, label, Tone::Neutral);
        let obj = round.obj;
        let step = Rc::new(RefCell::new(on_step));
        let last = Rc::new(Cell::new(0u32));
        {
            let (step, last) = (step.clone(), last.clone());
            on(obj, lv_event_code_t_LV_EVENT_PRESSED, move |_| {
                // SAFETY: LVGL tick read on the UI thread.
                last.set(unsafe { lv_tick_get() });
                (step.borrow_mut())();
            });
        }
        on(
            obj,
            lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT,
            move |_| {
                // SAFETY: LVGL tick reads on the UI thread.
                if unsafe { lv_tick_elaps(last.get()) } >= REPEAT_MS {
                    last.set(unsafe { lv_tick_get() });
                    (step.borrow_mut())();
                }
            },
        );
        Self { round }
    }

    pub fn set_icon(&self, icon: &str) {
        self.round.set_icon(icon);
    }

    pub fn set_label(&self, text: &str) {
        self.round.set_label(text);
    }

    pub fn obj(&self) -> Obj {
        self.round.obj
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::{
        lv_arc_get_value, lv_event_code_t_LV_EVENT_CLICKED, lv_obj_get_style_prop, lv_style_prop_t,
        LV_STATE_PRESSED, LV_STYLE_TEXT_COLOR, LV_STYLE_TEXT_FONT,
    };
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl, Frame};
    use crate::kit::tokens::{
        DETAIL_SIDE_LEFT, DETAIL_SIDE_LEFT_LABEL, DETAIL_SIDE_RIGHT, DETAIL_SIDE_RIGHT_LABEL,
        HOLD_TO_STOP_MS,
    };

    fn rect_of(obj: Obj) -> Rect {
        let a = obj.coords();
        Rect::new(a.x1, a.y1, a.x2 - a.x1 + 1, a.y2 - a.y1 + 1)
    }

    fn text_color(obj: Obj) -> u32 {
        // SAFETY: live label under the test lock.
        let c = unsafe {
            lv_obj_get_style_prop(obj.raw(), MAIN, LV_STYLE_TEXT_COLOR as lv_style_prop_t).color
        };
        (u32::from(c.red) << 16) | (u32::from(c.green) << 8) | u32::from(c.blue)
    }

    fn font_of(obj: Obj) -> *const c_void {
        // SAFETY: live label under the test lock.
        unsafe { lv_obj_get_style_prop(obj.raw(), MAIN, LV_STYLE_TEXT_FONT as lv_style_prop_t).ptr }
    }

    fn arc_value(arc: Obj) -> i32 {
        // SAFETY: live arc under the test lock.
        unsafe { lv_arc_get_value(arc.raw()) }
    }

    /// Within RGB565 rounding of `hex`.
    fn near(frame: &Frame, x: i32, y: i32, hex: u32) -> bool {
        let px = frame.pixel(x, y);
        let want = [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8];
        px.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 8)
    }

    #[test]
    fn back_goes_back() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, ui) = Dispatcher::test();
            let back = back_button(root, &dispatch);
            assert_golden("back_button", &lv.render());
            assert_targets_ok(root);
            lv.click(back);
            assert_eq!(ui.try_recv().unwrap(), UiEvent::Back);
        });
    }

    #[test]
    fn round_buttons_sit_on_their_spots_with_labels_below() {
        lvgl(|lv| {
            let root = lv.screen();
            for (spot, rect, (x, y), icon) in [
                (
                    BottomSpot::Main,
                    BOTTOM_MAIN,
                    BOTTOM_MAIN_LABEL,
                    Font::Icon26,
                ),
                (
                    BottomSpot::Left,
                    BOTTOM_LEFT,
                    BOTTOM_LEFT_LABEL,
                    Font::Icon22,
                ),
                (
                    BottomSpot::Right,
                    BOTTOM_RIGHT,
                    BOTTOM_RIGHT_LABEL,
                    Font::Icon22,
                ),
                (
                    BottomSpot::PairLeft,
                    BOTTOM_PAIR_LEFT,
                    BOTTOM_PAIR_LEFT_LABEL,
                    Font::Icon26,
                ),
                (
                    BottomSpot::PairRight,
                    BOTTOM_PAIR_RIGHT,
                    BOTTOM_PAIR_RIGHT_LABEL,
                    Font::Icon26,
                ),
            ] {
                let button =
                    RoundButton::new(root, spot, icons::MARK, "Mark", Tone::Neutral, || {});
                root.update_layout();
                assert_eq!(rect_of(button.obj()), rect, "{spot:?}");
                assert_eq!(spot.rect(), rect);
                let label = rect_of(button.label);
                assert_eq!(
                    (label.x, label.y, label.w),
                    (x, y, ROUND_LABEL_W),
                    "{spot:?}"
                );
                assert_eq!(font_of(button.icon), icon.ptr().cast(), "{spot:?}");
                assert_eq!(font_of(button.label), Font::Micro.ptr().cast());
            }
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_accent_colours_the_icon_and_the_label_where_it_carries_meaning() {
        lvgl(|lv| {
            let root = lv.screen();
            for (tone, icon, label) in [
                (Tone::Neutral, TEXT, TEXT_2),  // Mark, Pause
                (Tone::Rec, REC, TEXT_2),       // Record: only the dot is red
                (Tone::RecText, REC, REC),      // Stop, Delete, Decline
                (Tone::Live, LIVE, LIVE),       // Send, Answer
                (Tone::WarnText, WARN, WARN),   // Resume
                (Tone::LiveIcon, LIVE, TEXT_2), // File detail's Send / Send again
            ] {
                let button =
                    RoundButton::new(root, BottomSpot::Main, icons::STOP, "Stop", tone, || {});
                assert_eq!(
                    (text_color(button.icon), text_color(button.label)),
                    (icon, label),
                    "{tone:?}"
                );
            }
            // Red is never a fill.
            assert!(near(&lv.render(), 200, 215, SURFACE));
        });
    }

    #[test]
    fn a_live_icon_is_a_teal_accent_in_dialogs() {
        assert_eq!(
            (
                Tone::LiveIcon.fg(),
                Tone::LiveIcon.label(),
                Tone::LiveIcon.accent()
            ),
            (LIVE, TEXT_2, LIVE)
        );
    }

    #[test]
    fn a_round_button_at_an_explicit_spot_taps_once_and_never_repeats() {
        lvgl(|lv| {
            let root = lv.screen();
            let taps = Rc::new(Cell::new(0));
            let count = taps.clone();
            let send = RoundButton::at(
                root,
                DETAIL_SIDE_RIGHT,
                DETAIL_SIDE_RIGHT_LABEL,
                icons::SEND,
                "Send",
                Tone::LiveIcon,
                move || count.set(count.get() + 1),
            );
            root.update_layout();
            assert_eq!(rect_of(send.obj()), DETAIL_SIDE_RIGHT);
            let label = rect_of(send.label);
            assert_eq!(
                (label.x, label.y, label.w),
                (
                    DETAIL_SIDE_RIGHT_LABEL.0,
                    DETAIL_SIDE_RIGHT_LABEL.1,
                    ROUND_LABEL_W
                )
            );
            assert_eq!(font_of(send.icon), Font::Icon26.ptr().cast());
            assert_eq!(
                (text_color(send.icon), text_color(send.label)),
                (LIVE, TEXT_2)
            );
            assert!(send.ring.is_none(), "no hold ring");

            lv.click(send.obj());
            assert_eq!(taps.get(), 1, "one tap, one call");
            // Holding it does not repeat, unlike the side buttons.
            lv.send(send.obj(), lv_event_code_t_LV_EVENT_PRESSED);
            lv.advance(1_000);
            lv.send(send.obj(), lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT);
            lv.send(send.obj(), lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT);
            lv.send(send.obj(), lv_event_code_t_LV_EVENT_RELEASED);
            assert_eq!(taps.get(), 1, "a hold is not a tap and does not repeat");

            let label = send.label;
            send.obj().delete();
            // SAFETY: only compares the pointer against live objects.
            assert!(
                !unsafe { lv_obj_is_valid(label.raw()) },
                "the label goes with its button"
            );
            assert_eq!(root.child_count(), 0);
        });
    }

    #[test]
    fn the_file_detail_sides_match_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            RoundButton::at(
                root,
                DETAIL_SIDE_LEFT,
                DETAIL_SIDE_LEFT_LABEL,
                icons::TRASH,
                "Delete",
                Tone::Rec,
                || {},
            );
            RoundButton::at(
                root,
                DETAIL_SIDE_RIGHT,
                DETAIL_SIDE_RIGHT_LABEL,
                icons::REFRESH,
                "Send again",
                Tone::LiveIcon,
                || {},
            );
            assert_golden("round_detail_sides", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn pause_turns_into_resume_in_place_and_mark_can_be_disabled() {
        lvgl(|lv| {
            let root = lv.screen();
            let taps = Rc::new(Cell::new(0));
            let count = taps.clone();
            let mark = RoundButton::new(
                root,
                BottomSpot::Left,
                icons::MARK,
                "Mark",
                Tone::Neutral,
                move || count.set(count.get() + 1),
            );
            let pause = RoundButton::new(
                root,
                BottomSpot::Right,
                icons::PAUSE,
                "Pause",
                Tone::Neutral,
                || {},
            );
            pause.set_icon(icons::PLAY);
            pause.set_label("Resume");
            pause.set_accent(Tone::WarnText);
            root.update_layout(); // a one-line label shows "..." until it is laid out
            assert_eq!(pause.icon.text(), icons::PLAY);
            assert_eq!(pause.label.text(), "Resume");
            assert_eq!(
                (text_color(pause.icon), text_color(pause.label)),
                (WARN, WARN)
            );

            mark.set_enabled(false);
            assert!(!mark.obj().is_clickable());
            assert_eq!(
                (text_color(mark.icon), text_color(mark.label)),
                (TEXT_3, TEXT_3)
            );
            lv.click(mark.obj());
            assert_eq!(taps.get(), 0, "a disabled button ignores taps");
            // An accent change while disabled waits for enabling.
            mark.set_accent(Tone::Live);
            assert_eq!(text_color(mark.icon), TEXT_3);
            mark.set_enabled(true);
            assert_eq!(
                (text_color(mark.icon), text_color(mark.label)),
                (LIVE, LIVE)
            );
            lv.click(mark.obj());
            assert_eq!(taps.get(), 1);
        });
    }

    #[test]
    fn pressed_is_raised_with_a_live_ring_inside_the_circle() {
        lvgl(|lv| {
            let root = lv.screen();
            let record = RoundButton::new(
                root,
                BottomSpot::Main,
                icons::RECORD,
                "Record",
                Tone::Rec,
                || {},
            );
            record.obj().set_state(LV_STATE_PRESSED, true);
            let frame = lv.render();
            // Row 241 crosses the circle at x 168..231: the ring is its outer 2 px.
            assert!(near(&frame, 168, 241, LIVE));
            assert!(near(&frame, 169, 241, LIVE));
            assert!(near(&frame, 172, 241, RAISED));
            assert_eq!(frame.pixel(167, 241), [0, 0, 0], "nothing outside the box");
            assert_targets_ok(root);
        });
    }

    #[test]
    fn holding_stop_fills_the_ring_once_and_letting_go_early_resets_it() {
        lvgl(|lv| {
            let root = lv.screen();
            let (taps, done) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
            let (tap_count, done_count) = (taps.clone(), done.clone());
            let stop = RoundButton::new(
                root,
                BottomSpot::Main,
                icons::STOP,
                "Stop",
                Tone::RecText,
                move || tap_count.set(tap_count.get() + 1),
            )
            .hold(HOLD_TO_STOP_MS as u32, move || {
                done_count.set(done_count.get() + 1)
            });
            let ring = stop.ring.expect("hold ring");
            assert!(ring.is_hidden() && !ring.is_clickable());
            lv.render();
            // r 35 around the main spot's centre, stroke 3.
            assert_eq!(rect_of(ring), Rect::new(164, 205, 73, 73));

            // Let go halfway: the ring empties and hides, the tap explains the hold.
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_PRESSED);
            assert!(!ring.is_hidden());
            lv.advance(500);
            let half = arc_value(ring);
            assert!((450..=550).contains(&half), "{half}");
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_RELEASED);
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_CLICKED);
            assert!(ring.is_hidden());
            assert_eq!((arc_value(ring), taps.get(), done.get()), (0, 1, 0));
            lv.advance(1_000);
            assert_eq!(done.get(), 0, "an early release completes nothing");

            // Hold the whole second: completes once; the release is not a tap.
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_PRESSED);
            lv.advance(600);
            assert_eq!(done.get(), 0);
            lv.advance(500);
            assert_eq!((arc_value(ring), done.get()), (1000, 1));
            lv.advance(1_000);
            assert_eq!(done.get(), 1, "completion calls back once");
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_RELEASED);
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_CLICKED);
            assert_eq!(taps.get(), 1, "a completed hold is not also a tap");
            assert!(ring.is_hidden());

            // A lost press resets too.
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_PRESSED);
            lv.advance(300);
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_PRESS_LOST);
            assert!(ring.is_hidden());
            assert_eq!(arc_value(ring), 0);
            lv.advance(1_000);
            assert_eq!(done.get(), 1);

            // A disabled hold button does not start.
            stop.set_enabled(false);
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_PRESSED);
            assert!(ring.is_hidden());
        });
    }

    /// Width and height of the red pixels inside the main circle.
    fn red_box(frame: &Frame) -> (i32, i32) {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for y in BOTTOM_MAIN.y + 4..BOTTOM_MAIN.y + BOTTOM_MAIN.h - 4 {
            for x in BOTTOM_MAIN.x + 4..BOTTOM_MAIN.x + BOTTOM_MAIN.w - 4 {
                let [r, g, _] = frame.pixel(x, y);
                if r > 180 && g < 140 {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                }
            }
        }
        (x1 - x0 + 1, y1 - y0 + 1)
    }

    #[test]
    fn media_glyphs_take_the_mockup_size_without_being_asked() {
        lvgl(|lv| {
            // recording.png draws a 12 px square, home.png a 13 px dot.
            for name in ["stop", "record"] {
                let root = lv.screen();
                let button = if name == "stop" {
                    RoundButton::stop(root, || {}, || {})
                } else {
                    RoundButton::record(root, || {})
                };
                assert_eq!(font_of(button.icon), Font::Icon20.ptr().cast(), "{name}");
                let (w, h) = red_box(&lv.render());
                assert!(
                    (12..=14).contains(&w) && (12..=14).contains(&h),
                    "{name}: {w}x{h}"
                );
                button.obj().update_layout();
                let (c, i) = (rect_of(button.obj()), rect_of(button.icon));
                assert_eq!(i.x - c.x, c.x + c.w - (i.x + i.w), "{name} is centred");
            }
            let root = lv.screen();
            // Pause and Resume at a companion are media glyphs too...
            let pause = RoundButton::new(
                root,
                BottomSpot::Right,
                icons::PAUSE,
                "Pause",
                Tone::Neutral,
                || {},
            );
            assert_eq!(font_of(pause.icon), Font::Icon20.ptr().cast());
            pause.set_icon(icons::PLAY);
            assert_eq!(font_of(pause.icon), Font::Icon20.ptr().cast());
            // ...other glyphs keep the spot's size, also after a swap...
            pause.set_icon(icons::MARK);
            assert_eq!(font_of(pause.icon), Font::Icon22.ptr().cast());
            // ...and an explicit font wins over both.
            let send = RoundButton::new(
                root,
                BottomSpot::Main,
                icons::SEND,
                "Send",
                Tone::Live,
                || {},
            )
            .icon_font(Font::Icon16);
            send.set_icon(icons::STOP);
            assert_eq!(font_of(send.icon), Font::Icon16.ptr().cast());
        });
    }

    #[test]
    fn record_and_stop_are_the_main_spot_and_stop_holds() {
        lvgl(|lv| {
            let root = lv.screen();
            let (taps, done) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
            let (tap, stopped) = (taps.clone(), done.clone());
            let record = RoundButton::record(root, move || tap.set(tap.get() + 1));
            root.update_layout();
            assert_eq!(rect_of(record.obj()), BOTTOM_MAIN);
            assert_eq!(
                (record.icon.text(), record.label.text()),
                (icons::RECORD.into(), "Record".into())
            );
            assert_eq!(
                (text_color(record.icon), text_color(record.label)),
                (REC, TEXT_2)
            );
            lv.click(record.obj());
            assert_eq!(taps.get(), 1);

            let root = lv.screen();
            let stop = RoundButton::stop(root, || {}, move || stopped.set(stopped.get() + 1));
            root.update_layout();
            assert_eq!(
                (stop.icon.text(), stop.label.text()),
                (icons::STOP.into(), "Stop".into())
            );
            assert_eq!((text_color(stop.icon), text_color(stop.label)), (REC, REC));
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_PRESSED);
            lv.advance(HOLD_TO_STOP_MS as u32 + 100);
            assert_eq!(done.get(), 1);
        });
    }

    #[test]
    fn deleting_the_button_mid_hold_takes_its_ring_and_never_completes() {
        lvgl(|lv| {
            let root = lv.screen();
            let done = Rc::new(Cell::new(0));
            let count = done.clone();
            let stop = RoundButton::new(
                root,
                BottomSpot::Main,
                icons::STOP,
                "Stop",
                Tone::RecText,
                || {},
            )
            .hold(HOLD_TO_STOP_MS as u32, move || count.set(count.get() + 1));
            let (ring, label) = (stop.ring.expect("hold ring"), stop.label);
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_PRESSED);
            lv.advance(400);
            stop.obj().delete();
            // SAFETY: only compares the pointers against live objects.
            assert!(
                !unsafe { lv_obj_is_valid(ring.raw()) },
                "the ring goes with its button"
            );
            assert!(
                !unsafe { lv_obj_is_valid(label.raw()) },
                "the label goes with its button"
            );
            assert_eq!(root.child_count(), 0);
            lv.advance(HOLD_TO_STOP_MS as u32 + 200);
            lv.render();
            assert_eq!(done.get(), 0);
        });
    }

    #[test]
    fn replacing_the_screen_mid_hold_never_completes() {
        lvgl(|lv| {
            let root = lv.screen();
            let done = Rc::new(Cell::new(0));
            let count = done.clone();
            let stop = RoundButton::new(
                root,
                BottomSpot::Main,
                icons::STOP,
                "Stop",
                Tone::RecText,
                || {},
            )
            .hold(HOLD_TO_STOP_MS as u32, move || count.set(count.get() + 1));
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_PRESSED);
            lv.advance(400);
            lv.screen(); // deletes the old screen with the button and its ring
            lv.advance(HOLD_TO_STOP_MS as u32 + 200);
            lv.render();
            assert_eq!(done.get(), 0);
        });
    }

    #[test]
    fn home_record_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            RoundButton::record(root, || {});
            assert_golden("round_record", &lv.render());
            assert_targets_ok(root);
        });
    }

    /// Mark / Stop (hold) / Pause, as on Recording.
    fn recording_row(root: Obj) -> [RoundButton; 3] {
        [
            RoundButton::new(
                root,
                BottomSpot::Left,
                icons::MARK,
                "Mark",
                Tone::Neutral,
                || {},
            ),
            RoundButton::stop(root, || {}, || {}),
            RoundButton::new(
                root,
                BottomSpot::Right,
                icons::PAUSE,
                "Pause",
                Tone::Neutral,
                || {},
            ),
        ]
    }

    #[test]
    fn the_recording_row_matches_the_mockups() {
        lvgl(|lv| {
            let root = lv.screen();
            let [mark, stop, pause] = recording_row(root);
            assert_golden("round_recording_row", &lv.render());
            assert_targets_ok(root);

            // Paused: Mark dimmed, Resume amber.
            mark.set_enabled(false);
            pause.set_icon(icons::PLAY);
            pause.set_label("Resume");
            pause.set_accent(Tone::WarnText);
            assert_golden("round_paused_row", &lv.render());
            assert_targets_ok(root);

            // Holding Stop, 62 % of the way round (hold-stop.png draws 223°).
            mark.set_enabled(true);
            pause.set_icon(icons::PAUSE);
            pause.set_label("Pause");
            pause.set_accent(Tone::Neutral);
            stop.obj().set_state(LV_STATE_PRESSED, true);
            lv.send(stop.obj(), lv_event_code_t_LV_EVENT_PRESSED);
            lv.advance(620);
            assert_golden("round_stop_held", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_call_pair_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            RoundButton::new(
                root,
                BottomSpot::PairLeft,
                icons::PHONE_OFF,
                "Decline",
                Tone::RecText,
                || {},
            );
            RoundButton::new(
                root,
                BottomSpot::PairRight,
                icons::PHONE,
                "Answer",
                Tone::Live,
                || {},
            );
            assert_golden("round_call_pair", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn side_buttons_step_on_press_and_every_300_ms_while_held() {
        lvgl(|lv| {
            let root = lv.screen();
            let steps = Rc::new(Cell::new(0));
            let counter = steps.clone();
            SideButton::new(root, Side::Left, icons::VOLUME_LOW, "Quieter", || {});
            let louder =
                SideButton::new(root, Side::Right, icons::VOLUME_HIGH, "Louder", move || {
                    counter.set(counter.get() + 1)
                });
            assert_golden("side_buttons", &lv.render());
            assert_targets_ok(root);
            lv.send(louder.obj(), lv_event_code_t_LV_EVENT_PRESSED);
            assert_eq!(steps.get(), 1);
            lv.advance(100);
            lv.send(louder.obj(), lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT);
            assert_eq!(steps.get(), 1, "repeats wait 300 ms");
            lv.advance(250);
            lv.send(louder.obj(), lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT);
            assert_eq!(steps.get(), 2);
        });
    }

    #[test]
    fn side_buttons_press_like_the_bottom_buttons() {
        lvgl(|lv| {
            let root = lv.screen();
            let quieter = SideButton::new(root, Side::Left, icons::VOLUME_LOW, "Quieter", || {});
            quieter.obj().set_state(LV_STATE_PRESSED, true);
            let frame = lv.render();
            // Row 121 crosses the circle at x 26..85.
            assert!(near(&frame, 26, 121, LIVE));
            assert!(near(&frame, 30, 121, RAISED));
            assert_eq!(frame.pixel(25, 121), [0, 0, 0]);
        });
    }
}
