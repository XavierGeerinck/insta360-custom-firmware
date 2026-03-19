# Design: `waved` — Open Source Firmware for Insta360 Wave

**Date:** 2026-03-19
**Status:** Approved
**Device:** Insta360 Wave Conference Speakerphone (CINSABCA)
**Language:** Rust (core) + C FFI shims (hardware interfaces)
**Approach:** Staged Migration (Phase 1 MVP → Phase 2 Open Audio → Phase 3 Full Open)

---

## 1. Problem Statement

The Insta360 Wave ships with firmware that:
- Sends 35+ telemetry event types to ByteDance's DataFinder SDK (`snssdk.insta360.com`)
- Uploads meeting recordings to Insta360 cloud servers (geo-routed: CN/US/EU)
- Downloads phone contacts via Bluetooth PBAP without clear consent
- Has critical security vulnerabilities (RSA-512 signing, telnet as root, hardcoded passwords, disabled TLS verification)
- Cannot be configured to use a custom API endpoint for recordings

## 2. Goals

1. **Remove all telemetry** — no ByteDance DataFinder SDK, no Insta360 analytics
2. **Own your recordings** — upload raw audio to a user-configured HTTP API endpoint
3. **Maintain audio quality** — keep TEA SDK audio pipeline (AEC, beamforming, noise suppression) in Phase 1
4. **Keep hardware features** — USB audio (UAC2), Bluetooth, WiFi, motorized column, touchscreen UI, PUC camera
5. **USB speakerphone in macOS** — UAC2 + HID mute/volume buttons work natively (no Insta360 controller app needed)
6. **Safe and reversible** — stock firmware can always be restored

## 3. Non-Goals

