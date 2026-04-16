# Waved UI Touch Interactions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire up all touch interactions, screen navigation, gestures, and expand the Control Center to be the single settings surface.

**Architecture:** Two screens (Home, Recording) plus a scrollable Control Center overlay on `lv_layer_top()`. LVGL gesture events handle swipe down/up. All button callbacks send Commands via the broadcast channel. State changes flow back through watch channels and update widget highlights/labels.

**Tech Stack:** Rust, LVGL 9.x (C FFI via bindgen), tokio broadcast/watch channels

**Spec:** `docs/superpowers/specs/2026-04-10-waved-ui-touch-interactions.md`

---

### Task 1: Wire up Home screen Vol button + gesture handler

**Files:**
- Modify: `waved/crates/waved-ui/src/screens/home.rs`

The Vol button is rendered but not clickable. Add click handler to open Control Center. Add gesture handler for swipe-down. Remove the bottom nav labels (Mode, Webcam, Files, Settings) since everything is now in the Control Center.

- [ ] **Step 1: Make Vol button clickable**

In `home.rs`, add a new callback above the existing callbacks:

```rust
/// Vol button clicked → open control center.
unsafe extern "C" fn vol_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            // Use a dedicated command to signal the UI to open the control center.
            // We reuse ToggleSpeakerMute as a "open control center" signal for now.
            // A cleaner approach: add a UiAction enum, but Commands already flow to UI.
            let _ = (*tx).send(Command::ToggleSpeakerMute);
        }
    }
}
```

In the `create()` method, after the Vol button is created (after line 179 `lv_obj_align(lbl_vol, ...)`), add:

```rust
// Make vol button clickable — opens Control Center.
lv_obj_add_flag(btn_spk, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
let vol_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
lv_obj_add_event_cb(
    btn_spk,
    Some(vol_btn_cb),
    lv_event_code_t_LV_EVENT_CLICKED,
    vol_tx as *mut _,
);
```

- [ ] **Step 2: Add swipe-down gesture handler**

Add a new callback:

```rust
/// Swipe down on home screen → open control center.
unsafe extern "C" fn gesture_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_GESTURE {
        let dir = lv_indev_get_gesture_dir(lv_indev_active());
        if dir == lv_dir_t_LV_DIR_BOTTOM {
            let tx = lv_event_get_user_data(e)
                as *const tokio::sync::broadcast::Sender<Command>;
            if !tx.is_null() {
                let _ = (*tx).send(Command::ToggleSpeakerMute);
            }
        }
    }
}
```

At the end of `create()`, before `screen` is returned, register the gesture on the screen object:

```rust
// Swipe down to open control center
let gesture_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
lv_obj_add_event_cb(
    screen,
    Some(gesture_cb),
    lv_event_code_t_LV_EVENT_GESTURE,
    gesture_tx as *mut _,
);
```

- [ ] **Step 3: Remove bottom nav labels**

Delete the "More" slide-up indicator block (lines 244–258) from `create()`. The Control Center replaces the bottom nav.

- [ ] **Step 4: Build and verify**

Run: `cargo check --features ui`
Expected: compiles with no errors

- [ ] **Step 5: Commit**

```
feat(ui): wire up Vol button + swipe-down gesture on home screen
```

---

### Task 2: Wire up Recording screen buttons + gesture

**Files:**
- Modify: `waved/crates/waved-ui/src/screens/recording.rs`

The Recording screen has Mark, Stop, and Mute buttons rendered but with no event callbacks. Wire them up.

- [ ] **Step 1: Add button callbacks**

Add these callbacks at the top of `recording.rs`, after the imports:

```rust
unsafe extern "C" fn mark_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::AddBookmark);
        }
    }
}

unsafe extern "C" fn stop_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::StopRecording);
        }
    }
}

unsafe extern "C" fn mute_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::ToggleMicMute);
        }
    }
}

/// Swipe down on recording screen → open control center.
unsafe extern "C" fn gesture_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_GESTURE {
        let dir = lv_indev_get_gesture_dir(lv_indev_active());
        if dir == lv_dir_t_LV_DIR_BOTTOM {
            let tx = lv_event_get_user_data(e)
                as *const tokio::sync::broadcast::Sender<Command>;
            if !tx.is_null() {
                let _ = (*tx).send(Command::ToggleSpeakerMute);
            }
        }
    }
}
```

