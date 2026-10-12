//! PIN (K2, D10, pin.png): the keypad in front of Files, Wi-Fi, Bluetooth,
//! Uploads and System while a PIN is set. The screen only collects digits:
//! the last one sends them to the runtime (`UiEvent::PinEntered`), which
//! checks them (`runtime::pin`) and opens the guarded screen. Wrong tries and
//! the one-minute wait come back in `snap.pin`. A modal screen, no status.

use crate::kit::components::keypad::{Key, Keypad, PinDots};
use crate::kit::dispatch::{Dispatcher, PinCode, UiEvent};
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{SCREEN_W, TEXT, WARN};
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use waved_config::{pin_digits, PIN_MIN_DIGITS};

/// Top of the title (pin.png): there is no status row above it.
const PIN_TITLE_Y: i32 = 6;

pub const ENTER: &str = "Enter PIN";
pub const WRONG: &str = "Wrong PIN";
/// After five wrong tries (D10), for the minute the keypad waits.
pub const LOCKED: &str = "Wrong PIN. Try again in 1 min.";

/// What is on screen, so a frame without changes redraws nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Look {
    title: &'static str,
    filled: usize,
    enabled: bool,
}

struct State {
    dispatch: Dispatcher,
    /// The saved PIN's length (4 when none is saved, which the router never shows).
    digits: usize,
    entered: String,
    /// The digits went to the runtime; more wait for its answer.
    sent: bool,
    /// `snap.pin.wrong` as last seen: it growing means the last try was wrong.
    seen_wrong: u32,
    wrong: bool,
    locked: bool,
    title: Obj,
    dots: PinDots,
    keypad: Option<Keypad>,
    painted: Option<Look>,
}

impl State {
    fn on_key(&mut self, key: Key) {
        match key {
            Key::Cancel => {
                self.dispatch.back();
                return;
            }
            Key::Backspace => {
                self.entered.pop();
                self.sent = false;
            }
            Key::Digit(digit) => {
                if self.locked || self.sent || self.entered.len() >= self.digits {
                    return;
                }
                self.entered.push(char::from(b'0' + digit));
                self.wrong = false;
                if self.entered.len() == self.digits {
                    self.sent = true;
                    let code = PinCode::new(self.entered.clone());
                    self.dispatch.ui(UiEvent::PinEntered(code));
                }
            }
        }
        self.paint();
    }

    fn follow(&mut self, snap: &UiSnapshot) {
        if snap.pin.wrong != self.seen_wrong {
            self.seen_wrong = snap.pin.wrong;
            self.clear();
            self.wrong = true;
        }
        let locked = snap.pin.locked(snap.now_ms);
        if locked != self.locked {
            self.locked = locked;
            self.clear();
            self.wrong &= locked;
        }
        self.paint();
    }

    fn clear(&mut self) {
        self.entered.clear();
        self.sent = false;
    }

    fn look(&self) -> Look {
        let title = if self.locked {
            LOCKED
        } else if self.wrong {
            WRONG
        } else {
            ENTER
        };
        Look {
            title,
            filled: self.entered.len(),
            enabled: !self.locked,
        }
    }

    fn paint(&mut self) {
        let look = self.look();
        if self.painted == Some(look) {
            return;
        }
        self.title.set_text(look.title);
        self.title
            .text_color(if look.title == ENTER { TEXT } else { WARN });
        self.dots.set_filled(look.filled);
        if let Some(keypad) = &self.keypad {
            keypad.set_enabled(look.enabled);
        }
        self.painted = Some(look);
    }
}

#[derive(Default)]
pub struct PinScreen {
    state: Option<Rc<RefCell<State>>>,
}

