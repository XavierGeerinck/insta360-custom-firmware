//! Files' recording cards and the list that scrolls them (docs/mockups/
//! files.png): 296 x 52 cards every 58 px in y 80-222, fading out at both
//! edges, with an empty state when there are no recordings.

use crate::bindings::*;
use crate::kit::components::list::fade;
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{Obj, SIZE_CONTENT};
use crate::kit::tokens::{Rect, RAISED, SCREEN_W, SURFACE, TEXT, TEXT_2, TEXT_3};
use std::cell::RefCell;
use std::rc::Rc;
use uuid::Uuid;

const MAIN: lv_style_selector_t = LV_PART_MAIN as lv_style_selector_t;

/// What one row of the Files list shows. `screens_v2::files` builds these from
/// the snapshot; the row component draws them.
#[derive(Clone, Debug, PartialEq)]
pub struct RowLook {
    pub id: Uuid,
    /// Upload-state glyph from `kit::icons` and its colour.
    pub icon: &'static str,
    pub icon_color: u32,
    /// "Today, 14:02".
    pub title: String,
    /// "42 min, upload failed".
    pub line: String,
}

/// The list viewport, between the free-space line and Back.
pub const VIEW: Rect = Rect::new(0, 80, 400, 142);
pub const CARD_W: i32 = 296;
pub const CARD_H: i32 = 52;
/// A card starts every 58 px: 52 plus a 6 px gap.
pub const CARD_PITCH: i32 = 58;
const CARD_RADIUS: i32 = 16;
const CARD_PAD_X: i32 = 14;
const CARD_GAP: i32 = 12;
/// The text column: 296 - 2 x 14 padding - 20 icon - 20 chevron - 2 x 12 gaps.
const TEXT_W: i32 = 204;
/// The first card sits 2 px into the viewport (y 82), as in files.png...
const PAD_TOP: i32 = 2;
/// ...and the last one scrolls clear of the bottom fade.
const PAD_BOTTOM: i32 = 16;
const FADE_TOP: i32 = 14;
const FADE_BOTTOM: i32 = 16;
pub const EMPTY_TEXT: &str = "No recordings yet. Tap Record on the home screen to start one.";
/// Inside `VIEW`: the folder icon at y 90, the text at y 142.
const EMPTY_ICON_Y: i32 = 10;
const EMPTY_TEXT_Y: i32 = 62;
const EMPTY_TEXT_W: i32 = 240;

fn card(parent: Obj, look: &RowLook) -> Obj {
    let card = Obj::plain(parent)
        .size(CARD_W, CARD_H)
        .bg(SURFACE)
        .radius(CARD_RADIUS)
        .pressed_bg(RAISED)
        .clickable(true)
        .row_start(CARD_GAP, CARD_PAD_X);
    Obj::label(card, look.icon, Font::Icon20, look.icon_color);
    let text = Obj::plain(card).size(TEXT_W, SIZE_CONTENT).column(2);
    Obj::label(text, &look.title, Font::Body, TEXT)
        .width(TEXT_W)
        .one_line();
    Obj::label(text, &look.line, Font::Small, TEXT_2)
        .width(TEXT_W)
        .one_line();
    Obj::label(card, icons::RIGHT, Font::Icon20, TEXT_3);
    card
}

pub struct FileList {
    viewport: Obj,
    empty: Obj,
    cards: Vec<Obj>,
    rows: Option<Vec<RowLook>>,
    on_open: Rc<RefCell<dyn FnMut(Uuid)>>,
}

