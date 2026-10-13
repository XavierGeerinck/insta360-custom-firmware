# Insta360 Wave Speakerphone - Firmware Analysis Report

## 1. libdatabase.so Analysis

**File:** `decompiled/libdatabase.so.c`

### Finding: NO SQL database at all

Despite the name "libdatabase.so", this library contains **zero SQL statements** -- no CREATE TABLE, SELECT, INSERT, UPDATE, or DELETE queries. It is **not a SQLite database layer**. Instead, it implements an **in-memory Observer pattern** for status/configuration management:

**Architecture:**
- `database_init(param_1)` -- allocates a 0x128-byte (296 bytes) record, registers it in a global array (`DAT_0011dc08`). The `param_1` is an integer ID used to look up a `status_info_matcher` entry.
- `database_find(param_1)` -- searches the global array for a record matching the given ID.
- `database_edit_record(param_1)` -- updates a record's data, then notifies observers via the Subject/Observer pattern.
- `database_deinit(param_1)` -- frees records by ID.

**Observer pattern classes:**
- `subject_init/deinit` -- manages a subject with up to 32 observers (0x110 bytes, 0x108 = observer count)
- `observer_init/deinit/find` -- observers register with viewer_info_find() entries
- `subject_ops_notify_observers_reuqest` -- notifies observers by string name match (note: "reuqest" is a typo in the original code)

**Data types supported:** The record stores a type field at offset +4 of the payload. Types 6 and 7 result in `malloc()` for variable-length data (like strings), while other types store values directly as 8-byte integers.

**`get_meeting_language(param_1)`** -- converts an integer index (0-105, i.e., 0x69) to a language code string. The array starts at `PTR_s_zh_CN_0011b080`, meaning index 0 = "zh_CN". The device supports **up to 106 meeting languages**.

### Key insight
This is a **publish-subscribe data store** -- not a persistent database. It holds runtime device state (battery levels, connection status, audio modes, etc.) in memory and pushes changes to registered UI observers. The factory test mode also uses it (database ID 99 is used for factory LCD test state).

---

## 2. libui.so Analysis

**File:** `decompiled/libui.so.c`

### Complete Page/Screen Map (91 screens)

The `ui_page_id_to_string()` function at line 58340 reveals all 91 pages with their numeric IDs:

| ID | Page Name | Category |
|----|-----------|----------|
| 0 | PAGE_CONFIRM | Dialog |
| 1 | PAGE_DOUBLE_ICON_CONFIRM | Dialog |
| 2 | PAGE_REMIND_CONFIRM | Dialog |
| 3 | PAGE_ONLY_CONFIRM_WITH_IMG | Dialog |
| 4 | PAGE_CONFIG_SET | Settings |
| 5 | PAGE_DIALOG_BOX | Dialog |
| 6 | PAGE_DIALOG_BOX_WITH_IMG | Dialog |
| 7 | PAGE_DOUBLE_LONG_BTN | Controls |
| 8 | PAGE_LOADING | System |
| 9 | PAGE_WELCOME | System |
| **10** | **main** | **Home screen** |
| 0xB (11) | PAGE_SET_MENU | Settings menu |
| **0xC (12)** | **ai_record** | **AI Recording** |
| **0xD (13)** | **speaker** | **Speaker page** |
| 0xE (14) | PAGE_SET_SYSTEM | System settings |
| 0xF (15) | PAGE_SET_BLUETOOTH | Bluetooth |
| 0x10 (16) | PAGE_BLUETOOTH_CONNECTING | Bluetooth |
| **0x11 (17)** | **PAGE_SET_PASSWORD** | **Security** |
| **0x12 (18)** | **PAGE_SET_VERFCODE** | **Verification code** |
| 0x13 (19) | PAGE_AIRECORD_FOLD | AI Record list |
| 0x14 (20) | PAGE_AIRECORD_FILE_WRITE | Recording screen |
| 0x15 (21) | PAGE_AIRECORD_FILE_EDIT | File editing |
| 0x16 (22) | PAGE_SET_HEIGHT | Device height |
| 0x17 (23) | PAGE_SET_DISPLAY | Display settings |
| 0x18 (24) | PAGE_SET_USB_MODE | USB mode |
| 0x19 (25) | PAGE_UDISK_EXIT | USB disk |
| 0x1A (26) | PAGE_USB_RECHARG_FAILED | Charging |
| 0x1B (27) | PAGE_USB_RECHARGING | Charging |
| **0x1C (28)** | **microphone** | **Mic settings** |
| 0x1D (29) | PAGE_SET_COMMON | Common settings |
| 0x1E (30) | PAGE_DEV_INFO | Device info |
| 0x1F (31) | PAGE_STORAGE | Storage |
| 0x20 (32) | PAGE_STORAGE_FORMAT_BAR | Format progress |
| 0x21 (33) | PAGE_SET_LANGUAGE | Language |
| 0x22 (34) | PAGE_SET_REGION | Region |
| 0x23 (35) | PAGE_SET_TIMEZONE | Timezone |
| 0x24 (36) | PAGE_SET_TIME | Time |
| 0x25/0x26 | PAGE_SET_TIME_SUB_DATE/TIME | Date/Time sub |
| 0x27 (39) | PAGE_SET_KEYBOARD | Keyboard |
| 0x28 (40) | PAGE_SET_NUMKEYBOARD | Num keyboard |
| 0x29 (41) | PAGE_SET_PUC | PUC (panoramic camera) |
| 0x2A/0x2B | PAGE_SET_PUC_AUTO/MANUAL | PUC modes |
| 0x2C (44) | PAGE_SET_WIFI | WiFi |
| 0x2D (45) | PAGE_SET_WIFI_INFO | WiFi info |
| 0x2E-0x36 | PAGE_SET_OTA_* | 9 OTA update pages |
| 0x37 (55) | PAGE_POWEROFF_WAIT | Power off |
| 0x38 (56) | PAGE_POWEROFF_HEIGHT_TIP | Height tip |
| 0x39/0x3A/0x3B | PAGE_ID_UNBOUND/BOUND/UNBIND_CONFIRM | Device binding |
| 0x3C (60) | PAGE_ALERT_TONE | Alert tone |
| 0x3D-0x40 | PAGE_PRIVACY_POLICY_* | 4 Privacy pages |
| 0x41/0x42 | PAGE_AIRECORD_OPENFILE/CLEAN_SNACKBAR | Snackbars |
| 0x43 (67) | PAGE_INSTALL_GUIDE | Setup guide |
| 0x44-0x48 | PAGE_AI_PRIVILEGE_* | 5 AI privilege/subscription pages |
| **0x49 (73)** | **phone_call** | **Incoming call** |
| **0x4A (74)** | **phone_calling** | **Active call** |
| **0x4B (75)** | **PAGE_QRCODE** | **QR code** |
| **0x4C (76)** | **PAGE_SUPERVISION** | **Supervision mode** |
| **0x4D (77)** | **PAGE_AUDIO_MODE** | **Audio mode selection** |
| **0x4E (78)** | **PAGE_BEAMFORMING** | **Beamforming settings** |
| 0x4F (79) | PAGE_AUDIO_MODE_ALL_INFO | Audio mode detail |
| 0x50 (80) | PAGE_AIRECORD_FILE_SUMMARY | File summary |
| 0x51 (81) | PAGE_AI_ORIGINAL_AUDIO | Original audio |
| 0x52/0x53 | PAGE_SETTING_AIRECORD_LANGUAGE/_MORE | ASR language |
| 0x54 (84) | PAGE_GUIDE | Guide |
| 0x55/0x56 | PAGE_DONGLE_INFO/BIND | Dongle |
| 0x57 (87) | PAGE_BATTERY0_POWERON | Low battery power on |
| **0x58 (88)** | **PAGE_DEBUG** | **Debug screen** |
| **0x59 (89)** | **PAGE_FACTORY_LCD** | **Factory LCD test** |
| **0x5A (90)** | **PAGE_FACTORY_INFO** | **Factory info** |

### Debug Screen (PAGE_DEBUG, ID 0x58)

At line 23552, `ui_debug_screen_on_create()` creates a debug page with:
- **Title:** "Debug"
- **4 toggle switches** (likely labeled "Audio DUMP" based on `s_Audio_DUMP_0030b66c`), controlling debug dump features
- **1 text button** labeled "Software update" that triggers `ota_server_request_debug()` (debug OTA)
- **Back button** navigating to PAGE_DEV_INFO (0x1E)

`ui_debug_screen_on_show()` at line 23602 reads two settings:
- `gui_get_debug_switch()` -- toggles debug/serial output
- `gui_get_factory_mode()` -- toggles factory test mode

### Factory LCD Test (PAGE_FACTORY_LCD, ID 0x59)

