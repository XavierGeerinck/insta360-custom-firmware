# `waved-ui` LVGL Touchscreen UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the LVGL 9.x touchscreen UI for the Insta360 Wave (400x304px, 1.82" capacitive touch), providing 17 screens with status bar, toast notifications, and JSON localization, driven by a command channel from the headless MVP modules.

**Architecture:** LVGL 9 is compiled from C source via `build.rs` (targeting aarch64) and accessed from Rust through raw `bindgen` FFI. A dedicated OS thread runs the single-threaded LVGL event loop, polling both LVGL timers and a `tokio::sync::mpsc` command channel for state updates. User actions (button taps, slider changes) are sent back to the rest of `waved` via the `commands` broadcast channel on the `StateBus`. Screen navigation is a state machine — each screen is a separate module under `crates/waved-ui/src/screens/`.

**Tech Stack:** LVGL 9.2 (C, MIT), bindgen, cc (build.rs), Rust FFI, tokio::sync::mpsc, framebuffer `/dev/fb0`, evdev `/dev/input/event0`, serde_json (localization)

**Depends on:** waved Phase 1 MVP plan (waved-state, waved-config crates must exist)

---

## Errata (from plan review — apply these fixes during implementation)

The code in the tasks below is directionally correct but has compilation and design issues that must be fixed during implementation. Each implementing agent MUST apply these fixes:

### Critical (will not compile without these)

1. **LVGL C compile flags.** The `build.rs` uses `cc::Build`. If any LVGL source file fails to compile due to missing config, ensure `lv_conf.h` is on the include path via `.include("lvgl-src")` AND `.include("lvgl-src/src")`. LVGL 9.2 expects `lv_conf.h` to be findable from the parent directory of `lvgl/`.

2. **Bindgen allowlist may be incomplete.** If you get "undefined reference" errors for LVGL symbols, expand the allowlist in `build.rs` to include the missing `lv_*` functions. The allowlist in Task 1 covers core functions but not all of them — add as needed.

3. **`lv_display_set_flush_cb` callback signature.** The flush callback must be `extern "C" fn(*mut lv_display_t, *const lv_area_t, *mut u8)`. LVGL 9 changed from `lv_disp_drv_t` (v8) to `lv_display_t` (v9). Ensure all FFI callback signatures match LVGL 9.2 headers exactly.

4. **Thread safety of mpsc channel.** The `mpsc::Receiver` must be polled with `try_recv()` (non-blocking) inside the LVGL tick loop, NOT `.recv().await` — the LVGL thread is not an async context. Use `std::sync::mpsc` or `tokio::sync::mpsc::Receiver` with `try_recv()`.

### Important (spec requirements not met)

5. **Framebuffer pixel format.** The stock display uses RGB565 (16-bit). Verify with `fbset` on device. If it is RGB565, set `LV_COLOR_DEPTH=16` in `lv_conf.h`. The plan assumes RGB565 — adjust if device inspection reveals otherwise.

6. **Touch input coordinate mapping.** The evdev touch coordinates from `/dev/input/event0` may not map 1:1 to display pixels. Read `ABS_X` and `ABS_Y` min/max from evdev and scale to 400x304. The code in Task 3 does this but verify actual evdev axis ranges on device.

7. **LVGL tick source.** LVGL 9 requires `lv_tick_inc(ms)` called periodically. The main loop in Task 4 calls it with elapsed time, but if the loop blocks on something, ticks will drift. Ensure `lv_tick_inc` is called with real elapsed milliseconds.

8. **JSON localization file must exist.** If `/data/waved/lang/en.json` does not exist at startup, the UI should fall back to hardcoded English strings — not panic. Task 5 implements this but ensure every `tr()` call handles the fallback.

9. **Screen cleanup on navigation.** When navigating away from a screen, call `lv_obj_delete()` on the old screen's root object to free LVGL memory. The `ScreenManager` in Task 6 handles this, but each screen's `destroy()` must be called before `create()` of the next screen.

10. **Status bar must persist across screen changes.** The status bar is created once on the default LVGL layer (`lv_layer_top()`) and never destroyed. Screen content is created/destroyed on `lv_screen_active()` underneath it.

---

## File Structure

```
waved/crates/waved-ui/
├── Cargo.toml
├── build.rs                              # Compile LVGL 9 from source, run bindgen
├── lv_conf.h                             # LVGL configuration (400x304, RGB565, no GPU)
├── src/
│   ├── lib.rs                            # UiService::new(), run(), public API
│   ├── bindings.rs                       # Generated (build.rs) — #[allow(warnings)] include
│   ├── display.rs                        # Framebuffer flush, /dev/fb0 setup
│   ├── input.rs                          # Evdev touch driver, /dev/input/event0
│   ├── theme.rs                          # Colors, fonts, sizes, spacing constants
│   ├── i18n.rs                           # JSON string table loader, tr() function
│   ├── status_bar.rs                     # Persistent status bar (battery, WiFi, BT, mic, spk, USB)
│   ├── toast.rs                          # Toast notification overlay
│   ├── nav.rs                            # Screen enum, ScreenManager state machine
│   ├── widgets.rs                        # Reusable LVGL widget helpers (list_item, slider, toggle, etc.)
│   ├── command.rs                        # UiCommand enum + UiAction enum
│   └── screens/
│       ├── mod.rs                        # Screen trait + re-exports
│       ├── home.rs                       # Home screen (ID 10)
│       ├── recording.rs                  # Recording screen (ID 12)
│       ├── files.rs                      # File list screen (ID 19)
│       ├── audio_mode.rs                 # Audio mode selection (ID 77)
│       ├── wifi.rs                       # WiFi scan/connect (ID 44)
│       ├── wifi_info.rs                  # WiFi network details (ID 45)
│       ├── bluetooth.rs                  # Bluetooth settings (ID 15)
│       ├── settings.rs                   # Settings menu (ID 11)
│       ├── volume.rs                     # Speaker + mic volume (ID 13/28)
│       ├── height.rs                     # Motor height control (ID 22)
│       ├── camera.rs                     # Camera auto/manual (ID 41)
│       ├── camera_ptz.rs                 # Manual PTZ controls (ID 43)
│       ├── api_config.rs                 # API endpoint + token (keyboard entry)
│       ├── about.rs                      # Device info (ID 30)
│       ├── password.rs                   # Device password (ID 17)
│       ├── ota.rs                        # OTA update (ID 46)
│       ├── power_off.rs                  # Power off confirmation (ID 55)
│       └── loading.rs                    # Generic loading spinner (ID 8)
├── lang/
│   └── en.json                           # English string table (shipped default)
└── lvgl-src/                             # LVGL 9.2 source (git submodule or vendored)
    ├── lvgl/
    │   ├── src/
    │   ├── lv_conf_template.h
    │   └── ...
    └── lv_conf.h -> ../../lv_conf.h      # Symlink to our config
```

---

### Task 1: LVGL 9 C Build + Bindgen FFI Crate

**Files:**
- Create: `waved/crates/waved-ui/Cargo.toml`
- Create: `waved/crates/waved-ui/build.rs`
- Create: `waved/crates/waved-ui/lv_conf.h`
- Create: `waved/crates/waved-ui/src/lib.rs`
- Create: `waved/crates/waved-ui/src/bindings.rs`

- [ ] **Step 1: Vendor LVGL 9.2 source**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved/crates
mkdir -p waved-ui/src waved-ui/lvgl-src
cd waved-ui/lvgl-src
git clone --depth 1 --branch v9.2.0 https://github.com/lvgl/lvgl.git
```

- [ ] **Step 2: Write lv_conf.h**

Create `waved/crates/waved-ui/lv_conf.h`:
```c
#ifndef LV_CONF_H
#define LV_CONF_H

/* clang-format off */

/*=======================
   COLOR SETTINGS
 *=======================*/
/* Color depth: 16 = RGB565 (matches Insta360 Wave framebuffer) */
#define LV_COLOR_DEPTH 16

/* Swap the 2 bytes of RGB565 color for big-endian displays.
 * Amlogic A5 is little-endian, set 0. */
#define LV_COLOR_16_SWAP 0

/*=======================
   MEMORY SETTINGS
 *=======================*/
/* Size of the memory used by `lv_malloc()` in bytes (>= 2kB) */
#define LV_MEM_SIZE (128 * 1024)

/* Use stdlib malloc/free instead of LVGL's built-in pool */
#define LV_MEM_CUSTOM 0

/*=======================
   HAL SETTINGS
 *=======================*/
/* Default display refresh period in ms */
#define LV_DEF_REFR_PERIOD 33  /* ~30 fps */

/* Input device read period in ms */
#define LV_DEF_INPUT_READ_PERIOD 30

/* Default DPI. Insta360 Wave: 1.82" diagonal, 400x304px
 * diagonal_px = sqrt(400^2 + 304^2) = 502.4
 * DPI = 502.4 / 1.82 = 276 */
#define LV_DPI_DEF 276

/*=======================
   DISPLAY SETTINGS
 *=======================*/
#define LV_HOR_RES_MAX 400
#define LV_VER_RES_MAX 304

/*=======================
   FEATURE CONFIG
 *=======================*/
/* Drawing engine */
#define LV_USE_DRAW_SW 1
#define LV_USE_DRAW_SW_ASM LV_DRAW_SW_ASM_NONE

/* GPU — none on this device */
#define LV_USE_GPU_STM32_DMA2D 0
#define LV_USE_GPU_NXP_PXP 0
#define LV_USE_GPU_NXP_VG_LITE 0

/* Logging */
#define LV_USE_LOG 1
#define LV_LOG_LEVEL LV_LOG_LEVEL_WARN
#define LV_LOG_PRINTF 1

/* Asserts */
#define LV_USE_ASSERT_NULL 1
#define LV_USE_ASSERT_MALLOC 1
#define LV_USE_ASSERT_STYLE 0
#define LV_USE_ASSERT_MEM_INTEGRITY 0
#define LV_USE_ASSERT_OBJ 0

/* Others */
#define LV_USE_PERF_MONITOR 0
#define LV_USE_MEM_MONITOR 0

/*=======================
   FONT CONFIG
 *=======================*/
/* Built-in fonts */
#define LV_FONT_MONTSERRAT_14 1
#define LV_FONT_MONTSERRAT_16 1
#define LV_FONT_MONTSERRAT_20 1
#define LV_FONT_MONTSERRAT_24 1
#define LV_FONT_MONTSERRAT_28 1
#define LV_FONT_MONTSERRAT_12 1

/* Default font */
#define LV_FONT_DEFAULT &lv_font_montserrat_16

/* Enable FreeType for custom fonts if needed later */
#define LV_USE_FREETYPE 0

/*=======================
   WIDGET CONFIG
 *=======================*/
#define LV_USE_ARC        1
#define LV_USE_BAR        1
#define LV_USE_BTN        1
#define LV_USE_BTNMATRIX  1
#define LV_USE_CANVAS     0
#define LV_USE_CHECKBOX   1
#define LV_USE_DROPDOWN   1
#define LV_USE_IMAGE      1
#define LV_USE_LABEL      1
#define LV_USE_LINE       1
#define LV_USE_ROLLER     1
#define LV_USE_SCALE      0
#define LV_USE_SLIDER     1
#define LV_USE_SWITCH     1
#define LV_USE_TEXTAREA   1
#define LV_USE_TABLE      0
#define LV_USE_KEYBOARD   1
#define LV_USE_LIST       1
#define LV_USE_MENU       0
#define LV_USE_MSGBOX     1
#define LV_USE_SPINNER    1
#define LV_USE_TABVIEW    1
#define LV_USE_TILEVIEW   0
#define LV_USE_WIN        0

/* Animations */
#define LV_USE_ANIM 1

/* File system (not used — we load assets from compiled-in data) */
#define LV_USE_FS_STDIO 0
#define LV_USE_FS_POSIX 0

/* Image decoders */
#define LV_USE_PNG 1
#define LV_USE_BMP 0
#define LV_USE_SJPG 0
#define LV_USE_GIF 0

/* OS abstraction - none, we handle threading ourselves */
#define LV_USE_OS LV_OS_NONE

#endif /* LV_CONF_H */
```

- [ ] **Step 3: Write build.rs**

Create `waved/crates/waved-ui/build.rs`:
```rust
use std::env;
use std::path::PathBuf;

fn main() {
    let lvgl_src = PathBuf::from("lvgl-src/lvgl/src");
    let lvgl_root = PathBuf::from("lvgl-src/lvgl");
    let conf_dir = PathBuf::from(".");

    // Collect all .c files from LVGL src recursively
    let c_files = collect_c_files(&lvgl_src);

    println!("cargo:warning=Compiling {} LVGL source files", c_files.len());

    cc::Build::new()
        .files(&c_files)
        .include(&conf_dir)          // lv_conf.h lives here
        .include("lvgl-src")         // so #include "lvgl/lvgl.h" works
        .include(&lvgl_root)         // so #include "src/..." works
        .include(&lvgl_src)          // so internal includes work
        .define("LV_CONF_INCLUDE_SIMPLE", None)
        .flag("-w")                  // suppress C warnings
        .opt_level(2)
        .compile("lvgl");

    // Generate Rust bindings via bindgen
    let bindings = bindgen::Builder::default()
        .header("lvgl-src/lvgl/lvgl.h")
        .clang_arg(format!("-I{}", conf_dir.display()))
        .clang_arg("-Ilvgl-src")
        .clang_arg(format!("-I{}", lvgl_root.display()))
        .clang_arg(format!("-I{}", lvgl_src.display()))
        .clang_arg("-DLV_CONF_INCLUDE_SIMPLE")
        .allowlist_function("lv_.*")
        .allowlist_type("lv_.*")
        .allowlist_var("LV_.*")
        .derive_debug(true)
        .derive_default(true)
        .generate()
        .expect("Unable to generate LVGL bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("lvgl_bindings.rs"))
        .expect("Couldn't write bindings");

    println!("cargo:rerun-if-changed=lv_conf.h");
    println!("cargo:rerun-if-changed=lvgl-src/lvgl/src");
    println!("cargo:rerun-if-changed=build.rs");
}

fn collect_c_files(dir: &PathBuf) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_c_files(&path));
            } else if path.extension().map_or(false, |e| e == "c") {
                files.push(path);
            }
        }
    }
    files
}
```

- [ ] **Step 4: Write Cargo.toml**

Create `waved/crates/waved-ui/Cargo.toml`:
```toml
[package]
name = "waved-ui"
version.workspace = true
edition.workspace = true

[dependencies]
tokio = { workspace = true, features = ["sync", "rt"] }
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
thiserror.workspace = true
uuid.workspace = true
waved-state = { path = "../waved-state" }
waved-config = { path = "../waved-config" }
nix = { workspace = true, features = ["ioctl", "mman"] }
libc = "0.2"

[build-dependencies]
cc = "1"
bindgen = "0.71"
```

- [ ] **Step 5: Write bindings.rs wrapper**

Create `waved/crates/waved-ui/src/bindings.rs`:
```rust
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::all)]

include!(concat!(env!("OUT_DIR"), "/lvgl_bindings.rs"));
```

- [ ] **Step 6: Write minimal lib.rs**

Create `waved/crates/waved-ui/src/lib.rs`:
```rust
pub mod bindings;
pub mod command;
pub mod display;
pub mod i18n;
pub mod input;
pub mod nav;
pub mod screens;
pub mod status_bar;
pub mod theme;
pub mod toast;
pub mod widgets;

use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};
use waved_config::WavedConfig;
use waved_state::StateBus;

use command::UiCommand;

pub struct UiService {
    cmd_tx: mpsc::Sender<UiCommand>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl UiService {
    /// Create the UI service. Returns the service handle and a sender
    /// that other modules use to push state updates to the UI.
    pub fn new(bus: Arc<StateBus>, config: WavedConfig) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(256);

        let handle = std::thread::Builder::new()
            .name("ui-lvgl".into())
            .spawn(move || {
                if let Err(e) = ui_thread_main(bus, config, cmd_rx) {
                    error!("UI thread exited with error: {e}");
                }
            })
            .expect("failed to spawn UI thread");

        Self {
            cmd_tx,
            thread_handle: Some(handle),
        }
    }

    /// Get a clone of the command sender for other modules to use.
    pub fn cmd_sender(&self) -> mpsc::Sender<UiCommand> {
        self.cmd_tx.clone()
    }

    /// Block until the UI thread exits.
    pub fn join(mut self) {
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

fn ui_thread_main(
    bus: Arc<StateBus>,
    config: WavedConfig,
    cmd_rx: mpsc::Receiver<UiCommand>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("UI thread starting");

    // Initialize LVGL
    unsafe { bindings::lv_init(); }

    // Set up display driver (framebuffer)
    let _display = display::FramebufferDisplay::init()?;

    // Set up input driver (touchscreen)
    let _input = input::TouchInput::init()?;

    // Load localization
    let lang = &config.device.language;
    let i18n = i18n::I18n::load(lang);

    // Apply theme
    theme::apply_theme();

    // Create persistent status bar on top layer
    let mut status = status_bar::StatusBar::create(&i18n);

    // Create toast manager
    let mut toasts = toast::ToastManager::new();

    // Create screen manager and show home screen
    let mut screen_mgr = nav::ScreenManager::new(bus.clone(), i18n, config);
    screen_mgr.navigate(nav::Screen::Home);

    // Main LVGL event loop
    info!("UI entering main loop");
    let mut last_tick = std::time::Instant::now();
    let mut cmd_rx = cmd_rx; // move into loop scope

    loop {
        // 1. Advance LVGL tick
        let now = std::time::Instant::now();
        let elapsed_ms = now.duration_since(last_tick).as_millis() as u32;
        last_tick = now;
        unsafe { bindings::lv_tick_inc(elapsed_ms); }

        // 2. Process pending UI commands (non-blocking)
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                UiCommand::Shutdown => {
                    info!("UI received shutdown command");
                    return Ok(());
                }
                _ => {
                    screen_mgr.handle_command(&cmd);
                    status.handle_command(&cmd);
                    toasts.handle_command(&cmd);
                }
            }
        }

        // 3. Let LVGL process its internals (draw, animations, input)
        let next_ms = unsafe { bindings::lv_timer_handler() };

        // 4. Sleep until next LVGL timer needs attention (min 1ms, max 33ms)
        let sleep_ms = next_ms.clamp(1, 33);
        std::thread::sleep(std::time::Duration::from_millis(sleep_ms as u64));
    }
}
```

- [ ] **Step 7: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```
Expected: compiles (with stub modules — empty files created in subsequent tasks will need placeholder content).

- [ ] **Step 8: Commit**

```bash
git add waved/crates/waved-ui/
git commit -m "feat(waved-ui): scaffold LVGL 9 crate with C build, bindgen FFI, and main loop"
```

---

### Task 2: Framebuffer Display Driver

