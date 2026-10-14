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

## Boot screen

The Insta360 image at power-on is drawn by the bootloader and stays on the
panel until something draws over it (`fbtft` keeps the panel's contents; the
kernel has no logo, `CONFIG_LOGO` is unset). It cannot be replaced at the
source: the `logo` partition (`/dev/logo`, `mmcblk0p5`) is all zeros, the
bootloader's `load_bmp_logo` reads a missing `odm_ext` partition or that empty
one and `init_display` never calls it, and the image lives in the bootloader
itself, which is encrypted (entropy 8.0 bits/byte over 0xC0000–0x180000) and
verified at boot (`androidboot.verifiedbootstate=green`).

`deploy/S03splash` covers it instead: right after `S02overlayfs` mounts the
overlay (`/etc/init.d` and `/data/waved` both live on `/data`) it writes
`/data/waved/splash.fb` (waved's start-up ring without the version line,
built by `scripts/make-splash.py`) to `/dev/fb0`. `rcS` sets the backlight to
255 before any script runs. Measured 2026-10-12: userspace starts at 0.97 s,
the splash before `S50` at 2.24 s, waved's UI at ~10 s, so the Insta360 image
now shows for the bootloader's time plus ~1.5 s. Install: push both files and
`chmod 755 /etc/init.d/S03splash`.

## Deploy

`/data` is ~440 MB and nearly full; the stripped binary is ~63 MB. `adb push`
over the running binary deletes the old file before it fails for space, leaving
no binary. Deploy through `/tmp` and swap with the watchdog stopped.

`deploy/install.sh` does all of it, for a first install and for an update: after the
`cross build` below, run it from anywhere. In this order, which is the safety net:

1. Checks, before anything changes: the Wave answers, no over-the-air update is on trial
   (`/data/waved/update/trial`, `docs/UPDATES.md`), `/proc/mounts` shows S02overlayfs'
   layout (release mode: `/etc` and `/usr` overlays with `upperdir=/data/overlay/upper/etc`
   and `…/usr`; debug mode: one overlay on `/` with `upperdir=/data/overlay/upper`), and
   `/data` has room (`df -Pk`) for the stripped binary plus 20 MB, counting the installed
   binary's space as free.
2. Strips the binary (`llvm-objcopy` from `rustup component add llvm-tools`, or
   `OBJCOPY=...`) and pushes it, `S60waved`, `S03splash`, `rsyslog.conf`, `splash.fb`,
   `iptables.rules`, `scripts/patch-stock-usbgadget.sh` and (if the Wave has none)
   `config/default.toml` to `/tmp`, checking each one's md5.
3. Copies the Wave's `config.toml` to `/data/waved/config.toml.before-<UTC time>` (mode
   600); it never replaces it.
4. Runs the USB gadget patch (ADB at boot, `docs/USB_STORAGE.md`) and checks with awk that
   no `adbd_enable` in `S89usbgadget` is commented out and at least one is live inside
   `start_usbgadget()`. Nothing is stopped before this passes: a Wave without the stock app
   and without ADB can't be reached again.
5. Stops the stock app and the watchdog, waits for waved to exit (`killall -9` at 30 s,
   gives up at 40 s), swaps the binary (the old one waits in `/tmp/waved.old`, its md5 in
   `/tmp/waved.old.md5`, and goes back if the new one does not fit). The swap ignores
   SIGHUP/INT/TERM/PIPE: adbd SIGHUPs a session's processes when it closes, and a swap cut
   half-way through the 63 MB copy would leave no binary.
6. Installs `S60waved`, `S03splash`, `rsyslog.conf`, `splash.fb` and `iptables.rules`,
   each through a dot-named copy and a rename; the default `config.toml` if there was none.
7. Last, the stubs that keep the stock app and its telemetry off (`S60pspapp_daemon`,
   `libapplogrs.so`). One already in `/data/overlay/upper` is kept as it is, whatever it
   holds (on some Waves `libapplogrs.so` there is the compiled no-op
   `scripts/libapplogrs.so`, 8352 bytes). A missing one is written through the `/etc` and
   `/usr` mounts and read back (`#!/bin/sh` and a newline; 0 or 1 byte). Then both must be
   in `/data/overlay/upper`: the copies there are what replace the stock files at boot.
