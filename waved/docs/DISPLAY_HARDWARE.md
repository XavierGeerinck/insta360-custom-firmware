# Display Hardware — fb_ili79400 Circular LCD

## Hardware

- **Driver**: `fb_ili79400` (confirmed via `/proc/fb`)
- **Framebuffer**: 400×304 px, 16 bpp RGB565, stride 800 bytes
- **Physical shape**: circular LCD with a PWM backlight (`/sys/class/leds/lcd-backlight`, 0–255)
- **Panel type confirmed on the device (2026-10-05):** writing 0 to `/sys/class/leds/lcd-backlight/brightness` blanks the image completely. It is an LCD (the device tree lists an `ilitek,ili79400` TDDI controller and a `pwm-leds` `lcd-backlight`), so there is no burn-in and brightness means backlight level

## Visible Area (measured on the device, 2026-10-05)

The visible image is a **circle of radius ≈198 px centred at ≈(200, 106)** in framebuffer coordinates, **cut off flat at the top by framebuffer row 0** (the area above it is glass under the stand's notch, with no pixels). Left and right, the bezel sits just outside the framebuffer edges at the widest row; at the bottom the circle ends at about y 304.

Measured with calibration patterns written straight to `/dev/fb0` (waved stopped) and straight-down photos: a circle of r 196 around (200, 105) hugs the bezel all the way round the curve, r 202 and r 208 fall outside it; the y = 0 row is visible from about x 20 to 380.

> The earlier assumption here — a circle of r 190 centred at (200, 152) — was wrong by about 46 px vertically. Layouts built on it sit too low and leave the top third unused.

| y   | visible x range | width |
|-----|-----------------|-------|
| 0   | 33 → 367 | 335 |
| 32  | 16 → 384 | 369 |
| 64  | 7 → 393 | 387 |
| 106 | 2 → 398 | 397 |
| 160 | 10 → 390 | 381 |
| 220 | 38 → 362 | 325 |
| 260 | 76 → 324 | 249 |
| 290 | 127 → 273 | 147 |
| 303 | 180 → 220 | 41 |

The table is computed from the measured circle (r 198 at (200, 106)), clipped to the framebuffer. Keep tap targets ~8 px inside the curve.

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
| `LV_USE_LODEPNG`, `LV_USE_FS_POSIX` | `0` | No PNG assets since the milestone 2 runtime switch: icons come from the kit icon font |
| `LV_FONT_DEFAULT` | `&font_body_15` | The kit body font; Montserrat is not built |

`LV_COLOR_16_SWAP` was removed in LVGL 9. Byte-swapping is handled in `flush()` instead.

These stock pspapp assets are no longer used by waved (milestone 2); the table stays as a record of the stock firmware.

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

## Screen layout

The UI is the milestone-2 kit (`crates/waved-ui/src/kit/README.md`) on the
approved mockups (`../docs/mockups/`). Geometry tokens live in
`kit/tokens.rs` and follow the measured visible area above. The stock
firmware's own layout is kept for reference in `docs/ui/HOME_SCREEN.md`.

## Deploy

`/data` is ~440 MB and nearly full; the stripped binary is ~63 MB. `adb push`
over the running binary deletes the old file before it fails for space, leaving
no binary. Deploy through `/tmp` and swap with the watchdog stopped:

```bash
S=$(mktemp -d)
WAVED_BUILD_ID=$(git rev-parse --short HEAD) \
  cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
$(rustc --print sysroot)/lib/rustlib/aarch64-apple-darwin/bin/llvm-objcopy --strip-all \
  target/aarch64-unknown-linux-gnu/release/waved "$S/waved"
adb push "$S/waved" /tmp/waved.new && adb shell md5sum /tmp/waved.new   # compare with md5 -q "$S/waved"
adb shell 'kill $(ps | awk "/S60waved start/ && !/awk/ {print \$1}"); sleep 1; killall waved; sleep 3; \
  rm /data/waved/waved && cp /tmp/waved.new /data/waved/waved && chmod 755 /data/waved/waved; \
  start-stop-daemon -S -b -m -p /tmp/S60waved.pid -x /bin/sh -- /etc/init.d/S60waved start'
```

- If `~/.cargo/config.toml` sets `rustc-wrapper = "sccache"`, run `cross` with a copy
  of `Cross.toml` (`CROSS_CONFIG=...`) that adds `"RUSTC_WRAPPER="` to
  `[build.env] passthrough`. Don't use `CROSS_BUILD_ENV_PASSTHROUGH`: it replaces the
  list and drops the `CFLAGS ... -D__ARM_ARCH=8` that `ring` needs.
- Check `adb shell df -h /data` first. The stopped binary's space is only freed once
  waved exits, which is why the swap happens with the watchdog and waved stopped.
- `start-stop-daemon` needs `-m -p <pidfile>`; without it busybox matches any running
  `/bin/sh` and refuses to start.
- The framebuffer can be captured for checks with `adb shell 'cat /dev/fb0 | head -c 243200'`
  (RGB565 big-endian, rotated 180°; `scripts/png-to-fb.py` is the inverse).
