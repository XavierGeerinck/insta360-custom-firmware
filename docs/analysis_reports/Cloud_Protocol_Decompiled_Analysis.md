# Thorough Analysis of `libcloud_proto_https.so` - Insta360 Wave Cloud Protocol Library

**File**: `/Users/xaviergeerinck/Projects/insta360/decompiled/libcloud_proto_https.so.c`
**Size**: 168,649 lines, ~11MB, 6,457 functions
**Source path leaked in binary**: `/home/jasper/WORK/PSP_1110/SOC/vendor/insta360/pspapp/tools/cloud_proto/cloud_proto_https.cpp` (and `cloud_proto_https_c.cpp` for the C wrapper)

---

## 1. ALL HTTP ENDPOINTS CALLED

**Base URL**: `https://api.insta360.com` (hardcoded default, overridable via `base_url` parameter)

The `CloudUrls::updateUrls()` function at line 142879 constructs all 16 endpoints:

### Upload / Meeting File Management
| Endpoint | Purpose |
|---|---|
| `/upload/uploadInit` | Initialize a file upload session |
| `/upload/getUploadSignUrl` | Get a pre-signed upload URL |
| `/upload/reportFileUploadStatus` | Report file upload progress/completion status |
| `/upload/completeUpload` | Mark upload as complete |
| `/upload/uploadTaskFinishNotice` | Notify server that all upload tasks are finished |
| `/upload/dotTask` | Meeting "dot task" telemetry notice |
| `/upload/meetingInfo` | Upload meeting metadata/info |

### Vocabulary / Transcription
| Endpoint | Purpose |
|---|---|
| `/vocabulary/syncWords` | Sync vocabulary words for transcription |
| `/vocabulary/checkVersion` | Check vocabulary version for incremental sync |

### ASR / Summary / Transcription
| Endpoint | Purpose |
|---|---|
| `/device/summary/asr/task` | Submit ASR (Automatic Speech Recognition) task |
| `/device/summary/asr/startSummaryWithTranscription` | Start summary with transcription |
| `/device/summary/qrcode` | Get meeting QR code for sharing |
| `/summary/speaker/syncSpeaker` | Sync speaker identification data |

### Device Authentication
| Endpoint | Purpose |
|---|---|
| `/device/secret/register` | Register device and obtain encrypted device secret + salt |
| `/device/secret/token/refresh` | Refresh device authentication token |

### Data Collection
| Endpoint | Purpose |
|---|---|
| `/collect/getCollectUrl` | Get telemetry/data collection URL |

---

## 2. DATA SENT TO THE CLOUD

### Meeting Initialization (`meetingInit`, line 143968)
The device sends this JSON payload to `/upload/uploadInit`:
```json
{
  "localMeetingId": "<local_meeting_id>",
  "deviceType": "PSP",
  "deviceId": "<device_id>",
  "language": "<language_code>",
  "formatDatetime": "<YYYY-MM-DD HH:MM:SS>",
  "timezone": "<timezone>",
  "isTranscriptOnly": true/false,
  "isSummaryOpen": true/false,
  "startTime": <unix_timestamp>,
  "userCloudTranscription": true/false
}
```
The `device_token` is sent as a header `X-Device-Token`.

### File Uploads (`meetingFileUpload`, line 144449)
Files are uploaded with metadata including:
- `meetingId` (cloud meeting ID)
- `fileType` - an integer index into this array of file types:
  - Index 0: Unknown/default
  - Index 1: **`audio_media`** - recorded audio
  - Index 2: **`audio_play`** - played audio
  - Index 3: **`screen_shot`** - screenshots
  - Index 4: Unknown
  - Index 5: **`device_asr`** - on-device ASR results
  - Index 6: **`meeting_info`** - meeting metadata
- `fileMd5` - MD5 hash of the file
- `status` field for upload status reporting

### Upload Completion (`meetingFileUploadComplete`, line 145062)
```json
{
  "meetingId": "<cloud_meeting_id>",
  "isMeetingEnd": <bool>,
  "isAllFileUploadSuccess": <bool>
}
```

### Meeting Info Notice (`meetingInfoNotice`, line 146717)
Sends `meetingInfo` blob and `meetingId` to the server.

### ASR Notice (`meetingAsrNotice`, line 146917)
Sends `meetingId` plus ASR-related data. Debug logs show `asr_info` content is logged.

### Dot Task Notice (`meetingDotNotice`, line 146488)
Sends telemetry/analytics data tied to `meetingId`.

### Speaker Sync (`meetingSpeakerSync`, line 147153)
Syncs speaker identification data. Response contains `paragraphSpeakerList` with `speakerId` and `speakerName` fields -- indicating the cloud performs speaker diarization and returns identified speakers.

