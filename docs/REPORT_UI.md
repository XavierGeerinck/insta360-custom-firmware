# Insta360 Wave UI Screen Reference

**Device:** Insta360 Wave Conference Speakerphone
**Display:** 400 x 304 pixels, 1.82" capacitive touchscreen
**Framework:** LVGL (Light and Versatile Graphics Library)
**Total screens:** 91 pages
**Font:** MiSans (licensed from Xiaomi)
**Languages:** 20 (CN, EN, DE, FR, ES, NL, IT, PL, PT, RU, JP, KR, TC, AR, HI, ID, MY, TH, VN, TK)

---

## Screen Layout Convention

All screens share a common layout structure:

```
┌──────────────────────────────────────┐
│ [Status Bar - 400x32px]              │
│  🔋 📶 🔊 🎤  BT              12:30  │
├──────────────────────────────────────┤
│                                      │
│                                      │
│         [Content Area]               │
│          400 x 240px                 │
│                                      │
│                                      │
├──────────────────────────────────────┤
│ [Bottom Bar / Navigation - 400x32px] │
└──────────────────────────────────────┘
```

Status bar icons (from `/etc/pspapp/lvgl/icon/status/`):
- Battery: 11 levels (0-5%, 6-10%, 11-20%, ... 91-100%) + charging variants
- WiFi signal: 4 levels + off + error
- Mic: on/off/muted
- Speaker: on/off/muted
- Bluetooth: connected/disconnected
- USB: connected

---

## Navigation Flow

```
                    ┌─────────┐
                    │ Welcome │ (ID 9)
                    │  Boot   │
                    └────┬────┘
                         │
                    ┌────┴────┐
                    │ Privacy │ (ID 61-64)
                    │ Policy  │
                    └────┬────┘
                         │
                    ┌────┴────┐
                    │  Guide  │ (ID 67)
                    │  Setup  │
                    └────┬────┘
                         │
               ┌─────────┴─────────┐
               ▼                   ▼
         ┌──────────┐        ┌──────────┐
         │   MAIN   │◄──────►│ INCOMING │ (ID 73)
         │  (ID 10) │        │   CALL   │
         └──┬───┬───┘        └──────────┘
            │   │
    ┌───────┘   └───────┐
    ▼                   ▼
┌────────┐         ┌────────┐
│AI Rec  │         │Settings│ (ID 11)
│(ID 12) │         │  Menu  │
└───┬────┘         └───┬────┘
    │                   │
    ▼              ┌────┴────────────────────┐
┌────────┐         ▼         ▼         ▼     ▼
│Rec File│    ┌────────┐┌────────┐┌───────┐┌──────┐
│(ID 20) │    │System  ││Audio   ││BT     ││WiFi  │
└────────┘    │(ID 14) ││Mode 77 ││(ID 15)││(ID44)│
              └────────┘└────────┘└───────┘└──────┘
```

---

## 1. System Screens

### PAGE_WELCOME (ID 9)

```
┌──────────────────────────────────────┐
│                                      │
│                                      │
│           ┌──────────┐               │
│           │  INSTA   │               │
│           │   360    │               │
│           │  [logo]  │               │
│           └──────────┘               │
│                                      │
│            Welcome                   │
│                                      │
│                                      │
└──────────────────────────────────────┘
```

Shown on first boot or after factory reset. Animates the Insta360 logo, then transitions to privacy policy.

---

### MAIN (ID 10) — Home Screen

```
┌──────────────────────────────────────┐
│ 🔋85%  📶WiFi  🔊  🎤        12:30  │
├──────────────────────────────────────┤
│                                      │
│         ┌──────────────┐             │
│         │              │             │
│         │  ◉ Insta360  │             │
│         │    Wave      │             │
│         │              │             │
│         │  Connected   │             │
│         └──────────────┘             │
│                                      │
│   [🎤 Mic]    [🔊 Vol]   [● Rec]    │
│                                      │
├──────────────────────────────────────┤
│ [Audio Mode] [Webcam] [Files] [⚙Set]│
└──────────────────────────────────────┘
```

**Controls:**
- Center circle: Connection status indicator (USB/BT/Dongle/Disconnected)
- Mic button: Toggle mic mute (icon changes to red when muted)
- Volume button: Opens volume slider popup
- Record button: Starts AI recording (navigates to ID 12)
- Bottom bar: Audio Mode (→77), Webcam (→41), Files (→19), Settings (→11)

**Database keys read:** `bt_connect_status`, `wifi_connect_status`, `mcu_battery_level`, `audio_cur_mic_mute`, `audio_cur_spk_mute`

**Toasts on connection:**
- "USB-C cable connected. Entering Speakerphone mode."
- "Bluetooth connected. Speakerphone mode on."
- "Dongle connected. Speakerphone mode on."

---

### PAGE_LOADING (ID 8)

```
┌──────────────────────────────────────┐
│                                      │
│                                      │
│           ┌────────┐                 │
│           │  ◌◌◌◌  │                │
│           │ Loading │                │
│           │  ◌◌◌◌  │                │
│           └────────┘                 │
│                                      │
│       <context-specific text>        │
│                                      │
└──────────────────────────────────────┘
```

Generic loading spinner. Text varies:
- "Exporting logs..."
- "Cleaning..."
- "Updating..."
- "Saving recording..."
- "Ending recording..."
- "Resetting to factory settings..."

---

## 2. AI Recording Screens

### ai_record (ID 12) — Recording Screen

```
┌──────────────────────────────────────┐
│ 🔋  📶  🔊  🎤               12:30  │
├──────────────────────────────────────┤
│                                      │
│          Recording...                │
│          ⏱ 00:15:32                  │
│                                      │
│  ┌──────────────────────────────┐    │
│  │  Live Captions:              │    │
│  │  "So the next item on the    │    │
│  │   agenda is the Q2 budget"   │    │
│  └──────────────────────────────┘    │
│                                      │
│  [🔖 Mark]  [⏸ Pause]  [⏹ Stop]    │
│                                      │
├──────────────────────────────────────┤
│ [QR Code: Scan to view notes]        │
└──────────────────────────────────────┘
```