8. An update starts the watchdog again, unless one runs already (rcS starts it without a
   pidfile; two would fight over the devices), and ends with an `adb reboot` hint when the
   gadget patch, `S03splash`, `splash.fb` or `rsyslog.conf` changed (by md5; they take
   effect at boot). A first
   install (no `/data/waved/waved`) asks for `adb reboot`. A failure, Ctrl-C, closed
   terminal (SIGHUP) or closed output pipe after step 5 began does the same; on an update
   it first puts `/tmp/waved.old` back if `/data/waved/waved` is missing or matches neither
   the new md5 nor the old one (it waits for a swap still running). The old md5 counts only
   if this run's swap wrote it: the stop step deletes one an earlier update in the same boot
   left, and the swap writes it only after checking the copy.

Overlay files are written through the mounts, never into `/data/overlay/upper` directly
(`deploy/revert.sh` deletes there, on purpose). It tests the Wave's files on the Wave and
checks every step's output, because `adb shell`'s exit status is not reliable. Host-tested
against a fake `adb` (`deploy/tests/test-deploy-scripts.sh`,
`crates/waved-update/src/install_script.rs`).

By hand, the same steps for the binary alone:

```bash
S=$(mktemp -d)
WAVED_BUILD_ID=$(git rev-parse --short HEAD) \
  cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
$(rustc --print sysroot)/lib/rustlib/aarch64-apple-darwin/bin/llvm-objcopy --strip-all \
  target/aarch64-unknown-linux-gnu/release/waved "$S/waved"
adb push "$S/waved" /tmp/waved.new && adb shell md5sum /tmp/waved.new   # compare with md5 -q "$S/waved"
adb shell 'kill $(ps | awk "/S60waved start/ && !/awk/ {print \$1}"); sleep 1; killall waved; \
  while pidof waved >/dev/null; do sleep 1; done; \
  rm /data/waved/waved && cp /tmp/waved.new /data/waved/waved && chmod 755 /data/waved/waved; \
  start-stop-daemon -S -b -m -p /tmp/S60waved.pid -x /bin/sh -- /etc/init.d/S60waved start'
```

- If `~/.cargo/config.toml` sets `rustc-wrapper = "sccache"`, run `cross` with a copy
  of `Cross.toml` (`CROSS_CONFIG=...`) that adds `"RUSTC_WRAPPER="` to
  `[build.env] passthrough`. Don't use `CROSS_BUILD_ENV_PASSTHROUGH`: it replaces the
  list and drops the `CFLAGS ... -D__ARM_ARCH=8` that `ring` needs.
- Check `adb shell df -h /data` first. The stopped binary's space is only freed once
  waved exits, which is why the swap happens with the watchdog and waved stopped.
  Shutdown takes ~2 s (the motor retracts first): if the `cp` starts before waved
  has exited it fails with "No space left on device" over a partial file. The new
  binary is still in `/tmp`, so wait for waved to exit and run the swap again.
- The init script SIGTERMs the first waved 5 s after it starts (display
  workaround) and rotates `waved.log` to `waved.log.1` past 2 MB at each launch.
- Stock `rsyslogd` logged everything to `/opt/logs/psp.log`, which is on `/data`
  (overlay) and which nothing has rotated since the stock app stopped running. It
  reached 273 MB, mostly Wi-Fi driver (`atbm_log`) scan lines, and on 2026-10-08 it
  filled `/data`. Waved then exited at start-up without a word, because even its log
  write failed. `deploy/rsyslog.conf` drops the `atbm_log` lines and logs to
  `/tmp/psp.log`. `install.sh` installs it as `/etc/rsyslog.conf` (it lands in the
  overlay and survives reboots); it takes effect at the next boot, or at once with
  `/etc/init.d/S11rsyslogd restart`.
  If waved won't start, check `df -h /data` first.
- `start-stop-daemon` needs `-m -p <pidfile>`; without it busybox matches any running
  `/bin/sh` and refuses to start.
- The framebuffer can be captured for checks with `adb shell 'cat /dev/fb0 | head -c 243200'`
  (RGB565 big-endian, rotated 180°; `scripts/png-to-fb.py` is the inverse).
