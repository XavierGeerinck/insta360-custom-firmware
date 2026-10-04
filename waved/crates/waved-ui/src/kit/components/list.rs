//! The rotary list for round screens (LVGL's lv_example_scroll_6 pattern).

use crate::bindings::*;
use crate::kit::event::{on, on_click};
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{color, text_width, Obj};
use crate::kit::tokens::{lvgl_angle, Rect, BG, LINE, RAISED, REC, SURFACE, TEXT, TEXT_2, TEXT_3};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const MAIN: lv_style_selector_t = LV_PART_MAIN as lv_style_selector_t;
const INDICATOR: lv_style_selector_t = LV_PART_INDICATOR as lv_style_selector_t;

/// The list area, from under the status to just above Back.
pub const VIEW: Rect = Rect::new(0, 34, 400, 218);
/// Every row keeps a 48 px slot. Edge rows shrink their width and text, not
/// their height, so snapping and the scroll position stay exact.
pub const ROW_H: i32 = 48;
/// Where the focused row is centred (the mockup's focus line), a little below
/// the middle of `VIEW` so the top rows stay clear of the fade.
const FOCUS_Y: i32 = 152;

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    pub icon: &'static str,
    pub title: String,
    pub value: Option<String>,
    pub danger: bool,
    pub locked: bool,
}

impl ListItem {
    pub fn new(icon: &'static str, title: impl Into<String>) -> Self {
        Self { icon, title: title.into(), value: None, danger: false, locked: false }
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn locked(mut self, locked: bool) -> Self {
        self.locked = locked;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    Focus,
    Near,
    Far,
}

/// How a row looks in each band ("Rotary list" in docs/mockups/README.md).
struct Look {
    width: i32,
    fill: bool,
    title_font: Font,
    title_color: u32,
    value_font: Font,
    value_color: u32,
    /// Leaves the title at least 30 px next to a capped value and a lock.
    value_max_w: i32,
    opa: u8,
}

impl Band {
    fn look(self) -> Look {
        match self {
            Band::Focus => Look {
                width: 320,
                fill: true,
                title_font: Font::Title,
                title_color: TEXT,
                value_font: Font::Body,
                value_color: TEXT_2,
                value_max_w: 128,
                opa: 255,
            },
            Band::Near => Look {
                width: 296,
                fill: false,
                title_font: Font::Body,
                title_color: TEXT_2,
                value_font: Font::Small,
                value_color: TEXT_3,
                value_max_w: 118,
                opa: 255,
            },
            Band::Far => Look {
                width: 220,
                fill: false,
                title_font: Font::Small,
                title_color: TEXT_2,
                value_font: Font::Small,
                value_color: TEXT_3,
                value_max_w: 88,
                opa: 128,
            },
        }
    }
}

struct Row {
    obj: Obj,
    title: Obj,
    value: Obj,
    /// The value as given. A truncated label keeps its dots in LVGL's buffer, so
    /// the text is set again whenever the font or width limit changes.
    full_value: RefCell<String>,
    danger: bool,
    /// The band last applied, so scrolling restyles only rows that cross a band edge.
    band: Cell<Option<Band>>,
}

struct Inner {
    viewport: Obj,
    rows: Vec<Row>,
    scrollbar: Obj,
}

pub struct RotaryList {
    inner: Rc<Inner>,
}

impl RotaryList {
    pub fn new(parent: Obj, items: Vec<ListItem>, on_select: impl FnMut(usize) + 'static) -> Self {
        let viewport = Obj::plain(parent).rect(VIEW).column_centered(0);
        // LVGL snaps to the centre of the padded content area, which is FOCUS_Y.
        let pad_top = FOCUS_Y - VIEW.y - ROW_H / 2;
        let pad_bottom = VIEW.h - pad_top - ROW_H;
        // SAFETY: live object.
        unsafe {
            lv_obj_add_flag(viewport.raw(), lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            lv_obj_set_scroll_dir(viewport.raw(), lv_dir_t_LV_DIR_VER);
            lv_obj_set_scroll_snap_y(viewport.raw(), lv_scroll_snap_t_LV_SCROLL_SNAP_CENTER);
            lv_obj_set_scrollbar_mode(viewport.raw(), lv_scrollbar_mode_t_LV_SCROLLBAR_MODE_OFF);
            lv_obj_set_style_pad_top(viewport.raw(), pad_top, MAIN);
            lv_obj_set_style_pad_bottom(viewport.raw(), pad_bottom, MAIN);
        }
        let select = Rc::new(RefCell::new(on_select));
        let rows = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let obj = Obj::plain(viewport)
                    .size(320, ROW_H)
                    .radius(24)
                    .pressed_bg(RAISED)
                    .clickable(true)
                    .row_start(12, 18);
                Obj::label(obj, item.icon, Font::Icon20, if item.danger { REC } else { TEXT_2 });
                let title = Obj::label(obj, &item.title, Font::Body, TEXT_2).grow().one_line();
                let tail = Obj::plain(obj).fit().row(6);
                let full_value = item.value.clone().unwrap_or_default();
                let value = Obj::label(tail, &full_value, Font::Small, TEXT_3);
                if item.locked {
                    Obj::label(tail, icons::LOCK, Font::Icon16, TEXT_3);
                }
                let select = select.clone();
                on_click(obj, move || (select.borrow_mut())(index));
                Row { obj, title, value, full_value: RefCell::new(full_value), danger: item.danger, band: Cell::new(None) }
            })
            .collect();
        fade(parent, Rect::new(0, VIEW.y, VIEW.w, 18), true);
        fade(parent, Rect::new(0, VIEW.y + VIEW.h - 22, VIEW.w, 22), false);
        let scrollbar = scrollbar(parent);
        let inner = Rc::new(Inner { viewport, rows, scrollbar });
        {
            let inner = inner.clone();
            on(viewport, lv_event_code_t_LV_EVENT_SCROLL, move |_| inner.restyle());
        }
        inner.restyle();
        Self { inner }
    }

    /// Centre the row at `index` without animation. The scroll event restyles
    /// the rows, so nothing else is needed here.
    pub fn scroll_to(&self, index: usize) {
        let inner = &self.inner;
        // SAFETY: live viewport.
        unsafe {
            lv_obj_scroll_to_y(inner.viewport.raw(), index as i32 * ROW_H, lv_anim_enable_t_LV_ANIM_OFF);
        }
    }

    pub fn set_value(&self, index: usize, value: Option<&str>) {
        let inner = &self.inner;
        let row = &inner.rows[index];
        *row.full_value.borrow_mut() = value.unwrap_or("").to_string();
        // Forget the band so the next restyle measures the new value.
        row.band.set(None);
        inner.restyle();
    }

    pub fn row(&self, index: usize) -> Obj {
        self.inner.rows[index].obj
    }

    pub fn band(&self, index: usize) -> Band {
        self.inner.band_of(index)
    }
}

impl Inner {
    fn band_of(&self, index: usize) -> Band {
        let a = self.rows[index].obj.coords();
        let distance = ((a.y1 + a.y2) / 2 - FOCUS_Y).abs();
        if distance < 24 {
            Band::Focus
        } else if distance < 72 {
            Band::Near
        } else {
            Band::Far
        }
    }

