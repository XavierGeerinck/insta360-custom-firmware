# waved Display Support Design

**Date:** 2026-04-09  
**Status:** Approved  
**Depends on:** March 2026 `waved-lvgl-ui` plan (`docs/superpowers/plans/2026-03-19-waved-lvgl-ui.md`)

---

## Goal

Add a touchscreen UI to the `waved` daemon. The device screen is currently blank because `waved` is headless. This spec extends the existing `waved-ui` LVGL crate plan with the user's specific feature set: recording controls, mic mode selection, mute, noise reduction, swipe-down control center, VU meter, bookmark/chapter markers, and upload queue status.

---

## Hardware

| Property | Value |
|---|---|
| Display | 1.82" IPS capacitive touchscreen |
| Resolution | 400 × 304 px |
| Pixel format | RGB565 (16-bit) |
| Framebuffer | `/dev/fb0` |
| Backlight | `/sys/class/leds/lcd-backlight/brightness` (0–255) |
| Touch controller | ilitek, `/dev/input/event0`, evdev |
| Refresh rate | ~30 fps |

---

## Architecture

### Core infrastructure (from March plan — unchanged)

The `waved-ui` crate compiles LVGL 9.2 from C source via `build.rs` + `bindgen` and accesses it through raw Rust FFI. A dedicated OS thread named `ui-lvgl` runs the single-threaded LVGL event loop. On each tick the thread:

1. Calls `lv_tick_inc(elapsed_ms)` with real elapsed milliseconds
2. Polls the `UiCommand` channel with `try_recv()` (non-blocking — the LVGL thread is not async)
3. Calls `lv_timer_handler()`

User actions (button taps, slider changes) are sent back to the rest of `waved` by emitting variants on the `commands` broadcast channel of `StateBus`. The UI never writes to shared state directly.

The status bar is created once on `lv_layer_top()` and persists across screen changes. Screen content is created/destroyed on `lv_screen_active()` underneath it.

### New additions in this spec

| Addition | Description |
|---|---|
| Control Center overlay | Swipe-down from top triggers sliding panel: WiFi, NR, Mic Arm |
| Bookmark | Tap during recording → timestamped mark in `.marks` sidecar file |
| VU meter | 8-band audio level bars, driven by `AudioState.vu_level` |
| Upload queue indicator | `☁ N` on home bottom bar, from `MeetingState.upload_queue_count` |
| Noise reduction control | Off / Low / Med / High, in Control Center |
| Mic arm in Control Center | Extend / Retract motor command |

---

## Visual Design

**Palette:** Black background (`#000000`), teal accent (`#00c49a`), dark surface (`#0d1f1a`), white text (`#ffffff`), muted text (`#888888`). Matches the existing pspapp aesthetic exactly.

**Layout convention** (all screens):

```
┌──────────────────────────────────────┐  ← 400px wide
│  Status Bar                  32px    │
├──────────────────────────────────────┤
│                                      │
│         Content Area         240px   │
│                                      │
├──────────────────────────────────────┤
│  Bottom Bar / Nav            32px    │
└──────────────────────────────────────┘
```

**Central visual element:** A large radial-gradient sphere (180px diameter) centered horizontally, bottom-anchored, with a teal glow. On the home screen it is teal/dark-green; during recording it shifts to red/dark-red. This matches the `bg_main.png` and `bg_airecord.png` assets from the stock firmware.

---

## Screen Inventory

### Screens from March plan — kept unchanged

`files.rs`, `wifi.rs`, `wifi_info.rs`, `bluetooth.rs`, `settings.rs`, `volume.rs`, `height.rs`, `camera.rs`, `camera_ptz.rs`, `api_config.rs`, `about.rs`, `password.rs`, `ota.rs`, `power_off.rs`, `loading.rs`

### Screens from March plan — modified

#### `home.rs` (ID 10)