At line 14979, the factory test creates a 400x304 canvas and cycles through:
1. **Color display test** -- cycles through 5 colors (`three_color_change` at line 15094), using modulo 5 against a colors array. Sets `is_aging` flag and disables display timeout.
2. **Touch grid test** -- divides the screen into an 8x6 grid (48 zones). Each touch marks a zone green. When all 48 zones are touched (count == 0x30), it logs "LCD touch Check Success!" and navigates to PAGE_FACTORY_INFO.
3. **Aging/USB check mode** -- displays "Aging Remaining Time 01:20:34" label, with a timer countdown driven by database record 99.

### Password Protection System

The UI has extensive device password support:
- `set_first_password()` / `ui_show_first_set_password()` -- initial password setup
- `system_server_request_set_devpassword()` / `_clear_devpassword()` / `_close()` -- server-side password management
- `ui_data_dev_has_password()` / `ui_get_dev_password_flag()` -- password state queries
- `process_ntf_dev_password_reset_verfcode()` -- password reset via verification code
- `process_ntf_dev_bind_request_password_check()` / `_unbind_` -- password validation for device binding/unbinding
- `lv_textarea_set_password_mode()` -- LVGL password input masking
- `ui_set_config_airecord_fold_password_tip()` -- password hint for AI recording folder access
- `track_point_of_password_lock_toggle()` -- analytics tracking for password toggle

### AI Recording UI

The AI recording subsystem is substantial:
- `_ui_btn_airecord_start_cb` -- start recording button callback
- `_ui_airecord_start_from_pc_cb` -- remote start from PC
- `ui_airecord_update_subtitles()` -- real-time subtitle display
- `process_ntf_airecord_meeting_subtitles()` -- subtitle notification handler
- `process_ntf_airecord_meeting_duration()` -- duration tracking
- `ui_airecord_file_summary_enter_ecb()` -- AI-generated meeting summary
- `track_point_of_ai_record_start/pause/end` -- analytics
- `ui_get_airecord_audio_format()` -- audio format selection
- `ui_get_airecord_text_type()` / `ui_get_airecord_type()` -- transcription type selection

The recording can be paused via the double-long-button page (PAGE_DOUBLE_LONG_BTN), and the call-to-file handoff is supported (`ui_set_airecord_call_hangup_to_file_flag`).

### Voiceprint Privacy

There is a dedicated `ui_privacy_policy_voiceprint_screen_create()` that appears before AI recording when voiceprint (speaker identification) features are used. This confirms the device does speaker diarization using voiceprint technology.

---

## 3. libInstaAudio.so Analysis

**File:** `decompiled/libInstaAudio.so.c`

### Audio Server Version

From `Insta360_InitAudio()` at line 22101:
- **Build time:** "2025-11-28 20:10:10"
- **PSP_VERSION:** "CUSTOM"
- At startup, it writes to the GPU: `echo 350000000 > /sys/module/galcore/parameters/nanoqFreq` (sets NPU/GPU frequency to 350MHz, likely for ML inference)

### Audio Processing SDK: "TeaProcessSdk"

The core audio engine is called **"Tea"** (likely "TEA" = some internal acronym). It has:

**Two processing pipe modes** (`SetProcessPipeWorkingMode`):
- Mode 0: **Bypass/standard** processing -- EQ and Reverb are enabled
- Mode 1: **Live pipe** processing -- enables the "LiveEffectMode" beamforming modes

### Audio Modes (7 total, mapped in `setWorkMode()` at line 18712)

```
Mode 1: Pipe=0 (bypass), EQ=enabled, Reverb=enabled    --> "Noise Reduction" / standard mode
Mode 2: Pipe=1, LiveEffect=0                            --> Beamforming: Omnidirectional
Mode 3: Pipe=1, LiveEffect=1                            --> Beamforming: Cardioid
Mode 4: Pipe=1, LiveEffect=2                            --> Beamforming: Super-cardioid  
Mode 5: Pipe=1, LiveEffect=3                            --> Beamforming: Figure-8
Mode 6: Pipe=1, LiveEffect=4                            --> Beamforming: Stereo
```

