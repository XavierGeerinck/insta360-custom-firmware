pub mod bindings;
pub mod display;
pub mod i18n;
pub mod input;
pub mod kit;
pub mod runtime;
pub mod screens_v2;

use std::sync::Arc;
use tracing::error;
use waved_config::WavedConfig;
use waved_state::StateBus;

pub struct UiService {
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl UiService {
    /// Starts the `ui-lvgl` thread, which runs the revision-2 runtime
    /// (`runtime::run`) for the life of the process.
    pub fn new(bus: Arc<StateBus>, config: WavedConfig) -> Self {
        let handle = std::thread::Builder::new()
            .name("ui-lvgl".into())
            .spawn(move || {
                if let Err(e) = runtime::run::run(bus, config) {
                    error!("UI thread error: {e}");
                }
            })
            .expect("failed to spawn ui-lvgl thread");
        Self {
            thread_handle: Some(handle),
        }
    }

    pub fn join(mut self) {
        if let Some(h) = self.thread_handle.take() {
            let _ = h.join();
        }
    }
}
