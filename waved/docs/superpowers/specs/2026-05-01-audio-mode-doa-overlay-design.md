# Audio Mode DOA Overlay — Design

**Date:** 2026-05-01
**Status:** Spec
**Scope:** UI only (waved-ui, plus a one-field state plumb in waved-state / waved-audio)

## Motivation

The original Insta360 firmware shows a small directional arrow over the polar-pattern image on the audio-mode selection carousel. While the user is browsing modes, the arrow rotates in real time to indicate the direction of arrival of the current source — letting the user see how each pattern would "see" them before they commit to a mode.

Our reimplementation already runs the DOA pipeline (`waved-audio/src/doa/mod.rs`) and exposes `AudioState.doa_horizontal`, but the audio-mode screen does not surface it. This spec adds the overlay, matching the original behavior.

## Reference: original behavior

From `decompiled/libui.so.c:21183` (`ui_setting_beamforming_screen_update_doa`):

- Each card has a `bf_doa.png` arrow as a child of the polar-pattern background image, initially hidden (`LV_OBJ_FLAG_HIDDEN`).
- Every UI tick:
  - Read the current DOA azimuth.
  - For the **currently carousel-centered card only**: if the DOA is "hot" (signal present), rotate the arrow to `(360 − az) × 10` (LVGL 0.1° units) and clear the hidden flag. Otherwise set hidden.
  - For all other cards: keep hidden.

The "centered" index is independent of the "selected" mode — original tracks it via a carousel scroll callback (`FUN_0017aef8` hooked to `LV_EVENT_SCROLL_END` at decompile line 21259) that finds the focused child after each snap. This matters because the original interaction model lets the user scroll to preview a card without committing; our implementation commits on click, but the user can still scroll past the selected card before tapping. Following the centered card gives the "see where audio is coming from before selecting" preview the user asked for.

## Design

### 1. State plumbing — surface VAD activity

`waved-audio/src/doa/mod.rs` already returns `DoaUpdate.vad_active`, but `waved-audio/src/lib.rs` only forwards `azimuth_deg`/`elevation_deg` into `AudioState`. When VAD is silent, `doa_horizontal` simply freezes at its last smoothed value, with no way for the UI to tell.

**Change:**

```rust
// waved-state/src/lib.rs — AudioState
pub doa_active: bool,   // new field
```

```rust
// waved-audio/src/lib.rs — inside the DOA send_modify block
s.doa_horizontal = doa.azimuth_deg;
s.doa_pitch      = doa.elevation_deg;
s.doa_active     = doa.vad_active;   // new
```

`Default for AudioState` keeps `doa_active = false` (derived `Default`).

### 2. UI — `crates/waved-ui/src/screens/audio_mode.rs`

#### 2a. New screen field

```rust
pub struct AudioModeScreen {
    bus:          Arc<StateBus>,
    cards:        [*mut lv_obj_t; 5],
    radios:       [*mut lv_obj_t; 5],
    indicators:   [*mut lv_obj_t; 5],
    doa_arrows:   [*mut lv_obj_t; 5],            // new
    centered_idx: Arc<AtomicUsize>,              // new — updated by scroll callback
    prev_mode:    Option<AudioMode>,
    prev_doa:     Option<DoaCacheKey>,           // new (see 2e)
}
```

#### 2b. Per-card arrow creation

In the existing card-build loop, after `mode_img` (the `bg_bf_mode_*.png`) is created, add a child:

```rust
let arrow = lv_image_create(mode_img);   // child of bg image
lv_obj_set_size(arrow, 124, 124);
lv_obj_set_pos(arrow, 0, 0);             // overlays the bg exactly
lv_image_set_src(arrow, b"A:/etc/pspapp/lvgl/icon/beamforming/bf_doa.png\0".as_ptr() as *const _);
lv_image_set_pivot(arrow, 62, 62);       // centre of 124×124
lv_obj_remove_flag(arrow, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
lv_obj_add_flag(arrow, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
self.doa_arrows[i] = arrow;
```

The arrow is a sibling-equivalent on top of the bg image; LVGL draw order is creation order, so it will render above the polar pattern.

#### 2c. Azimuth → LVGL rotation mapping

LVGL rotates clockwise; `lv_image_set_rotation` takes an `i32` in 0.1° units. Our `azimuth_deg` is signed (`[-180, +180]`) with +X = forward (0°), +Y = left (+90°). The original firmware used `(360 − angle) × 10` on a non-negative angle, which flips counter-clockwise → clockwise.

Extract a pure function so we can unit-test it:

```rust
/// Convert our DOA azimuth to an LVGL `set_rotation` argument.
///
/// LVGL rotates clockwise in 0.1° units; our azimuth is signed and
/// counter-clockwise-positive. Quantises to whole degrees so micro-jitter
/// from the EWMA smoother does not retrigger redraws.
pub(crate) fn az_to_lvgl_rotation(az_deg: f32) -> i32 {
    // Wrap to [0, 360).
    let mut a = az_deg.rem_euclid(360.0);
    if a >= 360.0 { a -= 360.0; }            // guard f32 edge
    let clockwise = (360.0 - a).rem_euclid(360.0);
    let whole_deg = clockwise.round() as i32 % 360;
    whole_deg * 10
}
```

Unit tests cover: 0° → 0, +90° → 2700, −90° → 900, +180° → 1800, −180° → 1800, wrap (e.g. 359.4° → 0).

