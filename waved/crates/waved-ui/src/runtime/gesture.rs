//! The runtime's swipes: down from the top edge opens Menu, right goes Back
//! (mockups "Interaction model"). Every other swipe goes to the screen.
//!
//! One recogniser for everyone: `kit::event::{Swipe, SwipeTracker}`. A press
//! that starts on a control that keeps its drags (`Obj::keep_gestures`) never
//! swipes. The caller cancels the press (`lv_indev_wait_release`) whenever a
//! swipe is taken, so a swipe never also fires a tap.

use crate::bindings::{
    lv_event_code_t_LV_EVENT_PRESS_LOST, lv_indev_search_obj, lv_obj_get_screen, lv_obj_is_valid,
    lv_obj_send_event, lv_point_t,
};
use crate::kit::dispatch::UiEvent;
use crate::kit::event::{send_swipe, Swipe, SwipeTracker};
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::snapshot::UiSnapshot;
use crate::runtime::touch::TouchSample;
use waved_state::Command;

/// Bottom of the status zone (y 6–34): the swipe down to Menu starts above it.
pub const TOP_EDGE_Y: i32 = 34;

/// The object LVGL would press at (x, y) on `root`: what `lv_indev_search_obj`
/// returns, the topmost visible object whose hit test passes. LVGL 9.2's hit
/// test (`lv_obj_hit_test`) fails for any object without
/// `LV_OBJ_FLAG_CLICKABLE`, so the result is always clickable and needs no
/// walk up. It does not skip disabled objects: LVGL presses those too and
/// only withholds their events. None for a null root or when nothing is hit.
pub fn press_target(root: Obj, x: i32, y: i32) -> Option<Obj> {
    if root.is_null() {
        return None;
    }
    // The kit's debug-build liveness check (`Obj` handles are `Copy`).
    #[cfg(debug_assertions)]
    debug_assert!(
        // SAFETY: only compares the pointer against live objects.
        unsafe { lv_obj_is_valid(root.raw()) },
        "press_target on a deleted root"
    );
    let mut point = lv_point_t { x, y };
    // SAFETY: `root` is non-null and live (checked above in debug builds; the
    // caller passes the loaded screen); LVGL only reads the point (it
    // transforms a copy) and returns null or a live object under `root`.
    let hit = unsafe { lv_indev_search_obj(root.raw(), &mut point) };
    // SAFETY: LVGL returns null or a live descendant of `root`.
    (!hit.is_null()).then(|| unsafe { Obj::from_raw(hit) })
}

/// True when `obj` or one of its ancestors below `root` keeps its drags
/// (`LV_OBJ_FLAG_GESTURE_BUBBLE` cleared, `Obj::keep_gestures`).
pub fn keeps_drags(obj: Obj, root: Obj) -> bool {
    let mut current = Some(obj);
    while let Some(o) = current {
        if o == root || o.is_null() {
            return false;
        }
        if !o.has_flag(crate::bindings::lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE) {
            return true;
        }
        current = o.parent();
    }
    false
}

/// The runtime's own swipes (mockups "Interaction model"): Down that started at
/// y <= TOP_EDGE_Y on Home or Recording -> Navigate(Menu); Right on any
/// sub-screen (depth > 1, current != Resting) -> Back. Everything else is None
/// and belongs to the screen.
pub fn runtime_gesture(
    swipe: Swipe,
    start_y: i32,
    current: &Route,
    depth: usize,
) -> Option<UiEvent> {
    match swipe {
        Swipe::Down
            if start_y <= TOP_EDGE_Y && matches!(current, Route::Home | Route::Recording) =>
        {
            Some(UiEvent::Navigate(Route::Menu))
        }
        Swipe::Right if depth > 1 && *current != Route::Resting => Some(UiEvent::Back),
        _ => None,
    }
}

