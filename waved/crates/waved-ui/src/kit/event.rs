//! Event handlers whose captured state is freed when LVGL deletes the object.
//! Replaces the legacy `Box::into_raw` user data that leaked on every navigation.

use crate::bindings::*;
use crate::kit::obj::Obj;

type Handler = Box<dyn FnMut(&Event)>;

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
        // SAFETY: valid for the duration of the callback.
        unsafe { Obj::from_raw(lv_event_get_target(self.raw).cast()) }
    }
}

/// Run `handler` whenever `obj` emits `code`. The closure is dropped when
/// `obj` is deleted, so captured state never leaks.
pub fn on(obj: Obj, code: lv_event_code_t, handler: impl FnMut(&Event) + 'static) {
    let boxed: *mut Handler = Box::into_raw(Box::new(Box::new(handler)));
    // SAFETY: `boxed` stays valid until `release` runs on LV_EVENT_DELETE.
    unsafe {
        lv_obj_add_event_cb(obj.raw(), Some(dispatch), code, boxed.cast());
        lv_obj_add_event_cb(obj.raw(), Some(release), lv_event_code_t_LV_EVENT_DELETE, boxed.cast());
    }
}

/// Run `handler` on every tap of `obj`.
pub fn on_click(obj: Obj, mut handler: impl FnMut() + 'static) {
    on(obj, lv_event_code_t_LV_EVENT_CLICKED, move |_| handler());
}

unsafe extern "C" fn dispatch(e: *mut lv_event_t) {
    let handler = lv_event_get_user_data(e) as *mut Handler;
    if !handler.is_null() {
        (*handler)(&Event { raw: e });
    }
}

unsafe extern "C" fn release(e: *mut lv_event_t) {
    let handler = lv_event_get_user_data(e) as *mut Handler;
    if !handler.is_null() {
        drop(Box::from_raw(handler));
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
}