impl FileList {
    pub fn new(parent: Obj, on_open: impl FnMut(Uuid) + 'static) -> Self {
        let viewport = Obj::plain(parent)
            .rect(VIEW)
            .column_centered(CARD_PITCH - CARD_H);
        // SAFETY: live object.
        unsafe {
            lv_obj_add_flag(viewport.raw(), lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            lv_obj_set_scroll_dir(viewport.raw(), lv_dir_t_LV_DIR_VER);
            lv_obj_set_scrollbar_mode(viewport.raw(), lv_scrollbar_mode_t_LV_SCROLLBAR_MODE_OFF);
            lv_obj_set_style_pad_top(viewport.raw(), PAD_TOP, MAIN);
            lv_obj_set_style_pad_bottom(viewport.raw(), PAD_BOTTOM, MAIN);
        }
        fade(parent, Rect::new(0, VIEW.y, VIEW.w, FADE_TOP), true);
        fade(
            parent,
            Rect::new(0, VIEW.y + VIEW.h - FADE_BOTTOM, VIEW.w, FADE_BOTTOM),
            false,
        );
        let empty = Obj::plain(parent).rect(VIEW).hidden(true);
        Obj::label(empty, icons::FOLDER, Font::Icon44, TEXT_3)
            .centered_text(SCREEN_W)
            .pos(0, EMPTY_ICON_Y);
        Obj::label(empty, EMPTY_TEXT, Font::Small, TEXT_2)
            .wrapped_text(EMPTY_TEXT_W)
            .max_lines(3)
            .pos((SCREEN_W - EMPTY_TEXT_W) / 2, EMPTY_TEXT_Y);
        Self {
            viewport,
            empty,
            cards: Vec::new(),
            rows: None,
            on_open: Rc::new(RefCell::new(on_open)),
        }
    }

    /// Show `rows`. Unchanged rows keep their cards; a change rebuilds them
    /// and keeps the scroll position (clamped to the new content).
    ///
    /// While a card is pressed the rebuild waits: LVGL would hand the finger
    /// to whatever new card is under it, and lifting it would open another
    /// recording. `self.rows` still holds the rows on screen, so the next
    /// call after the finger lifts rebuilds.
    pub fn set_rows(&mut self, rows: Vec<RowLook>) {
        if self.rows.as_ref() == Some(&rows) {
            return;
        }
        if self
            .cards
            .iter()
            .any(|card| card.has_state(LV_STATE_PRESSED))
        {
            return;
        }
        let scroll = self.scroll_y();
        for card in self.cards.drain(..) {
            card.delete();
        }
        for look in &rows {
            let card = card(self.viewport, look);
            let (open, id) = (self.on_open.clone(), look.id);
            on_click(card, move || (open.borrow_mut())(id));
            self.cards.push(card);
        }
        self.empty.hidden(!rows.is_empty());
        self.viewport.update_layout();
        self.scroll_to(scroll);
        self.rows = Some(rows);
    }

    pub fn card(&self, index: usize) -> Obj {
        self.cards[index]
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// Scroll so content offset `y` is at the top (LVGL bounds it).
    pub fn scroll_to(&self, y: i32) {
        self.viewport.update_layout();
        // SAFETY: live viewport.
        unsafe { lv_obj_scroll_to_y(self.viewport.raw(), y, lv_anim_enable_t_LV_ANIM_OFF) };
    }

    pub fn scroll_y(&self) -> i32 {
        // SAFETY: live viewport.
        unsafe { lv_obj_get_scroll_y(self.viewport.raw()) }
    }

    pub fn empty_state(&self) -> Obj {
        self.empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::kit::tokens::{LIVE, TEXT_2, WARN};
    use std::cell::Cell;

    fn look(n: u128, title: &str) -> RowLook {
        RowLook {
            id: Uuid::from_u128(n),
            icon: icons::CLOCK,
            icon_color: TEXT_2,
            title: title.into(),
            line: "18 min, waiting for Wi-Fi".into(),
        }
    }

    fn three() -> Vec<RowLook> {
        vec![
            RowLook {
                icon: icons::ALERT,
                icon_color: WARN,
                line: "42 min, upload failed".into(),
                ..look(1, "Today, 14:02")
            },
            look(2, "Today, 10:30"),
            RowLook {
                icon: icons::CHECK_CIRCLE,
                icon_color: LIVE,
                line: "1 h 05 min, sent".into(),
                ..look(3, "Yesterday, 16:45")
            },
        ]
    }

    #[test]
    fn cards_follow_the_mockup_grid() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut list = FileList::new(root, |_| {});
            list.set_rows(three());
            root.update_layout();
            for (index, top) in [82, 140, 198].into_iter().enumerate() {
                let a = list.card(index).coords();
                assert_eq!((a.x1, a.y1), (52, top), "card {index}");
                assert_eq!((a.x2 - a.x1 + 1, a.y2 - a.y1 + 1), (CARD_W, CARD_H));
            }
            assert!(list.empty_state().is_hidden());
            assert_golden("file_list_cards", &lv.render());
        });
    }

