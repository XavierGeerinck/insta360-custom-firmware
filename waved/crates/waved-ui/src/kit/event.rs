//! Event handlers whose captured state is freed when LVGL deletes the object.
//! Replaces the legacy `Box::into_raw` user data that leaked on every navigation.

use crate::bindings::*;
use crate::kit::obj::Obj;

use std::panic::{catch_unwind, AssertUnwindSafe};

type Handler = Box<dyn FnMut(&Event)>;

/// A registered handler. LVGL holds a raw pointer to it as callback user data,
/// and both `dispatch` and `release` get that pointer.
///
/// The handler runs while `running` is set and is taken out of the slot, so a
/// nested event for the same slot finds nothing to call (no aliasing `&mut`).
/// If the handler deletes its own object, `release` only marks the slot `dead`
/// and `dispatch` frees it once the handler has returned.
struct Slot {
    handler: Option<Handler>,
    running: bool,
    dead: bool,
}

/// What a handler sees about the event that fired.
pub struct Event {
    raw: *mut lv_event_t,
}

impl Event {
    pub fn code(&self) -> lv_event_code_t {
        // SAFETY: valid for the duration of the callback.
        unsafe { lv_event_get_code(self.raw) }
    }

    pub fn target(&self) -> Obj {
        // SAFETY: valid for the duration of the callback; LVGL's target is a
        // live object (or null), which is `Obj::from_raw`'s contract.
        unsafe { Obj::from_raw(lv_event_get_target(self.raw).cast()) }
    }
}

/// Run `handler` whenever `obj` emits `code`. The closure is dropped when
/// `obj` is deleted, so captured state never leaks. A handler may delete its
/// own object: the closure is then dropped as soon as it returns.
///
/// A panic in a handler is caught at the FFI boundary and logged. In tests it
/// is re-raised by the next `Lvgl::send`, `click`, `render` or `advance`.
pub fn on(obj: Obj, code: lv_event_code_t, handler: impl FnMut(&Event) + 'static) {
    let slot: *mut Slot = Box::into_raw(Box::new(Slot {
        handler: Some(Box::new(handler)),
        running: false,
        dead: false,
    }));
    // SAFETY: `slot` stays valid until `release` (or `dispatch`, if `release`
    // ran while the handler was running) frees it on LV_EVENT_DELETE.
    unsafe {
        lv_obj_add_event_cb(obj.raw(), Some(dispatch), code, slot.cast());
        lv_obj_add_event_cb(
            obj.raw(),
            Some(release),
            lv_event_code_t_LV_EVENT_DELETE,
            slot.cast(),
        );
    }
}

/// Run `handler` on every tap of `obj`.
pub fn on_click(obj: Obj, mut handler: impl FnMut() + 'static) {
    on(obj, lv_event_code_t_LV_EVENT_CLICKED, move |_| handler());
}

/// A swipe direction, in screen coordinates (y grows downwards).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swipe {
    Left,
    Right,
    Up,
    Down,
}

/// Travel along the swipe direction before a movement counts as a swipe.
pub const SWIPE_MIN_PX: i32 = 60;
/// Longest time from press to swipe; a slower movement is a drag.
pub const SWIPE_MAX_MS: u64 = 700;

