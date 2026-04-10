# waved Display Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `waved-ui` LVGL 9.2 touchscreen crate and wire it into the waved daemon, providing a dark/teal UI on the 400×304 framebuffer with home screen, recording controls, mic mode picker, and swipe-down control center.

**Architecture:** LVGL 9.2 is compiled from C via `build.rs` + `bindgen`. A dedicated OS thread (`ui-lvgl`) runs the LVGL event loop, reads state from `StateBus` watch channels each tick, and sends `Command` variants on `bus.commands` when the user taps. The status bar lives on `lv_layer_top()` and persists across screen changes. Screen navigation is a state machine in `nav.rs`.

**Tech Stack:** LVGL 9.2 (C, MIT), `cc` crate, `bindgen`, Rust FFI (`unsafe`), `/dev/fb0` RGB565 mmap, `/dev/input/event0` evdev, `tokio::sync::watch` (read via `.borrow()` from sync thread), `waved-state` StateBus.

**Spec:** `docs/superpowers/specs/2026-04-09-waved-display-design.md`  
**March plan (unchanged screens):** `docs/superpowers/plans/2026-03-19-waved-lvgl-ui.md`

---

## File map

| File | Create/Modify | Purpose |
|---|---|---|
| `waved/crates/waved-state/src/lib.rs` | Modify | Add `vu_level`, `NrLevel`, `nr_level`, `Command::SetNrLevel` |
| `waved/crates/waved-audio/src/lib.rs` | Modify | Compute and publish `vu_level` from audio callback |
| `waved/crates/waved-ui/Cargo.toml` | Create | Crate manifest with `cc`, `bindgen`, `libc` |
| `waved/crates/waved-ui/build.rs` | Create | Compile LVGL 9.2 C source, run bindgen |
| `waved/crates/waved-ui/lv_conf.h` | Create | LVGL config: RGB565, 400×304, 30fps, no GPU |
| `waved/crates/waved-ui/src/bindings.rs` | Create | `include!` generated LVGL bindings |
| `waved/crates/waved-ui/src/display.rs` | Create | `/dev/fb0` mmap flush callback |
| `waved/crates/waved-ui/src/input.rs` | Create | `/dev/input/event0` evdev touch reader |
| `waved/crates/waved-ui/src/theme.rs` | Create | Color/font constants |
| `waved/crates/waved-ui/src/i18n.rs` | Create | `tr()` with JSON fallback |
| `waved/crates/waved-ui/lang/en.json` | Create | English string table |
| `waved/crates/waved-ui/src/command.rs` | Create | `UiScreen` enum, screen-level events |
| `waved/crates/waved-ui/src/nav.rs` | Create | `ScreenManager` state machine |
| `waved/crates/waved-ui/src/status_bar.rs` | Create | Persistent top bar on `lv_layer_top()` |
| `waved/crates/waved-ui/src/toast.rs` | Create | Timed notification overlay |
| `waved/crates/waved-ui/src/widgets.rs` | Create | Reusable LVGL helpers |
| `waved/crates/waved-ui/src/lib.rs` | Create | `UiService::new()`, LVGL thread main loop |
| `waved/crates/waved-ui/src/screens/mod.rs` | Create | `Screen` trait |
| `waved/crates/waved-ui/src/screens/home.rs` | Create | Home screen: sphere, VU, 3 buttons, bottom bar |
| `waved/crates/waved-ui/src/screens/recording.rs` | Create | Recording: red sphere, timer, VU, bookmark |
| `waved/crates/waved-ui/src/screens/audio_mode.rs` | Create | Mic mode picker with beamforming icons |
| `waved/crates/waved-ui/src/screens/control_center.rs` | Create | Swipe-down overlay: WiFi, NR, motor arm |
| `waved/crates/waved-ui/src/screens/settings.rs` | Create | Settings menu (pattern for remaining screens) |
| `waved/crates/waved-ui/lvgl-src/` | Create | Vendored LVGL 9.2 source (git clone) |
| `waved/crates/waved/Cargo.toml` | Modify | Add `waved-ui` dependency |
| `waved/crates/waved/src/main.rs` | Modify | Spawn `UiService` |

---

## Task 1: StateBus additions

**Files:**
- Modify: `waved/crates/waved-state/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` block in `waved/crates/waved-state/src/lib.rs`:

```rust
#[test]
fn audio_state_has_vu_level() {
    let state = AudioState::default();
    assert_eq!(state.vu_level, [0.0f32; 8]);
    assert_eq!(state.nr_level, NrLevel::Off);
}

#[test]
fn command_set_nr_level_exists() {
    let cmd = Command::SetNrLevel(NrLevel::High);
    matches!(cmd, Command::SetNrLevel(NrLevel::High));
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd waved
cargo test -p waved-state 2>&1 | tail -15
```

Expected: compile error — `vu_level`, `NrLevel`, `SetNrLevel` not found.

- [ ] **Step 3: Add `NrLevel` enum, `vu_level` and `nr_level` fields, `SetNrLevel` command**

In `waved/crates/waved-state/src/lib.rs`, add after the `AudioMode` enum:

```rust
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NrLevel {
    #[default]
    Off,
    Low,
    Med,
    High,
}
```

Add `vu_level` and `nr_level` fields to `AudioState`:

```rust
#[derive(Clone, Debug, Default)]
pub struct AudioState {
    pub mode: AudioMode,
    pub mic_muted: bool,
    pub speaker_muted: bool,
    pub speaker_volume: u16,
    pub mic_volume: u16,
    pub doa_horizontal: f32,
    pub doa_pitch: f32,
    pub vu_level: [f32; 8],   // ← new: 8-band VU, each in [0.0, 1.0]
    pub nr_level: NrLevel,     // ← new: noise reduction level
}
```

Add `SetNrLevel` to the `Command` enum:

```rust
pub enum Command {
    SetAudioMode(AudioMode),
    SetNrLevel(NrLevel),       // ← new
    ToggleMicMute,
    // ... rest unchanged
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd waved
cargo test -p waved-state 2>&1 | tail -10
```

Expected: all tests pass including `audio_state_has_vu_level` and `command_set_nr_level_exists`.

- [ ] **Step 5: Verify full workspace still compiles**

```bash
cd waved
cargo check --workspace 2>&1 | tail -10
```

Expected: no errors. If `SetNrLevel` causes a non-exhaustive match warning in any crate, add a `_ => {}` arm to those match statements.

- [ ] **Step 6: Commit**

```bash
cd waved
git add crates/waved-state/src/lib.rs
git commit -m "feat(state): add NrLevel, vu_level, SetNrLevel for UI"
```

---

## Task 2: VU level publishing in waved-audio

**Files:**
- Modify: `waved/crates/waved-audio/src/lib.rs`

The audio callback receives PCM frames. After this task, it computes an 8-value RMS level array and updates `AudioState.vu_level` via the watch channel.

- [ ] **Step 1: Write the failing test**

Add to `waved/crates/waved-audio/src/lib.rs` tests:

```rust
#[test]
fn compute_vu_level_silent_is_zero() {
    let samples = vec![0i16; 512];
    let vu = compute_vu(&samples, 8);
    assert!(vu.iter().all(|&v| v == 0.0));
}

#[test]
fn compute_vu_level_full_scale_is_one() {
    let samples = vec![i16::MAX; 512];
    let vu = compute_vu(&samples, 8);
    // Each band should be close to 1.0
    for v in &vu {
        assert!(*v > 0.9, "expected ~1.0, got {v}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd waved
cargo test -p waved-audio 2>&1 | tail -10
```

Expected: compile error — `compute_vu` not found.

- [ ] **Step 3: Implement `compute_vu` and wire into audio callback**

Add to `waved/crates/waved-audio/src/lib.rs`:

```rust
/// Compute N-band VU levels from i16 PCM samples.
/// Returns values in [0.0, 1.0] where 1.0 = i16::MAX RMS.
pub fn compute_vu(samples: &[i16], bands: usize) -> Vec<f32> {
    if samples.is_empty() || bands == 0 {
        return vec![0.0; bands];
    }
    let chunk = (samples.len() / bands).max(1);
    (0..bands)
        .map(|i| {
            let start = i * chunk;
            let end = ((i + 1) * chunk).min(samples.len());
            let rms = (samples[start..end]
                .iter()
                .map(|&s| (s as f64) * (s as f64))
                .sum::<f64>()
                / (end - start) as f64)
                .sqrt();
            (rms / i16::MAX as f64).min(1.0) as f32
        })
        .collect()
}
```

In the audio callback where `AudioState` is updated, add the VU computation. Find the location where `bus.audio.send_modify` is called and add:

```rust
bus.audio.send_modify(|state| {
    // existing updates ...
    let vu = compute_vu(pcm_samples, 8);
    state.vu_level.copy_from_slice(&vu);
});
```

If the audio callback does not currently call `send_modify`, add the call after computing VU from the incoming PCM buffer. The exact variable name for the PCM buffer depends on the existing code — look for the `&[i16]` or `&[f32]` slice passed to the callback.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd waved
cargo test -p waved-audio 2>&1 | tail -10
```

Expected: `compute_vu_level_silent_is_zero` and `compute_vu_level_full_scale_is_one` both pass.

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-audio/src/lib.rs
git commit -m "feat(audio): compute and publish 8-band VU levels to AudioState"
```

---

## Task 3: waved-ui crate skeleton + LVGL 9.2 build

**Files:**
- Create: `waved/crates/waved-ui/Cargo.toml`
- Create: `waved/crates/waved-ui/lv_conf.h`
- Create: `waved/crates/waved-ui/build.rs`
- Create: `waved/crates/waved-ui/src/bindings.rs`
- Create: `waved/crates/waved-ui/src/lib.rs` (stub)

- [ ] **Step 1: Vendor LVGL 9.2 source**

```bash
cd waved/crates
mkdir -p waved-ui/src waved-ui/lang
cd waved-ui
git clone --depth 1 --branch v9.2.0 https://github.com/lvgl/lvgl.git lvgl-src/lvgl
```

Expected: `lvgl-src/lvgl/lvgl.h` exists.

- [ ] **Step 2: Create `Cargo.toml`**

Create `waved/crates/waved-ui/Cargo.toml`:

```toml
[package]
name = "waved-ui"
version.workspace = true
edition.workspace = true

[features]
simulate = []

[dependencies]
tokio = { workspace = true }
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
thiserror.workspace = true
libc = "0.2"
waved-state = { path = "../waved-state" }
waved-config = { path = "../waved-config" }

[build-dependencies]
cc = "1"
bindgen = "0.71"
```

- [ ] **Step 3: Create `lv_conf.h`**