**Files:**
- Create: `waved/crates/waved-ui/src/display.rs`

- [ ] **Step 1: Write the framebuffer display driver**

Create `waved/crates/waved-ui/src/display.rs`:
```rust
use crate::bindings::*;
use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::ptr;
use tracing::{error, info};

/// Framebuffer info from ioctl
const FB_DEVICE: &str = "/dev/fb0";
const SCREEN_WIDTH: u32 = 400;
const SCREEN_HEIGHT: u32 = 304;
const BYTES_PER_PIXEL: u32 = 2; // RGB565

// ioctl constants for framebuffer
const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

/// Holds the framebuffer memory map and LVGL display handle.
pub struct FramebufferDisplay {
    fb_fd: std::fs::File,
    fb_ptr: *mut u8,
    fb_len: usize,
    display: *mut lv_display_t,
}

// SAFETY: The framebuffer pointer is only accessed from the LVGL thread.
unsafe impl Send for FramebufferDisplay {}

/// Static reference to the framebuffer pointer for the flush callback.
/// SAFETY: Only accessed from the single LVGL thread.
static mut FB_MAP: *mut u8 = ptr::null_mut();
static mut FB_STRIDE: u32 = 0;

impl FramebufferDisplay {
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        // Open framebuffer device
        let fb_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(FB_DEVICE)?;

        let fd = fb_file.as_raw_fd();

        // Get variable screen info to confirm resolution
        let mut vinfo: libc::c_int = 0;
        // We'll mmap the framebuffer directly
        let fb_len = (SCREEN_WIDTH * SCREEN_HEIGHT * BYTES_PER_PIXEL) as usize;
        let stride = SCREEN_WIDTH * BYTES_PER_PIXEL;

        // Memory-map the framebuffer
        let fb_ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                fb_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };

        if fb_ptr == libc::MAP_FAILED {
            return Err(format!("mmap failed: {}", std::io::Error::last_os_error()).into());
        }

        let fb_ptr = fb_ptr as *mut u8;
        info!(
            "Framebuffer mapped: {}x{} @ {} BPP, stride={}, size={}",
            SCREEN_WIDTH, SCREEN_HEIGHT, BYTES_PER_PIXEL * 8, stride, fb_len
        );

        // Store in statics for flush callback
        unsafe {
            FB_MAP = fb_ptr;
            FB_STRIDE = stride;
        }

        // Create LVGL display
        let display = unsafe {
            let disp = lv_display_create(SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32);
            if disp.is_null() {
                return Err("lv_display_create returned null".into());
            }

            // Allocate draw buffers (double-buffered, each one line)
            let buf_size = (SCREEN_WIDTH * BYTES_PER_PIXEL * 10) as usize; // 10 rows
            let buf1 = libc::malloc(buf_size) as *mut u8;
            let buf2 = libc::malloc(buf_size) as *mut u8;

            if buf1.is_null() || buf2.is_null() {
                return Err("failed to allocate LVGL draw buffers".into());
            }

            lv_display_set_buffers(
                disp,
                buf1 as *mut _,
                buf2 as *mut _,
                buf_size as u32,
                LV_DISPLAY_RENDER_MODE_PARTIAL,
            );

            lv_display_set_flush_cb(disp, Some(flush_cb));

            disp
        };

        Ok(Self {
            fb_fd: fb_file,
            fb_ptr,
            fb_len,
            display,
        })
    }
}

impl Drop for FramebufferDisplay {
    fn drop(&mut self) {
        unsafe {
            if !self.fb_ptr.is_null() {
                libc::munmap(self.fb_ptr as *mut _, self.fb_len);
            }
            FB_MAP = ptr::null_mut();
        }
    }
}

/// LVGL flush callback — copies rendered pixels to the framebuffer.
///
/// SAFETY: Called from the LVGL thread only. FB_MAP is set before the
/// display is created and only cleared on drop.
unsafe extern "C" fn flush_cb(
    display: *mut lv_display_t,
    area: *const lv_area_t,
    px_map: *mut u8,
) {
    let fb = FB_MAP;
    if fb.is_null() {
        lv_display_flush_ready(display);
        return;
    }

    let area = &*area;
    let x1 = area.x1 as u32;
    let y1 = area.y1 as u32;
    let x2 = area.x2 as u32;
    let y2 = area.y2 as u32;
    let stride = FB_STRIDE;
    let bpp = BYTES_PER_PIXEL;

    let w = (x2 - x1 + 1) as usize;
    let row_bytes = w * bpp as usize;

    let mut src = px_map;
    for y in y1..=y2 {
        let dst_offset = (y * stride + x1 * bpp) as usize;
        let dst = fb.add(dst_offset);
        ptr::copy_nonoverlapping(src, dst, row_bytes);
        src = src.add(row_bytes);
    }

    lv_display_flush_ready(display);
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/src/display.rs
git commit -m "feat(waved-ui): add framebuffer display driver with mmap flush"
```

---

### Task 3: Touch Input Driver (evdev)

**Files:**
- Create: `waved/crates/waved-ui/src/input.rs`

- [ ] **Step 1: Write the evdev touch input driver**

Create `waved/crates/waved-ui/src/input.rs`:
```rust
use crate::bindings::*;
use std::fs::File;
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::ptr;
use tracing::{debug, error, info, warn};

const TOUCH_DEVICE: &str = "/dev/input/event0";
const SCREEN_WIDTH: i32 = 400;
const SCREEN_HEIGHT: i32 = 304;

// Linux input event types
const EV_SYN: u16 = 0x00;
const EV_ABS: u16 = 0x03;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;

// ioctl for getting axis info
const EVIOCGABS_BASE: libc::c_ulong = 0x80184540;

/// Raw Linux input_event struct (16 bytes on 32-bit time, 24 bytes on 64-bit)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct InputEvent {
    tv_sec: libc::time_t,
    tv_usec: libc::suseconds_t,
    type_: u16,
    code: u16,
    value: i32,
}

/// Axis calibration range from evdev
struct AxisRange {
    min: i32,
    max: i32,
}

/// LVGL touch input state, shared via static for the read callback.
/// SAFETY: Only accessed from the single LVGL thread.
static mut TOUCH_STATE: TouchState = TouchState {
    x: 0,
    y: 0,
    pressed: false,
};

struct TouchState {
    x: i32,
    y: i32,
    pressed: bool,
}

/// Holds the evdev file descriptor and axis calibration.
pub struct TouchInput {
    _file: File,
    indev: *mut lv_indev_t,
    x_range: AxisRange,
    y_range: AxisRange,
}

// SAFETY: Only used from the single LVGL thread.
unsafe impl Send for TouchInput {}

/// Static file descriptor for the read callback
static mut TOUCH_FD: i32 = -1;
static mut TOUCH_X_RANGE: (i32, i32) = (0, 1);
static mut TOUCH_Y_RANGE: (i32, i32) = (0, 1);

impl TouchInput {
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        let file = File::open(TOUCH_DEVICE)?;
        let fd = file.as_raw_fd();

        // Get axis ranges via ioctl EVIOCGABS
        let x_range = Self::get_axis_range(fd, ABS_MT_POSITION_X as libc::c_ulong)
            .or_else(|_| Self::get_axis_range(fd, ABS_X as libc::c_ulong))
            .unwrap_or(AxisRange { min: 0, max: SCREEN_WIDTH - 1 });

        let y_range = Self::get_axis_range(fd, ABS_MT_POSITION_Y as libc::c_ulong)
            .or_else(|_| Self::get_axis_range(fd, ABS_Y as libc::c_ulong))
            .unwrap_or(AxisRange { min: 0, max: SCREEN_HEIGHT - 1 });

        info!(
            "Touch input: X range [{}, {}], Y range [{}, {}]",
            x_range.min, x_range.max, y_range.min, y_range.max
        );

        // Set non-blocking mode on the fd
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        // Store in statics for callback
        unsafe {
            TOUCH_FD = fd;
            TOUCH_X_RANGE = (x_range.min, x_range.max);
            TOUCH_Y_RANGE = (y_range.min, y_range.max);
        }

        // Create LVGL input device
        let indev = unsafe {
            let indev = lv_indev_create();
            if indev.is_null() {
                return Err("lv_indev_create returned null".into());
            }
            lv_indev_set_type(indev, lv_indev_type_t_LV_INDEV_TYPE_POINTER);
            lv_indev_set_read_cb(indev, Some(touch_read_cb));
            indev
        };

        Ok(Self {
            _file: file,
            indev,
            x_range,
            y_range,
        })
    }

    fn get_axis_range(fd: i32, axis: libc::c_ulong) -> Result<AxisRange, std::io::Error> {
        #[repr(C)]
        #[derive(Default)]
        struct AbsInfo {
            value: i32,
            minimum: i32,
            maximum: i32,
            fuzz: i32,
            flat: i32,
            resolution: i32,
        }

        let mut info = AbsInfo::default();
        let req = EVIOCGABS_BASE + axis;
        let ret = unsafe { libc::ioctl(fd, req, &mut info as *mut _) };
        if ret < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(AxisRange {
            min: info.minimum,
            max: info.maximum,
        })
    }
}

/// Scale a raw touch coordinate to screen pixels.
fn scale(value: i32, in_min: i32, in_max: i32, out_max: i32) -> i32 {
    if in_max <= in_min {
        return 0;
    }
    let clamped = value.clamp(in_min, in_max);
    ((clamped - in_min) as i64 * out_max as i64 / (in_max - in_min) as i64) as i32
}

/// LVGL input read callback — reads pending evdev events and updates touch state.
///
/// SAFETY: Called from the LVGL thread only.
unsafe extern "C" fn touch_read_cb(indev: *mut lv_indev_t, data: *mut lv_indev_data_t) {
    let fd = TOUCH_FD;
    if fd < 0 {
        return;
    }

    let event_size = std::mem::size_of::<InputEvent>();
    let mut buf = [0u8; 256]; // room for ~10 events
    let mut raw_x = TOUCH_STATE.x;
    let mut raw_y = TOUCH_STATE.y;
    let mut pressed = TOUCH_STATE.pressed;

    // Read all pending events
    loop {
        let n = libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len());
        if n <= 0 {
            break;
        }
        let count = n as usize / event_size;
        for i in 0..count {
            let event_ptr = buf.as_ptr().add(i * event_size) as *const InputEvent;
            let event = &*event_ptr;

            match event.type_ {
                EV_ABS => match event.code {
                    ABS_MT_POSITION_X | ABS_X => raw_x = event.value,
                    ABS_MT_POSITION_Y | ABS_Y => raw_y = event.value,
                    ABS_MT_TRACKING_ID => {
                        pressed = event.value != -1; // -1 = finger lift
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    // Scale to screen coordinates
    let (x_min, x_max) = TOUCH_X_RANGE;
    let (y_min, y_max) = TOUCH_Y_RANGE;
    let sx = scale(raw_x, x_min, x_max, SCREEN_WIDTH - 1);
    let sy = scale(raw_y, y_min, y_max, SCREEN_HEIGHT - 1);

    TOUCH_STATE = TouchState {
        x: raw_x,
        y: raw_y,
        pressed,
    };

    let data = &mut *data;
    data.point.x = sx;
    data.point.y = sy;
    data.state = if pressed {
        lv_indev_state_t_LV_INDEV_STATE_PRESSED
    } else {
        lv_indev_state_t_LV_INDEV_STATE_RELEASED
    };
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/src/input.rs
git commit -m "feat(waved-ui): add evdev touch input driver with axis calibration"
```

---

### Task 4: UiCommand + UiAction Enums

**Files:**
- Create: `waved/crates/waved-ui/src/command.rs`

- [ ] **Step 1: Write the command and action types**

Create `waved/crates/waved-ui/src/command.rs`:
```rust
use uuid::Uuid;
use waved_state::{
    AudioMode, AudioState, BatteryState, BluetoothState, McuState, MeetingState, MotorPosition,
    PucState, WifiState,
};

/// Commands sent TO the UI thread (state updates from other modules).
/// Sent over tokio::sync::mpsc by modules that own state.
#[derive(Clone, Debug)]
pub enum UiCommand {
    // --- State snapshot updates ---
    UpdateBattery(BatteryState),
    UpdateWifi(WifiState),
    UpdateBluetooth(BluetoothState),
    UpdateAudio(AudioState),
    UpdateMeeting(MeetingState),
    UpdateMcu(McuState),
    UpdatePuc(PucState),

    // --- WiFi scan results ---
    WifiScanResults(Vec<WifiNetwork>),

    // --- Recording duration tick ---
    RecordingTick { duration_secs: u32 },

    // --- File list update ---
    FileListUpdate(Vec<RecordingFile>),

    // --- Toast notifications ---
    ShowToast(ToastKind, String),

    // --- Navigation commands (from events) ---
    NavigateTo(crate::nav::Screen),

    // --- OTA progress ---
    OtaProgress { percent: u8, status: String },

    // --- Loading screen ---
    ShowLoading(String),
    HideLoading,

    // --- Shutdown the UI thread ---
    Shutdown,
}

/// Actions sent FROM the UI thread back to waved modules.
/// Sent over StateBus.commands broadcast channel.
/// These map 1:1 to waved_state::Command variants.
#[derive(Clone, Debug)]
pub enum UiAction {
    SetAudioMode(AudioMode),
    ToggleMicMute,
    ToggleSpeakerMute,
    SetSpeakerVolume(u16),
    SetMicVolume(u16),
    StartRecording,
    StopRecording,
    PauseRecording,
    ResumeRecording,
    AddBookmark,
    RetryUpload(Uuid),
    SetMotorPosition(MotorPosition),
    WifiConnect { ssid: String, password: String },
    WifiDisconnect,
    WifiScan,
    BtToggle(bool),
    BtConnect,
    BtDisconnect,
    SetApiEndpoint(String),
    SetApiToken(String),
    SetDevicePassword(String),
    PucAutoFrame(bool),
    PucPan(PtzDirection),
    PucZoom(i8),
    PucReset,
    StartOta,
    Shutdown,
    Reboot,
}

#[derive(Clone, Debug)]
pub enum PtzDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Debug)]
pub enum ToastKind {
    Success,
    Error,
    Warning,
    Info,
    BtConnected,
    UsbConnected,
    Battery,
}

#[derive(Clone, Debug)]
pub struct WifiNetwork {
    pub ssid: String,
    pub signal: i8,        // dBm
    pub secured: bool,
    pub connected: bool,
}

#[derive(Clone, Debug)]
pub struct RecordingFile {
    pub id: Uuid,
    pub name: String,
    pub date: String,
    pub duration_secs: u32,
    pub size_bytes: u64,
    pub upload_status: UploadStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UploadStatus {
    Pending,
    Uploading(u8),  // percent
    Uploaded,
    Failed,
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/src/command.rs
git commit -m "feat(waved-ui): add UiCommand and UiAction channel types"
```

---

### Task 5: JSON Localization (i18n)

**Files:**
- Create: `waved/crates/waved-ui/src/i18n.rs`
- Create: `waved/crates/waved-ui/lang/en.json`

- [ ] **Step 1: Write the English string table**

Create `waved/crates/waved-ui/lang/en.json`:
```json
{
  "app.name": "Wave Open",

  "home.title": "Home",
  "home.connected": "Connected",
  "home.disconnected": "Disconnected",
  "home.mic": "Mic",
  "home.volume": "Vol",
  "home.record": "Record",
  "home.usb_connected": "USB Connected",
  "home.bt_connected": "BT Connected",

  "nav.audio_mode": "Audio",
  "nav.camera": "Camera",
  "nav.files": "Files",
  "nav.settings": "Settings",

  "recording.title": "Recording",
  "recording.recording": "Recording...",
  "recording.paused": "Paused",
  "recording.mark": "Mark",
  "recording.pause": "Pause",
  "recording.resume": "Resume",
  "recording.stop": "Stop",
  "recording.confirm_stop": "Stop recording?",
  "recording.confirm_stop_body": "The current recording will be saved.",

  "files.title": "Files",
  "files.empty": "No recordings yet",
  "files.upload_pending": "Pending",
  "files.upload_uploading": "Uploading",
  "files.upload_done": "Uploaded",
  "files.upload_failed": "Failed",
  "files.retry": "Retry",
  "files.delete": "Delete",
  "files.confirm_delete": "Delete this recording?",

  "audio_mode.title": "Audio Mode",
  "audio_mode.noise_reduction": "Noise Reduction",
  "audio_mode.noise_reduction_desc": "AI noise cancellation. Ideal for meetings.",
  "audio_mode.original": "Original",
  "audio_mode.omni": "Omni",
  "audio_mode.cardioid": "Cardioid",
  "audio_mode.supercardioid": "Supercardioid",
  "audio_mode.figure8": "Figure-8",
  "audio_mode.stereo": "Stereo",
  "audio_mode.confirm": "Confirm",

  "wifi.title": "Wi-Fi",
  "wifi.scanning": "Scanning...",
  "wifi.connected": "Connected",
  "wifi.available": "Available Networks",
  "wifi.no_networks": "No networks found",
  "wifi.enter_password": "Enter Password",
  "wifi.connecting": "Connecting...",
  "wifi.forget": "Forget Network",

  "wifi_info.title": "Network Info",
  "wifi_info.status": "Status",
  "wifi_info.signal": "Signal",
  "wifi_info.security": "Security",
  "wifi_info.ip": "IP Address",
  "wifi_info.forget": "Forget this network",

  "bt.title": "Bluetooth",
  "bt.on": "Bluetooth",
  "bt.auto_reconnect": "Auto Reconnect",
  "bt.connect_new": "Connect New Device",
  "bt.waiting": "Waiting for connection...",
  "bt.connected_to": "Connected to",
  "bt.connecting": "Connecting...",
  "bt.disconnected": "Disconnected",

  "settings.title": "Settings",
  "settings.audio_mode": "Audio Mode",
  "settings.camera": "Camera",
  "settings.volume": "Volume",
  "settings.wifi": "Wi-Fi",
  "settings.bluetooth": "Bluetooth",
  "settings.height": "Device Height",
  "settings.api": "API Config",
  "settings.password": "Password",
  "settings.about": "About",
  "settings.ota": "Firmware Update",
  "settings.power_off": "Power Off",

  "volume.title": "Volume",
  "volume.speaker": "Speaker",
  "volume.mic": "Microphone",

  "height.title": "Device Height",
  "height.low": "Low",
  "height.medium": "Medium",
  "height.high": "High",
  "height.auto_lift": "Auto Lift",
  "height.auto_lift_desc": "Auto-lift if obstacle when closing",

  "camera.title": "Camera",
  "camera.auto_framing": "Auto Framing",
  "camera.auto_framing_desc": "Track speakers by sound source",
  "camera.manual": "Manual Adjust",
  "camera.whiteboard": "Smart Whiteboard",

  "camera_ptz.title": "Manual Adjust",
  "camera_ptz.reset": "Reset",
  "camera_ptz.zoom": "Zoom",

  "api.title": "API Config",
  "api.endpoint": "Endpoint URL",
  "api.token": "Auth Token",
  "api.auto_upload": "Auto Upload",
  "api.save": "Save",
  "api.saved": "API settings saved",

  "about.title": "About",
  "about.model": "Model",
  "about.serial": "Serial Number",
  "about.firmware": "Firmware",
  "about.ip": "IP Address",
  "about.storage": "Storage",
  "about.storage_fmt": "{used} / {total}",

  "password.title": "Password",
  "password.enable": "Enable Password",
  "password.change": "Change Password",
  "password.enter": "Enter Password",
  "password.confirm": "Confirm Password",

  "ota.title": "Firmware Update",
  "ota.current": "Current Version",
  "ota.check": "Check for Updates",
  "ota.checking": "Checking...",
  "ota.up_to_date": "You are up to date",
  "ota.available": "Update Available",
  "ota.downloading": "Downloading...",
  "ota.installing": "Installing...",
  "ota.complete": "Update complete. Restarting...",
  "ota.failed": "Update failed",
  "ota.update": "Update",
  "ota.cancel": "Cancel",

  "power_off.title": "Power Off",
  "power_off.confirm": "Power off the device?",
  "power_off.body": "Motor will retract before shutdown.",
  "power_off.shutting_down": "Shutting down...",

  "loading.text": "Loading...",

  "toast.bt_connected": "Bluetooth connected",
  "toast.bt_disconnected": "Bluetooth disconnected",
  "toast.usb_connected": "USB connected. Speakerphone mode.",
  "toast.usb_disconnected": "USB disconnected",
  "toast.wifi_connected": "Wi-Fi connected",
  "toast.wifi_disconnected": "Wi-Fi disconnected",
  "toast.recording_started": "Recording started",
  "toast.recording_stopped": "Recording saved",
  "toast.upload_complete": "Upload complete",
  "toast.upload_failed": "Upload failed",
  "toast.low_battery": "Low battery",
  "toast.motor_obstruction": "Lift obstructed",

  "dialog.cancel": "Cancel",
  "dialog.confirm": "Confirm",
  "dialog.ok": "OK",
  "dialog.yes": "Yes",
  "dialog.no": "No"
}
```