    fn restyle(&self) {
        self.viewport.update_layout();
        let mut changed = false;
        for (index, row) in self.rows.iter().enumerate() {
            let band = self.band_of(index);
            if row.band.replace(Some(band)) == Some(band) {
                continue;
            }
            changed = true;
            let look = band.look();
            row.obj.width(look.width).opa(look.opa);
            if look.fill {
                row.obj.bg(SURFACE);
            } else {
                row.obj.bg_opa(0);
            }
            row.title
                .font(look.title_font)
                .text_color(if row.danger { REC } else { look.title_color })
                .one_line();
            // The value gets an explicit width: a content-sized label would
            // measure its own dotted text and stay too narrow after a band change.
            let full = row.full_value.borrow();
            row.value.font(look.value_font).text_color(look.value_color).hidden(full.is_empty());
            row.value.set_text(&full);
            row.value.width(text_width(&full, look.value_font).min(look.value_max_w)).one_line();
        }
        if changed {
            self.viewport.update_layout();
        }
        self.update_scrollbar();
    }

    fn update_scrollbar(&self) {
        let rows = self.rows.len().max(1) as f32;
        let thumb = (56.0 * (4.2 / rows).min(1.0)).max(10.0);
        let max_scroll = ((rows - 1.0) * ROW_H as f32).max(1.0);
        // SAFETY: live viewport.
        let scrolled = unsafe { lv_obj_get_scroll_y(self.viewport.raw()) } as f32;
        let start = 62.0 + (56.0 - thumb) * (scrolled / max_scroll).clamp(0.0, 1.0);
        // SAFETY: live arc.
        unsafe { lv_arc_set_angles(self.scrollbar.raw(), lvgl_angle(start), lvgl_angle(start + thumb)) };
    }
}

fn scrollbar(parent: Obj) -> Obj {
    let (r, width) = (180, 4);
    let size = 2 * r + width;
    let arc = Obj::arc(parent).pos(200 - size / 2, 152 - size / 2).size(size, size);
    // SAFETY: live arc.
    unsafe {
        lv_arc_set_rotation(arc.raw(), 0);
        lv_arc_set_bg_angles(arc.raw(), lvgl_angle(62.0), lvgl_angle(118.0));
        for (part, hex) in [(MAIN, LINE), (INDICATOR, TEXT_2)] {
            lv_obj_set_style_arc_color(arc.raw(), color(hex), part);
            lv_obj_set_style_arc_width(arc.raw(), width, part);
            lv_obj_set_style_arc_rounded(arc.raw(), true, part);
            lv_obj_set_style_arc_opa(arc.raw(), LV_OPA_COVER as lv_opa_t, part);
        }
    }
    arc
}

/// A black gradient over the list edge so rows fade out instead of being cut.
fn fade(parent: Obj, rect: Rect, top: bool) {
    let obj = Obj::plain(parent).rect(rect).bg(BG);
    let (from, to) = if top { (255, 0) } else { (0, 255) };
    // SAFETY: live object.
    unsafe {
        lv_obj_set_style_bg_grad_color(obj.raw(), color(BG), MAIN);
        lv_obj_set_style_bg_grad_dir(obj.raw(), lv_grad_dir_t_LV_GRAD_DIR_VER, MAIN);
        lv_obj_set_style_bg_main_opa(obj.raw(), from, MAIN);
        lv_obj_set_style_bg_grad_opa(obj.raw(), to, MAIN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, lvgl};

    fn assert_children_inside(row: Obj, what: &str) {
        let bounds = row.coords();
        for child in 0..row.child_count() {
            let a = row.child(child as i32).coords();
            assert!(
                a.x1 >= bounds.x1 && a.x2 <= bounds.x2 && a.y1 >= bounds.y1 && a.y2 <= bounds.y2,
                "{what}: child {child} overflows the row"
            );
        }
    }

    fn menu() -> Vec<ListItem> {
        vec![
            ListItem::new(icons::FOLDER, "Files").value("12").locked(true),
            ListItem::new(icons::PATTERN, "Audio mode").value("Follow talker"),
            ListItem::new(icons::NOISE, "Noise reduction").value("Medium"),
            ListItem::new(icons::VOLUME_HIGH, "Volume").value("60%"),
            ListItem::new(icons::HEIGHT, "Mic height").value("Medium"),
            ListItem::new(icons::WIFI, "Wi-Fi").value("Office-5G").locked(true),
            ListItem::new(icons::BLUETOOTH, "Bluetooth").value("iPhone").locked(true),
            ListItem::new(icons::SEND, "Uploads").value("Ask me").locked(true),
            ListItem::new(icons::SETTINGS, "System").locked(true),
        ]
    }

    #[test]
    fn the_middle_row_is_focused() {
        lvgl(|lv| {
            let root = lv.screen();
            let list = RotaryList::new(root, menu(), |_| {});
            list.scroll_to(2);
            assert_eq!(list.band(2), Band::Focus);
            assert_eq!(list.band(1), Band::Near);
            assert_eq!(list.band(3), Band::Near);
            assert_eq!(list.band(4), Band::Far);
            list.row(2).update_layout();
            let a = list.row(2).coords();
            assert_eq!(a.x2 - a.x1 + 1, 320);
            // Snapping puts the focused row's centre on the focus line, and the
            // slots below and above sit one pitch apart.
            for (index, y) in [(0, 56), (1, 104), (2, 152), (3, 200), (4, 248)] {
                let a = list.row(index).coords();
                assert_eq!((a.y1 + a.y2 + 1) / 2, y, "row {index}");
            }
            assert_golden("list_menu", &lv.render());
        });
    }

    #[test]
    fn taps_report_the_row() {
        lvgl(|lv| {
            let root = lv.screen();
            let picked = Rc::new(Cell::new(usize::MAX));
            let chosen = picked.clone();
            let list = RotaryList::new(root, menu(), move |index| chosen.set(index));
            list.scroll_to(3);
            lv.click(list.row(3));
            assert_eq!(picked.get(), 3);
        });
    }

    #[test]
    fn visible_rows_stay_inside_the_tap_limit() {
        lvgl(|lv| {
            let root = lv.screen();
            let items = menu();
            let count = items.len();
            let list = RotaryList::new(root, items, |_| {});
            for focus in 0..count {
                list.scroll_to(focus);
                for index in 0..count {
                    let a = list.row(index).coords();
                    let centre = (a.y1 + a.y2) / 2;
                    if centre > VIEW.y && centre < VIEW.y + VIEW.h {
                        let rect = Rect::new(a.x1, a.y1, a.x2 - a.x1 + 1, a.y2 - a.y1 + 1);
                        assert!(rect.within_tap_limit(), "focus {focus}, row {index}: {rect:?}");
                    }
                }
            }
        });
    }

    #[test]
    fn long_titles_stay_inside_the_row() {
        lvgl(|lv| {
            let root = lv.screen();
            let list = RotaryList::new(
                root,
                vec![
                    ListItem::new(icons::WIFI, "A Wi-Fi network with an unusually long name for a meeting room")
                        .value("Connected"),
                    ListItem::new(icons::WIFI, "Wi-Fi").value("Board room network with a long name").locked(true),
                    ListItem::new(icons::SEND, "Uploads to a server with a long name")
                        .value("recordings.example-company.internal")
                        .locked(true),
                ],
                |_| {},
            );
            // Each focus puts the rows in different bands (focus, near, far).
            for focus in 0..3 {
                list.scroll_to(focus);
                for index in 0..3 {
                    assert_children_inside(list.row(index), &format!("focus {focus}, row {index}"));
                    let title = list.row(index).child(1);
                    let a = title.coords();
                    assert!(a.x2 - a.x1 + 1 > 0, "focus {focus}, row {index}: title has no width");
                }
            }
            // The two-long-text rows really end in dots, in every band.
            for focus in 0..3 {
                list.scroll_to(focus);
                assert!(list.row(0).child(1).text().ends_with("..."), "focus {focus}: long title");
                assert!(list.row(1).child(2).child(0).text().ends_with("..."), "focus {focus}: long value");
                assert!(list.row(2).child(2).child(0).text().ends_with("..."), "focus {focus}: long host");
            }
        });
    }

    #[test]
    fn a_long_value_set_later_stays_inside_the_row() {
        lvgl(|lv| {
            let root = lv.screen();
            let list = RotaryList::new(root, menu(), |_| {});
            list.scroll_to(2);
            let long = "A value that is far too long for any row";
            list.set_value(2, Some(long));
            list.set_value(3, Some(long));
            for (index, cap) in [(2, Band::Focus), (3, Band::Near)] {
                assert_eq!(list.band(index), cap);
                let value = list.row(index).child(2).child(0);
                let a = value.coords();
                let width = a.x2 - a.x1 + 1;
                assert!(width <= cap.look().value_max_w, "row {index}: value too wide");
                assert!(value.text().ends_with("..."), "row {index}");
                assert_children_inside(list.row(index), &format!("row {index}"));
                let title = list.row(index).child(1).coords();
                assert!(title.x2 - title.x1 + 1 > 0, "row {index}: title has no width");
            }
            // Clearing the value hides it and frees the room.
            list.set_value(2, None);
            assert!(list.row(2).child(2).child(0).is_hidden());
        });
    }

    #[test]
    fn the_end_of_the_system_list_shows_power_off_in_red() {
        lvgl(|lv| {
            let root = lv.screen();
            let list = RotaryList::new(
                root,
                vec![
                    ListItem::new(icons::CLOCK, "Pre-recording").value("On"),
                    ListItem::new(icons::INFO, "About").value("waved 0.9.3"),
                    ListItem::new(icons::REFRESH, "Firmware update").value("Up to date"),
                    ListItem::new(icons::TRASH, "Factory reset"),
                    ListItem::new(icons::POWER, "Power off").danger(),
                ],
                |_| {},
            );
            list.scroll_to(2);
            assert_golden("list_system", &lv.render());
        });
    }
}
