//! Color and font constants. All colors are RGB565 packed u32 for LVGL lv_color32_t.

// Primary palette
pub const CLR_BG: u32         = 0x000000; // black
pub const CLR_SURFACE: u32    = 0x0d1f1a; // dark teal surface
pub const CLR_ACCENT: u32     = 0x00c49a; // teal
pub const CLR_RED: u32        = 0xe74c3c; // recording red
pub const CLR_WHITE: u32      = 0xffffff;
pub const CLR_TEXT_DIM: u32   = 0x888888;
pub const CLR_TEXT: u32       = 0xcccccc;

/// Convert 0xRRGGBB u32 to lv_color_t (RGB565).
pub fn rgb(hex: u32) -> crate::bindings::lv_color_t {
    let r = ((hex >> 16) & 0xff) as u8;
    let g = ((hex >> 8) & 0xff) as u8;
    let b = (hex & 0xff) as u8;
    unsafe { crate::bindings::lv_color_make(r, g, b) }
}
