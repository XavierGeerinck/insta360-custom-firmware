I now have comprehensive data on all 6 libraries. Here is the full analysis report.

---

# Comprehensive Binary Analysis Report: Insta360 Wave Speakerphone Libraries

All 6 libraries are **ELF 64-bit LSB shared objects, ARM aarch64, dynamically linked, stripped**.

---

## 1. `libcloud_proto_https.so` (1.6 MB) -- Cloud Communication Protocol

**Source paths reveal:** Built from `/home/jasper/WORK/PSP_1110/SOC/vendor/insta360/pspapp/tools/cloud_proto/`

### Cloud API Endpoints
The library communicates exclusively with `https://api.insta360.com` using these REST API paths:

| Path | Purpose |
|------|---------|
| `/device/secret/register` | Device identity registration |
| `/device/secret/token/refresh` | Auth token refresh |
| `/collect/getCollectUrl` | Get data collection URL |
| `/device/summary/asr/startSummaryWithTranscription` | Start ASR/transcription |
| `/device/summary/asr/task` | ASR task management |
| `/device/summary/qrcode` | QR code for device pairing |
| `/summary/speaker/syncSpeaker` | Speaker diarization sync |
| `/upload/uploadInit` | Initialize file upload |
| `/upload/getUploadSignUrl` | Get signed upload URL |
| `/upload/completeUpload` | Complete upload |
| `/upload/reportFileUploadStatus` | Upload status report |
| `/upload/uploadTaskFinishNotice` | Upload completion notice |
| `/upload/meetingInfo` | Meeting metadata upload |
| `/upload/dotTask` | Analytics dot tracking |
| `/vocabulary/checkVersion` | Custom vocabulary version check |
| `/vocabulary/syncWords` | Vocabulary word sync |

### Authentication & Encryption
- **AES-128-CBC** encryption via `EVP_aes_128_cbc` for device secret/salt decryption
- **HMAC-SHA256** for request signing and key derivation
- **MD5** for file hash calculation (`Content-MD5` header, `fileMd5` field)
- **Base64** encoding/decoding for encrypted payloads
- Hardcoded key: `Insta360_Summary_Device_Security_Key_2024`
- Custom HTTP headers: `X-Device-Token`, `X-Signature`, `X-Nonce`, `X-Timestamp`
- Token management with expiration tracking (`expireTime`, `is_token_expered`)

### Data Sent to Cloud
- `device_id`, `firmwareVersion`, `device_asr` info
- Meeting recordings (audio files uploaded via signed URLs)
- ASR transcription results (`asr_info`)
- Speaker diarization data (`paragraphSpeakerList`, `speakerName`, `speakerOri`)
- Meeting metadata (`meetingId`, `userMeetingId`, `localMeetingId`, `meeting_type`)
- Analytics events via `dotTask` endpoint (`dotDetailList`)
- Transcription state (`userCloudTranscription`, `isTranscriptOnly`)
- Hotword configuration from `/etc/hotword.json`

### Architecture
- C++ class `cloud::CloudProtoHttps` (singleton via `getInstance()`)
- `cloud::CloudUrls` class manages URL construction from `base_url`
- Thread pool for async HTTP requests
- Uses libcurl for HTTP with `Content-Type: application/json`
- nlohmann/json (v3.12.0) for JSON handling

---

## 2. `libInstaAudio.so` (420 KB) -- Audio Processing Orchestrator

**Source path:** Part of the PSP audioserver subsystem

### Exported API Functions
```
Insta360_InitAudio / Insta360_UnInitAudio
Insta360_CreateAudioInPath / Insta360_DestoryAudioInPath
Insta360_CreateAudioOutPath / Insta360_DestoryAudioOutPath
Insta360_OpenAsrFeature / Insta360_CloseAsrFeature / Insta360_PauseAsrFeature
Insta360_RestartAsrFeature / Insta360_CancelAsrFeature
Insta360_SetAsrOnlyTrans / Insta360_SetAsrSpeakerName / Insta360_SwitchAsrType
Insta360_RegisterAsrInfoNotify / Insta360_RegisterAsrOnlineSubtitleNotify
Insta360_RegisterAsrOfflineSubtitleNotify / Insta360_RegisterAsrSDInfoNotify
Insta360_SetMicMute / Insta360_SetMicVolume / Insta360_GetMicMute / Insta360_GetMicVolume
Insta360_SetDoaEnable / Insta360_GetDoaData / Insta360_SetDumpEnable
Insta360_SetAudioMode / Insta360_GetAudioMode
Insta360_OpenAudioFile / Insta360_CloseAudioFile
Insta360_StartPlayToneFile / Insta360_StopPlayToneFile
```