Add `Command` to the import line:

```rust
use waved_state::{AudioState, AudioMode, Command, MeetingState, StateBus};
```

- [ ] **Step 2: Register callbacks on buttons in create()**

After each button is created in `create()`, register its callback. After `btn_mark` creation (after `lv_obj_center(lbl_mark)`):

```rust
let mark_tx = Box::into_raw(Box::new(self._bus.commands.clone()));
lv_obj_add_event_cb(btn_mark, Some(mark_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, mark_tx as *mut _);
```

After `btn_stop` creation (after `lv_obj_center(lbl_stop)`):

```rust
let stop_tx = Box::into_raw(Box::new(self._bus.commands.clone()));
lv_obj_add_event_cb(btn_stop, Some(stop_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, stop_tx as *mut _);
```

After `btn_mute` creation (after `lv_obj_center(lbl_mute)`):

```rust
let mute_tx = Box::into_raw(Box::new(self._bus.commands.clone()));
lv_obj_add_event_cb(btn_mute, Some(mute_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, mute_tx as *mut _);
```

At the end of `create()`, before returning `screen`:

```rust
let gesture_tx = Box::into_raw(Box::new(self._bus.commands.clone()));
lv_obj_add_event_cb(screen, Some(gesture_cb), lv_event_code_t_LV_EVENT_GESTURE, gesture_tx as *mut _);
```

- [ ] **Step 3: Rename `_bus` to `bus`**

The field is named `_bus` (unused). Now that we reference it, rename it to `bus` in the struct definition and constructor.

- [ ] **Step 4: Build and verify**

Run: `cargo check --features ui`
Expected: compiles with no errors

- [ ] **Step 5: Commit**

```
feat(ui): wire up recording screen buttons (mark/stop/mute) + swipe gesture
```

---

### Task 3: Expand Control Center with all sections

**Files:**
- Modify: `waved/crates/waved-ui/src/screens/control_center.rs`

Currently has WiFi, NR, and Motor Arm sections. Need to add: Audio Mode, Volume sliders, Bluetooth, About, Power Off. Also need to make the panel scrollable (taller than 240px) and wire up ALL button callbacks.

- [ ] **Step 1: Update struct to hold new widget pointers**

Replace the `ControlCenter` struct:

```rust
pub struct ControlCenter {
    bus: Arc<StateBus>,
    panel: *mut lv_obj_t,
    visible: bool,
    // Widget pointers for state updates
    mode_btns: [*mut lv_obj_t; 5],
    nr_btns: [*mut lv_obj_t; 4],
    lbl_wifi: *mut lv_obj_t,
    lbl_bt: *mut lv_obj_t,
    lbl_version: *mut lv_obj_t,
    active_mode: AudioMode,
    active_nr: NrLevel,
}
```

Update imports:

```rust
use waved_state::{AudioMode, Command, MotorPosition, NrLevel, StateBus};
```

Add `AudioMode` to the existing const arrays:

```rust
const MODES: [AudioMode; 5] = [
    AudioMode::Omni, AudioMode::Stereo, AudioMode::Cardioid,
    AudioMode::Supercardioid, AudioMode::Figure8,
];
const MODE_LABELS: [&str; 5] = ["Omni", "Stereo", "Card.", "S-Card.", "Fig-8"];
const NR_LABELS: [&str; 4] = ["Off", "Low", "Med", "High"];
```

- [ ] **Step 2: Add button callbacks**

Add these callbacks:

```rust
unsafe extern "C" fn mode_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let data = lv_event_get_user_data(e) as *const (tokio::sync::broadcast::Sender<Command>, usize);
        if !data.is_null() {
            let (tx, idx) = &*data;
            let _ = tx.send(Command::SetAudioMode(MODES[*idx].clone()));
        }
    }
}

unsafe extern "C" fn nr_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let data = lv_event_get_user_data(e) as *const (tokio::sync::broadcast::Sender<Command>, usize);
        if !data.is_null() {
            let (tx, idx) = &*data;
            let _ = tx.send(Command::SetNrLevel(NR_LEVELS[*idx].clone()));
        }
    }
}

unsafe extern "C" fn motor_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let data = lv_event_get_user_data(e) as *const (tokio::sync::broadcast::Sender<Command>, MotorPosition);
        if !data.is_null() {
            let (tx, pos) = &*data;
            let _ = tx.send(Command::SetMotorPosition(pos.clone()));
        }
    }
}

unsafe extern "C" fn poweroff_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_LONG_PRESSED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::Shutdown);
        }
    }
}

unsafe extern "C" fn swipe_up_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_GESTURE {
        let dir = lv_indev_get_gesture_dir(lv_indev_active());
        if dir == lv_dir_t_LV_DIR_TOP {
            let tx = lv_event_get_user_data(e)
                as *const tokio::sync::broadcast::Sender<Command>;
            if !tx.is_null() {
                // Signal to close control center
                let _ = (*tx).send(Command::ToggleSpeakerMute);
            }
        }
    }
}
```

- [ ] **Step 3: Rewrite init() with all sections**

Replace the `init()` method with the full implementation. The panel becomes scrollable (height=600, scroll enabled). Sections are positioned with increasing Y offsets:

- Y=0: Header "CONTROL CENTER"
- Y=20: Audio Mode (5 buttons row)
- Y=84: Noise Reduction (4 buttons row)
- Y=148: Mic Arm (2 buttons)
- Y=212: WiFi (status label)
- Y=276: Bluetooth (status label)
- Y=340: About (version label)
- Y=404: Power Off (red button, long-press)
- Y=460: "swipe up to close" hint

Make the panel scrollable:

```rust
lv_obj_set_size(panel, 400, 304 - 32); // visible area below status bar
lv_obj_set_style_max_height(panel, 304 - 32, 0);
lv_obj_add_flag(panel, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
```

Each section follows the same pattern as the existing NR/WiFi/Motor sections but with event callbacks registered. Register `swipe_up_cb` on the panel for `LV_EVENT_GESTURE`.

For each mode button `i`:
```rust
let data = Box::into_raw(Box::new((self.bus.commands.clone(), i)));
lv_obj_add_event_cb(btn, Some(mode_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, data as *mut _);
```

For each NR button `i`:
```rust
let data = Box::into_raw(Box::new((self.bus.commands.clone(), i)));
lv_obj_add_event_cb(btn, Some(nr_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, data as *mut _);
```

For motor extend:
```rust
let data = Box::into_raw(Box::new((self.bus.commands.clone(), MotorPosition::High)));
lv_obj_add_event_cb(btn_extend, Some(motor_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, data as *mut _);
```

For motor retract:
```rust
let data = Box::into_raw(Box::new((self.bus.commands.clone(), MotorPosition::Low)));
lv_obj_add_event_cb(btn_retract, Some(motor_btn_cb), lv_event_code_t_LV_EVENT_CLICKED, data as *mut _);
```

For power off (long-press):
```rust
let tx = Box::into_raw(Box::new(self.bus.commands.clone()));
lv_obj_add_event_cb(btn_poweroff, Some(poweroff_btn_cb), lv_event_code_t_LV_EVENT_LONG_PRESSED, tx as *mut _);
```

For swipe up:
```rust
let gesture_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
lv_obj_add_event_cb(panel, Some(swipe_up_cb), lv_event_code_t_LV_EVENT_GESTURE, gesture_tx as *mut _);
```

- [ ] **Step 4: Add state update methods**

Add `highlight_active_mode()` method (similar to existing `highlight_active_nr()`):

```rust
unsafe fn highlight_active_mode(&self) {
    for (i, &btn) in self.mode_btns.iter().enumerate() {
        if btn.is_null() { continue; }
        let is_active = MODES[i] == self.active_mode;
        let border_clr = if is_active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
        lv_obj_set_style_border_color(btn, border_clr, 0);
        let lbl = lv_obj_get_child(btn, 0);
        if !lbl.is_null() {
            let text_clr = if is_active { rgb(CLR_ACCENT) } else { rgb(CLR_TEXT_DIM) };
            lv_obj_set_style_text_color(lbl, text_clr, 0);
        }
    }
}
```