#### 2d. Carousel scroll callback — track centered card

Initialize `centered_idx` to the saved-mode index in `create()` (matches the existing `lv_obj_scroll_to_view` of the initial card).

Hook `LV_EVENT_SCROLL_END` on the carousel. Ownership: clone the `Arc<AtomicUsize>` and `Arc::into_raw` it for `user_data`; the same matches existing pattern in this file (`Box::into_raw(Box::new(self.bus.commands.clone()))` at lines 254 and 314 — leaks once per `create()`, acceptable since screens are not destroyed mid-session).

```rust
unsafe extern "C" fn carousel_scroll_end_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) != lv_event_code_t_LV_EVENT_SCROLL_END { return; }
    let carousel = lv_event_get_target(e) as *mut lv_obj_t;
    let cell_ptr = lv_event_get_user_data(e) as *const AtomicUsize;
    if cell_ptr.is_null() || carousel.is_null() { return; }
    let new_idx = closest_centered_child(carousel);
    (*cell_ptr).store(new_idx, Ordering::Relaxed);
}
```

`closest_centered_child` is a small helper that walks `lv_obj_get_child_count` children, computes each child's x-centre (`lv_obj_get_x` + half its width minus carousel scroll-x), and returns the index of the one nearest the carousel's mid-x in viewport space. Single LVGL-dependent function; we extract the index-selection arithmetic into a pure helper for unit testing (input: list of (centre_x, width), viewport mid-x → output: index).

#### 2e. Per-tick update

Restructure `update_state` so:

- The existing `prev_mode` early-return only gates the **selection visuals** (radio icon, border, indicator strip).
- DOA-arrow update runs every tick, but is itself short-circuited by a cache:

```rust
#[derive(Clone, PartialEq)]
struct DoaCacheKey {
    centered: usize,
    rotation: i32,   // already quantised to 10× whole-deg
    active:   bool,
}
```

Logic:

```rust
let centered = self.centered_idx.load(Ordering::Relaxed);
let rotation = az_to_lvgl_rotation(audio.doa_horizontal);
let key = DoaCacheKey { centered, rotation, active: audio.doa_active };

if self.prev_doa.as_ref() != Some(&key) {
    for i in 0..5 {
        let arrow = self.doa_arrows[i];
        if arrow.is_null() { continue; }
        let show = i == centered && audio.doa_active;
        if show {
            lv_image_set_rotation(arrow, rotation);
            lv_obj_remove_flag(arrow, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        } else {
            lv_obj_add_flag(arrow, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        }
    }
    self.prev_doa = Some(key);
}
```

Selection logic that depends on `prev_mode` (border, radio icon, indicator strip) stays tied to `audio.mode` (the committed mode), just hoisted above the DOA block.

When `prev_mode` changes (i.e. selection committed), additionally call `lv_obj_scroll_to_view(self.cards[idx], LV_ANIM_ON)` so the carousel slides to the newly-selected card. The `SCROLL_END` callback then updates `centered_idx`, and the next tick renders the arrow on the correct card. This keeps selection and centred-card in sync after any tap, while still allowing the user to scroll-preview without committing.

#### 2f. Lifecycle

`create()` zeros `doa_arrows` and `prev_doa`, and resets `centered_idx` to the saved-mode index, same pattern as the existing arrays.

### 3. Testing

#### Unit tests (`cargo test -p waved-ui --lib`)
`az_to_lvgl_rotation`:
- 0° → 0
- +90° → 2700
- −90° → 900
- +180° → 1800
- −180° → 1800
- 45° → 3150
- 359.4° → 0 (wrap edge)
- −0.4° → 0 (wrap edge)

`closest_centered_child` arithmetic helper:
- Single child centred → index 0.
- Three equal-width children, viewport-mid between #1 and #2 (closer to #1) → index 1.
- Equal distance ties → lower index (deterministic).

#### On-device acceptance (manual)
1. Build & deploy (`cross build … && adb push …`).
2. Open the audio-mode picker.
3. Speak from in front of the device → arrow appears on the centered card pointing forward.
4. Move to the left side → arrow rotates ~90° CCW; right side → ~90° CW; behind → ~180°.
5. Stay silent for >1 s → arrow disappears (VAD off).
6. **Scroll** the carousel to a different card without tapping → previous card's arrow hides, new centered card's arrow appears once VAD trips. The radio icon / border still highlights the originally-selected card (selection unchanged until tap).
7. Tap a non-centered card → carousel scrolls/snaps; on `SCROLL_END` the new centered card gets the arrow; `Command::SetAudioMode` also commits it.
8. Non-centered cards never show the arrow.

## What this spec does NOT include

- Asset work — `bf_doa.png` is already deployed under `/etc/pspapp/lvgl/icon/beamforming/`.
- Changes to recording-screen or home-screen DOA behavior.
- "Show on all cards" mode.
- Elevation indicator on this screen — original only used azimuth.

## Files touched

- `waved/crates/waved-state/src/lib.rs` — add `doa_active: bool` to `AudioState`.
- `waved/crates/waved-audio/src/lib.rs` — assign `s.doa_active = doa.vad_active`.
- `waved/crates/waved-ui/src/screens/audio_mode.rs` — arrow creation, per-tick update, `az_to_lvgl_rotation` helper + unit tests.
