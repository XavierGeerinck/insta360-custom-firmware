# Insta360 Wave Firmware Analysis: `libins_entry.so`

**File**: `decompiled/libins_entry.so.c`
**Size**: 173,122 lines, ~3,882 functions
**Build path**: `/home/jenkins/workspace/PSP_Releasebuild/575/projectCode/SOC/output/a5_av400_psp_a64_release/build/pspapp-0.0.0/`

---

## 1. CLOUD/NETWORK COMMUNICATION

### API Endpoints Discovered

**OTA/Update servers** (lines 40638-40666):
- Production: `https://openapi.insta360.com/app/v1/common/upgrade/psp_firmware`
- Production: `https://openapi.insta360.com/app/v1/common/upgrade/psp_model`
- Test: `https://openapi-test.insta360.com/app/v1/common/upgrade/psp_firmware`
- Test: `https://openapi-test.insta360.com/app/v1/common/upgrade/psp_model`
- Controlled by `PTR_g_ota_server_env_00339848`: when set to 1, test URLs are used.

**Summary/Cloud service** (lines 117173-117183) -- geo-fenced by country code:
- CN: `https://openapi-hz.insta360.com/summary-service`
- US: `https://openapi-va.insta360.com/summary-service`
- All others (EU default): `https://openapi-fra.insta360.com/summary-service`

**Telemetry/DataFinder** (line 113239):
- Default analytics endpoint: `https://snssdk.insta360.com`

**Password reset endpoints** (lines 154276, 154626):
- `{base_url}/summary/service/device/password/recover/sendcode`
- `{base_url}/summary/service/device/password/recover/verifycode`

### HTTP Client Implementation

The HTTPS client at `https_client_post` (line ~140420) uses libcurl and sets these headers on every request:
```
Content-Type: application/json
User-Agent: PSP_DEVICE
Accept: */*
Cache-Control: no-cache
Accept-Encoding: gzip, deflate, br
Connection: keep-alive
```
Every request also calls `get_signature_headers()` to add cryptographic authentication headers (HMAC-based, see Security section). Timeouts are set to 100 seconds for both connect and transfer.

OTA downloads use raw `curl` shell commands (line 42792):
```
curl -s -vk -H "X-Device-Serial: %s" -H "X-Device-Version: %s" %s -o %s
```
The `-k` flag disables SSL certificate verification for OTA JSON downloads. This is a security issue -- firmware update metadata is fetched without TLS verification.

### Data Sent to Servers

On device registration (`dev_register`, line 98193), the device sends:
- Hardcoded client version string: `"PSP_0.3.19"`
- Device serial number (from UNIFYKEYS slot 0)
- Current Unix timestamp
- Device secret (from database entry 0xe7)

On OTA checks (line 42792), the device sends:
- `X-Device-Serial` header with device serial number
- `X-Device-Version` header with firmware version

---

## 2. AUTHENTICATION & SECURITY

### Device Registration & Token System

**`cloud_device_secret_register`** (line 98193): Registers the device with Insta360's cloud. Sends `PSP_0.3.19`, serial number, timestamp, and a device-specific secret. Receives back a `device_secret` and `salt`, which are stored in both the database (entries 0x11a and 0x11b) and in UNIFYKEYS slots 0xe and 0xf.

**`refresh_device_token`** (line 98263): Refreshes the device's authentication token. Generates a 16-character random nonce (lowercase letters only, seeded with `time()` then `rand()` -- weak randomness). Calls `cloud_device_token_refresh()` with serial number, timestamp, nonce, device area URL, device_secret, salt, and device secret. Stores the resulting token in database entry 0xe8.

**`get_signature_headers`** (line 101329): Creates signed request headers for all cloud API calls. Uses device_id (serial), device_secret, and device_salt. Generates a nonce using `srand(time(NULL))` + `rand()` -- this is cryptographically weak since `rand()` output is predictable.

### AES Encryption

Two separate AES implementations exist:

1. **`aes_encrypt`/`aes_decrypt`** at 0x001ccb60/0x001ccd20 (lines 70376-70465): Uses OpenSSL's AES-256-CBC. Key from `PTR_aes_key_003398f0` (256-bit), IV from `PTR_aes_iv_00339fd0` (global, reused). Both the key and IV are loaded from global pointers at fixed addresses -- meaning they are either hardcoded or set once at init. The IV is copied to a local variable before each use but this is still a static IV, which is a CBC mode vulnerability.

2. **`aesEncrypt`/`aesDecrypt`** at 0x001896d0/0x001899f0 (lines 37082-37248): A custom hand-rolled AES-128 implementation (ECB mode, no IV). Key must be exactly 16 bytes. Used for PC Link protocol communication. This is textbook AES ECB -- no IV, no padding scheme beyond block alignment requirement, vulnerable to pattern analysis.