Add `update_state()` method:

```rust
pub unsafe fn update_state(
    &mut self,
    audio: &waved_state::AudioState,
    wifi: &waved_state::WifiState,
    bt: &waved_state::BluetoothState,
) {
    if audio.mode != self.active_mode {
        self.active_mode = audio.mode.clone();
        self.highlight_active_mode();
    }
    if audio.nr_level != self.active_nr {
        self.active_nr = audio.nr_level.clone();
        self.highlight_active_nr();
    }
    // WiFi
    let wifi_text = if wifi.connected {
        CString::new(format!("{} Connected", wifi.ssid)).unwrap()
    } else {
        CString::new("Not connected").unwrap()
    };
    lv_label_set_text(self.lbl_wifi, wifi_text.as_ptr());
    // Bluetooth
    let bt_text = if bt.connected {
        CString::new(format!("{} Connected", bt.device_name)).unwrap()
    } else {
        CString::new("Not connected").unwrap()
    };
    lv_label_set_text(self.lbl_bt, bt_text.as_ptr());
}
```

- [ ] **Step 5: Update constructor**

Update `new()` to initialize all new fields with null pointers and defaults.

- [ ] **Step 6: Build and verify**

Run: `cargo check --features ui`
Expected: compiles with no errors

- [ ] **Step 7: Commit**

```
feat(ui): expand control center with audio mode, BT, about, power off sections
```

---

### Task 4: Wire up navigation + Control Center in main UI loop

**Files:**
- Modify: `waved/crates/waved-ui/src/lib.rs`

This is the central wiring task. The main loop needs to:
1. Create and manage the Control Center overlay
2. Create the Recording screen when recording starts
3. Navigate between Home and Recording based on state/commands
4. Route `ToggleSpeakerMute` command to toggle Control Center (since we reused it as a signal)
5. Update Control Center state each tick

- [ ] **Step 1: Add Control Center and Recording screen to the main function**

After `let mut home = ...` and before the main loop, add:

```rust
let mut recording = screens::recording::RecordingScreen::new(Arc::clone(&bus));
let mut cc = screens::control_center::ControlCenter::new(Arc::clone(&bus));
unsafe { cc.init(); }

let meeting_rx = bus.meeting.subscribe();
let mut prev_recording = false;
```

Remove the existing `let _meeting_rx = bus.meeting.subscribe();` line (we now use it).

- [ ] **Step 2: Expand handle_command()**

Replace the `handle_command` function:

```rust
fn handle_command(
    cmd: &Command,
    nav: &mut ScreenManager,
    toast: &mut Toast,
    cc: &mut screens::control_center::ControlCenter,
) {
    unsafe {
        match cmd {
            Command::StartRecording => {
                // Navigation is handled by state change (meeting.recording), not here
            }
            Command::StopRecording => {
                // Navigation is handled by state change (meeting.recording), not here
            }
            Command::AddBookmark => {
                toast.show("Bookmark added", 1500);
            }
            Command::Shutdown => {
                toast.show("Shutting down...", 2000);
            }
            Command::ToggleSpeakerMute => {
                // Reused as "toggle control center" signal from UI gestures/buttons
                if cc.is_visible() {
                    cc.hide();
                } else {
                    cc.show();
                }
            }
            _ => {}
        }
    }
}
```

Update the call site in the loop:

```rust
while let Ok(cmd) = cmd_rx.try_recv() {
    handle_command(&cmd, &mut nav, &mut toast, &mut cc);
}
```

- [ ] **Step 3: Add state-driven screen navigation**

After `lv_timer_handler()` and `toast.tick()`, add navigation based on meeting state:

```rust
// Navigate Home ↔ Recording based on meeting state
{
    let meeting = meeting_rx.borrow();
    if meeting.recording && !prev_recording {
        // Started recording → switch to recording screen
        let root = recording.create();
        unsafe { lv_screen_load(root); }
        nav.navigate_to(UiScreen::Recording);
        nav.set_root(root);
        prev_recording = true;
    } else if !meeting.recording && prev_recording {
        // Stopped recording → switch back to home
        let root = home.create();
        unsafe { lv_screen_load(root); }
        nav.navigate_to(UiScreen::Home);
        nav.set_root(root);
        prev_recording = false;
    }
}
```