- Insta360 Wave Controller app compatibility (replaced by on-device UI + own API)
- On-device ASR/transcription (offloaded to user's server)
- Insta360 account binding / VIP membership features
- Spotify Connect
- Phone book download (PBAP)

## 4. Constraints

| Constraint | Details |
|---|---|
| **Black-box firmware** | BT.bin, MCU.bin, DONGLE.bin cannot be replaced — must reuse stock |
| **TEA SDK (Phase 1)** | Proprietary audio library, license-enforced via `tea_license.ko` — used as-is via FFI |
| **Amlogic BSP kernel** | NDA-restricted patches — use stock kernel in Phase 1 |
| **Display** | 400×304 px, 1.82" touchscreen, LVGL framework |
| **Storage** | 32GB eMMC: SquashFS rootfs (RO), ext4 `/data` (RW), exFAT `/mnt/udisk` (RW) |
| **Model partition** | 3GB `/dev/model` — can be wiped (non-fatal to boot), see recovery guide |

---

## 5. Architecture

### 5.1 High-Level

A single Rust binary `waved` replaces `pspapp` + `libins_entry.so` + `libcloud_proto_https.so` + `libapplogrs.so`. It runs on the stock Linux 5.4.242 kernel with stock drivers.

```
┌──────────────────────────────────────────────────────────────┐
│                    Stock Linux 5.4.242                         │
│              (Amlogic A5 / AV400, unchanged)                  │
├──────────────────────────────────────────────────────────────┤
│  Stock: galcore.ko, atbm613x_wifi, aml_wifi, ALSA, USB       │
├──────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌──────────────────────────────────────────────────────┐    │
│  │              waved (Rust binary)                       │    │
│  │                                                       │    │
│  │  audio_srv ─── bt_srv ─── meeting_srv ─── puc_srv    │    │
│  │  mcu_srv ──── wifi_srv ── ui_srv ──────── hid_srv    │    │
│  │  system_srv ─ config_srv ─ uart_mux                   │    │
│  │                                                       │    │
│  │  State Bus: tokio::sync::watch (state)                │    │
│  │           + tokio::sync::broadcast (events)           │    │
│  └──────────────────────────────────────────────────────┘    │
│                                                               │
│  KEPT: libInstaAudio.so, libTEAProcessSdk.so, libbva_audio   │
│  KEPT: liblvgl.so, libfreetype.so, libsqlite3.so             │
│  REMOVED: libapplogrs.so, libcloud_proto_https.so             │
│  REMOVED: pspapp, libins_entry.so                             │
└──────────────────────────────────────────────────────────────┘
```

### 5.2 Modules

#### `uart_mux` — Shared UART Multiplexer
- Owns exclusive access to `/dev/ttyS1` (115200 baud)
- Single reader thread parses incoming bytes into frames by header type
- Dispatch: `0x75 0x46` and `0x75 0x45` frames → `bt_srv` channel, `0xDC 0xCD` frames → `mcu_srv` channel
- Single writer with priority queue (MCU commands like motor-stop take priority over BT)
- Frame format: `[header: 2B] [length: 1B] [data: NB] [checksum: 1B]`, checksum = additive sum of all bytes
- Runs on a dedicated OS thread (not tokio), communicates via `crossbeam` channels

#### `audio_srv` — Audio Pipeline Control
- FFI to `libInstaAudio.so` for TEA SDK pipeline control
- **Threading model:** All TEA SDK FFI calls happen on a dedicated OS thread (not a tokio worker) via `tokio::task::spawn_blocking`. TEA SDK internally manages 8 threads across 3 CPU cores — `waved` does not manage these.
- **Initialization dependency:** `tea_license.ko` must be loaded before `audio_srv` starts (handled by stock init script `S50_ko_install` which runs before `S60waved`)
- **Callback bridging:** TEA SDK delivers DOA data and audio frames via C callbacks. These callbacks push into `tokio::sync::mpsc` channels, which async tasks consume. The callback functions are registered during `Insta360_InitAudio()` and must be `extern "C"` with `#[no_mangle]`.
- Functions: `Insta360_InitAudio()`, `setWorkMode()`, `SetDumpEnable()`
- 6 audio modes: Noise Reduction, Omni, Cardioid, Supercardioid, Figure-8, Stereo
- **Audio routing:** TEA SDK writes processed output internally to ALSA devices (`hw:0,0` for speaker, `hw:1,0` for USB UAC2). `audio_srv` controls which outputs are active but does not write PCM frames to ALSA directly — TEA SDK handles the isochronous USB audio timing.
- Audio frame delivery to `meeting_srv` for recording (via the 3A output callback)
- Optional: load tuned TEA config from `/data/waved/tea_config.json` to fix latency issues (lower `ec_aggr` to 0.8, increase `agc_alpha` to 0.3, decrease `agc_attack` to 0.1)

#### `bt_srv` — Bluetooth Manager
- Receives parsed frames from `uart_mux` via channel, sends frames back through `uart_mux` writer
- Commands: connect/disconnect, open/close, get MAC/name/status/version, auto-reconnect, phone call HFP, volume, set name
- Dongle: 2.4GHz pairing (`"insta36024g"` keyword), version, OTA
- Excluded: PBAP phone book download (privacy)

#### `mcu_srv` — Hardware Control
- Receives parsed frames from `uart_mux` via channel, sends frames back through `uart_mux` writer
- Battery: level, voltage, temperature, current, charging state
- Motor: extend/retract column (low/medium/high presets), obstruction detection, auto-lift
- LEDs: status indicators
- Buttons: mute (single/double/long press), USB connect/disconnect events
- Power: shutdown, reboot commands
- **Low battery behavior:** at <10% auto-save any active recording, retract motor, show warning. At <5% auto-shutdown via MCU command.

#### `meeting_srv` — Recording & Upload
- Capture 3A output frames from `audio_srv` callback channel (48kHz/16bit/2ch stereo)
- Encode to Opus via `opus` crate (48kHz, stereo, 64kbps VBR, 20ms frames)
- Write to `/mnt/udisk/recordings/<uuid>/` (audio.opus + metadata.json + marks.json)
- Upload via HTTP POST (multipart, `reqwest`) to user-configured API endpoint
- Offline queue: store pending uploads in SQLite, flush on WiFi reconnect
- Retry: exponential backoff, max 5 attempts. After 5 failures, recording stays in "upload_failed" state. UI shows failed uploads with manual retry button. Recordings are always accessible via USB file transfer regardless of upload status.
- SQLite DB on `/data/waved/meetings.db` for metadata + upload status

#### `puc_srv` — PTZ Camera Control
- MAVLink protocol on `/dev/ttyS3`
- Auto-framing: receive DOA angles from `audio_srv`, steer camera to active speaker
- Manual PTZ: up/down/left/right/zoom commands from UI
- Smart whiteboard detection toggle
- Reset to home position

#### `wifi_srv` — Network Management
- `wpa_supplicant` CLI wrapper for scan/connect/disconnect/forget
- Connection state monitoring
- NTP time sync on connect (servers: `pool.ntp.org`, `time.google.com` — no Chinese NTP)
- IP/signal strength reporting to UI and state bus

#### `ui_srv` — Touchscreen UI
- LVGL via FFI on 400×304 framebuffer (`/dev/fb0`)
- **Threading model:** LVGL is single-threaded. `ui_srv` runs the LVGL event loop on a dedicated OS thread. All UI updates from other modules are marshaled through a command channel (`tokio::sync::mpsc`). The UI thread polls both the LVGL timer and the command channel.
- **LVGL strategy:** Bundle LVGL 9.x from source (compiled for aarch64) rather than using the stock `liblvgl.so` (unknown version, ABI risk). LVGL 9 is MIT-licensed and has stable Rust bindings via `lvgl-rs`.
- **Localization:** JSON string table (`/data/waved/lang/<code>.json`) loaded at startup. All UI strings referenced by key, never hardcoded. Ship with `en.json`; additional languages can be added as JSON files.
- ~17 screens (down from 91):

| Screen | Purpose |
|---|---|
| Home | Connection status, mic/speaker mute, record button |
| Recording | Timer, pause/stop, bookmark |
| Files | List recordings, delete, upload status |
| Audio Mode | Noise reduction / beamforming pattern selection |
| WiFi | Scan, connect, status |
| Bluetooth | Pair, connect, status |
| Settings | Language, display, height, volume, about |
| Volume | Speaker + mic sliders |
| Height | Motor control (low/med/high) |
| Camera | Auto-frame toggle, manual/auto mode |
| Camera PTZ | D-pad + zoom controls |
| API Config | Endpoint URL, auth token (keyboard input) |
| About | FW version, serial, IP, storage |
| Password | Optional device lock |
| OTA | Custom OTA from user's server |
| Power Off | Confirmation + motor retract |
| Loading | Generic spinner |

- Status bar: battery (11 levels), WiFi signal, BT, mic, speaker, USB
- Toast notifications for connection events
- Language support: start with English, expandable

#### `hid_srv` — USB HID
- Write HID reports to `/dev/hidg0`
- Map physical mute button → HID Telephone mute report (macOS recognizes natively)
- Volume up/down → HID Consumer volume reports
- Uses existing USB gadget HID descriptor (Telephone + Consumer usage pages)

#### `system_srv` — System Management
- USB gadget mode switching (speakerphone / file transfer)
- Custom OTA: download SWU from user's server, verify SHA-256 + RSA-2048 signature, install via `swupdate`
- Factory reset: wipe `/data/waved/`, keep stock rootfs

#### `config_srv` — Configuration
- Load/save `/data/waved/config.toml`
- Watch for changes, notify modules via state bus

Config schema:
```toml
[device]
name = "Wave Open"
language = "en"

[audio]
mode = "noise_reduction"
speaker_volume = 150
mic_volume = 200

[api]
endpoint = "https://your-api.example.com/meetings/upload"
auth_token = "your-bearer-token"
auto_upload = true
retry_max = 5
retry_backoff_ms = 1000

[security]
device_password = ""
ota_server = "https://your-ota.example.com/firmware"

[ntp]
servers = ["pool.ntp.org", "time.google.com"]
```

### 5.3 State Bus

Replaces the stock `libdatabase.so` (in-memory observer pattern with 296-byte records) with two channel types:

- **`tokio::sync::watch`** for continuous state (battery level, WiFi status, audio mode) — latest value only, no missed updates matter
- **`tokio::sync::broadcast`** for discrete events (button presses, USB connect/disconnect, recording state changes) — consumers must not miss these

```rust
pub struct StateBus {
    // Continuous state (watch: latest value, multiple consumers)
    pub audio: watch::Sender<AudioState>,
    pub battery: watch::Sender<BatteryState>,
    pub bt: watch::Sender<BluetoothState>,
    pub wifi: watch::Sender<WifiState>,
    pub meeting: watch::Sender<MeetingState>,
    pub mcu: watch::Sender<McuState>,
    pub puc: watch::Sender<PucState>,

    // Discrete events (broadcast: all consumers receive all events)
    pub events: broadcast::Sender<DeviceEvent>,
}

pub enum DeviceEvent {
    ButtonPress(ButtonType),      // mute, volume_up, volume_down
    UsbConnected,
    UsbDisconnected,
    BtConnected(String),          // device name
    BtDisconnected,
    WifiConnected(String),        // SSID
    WifiDisconnected,
    RecordingStarted(Uuid),
    RecordingStopped(Uuid),
    LowBattery(u8),               // percentage
    MotorObstruction,
}
```

### 5.4 Inter-Module Communication

| From | To | Mechanism | Data |
|---|---|---|---|
| `audio_srv` | `meeting_srv` | Bounded channel | Audio frames (48kHz/16bit/2ch) |
| `audio_srv` | `puc_srv` | State bus | DOA angles (horizontal, pitch) |
| `mcu_srv` | `hid_srv` | State bus | Button events (mute, volume) |
| `mcu_srv` | `ui_srv` | State bus | Battery level, USB state |
| `bt_srv` | `ui_srv` | State bus | Connection status, device name |
| `wifi_srv` | `meeting_srv` | State bus | Connection state (triggers upload flush) |
| `config_srv` | All | State bus | Config changes |
| `ui_srv` | All | Command channel | User actions (start record, change mode, etc.) |

---

## 6. Deployment

### 6.1 Build

Cross-compile on host (macOS/Linux) targeting `aarch64-unknown-linux-gnu` using `cross` or Cargo with Buildroot toolchain.

### 6.2 Installation (Phase 1 — overlay)

Stock rootfs SquashFS is never modified. All changes live on `/data/` overlay.

1. Enable ADB via USB HID packet (see `GUIDE_DEVICE_ACCESS.md`)
2. `killall pspapp pspapp_daemon pspapp_watchdog`
3. `adb push waved /data/waved/waved`
4. `adb push config.toml /data/waved/config.toml`
5. `adb push S60waved /data/overlay/upper/etc/init.d/S60waved` (init script with watchdog respawn loop)
6. Delete telemetry library: overlay removes `libapplogrs.so`
7. Install firewall rules: `adb push iptables.rules /data/waved/iptables.rules`
8. `reboot`

### 6.3 Process Supervision

The `S60waved` init script implements a simple watchdog:
```bash
#!/bin/sh
while true; do
    /data/waved/waved 2>&1 | logger -t waved
    echo "waved exited ($?), restarting in 3s..." | logger -t waved
    sleep 3
done &
```

`waved` handles `SIGTERM` for graceful shutdown:
1. Save any active recording
2. Retract motor column to safe position
3. Flush pending log writes
4. Exit cleanly

### 6.4 Firewall

Deployed as part of installation. Rules applied at boot by `S60waved` before starting `waved`:
```
# Default: drop all inbound
iptables -P INPUT DROP
iptables -P FORWARD DROP
iptables -P OUTPUT ACCEPT

# Allow loopback
iptables -A INPUT -i lo -j ACCEPT

# Allow established connections
iptables -A INPUT -m state --state ESTABLISHED,RELATED -j ACCEPT

# Allow DHCP (WiFi client)
iptables -A INPUT -p udp --sport 67 --dport 68 -j ACCEPT

# Allow mDNS (device discovery, optional)
iptables -A INPUT -p udp --dport 5353 -j ACCEPT

# Drop everything else (no telnet, no CGI, no ZeroMQ)
```

### 6.5 Revert to Stock

```bash
rm -rf /data/overlay/upper/etc/init.d/S60waved
rm -rf /data/overlay/upper/usr/lib/libapplogrs.so
rm -rf /data/waved/
sync && reboot
# Stock firmware boots unchanged
```

### 6.6 Auth Token Security

The API auth token in `config.toml` is stored with restrictive file permissions (`chmod 0600`). The `install.sh` script sets this automatically. For Phase 2, consider encrypting at rest using a key derived from the device's Amlogic unifykeys hardware store. Accepted risk for Phase 1: physical access to `/data/` partition could expose the token, but this requires ADB or UART access which already grants full root.

### 6.7 Model Partition

The 3GB `/dev/model` (ASR models) can be wiped — boot is not affected. Full recovery guide at `docs/GUIDE_MODEL_PARTITION_RECOVERY.md`. Back up first: `adb pull /model/ ./model_backup/`

---

## 7. Security Improvements Over Stock

| Issue | Stock | `waved` |
|---|---|---|
| Telemetry | 35+ events to ByteDance | None |
| Cloud upload | Insta360 servers (CN/US/EU) | User's own API only |
| PBAP contacts | Downloads entire phone book | Disabled |
| OTA signing | RSA-512 (broken since 1999) | RSA-2048 + SHA-256 |
| OTA TLS | `curl -k` (verification disabled) | Proper TLS verification |
| Firmware hash | MD5 only | SHA-256 |
| Telnet | Root on port 23, no firewall | Disabled |
| Root password | `123456` | Randomized on first boot |
| Logging secrets | Tokens, keys logged to syslog | Secrets never logged |
| Nonce generation | `srand(time()) + rand()` | `getrandom()` / CSPRNG |
| AES mode | ECB (pattern-vulnerable) | Not needed (no Insta360 protocol) |
| WiFi passwords | Logged in plaintext | Never logged |
| Firewall | None | iptables: block all inbound except needed |

---

## 8. Phased Roadmap

### Phase 1: MVP (weeks)
- `waved` binary replaces stock app
- TEA SDK audio via FFI (stock quality + tuned params)
- Recording + upload to user API
- Simplified LVGL UI (~17 screens)
- BT, WiFi, MCU, HID, PUC all working
- No telemetry, no Insta360 cloud
- Deployed via overlay (reversible)

### Phase 2: Open Audio (months)
- Replace TEA SDK with open-source pipeline:
  - AEC: WebRTC AEC3
  - Noise suppression: RNNoise or DTLN
  - Beamforming: ODAS or custom MVDR
  - DOA: ODAS
  - AGC: WebRTC AGC
- Tune for the specific 8-mic array geometry
- Remove `tea_license.ko` dependency
- Custom Buildroot config
- Own RSA-2048 SWUpdate signing

### Phase 3: Full Open (ongoing)
- Open-source the full stack
- Community-driven audio model training
- Mainline kernel support exploration
- Optional NPU inference for speaker diarization
- Web-based configuration UI (alternative to touchscreen)

---

## 9. Rust Project Structure

```
waved/
├── Cargo.toml                    # Workspace root
├── Cross.toml                    # cross-rs config for aarch64
├── config/
│   └── default.toml              # Default config
├── deploy/
│   ├── install.sh                # ADB install script
│   ├── revert.sh                 # Revert to stock
│   ├── S60waved                  # Init script
│   └── swupdate/
│       ├── genkeys.sh            # RSA-2048 key generation
│       └── build_swu.sh          # Package as SWU
├── crates/
│   ├── waved/                    # Main binary (entry, signals, module init)
│   ├── waved-audio/              # Audio pipeline FFI + routing
│   ├── waved-uart/               # Shared UART mux (owns /dev/ttyS1, dispatches frames)
│   ├── waved-bt/                 # Bluetooth protocol (receives frames from uart_mux)
│   ├── waved-mcu/                # MCU protocol (receives frames from uart_mux)
│   ├── waved-meeting/            # Recording + Opus encoding + HTTP upload
│   ├── waved-puc/                # PUC camera MAVLink + auto-framing
│   ├── waved-ui/                 # LVGL UI (17 screens)
│   ├── waved-hid/                # USB HID gadget (mute/volume)
│   ├── waved-wifi/               # wpa_supplicant wrapper + NTP
│   └── waved-system/             # OTA, USB mode, factory reset
```

### Key Dependencies

| Crate | Purpose |
|---|---|
| `tokio` | Async runtime (all modules are async tasks) |
| `reqwest` | HTTP client for API upload |
| `serde` + `toml` | Config serialization |
| `sqlx` (SQLite) | Meeting metadata DB |
| `opus` | Audio encoding |
| `alsa` | ALSA bindings |
| `serialport` | UART for BT/MCU |
| `tracing` | Structured logging (no secrets!) |
| `nix` | Unix signal handling |

---

## 10. Pre-Implementation Validation (Go/No-Go Gates)

These must be tested on-device before committing to Phase 1 implementation:

1. **`tea_license.ko` behavior (CRITICAL):** Does the TEA SDK actually stop processing if the license check fails (returns -1), or does it continue? If it blocks, Phase 1 cannot use TEA SDK and we fall back to open-source audio immediately. **Test:** Boot device, `rmmod tea_license`, run `pspapp`, observe audio output.
2. **TEA SDK config override:** Can `libInstaAudio.so` load an external JSON config, or must we binary-patch the embedded config in `libTEAProcessSdk.so`? **Test:** Place a modified JSON at candidate paths, check logs for config loading.
3. **USB gadget reconfiguration:** Can we modify the USB composite gadget at runtime without re-enumeration, or must we tear down and rebuild? **Test:** Toggle gadget config via sysfs while connected to Mac, observe macOS behavior.

## 11. Resolved Design Decisions

These were raised during review and resolved in the spec:

| Decision | Resolution |
|---|---|
| **Shared UART mux** | Dedicated `uart_mux` module owns `/dev/ttyS1`, dispatches by frame header |
| **TEA SDK threading** | FFI on dedicated OS thread, callbacks bridge to async via `mpsc` channels |
| **LVGL strategy** | Bundle LVGL 9.x from source (not stock `liblvgl.so`) to avoid ABI risk |
| **State bus channels** | `watch` for continuous state, `broadcast` for discrete events |
| **Auth token storage** | Plaintext with `chmod 0600`, accepted risk for Phase 1 |
| **Process supervision** | Init script with respawn loop, `SIGTERM` graceful shutdown (save recording, retract motor) |
| **Firewall** | iptables rules deployed at boot, drop all inbound by default |
| **Upload failure** | After 5 retries, stays in "failed" state. UI shows manual retry. Files always accessible via USB. |
| **Recording path** | New path `/mnt/udisk/recordings/` (clean break from stock `AIRecord/` format) |
| **Opus encoding** | 48kHz stereo from 3A output, 64kbps VBR, 20ms frames |
| **Localization** | JSON string tables in `/data/waved/lang/`, English default |
| **Low battery** | <10%: auto-save recording, retract motor, warn. <5%: auto-shutdown. |

---

*Spec based on firmware analysis of Insta360 Wave v1.1.6.6 (Ghidra decompilation of 15,597 functions across 6 binaries).*