```
┌──────────────────────────────────────┐
│ 🔋78%  📶  BT                  09:41 │
├──────────────────────────────────────┤
│           ↓ swipe for controls       │
│                                      │
│        ╭──── sphere ────╮            │
│        │  teal glow     │            │
│        │  VU meter bars │            │
│        │  "Wave"        │            │
│        ╰────────────────╯            │
│                                      │
│   [🎙 OMNI]  [● REC]  [🔇 Mute]    │
│                                      │
├──────────────────────────────────────┤
│ [🔊 NR:Off] [📁 Files] [☁ 0] [⚙]  │
└──────────────────────────────────────┘
```

- Sphere: 180px circle, teal radial gradient, teal glow shadow, VU meter (8 bars) inside sphere area
- Three buttons below sphere: Mic Mode circle (shows current mode), REC pill (green gradient), Mute circle
- Bottom bar: NR indicator (shows current level), Files, Upload queue count (pulses teal when > 0), Settings
- Swipe-down from anywhere in top 40px opens Control Center

#### `recording.rs` (ID 12)

```
┌──────────────────────────────────────┐
│ 🔋78%  📶                  ● REC     │
├──────────────────────────────────────┤
│         [OMNI · NR: Off]             │
│                                      │
│        ╭──── red sphere ────╮        │
│        │  ● RECORDING       │        │
│        │  00:04:32          │        │
│        │  VU meter bars     │        │
│        ╰────────────────────╯        │
│                                      │
│  [🔖 Mark]  [■ STOP]  [🔇 Mute]    │
│                                      │
├──────────────────────────────────────┤
│ [🎙 OMNI] [🔊 NR:Off] [💾 47GB] [⚙]│
└──────────────────────────────────────┘
```

- Sphere shifts to red radial gradient + red glow
- Timer: large tabular-nums font, 00:00:00 format
- Bookmark button (left) replaces Mic Mode during active recording
- STOP pill (red gradient) replaces REC
- Mode badge at top shows current mic mode and NR level
- Bottom bar: Mic Mode, NR, Storage remaining, Settings

#### `audio_mode.rs` (ID 77) — renamed "Mic Mode"

5 modes displayed as selectable cards with beamforming pattern icons (reused from stock firmware assets at `/etc/pspapp/lvgl/icon/beamforming/`):

| Mode | Icon asset | Description |
|---|---|---|
| Omni | `bf_mode_all.png` | 360° pickup |
| Stereo | `bf_mode_stereo.png` | Left/right separation |
| Cardioid | `bf_mode_heart.png` | Front-facing |
| Super-cardioid | `bf_mode_super_heart.png` | Narrower front |
| Figure-8 | `bf_mode_eight.png` | Front + rear |

Tapping a card: sends `SetMicMode(mode)` on `commands` channel, navigates back to home. Active mode uses the `_check` icon variant (e.g. `bf_mode_heart_check.png`); inactive modes use `_uncheck`; unavailable modes use `_grey`. Active card has a teal border.

### New screens

#### `control_center.rs` — overlay (not a full screen)

Triggered by downward swipe starting in the top 40px of any screen. Slides down over the content area with a 200ms ease-out animation (y: −240 → 0, relative to the top of the content area). Status bar remains visible above it. Dismissed by swipe-up or tap outside.

Three stacked sections inside a dark surface panel (`#0d1f1a`, border-radius 12px):

**WiFi section**
- Current SSID + signal strength icon
- Connection status indicator (teal = connected, grey = not connected)
- Tap anywhere in section → navigates to `wifi.rs`

**Noise Reduction section**
- Label: "NOISE REDUCTION"
- 4-segment button matrix: Off | Low | Med | High
- Active segment: teal background + teal border
- Tap → sends `SetNoiseReduction(NrLevel)` on `commands` channel

**Mic Arm section**
- Label: "MIC ARM"
- Two buttons: ↑ Extend | ↓ Retract
- Active state shown by teal border on current position
- Tap → sends `SetMotorPosition(MotorCmd)` on `commands` channel

---

## StateBus Changes

### `AudioState` — add field

```rust
pub struct AudioState {
    // ... existing fields ...
    /// 8-band VU levels, each in [0.0, 1.0]. Updated at ~30fps by waved-audio.
    pub vu_level: [f32; 8],
}
```

