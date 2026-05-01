# Audio Mode DOA Overlay — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show a real-time direction-of-arrival arrow on the carousel-centered card of the audio-mode picker, matching the original Insta360 firmware behavior.

**Architecture:** Plumb the existing `DoaUpdate.vad_active` flag through `AudioState.doa_active` so the UI can gate visibility. In `screens/audio_mode.rs`, add a `bf_doa.png` child image to each card (hidden by default), hook `LV_EVENT_SCROLL_END` on the carousel to track which card is centered, and on every UI tick rotate + show/hide the arrow on the centered card based on `doa_horizontal` and `doa_active`.

**Tech Stack:** Rust, LVGL v9 (via FFI bindings in `waved-ui::bindings`), `tokio::sync::watch` for state propagation, `std::sync::atomic::AtomicUsize` for the centered-card index shared with the LVGL event callback.

**Spec:** `waved/docs/superpowers/specs/2026-05-01-audio-mode-doa-overlay-design.md`

---

## File Structure

| File | Responsibility | Change type |
|---|---|---|
| `waved/crates/waved-state/src/lib.rs` | `AudioState.doa_active: bool` field | Modify (1 field) |
| `waved/crates/waved-audio/src/lib.rs` | Forward `doa.vad_active` into `AudioState` | Modify (1 line) |
| `waved/crates/waved-ui/src/screens/audio_mode.rs` | Arrow overlay creation, scroll-end callback, per-tick rotation/visibility, helper functions | Modify (substantial) |

The two helper functions added to `audio_mode.rs` (`az_to_lvgl_rotation` and `nearest_index_to_midline`) are pure and unit-tested in the same file.

---

## Task 1: State plumbing — `AudioState.doa_active`

**Files:**
- Modify: `waved/crates/waved-state/src/lib.rs:16-34` (add field to `AudioState` struct)
- Modify: `waved/crates/waved-audio/src/lib.rs:269-273` (assign `doa.vad_active`)

- [ ] **Step 1: Add `doa_active` field to `AudioState`**

In `waved/crates/waved-state/src/lib.rs`, find the `AudioState` struct (around line 16) and add `doa_active: bool` after `doa_pitch`:

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
    pub doa_active: bool,
    pub vu_level: [f32; 8],
    pub incoming_vu_level: [f32; 8],
    // ... rest unchanged
```

`Default` is derived, so `doa_active` defaults to `false`. No other changes in this file.

- [ ] **Step 2: Forward `vad_active` in waved-audio**

In `waved/crates/waved-audio/src/lib.rs`, find the `send_modify` block at line 270. Currently:

```rust
self.audio_tx.send_modify(|s| {
    s.doa_horizontal = doa.azimuth_deg;
    s.doa_pitch = doa.elevation_deg;
});
```

Change to:

```rust
self.audio_tx.send_modify(|s| {
    s.doa_horizontal = doa.azimuth_deg;
    s.doa_pitch = doa.elevation_deg;
    s.doa_active = doa.vad_active;
});
```

- [ ] **Step 3: Build the workspace to verify the field is consumed correctly**

Run: `cd waved && cargo check -p waved-state -p waved-audio`
Expected: clean build with no warnings about the new field. Any consumers that destructure `AudioState` non-exhaustively will compile; any exhaustive match would error — there are none.

- [ ] **Step 4: Run existing tests**

Run: `cd waved && cargo test -p waved-state --lib && cargo test -p waved-audio --lib`
Expected: all existing tests pass.

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-state/src/lib.rs waved/crates/waved-audio/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(state): expose DOA VAD-active as AudioState.doa_active

Surface the VAD-active flag from the DOA pipeline into AudioState so
UI consumers can distinguish "live signal" from "frozen last azimuth".
EOF
)"
```

---

## Task 2: Pure helper — `az_to_lvgl_rotation`

**Files:**
- Modify: `waved/crates/waved-ui/src/screens/audio_mode.rs` (add helper + tests)

- [ ] **Step 1: Write the failing tests**