**Controls:**
- Mark button: Creates timestamp bookmark
- Pause/Resume: Toggle recording pause
- Stop: Shows confirmation dialog (ID 0)
- QR code: Displays meeting QR for sharing
- Live captions: Real-time ASR subtitles (if enabled)

**States:**
- "Recording..." (active, green indicator)
- "Recording Paused" (paused, yellow indicator)
- "Low volume" (when mic input is too quiet)
- "AI model loading..." (on first use)

**Database keys:** `meeting_server_record_state`, `meeting_server_record_duration`, `audio_asr_txt_update`

---

### PAGE_AIRECORD_FOLD (ID 19) — Recording File List

```
┌──────────────────────────────────────┐
│ [←]  Files                    [Edit] │
├──────────────────────────────────────┤
│ [All] [Audio Trans] [Trans Only]     │
│ [Favorites] [Recycle Bin]            │
├──────────────────────────────────────┤
│ ┌──────────────────────────────────┐ │
│ │ 📄 Meeting 2025-11-28 14:30     │ │
│ │    Duration: 45:12  Size: 28MB  │ │
│ │    ★ Bookmarked                 │ │
│ ├──────────────────────────────────┤ │
│ │ 📄 Meeting 2025-11-28 10:00     │ │
│ │    Duration: 1:02:33  Size: 41MB│ │
│ ├──────────────────────────────────┤ │
│ │ 📄 Meeting 2025-11-27 16:00     │ │
│ │    Duration: 32:05  Size: 19MB  │ │
│ └──────────────────────────────────┘ │
│                                      │
│ No file (if empty)                   │
└──────────────────────────────────────┘
```

**Tabs:** All, Audio Transcription, Transcription Only, Favorites, Recycle Bin
**Actions per file:** Open, Delete, Bookmark/Unbookmark
**Edit mode:** Select all, multi-delete with confirmation

---

### PAGE_AIRECORD_FILE_WRITE (ID 20) — Recording In Progress

Detailed recording view with waveform visualization, timer, and real-time subtitle display.

---

### PAGE_AIRECORD_FILE_SUMMARY (ID 80) — AI Summary

```
┌──────────────────────────────────────┐
│ [←]  AI Summary                      │
├──────────────────────────────────────┤
│                                      │
│  Meeting: 2025-11-28 14:30           │
│  Duration: 45:12                     │
│                                      │
│  ┌─ Key Points ─────────────────┐    │
│  │ 1. Q2 budget approved        │    │
│  │ 2. Marketing needs 3 hires   │    │
│  │ 3. Product launch March 15   │    │
│  └──────────────────────────────┘    │
│                                      │
│  ┌─ Action Items ───────────────┐    │
│  │ • John: Submit budget by Fri │    │
│  │ • Sarah: Draft job posts     │    │
│  └──────────────────────────────┘    │
│                                      │
│  [Scan QR to view full notes]        │
└──────────────────────────────────────┘
```

Shows AI-generated summary with key points, decisions, and action items. "AI Summarizing..." shown during generation.

---

## 3. Settings Screens

### PAGE_SET_MENU (ID 11) — Settings Menu

```
┌──────────────────────────────────────┐
│ [←]  Settings                        │
├──────────────────────────────────────┤
│                                      │
│  ┌──────────────────────────── ▶┐    │
│  │ 🔊 Audio Mode                │    │
│  ├──────────────────────────── ▶┤    │
│  │ 📷 Webcam                    │    │
│  ├──────────────────────────── ▶┤    │
│  │ 🎤 Mic Volume                │    │
│  ├──────────────────────────── ▶┤    │
│  │ 📶 Wi-Fi                     │    │
│  ├──────────────────────────── ▶┤    │
│  │ 🔵 Bluetooth                 │    │
│  ├──────────────────────────── ▶┤    │
│  │ ⚙ General                    │    │
│  ├──────────────────────────── ▶┤    │
│  │ 🤖 AI Features               │    │
│  └──────────────────────────────┘    │
│                                      │
└──────────────────────────────────────┘
```

Scrollable list. Each item navigates to its dedicated settings page.

---

### PAGE_SET_SYSTEM (ID 14) — System Settings

```
┌──────────────────────────────────────┐
│ [←]  General                         │
├──────────────────────────────────────┤
│  ┌──────────────────────────── ▶┐    │
│  │ 🔑 Password                  │    │
│  ├──────────────────────────── ▶┤    │
│  │ 📐 Device Height        [▲]  │    │
│  ├──────────────────────────── ▶┤    │
│  │ 💡 Display                   │    │
│  ├──────────────────────────── ▶┤    │
│  │ 🔌 USB Mode                  │    │
│  ├──────────────────────────── ▶┤    │
│  │ 🌐 Language           [EN]   │    │
│  ├──────────────────────────── ▶┤    │
│  │ 🕐 Date and Time             │    │
│  ├──────────────────────────── ▶┤    │
│  │ 🔔 Alert Tone                │    │
│  ├──────────────────────────── ▶┤    │
│  │ 📋 About                     │    │
│  ├──────────────────────────── ▶┤    │
│  │ ⬆ Firmware Update            │    │
│  ├──────────────────────────── ▶┤    │
│  │ 💾 Storage Space             │    │
│  ├──────────────────────────── ▶┤    │
│  │ 🔄 Factory Reset             │    │
│  ├──────────────────────────── ▶┤    │
│  │ ⏻ Power Off                  │    │
│  └──────────────────────────────┘    │
└──────────────────────────────────────┘
```

---

### PAGE_AUDIO_MODE (ID 77) — Audio Mode Selection

