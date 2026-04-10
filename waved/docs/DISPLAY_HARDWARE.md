# Display Hardware — fb_ili79400 Circular OLED

## Hardware

- **Driver**: `fb_ili79400` (confirmed via `/proc/fb`)
- **Framebuffer**: 400×304 px, 16 bpp RGB565, stride 800 bytes
- **Physical shape**: circular OLED, radius ≈190 px centred at **(200, 152)** in framebuffer coordinates
- The circle is larger than the framebuffer height — it is clipped top and bottom by the 304 px limit

## Visible Area

| y   | visible x range | width |
|-----|-----------------|-------|
| 0   | ~86 → ~314      | ~228  |
| 32  | ~67 → ~333      | ~266  |
| 152 | ~10 → ~390      | ~380  |
| 272 | ~67 → ~333      | ~266  |
| 304 | ~86 → ~314      | ~228  |

**Safe content zone**: keep content within x ≈ 90–310 at the top/bottom thirds; the full centre row (~380 px) is usable.

## flush() — Rotation + Byte-Swap

Two transforms must be applied together when writing LVGL pixels to `/dev/fb0`:

### 1. 180° rotation

The hardware is mounted upside-down. There is no HW rotation support (`/sys/class/graphics/fb0/rotate` = 0). Applied as vertical + horizontal flip in the flush callback.

### 2. Byte-swap per pixel

`fb_ili79400` reads RGB565 pixels MSB-first (big-endian). ARM is little-endian. Each u16 pixel must be byte-swapped before writing.

```rust
// Sequential destination writes (cache-friendly), reversed source reads.
let dst_x_start = SCREEN_W - x2 - 1;
for row in 0..h {
    let phys_y = SCREEN_H - 1 - (y1 + row);
    let src = color_p.add(row * w * 2) as *const u16;
    let dst = fb_ptr.add((phys_y * SCREEN_W + dst_x_start) * 2) as *mut u16;
    for col in 0..w {
        *dst.add(col) = (*src.add(w - 1 - col)).swap_bytes();
    }
}
```

### Why not `lv_display_set_rotation(180)`?

In PARTIAL render mode, LVGL transforms the `lv_area_t` coordinates passed to `flush_cb` into rotated space, but the pixel buffer remains in logical order. Writing logical-order pixels at physically-rotated coordinates scrambles colors. Manual rotation in `flush()` avoids this entirely.

## LVGL Configuration (`lv_conf.h`)

| Setting | Value | Reason |
|---------|-------|--------|
| `LV_COLOR_DEPTH` | `16` | RGB565 matches fb0 |
| `LV_USE_STDLIB_MALLOC` | `LV_STDLIB_CLIB` | System malloc required — decoded PNGs (~240 KB for 400×304) exceed the old 128 KB fixed pool |
| `LV_USE_LODEPNG` | `1` | Built-in PNG decoder |
| `LV_USE_FS_POSIX` | `1` | Maps `A:` drive letter to device root filesystem |
| `LV_FS_POSIX_LETTER` | `'A'` | Asset paths like `A:/etc/pspapp/lvgl/...` resolve to real device files |

Call `lv_fs_posix_init()` immediately after `lv_init()`.

`LV_COLOR_16_SWAP` was removed in LVGL 9. Byte-swapping is handled in `flush()` instead.

## Asset Paths on Device

Original pspapp assets at `/etc/pspapp/lvgl/` (referenced via the `A:` LVGL FS drive):

| Asset | Path | Size |
|-------|------|------|
| Mode button background | `A:/etc/pspapp/lvgl/icon/msc/bg_main_circule_btn.png` | 76×76 |
| REC button background | `A:/etc/pspapp/lvgl/icon/msc/bg_main_record_btn.png` | 180×76 |
| Mute button background | `A:/etc/pspapp/lvgl/icon/msc/bg_main_mic_btn.png` | 76×76 |
| Beamforming mode icons | `A:/etc/pspapp/lvgl/icon/beamforming/bf_mode_*.png` | 32×32 |
| Battery status icons | `A:/etc/pspapp/lvgl/icon/status/battery_*.png` | 20×20 |
| WiFi status icons | `A:/etc/pspapp/lvgl/icon/status/wifi_*.png` | 20×20 |
| Bluetooth status icons | `A:/etc/pspapp/lvgl/icon/status/bluetooth*.png` | 20×20 |
| Mic icon | `A:/etc/pspapp/lvgl/icon/volume/mic_on.png` | — |

> **Do NOT use `bg_main.png`** — it is a 400×304 rectangular image with a glowing teal sphere that looks like a "weird orb" on the circular display.

## Home Screen Layout

- **Background**: solid `CLR_BG` (no background image)
- **Status bar**: icons clustered around x=200 using `LV_ALIGN_CENTER` with offsets −46, −22, +2, +26, +52
- **NR label**: `CENTER` offset (0, −80) → y ≈ 72
- **Button row**: `CENTER` alignment, y_offset = −10 → button centres at y ≈ 142
  - Mode button: x_offset = −130
  - REC button:  x_offset = 0
  - Mute button: x_offset = +130

Use `LV_ALIGN_CENTER` (not `LV_ALIGN_BOTTOM_MID`) for the button row — `BOTTOM_MID` pushes buttons to y ≈ 216 where the circle narrows significantly.

## Deploy

```bash
cross build --release --target aarch64-unknown-linux-gnu --features "ui" -p waved
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "kill \$(ps | awk '/\/data\/waved\/waved/{print \$1}' | head -1)"
# init.d script (PID 329) auto-restarts waved within ~3 s
```
