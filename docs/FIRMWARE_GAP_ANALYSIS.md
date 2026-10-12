# Stock firmware vs waved — feature gap analysis

_As of 2026-10-04, branch `feat/wifi-provisioning`._

## Latest stock firmware

| SKU | Latest | Date | We have |
|---|---|---|---|
| Wave (consumer) | **1.2.2.6** (package label still `3.0.0.1`) | 2026-06-15 | 1.2.0.6 in `firmwares/fw_v3.0.0.1`, plus **1.2.2.6 in `firmwares/fw_v1.2.2.6`** (new, extracted and decompiled) |
| Wave For Business | 1.2.0.6 (adds Insta360 Connect pairing and 3-device cascade); hidden 1.0.4.6 maintenance build | 2026-07-09 / 2026-08-31 | 1.0.2.6 |
| Wave Controller (Mac/Win) | 1.1.0 build3 | 2026-03-18 | same, byte-identical pkg |

`3.0.0.1` is the **firmware package** version, not a Wave Controller version. The
vendor catalog says so: "3.0.0.1 is the package version; the firmware version
after update is 1.2.2.6". Earlier docs that call it the "Wave Controller installer version"
are wrong.

Catalog endpoint for future checks (no device serial needed):

```bash
curl -s 'https://openapi.insta360.com/app/appDownload/getGroupApp?group=insta360-wave&X-Language=en-us' | jq
```

**1.2.2.6 is a maintenance build:** BT and dongle firmware, a Brazil ANATEL label, a new
root hash, and removal of the cloud auto-upload `"NONE"` skip. It adds no user-facing
features and leaves the Wave Controller HID protocol unchanged, so the parity gap is the
same as against 1.2.0.6. Details: [`firmwares/fw_v1.2.2.6/CHANGELOG.md`](../firmwares/fw_v1.2.2.6/CHANGELOG.md).

## Parity summary

Status comes from static code reading. "Done" means implemented and wired up, not
measured against stock on the device.

### Audio

| Feature | Stock | waved | Evidence |
|---|---|---|---|
| 8-mic dual-stream capture | ✓ | Done | `waved-audio/src/capture/alsa_dual_stream.rs` |
| AEC | TEA Kalman/NLMS/neural | Done (ERLE ~27 dB, CPU over budget) | `waved-audio/src/aec/` |
| Beam modes (Omni/Cardioid/Super/Fig-8) | ✓ | Done, plus a DoA-steered mode | `waved-audio/src/beam/mod.rs` |
| **Stereo mode over USB** | ✓ | **Partial**: L/R is computed but USB sends mono duplicated to both channels | `waved-audio/src/uac/upstream.rs:110` |
| DoA + beamforming screen | ✓ | Done | `waved-audio/src/doa/`, `waved-ui/src/screens/audio_mode.rs` |
| Noise reduction | NNPOST | Done (DFN3 on CPU, Off/Low/Med/High) | `waved-audio/src/aec/deep_filter.rs` |
| Playback EQ / MBAGC / limiter | ✓ | Done | `waved-audio/src/playback/dsp/` |
| Mic health check | new in 1.2.0.6 | Partial: module exists but nothing calls it | `waved-audio/src/mic_check.rs` |
| macOS master volume | ✓ | Partial: needs an `f_uac2` kernel patch | memory `project_mac_volume_keys` |

### Recording and storage

| Feature | Stock | waved | Evidence |
|---|---|---|---|
| Local recording | Opus/MP3 | Done (lossless WAV, integrity, reconcile) | `waved-meeting/src/recorder.rs` |
| HD recording mode | new in 1.2.0.6 | Dropped on purpose: WAV is already better | — |
| Pre-record ring buffer | new in 1.2.0.6, with toggle | **Partial**: always on (30 s), no indicator or toggle | `waved-meeting/src/recorder.rs:11` |
| **Pause / resume** | ✓ | **Missing**: `Command::PauseRecording` is defined but never handled | `waved-state/src/lib.rs:347` |
| Bookmarks | ✓ | Done (backend only, no button while recording) | `waved-meeting/src/metadata.rs` |
| **On-device file list / playback / delete** | ✓ | **Missing**: "Files" is a placeholder | `waved-ui/src/screens/settings.rs:191` |
| Storage page / format | ✓ | Partial: auto-retention only | `waved-meeting/src/retention.rs` |
| **USB mass-storage mode** | ✓ | **Missing** | `waved-ui/src/screens/system.rs:49` |
| Upload | Insta360 cloud | Replaced with a self-hosted webhook | `waved-meeting/src/upload.rs` |
| ASR, subtitles, AI summary, QR activation | ✓ | Dropped on purpose (left to downstream services) | `waved/TODO.md` |
| Diarization | ASR-coupled, named | Done (DoA numeric IDs, no names by design) | `waved-meeting/src/diarization.rs` |

### Host integration