```
┌──────────────────────────────────────┐
│ [←]  Audio Mode                      │
├──────────────────────────────────────┤
│                                      │
│  ┌──────────────────────────────┐    │
│  │ ◉ Noise Reduction            │    │
│  │   Eliminates environmental   │    │
│  │   noise. Ideal for meetings. │    │
│  ├──────────────────────────────┤    │
│  │ ○ Original                   │    │
│  │   Reproduces authentic sound │    │
│  │                              │    │
│  │   Beamforming Pattern:       │    │
│  │   ┌────┐ ┌────┐ ┌────┐      │    │
│  │   │Omni│ │Card│ │Sup.│      │    │
│  │   │ ◎  │ │ ♥  │ │ ♥♥ │      │    │
│  │   └────┘ └────┘ └────┘      │    │
│  │   ┌────┐ ┌────┐             │    │
│  │   │Fig8│ │Ster│             │    │
│  │   │ ∞  │ │L/R │             │    │
│  │   └────┘ └────┘             │    │
│  └──────────────────────────────┘    │
│                                      │
│             [Confirm]                │
└──────────────────────────────────────┘
```

**Radio button selection:**
- Noise Reduction (Mode 1): Default, AI-powered noise cancellation
- Original with Beamforming sub-selection:
  - Omni (Mode 2): All directions
  - Cardioid (Mode 3): Front pickup
  - Supercardioid (Mode 4): Narrow front
  - Figure-8 (Mode 5): Front + back
  - Stereo (Mode 6): L/R channels (USB only)

---

### PAGE_BEAMFORMING (ID 78) — Beamforming Visualization

```
┌──────────────────────────────────────┐
│ [←]  Beamforming                     │
├──────────────────────────────────────┤
│                                      │
│         ┌──────────────┐             │
│         │              │             │
│         │   ◉ ← DOA    │             │
│         │  /  \        │             │
│         │ /    \       │             │
│         │      ●device │             │
│         │              │             │
│         └──────────────┘             │
│                                      │
│   Direction: 45° H, 12° V           │
│   Quality: ████████░░ Good           │
│   Volume:  ████████████ Loud         │
│                                      │
│   Mode: Cardioid ♥                   │
│                                      │
└──────────────────────────────────────┘
```

Real-time Direction of Arrival (DOA) visualization showing where sound is coming from. Updates the `bf_doa.png` indicator position based on `audio_mic_pickup_horizon_angle` and `audio_mic_pickup_pitch_angle`.

Quality indicators:
- `bf_good_loud.png` / `bf_good_low.png`: Good signal, loud/low
- `bf_bad_loud.png` / `bf_bad_low.png`: Bad signal quality

---

## 4. Bluetooth Screens

### PAGE_SET_BLUETOOTH (ID 15)

```
┌──────────────────────────────────────┐
│ [←]  Bluetooth                       │
├──────────────────────────────────────┤
│                                      │
│  Status: Connected to iPhone 15      │
│                                      │
│  ┌──────────────────────────────┐    │
│  │ Bluetooth          [ON/OFF]  │    │
│  ├──────────────────────────────┤    │
│  │ Auto Reconnect     [ON/OFF]  │    │
│  │ Auto-connects to last device │    │
│  │ when powered on              │    │
│  ├──────────────────────────────┤    │
│  │ [Connect New Device]         │    │
│  │ Waiting for new device       │    │
│  │ connection                   │    │
│  └──────────────────────────────┘    │
│                                      │
│  Device can be discovered as         │
│  "Insta360 Wave XXXX"               │
│                                      │
└──────────────────────────────────────┘
```

---

### PAGE_BLUETOOTH_CONNECTING (ID 16)

```
┌──────────────────────────────────────┐
│ [←]  Bluetooth                       │
├──────────────────────────────────────┤
│                                      │
│                                      │
│           ┌────────┐                 │
│           │  ◌◌◌◌  │                │
│           │  🔵    │                 │
│           │  ◌◌◌◌  │                │
│           └────────┘                 │
│                                      │
│       Bluetooth connecting...        │
│                                      │
│                                      │
└──────────────────────────────────────┘
```

---

## 5. WiFi Screens

### PAGE_SET_WIFI (ID 44)

```
┌──────────────────────────────────────┐
│ [←]  Wi-Fi                           │
├──────────────────────────────────────┤
│                                      │
│  Wi-Fi Connection                    │
│  Wi-Fi connection is required to     │
│  sync recordings to the cloud        │
│                                      │
│  Connected: ✓ MyNetwork             │
│                                      │
│  Available networks:                 │
│  ┌──────────────────────────────┐    │
│  │ 📶███ Office-5G        🔒   │    │
│  ├──────────────────────────────┤    │
│  │ 📶██░ Guest-WiFi       🔒   │    │
│  ├──────────────────────────────┤    │
│  │ 📶█░░ Neighbor          🔒   │    │
│  └──────────────────────────────┘    │
│                                      │
└──────────────────────────────────────┘
```

Signal indicators: `ui_signal_50_100.png`, `ui_signal_15_30.png`, `ui_signal_1_15.png`
Lock icon: `ui_lock.png` for password-protected networks

---

### PAGE_SET_WIFI_INFO (ID 45)

```
┌──────────────────────────────────────┐
│ [←]  MyNetwork                       │
├──────────────────────────────────────┤
│                                      │
│  Status: Connected ✓                │
│                                      │
│  Signal strength: Strong             │
│  Safe: WPA/WPA2                      │
│  IP Address: 192.168.1.42            │
│  MAC Address: AA:BB:CC:DD:EE:FF      │
│                                      │
│  IP Configuration:                   │
│  ○ Automatic                         │
│  ○ Manual IP                         │
│                                      │
│  ┌──────────────────────────────┐    │
│  │ [Ignore this network]        │    │
│  └──────────────────────────────┘    │
│                                      │
└──────────────────────────────────────┘
```

---

### PAGE_SET_KEYBOARD (ID 39)

