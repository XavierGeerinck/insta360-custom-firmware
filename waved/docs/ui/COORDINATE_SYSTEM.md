# UI Coordinate System

## TL;DR

**LVGL logical coordinates = user-visible coordinates.** Use them directly. No mental inversion needed.

## Why

The display is mounted 180° upside-down, so the hardware applies a 180° rotation to the framebuffer. Our `flush()` callback in `display.rs` applies another 180° rotation (vertical + horizontal flip + byte-swap). Two 180° rotations cancel out:

```
LVGL pixel (lx, ly)
  → flush writes to FB at (399−lx, 303−ly)   [180° rotation]
  → panel mounted 180° upside-down
  → user sees at (399−(399−lx), 303−(303−ly)) = (lx, ly)  ✓
```

So `lv_obj_align(obj, LV_ALIGN_TOP_LEFT, 0, 0)` places the object at the **physical top-left** as seen by the user.

## Original pspapp

The original pspapp used `lv_display_set_rotation(180)` in LVGL (full render mode). This achieves the same result: LVGL handles the 180° correction internally, so their logical coordinates also equal user-visible coordinates. That means **you can use the original decompiled x/y values directly** in our code without any coordinate transformation.

## Why We Do NOT Use `lv_display_set_rotation(180)`

In **PARTIAL render mode** (which we use for performance), LVGL transforms the dirty-area coordinates passed to `flush_cb` into rotated space, but the pixel buffer stays in logical order. Writing logical-order pixels at rotated coordinates scrambles colors. We avoid this by leaving LVGL rotation at 0 and doing the rotation manually in `flush()`.

## Screen Dimensions

| Constant | Value |
|----------|-------|
| Width    | 400 px |
| Height   | 304 px |
| Center   | (200, 152) |

## Circular Display Clipping

The physical display is circular (radius ≈ 190 px, center at (200, 152)). Content outside the circle is invisible. Safe positioning:

| y   | visible x range | usable width |
|-----|-----------------|--------------|
| 0   | ~86 – ~314      | ~228 px |
| 32  | ~67 – ~333      | ~266 px |
| 122 | ~14 – ~386      | ~372 px |
| 152 | ~10 – ~390      | ~380 px |
| 230 | ~27 – ~373      | ~346 px |
| 272 | ~67 – ~333      | ~266 px |
| 288 | ~77 – ~323      | ~246 px |
| 304 | ~86 – ~314      | ~228 px |

Formula: `x_half = sqrt(190² − (y − 152)²)`, visible x = `200 ± x_half`.
