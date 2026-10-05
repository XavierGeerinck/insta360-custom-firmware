//! Settings controls: radio lists, value rows, switch rows and the value arc.

use crate::bindings::*;
use crate::kit::event::{on, on_click};
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{color, text_width, Obj};
use crate::kit::tokens::{
    lvgl_angle, Rect, BACK, BG, LINE, LIVE, RAISED, SURFACE, TEXT, TEXT_2, TEXT_3, WHITE,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const MAIN: lv_style_selector_t = LV_PART_MAIN as lv_style_selector_t;
const INDICATOR: lv_style_selector_t = LV_PART_INDICATOR as lv_style_selector_t;
const KNOB: lv_style_selector_t = LV_PART_KNOB as lv_style_selector_t;

/// Text width in a radio row: 296 - 2 × 16 padding - 20 ring - 12 gap.
const TEXT_W: i32 = 232;

#[derive(Clone, Debug, PartialEq)]
pub struct RadioOption {
    pub title: String,
    pub detail: String,
}

impl RadioOption {
    pub fn new(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
        }
    }
}

struct RadioRow {
    obj: Obj,
    ring: Obj,
    dot: Obj,
}

pub struct RadioList {
    viewport: Obj,
    rows: Rc<Vec<RadioRow>>,
    selected: Rc<Cell<usize>>,
}

impl RadioList {
    /// Option rows (296×52, 4 px apart: y 68 / 124 / 180) from `top` (`RADIO_TOP` under a title),
    /// scrolling if they don't fit above Back.
    pub fn new(
        parent: Obj,
        top: i32,
        options: Vec<RadioOption>,
        selected: usize,
        on_select: impl FnMut(usize) + 'static,
    ) -> Self {
        let viewport = Obj::plain(parent)
            .rect(Rect::new(0, top, 400, BACK.y - top))
            .column_centered(4);
        // SAFETY: live object.
        unsafe {
            lv_obj_add_flag(viewport.raw(), lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            lv_obj_set_scroll_dir(viewport.raw(), lv_dir_t_LV_DIR_VER);
            lv_obj_set_scrollbar_mode(viewport.raw(), lv_scrollbar_mode_t_LV_SCROLLBAR_MODE_OFF);
        }
        let rows: Vec<RadioRow> = options
            .iter()
            .map(|option| {
                // The mockup's 2 px border is an inset shadow inside a 16 px inset; LVGL's border
                // adds to the padding, so pad 14 puts the ring 16 px from the row edge.
                let obj = Obj::plain(viewport)
                    .size(296, 52)
                    .radius(20)
                    .clickable(true)
                    .row_start(12, 14);
                let ring = Obj::plain(obj).size(20, 20).round().border(2, TEXT_3);
                let dot = Obj::plain(ring)
                    .size(10, 10)
                    .round()
                    .bg(LIVE)
                    .center()
                    .hidden(true);
                let text = Obj::plain(obj).fit().column(2);
                // Explicit widths: a content-width one-line label collapses to "...".
                Obj::label(text, &option.title, Font::Body, TEXT)
                    .one_line()
                    .width(text_width(&option.title, Font::Body).min(TEXT_W));
                Obj::label(text, &option.detail, Font::Micro, TEXT_2)
                    .one_line()
                    .width(text_width(&option.detail, Font::Micro).min(TEXT_W));
                RadioRow { obj, ring, dot }
            })
            .collect();
        let list = Self {
            viewport,
            rows: Rc::new(rows),
            selected: Rc::new(Cell::new(selected)),
        };
        let handler = Rc::new(RefCell::new(on_select));
        for index in 0..list.rows.len() {
            let (rows, chosen, handler) =
                (list.rows.clone(), list.selected.clone(), handler.clone());
            on_click(list.rows[index].obj, move || {
                paint_radio(&rows, index);
                chosen.set(index);
                (handler.borrow_mut())(index);
            });
        }
        paint_radio(&list.rows, selected);
        list
    }

    pub fn set_selected(&self, index: usize) {
        paint_radio(&self.rows, index);
        self.selected.set(index);
    }

    pub fn selected(&self) -> usize {
        self.selected.get()
    }

    /// Scroll the list so the row at `index` is fully visible, without
    /// animation (Time zone and Dim after open on the selected row).
    pub fn scroll_to(&self, index: usize) {
        // SAFETY: live row inside the live viewport.
        unsafe { lv_obj_scroll_to_view(self.rows[index].obj.raw(), lv_anim_enable_t_LV_ANIM_OFF) };
        self.viewport.update_layout();
    }

    pub fn row(&self, index: usize) -> Obj {
        self.rows[index].obj
    }
}

fn paint_radio(rows: &[RadioRow], selected: usize) {
    for (index, row) in rows.iter().enumerate() {
        let on = index == selected;
        if on {
            row.obj.bg(SURFACE).border(2, LIVE);
        } else {
            // Keep the 2 px border (black) so the ring does not shift.
            row.obj.bg_opa(0).border(2, BG);
        }
        row.ring.border(2, if on { LIVE } else { TEXT_3 });
        row.dot.hidden(!on);
    }
}

pub struct ValueRow {
    obj: Obj,
    value: Obj,
    cap: i32,
}

impl ValueRow {
    /// A tappable row: label on the left, current value and a chevron on the right.
    pub fn new(
        parent: Obj,
        rect: Rect,
        label: &str,
        value: &str,
        on_tap: impl FnMut() + 'static,
    ) -> Self {
        let obj = Obj::plain(parent)
            .rect(rect)
            .bg(SURFACE)
            .radius(20)
            .pressed_bg(RAISED)
            .clickable(true)
            .row_start(8, 16);
        Obj::label(obj, label, Font::Small, TEXT_2)
            .grow()
            .one_line();
        // An explicit width, not `max_width`: LVGL keeps dotted text when a
        // content-width label grows later, so `set_value` recomputes it.
        let cap = rect.w / 2;
        let value_label = Obj::label(obj, value, Font::Body, TEXT).one_line();
        value_label.width(text_width(value, Font::Body).min(cap));
        Obj::label(obj, icons::RIGHT, Font::Icon16, TEXT_3);
        on_click(obj, on_tap);
        Self {
            obj,
            value: value_label,
            cap,
        }
    }