`waved-audio` computes band levels from the audio callback and sends updated `AudioState` on the watch channel. The UI thread reads it each LVGL tick and updates bar heights.

### `MeetingState` — add field

```rust
pub struct MeetingState {
    // ... existing fields ...
    /// Number of recordings pending upload to the self-hosted server.
    pub upload_queue_count: u32,
}
```

### New `UiCommand` variants

```rust
pub enum UiCommand {
    // ... existing variants ...
    SetNoiseReduction(NrLevel),
    SetMicMode(MicMode),
    BookmarkNow,
    SetMotorPosition(MotorCmd),
}

pub enum NrLevel { Off, Low, Med, High }

pub enum MicMode { Omni, Stereo, Cardioid, SuperCardioid, FigureEight }

pub enum MotorCmd { Extend, Retract }
```

---

## Bookmark Mechanics

Bookmark is only available during active recording. When the user taps the Bookmark button:

1. UI emits `BookmarkNow` on the `commands` broadcast channel
2. `waved-meeting` listens for `BookmarkNow`, records `{ timestamp_ms, session_id }` to a sidecar `.marks` file (one JSON object per line) alongside the active recording file
3. UI shows a brief toast ("Bookmark added") for 1.5s

The `.marks` file is named `<recording-stem>.marks` and lives next to the recording in `/mnt/udisk/recordings/`.

---

## VU Meter

8 vertical bars, each 3px wide, 2px gap between bars, maximum height 20px. Color: teal (`#00c49a`) at low levels, interpolates toward white (`#ffffff`) at peak. Bar height = `vu_level[i] * 20px`. Updated every LVGL tick from the latest `AudioState` watch value (no additional channel needed).

Positioned inside the sphere area: centered horizontally, bottom of sphere.

---

## Simulate Mode

`waved-ui` gains a `simulate` Cargo feature flag (consistent with all other waved crates). When active:

- `/dev/fb0` and `/dev/input/event0` are not opened
- LVGL renders to an in-memory pixel buffer
- An SDL2 window displays the buffer on the Mac host
- Mouse clicks map to touch events

This enables full UI development and testing without the device.

---

## Error Handling

| Failure | Behavior |
|---|---|
| `/dev/fb0` open fails | UI thread logs error and exits; `waved` continues headless |
| `/dev/input/event0` unavailable | Touch silently disabled; display-only mode |
| LVGL C assert / panic | OS thread crashes; `S60waved` watchdog restarts daemon within 3s |
| `lang/en.json` missing | `tr()` returns the key string as fallback; no panic |
| WiFi state unknown | Control Center WiFi section shows "Not connected" |
| Bookmark tap outside recording | Button not rendered; command not emittable |

---

## Testing

| Test | Type | What it checks |
|---|---|---|
| `nav_state_machine` | Unit | Each `UiCommand` produces correct screen transition in `nav.rs` |
| `control_center_nr_picker` | Unit | Tapping each NR segment emits correct `SetNoiseReduction` variant |
| `i18n_missing_key_fallback` | Unit | Missing key returns key name, does not panic |
| `bookmark_only_during_recording` | Unit | `BookmarkNow` only emitted when `RecordingState::Recording` |
| `vu_meter_clamp` | Unit | `vu_level` values > 1.0 are clamped; bars never exceed max height |
| On-device smoke | Manual | UI starts, home screen renders, status bar shows battery/WiFi, REC tap navigates to recording screen |

---

## File Structure (delta from March plan)

All existing files from the March plan are retained. Additions:

```
waved/crates/waved-ui/src/screens/
└── control_center.rs          ← NEW: swipe-down overlay
```

Modified files:
```
waved/crates/waved-ui/src/screens/home.rs         ← sphere layout, VU, upload queue
waved/crates/waved-ui/src/screens/recording.rs    ← bookmark, VU, red sphere
waved/crates/waved-ui/src/screens/audio_mode.rs   ← 5 mic modes with beamforming icons
waved/crates/waved-ui/src/command.rs              ← new UiCommand variants
waved-state/src/lib.rs                             ← vu_level, upload_queue_count fields
```
