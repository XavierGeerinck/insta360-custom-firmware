#ifndef LV_CONF_H
#define LV_CONF_H

/*====================
   COLOR SETTINGS
 *====================*/

/* RGB565 — matches /dev/fb0 */
#define LV_COLOR_DEPTH 16

/*=========================
   STDLIB WRAPPER SETTINGS
 *=========================*/

/* Use system malloc/free — no fixed heap cap; PNG images need ~500 KiB */
#define LV_USE_STDLIB_MALLOC    LV_STDLIB_CLIB
#define LV_USE_STDLIB_STRING    LV_STDLIB_CLIB
#define LV_USE_STDLIB_SPRINTF   LV_STDLIB_CLIB

/*====================
   HAL SETTINGS
 *====================*/

/* ~30 fps */
#define LV_DEF_REFR_PERIOD  33

/* 1.82" diagonal, 400x304px */
#define LV_DPI_DEF 276

/*=================
 * OPERATING SYSTEM
 *=================*/

#define LV_USE_OS   LV_OS_NONE

/*========================
 * RENDERING CONFIGURATION
 *========================*/

#define LV_USE_DRAW_SW 1

/* Amlogic A5 is ARM, no NEON optimisation needed for now */
#define LV_USE_DRAW_SW_ASM LV_DRAW_SW_ASM_NONE

/*=======================
 * FEATURE CONFIGURATION
 *=======================*/

#define LV_USE_LOG 1
#define LV_LOG_LEVEL LV_LOG_LEVEL_WARN
#define LV_LOG_PRINTF 1

#define LV_USE_ASSERT_NULL          1
#define LV_USE_ASSERT_MALLOC        1
#define LV_USE_ASSERT_STYLE         0
#define LV_USE_ASSERT_MEM_INTEGRITY 0
#define LV_USE_ASSERT_OBJ           0

/*==================
 *   FONT USAGE
 *===================*/

/* Kit fonts (src/kit/fonts, built by fonts/build_fonts.sh). Every kit label
 * sets its font; the default only covers LVGL's own fallbacks. */
#define LV_FONT_CUSTOM_DECLARE LV_FONT_DECLARE(font_body_15)
#define LV_FONT_DEFAULT &font_body_15

/* Dark default theme: unstyled widgets render dark instead of white discs. */
#define LV_USE_THEME_DEFAULT 1
#define LV_THEME_DEFAULT_DARK 1
#define LV_USE_FREETYPE 0

/*==================
 * WIDGETS
 *================*/

#define LV_USE_ARC        1
#define LV_USE_BAR        1
#define LV_USE_BUTTON     1
#define LV_USE_BUTTONMATRIX 1
#define LV_USE_CANVAS     1
#define LV_USE_IMAGE      1
#define LV_USE_LABEL      1
#define LV_USE_SLIDER     1
#define LV_USE_SWITCH     1
#define LV_USE_LIST       1
#define LV_USE_SPINNER    1
#define LV_USE_MSGBOX     1

/*==================
 * OTHERS
 *===================*/

#define LV_USE_LODEPNG 0  /* no PNG assets: icons are the kit icon font */
#define LV_USE_QRCODE  1  /* Wi-Fi setup QR code */
#define LV_USE_FS_POSIX 0 /* no asset files are read */

#endif /* LV_CONF_H */