    pub fn set_value(&self, value: &str) {
        self.value.set_text(value);
        self.value
            .width(text_width(value, Font::Body).min(self.cap));
    }

    pub fn obj(&self) -> Obj {
        self.obj
    }
}

pub struct SwitchRow {
    obj: Obj,
    track: Obj,
    knob: Obj,
    on: Rc<Cell<bool>>,
}

impl SwitchRow {
    /// A row that toggles a setting; the whole row is the target.
    pub fn new(
        parent: Obj,
        rect: Rect,
        label: &str,
        on: bool,
        on_toggle: impl FnMut(bool) + 'static,
    ) -> Self {
        let obj = Obj::plain(parent)
            .rect(rect)
            .bg(SURFACE)
            .radius(20)
            .pressed_bg(RAISED)
            .clickable(true)
            .row_start(8, 16);
        Obj::label(obj, label, Font::Small, TEXT_2)
            .grow()
            .one_line();
        let track = Obj::plain(obj).size(44, 26).radius(13);
        let knob = Obj::plain(track).size(20, 20).round().bg(WHITE);
        let state = Rc::new(Cell::new(on));
        paint_switch(track, knob, on);
        let shared = state.clone();
        let mut toggle = on_toggle;
        on_click(obj, move || {
            let next = !shared.get();
            shared.set(next);
            paint_switch(track, knob, next);
            toggle(next);
        });
        Self {
            obj,
            track,
            knob,
            on: state,
        }
    }

    pub fn set_on(&self, on: bool) {
        self.on.set(on);
        paint_switch(self.track, self.knob, on);
    }

    pub fn is_on(&self) -> bool {
        self.on.get()
    }