At the bottom of `waved/crates/waved-ui/src/screens/audio_mode.rs`, append:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn az_zero_maps_to_zero() {
        assert_eq!(az_to_lvgl_rotation(0.0), 0);
    }

    #[test]
    fn az_positive_90_maps_to_2700() {
        // +90° (left in our frame) → LVGL clockwise 270°
        assert_eq!(az_to_lvgl_rotation(90.0), 2700);
    }

    #[test]
    fn az_negative_90_maps_to_900() {
        assert_eq!(az_to_lvgl_rotation(-90.0), 900);
    }

    #[test]
    fn az_180_maps_to_1800() {
        assert_eq!(az_to_lvgl_rotation(180.0), 1800);
    }

    #[test]
    fn az_negative_180_maps_to_1800() {
        assert_eq!(az_to_lvgl_rotation(-180.0), 1800);
    }

    #[test]
    fn az_45_maps_to_3150() {
        assert_eq!(az_to_lvgl_rotation(45.0), 3150);
    }

    #[test]
    fn az_359_4_wraps_to_zero() {
        // 359.4° rounds to 359 → clockwise (360-359)=1, but the input is
        // already extremely close to 0, so after rounding it should be 0.
        // Verify wrap is well-behaved at the seam.
        let r = az_to_lvgl_rotation(359.4);
        assert!(r == 0 || r == 10, "got {r}");
    }

    #[test]
    fn az_negative_0_4_wraps_to_zero() {
        let r = az_to_lvgl_rotation(-0.4);
        assert!(r == 0 || r == 3590, "got {r}");
    }
}
```

The two wrap tests assert a tolerance because rounding-direction at the seam isn't load-bearing — both 0 and ±0.1° are visually indistinguishable on a 124×124 image.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd waved && cargo test -p waved-ui --lib screens::audio_mode::tests`
Expected: compile error — `az_to_lvgl_rotation` not defined.

- [ ] **Step 3: Implement the helper**

Above the `mod_to_idx` function (around line 51) in `waved/crates/waved-ui/src/screens/audio_mode.rs`, add:

```rust
/// Convert our DOA azimuth (signed, counter-clockwise positive, +X = forward)
/// to an `lv_image_set_rotation` argument (clockwise, 0.1° units).
///
/// Quantises to whole degrees so EWMA jitter does not retrigger redraws.
pub(crate) fn az_to_lvgl_rotation(az_deg: f32) -> i32 {
    let wrapped = az_deg.rem_euclid(360.0);
    let clockwise = (360.0 - wrapped).rem_euclid(360.0);
    let whole_deg = clockwise.round() as i32 % 360;
    whole_deg * 10
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd waved && cargo test -p waved-ui --lib screens::audio_mode::tests`
Expected: all 8 tests pass.

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-ui/src/screens/audio_mode.rs
git commit -m "$(cat <<'EOF'
feat(ui/audio_mode): add az_to_lvgl_rotation helper

Pure function mapping signed CCW-positive azimuth degrees to LVGL's
clockwise 0.1°-unit rotation argument. Quantises to whole degrees so
EWMA-smoother micro-jitter does not retrigger redraws.
EOF
)"
```

---

## Task 3: Pure helper — `nearest_index_to_midline`

**Files:**
- Modify: `waved/crates/waved-ui/src/screens/audio_mode.rs` (add helper + tests)

- [ ] **Step 1: Write the failing tests**

Inside the existing `tests` module appended in Task 2, add:

```rust
#[test]
fn nearest_single_child_returns_zero() {
    assert_eq!(nearest_index_to_midline(&[100], 200), 0);
}

#[test]
fn nearest_three_children_picks_closest_to_midline() {
    // Three card centres at 50, 200, 350; midline at 220 → centre #1 is closest.
    assert_eq!(nearest_index_to_midline(&[50, 200, 350], 220), 1);
}

#[test]
fn nearest_picks_lower_index_on_tie() {
    // Two equidistant from midline → return the lower index, deterministically.
    assert_eq!(nearest_index_to_midline(&[100, 300], 200), 0);
}

#[test]
fn nearest_works_when_midline_left_of_all() {
    assert_eq!(nearest_index_to_midline(&[100, 200, 300], 0), 0);
}

#[test]
fn nearest_works_when_midline_right_of_all() {
    assert_eq!(nearest_index_to_midline(&[100, 200, 300], 1000), 2);
}