### Audio Hardware Configuration
- **10-channel microphone array**: `dump_mic_in_48k_32bit_10ch.pcm`
- ALSA PCM devices: `pcmC0D0c`, `pcmC0D0p`, `pcmC0D1p`, `pcmC0D3c`, `pcmC1D0c`, `pcmC1D0p`
- Input: 48kHz/32-bit/10ch mic, 48kHz/16-bit/2ch (BT, dongle, UAC, file inputs)
- Output: 48kHz/16-bit/2ch (3A processed), 32kHz/16-bit/1ch (ASR processed)

### Audio Processing Pipeline Classes
- `Insta360_Sigp3A` -- 3A (AEC/ANS/AGC) signal processing
- `Insta360_TeaSdk3AOut` / `Insta360_TeaSdkAsrOut` -- TEA SDK integration
- `Insta360_MicChannelMapProccess` -- Microphone channel mapping
- `Insta360_MicCheckAlgo` -- Mic health checking
- `Insta360_UplinkAsrTransAudioAglo` / `Insta360_DownlinkAsrTransAudioAglo` -- Uplink/downlink ASR
- `Insta360_UplinkAsrPlayAudioAglo` / `Insta360_DownlinkAsrPlayAudioAglo` -- ASR audio playback
- `Insta360_AsrProccessAglo` -- ASR processing algorithm
- `InstaDynamicEq` -- Dynamic equalization

### ASR Model Paths (loaded on-device)
- `/model/asr_ch/offline_sensevoice` -- Chinese offline (SenseVoice)
- `/model/asr_ch/online_zipformer` -- Chinese online (Zipformer)
- `/model/asr_en/reverb-asr-10s` -- English offline
- `/model/asr_en/reverb_asr_online` -- English online
- `/model/asr_de/de_offline` / `de_online` -- German
- `/model/asr_fr/fr_offline` / `fr_online` -- French
- `/model/asr_ja/ja_online` -- Japanese online
- `/model/diarization/campp_118_cn_en_fp16.nb` -- Speaker diarization (CAM++)
- `/model/vad/silero_vad_v5.onnx` -- Voice Activity Detection (Silero v5)
- `/model/punc_mino/` / `/model/punc_model/` -- Punctuation restoration
- `/model/model_version.txt` -- Version tracking

### Configuration Files
- `/etc/audio_config.json` -- Main audio configuration
- `/etc/channel_map_config_default.json` / `_evt1_2.json` -- Channel mapping
- `/etc/dynamic_eq_config.json` -- Dynamic EQ settings
- `/etc/reverb_config.json` -- Reverb parameters
- `/data/common_settings/asr/hotword.list` -- Custom hotwords

### Language Support
- Chinese (with OpenCC for simplified/traditional conversion)
- English, German, French, Japanese, Cantonese (Yue)
- Links to `libopencc.so` for Chinese character conversion

### Debug/Dump Paths
- `/mnt/udisk/audio_dump/` -- PCM dumps for all audio stages
- `/mnt/udisk/asr_dump/` -- ASR audio dumps
- `/tmp/AUDIO_VERBOSE`, `/tmp/DUMP_ASR_TRANS_AUDIO` -- Debug flags
- Recordings to `/.airecord_data/` and `/mnt/udisk/AIRecord/`

---

## 3. `libdatabase.so` (58 KB) -- Key-Value Database / Observer Pattern

**No SQL queries found.** This is NOT a traditional SQL database library. It implements an **in-memory key-value store with an observer/subject notification pattern** for system state management.

### Core API
```
database_init / database_deinit
database_edit_record / database_find
database_update_request
observer_init / observer_find / observer_deinit
observer_ops_update
subject_init / subject_deinit
subject_ops_register_observer / subject_ops_remove_observer
subject_ops_notify_observers_reuqest
status_info_find / viewer_info_find
```

### Database Keys (comprehensive list by category)

**Audio subsystem:** `audio_asr_open/close/pause/restart`, `audio_asr_enable_reply`, `audio_asr_only_trans`, `audio_asr_status_update`, `audio_asr_txt_update`, `audio_cur_in_path`, `audio_cur_out_path`, `audio_cur_mic_mute/vol`, `audio_cur_spk_mute/vol`, `audio_dump_enable_value`, `audio_eq_enable_value`, `audio_eq_mode`, `audio_mic_pickup_horizon_angle/pitch_angle/level/quality`, `audio_reverb_enable_value`, `audio_work_mode`