/// Commands that go with leaving `current` — the same commands its own
/// Back/Cancel button sends. The runtime sends them once when the route leaves
/// the router stack, whatever removed it (a duplicate of the button's own
/// send is accepted):
/// WifiSetup -> [CancelWifiProvisioning] (always, as its Cancel does; the
/// provisioning service ignores a Cancel when idle);
/// FileDetail(id) with snap.playback.recording == Some(id) -> [StopPlayback];
/// otherwise empty.
pub fn leave_commands(current: &Route, snap: &UiSnapshot) -> Vec<Command> {
    match current {
        Route::WifiSetup => vec![Command::CancelWifiProvisioning],
        Route::FileDetail(id) if snap.playback.recording == Some(*id) => {
            vec![Command::StopPlayback]
        }
        _ => Vec::new(),
    }
}

/// Who took a swipe. Either way the caller cancels the press
/// (`lv_indev_wait_release`) so a swipe never also fires a tap.
#[derive(Debug, PartialEq)]
pub enum SwipeTaken {
    /// The runtime's Menu or Back.
    Runtime(UiEvent),
    /// A screen's `on_swipe` handler ran (`send_swipe` returned true).
    Screen,
}

/// What the runtime learnt when the current press started.
#[derive(Clone, Copy, Debug)]
struct PressStart {
    /// The object LVGL pressed, if any.
    target: Option<Obj>,
    /// The press started on a control that keeps its drags: it never swipes.
    blocked: bool,
}

/// The runtime's swipe recogniser over the touch samples LVGL sees.
#[derive(Debug, Default)]
pub struct Gestures {
    tracker: SwipeTracker,
    press: Option<PressStart>,
}

impl Gestures {
    /// Feed every touch sample LVGL will see (after the wake gate; a swallowed
    /// press is fed as released). At press start it records the press target on
    /// `root` and whether it keeps its drags: such a press never swipes, in any
    /// direction (checked before recognising anything). On a recognised swipe:
    /// `runtime_gesture` first; otherwise `send_swipe(target, dir)`, where target is the recorded press
    /// target if still valid (`lv_obj_is_valid`, and still on `root`), else
    /// `root` -> `Screen` if it returned true. Returns None when nobody took it.
    ///
    /// A null `root` (no screen loaded yet) blocks nothing and takes no screen
    /// swipes; the runtime's own swipes still work.
    pub fn sample(
        &mut self,
        s: TouchSample,
        now_ms: u64,
        root: Obj,
        current: &Route,
        depth: usize,
    ) -> Option<SwipeTaken> {
        if !s.pressed {
            self.press = None;
            self.tracker.sample(s.x, s.y, false, now_ms);
            return None;
        }
        let press = *self.press.get_or_insert_with(|| {
            let target = press_target(root, s.x, s.y);
            PressStart {
                target,
                blocked: target.is_some_and(|t| keeps_drags(t, root)),
            }
        });
        if press.blocked {
            return None;
        }
        let direction = self.tracker.sample(s.x, s.y, true, now_ms)?;
        let (_, start_y) = self.tracker.start()?;
        if let Some(event) = runtime_gesture(direction, start_y, current, depth) {
            return Some(SwipeTaken::Runtime(event));
        }
        if root.is_null() {
            return None;
        }
        let target = press.target.filter(|&t| still_on(t, root)).unwrap_or(root);
        send_swipe(target, direction).then_some(SwipeTaken::Screen)
    }

    /// The press that is down now is being taken away from the object it
    /// pressed on `root` (the screen is being parked, so the object stays
    /// alive): tell that object `LV_EVENT_PRESS_LOST`, as LVGL does when a
    /// press is lost, so what it started (Stop's hold ring) resets. LVGL's own
    /// PRESS_LOST after `lv_indev_wait_release` comes only on the release.
    pub fn press_lost(&self, root: Obj) {
        let Some(target) = self
            .press
            .and_then(|press| press.target)
            .filter(|&target| !root.is_null() && still_on(target, root))
        else {
            return;
        };
        // SAFETY: `still_on` checked that `target` is a live object.
        unsafe {
            lv_obj_send_event(
                target.raw(),
                lv_event_code_t_LV_EVENT_PRESS_LOST,
                std::ptr::null_mut(),
            );
        }
    }