```
┌──────────────────────────────────────┐
│ [←]  Enter Password          [Done]  │
├──────────────────────────────────────┤
│                                      │
│  ┌──────────────────────────────┐    │
│  │ ●●●●●●●●                    │    │
│  └──────────────────────────────┘    │
│                                      │
│  ┌──┬──┬──┬──┬──┬──┬──┬──┬──┬──┐    │
│  │q │w │e │r │t │y │u │i │o │p │    │
│  ├──┼──┼──┼──┼──┼──┼──┼──┼──┼──┤    │
│  │a │s │d │f │g │h │j │k │l │  │    │
│  ├──┼──┼──┼──┼──┼──┼──┼──┼──┼──┤    │
│  │⇧ │z │x │c │v │b │n │m │⌫ │  │    │
│  ├──┼──┼──┼──┼──────────┼──┼──┤      │
│  │12│  │  │  SPACE  │  │  │.  │      │
│  └──┴──┴──┴─────────┴──┴──┴───┘      │
└──────────────────────────────────────┘
```

Full QWERTY keyboard with LVGL textarea. Password mode shows dots. Delete icon: `ui_keyboard_del.png`.

---

## 6. Phone Call Screens

### phone_call (ID 73) — Incoming Call

```
┌──────────────────────────────────────┐
│                                      │
│                                      │
│           ┌──────────┐               │
│           │  👤      │               │
│           │  Avatar  │               │
│           └──────────┘               │
│                                      │
│         John Smith                   │
│         +1 (555) 123-4567            │
│         (from PHONE_BOOK.bin)        │
│                                      │
│                                      │
│    ┌──────────┐  ┌──────────┐        │
│    │ 📞 Answer│  │ 📵 Decline│       │
│    │  (green) │  │   (red)  │        │
│    └──────────┘  └──────────┘        │
│                                      │
└──────────────────────────────────────┘
```

Shows caller name from phone book (downloaded via BT PBAP). Falls back to "Unknown Call" if not in contacts.

---

### phone_calling (ID 74) — Active Call

```
┌──────────────────────────────────────┐
│                                      │
│           Calling                    │
│                                      │
│           ┌──────────┐               │
│           │  👤      │               │
│           │  Avatar  │               │
│           └──────────┘               │
│                                      │
│         John Smith                   │
│         00:05:32                     │
│                                      │
│                                      │
│  [🎤Mute]  [🔊Vol]  [📵 Hang up]    │
│                                      │
│                                      │
│  ┌──────────────────────────────┐    │
│  │ [Record call?]  Call recording│   │
│  └──────────────────────────────┘    │
└──────────────────────────────────────┘
```

Option to record the call. Mute/volume controls during call.

---

## 7. Device Height Screen

### PAGE_SET_HEIGHT (ID 22)

```
┌──────────────────────────────────────┐
│ [←]  Device Height                   │
├──────────────────────────────────────┤
│                                      │
│        ┌───┐                         │
│        │   │  ← High                 │
│        │   │                         │
│        │   │  ← Medium               │
│        │   │                         │
│        │   │  ← Low                  │
│        └───┘                         │
│                                      │
│  Height: [Low] [Medium] [High]       │
│           ○      ●        ○          │
│                                      │
│  ┌──────────────────────────────┐    │
│  │ Auto Lift            [ON/OFF]│    │
│  │ Auto-lift if obstacle when   │    │
│  │ closing                      │    │
│  └──────────────────────────────┘    │
│                                      │
└──────────────────────────────────────┘
```

Controls the motorized extending column. Three height presets.

**Toasts:**
- "Hand detected, auto-lift activated."
- "Lift obstructed, trying again..."
- "Failed to lift, maintaining current height."

---

## 8. Volume Screens

### speaker (ID 13) — Speaker Volume

```
┌──────────────────────────────────────┐
│ [←]  Volume                          │
├──────────────────────────────────────┤
│                                      │
│                                      │
│            🔊                        │
│                                      │
│   Off ├────────●─────────────┤ Max   │
│                                      │
│        Volume: 65%                   │
│                                      │
│                                      │
│                                      │
└──────────────────────────────────────┘
```

Slider control. Volume bars: `bar_vol_0.png` through `bar_vol_100.png` (11 steps).

### microphone (ID 28) — Mic Volume

```
┌──────────────────────────────────────┐
│ [←]  Mic Volume                      │
├──────────────────────────────────────┤
│                                      │
│                                      │
│            🎤                        │
│                                      │
│  Muted ├──────────●──────────┤ Max   │
│                                      │
│        Mic Volume: 80%               │
│                                      │
│                                      │
│                                      │
└──────────────────────────────────────┘
```

---

## 9. OTA Update Screens (IDs 46-54)

### PAGE_SET_OTA (IDs 46-54) — 9 Update Screens

```
┌──────────────────────────────────────┐    ┌──────────────────────────────────────┐
│ [←]  Firmware Update                 │    │ [←]  Firmware Update                 │
├──────────────────────────────────────┤    ├──────────────────────────────────────┤
│                                      │    │                                      │
│  Auto Download          [ON/OFF]     │    │  Updates to version 1.1.6.6          │
│  When connected to Wi-Fi, the device │    │                                      │
│  will automatically download updates │    │  Changes:                            │
│  during idle time.                   │    │  1. Cancel recording feature         │
│                                      │    │  2. Cantonese transcription          │
│  Current version: 1.0.13.6           │    │  3. Bug fixes                        │
│                                      │    │                                      │
│  [Check for Updates]                 │    │       [Update]  [Later]              │
│                                      │    │                                      │
│  ✓ 1.0.13.6 is the latest version   │    │                                      │
│                                      │    │                                      │
└──────────────────────────────────────┘    └──────────────────────────────────────┘

┌──────────────────────────────────────┐    ┌──────────────────────────────────────┐
│ [←]  Updating...                     │    │      Update Complete                  │
├──────────────────────────────────────┤    ├──────────────────────────────────────┤
│                                      │    │                                      │
│  Downloading...                      │    │  Update complete, Insta360 Wave      │
│                                      │    │  will restart.                       │
│  ████████████░░░░░░░░  65%           │    │                                      │
│                                      │    │          ✓                           │
│  Updated                             │    │                                      │
│                                      │    │                                      │
│         [Cancel]                     │    │                                      │
│                                      │    │                                      │
└──────────────────────────────────────┘    └──────────────────────────────────────┘
```

9 sub-screens cover: auto-download toggle, version check, update available, forced update, download progress, installing, complete, failed, battery too low.

---

## 10. PUC Camera Screens

