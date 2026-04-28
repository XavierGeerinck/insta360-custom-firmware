//! Screen navigation state machine.

#[derive(Clone, Debug, PartialEq)]
pub enum UiScreen {
    Home,
    Recording,
    /// "More" page — pspapp page 0xb / `ui_setting_screen`.
    Settings,
    /// Audio mode carousel — pspapp beamforming page.
    AudioMode,
    /// Noise Reduction strength selector (SP#4 — DFN3 Off/Low/Med/High).
    NoiseReduction,
    /// System Settings list — pspapp page 0xe / `ui_setting_system`.
    System,
    /// Speaker volume control — pspapp page 0xd / `ui_volume_screen`.
    Volume,
}

impl Default for UiScreen {
    fn default() -> Self { Self::Home }
}

pub struct ScreenManager {
    pub current: UiScreen,
    current_root: *mut crate::bindings::lv_obj_t,
}

impl ScreenManager {
    pub fn new() -> Self {
        Self {
            current: UiScreen::default(),
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
    }

    #[test]
    fn navigate_changes_current() {
        let mut nav = ScreenManager::new();
        nav.navigate_to(UiScreen::Recording);
        assert_eq!(nav.current, UiScreen::Recording);
    }
}