impl Swipe {
    /// The one swipe recogniser, shared by screens (`on_swipe`) and the
    /// runtime's Menu and Back gestures. A finger that moved `(dx, dy)` px in
    /// `elapsed_ms` since it pressed swipes when it travelled at least
    /// `SWIPE_MIN_PX` along one axis and at most half as far across it (within
    /// about 27° of the direction), in at most `SWIPE_MAX_MS`.
    pub fn recognise(dx: i32, dy: i32, elapsed_ms: u64) -> Option<Swipe> {
        if elapsed_ms > SWIPE_MAX_MS {
            return None;
        }
        let (along_x, along_y) = (dx.abs(), dy.abs());
        if along_x >= SWIPE_MIN_PX && along_y * 2 <= along_x {
            Some(if dx > 0 { Swipe::Right } else { Swipe::Left })
        } else if along_y >= SWIPE_MIN_PX && along_x * 2 <= along_y {
            Some(if dy > 0 { Swipe::Down } else { Swipe::Up })
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Press {
    x: i32,
    y: i32,
    at_ms: u64,
    done: bool,
}

/// Follows one finger through its touch samples and applies
/// [`Swipe::recognise`] from where the press started. Reports at most one
/// swipe per press.
#[derive(Debug, Default)]
pub struct SwipeTracker {
    press: Option<Press>,
}

impl SwipeTracker {
    /// Feed every touch sample, in screen pixels.
    pub fn sample(&mut self, x: i32, y: i32, pressed: bool, now_ms: u64) -> Option<Swipe> {
        if !pressed {
            self.press = None;
            return None;
        }
        let Some(press) = self.press.as_mut() else {
            self.press = Some(Press {
                x,
                y,
                at_ms: now_ms,
                done: false,
            });
            return None;
        };
        if press.done {
            return None;
        }
        let swipe = Swipe::recognise(x - press.x, y - press.y, now_ms.saturating_sub(press.at_ms));
        press.done = swipe.is_some();
        swipe
    }

    /// Where the current press started, while a finger is down.
    pub fn start(&self) -> Option<(i32, i32)> {
        self.press.map(|press| (press.x, press.y))
    }
}

/// What a swipe event carries to the handlers.
struct SwipeParam {
    direction: Swipe,
    handled: std::cell::Cell<bool>,
}

/// The kit's own event code for a recognised swipe. LVGL's built-in
/// `LV_EVENT_GESTURE` uses a different recogniser, so `on_swipe` ignores it.
fn swipe_code() -> lv_event_code_t {
    static CODE: std::sync::OnceLock<lv_event_code_t> = std::sync::OnceLock::new();
    // SAFETY: LVGL is initialised before any object exists to swipe on.
    *CODE.get_or_init(|| unsafe { lv_event_register_id() } as lv_event_code_t)
}

/// Run `handler` when a swipe in `direction` is delivered to `obj` (see
/// [`send_swipe`]). Like `on`, the closure is dropped with `obj`.
pub fn on_swipe(obj: Obj, direction: Swipe, mut handler: impl FnMut() + 'static) {
    on(obj, swipe_code(), move |event| {
        // SAFETY: only `send_swipe` sends this code, with a `SwipeParam` that
        // outlives the event.
        let param = unsafe { &*(lv_event_get_param(event.raw) as *const SwipeParam) };
        if param.direction == direction {
            param.handled.set(true);
            handler();
        }
    });
}

/// Deliver a recognised swipe whose press started on `obj`. It goes where
/// LVGL sends gestures: to `obj` if it keeps its drags
/// (`Obj::keep_gestures`), else to its nearest ancestor that does, else to
/// the screen. Returns whether an `on_swipe` handler for `direction` ran.
pub fn send_swipe(obj: Obj, direction: Swipe) -> bool {
    let mut target = obj;
    while target.has_flag(lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE) {
        match target.parent() {
            Some(parent) => target = parent,
            None => break,
        }
    }
    let param = SwipeParam {
        direction,
        handled: std::cell::Cell::new(false),
    };
    // SAFETY: live object; `param` outlives the synchronous send.
    unsafe {
        lv_obj_send_event(
            target.raw(),
            swipe_code(),
            (&raw const param).cast_mut().cast(),
        );
    }
    param.handled.get()
}

unsafe extern "C" fn dispatch(e: *mut lv_event_t) {
    let slot = lv_event_get_user_data(e) as *mut Slot;
    if slot.is_null() {
        return;
    }
    // Nested event for a handler that is already running: nothing to call.
    let Some(mut handler) = (*slot).handler.take() else {
        return;
    };
    (*slot).running = true;
    let result = catch_unwind(AssertUnwindSafe(|| handler(&Event { raw: e })));
    (*slot).running = false;
    if let Err(payload) = result {
        record_panic(payload);
    }
    if (*slot).dead {
        // The object was deleted from inside the handler.
        drop(handler);
        drop(Box::from_raw(slot));
    } else {
        (*slot).handler = Some(handler);
    }
}

unsafe extern "C" fn release(e: *mut lv_event_t) {
    let slot = lv_event_get_user_data(e) as *mut Slot;
    if slot.is_null() {
        return;
    }
    if (*slot).running {
        (*slot).dead = true;
    } else {
        drop(Box::from_raw(slot));
    }
}

fn record_panic(payload: Box<dyn std::any::Any + Send>) {
    tracing::error!("UI event handler panicked");
    #[cfg(test)]
    PANIC.with(|slot| {
        slot.borrow_mut().get_or_insert(payload);
    });
    #[cfg(not(test))]
    drop(payload);
}

#[cfg(test)]
thread_local! {
    static PANIC: std::cell::RefCell<Option<Box<dyn std::any::Any + Send>>> =
        const { std::cell::RefCell::new(None) };
}

/// Re-raise a panic that a handler caught at the FFI boundary. The testkit
/// calls this after every LVGL call that can run handlers.
#[cfg(test)]
pub(crate) fn resume_handler_panic() {
    if let Some(payload) = PANIC.with(|slot| slot.borrow_mut().take()) {
        std::panic::resume_unwind(payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::Rect;
    use std::cell::Cell;
    use std::rc::Rc;

    struct DropCounter(Rc<Cell<u32>>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 100);
        }
    }

    #[test]
    fn clicks_run_the_handler_and_deleting_frees_it() {
        lvgl(|lv| {
            let root = lv.screen();
            let counter = Rc::new(Cell::new(0));
            let button = Obj::plain(root)
                .rect(Rect::new(150, 120, 100, 60))
                .clickable(true);
            let guard = DropCounter(counter.clone());
            let clicks = counter.clone();
            on_click(button, move || {
                let _owned = &guard;
                clicks.set(clicks.get() + 1);
            });
            lv.click(button);
            lv.click(button);
            assert_eq!(counter.get(), 2);
            button.delete();
            assert_eq!(counter.get(), 102, "deleting the button drops the handler");
        });
    }

    #[test]
    fn handlers_see_the_event_code() {
        lvgl(|lv| {
            let root = lv.screen();
            let seen = Rc::new(Cell::new(0u32));
            let target = Obj::plain(root)
                .rect(Rect::new(150, 120, 100, 60))
                .clickable(true);
            let record = seen.clone();
            on(
                target,
                crate::bindings::lv_event_code_t_LV_EVENT_PRESSED,
                move |event| {
                    record.set(event.code());
                },
            );
            lv.send(target, crate::bindings::lv_event_code_t_LV_EVENT_PRESSED);
            assert_eq!(
                seen.get(),
                crate::bindings::lv_event_code_t_LV_EVENT_PRESSED
            );
        });
    }

    #[test]
    fn a_handler_may_delete_its_own_button() {
        lvgl(|lv| {
            let root = lv.screen();
            let counter = Rc::new(Cell::new(0));
            let button = Obj::plain(root)
                .rect(Rect::new(150, 120, 100, 60))
                .clickable(true);
            let guard = DropCounter(counter.clone());
            let clicks = counter.clone();
            on_click(button, move || {
                let _owned = &guard;
                button.delete();
                // The closure is still alive here; touching its captures is fine.
                clicks.set(clicks.get() + 1);
            });
            lv.click(button);
            assert_eq!(
                counter.get(),
                101,
                "handler ran once, then its state was dropped once"
            );
        });
    }

    #[test]
    fn a_handler_that_resends_its_own_event_does_not_nest() {
        lvgl(|lv| {
            let root = lv.screen();
            let calls = Rc::new(Cell::new(0u32));
            let target = Obj::plain(root)
                .rect(Rect::new(150, 120, 100, 60))
                .clickable(true);
            let seen = calls.clone();
            on(
                target,
                crate::bindings::lv_event_code_t_LV_EVENT_PRESSED,
                move |event| {
                    seen.set(seen.get() + 1);
                    // SAFETY: the target is live for the duration of the callback.
                    unsafe {
                        crate::bindings::lv_obj_send_event(
                            event.target().raw(),
                            crate::bindings::lv_event_code_t_LV_EVENT_PRESSED,
                            std::ptr::null_mut(),
                        );
                    }
                },
            );
            lv.send(target, crate::bindings::lv_event_code_t_LV_EVENT_PRESSED);
            assert_eq!(calls.get(), 1);
            lv.send(target, crate::bindings::lv_event_code_t_LV_EVENT_PRESSED);
            assert_eq!(
                calls.get(),
                2,
                "the handler is back in its slot after a nested send"
            );
        });
    }

    #[test]
    fn a_swipe_travels_sixty_px_along_one_axis_within_700_ms() {
        assert_eq!(Swipe::recognise(60, 0, 100), Some(Swipe::Right));
        assert_eq!(Swipe::recognise(-60, 0, 100), Some(Swipe::Left));
        assert_eq!(Swipe::recognise(0, 60, 100), Some(Swipe::Down));
        assert_eq!(Swipe::recognise(0, -60, 100), Some(Swipe::Up));
        assert_eq!(Swipe::recognise(59, 0, 100), None, "too short");
        assert_eq!(Swipe::recognise(0, -59, 100), None, "too short");
        assert_eq!(
            Swipe::recognise(80, 40, 100),
            Some(Swipe::Right),
            "half as far across"
        );
        assert_eq!(Swipe::recognise(80, 41, 100), None, "too diagonal");
        assert_eq!(Swipe::recognise(-41, -80, 100), None, "too diagonal");
        assert_eq!(Swipe::recognise(80, 0, SWIPE_MAX_MS), Some(Swipe::Right));
        assert_eq!(
            Swipe::recognise(80, 0, SWIPE_MAX_MS + 1),
            None,
            "a slow drag"
        );
    }

    #[test]
    fn the_tracker_reports_one_swipe_per_press() {
        let mut t = SwipeTracker::default();
        assert_eq!(t.sample(300, 150, true, 1_000), None);
        assert_eq!(t.start(), Some((300, 150)));
        assert_eq!(
            t.sample(260, 152, true, 1_080),
            None,
            "40 px is not yet a swipe"
        );
        assert_eq!(t.sample(235, 156, true, 1_160), Some(Swipe::Left));
        assert_eq!(t.sample(150, 156, true, 1_240), None, "once per press");
        assert_eq!(t.sample(150, 156, false, 1_300), None);
        assert_eq!(t.start(), None);
        // The next press starts afresh; one that moves too slowly never swipes.
        assert_eq!(t.sample(100, 150, true, 2_000), None);
        assert_eq!(t.sample(130, 150, true, 2_500), None);
        assert_eq!(
            t.sample(190, 150, true, 2_800),
            None,
            "800 ms after the press"
        );
        assert_eq!(t.sample(100, 150, false, 2_900), None);
        // Down, then up.
        t.sample(200, 12, true, 3_000);
        assert_eq!(t.sample(205, 80, true, 3_150), Some(Swipe::Down));
        t.sample(0, 0, false, 3_200);
        t.sample(200, 200, true, 4_000);
        assert_eq!(t.sample(190, 120, true, 4_150), Some(Swipe::Up));
    }

    #[test]
    fn swipe_handlers_hear_only_their_direction() {
        lvgl(|lv| {
            let root = lv.screen();
            let (left, right) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
            let (l, r) = (left.clone(), right.clone());
            on_swipe(root, Swipe::Left, move || l.set(l.get() + 1));
            on_swipe(root, Swipe::Right, move || r.set(r.get() + 1));
            lv.swipe(root, Swipe::Left);
            assert_eq!((left.get(), right.get()), (1, 0));
            lv.swipe(root, Swipe::Right);
            assert_eq!((left.get(), right.get()), (1, 1));
            assert!(!send_swipe(root, Swipe::Up), "nobody listens for up");
            assert!(send_swipe(root, Swipe::Left));
            assert_eq!(left.get(), 2);
        });
    }

    #[test]
    fn a_swipe_on_a_child_reaches_the_screen_unless_a_control_keeps_it() {
        lvgl(|lv| {
            let root = lv.screen();
            let swipes = Rc::new(Cell::new(0));
            let count = swipes.clone();
            on_swipe(root, Swipe::Left, move || count.set(count.get() + 1));
            let card = Obj::plain(root).rect(Rect::new(60, 60, 280, 120));
            let button = Obj::plain(card)
                .rect(Rect::new(10, 10, 100, 60))
                .clickable(true);
            let knob = Obj::plain(card)
                .rect(Rect::new(150, 10, 100, 60))
                .clickable(true)
                .keep_gestures();
            assert!(
                send_swipe(button, Swipe::Left),
                "a plain button passes it up"
            );
            assert_eq!(swipes.get(), 1);
            assert!(!send_swipe(knob, Swipe::Left), "the knob keeps its drags");
            assert_eq!(swipes.get(), 1);
            // A control that keeps its drags can still take a swipe itself.
            let kept = Rc::new(Cell::new(0));
            let mine = kept.clone();
            on_swipe(knob, Swipe::Left, move || mine.set(mine.get() + 1));
            lv.swipe(knob, Swipe::Left);
            assert_eq!((swipes.get(), kept.get()), (1, 1));
        });
    }

    #[test]
    fn a_swipe_handler_may_delete_its_own_page() {
        lvgl(|lv| {
            let root = lv.screen();
            let page = Obj::plain(root)
                .rect(Rect::new(0, 0, 400, 304))
                .keep_gestures();
            let counter = Rc::new(Cell::new(0));
            let guard = DropCounter(counter.clone());
            let swipes = counter.clone();
            on_swipe(page, Swipe::Left, move || {
                let _owned = &guard;
                swipes.set(swipes.get() + 1);
                page.delete();
            });
            lv.swipe(page, Swipe::Left);
            assert_eq!(counter.get(), 101, "ran once, then its state was dropped");
        });
    }

    #[test]
    #[should_panic(expected = "handler blew up")]
    fn a_panicking_handler_surfaces_as_a_test_panic() {
        lvgl(|lv| {
            let root = lv.screen();
            let button = Obj::plain(root)
                .rect(Rect::new(150, 120, 100, 60))
                .clickable(true);
            on_click(button, || panic!("handler blew up"));
            lv.click(button);
        });
    }
}
