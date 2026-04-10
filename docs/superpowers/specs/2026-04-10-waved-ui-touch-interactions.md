# Waved UI Touch Interactions & Navigation

## Summary

Wire up all touch interactions, screen navigation, and gesture handling for the waved UI on the Insta360 Wave. Consolidate settings into a scrollable swipe-down Control Center overlay. Two main screens (Home, Recording) plus the Control Center overlay.

## Screens

### Home Screen (existing, needs wiring)

- **Rec button**: navigate to Recording screen + send `Command::StartRecording`
- **Mic button**: send `Command::ToggleMicMute` (stays on Home, UI updates via state)
- **Vol button**: open Control Center (currently not clickable — add `LV_OBJ_FLAG_CLICKABLE` + event callback)
- **Swipe down**: open Control Center overlay
- Bottom nav labels (Mode, Webcam, Files, Settings): remove — no longer needed since everything is in Control Center

### Recording Screen (existing, needs wiring)

- **Mark button**: send `Command::AddBookmark` + show toast "Bookmark added"
- **Stop button**: send `Command::StopRecording` + navigate back to Home
- **Mute button**: send `Command::ToggleMicMute`
- **Swipe down**: open Control Center overlay
- Timer, VU meter, mode badge: already update from state — no changes needed

### Control Center (existing overlay, needs full wiring)

Scrollable panel on `lv_layer_top()`. Slides down from y=-400 to y=32 (200ms animation, already implemented). Sections top to bottom:

1. **Audio Mode** — 5 buttons in a row (Omni, Stereo, Cardioid, Supercardioid, Figure-8). Active mode highlighted green. Click sends `Command::SetAudioMode(mode)`.
2. **Noise Reduction** — 4 buttons in a row (Off, Low, Med, High). Active level highlighted. Click sends `Command::SetNrLevel(level)`.
3. **Volume** — Speaker slider + Mic slider. Drag sends `Command::SetSpeakerVolume(val)` / `Command::SetMicVolume(val)`.
4. **WiFi** — Status label showing SSID + connected/disconnected. Read-only for now.
5. **Bluetooth** — Status label showing connected/disconnected. Read-only for now.
6. **Mic Arm** — Two buttons (Extend, Retract). Click sends `Command::SetMotorPosition(Extend/Retract)`.
7. **About** — Firmware version + serial number labels. Read-only.
8. **Power Off** — Single red button. Long-press (3s) sends `Command::Shutdown`.

Bottom hint label: "swipe up to close". Total scroll height ~600px.

**Swipe up** on Control Center → close (slide up animation).

## Navigation State Machine

```
Home ──(Rec btn)──> Recording
Home ──(Vol btn)──> [Control Center overlay]
Home ──(swipe down)──> [Control Center overlay]

Recording ──(Stop btn)──> Home
Recording ──(swipe down)──> [Control Center overlay]

[Control Center] ──(swipe up)──> close overlay (return to underlying screen)
```

Only two real screens (Home, Recording). Control Center is an overlay that can appear on top of either.

## Gesture Detection

Use LVGL's built-in gesture support:
- Register `LV_EVENT_GESTURE` on Home and Recording screen root objects
- Check `lv_indev_get_gesture_dir()` for `LV_DIR_BOTTOM` → open Control Center
- Register `LV_EVENT_GESTURE` on Control Center panel for `LV_DIR_TOP` → close
- LVGL handles scroll within the Control Center natively (content taller than viewport)

## Command → UI State Flow

Commands sent from UI buttons reach backend services via the broadcast channel. Services update state on the state bus. The UI loop subscribes to state changes and updates widgets:

- `audio.mode` changes → update Control Center audio mode highlight + Recording mode badge
- `audio.nr_level` changes → update Control Center NR highlight
- `audio.mic_muted` changes → update Home mic icon/label + Recording mute button + status bar
- `audio.speaker_volume` / `audio.mic_volume` → update Control Center sliders
- `meeting.recording` changes → trigger navigation (Home ↔ Recording)
- `wifi` / `bt` state → update Control Center labels + status bar icons
- `mcu.motor_position` → update Control Center arm button highlights

## handle_command() Updates

Expand the stub in `lib.rs` to handle UI-side effects:

```
StartRecording → navigate to Recording screen
StopRecording → navigate to Home screen  
AddBookmark → show toast "Bookmark added"
Shutdown → show toast "Shutting down..."
```

Other commands don't need UI-side handling — state updates flow through the state bus.

## Files to Modify

- `lib.rs` — expand `handle_command()`, add gesture handlers, instantiate Control Center, add Recording screen lifecycle, extend state update loop
- `screens/home.rs` — add Vol button click handler, add gesture handler, remove bottom nav labels
- `screens/recording.rs` — add Mark/Stop/Mute button event callbacks, add gesture handler
- `screens/control_center.rs` — add all button/slider event callbacks, add state update method, add WiFi/BT/About/PowerOff sections (currently only has NR + Motor)
- `nav.rs` — no changes needed (navigate_to + UiScreen enum already sufficient)

## Files to Remove

- `screens/settings.rs` — replaced by Control Center
- `screens/audio_mode.rs` — absorbed into Control Center

## Error Handling

- Disabled controls shown dimmed (e.g., WiFi section when no WiFi module)
- Power Off requires long-press (3s) to prevent accidental shutdown
- Toast confirmations for destructive actions ("Motor extending...", "Recording stopped")