### Vocabulary Sync (`meetingSyncWords`, `meetingSyncAllVocabularyWords`)
Paginates through vocabulary words from the server (`totalPage`, `totalCount`, `list` fields in response). Words are synced to improve transcription accuracy.

### Start Transcription (`meetingStartTranscription`, line 147379)
Sends `meetingId` to start cloud-based transcription.

---

## 3. AUTHENTICATION FLOW

### Phase 1: Device Secret Registration (`deviceSecretRegister`, line 147652)
**Endpoint**: `POST /device/secret/register`
**Request JSON**:
```json
{
  "firmwareVersion": "<version>",
  "deviceId": "<device_id>",
  "nonce": "<random_nonce>"
}
```
**Response**: Server returns encrypted `deviceSecret` and `salt` inside `data` field. These are **AES-128-CBC decrypted** using the nonce as the key derivation input (see Encryption section). The decrypted values are logged to syslog (a security concern).

### Phase 2: Device Token Refresh (`deviceTokenRefresh`, line 148202)
**Endpoint**: `POST /device/secret/token/refresh`
**Required parameters**: `deviceId`, `timestamp`, `nonce`, `oldToken`, `deviceSecret`, `salt`, `base_url`

The flow:
1. Generate HMAC-SHA256 signature from: `deviceId + timestamp + nonce + deviceSecret` (concatenated), keyed with `salt`
2. Construct custom headers:
   - `X-Device-Id: <deviceId>`
   - `X-Timestamp: <timestamp>`
   - `X-Nonce: <nonce>`
   - `X-Signature: <hex_encoded_hmac_sha256>`
3. POST with `{deviceId, oldToken}` body
4. Response contains `newDeviceToken` and `expireTime`

### Ongoing Authentication
All subsequent API calls include:
- `X-Device-Token: <token>` header

When token expires (`is_token_expered` flag), the library sets `X-Check:IGNORE` header and triggers async token refresh via `handleResponseCode`.

---

## 4. DATA STRUCTURES (JSON, not Protobuf)

Despite the library name containing "proto", this library uses **nlohmann/json v3.12.0** (not Protocol Buffers) for all serialization. All API communication is JSON over HTTPS.

### Standard API Response Format
```json
{
  "code": 0,       // 0 = success
  "data": { ... }  // Response payload
}
```

### Meeting Init Response
```json
{
  "code": 0,
  "data": {
    "meetingId": "<cloud_meeting_id>",
    "userMeetingId": "<user_meeting_id>"
  }
}
```

### QR Code Response
```json
{
  "code": 0,
  "data": {
    "url": "<qr_code_url>",
    "status": "<status>",
    "effectiveDuration": <seconds>
  }
}
```

### Token Refresh Response
```json
{
  "code": 0,
  "data": {
    "newDeviceToken": "<token>",
    "expireTime": <seconds>
  }
}
```

### Speaker Sync Response
```json
{
  "data": {
    "paragraphSpeakerList": [
      { "speakerId": "...", "speakerName": "..." }
    ]
  }
}
```

---

## 5. ENCRYPTION

### TLS
All communication uses HTTPS. CURL is configured with:
- `curl_easy_setopt(handle, 0x27fe, "https")` - forces HTTPS protocol
- `curl_easy_setopt(handle, 0x29, 1)` - `CURLOPT_VERBOSE` enabled
- `curl_easy_setopt(handle, 0x34, 1)` - `CURLOPT_FOLLOWLOCATION`

### AES-128-CBC Decryption (line 149064)
Used to decrypt `deviceSecret` and `salt` returned during device registration:
- **Algorithm**: AES-128-CBC via OpenSSL EVP
- **Key derivation**: `HMAC-SHA256(nonce, "key:" + nonce)` truncated to first 16 bytes
- **IV derivation**: `HMAC-SHA256(nonce, "iv:" + nonce)` truncated to first 16 bytes
- Input data is base64-decoded before decryption

### HMAC-SHA256 Signing (line 149442)
Used for token refresh authentication:
- `HMAC-SHA256(salt, deviceId + timestamp + nonce + deviceSecret)`
- Output is hex-encoded (32 bytes = 64 hex chars)

### Security Concern: Sensitive Data Logged to Syslog
The library logs decrypted secrets, tokens, and signatures to syslog at INFO level:
- Line 148022: `"Decrypted device secret: %s"`
- Line 148036: `"Decrypted salt: %s"`
- Line 148468: `"signature: %s"`
- Line 148481: `"device_id: %s"`
- Line 148520: `"device_secret: %s"`
- Line 148533: `"salt: %s"`
- Line 143298: `"Post data: %s"` (full JSON payloads)
- Line 143336: `"Response: %s"` (full server responses)