**Meeting/Cloud:** `meeting_server_base`, `meeting_server_current_meeting_id`, `meeting_server_record_asr_format/model_load/txt_fst/txt_snd`, `meeting_server_record_audio_format/duration/language/state/type/url`, `meeting_server_benefit_cloudtranscriptionlimit/used/llminvokelimit/llminvokeused/storagelimit/storageused/viplevel/vip_expire_time`, `meeting_server_qr_code_timeout`

**Cloud/Binding:** `cloud_server_base`, `cloud_server_salt`, `cloud_server_secret`, `system_bind_flag/area/cs_token/email/key/nickname/phone/userid`, `membership_purcharse_url`

**System:** `system_hw_version`, `system_soc_version/build_time`, `system_mcu_version/build_time`, `system_product_name`, `system_key`, `system_dev_mode/password`, `system_join_improvement`, `system_agree_privacy`, `system_storage_free/total/used/full/warn`

**Bluetooth:** `bt_connect_status`, `bt_work_mode/status`, `bt_music_volume`, `bt_phone_volume/volume_max`, `bt_has_ever_connect`, `bt_ota_status`

**WiFi:** `wifi_connect_status`, `wifi_ipaddr`, `wifi_mac_addr`, `wifi_signal`, `wifi_switch`, `wifi_country_code`

**Hardware/MCU:** `mcu_battery_level/voltage/temperature/current/is_charging`, `mcu_motor_state/direction/speed/current`, `mcu_hall_a/b/c/pos`, `mcu_usb_connect/disconnect/state`, `mcu_button_clicked/double_clicked/long_pressed_*`

**Dongle:** `dongle_version`, `dongle_mac`, `dongle_rssi`, `dongle_freq`, `dongle_uuid`

**Viewers (UI observers):** `AUDIO_VIEWER`, `CLOUD_VIEWER`, `MEETING_VIEWER`, `MCU_VIEWER`, `WIFI_VIEWER`, `OTA_VIEWER`, `GUI_VIEWER`, `SYSTEM_VIEWER`, `TIME_VIEWER`, `SYSDB_VIEWER`

---

## 4. `libasr_psp.so` (401 KB) -- ASR Engine (Automatic Speech Recognition)

**Version string:** `ASR_PSP 0.4.23, 2025.10.27`
**Source paths:** `/root/workspace/asr/asr/api/src/`

