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
        lv_obj_add_event_cb(obj.raw(), Some(release), lv_event_code_t_LV_EVENT_DELETE, slot.cast());
    }
}

/// Run `handler` on every tap of `obj`.
pub fn on_click(obj: Obj, mut handler: impl FnMut() + 'static) {
    on(obj, lv_event_code_t_LV_EVENT_CLICKED, move |_| handler());
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
            let button = Obj::plain(root).rect(Rect::new(150, 120, 100, 60)).clickable(true);
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
            let target = Obj::plain(root).rect(Rect::new(150, 120, 100, 60)).clickable(true);
            let record = seen.clone();
            on(target, crate::bindings::lv_event_code_t_LV_EVENT_PRESSED, move |event| {
                record.set(event.code());
            });
            lv.send(target, crate::bindings::lv_event_code_t_LV_EVENT_PRESSED);
            assert_eq!(seen.get(), crate::bindings::lv_event_code_t_LV_EVENT_PRESSED);
        });
    }

    #[test]
    fn a_handler_may_delete_its_own_button() {
        lvgl(|lv| {
            let root = lv.screen();
            let counter = Rc::new(Cell::new(0));
            let button = Obj::plain(root).rect(Rect::new(150, 120, 100, 60)).clickable(true);
            let guard = DropCounter(counter.clone());
            let clicks = counter.clone();
            on_click(button, move || {
                let _owned = &guard;
                button.delete();
                // The closure is still alive here; touching its captures is fine.
                clicks.set(clicks.get() + 1);
            });
            lv.click(button);
            assert_eq!(counter.get(), 101, "handler ran once, then its state was dropped once");
        });
    }

    #[test]
    fn a_handler_that_resends_its_own_event_does_not_nest() {
        lvgl(|lv| {
            let root = lv.screen();
            let calls = Rc::new(Cell::new(0u32));
            let target = Obj::plain(root).rect(Rect::new(150, 120, 100, 60)).clickable(true);
            let seen = calls.clone();
            on(target, crate::bindings::lv_event_code_t_LV_EVENT_PRESSED, move |event| {
                seen.set(seen.get() + 1);
                // SAFETY: the target is live for the duration of the callback.
                unsafe {
                    crate::bindings::lv_obj_send_event(
                        event.target().raw(),
                        crate::bindings::lv_event_code_t_LV_EVENT_PRESSED,
                        std::ptr::null_mut(),
                    );
                }
            });
            lv.send(target, crate::bindings::lv_event_code_t_LV_EVENT_PRESSED);
            assert_eq!(calls.get(), 1);
            lv.send(target, crate::bindings::lv_event_code_t_LV_EVENT_PRESSED);
            assert_eq!(calls.get(), 2, "the handler is back in its slot after a nested send");
        });
    }

    #[test]
    #[should_panic(expected = "handler blew up")]
    fn a_panicking_handler_surfaces_as_a_test_panic() {
        lvgl(|lv| {
            let root = lv.screen();
            let button = Obj::plain(root).rect(Rect::new(150, 120, 100, 60)).clickable(true);
            on_click(button, || panic!("handler blew up"));
            lv.click(button);
        });
    }
}
