# Home Screen (MAIN, ID 10)

## Source

Decompiled from `libui.so.c` → `ui_Main_screen_on_create()` at line 14310.

## Visual Layout

```
┌──────────────────────────────────────┐  ← circular display, r≈190, centre (200,152)
│  🔋  📶  🔊  🎤           12:30  │  ← status bar (lv_layer_top, y=0–32)
│                                      │
│           Not Connected              │  ← connection status label, y≈40
│                                      │
│                                      │
│ [🔊 Vol]    [●     REC    ]  [🎤 M] │  ← buttons, y≈122–236
│                                      │
│  [Mode]  [Webcam]  [Files]  [Set]   │  ← bottom nav, y≈288
└──────────────────────────────────────┘
```

## Content Container

The original pspapp's page content uses the **full 400×304 screen** as the container (status icons are embedded within the page, not a separate layer). In our implementation the top 32 px is overlaid by `StatusBar` on `lv_layer_top()`.

## Element Positions

All coordinates are **user-visible = LVGL logical** (see `COORDINATE_SYSTEM.md`).

### Connection Status Label

| Property | Value |
|----------|-------|
| Widget | `lv_label` |
| Text | "Not Connected" / "BT Connected" / "WiFi Connected" |
| Align | `LV_ALIGN_CENTER, x=0, y=−112` → user (200, 40) |
| Color | `CLR_TEXT_DIM` when disconnected, `0x00c85a` (green) when connected |

Original: a pill-shaped `lv_btn` (`DAT_00323898`) created by `FUN_0016834c`, hidden when connected, shown at `TOP_RIGHT, x=−26, y=7`. We simplify to a centred label.

### Vol / Speaker Button

| Property | Value |
|----------|-------|
| Widget | `lv_image` |
| Asset | `A:/etc/pspapp/lvgl/icon/msc/bg_main_spk_btn.png` (76×76) |
| Align | `LV_ALIGN_CENTER, x=−142, y=+8` → user top-left (20, 122) |
| Inner icon | `spk_on.png` at `TOP_MID, y=+10` inside button |
| Label | "Vol" at `BOTTOM_MID, y=−8` inside button |

Original decompiled: `lv_obj_align(btn, LV_ALIGN_TOP_LEFT, 0x14, 0x7a)` = `(20, 122)`.

### Record Button

| Property | Value |
|----------|-------|
| Widget | `lv_image` |
| Asset | `A:/etc/pspapp/lvgl/icon/msc/bg_main_record_btn.png` (180×76) |
| Align | `LV_ALIGN_CENTER, x=0, y=+46` → user top-center (200, 160) |
| Label | "> REC" centered inside button |

Original decompiled: `lv_obj_align(btn, LV_ALIGN_TOP_MID, 0, 0xa0)` = y=160.

### Mic Button

| Property | Value |
|----------|-------|
| Widget | `lv_image` |
| Asset | `A:/etc/pspapp/lvgl/icon/msc/bg_main_mic_btn.png` (76×76) |
| Align | `LV_ALIGN_CENTER, x=+142, y=+8` → user top-left (304, 122) |
| Inner icon | `mic_on.png` (normal) / `mic_off_red.png` (muted) at `TOP_MID, y=+10` |
| Label | "Mute" (normal) / "MUTED" in `CLR_RED` (muted) at `BOTTOM_MID, y=−8` |

Original decompiled: `lv_obj_align(btn, LV_ALIGN_TOP_LEFT, 0x130, 0x7a)` = `(304, 122)`.

### Bottom Navigation

4 text labels at `LV_ALIGN_CENTER, y=+136` (user y≈288):

| Label | x offset | user x |
|-------|----------|--------|
| "Mode" | −90 | 110 |
| "Webcam" | −30 | 170 |
| "Files" | +30 | 230 |
| "Settings" | +90 | 290 |

All within visible circle at y=288 (visible x ≈ 77–323). ✓

## State Updates (`update_state`)

Called every 10 ms from the UI tick loop; uses `prev_*` fields to skip redundant LVGL calls.

| Field | Triggers update when |
|-------|---------------------|
| `lbl_status` | `bt.connected` or `wifi.connected` changes |
| `img_mic` / `lbl_mute` | `audio.mic_muted` changes |

## Assets Used

| Asset | Path |
|-------|------|
| Vol button bg | `A:/etc/pspapp/lvgl/icon/msc/bg_main_spk_btn.png` |
| Rec button bg | `A:/etc/pspapp/lvgl/icon/msc/bg_main_record_btn.png` |
| Mic button bg | `A:/etc/pspapp/lvgl/icon/msc/bg_main_mic_btn.png` |
| Speaker icon | `A:/etc/pspapp/lvgl/icon/volume/spk_on.png` |
| Mic on icon | `A:/etc/pspapp/lvgl/icon/volume/mic_on.png` |
| Mic muted icon | `A:/etc/pspapp/lvgl/icon/volume/mic_off_red.png` |

> **Note**: `bg_main_spk_btn.png` is the correct asset for the Vol button (not `bg_main_circule_btn.png`).
> `mic_off_red.png` is correct for muted state (not `mic_off.png` which does not exist).

## What the Original Also Had (Not Yet Implemented)

| Element | Description |
|---------|-------------|
| Background image | `bg_main.png` (400×400 sphere) — hidden (`LV_OBJ_FLAG_HIDDEN`) in original too; looks like a "weird orb" on circular display, keep hidden |
| "More" circle | 120 px circle at `TOP_MID, y=264` — a drag-up indicator, mostly off-screen. Not implemented. |
| Scroll area | 300×60 transparent object at `CENTER` — gesture target for dragging up to "more". Not implemented. |
| Button click events | Original uses `lv_obj_add_event_cb` for each button. Not implemented yet. |
| Status icons embedded in page | Original embeds battery/wifi/bt icons in the page content. We use `StatusBar` on `lv_layer_top()` instead. |

## Key Decisions

**Why no large connection circle?** The REPORT_UI diagram shows a stylised "Insta360 Wave / Connected" circle, but examining the decompiled code confirms there is no such LVGL widget. The visual impression comes from the circular display crop + the sphere background image. We show a simple text label instead.

**Why Vol on left, Mic on right?** From the decompiled code: `vol` button at x=20 (left edge), `mic` button at x=304 (right edge). Since LVGL coords = user-visible coords, this places Vol on the left and Mic on the right as seen by the user.
