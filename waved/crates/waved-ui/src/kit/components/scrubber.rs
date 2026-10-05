//! The recording detail's playback scrubber (docs/mockups file-detail.png):
//! a progress ring of r 60 at (200, 140) with mark ticks on r 70, a knob to
//! drag, play/pause in the middle and the position under it. One round hit
//! area takes every touch; `press` / `drag` / `release` hold the gesture
//! logic so tests can drive it without an input device.

use crate::bindings::*;
use crate::kit::components::dial::{arc, arc_style, full_ring, INDICATOR, MAIN};
use crate::kit::event::on;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::tokens::{
    lvgl_angle, polar, Rect, BG, LINE, LIVE, SCREEN_H, SCREEN_W, TEXT, TEXT_2,
};
use std::cell::RefCell;
use std::rc::Rc;

pub const CX: i32 = 200;
pub const CY: i32 = 140;
pub const R: i32 = 60;
const WIDTH: i32 = 4;
const TICK_R: i32 = 70;
const TICK_WIDTH: i32 = 6;
const TICK_SPAN: f32 = 3.0;
/// The knob: r 8 plus the mockup's 3 px black stroke, as a 20 px disc with a 3 px border.
const KNOB: i32 = 20;
const KNOB_BORDER: i32 = 3;
const PLAY_BOX: Rect = Rect::new(178, 102, 44, 44);
const TIME_Y: i32 = 152;
const TIME_W: i32 = 160;
/// Touches closer than this to the centre play or pause...
pub const CENTRE_R: f32 = 44.0;
/// ...and out to this radius they scrub (the ring, the ticks and some slop).
/// 88 makes the ring band 44 px wide, the minimum target size.
pub const HIT_R: i32 = 88;
/// A ring tap within this many degrees of a tick jumps to its mark: ±18° is
/// a 44 px arc on r 70, the minimum target size.
pub const MARK_SNAP_DEG: f32 = 18.0;
/// Movement beyond this turns a ring tap into a drag.
const DRAG_SLOP: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Centre,
    Ring,
    Outside,
}

pub fn zone(x: i32, y: i32) -> Zone {
    let d = ((x - CX) as f32).hypot((y - CY) as f32);
    if d < CENTRE_R {
        Zone::Centre
    } else if d <= HIT_R as f32 {
        Zone::Ring
    } else {
        Zone::Outside
    }
}

/// Device degrees of (x, y) around the scrubber centre: 0 at the top, clockwise.
pub fn angle_at(x: i32, y: i32) -> f32 {
    ((x - CX) as f32)
        .atan2((CY - y) as f32)
        .to_degrees()
        .rem_euclid(360.0)
}

/// The finger's angle `angle_deg` (0..360) unwound from the last reading
/// `prev_deg`: each step goes the short way round, so crossing the top
/// clockwise counts past 360 and counter-clockwise below 0.
pub fn unwind(prev_deg: f32, angle_deg: f32) -> f32 {
    let step = (angle_deg - prev_deg).rem_euclid(360.0);
    prev_deg + if step > 180.0 { step - 360.0 } else { step }
}

/// The drag position (0..1) for an unwound finger angle. Past the top either
/// way the knob stays at that end until the finger comes back across it,
/// however far round it goes meanwhile.
pub fn follow(turn_deg: f32) -> f32 {
    (turn_deg / 360.0).clamp(0.0, 1.0)
}

pub fn fraction(position_ms: u32, duration_ms: u32) -> f32 {
    if duration_ms == 0 {
        0.0
    } else {
        (f64::from(position_ms.min(duration_ms)) / f64::from(duration_ms)) as f32
    }
}

pub fn to_ms(fraction: f32, duration_ms: u32) -> u32 {
    (f64::from(fraction.clamp(0.0, 1.0)) * f64::from(duration_ms)).round() as u32
}

fn angular_gap(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// The mark closest to `angle_deg`, if one is within `MARK_SNAP_DEG`.
pub fn nearest_mark(angle_deg: f32, marks_ms: &[u32], duration_ms: u32) -> Option<u32> {
    if duration_ms == 0 {
        return None;
    }
    marks_ms
        .iter()
        .map(|&mark| {
            (
                mark,
                angular_gap(angle_deg, fraction(mark, duration_ms) * 360.0),
            )
        })
        .filter(|(_, gap)| *gap <= MARK_SNAP_DEG)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(mark, _)| mark)
}