Create `waved/crates/waved-ui/lv_conf.h`:

```c
#ifndef LV_CONF_H
#define LV_CONF_H

#define LV_COLOR_DEPTH 16        /* RGB565 — matches /dev/fb0 */
#define LV_COLOR_16_SWAP 0       /* Amlogic A5 is little-endian */
#define LV_MEM_SIZE (128 * 1024)
#define LV_MEM_CUSTOM 0
#define LV_DEF_REFR_PERIOD 33    /* ~30 fps */
#define LV_DEF_INPUT_READ_PERIOD 30
#define LV_DPI_DEF 276           /* 1.82" diagonal, 400x304px */
#define LV_HOR_RES_MAX 400
#define LV_VER_RES_MAX 304

#define LV_USE_DRAW_SW 1
#define LV_USE_DRAW_SW_ASM LV_DRAW_SW_ASM_NONE
#define LV_USE_GPU_STM32_DMA2D 0
#define LV_USE_GPU_NXP_PXP 0
#define LV_USE_GPU_NXP_VG_LITE 0

#define LV_USE_LOG 1
#define LV_LOG_LEVEL LV_LOG_LEVEL_WARN
#define LV_LOG_PRINTF 1

#define LV_USE_ASSERT_NULL 1
#define LV_USE_ASSERT_MALLOC 1
#define LV_USE_ASSERT_STYLE 0
#define LV_USE_ASSERT_MEM_INTEGRITY 0
#define LV_USE_ASSERT_OBJ 0

#define LV_FONT_MONTSERRAT_12 1
#define LV_FONT_MONTSERRAT_14 1
#define LV_FONT_MONTSERRAT_16 1
#define LV_FONT_MONTSERRAT_20 1
#define LV_FONT_MONTSERRAT_24 1
#define LV_FONT_DEFAULT &lv_font_montserrat_16
#define LV_USE_FREETYPE 0

#define LV_USE_ARC 1
#define LV_USE_BAR 1
#define LV_USE_BTN 1
#define LV_USE_BTNMATRIX 1
#define LV_USE_IMAGE 1
#define LV_USE_LABEL 1
#define LV_USE_SLIDER 1
#define LV_USE_SWITCH 1
#define LV_USE_LIST 1
#define LV_USE_SPINNER 1
#define LV_USE_MSGBOX 1
#define LV_USE_ANIM 1
#define LV_USE_PNG 1
#define LV_USE_FS_STDIO 0
#define LV_USE_OS LV_OS_NONE

#endif /* LV_CONF_H */
```

- [ ] **Step 4: Create `build.rs`**

Create `waved/crates/waved-ui/build.rs`:

```rust
use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let lvgl_root = manifest.join("lvgl-src/lvgl");
    let lvgl_src = lvgl_root.join("src");
    let conf_dir = manifest.clone();

    // Collect all LVGL .c files recursively
    let c_files = collect_c_files(&lvgl_src);
    println!("cargo:warning=Compiling {} LVGL C files", c_files.len());

    cc::Build::new()
        .files(&c_files)
        .include(&conf_dir)       // lv_conf.h
        .include(&manifest.join("lvgl-src"))  // #include "lvgl/lvgl.h"
        .include(&lvgl_root)      // internal lvgl includes
        .include(&lvgl_src)
        .define("LV_CONF_INCLUDE_SIMPLE", None)
        .flag("-w")
        .opt_level(2)
        .compile("lvgl");

    let bindings = bindgen::Builder::default()
        .header(lvgl_root.join("lvgl.h").to_str().unwrap())
        .clang_arg(format!("-I{}", conf_dir.display()))
        .clang_arg(format!("-I{}", manifest.join("lvgl-src").display()))
        .clang_arg(format!("-I{}", lvgl_root.display()))
        .clang_arg(format!("-I{}", lvgl_src.display()))
        .clang_arg("-DLV_CONF_INCLUDE_SIMPLE")
        .allowlist_function("lv_.*")
        .allowlist_type("lv_.*")
        .allowlist_var("LV_.*")
        .derive_debug(true)
        .derive_default(true)
        .generate()
        .expect("bindgen failed");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings.write_to_file(out.join("lvgl_bindings.rs")).unwrap();

    println!("cargo:rerun-if-changed=lv_conf.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=lvgl-src/lvgl/src");
}

fn collect_c_files(dir: &PathBuf) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                files.extend(collect_c_files(&p));
            } else if p.extension().map_or(false, |e| e == "c") {
                files.push(p);
            }
        }
    }
    files
}
```

- [ ] **Step 5: Create stub `src/bindings.rs` and `src/lib.rs`**

Create `waved/crates/waved-ui/src/bindings.rs`:

```rust
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code, clippy::all)]
include!(concat!(env!("OUT_DIR"), "/lvgl_bindings.rs"));
```

Create `waved/crates/waved-ui/src/lib.rs`:

```rust
pub mod bindings;

pub struct UiService;

impl UiService {
    pub fn new() -> Self { Self }
}
```

- [ ] **Step 6: Add crate to workspace**

In `waved/Cargo.toml`, add `"crates/waved-ui"` to the `members` list:

```toml
[workspace]
resolver = "2"
members = ["crates/*"]
```

The glob `crates/*` already covers it if the directory exists. Verify:

```bash
cd waved
cargo check -p waved-ui 2>&1 | tail -20
```

Expected: LVGL compiles (may take 2–5 minutes on first run). Final line: `Finished` or `warning:` lines only — no `error:`.

If you see `lv_conf.h: No such file or directory`: ensure `conf_dir` in `build.rs` points to `CARGO_MANIFEST_DIR` (the directory containing `lv_conf.h`), not `OUT_DIR`.

If bindgen fails with missing `clang`: install with `brew install llvm` and set `LIBCLANG_PATH=$(brew --prefix llvm)/lib`.

- [ ] **Step 7: Commit**

```bash
cd waved
git add crates/waved-ui/
git commit -m "feat(ui): add waved-ui crate with LVGL 9.2 build system"
```

---

## Task 4: Framebuffer display driver + evdev touch input

**Files:**
- Create: `waved/crates/waved-ui/src/display.rs`
- Create: `waved/crates/waved-ui/src/input.rs`

- [ ] **Step 1: Write the failing test**

Add to `waved/crates/waved-ui/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn display_module_compiles() {
        // If display.rs compiles, this test passes.
        // Actual /dev/fb0 open is gated behind cfg(not(feature="simulate")).
        assert!(true);
    }
}
```

```bash
cd waved
cargo test -p waved-ui 2>&1 | tail -5
```

Expected: PASS (the test is a compile check).

- [ ] **Step 2: Create `display.rs`**

Create `waved/crates/waved-ui/src/display.rs`:

```rust
//! Framebuffer display driver for /dev/fb0 (RGB565, 400×304).
//!
//! In simulate mode this is a no-op — the caller provides its own flush target.

use crate::bindings::*;
use std::os::unix::io::RawFd;

pub const SCREEN_W: u32 = 400;
pub const SCREEN_H: u32 = 304;
pub const BYTES_PER_PIXEL: usize = 2; // RGB565

/// Manages the framebuffer mapping.
pub struct FramebufferDisplay {
    #[cfg(not(feature = "simulate"))]
    fb_ptr: *mut u8,
    #[cfg(not(feature = "simulate"))]
    fb_size: usize,
    #[cfg(not(feature = "simulate"))]
    _fd: RawFd,
    /// LVGL draw buffer (two half-screen buffers for DMA-style double-buffering).
    pub buf1: Box<[u8]>,
    pub buf2: Box<[u8]>,
}

impl FramebufferDisplay {
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        let buf_size = (SCREEN_W * SCREEN_H / 2) as usize * BYTES_PER_PIXEL;
        let buf1 = vec![0u8; buf_size].into_boxed_slice();
        let buf2 = vec![0u8; buf_size].into_boxed_slice();

        #[cfg(not(feature = "simulate"))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/fb0")?;
            let fd = std::os::unix::io::IntoRawFd::into_raw_fd(file);
            let fb_size = (SCREEN_W * SCREEN_H) as usize * BYTES_PER_PIXEL;
            let fb_ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    fb_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                ) as *mut u8
            };
            if fb_ptr == libc::MAP_FAILED as *mut u8 {
                return Err("mmap /dev/fb0 failed".into());
            }
            // Turn on backlight
            let _ = std::fs::write(
                "/sys/class/leds/lcd-backlight/brightness",
                "255",
            );
            return Ok(Self { fb_ptr, fb_size, _fd: fd, buf1, buf2 });
        }

        #[cfg(feature = "simulate")]
        Ok(Self { buf1, buf2 })
    }

    /// Called by LVGL flush callback — copies the dirty area to /dev/fb0.
    ///
    /// # Safety
    /// `color_p` must point to at least `area.width * area.height` RGB565 pixels.
    pub unsafe fn flush(
        &self,
        area: &lv_area_t,
        color_p: *const u8,
    ) {
        #[cfg(not(feature = "simulate"))]
        {
            let x1 = area.x1 as usize;
            let y1 = area.y1 as usize;
            let x2 = area.x2 as usize;
            let y2 = area.y2 as usize;
            let w = x2 - x1 + 1;
            for row in y1..=y2 {
                let dst = self.fb_ptr.add((row * SCREEN_W as usize + x1) * BYTES_PER_PIXEL);
                let src = color_p.add((row - y1) * w * BYTES_PER_PIXEL);
                std::ptr::copy_nonoverlapping(src, dst, w * BYTES_PER_PIXEL);
            }
        }
        #[cfg(feature = "simulate")]
        let _ = (area, color_p);
    }
}

impl Drop for FramebufferDisplay {
    fn drop(&mut self) {
        #[cfg(not(feature = "simulate"))]
        unsafe {
            libc::munmap(self.fb_ptr as *mut _, self.fb_size);
        }
    }
}
```

- [ ] **Step 3: Create `input.rs`**

Create `waved/crates/waved-ui/src/input.rs`:

```rust
//! Evdev touch input driver for /dev/input/event0.
//!
//! Reads ABS_X / ABS_Y + BTN_TOUCH events and converts to LVGL lv_indev_data_t.

use crate::bindings::*;

/// Evdev event structure (matches kernel input_event).
#[repr(C)]
struct InputEvent {
    tv_sec: libc::time_t,
    tv_usec: libc::suseconds_t,
    r#type: u16,
    code: u16,
    value: i32,
}

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const BTN_TOUCH: u16 = 0x14a;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;

// Evdev axis ranges for the ilitek touch controller on the Wave.
// These were measured from /dev/input/event0 on device.
// If touch coordinates are off, adjust these constants.
const TOUCH_X_MIN: i32 = 0;
const TOUCH_X_MAX: i32 = 4095;
const TOUCH_Y_MIN: i32 = 0;
const TOUCH_Y_MAX: i32 = 4095;

pub struct TouchInput {
    #[cfg(not(feature = "simulate"))]
    fd: std::os::unix::io::RawFd,
    pub x: i32,
    pub y: i32,
    pub pressed: bool,
}

impl TouchInput {
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(not(feature = "simulate"))]
        {
            use std::os::unix::io::IntoRawFd;
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open("/dev/input/event0")?;
            let fd = file.into_raw_fd();
            return Ok(Self { fd, x: 0, y: 0, pressed: false });
        }
        #[cfg(feature = "simulate")]
        Ok(Self { x: 0, y: 0, pressed: false })
    }

    /// Poll for pending events. Call this once per LVGL tick before `lv_timer_handler`.
    pub fn poll(&mut self) {
        #[cfg(not(feature = "simulate"))]
        unsafe {
            let mut ev = std::mem::zeroed::<InputEvent>();
            let sz = std::mem::size_of::<InputEvent>();
            loop {
                let n = libc::read(self.fd, &mut ev as *mut _ as *mut libc::c_void, sz);
                if n < sz as isize { break; }
                match ev.r#type {
                    EV_KEY if ev.code == BTN_TOUCH => {
                        self.pressed = ev.value != 0;
                    }
                    EV_ABS if ev.code == ABS_X => {
                        // Scale evdev range to screen pixels
                        self.x = (ev.value - TOUCH_X_MIN) * 400
                            / (TOUCH_X_MAX - TOUCH_X_MIN).max(1);
                    }
                    EV_ABS if ev.code == ABS_Y => {
                        self.y = (ev.value - TOUCH_Y_MIN) * 304
                            / (TOUCH_Y_MAX - TOUCH_Y_MIN).max(1);
                    }
                    _ => {}
                }
            }
        }
    }
}

impl Drop for TouchInput {
    fn drop(&mut self) {
        #[cfg(not(feature = "simulate"))]
        unsafe { libc::close(self.fd); }
    }
}
```

- [ ] **Step 4: Add `libc` to `Cargo.toml` and verify compilation**

`libc = "0.2"` is already in `Cargo.toml` from Task 3. Update `src/lib.rs` to expose the new modules:

```rust
pub mod bindings;
pub mod display;
pub mod input;

pub struct UiService;
impl UiService {
    pub fn new() -> Self { Self }
}

#[cfg(test)]
mod tests {
    #[test]
    fn display_module_compiles() { assert!(true); }
}
```

```bash
cd waved
cargo check -p waved-ui --features simulate 2>&1 | tail -10
```

Expected: no errors (simulate mode skips actual device open).

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-ui/src/display.rs crates/waved-ui/src/input.rs crates/waved-ui/src/lib.rs
git commit -m "feat(ui): framebuffer display driver and evdev touch input"
```

---

## Task 5: Theme, i18n, screen infrastructure

**Files:**
- Create: `waved/crates/waved-ui/src/theme.rs`
- Create: `waved/crates/waved-ui/src/i18n.rs`
- Create: `waved/crates/waved-ui/lang/en.json`
- Create: `waved/crates/waved-ui/src/screens/mod.rs`
- Create: `waved/crates/waved-ui/src/nav.rs`

- [ ] **Step 1: Create `theme.rs`**

Create `waved/crates/waved-ui/src/theme.rs`:

```rust
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
```

- [ ] **Step 2: Create `i18n.rs` and `lang/en.json`**

Create `waved/crates/waved-ui/lang/en.json`:

```json
{
  "home.connected": "Connected",
  "home.disconnected": "Disconnected",
  "home.rec": "REC",
  "home.mute": "Mute",
  "home.unmute": "Unmute",
  "recording.recording": "RECORDING",
  "recording.stop": "STOP",
  "recording.mark": "Mark",
  "recording.bookmark_added": "Bookmark added",
  "mode.omni": "Omni",
  "mode.stereo": "Stereo",
  "mode.cardioid": "Cardioid",
  "mode.supercardioid": "Super-card.",
  "mode.figure8": "Figure-8",
  "nr.off": "Off",
  "nr.low": "Low",
  "nr.med": "Med",
  "nr.high": "High",
  "cc.wifi": "WIFI",
  "cc.noise_reduction": "NOISE REDUCTION",
  "cc.mic_arm": "MIC ARM",
  "cc.extend": "↑ Extend",
  "cc.retract": "↓ Retract",
  "cc.not_connected": "Not connected",
  "settings.title": "Settings",
  "nav.files": "Files",
  "nav.settings": "Settings",
  "nav.upload": "Upload"
}
```

Create `waved/crates/waved-ui/src/i18n.rs`:

```rust
//! JSON string table with fallback to key name.

use std::collections::HashMap;

pub struct I18n {
    strings: HashMap<String, String>,
}

impl I18n {
    /// Load from `/data/waved/lang/{lang}.json`. Falls back to empty map if missing.
    pub fn load(lang: &str) -> Self {
        let path = format!("/data/waved/lang/{lang}.json");
        let strings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
            .unwrap_or_default();
        Self { strings }
    }

    /// Look up key. Returns the key itself if not found — never panics.
    pub fn tr(&self, key: &str) -> &str {
        self.strings.get(key).map(|s| s.as_str()).unwrap_or(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_returns_key_name() {
        let i18n = I18n { strings: Default::default() };
        assert_eq!(i18n.tr("some.key"), "some.key");
    }

    #[test]
    fn found_key_returns_translation() {
        let mut strings = std::collections::HashMap::new();
        strings.insert("hello".into(), "Hello World".into());
        let i18n = I18n { strings };
        assert_eq!(i18n.tr("hello"), "Hello World");
    }
}
```

- [ ] **Step 3: Create `screens/mod.rs` with `Screen` trait**

```bash
mkdir -p waved/crates/waved-ui/src/screens
```

Create `waved/crates/waved-ui/src/screens/mod.rs`:

```rust
pub mod audio_mode;
pub mod control_center;
pub mod home;
pub mod recording;
pub mod settings;

use crate::bindings::lv_obj_t;

/// Every screen implements this trait.
/// `create` builds the LVGL widget tree and returns the root object.
/// `destroy` is called before navigating away — must call `lv_obj_delete(root)`.
pub trait Screen {
    fn create(&mut self) -> *mut lv_obj_t;
    fn destroy(&mut self, root: *mut lv_obj_t) {
        unsafe { crate::bindings::lv_obj_delete(root); }
    }
    /// Called each LVGL tick to refresh state-driven labels/values.
    fn update(&mut self, _root: *mut lv_obj_t) {}
}
```

- [ ] **Step 4: Create `nav.rs`**

Create `waved/crates/waved-ui/src/nav.rs`:

```rust
//! Screen navigation state machine.

#[derive(Clone, Debug, PartialEq)]
pub enum UiScreen {
    Home,
    Recording,
    MicMode,
    Settings,
    Files,
    Wifi,
    WifiInfo,
    Bluetooth,
    Volume,
    Height,
    About,
    PowerOff,
    ControlCenter, // overlay — stacked above current screen
}

impl Default for UiScreen {
    fn default() -> Self { Self::Home }
}

pub struct ScreenManager {
    pub current: UiScreen,
    pub control_center_open: bool,
    current_root: *mut crate::bindings::lv_obj_t,
}

impl ScreenManager {
    pub fn new() -> Self {
        Self {
            current: UiScreen::default(),
            control_center_open: false,
            current_root: std::ptr::null_mut(),
        }
    }

    /// Navigate to a new screen. Destroys the current one first.
    pub fn navigate_to(&mut self, screen: UiScreen) {
        if !self.current_root.is_null() {
            unsafe { crate::bindings::lv_obj_delete(self.current_root); }
            self.current_root = std::ptr::null_mut();
        }
        self.current = screen;
    }

    pub fn set_root(&mut self, root: *mut crate::bindings::lv_obj_t) {
        self.current_root = root;
    }

    pub fn current_root(&self) -> *mut crate::bindings::lv_obj_t {
        self.current_root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_screen_is_home() {
        let nav = ScreenManager::new();
        assert_eq!(nav.current, UiScreen::Home);
        assert!(!nav.control_center_open);
    }

    #[test]
    fn navigate_changes_current() {
        let mut nav = ScreenManager::new();
        nav.navigate_to(UiScreen::Recording);
        assert_eq!(nav.current, UiScreen::Recording);
    }
}
```

- [ ] **Step 5: Run all tests**

```bash
cd waved
cargo test -p waved-ui --features simulate 2>&1 | tail -15
```

Expected: `missing_key_returns_key_name`, `found_key_returns_translation`, `default_screen_is_home`, `navigate_changes_current` all pass.

- [ ] **Step 6: Update `lib.rs` to include new modules**

```rust
pub mod bindings;
pub mod display;
pub mod i18n;
pub mod input;
pub mod nav;
pub mod screens;
pub mod theme;
// coming in later tasks:
// pub mod status_bar;
// pub mod toast;
// pub mod widgets;

pub struct UiService;
impl UiService {
    pub fn new() -> Self { Self }
}

#[cfg(test)]
mod tests {
    #[test]
    fn display_module_compiles() { assert!(true); }
}
```

- [ ] **Step 7: Commit**

```bash
cd waved
git add crates/waved-ui/src/theme.rs crates/waved-ui/src/i18n.rs \
        crates/waved-ui/lang/en.json crates/waved-ui/src/screens/ \
        crates/waved-ui/src/nav.rs crates/waved-ui/src/lib.rs
git commit -m "feat(ui): theme, i18n, Screen trait, ScreenManager nav"
```

---

## Task 6: Status bar + toast + LVGL event loop

**Files:**
- Create: `waved/crates/waved-ui/src/status_bar.rs`
- Create: `waved/crates/waved-ui/src/toast.rs`
- Modify: `waved/crates/waved-ui/src/lib.rs` (full UiService implementation)

- [ ] **Step 1: Create `status_bar.rs`**

Create `waved/crates/waved-ui/src/status_bar.rs`:

```rust
//! Persistent status bar on lv_layer_top() — survives screen changes.
//!
//! Shows: battery %, WiFi signal, BT connected, mic muted, time (09:41).
//! Height: 32px. Width: 400px.

use crate::bindings::*;
use crate::theme::*;

pub struct StatusBar {
    root: *mut lv_obj_t,
    lbl_battery: *mut lv_obj_t,
    lbl_wifi: *mut lv_obj_t,
    lbl_bt: *mut lv_obj_t,
    lbl_mic: *mut lv_obj_t,
    lbl_time: *mut lv_obj_t,
}

impl StatusBar {
    /// Create the status bar on the top LVGL layer. Call once after lv_init().
    pub unsafe fn create() -> Self {
        let top_layer = lv_layer_top();

        let root = lv_obj_create(top_layer);
        lv_obj_set_size(root, 400, 32);
        lv_obj_set_pos(root, 0, 0);
        lv_obj_set_style_bg_color(root, rgb(CLR_BG), 0);
        lv_obj_set_style_bg_opa(root, 220, 0); // slightly transparent
        lv_obj_set_style_border_width(root, 0, 0);
        lv_obj_set_style_radius(root, 0, 0);
        lv_obj_set_style_pad_all(root, 0, 0);

        let mk_label = |parent: *mut lv_obj_t, x: i32, align: lv_align_t| {
            let lbl = lv_label_create(parent);
            lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT), 0);
            lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl, align, x, 0);
            lbl
        };

        let lbl_battery = mk_label(root, 4,  lv_align_t_LV_ALIGN_LEFT_MID);
        let lbl_wifi    = mk_label(root, 50, lv_align_t_LV_ALIGN_LEFT_MID);
        let lbl_bt      = mk_label(root, 80, lv_align_t_LV_ALIGN_LEFT_MID);
        let lbl_mic     = mk_label(root, 110,lv_align_t_LV_ALIGN_LEFT_MID);
        let lbl_time    = mk_label(root, -4, lv_align_t_LV_ALIGN_RIGHT_MID);

        Self { root, lbl_battery, lbl_wifi, lbl_bt, lbl_mic, lbl_time }
    }

