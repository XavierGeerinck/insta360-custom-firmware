# UI Assets Reference

All assets live on the device at `/etc/pspapp/lvgl/`. Accessed via LVGL's POSIX FS driver with drive letter `A:` → device root, so paths are `A:/etc/pspapp/lvgl/...`.

## Main Screen (Home) Buttons — `/icon/msc/`

| File | Size | Used for |
|------|------|---------|
| `bg_main_spk_btn.png` | 76×76 | Vol / Speaker button background |
| `bg_main_mic_btn.png` | 76×76 | Mic button background |
| `bg_main_record_btn.png` | 180×76 | Record button background |
| `bg_main_circule_btn.png` | 76×76 | Generic circular button (mode, settings, etc.) |
| `bg_main.png` | 400×304 | Sphere background — **do not use** (looks like a "weird orb" on circular display) |
| `bg_main_disconnect.png` | — | Disconnected state indicator |

## Volume / Mic Icons — `/icon/volume/`

| File | Used for |
|------|---------|
| `mic_on.png` | Mic button normal state |
| `mic_off_red.png` | Mic button muted state (**not** `mic_off.png`) |
| `mic_up.png` / `mic_down.png` | Mic level animation |
| `mic_real_1_25.png` … `mic_real_75_100.png` | Real-time mic level (4 levels) |
| `spk_on.png` | Speaker / Vol button normal state |
| `spk_off.png` | Speaker muted |
| `spk_up.png` / `spk_down.png` | Speaker volume animation |
| `bar_vol_0.png` … `bar_vol_100.png` | Volume bar levels (0–100, step 10) |

## Status Bar Icons — `/icon/status/`

| File | Used for |
|------|---------|
| `battery_0_5.png` … `battery_91_100.png` | Battery level (11 bands) |
| `battery_charging_*.png` / `battery_recharging_*.png` | Charging variants |
| `bluetooth.png` | BT disconnected |
| `bluetooth_green.png` | BT connected |
| `wifi_unconnected.png` | WiFi off |
| `wifi_signal_1_15.png` … `wifi_signal_50_100.png` | WiFi strength (3 levels) |
| `wifi_no_wan.png` | WiFi connected but no internet |
| `usb.png` / `usb_green.png` | USB connected |
| `dongle.png` / `dongle_green.png` | USB dongle connected |
| `help.png` | Connection status indicator |

## Navigation Buttons — `/icon/btn/`

| File | Used for |
|------|---------|
| `arrow_down.png` | "More" / scroll indicator (rotated 180° for up arrow) |
| `btn_settings.png` | Settings button |
| `btn_airecord.png` | AI record button |
| `btn_goback.png` | Back navigation |
| `btn_play.png` / `btn_pause.png` / `btn_stop.png` | Playback controls |
| `btn_folder.png` / `btn_folder_28.png` | File browser |

## Beamforming Icons — `/icon/beamforming/` (32×32)

| File | Mode |
|------|------|
| `bf_mode_all.png` | Omnidirectional |
| `bf_mode_heart.png` | Cardioid |
| `bf_mode_super_heart.png` | Super-cardioid |
| `bf_mode_eight.png` | Figure-8 |
| `bf_mode_stereo.png` | Stereo |

## Other

| File | Path | Used for |
|------|------|---------|
| `bg_airecord.png` etc. | `/icon/msc/` | AI recording screen buttons |
| `recharging.png` | `/icon/msc/` | Recharging screen |