    pub fn obj(&self) -> Obj {
        self.obj
    }
}

fn paint_switch(track: Obj, knob: Obj, on: bool) {
    track.bg(if on { LIVE } else { LINE });
    knob.pos(if on { 21 } else { 3 }, 3);
}

pub struct ValueArc {
    arc: Obj,
}

impl ValueArc {
    /// A 270° slider (−135°…135°) of radius `r` from 0 to 100 with a 22 px knob.
    pub fn new(parent: Obj, cx: i32, cy: i32, r: i32, on_change: impl FnMut(u8) + 'static) -> Self {
        let width = 8;
        let size = 2 * r + width;
        let arc = Obj::arc(parent)
            .pos(cx - size / 2, cy - size / 2)
            .size(size, size)
            .clickable(true)
            .keep_gestures();
        // SAFETY: live arc.
        unsafe {
            lv_arc_set_rotation(arc.raw(), 0);
            lv_arc_set_bg_angles(arc.raw(), lvgl_angle(-135.0), lvgl_angle(135.0));
            lv_arc_set_range(arc.raw(), 0, 100);
            for (part, hex) in [(MAIN, LINE), (INDICATOR, LIVE)] {
                lv_obj_set_style_arc_color(arc.raw(), color(hex), part);
                lv_obj_set_style_arc_width(arc.raw(), width, part);
                lv_obj_set_style_arc_rounded(arc.raw(), true, part);
                lv_obj_set_style_arc_opa(arc.raw(), LV_OPA_COVER as lv_opa_t, part);
            }
            lv_obj_set_style_bg_color(arc.raw(), color(WHITE), KNOB);
            lv_obj_set_style_bg_opa(arc.raw(), LV_OPA_COVER as lv_opa_t, KNOB);
            lv_obj_set_style_radius(arc.raw(), LV_RADIUS_CIRCLE as i32, KNOB);
            lv_obj_set_style_pad_top(arc.raw(), 7, KNOB);
            lv_obj_set_style_pad_bottom(arc.raw(), 7, KNOB);
            lv_obj_set_style_pad_left(arc.raw(), 7, KNOB);
            lv_obj_set_style_pad_right(arc.raw(), 7, KNOB);
        }
        let mut changed = on_change;
        on(arc, lv_event_code_t_LV_EVENT_VALUE_CHANGED, move |event| {
            // SAFETY: the target is this arc.
            let value = unsafe { lv_arc_get_value(event.target().raw()) };
            changed(value.clamp(0, 100) as u8);
        });
        Self { arc }
    }

    pub fn set_value(&self, percent: u8) {
        // SAFETY: live arc.
        unsafe { lv_arc_set_value(self.arc.raw(), i32::from(percent.min(100))) };
    }

    pub fn value(&self) -> u8 {
        // SAFETY: live arc.
        unsafe { lv_arc_get_value(self.arc.raw()) }.clamp(0, 100) as u8
    }