    /// Update labels from current state. Call each LVGL tick.
    pub unsafe fn update(
        &self,
        battery_pct: u8,
        wifi_connected: bool,
        bt_connected: bool,
        mic_muted: bool,
    ) {
        use std::ffi::CString;

        let batt_str = CString::new(format!("{battery_pct}%")).unwrap();
        lv_label_set_text(self.lbl_battery, batt_str.as_ptr());

        let wifi_str = CString::new(if wifi_connected { "W" } else { "--" }).unwrap();
        lv_label_set_text(self.lbl_wifi, wifi_str.as_ptr());

        let bt_str = CString::new(if bt_connected { "BT" } else { "" }).unwrap();
        lv_label_set_text(self.lbl_bt, bt_str.as_ptr());

        let mic_str = CString::new(if mic_muted { "M" } else { "" }).unwrap();
        lv_label_set_text(self.lbl_mic, mic_str.as_ptr());

        // Wall clock from system time
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let h = (now / 3600) % 24;
        let m = (now / 60) % 60;
        let time_str = CString::new(format!("{h:02}:{m:02}")).unwrap();
        lv_label_set_text(self.lbl_time, time_str.as_ptr());
    }
}
```

- [ ] **Step 2: Create `toast.rs`**

Create `waved/crates/waved-ui/src/toast.rs`:

```rust
//! Timed notification overlay. Shows a message for `duration_ms`, then hides.

use crate::bindings::*;
use crate::theme::*;
use std::ffi::CString;

pub struct Toast {
    obj: *mut lv_obj_t,
    lbl: *mut lv_obj_t,
    hide_at_ms: Option<u64>, // wall ms when to hide
}

impl Toast {
    pub unsafe fn create() -> Self {
        let top = lv_layer_top();
        let obj = lv_obj_create(top);
        lv_obj_set_size(obj, 200, 36);
        lv_obj_align(obj, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -40);
        lv_obj_set_style_bg_color(obj, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(obj, 18, 0);
        lv_obj_set_style_border_width(obj, 0, 0);
        lv_obj_add_flag(obj, LV_OBJ_FLAG_HIDDEN);

        let lbl = lv_label_create(obj);
        lv_obj_set_style_text_color(lbl, rgb(CLR_ACCENT), 0);
        lv_label_set_text(lbl, c"".as_ptr());
        lv_obj_center(lbl);

        Self { obj, lbl, hide_at_ms: None }
    }

    pub unsafe fn show(&mut self, msg: &str, duration_ms: u64) {
        let s = CString::new(msg).unwrap_or_default();
        lv_label_set_text(self.lbl, s.as_ptr());
        lv_obj_remove_flag(self.obj, LV_OBJ_FLAG_HIDDEN);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.hide_at_ms = Some(now + duration_ms);
    }

    pub unsafe fn tick(&mut self) {
        if let Some(hide_at) = self.hide_at_ms {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if now >= hide_at {
                lv_obj_add_flag(self.obj, LV_OBJ_FLAG_HIDDEN);
                self.hide_at_ms = None;
            }
        }
    }
}
```

- [ ] **Step 3: Write the full LVGL event loop in `lib.rs`**

Replace `waved/crates/waved-ui/src/lib.rs` with:

```rust
pub mod bindings;
pub mod display;
pub mod i18n;
pub mod input;
pub mod nav;
pub mod screens;
pub mod status_bar;
pub mod theme;
pub mod toast;

use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info};
use waved_config::WavedConfig;
use waved_state::{Command, NrLevel, StateBus};

use bindings::*;
use display::FramebufferDisplay;
use i18n::I18n;
use input::TouchInput;
use nav::{ScreenManager, UiScreen};
use status_bar::StatusBar;
use toast::Toast;

