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

#define LV_USE_STDLIB_MALLOC    LV_STDLIB_BUILTIN
#define LV_USE_STDLIB_STRING    LV_STDLIB_BUILTIN
#define LV_USE_STDLIB_SPRINTF   LV_STDLIB_BUILTIN

/* 128 KiB heap for LVGL */
#define LV_MEM_SIZE (128 * 1024U)
#define LV_MEM_POOL_EXPAND_SIZE 0
#define LV_MEM_ADR 0

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

#define LV_FONT_MONTSERRAT_12 1
#define LV_FONT_MONTSERRAT_14 1
#define LV_FONT_MONTSERRAT_16 1
#define LV_FONT_MONTSERRAT_20 1
#define LV_FONT_MONTSERRAT_24 1
#define LV_FONT_DEFAULT &lv_font_montserrat_16
#define LV_USE_FREETYPE 0

/*==================
 * WIDGETS
 *================*/

#define LV_USE_ARC        1
#define LV_USE_BAR        1
#define LV_USE_BUTTON     1
#define LV_USE_BUTTONMATRIX 1
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

#define LV_USE_PNG 1
#define LV_USE_FS_STDIO 0

#endif /* LV_CONF_H */