impl Screen for PinScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        let digits = pin_digits(&ctx.config.security).unwrap_or(PIN_MIN_DIGITS);
        let title = Obj::label(root, ENTER, Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, PIN_TITLE_Y);
        let state = Rc::new(RefCell::new(State {
            dispatch: ctx.dispatch.clone(),
            digits,
            entered: String::new(),
            sent: false,
            seen_wrong: snap.pin.wrong,
            wrong: false,
            locked: false,
            title,
            dots: PinDots::new(root, digits),
            keypad: None,
            painted: None,
        }));
        let weak: Weak<RefCell<State>> = Rc::downgrade(&state);
        let keypad = Keypad::new(root, move |key| {
            if let Some(state) = weak.upgrade() {
                state.borrow_mut().on_key(key);
            }
        });
        state.borrow_mut().keypad = Some(keypad);
        state.borrow_mut().follow(snap);
        self.state = Some(state);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(state) = &self.state {
            state.borrow_mut().follow(snap);
        }
    }
}

#[cfg(test)]
impl PinScreen {
    fn state(&self) -> std::cell::Ref<'_, State> {
        self.state.as_ref().expect("built").borrow()
    }

    fn key(&self, key: Key) -> Obj {
        self.state().keypad.as_ref().expect("built").key(key)
    }

    /// The title as drawn (after layout: before it, a one-line label holds "...").
    fn title(&self) -> String {
        let title = self.state().title;
        title.update_layout();
        title.text()
    }

    /// Filled dots as drawn.
    fn filled(&self) -> usize {
        self.state().painted.map_or(0, |look| look.filled)
    }

    fn dot_count(&self) -> usize {
        self.state().dots.count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::components::keypad::Key;
    use crate::kit::dispatch::{PinCode, UiEvent};
    use crate::kit::fonts::Font;
    use crate::kit::obj::text_width;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl, Lvgl};
    use crate::screens_v2::menu::fixture::{fixture_with, Fixture};
    use waved_config::{hash_pin, WavedConfig};

    fn config(pin: &str) -> WavedConfig {
        let mut config = WavedConfig::default();
        config.security.device_password = hash_pin(pin);
        config
    }

    fn at(now_ms: u64) -> UiSnapshot {
        UiSnapshot {
            now_ms,
            ..UiSnapshot::default()
        }
    }

    fn build(lv: &mut Lvgl, pin: &str, snap: &UiSnapshot) -> (Fixture, PinScreen, Obj) {
        let root = lv.screen();
        let f = fixture_with(config(pin));
        let mut screen = PinScreen::default();
        screen.build(root, &f.ctx, snap);
        (f, screen, root)
    }

    fn tap(lv: &mut Lvgl, screen: &PinScreen, keys: &str) {
        for ch in keys.chars() {
            let key = match ch {
                '<' => Key::Backspace,
                'x' => Key::Cancel,
                digit => Key::Digit(digit.to_digit(10).unwrap() as u8),
            };
            lv.click(screen.key(key));
        }
    }

    fn entered(code: &str) -> UiEvent {
        UiEvent::PinEntered(PinCode::new(code))
    }

    #[test]
    fn pin_matches_the_mockup() {
        lvgl(|lv| {
            let (_f, screen, root) = build(lv, "4821", &at(0));
            tap(lv, &screen, "48");
            assert_eq!(screen.filled(), 2);
            assert_golden("screen_pin", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_last_digit_sends_the_pin_once() {
        lvgl(|lv| {
            let (f, screen, _root) = build(lv, "4821", &at(0));
            tap(lv, &screen, "482");
            assert!(f.events().is_empty());
            tap(lv, &screen, "19");
            assert_eq!(f.events(), [entered("4821")], "nothing past the last dot");
            assert_eq!(screen.filled(), 4, "the dots stay full while it checks");
        });
    }

    #[test]
    fn backspace_takes_back_a_digit() {
        lvgl(|lv| {
            let (f, screen, _root) = build(lv, "4821", &at(0));
            tap(lv, &screen, "<48<2<<");
            assert_eq!(screen.filled(), 0);
            tap(lv, &screen, "4221");
            assert_eq!(f.events(), [entered("4221")]);
        });
    }

    #[test]
    fn cancel_goes_back() {
        lvgl(|lv| {
            let (f, screen, _root) = build(lv, "4821", &at(0));
            tap(lv, &screen, "48x");
            assert_eq!(f.events(), [UiEvent::Back]);
        });
    }

    #[test]
    fn a_wrong_try_clears_the_dots_and_says_so() {
        lvgl(|lv| {
            let (f, mut screen, root) = build(lv, "4821", &at(0));
            tap(lv, &screen, "1111");
            assert_eq!(f.events(), [entered("1111")]);
            let mut snap = at(100);
            snap.pin.wrong = 1;
            screen.update(&snap);
            assert_eq!((screen.title(), screen.filled()), (WRONG.to_string(), 0));
            assert_golden("screen_pin_wrong", &lv.render());
            assert_targets_ok(root);
            screen.update(&snap);
            assert_eq!(screen.title(), WRONG, "until the next digit");
            tap(lv, &screen, "4");
            assert_eq!((screen.title(), screen.filled()), (ENTER.to_string(), 1));
        });
    }

    #[test]
    fn a_screen_opened_after_wrong_tries_starts_plain() {
        lvgl(|lv| {
            let mut snap = at(0);
            snap.pin.wrong = 3;
            let (_f, mut screen, _root) = build(lv, "4821", &snap);
            screen.update(&snap);
            assert_eq!(screen.title(), ENTER);
        });
    }

    #[test]
    fn the_keypad_waits_out_the_lockout() {
        lvgl(|lv| {
            let mut snap = at(1_000);
            snap.pin = crate::kit::snapshot::PinStatus {
                wrong: 5,
                locked_until_ms: Some(61_000),
            };
            let (f, mut screen, root) = build(lv, "4821", &snap);
            screen.update(&snap);
            assert_eq!(screen.title(), LOCKED);
            assert_golden("screen_pin_locked", &lv.render());
            assert_targets_ok(root);
            tap(lv, &screen, "4821");
            assert!(f.events().is_empty(), "digits wait");
            assert_eq!(screen.filled(), 0);

            snap.now_ms = 61_000;
            screen.update(&snap);
            assert_eq!(screen.title(), ENTER);
            tap(lv, &screen, "4821");
            assert_eq!(f.events(), [entered("4821")]);
        });
    }

    #[test]
    fn the_lockout_starting_clears_what_was_typed() {
        lvgl(|lv| {
            let (_f, mut screen, _root) = build(lv, "4821", &at(0));
            tap(lv, &screen, "1111");
            let mut snap = at(100);
            snap.pin = crate::kit::snapshot::PinStatus {
                wrong: 5,
                locked_until_ms: Some(60_100),
            };
            screen.update(&snap);
            assert_eq!((screen.title(), screen.filled()), (LOCKED.to_string(), 0));
        });
    }

    #[test]
    fn a_six_digit_pin_shows_six_dots() {
        lvgl(|lv| {
            let (f, screen, root) = build(lv, "482100", &at(0));
            assert_eq!(screen.dot_count(), 6);
            tap(lv, &screen, "48210");
            assert!(f.events().is_empty());
            tap(lv, &screen, "0");
            assert_eq!(f.events(), [entered("482100")]);
            assert_targets_ok(root);
        });
    }

    #[test]
    fn without_a_saved_pin_it_still_builds_four_dots() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture_with(WavedConfig::default());
            let mut screen = PinScreen::default();
            screen.build(root, &f.ctx, &UiSnapshot::default());
            assert_eq!(screen.dot_count(), 4);
        });
    }

    #[test]
    fn the_titles_fit_the_flat_top_of_the_panel() {
        // The visible chord at y 6 is 342 px wide; keep a margin.
        for title in [ENTER, WRONG, LOCKED] {
            lvgl(|_| assert!(text_width(title, Font::Title) <= 320, "{title}"));
        }
    }
}