    /// The screen changed under the current press: it swipes nothing more,
    /// in any direction, until it is lifted.
    pub fn ignore_press(&mut self) {
        self.press = Some(PressStart {
            target: None,
            blocked: true,
        });
    }
}

/// `obj` still exists and is on `root` (a handler may have deleted it, or the
/// screen may have changed, since the press started).
fn still_on(obj: Obj, root: Obj) -> bool {
    // SAFETY: `lv_obj_is_valid` only compares the pointer against live
    // objects; `lv_obj_get_screen` runs only on a live one.
    unsafe { lv_obj_is_valid(obj.raw()) && lv_obj_get_screen(obj.raw()) == root.raw() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::components::controls::ValueArc;
    use crate::kit::components::mode_picker::{Glyph, ModePicker, CENTRE};
    use crate::kit::components::scrubber::{self, Scrubber};
    use crate::kit::event::on_swipe;
    use crate::kit::route::ConfirmKind;
    use crate::kit::screen::Screen;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::Rect;
    use crate::screens_v2::menu::fixture::{fixture, snap};
    use crate::screens_v2::volume::VolumeScreen;
    use std::cell::Cell;
    use std::rc::Rc;
    use uuid::Uuid;
    use waved_state::Command;

    fn down(x: i32, y: i32) -> TouchSample {
        TouchSample {
            x,
            y,
            pressed: true,
        }
    }

    fn up() -> TouchSample {
        TouchSample::default()
    }

    fn null() -> Obj {
        // SAFETY: a null handle is what LVGL reports before the first screen loads.
        unsafe { Obj::from_raw(std::ptr::null_mut()) }
    }

    /// Press at `from`, move to `to` after `ms`, and lift; what the move did.
    fn swipe(
        root: Obj,
        current: &Route,
        depth: usize,
        from: (i32, i32),
        to: (i32, i32),
        ms: u64,
    ) -> Option<SwipeTaken> {
        let mut g = Gestures::default();
        assert_eq!(
            g.sample(down(from.0, from.1), 1_000, root, current, depth),
            None
        );
        let taken = g.sample(down(to.0, to.1), 1_000 + ms, root, current, depth);
        assert_eq!(g.sample(up(), 1_000 + ms + 20, root, current, depth), None);
        taken
    }

    fn menu() -> Option<SwipeTaken> {
        Some(SwipeTaken::Runtime(UiEvent::Navigate(Route::Menu)))
    }

    fn back() -> Option<SwipeTaken> {
        Some(SwipeTaken::Runtime(UiEvent::Back))
    }

    fn counter(root: Obj, direction: Swipe) -> Rc<Cell<u32>> {
        let count = Rc::new(Cell::new(0));
        let seen = count.clone();
        on_swipe(root, direction, move || seen.set(seen.get() + 1));
        count
    }

    #[test]
    fn swipe_down_from_the_top_edge_opens_menu_once_per_press() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut g = Gestures::default();
            let mut feed = |s, at| g.sample(s, at, root, &Route::Home, 1);
            assert_eq!(feed(down(200, 12), 0), None);
            assert_eq!(feed(down(202, 50), 80), None, "38 px is not yet a swipe");
            assert_eq!(feed(down(206, 75), 160), menu());
            assert_eq!(feed(down(206, 140), 240), None, "once per press");
            assert_eq!(feed(up(), 300), None);
            // The edge itself still counts; Recording is a base too.
            assert_eq!(
                swipe(
                    root,
                    &Route::Recording,
                    1,
                    (200, TOP_EDGE_Y),
                    (200, 110),
                    150
                ),
                menu()
            );
        });
    }

    #[test]
    fn a_swipe_down_from_below_the_edge_belongs_to_the_screen() {
        lvgl(|lv| {
            let root = lv.screen();
            assert_eq!(
                swipe(
                    root,
                    &Route::Home,
                    1,
                    (200, TOP_EDGE_Y + 1),
                    (200, 120),
                    150
                ),
                None,
                "no handler for it"
            );
            let downs = counter(root, Swipe::Down);
            assert_eq!(
                swipe(root, &Route::Home, 1, (200, 60), (200, 160), 150),
                Some(SwipeTaken::Screen)
            );
            assert_eq!(downs.get(), 1);
        });
    }

    #[test]
    fn swipe_down_opens_menu_only_on_home_and_recording() {
        lvgl(|lv| {
            let root = lv.screen();
            assert_eq!(
                swipe(root, &Route::Decision, 1, (200, 12), (200, 90), 150),
                None
            );
            assert_eq!(
                swipe(root, &Route::Menu, 2, (200, 12), (200, 90), 150),
                None
            );
        });
    }

    #[test]
    fn swipe_right_goes_back_on_sub_screens() {
        lvgl(|lv| {
            let root = lv.screen();
            assert_eq!(
                swipe(root, &Route::Volume, 3, (100, 150), (170, 160), 150),
                back()
            );
            assert_eq!(
                swipe(
                    root,
                    &Route::Confirm(ConfirmKind::PowerOff),
                    2,
                    (100, 150),
                    (170, 150),
                    150
                ),
                back(),
                "swiping a dialog away cancels it"
            );
            assert_eq!(
                swipe(root, &Route::Home, 1, (100, 150), (170, 150), 150),
                None
            );
            assert_eq!(
                swipe(root, &Route::Resting, 2, (100, 150), (170, 150), 150),
                None
            );
        });
    }

    #[test]
    fn too_slow_too_diagonal_or_leftwards_is_not_a_runtime_swipe() {
        lvgl(|lv| {
            let root = lv.screen();
            for (end, ms) in [((170, 200), 150), ((170, 150), 800), ((30, 150), 150)] {
                assert_eq!(
                    swipe(root, &Route::Volume, 2, (100, 150), end, ms),
                    None,
                    "{end:?} after {ms} ms"
                );
            }
        });
    }

    #[test]
    fn runtime_gesture_maps_swipes_by_route_and_depth() {
        assert_eq!(
            runtime_gesture(Swipe::Down, 12, &Route::Home, 1),
            Some(UiEvent::Navigate(Route::Menu))
        );
        assert_eq!(runtime_gesture(Swipe::Down, 35, &Route::Home, 1), None);
        assert_eq!(runtime_gesture(Swipe::Down, 12, &Route::Menu, 2), None);
        assert_eq!(
            runtime_gesture(Swipe::Right, 150, &Route::Volume, 3),
            Some(UiEvent::Back)
        );
        assert_eq!(runtime_gesture(Swipe::Right, 150, &Route::Home, 1), None);
        assert_eq!(runtime_gesture(Swipe::Right, 150, &Route::Resting, 2), None);
        assert_eq!(runtime_gesture(Swipe::Left, 150, &Route::Volume, 3), None);
        assert_eq!(runtime_gesture(Swipe::Up, 150, &Route::Volume, 3), None);
    }

    #[test]
    fn press_target_and_keeps_drags_find_controls_that_keep_gestures() {
        lvgl(|lv| {
            let root = lv.screen();
            let button = Obj::plain(root)
                .rect(Rect::new(40, 60, 100, 60))
                .clickable(true);
            let knob = Obj::plain(root)
                .rect(Rect::new(220, 60, 100, 60))
                .clickable(true)
                .keep_gestures();
            let owner = Obj::plain(root)
                .rect(Rect::new(120, 160, 160, 60))
                .keep_gestures();
            let inner = Obj::plain(owner)
                .rect(Rect::new(10, 10, 60, 40))
                .clickable(true);
            root.update_layout();
            assert_eq!(press_target(root, 90, 90), Some(button));
            assert!(
                !keeps_drags(button, root),
                "a plain button lets swipes through"
            );
            assert_eq!(press_target(root, 270, 90), Some(knob));
            assert!(keeps_drags(knob, root), "a control that keeps its drags");
            assert_eq!(
                press_target(root, 150, 190),
                Some(inner),
                "LVGL presses the clickable child, not the container"
            );
            assert!(
                keeps_drags(inner, root),
                "inside a container that keeps them"
            );
            assert_eq!(
                press_target(root, 200, 205),
                None,
                "the container is not clickable"
            );
            assert_eq!(press_target(root, 380, 280), None, "empty space");
            assert!(!keeps_drags(root, root));
            assert_eq!(press_target(null(), 90, 90), None);
            // The blocks, through the runtime: none of these swipes back.
            for (start, blocked) in [
                ((90, 90), false),
                ((270, 90), true),
                ((150, 190), true),
                ((380, 280), false),
            ] {
                let taken = swipe(root, &Route::Volume, 2, start, (start.0 + 70, start.1), 150);
                assert_eq!(taken.is_some(), !blocked, "press at {start:?}");
            }
            assert_eq!(
                swipe(null(), &Route::Volume, 2, (90, 90), (160, 90), 150),
                back(),
                "a null root blocks nothing"
            );
        });
    }

    #[test]
    fn a_blocked_press_never_swipes_in_any_direction() {
        lvgl(|lv| {
            let root = lv.screen();
            Obj::plain(root)
                .rect(Rect::new(0, 0, 400, 304))
                .clickable(true)
                .keep_gestures();
            root.update_layout();
            let lefts = counter(root, Swipe::Left);
            assert_eq!(
                swipe(root, &Route::Home, 1, (200, 10), (200, 100), 150),
                None,
                "down from the edge"
            );
            assert_eq!(
                swipe(root, &Route::Volume, 2, (100, 150), (180, 150), 150),
                None,
                "right"
            );
            assert_eq!(
                swipe(root, &Route::Volume, 2, (300, 150), (220, 150), 150),
                None,
                "left"
            );
            assert_eq!(lefts.get(), 0);
        });
    }

    #[test]
    fn drags_on_real_controls_never_swipe() {
        lvgl(|lv| {
            // ValueArc: the ring and the knob (at 0, bottom-left).
            let root = lv.screen();
            let rights = counter(root, Swipe::Right);
            let arc = ValueArc::new(root, 200, 160, 100, |_| {});
            arc.set_value(0);
            root.update_layout();
            for at in [(200, 60), (129, 231)] {
                assert_eq!(
                    press_target(root, at.0, at.1),
                    Some(arc.obj()),
                    "{at:?} is on the arc"
                );
                assert_eq!(
                    swipe(root, &Route::Volume, 2, at, (at.0 + 70, at.1), 150),
                    None,
                    "{at:?}"
                );
            }
            // Only the ring band takes presses: the dial centre lets swipes through.
            assert_eq!(press_target(root, 200, 160), None, "the dial centre");
            assert_eq!(
                swipe(root, &Route::Volume, 2, (200, 160), (270, 160), 150),
                back(),
                "from the dial centre"
            );
            assert_eq!(
                swipe(root, &Route::Volume, 2, (20, 160), (90, 160), 150),
                back(),
                "beside the arc"
            );
            assert_eq!(rights.get(), 0);

            // ModePicker: the aim area while aimable; not clickable otherwise.
            let root = lv.screen();
            let glyphs = std::array::from_fn(|_| Glyph {
                lobes: Vec::new(),
                ghost: None,
            });
            let picker = ModePicker::new(root, glyphs, 2, |_| {}, |_| {});
            picker.set_aimable(true);
            root.update_layout();
            let centre = CENTRE;
            assert_eq!(
                press_target(root, centre.0, centre.1),
                Some(picker.aim_area())
            );
            assert_eq!(
                swipe(
                    root,
                    &Route::AudioMode,
                    2,
                    centre,
                    (centre.0 + 70, centre.1),
                    150
                ),
                None
            );
            picker.set_aimable(false);
            assert_eq!(
                swipe(
                    root,
                    &Route::AudioMode,
                    2,
                    centre,
                    (centre.0 + 70, centre.1),
                    150
                ),
                back(),
                "a mode without aim takes no drags"
            );

            // Scrubber: its hit disc.
            let root = lv.screen();
            let scrub = Scrubber::new(root, |_| {});
            root.update_layout();
            let on_ring = (scrubber::CX, scrubber::CY - scrubber::R);
            assert_eq!(press_target(root, on_ring.0, on_ring.1), Some(scrub.hit()));
            assert_eq!(
                swipe(
                    root,
                    &Route::Files,
                    3,
                    on_ring,
                    (on_ring.0 + 70, on_ring.1),
                    150
                ),
                None
            );
        });
    }

    #[test]
    fn other_swipes_go_to_the_screen_and_right_stays_the_runtimes() {
        lvgl(|lv| {
            let root = lv.screen();
            let lefts = counter(root, Swipe::Left);
            let rights = counter(root, Swipe::Right);
            let button = Obj::plain(root)
                .rect(Rect::new(150, 120, 100, 60))
                .clickable(true);
            root.update_layout();
            assert_eq!(press_target(root, 230, 150), Some(button));
            assert_eq!(
                swipe(root, &Route::Volume, 2, (230, 150), (160, 150), 150),
                Some(SwipeTaken::Screen),
                "from the button, up to the screen"
            );
            assert_eq!(lefts.get(), 1);
            assert_eq!(
                swipe(root, &Route::Volume, 2, (160, 150), (240, 150), 150),
                back()
            );
            assert_eq!(
                rights.get(),
                0,
                "Back on a sub-screen wins over the screen's handler"
            );
            assert_eq!(
                swipe(root, &Route::Home, 1, (160, 150), (240, 150), 150),
                Some(SwipeTaken::Screen)
            );
            assert_eq!(rights.get(), 1, "a base screen gets swipe right");
        });
    }

    #[test]
    fn leave_commands_cancel_setup_and_stop_this_recordings_playback() {
        let id = Uuid::from_u128(7);
        let mut snap = UiSnapshot::default();
        assert_eq!(
            leave_commands(&Route::WifiSetup, &snap),
            vec![Command::CancelWifiProvisioning]
        );
        assert!(
            leave_commands(&Route::FileDetail(id), &snap).is_empty(),
            "nothing playing"
        );
        snap.playback.recording = Some(Uuid::from_u128(8));
        assert!(
            leave_commands(&Route::FileDetail(id), &snap).is_empty(),
            "another recording plays"
        );
        snap.playback.recording = Some(id);
        assert_eq!(
            leave_commands(&Route::FileDetail(id), &snap),
            vec![Command::StopPlayback]
        );
        assert!(leave_commands(&Route::Menu, &snap).is_empty());
    }

    #[test]
    fn the_volume_dial_centre_lets_swipes_through() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = VolumeScreen::speaker();
            screen.build(root, &f.ctx, &snap());
            let centre = (200, 132);
            assert_eq!(press_target(root, centre.0, centre.1), None);
            assert_eq!(
                swipe(root, &Route::Volume, 2, centre, (130, 132), 150),
                Some(SwipeTaken::Screen),
                "swipe left opens Mic level"
            );
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::MicLevel)]);
            assert_eq!(
                swipe(root, &Route::Volume, 2, centre, (270, 132), 150),
                back()
            );
            // The band stays clear of the − and + buttons and the mute button.
            let ring = press_target(root, 200, 44).expect("the arc's ring");
            for at in [(81, 132), (318, 132), (200, 200)] {
                let hit = press_target(root, at.0, at.1);
                assert!(hit.is_some() && hit != Some(ring), "{at:?} is a button");
            }
            // The ring and the knob still keep their drags.
            for at in [(200, 44), (288, 132)] {
                assert_eq!(
                    swipe(root, &Route::Volume, 2, at, (at.0 - 70, at.1), 150),
                    None,
                    "{at:?}"
                );
            }
            assert_eq!(f.events(), vec![]);
        });
    }

    #[test]
    fn one_tracker_blocks_a_press_and_lets_the_next_one_swipe() {
        lvgl(|lv| {
            let root = lv.screen();
            Obj::plain(root)
                .rect(Rect::new(40, 60, 100, 60))
                .clickable(true)
                .keep_gestures();
            root.update_layout();
            let mut g = Gestures::default();
            let mut feed = |s, at| g.sample(s, at, root, &Route::Volume, 2);
            assert_eq!(feed(down(90, 90), 0), None);
            assert_eq!(feed(down(170, 90), 100), None, "blocked");
            assert_eq!(feed(up(), 150), None);
            assert_eq!(feed(down(200, 200), 1_000), None);
            assert_eq!(feed(down(270, 200), 1_100), back(), "the next press swipes");
            assert_eq!(feed(up(), 1_150), None);
            // And back again: a blocked press after a swipe.
            assert_eq!(feed(down(90, 90), 2_000), None);
            assert_eq!(feed(down(170, 90), 2_100), None, "blocked again");
        });
    }

    #[test]
    fn an_ignored_press_swipes_nothing_until_it_is_lifted() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut g = Gestures::default();
            let feed = |g: &mut Gestures, s, at| g.sample(s, at, root, &Route::Volume, 2);
            assert_eq!(feed(&mut g, down(100, 150), 0), None);
            g.ignore_press();
            assert_eq!(feed(&mut g, down(170, 150), 100), None, "ignored");
            assert_eq!(feed(&mut g, up(), 150), None);
            assert_eq!(feed(&mut g, down(100, 150), 1_000), None);
            assert_eq!(
                feed(&mut g, down(170, 150), 1_100),
                back(),
                "the next press swipes"
            );
        });
    }

    #[test]
    fn a_press_target_deleted_mid_press_hands_the_swipe_to_the_screen() {
        lvgl(|lv| {
            let root = lv.screen();
            let lefts = counter(root, Swipe::Left);
            let page = Obj::plain(root).rect(Rect::new(0, 0, 400, 304));
            let button = Obj::plain(page)
                .rect(Rect::new(150, 120, 100, 60))
                .clickable(true);
            root.update_layout();
            let mut g = Gestures::default();
            assert_eq!(g.sample(down(230, 150), 0, root, &Route::Volume, 2), None);
            assert_eq!(press_target(root, 230, 150), Some(button));
            page.delete();
            assert_eq!(
                g.sample(down(160, 150), 100, root, &Route::Volume, 2),
                Some(SwipeTaken::Screen)
            );
            assert_eq!(lefts.get(), 1);
        });
    }

    #[test]
    fn a_press_target_on_a_screen_that_was_replaced_hands_the_swipe_to_the_new_one() {
        lvgl(|lv| {
            let old = lv.screen();
            let old_lefts = counter(old, Swipe::Left);
            let button = Obj::plain(old)
                .rect(Rect::new(150, 120, 100, 60))
                .clickable(true);
            old.update_layout();
            let mut g = Gestures::default();
            g.sample(down(230, 150), 0, old, &Route::Volume, 2);
            assert_eq!(press_target(old, 230, 150), Some(button));
            // A second screen, loaded while the old one still exists.
            let new = Obj::screen();
            let new_lefts = counter(new, Swipe::Left);
            assert_eq!(
                g.sample(down(160, 150), 100, new, &Route::Volume, 2),
                Some(SwipeTaken::Screen)
            );
            assert_eq!((old_lefts.get(), new_lefts.get()), (0, 1));
            new.delete();
        });
    }
}
