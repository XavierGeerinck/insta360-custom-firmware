//! A thin, copyable handle over `lv_obj_t` with chainable setters. Screens and
//! components use this instead of raw FFI. Call it on the UI thread only; the
//! raw pointer keeps `Obj` from being `Send`.

use crate::bindings::*;
use crate::kit::fonts::Font;
use crate::kit::tokens::{Rect, BG, SCREEN_H, SCREEN_W};
use std::ffi::{CStr, CString};

const MAIN: lv_style_selector_t = LV_PART_MAIN as lv_style_selector_t;

/// LVGL's "size to content" (LV_COORD_SET_SPEC(LV_COORD_MAX) in lv_area.h).
pub const SIZE_CONTENT: i32 = ((1 << 29) - 1) | (1 << 29);

/// Width of `text` on one line in `font`, in pixels.
pub fn text_width(text: &str, font: Font) -> i32 {
    let text = CString::new(text.replace('\0', "")).expect("NUL bytes removed");
    // SAFETY: NUL-terminated text and a static font.
    unsafe { lv_text_get_width(text.as_ptr(), text.as_bytes().len() as u32, font.ptr(), 0) }
}

/// 0xRRGGBB to an LVGL colour.
pub fn color(hex: u32) -> lv_color_t {
    // SAFETY: pure conversion.
    unsafe { lv_color_hex(hex) }
}

/// A handle to an LVGL object. It is `Copy`, so every copy dangles once the
/// object is deleted (by `delete()` or because an ancestor was deleted); don't
/// use a handle after that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Obj(*mut lv_obj_t);

impl Obj {
    /// Wrap a raw LVGL object.
    ///
    /// # Safety
    /// `raw` must be null or point to a live `lv_obj_t`, and must not be used
    /// through this handle after the object is deleted.
    pub unsafe fn from_raw(raw: *mut lv_obj_t) -> Self {
        Self(raw)
    }

    /// Debug-build check that this is a label, for the label-only methods.
    fn assert_label(self) {
        self.check_live();
        debug_assert!(
            // SAFETY: live object; `lv_label_class` is a static class descriptor.
            unsafe { lv_obj_check_type(self.0, &raw const lv_label_class) },
            "this Obj is not a label"
        );
    }

    /// Debug-build check that the object still exists. `Obj` is `Copy`, so a
    /// handle can outlive its object; using it then is a use-after-free.
    /// `lv_obj_is_valid` walks every object, so release builds skip it.
    #[inline]
    fn check_live(self) {
        #[cfg(debug_assertions)]
        debug_assert!(
            // SAFETY: only compares the pointer against live objects.
            unsafe { lv_obj_is_valid(self.0) },
            "Obj used after its object was deleted"
        );
    }

    pub fn raw(self) -> *mut lv_obj_t {
        self.0
    }

    pub fn is_null(self) -> bool {
        self.0.is_null()
    }

    /// A new top-level screen: 400×304, black, no padding, not scrollable.
    pub fn screen() -> Obj {
        // SAFETY: a null parent creates a screen.
        unsafe { Obj(lv_obj_create(std::ptr::null_mut())) }
            .reset()
            .size(SCREEN_W, SCREEN_H)
            .bg(BG)
    }

    /// A container with every theme style removed: transparent, no padding,
    /// border or scrolling, not clickable.
    pub fn plain(parent: Obj) -> Obj {
        parent.check_live();
        // SAFETY: parent is a live object.
        unsafe { Obj(lv_obj_create(parent.0)) }.reset()
    }

    pub fn label(parent: Obj, text: &str, font: Font, hex: u32) -> Obj {
        parent.check_live();
        // SAFETY: parent is a live object.
        let label = unsafe { Obj(lv_label_create(parent.0)) }.reset();
        label.set_text(text);
        label.font(font).text_color(hex)
    }

    pub fn arc(parent: Obj) -> Obj {
        parent.check_live();
        // SAFETY: parent is a live object.
        unsafe { Obj(lv_arc_create(parent.0)) }.reset()
    }

    pub fn line(parent: Obj) -> Obj {
        parent.check_live();
        // SAFETY: parent is a live object.
        unsafe { Obj(lv_line_create(parent.0)) }.reset()
    }

    fn reset(self) -> Self {
        self.check_live();
        // SAFETY: self is a live object.
        unsafe {
            lv_obj_remove_style(
                self.0,
                std::ptr::null(),
                (LV_PART_ANY | LV_STATE_ANY) as lv_style_selector_t,
            );
            lv_obj_remove_flag(self.0, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            lv_obj_remove_flag(self.0, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
        }
        self
    }

    pub fn pos(self, x: i32, y: i32) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_set_pos(self.0, x, y) };
        self
    }