### Password Verification

`slave_verify_dev_passwd` (line 17677): Handles device password verification over USB PC Link. When password is empty, it sets `SYSTEM_PASWD_VERITY_RESULT` to 0 and triggers a UI verification page. When password is provided, it checks against a stored result. The password verification is delegated to a UI page rather than done cryptographically.

### Log Encryption Key Generation

`generate_log_encrypt_key` (line 116024): Derives an encryption key for uploaded logs by:
1. Reading the device's MAC address (eth0 or wlan0, falling back to `"default-device"`)
2. Appending the current Unix timestamp
3. Computing SHA-256 of the concatenation

This is a weak key derivation scheme -- the MAC address is not secret, and the timestamp is predictable.

### Key Dilution (Obfuscation)

`dilute_encryption_key` (line 116690): A custom steganographic scheme that hides a 32-byte encryption key inside 1024 bytes of random data. For each key byte i, it:
- Computes offset = `i * 0x20 + (i & 7)`
- Places `0xAA` marker at that offset
- Places the key byte at offset+1
- Places `0xAA` marker at offset+2
- Writes magic bytes `0xDEADBEEF 0x2020` at the end (bytes 0x3FA-0x3FF)

`recover_encryption_key` (line 116768) reverses this by checking for the `0xDEADBEEF` magic and extracting key bytes between `0xAA` markers. This is security through obscurity -- the algorithm is fully deterministic and now reverse-engineered.

---

## 3. BLUETOOTH PROTOCOL

### BT Command Parser

`bt_cmd_parser` (line 62334): Implements a state machine for parsing BT serial commands. Protocol frame structure:
- Start bytes: `0x75 0x46` (command type 1), `0x75 0x45` (type 2), or `0xDC 0xCD` (type 3)
- Length byte
- Data payload
- Checksum byte (additive)

The state machine transitions: START(1) -> HEADER1(2) -> HEADER2(3) -> LENGTH(4) -> DATA(5) -> CHECKSUM(6)

### BT Command IDs

Over 30 BT commands identified (lines 67028-67620), sent via `bt_ctrl_cmd_send(cmd_id, data, 0x104, 0)`:
- 1: bt_connect_reply, 2: bt_disconnect, 3: bt_close_switch, 4: bt_reconnect_switch
- 7/8: bt_open_switch/bt_switch_mode, 9/10: bt_get_mac/bt_get_name
- 0xd/0xe: bt_get_version/bt_get_status, 0xf/0x10: bt_repair/bt_get_buildtime
- 0x11/0x12: bt_get_phone_book/bt_get_pbap_status
- 0x16/0x17: bt_set_name/bt_get_buildtime, 0x18-0x1e: config commands

### Phone Book Access (PBAP)

Source path: `src/server/bt/bt_phone_book/bt_phone_book.c` (line 71536)

The device downloads the phone's contact list via Bluetooth PBAP to `/tmp/PHONE_BOOK.bin`. Functions `bt_get_phone_book` and `bt_get_pbap_status` manage this. Contacts are serialized/deserialized via `serialize_contact`, `deserialize_contact`, `serialize_contacts`, `deserialize_contacts`, and stored on the device. Function `find_contact_phone` searches contacts by phone number. This means the device has full access to the paired phone's contact list.

---

## 4. OTA UPDATE MECHANISM

### Update Check Flow

`ota_upgrade_thread` (line 43044): When WiFi is connected:
1. Fetches `psp_firmware.json` from `openapi.insta360.com` (firmware update) 
2. Fetches `psp_model.json` (AI model update)
3. Both fetched via `curl -s -vk` (**SSL verification disabled with -k flag**)
4. Sends device serial number (`X-Device-Serial`) and current version (`X-Device-Version`)

### JSON Parsing

`download_parse_swu_upgrade_json` (line 42750): Parses the JSON response containing:
- `version` (new version string)
- `download_url` (firmware download URL)
- `file_md5` (MD5 checksum of the firmware)
- `descriptions` (localized release notes with `html_note` per language)
- `forced` (force upgrade flag)

### Download & Verification

Firmware is downloaded to `/mnt/udisk/misc/data/wavesystemfile/software.swu` using:
```
curl -C - -o %s %s --progress-bar --write-out '%{size_download}/%{size_total}' > /tmp/curl_progress_%s 2>&1 && echo done > /tmp/swu.done
```