- [ ] **Step 2: Write the i18n loader**

Create `waved/crates/waved-ui/src/i18n.rs`:
```rust
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};

/// Runtime string table loaded from JSON.
/// Falls back to compiled-in English if the file is missing.
pub struct I18n {
    strings: HashMap<String, String>,
}

/// Compiled-in fallback English strings.
const FALLBACK_EN: &str = include_str!("../lang/en.json");

impl I18n {
    /// Load a language file from `/data/waved/lang/<code>.json`.
    /// Falls back to the compiled-in English table on any failure.
    pub fn load(lang_code: &str) -> Self {
        let path = format!("/data/waved/lang/{}.json", lang_code);
        match Self::load_from_file(Path::new(&path)) {
            Ok(i18n) => {
                info!("Loaded language: {} ({} strings)", lang_code, i18n.strings.len());
                i18n
            }
            Err(e) => {
                warn!("Failed to load language '{}' from {}: {}", lang_code, path, e);
                info!("Using compiled-in English fallback");
                Self::load_from_str(FALLBACK_EN)
                    .expect("compiled-in en.json must be valid")
            }
        }
    }

    fn load_from_file(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        Self::load_from_str(&contents)
    }

    fn load_from_str(json: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let value: Value = serde_json::from_str(json)?;
        let map = value
            .as_object()
            .ok_or("JSON root must be an object")?;

        let mut strings = HashMap::with_capacity(map.len());
        for (key, val) in map {
            if let Some(s) = val.as_str() {
                strings.insert(key.clone(), s.to_string());
            }
        }

        Ok(Self { strings })
    }

    /// Look up a translated string by key.
    /// Returns the key itself if not found (makes missing strings visible in UI).
    pub fn tr(&self, key: &str) -> &str {
        self.strings.get(key).map(|s| s.as_str()).unwrap_or(key)
    }

    /// Look up a string and replace `{placeholder}` tokens.
    pub fn tr_fmt(&self, key: &str, replacements: &[(&str, &str)]) -> String {
        let mut result = self.tr(key).to_string();
        for (placeholder, value) in replacements {
            let token = format!("{{{}}}", placeholder);
            result = result.replace(&token, value);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_loads() {
        let i18n = I18n::load_from_str(FALLBACK_EN).unwrap();
        assert_eq!(i18n.tr("app.name"), "Wave Open");
        assert_eq!(i18n.tr("home.title"), "Home");
    }

    #[test]
    fn missing_key_returns_key() {
        let i18n = I18n::load_from_str(FALLBACK_EN).unwrap();
        assert_eq!(i18n.tr("nonexistent.key"), "nonexistent.key");
    }

    #[test]
    fn tr_fmt_replaces_placeholders() {
        let i18n = I18n::load_from_str(FALLBACK_EN).unwrap();
        let result = i18n.tr_fmt("about.storage_fmt", &[("used", "4GB"), ("total", "32GB")]);
        assert_eq!(result, "4GB / 32GB");
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo test -p waved-ui -- i18n
```
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add waved/crates/waved-ui/src/i18n.rs waved/crates/waved-ui/lang/
git commit -m "feat(waved-ui): add JSON-based localization with English string table"
```

---

### Task 6: Theme + Color Constants

**Files:**
- Create: `waved/crates/waved-ui/src/theme.rs`

- [ ] **Step 1: Write the theme module**

Create `waved/crates/waved-ui/src/theme.rs`:
```rust
use crate::bindings::*;

// Display dimensions
pub const SCREEN_W: i32 = 400;
pub const SCREEN_H: i32 = 304;

// Layout regions
pub const STATUS_BAR_H: i32 = 32;
pub const BOTTOM_BAR_H: i32 = 32;
pub const CONTENT_Y: i32 = STATUS_BAR_H;
pub const CONTENT_H: i32 = SCREEN_H - STATUS_BAR_H - BOTTOM_BAR_H;

// Padding / spacing
pub const PAD_SMALL: i32 = 4;
pub const PAD_MEDIUM: i32 = 8;
pub const PAD_LARGE: i32 = 16;
pub const LIST_ITEM_H: i32 = 44;

// Colors — dark theme matching stock Wave UI
pub const COLOR_BG: u32 = 0x1A1A2E;           // Deep navy background
pub const COLOR_BG_CARD: u32 = 0x16213E;       // Card/panel background
pub const COLOR_PRIMARY: u32 = 0x0F3460;       // Primary accent
pub const COLOR_ACCENT: u32 = 0x533483;        // Accent purple
pub const COLOR_TEXT: u32 = 0xFFFFFF;           // Primary text (white)
pub const COLOR_TEXT_SEC: u32 = 0xA0A0A0;       // Secondary text (gray)
pub const COLOR_SUCCESS: u32 = 0x4CAF50;        // Green (connected, success)
pub const COLOR_ERROR: u32 = 0xF44336;          // Red (error, muted)
pub const COLOR_WARNING: u32 = 0xFFC107;        // Yellow (warning)
pub const COLOR_RECORDING: u32 = 0xFF1744;      // Recording red
pub const COLOR_SLIDER_BG: u32 = 0x333333;      // Slider background
pub const COLOR_SLIDER_IND: u32 = 0x2196F3;     // Slider active indicator
pub const COLOR_DIVIDER: u32 = 0x333333;         // List divider

/// Helper: create an lv_color_t from a 0xRRGGBB hex value.
pub fn color_hex(hex: u32) -> lv_color_t {
    unsafe { lv_color_hex(hex) }
}

