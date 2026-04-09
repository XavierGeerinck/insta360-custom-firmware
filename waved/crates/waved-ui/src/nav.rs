//! Screen navigation state machine.

#[derive(Clone, Debug, PartialEq)]
pub enum UiScreen {
    Home,
    Recording,
    MicMode,
    Settings,
    Files,
    Wifi,
    WifiInfo,
    Bluetooth,
    Volume,
    Height,
    About,
    PowerOff,
    ControlCenter, // overlay — stacked above current screen
}

impl Default for UiScreen {
    fn default() -> Self { Self::Home }
}

pub struct ScreenManager {
    pub current: UiScreen,
    pub control_center_open: bool,
    current_root: *mut crate::bindings::lv_obj_t,
}

impl ScreenManager {
    pub fn new() -> Self {
        Self {
            current: UiScreen::default(),
            control_center_open: false,
            current_root: std::ptr::null_mut(),
        }
    }

    /// Navigate to a new screen. Destroys the current one first.
    pub fn navigate_to(&mut self, screen: UiScreen) {
        if !self.current_root.is_null() {
            unsafe { crate::bindings::lv_obj_delete(self.current_root); }
            self.current_root = std::ptr::null_mut();
        }
        self.current = screen;
    }

    pub fn set_root(&mut self, root: *mut crate::bindings::lv_obj_t) {
        self.current_root = root;
    }

    pub fn current_root(&self) -> *mut crate::bindings::lv_obj_t {
        self.current_root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_screen_is_home() {
        let nav = ScreenManager::new();
        assert_eq!(nav.current, UiScreen::Home);
        assert!(!nav.control_center_open);
    }

    #[test]
    fn navigate_changes_current() {
        let mut nav = ScreenManager::new();
        nav.navigate_to(UiScreen::Recording);
        assert_eq!(nav.current, UiScreen::Recording);
    }
}
