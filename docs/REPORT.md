# Insta360 Wave Firmware Deep Analysis Report

> The decompiled C these reports cite (`decompiled/*.c`, with line numbers) is not
> distributed. `scripts/decompile_version.sh` regenerates it from a stock firmware
> image with Ghidra (into `firmwares/<version>/decompiled/`); line numbers match
> the 1.1.6.6 image.

**Date:** 2026-03-18
**Firmware versions analyzed:** v1.0.13.6 (Oct 27, 2025) and v1.1.6.6 (Nov 28, 2025)
**Device:** Insta360 Wave Conference Speakerphone (Model CINSABCA)
**Methodology:** Firmware extraction, Ghidra decompilation (15,597 functions across 6 binaries), string analysis, config/script review

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Hardware Platform](#2-hardware-platform)
3. [Firmware Format & Extraction](#3-firmware-format--extraction)
4. [Software Architecture](#4-software-architecture)
5. [Boot Sequence](#5-boot-sequence)
6. [Network Services & Attack Surface](#6-network-services--attack-surface)
7. [Cloud Communication](#7-cloud-communication)
8. [Authentication & Cryptography](#8-authentication--cryptography)
9. [Telemetry & Data Collection](#9-telemetry--data-collection)
10. [OTA Update Mechanism](#10-ota-update-mechanism)
11. [Audio Processing Pipeline](#11-audio-processing-pipeline)
12. [AI/ML Subsystem](#12-aiml-subsystem)
13. [Bluetooth Protocol](#13-bluetooth-protocol)
14. [UI System](#14-ui-system)
15. [Meeting Recording & Cloud Upload](#15-meeting-recording--cloud-upload)
16. [Security Findings](#16-security-findings)
17. [Privacy Findings](#17-privacy-findings)
18. [Version Differences (v1.0.13.6 → v1.1.6.6)](#18-version-differences)
19. [Open Source Reconstruction Guide](#19-open-source-reconstruction-guide)
20. [Appendices](#20-appendices)

---

## 1. Executive Summary

The Insta360 Wave is a conference speakerphone with AI-powered meeting recording, transcription, and summarization. This report documents a complete firmware teardown including Ghidra decompilation of all core binaries.

**Key findings:**

- **Critical security vulnerabilities**: RSA-512 firmware signing (factorable since 1999), OTA without TLS verification, MD5-only firmware validation, telnet as root with no firewall, unauthenticated CGI endpoints that allow remote audio recording control
- **Extensive telemetry**: 35+ event types sent to ByteDance's DataFinder SDK infrastructure (`snssdk.insta360.com`), tracking every user interaction including mute toggles, recordings, file operations, and physical device adjustments
- **Privacy concerns**: Meeting audio uploaded to Insta360 cloud for AI processing, phone contact list downloaded via Bluetooth PBAP, speaker voiceprint collection, WiFi passwords logged in plaintext
- **Sophisticated audio AI**: 15 neural network models for beamforming, echo cancellation, noise reduction; on-device ASR using SenseVoice, Zipformer, CAM++ speaker diarization, Silero VAD
- **Standard embedded Linux stack**: Amlogic A5 SoC, Linux 5.4.242, BusyBox, SWUpdate, largely reconstructible with open-source components

---

## 2. Hardware Platform

| Component | Details |
|---|---|
| **SoC** | Amlogic A5 (AV400) — ARM Cortex-A53 (aarch64) |
| **AOCPU** | RISC-V core running FreeRTOS (power management) |
| **NPU** | Amlogic NPU via `galcore.ko` (VeriSilicon Vivante, 350MHz) |
| **RAM** | 4GB (with 800MB zram swap) |
| **Storage** | 32GB eMMC |
| **Linux Kernel** | 5.4.242 (aarch64) |
| **Display** | 1.82" touchscreen, 400×304 pixels, framebuffer `/dev/fb0` |
| **Microphones** | 8 MEMS units (10-channel array including reference channels) |
| **Speaker** | Dual TAS5707 amplifier chips (A/B), 91 dB SPL |
| **WiFi** | ATBM6031 (SDIO), 2.4/5 GHz dual-band |
| **Bluetooth** | Amlogic SDIO BT module, Classic + BLE + 2.4GHz proprietary dongle |
| **Motor** | Motorized extending/retracting speaker column |
| **Battery** | 9800 mAh, reverse charging support |
| **USB** | USB-C, UAC 2.0 + HID + MSC + ADB gadget composite |
| **Board name** | `AmlSpeaker` (Amlogic reference speaker platform) |
| **USB IDs** | VID `0x18F0`, PID `0x4e40` |

### Partition Layout

| Partition | Mount | Filesystem | Purpose |
|---|---|---|---|
| `/dev/boot` | - | Android boot image | Linux kernel |
| `/dev/dtb` | - | Device tree blob | Hardware description |
| `/dev/system` | `/` | SquashFS (gzip, RO) | Root filesystem |
| `/dev/data` | `/data` | ext4 (RW) | Persistent user data |
| `/dev/model` | `/model` | ext4 (RO) | AI models (3GB partition) |
| `/dev/psp_udisk_s` | `/mnt/udisk` | exFAT (RW) | User recordings |
| unifykey | `/unifykey` | Raw | Amlogic secure key storage |

---

## 3. Firmware Format & Extraction

### SWU File Structure

```
Offset 0x000000: [4 bytes] Magic: 0x21 0x43 0x23 0x01 ("!C#\x01")
Offset 0x000004: [1 MB]    JSON header (version, changelog, sizes, MD5)
Offset 0x100004: [rest]    CPIO archive (SWUpdate format, "070702" newc CRC)
```

### JSON Header Fields

```json
{
  "json_version": 1,
  "data": {
    "appVersion": {
      "version": { "zh_cn": "1.1.6.6", "en_us": "1.1.6.6" },
      "description": { "en_us": "1. Cancel recording feature added..." },
      "has_model_package": false,
      "bin_size": 440935424,
      "swu_size": 440935424,
      "model_size": 3221225472,
      "total_size": 441984004,
      "model_md5": "a21d244e871aee5efb1ce581d771e2db",
      "force": false,
      "dongle": { "version": { "en_us": "0.0.0.28" } }
    }
  }
}
```

### CPIO Archive Contents

| File | Size | Target | Purpose |
|---|---|---|---|
| `sw-description` | 1.1K | SWUpdate manifest | Defines update targets + SHA256 |
| `sw-description.sig` | 64B | - | RSA-512 signature (!) |
| `u-boot.bin.signed` | 3.0 MB | Bootloader | Signed U-Boot |
| `dtb.img` | 58 KB | `/dev/dtb` | Device tree binary |
| `boot.img` | 14 MB | `/dev/boot` | Linux kernel |
| `rootfs.squashfs` | 203-206 MB | `/dev/system` | Root filesystem |
| `update.sh` | 542B | - | Pre/post install hooks |
| `udisk_s.exfat` | 200 MB | `/dev/psp_udisk_s` | User storage image |
| `BT.bin` | ~605 KB | BT module | Bluetooth firmware |
| `MCU.bin` | 128 KB | MCU UART | MCU firmware |
| `DONGLE.bin` | ~175 KB | Dongle OTA | Wireless dongle firmware |

### Extraction Commands

```bash
# 1. Extract JSON header
python3 -c "
with open('firmware.swu', 'rb') as f:
    f.seek(4)
    data = f.read(16384).decode('utf-8', errors='replace')
    # Find JSON end by brace counting
    depth, end = 0, 0
    for i, c in enumerate(data):
        if c == '{': depth += 1
        elif c == '}':
            depth -= 1
            if depth == 0: end = i + 1; break
    print(data[:end])
"

# 2. Extract CPIO archive (starts at offset 0x100004)
dd if=firmware.swu bs=1 skip=1048580 of=firmware.cpio

# 3. Extract CPIO contents
cpio -idv < firmware.cpio

# 4. Extract SquashFS root filesystem
unsquashfs -d rootfs rootfs.squashfs
```

---

## 4. Software Architecture

### Internal Codename: "PSP" (Personal Smart Peripheral)

The application follows a **microservice-like architecture** with a thin launcher (`pspapp`, 10K) that calls `ins_entry_init()` from `libins_entry.so`, which orchestrates ~16 server modules communicating via an in-memory observer-pattern database and ZeroMQ IPC.

### Binary Components

| Binary | Functions | Size | Purpose |
|---|---|---|---|
| `pspapp` | 60 | 10K | Launcher, signal handlers, calls `ins_entry_init()` |
| `libins_entry.so` | 3,882 | 2.4 MB | Core application — all server modules |
| `libcloud_proto_https.so` | 6,457 | 1.6 MB | Cloud API client (C++, nlohmann/json v3.12) |
| `libui.so` | 3,506 | 2.2 MB | LVGL-based touchscreen UI (91 screens) |
| `libInstaAudio.so` | 1,634 | 420 KB | Audio pipeline orchestration |
| `libTEAProcessSdk.so` | - | 8.5 MB | TEA audio DSP SDK (by Roomix/QCI) |
| `libasr_psp.so` | - | 401 KB | ASR engine (WeNet, SenseVoice, Zipformer) |
| `libbva_audio.so` | - | 1.9 MB | Low-level DSP (BVA/RDA silicon vendor) |
| `libdatabase.so` | 58 | 58 KB | In-memory observer-pattern data store |
| `libapplogrs.so` | - | - | ByteDance DataFinder SDK (Rust) |

### Server Modules (in libins_entry.so)

| Server | Source File | Purpose |
|---|---|---|
| `audio_server` | `server/audio/audio_server.c` | Audio I/O, mixing, ASR integration |
| `bt_server` | `server/bt/bt_server.c` | Bluetooth Classic + BLE + dongle |
| `cloud_server` | `server/cloud/cloud_server.c` | Cloud API, file upload, transcription |
| `meeting_server` | `server/meeting/meeting_server.c` | Recording lifecycle, project DB |
| `wifi_server` | `server/wifi/wifi_server.c` | WiFi STA management |
| `hid_server` | `server/hid/hid_server.c` | USB HID protocol for PC communication |
| `puc_server` | `server/puc/puc_server.c` | PTZ camera control via MAVLink |
| `mcu_server` | `server/mcu/mcu_server.c` | Battery, motor, LEDs, sensors |
| `ota_server` | `server/ota/ota_server.c` | Firmware OTA updates |
| `devbind_server` | `server/bind/devicebind_server.c` | User account binding |
| `file_server` | `server/file/file_server.c` | Recording file management |
| `system_server` | `server/system/system_server.c` | Config, USB mode, factory reset |
| `time_server` | `server/time/time_server.c` | NTP sync, RTC, timezone |
| `subtitle_server` | `server/subtitle/subtitle_server.c` | Real-time subtitle display |
| `finder_server` | `server/datafinder/datafinder_server.c` | Analytics/telemetry |
| `logs_upload_server` | `server/logs_upload/logs_upload_server.c` | Log collection and upload |
| `sysdb_server` | `server/sysdb/sysdb_server.c` | Factory tool interface |

### IPC Mechanisms

| Mechanism | Usage |
|---|---|
| **Observer-pattern database** (`libdatabase.so`) | Real-time state propagation between modules |
| **ZeroMQ** (`libczmq.so`) | `tcp://*:5533` (server), `tcp://127.0.0.1:5544` (client) |
| **D-Bus** | System bus for inter-process commands |
| **Named pipes** | `/tmp/hid_fifo_input`, `/tmp/hid_fifo_output` (USB HID) |
| **UART** | `/dev/ttyS1` (BT/MCU), `/dev/ttyS3` (PUC camera) |

### Build Environment (leaked from debug paths)

```
Jenkins CI: /data/jenkins/workspace/PSP_Releasebuild/575/
Developer:  /home/jasper/WORK/PSP_1110/SOC/vendor/insta360/pspapp/
Platform:   a5_av400_psp_a64_release
Source:     pspapp-0.0.0/src/
```

---

## 5. Boot Sequence

BusyBox init driven via `/etc/inittab`:

1. **Kernel init** — mount proc/devpts/shm, set hostname "buildroot", inject `nameserver 8.8.8.8`
2. **Serial console** — `ttyS0::respawn:-/bin/sh` (root shell, **no authentication**)
3. **S02overlayfs** — Read "deviceid" from Amlogic unifykeys; debug mode = full writable overlay, release mode = overlay `/etc`, `/opt`, `/usr` only. Mount `/dev/data` (ext4), `/dev/model` (ext4 ro), `/dev/psp_udisk_s` (exfAT)
4. **S10mdev** — Start mdev device manager
5. **S11rsyslogd** — Log to `/opt/logs/psp.log`
6. **S30dbus** — Start D-Bus system bus
7. **S45asound** — Init ALSA for dual TAS5707 amplifiers (volume 150/511 default)
8. **S50_ko_install** (background) — Load kernel modules:
   - `ddr_access_priority.ko`
   - Create 800MB zram swap (priority 10, swappiness 10)
   - `tea_license.ko` (proprietary audio license enforcement)
   - `aml_wifi.ko` + `atbm613x_wifi_sdio.ko` (WiFi, 5 retries with GPIO power cycle)
   - `galcore.ko` (NPU, background)
   - Boot animation (raw RGB frames → framebuffer)
9. **S60pspapp_daemon** — Start `pspapp_daemon` + `pspapp_watchdog`
10. **S89usbgadget** — Configure USB composite gadget: UAC2 (48kHz/16-bit stereo) + HID (Vendor+Telephone+Consumer) + ADB (initialized but disabled) + MSC
11. **S90_emmc_rcv** — eMMC firmware revision check/upgrade
12. **S99_PSPLauncher** — Read hardware revision via ADC, enable core dumps, setup SEGV/ABRT/FPE handlers

---

## 6. Network Services & Attack Surface

### Exposed Services

| Service | Port/Interface | Authentication | Risk |
|---|---|---|---|
| **Telnet** | TCP 23 (via inetd) | Root password hash only | **CRITICAL** — root shell over network |
| **Serial console** | UART ttyS0 | **None** | **CRITICAL** — direct root shell |
| **CGI Web UI** | HTTP (likely 80) | **None** | **CRITICAL** — unauthenticated commands |
| **WiFi AP** | 192.168.2.1 | WPA2 (runtime configured) | Medium |
| **dnsmasq** | UDP 53/67 | N/A (AP mode only) | Low |
| **USB ADB** | USB | Initialized but disabled | Low (requires physical access) |
| **USB Audio** | USB UAC2 | N/A | Low |
| **USB HID** | USB `/dev/hidg0` | Session token (AES-ECB) | Medium |
| **gdbserver** | Binary present | Not auto-started | Low |

### CGI Endpoints (unauthenticated, root-level)

| Endpoint | Capability |
|---|---|
| `/cgi-bin/record/record_ctl.sh` | **Start/stop/delete/play audio recordings** |
| `/cgi-bin/record/record_list.sh` | List all recordings |
| `/cgi-bin/wifi/wifi_tool.sh` | Configure WiFi |
| `/cgi-bin/wifi/scan_list.sh` | Scan WiFi networks |
| `/cgi-bin/spotify/set_spotify.sh` | Set Spotify credentials |
| `/cgi-bin/swupdate/swupdate.sh` | Trigger firmware update |
| `/cgi-bin/swupdate/device_info.sh` | Device info disclosure |

**No firewall rules exist anywhere in the filesystem.** Zero iptables/nftables configuration.

---

## 7. Cloud Communication

### API Base URLs

| Region | URL | Purpose |
|---|---|---|
| Global | `https://api.insta360.com` | Core API |
| China | `https://openapi-hz.insta360.com/summary-service` | Summary/transcription |
| US | `https://openapi-va.insta360.com/summary-service` | Summary/transcription |
| EU | `https://openapi-fra.insta360.com/summary-service` | Summary/transcription |
| OTA | `https://openapi.insta360.com/app/v1/common/upgrade/psp_firmware` | Firmware updates |
| OTA (model) | `https://openapi.insta360.com/app/v1/common/upgrade/psp_model` | AI model updates |
| Test | `https://openapi-test.insta360.com/...` | Staging (still in binary) |
| Test | `https://service-met-hk-test.insta360.com/summary-service` | HK test server |
| Analytics | `https://snssdk.insta360.com` | ByteDance DataFinder |

### All 16 REST Endpoints

| Endpoint | Method | Purpose |
|---|---|---|
| `/device/secret/register` | POST | Register device, get encrypted secret + salt |
| `/device/secret/token/refresh` | POST | HMAC-SHA256 signed token refresh |
| `/upload/uploadInit` | POST | Initialize meeting upload session |
| `/upload/getUploadSignUrl` | POST | Get pre-signed S3 upload URL |
| `/upload/reportFileUploadStatus` | POST | Track upload progress |
| `/upload/completeUpload` | POST | Finalize upload |
| `/upload/uploadTaskFinishNotice` | POST | All uploads done notification |
| `/upload/meetingInfo` | POST | Upload meeting metadata |
| `/upload/dotTask` | POST | Analytics/telemetry dot task |
| `/device/summary/asr/task` | POST | Submit ASR task |
| `/device/summary/asr/startSummaryWithTranscription` | POST | Cloud transcription + AI summary |
| `/device/summary/qrcode` | POST | Generate meeting QR code |
| `/summary/speaker/syncSpeaker` | POST | Speaker diarization sync |
| `/vocabulary/syncWords` | POST | Sync custom vocabulary |
| `/vocabulary/checkVersion` | POST | Check vocabulary version |
| `/collect/getCollectUrl` | POST | Get data collection URL |

### Data Uploaded Per Meeting

```json
{
  "localMeetingId": "...",
  "deviceType": "PSP",
  "deviceId": "...",
  "language": "en_US",
  "timezone": "...",
  "isTranscriptOnly": false,
  "isSummaryOpen": true,
  "userCloudTranscription": true,
  "startTime": 1732000000
}
```

**File types uploaded:**
- `audio_media` — Full meeting audio recording
- `audio_play` — Playback audio content
- `screen_shot` — Screenshots captured during meetings
- `device_asr` — On-device ASR transcription results
- `meeting_info` — Meeting metadata

### HTTP Headers on Every Request

```
Content-Type: application/json
User-Agent: PSP_DEVICE
Accept: */*
Cache-Control: no-cache
Accept-Encoding: gzip, deflate, br
Connection: keep-alive
X-Device-Token: <token>
X-Device-Id: <device_serial>
X-Timestamp: <unix_timestamp>
X-Nonce: <random_nonce>
X-Signature: <hmac_sha256_hex>
```

---

## 8. Authentication & Cryptography

### Device Registration Flow

1. Device sends `{firmwareVersion, deviceId, nonce}` to `/device/secret/register`
2. Server returns AES-128-CBC encrypted `deviceSecret` and `salt`
3. Decryption key: `HMAC-SHA256(nonce, "key:" + nonce)` truncated to 16 bytes
4. Decryption IV: `HMAC-SHA256(nonce, "iv:" + nonce)` truncated to 16 bytes
5. Decrypted values stored in Amlogic UNIFYKEYS (hardware-backed) and in-memory database

### Token Refresh

1. Generate HMAC-SHA256: `HMAC(salt, deviceId + timestamp + nonce + deviceSecret)`
2. Send with headers: `X-Device-Id`, `X-Timestamp`, `X-Nonce`, `X-Signature`
3. Receive `newDeviceToken` + `expireTime`
4. On expiry: set `X-Check:IGNORE` header to bypass server validation during refresh

### Cryptographic Implementations Found

| Implementation | Algorithm | Usage | Issues |
|---|---|---|---|
| OpenSSL AES-256-CBC | `EVP_aes_256_cbc` | Log encryption | **Static IV from global pointer** |
| Hand-rolled AES-128-ECB | Custom `aesEncrypt`/`aesDecrypt` | PC Link protocol | **No IV, ECB mode, pattern-vulnerable** |
| OpenSSL AES-128-CBC | `EVP_aes_128_cbc` | Device secret decryption | Key derived from nonce (OK) |
| HMAC-SHA256 | OpenSSL | Request signing, token refresh | Nonce uses `srand(time())+rand()` (**weak**) |
| MD5 | OpenSSL | File integrity, firmware verification | **Cryptographically broken** |
| SHA-256 | OpenSSL | Log key derivation, password hash | MAC + timestamp as input (**predictable**) |
| RSA-512 | SWUpdate | Firmware signature | **Factorable in hours** |

### Key Obfuscation Scheme (Reverse-Engineered)

`dilute_encryption_key` hides a 32-byte key in 1024 bytes of random data:

```c
for (i = 0; i < 32; i++) {
    offset = i * 0x20 + (i & 7);
    buffer[offset]     = 0xAA;      // marker
    buffer[offset + 1] = key[i];    // actual key byte
    buffer[offset + 2] = 0xAA;      // marker
}
// Magic at end: 0xDEADBEEF 0x2020 at bytes 0x3FA-0x3FF
```

Recovery: Check for `0xDEADBEEF` magic at 0x3FA, extract bytes between `0xAA` markers.

### Hardcoded Secrets Found

| Secret | Location | Value |
|---|---|---|
| Default AES key | `libins_entry.so` | `0123456789abcdef0123456789abcdefabcdef9876543210` |
| Test CS token | `libins_entry.so` | `cstoken123` |
| Security key seed | `libcloud_proto_https.so` | `Insta360_Summary_Device_Security_Key_2024` |
| WiFi PSK | `/etc/wpa_supplicant.conf` | SSID: `insta360-guest`, PSK: `insta360` |
| Root password | `/etc/shadow` | **`123456`** (SHA-256-crypt, cracked via hashcat brute-force in <3 min) |
| SWUpdate key | `/etc/swupdate-public.pem` | RSA-512 public key |
| Fallback USB serial | `S89usbgadget` | `0123456789ABCDEF` |
| Client version | `libins_entry.so` | `PSP_0.3.19` |

---

## 9. Telemetry & Data Collection

### SDK: ByteDance "Rangers AppLog" (DataFinder)

- **App ID**: `10000022` (hardcoded)
- **Endpoint**: `https://snssdk.insta360.com` (dynamic, based on bind region)
- **User ID**: Device serial number
- **Open UDID**: Derived from device MAC address
- **SDK DB**: `/data/common_settings/finder/finder_sdk_dir` (SQLite with `event_data` table)
- **Library**: `libapplogrs.so` (Rust-based)

### All Tracked Events (35+)

| Category | Event | Data Fields |
|---|---|---|
| **Power** | `device_power_on` | serial, fw_ver, language, timezone, storage sizes |
| | `device_power_off` | serial, fw_ver, battery, power_off_type |
| | `system_crash` | serial, fw_ver, crash details |
| **Recording** | `ai_record_start` | serial, fw_ver, transcription mode, language |
| | `ai_record_end` | serial, fw_ver, duration, file size |
| | `ai_record_pause` | serial, fw_ver |
| | `ai_record_mark` | serial, fw_ver, record_id |
| **Network** | `device_network_connect` | serial, fw_ver, connection details |
| | `device_network_disconnect` | serial, fw_ver |
| **Bluetooth** | `bluetooth_connect` | serial, fw_ver, BT details |
| **Files** | `open_record_file` | serial, fw_ver, file details |
| | `listen_file` | serial, fw_ver, file details |
| | `delete_file` | serial, fw_ver, file details |
| | `upload_cloud_file` | serial, fw_ver |
| | `upload_to_cloud_toggle` | serial, fw_ver |
| **OTA** | `check_update` | serial, fw_ver |
| | `firmware_download` | serial, fw_ver, details |
| | `firmware_install` | serial, fw_ver, details |
| **User Actions** | `toggle_mic_mute` | serial, fw_ver, state |
| | `toggle_speaker_mute` | serial, fw_ver, state |
| | `prompt_tone_setting` | serial, fw_ver, settings |
| | `password_lock_toggle` | serial, fw_ver, state |
| | `set_language_region` | serial, fw_ver, language |
| **Phone** | `call_answer` | serial, fw_ver |
| | `call_end` | serial, fw_ver |
| **Binding** | `bind_id` | serial, fw_ver |
| | `unbind_id` | serial, fw_ver |
| **Hardware** | `mechanical_height_adjust` | serial, fw_ver |
| | `enter_usb_storage_mode` | serial, fw_ver |
| | `enter_reverse_charging_page` | serial, fw_ver |
| | `reverse_charging_start/end` | serial, fw_ver, details |
| **Setup** | `device_activation` | serial, fw_ver |
| | `factory_reset` | serial, fw_ver |
| | `privacy_agreement` | serial, fw_ver |
| | `join_user_experience` | serial, fw_ver |
| | `init_start` / `init_end` | serial, fw_ver, timing |
| **Transcription** | `transcription_mode_set` | serial, fw_ver, mode |
| | `transcription_language_set` | serial, fw_ver, language |

Every event includes: `mic_serial_number`, `fw_ver`, `evt_language`, `evt_timezone`, `evt_time`.

---

## 10. OTA Update Mechanism

### Multi-Component Update System

| Component | Package | Transport | Verification |
|---|---|---|---|
| SOC firmware | `software.swu` | HTTPS (curl) | **MD5 only** |
| AI Model | `software_model.swu` | HTTPS (curl) | MD5 + dd to `/dev/model` |
| BT module | `BT.bin` | UART via bt_server | Checksum |
| Dongle | `DONGLE.bin` | BT OTA protocol | Checksum |
| MCU | `MCU.bin` | UART via mcu_server | Checksum |

### SOC Firmware Update Flow

```
1. Check: curl -s -vk \                              # ← TLS verification DISABLED
     -H "X-Device-Serial: <serial>" \
     -H "X-Device-Version: <version>" \
     https://openapi.insta360.com/app/v1/common/upgrade/psp_firmware \
     -o /tmp/psp_firmware.json

2. Parse JSON: { version, download_url, file_md5, forced, descriptions }

3. Download: curl -C - -o /mnt/udisk/misc/data/wavesystemfile/software.swu \
     <download_url> --progress-bar

4. Verify: MD5 hash comparison only (no signature verification on binary)

5. Install: swupdate -i <path> -k /etc/swupdate-public.pem
     (RSA-512 signature — trivially forgeable)

6. Reboot via MCU command
```

**Critical issues:**
- Step 1 uses `curl -k` (disables TLS certificate verification) — MITM can serve malicious JSON
- Step 4 uses MD5 only — MD5 collisions are trivial to produce
- Step 5 uses RSA-512 — factorable on commodity hardware in hours
- `forced` flag allows silent updates without user consent

---

## 11. Audio Processing Pipeline

### Hardware Path

```
8 MEMS Mics → 10-ch ADC (48kHz/32bit) → Amlogic SoC Audio Codec
                                              ↓
                                    TEA SDK (8-stage pipeline)
                                              ↓
                                    ┌─────────┴─────────┐
                                    ↓                   ↓
                              3A Output            ASR Output
                           (48kHz/16bit/2ch)    (32kHz/16bit/1ch)
                                    ↓                   ↓
                              ┌─────┴─────┐      ASR Engine
                              ↓     ↓     ↓    (SenseVoice/
                             USB   BT   Speaker  Zipformer)
                            (UAC) (A2DP) (TAS5707)
```

### TEA SDK Processing Pipeline (8 stages)

| Stage | Filter | Algorithm |
|---|---|---|
| 1 | AEC | Kalman (500ms) + NLMS (180ms) + Neural Echo Cancellation |
| 2 | ARRAY_SE | Microphone array signal enhancement |
| 3 | DOA | Direction of Arrival estimation |
| 4 | NNPOST | Neural network post-processing (Chimera/RAFA models) |
| 5 | DBSS | Direction-Based Source Separation |
| 6 | EXTRACT | Signal extraction |
| 7 | MIXER_AGC | Mixer with Automatic Gain Control |
| 8 | CUSTOM | Custom processing (EQ, reverb) |

### Audio Modes

| Mode | Pipeline | Pattern | Use Case |
|---|---|---|---|
| 1 | Bypass | Noise Reduction + EQ + Reverb | Standard calls |
| 2 | Live | Omnidirectional | Open meeting |
| 3 | Live | Cardioid | Single presenter |
| 4 | Live | Super-cardioid | Distant speaker |
| 5 | Live | Figure-8 | Interview/podcast |
| 6 | Live | Stereo | Music/recording |

### Audio SDK Vendor

TEA SDK by **Roomix** (QCI, Shenzhen-based audio DSP company):
- Version: `3.10.1.1153`
- License: Hardware-bound via encryption chip, RSA + AES verification, kernel module `tea_license.ko`
- Source: `/data/__qci/root-workspaces/`
- Dump path: `/userdata/roomix/dump/`

---

## 12. AI/ML Subsystem

### On-Device Models

| Model | Framework | Format | Purpose |
|---|---|---|---|
| SenseVoice | ONNX Runtime 1.15.1 | ONNX | Chinese offline ASR (Alibaba) |
| Zipformer | ONNX Runtime | ONNX | Chinese online/streaming ASR |
| Reverb-ASR | ONNX Runtime | ONNX | English offline ASR |
| Reverb-ASR Online | ONNX Runtime | ONNX | English streaming ASR |
| DE/FR offline/online | ONNX Runtime | ONNX | German/French ASR |
| JA online | ONNX Runtime | ONNX | Japanese streaming ASR |
| CAM++ | VeriSilicon NPU | `.nb` (FP16) | Speaker diarization |
| Silero VAD v5 | ONNX Runtime | ONNX | Voice Activity Detection |
| CT-Transformer | ONNX Runtime | ONNX | Punctuation restoration (online) |
| PuncMino | ONNX Runtime | ONNX | Punctuation restoration (offline) |
| Chimera | Custom (TEA) | `.bin` | Neural beamforming pre-mask |
| RAFA | Custom (TEA) | `.bin` | Neural noise reduction |
| NNEC | Custom (TEA) | `.bin` | Neural echo cancellation (32kHz) |
| Reverb Mask | Custom (TEA) | `.bin` | Dereverberation mask |
| Beamforming (9 variants) | Custom (TEA) | `.bin` | Cardioid/Super/Figure-8 × High/Low/VAD |

### ASR Framework: WeNet

Open-source components:
- `wenet::AsrDecoder` — Main decoder with rescoring
- `wenet::FeaturePipeline` — Fbank feature extraction
- `wenet::VoiceActivityDetector` — Silero VAD integration
- `wenet::ContextGraph` — Contextual biasing for custom hotwords
- `wenet::CTTransformerOnline` — Online punctuation
- Sentence piece tokenization (`ssentencepiece::Ssentencepiece`)

### Model Paths

```
/model/asr_ch/offline_sensevoice     — Chinese offline
/model/asr_ch/online_zipformer       — Chinese streaming
/model/asr_en/reverb-asr-10s         — English offline
/model/asr_en/reverb_asr_online      — English streaming
/model/asr_de/de_offline             — German offline
/model/asr_de/de_online              — German streaming
/model/asr_fr/fr_offline             — French offline
/model/asr_fr/fr_online              — French streaming
/model/asr_ja/ja_online              — Japanese streaming
/model/vad/silero_vad_v5.onnx        — Voice Activity Detection
/model/diarization/campp_118_cn_en_fp16.nb — Speaker diarization
/model/punc_model/                   — Punctuation model
/usr/bin/tea_models/                 — 15 audio enhancement models
```

### NPU Configuration

At audio init, the NPU frequency is set:
```bash
echo 350000000 > /sys/module/galcore/parameters/nanoqFreq
```
VeriSilicon Vivante NPU running CAM++ speaker diarization in FP16 quantization.

---

## 13. Bluetooth Protocol

### Frame Format

```
Type 1: [0x75] [0x46] [length] [data...] [checksum]
Type 2: [0x75] [0x45] [length] [data...] [checksum]
Type 3: [0xDC] [0xCD] [length] [data...] [checksum]
```

Checksum: Additive sum of all bytes.

State machine: `START(1) → HEADER1(2) → HEADER2(3) → LENGTH(4) → DATA(5) → CHECKSUM(6)`

### Command Set (30+ commands)

| ID | Command | Purpose |
|---|---|---|
| 0x01 | `bt_connect_reply` | Connection response |
| 0x02 | `bt_disconnect` | Disconnect |
| 0x03 | `bt_close_switch` | Close BT |
| 0x04 | `bt_reconnect_switch` | Auto-reconnect toggle |
| 0x07 | `bt_open_switch` | Open BT |
| 0x08 | `bt_switch_mode` | Switch BT mode |
| 0x09 | `bt_get_mac` | Get MAC address |
| 0x0A | `bt_get_name` | Get device name |
| 0x0D | `bt_get_version` | Get BT firmware version |
| 0x0E | `bt_get_status` | Get connection status |
| 0x0F | `bt_repair` | Repair/re-pair |
| 0x11 | `bt_get_phone_book` | Download phone contacts via PBAP |
| 0x12 | `bt_get_pbap_status` | Phone book download status |
| 0x16 | `bt_set_name` | Set device name |
| 0x18-0x1E | Config commands | Various configuration |

### Phone Book Access (PBAP)

The device downloads the **entire phone contact list** via Bluetooth PBAP to `/tmp/PHONE_BOOK.bin`. Contacts are serialized with custom functions: `serialize_contact`, `deserialize_contact`, `find_contact_phone`.

### Dongle Protocol

- 2.4GHz proprietary dongle (separate from BT Classic)
- Pairing: `bt_pairing_24g`, keyword `"insta36024g"`
- OTA upgrade capability over BT
- Tracks: version, MAC, RSSI, frequency, UUID

---

## 14. UI System

### Framework: LVGL (Light and Versatile Graphics Library)

- Screen: 400×304 pixels, 1.82" touchscreen
- 91 screens/pages total
- Font rendering via FreeType

### Screen Map (Selected)

| ID | Name | Purpose |
|---|---|---|
| 10 | `main` | Home screen |
| 12 | `ai_record` | AI Recording |
| 13 | `speaker` | Speaker controls |
| 15 | `PAGE_SET_BLUETOOTH` | Bluetooth settings |
| 17 | `PAGE_SET_PASSWORD` | Device password |
| 44 | `PAGE_SET_WIFI` | WiFi settings |
| 73 | `phone_call` | Incoming call |
| 74 | `phone_calling` | Active call |
| 75 | `PAGE_QRCODE` | Meeting QR code |
| 76 | `PAGE_SUPERVISION` | Supervision mode |
| 77 | `PAGE_AUDIO_MODE` | Audio mode selection |
| 78 | `PAGE_BEAMFORMING` | Beamforming settings (with DOA visualization) |
| **88** | **`PAGE_DEBUG`** | **Hidden debug screen** (4 dump toggles + debug OTA) |
| **89** | **`PAGE_FACTORY_LCD`** | **Factory LCD test** (color cycle + 8×6 touch grid) |
| **90** | **`PAGE_FACTORY_INFO`** | **Factory info display** |

### Localization

20 languages: Chinese (Simplified + Traditional), English, French, German, Spanish, Dutch, Italian, Polish, Portuguese, Russian, Japanese, Korean, Arabic, Hindi, Indonesian, Malay, Thai, Vietnamese, Turkish.

---

## 15. Meeting Recording & Cloud Upload

### SQLite Schema

```sql
CREATE TABLE meeting_projects (
    local_meeting_id, cloud_meeting_id, user_meeting_id,
    qr_code_url, meeting_name, user_name, directory_path,
    language, start_time, end_time, duration, project_type,
    delete_flag, collect_flag, upload_state, is_scaned, is_broken
);

CREATE TABLE project_files (
    meeting_id, file_name, file_hash, upload_flag, file_index, file_type
);

CREATE TABLE project_marks (
    meeting_id, mark_type, duration, screen_shot_path
);

CREATE TABLE speaker_mappings (
    speaker_id, speaker_name
);
```

### Recording Lifecycle

```
init_record → start_record → [pause_record ↔ resume] → finish_record
     ↓              ↓                                        ↓
  Create DB    Start audio capture                    Finalize JSON
  entry        Start ASR                              Upload to cloud
               Write Opus slices                      Trigger summary
               Write asr_audio_slice/*.opus
```

### Storage Layout

```
/mnt/udisk/AIRecord/
  └── <meeting_id>/
      ├── record_info.json          — Meeting metadata
      ├── audio_asr_total.json      — ASR results
      ├── asr_audio_slice/          — Opus audio segments
      │   ├── 0001.opus
      │   ├── 0002.opus
      │   └── ...
      └── screen_shot/              — Timestamp bookmarks
```

### Cloud Upload Flow

1. `cloud_meeting_init` → Get cloud `meetingId`
2. For each file: `cloud_meeting_file_upload` → Get signed URL → Upload
3. `cloud_meeting_file_upload_complete` → Mark done
4. `cloud_meeting_start_transcription` → Trigger cloud ASR
5. `cloud_meeting_dot_notice` → Send telemetry
6. `cloud_meeting_sync_speaker` → Get speaker diarization results

### LLM Integration

Fields `llmInvokeLimit` / `llmInvokeUsed` indicate cloud-side LLM processing:
- Meeting summaries
- Action item extraction
- Key point identification
- Tracked per-user with quota limits tied to VIP membership tier

---

## 16. Security Findings

### Critical Vulnerabilities

| # | Finding | Severity | Details |
|---|---|---|---|
| 1 | **RSA-512 firmware signing** | Critical | SWUpdate public key is RSA-512, factorable since 1999. Attackers can forge firmware updates. |
| 2 | **OTA TLS verification disabled** | Critical | `curl -k` flag disables certificate validation for firmware update JSON. MITM can redirect to malicious firmware. |
| 3 | **MD5-only firmware verification** | Critical | No cryptographic signature on downloaded firmware binary. MD5 collisions are trivial. |
| 4 | **Root telnet with no firewall** | Critical | Telnet (port 23) runs as root. No iptables/nftables rules exist. Network-accessible root shell. |
| 5 | **Unauthenticated recording CGI** | Critical | `/cgi-bin/record/record_ctl.sh` allows anyone on the network to start/stop audio recording. Remote eavesdropping vector. |
| 6 | **Root UART shell** | High | `ttyS0::respawn:-/bin/sh` — no login prompt on serial console. |
| 7 | **Static AES IV** | High | Global IV pointer `PTR_aes_iv_00339fd0` reused for all AES-256-CBC operations. |
| 8 | **AES-128-ECB for PC Link** | High | Hand-rolled ECB mode without IV. Vulnerable to pattern analysis. |
| 9 | **Weak nonce generation** | High | `srand(time(NULL))` + `rand()` for cloud authentication nonces. Predictable. |
| 10 | **WiFi passwords logged** | Medium | `"wifi connect: ssid %s passwd %s"` logged to syslog. |
| 11 | **Secrets logged to syslog** | Medium | Decrypted device secrets, tokens, signatures all logged at INFO level. |
| 12 | **Hardcoded credentials** | Medium | Default AES key, test tokens, WiFi PSK `insta360` for SSID `insta360-guest`. |
| 13 | **Static entropy file** | Medium | `/etc/entropy.bin` identical across all devices. Weakens hostapd WPA key generation. |
| 14 | **CGI command injection** | Medium | CGI scripts pass user input directly to shell commands without sanitization. |
| 15 | **Debug mode via unifykey** | Low | Setting "deviceid" unifykey to "debug" makes entire rootfs writable. |

---

## 17. Privacy Findings

| # | Finding | Impact |
|---|---|---|
| 1 | **35+ telemetry events to ByteDance** | Every boot, shutdown, recording, mute toggle, file operation, BT connection, and physical adjustment is tracked via ByteDance's DataFinder SDK |
| 2 | **Meeting audio uploaded to cloud** | Full audio recordings sent to Insta360 servers for AI processing (ASR, summarization, speaker diarization) |
| 3 | **Phone contacts downloaded** | Entire phone book downloaded via Bluetooth PBAP and stored on device |
| 4 | **Speaker voiceprints collected** | CAM++ model creates speaker embeddings; voiceprint data synced to cloud per meeting |
| 5 | **WiFi passwords in syslog** | WiFi SSIDs and passwords logged in plaintext at DEBUG level |
| 6 | **Device serial as user ID** | Serial number used as unique identifier for ByteDance analytics |
| 7 | **Direction of Arrival data** | DOA angles (which direction speech comes from) can be uploaded |
| 8 | **Spotify credentials in plaintext** | Stored unencrypted in `/var/www/cgi-bin/spotify/spotify.conf` |
| 9 | **Crash dumps to cloud** | Core dumps collected to `/mnt/udisk/coredump/`, logs encrypted and uploaded |
| 10 | **LLM processing** | Meeting content processed by cloud LLM (quota-tracked per user) |

---

## 18. Version Differences

### v1.0.13.6 (Oct 27, 2025) → v1.1.6.6 (Nov 28, 2025)

#### Changelog (official)

1. Cancel recording feature added (once started)
2. Enhanced Cantonese transcription speed and accuracy
3. General improvements and bug fixes

#### Actual Changes Found

**New features:**
- Recording cancel UI: "Discard", "Save Recording", "This recording will not be saved"
- Live captions: local model-powered subtitle generation
- Recording security: password protection for recordings
- Insight Web integration: `https://insight.insta360.com`
- WiFi disconnect warnings for cloud sync and auto-summary

**New libraries:**
- `libopencc.so` (298 KB) — OpenCC for Simplified↔Traditional Chinese conversion
- `libmarisa.so` (154 KB) — MARISA trie for NLP text processing
- `s2t_data/` — Conversion data files (STCharacters, STPhrases, TSCharacters, TSPhrases, s2t.json, t2s.json)

**New cloud endpoints:**
- `https://openapi-fra.insta360.com/summary-service` (Frankfurt/EU)
- `https://openapi-hz.insta360.com/summary-service` (Hangzhou/CN)
- `https://openapi-va.insta360.com/summary-service` (Virginia/US)

**Binary size changes (significant shrinkage = refactoring):**
| Component | v1.0.13.6 | v1.1.6.6 | Delta |
|---|---|---|---|
| `libins_entry.so` | 3.8 MB | 2.4 MB | -38% |
| `libcloud_proto_https.so` | 5.8 MB | 1.6 MB | -72% |
| `liblvgl.so` | 2.7 MB | 1.0 MB | -61% |
| `libui.so` | 3.9 MB | 2.2 MB | -44% |

**UI changes:**
- "Link 2" renamed to "Webcam"
- Removed: `ui_favour_btn.png`, `ui_play_btn.png`, `ui_unfavour_btn.png`, and several navigation icons

**Dongle firmware:** v0.0.0.27 → v0.0.0.28

**Identical between versions:** `dtb.img`, `update.sh`

---

## 19. Open Source Reconstruction Guide

This section documents how to reconstruct an open-source firmware for the Insta360 Wave from scratch, replacing all proprietary components with open alternatives.

### 19.1 Hardware Abstraction

The Insta360 Wave is built on the **Amlogic A5 (AV400)** platform with the board name `AmlSpeaker`. To build a custom firmware, you need:

```
Target:  aarch64 (ARM Cortex-A53)
Kernel:  Linux 5.4.x (Amlogic fork)
Board:   AmlSpeaker / a5_av400
Rootfs:  SquashFS (gzip)
Update:  SWUpdate with A/B partitioning
```

### 19.2 Build System

The original firmware uses **Buildroot 2020.02.1**. A complete reconstruction:

```bash
# 1. Clone Buildroot
git clone https://github.com/buildroot/buildroot.git
cd buildroot
git checkout 2020.02.1

# 2. Create board config
mkdir -p board/insta360/wave
# Copy extracted DTB for reference:
cp /path/to/extracted/dtb.img board/insta360/wave/

# 3. Create defconfig based on Amlogic A5
# Use aarch64 toolchain, kernel 5.4.x, SquashFS rootfs
make aarch64_efi_defconfig  # Starting point
```

### 19.3 Kernel

The kernel is Linux 5.4.242 with Amlogic patches:

```bash
# Amlogic kernel fork (closest match)
git clone https://github.com/khadas/linux.git -b khadas-vims-5.4.y
# Or use the official Amlogic BSP kernel for A5/AV400

# Required modules:
# - galcore.ko (VeriSilicon NPU) — needs Amlogic BSP
# - atbm613x_wifi_sdio.ko — ATBM6031 WiFi driver
# - aml_wifi.ko — Amlogic WiFi wrapper
# - aml_sdio.ko + sdio_bt.ko — Amlogic BT
# - snd-aloop.ko — ALSA loopback
# - ntfs3.ko — NTFS filesystem
```

**Note:** The `tea_license.ko` kernel module is proprietary (Roomix license enforcement). An open replacement would skip this entirely and use open-source audio processing.

### 19.4 U-Boot

The bootloader is U-Boot with Amlogic secure boot. For an open version:

```bash
# Amlogic U-Boot fork
git clone https://github.com/LibreELEC/amlogic-boot-fip.git
# Or use mainline U-Boot with Amlogic A5 support (if available)
```

### 19.5 Component Replacement Map

| Original Component | Open Source Replacement | Notes |
|---|---|---|
| **pspapp** (launcher) | Custom C program | Trivial — just init + signal handlers + sleep loop |
| **libins_entry.so** (core) | Custom C application | Modular server architecture; rewrite each module |
| **libcloud_proto_https.so** | libcurl + cJSON | JSON/HTTPS client; replace with local-only or custom server |
| **libui.so** | LVGL (MIT license) | Already uses open-source LVGL; recreate screens |
| **libInstaAudio.so** | PipeWire/PulseAudio + custom | Audio routing; replace TEA SDK |
| **libTEAProcessSdk.so** | SpeexDSP + WebRTC AEC + custom NN | See audio section below |
| **libasr_psp.so** | WeNet + Sherpa-ONNX | Already uses open WeNet framework |
| **libbva_audio.so** | SpeexDSP + FFTS | Low-level DSP, replaceable |
| **libdatabase.so** | Custom observer pattern | 58 functions, trivial to rewrite |
| **libapplogrs.so** | Remove entirely | ByteDance telemetry — not needed |
| **BusyBox** | BusyBox (GPL) | Already open source |
| **SQLite** | SQLite (public domain) | Already open source |
| **OpenSSL** | OpenSSL / LibreSSL | Already open source |
| **ALSA** | ALSA (GPL) | Already open source |
| **SWUpdate** | SWUpdate (GPL) | Already open source |
| **wpa_supplicant** | wpa_supplicant (BSD) | Already open source |
| **dbus** | dbus (AFL/GPL) | Already open source |
| **FreeType** | FreeType (FTL/GPL) | Already open source |

### 19.6 Audio Pipeline Reconstruction

The most complex component to replace is the TEA SDK audio processing:

```
Open Source Audio Pipeline:

1. AEC (Acoustic Echo Cancellation)
   → WebRTC AEC3: https://webrtc.googlesource.com/src/
   → SpeexDSP: https://github.com/xiph/speexdsp

2. Beamforming
   → BeamformIt: https://github.com/xanguera/BeamformIt
   → Custom MVDR implementation using NumPy/ONNX
   → Use the open-source neural beamforming models if available

3. Noise Suppression
   → RNNoise: https://github.com/xiph/rnnoise
   → DTLN: https://github.com/breizhn/DTLN

4. AGC (Automatic Gain Control)
   → WebRTC AGC
   → SpeexDSP AGC

5. DOA (Direction of Arrival)
   → ODAS: https://github.com/introlab/odas
   → pyroomacoustics: https://github.com/LCAV/pyroomacoustics

6. Dynamic EQ / Reverb
   → SpeexDSP
   → Custom IIR filters
```

### 19.7 ASR Reconstruction

The ASR subsystem already uses open-source frameworks:

```bash
# WeNet (core ASR framework)
git clone https://github.com/wenet-io/wenet.git

# Sherpa-ONNX (lightweight deployment)
git clone https://github.com/k2-fsa/sherpa-onnx.git

# Models (all available open-source):
# - SenseVoice: https://github.com/FunAudioLLM/SenseVoice
# - Zipformer: https://github.com/k2-fsa/icefall (recipes/librispeech)
# - Silero VAD: https://github.com/snakers4/silero-vad
# - CAM++ (speaker diarization): https://github.com/modelscope/3D-Speaker
# - OpenCC (Chinese conversion): https://github.com/BYVoid/OpenCC
# - MARISA trie: https://github.com/s-yata/marisa-trie
# - Sentence piece: https://github.com/google/sentencepiece

# ONNX Runtime (inference)
# Build for aarch64 with NPU acceleration:
git clone https://github.com/microsoft/onnxruntime.git
# Use VeriSilicon NPU EP if Amlogic BSP provides it
```

### 19.8 UI Reconstruction

The UI uses LVGL (MIT license):

```bash
# LVGL graphics library
git clone https://github.com/lvgl/lvgl.git

# Screen layout (400x304):
# Recreate the 91 screens documented in this report
# Use LVGL's SquareLine Studio for visual design

# Font rendering
# FreeType is already open source
# Language files are JSON-based — create your own translations
```

### 19.9 MCU/BT Protocol

The MCU and Bluetooth protocols are custom serial protocols:

```c
// BT Frame format (documented above):
struct bt_frame {
    uint8_t header[2];   // 0x75,0x46 or 0x75,0x45 or 0xDC,0xCD
    uint8_t length;
    uint8_t data[256];
    uint8_t checksum;    // Additive sum
};

// MCU communication via /dev/ttyS1
// MAVLink protocol for PUC camera via /dev/ttyS3
// Both protocols documented via decompilation
```

### 19.10 SWUpdate Package Creation

```bash
# Install SWUpdate tools
apt install swupdate

# Create sw-description
cat > sw-description << 'EOF'
software = {
    version = "1.0.0";
    hardware-compatibility: [ "1.0" ];
    images: (
        { filename = "rootfs.squashfs"; device = "/dev/system"; },
        { filename = "dtb.img"; device = "/dev/dtb"; },
        { filename = "boot.img"; device = "/dev/boot"; }
    );
}
EOF

# Generate RSA-2048 key pair (NOT RSA-512!)
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out swupdate-private.pem
openssl rsa -in swupdate-private.pem -pubout -out swupdate-public.pem

# Sign
openssl dgst -sha256 -sign swupdate-private.pem sw-description > sw-description.sig

# Create CPIO archive
echo sw-description sw-description.sig rootfs.squashfs dtb.img boot.img | \
  tr ' ' '\n' | cpio -ov -H crc > firmware.swu

# Add Insta360 JSON header (optional, for compatibility)
python3 -c "
import json, struct
header = {'json_version': 1, 'data': {'appVersion': {'version': {'en_us': '1.0.0'}}}}
json_bytes = json.dumps(header, indent=2).encode()
padded = json_bytes.ljust(1048576, b'\x00')
with open('firmware_with_header.swu', 'wb') as f:
    f.write(b'!C#\x01')
    f.write(padded)
    with open('firmware.swu', 'rb') as src:
        f.write(src.read())
"
```

### 19.11 Complete Build Script

```bash
#!/bin/bash
# Open Source Insta360 Wave Firmware Build

set -e

BUILDROOT_VER="2024.02"
KERNEL_VER="5.4.242"
LVGL_VER="9.0"
WENET_VER="2.1.0"

# === 1. Buildroot ===
git clone https://github.com/buildroot/buildroot.git -b $BUILDROOT_VER
cd buildroot

# === 2. Configure ===
cat > configs/insta360_wave_defconfig << 'EOF'
BR2_aarch64=y
BR2_TOOLCHAIN_BUILDROOT_GLIBC=y
BR2_LINUX_KERNEL=y
BR2_LINUX_KERNEL_CUSTOM_VERSION=y
BR2_LINUX_KERNEL_CUSTOM_VERSION_VALUE="5.4.242"
BR2_TARGET_ROOTFS_SQUASHFS=y
BR2_TARGET_ROOTFS_SQUASHFS_4_0=y
BR2_PACKAGE_BUSYBOX=y
BR2_PACKAGE_ALSA_LIB=y
BR2_PACKAGE_ALSA_UTILS=y
BR2_PACKAGE_DBUS=y
BR2_PACKAGE_SQLITE=y
BR2_PACKAGE_OPENSSL=y
BR2_PACKAGE_LIBCURL=y
BR2_PACKAGE_CJSON=y
BR2_PACKAGE_ZEROMQ=y
BR2_PACKAGE_WPA_SUPPLICANT=y
BR2_PACKAGE_HOSTAPD=y
BR2_PACKAGE_SWUPDATE=y
BR2_PACKAGE_FREETYPE=y
BR2_PACKAGE_LIBRESPOT=y
EOF

make insta360_wave_defconfig
make -j$(nproc)

# === 3. Add custom packages ===
# LVGL, WeNet, Sherpa-ONNX, RNNoise, WebRTC AEC, ODAS
# Each needs a package .mk file in buildroot

# === 4. Build SWU ===
cd output/images
# ... (see SWUpdate section above)
```

### 19.12 What Cannot Be Open-Sourced

| Component | Reason | Workaround |
|---|---|---|
| **Amlogic BSP kernel patches** | NDA-restricted | Use community Amlogic kernel forks (Khadas, LibreELEC) |
| **galcore.ko (NPU driver)** | Proprietary VeriSilicon | Use CPU inference, or extract from BSP |
| **TEA SDK** | Licensed from Roomix/QCI | Replace with open-source audio pipeline |
| **tea_license.ko** | License enforcement | Skip entirely |
| **BT.bin / MCU.bin / DONGLE.bin** | Black-box firmware | Reuse from original (no open alternative) |
| **u-boot.bin.signed** | Amlogic secure boot chain | Use Amlogic FIP tools or unsigned u-boot |
| **Insta360 cloud APIs** | Proprietary server | Build local alternative or use open APIs |

### 19.13 Recommended Architecture for Open Firmware

```
┌──────────────────────────────────────────────────────┐
│                    Linux 5.4.x                        │
│              (Amlogic A5 / AV400 BSP)                 │
├──────────────────────────────────────────────────────┤
│     BusyBox    │  ALSA  │  WiFi/BT  │  USB Gadget    │
├──────────────────────────────────────────────────────┤
│                                                       │
│  ┌─────────┐ ┌──────────┐ ┌────────┐ ┌────────────┐ │
│  │  LVGL   │ │ PipeWire │ │ WeNet  │ │ Local HTTP │ │
│  │   UI    │ │  Audio   │ │  ASR   │ │   Server   │ │
│  │ (MIT)   │ │  (LGPL)  │ │(Apache)│ │  (custom)  │ │
│  └────┬────┘ └────┬─────┘ └───┬────┘ └─────┬──────┘ │
│       │           │           │             │         │
│  ┌────┴───────────┴───────────┴─────────────┴──────┐ │
│  │           Main Application (custom C)            │ │
│  │  - Audio server (WebRTC AEC + RNNoise + ODAS)   │ │
│  │  - Meeting recorder (Opus + SQLite)              │ │
│  │  - BT manager (serial protocol)                  │ │
│  │  - MCU interface (motor, battery, LEDs)          │ │
│  │  - Local transcription (Sherpa-ONNX)             │ │
│  │  - NO telemetry / NO cloud dependency            │ │
│  └──────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────┘
```

Key differences from stock firmware:
- **No ByteDance telemetry** — remove `libapplogrs.so` entirely
- **No cloud dependency** — all processing local, optional self-hosted sync
- **Proper cryptography** — RSA-2048+ for signing, TLS verification enabled, no hardcoded keys
- **No telnet** — SSH only, with key-based auth
- **Firewall** — iptables/nftables rules by default
- **No plaintext logging of secrets** — sanitize all log output

---

## 20. Appendices

### A. File Listing of Decompiled Sources

```
decompiled/
├── pspapp.c                    (16 KB,  60 functions)
├── libins_entry.so.c           (5.8 MB, 3,882 functions)
├── libcloud_proto_https.so.c   (11 MB,  6,457 functions)
├── libui.so.c                  (2.8 MB, 3,506 functions)
├── libInstaAudio.so.c          (1.2 MB, 1,634 functions)
└── libdatabase.so.c            (29 KB,  58 functions)
```

### B. Detailed Analysis Reports

```
analysis_reports/
├── Init_Scripts_and_Config_Analysis.md
├── libins_entry_Binary_Analysis.md
├── libins_entry_Decompiled_Analysis.md
├── Cloud_and_Audio_Library_Analysis.md
├── Cloud_Protocol_Decompiled_Analysis.md
└── UI_and_Database_Decompiled_Analysis.md
```

### C. Extracted Firmware Trees

```
fw_v1.0.13.6/rootfs/    — Full rootfs for v1.0.13.6 (Oct 2025)
fw_v1.1.6.6/rootfs/     — Full rootfs for v1.1.6.6 (Nov 2025)
```

### D. Tools Used

| Tool | Version | Purpose |
|---|---|---|
| Ghidra | 12.0.4 | Headless decompilation (15,597 functions) |
| unsquashfs | 4.7.5 | SquashFS extraction |
| cpio | macOS builtin | CPIO archive extraction |
| strings | macOS builtin | String extraction |
| rizin | 0.8.2 | Binary analysis |
| Python 3 | 3.x | Header parsing, automation |
| diff | macOS builtin | Version comparison |

### E. Database Keys (87 configuration keys)

Full list of all observable runtime state keys tracked in the in-memory database:

**Audio:** `audio_asr_open/close/pause/restart/enable_reply/only_trans/status_update/txt_update`, `audio_cur_in_path/out_path/mic_mute/mic_vol/spk_mute/spk_vol`, `audio_dump_enable_value/eq_enable_value/eq_mode/reverb_enable_value/work_mode`, `audio_mic_pickup_horizon_angle/pitch_angle/level/quality`

**Meeting/Cloud:** `meeting_server_base/current_meeting_id`, `meeting_server_record_*` (asr_format, model_load, txt_fst/snd, audio_format, duration, language, state, type, url), `meeting_server_benefit_*` (cloudtranscriptionlimit/used, llminvokelimit/used, storagelimit/used, viplevel, vip_expire_time), `meeting_server_qr_code_timeout`

**System:** `system_hw_version/soc_version/build_time/mcu_version/product_name/key`, `system_dev_mode/password/join_improvement/agree_privacy`, `system_storage_free/total/used/full/warn`, `system_bind_flag/area/cs_token/email/key/nickname/phone/userid`

**BT:** `bt_connect_status/work_mode/status/music_volume/phone_volume/volume_max/has_ever_connect/ota_status`

**WiFi:** `wifi_connect_status/ipaddr/mac_addr/signal/switch/country_code`

**MCU:** `mcu_battery_level/voltage/temperature/current/is_charging`, `mcu_motor_state/direction/speed/current`, `mcu_hall_a/b/c/pos`, `mcu_usb_connect/disconnect/state`, `mcu_button_clicked/double_clicked/long_pressed_*`

**Dongle:** `dongle_version/mac/rssi/freq/uuid`

---

*Report generated from firmware analysis of Insta360 Wave (CINSABCA). All findings are based on static analysis of publicly available firmware files. Decompiled code is for research and interoperability purposes.*