---

## 6. DEVICE BINDING

The device binding follows a two-step process:

1. **`cloud_device_secret_register`** (C API, line 50714): Takes `firmwareVersion`, `deviceId`, `nonce`, `base_url` and returns a `deviceSecret` + `salt` (AES-encrypted by server, decrypted on device using nonce)

2. **`cloud_device_token_refresh`** (C API, line 50983): Takes `oldToken`, `deviceId`, `deviceSecret`, `salt`, `timestamp`, `nonce`, `base_url` and returns `newDeviceToken` + `expireTime`

The device identifies itself as type `"PSP"` (line 144168 - PSP = "Personal Smart Peripheral", the Insta360 Wave's internal codename). The `deviceId` is the unique identifier binding the device to the cloud account.

---

## 7. PRIVACY IMPLICATIONS

### Meeting Recordings Uploaded
The device uploads multiple types of meeting data to Insta360's cloud:
- **`audio_media`**: The actual meeting audio recording
- **`audio_play`**: Audio playback content
- **`screen_shot`**: Screenshots captured during meetings
- **`device_asr`**: On-device speech recognition results (partial transcriptions)
- **`meeting_info`**: Meeting metadata

### Cloud-Side Processing
- **ASR/Transcription**: The device triggers cloud-based transcription (`startSummaryWithTranscription`), meaning meeting audio is processed on Insta360 servers
- **Speaker Diarization**: The cloud identifies and returns speaker identities (`paragraphSpeakerList` with `speakerId`/`speakerName`)
- **Meeting Summaries**: `isSummaryOpen` parameter controls whether AI-generated meeting summaries are produced

### Language/Locale Tracking
The device sends the meeting language (supported: `zh_CN`, `en_US`, `de_DE`, `fr_FR`, `ja_JP`, `zh_CN_HK`, `zh_TW`, `zh_CN_YUE`) and timezone.

### Telemetry
- **Dot task telemetry** sent for meetings
- **`collectGetCollectUrl`** retrieves a URL for additional data collection
- Full upload progress is tracked: `"Upload progress: %.0f/%.0f bytes (%.1f%%), Speed: %.2f bytes/sec"`

### QR Code Sharing
The device can generate QR codes for meetings with an `effectiveDuration` (expiration), suggesting meeting content can be shared/accessed via QR links.

---

## 8. WEBSOCKET/MQTT CONNECTIONS

**None found.** The library uses only synchronous HTTPS (libcurl) for all communication. There are no WebSocket, MQTT, or other persistent connection mechanisms. All operations are request-response via `httpsClientPost` and `httpsClientGet`.

---

## 9. INTERESTING HARDCODED STRINGS

### Build Environment (leaked paths)
- `/home/jasper/WORK/PSP_1110/SOC/vendor/insta360/pspapp/tools/cloud_proto/cloud_proto_https.cpp`
- `/home/jasper/WORK/PSP_1110/SOC/vendor/insta360/pspapp/tools/cloud_proto/cloud_proto_https_c.cpp`
- Developer name "jasper" and project code "PSP_1110" are embedded

### HTTP Headers
- `User-Agent: PSP_DEVICE` (identifies the device in all requests)
- `Content-Type: application/json`
- `Accept-Encoding: gzip, deflate, br`
- `Cache-Control: no-cache`
- `Connection: keep-alive`
- `X-Check:IGNORE` (sent when token is expired, to bypass server-side validation)

### Device Type Identifier
- `"PSP"` - the device type sent in meeting init

### Base64 Alphabet
- Standard base64: `ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/`

### Timeout Configuration
- `curl_easy_setopt(handle, 0x4e, 0x1e)` - `CURLOPT_TIMEOUT` = 30 seconds
- `curl_easy_setopt(handle, 0xd, 0x1e)` - `CURLOPT_TIMEOUT_MS` = 30 seconds
- Minimum upload speed enforcement is configurable

### Error Code Mapping
- HTTP error codes are converted to "unified codes" with offsets: `HTTP_code + 0x7580910` (line 143303) and curl errors with `curl_error + 0x7583020` (line 143340)

### Singleton Pattern
- `CloudProtoHttps::getInstance()` uses a thread-safe singleton with `__cxa_guard_acquire` at global address `DAT_0029a5f8`
- `CloudUrls::getInstance()` is similarly a singleton
- A thread pool is initialized for async response code handling

### Token Expiration Flag
- `is_token_expered` (note: typo for "expired") at line 148898 - a flag that triggers the `X-Check:IGNORE` bypass header