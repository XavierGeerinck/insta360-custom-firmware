//! Start-up (A5, docs/mockups/png/boot.png): the ring draws itself, then the
//! screen hands over to First start or Home with `UiEvent::BootDone`. The
//! only animation that plays without a touch.

use crate::kit::components::dial::Dial;
use crate::kit::dispatch::{Dispatcher, UiEvent};
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{DIAL_CX, DIAL_CY, DIAL_R, SCREEN_W, TEXT, TEXT_3};

/// How long the ring takes to draw before start-up is done.
pub const RING_MS: u64 = 1_200;
/// boot.png: "Starting" (title, CSS top 99) and the version (micro, top 129).
const STARTING_Y: i32 = 99;
const VERSION_Y: i32 = 129;

pub struct BootScreen {
    version: String,
    parts: Option<Parts>,
}

struct Parts {
    dial: Dial,
    dispatch: Dispatcher,
    started_ms: u64,
    shown_permille: Option<u64>,
    done: bool,
}

impl Default for BootScreen {
    fn default() -> Self {
        Self::with_version(env!("CARGO_PKG_VERSION"))
    }
}

impl BootScreen {
    /// A start-up screen showing `version` (tests pin it so goldens stay put).
    pub fn with_version(version: &str) -> Self {
        Self {
            version: version.into(),
            parts: None,
        }
    }
}

impl Screen for BootScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        let dial = Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3);
        Obj::label(root, "Starting", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, STARTING_Y);
        Obj::label(root, &format!("waved {}", self.version), Font::Micro, TEXT_3)
            .centered_text(SCREEN_W)
            .pos(0, VERSION_Y);
        self.parts = Some(Parts {
            dial,
            dispatch: ctx.dispatch.clone(),
            started_ms: snap.now_ms,
            shown_permille: None,
            done: false,
        });
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        let elapsed = snap.now_ms.saturating_sub(p.started_ms);
        let permille = elapsed.min(RING_MS) * 1_000 / RING_MS;
        if p.shown_permille != Some(permille) {
            p.dial.set_progress(Some(permille as f32 / 1_000.0));
            p.shown_permille = Some(permille);
        }
        if elapsed >= RING_MS && !p.done {
            p.done = true;
            p.dispatch.ui(UiEvent::BootDone);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::main_parts::fixtures::{ctx, events, snap};

    #[test]
    fn the_ring_draws_itself_then_boot_is_done_once() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, ui) = ctx();
            let mut now = snap();
            let mut boot = BootScreen::with_version("0.9.3");
            boot.build(root, &ctx, &now);
            now.now_ms += RING_MS - 1;
            boot.update(&now);
            assert!(events(&ui).is_empty(), "not before the ring is full");
            now.now_ms += 1;
            boot.update(&now);
            now.now_ms += 500;
            boot.update(&now);
            assert_eq!(events(&ui), [UiEvent::BootDone]);
        });
    }

    #[test]
    fn the_default_shows_this_build_s_version() {
        assert_eq!(BootScreen::default().version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn boot_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut now = snap();
            now.now_ms = 0;
            let mut boot = BootScreen::with_version("0.9.3");
            boot.build(root, &ctx, &now);
            now.now_ms = 733; // 61 %: the mockup's 220° arc
            boot.update(&now);
            assert_golden("boot", &lv.render());
            assert_targets_ok(root);
        });
    }
}