#[test]
#[should_panic]
fn nearest_panics_on_empty() {
    nearest_index_to_midline(&[], 0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd waved && cargo test -p waved-ui --lib screens::audio_mode::tests::nearest`
Expected: compile error — `nearest_index_to_midline` not defined.

- [ ] **Step 3: Implement the helper**

Below `az_to_lvgl_rotation` (added in Task 2), add:

```rust
/// Return the index of the centre closest to `midline`. Ties go to the
/// lower index (deterministic). Panics on empty input — caller should
/// always have ≥1 card.
pub(crate) fn nearest_index_to_midline(centres: &[i32], midline: i32) -> usize {
    assert!(!centres.is_empty(), "no carousel children");
    let mut best_idx = 0usize;
    let mut best_dist = (centres[0] - midline).abs();
    for (i, &c) in centres.iter().enumerate().skip(1) {
        let d = (c - midline).abs();
        if d < best_dist {
            best_dist = d;
            best_idx = i;
        }
    }
    best_idx
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd waved && cargo test -p waved-ui --lib screens::audio_mode::tests::nearest`
Expected: all 6 tests pass.

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-ui/src/screens/audio_mode.rs
git commit -m "$(cat <<'EOF'
feat(ui/audio_mode): add nearest_index_to_midline helper

Pure index picker for finding the carousel-centered card. Used by the
LVGL SCROLL_END callback to track which card the user is previewing.
EOF
)"
```

---

## Task 4: Add `bf_doa.png` arrow child + struct field (creation only)

**Files:**
- Modify: `waved/crates/waved-ui/src/screens/audio_mode.rs` (struct + create loop)

This task adds the overlay images but leaves them hidden — no rotation logic yet. The next task wires SCROLL_END tracking, then Task 6 turns rotation on.

- [ ] **Step 1: Add `doa_arrows` field to the struct**

In `waved/crates/waved-ui/src/screens/audio_mode.rs`, modify the `AudioModeScreen` struct (around line 99):

```rust
pub struct AudioModeScreen {
    bus:        Arc<StateBus>,
    cards:      [*mut lv_obj_t; 5],
    radios:     [*mut lv_obj_t; 5],
    indicators: [*mut lv_obj_t; 5],
    doa_arrows: [*mut lv_obj_t; 5],
    prev_mode:  Option<AudioMode>,
}
```

Update the `new()` constructor (line 108) to initialise the new field:

```rust
impl AudioModeScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            cards:      [std::ptr::null_mut(); 5],
            radios:     [std::ptr::null_mut(); 5],
            indicators: [std::ptr::null_mut(); 5],
            doa_arrows: [std::ptr::null_mut(); 5],
            prev_mode:  None,
        }
    }
    // ...
}
```

Update the reset block at the top of `create()` (around line 165):

```rust
self.prev_mode = None;
self.cards      = [std::ptr::null_mut(); 5];
self.radios     = [std::ptr::null_mut(); 5];
self.indicators = [std::ptr::null_mut(); 5];
self.doa_arrows = [std::ptr::null_mut(); 5];
```

- [ ] **Step 2: Create the arrow image as a child of `mode_img`**

In the per-card build loop (around line 236, inside `for i in 0..5usize`), after the `mode_img` is created and just before the label, add:

```rust
// DOA arrow overlay — child of the polar-pattern bg image. Initially
// hidden; shown/rotated by update_state on the centered card only.
let arrow = lv_image_create(mode_img);
lv_obj_set_size(arrow, 124, 124);
lv_obj_set_pos(arrow, 0, 0);
lv_image_set_src(
    arrow,
    b"A:/etc/pspapp/lvgl/icon/beamforming/bf_doa.png\0".as_ptr() as *const _,
);
lv_image_set_pivot(arrow, 62, 62);
lv_obj_remove_flag(arrow, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
lv_obj_add_flag(arrow, lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
self.doa_arrows[i] = arrow;
```

- [ ] **Step 3: Build to verify it compiles**

Run: `cd waved && cargo check -p waved-ui --features ui`
Expected: clean build.

- [ ] **Step 4: Cross-build for device**

Run: `cd waved && cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved`
Expected: clean build.

- [ ] **Step 5: Deploy and visually verify**

Run:
```bash
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "killall waved 2>/dev/null; sleep 2; (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &)"
```

Open the audio-mode picker on device. Expected: identical to before — no arrow visible (HIDDEN flag is set). If anything changed visually, the arrow placement is wrong; revisit step 2.

- [ ] **Step 6: Commit**

```bash
git add waved/crates/waved-ui/src/screens/audio_mode.rs
git commit -m "$(cat <<'EOF'
feat(ui/audio_mode): add hidden bf_doa.png overlay per card

Creates the DOA arrow image on each carousel card as a child of the
polar-pattern bg image, hidden by default. Subsequent commits wire
the carousel-centered tracking and per-tick rotation.
EOF
)"
```

---

## Task 5: Track centered card via `LV_EVENT_SCROLL_END`

**Files:**
- Modify: `waved/crates/waved-ui/src/screens/audio_mode.rs` (struct, callback, hook)

- [ ] **Step 1: Add `centered_idx` field and imports**

At the top of `waved/crates/waved-ui/src/screens/audio_mode.rs`, after the existing imports, add:

```rust
use std::sync::atomic::{AtomicUsize, Ordering};
```

In the `AudioModeScreen` struct, add `centered_idx: Arc<AtomicUsize>`:

```rust
pub struct AudioModeScreen {
    bus:          Arc<StateBus>,
    cards:        [*mut lv_obj_t; 5],
    radios:       [*mut lv_obj_t; 5],
    indicators:   [*mut lv_obj_t; 5],
    doa_arrows:   [*mut lv_obj_t; 5],
    centered_idx: Arc<AtomicUsize>,
    prev_mode:    Option<AudioMode>,
}
```

Update `new()`:

```rust
impl AudioModeScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self {
            bus,
            cards:        [std::ptr::null_mut(); 5],
            radios:       [std::ptr::null_mut(); 5],
            indicators:   [std::ptr::null_mut(); 5],
            doa_arrows:   [std::ptr::null_mut(); 5],
            centered_idx: Arc::new(AtomicUsize::new(0)),
            prev_mode:    None,
        }
    }
    // ...
}
```

- [ ] **Step 2: Add the SCROLL_END callback function**

Below `done_btn_cb` (around line 95), add:

```rust
/// LVGL `SCROLL_END` handler on the carousel. user_data is a raw
/// `Arc<AtomicUsize>` (via `Arc::into_raw`) sharing the centered-card
/// index with `AudioModeScreen::update_state`.
unsafe extern "C" fn carousel_scroll_end_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) != lv_event_code_t_LV_EVENT_SCROLL_END {
        return;
    }
    let carousel = lv_event_get_target(e) as *mut lv_obj_t;
    let cell_ptr = lv_event_get_user_data(e) as *const AtomicUsize;
    if carousel.is_null() || cell_ptr.is_null() {
        return;
    }

    let count = lv_obj_get_child_count(carousel) as usize;
    if count == 0 {
        return;
    }
    let scroll_x = lv_obj_get_scroll_x(carousel);
    let viewport_mid = scroll_x + lv_obj_get_width(carousel) / 2;

    let mut centres = Vec::with_capacity(count);
    for i in 0..count {
        let child = lv_obj_get_child(carousel, i as i32);
        if child.is_null() { return; }
        let cx = lv_obj_get_x(child) + lv_obj_get_width(child) / 2;
        centres.push(cx);
    }
    let idx = nearest_index_to_midline(&centres, viewport_mid);
    (*cell_ptr).store(idx, Ordering::Relaxed);
}
```

- [ ] **Step 3: Initialise `centered_idx` and hook the callback in `create()`**

In `create()`, after `let initial_idx = mode_to_idx(&current_mode);` (around line 204), add:

```rust
self.centered_idx.store(initial_idx, Ordering::Relaxed);
```

After the carousel's flex/scroll setup but before the card-build loop (right after line 200, the `lv_obj_set_scroll_dir` call), hook the callback:

```rust
let cell_raw = Arc::into_raw(Arc::clone(&self.centered_idx)) as *mut std::ffi::c_void;
lv_obj_add_event_cb(
    carousel,
    Some(carousel_scroll_end_cb),
    lv_event_code_t_LV_EVENT_SCROLL_END,
    cell_raw,
);
```

This matches the existing `Box::into_raw` pattern at lines 254 and 314 — leaks once per `create()`, acceptable since screens persist for the session.

- [ ] **Step 4: Add a tracing log so we can verify on-device**

Inside `carousel_scroll_end_cb`, just before the `store`, add:

```rust
tracing::debug!(target: "waved_ui::audio_mode", "carousel centered idx = {}", idx);
```

- [ ] **Step 5: Build for host to verify it compiles**

Run: `cd waved && cargo check -p waved-ui --features ui`
Expected: clean build.

- [ ] **Step 6: Cross-build and deploy**

Run:
```bash
cd waved && cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "killall waved 2>/dev/null; sleep 2; (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &)"
```

- [ ] **Step 7: Verify SCROLL_END fires on device**

Open the audio-mode picker; scroll left/right between cards. In another terminal:
```bash
adb shell "tail -f /tmp/waved.log | grep audio_mode"
```
Expected: a `carousel centered idx = N` line each time the carousel snaps to a new card. If nothing logs, the event hook isn't wired correctly.

- [ ] **Step 8: Commit**

```bash
git add waved/crates/waved-ui/src/screens/audio_mode.rs
git commit -m "$(cat <<'EOF'
feat(ui/audio_mode): track carousel-centered card via SCROLL_END

Hooks LV_EVENT_SCROLL_END on the carousel and updates an
Arc<AtomicUsize> with the index of whichever card sits closest to
the viewport midline after each snap. The next commit consumes this
to gate the DOA arrow visibility.
EOF
)"
```

---

## Task 6: Wire DOA arrow rotation + visibility in `update_state`

**Files:**
- Modify: `waved/crates/waved-ui/src/screens/audio_mode.rs` (update_state, struct field, create scroll-to-view)

- [ ] **Step 1: Add `prev_doa` cache field + `DoaCacheKey` type**

Below the existing struct definitions in `waved/crates/waved-ui/src/screens/audio_mode.rs`, add:

```rust
#[derive(Clone, PartialEq)]
struct DoaCacheKey {
    centered: usize,
    rotation: i32,
    active:   bool,
}
```

Add `prev_doa: Option<DoaCacheKey>` to the struct:

```rust
pub struct AudioModeScreen {
    bus:          Arc<StateBus>,
    cards:        [*mut lv_obj_t; 5],
    radios:       [*mut lv_obj_t; 5],
    indicators:   [*mut lv_obj_t; 5],
    doa_arrows:   [*mut lv_obj_t; 5],
    centered_idx: Arc<AtomicUsize>,
    prev_mode:    Option<AudioMode>,
    prev_doa:     Option<DoaCacheKey>,
}
```

Initialise in `new()`:

```rust
prev_doa: None,
```

Reset in `create()` alongside `prev_mode`:

```rust
self.prev_mode = None;
self.prev_doa = None;
```

- [ ] **Step 2: Restructure `update_state`**

Replace the entire `update_state` body (currently lines 119-155) with:

```rust
/// Called each tick to update radio icons, indicator strip, border, and DOA arrow.
pub unsafe fn update_state(&mut self, audio: &AudioState) {
    let selected_idx = mode_to_idx(&audio.mode);

    // ── Selection visuals: border, radio icon, indicator strip ─────────────
    if self.prev_mode.as_ref() != Some(&audio.mode) {
        for i in 0..5 {
            if !self.radios[i].is_null() {
                let src = if i == selected_idx {
                    b"A:/etc/pspapp/lvgl/icon/btn/radio_checked.png\0".as_ptr()
                } else {
                    b"A:/etc/pspapp/lvgl/icon/btn/radio_unchecked.png\0".as_ptr()
                };
                lv_image_set_src(self.radios[i], src as *const _);
            }
            if !self.cards[i].is_null() {
                let side = if i == selected_idx {
                    lv_border_side_t_LV_BORDER_SIDE_FULL
                } else {
                    lv_border_side_t_LV_BORDER_SIDE_NONE
                };
                lv_obj_set_style_border_side(self.cards[i], side as u32, 0);
            }
            if !self.indicators[i].is_null() {
                let src = if i == selected_idx {
                    INDICATOR_CHECK[i].as_ptr()
                } else {
                    INDICATOR_UNCHECK[i].as_ptr()
                };
                lv_image_set_src(self.indicators[i], src as *const _);
            }
        }

        // Slide carousel to the newly-selected card so centered tracks
        // selection on every tap. Skipped on the first tick (prev_mode None
        // means we just entered the screen and create() already aligned).
        if self.prev_mode.is_some() && !self.cards[selected_idx].is_null() {
            lv_obj_scroll_to_view(
                self.cards[selected_idx],
                lv_anim_enable_t_LV_ANIM_ON,
            );
        }

        self.prev_mode = Some(audio.mode.clone());
    }

    // ── DOA arrow on the carousel-centered card ────────────────────────────
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
}
```

- [ ] **Step 3: Build to verify it compiles**

Run: `cd waved && cargo check -p waved-ui --features ui`
Expected: clean build.

- [ ] **Step 4: Cross-build for device**

Run: `cd waved && cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved`
Expected: clean build.

- [ ] **Step 5: Deploy**

```bash
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "killall waved 2>/dev/null; sleep 2; (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &)"
```

- [ ] **Step 6: Commit**

```bash
git add waved/crates/waved-ui/src/screens/audio_mode.rs
git commit -m "$(cat <<'EOF'
feat(ui/audio_mode): rotate bf_doa.png arrow on centered card

Per UI tick, rotate the DOA arrow on the carousel-centered card to
match AudioState.doa_horizontal and gate visibility on doa_active.
On selection commit, also scroll-to-view so the centered card stays
in sync with the user's selection.

Closes the original-firmware "preview where audio is coming from"
behavior on the audio-mode picker.
EOF
)"
```

---

## Task 7: On-device acceptance

**Files:** none (manual verification)

- [ ] **Step 1: Forward-arrival test**

Speak directly in front of the device while the audio-mode picker is open.
Expected: arrow appears on the centered card and points "up" / forward.

- [ ] **Step 2: Cardinal-arrival sweep**

Move source to left side of device.
Expected: arrow rotates ~90° counter-clockwise (left).

Move source to right side.
Expected: arrow rotates ~90° clockwise (right).

Move behind device.
Expected: arrow points down / 180°.

- [ ] **Step 3: VAD-off → arrow disappears**

Stay silent for >1 second.
Expected: arrow disappears (`doa_active = false`).

Speak again briefly.
Expected: arrow reappears at the new direction.

- [ ] **Step 4: Scroll-without-tap → arrow follows centered card, selection unchanged**

Scroll the carousel to a non-selected card (without tapping).
Expected:
- Previous card's arrow hides.
- New centered card's arrow appears once VAD trips.
- Radio icon and border still highlight the originally-selected card (selection unchanged).

- [ ] **Step 5: Tap a non-centered card → carousel slides + arrow follows**

Without scrolling, tap a card that is not centered.
Expected:
- Carousel animates a slide so the tapped card becomes centered.
- Border / radio / indicator update to the new selection.
- Arrow ends up on the now-centered card (the tapped one).

- [ ] **Step 6: Non-centered cards never show the arrow**

While speaking, scroll back and forth and confirm the arrow only ever shows on a single card — the one that snapped to centre. No flicker or ghost arrows on other cards.

- [ ] **Step 7: Document the result**

Append a short note in `waved/docs/AUDIO_HARDWARE.md` under a new "## Audio Mode UI" section, summarising on-device behavior:

```markdown
## Audio Mode UI — DOA preview

The audio-mode picker (`screens/audio_mode.rs`) shows a `bf_doa.png`
arrow over the polar-pattern image on the carousel-centered card,
gated by `AudioState.doa_active`. Rotation tracks
`AudioState.doa_horizontal` quantised to whole degrees. On selection
commit, the carousel scrolls to the newly-selected card via
`lv_obj_scroll_to_view`, keeping centered ≡ selected after every tap
while still allowing scroll-preview without committing.
```

- [ ] **Step 8: Commit the doc note**

```bash
git add waved/docs/AUDIO_HARDWARE.md
git commit -m "docs(audio): document audio-mode DOA preview behavior"
```

---

## Self-Review

**Spec coverage:**
- §1 State plumbing (`doa_active` field) → Task 1.
- §2a struct field additions → Tasks 4, 5, 6.
- §2b arrow creation → Task 4.
- §2c `az_to_lvgl_rotation` + tests → Task 2.
- §2d `closest_centered_child` / `nearest_index_to_midline` + scroll callback → Tasks 3, 5.
- §2e per-tick update with cache → Task 6.
- §2e selection-commit scroll-to-view → Task 6 step 2.
- §2f lifecycle (zero arrays in `create()`) → Tasks 4, 5, 6.
- §3 unit tests → Tasks 2, 3.
- §3 on-device acceptance → Task 7.

**Placeholder scan:** No TBDs, TODOs, or "similar to" handwaves. All code blocks contain the actual code.

**Type consistency:** `az_to_lvgl_rotation`, `nearest_index_to_midline`, `DoaCacheKey`, `centered_idx`, `prev_doa`, `doa_arrows` — names and signatures match across tasks.
