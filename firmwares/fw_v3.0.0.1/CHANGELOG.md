# Firmware changelog: `fw_v1.1.6.6` → `fw_v3.0.0.1`

> The directory name `fw_v3.0.0.1` is the **Wave Controller installer/package**
> version. The on-device firmware version reported by the JSON header is
> **`appVersion: 1.2.0.6`**. This document compares the on-device behaviour
> between v1.1.6.6 and v1.2.0.6.

Generated from byte-level diffing of the extracted rootfs and unified diffs
of decompiled binaries (see `firmwares/fw_v3.0.0.1/decompiled/SUMMARY.md` for
the underlying numeric stats).

## Vendor-supplied release notes (verbatim from JSON header)

> 1. Pre-recording feature added (audio buffered before recording starts, so
>    you never miss a moment).
> 2. Improved English transcription accuracy and reliability.
> 3. QR code activation added for AI features (transcription, summary, and
>    more).
> 4. HD recording mode added for clearer audio quality.

## Architectural changes

### 1. On-device ASR engine (the big one)

A complete on-device ASR (Automatic Speech Recognition) pipeline has been
added, running on the **VeriSilicon VIP9000 Nano-Q NPU** (same `/dev/galcore`
surface that's been there since v1.x).

- New libraries:
  - `libsentencepiece_core.so` (1 MB) — Google's BPE subword tokenizer.
  - `libmodel_infer.so` (2.7 MB, internal version 3.3.0) — model loader /
    runtime shim. Exports a **generic** `model_infer_aes_load_from_{buf,file}`
    + `model_infer_ans_load_from_{buf,file}` plus model-specific convenience
    wrappers for the `vdcrn` / `vaibf` / `vdf2` / `mulch_fusion` model
    families, and classic-DSP `bf_*` (beamforming) + `doa_srp_*` (DoA)
    APIs. The "Navie RNN only support onnx model" string applies to one
    backend only, not the whole library.
  - `libasr_psp.so`: 392 KB → 490 KB (+25 %).
  - `libasr_reverb.so`: 6.5 MB → 8.3 MB (+29 %).
- The new packed firmware container ships an extra 3 GiB payload after the
  SWU body — saved to `extracted/model_package.bin`. Unpacked on-device to
  `/model/asr_en/{en_offline,en_online}` (path strings in `libInstaAudio.so`).
- New source files visible in build paths:
  - `audioserver-0.0.0/Insta360_AsrPlayAudioAglo.cpp`
  - `audioserver-0.0.0/Insta360_AsrProccessAglo.cpp`
  - `audioserver-0.0.0/Insta360_AsrTransAudioAglo.cpp`
  - `audioserver-0.0.0/Insta360_TeaSdkAsrOut.cpp`
  - `audioserver-0.0.0/Insta360_AudioRingBuffer.cpp` (← pre-record buffer)
- New ASR API surface (in `libInstaAudio.so`):
  `ASRCreate / ASRSetHwAndStartOffline / ASRSendAudioData / ASRGetOfflineResult /
  ASRInitOfflineThread / ASRDestroy / ASRReset`.
- ASR state machine:
  `AUDIO_ASR_STATUS_{IDLE, CREATING, CREATED, TRANSING, DIARIZATING,
  FORCE_STOP, DESTROYING, DESTROYED}` — note **`DIARIZATING`**:
  speaker-separation also runs locally.
- NPU is clocked up when ASR runs: `libInstaAudio.so` and `audioserver`
  write `350000000 > /sys/module/galcore/parameters/nanoqFreq` (350 MHz).

See [docs/analysis_reports/NPU_Stack_Inventory.md](../../docs/analysis_reports/NPU_Stack_Inventory.md)
for the full validated inventory of the VIP9000 userspace stack (what's
present, with version evidence and ELF symbol counts).

### 2. MQTT IPC + cloud push channel (mosquitto)

A full Mosquitto MQTT stack is now part of the rootfs.

- New binaries: `mosquitto` (broker, 200 KB), `mosquitto_pub/sub/rr/passwd`.
- New libraries: `libmosquitto.so`, `libmosquittopp.so`, `libjansson.so`
  (JSON for the MQTT payloads).
- `/etc/mosquitto/mosquitto.conf` ships as the **unmodified upstream default**
  — actual broker configuration is pushed at runtime by pspapp / webshell_cli.
- Build paths reveal the new pspapp subsystem:
  `pspapp-0.0.0/src/server/mqtt_client/mqtt_client_server.c` and a new
  `-mqttcli_server` CLI flag.
- Only one rootfs binary directly links libmosquitto besides the broker and
  the CLI tools: **`webshell_cli`**. It is the device-side MQTT bridge to
  the cloud.
- See [docs/analysis_reports/MQTT_Topic_Map.md](../../docs/analysis_reports/MQTT_Topic_Map.md)
  for the full topic / event-name catalog.

### 3. Cloud-protocol rework

- **Endpoint discovery is now runtime**, not hardcoded. New log strings in
  `libcloud_proto_https.so`:
  - `[CLOUD_PROTO] [INFO] User endpoint found: %s`
  - `[CLOUD_PROTO] [INFO] Endpoint buffer too small: need %zu, have %zu`
  - `[CLOUD_PROTO] [INFO] Invalid response format: missing endpoint`
  - `[CLOUD_PROTO] [INFO] Invalid response format: missing deviceToken`
  - `DEVICE_TOKEN_REFRESH`
- New HTTPS endpoints (in `libcloud_proto_https.so`):
  - `/device/abnormalMeeting/list`
  - `/device/abnormalMeeting/reportComplete`
  - `/device/secret/register`
  - `/device/secret/token/refresh`
  - `/device/summary/asr/startSummaryWithTranscription`
  - `/device/summary/asr/task`
  - `/device/summary/qrcode`
  - `/wave/binding/task/create / confirm / cancel / status`
- New internal IPC surface (CLOUD_CMD_*, 11 commands, all new in v3) — see
  [docs/analysis_reports/Cloud_Command_Protocol.md](../../docs/analysis_reports/Cloud_Command_Protocol.md)
  for the full enum + payload schemas.
- Privacy-bypass artifact: writing `/tmp/send_no_transcription.txt` opts a
  single recording out of cloud transcription.

## File-level changes

### Added in v3 (1 055 new rootfs files; highlights only)

| Path | Notes |
|---|---|
| `etc/mosquitto/mosquitto.conf` | Stock upstream default. |
| `etc/pspapp/lvgl/fonts/MiSans-ExtraLight.otf` | Font swap (licensing); replaces `gilroy-ultralight.otf`. |
| `etc/pspapp/lvgl/icon/2B/bin/guide_audio_all/guide_audio_all_NNNNN.bin` (×70+) | LVGL animation frames for the new QR-activation onboarding flow. |
| `usr/bin/ins_lic_tool` | Insta360 license tool (per-device AI feature licensing). |
| `usr/bin/lvgl_ctrl` | LVGL UI control helper. |
| `usr/bin/mosquitto_{pub,sub,rr,passwd}` | MQTT CLI tools. |
| `usr/bin/sigp_sdk_demo` | Signal-processing SDK demo / probe. |
| `usr/lib/libjansson.so{,.4,.4.11.1}` | JSON-C dep of mosquitto. |
| `usr/lib/libmodel_infer.so` | ONNX inference shim. |
| `usr/lib/libmosquitto.so{,.1}` + `libmosquittopp.so{,.1}` | MQTT client libs. |
| `usr/lib/libsentencepiece_core.so` | BPE tokenizer for ASR. |
| `usr/lib/libspai_mic_sigp.so` | Was `liblibspai_mic_sigp.so` in v1.1.6.6 (typo fix, same lib). |
| `usr/sbin/mosquitto` | MQTT broker (200 KB). |

### Removed in v3 (7 files)

| Path | Notes |
|---|---|
| `etc/pspapp/lvgl/fonts/gilroy-ultralight.otf` | Replaced by `MiSans-ExtraLight.otf`. |
| `etc/pspapp/lvgl/icon/msc/bg_airecord.png` | Old AI-record UI bg, replaced by `guide_audio_all/*`. |
| `etc/pspapp/lvgl/icon/privilege/card_dot_{1,2,3}.png` | Privilege/onboarding cards replaced. |
| `usr/lib/liblibmdlfwd.so` | Renamed → `libmodel_infer.so` (functionality reworked). |
| `usr/lib/liblibspai_mic_sigp.so` | Renamed → `libspai_mic_sigp.so` (typo fix). |

### Unchanged or trivially changed

- `dtb.img` — **byte-identical** (no DT/device changes).
- `MCU.bin` — **byte-identical** (no firmware update for the keypad/MCU).
- `boot.img` — 55 bytes different. Kernel image essentially unchanged.
- `BT.bin` — 3 KB larger (minor controller firmware bump).
- `DONGLE.bin` — 44 bytes smaller (minor dongle firmware bump).
- `update.sh` — identical.

## Per-binary decompiled-diff summary

(Generated by `scripts/diff_versions.sh`; see
`decompiled/SUMMARY.md` and the `diff_*.diff` files for full diffs.)

| Binary | v1.1.6.6 | v3.0.0.1 | +lines / −lines |
|---|---:|---:|---|
| `libInstaAudio.so` | 1 235 KB | 2 009 KB | +32 131 / −18 422 |
| `libcloud_proto_https.so` | 11 201 KB | 14 223 KB | +114 735 / −102 302 |
| `libdatabase.so` | 29 KB | 32 KB | +395 / −299 |
| `libins_entry.so` | 5 979 KB | 7 702 KB | +129 947 / −95 959 |
| `libui.so` | 2 834 KB | 3 771 KB | +77 417 / −57 036 |
| `pspapp` | 15 KB | 16 KB | +26 / −26 |

`pspapp` is a tiny shim; the real changes are in the libraries it links.

`libdatabase.so` in v3 is **byte-identical to the v1.0.2.6 business build**'s
decompiled output (33 660 B `.c` file, identical post-grep diff = 41 617 B
in both `business→1.1.6.6` and `1.1.6.6→3.0.0.1` directions) — suggests the
DB layer in v3 was reverted/aligned to the business base rather than the
v1.1.6.6 consumer path.

## Adopt-for-`waved/` recommendations

### Worth porting

- **Pre-record ring buffer** — `Insta360_AudioRingBuffer.cpp` pattern.
  Pure-local UX win, no cloud, no NPU. Drop-in for the existing audio
  pipeline as a small Rust crate (`waved-prebuffer`) wrapping a
  `VecDeque<i16>` sized for N seconds; on Record press, splice the
  prebuffer onto the front of the new file. Fits cleanly with SP#5/SP#6
  audio-thread architecture.
- **`objFFTEQ` vs `objIIREQ` split** in the EQ chain (visible as new
  symbols in `libInstaAudio.so`). Worth checking against our SP#5 EQ —
  if v3 now runs FFT-EQ for playback and IIR for capture, our single-
  implementation EQ may want the same split for latency reasons. See
  [[project_sp5_status]].
- **HD/SD recording-mode flag**. Cheap UX feature; in v3 it toggles
  bitrate/sample-rate on a fixed encoder. One-line addition to
  `waved-config`.

### Worth documenting (decide later)

- **MQTT topic map** — if Wave Controller compatibility is a project goal
  ([[project_goals]]), the v3 controller almost certainly talks to the
  device over MQTT. Documented current understanding in
  [docs/analysis_reports/MQTT_Topic_Map.md](../../docs/analysis_reports/MQTT_Topic_Map.md);
  remaining unknowns require capturing live device traffic.
- **Cloud endpoint-discovery handshake** — endpoint URLs now come from a
  discovery RPC + refreshable device token. Response schema (`userEndPoint`,
  `deviceToken`, `confirmInfo`) is documented from log strings so the
  open-firmware HTTPS proxy / null backend can mimic the shape.
- **Speaker-mapping CRUD** — local-only data model. Captured in
  [docs/analysis_reports/Cloud_Command_Protocol.md](../../docs/analysis_reports/Cloud_Command_Protocol.md).

### Skip — against project goals

- **On-device ASR + model package.** Adds 3 GiB of opaque NPU blobs;
  depends on the closed VIP9000Nano-S toolchain ([[project_sp4_npu_status]]
  still blocked). Leave transcription to whatever the user runs server-side.
- **`ins_lic_tool` / QR activation.** The whole point of open firmware
  is no per-device licensing barrier.

## Reproducing this report

```bash
scripts/decompile_version.sh fw_v1.1.6.6
scripts/decompile_version.sh fw_v3.0.0.1
scripts/diff_versions.sh
# Walk the resulting firmwares/fw_v3.0.0.1/decompiled/SUMMARY.md +
# diff_*.diff files; cross-reference rootfs file-tree diffs:
diff <(cd firmwares/fw_v1.1.6.6/extracted/rootfs && find . -type f -o -type l | sort) \
     <(cd firmwares/fw_v3.0.0.1/extracted/rootfs && find . -type f -o -type l | sort)
```