/// "14:52", or "1:05:00" from an hour on.
pub fn clock(ms: u32) -> String {
    let s = ms / 1_000;
    if s >= 3_600 {
        format!("{}:{:02}:{:02}", s / 3_600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn time_text(position_ms: u32, duration_ms: u32) -> String {
    format!("{} / {}", clock(position_ms), clock(duration_ms))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrubEvent {
    /// The middle was tapped.
    TogglePlay,
    /// The ring was tapped or dragged and let go here (ms from the start).
    Seek(u32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Gesture {
    Idle,
    Centre,
    Scrub {
        x: i32,
        y: i32,
        /// Where the knob is (0..1).
        at: f32,
        moved: bool,
        /// The finger's angle unwound across the top (`unwind`).
        turn: f32,
        /// The finger went through the middle, where its angle means nothing.
        centred: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Shown {
    position_ms: u32,
    duration_ms: u32,
    marks_ms: Vec<u32>,
    playing: bool,
    playable: bool,
}

struct Inner {
    layer: Obj,
    ring: Obj,
    knob: Obj,
    icon: Obj,
    time: Obj,
    hit: Obj,
    ticks: Vec<Obj>,
    shown: Option<Shown>,
    gesture: Gesture,
}

#[derive(Clone)]
pub struct Scrubber {
    inner: Rc<RefCell<Inner>>,
    on_event: Rc<RefCell<dyn FnMut(ScrubEvent)>>,
}

/// Where the active pointer is, during one of its events; `None` in the
/// headless tests, which have no input device.
fn point() -> Option<(i32, i32)> {
    // SAFETY: the active input device is valid during its event; null otherwise.
    unsafe {
        let indev = lv_indev_active();
        if indev.is_null() {
            return None;
        }
        let mut p = lv_point_t::default();
        lv_indev_get_point(indev, &mut p);
        Some((p.x, p.y))
    }
}

impl Scrubber {
    pub fn new(parent: Obj, on_event: impl FnMut(ScrubEvent) + 'static) -> Self {
        let ring = arc(parent, CX, CY, R, WIDTH);
        full_ring(ring);
        arc_style(ring, MAIN, LINE, 255, WIDTH);
        arc_style(ring, INDICATOR, LIVE, 255, WIDTH);
        // Ticks go in a full-screen layer under the knob, created now so
        // rebuilding them never changes the stacking order.
        let layer = Obj::plain(parent).rect(Rect::new(0, 0, SCREEN_W, SCREEN_H));
        let knob = Obj::plain(parent)
            .size(KNOB, KNOB)
            .round()
            .bg(TEXT)
            .border(KNOB_BORDER, BG)
            .hidden(true);
        let play = Obj::plain(parent).rect(PLAY_BOX);
        let icon = Obj::label(play, icons::PLAY, Font::Icon44, TEXT)
            .center()
            .hidden(true);
        let time = Obj::label(parent, "", Font::Num13, TEXT_2)
            .centered_text(TIME_W)
            .pos(CX - TIME_W / 2, TIME_Y)
            .hidden(true);
        // A one-line label ends in "..." until it has its width: lay it out
        // once so every later `set_text` fits the caption at once.
        time.update_layout();
        let hit = Obj::plain(parent)
            .pos(CX - HIT_R, CY - HIT_R)
            .size(2 * HIT_R, 2 * HIT_R)
            .round()
            .clickable(true)
            // A knob drag must never read as swipe-right (Back) or swipe-down (Menu).
            .keep_gestures();
        let scrubber = Self {
            inner: Rc::new(RefCell::new(Inner {
                layer,
                ring,
                knob,
                icon,
                time,
                hit,
                ticks: Vec::new(),
                shown: None,
                gesture: Gesture::Idle,
            })),
            on_event: Rc::new(RefCell::new(on_event)),
        };
        let s = scrubber.clone();
        on(hit, lv_event_code_t_LV_EVENT_PRESSED, move |_| {
            if let Some((x, y)) = point() {
                s.press(x, y);
            }
        });
        let s = scrubber.clone();
        on(hit, lv_event_code_t_LV_EVENT_PRESSING, move |_| {
            if let Some((x, y)) = point() {
                s.drag(x, y);
            }
        });
        let s = scrubber.clone();
        on(hit, lv_event_code_t_LV_EVENT_RELEASED, move |_| s.release());
        let s = scrubber.clone();
        on(hit, lv_event_code_t_LV_EVENT_PRESS_LOST, move |_| s.lost());
        scrubber
    }

    /// Show playback: the ring, knob and caption follow `position_ms` unless
    /// a finger is dragging; ticks are rebuilt only when the marks change.
    pub fn set(
        &self,
        position_ms: u32,
        duration_ms: u32,
        marks_ms: &[u32],
        playing: bool,
        playable: bool,
    ) {
        let mut inner = self.inner.borrow_mut();
        let next = Shown {
            position_ms,
            duration_ms,
            marks_ms: marks_ms.to_vec(),
            playing,
            playable,
        };
        if inner.shown.as_ref() == Some(&next) {
            return;
        }
        let ticks_changed = inner.shown.as_ref().is_none_or(|shown| {
            shown.duration_ms != duration_ms
                || shown.marks_ms != marks_ms
                || shown.playable != playable
        });
        inner
            .icon
            .hidden(!playable)
            .set_text(if playing { icons::PAUSE } else { icons::PLAY });
        inner.shown = Some(next);
        if ticks_changed {
            inner.rebuild_ticks();
        }
        if !matches!(inner.gesture, Gesture::Scrub { .. }) {
            inner.draw_at(fraction(position_ms, duration_ms));
        }
    }

    pub fn press(&self, x: i32, y: i32) {
        let mut inner = self.inner.borrow_mut();
        if !inner.playable() {
            inner.gesture = Gesture::Idle;
            return;
        }
        inner.gesture = match zone(x, y) {
            Zone::Centre => Gesture::Centre,
            Zone::Ring => {
                let turn = angle_at(x, y);
                let at = turn / 360.0;
                inner.draw_at(at);
                Gesture::Scrub {
                    x,
                    y,
                    at,
                    moved: false,
                    turn,
                    centred: false,
                }
            }
            Zone::Outside => Gesture::Idle,
        };
    }

    pub fn drag(&self, x: i32, y: i32) {
        let mut inner = self.inner.borrow_mut();
        match inner.gesture {
            Gesture::Centre if zone(x, y) != Zone::Centre => inner.gesture = Gesture::Idle,
            Gesture::Scrub {
                x: x0,
                y: y0,
                at,
                moved,
                turn,
                centred,
            } => {
                let moved = moved || ((x - x0) as f32).hypot((y - y0) as f32) > DRAG_SLOP;
                let (at, turn, centred) = if !moved {
                    (at, turn, centred)
                } else if zone(x, y) == Zone::Centre {
                    // Near the middle the angle swings wildly: the knob waits.
                    (at, turn, true)
                } else if centred {
                    // Back from across the middle: the knob goes where the
                    // finger is, with no turn to unwind from.
                    let turn = angle_at(x, y);
                    (turn / 360.0, turn, false)
                } else {
                    let turn = unwind(turn, angle_at(x, y));
                    (follow(turn), turn, false)
                };
                inner.draw_at(at);
                inner.gesture = Gesture::Scrub {
                    x: x0,
                    y: y0,
                    at,
                    moved,
                    turn,
                    centred,
                };
            }
            _ => {}
        }
    }

    /// The finger lifted: the middle toggles playback; the ring seeks, to the
    /// nearest mark when it was a tap near a tick.
    pub fn release(&self) {
        let event = {
            let mut inner = self.inner.borrow_mut();
            match std::mem::replace(&mut inner.gesture, Gesture::Idle) {
                Gesture::Idle => None,
                Gesture::Centre => Some(ScrubEvent::TogglePlay),
                Gesture::Scrub { at, moved, .. } => {
                    let duration = inner.duration();
                    let snapped = if moved {
                        None
                    } else {
                        let marks = inner
                            .shown
                            .as_ref()
                            .map(|s| s.marks_ms.clone())
                            .unwrap_or_default();
                        nearest_mark(at * 360.0, &marks, duration)
                    };
                    let ms = snapped.unwrap_or_else(|| to_ms(at, duration));
                    if let Some(shown) = inner.shown.as_mut() {
                        shown.position_ms = ms;
                    }
                    inner.draw_at(fraction(ms, duration));
                    Some(ScrubEvent::Seek(ms))
                }
            }
        };
        if let Some(event) = event {
            (self.on_event.borrow_mut())(event);
        }
    }

    /// LVGL lost the press: a scrub commits where it is, a centre press is dropped.
    pub fn lost(&self) {
        let centre = matches!(self.inner.borrow().gesture, Gesture::Centre);
        if centre {
            self.inner.borrow_mut().gesture = Gesture::Idle;
        } else {
            self.release();
        }
    }

    pub fn hit(&self) -> Obj {
        self.inner.borrow().hit
    }

    pub fn knob(&self) -> Obj {
        self.inner.borrow().knob
    }

    pub fn icon(&self) -> Obj {
        self.inner.borrow().icon
    }

    pub fn time(&self) -> Obj {
        self.inner.borrow().time
    }

    pub fn tick_count(&self) -> usize {
        self.inner.borrow().ticks.len()
    }
}

impl Inner {
    fn playable(&self) -> bool {
        self.shown.as_ref().is_some_and(|s| s.playable)
    }

    fn duration(&self) -> u32 {
        self.shown.as_ref().map_or(0, |s| s.duration_ms)
    }

    /// Progress, knob and caption at `at` (0..1 of the file).
    fn draw_at(&self, at: f32) {
        let at = at.clamp(0.0, 1.0);
        let duration = self.duration();
        let playable = self.playable();
        // SAFETY: live arc (range 0..1000 from `full_ring`).
        unsafe { lv_arc_set_value(self.ring.raw(), (at * 1000.0).round() as i32) };
        let (x, y) = polar(CX, CY, R as f32, at * 360.0);
        self.knob.pos(x - KNOB / 2, y - KNOB / 2).hidden(!playable);
        self.time
            .hidden(!playable)
            .set_text(&time_text(to_ms(at, duration), duration));
    }

    /// One 3° tick on r 70 per mark, square-ended, in Text 2.
    fn rebuild_ticks(&mut self) {
        for tick in self.ticks.drain(..) {
            tick.delete();
        }
        let duration = self.duration();
        if !self.playable() || duration == 0 {
            return;
        }
        let marks = self
            .shown
            .as_ref()
            .map(|s| s.marks_ms.clone())
            .unwrap_or_default();
        for mark in marks {
            let deg = fraction(mark, duration) * 360.0;
            let tick = arc(self.layer, CX, CY, TICK_R, TICK_WIDTH);
            // SAFETY: live arc.
            unsafe {
                lv_arc_set_bg_angles(
                    tick.raw(),
                    lvgl_angle(deg - TICK_SPAN / 2.0),
                    lvgl_angle(deg + TICK_SPAN / 2.0),
                );
            }
            arc_style(tick, MAIN, TEXT_2, 255, TICK_WIDTH);
            arc_style(tick, INDICATOR, TEXT_2, 0, TICK_WIDTH);
            // SAFETY: live arc; the mockup's ticks have butt ends.
            unsafe { lv_obj_set_style_arc_rounded(tick.raw(), false, MAIN) };
            self.ticks.push(tick);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use std::cell::RefCell;

    const DURATION: u32 = 2_537_000; // 42:17
    const MARKS: [u32; 2] = [384_000, 1_413_000]; // ticks at 54.5° and 200.5°

    fn at(deg: f32, r: f32) -> (i32, i32) {
        polar(CX, CY, r, deg)
    }

    fn rig(root: Obj) -> (Scrubber, Rc<RefCell<Vec<ScrubEvent>>>) {
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        let scrubber = Scrubber::new(root, move |event| sink.borrow_mut().push(event));
        scrubber.set(892_000, DURATION, &MARKS, true, true);
        (scrubber, events)
    }

    #[test]
    fn geometry_follows_the_device_angle_convention() {
        assert_eq!(zone(CX, CY), Zone::Centre);
        assert_eq!(zone(CX + 43, CY), Zone::Centre);
        assert_eq!(zone(CX + 60, CY), Zone::Ring);
        assert_eq!(zone(CX, CY - HIT_R), Zone::Ring);
        assert_eq!(zone(CX + 80, CY + 80), Zone::Outside);
        // The ring band is a 44 px target, like every other one.
        assert!(HIT_R as f32 - CENTRE_R >= 44.0, "ring band too thin");
        assert_eq!(angle_at(CX, CY - 60), 0.0);
        assert_eq!(angle_at(CX + 60, CY), 90.0);
        assert_eq!(angle_at(CX, CY + 60), 180.0);
        assert_eq!(angle_at(CX - 60, CY), 270.0);
        assert_eq!(time_text(892_000, DURATION), "14:52 / 42:17");
        assert_eq!(clock(3_900_000), "1:05:00");
        assert_eq!(clock(0), "0:00");
        assert_eq!(to_ms(0.25, DURATION), 634_250);
        assert_eq!(fraction(5_000, 0), 0.0);
        assert_eq!(fraction(DURATION * 2, DURATION), 1.0);
    }

    #[test]
    fn taps_near_a_tick_snap_to_its_mark() {
        assert_eq!(nearest_mark(60.0, &MARKS, DURATION), Some(384_000));
        assert_eq!(nearest_mark(90.0, &MARKS, DURATION), None);
        assert_eq!(nearest_mark(205.0, &MARKS, DURATION), Some(1_413_000));
        // Across the top: a mark at 359° and a tap at 1°.
        assert_eq!(nearest_mark(1.0, &[2_530_000], DURATION), Some(2_530_000));
        assert_eq!(nearest_mark(1.0, &MARKS, 0), None);
    }

    #[test]
    fn dragging_past_the_top_clamps_instead_of_wrapping() {
        assert_eq!(unwind(350.0, 20.0), 380.0);
        assert_eq!(unwind(10.0, 340.0), -20.0);
        assert_eq!(unwind(380.0, 340.0), 340.0);
        assert_eq!(unwind(90.0, 100.0), 100.0);
        assert_eq!(follow(380.0), 1.0);
        assert_eq!(follow(-20.0), 0.0);
        assert!((follow(340.0) - 340.0 / 360.0).abs() < 1e-6);
        lvgl(|lv| {
            let root = lv.screen();
            let (scrubber, events) = rig(root);
            let (x, y) = at(350.0, 60.0);
            scrubber.press(x, y);
            let (x, y) = at(20.0, 60.0);
            scrubber.drag(x, y);
            scrubber.release();
            assert_eq!(*events.borrow(), vec![ScrubEvent::Seek(DURATION)]);
        });
    }

    #[test]
    fn a_drag_through_the_middle_lands_where_the_finger_comes_back() {
        lvgl(|lv| {
            let root = lv.screen();
            let (scrubber, events) = rig(root);
            let (x, y) = at(90.0, 60.0);
            scrubber.press(x, y);
            // Across the middle, where the angle means nothing...
            let (x, y) = at(90.0, 20.0);
            scrubber.drag(x, y);
            scrubber.drag(CX, CY);
            let (x, y) = at(270.0, 20.0);
            scrubber.drag(x, y);
            // ...the knob waits where it was.
            assert_eq!(
                scrubber.time().text(),
                time_text(to_ms(0.25, DURATION), DURATION)
            );
            // Back on the ring at 270°: the knob is there, not at an end.
            let (x, y) = at(270.0, 60.0);
            scrubber.drag(x, y);
            scrubber.release();
            assert_eq!(
                *events.borrow(),
                vec![ScrubEvent::Seek(to_ms(0.75, DURATION))]
            );
        });
    }

    #[test]
    fn past_the_end_the_knob_stays_until_the_finger_comes_back_across_the_top() {
        lvgl(|lv| {
            let root = lv.screen();
            let (scrubber, events) = rig(root);
            let end = time_text(DURATION, DURATION);
            let (x, y) = at(350.0, 60.0);
            scrubber.press(x, y);
            // Clockwise past the top and on round to 190°: still at the end.
            for deg in [20.0, 60.0, 100.0, 140.0, 190.0] {
                let (x, y) = at(deg, 60.0);
                scrubber.drag(x, y);
                assert_eq!(scrubber.time().text(), end, "at {deg}°");
            }
            // Back the same way: still the end until the finger crosses the top.
            for deg in [140.0, 60.0, 10.0] {
                let (x, y) = at(deg, 60.0);
                scrubber.drag(x, y);
                assert_eq!(scrubber.time().text(), end, "back at {deg}°");
            }
            let (x, y) = at(340.0, 60.0);
            scrubber.drag(x, y);
            let ms = to_ms(angle_at(x, y) / 360.0, DURATION);
            assert_eq!(scrubber.time().text(), time_text(ms, DURATION));
            scrubber.release();
            assert_eq!(*events.borrow(), vec![ScrubEvent::Seek(ms)]);
            // The same holds at the start, counter-clockwise.
            events.borrow_mut().clear();
            let (x, y) = at(10.0, 60.0);
            scrubber.press(x, y);
            for deg in [340.0, 300.0, 250.0, 170.0] {
                let (x, y) = at(deg, 60.0);
                scrubber.drag(x, y);
                assert_eq!(scrubber.time().text(), time_text(0, DURATION), "at {deg}°");
            }
            scrubber.release();
            assert_eq!(*events.borrow(), vec![ScrubEvent::Seek(0)]);
        });
    }

    #[test]
    fn the_middle_plays_and_the_ring_seeks() {
        lvgl(|lv| {
            let root = lv.screen();
            let (scrubber, events) = rig(root);
            scrubber.press(CX, CY);
            scrubber.release();
            // A ring tap away from any tick seeks to that angle.
            let (x, y) = at(90.0, 60.0);
            scrubber.press(x, y);
            scrubber.release();
            // A ring tap near a tick jumps to the mark.
            let (x, y) = at(62.0, 70.0);
            scrubber.press(x, y);
            scrubber.release();
            // A press in the middle that slides onto the ring does nothing.
            scrubber.press(CX, CY);
            let (x, y) = at(0.0, 60.0);
            scrubber.drag(x, y);
            scrubber.release();
            assert_eq!(
                *events.borrow(),
                vec![
                    ScrubEvent::TogglePlay,
                    ScrubEvent::Seek(to_ms(0.25, DURATION)),
                    ScrubEvent::Seek(384_000),
                ]
            );
            assert_eq!(scrubber.time().text(), "6:24 / 42:17");
        });
    }

    #[test]
    fn a_drag_follows_the_finger_and_ignores_playback_until_released() {
        lvgl(|lv| {
            let root = lv.screen();
            let (scrubber, events) = rig(root);
            let (x, y) = at(90.0, 60.0);
            scrubber.press(x, y);
            let (x, y) = at(180.0, 64.0);
            scrubber.drag(x, y);
            // Playback keeps reporting the old position while the finger is down.
            scrubber.set(893_000, DURATION, &MARKS, true, true);
            assert_eq!(scrubber.time().text(), "21:08 / 42:17");
            scrubber.release();
            assert_eq!(
                *events.borrow(),
                vec![ScrubEvent::Seek(to_ms(0.5, DURATION))]
            );
        });
    }

    #[test]
    fn an_unplayable_file_shows_only_the_track() {
        lvgl(|lv| {
            let root = lv.screen();
            let (scrubber, events) = rig(root);
            scrubber.set(0, 0, &[], false, false);
            assert!(scrubber.knob().is_hidden() && scrubber.icon().is_hidden());
            assert!(scrubber.time().is_hidden());
            assert_eq!(scrubber.tick_count(), 0);
            scrubber.press(CX, CY);
            scrubber.release();
            assert!(events.borrow().is_empty());
        });
    }

    #[test]
    fn the_scrubber_matches_the_detail_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (scrubber, _events) = rig(root);
            assert_eq!(scrubber.icon().text(), icons::PAUSE);
            assert_eq!(scrubber.tick_count(), 2);
            assert_eq!(scrubber.time().text(), "14:52 / 42:17");
            let frame = lv.render();
            let lit = |(x, y): (i32, i32)| frame.pixel(x, y) != [0, 0, 0];
            // Knob at 126.6° on r 60: a light disc.
            let (x, y) = at(126.6, 60.0);
            assert!(frame.pixel(x, y).iter().all(|c| *c > 220), "knob");
            // Teal progress at 90°, the Line track at 250°.
            let [r, g, _] = frame.pixel(CX + 60, CY);
            assert!(g > 150 && r < 120, "progress is teal");
            assert!(lit(at(250.0, 60.0)), "track");
            // Ticks at the marks on r 70, nothing between them.
            assert!(lit(at(54.5, 70.0)) && lit(at(200.5, 70.0)), "ticks");
            assert!(!lit(at(110.0, 70.0)), "no tick at 110°");
            assert_golden("scrubber_playing", &frame);
            assert_targets_ok(root);
        });
    }

    #[test]
    fn dragging_the_knob_never_reads_as_a_screen_swipe() {
        lvgl(|lv| {
            let root = lv.screen();
            let (scrubber, _events) = rig(root);
            let hit = scrubber.hit();
            assert!(!hit.has_flag(lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE));
            assert!(hit.is_clickable(), "keeping gestures leaves taps alone");
        });
    }
}