### PAGE_SET_PUC (ID 41) — Webcam Control

```
┌──────────────────────────────────────┐
│ [←]  Camera control                  │
├──────────────────────────────────────┤
│                                      │
│  ┌──────────────────────────────┐    │
│  │ ◉ Auto Framing               │    │
│  │   Speaker Focus: auto track  │    │
│  │   speakers by sound source   │    │
│  ├──────────────────────────────┤    │
│  │ ○ Manual adjust               │    │
│  └──────────────────────────────┘    │
│                                      │
│  ┌──────────────────────────────┐    │
│  │ Smart Whiteboard    [ON/OFF] │    │
│  │ Auto-detect whiteboard       │    │
│  └──────────────────────────────┘    │
│                                      │
└──────────────────────────────────────┘
```

### PAGE_SET_PUC_MANUAL (ID 43) — Manual PTZ

```
┌──────────────────────────────────────┐
│ [←]  Manual adjust           [Reset] │
├──────────────────────────────────────┤
│                                      │
│              [▲]                      │
│                                      │
│         [◄]  ●  [►]                  │
│                                      │
│              [▼]                      │
│                                      │
│    [🔍-]  Zoom  [🔍+]               │
│                                      │
│                                      │
└──────────────────────────────────────┘
```

Pan/tilt/zoom controls via MAVLink to the PUC camera unit.

---

## 11. Device Binding Screens

### PAGE_ID_UNBOUND (ID 57) — Unbound State

```
┌──────────────────────────────────────┐
│ [←]  ID Information linking          │
├──────────────────────────────────────┤
│                                      │
│           ┌──────────┐               │
│           │  👤      │               │
│           │ No Acct  │               │
│           └──────────┘               │
│                                      │
│  Install Insta360 Wave Controller    │
│  client, register account, and link  │
│  device for cloud and AI features.   │
│                                      │
│                                      │
│                                      │
└──────────────────────────────────────┘
```

### PAGE_ID_BOUND (ID 58) — Bound State

```
┌──────────────────────────────────────┐
│ [←]  Account                         │
├──────────────────────────────────────┤
│                                      │
│  ┌────┐  user@email.com              │
│  │ 👤 │  Basic Plan                  │
│  └────┘  Remaining 280/300 minutes   │
│          Resets on 2026-04-01        │
│                                      │
│  ┌──────────────────────────── ▶┐    │
│  │ 🔑 Password                  │    │
│  ├──────────────────────────── ▶┤    │
│  │ ☁ Cloud Management           │    │
│  │   Used: 120MB / 5000MB       │    │
│  ├──────────────────────────── ▶┤    │
│  │ 🤖 AI Summary      [ON/OFF]  │    │
│  ├──────────────────────────── ▶┤    │
│  │ 🌐 Transcription Language  ▶ │    │
│  ├──────────────────────────── ▶┤    │
│  │ 📋 Terms & Policies       ▶ │    │
│  ├──────────────────────────────┤    │
│  │ [Unpair]                      │    │
│  └──────────────────────────────┘    │
└──────────────────────────────────────┘
```

---

## 12. QR Code Screen

### PAGE_QRCODE (ID 75)

```
┌──────────────────────────────────────┐
│                                      │
│                                      │
│         ┌──────────────┐             │
│         │ ██ ██ ██ ██  │             │
│         │ █  █  ██  █  │             │
│         │ ██ █  █  ██  │             │
│         │ █  ██ ██  █  │             │
│         │ ██ ██ ██ ██  │             │
│         └──────────────┘             │
│                                      │
│   Scan to view meeting notes         │
│                                      │
│   Closing in 58 seconds.             │
│                                      │
└──────────────────────────────────────┘
```

Auto-closes after `effectiveDuration` seconds from cloud response.

---

## 13. Password Screens

### PAGE_SET_PASSWORD (ID 17)

```
┌──────────────────────────────────────┐
│ [←]  Password                        │
├──────────────────────────────────────┤
│                                      │
│  ┌──────────────────────────────┐    │
│  │ Enable Password     [ON/OFF] │    │
│  ├──────────────────────────── ▶┤    │
│  │ Change Password               │    │
│  ├──────────────────────────── ▶┤    │
│  │ Forgot Password               │    │
│  ├──────────────────────────────┤    │
│  │ Disable Password              │    │
│  └──────────────────────────────┘    │
│                                      │
│  Secures your recordings and         │
│  account security                    │
│                                      │
└──────────────────────────────────────┘
```

### PAGE_SET_VERFCODE (ID 18) — Verification Code

```
┌──────────────────────────────────────┐
│ [←]  Verification mode               │
├──────────────────────────────────────┤
│                                      │
│  Verification code has been sent     │
│  to user@email.com                   │
│                                      │
│  Enter 6-digit password:             │
│  ┌──┬──┬──┬──┬──┬──┐                │
│  │ 4│ 8│ 2│  │  │  │                │
│  └──┴──┴──┴──┴──┴──┘                │
│                                      │
│  [Resend in 45s]                     │
│                                      │
│  Verification mode:                  │
│  ○ Phone    ● Email                  │
│                                      │
└──────────────────────────────────────┘
```

---

## 14. USB Mode Screens

### PAGE_SET_USB_MODE (ID 24)

```
┌──────────────────────────────────────┐
│ [←]  USB Mode                        │
├──────────────────────────────────────┤
│                                      │
│  ┌──────────────────────────────┐    │
│  │ 📂 File Transfer              │    │
│  │   Files can be transferred    │    │
│  │   to PC                       │    │
│  ├──────────────────────────────┤    │
│  │ 🔌 Reverse Charging          │    │
│  │   Charge other devices        │    │
│  └──────────────────────────────┘    │
│                                      │
└──────────────────────────────────────┘
```

### PAGE_USB_RECHARGING (ID 27)

