pub mod audio_mode;
pub mod home;
pub mod noise_reduction;
pub mod recording;
pub mod settings;
pub mod system;
pub mod volume;

use crate::bindings::lv_obj_t;

/// Every screen implements this trait.
/// `create` builds the LVGL widget tree and returns the root object.
/// `destroy` is called before navigating away — must call `lv_obj_delete(root)`.
pub trait Screen {
    fn create(&mut self) -> *mut lv_obj_t;
    fn destroy(&mut self, root: *mut lv_obj_t) {
        unsafe { crate::bindings::lv_obj_delete(root); }
    }
    /// Called each LVGL tick to refresh state-driven labels/values.
    fn update(&mut self, _root: *mut lv_obj_t) {}
}