    #[test]
    fn long_titles_stay_inside_the_card() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut list = FileList::new(root, |_| {});
            list.set_rows(vec![look(
                1,
                "A title far longer than any date label could ever be",
            )]);
            root.update_layout();
            let card = list.card(0);
            let bounds = card.coords();
            let text = card.child(1);
            let title = text.child(0);
            let a = title.coords();
            assert!(
                a.x2 <= bounds.x2 && a.y2 <= bounds.y2,
                "title overflows the card"
            );
            assert!(title.text().ends_with("..."), "{:?}", title.text());
        });
    }

    #[test]
    fn tapping_a_card_opens_its_recording() {
        lvgl(|lv| {
            let root = lv.screen();
            let opened = Rc::new(Cell::new(None));
            let seen = opened.clone();
            let mut list = FileList::new(root, move |id| seen.set(Some(id)));
            list.set_rows(three());
            lv.click(list.card(1));
            assert_eq!(opened.get(), Some(Uuid::from_u128(2)));
        });
    }

    #[test]
    fn unchanged_rows_keep_their_cards() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut list = FileList::new(root, |_| {});
            list.set_rows(three());
            let first = list.card(0);
            list.set_rows(three());
            assert_eq!(list.card(0), first, "same rows: no rebuild");
            let mut changed = three();
            changed[0].line = "42 min, sending".into();
            list.set_rows(changed);
            assert_eq!(list.len(), 3);
            assert_eq!(list.card(0).child(1).child(1).text(), "42 min, sending");
        });
    }

    #[test]
    fn a_pressed_card_is_not_rebuilt_under_the_finger() {
        lvgl(|lv| {
            let root = lv.screen();
            let opened = Rc::new(Cell::new(None));
            let seen = opened.clone();
            let mut list = FileList::new(root, move |id| seen.set(Some(id)));
            list.set_rows(three());
            let pressed = list.card(1);
            pressed.set_state(LV_STATE_PRESSED, true);
            // An upload fails meanwhile and the order changes.
            let mut reordered = three();
            reordered.swap(0, 1);
            list.set_rows(reordered.clone());
            assert_eq!(list.card(0).child(1).child(0).text(), "Today, 14:02");
            assert_eq!(list.card(1), pressed, "the finger keeps its card");
            lv.click(pressed);
            assert_eq!(opened.get(), Some(Uuid::from_u128(2)));
            // Once the finger lifts, the next update shows the new order.
            pressed.set_state(LV_STATE_PRESSED, false);
            list.set_rows(reordered);
            assert_eq!(list.card(0).child(1).child(0).text(), "Today, 10:30");
            assert_eq!(list.card(1).child(1).child(0).text(), "Today, 14:02");
        });
    }

    #[test]
    fn rebuilding_keeps_the_scroll_position() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut list = FileList::new(root, |_| {});
            let rows: Vec<RowLook> = (1..=8).map(|n| look(n, "Today, 10:30")).collect();
            list.set_rows(rows.clone());
            list.scroll_to(100);
            assert_eq!(list.scroll_y(), 100);
            let mut changed = rows;
            changed[7].title = "Today, 10:31".into();
            list.set_rows(changed);
            assert_eq!(list.scroll_y(), 100);
        });
    }

    #[test]
    fn no_rows_shows_the_empty_state() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut list = FileList::new(root, |_| {});
            list.set_rows(three());
            list.set_rows(Vec::new());
            assert!(list.is_empty());
            assert!(!list.empty_state().is_hidden());
            assert_eq!(list.empty_state().child(1).text(), EMPTY_TEXT);
        });
    }

    #[test]
    fn visible_cards_stay_inside_the_tap_limit() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut list = FileList::new(root, |_| {});
            list.set_rows((1..=8).map(|n| look(n, "Today, 10:30")).collect());
            for y in [0, 29, 58, 200, 10_000] {
                list.scroll_to(y);
                assert_targets_ok(root);
            }
        });
    }
}