    pub fn obj(&self) -> Obj {
        self.arc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::kit::tokens::RADIO_TOP;

    #[test]
    fn a_radio_list_selects_one_option() {
        lvgl(|lv| {
            let root = lv.screen();
            let picked = Rc::new(Cell::new(usize::MAX));
            let chosen = picked.clone();
            let list = RadioList::new(
                root,
                RADIO_TOP,
                vec![
                    RadioOption::new("Send automatically", "As soon as Wi-Fi allows"),
                    RadioOption::new("Ask me each time", "Choose Send, Keep or Delete"),
                    RadioOption::new("Keep on Wave", "Nothing leaves the device"),
                ],
                1,
                move |index| chosen.set(index),
            );
            assert_golden("radio_after_recording", &lv.render());
            assert_targets_ok(root);
            // Rows 296 × 52 at x 52, y 68 / 124 / 180, all above Back.
            for (index, y) in [(0, 68), (1, 124), (2, 180)] {
                let a = list.row(index).coords();
                assert_eq!(
                    (a.x1, a.y1, a.x2 - a.x1 + 1, a.y2 - a.y1 + 1),
                    (52, y, 296, 52)
                );
            }
            let viewport = root.child(0);
            assert!(viewport.coords().y2 < BACK.y, "the list stops above Back");
            lv.click(list.row(2));
            assert_eq!((picked.get(), list.selected()), (2, 2));
        });
    }

    #[test]
    fn a_long_radio_list_opens_on_the_selected_row() {
        lvgl(|lv| {
            let root = lv.screen();
            let options = (0..12)
                .map(|i| RadioOption::new(format!("Zone {i}"), "UTC+01:00"))
                .collect();
            let list = RadioList::new(root, RADIO_TOP, options, 9, |_| {});
            root.update_layout();
            let viewport = root.child(0).coords();
            let fully_visible = |index: usize| {
                let a = list.row(index).coords();
                a.y1 >= viewport.y1 && a.y2 <= viewport.y2
            };
            assert!(
                fully_visible(0) && !fully_visible(9),
                "lists open at the top"
            );
            list.scroll_to(9);
            assert!(fully_visible(9), "row 9 shows without animation");
            list.scroll_to(11);
            assert!(fully_visible(11), "the last row");
            list.scroll_to(1);
            assert!(fully_visible(1));
            list.scroll_to(0);
            assert_eq!(list.row(0).coords().y1, RADIO_TOP, "back at the top");
        });
    }

    #[test]
    fn value_and_switch_rows_report_taps() {
        lvgl(|lv| {
            let root = lv.screen();
            let taps = Rc::new(Cell::new(0));
            let counter = taps.clone();
            let zone = ValueRow::new(
                root,
                Rect::new(60, 146, 280, 44),
                "Time zone",
                "Brussels",
                move || counter.set(counter.get() + 1),
            );
            let toggled = Rc::new(Cell::new(None));
            let record = toggled.clone();
            let clock = SwitchRow::new(
                root,
                Rect::new(60, 196, 280, 44),
                "24-hour clock",
                true,
                move |on| record.set(Some(on)),
            );
            assert_golden("value_and_switch_rows", &lv.render());
            assert_targets_ok(root);
            lv.click(zone.obj());
            assert_eq!(taps.get(), 1);
            lv.click(clock.obj());
            assert_eq!((toggled.get(), clock.is_on()), (Some(false), false));
        });
    }

    #[test]
    fn long_values_stay_inside_the_row() {
        lvgl(|lv| {
            let root = lv.screen();
            let host = ValueRow::new(
                root,
                Rect::new(60, 146, 280, 44),
                "Server",
                "recordings.meeting-rooms.example-company.internal",
                || {},
            );
            let row = host.obj();
            row.update_layout();
            let bounds = row.coords();
            for index in 0..row.child_count() {
                let a = row.child(index as i32).coords();
                assert!(
                    a.x1 >= bounds.x1
                        && a.x2 <= bounds.x2
                        && a.y1 >= bounds.y1
                        && a.y2 <= bounds.y2,
                    "child {index} overflows the row"
                );
            }
            let value = host.value;
            assert!(
                value.text().ends_with("..."),
                "long value is cut with dots: {:?}",
                value.text()
            );
            let a = value.coords();
            assert!(
                a.x2 - a.x1 + 1 >= 100,
                "value label is only {} px wide",
                a.x2 - a.x1 + 1
            );
        });
    }

    #[test]
    fn value_arc_keeps_its_drags() {
        lvgl(|lv| {
            let root = lv.screen();
            let arc = ValueArc::new(root, 200, 132, 88, |_| {});
            assert!(
                !arc.obj().has_flag(lv_obj_flag_t_LV_OBJ_FLAG_GESTURE_BUBBLE),
                "dragging the knob right must not read as swipe-right-for-Back"
            );
        });
    }

    #[test]
    fn the_value_arc_reports_changes() {
        lvgl(|lv| {
            let root = lv.screen();
            let seen = Rc::new(Cell::new(0u8));
            let record = seen.clone();
            let arc = ValueArc::new(root, 200, 146, 92, move |value| record.set(value));
            arc.set_value(60);
            assert_golden("value_arc_60", &lv.render());
            assert_targets_ok(root);
            arc.set_value(75);
            lv.send(arc.obj(), lv_event_code_t_LV_EVENT_VALUE_CHANGED);
            assert_eq!((seen.get(), arc.value()), (75, 75));
        });
    }
}