    pub fn size(self, w: i32, h: i32) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_set_size(self.0, w, h) };
        self
    }

    pub fn rect(self, r: Rect) -> Self {
        self.pos(r.x, r.y).size(r.w, r.h)
    }

    /// Size to the content (text or children).
    pub fn fit(self) -> Self {
        self.size(SIZE_CONTENT, SIZE_CONTENT)
    }

    /// Children in a row, centred both ways, `gap` px apart.
    pub fn row(self, gap: i32) -> Self {
        self.flex(
            lv_flex_flow_t_LV_FLEX_FLOW_ROW,
            lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            gap,
        )
    }

    /// Children in a row from the left, vertically centred, with side padding.
    pub fn row_start(self, gap: i32, pad_x: i32) -> Self {
        self.check_live();
        let row = self.flex(
            lv_flex_flow_t_LV_FLEX_FLOW_ROW,
            lv_flex_align_t_LV_FLEX_ALIGN_START,
            lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            gap,
        );
        // SAFETY: live object.
        unsafe {
            lv_obj_set_style_pad_left(self.0, pad_x, MAIN);
            lv_obj_set_style_pad_right(self.0, pad_x, MAIN);
        }
        row
    }

    /// Children in a column, left-aligned, `gap` px apart.
    pub fn column(self, gap: i32) -> Self {
        self.flex(
            lv_flex_flow_t_LV_FLEX_FLOW_COLUMN,
            lv_flex_align_t_LV_FLEX_ALIGN_START,
            lv_flex_align_t_LV_FLEX_ALIGN_START,
            gap,
        )
    }

    /// Children in a column, centred across, `gap` px apart.
    pub fn column_centered(self, gap: i32) -> Self {
        self.flex(
            lv_flex_flow_t_LV_FLEX_FLOW_COLUMN,
            lv_flex_align_t_LV_FLEX_ALIGN_START,
            lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            gap,
        )
    }

    fn flex(
        self,
        flow: lv_flex_flow_t,
        main: lv_flex_align_t,
        cross: lv_flex_align_t,
        gap: i32,
    ) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe {
            lv_obj_set_flex_flow(self.0, flow);
            lv_obj_set_flex_align(self.0, main, cross, cross);
            lv_obj_set_style_pad_row(self.0, gap, MAIN);
            lv_obj_set_style_pad_column(self.0, gap, MAIN);
        }
        self
    }

    /// Take the remaining space in a flex row.
    pub fn grow(self) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_set_flex_grow(self.0, 1) };
        self
    }

    /// Centre in the parent.
    pub fn center(self) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_center(self.0) };
        self
    }

    /// Border while pressed (for example the teal ring on side buttons).
    pub fn pressed_border(self, width: i32, hex: u32) -> Self {
        self.check_live();
        let pressed = MAIN | LV_STATE_PRESSED as lv_style_selector_t;
        // SAFETY: live object.
        unsafe {
            lv_obj_set_style_border_width(self.0, width, pressed);
            lv_obj_set_style_border_color(self.0, color(hex), pressed);
            lv_obj_set_style_border_opa(self.0, LV_OPA_COVER as lv_opa_t, pressed);
        }
        self
    }

    /// Centred text wrapping within `width`.
    pub fn wrapped_text(self, width: i32) -> Self {
        self.check_live();
        self.assert_label();
        // SAFETY: live label.
        unsafe {
            lv_obj_set_width(self.0, width);
            lv_obj_set_style_text_align(self.0, lv_text_align_t_LV_TEXT_ALIGN_CENTER, MAIN);
            lv_label_set_long_mode(self.0, lv_label_long_mode_t_LV_LABEL_LONG_WRAP);
        }
        self
    }

    pub fn width(self, w: i32) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_set_width(self.0, w) };
        self
    }

    pub fn bg(self, hex: u32) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe {
            lv_obj_set_style_bg_color(self.0, color(hex), MAIN);
            lv_obj_set_style_bg_opa(self.0, LV_OPA_COVER as lv_opa_t, MAIN);
        }
        self
    }

    pub fn bg_opa(self, opa: u8) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_set_style_bg_opa(self.0, opa, MAIN) };
        self
    }

    pub fn radius(self, r: i32) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_set_style_radius(self.0, r, MAIN) };
        self
    }

    pub fn round(self) -> Self {
        self.radius(LV_RADIUS_CIRCLE as i32)
    }

    pub fn border(self, width: i32, hex: u32) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe {
            lv_obj_set_style_border_width(self.0, width, MAIN);
            lv_obj_set_style_border_color(self.0, color(hex), MAIN);
            lv_obj_set_style_border_opa(self.0, LV_OPA_COVER as lv_opa_t, MAIN);
        }
        self
    }

    pub fn font(self, font: Font) -> Self {
        self.check_live();
        // SAFETY: live object; static font.
        unsafe { lv_obj_set_style_text_font(self.0, font.ptr(), MAIN) };
        self
    }

    pub fn text_color(self, hex: u32) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_set_style_text_color(self.0, color(hex), MAIN) };
        self
    }

    pub fn opa(self, opa: u8) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_set_style_opa(self.0, opa, MAIN) };
        self
    }

    /// Centre the text across `width` on one line, ending in "..." if too long.
    pub fn centered_text(self, width: i32) -> Self {
        self.check_live();
        self.assert_label();
        // SAFETY: live label.
        unsafe { lv_obj_set_style_text_align(self.0, lv_text_align_t_LV_TEXT_ALIGN_CENTER, MAIN) };
        self.width(width).one_line()
    }

    /// Keep a label on one line, ending in "..." when the text is wider than
    /// the label. LVGL only truncates a label whose height is fixed; with the
    /// default content height it wraps onto more lines instead. So this sets
    /// the height to one line of the current font: call it after `font`, and
    /// again whenever the font changes.
    pub fn one_line(self) -> Self {
        self.check_live();
        self.assert_label();
        // SAFETY: live label. LV_STYLE_TEXT_FONT always resolves to a font
        // (inherited, or LV_FONT_DEFAULT).
        unsafe {
            let font = lv_obj_get_style_prop(self.0, MAIN, LV_STYLE_TEXT_FONT as lv_style_prop_t);
            lv_obj_set_height(self.0, lv_font_get_line_height(font.ptr.cast()));
            lv_label_set_long_mode(self.0, lv_label_long_mode_t_LV_LABEL_LONG_DOT);
        }
        self
    }

    /// Wrapped text limited to `lines` lines of the current font, ending in
    /// "..." when it needs more. Call after `font` and `wrapped_text`: the
    /// height is fixed, which is what makes LVGL truncate instead of grow.
    pub fn max_lines(self, lines: i32) -> Self {
        self.check_live();
        self.assert_label();
        // SAFETY: live label (see `one_line`).
        unsafe {
            let font = lv_obj_get_style_prop(self.0, MAIN, LV_STYLE_TEXT_FONT as lv_style_prop_t);
            lv_obj_set_height(self.0, lines * lv_font_get_line_height(font.ptr.cast()));
            lv_label_set_long_mode(self.0, lv_label_long_mode_t_LV_LABEL_LONG_DOT);
        }
        self
    }

    /// Background while pressed.
    pub fn pressed_bg(self, hex: u32) -> Self {
        self.check_live();
        let pressed = MAIN | LV_STATE_PRESSED as lv_style_selector_t;
        // SAFETY: live object.
        unsafe {
            lv_obj_set_style_bg_color(self.0, color(hex), pressed);
            lv_obj_set_style_bg_opa(self.0, LV_OPA_COVER as lv_opa_t, pressed);
        }
        self
    }

    pub fn set_text(self, text: &str) {
        self.check_live();
        self.assert_label();
        let text = CString::new(text.replace('\0', "")).expect("NUL bytes removed");
        // SAFETY: live label; LVGL copies the text.
        unsafe { lv_label_set_text(self.0, text.as_ptr()) };
    }

    /// The label's current text. For a label that ends in "..." after layout
    /// this is LVGL's internal buffer, which holds the truncated text with the
    /// dots, not the string passed to `set_text`.
    pub fn text(self) -> String {
        self.check_live();
        self.assert_label();
        // SAFETY: live label; the pointer is LVGL's NUL-terminated buffer and
        // is copied before anything can change it.
        unsafe {
            CStr::from_ptr(lv_label_get_text(self.0))
                .to_string_lossy()
                .into_owned()
        }
    }

    pub fn clickable(self, on: bool) -> Self {
        self.flag(lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE, on)
    }

    pub fn hidden(self, on: bool) -> Self {
        self.flag(lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN, on)
    }

    pub fn flag(self, flag: lv_obj_flag_t, on: bool) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe {
            if on {
                lv_obj_add_flag(self.0, flag)
            } else {
                lv_obj_remove_flag(self.0, flag)
            }
        }
        self
    }

    /// Grow the tap area by `px` on every side without changing the visible box.
    pub fn ext_click(self, px: i32) -> Self {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_set_ext_click_area(self.0, px) };
        self
    }

    pub fn set_state(self, state: u32, on: bool) {
        self.check_live();
        // SAFETY: live object.
        unsafe {
            if on {
                lv_obj_add_state(self.0, state as lv_state_t)
            } else {
                lv_obj_remove_state(self.0, state as lv_state_t)
            }
        }
    }

    pub fn has_state(self, state: u32) -> bool {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_has_state(self.0, state as lv_state_t) }
    }

    pub fn has_flag(self, flag: lv_obj_flag_t) -> bool {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_has_flag(self.0, flag) }
    }

    pub fn is_clickable(self) -> bool {
        self.has_flag(lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE)
    }

    pub fn is_hidden(self) -> bool {
        self.has_flag(lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN)
    }

    pub fn coords(self) -> lv_area_t {
        self.check_live();
        let mut area = lv_area_t::default();
        // SAFETY: live object.
        unsafe { lv_obj_get_coords(self.0, &mut area) };
        area
    }

    pub fn click_area(self) -> lv_area_t {
        self.check_live();
        let mut area = lv_area_t::default();
        // SAFETY: live object.
        unsafe { lv_obj_get_click_area(self.0, &mut area) };
        area
    }

    pub fn child_count(self) -> u32 {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_get_child_count(self.0) }
    }

    pub fn child(self, index: i32) -> Obj {
        self.check_live();
        // SAFETY: live object.
        unsafe { Obj(lv_obj_get_child(self.0, index)) }
    }

    pub fn update_layout(self) {
        self.check_live();
        // SAFETY: live object.
        unsafe { lv_obj_update_layout(self.0) };
    }

    /// Delete the object and its children; their handlers are dropped.
    pub fn delete(self) {
        self.check_live();
        // SAFETY: live object; not used afterwards by the caller.
        unsafe { lv_obj_delete(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::{Rect, TEXT};

    #[test]
    fn plain_objects_start_transparent_and_unclickable() {
        lvgl(|lv| {
            let root = lv.screen();
            let obj = Obj::plain(root).rect(Rect::new(0, 0, 20, 20));
            assert!(!obj.is_clickable());
            assert_eq!(lv.render().pixel(10, 10), [0, 0, 0]);
        });
    }

    #[test]
    fn labels_truncate_inside_their_width() {
        lvgl(|lv| {
            let root = lv.screen();
            let label = Obj::label(
                root,
                "Board room network with a very long name",
                Font::Body,
                TEXT,
            )
            .centered_text(120)
            .pos(140, 100);
            label.update_layout();
            let a = label.coords();
            let width = a.x2 - a.x1 + 1;
            assert!(width <= 120, "label is {width} px wide");
            assert!(
                label.text().ends_with("..."),
                "long text is cut with dots: {:?}",
                label.text()
            );
            assert_eq!(
                a.y2 - a.y1 + 1,
                Font::Body.line_height(),
                "one line, not wrapped"
            );
        });
    }

    #[test]
    fn short_labels_keep_their_text() {
        lvgl(|lv| {
            let root = lv.screen();
            let label = Obj::label(root, "Mic on", Font::Body, TEXT)
                .centered_text(120)
                .pos(140, 100);
            label.update_layout();
            assert_eq!(label.text(), "Mic on");
        });
    }

    #[test]
    fn text_with_a_nul_byte_is_kept_without_it() {
        lvgl(|lv| {
            let root = lv.screen();
            let label = Obj::label(root, "Mic\0 on", Font::Body, TEXT);
            assert_eq!(label.text(), "Mic on");
        });
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "used after its object was deleted")]
    fn a_stale_handle_is_caught_in_debug_builds() {
        lvgl(|lv| {
            let root = lv.screen();
            let obj = Obj::plain(root);
            obj.delete();
            obj.pos(0, 0);
        });
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "used after its object was deleted")]
    fn a_child_of_a_deleted_parent_is_caught_in_debug_builds() {
        lvgl(|lv| {
            let root = lv.screen();
            let parent = Obj::plain(root);
            let label = Obj::label(parent, "Mic on", Font::Body, TEXT);
            parent.delete();
            label.set_text("Mic off");
        });
    }

    #[test]
    fn rows_centre_their_children() {
        lvgl(|lv| {
            let root = lv.screen();
            let row = Obj::plain(root).rect(Rect::new(100, 100, 200, 40)).row(10);
            let first = Obj::plain(row).size(20, 20);
            let second = Obj::plain(row).size(20, 20);
            row.update_layout();
            assert_eq!((first.coords().x1, second.coords().x1), (175, 205));
            assert_eq!(first.coords().y1, 110);
        });
    }
}