```
┌──────────────────────────────────────┐
│       Reverse Charging...            │
├──────────────────────────────────────┤
│                                      │
│           ┌────────┐                 │
│           │ 🔋→🔋  │                │
│           └────────┘                 │
│                                      │
│  Device battery remaining: 72%       │
│  Reverse charging will stop          │
│  when below 10%.                     │
│                                      │
│  Estimated 2hrs 15mins remaining     │
│                                      │
│           [Stop]                     │
│                                      │
└──────────────────────────────────────┘
```

---

## 15. Privacy & Setup Screens

### PAGE_PRIVACY_POLICY (IDs 61-64)

```
┌──────────────────────────────────────┐
│       Terms & Policies               │
├──────────────────────────────────────┤
│                                      │
│  By agreeing, you have read and      │
│  agreed to all terms and policies.   │
│                                      │
│  ┌──────────────────────────────┐    │
│  │ 📄 User Agreement         ▶ │    │
│  ├──────────────────────────────┤    │
│  │ 📄 Privacy Policy         ▶ │    │
│  ├──────────────────────────────┤    │
│  │ 📄 Voiceprint Recognition  ▶ │    │
│  │   Service Privacy Statement  │    │
│  └──────────────────────────────┘    │
│                                      │
│  ↑ Swipe up                         │
│                                      │
│  Click "Agree" to proceed            │
│                                      │
│  [Disagree]         [Agree]          │
└──────────────────────────────────────┘
```

4 privacy screens: Terms, User Experience Program, Voiceprint consent, Friendly Reminder.

---

### PAGE_INSTALL_GUIDE (ID 67) — Setup Guide

```
Screen 1/3:                          Screen 2/3:
┌────────────────────────────┐       ┌────────────────────────────┐
│    Install Insta360 Wave   │       │                            │
│       Controller           │       │    [USB-C cable image]     │
│                            │       │                            │
│  ┌────────────────────┐    │       │  Connect to PC using a     │
│  │  [Windows] [macOS] │    │       │  USB-C cable.              │
│  └────────────────────┘    │       │                            │
│                            │       │                            │
│  Once installed, you can   │       │                            │
│  control device and use    │       │         [Next]             │
│  AI features.              │       │                            │
│         [Next]             │       └────────────────────────────┘
└────────────────────────────┘

Screen 3/3:
┌────────────────────────────┐
│                            │
│  Download and install      │
│  software from the         │
│  official website.         │
│                            │
│  ┌────────────────────┐    │
│  │  [QR Code to       │    │
│  │   download page]   │    │
│  └────────────────────┘    │
│                            │
│         [Got it]           │
└────────────────────────────┘
```

3-step guide with swipeable cards.

---

## 16. Dongle Screens

### PAGE_DONGLE_INFO (ID 85)

```
┌──────────────────────────────────────┐
│ [←]  Dongle                          │
├──────────────────────────────────────┤
│                                      │
│  ┌──────────────────────────────┐    │
│  │  Insta360 Wave Dongle        │    │
│  │  ┌──────┐                    │    │
│  │  │dongle│  Status: Connected │    │
│  │  │ img  │  Version: 0.0.0.28│    │
│  │  └──────┘                    │    │
│  └──────────────────────────────┘    │
│                                      │
│  [Unlink]                            │
│                                      │
│  Can be used with Dongle to          │
│  connect to PC                       │
│                                      │
└──────────────────────────────────────┘
```

---

## 17. Hidden / Debug Screens

### PAGE_DEBUG (ID 88) — Debug Screen

```
┌──────────────────────────────────────┐
│ [←]  Debug                           │
├──────────────────────────────────────┤
│                                      │
│  ┌──────────────────────────────┐    │
│  │ Audio DUMP 1        [ON/OFF] │    │
│  ├──────────────────────────────┤    │
│  │ Audio DUMP 2        [ON/OFF] │    │
│  ├──────────────────────────────┤    │
│  │ Audio DUMP 3        [ON/OFF] │    │
│  ├──────────────────────────────┤    │
│  │ Audio DUMP 4        [ON/OFF] │    │
│  ├──────────────────────────────┤    │
│  │ Debug Switch        [ON/OFF] │    │
│  ├──────────────────────────────┤    │
│  │ Factory Mode        [ON/OFF] │    │
│  ├──────────────────────────────┤    │
│  │ [Software update (debug)]    │    │
│  └──────────────────────────────┘    │
│                                      │
└──────────────────────────────────────┘
```

**Access:** Likely via hidden gesture or developer tools. Not in normal navigation.

**Controls:**
- 4 audio dump toggles → write PCM dumps to `/mnt/udisk/audio_dump/`
- Debug switch → enable serial debug output
- Factory mode → enter factory test mode
- Software update → trigger `ota_server_request_debug()` (debug OTA)

---

### PAGE_FACTORY_LCD (ID 89) — Factory LCD Test

```
Phase 1: Color Test                  Phase 2: Touch Grid Test
┌────────────────────────────┐       ┌────────────────────────────┐
│████████████████████████████│       │ ┌──┬──┬──┬──┬──┬──┬──┬──┐ │
│████████████████████████████│       │ │  │  │  │██│  │  │  │  │ │
│████████████████████████████│       │ ├──┼──┼──┼──┼──┼──┼──┼──┤ │
│████████████████████████████│       │ │  │██│  │  │  │██│  │  │ │
│████████ RED ███████████████│       │ ├──┼──┼──┼──┼──┼──┼──┼──┤ │
│████████████████████████████│       │ │  │  │  │  │██│  │  │  │ │
│████████████████████████████│       │ ├──┼──┼──┼──┼──┼──┼──┼──┤ │
│████████████████████████████│       │ │██│  │  │  │  │  │██│  │ │
│████████████████████████████│       │ ├──┼──┼──┼──┼──┼──┼──┼──┤ │
│                            │       │ │  │  │██│  │  │  │  │██│ │
│  (tap to cycle: R→G→B→W→K)│       │ ├──┼──┼──┼──┼──┼──┼──┼──┤ │
└────────────────────────────┘       │ │  │  │  │  │  │  │  │  │ │
                                     │ └──┴──┴──┴──┴──┴──┴──┴──┘ │
                                     │ Touch all 48 zones (8x6)  │
                                     │ ██ = touched (green)       │
                                     └────────────────────────────┘

Phase 3: Aging Test
┌────────────────────────────┐
│                            │
│                            │
│   Aging Remaining Time     │
│       01:20:34             │
│                            │
│   USB: Connected           │
│                            │
│                            │
└────────────────────────────┘
```