**Verification**: Only MD5 is used (`calculate_md5` at line 41297, which uses OpenSSL's MD5_Init/MD5_Update/MD5_Final). MD5 is cryptographically broken -- no signature verification is present. If an attacker can MITM the connection (possible since `-k` disables TLS verification for the JSON check), they could serve a malicious firmware.

### Installation

After MD5 verification passes, the `soc_upgrade_timer_thread` manages the actual SWU package installation. A forced upgrade mode exists (controlled by `forced` field in JSON) that can push updates without user consent. Battery level is checked before OTA (minimum threshold enforced at line 24661). After installation, the device reboots via `mcu_reboot`.

---

## 5. MEETING RECORDING

### Recording Architecture

The meeting system has a full lifecycle: `meeting_server_request_init_record` -> `meeting_server_request_start_record` -> `meeting_server_request_pause_record` -> `meeting_server_request_finish_record`. Recordings are managed by `record_prj_manager_*` functions backed by SQLite (transactions, tables, rollbacks).

### Cloud Meeting Integration

`cloud_triger_summary` (line 99770): After a recording completes, triggers cloud-side transcription by calling `cloud_meeting_start_transcription` with the cloud meeting ID, device credentials. It retries once with 100ms delay on failure.

### Recording Upload

Files are uploaded via `cloud_meeting_file_upload` and tracked with `cloud_meeting_file_upload_complete`. Upload status is tracked per-file in the project database via `record_prj_manager_update_file_upload_status`.

---

## 6. AI/TRANSCRIPTION INTEGRATION

### ASR (Automatic Speech Recognition)

The ASR engine is accessed via the `Insta360_*AsrFeature` API:
- `Insta360_OpenAsrFeature` -- starts ASR, takes audio server info pointer
- `Insta360_CloseAsrFeature` -- stops ASR
- `Insta360_PauseAsrFeature` / `Insta360_RestartAsrFeature`
- `Insta360_SwitchAsrType` -- switches between ASR modes
- `Insta360_SetAsrOnlyTrans` -- sets transcription-only mode
- `Insta360_RegisterAsrInfoNotify` / `Insta360_RegisterAsrOnlineSubtitleNotify` -- registers callbacks for transcription results

### Transcription Modes

Tracked via telemetry (line 55134), three modes exist:
- `"record_transcribe"` (mode 0) -- record audio + transcribe
- `"only_transcribe"` (mode 1) -- transcribe only, no recording
- `"other_transcribe"` (mode 2) -- other/unspecified

### Cloud Transcription

`cloud_meeting_start_transcription` triggers server-side transcription. `cloud_meeting_asr_notice` sends ASR notifications to the cloud. `cloud_meeting_sync_words` syncs transcription results. The `cloud_meeting_dot_notice` function appears to send progress/status updates.

---

## 7. TELEMETRY/ANALYTICS (DataFinder)

### Telemetry SDK

The device uses ByteDance's "Rangers AppLog" SDK (DataFinder), initialized in `finder_sdk_init` (line 113476):
- **App ID**: `"10000022"` (hardcoded)
- **SDK DB path**: `/data/common_settings/finder/finder_sdk_dir`
- **Custom domain**: Dynamic, based on bind area (defaults to `https://snssdk.insta360.com`)
- **User unique ID**: Set to device serial number
- **Open UDID**: Generated from device MAC address
- BD_DID (ByteDance Device ID) is tracked via observer callback

### Tracked Events

At least **35 distinct telemetry events** are sent to DataFinder via `event_to_finder_buffer()`:

| Event Name | Data Fields | Trigger |
|---|---|---|
| `device_power_on` | serial, fw_ver, language, timezone, storage sizes, time | Every boot (once per boot via `/tmp/power_on_report.txt` flag) |
| `device_power_off` | serial, fw_ver, language, timezone, time, battery, power_off_type | Every shutdown |
| `device_network_connect` | serial, fw_ver, language, timezone, time, connection details | WiFi connect |
| `device_network_disconnect` | serial, fw_ver, language, timezone, time | WiFi disconnect |
| `ai_record_start` | serial, fw_ver, language, timezone, time, transcription details | Recording starts |
| `ai_record_end` | serial, fw_ver, language, timezone, time, duration, file size | Recording ends |
| `ai_record_pause` | serial, fw_ver, language, timezone, time | Recording paused |
| `ai_record_mark` | serial, fw_ver, language, timezone, time, record_id | Bookmark created |
| `firmware_download` | serial, fw_ver, details | OTA download |
| `firmware_install` | serial, fw_ver, details | OTA install |
| `system_crash` | serial, fw_ver, crash details | Crash detected |
| `check_update` | serial, fw_ver | Update check |
| `factory_reset` | serial, fw_ver | Factory reset |
| `init_start` / `init_end` | serial, fw_ver, timing | Boot timing |
| `privacy_agreement` | serial, fw_ver | Privacy agreement interaction |
| `join_user_experience` | serial, fw_ver | User experience program opt-in |
| `set_language_region` | serial, fw_ver, language details | Language changed |
| `toggle_mic_mute` / `toggle_speaker_mute` | serial, fw_ver, state | Mute toggles |
| `prompt_tone_setting` | serial, fw_ver, settings | Prompt tone change |
| `call_answer` / `call_end` | serial, fw_ver | Phone call events |
| `password_lock_toggle` | serial, fw_ver, state | Password lock toggled |
| `bluetooth_connect` | serial, fw_ver, BT details | BT connection |
| `enter_reverse_charging_page` / `reverse_charging_start` / `reverse_charging_end` | serial, fw_ver, charging details | Reverse charging |
| `enter_usb_storage_mode` | serial, fw_ver | USB mode entered |
| `mechanical_height_adjust` | serial, fw_ver | Physical adjustment |
| `transcription_mode_set` | serial, fw_ver, mode_option | ASR mode changed |
| `transcription_language_set` | serial, fw_ver, language | Transcription language changed |
| `bind_id` / `unbind_id` | serial, fw_ver | Device bind/unbind |
| `open_record_file` / `listen_file` / `delete_file` | serial, fw_ver, file details | File operations |
| `upload_cloud_file` / `upload_to_cloud_toggle` | serial, fw_ver | Cloud upload events |

Every telemetry event includes at minimum: `mic_serial_number`, `fw_ver`, `evt_language`, `evt_timezone`, and `evt_time`.

### Crash Detection

On boot, `ins_entry_init` (line 16311) checks for `/tmp/pspapp.pid`. If it exists from a previous session, it concludes the previous run crashed and fires `track_point_of_system_crash("pspapp crash", ..., "pspapp exception crash!")`.

---

## 8. INTERESTING HARDCODED VALUES

### URLs & Endpoints
- `https://snssdk.insta360.com` -- Default telemetry/DataFinder endpoint
- `https://openapi.insta360.com/app/v1/common/upgrade/psp_firmware` -- Production OTA endpoint
- `https://openapi-hz.insta360.com/summary-service` -- China cloud service
- `https://openapi-va.insta360.com/summary-service` -- US cloud service
- `https://openapi-fra.insta360.com/summary-service` -- EU cloud service
- `http://www.download.com/xxyyzz` -- Appears to be a placeholder/test URL (line 19917)

### Identifiers & Constants
- App ID for ByteDance DataFinder SDK: `"10000022"` (line 113551)
- User-Agent: `"PSP_DEVICE"` (line 140492)
- Client version: `"PSP_0.3.19"` (line 98194)
- Firmware SWU path: `/mnt/udisk/misc/data/wavesystemfile/software.swu`
- Phone book storage: `/tmp/PHONE_BOOK.bin`
- PID file: `/tmp/pspapp.pid`
- DataFinder DB: `/data/common_settings/finder/finder_sdk_dir`
- WPA supplicant config: `/tmp/wpa_supplicant.conf`

### Cryptographic Constants
- AES key at global pointer `PTR_aes_key_003398f0` (256-bit for CBC mode, 128-bit for ECB)
- AES IV at global pointer `PTR_aes_iv_00339fd0` (static, reused)
- Key dilution magic bytes: `0xDEADBEEF 0x2020` at offsets 0x3FA-0x3FF

### Security Issues Identified in WiFi Password Logging
Lines 23141-23147 and 23173-23178: WiFi passwords are logged in plaintext to syslog at DEBUG level:
```
"wifi connect: ssid %s passwd %s encry_mode %d"
```
Line 45678 also logs passwords: `"add ssid %s password %s enctype %d"`

---

## SECURITY SUMMARY

**Critical Issues:**
1. **OTA SSL verification disabled** (`curl -vk`): Firmware update metadata fetched without TLS certificate validation
2. **MD5-only firmware verification**: No cryptographic signatures on firmware updates
3. **WiFi passwords logged in plaintext** to syslog
4. **Weak nonce generation**: Uses `srand(time(NULL))` + `rand()` for security-critical nonces in cloud authentication
5. **Static AES IV**: Reused global IV for AES-CBC encryption
6. **Hand-rolled AES-ECB** for PC Link protocol: No IV, vulnerable to pattern analysis
7. **Log encryption key derivation** based on MAC address + timestamp (both predictable)
8. **Key dilution is obscurity, not security**: Fully deterministic algorithm with known magic bytes

**Privacy Concerns:**
1. **35+ telemetry events** sent to ByteDance-operated analytics infrastructure (`snssdk.insta360.com`)
2. **Full phone contact list** downloaded and stored via Bluetooth PBAP
3. **Device serial, firmware version, language, timezone, time** sent with every telemetry event
4. **All user interactions tracked**: mute toggles, recording events, file operations, charging events, physical adjustments
5. Device tokens and secrets logged at INFO/DEBUG level in syslog