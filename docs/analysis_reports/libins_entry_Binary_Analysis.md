# Comprehensive Binary Analysis Report: `libins_entry.so`

**File:** `fw_v1.1.6.6/rootfs/usr/lib/libins_entry.so`

## 1. Binary Metadata

| Property | Value |
|----------|-------|
| Type | ELF 64-bit LSB shared object |
| Architecture | ARM aarch64 |
| Linking | Dynamically linked, stripped |
| Size | 2.2 MB |
| Build System | Jenkins CI (build #575) |
| Source Project | `pspapp-0.0.0` |
| Platform Codename | `a5_av400_psp_a64_release` (Amlogic A5 / AV400 SoC) |
| Unique Strings (8+ chars) | ~6,768 |

**Note:** Rizin, readelf, objdump, and nm were blocked by the sandbox environment. Analysis relies on `file` and `strings` output.

## 2. Library Dependencies

The binary links against:
- `libInstaAudio.so` - Insta360's proprietary audio processing
- `libcloud_proto_https.so` - Cloud protocol over HTTPS
- `libdatabase.so` - Internal database abstraction
- `libui.so` - UI framework
- `libapplogrs.so` - App logging/analytics (Rust-based, likely Bytedance DataFinder SDK)
- `libcjson.so.1` - JSON parsing
- `libsqlite3.so.0` - SQLite database
- `libcurl.so.4` - HTTP client
- `libczmq.so.4` - ZeroMQ (CZMQ bindings) for inter-process messaging
- `libcrypto.so.1.1` / `libssl.so.1.1` - OpenSSL 1.1.x
- `libfreetype.so.6` - Font rendering (for LCD display)
- `libmxml.so.1` - Mini-XML parser
- `libapr-1.so.0` / `libaprutil-1.so.0` - Apache Portable Runtime
- Standard: `libc.so.6`, `libpthread.so.0`, `libz.so.1`

## 3. Software Architecture

The binary is the **main application entry point** (`ins_entry_init` / `ins_entry_deinit`) for the Insta360 Wave speakerphone. It follows a client-server architecture with a centralized event system. Source tree reveals:

```
pspapp-0.0.0/src/
  client/pclink/pc_protocol/ (pc_link.c, pc_mavlink.c, pc_uart.c)
  server/bt/bt_phone_book/
  server/hid/
  test/server_test/{audio,bt,file,machine,mcu,meeting,ota}_test/
  test/util_test/{command,logger,status}_test/
```

### Core Servers (microservice-like components):
| Server | Purpose |
|--------|---------|
| `audio_server` | Audio I/O, mixing, ASR integration, AEC/AGC |
| `bt_server` | Bluetooth Classic + BLE, phone book, dongle comms |
| `wifi_server` | WiFi STA management (wlan0), WPA supplicant control |
| `cloud_server` | Cloud API, file upload, transcription, hotwords sync |
| `meeting_server` | Meeting recording, ASR callbacks, project management |
| `hid_server` | USB HID protocol for PC communication |
| `puc_server` | Pan/Tilt camera unit (PUC) control via MAVLink |
| `mcu_server` | MCU communication (battery, motor, LEDs, sensors) |
| `ota_server` / `ota_wifi_server` | Firmware OTA updates (SWUpdate) |
| `devbind_server` | Device binding, user accounts, membership |
| `file_server` | Recording file management, SQLite-backed project DB |
| `system_server` | System config, USB mode, password, factory reset |
| `time_server` | NTP sync, RTC chip management, timezone |
| `subtitle_server` | Real-time subtitle display from ASR |
| `finder_server` | Analytics/data collection (DataFinder SDK) |
| `logs_upload_server` | Log collection, encryption, chunked upload |
| `dbus_server` | D-Bus IPC for inter-process commands |

### Clients:
- `gui_client` / `pclink_client` - GUI display and PC link protocol

## 4. API Endpoints and Cloud Communication

### Production URLs:
- `https://openapi.insta360.com/app/v1/common/upgrade/psp_firmware` - SOC firmware OTA check
- `https://openapi.insta360.com/app/v1/common/upgrade/psp_model` - AI model OTA check
- `https://snssdk.insta360.com` - Analytics/DataFinder SDK

### Regional Summary Service Endpoints:
- `https://openapi-hz.insta360.com/summary-service` (China - Hangzhou)
- `https://openapi-va.insta360.com/summary-service` (US - Virginia)
- `https://openapi-fra.insta360.com/summary-service` (EU - Frankfurt)

### Test/Staging URLs:
- `https://openapi-test.insta360.com/app/v1/common/upgrade/psp_firmware`
- `https://openapi-test.insta360.com/app/v1/common/upgrade/psp_model`
- `https://service-met-hk-test.insta360.com/summary-service`

### Cloud API Paths:
- `/deviceBinding/getUserInfo` - Get bound user info
- `/deviceBinding/membership` - Get membership/subscription status
- `/deviceBinding/syncWebUnbindInfo` - Sync unbind from web
- `/collect/getCollectUrl` - Get analytics collection URL
- `/summary/service/device/password/recover/sendcode` - Password reset send code
- `/summary/service/device/password/recover/verifycode` - Password reset verify code
- `/firmware/log/upload/{prepare,chunk,chunk/complete,chunk/error,complete,progress,cancel}` - Chunked log upload API

### HTTP Headers:
- `X-Device-Token: %s` - Device authentication token
- `X-Device-Serial: %s` - Device serial number
- `X-Device-Version: %s` - Device firmware version

## 5. Bluetooth Protocol

### BT Architecture:
- Dual-mode: Bluetooth Classic (phone calls, A2DP audio) + 2.4GHz proprietary dongle
- UART-based communication (`/dev/ttyS1` for BT, `/dev/ttyS3` for PUC)
- Custom binary protocol with CRC checks and ACK/NACK

### BT Commands:
- `BT_CMD_BT_DATA_TEST_MODE` / `BT_CMD_BT_START_DATA_FLOW` - Data flow control
- `BT_CMD_GET_DONGLE_VERSION` / `BT_CMD_GET_DONGLE_BUILDTIME`
- `BT_CMD_SET_PTA_MODE` - Packet Traffic Arbitration (BT/WiFi coexistence)
- `BT_CMD_LL_SYNC_DATA_TEST_MODE` - Link Layer sync testing

### BT Features:
- Phone book access via PBAP (`bt_get_phone_book`, `bt_phone_book.c`)
- Call management (`call_in_event_cb`, `call_phone_act_event_cb`)
- BT name management, MAC address retrieval, RSSI monitoring
- Auto-reconnect logic (`bt_reconnect_switch`, `BT_IS_RECONNECT`)
- Dongle OTA upgrade over BT (`bt_upgrade_dongle`, `OTA_DONGLE_*`)
- 2.4GHz pairing: `bt_pairing_24g`, `enter_pairing_24g`, `insta36024g`

### BT Firmware:
- `/usr/lib/BT.bin` - BT module firmware
- `/opt/DONGLE.bin` / `/mnt/udisk/misc/data/DONGLE.bin` - Dongle firmware

## 6. WiFi Configuration

### WiFi Chip: ATBM613x (USB/SDIO dual-band WiFi)
- Driver parameters: `/sys/module/atbm613x_wifi_sdio/parameters/`
- Interface: `wlan0` (STA mode, nl80211 driver)
- WPA supplicant config: `/tmp/wpa_supplicant.conf`
- Band support: 2.4GHz (`PSP_WIFI24G`) and 5GHz (`PSP_WIFI5G`)

### WiFi Operations:
- `wpa_supplicant -Dnl80211 -iwlan0 -c /tmp/wpa_supplicant.conf -B -f /tmp/wifi.log`
- `udhcpc -i wlan0 -b -R -t 100 -A 20 -T 1` - DHCP client
- WAN connectivity check: `run_wifi_wan_ping` -> ping multiple servers
- Auto-connect, scan, remember SSID, forget, country code management
- TX power configuration: `/etc/pspapp/set_rate_power.txt`, `set_rate_power_5g.txt`

### WiFi State Machine:
- States: Not ready -> Scanning -> Connecting -> Connected (LAN) -> WAN OK
- Auto-reconnect with retry logic
- WiFi module reinit on persistent scan failure

## 7. Audio Processing Pipeline

### Audio Server (`libInstaAudio.so` integration):
- **ASR (Automatic Speech Recognition)**: `Insta360_OpenAsrFeature`, `Insta360_CloseAsrFeature`, `Insta360_PauseAsrFeature`, `Insta360_RestartAsrFeature`, `Insta360_CancelAsrFeature`
- **ASR Types**: Online and offline modes (`Insta360_SwitchAsrType`)
- **Subtitle Output**: Online subtitle (`Insta360_RegisterAsrOnlineSubtitleNotify`), offline subtitle, speaker diarization (`Insta360_RegisterAsrSDInfoNotify`)
- **Audio Processing Features**:
  - `Insta360_SetUplinkEqEnable` - Uplink equalizer
  - `Insta360_SetUplinkReverbEnable` - Reverb cancellation
  - `Insta360_SetCheckEnable` - Audio check/monitoring
  - `Insta360_SetDumpEnable` - Audio dump for debugging
- **Hotwords**: Custom vocabulary lists (Chinese/English), version-synced from cloud

### Audio Path Routing:
- Multiple paths: USB (`uac2.0`), Bluetooth, 2.4GHz dongle
- Path selection bitmask: `select_path 0x%x`
- Mic mute/unmute, speaker mute/unmute
- Volume control per-path (BT mic/spk, dongle, USB)
- ALSA integration: `/dev/snd/controlC1`, `snd_ctl_elem_*`
- Audio recording: PCM capture to `/data/insta360_audio_in.pcm`

### Audio Modes:
- Conference/meeting recording
- Phone call (BT)
- ASR-only transcription mode
- USB audio class 2.0 (UAC)

## 8. Meeting/Recording Logic

### Database Schema (SQLite):
Four core tables:
1. **`meeting_projects`**: local_meeting_id, cloud_meeting_id, user_meeting_id, qr_code_url, meeting_name, user_name, directory_path, language, start_time, end_time, duration, project_type, delete_flag, collect_flag, upload_state, is_scaned, is_broken
2. **`project_files`**: meeting_id, file_name, file_hash, upload_flag, file_index, file_type
3. **`project_marks`**: meeting_id, mark_type, duration, screen_shot_path (timestamps/bookmarks)
4. **`speaker_mappings`**: speaker_id, speaker_name (speaker diarization labels)

### Meeting Workflow:
1. Start recording -> create `meeting_projects` entry
2. Audio slices captured as `asr_audio_slice/*.opus`
3. ASR results written to `audio_asr_total.json`
4. Cloud meeting init: creates cloud meeting ID, gets QR code URL
5. ASR slices uploaded to cloud for transcription
6. Meeting end: finalize JSON, update DB
7. Cloud upload: chunked file upload with MD5 verification

### Recording Storage:
- `/mnt/udisk/AIRecord/` - Main recording storage
- `record_info.json` - Per-recording metadata
- `screen_shot/` - Screenshot marks
- File formats: Opus audio, JSON metadata

### Cloud Meeting Features:
- Real-time transcription triggers
- QR code generation for meeting sharing
- Speaker name management (ID -> name mapping)
- Summary trigger (`CLOUD_CMD_TRIGGER_SUMMARY`)
- Upload mode toggle (manual/auto)
- LLM invoke tracking: `llmInvokeLimit` / `llmInvokeUsed`
- Cloud transcription quota: `cloudTranscriptionLimit` / `cloudTranscriptionUsed`

## 9. OTA Update Mechanism

### Multi-Component OTA:
| Component | Package | Device |
|-----------|---------|--------|
| SOC firmware | `software.swu` | SWUpdate-based install |
| AI Model | `software_model.swu` | Written to `/dev/model` |
| BT Module | `BT.bin` | Via BT server UART |
| Dongle | `DONGLE.bin` | Via BT OTA protocol |
| MCU | `MCU.bin` | Via MCU UART |

### OTA Flow:
1. Check version via `https://openapi.insta360.com/app/v1/common/upgrade/psp_firmware`
2. Download SWU package with curl (`curl -C -` for resume)
3. MD5 verification of downloaded package
4. Stuck download detection (30s no-progress timeout, auto-restart)
5. Version comparison: `major.minor.patch.build`
6. Installation via `swupdate -i %s -k /etc/swupdate-public.pem` (signed packages)
7. Model write: `dd if=%s of=/dev/model` with MD5 verification and retry logic
8. Force upgrade support, popup interval management

### OTA Storage:
- Download path: `/mnt/udisk/misc/data/wavesystemfile/`
- Progress tracking: `/tmp/curl_progress_%s`
- Completion marker: `/tmp/swu.done`
- Size limits: Model packages up to 5GB, non-model up to 500MB

## 10. Encryption/Authentication

### Cryptographic Primitives:
- **AES-256-CBC**: `EVP_aes_256_cbc`, `AES_set_encrypt_key`, `AES_set_decrypt_key`, `AES_cbc_encrypt`
- **SHA-256**: `EVP_sha256`
- **MD5**: `MD5_Init`, `MD5_Update`, `MD5_Final` (file integrity)
- **Custom AES**: `aesEncrypt`, `aesDecrypt`, `keyExpansion` (16-byte key, custom implementation for PC link)

### Notable Finding - Hardcoded Key Material:
```
0123456789abcdef0123456789abcdefabcdef9876543210
```
This appears immediately before a "Failed to set encryption key" error, strongly suggesting it is a fallback or default encryption key.

### Token:
```
cstoken123
```
Appears to be a default/test CS token value.

### Log Encryption:
- Key generation: `generate_log_encrypt_key`
- Key dilution: `dilute_encryption_key` - key bytes scattered in random data
- Key appended to encrypted file for recovery
- AES-256-CBC for log file encryption before upload

### PC Link Authentication:
- Session token mechanism (`pc_link_reset_session_token`)
- AES-encrypted communication (`set encry key`, `aes decrypt encryKey`)
- Bind key verification for device pairing

### Firmware Signing:
- SWUpdate with public key verification: `/etc/swupdate-public.pem`

## 11. Device Binding Flow

### Bind Process:
1. App initiates bind via PC Link HID protocol
2. Device receives: `userid`, `key`, `area`, `cs_token`
3. Stored in UNIFYKEYS (hardware-backed secure storage: `/sys/class/unifykeys/`)
4. Keys: `UNIFYKEYS_BIND_FLAG`, `UNIFYKEYS_BIND_USERID`, `SYSTEM_BIND_KEY`, `SYSTEM_BIND_CS_TOKEN`, `SYSTEM_BIND_AREA`
5. Cloud sync: user info, membership status, benefits

### Bind/Unbind Features:
- Password protection for device lock
- Password reset via email verification code
- Cloud unbind sync
- Membership/VIP status tracking (`vipLevel`, `vipExpireTime`)
- Storage quota tracking (`storageLimit`, `storageUsed`)

### UNIFYKEYS (Amlogic Secure Storage):
- Hardware-backed key-value store at `/sys/class/unifykeys/`
- Thread-safe mutex locking with retry (up to 5 retries)
- File-based migration support (`/unifykey/.data_migration_flag`)
- Stores: device SN, bind credentials, device token, device secret

## 12. PUC (Camera Unit) Control

### PUC Communication:
- MAVLink protocol over UART (`/dev/ttyS3`)
- Custom protobuf encoding (`proto_encode_msg`)
- FIFO-based send queue

### PUC Capabilities:
- **Tracking**: `INSPROTO_MSG_PUC_CMD_TRACK` - speaker tracking modes (single/multi)
- **Zoom**: Digital zoom control (`puc_cmd_set_zoom`, `puc_cmd_zoom_add/sub`)
- **PTZ**: Pan-Tilt control (`puc_cmd_set_ptz` with x/y speed)
- **DOA (Direction of Arrival)**: Audio-visual speaker localization (`INSPROTO_MSG_PUC_CMD_DOA_ENABLE`, `puc_cmd_set_doa_angle`)
- **Privacy Mode**: Camera privacy shutter (`puc_cmd_set_privacy`, `puc_cmd_set_ext_privacy`)
- **White Board Mode**: Whiteboard capture/enhancement
- **Desk View Mode**: Overhead document camera
- **UVC Frame**: Get frame count from UVC stream
- Hot-plug detection (insert/remove events via MCU)
- PUC firmware upgrade via OTA

## 13. Hardware / MCU Interface

### MCU Communication:
- UART at `/dev/ttyS1` (`UART_MCU`)
- Custom protocol with ACK mechanism

### Motor Control:
- Motorized height adjustment (aging test for motor positions)
- `mcu_check_puc_position_level`, `mcu_dev_mcu_init_pos`

### Battery Management:
- Battery level, temperature, current, voltage monitoring
- Charger state machine: `g_charger_machine_state_table`
- OTG charging control, USB PD type detection
- Factory mode charge limits (20% enable, 35% disable)
- Reverse charging support (charging other devices)

### LED Control:
- State machine: `g_led_machine_state_table`
- BT power LED: `/sys/class/leds/btpower/brightness`
- LCD backlight: `/sys/class/leds/lcd-backlight/brightness`

### USB Modes:
- Configurable via USB gadget framework (`/sys/kernel/config/usb_gadget/amlogic/`)
- Modes: UAC2.0 + HID, HID + Mass Storage, UAC + HID + Mass Storage
- USB state monitoring: DISCONNECTED -> CONNECTED -> CONFIGURED
- U-Disk mode (mass storage for meeting files)

### I2C:
- RTC chip on `/dev/i2c-3` (BCD time format)
- Sensor access via I2C

### Sensors:
- Temperature: `/sys/class/thermal/thermal_zone0/temp` (CPU/SoC)
- ADC voltage: `/sys/bus/iio/devices/iio:device0/in_voltage{0,3}_input`
- Proximity/position sensor: `mcu_sensor_reset_enable_ctrl`

## 14. Analytics and Telemetry

### DataFinder SDK (Bytedance):
- `AppLog_registerBdDidObserver`, `AppLog_registerSsidObserver`, `AppLog_setLogEnabled`
- SQLite event store: `event_data` table with upload_status tracking
- Batched upload with retry

### Tracked Events (`track_point_of_*`):
Power: `power_on`, `power_off`, `system_crash`
Meeting: `ai_record_start`, `ai_record_end`, `ai_record_pause`, `ai_record_mark`
Network: `device_network_connect`, `device_network_disconnect`
Bluetooth: `bluetooth_connect`
Files: `delete_file`, `open_record_file`, `listen_file`, `upload_cloud_file`
OTA: `check_version_update`, `firmware_download`, `firmware_install`
User: `device_activation`, `bind_id`, `unbind_id`, `factory_reset`, `set_language_region`
Camera: `enter_puc_main`, `mechanical_height_adjust`
Audio: `toggle_mic_mute`, `toggle_speaker_mute`, `prompt_tone_setting`
Privacy: `privacy_agreement`, `join_user_experience`
Transcription: `transcription_language_set`, `transcription_mode_set`

## 15. Inter-Process Communication

### ZeroMQ (CZMQ):
- `tcp://*:5533` - Server socket (likely HID server)
- `tcp://127.0.0.1:5544` - Client socket

### D-Bus:
- `dbus_server_init`, `dbus_server_register_cmd_handler`
- Machine ID: `/var/lib/dbus/machine-id`, `/etc/machine-id`

### HID Protocol:
- Custom reliable transport over USB HID (`/dev/hidg0`)
- Heartbeat mechanism
- Packet loss detection and retransmission
- MAVLink encapsulation within HID payloads

## 16. Configuration System

### Configuration Keys (`scfg_*`):
87 distinct configuration keys covering:
- Audio settings: mic/speaker volume/mute, EQ, reverb, dump, work mode
- Bluetooth: name, connect status, reconnect
- WiFi: config, country code
- Display: brightness, display interval, debug switch
- Meeting: language, ASR format, audio format, record type
- System: privacy agreement, alert tone, init step/done
- OTA: auto-update switch, upgrade size, forbid cancel
- Binding: area, CS token, key, user ID
- Benefits: cloud transcription limits, LLM invoke limits, VIP level, storage

### NTP Servers:
- `pool.ntp.org`
- `ntp.ntsc.ac.cn` (Chinese NTP)
- `ntp2.cstnet.cn` (Chinese NTP)

## 17. Factory/Test Mode

- Factory mode entry: `/data/enter_factory_mode`
- Aging tests: motor position cycling, DDR stress (`/data/aging_result.txt`, `/data/aging_result_ddr.log`)
- Charge test: `/data/charge_test_result.txt`
- Factory-specific settings: timezone forced to Beijing, charge limits 20-35%
- Test suites for all subsystems: audio, BT, file, state machine, MCU, meeting, OTA
- SSH password set: `echo "root:%s" | chpasswd`
- ADB mode force-enable: `HID force enable adb mode`

## Summary of Notable Findings

1. **Hardcoded cryptographic key**: `0123456789abcdef0123456789abcdefabcdef9876543210` is embedded alongside encryption error messages, likely a default/fallback AES key.

2. **Test token in binary**: `cstoken123` appears as a default CS token value.

3. **Log encryption with key appended**: The log upload system encrypts logs with AES-256-CBC but appends a "diluted" (obfuscated) copy of the key to the encrypted file itself, which weakens the encryption significantly since the key can be recovered from the file.

4. **Jenkins build paths leak full source tree structure**: All `__FILE__` macros are compiled with absolute Jenkins paths, revealing the complete project structure.

5. **SSH root password settable via debug**: `echo "root:%s" | chpasswd` enables runtime root password changes.

6. **Comprehensive cloud integration**: The device communicates with Insta360 cloud services across 3 geographic regions for meeting transcription, file upload, hotword sync, device registration, and analytics.

7. **LLM integration**: `llmInvokeLimit` / `llmInvokeUsed` fields indicate the device or its cloud service uses LLM (Large Language Model) processing, tracked per-user with quota limits tied to membership tier.

8. **MAVLink protocol reuse**: The camera unit (PUC) is controlled via MAVLink, a protocol commonly used in drones/UAVs, adapted here for PTZ camera control.