**Test sequence:**
1. Color cycle: Red → Green → Blue → White → Black (tap to advance)
2. Touch grid: 8×6 = 48 zones. Each touch marks zone green. All 48 required.
3. Aging test: Timer countdown for burn-in testing.

Completion: "LCD touch Check Success!" → navigate to PAGE_FACTORY_INFO.

---

### PAGE_FACTORY_INFO (ID 90)

```
┌──────────────────────────────────────┐
│       Factory Info                    │
├──────────────────────────────────────┤
│                                      │
│  Serial: INSWAVEXXXXXXXXXX           │
│  HW Rev: 3                           │
│  SW Ver: 1.1.6.6                     │
│  MCU:    0.1.3.3                     │
│  BT:     0.0.0.28                    │
│  WiFi MAC: AA:BB:CC:DD:EE:FF        │
│  BT MAC:   11:22:33:44:55:66        │
│                                      │
│  LCD Test: PASS ✓                    │
│  Touch Test: PASS ✓                  │
│  Aging: PASS ✓                       │
│                                      │
│  Board: AmlSpeaker                   │
│  Build: 2025-11-28                   │
│                                      │
└──────────────────────────────────────┘
```

---

## 18. Misc Dialog Screens

### PAGE_CONFIRM (ID 0) — Generic Confirmation

```
┌──────────────────────────────────────┐
│                                      │
│                                      │
│    ┌────────────────────────────┐    │
│    │  <Title>                   │    │
│    │                            │    │
│    │  <Description text>        │    │
│    │                            │    │
│    │  [Cancel]      [Confirm]   │    │
│    └────────────────────────────┘    │
│                                      │
│                                      │
└──────────────────────────────────────┘
```

Used for: stop recording, factory reset, power off, delete files, ignore WiFi, disconnect BT.

### PAGE_DIALOG_BOX (ID 5) — Info Dialog

```
┌──────────────────────────────────────┐
│                                      │
│    ┌────────────────────────────┐    │
│    │  <Title>                   │    │
│    │                            │    │
│    │  <Info text>               │    │
│    │                            │    │
│    │         [Got it]           │    │
│    └────────────────────────────┘    │
│                                      │
└──────────────────────────────────────┘
```

### Toast Notifications

```
┌──────────────────────────────────────┐
│ ┌──────────────────────────────────┐ │
│ │ ✓ Bluetooth connected           │ │
│ └──────────────────────────────────┘ │
│                                      │
│           (main content)             │
│                                      │
└──────────────────────────────────────┘
```

Toast icons: `toast_check.png` (success), `toast_err.png` (error), `toast_war.png` (warning), `toast_info.png` (info), `toast_battery.png`, `toast_bluetooth_connect.png`, `toast_usb_connect.png`, `toast_dongle_connect.png`.

---

## 19. Regulatory Screen

### PAGE_DEV_INFO (ID 30) — Device Info / About

```
┌──────────────────────────────────────┐
│ [←]  About                           │
├──────────────────────────────────────┤
│                                      │
│  Device Model:    Insta360 Wave      │
│  Serial Number:   INSWAVEXXXXXXXXXX  │
│  Firmware Version: 1.1.6.6           │
│  Hardware Version: 3                 │
│  Firmware Font:    MiSans            │
│                                      │
│  [Export Logs]                        │
│                                      │
│  ── Regulatory ──                    │
│  Manufacture: Arashi Vision Inc.     │
│  FCC ID: 2AWWH-CINSABCA             │
│  Model: CINSABCA                     │
│  NCC ID: CCAJ25LPADS0T2             │
│                                      │
│  This device complies with part 15   │
│  of the FCC Rules...                 │
│                                      │
└──────────────────────────────────────┘
```

Scrollable. Contains FCC compliance text. "Export Logs" button triggers log collection, AES encryption, and upload.

---

## 20. Complete Screen ID Table