/// Apply the global dark theme to LVGL defaults.
pub fn apply_theme() {
    unsafe {
        // Get active display
        let disp = lv_display_get_default();
        if disp.is_null() {
            return;
        }

        // Set screen background color
        let scr = lv_display_get_screen_active(disp);
        if !scr.is_null() {
            lv_obj_set_style_bg_color(scr, color_hex(COLOR_BG), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(scr, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_text_color(scr, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);
        }
    }
}

/// Get the LVGL built-in font for the given size.
/// Returns the closest available Montserrat font.
pub fn font(size: u8) -> *const lv_font_t {
    unsafe {
        match size {
            0..=13 => &lv_font_montserrat_12 as *const _,
            14..=15 => &lv_font_montserrat_14 as *const _,
            16..=19 => &lv_font_montserrat_16 as *const _,
            20..=23 => &lv_font_montserrat_20 as *const _,
            24..=27 => &lv_font_montserrat_24 as *const _,
            _ => &lv_font_montserrat_28 as *const _,
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/src/theme.rs
git commit -m "feat(waved-ui): add dark theme with color palette and layout constants"
```

---

### Task 7: Reusable Widget Helpers

**Files:**
- Create: `waved/crates/waved-ui/src/widgets.rs`

- [ ] **Step 1: Write reusable LVGL widget wrappers**

Create `waved/crates/waved-ui/src/widgets.rs`:
```rust
use crate::bindings::*;
use crate::theme::*;
use std::ffi::CString;

/// Create a full-width label on the given parent.
pub unsafe fn label(parent: *mut lv_obj_t, text: &str) -> *mut lv_obj_t {
    let lbl = lv_label_create(parent);
    let cstr = CString::new(text).unwrap();
    lv_label_set_text(lbl, cstr.as_ptr());
    lv_obj_set_style_text_color(lbl, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);
    // Keep CString alive until text is set — LVGL copies the string internally.
    lbl
}

/// Create a label with a specific font size.
pub unsafe fn label_sized(parent: *mut lv_obj_t, text: &str, size: u8) -> *mut lv_obj_t {
    let lbl = label(parent, text);
    lv_obj_set_style_text_font(lbl, font(size), LV_PART_MAIN as u32);
    lbl
}

/// Create a secondary-colored label (for descriptions, hints).
pub unsafe fn label_secondary(parent: *mut lv_obj_t, text: &str) -> *mut lv_obj_t {
    let lbl = label(parent, text);
    lv_obj_set_style_text_color(lbl, color_hex(COLOR_TEXT_SEC), LV_PART_MAIN as u32);
    lv_obj_set_style_text_font(lbl, font(14), LV_PART_MAIN as u32);
    lbl
}

/// Set text on an existing label.
pub unsafe fn set_label_text(lbl: *mut lv_obj_t, text: &str) {
    let cstr = CString::new(text).unwrap();
    lv_label_set_text(lbl, cstr.as_ptr());
}

/// Create a styled button with text.
pub unsafe fn button(parent: *mut lv_obj_t, text: &str, width: i32, height: i32) -> *mut lv_obj_t {
    let btn = lv_btn_create(parent);
    lv_obj_set_size(btn, width, height);
    lv_obj_set_style_bg_color(btn, color_hex(COLOR_PRIMARY), LV_PART_MAIN as u32);
    lv_obj_set_style_radius(btn, 8, LV_PART_MAIN as u32);

    let lbl = lv_label_create(btn);
    let cstr = CString::new(text).unwrap();
    lv_label_set_text(lbl, cstr.as_ptr());
    lv_obj_center(lbl);
    lv_obj_set_style_text_color(lbl, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);

    btn
}

/// Create a list item row: "Label text" with optional right-arrow indicator.
/// Returns the button (clickable row).
pub unsafe fn list_item(
    parent: *mut lv_obj_t,
    text: &str,
    show_arrow: bool,
) -> *mut lv_obj_t {
    let btn = lv_btn_create(parent);
    lv_obj_set_size(btn, SCREEN_W - PAD_MEDIUM * 2, LIST_ITEM_H);
    lv_obj_set_style_bg_color(btn, color_hex(COLOR_BG_CARD), LV_PART_MAIN as u32);
    lv_obj_set_style_radius(btn, 0, LV_PART_MAIN as u32);
    lv_obj_set_style_pad_left(btn, PAD_MEDIUM, LV_PART_MAIN as u32);
    lv_obj_set_style_pad_right(btn, PAD_MEDIUM, LV_PART_MAIN as u32);

    let lbl = lv_label_create(btn);
    let cstr = CString::new(text).unwrap();
    lv_label_set_text(lbl, cstr.as_ptr());
    lv_obj_align(lbl, LV_ALIGN_LEFT_MID as u8, 0, 0);
    lv_obj_set_style_text_color(lbl, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);

    if show_arrow {
        let arrow = lv_label_create(btn);
        let arrow_str = CString::new(">").unwrap();
        lv_label_set_text(arrow, arrow_str.as_ptr());
        lv_obj_align(arrow, LV_ALIGN_RIGHT_MID as u8, 0, 0);
        lv_obj_set_style_text_color(arrow, color_hex(COLOR_TEXT_SEC), LV_PART_MAIN as u32);
    }

    btn
}

/// Create a list item with a value string on the right side.
pub unsafe fn list_item_value(
    parent: *mut lv_obj_t,
    text: &str,
    value: &str,
) -> (*mut lv_obj_t, *mut lv_obj_t) {
    let btn = list_item(parent, text, false);

    let val_lbl = lv_label_create(btn);
    let cstr = CString::new(value).unwrap();
    lv_label_set_text(val_lbl, cstr.as_ptr());
    lv_obj_align(val_lbl, LV_ALIGN_RIGHT_MID as u8, 0, 0);
    lv_obj_set_style_text_color(val_lbl, color_hex(COLOR_TEXT_SEC), LV_PART_MAIN as u32);

    (btn, val_lbl)
}

/// Create a toggle switch row: "Label [ON/OFF]".
/// Returns (row button, switch widget).
pub unsafe fn toggle_row(
    parent: *mut lv_obj_t,
    text: &str,
    initial: bool,
) -> (*mut lv_obj_t, *mut lv_obj_t) {
    let btn = lv_btn_create(parent);
    lv_obj_set_size(btn, SCREEN_W - PAD_MEDIUM * 2, LIST_ITEM_H);
    lv_obj_set_style_bg_color(btn, color_hex(COLOR_BG_CARD), LV_PART_MAIN as u32);
    lv_obj_set_style_radius(btn, 0, LV_PART_MAIN as u32);
    lv_obj_set_style_pad_left(btn, PAD_MEDIUM, LV_PART_MAIN as u32);
    lv_obj_set_style_pad_right(btn, PAD_MEDIUM, LV_PART_MAIN as u32);

    let lbl = lv_label_create(btn);
    let cstr = CString::new(text).unwrap();
    lv_label_set_text(lbl, cstr.as_ptr());
    lv_obj_align(lbl, LV_ALIGN_LEFT_MID as u8, 0, 0);
    lv_obj_set_style_text_color(lbl, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);

    let sw = lv_switch_create(btn);
    lv_obj_align(sw, LV_ALIGN_RIGHT_MID as u8, 0, 0);
    if initial {
        lv_obj_add_state(sw, LV_STATE_CHECKED as u16);
    }

    (btn, sw)
}

/// Create a content container positioned below the status bar
/// and above the bottom bar.
pub unsafe fn content_container(parent: *mut lv_obj_t) -> *mut lv_obj_t {
    let cont = lv_obj_create(parent);
    lv_obj_set_pos(cont, 0, CONTENT_Y);
    lv_obj_set_size(cont, SCREEN_W, CONTENT_H);
    lv_obj_set_style_bg_color(cont, color_hex(COLOR_BG), LV_PART_MAIN as u32);
    lv_obj_set_style_bg_opa(cont, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
    lv_obj_set_style_border_width(cont, 0, LV_PART_MAIN as u32);
    lv_obj_set_style_radius(cont, 0, LV_PART_MAIN as u32);
    lv_obj_set_style_pad_all(cont, 0, LV_PART_MAIN as u32);
    // Enable scrolling for lists
    lv_obj_set_scrollbar_mode(cont, LV_SCROLLBAR_MODE_AUTO as i32 as u8);
    lv_obj_set_scroll_dir(cont, LV_DIR_VER as u8);
    cont
}

/// Create a title bar row with back button and title text.
/// Returns (container, back_button).
pub unsafe fn title_bar(parent: *mut lv_obj_t, title: &str) -> (*mut lv_obj_t, *mut lv_obj_t) {
    let bar = lv_obj_create(parent);
    lv_obj_set_pos(bar, 0, STATUS_BAR_H);
    lv_obj_set_size(bar, SCREEN_W, 36);
    lv_obj_set_style_bg_color(bar, color_hex(COLOR_BG), LV_PART_MAIN as u32);
    lv_obj_set_style_bg_opa(bar, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
    lv_obj_set_style_border_width(bar, 0, LV_PART_MAIN as u32);
    lv_obj_set_style_radius(bar, 0, LV_PART_MAIN as u32);
    lv_obj_set_style_pad_left(bar, PAD_SMALL, LV_PART_MAIN as u32);

    // Back button
    let back = lv_btn_create(bar);
    lv_obj_set_size(back, 40, 32);
    lv_obj_align(back, LV_ALIGN_LEFT_MID as u8, 0, 0);
    lv_obj_set_style_bg_opa(back, LV_OPA_TRANSP as u8, LV_PART_MAIN as u32);
    let back_lbl = lv_label_create(back);
    let arrow = CString::new("<").unwrap();
    lv_label_set_text(back_lbl, arrow.as_ptr());
    lv_obj_center(back_lbl);
    lv_obj_set_style_text_color(back_lbl, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);
    lv_obj_set_style_text_font(back_lbl, font(20), LV_PART_MAIN as u32);

    // Title
    let title_lbl = lv_label_create(bar);
    let cstr = CString::new(title).unwrap();
    lv_label_set_text(title_lbl, cstr.as_ptr());
    lv_obj_align(title_lbl, LV_ALIGN_CENTER as u8, 0, 0);
    lv_obj_set_style_text_color(title_lbl, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);
    lv_obj_set_style_text_font(title_lbl, font(16), LV_PART_MAIN as u32);

    (bar, back)
}

/// Create a horizontal slider with min/max.
/// Returns the slider widget.
pub unsafe fn slider(
    parent: *mut lv_obj_t,
    min: i32,
    max: i32,
    initial: i32,
    width: i32,
) -> *mut lv_obj_t {
    let sl = lv_slider_create(parent);
    lv_slider_set_range(sl, min, max);
    lv_slider_set_value(sl, initial, LV_ANIM_ON as u32);
    lv_obj_set_width(sl, width);
    lv_obj_set_style_bg_color(sl, color_hex(COLOR_SLIDER_BG), LV_PART_MAIN as u32);
    lv_obj_set_style_bg_color(sl, color_hex(COLOR_SLIDER_IND), LV_PART_INDICATOR as u32);
    lv_obj_set_style_bg_color(sl, color_hex(COLOR_TEXT), LV_PART_KNOB as u32);
    sl
}

/// Create a confirmation dialog with title, body, cancel and confirm buttons.
/// Returns (dialog container, cancel_btn, confirm_btn).
pub unsafe fn confirm_dialog(
    title: &str,
    body: &str,
    cancel_text: &str,
    confirm_text: &str,
) -> (*mut lv_obj_t, *mut lv_obj_t, *mut lv_obj_t) {
    // Create overlay on top layer
    let overlay = lv_obj_create(lv_layer_top());
    lv_obj_set_size(overlay, SCREEN_W, SCREEN_H);
    lv_obj_set_style_bg_color(overlay, color_hex(0x000000), LV_PART_MAIN as u32);
    lv_obj_set_style_bg_opa(overlay, 180, LV_PART_MAIN as u32);
    lv_obj_set_style_border_width(overlay, 0, LV_PART_MAIN as u32);
    lv_obj_set_style_radius(overlay, 0, LV_PART_MAIN as u32);

    // Dialog box
    let dlg = lv_obj_create(overlay);
    lv_obj_set_size(dlg, 320, 160);
    lv_obj_center(dlg);
    lv_obj_set_style_bg_color(dlg, color_hex(COLOR_BG_CARD), LV_PART_MAIN as u32);
    lv_obj_set_style_radius(dlg, 12, LV_PART_MAIN as u32);
    lv_obj_set_style_pad_all(dlg, PAD_LARGE, LV_PART_MAIN as u32);

    // Title
    let _title_lbl = label_sized(dlg, title, 18);

    // Body
    let body_lbl = label_secondary(dlg, body);
    lv_obj_align(body_lbl, LV_ALIGN_TOP_LEFT as u8, 0, 30);
    lv_label_set_long_mode(body_lbl, lv_label_long_mode_t_LV_LABEL_LONG_WRAP);
    lv_obj_set_width(body_lbl, 280);

    // Cancel button
    let cancel_btn = button(dlg, cancel_text, 120, 36);
    lv_obj_align(cancel_btn, LV_ALIGN_BOTTOM_LEFT as u8, 0, 0);

    // Confirm button
    let confirm_btn = button(dlg, confirm_text, 120, 36);
    lv_obj_align(confirm_btn, LV_ALIGN_BOTTOM_RIGHT as u8, 0, 0);
    lv_obj_set_style_bg_color(confirm_btn, color_hex(COLOR_ACCENT), LV_PART_MAIN as u32);

    (overlay, cancel_btn, confirm_btn)
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/src/widgets.rs
git commit -m "feat(waved-ui): add reusable LVGL widget helpers (label, button, list, slider, dialog)"
```

---

### Task 8: Status Bar

**Files:**
- Create: `waved/crates/waved-ui/src/status_bar.rs`

- [ ] **Step 1: Write the persistent status bar**

Create `waved/crates/waved-ui/src/status_bar.rs`:
```rust
use crate::bindings::*;
use crate::command::UiCommand;
use crate::i18n::I18n;
use crate::theme::*;
use crate::widgets;
use std::ffi::CString;
use waved_state::{BatteryState, BluetoothState, WifiState};

/// The persistent status bar rendered on `lv_layer_top()`.
/// Never destroyed during screen navigation.
pub struct StatusBar {
    container: *mut lv_obj_t,
    battery_label: *mut lv_obj_t,
    wifi_label: *mut lv_obj_t,
    bt_label: *mut lv_obj_t,
    mic_label: *mut lv_obj_t,
    spk_label: *mut lv_obj_t,
    usb_label: *mut lv_obj_t,
    // Cached state for update comparisons
    last_battery: u8,
    last_charging: bool,
    last_wifi_connected: bool,
    last_wifi_signal: i8,
    last_bt_connected: bool,
    last_mic_muted: bool,
    last_spk_muted: bool,
    last_usb: bool,
}

impl StatusBar {
    pub fn create(_i18n: &I18n) -> Self {
        unsafe {
            // Create container on the top layer so it persists across screens
            let top = lv_layer_top();
            let bar = lv_obj_create(top);
            lv_obj_set_pos(bar, 0, 0);
            lv_obj_set_size(bar, SCREEN_W, STATUS_BAR_H);
            lv_obj_set_style_bg_color(bar, color_hex(COLOR_BG_CARD), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(bar, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(bar, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_radius(bar, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_left(bar, PAD_SMALL, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_right(bar, PAD_SMALL, LV_PART_MAIN as u32);

            // Battery icon + percentage (leftmost)
            let battery_label = lv_label_create(bar);
            let batt_str = CString::new("100%").unwrap();
            lv_label_set_text(battery_label, batt_str.as_ptr());
            lv_obj_align(battery_label, LV_ALIGN_LEFT_MID as u8, 0, 0);
            lv_obj_set_style_text_color(battery_label, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);
            lv_obj_set_style_text_font(battery_label, font(12), LV_PART_MAIN as u32);

            // WiFi indicator
            let wifi_label = lv_label_create(bar);
            let wifi_str = CString::new("WiFi").unwrap();
            lv_label_set_text(wifi_label, wifi_str.as_ptr());
            lv_obj_align(wifi_label, LV_ALIGN_LEFT_MID as u8, 48, 0);
            lv_obj_set_style_text_color(wifi_label, color_hex(COLOR_TEXT_SEC), LV_PART_MAIN as u32);
            lv_obj_set_style_text_font(wifi_label, font(12), LV_PART_MAIN as u32);

            // BT indicator
            let bt_label = lv_label_create(bar);
            let bt_str = CString::new("BT").unwrap();
            lv_label_set_text(bt_label, bt_str.as_ptr());
            lv_obj_align(bt_label, LV_ALIGN_LEFT_MID as u8, 96, 0);
            lv_obj_set_style_text_color(bt_label, color_hex(COLOR_TEXT_SEC), LV_PART_MAIN as u32);
            lv_obj_set_style_text_font(bt_label, font(12), LV_PART_MAIN as u32);

            // Mic indicator
            let mic_label = lv_label_create(bar);
            let mic_str = CString::new("MIC").unwrap();
            lv_label_set_text(mic_label, mic_str.as_ptr());
            lv_obj_align(mic_label, LV_ALIGN_LEFT_MID as u8, 130, 0);
            lv_obj_set_style_text_color(mic_label, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);
            lv_obj_set_style_text_font(mic_label, font(12), LV_PART_MAIN as u32);

            // Speaker indicator
            let spk_label = lv_label_create(bar);
            let spk_str = CString::new("SPK").unwrap();
            lv_label_set_text(spk_label, spk_str.as_ptr());
            lv_obj_align(spk_label, LV_ALIGN_LEFT_MID as u8, 168, 0);
            lv_obj_set_style_text_color(spk_label, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);
            lv_obj_set_style_text_font(spk_label, font(12), LV_PART_MAIN as u32);

            // USB indicator (rightish)
            let usb_label = lv_label_create(bar);
            let usb_str = CString::new("USB").unwrap();
            lv_label_set_text(usb_label, usb_str.as_ptr());
            lv_obj_align(usb_label, LV_ALIGN_LEFT_MID as u8, 206, 0);
            lv_obj_set_style_text_color(usb_label, color_hex(COLOR_TEXT_SEC), LV_PART_MAIN as u32);
            lv_obj_set_style_text_font(usb_label, font(12), LV_PART_MAIN as u32);
            lv_obj_add_flag(usb_label, LV_OBJ_FLAG_HIDDEN);

            Self {
                container: bar,
                battery_label,
                wifi_label,
                bt_label,
                mic_label,
                spk_label,
                usb_label,
                last_battery: 100,
                last_charging: false,
                last_wifi_connected: false,
                last_wifi_signal: 0,
                last_bt_connected: false,
                last_mic_muted: false,
                last_spk_muted: false,
                last_usb: false,
            }
        }
    }

    pub fn handle_command(&mut self, cmd: &UiCommand) {
        match cmd {
            UiCommand::UpdateBattery(state) => self.update_battery(state),
            UiCommand::UpdateWifi(state) => self.update_wifi(state),
            UiCommand::UpdateBluetooth(state) => self.update_bt(state),
            UiCommand::UpdateAudio(state) => self.update_audio(state),
            UiCommand::UpdateMcu(state) => self.update_mcu(state),
            _ => {}
        }
    }

    fn update_battery(&mut self, state: &BatteryState) {
        if state.level == self.last_battery && state.is_charging == self.last_charging {
            return;
        }
        self.last_battery = state.level;
        self.last_charging = state.is_charging;

        let text = if state.is_charging {
            format!("{}%+", state.level)
        } else {
            format!("{}%", state.level)
        };

        let color = match state.level {
            0..=10 => COLOR_ERROR,
            11..=20 => COLOR_WARNING,
            _ => COLOR_TEXT,
        };

        unsafe {
            widgets::set_label_text(self.battery_label, &text);
            lv_obj_set_style_text_color(
                self.battery_label,
                color_hex(color),
                LV_PART_MAIN as u32,
            );
        }
    }

    fn update_wifi(&mut self, state: &WifiState) {
        if state.connected == self.last_wifi_connected
            && state.signal_strength == self.last_wifi_signal
        {
            return;
        }
        self.last_wifi_connected = state.connected;
        self.last_wifi_signal = state.signal_strength;

        let (text, color) = if state.connected {
            let bars = match state.signal_strength {
                -50..=0 => "WiFi+++",
                -70..=-51 => "WiFi++",
                -80..=-71 => "WiFi+",
                _ => "WiFi.",
            };
            (bars.to_string(), COLOR_SUCCESS)
        } else {
            ("WiFi".to_string(), COLOR_TEXT_SEC)
        };

        unsafe {
            widgets::set_label_text(self.wifi_label, &text);
            lv_obj_set_style_text_color(
                self.wifi_label,
                color_hex(color),
                LV_PART_MAIN as u32,
            );
        }
    }

    fn update_bt(&mut self, state: &BluetoothState) {
        if state.connected == self.last_bt_connected {
            return;
        }
        self.last_bt_connected = state.connected;

        let color = if state.connected {
            COLOR_SUCCESS
        } else {
            COLOR_TEXT_SEC
        };

        unsafe {
            lv_obj_set_style_text_color(
                self.bt_label,
                color_hex(color),
                LV_PART_MAIN as u32,
            );
        }
    }

    fn update_audio(&mut self, state: &waved_state::AudioState) {
        // Mic mute
        if state.mic_muted != self.last_mic_muted {
            self.last_mic_muted = state.mic_muted;
            let color = if state.mic_muted {
                COLOR_ERROR
            } else {
                COLOR_TEXT
            };
            unsafe {
                lv_obj_set_style_text_color(
                    self.mic_label,
                    color_hex(color),
                    LV_PART_MAIN as u32,
                );
            }
        }

        // Speaker mute
        if state.speaker_muted != self.last_spk_muted {
            self.last_spk_muted = state.speaker_muted;
            let color = if state.speaker_muted {
                COLOR_ERROR
            } else {
                COLOR_TEXT
            };
            unsafe {
                lv_obj_set_style_text_color(
                    self.spk_label,
                    color_hex(color),
                    LV_PART_MAIN as u32,
                );
            }
        }
    }

    fn update_mcu(&mut self, state: &waved_state::McuState) {
        if state.usb_connected != self.last_usb {
            self.last_usb = state.usb_connected;
            unsafe {
                if state.usb_connected {
                    lv_obj_remove_flag(self.usb_label, LV_OBJ_FLAG_HIDDEN);
                } else {
                    lv_obj_add_flag(self.usb_label, LV_OBJ_FLAG_HIDDEN);
                }
            }
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/src/status_bar.rs
git commit -m "feat(waved-ui): add persistent status bar with battery, WiFi, BT, mic, speaker, USB"
```

---

### Task 9: Toast Notification System

**Files:**
- Create: `waved/crates/waved-ui/src/toast.rs`

- [ ] **Step 1: Write the toast overlay manager**

Create `waved/crates/waved-ui/src/toast.rs`:
```rust
use crate::bindings::*;
use crate::command::{ToastKind, UiCommand};
use crate::theme::*;
use crate::widgets;
use std::ffi::CString;
use std::time::{Duration, Instant};

const TOAST_DURATION_MS: u64 = 3000;
const TOAST_HEIGHT: i32 = 36;
const TOAST_Y: i32 = STATUS_BAR_H + 4;

pub struct ToastManager {
    current: Option<ActiveToast>,
}

struct ActiveToast {
    container: *mut lv_obj_t,
    expires: Instant,
}

impl ToastManager {
    pub fn new() -> Self {
        Self { current: None }
    }

    pub fn handle_command(&mut self, cmd: &UiCommand) {
        match cmd {
            UiCommand::ShowToast(kind, message) => {
                self.show(kind, message);
            }
            _ => {}
        }

        // Check expiry
        self.tick();
    }

    pub fn tick(&mut self) {
        if let Some(toast) = &self.current {
            if Instant::now() >= toast.expires {
                self.dismiss();
            }
        }
    }

    fn show(&mut self, kind: &ToastKind, message: &str) {
        // Dismiss any existing toast
        self.dismiss();

        let color = match kind {
            ToastKind::Success => COLOR_SUCCESS,
            ToastKind::Error => COLOR_ERROR,
            ToastKind::Warning => COLOR_WARNING,
            ToastKind::Info => COLOR_PRIMARY,
            ToastKind::BtConnected => COLOR_SUCCESS,
            ToastKind::UsbConnected => COLOR_SUCCESS,
            ToastKind::Battery => COLOR_WARNING,
        };

        unsafe {
            let top = lv_layer_top();

            let container = lv_obj_create(top);
            lv_obj_set_pos(container, PAD_MEDIUM, TOAST_Y);
            lv_obj_set_size(container, SCREEN_W - PAD_MEDIUM * 2, TOAST_HEIGHT);
            lv_obj_set_style_bg_color(container, color_hex(color), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(container, 230, LV_PART_MAIN as u32);
            lv_obj_set_style_radius(container, 8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(container, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_left(container, PAD_MEDIUM, LV_PART_MAIN as u32);

            let lbl = lv_label_create(container);
            let cstr = CString::new(message.to_string()).unwrap();
            lv_label_set_text(lbl, cstr.as_ptr());
            lv_obj_align(lbl, LV_ALIGN_LEFT_MID as u8, 0, 0);
            lv_obj_set_style_text_color(lbl, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);
            lv_obj_set_style_text_font(lbl, font(14), LV_PART_MAIN as u32);

            self.current = Some(ActiveToast {
                container,
                expires: Instant::now() + Duration::from_millis(TOAST_DURATION_MS),
            });
        }
    }

    fn dismiss(&mut self) {
        if let Some(toast) = self.current.take() {
            unsafe {
                lv_obj_delete(toast.container);
            }
        }
    }
}

impl Drop for ToastManager {
    fn drop(&mut self) {
        self.dismiss();
    }
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/src/toast.rs
git commit -m "feat(waved-ui): add toast notification overlay with auto-dismiss"
```

---

### Task 10: Screen Trait + Navigation State Machine

**Files:**
- Create: `waved/crates/waved-ui/src/nav.rs`
- Create: `waved/crates/waved-ui/src/screens/mod.rs`

- [ ] **Step 1: Write the Screen enum and navigation state machine**

Create `waved/crates/waved-ui/src/nav.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use std::sync::Arc;
use tracing::{debug, info};
use waved_config::WavedConfig;
use waved_state::StateBus;

/// All navigable screens.
#[derive(Clone, Debug, PartialEq)]
pub enum Screen {
    Home,
    Recording,
    Files,
    AudioMode,
    Wifi,
    WifiInfo,
    Bluetooth,
    Settings,
    Volume,
    Height,
    Camera,
    CameraPtz,
    ApiConfig,
    About,
    Password,
    Ota,
    PowerOff,
    Loading,
}

impl Screen {
    /// Get the parent screen for back navigation.
    pub fn parent(&self) -> Option<Screen> {
        match self {
            Screen::Home => None,
            Screen::Recording => Some(Screen::Home),
            Screen::Files => Some(Screen::Home),
            Screen::AudioMode => Some(Screen::Settings),
            Screen::Wifi => Some(Screen::Settings),
            Screen::WifiInfo => Some(Screen::Wifi),
            Screen::Bluetooth => Some(Screen::Settings),
            Screen::Settings => Some(Screen::Home),
            Screen::Volume => Some(Screen::Settings),
            Screen::Height => Some(Screen::Settings),
            Screen::Camera => Some(Screen::Settings),
            Screen::CameraPtz => Some(Screen::Camera),
            Screen::ApiConfig => Some(Screen::Settings),
            Screen::About => Some(Screen::Settings),
            Screen::Password => Some(Screen::Settings),
            Screen::Ota => Some(Screen::Settings),
            Screen::PowerOff => Some(Screen::Settings),
            Screen::Loading => None, // Loading is a transient overlay
        }
    }
}

pub struct ScreenManager {
    bus: Arc<StateBus>,
    i18n: I18n,
    config: WavedConfig,
    current_screen: Option<Screen>,
    current_impl: Option<Box<dyn ScreenTrait>>,
    /// History stack for back navigation
    history: Vec<Screen>,
}

impl ScreenManager {
    pub fn new(bus: Arc<StateBus>, i18n: I18n, config: WavedConfig) -> Self {
        Self {
            bus,
            i18n,
            config,
            current_screen: None,
            current_impl: None,
            history: Vec::with_capacity(8),
        }
    }

    /// Navigate to a new screen, pushing the current one onto the history stack.
    pub fn navigate(&mut self, screen: Screen) {
        info!("Navigating to {:?}", screen);

        // Destroy current screen
        if let Some(mut current) = self.current_impl.take() {
            current.destroy();
        }
        if let Some(prev) = self.current_screen.take() {
            self.history.push(prev);
        }

        self.show_screen(screen);
    }

    /// Go back to the previous screen in the history stack.
    pub fn go_back(&mut self) {
        if let Some(prev) = self.history.pop() {
            info!("Going back to {:?}", prev);

            if let Some(mut current) = self.current_impl.take() {
                current.destroy();
            }
            self.current_screen = None;

            self.show_screen(prev);
        } else {
            debug!("No history to go back to");
        }
    }

    fn show_screen(&mut self, screen: Screen) {
        // Create new LVGL screen object
        let scr = unsafe {
            let scr = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_style_bg_color(scr, color_hex(COLOR_BG), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(scr, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
            lv_screen_load(scr);
            scr
        };

        // Instantiate the screen implementation
        let screen_impl = crate::screens::create_screen(
            &screen,
            scr,
            &self.i18n,
            &self.config,
            &self.bus,
        );

        self.current_screen = Some(screen.clone());
        self.current_impl = Some(screen_impl);
    }

    pub fn handle_command(&mut self, cmd: &UiCommand) {
        match cmd {
            UiCommand::NavigateTo(screen) => {
                self.navigate(screen.clone());
            }
            _ => {
                // Forward to current screen
                if let Some(screen) = &mut self.current_impl {
                    if let Some(action) = screen.handle_command(cmd) {
                        self.dispatch_action(action);
                    }
                }
            }
        }
    }

    /// Dispatch a UiAction from a screen to the StateBus commands channel.
    fn dispatch_action(&self, action: UiAction) {
        use waved_state::Command;

        let cmd = match action {
            UiAction::SetAudioMode(mode) => Command::SetAudioMode(mode),
            UiAction::ToggleMicMute => Command::ToggleMicMute,
            UiAction::ToggleSpeakerMute => Command::ToggleSpeakerMute,
            UiAction::SetSpeakerVolume(v) => Command::SetSpeakerVolume(v),
            UiAction::SetMicVolume(v) => Command::SetMicVolume(v),
            UiAction::StartRecording => Command::StartRecording,
            UiAction::StopRecording => Command::StopRecording,
            UiAction::PauseRecording => Command::PauseRecording,
            UiAction::ResumeRecording => Command::ResumeRecording,
            UiAction::AddBookmark => Command::AddBookmark,
            UiAction::RetryUpload(id) => Command::RetryUpload(id),
            UiAction::SetMotorPosition(pos) => Command::SetMotorPosition(pos),
            UiAction::Shutdown => Command::Shutdown,
            // Actions that don't map to a Command are handled locally
            _ => return,
        };

        let _ = self.bus.commands.send(cmd);
    }
}
```

- [ ] **Step 2: Write the Screen trait and factory**

Create `waved/crates/waved-ui/src/screens/mod.rs`:
```rust
pub mod home;
pub mod recording;
pub mod files;
pub mod audio_mode;
pub mod wifi;
pub mod wifi_info;
pub mod bluetooth;
pub mod settings;
pub mod volume;
pub mod height;
pub mod camera;
pub mod camera_ptz;
pub mod api_config;
pub mod about;
pub mod password;
pub mod ota;
pub mod power_off;
pub mod loading;

use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::nav::Screen;
use std::sync::Arc;
use waved_config::WavedConfig;
use waved_state::StateBus;

/// Trait that all screens implement.
pub trait ScreenTrait {
    /// Handle a UI command (state update). Return an optional action to dispatch.
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction>;

    /// Clean up LVGL objects when navigating away.
    fn destroy(&mut self);
}

/// Factory function to create a screen implementation.
pub fn create_screen(
    screen: &Screen,
    parent: *mut lv_obj_t,
    i18n: &I18n,
    config: &WavedConfig,
    bus: &Arc<StateBus>,
) -> Box<dyn ScreenTrait> {
    match screen {
        Screen::Home => Box::new(home::HomeScreen::create(parent, i18n, bus)),
        Screen::Recording => Box::new(recording::RecordingScreen::create(parent, i18n)),
        Screen::Files => Box::new(files::FilesScreen::create(parent, i18n)),
        Screen::AudioMode => Box::new(audio_mode::AudioModeScreen::create(parent, i18n, bus)),
        Screen::Wifi => Box::new(wifi::WifiScreen::create(parent, i18n)),
        Screen::WifiInfo => Box::new(wifi_info::WifiInfoScreen::create(parent, i18n)),
        Screen::Bluetooth => Box::new(bluetooth::BluetoothScreen::create(parent, i18n, bus)),
        Screen::Settings => Box::new(settings::SettingsScreen::create(parent, i18n)),
        Screen::Volume => Box::new(volume::VolumeScreen::create(parent, i18n, bus)),
        Screen::Height => Box::new(height::HeightScreen::create(parent, i18n, bus)),
        Screen::Camera => Box::new(camera::CameraScreen::create(parent, i18n, bus)),
        Screen::CameraPtz => Box::new(camera_ptz::CameraPtzScreen::create(parent, i18n)),
        Screen::ApiConfig => Box::new(api_config::ApiConfigScreen::create(parent, i18n, config)),
        Screen::About => Box::new(about::AboutScreen::create(parent, i18n, config)),
        Screen::Password => Box::new(password::PasswordScreen::create(parent, i18n)),
        Screen::Ota => Box::new(ota::OtaScreen::create(parent, i18n, config)),
        Screen::PowerOff => Box::new(power_off::PowerOffScreen::create(parent, i18n)),
        Screen::Loading => Box::new(loading::LoadingScreen::create(parent, i18n)),
    }
}
```

- [ ] **Step 3: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 4: Commit**

```bash
git add waved/crates/waved-ui/src/nav.rs waved/crates/waved-ui/src/screens/mod.rs
git commit -m "feat(waved-ui): add Screen trait, navigation state machine, and screen factory"
```

---

### Task 11: Home Screen

**Files:**
- Create: `waved/crates/waved-ui/src/screens/home.rs`

- [ ] **Step 1: Write the Home screen**

Create `waved/crates/waved-ui/src/screens/home.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use std::ffi::CString;
use std::sync::Arc;
use waved_state::StateBus;

pub struct HomeScreen {
    root: *mut lv_obj_t,
    status_label: *mut lv_obj_t,
    mic_btn: *mut lv_obj_t,
    vol_btn: *mut lv_obj_t,
    rec_btn: *mut lv_obj_t,
    nav_audio: *mut lv_obj_t,
    nav_camera: *mut lv_obj_t,
    nav_files: *mut lv_obj_t,
    nav_settings: *mut lv_obj_t,
}

impl HomeScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n, bus: &Arc<StateBus>) -> Self {
        unsafe {
            let root = parent;

            // --- Center area: connection status ---
            let center_cont = lv_obj_create(root);
            lv_obj_set_pos(center_cont, 0, STATUS_BAR_H);
            lv_obj_set_size(center_cont, SCREEN_W, CONTENT_H - 80); // leave room for buttons
            lv_obj_set_style_bg_opa(center_cont, LV_OPA_TRANSP as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(center_cont, 0, LV_PART_MAIN as u32);

            // Device name
            let name_lbl = widgets::label_sized(center_cont, i18n.tr("app.name"), 24);
            lv_obj_align(name_lbl, LV_ALIGN_CENTER as u8, 0, -20);

            // Connection status
            let status_label = widgets::label_secondary(center_cont, i18n.tr("home.disconnected"));
            lv_obj_align(status_label, LV_ALIGN_CENTER as u8, 0, 10);

            // --- Action buttons row (Mic, Volume, Record) ---
            let action_row = lv_obj_create(root);
            lv_obj_set_pos(action_row, 0, SCREEN_H - BOTTOM_BAR_H - 56);
            lv_obj_set_size(action_row, SCREEN_W, 48);
            lv_obj_set_style_bg_opa(action_row, LV_OPA_TRANSP as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(action_row, 0, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(action_row, LV_FLEX_FLOW_ROW as u32);
            lv_obj_set_flex_align(
                action_row,
                LV_FLEX_ALIGN_SPACE_EVENLY as u8,
                LV_FLEX_ALIGN_CENTER as u8,
                LV_FLEX_ALIGN_CENTER as u8,
            );

            let mic_btn = widgets::button(action_row, i18n.tr("home.mic"), 100, 40);
            let vol_btn = widgets::button(action_row, i18n.tr("home.volume"), 100, 40);
            let rec_btn = widgets::button(action_row, i18n.tr("home.record"), 100, 40);
            lv_obj_set_style_bg_color(rec_btn, color_hex(COLOR_RECORDING), LV_PART_MAIN as u32);

            // --- Bottom navigation bar ---
            let nav_bar = lv_obj_create(root);
            lv_obj_set_pos(nav_bar, 0, SCREEN_H - BOTTOM_BAR_H);
            lv_obj_set_size(nav_bar, SCREEN_W, BOTTOM_BAR_H);
            lv_obj_set_style_bg_color(nav_bar, color_hex(COLOR_BG_CARD), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(nav_bar, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(nav_bar, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_radius(nav_bar, 0, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(nav_bar, LV_FLEX_FLOW_ROW as u32);
            lv_obj_set_flex_align(
                nav_bar,
                LV_FLEX_ALIGN_SPACE_EVENLY as u8,
                LV_FLEX_ALIGN_CENTER as u8,
                LV_FLEX_ALIGN_CENTER as u8,
            );

            let nav_audio = Self::nav_btn(nav_bar, i18n.tr("nav.audio_mode"));
            let nav_camera = Self::nav_btn(nav_bar, i18n.tr("nav.camera"));
            let nav_files = Self::nav_btn(nav_bar, i18n.tr("nav.files"));
            let nav_settings = Self::nav_btn(nav_bar, i18n.tr("nav.settings"));

            // TODO: Register LVGL event callbacks on buttons to emit UiActions.
            // For now, button taps are handled by polling in handle_command.

            Self {
                root,
                status_label,
                mic_btn,
                vol_btn,
                rec_btn,
                nav_audio,
                nav_camera,
                nav_files,
                nav_settings,
            }
        }
    }

    unsafe fn nav_btn(parent: *mut lv_obj_t, text: &str) -> *mut lv_obj_t {
        let btn = lv_btn_create(parent);
        lv_obj_set_size(btn, 90, 28);
        lv_obj_set_style_bg_opa(btn, LV_OPA_TRANSP as u8, LV_PART_MAIN as u32);
        lv_obj_set_style_radius(btn, 0, LV_PART_MAIN as u32);

        let lbl = lv_label_create(btn);
        let cstr = CString::new(text).unwrap();
        lv_label_set_text(lbl, cstr.as_ptr());
        lv_obj_center(lbl);
        lv_obj_set_style_text_color(lbl, color_hex(COLOR_TEXT_SEC), LV_PART_MAIN as u32);
        lv_obj_set_style_text_font(lbl, font(12), LV_PART_MAIN as u32);

        btn
    }
}

impl ScreenTrait for HomeScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::UpdateBluetooth(state) => {
                let text = if state.connected {
                    format!("BT: {}", state.device_name)
                } else {
                    "Disconnected".to_string()
                };
                unsafe { widgets::set_label_text(self.status_label, &text); }
                None
            }
            UiCommand::UpdateMcu(state) => {
                if state.usb_connected {
                    unsafe { widgets::set_label_text(self.status_label, "USB Connected"); }
                }
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {
        // The root is the LVGL screen object — it will be cleaned up
        // by lv_screen_load() when the next screen is loaded.
    }
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/src/screens/home.rs
git commit -m "feat(waved-ui): add Home screen with connection status, action buttons, and bottom nav"
```

---

### Task 12: Recording Screen

**Files:**
- Create: `waved/crates/waved-ui/src/screens/recording.rs`

- [ ] **Step 1: Write the Recording screen**

Create `waved/crates/waved-ui/src/screens/recording.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use std::ffi::CString;

pub struct RecordingScreen {
    root: *mut lv_obj_t,
    status_label: *mut lv_obj_t,
    timer_label: *mut lv_obj_t,
    mark_btn: *mut lv_obj_t,
    pause_btn: *mut lv_obj_t,
    stop_btn: *mut lv_obj_t,
    is_paused: bool,
}

impl RecordingScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n) -> Self {
        unsafe {
            let root = parent;

            // Title bar
            let (_, back_btn) = widgets::title_bar(root, i18n.tr("recording.title"));

            // Content area
            let content = lv_obj_create(root);
            lv_obj_set_pos(content, 0, STATUS_BAR_H + 36);
            lv_obj_set_size(content, SCREEN_W, CONTENT_H - 36 - 56);
            lv_obj_set_style_bg_opa(content, LV_OPA_TRANSP as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(content, 0, LV_PART_MAIN as u32);

            // Recording status
            let status_label = widgets::label_sized(content, i18n.tr("recording.recording"), 20);
            lv_obj_align(status_label, LV_ALIGN_TOP_MID as u8, 0, 10);
            lv_obj_set_style_text_color(
                status_label,
                color_hex(COLOR_RECORDING),
                LV_PART_MAIN as u32,
            );

            // Timer display
            let timer_label = widgets::label_sized(content, "00:00:00", 28);
            lv_obj_align(timer_label, LV_ALIGN_CENTER as u8, 0, -10);

            // Action buttons row
            let btn_row = lv_obj_create(root);
            lv_obj_set_pos(btn_row, 0, SCREEN_H - BOTTOM_BAR_H - 52);
            lv_obj_set_size(btn_row, SCREEN_W, 48);
            lv_obj_set_style_bg_opa(btn_row, LV_OPA_TRANSP as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(btn_row, 0, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(btn_row, LV_FLEX_FLOW_ROW as u32);
            lv_obj_set_flex_align(
                btn_row,
                LV_FLEX_ALIGN_SPACE_EVENLY as u8,
                LV_FLEX_ALIGN_CENTER as u8,
                LV_FLEX_ALIGN_CENTER as u8,
            );

            let mark_btn = widgets::button(btn_row, i18n.tr("recording.mark"), 100, 40);
            let pause_btn = widgets::button(btn_row, i18n.tr("recording.pause"), 100, 40);
            lv_obj_set_style_bg_color(pause_btn, color_hex(COLOR_WARNING), LV_PART_MAIN as u32);
            let stop_btn = widgets::button(btn_row, i18n.tr("recording.stop"), 100, 40);
            lv_obj_set_style_bg_color(stop_btn, color_hex(COLOR_ERROR), LV_PART_MAIN as u32);

            Self {
                root,
                status_label,
                timer_label,
                mark_btn,
                pause_btn,
                stop_btn,
                is_paused: false,
            }
        }
    }

    fn format_duration(secs: u32) -> String {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{:02}:{:02}:{:02}", h, m, s)
    }
}

impl ScreenTrait for RecordingScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::RecordingTick { duration_secs } => {
                let text = Self::format_duration(*duration_secs);
                unsafe { widgets::set_label_text(self.timer_label, &text); }
                None
            }
            UiCommand::UpdateMeeting(state) => {
                if state.paused != self.is_paused {
                    self.is_paused = state.paused;
                    unsafe {
                        if state.paused {
                            widgets::set_label_text(self.status_label, "Paused");
                            lv_obj_set_style_text_color(
                                self.status_label,
                                color_hex(COLOR_WARNING),
                                LV_PART_MAIN as u32,
                            );
                        } else {
                            widgets::set_label_text(self.status_label, "Recording...");
                            lv_obj_set_style_text_color(
                                self.status_label,
                                color_hex(COLOR_RECORDING),
                                LV_PART_MAIN as u32,
                            );
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {}
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/src/screens/recording.rs
git commit -m "feat(waved-ui): add Recording screen with timer, pause, mark, and stop"
```

---

### Task 13: Files Screen

**Files:**
- Create: `waved/crates/waved-ui/src/screens/files.rs`

- [ ] **Step 1: Write the Files screen**

Create `waved/crates/waved-ui/src/screens/files.rs`:
```rust
use crate::bindings::*;
use crate::command::{RecordingFile, UiAction, UiCommand, UploadStatus};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use std::ffi::CString;

pub struct FilesScreen {
    root: *mut lv_obj_t,
    list_container: *mut lv_obj_t,
    empty_label: *mut lv_obj_t,
    file_rows: Vec<*mut lv_obj_t>,
}

impl FilesScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n) -> Self {
        unsafe {
            let root = parent;

            // Title bar with back button
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("files.title"));

            // Scrollable list container
            let list_container = lv_obj_create(root);
            lv_obj_set_pos(list_container, 0, STATUS_BAR_H + 36);
            lv_obj_set_size(list_container, SCREEN_W, SCREEN_H - STATUS_BAR_H - 36);
            lv_obj_set_style_bg_color(list_container, color_hex(COLOR_BG), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(
                list_container,
                LV_OPA_COVER as u8,
                LV_PART_MAIN as u32,
            );
            lv_obj_set_style_border_width(list_container, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_radius(list_container, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_all(list_container, PAD_SMALL, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(list_container, LV_FLEX_FLOW_COLUMN as u32);
            lv_obj_set_scrollbar_mode(list_container, LV_SCROLLBAR_MODE_AUTO as i32 as u8);

            // Empty state label
            let empty_label = widgets::label_secondary(list_container, i18n.tr("files.empty"));
            lv_obj_align(empty_label, LV_ALIGN_CENTER as u8, 0, 0);

            Self {
                root,
                list_container,
                empty_label,
                file_rows: Vec::new(),
            }
        }
    }

    fn rebuild_list(&mut self, files: &[RecordingFile]) {
        unsafe {
            // Remove existing rows
            for row in self.file_rows.drain(..) {
                lv_obj_delete(row);
            }

            if files.is_empty() {
                lv_obj_remove_flag(self.empty_label, LV_OBJ_FLAG_HIDDEN);
                return;
            }

            lv_obj_add_flag(self.empty_label, LV_OBJ_FLAG_HIDDEN);

            for file in files {
                let row = lv_obj_create(self.list_container);
                lv_obj_set_size(row, SCREEN_W - PAD_MEDIUM * 2, 60);
                lv_obj_set_style_bg_color(row, color_hex(COLOR_BG_CARD), LV_PART_MAIN as u32);
                lv_obj_set_style_radius(row, 4, LV_PART_MAIN as u32);
                lv_obj_set_style_pad_all(row, PAD_SMALL, LV_PART_MAIN as u32);
                lv_obj_set_style_border_width(row, 0, LV_PART_MAIN as u32);

                // File name
                let name = widgets::label(row, &file.name);
                lv_obj_align(name, LV_ALIGN_TOP_LEFT as u8, 0, 0);
                lv_obj_set_style_text_font(name, font(14), LV_PART_MAIN as u32);

                // Duration + size
                let dur_m = file.duration_secs / 60;
                let dur_s = file.duration_secs % 60;
                let size_mb = file.size_bytes as f64 / (1024.0 * 1024.0);
                let detail = format!("{:02}:{:02}  {:.1}MB", dur_m, dur_s, size_mb);
                let detail_lbl = widgets::label_secondary(row, &detail);
                lv_obj_align(detail_lbl, LV_ALIGN_BOTTOM_LEFT as u8, 0, 0);

                // Upload status badge
                let (status_text, status_color) = match &file.upload_status {
                    UploadStatus::Pending => ("Pending", COLOR_TEXT_SEC),
                    UploadStatus::Uploading(pct) => ("Uploading", COLOR_WARNING),
                    UploadStatus::Uploaded => ("Uploaded", COLOR_SUCCESS),
                    UploadStatus::Failed => ("Failed", COLOR_ERROR),
                };
                let badge = widgets::label(row, status_text);
                lv_obj_align(badge, LV_ALIGN_TOP_RIGHT as u8, 0, 0);
                lv_obj_set_style_text_color(
                    badge,
                    color_hex(status_color),
                    LV_PART_MAIN as u32,
                );
                lv_obj_set_style_text_font(badge, font(12), LV_PART_MAIN as u32);

                self.file_rows.push(row);
            }
        }
    }
}

impl ScreenTrait for FilesScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::FileListUpdate(files) => {
                self.rebuild_list(files);
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {
        self.file_rows.clear();
    }
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/src/screens/files.rs
git commit -m "feat(waved-ui): add Files screen with scrollable recording list and upload status"
```

---

### Task 14: Audio Mode Screen

**Files:**
- Create: `waved/crates/waved-ui/src/screens/audio_mode.rs`

- [ ] **Step 1: Write the Audio Mode screen**

Create `waved/crates/waved-ui/src/screens/audio_mode.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use std::ffi::CString;
use std::sync::Arc;
use waved_state::{AudioMode, StateBus};

pub struct AudioModeScreen {
    root: *mut lv_obj_t,
    mode_btns: Vec<(*mut lv_obj_t, AudioMode)>,
    confirm_btn: *mut lv_obj_t,
    selected: AudioMode,
}

impl AudioModeScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n, bus: &Arc<StateBus>) -> Self {
        let current_mode = bus.audio.borrow().mode.clone();

        unsafe {
            let root = parent;

            // Title bar
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("audio_mode.title"));

            // Mode list container
            let list = lv_obj_create(root);
            lv_obj_set_pos(list, PAD_MEDIUM, STATUS_BAR_H + 40);
            lv_obj_set_size(list, SCREEN_W - PAD_MEDIUM * 2, CONTENT_H - 80);
            lv_obj_set_style_bg_color(list, color_hex(COLOR_BG_CARD), LV_PART_MAIN as u32);
            lv_obj_set_style_radius(list, 8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(list, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_all(list, PAD_SMALL, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(list, LV_FLEX_FLOW_COLUMN as u32);
            lv_obj_set_scrollbar_mode(list, LV_SCROLLBAR_MODE_AUTO as i32 as u8);

            let modes = [
                (AudioMode::NoiseReduction, "audio_mode.noise_reduction"),
                (AudioMode::Omni, "audio_mode.omni"),
                (AudioMode::Cardioid, "audio_mode.cardioid"),
                (AudioMode::Supercardioid, "audio_mode.supercardioid"),
                (AudioMode::Figure8, "audio_mode.figure8"),
                (AudioMode::Stereo, "audio_mode.stereo"),
            ];

            let mut mode_btns = Vec::new();
            for (mode, key) in &modes {
                let btn = lv_btn_create(list);
                lv_obj_set_size(btn, SCREEN_W - PAD_MEDIUM * 4, 36);
                lv_obj_set_style_radius(btn, 4, LV_PART_MAIN as u32);

                let is_selected = *mode == current_mode;
                let bg = if is_selected { COLOR_ACCENT } else { COLOR_BG_CARD };
                lv_obj_set_style_bg_color(btn, color_hex(bg), LV_PART_MAIN as u32);

                let lbl = lv_label_create(btn);
                let text = i18n.tr(key);
                let prefix = if is_selected { "* " } else { "  " };
                let display_text = format!("{}{}", prefix, text);
                let cstr = CString::new(display_text).unwrap();
                lv_label_set_text(lbl, cstr.as_ptr());
                lv_obj_align(lbl, LV_ALIGN_LEFT_MID as u8, 0, 0);
                lv_obj_set_style_text_color(lbl, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);

                mode_btns.push((btn, mode.clone()));
            }

            // Confirm button
            let confirm_btn = widgets::button(root, i18n.tr("audio_mode.confirm"), 160, 40);
            lv_obj_align(confirm_btn, LV_ALIGN_BOTTOM_MID as u8, 0, -PAD_MEDIUM);
            lv_obj_set_style_bg_color(
                confirm_btn,
                color_hex(COLOR_ACCENT),
                LV_PART_MAIN as u32,
            );

            Self {
                root,
                mode_btns,
                confirm_btn,
                selected: current_mode,
            }
        }
    }
}

impl ScreenTrait for AudioModeScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::UpdateAudio(state) => {
                // Reflect external mode change if it happens while on this screen
                self.selected = state.mode.clone();
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {
        self.mode_btns.clear();
    }
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/src/screens/audio_mode.rs
git commit -m "feat(waved-ui): add Audio Mode screen with 6 mode radio selection"
```

---

### Task 15: Settings, WiFi, WiFi Info, and Bluetooth Screens

**Files:**
- Create: `waved/crates/waved-ui/src/screens/settings.rs`
- Create: `waved/crates/waved-ui/src/screens/wifi.rs`
- Create: `waved/crates/waved-ui/src/screens/wifi_info.rs`
- Create: `waved/crates/waved-ui/src/screens/bluetooth.rs`

- [ ] **Step 1: Write the Settings menu screen**

Create `waved/crates/waved-ui/src/screens/settings.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;

pub struct SettingsScreen {
    root: *mut lv_obj_t,
    items: Vec<*mut lv_obj_t>,
}

impl SettingsScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n) -> Self {
        unsafe {
            let root = parent;
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("settings.title"));

            let list = lv_obj_create(root);
            lv_obj_set_pos(list, 0, STATUS_BAR_H + 36);
            lv_obj_set_size(list, SCREEN_W, SCREEN_H - STATUS_BAR_H - 36);
            lv_obj_set_style_bg_color(list, color_hex(COLOR_BG), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(list, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(list, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_radius(list, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_all(list, PAD_SMALL, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(list, LV_FLEX_FLOW_COLUMN as u32);
            lv_obj_set_scrollbar_mode(list, LV_SCROLLBAR_MODE_AUTO as i32 as u8);

            let menu_items = [
                "settings.audio_mode",
                "settings.camera",
                "settings.volume",
                "settings.wifi",
                "settings.bluetooth",
                "settings.height",
                "settings.api",
                "settings.password",
                "settings.about",
                "settings.ota",
                "settings.power_off",
            ];

            let mut items = Vec::new();
            for key in &menu_items {
                let item = widgets::list_item(list, i18n.tr(key), true);
                items.push(item);
            }

            Self { root, items }
        }
    }
}

impl ScreenTrait for SettingsScreen {
    fn handle_command(&mut self, _cmd: &UiCommand) -> Option<UiAction> {
        None
    }

    fn destroy(&mut self) {
        self.items.clear();
    }
}
```

- [ ] **Step 2: Write the WiFi screen**

Create `waved/crates/waved-ui/src/screens/wifi.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand, WifiNetwork};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use std::ffi::CString;

pub struct WifiScreen {
    root: *mut lv_obj_t,
    list: *mut lv_obj_t,
    scanning_label: *mut lv_obj_t,
    network_rows: Vec<*mut lv_obj_t>,
}

impl WifiScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n) -> Self {
        unsafe {
            let root = parent;
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("wifi.title"));

            let list = lv_obj_create(root);
            lv_obj_set_pos(list, 0, STATUS_BAR_H + 36);
            lv_obj_set_size(list, SCREEN_W, SCREEN_H - STATUS_BAR_H - 36);
            lv_obj_set_style_bg_color(list, color_hex(COLOR_BG), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(list, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(list, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_radius(list, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_all(list, PAD_SMALL, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(list, LV_FLEX_FLOW_COLUMN as u32);
            lv_obj_set_scrollbar_mode(list, LV_SCROLLBAR_MODE_AUTO as i32 as u8);

            // Section header
            let _hdr = widgets::label_secondary(list, i18n.tr("wifi.available"));

            // Scanning indicator
            let scanning_label = widgets::label_secondary(list, i18n.tr("wifi.scanning"));

            Self {
                root,
                list,
                scanning_label,
                network_rows: Vec::new(),
            }
        }
    }

    fn rebuild_network_list(&mut self, networks: &[WifiNetwork]) {
        unsafe {
            for row in self.network_rows.drain(..) {
                lv_obj_delete(row);
            }

            lv_obj_add_flag(self.scanning_label, LV_OBJ_FLAG_HIDDEN);

            for net in networks {
                let signal_indicator = match net.signal {
                    -50..=0 => "+++",
                    -70..=-51 => "++",
                    -80..=-71 => "+",
                    _ => ".",
                };
                let lock = if net.secured { " [lock]" } else { "" };
                let connected = if net.connected { " *" } else { "" };
                let text = format!(
                    "{} {}{}{}",
                    signal_indicator, net.ssid, lock, connected
                );

                let row = widgets::list_item(self.list, &text, true);
                self.network_rows.push(row);
            }
        }
    }
}

impl ScreenTrait for WifiScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::WifiScanResults(networks) => {
                self.rebuild_network_list(networks);
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {
        self.network_rows.clear();
    }
}
```

- [ ] **Step 3: Write the WiFi Info screen**

Create `waved/crates/waved-ui/src/screens/wifi_info.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;

pub struct WifiInfoScreen {
    root: *mut lv_obj_t,
    status_label: *mut lv_obj_t,
    ip_label: *mut lv_obj_t,
    signal_label: *mut lv_obj_t,
    forget_btn: *mut lv_obj_t,
}

impl WifiInfoScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n) -> Self {
        unsafe {
            let root = parent;
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("wifi_info.title"));

            let content = widgets::content_container(root);
            lv_obj_set_pos(content, 0, STATUS_BAR_H + 36);
            lv_obj_set_size(content, SCREEN_W, CONTENT_H - 36);
            lv_obj_set_style_pad_all(content, PAD_LARGE, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(content, LV_FLEX_FLOW_COLUMN as u32);

            let (_, status_label) =
                widgets::list_item_value(content, i18n.tr("wifi_info.status"), "---");
            let (_, signal_label) =
                widgets::list_item_value(content, i18n.tr("wifi_info.signal"), "---");
            let (_, ip_label) =
                widgets::list_item_value(content, i18n.tr("wifi_info.ip"), "---");

            let forget_btn = widgets::button(
                content,
                i18n.tr("wifi_info.forget"),
                SCREEN_W - PAD_LARGE * 4,
                40,
            );
            lv_obj_set_style_bg_color(
                forget_btn,
                color_hex(COLOR_ERROR),
                LV_PART_MAIN as u32,
            );

            Self {
                root,
                status_label,
                ip_label,
                signal_label,
                forget_btn,
            }
        }
    }
}

impl ScreenTrait for WifiInfoScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::UpdateWifi(state) => {
                unsafe {
                    let status = if state.connected { "Connected" } else { "Disconnected" };
                    widgets::set_label_text(self.status_label, status);
                    widgets::set_label_text(self.ip_label, &state.ip_addr);

                    let signal = format!("{} dBm", state.signal_strength);
                    widgets::set_label_text(self.signal_label, &signal);
                }
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {}
}
```

- [ ] **Step 4: Write the Bluetooth screen**

Create `waved/crates/waved-ui/src/screens/bluetooth.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use std::sync::Arc;
use waved_state::StateBus;

pub struct BluetoothScreen {
    root: *mut lv_obj_t,
    status_label: *mut lv_obj_t,
    bt_switch: *mut lv_obj_t,
    auto_reconnect_switch: *mut lv_obj_t,
    connect_btn: *mut lv_obj_t,
}

impl BluetoothScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n, bus: &Arc<StateBus>) -> Self {
        let bt_state = bus.bt.borrow().clone();

        unsafe {
            let root = parent;
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("bt.title"));

            let content = lv_obj_create(root);
            lv_obj_set_pos(content, 0, STATUS_BAR_H + 36);
            lv_obj_set_size(content, SCREEN_W, SCREEN_H - STATUS_BAR_H - 36);
            lv_obj_set_style_bg_color(content, color_hex(COLOR_BG), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(content, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(content, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_radius(content, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_all(content, PAD_SMALL, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(content, LV_FLEX_FLOW_COLUMN as u32);

            // Status text
            let status_text = if bt_state.connected {
                format!("{}: {}", i18n.tr("bt.connected_to"), bt_state.device_name)
            } else {
                i18n.tr("bt.disconnected").to_string()
            };
            let status_label = widgets::label(content, &status_text);

            // BT toggle
            let (_, bt_switch) = widgets::toggle_row(content, i18n.tr("bt.on"), bt_state.bt_on);

            // Auto reconnect toggle
            let (_, auto_reconnect_switch) =
                widgets::toggle_row(content, i18n.tr("bt.auto_reconnect"), true);

            // Connect new device button
            let connect_btn = widgets::button(
                content,
                i18n.tr("bt.connect_new"),
                SCREEN_W - PAD_MEDIUM * 4,
                40,
            );

            Self {
                root,
                status_label,
                bt_switch,
                auto_reconnect_switch,
                connect_btn,
            }
        }
    }
}

impl ScreenTrait for BluetoothScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::UpdateBluetooth(state) => {
                let text = if state.connected {
                    format!("Connected: {}", state.device_name)
                } else {
                    "Disconnected".to_string()
                };
                unsafe { widgets::set_label_text(self.status_label, &text); }
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {}
}
```

- [ ] **Step 5: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 6: Commit**

```bash
git add waved/crates/waved-ui/src/screens/settings.rs \
       waved/crates/waved-ui/src/screens/wifi.rs \
       waved/crates/waved-ui/src/screens/wifi_info.rs \
       waved/crates/waved-ui/src/screens/bluetooth.rs
git commit -m "feat(waved-ui): add Settings menu, WiFi, WiFi Info, and Bluetooth screens"
```

---

### Task 16: Volume and Height Screens

**Files:**
- Create: `waved/crates/waved-ui/src/screens/volume.rs`
- Create: `waved/crates/waved-ui/src/screens/height.rs`

- [ ] **Step 1: Write the Volume screen (speaker + mic sliders)**

Create `waved/crates/waved-ui/src/screens/volume.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use std::sync::Arc;
use waved_state::StateBus;

pub struct VolumeScreen {
    root: *mut lv_obj_t,
    speaker_slider: *mut lv_obj_t,
    speaker_value_label: *mut lv_obj_t,
    mic_slider: *mut lv_obj_t,
    mic_value_label: *mut lv_obj_t,
}

impl VolumeScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n, bus: &Arc<StateBus>) -> Self {
        let audio = bus.audio.borrow().clone();

        unsafe {
            let root = parent;
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("volume.title"));

            let content = lv_obj_create(root);
            lv_obj_set_pos(content, 0, STATUS_BAR_H + 40);
            lv_obj_set_size(content, SCREEN_W, CONTENT_H - 40);
            lv_obj_set_style_bg_opa(content, LV_OPA_TRANSP as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(content, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_all(content, PAD_LARGE, LV_PART_MAIN as u32);

            // Speaker volume
            let _spk_label = widgets::label(content, i18n.tr("volume.speaker"));
            lv_obj_align(_spk_label, LV_ALIGN_TOP_LEFT as u8, 0, 0);

            let spk_pct = (audio.speaker_volume as i32 * 100) / 255;
            let speaker_slider = widgets::slider(content, 0, 255, audio.speaker_volume as i32, 300);
            lv_obj_align(speaker_slider, LV_ALIGN_TOP_MID as u8, 0, 30);

            let speaker_value_label =
                widgets::label_secondary(content, &format!("{}%", spk_pct));
            lv_obj_align(speaker_value_label, LV_ALIGN_TOP_RIGHT as u8, 0, 30);

            // Mic volume
            let _mic_label = widgets::label(content, i18n.tr("volume.mic"));
            lv_obj_align(_mic_label, LV_ALIGN_TOP_LEFT as u8, 0, 80);

            let mic_pct = (audio.mic_volume as i32 * 100) / 255;
            let mic_slider = widgets::slider(content, 0, 255, audio.mic_volume as i32, 300);
            lv_obj_align(mic_slider, LV_ALIGN_TOP_MID as u8, 0, 110);

            let mic_value_label =
                widgets::label_secondary(content, &format!("{}%", mic_pct));
            lv_obj_align(mic_value_label, LV_ALIGN_TOP_RIGHT as u8, 0, 110);

            Self {
                root,
                speaker_slider,
                speaker_value_label,
                mic_slider,
                mic_value_label,
            }
        }
    }
}

impl ScreenTrait for VolumeScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::UpdateAudio(state) => {
                unsafe {
                    lv_slider_set_value(
                        self.speaker_slider,
                        state.speaker_volume as i32,
                        LV_ANIM_ON as u32,
                    );
                    let pct = (state.speaker_volume as i32 * 100) / 255;
                    widgets::set_label_text(self.speaker_value_label, &format!("{}%", pct));

                    lv_slider_set_value(
                        self.mic_slider,
                        state.mic_volume as i32,
                        LV_ANIM_ON as u32,
                    );
                    let pct = (state.mic_volume as i32 * 100) / 255;
                    widgets::set_label_text(self.mic_value_label, &format!("{}%", pct));
                }
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {}
}
```

- [ ] **Step 2: Write the Height screen**

Create `waved/crates/waved-ui/src/screens/height.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use std::ffi::CString;
use std::sync::Arc;
use waved_state::{MotorPosition, StateBus};

pub struct HeightScreen {
    root: *mut lv_obj_t,
    low_btn: *mut lv_obj_t,
    med_btn: *mut lv_obj_t,
    high_btn: *mut lv_obj_t,
    auto_lift_switch: *mut lv_obj_t,
    current: MotorPosition,
}

impl HeightScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n, bus: &Arc<StateBus>) -> Self {
        let mcu = bus.mcu.borrow().clone();

        unsafe {
            let root = parent;
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("height.title"));

            let content = lv_obj_create(root);
            lv_obj_set_pos(content, 0, STATUS_BAR_H + 40);
            lv_obj_set_size(content, SCREEN_W, CONTENT_H - 40);
            lv_obj_set_style_bg_opa(content, LV_OPA_TRANSP as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(content, 0, LV_PART_MAIN as u32);

            // Height preset buttons
            let btn_row = lv_obj_create(content);
            lv_obj_set_size(btn_row, SCREEN_W - PAD_LARGE * 2, 50);
            lv_obj_center(btn_row);
            lv_obj_set_style_bg_opa(btn_row, LV_OPA_TRANSP as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(btn_row, 0, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(btn_row, LV_FLEX_FLOW_ROW as u32);
            lv_obj_set_flex_align(
                btn_row,
                LV_FLEX_ALIGN_SPACE_EVENLY as u8,
                LV_FLEX_ALIGN_CENTER as u8,
                LV_FLEX_ALIGN_CENTER as u8,
            );

            let low_btn = widgets::button(btn_row, i18n.tr("height.low"), 100, 40);
            let med_btn = widgets::button(btn_row, i18n.tr("height.medium"), 100, 40);
            let high_btn = widgets::button(btn_row, i18n.tr("height.high"), 100, 40);

            // Highlight current position
            let active_btn = match &mcu.motor_position {
                MotorPosition::Low => low_btn,
                MotorPosition::Medium => med_btn,
                MotorPosition::High => high_btn,
                MotorPosition::Moving => low_btn, // default during movement
            };
            lv_obj_set_style_bg_color(active_btn, color_hex(COLOR_ACCENT), LV_PART_MAIN as u32);

            // Auto-lift toggle
            let (_, auto_lift_switch) = widgets::toggle_row(
                content,
                i18n.tr("height.auto_lift"),
                true,
            );
            let desc = widgets::label_secondary(content, i18n.tr("height.auto_lift_desc"));

            Self {
                root,
                low_btn,
                med_btn,
                high_btn,
                auto_lift_switch,
                current: mcu.motor_position,
            }
        }
    }

    fn update_highlight(&self) {
        unsafe {
            // Reset all to default
            lv_obj_set_style_bg_color(self.low_btn, color_hex(COLOR_PRIMARY), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_color(self.med_btn, color_hex(COLOR_PRIMARY), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_color(
                self.high_btn,
                color_hex(COLOR_PRIMARY),
                LV_PART_MAIN as u32,
            );

            let active = match &self.current {
                MotorPosition::Low => self.low_btn,
                MotorPosition::Medium => self.med_btn,
                MotorPosition::High => self.high_btn,
                MotorPosition::Moving => return,
            };
            lv_obj_set_style_bg_color(active, color_hex(COLOR_ACCENT), LV_PART_MAIN as u32);
        }
    }
}

impl ScreenTrait for HeightScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::UpdateMcu(state) => {
                if state.motor_position != self.current {
                    self.current = state.motor_position.clone();
                    self.update_highlight();
                }
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {}
}
```

- [ ] **Step 3: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 4: Commit**

```bash
git add waved/crates/waved-ui/src/screens/volume.rs \
       waved/crates/waved-ui/src/screens/height.rs
git commit -m "feat(waved-ui): add Volume (speaker+mic sliders) and Height (motor presets) screens"
```

---

### Task 17: Camera and Camera PTZ Screens

**Files:**
- Create: `waved/crates/waved-ui/src/screens/camera.rs`
- Create: `waved/crates/waved-ui/src/screens/camera_ptz.rs`

- [ ] **Step 1: Write the Camera screen**

Create `waved/crates/waved-ui/src/screens/camera.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use std::sync::Arc;
use waved_state::StateBus;

pub struct CameraScreen {
    root: *mut lv_obj_t,
    auto_frame_switch: *mut lv_obj_t,
    whiteboard_switch: *mut lv_obj_t,
    manual_btn: *mut lv_obj_t,
}

impl CameraScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n, bus: &Arc<StateBus>) -> Self {
        let puc = bus.puc.borrow().clone();

        unsafe {
            let root = parent;
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("camera.title"));

            let content = lv_obj_create(root);
            lv_obj_set_pos(content, 0, STATUS_BAR_H + 36);
            lv_obj_set_size(content, SCREEN_W, SCREEN_H - STATUS_BAR_H - 36);
            lv_obj_set_style_bg_color(content, color_hex(COLOR_BG), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(content, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(content, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_radius(content, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_all(content, PAD_SMALL, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(content, LV_FLEX_FLOW_COLUMN as u32);

            // Auto framing toggle
            let (_, auto_frame_switch) = widgets::toggle_row(
                content,
                i18n.tr("camera.auto_framing"),
                puc.auto_framing,
            );
            let _desc = widgets::label_secondary(content, i18n.tr("camera.auto_framing_desc"));

            // Smart whiteboard toggle
            let (_, whiteboard_switch) = widgets::toggle_row(
                content,
                i18n.tr("camera.whiteboard"),
                false,
            );

            // Manual adjust button -> navigates to CameraPtz
            let manual_btn = widgets::list_item(content, i18n.tr("camera.manual"), true);

            Self {
                root,
                auto_frame_switch,
                whiteboard_switch,
                manual_btn,
            }
        }
    }
}

impl ScreenTrait for CameraScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::UpdatePuc(state) => {
                unsafe {
                    if state.auto_framing {
                        lv_obj_add_state(self.auto_frame_switch, LV_STATE_CHECKED as u16);
                    } else {
                        lv_obj_remove_state(self.auto_frame_switch, LV_STATE_CHECKED as u16);
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {}
}
```

- [ ] **Step 2: Write the Camera PTZ screen**

Create `waved/crates/waved-ui/src/screens/camera_ptz.rs`:
```rust
use crate::bindings::*;
use crate::command::{PtzDirection, UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;

pub struct CameraPtzScreen {
    root: *mut lv_obj_t,
    up_btn: *mut lv_obj_t,
    down_btn: *mut lv_obj_t,
    left_btn: *mut lv_obj_t,
    right_btn: *mut lv_obj_t,
    zoom_in_btn: *mut lv_obj_t,
    zoom_out_btn: *mut lv_obj_t,
    reset_btn: *mut lv_obj_t,
}

impl CameraPtzScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n) -> Self {
        unsafe {
            let root = parent;
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("camera_ptz.title"));

            let content = lv_obj_create(root);
            lv_obj_set_pos(content, 0, STATUS_BAR_H + 40);
            lv_obj_set_size(content, SCREEN_W, CONTENT_H - 40);
            lv_obj_set_style_bg_opa(content, LV_OPA_TRANSP as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(content, 0, LV_PART_MAIN as u32);

            // D-pad layout
            let up_btn = widgets::button(content, "^", 60, 50);
            lv_obj_align(up_btn, LV_ALIGN_CENTER as u8, 0, -60);

            let down_btn = widgets::button(content, "v", 60, 50);
            lv_obj_align(down_btn, LV_ALIGN_CENTER as u8, 0, 10);

            let left_btn = widgets::button(content, "<", 60, 50);
            lv_obj_align(left_btn, LV_ALIGN_CENTER as u8, -70, -25);

            let right_btn = widgets::button(content, ">", 60, 50);
            lv_obj_align(right_btn, LV_ALIGN_CENTER as u8, 70, -25);

            // Zoom row
            let zoom_out_btn = widgets::button(content, "-", 60, 36);
            lv_obj_align(zoom_out_btn, LV_ALIGN_BOTTOM_LEFT as u8, PAD_LARGE, -PAD_LARGE);

            let zoom_label = widgets::label(content, i18n.tr("camera_ptz.zoom"));
            lv_obj_align(zoom_label, LV_ALIGN_BOTTOM_MID as u8, 0, -PAD_LARGE - 8);

            let zoom_in_btn = widgets::button(content, "+", 60, 36);
            lv_obj_align(zoom_in_btn, LV_ALIGN_BOTTOM_RIGHT as u8, -PAD_LARGE, -PAD_LARGE);

            // Reset button in title bar area
            let reset_btn = widgets::button(root, i18n.tr("camera_ptz.reset"), 80, 28);
            lv_obj_align(reset_btn, LV_ALIGN_TOP_RIGHT as u8, -PAD_SMALL, STATUS_BAR_H + 4);

            Self {
                root,
                up_btn,
                down_btn,
                left_btn,
                right_btn,
                zoom_in_btn,
                zoom_out_btn,
                reset_btn,
            }
        }
    }
}

impl ScreenTrait for CameraPtzScreen {
    fn handle_command(&mut self, _cmd: &UiCommand) -> Option<UiAction> {
        // PTZ screen is primarily driven by button callbacks.
        // Event callbacks would emit:
        //   UiAction::PucPan(PtzDirection::Up/Down/Left/Right)
        //   UiAction::PucZoom(+1 / -1)
        //   UiAction::PucReset
        None
    }

    fn destroy(&mut self) {}
}
```

- [ ] **Step 3: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 4: Commit**

```bash
git add waved/crates/waved-ui/src/screens/camera.rs \
       waved/crates/waved-ui/src/screens/camera_ptz.rs
git commit -m "feat(waved-ui): add Camera (auto-frame, whiteboard) and PTZ (D-pad, zoom) screens"
```

---

### Task 18: API Config, About, Password, OTA, Power Off, and Loading Screens

**Files:**
- Create: `waved/crates/waved-ui/src/screens/api_config.rs`
- Create: `waved/crates/waved-ui/src/screens/about.rs`
- Create: `waved/crates/waved-ui/src/screens/password.rs`
- Create: `waved/crates/waved-ui/src/screens/ota.rs`
- Create: `waved/crates/waved-ui/src/screens/power_off.rs`
- Create: `waved/crates/waved-ui/src/screens/loading.rs`

- [ ] **Step 1: Write the API Config screen (endpoint URL + auth token via keyboard)**

Create `waved/crates/waved-ui/src/screens/api_config.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use std::ffi::CString;
use waved_config::WavedConfig;

pub struct ApiConfigScreen {
    root: *mut lv_obj_t,
    endpoint_ta: *mut lv_obj_t,
    token_ta: *mut lv_obj_t,
    keyboard: *mut lv_obj_t,
    auto_upload_switch: *mut lv_obj_t,
    save_btn: *mut lv_obj_t,
}

impl ApiConfigScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n, config: &WavedConfig) -> Self {
        unsafe {
            let root = parent;
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("api.title"));

            let content = lv_obj_create(root);
            lv_obj_set_pos(content, 0, STATUS_BAR_H + 36);
            lv_obj_set_size(content, SCREEN_W, 140);
            lv_obj_set_style_bg_opa(content, LV_OPA_TRANSP as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(content, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_all(content, PAD_MEDIUM, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(content, LV_FLEX_FLOW_COLUMN as u32);

            // Endpoint URL label + textarea
            let _ep_label = widgets::label(content, i18n.tr("api.endpoint"));
            let endpoint_ta = lv_textarea_create(content);
            lv_obj_set_size(endpoint_ta, SCREEN_W - PAD_LARGE * 2, 32);
            let ep_str = CString::new(config.api.endpoint.as_str()).unwrap();
            lv_textarea_set_text(endpoint_ta, ep_str.as_ptr());
            lv_textarea_set_one_line(endpoint_ta, true);
            lv_obj_set_style_bg_color(
                endpoint_ta,
                color_hex(COLOR_BG_CARD),
                LV_PART_MAIN as u32,
            );
            lv_obj_set_style_text_color(
                endpoint_ta,
                color_hex(COLOR_TEXT),
                LV_PART_MAIN as u32,
            );

            // Token label + textarea
            let _tok_label = widgets::label(content, i18n.tr("api.token"));
            let token_ta = lv_textarea_create(content);
            lv_obj_set_size(token_ta, SCREEN_W - PAD_LARGE * 2, 32);
            let tok_str = CString::new(config.api.auth_token.as_str()).unwrap();
            lv_textarea_set_text(token_ta, tok_str.as_ptr());
            lv_textarea_set_one_line(token_ta, true);
            lv_textarea_set_password_mode(token_ta, true);
            lv_obj_set_style_bg_color(token_ta, color_hex(COLOR_BG_CARD), LV_PART_MAIN as u32);
            lv_obj_set_style_text_color(token_ta, color_hex(COLOR_TEXT), LV_PART_MAIN as u32);

            // Auto upload toggle
            let (_, auto_upload_switch) =
                widgets::toggle_row(content, i18n.tr("api.auto_upload"), config.api.auto_upload);

            // Save button
            let save_btn = widgets::button(root, i18n.tr("api.save"), 160, 40);
            lv_obj_align(save_btn, LV_ALIGN_BOTTOM_MID as u8, 0, -80);
            lv_obj_set_style_bg_color(save_btn, color_hex(COLOR_ACCENT), LV_PART_MAIN as u32);

            // LVGL keyboard (shown at bottom, associates with focused textarea)
            let keyboard = lv_keyboard_create(root);
            lv_obj_set_size(keyboard, SCREEN_W, 120);
            lv_obj_align(keyboard, LV_ALIGN_BOTTOM_MID as u8, 0, 0);
            lv_keyboard_set_textarea(keyboard, endpoint_ta);

            Self {
                root,
                endpoint_ta,
                token_ta,
                keyboard,
                auto_upload_switch,
                save_btn,
            }
        }
    }
}

impl ScreenTrait for ApiConfigScreen {
    fn handle_command(&mut self, _cmd: &UiCommand) -> Option<UiAction> {
        // Save action would read textarea values and emit UiAction::SetApiEndpoint + SetApiToken.
        // Keyboard focus switching handled by LVGL's built-in textarea focus logic.
        None
    }

    fn destroy(&mut self) {}
}
```

- [ ] **Step 2: Write the About screen**

Create `waved/crates/waved-ui/src/screens/about.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use waved_config::WavedConfig;

pub struct AboutScreen {
    root: *mut lv_obj_t,
    ip_label: *mut lv_obj_t,
}

impl AboutScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n, config: &WavedConfig) -> Self {
        unsafe {
            let root = parent;
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("about.title"));

            let content = lv_obj_create(root);
            lv_obj_set_pos(content, 0, STATUS_BAR_H + 36);
            lv_obj_set_size(content, SCREEN_W, SCREEN_H - STATUS_BAR_H - 36);
            lv_obj_set_style_bg_color(content, color_hex(COLOR_BG), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(content, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(content, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_radius(content, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_all(content, PAD_MEDIUM, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(content, LV_FLEX_FLOW_COLUMN as u32);
            lv_obj_set_scrollbar_mode(content, LV_SCROLLBAR_MODE_AUTO as i32 as u8);

            let version = env!("CARGO_PKG_VERSION");

            widgets::list_item_value(content, i18n.tr("about.model"), "Insta360 Wave");
            widgets::list_item_value(content, i18n.tr("about.serial"), "---");
            widgets::list_item_value(content, i18n.tr("about.firmware"), version);

            let (_, ip_label) = widgets::list_item_value(content, i18n.tr("about.ip"), "---");
            widgets::list_item_value(content, i18n.tr("about.storage"), "---");

            Self { root, ip_label }
        }
    }
}

impl ScreenTrait for AboutScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::UpdateWifi(state) => {
                if !state.ip_addr.is_empty() {
                    unsafe { widgets::set_label_text(self.ip_label, &state.ip_addr); }
                }
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {}
}
```

- [ ] **Step 3: Write the Password screen**

Create `waved/crates/waved-ui/src/screens/password.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;

pub struct PasswordScreen {
    root: *mut lv_obj_t,
    enable_switch: *mut lv_obj_t,
    change_btn: *mut lv_obj_t,
}

impl PasswordScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n) -> Self {
        unsafe {
            let root = parent;
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("password.title"));

            let content = lv_obj_create(root);
            lv_obj_set_pos(content, 0, STATUS_BAR_H + 36);
            lv_obj_set_size(content, SCREEN_W, SCREEN_H - STATUS_BAR_H - 36);
            lv_obj_set_style_bg_color(content, color_hex(COLOR_BG), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(content, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(content, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_radius(content, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_all(content, PAD_SMALL, LV_PART_MAIN as u32);
            lv_obj_set_flex_flow(content, LV_FLEX_FLOW_COLUMN as u32);

            let (_, enable_switch) =
                widgets::toggle_row(content, i18n.tr("password.enable"), false);

            let change_btn = widgets::list_item(content, i18n.tr("password.change"), true);

            Self {
                root,
                enable_switch,
                change_btn,
            }
        }
    }
}

impl ScreenTrait for PasswordScreen {
    fn handle_command(&mut self, _cmd: &UiCommand) -> Option<UiAction> {
        None
    }

    fn destroy(&mut self) {}
}
```

- [ ] **Step 4: Write the OTA screen**

Create `waved/crates/waved-ui/src/screens/ota.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use std::ffi::CString;
use waved_config::WavedConfig;

pub struct OtaScreen {
    root: *mut lv_obj_t,
    version_label: *mut lv_obj_t,
    status_label: *mut lv_obj_t,
    progress_bar: *mut lv_obj_t,
    check_btn: *mut lv_obj_t,
    update_btn: *mut lv_obj_t,
}

impl OtaScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n, config: &WavedConfig) -> Self {
        unsafe {
            let root = parent;
            let (_, _back_btn) = widgets::title_bar(root, i18n.tr("ota.title"));

            let content = lv_obj_create(root);
            lv_obj_set_pos(content, 0, STATUS_BAR_H + 36);
            lv_obj_set_size(content, SCREEN_W, SCREEN_H - STATUS_BAR_H - 36);
            lv_obj_set_style_bg_color(content, color_hex(COLOR_BG), LV_PART_MAIN as u32);
            lv_obj_set_style_bg_opa(content, LV_OPA_COVER as u8, LV_PART_MAIN as u32);
            lv_obj_set_style_border_width(content, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_radius(content, 0, LV_PART_MAIN as u32);
            lv_obj_set_style_pad_all(content, PAD_LARGE, LV_PART_MAIN as u32);

            // Current version
            let version_text = format!(
                "{}: {}",
                i18n.tr("ota.current"),
                env!("CARGO_PKG_VERSION")
            );
            let version_label = widgets::label(content, &version_text);
            lv_obj_align(version_label, LV_ALIGN_TOP_LEFT as u8, 0, 0);

            // Status label
            let status_label = widgets::label_secondary(content, "");
            lv_obj_align(status_label, LV_ALIGN_TOP_LEFT as u8, 0, 30);

            // Progress bar (hidden by default)
            let progress_bar = lv_bar_create(content);
            lv_obj_set_size(progress_bar, SCREEN_W - PAD_LARGE * 4, 16);
            lv_obj_align(progress_bar, LV_ALIGN_CENTER as u8, 0, 0);
            lv_bar_set_range(progress_bar, 0, 100);
            lv_bar_set_value(progress_bar, 0, LV_ANIM_OFF as u32);
            lv_obj_set_style_bg_color(
                progress_bar,
                color_hex(COLOR_SLIDER_BG),
                LV_PART_MAIN as u32,
            );
            lv_obj_set_style_bg_color(
                progress_bar,
                color_hex(COLOR_ACCENT),
                LV_PART_INDICATOR as u32,
            );
            lv_obj_add_flag(progress_bar, LV_OBJ_FLAG_HIDDEN);

            // Check for updates button
            let check_btn = widgets::button(content, i18n.tr("ota.check"), 200, 40);
            lv_obj_align(check_btn, LV_ALIGN_BOTTOM_LEFT as u8, 0, 0);

            // Update button (hidden until update is available)
            let update_btn = widgets::button(content, i18n.tr("ota.update"), 200, 40);
            lv_obj_align(update_btn, LV_ALIGN_BOTTOM_RIGHT as u8, 0, 0);
            lv_obj_set_style_bg_color(update_btn, color_hex(COLOR_ACCENT), LV_PART_MAIN as u32);
            lv_obj_add_flag(update_btn, LV_OBJ_FLAG_HIDDEN);

            Self {
                root,
                version_label,
                status_label,
                progress_bar,
                check_btn,
                update_btn,
            }
        }
    }
}

impl ScreenTrait for OtaScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::OtaProgress { percent, status } => {
                unsafe {
                    widgets::set_label_text(self.status_label, status);
                    lv_obj_remove_flag(self.progress_bar, LV_OBJ_FLAG_HIDDEN);
                    lv_bar_set_value(
                        self.progress_bar,
                        *percent as i32,
                        LV_ANIM_ON as u32,
                    );
                }
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {}
}
```

- [ ] **Step 5: Write the Power Off screen**

Create `waved/crates/waved-ui/src/screens/power_off.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;

pub struct PowerOffScreen {
    root: *mut lv_obj_t,
    dialog_overlay: Option<*mut lv_obj_t>,
}

impl PowerOffScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n) -> Self {
        unsafe {
            let root = parent;

            // Immediately show confirmation dialog
            let (overlay, cancel_btn, confirm_btn) = widgets::confirm_dialog(
                i18n.tr("power_off.title"),
                i18n.tr("power_off.body"),
                i18n.tr("dialog.cancel"),
                i18n.tr("dialog.confirm"),
            );

            // TODO: Register LVGL callbacks:
            //   cancel_btn -> navigate back
            //   confirm_btn -> emit UiAction::Shutdown

            Self {
                root,
                dialog_overlay: Some(overlay),
            }
        }
    }
}

impl ScreenTrait for PowerOffScreen {
    fn handle_command(&mut self, _cmd: &UiCommand) -> Option<UiAction> {
        None
    }

    fn destroy(&mut self) {
        if let Some(overlay) = self.dialog_overlay.take() {
            unsafe { lv_obj_delete(overlay); }
        }
    }
}
```

- [ ] **Step 6: Write the Loading screen**

Create `waved/crates/waved-ui/src/screens/loading.rs`:
```rust
use crate::bindings::*;
use crate::command::{UiAction, UiCommand};
use crate::i18n::I18n;
use crate::screens::ScreenTrait;
use crate::theme::*;
use crate::widgets;
use std::ffi::CString;

pub struct LoadingScreen {
    root: *mut lv_obj_t,
    spinner: *mut lv_obj_t,
    text_label: *mut lv_obj_t,
}

impl LoadingScreen {
    pub fn create(parent: *mut lv_obj_t, i18n: &I18n) -> Self {
        unsafe {
            let root = parent;

            // Spinner animation
            let spinner = lv_spinner_create(root);
            lv_obj_set_size(spinner, 60, 60);
            lv_obj_align(spinner, LV_ALIGN_CENTER as u8, 0, -20);
            lv_spinner_set_anim_params(spinner, 1000, 270);

            // Text label
            let text_label = widgets::label(root, i18n.tr("loading.text"));
            lv_obj_align(text_label, LV_ALIGN_CENTER as u8, 0, 30);
            lv_obj_set_style_text_color(
                text_label,
                color_hex(COLOR_TEXT_SEC),
                LV_PART_MAIN as u32,
            );

            Self {
                root,
                spinner,
                text_label,
            }
        }
    }
}

impl ScreenTrait for LoadingScreen {
    fn handle_command(&mut self, cmd: &UiCommand) -> Option<UiAction> {
        match cmd {
            UiCommand::ShowLoading(text) => {
                unsafe { widgets::set_label_text(self.text_label, text); }
                None
            }
            _ => None,
        }
    }

    fn destroy(&mut self) {}
}
```

- [ ] **Step 7: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 8: Commit**

```bash
git add waved/crates/waved-ui/src/screens/api_config.rs \
       waved/crates/waved-ui/src/screens/about.rs \
       waved/crates/waved-ui/src/screens/password.rs \
       waved/crates/waved-ui/src/screens/ota.rs \
       waved/crates/waved-ui/src/screens/power_off.rs \
       waved/crates/waved-ui/src/screens/loading.rs
git commit -m "feat(waved-ui): add API Config, About, Password, OTA, Power Off, and Loading screens"
```

---

### Task 19: LVGL Event Callback Bridge (Button Actions)

**Files:**
- Modify: `waved/crates/waved-ui/src/screens/home.rs`
- Modify: `waved/crates/waved-ui/src/nav.rs`

This task adds the mechanism for LVGL button callbacks to trigger `UiAction` dispatches. LVGL callbacks are `extern "C"` — they need a way to communicate back to the Rust screen logic.

- [ ] **Step 1: Add a global action queue**

Add to `waved/crates/waved-ui/src/command.rs`:
```rust
use std::sync::Mutex;

/// Global queue for actions produced by LVGL callbacks.
/// The LVGL event loop drains this queue each tick and dispatches via StateBus.
static ACTION_QUEUE: Mutex<Vec<UiAction>> = Mutex::new(Vec::new());

/// Called from LVGL C callbacks to queue an action.
pub fn enqueue_action(action: UiAction) {
    if let Ok(mut queue) = ACTION_QUEUE.lock() {
        queue.push(action);
    }
}

/// Drain all pending actions. Called from the LVGL main loop.
pub fn drain_actions() -> Vec<UiAction> {
    if let Ok(mut queue) = ACTION_QUEUE.lock() {
        std::mem::take(&mut *queue)
    } else {
        Vec::new()
    }
}

/// Global navigation queue (screen transitions from LVGL callbacks).
static NAV_QUEUE: Mutex<Vec<crate::nav::Screen>> = Mutex::new(Vec::new());

pub fn enqueue_nav(screen: crate::nav::Screen) {
    if let Ok(mut queue) = NAV_QUEUE.lock() {
        queue.push(screen);
    }
}

pub fn drain_nav() -> Vec<crate::nav::Screen> {
    if let Ok(mut queue) = NAV_QUEUE.lock() {
        std::mem::take(&mut *queue)
    } else {
        Vec::new()
    }
}
```

- [ ] **Step 2: Integrate the action/nav queues into the main loop**

Modify the LVGL main loop in `lib.rs` to drain and dispatch these queues each tick:

Add after `// 2. Process pending UI commands`:
```rust
        // 2b. Drain actions from LVGL callbacks
        for action in command::drain_actions() {
            screen_mgr.dispatch_action(action);
        }

        // 2c. Drain navigation requests from LVGL callbacks
        for screen in command::drain_nav() {
            screen_mgr.navigate(screen);
        }
```

- [ ] **Step 3: Add LVGL callback wiring to Home screen buttons**

Update `home.rs` to register LVGL event callbacks that enqueue actions:

```rust
// After creating rec_btn:
lv_obj_add_event_cb(
    rec_btn,
    Some(on_record_pressed),
    LV_EVENT_CLICKED,
    std::ptr::null_mut(),
);

lv_obj_add_event_cb(
    mic_btn,
    Some(on_mic_pressed),
    LV_EVENT_CLICKED,
    std::ptr::null_mut(),
);

lv_obj_add_event_cb(
    nav_settings,
    Some(on_nav_settings),
    LV_EVENT_CLICKED,
    std::ptr::null_mut(),
);

lv_obj_add_event_cb(
    nav_files,
    Some(on_nav_files),
    LV_EVENT_CLICKED,
    std::ptr::null_mut(),
);

lv_obj_add_event_cb(
    nav_audio,
    Some(on_nav_audio),
    LV_EVENT_CLICKED,
    std::ptr::null_mut(),
);

lv_obj_add_event_cb(
    nav_camera,
    Some(on_nav_camera),
    LV_EVENT_CLICKED,
    std::ptr::null_mut(),
);

// Callback functions (outside impl block):
unsafe extern "C" fn on_record_pressed(_e: *mut lv_event_t) {
    crate::command::enqueue_action(UiAction::StartRecording);
    crate::command::enqueue_nav(crate::nav::Screen::Recording);
}

unsafe extern "C" fn on_mic_pressed(_e: *mut lv_event_t) {
    crate::command::enqueue_action(UiAction::ToggleMicMute);
}

unsafe extern "C" fn on_nav_settings(_e: *mut lv_event_t) {
    crate::command::enqueue_nav(crate::nav::Screen::Settings);
}

unsafe extern "C" fn on_nav_files(_e: *mut lv_event_t) {
    crate::command::enqueue_nav(crate::nav::Screen::Files);
}

unsafe extern "C" fn on_nav_audio(_e: *mut lv_event_t) {
    crate::command::enqueue_nav(crate::nav::Screen::AudioMode);
}

unsafe extern "C" fn on_nav_camera(_e: *mut lv_event_t) {
    crate::command::enqueue_nav(crate::nav::Screen::Camera);
}
```

- [ ] **Step 4: Add back-button callback pattern for screens with title bars**

In any screen that uses `widgets::title_bar()`, register a back callback:
```rust
lv_obj_add_event_cb(
    back_btn,
    Some(on_back_pressed),
    LV_EVENT_CLICKED,
    std::ptr::null_mut(),
);

// At module level:
unsafe extern "C" fn on_back_pressed(_e: *mut lv_event_t) {
    // Special sentinel: navigate back
    crate::command::enqueue_nav(crate::nav::Screen::Home); // or use a GoBack sentinel
}
```

- [ ] **Step 5: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 6: Commit**

```bash
git add waved/crates/waved-ui/
git commit -m "feat(waved-ui): add LVGL callback bridge for button actions and navigation"
```

---

### Task 20: State Bus Bridge (Async-to-UI Forwarder)

**Files:**
- Modify: `waved/crates/waved-ui/src/lib.rs`

This task adds the async task that subscribes to all `StateBus` watch channels and forwards state changes as `UiCommand` messages to the UI thread via the `mpsc` channel.

- [ ] **Step 1: Write the state forwarder function**

Add to `waved/crates/waved-ui/src/lib.rs`:
```rust
/// Spawn an async task that watches all StateBus channels and forwards
/// state changes to the UI thread as UiCommand messages.
pub fn spawn_state_forwarder(
    bus: Arc<StateBus>,
    ui_tx: mpsc::Sender<UiCommand>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut battery_rx = bus.battery.subscribe();
        let mut wifi_rx = bus.wifi.subscribe();
        let mut bt_rx = bus.bt.subscribe();
        let mut audio_rx = bus.audio.subscribe();
        let mut meeting_rx = bus.meeting.subscribe();
        let mut mcu_rx = bus.mcu.subscribe();
        let mut puc_rx = bus.puc.subscribe();
        let mut events_rx = bus.events.subscribe();

        loop {
            tokio::select! {
                Ok(()) = battery_rx.changed() => {
                    let state = battery_rx.borrow_and_update().clone();
                    let _ = ui_tx.send(UiCommand::UpdateBattery(state)).await;
                }
                Ok(()) = wifi_rx.changed() => {
                    let state = wifi_rx.borrow_and_update().clone();
                    let _ = ui_tx.send(UiCommand::UpdateWifi(state)).await;
                }
                Ok(()) = bt_rx.changed() => {
                    let state = bt_rx.borrow_and_update().clone();
                    let _ = ui_tx.send(UiCommand::UpdateBluetooth(state)).await;
                }
                Ok(()) = audio_rx.changed() => {
                    let state = audio_rx.borrow_and_update().clone();
                    let _ = ui_tx.send(UiCommand::UpdateAudio(state)).await;
                }
                Ok(()) = meeting_rx.changed() => {
                    let state = meeting_rx.borrow_and_update().clone();
                    let _ = ui_tx.send(UiCommand::UpdateMeeting(state)).await;
                }
                Ok(()) = mcu_rx.changed() => {
                    let state = mcu_rx.borrow_and_update().clone();
                    let _ = ui_tx.send(UiCommand::UpdateMcu(state)).await;
                }
                Ok(()) = puc_rx.changed() => {
                    let state = puc_rx.borrow_and_update().clone();
                    let _ = ui_tx.send(UiCommand::UpdatePuc(state)).await;
                }
                Ok(event) = events_rx.recv() => {
                    // Convert device events to toast notifications
                    use waved_state::DeviceEvent;
                    let toast = match &event {
                        DeviceEvent::BtConnected(name) => {
                            Some((command::ToastKind::BtConnected, format!("BT: {}", name)))
                        }
                        DeviceEvent::BtDisconnected => {
                            Some((command::ToastKind::Info, "BT disconnected".into()))
                        }
                        DeviceEvent::UsbConnected => {
                            Some((command::ToastKind::UsbConnected, "USB connected".into()))
                        }
                        DeviceEvent::UsbDisconnected => {
                            Some((command::ToastKind::Info, "USB disconnected".into()))
                        }
                        DeviceEvent::WifiConnected(ssid) => {
                            Some((command::ToastKind::Success, format!("WiFi: {}", ssid)))
                        }
                        DeviceEvent::WifiDisconnected => {
                            Some((command::ToastKind::Warning, "WiFi disconnected".into()))
                        }
                        DeviceEvent::LowBattery(pct) => {
                            Some((command::ToastKind::Battery, format!("Battery {}%", pct)))
                        }
                        DeviceEvent::MotorObstruction => {
                            Some((command::ToastKind::Warning, "Lift obstructed".into()))
                        }
                        DeviceEvent::RecordingStarted(_) => {
                            let _ = ui_tx.send(UiCommand::NavigateTo(
                                crate::nav::Screen::Recording
                            )).await;
                            None
                        }
                        DeviceEvent::RecordingStopped(_) => {
                            let _ = ui_tx.send(UiCommand::NavigateTo(
                                crate::nav::Screen::Home
                            )).await;
                            Some((command::ToastKind::Success, "Recording saved".into()))
                        }
                        _ => None,
                    };

                    if let Some((kind, msg)) = toast {
                        let _ = ui_tx.send(UiCommand::ShowToast(kind, msg)).await;
                    }
                }
            }
        }
    })
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-ui
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/src/lib.rs
git commit -m "feat(waved-ui): add async state forwarder bridging StateBus to UI command channel"
```

---

### Task 21: Wire `waved-ui` into Main Binary

**Files:**
- Modify: `waved/crates/waved/Cargo.toml`
- Modify: `waved/crates/waved/src/main.rs`

- [ ] **Step 1: Add waved-ui dependency to the main binary**

Add to `waved/crates/waved/Cargo.toml` under `[dependencies]`:
```toml
waved-ui = { path = "../waved-ui" }
```

- [ ] **Step 2: Initialize the UI service in main.rs**

Add to the async block in `main.rs`, after StateBus creation and config loading:
```rust
    // Start UI service (spawns the LVGL thread)
    let ui = waved_ui::UiService::new(bus.clone(), config.clone());
    let ui_tx = ui.cmd_sender();

    // Start state forwarder (bridges watch channels -> UiCommand)
    let _state_fwd = waved_ui::spawn_state_forwarder(bus.clone(), ui_tx.clone());

    // ... (existing module initialization) ...

    // On shutdown, tell UI to exit
    let _ = ui_tx.send(waved_ui::command::UiCommand::Shutdown).await;
    ui.join();
```

- [ ] **Step 3: Verify full workspace compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check
```

- [ ] **Step 4: Commit**

```bash
git add waved/crates/waved/Cargo.toml waved/crates/waved/src/main.rs
git commit -m "feat: wire waved-ui into main binary with state forwarder"
```

---

### Task 22: Deploy English Language File

**Files:**
- Modify: `waved/deploy/install.sh`

- [ ] **Step 1: Add language file deployment to install script**

Add to `waved/deploy/install.sh`:
```bash
# Deploy language files
echo "Deploying language files..."
adb shell mkdir -p /data/waved/lang
adb push ../crates/waved-ui/lang/en.json /data/waved/lang/en.json
echo "Language files deployed."
```

- [ ] **Step 2: Commit**

```bash
git add waved/deploy/install.sh
git commit -m "feat: deploy English language file to /data/waved/lang/ on install"
```

---

### Task 23: Integration Test — UI Command Round-Trip

**Files:**
- Create: `waved/crates/waved-ui/tests/command_roundtrip.rs`

- [ ] **Step 1: Write an integration test verifying command dispatch**

Create `waved/crates/waved-ui/tests/command_roundtrip.rs`:
```rust
//! Integration test: verify UiCommand/UiAction types serialize and the
//! action queue works correctly. This test does NOT initialize LVGL
//! (no framebuffer needed), only tests the Rust-side plumbing.

use waved_ui::command::*;
use waved_state::{AudioMode, MotorPosition};

#[test]
fn action_queue_enqueue_drain() {
    // Queue should start empty
    let actions = drain_actions();
    assert!(actions.is_empty());

    // Enqueue some actions
    enqueue_action(UiAction::ToggleMicMute);
    enqueue_action(UiAction::SetSpeakerVolume(150));
    enqueue_action(UiAction::SetAudioMode(AudioMode::Cardioid));

    // Drain should return all three
    let actions = drain_actions();
    assert_eq!(actions.len(), 3);

    // Queue should be empty after drain
    let actions = drain_actions();
    assert!(actions.is_empty());
}

#[test]
fn nav_queue_enqueue_drain() {
    use waved_ui::nav::Screen;

    let navs = drain_nav();
    assert!(navs.is_empty());

    enqueue_nav(Screen::Settings);
    enqueue_nav(Screen::Wifi);

    let navs = drain_nav();
    assert_eq!(navs.len(), 2);
    assert_eq!(navs[0], Screen::Settings);
    assert_eq!(navs[1], Screen::Wifi);
}

#[test]
fn toast_kind_variants_exist() {
    // Compile-time check that all variants exist
    let _s = ToastKind::Success;
    let _e = ToastKind::Error;
    let _w = ToastKind::Warning;
    let _i = ToastKind::Info;
    let _b = ToastKind::BtConnected;
    let _u = ToastKind::UsbConnected;
    let _bat = ToastKind::Battery;
}

#[test]
fn upload_status_eq() {
    assert_eq!(UploadStatus::Pending, UploadStatus::Pending);
    assert_ne!(UploadStatus::Pending, UploadStatus::Failed);
}
```

- [ ] **Step 2: Run tests**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo test -p waved-ui --test command_roundtrip
```
Expected: 4 tests pass (these tests don't need LVGL initialization).

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-ui/tests/
git commit -m "test(waved-ui): add command round-trip integration tests"
```

---

### Task 24: Cross-Compile Verification for aarch64

**Files:**
- No new files

- [ ] **Step 1: Verify cross-compilation targeting aarch64**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cross build --target aarch64-unknown-linux-gnu --release -p waved-ui
```

If `cross` is not installed:
```bash
cargo install cross
cross build --target aarch64-unknown-linux-gnu --release -p waved-ui
```

Expected: successful build producing `target/aarch64-unknown-linux-gnu/release/libwaved_ui.rlib`.

Note: The full `waved` binary (which links `waved-ui`) will also need `libInstaAudio.so` headers for the audio crate — that's outside this plan's scope.

- [ ] **Step 2: Check binary size**

```bash
ls -lh target/aarch64-unknown-linux-gnu/release/libwaved_ui.rlib
```

The LVGL library adds ~200-400KB. The total `waved` binary should stay under 5MB.

- [ ] **Step 3: Commit (if any cross-compile fixes were needed)**

```bash
git add -u
git commit -m "fix(waved-ui): cross-compilation fixes for aarch64"
```