| Feature | Stock | waved | Evidence |
|---|---|---|---|
| UAC2 capture/playback | ✓ | Done | `waved-audio/src/uac/` |
| HID consumer (vol, mute, media) | ✓ | Done | `waved-hid/src/report.rs` |
| HID telephony | ✓ | Partial: PhoneMute only, no HookSwitch, no call UI | `waved-hid/src/events.rs` |
| **Wave Controller "PC Link" (vendor HID 0x03, MAVLink, 62 commands)** | ✓ | **Partial**: built from the stock 1.2.2.6 code (audio, meeting, system, Wi-Fi status; binding, cloud and OTA answered "unsupported"); not yet checked against the app (J5) | `waved-hid/src/pclink/`, `waved/docs/WAVE_CONTROLLER_PROTOCOL.md` |
| Mobile app (MQTT binding) | new in 1.2.0.6 | Missing / mostly dropped | — |
| OTA | 9 screens + PC Link | **Missing**: adb deploy only; `security.ota_server` is unused | — |

### System and peripherals

| Feature | Stock | waved | Evidence |
|---|---|---|---|
| Wi-Fi station + provisioning | keyboard | Done, better (WPA2 AP + QR + captive portal) | `waved-provision/` |
| Time / timezone / RTC | ✓ | Partial: NTP only, clock shows UTC | `waved-ui/src/status_bar.rs:143` |
| Language (20) | ✓ | Missing: `i18n` loads but is never used | `waved-ui/src/lib.rs:71` |
| Display brightness / auto sleep | ✓ | Missing (placeholder row) | `system.rs:51` |
| Factory reset, About, device password | ✓ | Missing | — |
| Alert tones | ✓ | Partial: they play, but there's no on/off toggle | `waved-ui/src/audio_fx.rs` |
| Mic volume page | ✓ | Missing (placeholder row) | `system.rs:45` |
| Power off | ✓ (with confirmation) | Done, **but no confirmation step** | `system.rs:52` |
| Battery | ✓ | Partial (not verified on device) | `waved-mcu/src/battery.rs` |
| Motorized mic arm | ✓ | Partial (Control Center code is dead) | `waved-mcu/src/motor.rs` |
| Physical buttons / LEDs | ✓ | Missing: input path not yet found | `waved/docs/AUDIO_HARDWARE.md:648` |
| Bluetooth pairing / audio / calls | ✓ | Partial: status icon only | `waved-bt/` |
| Phone book (PBAP) | ✓ | Missing; suggest dropping it for privacy | — |
| 2.4 GHz dongle, PUC camera, cascade | ✓ | Missing (niche) | — |

UI coverage is 11 waved screens against 91 stock pages. The redesign in
[`docs/mockups/`](mockups/) covers the screens needed to close the gaps below.

## Top 10 to close, prioritized

| # | Item | Effort | Why |
|---|---|---|---|
| 1 | **Wave Controller PC Link** (vendor HID 0x03): MAVLink framing, heartbeat, `auth_dev`, then audio, meeting and sys commands. Start with a usbmon capture against stock. | L | Biggest compatibility gap and a stated project goal |
| 2 | **Stereo over USB**: send `mic_stereo_f32` when the mode is Stereo | S | Selectable in the UI, silently mono |
| 3 | **Pause / resume**: handle the existing commands and add a button | S–M | Core recording control |
| 4 | **On-device Files** (list, play, delete, retry upload) | M | Placeholder in the main menu |
| 5 | **USB mass-storage toggle** | M | Offline way to get recordings off the device |
| 6 | **Self-hosted, signed OTA** for the waved image | M–L | Needed before anyone but us can run it |
| 7 | **Pre-record visibility**: indicator plus toggle | S | Always-on buffering with no indicator is a privacy problem |
| 8 | **Settings basics**: timezone, brightness/sleep, mic volume, tones toggle, factory reset, About, power-off confirmation | M (several S items) | Six System rows are fake today |
| 9 | **Bluetooth pairing / routing** plus call screen and HID HookSwitch | L | Parity with stock |
| 10 | **Hardware verification**: MCU battery, motor, LEDs, physical buttons | M | Unverified frame layouts |

## UI defects found during the audit (fix regardless of redesign)

1. The back button on System and Webhook sits outside the visible circle at (8,8): the icon centre is at d≈199 px, beyond the r≈190 edge.
2. The System and Webhook titles sit under the status bar at y=20.
3. The toast and status-bar `lv_obj` roots are clickable and swallow taps. The toast covers Confirmation's buttons for 1.5 s on every stop. Fix: clear `LV_OBJ_FLAG_CLICKABLE`.
4. `·` and `…` have no glyph in the Montserrat build and render as boxes.
5. `btn_save` / `btn_send` / `btn_discard` are not deployed to the device, so the Confirmation buttons render blank.
6. The Done buttons are white discs: the default light theme with no background colour set.
7. Six System rows and "Files" show a "Bookmark added" toast as a placeholder.
8. The status bar clock shows UTC and only refreshes when the icons change.