| ID | Name | Category | Back → |
|---|---|---|---|
| 0 | PAGE_CONFIRM | Dialog | Parent |
| 1 | PAGE_DOUBLE_ICON_CONFIRM | Dialog | Parent |
| 2 | PAGE_REMIND_CONFIRM | Dialog | Parent |
| 3 | PAGE_ONLY_CONFIRM_WITH_IMG | Dialog | Parent |
| 4 | PAGE_CONFIG_SET | Settings | Parent |
| 5 | PAGE_DIALOG_BOX | Dialog | Parent |
| 6 | PAGE_DIALOG_BOX_WITH_IMG | Dialog | Parent |
| 7 | PAGE_DOUBLE_LONG_BTN | Controls | Parent |
| 8 | PAGE_LOADING | System | Auto |
| 9 | PAGE_WELCOME | System | → Privacy |
| 10 | main | Home | - |
| 11 | PAGE_SET_MENU | Settings | → 10 |
| 12 | ai_record | Recording | → 10 |
| 13 | speaker | Volume | → 10 |
| 14 | PAGE_SET_SYSTEM | System Settings | → 11 |
| 15 | PAGE_SET_BLUETOOTH | Bluetooth | → 11 |
| 16 | PAGE_BLUETOOTH_CONNECTING | Bluetooth | → 15 |
| 17 | PAGE_SET_PASSWORD | Security | → 14 |
| 18 | PAGE_SET_VERFCODE | Verification | → 17 |
| 19 | PAGE_AIRECORD_FOLD | File List | → 10 |
| 20 | PAGE_AIRECORD_FILE_WRITE | Recording | → 12 |
| 21 | PAGE_AIRECORD_FILE_EDIT | File Edit | → 19 |
| 22 | PAGE_SET_HEIGHT | Height | → 14 |
| 23 | PAGE_SET_DISPLAY | Display | → 14 |
| 24 | PAGE_SET_USB_MODE | USB Mode | → 14 |
| 25 | PAGE_UDISK_EXIT | USB Disk | → 24 |
| 26 | PAGE_USB_RECHARG_FAILED | Charging | → 24 |
| 27 | PAGE_USB_RECHARGING | Charging | → 24 |
| 28 | microphone | Mic Volume | → 11 |
| 29 | PAGE_SET_COMMON | Common | → 14 |
| 30 | PAGE_DEV_INFO | About | → 14 |
| 31 | PAGE_STORAGE | Storage | → 14 |
| 32 | PAGE_STORAGE_FORMAT_BAR | Format | → 31 |
| 33 | PAGE_SET_LANGUAGE | Language | → 14 |
| 34 | PAGE_SET_REGION | Region | → 14 |
| 35 | PAGE_SET_TIMEZONE | Timezone | → 36 |
| 36 | PAGE_SET_TIME | Time | → 14 |
| 37 | PAGE_SET_TIME_SUB_DATE | Date | → 36 |
| 38 | PAGE_SET_TIME_SUB_TIME | Time | → 36 |
| 39 | PAGE_SET_KEYBOARD | Keyboard | Parent |
| 40 | PAGE_SET_NUMKEYBOARD | Numpad | Parent |
| 41 | PAGE_SET_PUC | Camera | → 11 |
| 42 | PAGE_SET_PUC_AUTO | Auto Frame | → 41 |
| 43 | PAGE_SET_PUC_MANUAL | Manual PTZ | → 41 |
| 44 | PAGE_SET_WIFI | WiFi | → 11 |
| 45 | PAGE_SET_WIFI_INFO | WiFi Info | → 44 |
| 46-54 | PAGE_SET_OTA_* | OTA (9 screens) | → 14 |
| 55 | PAGE_POWEROFF_WAIT | Power Off | → 14 |
| 56 | PAGE_POWEROFF_HEIGHT_TIP | Height Tip | → 55 |
| 57 | PAGE_ID_UNBOUND | Unbound | → 11 |
| 58 | PAGE_ID_BOUND | Bound | → 11 |
| 59 | PAGE_UNBIND_CONFIRM | Unbind | → 58 |
| 60 | PAGE_ALERT_TONE | Alert Tone | → 14 |
| 61 | PAGE_PRIVACY_POLICY | Privacy | → 9 |
| 62 | PAGE_PRIVACY_POLICY_2 | UX Program | → 61 |
| 63 | PAGE_PRIVACY_POLICY_3 | Voiceprint | → 61 |
| 64 | PAGE_PRIVACY_POLICY_4 | Reminder | → 61 |
| 65 | PAGE_AIRECORD_OPENFILE | Open File | → 19 |
| 66 | PAGE_CLEAN_SNACKBAR | Cleanup | → 19 |
| 67 | PAGE_INSTALL_GUIDE | Setup Guide | → 10 |
| 68 | PAGE_AI_PRIVILEGE_1 | AI Features | → 58 |
| 69 | PAGE_AI_PRIVILEGE_2 | AI Activate | → 68 |
| 70 | PAGE_AI_PRIVILEGE_3 | Cloud Sync | → 68 |
| 71 | PAGE_AI_PRIVILEGE_4 | Summary | → 68 |
| 72 | PAGE_AI_PRIVILEGE_5 | Insight Web | → 68 |
| 73 | phone_call | Incoming Call | → 10 |
| 74 | phone_calling | Active Call | → 10 |
| 75 | PAGE_QRCODE | QR Code | Auto-close |
| 76 | PAGE_SUPERVISION | Supervision | → 10 |
| 77 | PAGE_AUDIO_MODE | Audio Mode | → 11 |
| 78 | PAGE_BEAMFORMING | Beamforming | → 77 |
| 79 | PAGE_AUDIO_MODE_ALL_INFO | Mode Detail | → 77 |
| 80 | PAGE_AIRECORD_FILE_SUMMARY | AI Summary | → 19 |
| 81 | PAGE_AI_ORIGINAL_AUDIO | Lossless | → 14 |
| 82 | PAGE_SETTING_AIRECORD_LANGUAGE | ASR Lang | → 68 |
| 83 | PAGE_SETTING_AIRECORD_LANGUAGE_MORE | More Lang | → 82 |
| 84 | PAGE_GUIDE | Guide | → 10 |
| 85 | PAGE_DONGLE_INFO | Dongle | → 11 |
| 86 | PAGE_DONGLE_BIND | Dongle Bind | → 85 |
| 87 | PAGE_BATTERY0_POWERON | Low Battery | Auto |
| 88 | PAGE_DEBUG | Debug | → 30 |
| 89 | PAGE_FACTORY_LCD | Factory LCD | → 90 |
| 90 | PAGE_FACTORY_INFO | Factory Info | → 88 |

---

## 21. Icon Asset Reference

| Directory | Count | Purpose |
|---|---|---|
| `airecord_icon/` | 11 | Recording controls (start, stop, delete, bookmark) |
| `bar/` | 10 | Progress bar segments |
| `beamforming/` | 15+ | DOA indicator, mode icons (all/heart/super/eight/stereo) |
| `bin/` | 12 dirs | Animation frames (boot, recording, call, connecting) |
| `btn/` | 20+ | Button icons (back, cancel, done, folder, hangup, etc.) |
| `id/` | 12 | Account/binding (avatar, logo, keyboard, platforms) |
| `loading/` | 2 | Spinner animations |
| `msc/` | 15+ | Background images (main, record, logo, cleaning) |
| `privilege/` | 7 | AI subscription cards |
| `puc/` | 10+ | PTZ controls (arrows, zoom, auto/manual frame) |
| `status/` | 30+ | Status bar (battery ×22, WiFi, BT, USB, mic, speaker) |
| `toast/` | 8 | Toast icons (check, error, warning, info, connections) |
| `volume/` | 20+ | Volume bars (0-100%), mic states |
| `wifi/` | 8 | WiFi signal levels, lock, error |

---

*Document generated from Ghidra decompilation of libui.so (3,506 functions) and language_EN.json string analysis.*
