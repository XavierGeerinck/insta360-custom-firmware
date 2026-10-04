//! The contract every new screen implements.

use crate::kit::dispatch::Dispatcher;
use crate::kit::obj::Obj;
use crate::kit::snapshot::UiSnapshot;
use std::sync::Arc;
use waved_config::WavedConfig;

/// What every screen receives when it is built.
#[derive(Clone)]
pub struct ScreenCtx {
    pub dispatch: Dispatcher,
    pub config: Arc<WavedConfig>,
}

/// A full-screen view. The runtime builds a fresh instance for each navigation
/// and deletes `root`, with every handler, when navigating away.
pub trait Screen {
    /// Build widgets into `root`, a black 400×304 screen owned by the runtime.
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot);
    /// Refresh from the latest snapshot; called on every UI loop iteration.
    fn update(&mut self, snap: &UiSnapshot);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fonts::Font;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::TEXT;

    struct Speaker {
        label: Option<Obj>,
    }

    impl Screen for Speaker {
        fn build(&mut self, root: Obj, _ctx: &ScreenCtx, snap: &UiSnapshot) {
            self.label = Some(Obj::label(root, "", Font::Body, TEXT).pos(100, 100));
            self.update(snap);
        }

        fn update(&mut self, snap: &UiSnapshot) {
            if let Some(label) = self.label {
                label.set_text(&format!("Volume {}%", snap.audio.speaker_volume));
            }
        }
    }

    #[test]
    fn screens_build_into_a_root_and_follow_snapshots() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let ctx = ScreenCtx {
                dispatch,
                config: Arc::new(WavedConfig::default()),
            };
            let mut snap = UiSnapshot::default();
            snap.audio.speaker_volume = 60;
            let mut screen = Speaker { label: None };
            screen.build(root, &ctx, &snap);
            assert_eq!(screen.label.unwrap().text(), "Volume 60%");
            snap.audio.speaker_volume = 70;
            screen.update(&snap);
            assert_eq!(screen.label.unwrap().text(), "Volume 70%");
        });
    }
}