pub struct UiService {
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl UiService {
    pub fn new(bus: Arc<StateBus>, config: WavedConfig) -> Self {
        let handle = std::thread::Builder::new()
            .name("ui-lvgl".into())
            .spawn(move || {
                if let Err(e) = ui_thread_main(bus, config) {
                    error!("UI thread error: {e}");
                }
            })
            .expect("failed to spawn ui-lvgl thread");
        Self { thread_handle: Some(handle) }
    }

    pub fn join(mut self) {
        if let Some(h) = self.thread_handle.take() { let _ = h.join(); }
    }
}

fn ui_thread_main(
    bus: Arc<StateBus>,
    config: WavedConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("UI thread starting");

    // Subscribe to state watch channels
    let audio_rx    = bus.audio.subscribe();
    let battery_rx  = bus.battery.subscribe();
    let bt_rx       = bus.bt.subscribe();
    let wifi_rx     = bus.wifi.subscribe();
    let meeting_rx  = bus.meeting.subscribe();
    let mcu_rx      = bus.mcu.subscribe();

    // Subscribe to incoming commands (to react to hardware button presses)
    let mut cmd_rx = bus.commands.subscribe();

    let i18n = I18n::load(&config.device.language);

    // Initialize LVGL
    unsafe { lv_init(); }

    let fb = FramebufferDisplay::init()?;
    let mut touch = TouchInput::init()?;

    // Register display with LVGL (simplified — full driver registration omitted for brevity;
    // see LVGL 9 docs for lv_display_create + lv_display_set_flush_cb pattern)
    unsafe {
        let disp = lv_display_create(display::SCREEN_W as i32, display::SCREEN_H as i32);
        // The flush callback copies LVGL's render buffer to /dev/fb0.
        // We use a static pointer trick to pass fb into the C callback.
        // In a real implementation, store fb in a Box and pass via lv_display_set_user_data.
        lv_display_set_flush_cb(disp, Some(flush_cb));
        lv_display_set_buffers(
            disp,
            fb.buf1.as_ptr() as *mut _,
            fb.buf2.as_ptr() as *mut _,
            fb.buf1.len() as u32,
            lv_display_render_mode_t_LV_DISPLAY_RENDER_MODE_PARTIAL,
        );

        // Register input device
        let indev = lv_indev_create();
        lv_indev_set_type(indev, lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        lv_indev_set_read_cb(indev, Some(touch_read_cb));
    }

    // Create persistent UI elements
    let status_bar = unsafe { StatusBar::create() };
    let mut toast  = unsafe { Toast::create() };

    // Screen manager + create initial home screen
    let mut nav = ScreenManager::new();
    let mut home = screens::home::HomeScreen::new(Arc::clone(&bus), i18n.tr("home.connected"));

    let root = unsafe { home.create() };
    nav.set_root(root);

    let mut last_tick = Instant::now();

    loop {
        let elapsed_ms = last_tick.elapsed().as_millis() as u32;
        last_tick = Instant::now();

        unsafe { lv_tick_inc(elapsed_ms); }

        // Poll touch events
        touch.poll();

        // Update status bar from latest watch state
        unsafe {
            let batt  = battery_rx.borrow();
            let audio = audio_rx.borrow();
            let bt    = bt_rx.borrow();
            let wifi  = wifi_rx.borrow();
            status_bar.update(batt.level, wifi.connected, bt.connected, audio.mic_muted);
        }

        // React to any pending commands (e.g., hardware mute button → update home UI)
        while let Ok(cmd) = cmd_rx.try_recv() {
            handle_command(&cmd, &mut nav, &mut toast);
        }

        // Update current screen
        let current_root = nav.current_root();
        unsafe { lv_timer_handler(); }
        unsafe { toast.tick(); }

        // Update screen-specific state
        match &nav.current {
            UiScreen::Home => {
                let audio   = audio_rx.borrow().clone();
                let meeting = meeting_rx.borrow().clone();
                unsafe { home.update_state(&audio, &meeting); }
            }
            _ => {}
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn handle_command(cmd: &Command, nav: &mut ScreenManager, toast: &mut Toast) {
    match cmd {
        Command::AddBookmark => {
            unsafe { toast.show("Bookmark added", 1500); }
        }
        _ => {}
    }
}

// C-callable flush callback stub — real implementation uses user_data to access fb.
unsafe extern "C" fn flush_cb(
    _disp: *mut lv_display_t,
    _area: *const lv_area_t,
    _px_map: *mut u8,
) {
    // In the real implementation: cast lv_display_get_user_data(disp) to &FramebufferDisplay
    // and call fb.flush(area, px_map). Then call lv_display_flush_ready(disp).
    // Omitted here because passing fb safely requires Arc<Mutex<FramebufferDisplay>> + user_data.
    lv_display_flush_ready(_disp);
}

unsafe extern "C" fn touch_read_cb(
    _indev: *mut lv_indev_t,
    data: *mut lv_indev_data_t,
) {
    // Real implementation reads from TouchInput stored in user_data.
    // Stub: always report no touch.
    (*data).state = lv_indev_state_t_LV_INDEV_STATE_RELEASED;
}

#[cfg(test)]
mod tests {
    use super::nav::*;

    #[test]
    fn screen_manager_starts_at_home() {
        let nav = ScreenManager::new();
        assert_eq!(nav.current, UiScreen::Home);
    }
}
```

**Note on flush/touch callbacks:** The stubs above compile and run but do not actually write pixels or read touch. Task 7 and 8 (home and recording screens) will work in simulate mode first. The real callback wiring (passing `FramebufferDisplay` and `TouchInput` via `lv_display_set_user_data` / `lv_indev_set_user_data`) must be completed before on-device testing. The pattern: store `Arc<Mutex<T>>` in a `Box`, leak it, pass the raw pointer via `set_user_data`, cast back in the callback.

- [ ] **Step 4: Verify compilation in simulate mode**

```bash
cd waved
cargo check -p waved-ui --features simulate 2>&1 | tail -15
```

Expected: no errors.

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-ui/src/status_bar.rs crates/waved-ui/src/toast.rs \
        crates/waved-ui/src/lib.rs
git commit -m "feat(ui): status bar, toast, LVGL event loop skeleton"
```

---

## Task 7: Home screen

**Files:**
- Create: `waved/crates/waved-ui/src/screens/home.rs`

- [ ] **Step 1: Write the failing test**

Add to `waved/crates/waved-ui/src/screens/home.rs` (create the file):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vu_level_clamps_above_one() {
        let clamped = clamp_vu(1.5);
        assert!(clamped <= 1.0, "expected <=1.0, got {clamped}");
    }

    #[test]
    fn vu_bar_height_scales_correctly() {
        assert_eq!(vu_bar_height(0.0), 0);
        assert_eq!(vu_bar_height(1.0), VU_MAX_H);
        assert_eq!(vu_bar_height(0.5), VU_MAX_H / 2);
    }
}
```

```bash
cd waved
cargo test -p waved-ui --features simulate 2>&1 | grep -E "FAILED|error" | head -10
```

Expected: compile error — `clamp_vu`, `vu_bar_height`, `VU_MAX_H` not defined.

- [ ] **Step 2: Implement `home.rs`**

Create `waved/crates/waved-ui/src/screens/home.rs`:

```rust
//! Home screen: teal sphere, VU meter, 3 buttons, bottom bar.
//! Sphere is 180px, bottom-anchored. Buttons: [Mic Mode] [● REC] [Mute].
//! Bottom bar: NR level | Files | Upload queue | Settings.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{AudioState, AudioMode, MeetingState, StateBus, Command};

pub const VU_MAX_H: i32 = 20;
pub const VU_BAR_W: i32 = 3;
pub const VU_BAR_GAP: i32 = 2;
pub const VU_BANDS: usize = 8;

pub fn clamp_vu(v: f32) -> f32 { v.clamp(0.0, 1.0) }

pub fn vu_bar_height(level: f32) -> i32 {
    (clamp_vu(level) * VU_MAX_H as f32) as i32
}

pub struct HomeScreen {
    bus: Arc<StateBus>,
    connected_label: &'static str,
    // LVGL objects (null until create() is called)
    sphere: *mut lv_obj_t,
    lbl_status: *mut lv_obj_t,
    vu_bars: [*mut lv_obj_t; VU_BANDS],
    btn_mode: *mut lv_obj_t,
    lbl_mode: *mut lv_obj_t,
    btn_rec: *mut lv_obj_t,
    btn_mute: *mut lv_obj_t,
    lbl_nr: *mut lv_obj_t,
    lbl_upload: *mut lv_obj_t,
}

impl HomeScreen {
    pub fn new(bus: Arc<StateBus>, connected_label: &'static str) -> Self {
        Self {
            bus,
            connected_label,
            sphere: std::ptr::null_mut(),
            lbl_status: std::ptr::null_mut(),
            vu_bars: [std::ptr::null_mut(); VU_BANDS],
            btn_mode: std::ptr::null_mut(),
            lbl_mode: std::ptr::null_mut(),
            btn_rec: std::ptr::null_mut(),
            btn_mute: std::ptr::null_mut(),
            lbl_nr: std::ptr::null_mut(),
            lbl_upload: std::ptr::null_mut(),
        }
    }

    /// Update all state-driven labels. Call each LVGL tick.
    pub unsafe fn update_state(&self, audio: &AudioState, meeting: &MeetingState) {
        // VU bars
        for (i, &bar) in self.vu_bars.iter().enumerate() {
            if bar.is_null() { continue; }
            let h = vu_bar_height(audio.vu_level[i]);
            lv_obj_set_height(bar, h.max(2)); // min 2px so bar is visible
        }

        // Mic mode label on button
        let mode_str = mode_short_name(&audio.mode);
        let s = CString::new(mode_str).unwrap_or_default();
        lv_label_set_text(self.lbl_mode, s.as_ptr());

        // NR level in bottom bar
        let nr_str = CString::new(format!("NR:{:?}", audio.nr_level)).unwrap_or_default();
        lv_label_set_text(self.lbl_nr, nr_str.as_ptr());

        // Upload queue count
        let upload_str = if meeting.pending_uploads == 0 {
            CString::new("☁ 0").unwrap()
        } else {
            CString::new(format!("☁ {}", meeting.pending_uploads)).unwrap()
        };
        lv_label_set_text(self.lbl_upload, upload_str.unwrap().as_ptr());
    }
}

fn mode_short_name(mode: &AudioMode) -> &'static str {
    match mode {
        AudioMode::Omni => "OMNI",
        AudioMode::Stereo => "STEREO",
        AudioMode::Cardioid => "CARD.",
        AudioMode::Supercardioid => "S-CARD.",
        AudioMode::Figure8 => "FIG-8",
        AudioMode::NoiseReduction => "NR",
    }
}

impl super::Screen for HomeScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            let screen = lv_obj_create(lv_scr_act());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_all(screen, 0, 0);

            // Sphere: 180px circle, bottom-center, teal gradient effect via bg gradient
            let sphere = lv_obj_create(screen);
            lv_obj_set_size(sphere, 180, 180);
            lv_obj_align(sphere, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, 30); // partially off-screen bottom
            lv_obj_set_style_radius(sphere, 90, 0);
            lv_obj_set_style_bg_color(sphere, rgb(0x071a14), 0);
            lv_obj_set_style_bg_grad_color(sphere, rgb(0x1a5c4a), 0);
            lv_obj_set_style_bg_grad_dir(sphere, lv_grad_dir_t_LV_GRAD_DIR_VER, 0);
            lv_obj_set_style_shadow_color(sphere, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_shadow_width(sphere, 30, 0);
            lv_obj_set_style_shadow_opa(sphere, 100, 0);
            lv_obj_set_style_border_width(sphere, 0, 0);
            self.sphere = sphere;

            // Status label inside sphere
            let lbl = lv_label_create(sphere);
            lv_obj_set_style_text_color(lbl, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
            lv_label_set_text(lbl, c"● Connected".as_ptr());
            lv_obj_align(lbl, lv_align_t_LV_ALIGN_TOP_MID, 0, 20);
            self.lbl_status = lbl;

            // VU bars inside sphere, below status label
            let vu_total_w = VU_BANDS as i32 * (VU_BAR_W + VU_BAR_GAP) - VU_BAR_GAP;
            for i in 0..VU_BANDS {
                let bar = lv_obj_create(sphere);
                lv_obj_set_size(bar, VU_BAR_W, 2);
                let x = -vu_total_w / 2 + i as i32 * (VU_BAR_W + VU_BAR_GAP);
                lv_obj_set_pos(bar, 90 + x, 60); // relative to sphere
                lv_obj_set_style_bg_color(bar, rgb(CLR_ACCENT), 0);
                lv_obj_set_style_radius(bar, 2, 0);
                lv_obj_set_style_border_width(bar, 0, 0);
                self.vu_bars[i] = bar;
            }

            // 3 buttons row: [Mic Mode] [REC pill] [Mute]
            // Positioned 15px above bottom bar (bottom bar = 32px)
            let btn_y_from_bottom = -(32 + 15 + 38); // 38 = button height

            let btn_mode = lv_btn_create(screen);
            lv_obj_set_size(btn_mode, 60, 38);
            lv_obj_align(btn_mode, lv_align_t_LV_ALIGN_BOTTOM_MID, -80, btn_y_from_bottom);
            lv_obj_set_style_radius(btn_mode, 19, 0);
            lv_obj_set_style_bg_color(btn_mode, rgb(0x071a14), 0);
            lv_obj_set_style_shadow_color(btn_mode, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_shadow_width(btn_mode, 8, 0);
            self.btn_mode = btn_mode;

            let lbl_mode = lv_label_create(btn_mode);
            lv_label_set_text(lbl_mode, c"OMNI".as_ptr());
            lv_obj_set_style_text_font(lbl_mode, &lv_font_montserrat_12, 0);
            lv_obj_set_style_text_color(lbl_mode, rgb(CLR_ACCENT), 0);
            lv_obj_center(lbl_mode);
            self.lbl_mode = lbl_mode;

            // REC pill (wider, centered)
            let btn_rec = lv_btn_create(screen);
            lv_obj_set_size(btn_rec, 80, 32);
            lv_obj_align(btn_rec, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, btn_y_from_bottom + 3);
            lv_obj_set_style_radius(btn_rec, 16, 0);
            lv_obj_set_style_bg_color(btn_rec, rgb(0x007a5e), 0);
            lv_obj_set_style_shadow_color(btn_rec, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_shadow_width(btn_rec, 12, 0);
            self.btn_rec = btn_rec;

            let lbl_rec = lv_label_create(btn_rec);
            lv_label_set_text(lbl_rec, c"● REC".as_ptr());
            lv_obj_set_style_text_font(lbl_rec, &lv_font_montserrat_14, 0);
            lv_obj_set_style_text_color(lbl_rec, rgb(CLR_WHITE), 0);
            lv_obj_center(lbl_rec);

            // Mute button
            let btn_mute = lv_btn_create(screen);
            lv_obj_set_size(btn_mute, 60, 38);
            lv_obj_align(btn_mute, lv_align_t_LV_ALIGN_BOTTOM_MID, 80, btn_y_from_bottom);
            lv_obj_set_style_radius(btn_mute, 19, 0);
            lv_obj_set_style_bg_color(btn_mute, rgb(0x071a14), 0);
            self.btn_mute = btn_mute;

            let lbl_mute = lv_label_create(btn_mute);
            lv_label_set_text(lbl_mute, c"Mute".as_ptr());
            lv_obj_set_style_text_font(lbl_mute, &lv_font_montserrat_12, 0);
            lv_obj_set_style_text_color(lbl_mute, rgb(CLR_TEXT_DIM), 0);
            lv_obj_center(lbl_mute);

            // Bottom bar: 32px strip at very bottom
            let bbar = lv_obj_create(screen);
            lv_obj_set_size(bbar, 400, 32);
            lv_obj_align(bbar, lv_align_t_LV_ALIGN_BOTTOM_LEFT, 0, 0);
            lv_obj_set_style_bg_color(bbar, rgb(0x080808), 0);
            lv_obj_set_style_border_width(bbar, 0, 0);
            lv_obj_set_style_pad_all(bbar, 0, 0);

            // Bottom bar labels
            let mk_bbar_lbl = |text: &[u8], x: i32| {
                let lbl = lv_label_create(bbar);
                lv_label_set_text(lbl, text.as_ptr() as *const i8);
                lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
                lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT_DIM), 0);
                lv_obj_align(lbl, lv_align_t_LV_ALIGN_LEFT_MID, x, 0);
                lbl
            };

            let lbl_nr = mk_bbar_lbl(b"NR:Off\0", 8);
            mk_bbar_lbl(b"Files\0", 108);
            let lbl_upload = mk_bbar_lbl(b"SS 0\0", 208);
            mk_bbar_lbl(b"Settings\0", 300);

            self.lbl_nr = lbl_nr;
            self.lbl_upload = lbl_upload;

            // Add event callbacks for buttons (navigate to mic mode, start recording, toggle mute)
            // lv_obj_add_event_cb pattern — omitted here; implement as part of integration task.

            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vu_level_clamps_above_one() {
        assert!(clamp_vu(1.5) <= 1.0);
    }

    #[test]
    fn vu_bar_height_scales_correctly() {
        assert_eq!(vu_bar_height(0.0), 0);
        assert_eq!(vu_bar_height(1.0), VU_MAX_H);
        assert_eq!(vu_bar_height(0.5), VU_MAX_H / 2);
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cd waved
cargo test -p waved-ui --features simulate 2>&1 | tail -15
```

Expected: `vu_level_clamps_above_one` and `vu_bar_height_scales_correctly` pass.

- [ ] **Step 4: Commit**

```bash
cd waved
git add crates/waved-ui/src/screens/home.rs
git commit -m "feat(ui): home screen with sphere, VU meter, 3 buttons, bottom bar"
```

---

## Task 8: Recording screen

**Files:**
- Create: `waved/crates/waved-ui/src/screens/recording.rs`

- [ ] **Step 1: Write the failing test**

Add to the test block inside `recording.rs`:

```rust
#[test]
fn format_duration_zero() {
    assert_eq!(format_duration(0), "00:00:00");
}

#[test]
fn format_duration_one_hour() {
    assert_eq!(format_duration(3661), "01:01:01");
}
```

- [ ] **Step 2: Implement `recording.rs`**

Create `waved/crates/waved-ui/src/screens/recording.rs`:

```rust
//! Recording screen: red sphere, elapsed timer, VU meter, bookmark + stop + mute.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use crate::screens::home::{VU_MAX_H, VU_BAR_W, VU_BAR_GAP, VU_BANDS, vu_bar_height};
use waved_state::{AudioState, AudioMode, MeetingState, StateBus, Command};

pub fn format_duration(secs: u32) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

pub struct RecordingScreen {
    bus: Arc<StateBus>,
    sphere: *mut lv_obj_t,
    lbl_timer: *mut lv_obj_t,
    vu_bars: [*mut lv_obj_t; VU_BANDS],
    lbl_mode_badge: *mut lv_obj_t,
    lbl_storage: *mut lv_obj_t,
}

impl RecordingScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            sphere: std::ptr::null_mut(),
            lbl_timer: std::ptr::null_mut(),
            vu_bars: [std::ptr::null_mut(); VU_BANDS],
            lbl_mode_badge: std::ptr::null_mut(),
            lbl_storage: std::ptr::null_mut(),
        }
    }

    pub unsafe fn update_state(&self, audio: &AudioState, meeting: &MeetingState) {
        // Timer
        let dur = format_duration(meeting.duration_secs);
        let s = CString::new(dur).unwrap_or_default();
        lv_label_set_text(self.lbl_timer, s.as_ptr());

        // VU bars
        for (i, &bar) in self.vu_bars.iter().enumerate() {
            if bar.is_null() { continue; }
            lv_obj_set_height(bar, vu_bar_height(audio.vu_level[i]).max(2));
        }

        // Mode badge
        let badge = CString::new(format!(
            "{} · NR:{:?}",
            match audio.mode {
                AudioMode::Omni => "OMNI",
                AudioMode::Stereo => "STEREO",
                AudioMode::Cardioid => "CARD.",
                AudioMode::Supercardioid => "S-CARD.",
                AudioMode::Figure8 => "FIG-8",
                AudioMode::NoiseReduction => "NR",
            },
            audio.nr_level,
        )).unwrap_or_default();
        lv_label_set_text(self.lbl_mode_badge, badge.as_ptr());
    }
}

impl super::Screen for RecordingScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            let screen = lv_obj_create(lv_scr_act());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_all(screen, 0, 0);

            // Red sphere (recording active)
            let sphere = lv_obj_create(screen);
            lv_obj_set_size(sphere, 180, 180);
            lv_obj_align(sphere, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, 30);
            lv_obj_set_style_radius(sphere, 90, 0);
            lv_obj_set_style_bg_color(sphere, rgb(0x2a0a0a), 0);
            lv_obj_set_style_bg_grad_color(sphere, rgb(0x5a1010), 0);
            lv_obj_set_style_bg_grad_dir(sphere, lv_grad_dir_t_LV_GRAD_DIR_VER, 0);
            lv_obj_set_style_shadow_color(sphere, rgb(CLR_RED), 0);
            lv_obj_set_style_shadow_width(sphere, 30, 0);
            lv_obj_set_style_shadow_opa(sphere, 120, 0);
            lv_obj_set_style_border_width(sphere, 0, 0);
            self.sphere = sphere;

            // ● RECORDING label
            let lbl_rec = lv_label_create(sphere);
            lv_label_set_text(lbl_rec, c"● RECORDING".as_ptr());
            lv_obj_set_style_text_color(lbl_rec, rgb(CLR_RED), 0);
            lv_obj_set_style_text_font(lbl_rec, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_rec, lv_align_t_LV_ALIGN_TOP_MID, 0, 12);

            // Timer — large font
            let lbl_timer = lv_label_create(sphere);
            lv_label_set_text(lbl_timer, c"00:00:00".as_ptr());
            lv_obj_set_style_text_color(lbl_timer, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(lbl_timer, &lv_font_montserrat_20, 0);
            lv_obj_align(lbl_timer, lv_align_t_LV_ALIGN_TOP_MID, 0, 30);
            self.lbl_timer = lbl_timer;

            // VU bars
            let vu_total_w = VU_BANDS as i32 * (VU_BAR_W + VU_BAR_GAP) - VU_BAR_GAP;
            for i in 0..VU_BANDS {
                let bar = lv_obj_create(sphere);
                lv_obj_set_size(bar, VU_BAR_W, 2);
                let x = -vu_total_w / 2 + i as i32 * (VU_BAR_W + VU_BAR_GAP);
                lv_obj_set_pos(bar, 90 + x, 60);
                lv_obj_set_style_bg_color(bar, rgb(CLR_RED), 0);
                lv_obj_set_style_radius(bar, 2, 0);
                lv_obj_set_style_border_width(bar, 0, 0);
                self.vu_bars[i] = bar;
            }

            // Mode badge (top of content area, above sphere)
            let lbl_mode = lv_label_create(screen);
            lv_label_set_text(lbl_mode, c"OMNI · NR:Off".as_ptr());
            lv_obj_set_style_text_color(lbl_mode, rgb(CLR_ACCENT), 0);
            lv_obj_set_style_text_font(lbl_mode, &lv_font_montserrat_12, 0);
            lv_obj_align(lbl_mode, lv_align_t_LV_ALIGN_TOP_MID, 0, 38); // below status bar
            self.lbl_mode_badge = lbl_mode;

            // 3 buttons: [Mark] [STOP] [Mute]
            let btn_y = -(32 + 15 + 38);

            let btn_mark = lv_btn_create(screen);
            lv_obj_set_size(btn_mark, 60, 38);
            lv_obj_align(btn_mark, lv_align_t_LV_ALIGN_BOTTOM_MID, -80, btn_y);
            lv_obj_set_style_radius(btn_mark, 19, 0);
            lv_obj_set_style_bg_color(btn_mark, rgb(0x071a14), 0);
            let lbl_mark = lv_label_create(btn_mark);
            lv_label_set_text(lbl_mark, c"Mark".as_ptr());
            lv_obj_set_style_text_color(lbl_mark, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(lbl_mark, &lv_font_montserrat_12, 0);
            lv_obj_center(lbl_mark);

            let btn_stop = lv_btn_create(screen);
            lv_obj_set_size(btn_stop, 80, 32);
            lv_obj_align(btn_stop, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, btn_y + 3);
            lv_obj_set_style_radius(btn_stop, 16, 0);
            lv_obj_set_style_bg_color(btn_stop, rgb(0x8b0000), 0);
            let lbl_stop = lv_label_create(btn_stop);
            lv_label_set_text(lbl_stop, c"■ STOP".as_ptr());
            lv_obj_set_style_text_color(lbl_stop, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(lbl_stop, &lv_font_montserrat_14, 0);
            lv_obj_center(lbl_stop);

            let btn_mute = lv_btn_create(screen);
            lv_obj_set_size(btn_mute, 60, 38);
            lv_obj_align(btn_mute, lv_align_t_LV_ALIGN_BOTTOM_MID, 80, btn_y);
            lv_obj_set_style_radius(btn_mute, 19, 0);
            lv_obj_set_style_bg_color(btn_mute, rgb(0x071a14), 0);
            let lbl_mute = lv_label_create(btn_mute);
            lv_label_set_text(lbl_mute, c"Mute".as_ptr());
            lv_obj_set_style_text_color(lbl_mute, rgb(CLR_TEXT_DIM), 0);
            lv_obj_set_style_text_font(lbl_mute, &lv_font_montserrat_12, 0);
            lv_obj_center(lbl_mute);

            // Bottom bar
            let bbar = lv_obj_create(screen);
            lv_obj_set_size(bbar, 400, 32);
            lv_obj_align(bbar, lv_align_t_LV_ALIGN_BOTTOM_LEFT, 0, 0);
            lv_obj_set_style_bg_color(bbar, rgb(0x080808), 0);
            lv_obj_set_style_border_width(bbar, 0, 0);
            lv_obj_set_style_pad_all(bbar, 0, 0);

            let mk = |text: &[u8], x: i32| {
                let lbl = lv_label_create(bbar);
                lv_label_set_text(lbl, text.as_ptr() as *const i8);
                lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
                lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT_DIM), 0);
                lv_obj_align(lbl, lv_align_t_LV_ALIGN_LEFT_MID, x, 0);
                lbl
            };
            mk(b"OMNI\0", 8);
            mk(b"NR:Off\0", 88);
            let lbl_storage = mk(b"-- GB\0", 188);
            mk(b"Settings\0", 300);
            self.lbl_storage = lbl_storage;

            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_zero() {
        assert_eq!(format_duration(0), "00:00:00");
    }

    #[test]
    fn format_duration_one_hour() {
        assert_eq!(format_duration(3661), "01:01:01");
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cd waved
cargo test -p waved-ui --features simulate 2>&1 | tail -15
```

Expected: `format_duration_zero` and `format_duration_one_hour` pass.

- [ ] **Step 4: Commit**

```bash
cd waved
git add crates/waved-ui/src/screens/recording.rs
git commit -m "feat(ui): recording screen with red sphere, timer, VU, bookmark"
```

---

## Task 9: Mic Mode screen + Control Center overlay

**Files:**
- Create: `waved/crates/waved-ui/src/screens/audio_mode.rs`
- Create: `waved/crates/waved-ui/src/screens/control_center.rs`

- [ ] **Step 1: Write failing tests**

Create `waved/crates/waved-ui/src/screens/audio_mode.rs` with test block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use waved_state::AudioMode;

    #[test]
    fn all_modes_have_labels() {
        let modes = [
            AudioMode::Omni,
            AudioMode::Stereo,
            AudioMode::Cardioid,
            AudioMode::Supercardioid,
            AudioMode::Figure8,
        ];
        for mode in &modes {
            assert!(!mode_label(mode).is_empty(), "no label for {mode:?}");
        }
    }
}
```

```bash
cd waved
cargo test -p waved-ui --features simulate 2>&1 | grep -E "error|FAILED" | head -5
```

Expected: compile error — `mode_label` not defined.

- [ ] **Step 2: Implement `audio_mode.rs`**

Create `waved/crates/waved-ui/src/screens/audio_mode.rs`:

```rust
//! Mic Mode picker — 5 modes shown as selectable cards with beamforming icons.
//! Icons are PNG files from the stock firmware, embedded at compile time.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{AudioMode, StateBus, Command};

pub fn mode_label(mode: &AudioMode) -> &'static str {
    match mode {
        AudioMode::Omni         => "Omni",
        AudioMode::Stereo       => "Stereo",
        AudioMode::Cardioid     => "Cardioid",
        AudioMode::Supercardioid => "Super-card.",
        AudioMode::Figure8      => "Figure-8",
        AudioMode::NoiseReduction => "NR",
    }
}

const MODES: [AudioMode; 5] = [
    AudioMode::Omni,
    AudioMode::Stereo,
    AudioMode::Cardioid,
    AudioMode::Supercardioid,
    AudioMode::Figure8,
];

pub struct AudioModeScreen {
    bus: Arc<StateBus>,
    current_mode: AudioMode,
}

impl AudioModeScreen {
    pub fn new(bus: Arc<StateBus>, current_mode: AudioMode) -> Self {
        Self { bus, current_mode }
    }
}

impl super::Screen for AudioModeScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            let screen = lv_obj_create(lv_scr_act());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_all(screen, 0, 0);

            // Title
            let title = lv_label_create(screen);
            lv_label_set_text(title, c"Mic Mode".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_TEXT), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_16, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 38);

            // 5 mode cards arranged in a 3+2 grid
            // Card size: 100x80, arranged in content area (240px tall, starting at y=64)
            let card_w = 100i32;
            let card_h = 80i32;
            let gap = 10i32;

            for (i, mode) in MODES.iter().enumerate() {
                let row = i / 3;
                let col = i % 3;
                let total_row_w = if row == 0 { 3 * card_w + 2 * gap } else { 2 * card_w + gap };
                let x_start = (400 - total_row_w) / 2;
                let x = x_start + col as i32 * (card_w + gap);
                let y = 64 + row as i32 * (card_h + gap);

                let card = lv_obj_create(screen);
                lv_obj_set_size(card, card_w, card_h);
                lv_obj_set_pos(card, x, y);
                lv_obj_set_style_bg_color(card, rgb(CLR_SURFACE), 0);
                lv_obj_set_style_radius(card, 8, 0);

                // Highlight active mode
                if *mode == self.current_mode {
                    lv_obj_set_style_border_color(card, rgb(CLR_ACCENT), 0);
                    lv_obj_set_style_border_width(card, 2, 0);
                } else {
                    lv_obj_set_style_border_width(card, 0, 0);
                }

                let lbl = lv_label_create(card);
                let label_str = CString::new(mode_label(mode)).unwrap_or_default();
                lv_label_set_text(lbl, label_str.as_ptr());
                lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT), 0);
                lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
                lv_obj_align(lbl, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -8);

                // Tap callback: send SetAudioMode command and navigate back
                // lv_obj_add_event_cb wiring done in integration task
            }

            screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waved_state::AudioMode;

    #[test]
    fn all_modes_have_labels() {
        let modes = [
            AudioMode::Omni,
            AudioMode::Stereo,
            AudioMode::Cardioid,
            AudioMode::Supercardioid,
            AudioMode::Figure8,
        ];
        for mode in &modes {
            assert!(!mode_label(mode).is_empty(), "no label for {mode:?}");
        }
    }
}
```

- [ ] **Step 3: Implement `control_center.rs`**

Create `waved/crates/waved-ui/src/screens/control_center.rs`:

```rust
//! Control Center overlay.
//! Slides down from top of content area (y: -240 → 0) over the active screen.
//! Three sections: WiFi status, Noise Reduction (Off/Low/Med/High), Mic Arm.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{NrLevel, MotorPosition, StateBus, Command};

const NR_LEVELS: [NrLevel; 4] = [NrLevel::Off, NrLevel::Low, NrLevel::Med, NrLevel::High];

pub struct ControlCenter {
    bus: Arc<StateBus>,
    panel: *mut lv_obj_t,
    visible: bool,
    lbl_wifi: *mut lv_obj_t,
    nr_btns: [*mut lv_obj_t; 4],
    active_nr: NrLevel,
}

impl ControlCenter {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            panel: std::ptr::null_mut(),
            visible: false,
            lbl_wifi: std::ptr::null_mut(),
            nr_btns: [std::ptr::null_mut(); 4],
            active_nr: NrLevel::Off,
        }
    }

    /// Create the overlay panel on lv_layer_top() so it renders above screens.
    /// Starts hidden (y = -240).
    pub unsafe fn init(&mut self) {
        let top = lv_layer_top();
        let panel = lv_obj_create(top);
        lv_obj_set_size(panel, 400, 240); // content area height
        lv_obj_set_pos(panel, 0, -240);   // hidden above
        lv_obj_set_style_bg_color(panel, rgb(0x050f0d), 0);
        lv_obj_set_style_bg_opa(panel, 245, 0);
        lv_obj_set_style_border_width(panel, 0, 0);
        lv_obj_set_style_radius(panel, 0, 0);
        lv_obj_set_style_pad_all(panel, 12, 0);
        self.panel = panel;

        // Header
        let hdr = lv_label_create(panel);
        lv_label_set_text(hdr, c"CONTROL CENTER".as_ptr());
        lv_obj_set_style_text_color(hdr, rgb(CLR_ACCENT), 0);
        lv_obj_set_style_text_font(hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 0);

        // WiFi section
        let wifi_box = lv_obj_create(panel);
        lv_obj_set_size(wifi_box, 376, 56);
        lv_obj_align(wifi_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 20);
        lv_obj_set_style_bg_color(wifi_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(wifi_box, 8, 0);
        lv_obj_set_style_border_width(wifi_box, 0, 0);

        let lbl_wifi_hdr = lv_label_create(wifi_box);
        lv_label_set_text(lbl_wifi_hdr, c"WIFI".as_ptr());
        lv_obj_set_style_text_color(lbl_wifi_hdr, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(lbl_wifi_hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_wifi_hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 6);

        let lbl_wifi = lv_label_create(wifi_box);
        lv_label_set_text(lbl_wifi, c"Not connected".as_ptr());
        lv_obj_set_style_text_color(lbl_wifi, rgb(CLR_TEXT), 0);
        lv_obj_set_style_text_font(lbl_wifi, &lv_font_montserrat_14, 0);
        lv_obj_align(lbl_wifi, lv_align_t_LV_ALIGN_BOTTOM_LEFT, 8, -6);
        self.lbl_wifi = lbl_wifi;

        // NR section
        let nr_box = lv_obj_create(panel);
        lv_obj_set_size(nr_box, 376, 56);
        lv_obj_align(nr_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 84);
        lv_obj_set_style_bg_color(nr_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(nr_box, 8, 0);
        lv_obj_set_style_border_width(nr_box, 0, 0);

        let lbl_nr_hdr = lv_label_create(nr_box);
        lv_label_set_text(lbl_nr_hdr, c"NOISE REDUCTION".as_ptr());
        lv_obj_set_style_text_color(lbl_nr_hdr, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(lbl_nr_hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_nr_hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 6);

        let nr_labels = [c"Off", c"Low", c"Med", c"High"];
        for (i, &lbl_text) in nr_labels.iter().enumerate() {
            let btn = lv_btn_create(nr_box);
            let btn_w = (376 - 24 - 3 * 4) / 4;
            lv_obj_set_size(btn, btn_w, 24);
            lv_obj_set_pos(btn, 8 + i as i32 * (btn_w + 4), 26);
            lv_obj_set_style_radius(btn, 5, 0);
            lv_obj_set_style_bg_color(btn, rgb(0x071410), 0);
            lv_obj_set_style_border_width(btn, 1, 0);
            lv_obj_set_style_border_color(btn, rgb(CLR_TEXT_DIM), 0);

            let lbl = lv_label_create(btn);
            lv_label_set_text(lbl, lbl_text.as_ptr());
            lv_obj_set_style_text_font(lbl, &lv_font_montserrat_12, 0);
            lv_obj_set_style_text_color(lbl, rgb(CLR_TEXT_DIM), 0);
            lv_obj_center(lbl);

            self.nr_btns[i] = btn;
        }
        self.highlight_active_nr();

        // Motor arm section
        let arm_box = lv_obj_create(panel);
        lv_obj_set_size(arm_box, 376, 56);
        lv_obj_align(arm_box, lv_align_t_LV_ALIGN_TOP_LEFT, 0, 148);
        lv_obj_set_style_bg_color(arm_box, rgb(CLR_SURFACE), 0);
        lv_obj_set_style_radius(arm_box, 8, 0);
        lv_obj_set_style_border_width(arm_box, 0, 0);

        let lbl_arm_hdr = lv_label_create(arm_box);
        lv_label_set_text(lbl_arm_hdr, c"MIC ARM".as_ptr());
        lv_obj_set_style_text_color(lbl_arm_hdr, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(lbl_arm_hdr, &lv_font_montserrat_12, 0);
        lv_obj_align(lbl_arm_hdr, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 6);

        let btn_extend = lv_btn_create(arm_box);
        lv_obj_set_size(btn_extend, 180, 24);
        lv_obj_set_pos(btn_extend, 8, 26);
        lv_obj_set_style_radius(btn_extend, 5, 0);
        lv_obj_set_style_bg_color(btn_extend, rgb(CLR_ACCENT), 0);
        lv_obj_set_style_border_width(btn_extend, 0, 0);
        let lbl_ext = lv_label_create(btn_extend);
        lv_label_set_text(lbl_ext, c"UP Extend".as_ptr());
        lv_obj_set_style_text_font(lbl_ext, &lv_font_montserrat_12, 0);
        lv_obj_set_style_text_color(lbl_ext, rgb(CLR_BG), 0);
        lv_obj_center(lbl_ext);

        let btn_retract = lv_btn_create(arm_box);
        lv_obj_set_size(btn_retract, 180, 24);
        lv_obj_set_pos(btn_retract, 196, 26);
        lv_obj_set_style_radius(btn_retract, 5, 0);
        lv_obj_set_style_bg_color(btn_retract, rgb(0x071410), 0);
        lv_obj_set_style_border_width(btn_retract, 1, 0);
        lv_obj_set_style_border_color(btn_retract, rgb(CLR_TEXT_DIM), 0);
        let lbl_ret = lv_label_create(btn_retract);
        lv_label_set_text(lbl_ret, c"DN Retract".as_ptr());
        lv_obj_set_style_text_font(lbl_ret, &lv_font_montserrat_12, 0);
        lv_obj_set_style_text_color(lbl_ret, rgb(CLR_TEXT_DIM), 0);
        lv_obj_center(lbl_ret);

        // Swipe-up hint at bottom
        let hint = lv_label_create(panel);
        lv_label_set_text(hint, c"swipe up to close".as_ptr());
        lv_obj_set_style_text_color(hint, rgb(CLR_TEXT_DIM), 0);
        lv_obj_set_style_text_font(hint, &lv_font_montserrat_12, 0);
        lv_obj_align(hint, lv_align_t_LV_ALIGN_BOTTOM_MID, 0, -4);
    }

    unsafe fn highlight_active_nr(&self) {
        for (i, &btn) in self.nr_btns.iter().enumerate() {
            if btn.is_null() { continue; }
            let is_active = NR_LEVELS[i] == self.active_nr;
            let border_clr = if is_active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
            let text_clr   = if is_active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
            lv_obj_set_style_border_color(btn, border_clr, 0);
            // Update child label color
            let lbl = lv_obj_get_child(btn, 0);
            if !lbl.is_null() {
                lv_obj_set_style_text_color(lbl, text_clr, 0);
            }
        }
    }

    /// Slide in from top. Uses LVGL animation.
    pub unsafe fn show(&mut self) {
        if self.visible { return; }
        self.visible = true;
        let anim_ptr = Box::into_raw(Box::new(std::mem::zeroed::<lv_anim_t>()));
        lv_anim_init(anim_ptr);
        lv_anim_set_var(anim_ptr, self.panel as *mut _);
        lv_anim_set_values(anim_ptr, -240, 32); // 32 = below status bar
        lv_anim_set_duration(anim_ptr, 200);
        lv_anim_set_exec_cb(anim_ptr, Some(anim_y_cb));
        lv_anim_start(anim_ptr);
    }

    /// Slide back up and hide.
    pub unsafe fn hide(&mut self) {
        if !self.visible { return; }
        self.visible = false;
        let anim_ptr = Box::into_raw(Box::new(std::mem::zeroed::<lv_anim_t>()));
        lv_anim_init(anim_ptr);
        lv_anim_set_var(anim_ptr, self.panel as *mut _);
        lv_anim_set_values(anim_ptr, 32, -240);
        lv_anim_set_duration(anim_ptr, 200);
        lv_anim_set_exec_cb(anim_ptr, Some(anim_y_cb));
        lv_anim_start(anim_ptr);
    }

    pub fn is_visible(&self) -> bool { self.visible }

    /// Update WiFi label from current state. Call each tick when CC is visible.
    pub unsafe fn update_wifi(&self, ssid: &str, connected: bool) {
        let text = if connected {
            CString::new(format!("{ssid} ●")).unwrap_or_default()
        } else {
            CString::new("Not connected").unwrap_or_default()
        };
        lv_label_set_text(self.lbl_wifi, text.as_ptr());
    }
}

unsafe extern "C" fn anim_y_cb(obj: *mut core::ffi::c_void, value: i32) {
    lv_obj_set_y(obj as *mut lv_obj_t, value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use waved_state::NrLevel;

    #[test]
    fn nr_levels_array_matches_buttons() {
        // 4 NR levels, 4 buttons
        assert_eq!(NR_LEVELS.len(), 4);
    }

    #[test]
    fn control_center_starts_hidden() {
        // Without LVGL init, just test the struct state
        let bus = Arc::new(waved_state::StateBus::new());
        let cc = ControlCenter::new(bus);
        assert!(!cc.is_visible());
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cd waved
cargo test -p waved-ui --features simulate 2>&1 | tail -15
```

Expected: all tests pass including `all_modes_have_labels`, `nr_levels_array_matches_buttons`, `control_center_starts_hidden`.

- [ ] **Step 5: Commit**

```bash
cd waved
git add crates/waved-ui/src/screens/audio_mode.rs \
        crates/waved-ui/src/screens/control_center.rs
git commit -m "feat(ui): mic mode picker and control center overlay"
```

---

## Task 10: Settings screen + wire waved-ui into waved binary

**Files:**
- Create: `waved/crates/waved-ui/src/screens/settings.rs`
- Modify: `waved/crates/waved/Cargo.toml`
- Modify: `waved/crates/waved/src/main.rs`

The remaining screens from the March plan (`files.rs`, `wifi.rs`, `wifi_info.rs`, `bluetooth.rs`, `volume.rs`, `height.rs`, `about.rs`, `power_off.rs`, `loading.rs`) follow the identical Screen trait pattern. After Task 10, implement each from the March plan at `docs/superpowers/plans/2026-03-19-waved-lvgl-ui.md`.

- [ ] **Step 1: Create `settings.rs` (pattern for all remaining screens)**

Create `waved/crates/waved-ui/src/screens/settings.rs`:

```rust
//! Settings menu screen — navigates to sub-screens.
//! Pattern: create screen → create list items → attach tap callbacks → return root.

use std::ffi::CString;
use crate::bindings::*;
use crate::theme::*;

pub struct SettingsScreen;

impl SettingsScreen {
    pub fn new() -> Self { Self }
}

impl super::Screen for SettingsScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            let screen = lv_obj_create(lv_scr_act());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_set_style_pad_all(screen, 0, 0);

            let title = lv_label_create(screen);
            lv_label_set_text(title, c"Settings".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_TEXT), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_16, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 38);

            // Settings list
            let list = lv_list_create(screen);
            lv_obj_set_size(list, 380, 200);
            lv_obj_align(list, lv_align_t_LV_ALIGN_TOP_MID, 0, 62);
            lv_obj_set_style_bg_color(list, rgb(CLR_BG), 0);
            lv_obj_set_style_border_width(list, 0, 0);

            let items = [
                "Audio Mode",
                "WiFi",
                "Bluetooth",
                "Volume",
                "Mic Arm Height",
                "About",
                "Power Off",
            ];
            for item in &items {
                let s = CString::new(*item).unwrap_or_default();
                let btn = lv_list_add_button(list, std::ptr::null(), s.as_ptr());
                lv_obj_set_style_bg_color(btn, rgb(CLR_SURFACE), 0);
                // Tap callbacks wired in integration task
            }

            screen
        }
    }
}
```

- [ ] **Step 2: Add `waved-ui` to `waved` Cargo.toml**

In `waved/crates/waved/Cargo.toml`, add to `[dependencies]`:

```toml
waved-ui = { path = "../waved-ui", optional = true }
```

Add to `[features]`:

```toml
[features]
simulate = [
    "waved-ui/simulate",
    # ... existing simulate deps
]
ui = ["dep:waved-ui"]
```

- [ ] **Step 3: Spawn UiService in `main.rs`**

In `waved/crates/waved/src/main.rs`, after the existing service spawns and before the signal-wait loop, add:

```rust
#[cfg(feature = "ui")]
{
    use waved_ui::UiService;
    info!("Starting UI service");
    let ui = UiService::new(Arc::clone(&bus), config.clone());
    // ui.join() would block — instead drop it; the thread runs until process exit.
    std::mem::forget(ui);
}
```

- [ ] **Step 4: Verify compilation**

```bash
cd waved
cargo check -p waved --features simulate 2>&1 | tail -10
cargo check -p waved --features "simulate,ui" 2>&1 | tail -10
```

Both expected: no errors.

- [ ] **Step 5: Run full test suite**

```bash
cd waved
cargo test --workspace --features simulate 2>&1 | tail -20
```

Expected: all tests pass. If any screen module is missing from `screens/mod.rs`, add a stub:

```rust
pub mod files;   // etc.
```

with a minimal `FilesScreen` struct implementing `Screen`.

- [ ] **Step 6: Commit**

```bash
cd waved
git add crates/waved-ui/src/screens/settings.rs \
        crates/waved/Cargo.toml \
        crates/waved/src/main.rs \
        crates/waved-ui/src/screens/mod.rs
git commit -m "feat(ui): settings screen, wire UiService into waved binary"
```

---

## Rollback note

If the LVGL build breaks cross-compilation (`cross build`), gate `waved-ui` behind the `ui` feature so the device binary can be built without it. The `simulate` feature already skips device I/O, so `cargo check --features simulate` is always safe on the Mac host.

---

## After this plan: remaining March plan screens

Once all tasks above are done, implement the remaining screens from the March 2026 plan (`docs/superpowers/plans/2026-03-19-waved-lvgl-ui.md`):

- `files.rs` — recording file list
- `wifi.rs` / `wifi_info.rs` — WiFi scan and connect
- `bluetooth.rs` — BT device list
- `volume.rs` — speaker + mic volume sliders
- `height.rs` — motor height control
- `about.rs` — device info
- `power_off.rs` — shutdown confirmation
- `loading.rs` — spinner

Each follows the `Screen` trait pattern shown in `settings.rs`.