### ASR Engine Architecture
Three ASR implementation backends (class hierarchy under `asr::ASRImpl`):
1. **`asr::ReverbASRImpl`** -- English ASR using "Reverb" algorithm (Zipformer-based)
2. **`asr::SenseVoiceASRImpl`** -- Chinese ASR using SenseVoice (Alibaba's model)
3. **`asr::MinorLangASRImpl`** -- German, French, Japanese support

### Core C API
```
ASRCreate(PSPASRConfig) -- Create ASR instance
ASRDestroy -- Destroy instance
ASRSendAudioData -- Feed audio (16kHz 16-bit required, 32ms frames)
ASRInitOfflineThread -- Start offline processing thread
ASRGetOfflineResult -- Get offline ASR results with diarization
Diarization -- Run speaker diarization
ReviseSpeakerDestroy -- Cleanup speaker revision
RegisterLogger -- Register log callback
Version -- Get version string
```

### Speech Processing Pipeline
- **WeNet framework** (`wenet::` namespace) -- Open-source speech recognition toolkit
  - `wenet::AsrDecoder` -- Main decoder with rescoring support
  - `wenet::FeaturePipeline` -- Fbank feature extraction
  - `wenet::VoiceActivityDetector` -- Silero VAD integration
  - `wenet::ContextGraph` -- Contextual biasing for hotwords
  - `wenet::CTTransformerOnline` -- Online CT-Transformer punctuation
  - `wenet::PuncMinoModel` -- Punctuation restoration
- **Linear resampling** (`asr::LinearResample`)
- **Sentence piece tokenization** (`ssentencepiece::Ssentencepiece`)
- **ONNX Runtime** (v1.15.1) for neural model inference
- **OpenCC** for Chinese character conversion
- **Darts double-array trie** for fast dictionary lookup

### Speaker Diarization
- **CAM++ model** (`ONNX_CAMPPlus_118_cn_en.onnx`) for speaker embedding
- Outputs: `Speaker_id`, `Speaker_ori`, `Speaker_prob`, `Recommend_speakers`, `TopK_SD_speakers`
- Speaker identification with enrollment data at `/mnt/udisk/common_settings/asr/identification_dataset`
- Per-meeting speaker data at `/mnt/udisk/common_settings/asr/identification_meeting`

### Supported Languages
Chinese, English, French, German, Japanese, Cantonese -- with both online (streaming) and offline modes

### Linked Libraries
`libfst.so.16`, `libgflags_nothreads.so.2.2`, `libglog.so.0`, `libmarisa.so`, `libnnsdk.so`, `libonnxruntime.so.1.15.1`, `libopencc.so`, `libssentencepiece_core.so`, `libyaml-cpp.so.0.6`, `libasr_reverb.so`

---

## 5. `libTEAProcessSdk.so` (8.5 MB) -- TEA Audio Processing SDK

**Version:** `3.10.1.1153.@TEAProcessSdk@dev/tea_process_x2_sdk_v7u_qoroliang.Roomix-dev`
**Vendor:** **Roomix** (QCI -- appears to be a Shenzhen-based audio DSP company)
**Source paths:** `/data/__qci/root-workspaces/__qci-pipeline-10898588-1/`

### Audio Processing Pipeline Architecture
Multi-threaded filter pipeline with 8 processing stages:

| Thread | Filter | Purpose |
|--------|--------|---------|
| 1 | AEC | Acoustic Echo Cancellation (Kalman + NLMS, 500ms/180ms) |
| 2 | ARRAY_SE | Microphone array signal enhancement |
| 3 | DOA | Direction of Arrival estimation |
| 4 | NNPOST | Neural network post-processing |
| 5 | DBSS | Direction-Based Source Separation |
| 6 | EXTRACT | Signal extraction |
| 7 | MIXER_AGC | Mixer with AGC |
| 8 | CUSTOM | Custom processing |

Additional filter wrappers: PEQ (parametric EQ), WPE (Weighted Prediction Error dereverberation), Pre-AGC

### Neural Network Models (embedded in binary config)
- `insta360_psp_pre_chimera_model.bin` -- Pre-mask Chimera network
- `premask_reverb_best_model-220000_model.bin` -- Dereverberation pre-mask
- `rafa-best_model-11000_model.bin` -- RAFA noise reduction model
- `insta360_az_taer_cardiod_high/low/vad_rotate_model.bin` -- Cardioid beamforming
- `insta360_az_taer_figure8_high/low/vad_rotate_model.bin` -- Figure-8 beamforming
- `insta360_az_taer_supercardiod_high/low/vad_rotate_model.bin` -- Supercardioid beamforming

### Audio Processing Parameters (from embedded JSON config)
- **Input:** 48kHz, 10-channel (6 processed), 32-bit
- **Processing:** 32kHz internal, 1024 FFT, 640 hop, 20ms frames
- **Two pipeline modes:** "preprocess_pipe_live" (mode 0) and "preprocess_pipe_meeting" (mode 1)
- **AEC:** Kalman filter (500ms) + NLMS (180ms), neural echo cancellation (NNEC=3)
- **Beamforming:** MVDR (5 iterations), array processing with 6 mics, azimuth steering
- **WPE dereverberation:** FFT 1024, reverb detection
- **AGC:** Target 0.8, attack/release 0.5s, gain range -1.5 to 15.0 dB
- **Noise suppression:** MMSE-based with neural mask support
- **DOA:** Uploads direction-of-arrival data with configurable interval

### Licensing / DRM System
Multi-version authentication system:
- **V3 Auth** and **V4 Auth** with encryption chip (`/sys/kernel/rkflash/encryption`, `/dev/unifykeys`)
- RSA + AES key generation and verification
- License stored on hardware encryption chip
- Trial mode fallback: "License authorized failed, enable trial-version to mix"
- `RxCryptoAuth` / `RxCryptoAuthV2` classes
- `CryptoDog` module for key production and storage
- Checks firmware version from `/opt/firmware_version`

### DOA (Direction of Arrival) System
- Full azimuth/elevation DOA estimation
- Configurable upload to UART or TEA SDK
- Multiple beamforming modes: cardioid, supercardioid, figure-8
- Zooming angle support with start/end angle configuration

### Storage Paths
- `/userdata/roomix/dump` -- Audio debug dumps
- `/userdata/roomix/log` -- Roomix logs
- `/roomix_setting.json` -- Runtime configuration

---

## 6. `libbva_audio.so` (1.9 MB) -- BVA Audio Processing (Low-Level DSP)

**Source paths:** `/root/workspace/PSP/production_psp_camera/rda/bva/src/nlp/asp/`
**Vendor:** BVA/RDA (appears to be the silicon vendor's audio subsystem)

### Audio Signal Processing (ASP) Modules

**Noise Suppression (`asp_ns`):**
- MMSE-based noise estimation (`asp_ns_imcra`, `asp_ns_min_gain`, `asp_ns_wf_gain`)
- Overlap-add synthesis (`asp_ns_ola`)
- Gain smoothing

**Acoustic Echo Cancellation:**
- Integrated with TEA SDK's AEC module

**Beamforming (`asp_bf`):**
- Microphone array beamforming initialization
- Complex signal processing (convolution, FFT operations)

**Reverberation (`asp_rev`):**
- All-pass filter reverb (`calc_rev_APF`)
- LBCF (Late-arriving reverb) estimation
- Reverb parameters: RT60, decay, gain

**AGC (Automatic Gain Control):**
- Multi-target AGC with knee, attack/release time
- Gain calculation with subtraction method

**EQ (Equalization):**
- FFT-based EQ and IIR-based EQ
- Parametric EQ with configurable filters

**VAD (Voice Activity Detection):**
- Local VAD implementation

**Focus/Ambisonic Processing:**
```
AspFocusInit / AspFocusProcess / AspFocusReset / AspFocusSetAngle / AspFocusUninit
AspAccInitApi / AspAccProcessApi / AspAccResetApi / AspAccUninitApi  (Ambisonic)
AspRevInitApi / AspRevProcessApi / AspRevResetApi / AspRevUninitApi  (Reverb)
```
- Ambisonic decoder: only supports 48kHz
- Crossover filters (2, 3, 4 band)

### Keyword Spotting (KWS)
- **CTC-based keyword spotting** (`CTC_KWS_*` error codes)
- Audio preprocessing for KWS (`asp_kws_preproc_init/process/reset/uninit`)
- MFCC feature extraction
- MNN (Alibaba's Mobile Neural Network) inference backend
- Device-specific implementations (DEVICE_NAME 7 and 9)

### ML/Classifier Framework
- Generic classifier/detector framework with error codes for:
  - Model input/output handling
  - Device buffer management
  - TensorRT plugin support
  - Image format handling (likely for visual features)

### NEON SIMD Optimizations
Extensive ARM NEON intrinsics for:
- `VADDQ_F32`, `VLOG10_F32`, `VMAXVQ_F32`, `VMINVQ_F32`, `VSQRTQ_F32`
- Complex number arithmetic (add, sub, mul, div, conjugate, sqrt)

### HRTF Data
Embedded head-related transfer function coefficients: `h30e54a_l_48k`, `h30e54a_r_48k`, `h30e126a_l_48k`, `h30e126a_r_48k`, `hn30e54a_l_48k`, `hn30e54a_r_48k` -- used for spatial audio rendering at 48kHz.

---

## Cross-Library Summary of Key Findings

### Data Flow to Cloud
1. **Audio recordings** are uploaded via signed URLs to Insta360's cloud (`https://api.insta360.com`)
2. **ASR transcriptions** are sent as part of meeting summaries
3. **Speaker diarization data** (who said what) is synced to cloud
4. **Analytics/telemetry** via `dotTask` endpoint
5. **Meeting metadata** including duration, language, type, QR codes
6. **Device identity** registered with secrets encrypted using a hardcoded 2024 key
7. **Membership/benefit tracking** -- cloud transcription limits, LLM invocation limits, storage usage, VIP level/expiration

### Privacy-Relevant Observations
- The database tracks `system_agree_privacy` and `system_join_improvement` flags
- DOA data (direction of arrival -- which direction speech comes from) can be uploaded
- Speaker identification data is stored locally and synced per meeting
- Meeting recordings can be uploaded to cloud storage
- LLM invocation tracking suggests cloud-side AI processing features
- Device binding tracks email, phone, nickname, and user ID

### Security Architecture
- Device authentication uses AES-128-CBC + HMAC-SHA256 with a hardcoded key seed
- TEA SDK has hardware-bound licensing via encryption chip
- RSA + AES key pair generation for license verification
- Token-based auth with refresh mechanism and expiration
- File integrity via MD5 hashing

### AI/ML Models (all running on-device)
- **ASR:** SenseVoice (Chinese), Zipformer/Reverb (English/multilingual), WeNet framework
- **Speaker Diarization:** CAM++ (Chinese/English)
- **VAD:** Silero VAD v5
- **Punctuation:** CT-Transformer Online, PuncMino
- **Audio Enhancement:** Chimera neural mask, RAFA noise reduction, neural beamforming (TAER models)
- **KWS:** CTC-based keyword spotting with MNN inference
- **Inference engines:** ONNX Runtime 1.15.1, MNN, libnnsdk