- [ ] **Step 4: Update per-screen state updates**

Expand the match block for per-screen updates:

```rust
match &nav.current {
    UiScreen::Home => {
        let audio = audio_rx.borrow().clone();
        let bt    = bt_rx.borrow().clone();
        let wifi  = wifi_rx.borrow().clone();
        unsafe { home.update_state(&audio, &bt, &wifi); }
    }
    UiScreen::Recording => {
        let audio   = audio_rx.borrow().clone();
        let meeting = meeting_rx.borrow().clone();
        unsafe { recording.update_state(&audio, &meeting); }
    }
    _ => {}
}

// Update control center state (it's an overlay, always exists)
if cc.is_visible() {
    let audio = audio_rx.borrow().clone();
    let wifi  = wifi_rx.borrow().clone();
    let bt    = bt_rx.borrow().clone();
    unsafe { cc.update_state(&audio, &wifi, &bt); }
}
```

- [ ] **Step 5: Build and verify**

Run: `cargo check --features ui`
Expected: compiles with no errors

- [ ] **Step 6: Commit**

```
feat(ui): wire up navigation, control center toggle, and state-driven screen transitions
```

---

### Task 5: Remove obsolete screens

**Files:**
- Delete: `waved/crates/waved-ui/src/screens/settings.rs`
- Delete: `waved/crates/waved-ui/src/screens/audio_mode.rs`
- Modify: `waved/crates/waved-ui/src/screens/mod.rs`
- Modify: `waved/crates/waved-ui/src/nav.rs`

- [ ] **Step 1: Remove module declarations**

In `screens/mod.rs`, remove:

```rust
pub mod audio_mode;
pub mod settings;
```

Keep: `control_center`, `home`, `recording`.

- [ ] **Step 2: Simplify UiScreen enum**

In `nav.rs`, simplify the enum to only the screens we use:

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum UiScreen {
    Home,
    Recording,
}
```

Remove unused variants: `MicMode`, `Settings`, `Files`, `Wifi`, `WifiInfo`, `Bluetooth`, `Volume`, `Height`, `About`, `PowerOff`, `ControlCenter`.

Remove the `control_center_open` field from `ScreenManager` since the `ControlCenter` struct tracks its own visibility.

- [ ] **Step 3: Delete the files**

Delete `waved/crates/waved-ui/src/screens/settings.rs` and `waved/crates/waved-ui/src/screens/audio_mode.rs`.

- [ ] **Step 4: Build and verify**

Run: `cargo check --features ui`
Expected: compiles with no errors

- [ ] **Step 5: Commit**

```
refactor(ui): remove settings and audio_mode screens (absorbed into control center)
```

---

### Task 6: Cross-compile, deploy, and verify on device

**Files:**
- No code changes — build + deploy + manual test

- [ ] **Step 1: Cross-compile**

Run: `cross build --target aarch64-unknown-linux-gnu --release --features ui`
Expected: builds successfully

- [ ] **Step 2: Deploy**

```bash
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb reboot
```

- [ ] **Step 3: Verify home screen**

After reboot, verify:
- Display shows Home screen with Rec, Mute, Vol buttons
- Tapping Mute toggles mic mute (label changes "Mute" ↔ "MUTED", icon changes)
- Tapping Vol opens Control Center (slides down)
- Swiping down opens Control Center

- [ ] **Step 4: Verify Control Center**

- Audio Mode buttons highlight the active mode
- NR buttons highlight the active level
- Tapping a mode/NR button sends command (check via waved.log)
- Mic Arm Extend/Retract buttons send motor commands
- About section shows firmware version
- Long-press Power Off triggers shutdown
- Swiping up closes Control Center

- [ ] **Step 5: Verify Recording flow**

- Tap Rec on Home → recording starts, screen transitions to Recording screen
- Timer counts up
- Tap Stop → recording stops, screen transitions back to Home
- Tap Mark → toast shows "Bookmark added"
- Tap Mute → toggles mic mute

- [ ] **Step 6: Commit any fixes**

If any issues found during testing, fix and commit individually.