From the UI code (`ui_change_checkbox_img` at line 20343), the mode names map to:
| Mode | UI String Key | Beamforming Icon | Pattern |
|------|--------------|------------------|---------|
| 1 | `setting_audio_mode_bf_tip` | (noise reduction) | Omnidirectional with NR |
| 2 | `setting_audio_bf_all` | `bf_mode_all.png` | Omnidirectional pickup |
| 3 | `setting_audio_bf_heart` | `bf_mode_heart.png` | Cardioid |
| 4 | `setting_audio_bf_superheart` | `bf_mode_super_heart.png` | Super-cardioid |
| 5 | `setting_audio_bf_eight` | `bf_mode_eight.png` | Figure-of-eight (bidirectional) |
| 6 | `setting_audio_bf_stereo` | `bf_mode_stereo.png` | Stereo |

The UI presents this as a radio button choice between "Noise Reduction" (mode 1) and "Beamforming" (modes 2-6), with a sub-selector for the beamforming pattern. When a "reduce" command is received from the app, it forces mode 1.

### Direction of Arrival (DOA) System

The device has a full DOA implementation for tracking sound direction:
- `Insta360_SetDoaEnable(param)` -- enables/disables DOA tracking
- `Insta360_GetDoaData(horizon_angle, pitch_angle)` -- returns two floats: horizontal and pitch angles
- `TeaProcessSdk::EnableDoaInformationUpload(bool, freq)` -- enables periodic DOA data uploads
- `OnDoaInformationUpdate(float, float)` -- callback for DOA changes
- Debug flag file: `/data/DOA_ENABLE` controls DOA debug output
- The UI has `ui_setting_beamforming_screen_update_doa()` to visualize the DOA on screen

### Automatic Gain Control (AGC) - Multi-band

The AGC implementation (`aspMtagcProcessApiInt16/InitApi/UninitApi`) supports:
- **Uplink AGC** (mic to network) + **Downlink AGC** (network to speaker) -- each with separate dump files:
  - `/uplink_agc_in_dump.pcm`, `/uplink_agc_out_dump.pcm`
  - `/downlink_agc_in_dump.pcm`, `/downlink_agc_out_dump.pcm`
- **Multi-band AGC (MBAGC)** with per-band parameters:
  - `mbagc_target`, `mbagc_max_gain`, `mbagc_gain_knee`, `mbagc_release_time`
  - Crossover frequencies for band splitting
- Single-band AGC parameters:
  - `agc_target`, `agc_max_gain`, `agc_gain_knee`, `agc_release_time`
  - `agc_lt_attack_time`, `agc_lt_release_time` (long-term envelope)

### Dynamic EQ Processing

`Insta360_EqProccess` (singleton) loads from `/etc/dynamic_eq_config.json`:
- Parameters at line 9383 (`insta360_get_dynamic_eq_cfg`):
  - `mic_num`, `sample_rate`, `data_len`
  - `asp_output_ch_num`, `system_output_ch_num`
  - `hpf_switch` (high-pass filter), `iir_eq_switch`, `vad_switch`
  - `rms_thres_lb/ub`, `zcr_thres_lb/ub` (voice activity detection thresholds)
  - `cut_off_freq`, `eq_filter_num`, `eq_setting_num`, `eq_smoothing_time`
  - Per-filter: `eq_type`, `eq_center_freq`, `eq_gain_dB`, `eq_Q`
  - `multi_band_num` with crossover frequencies
  - `device` type flag

### Reverb Processing

`Insta360_ReverbProccess` (singleton) loads from `/etc/reverb_config.json`:
- Parameters: `mic_num`, `sample_rate`, `data_len`
- AGC within reverb chain: `agc_switch`, `agc_target`, `agc_gain`, `agc_release_time`
- Reverb-specific: `rt60` (reverberation time), `d` (delay), `g` (gain), `rev_rate`
- Toggle: `Insta360_SetUplinkReverbEnable()` / `Insta360_GetUplinkReverbEnable()`
- Initialized via `AspRevInitApi`

### AI/ML Model Loading (ASR - Automatic Speech Recognition)

`Insta360_AsrProccessAglo::loadAsrModel(int)` at line 26614 is the core model loader. It:

1. **Validates model version** by reading `/model/model_version.txt` and checking for `"V2.0.0"`
2. **Destroys existing model** via `ASRDestroy()` before loading new one
3. **Loads language-specific models** based on `param_1`:

| ASR Type | Language | Offline Model | Online Model |
|----------|----------|--------------|--------------|
| 0 | Chinese (Simplified) | `/model/asr_ch/offline_sensevoice` | `/model/asr_ch/online_zipformer` |
| 1 | English | `/model/asr_en/reverb-asr-10s` | `/model/asr_en/reverb_asr_online` |
| 2 | German | `/model/asr_de/de_offline` | `/model/asr_de/de_online` |
| 3 | French | `/model/asr_fr/fr_offline` | `/model/asr_fr/fr_online` |
| 4 | Japanese | `/model/asr_ch/offline_sensevoice` | `/model/asr_ja/ja_online` |
| 6 / 0x66 | Chinese (Traditional/Cantonese) | `/model/asr_ch/offline_sensevoice` | `/model/asr_ch/online_zipformer` |
| 0x67 | Chinese (Cantonese variant) | same as above | same as above |

4. **Common models loaded for all languages:**
   - **VAD:** `/model/vad/silero_vad_v5.onnx` (Silero VAD v5, ONNX format)
   - **Speaker diarization:** `/model/diarization/campp_118_cn_en_fp16.nb` (CAM++ model, FP16 quantized, `.nb` = VeriSilicon NPU binary)
   - **Punctuation:** `/model/punc_model/` or `/model/punc_mino/`
   - **Hotword list:** `/data/common_settings/asr/hotword.list`
   - **S2T data:** `/usr/lib/s2t_data`

5. **Model architectures identified:**
   - **SenseVoice** -- Alibaba's open-source multilingual speech recognition model (used for Chinese offline)
   - **Zipformer** -- k2-fsa/icefall's streaming ASR architecture (used for Chinese online)
   - **Reverb-ASR** -- English models named "reverb-asr" suggesting they're trained with reverb augmentation for conference room acoustics
   - **CAM++** (campp_118_cn_en_fp16.nb) -- Speaker diarization model supporting Chinese and English, running on NPU in FP16
   - **Silero VAD v5** -- Voice Activity Detection in ONNX format

6. **Model loading runs in a dedicated thread** (`threadLoopLoadModel`, thread name: "asr_load_model")

### ASR Feature Lifecycle

The complete ASR API:
- `Insta360_OpenAsrFeature(insta_asr_init_param*)` -- opens ASR with type + trans-only flag + output path
- `Insta360_CloseAsrFeature()` -- deinit
- `Insta360_CancelAsrFeature()` -- cancel current recognition
- `Insta360_PauseAsrFeature()` / `Insta360_RestartAsrFeature()` -- pause/resume
- `Insta360_SwitchAsrType(int)` -- switch language at runtime
- `Insta360_SetAsrOnlyTrans(bool)` -- toggle transcription-only mode (no summary?)
- `Insta360_SetAsrSpeakerName(insta_asr_speaker_name_param*)` -- assign speaker names for diarization
- Online/offline subtitle callbacks registered separately:
  - `registerAsrOnlineSubtitleNotify` -- real-time streaming subtitles
  - `registerAsrOfflineSubtitleNotify` -- post-processing subtitles
  - `registerAsrSDInfoNotify` -- speaker diarization info notifications

### Audio Dump / Debug Features

- ASR audio dumps to `/mnt/udisk/asr_dump/` when USB disk is connected
- Dump files: uplink/downlink AGC in/out PCM files
- `Insta360_SetDumpEnable(param)` -- enables full audio pipeline dump
- Environment flag: `/tmp/DUMP_ASR_TRANS_AUDIO` enables ASR transcription audio dump

### Audio Configuration Files

All loaded from `/etc/`:
- `/etc/audio_config.json` -- master config with `aglo_type` (0=bypass, 1=tea, 2=sky), `insta_eq_en`, `insta_eq_pregain`, `insta_reverb_en`
- `/etc/dynamic_eq_config.json` -- EQ filter parameters
- `/etc/reverb_config.json` -- reverb processing parameters
- Channel mapping config (path varies by hardware version)

### Hardware Version Detection

`insta360_get_hw_version()` reads `/etc/hwrevision`. If the value is "3", it returns 1 (true), suggesting hardware revision 3 is a specific variant with different audio configuration.

### License Verification

`Insta360_CheckAlgoLicense()` / `TeaProcessSdk::VerifyLicense()` -- the Tea SDK requires a license check. Failure logs "check license failed" but does not appear to prevent operation (returns -1 but doesn't abort).

### Quality Monitoring

- `Insta360_SetQualityCheckEnable(param)` -- enables speech quality monitoring
- `Insta360_GetQualityVal(float*)` -- returns a quality score
- `TeaProcessSdk::EnableSpeechQualityInformationUpload()` -- uploads quality metrics

### Audio Resampling

`asr::LinearResample::FilterFunc(float)` at line 37001 implements a sinc-based resampling filter, used to convert audio sample rates for the ASR pipeline.