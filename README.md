# Insta360 Wave Custom Firmware

Open source firmware replacement for the **Insta360 Wave** conference speakerphone (CINSABCA). A single Rust binary (`waved`) replaces the stock `pspapp`, removing all telemetry while maintaining full hardware functionality.

## Why

The stock firmware:
- Sends 35+ telemetry event types to ByteDance's DataFinder SDK
- Uploads meeting recordings to Insta360 cloud servers
- Downloads phone contacts via Bluetooth without clear consent
- Ships with critical security issues (RSA-512 signing, telnet as root, hardcoded passwords, disabled TLS verification)

This project gives you full control over your device.

## Features

- **Zero telemetry** — no ByteDance DataFinder SDK, no Insta360 analytics
- **Self-hosted recordings** — upload audio to your own HTTP API endpoint
- **Full hardware support** — USB audio (UAC2), Bluetooth, WiFi, motorized column, mute/volume buttons
- **macOS native speakerphone** — UAC2 + HID works without the Insta360 controller app
- **TEA SDK audio pipeline** — preserves AEC, beamforming, noise suppression via FFI
- **Safe and reversible** — stock firmware can always be restored with one script

## Architecture

```
waved (single Rust binary, tokio async)
├── waved-state      State bus: typed watch/broadcast channels
├── waved-config     TOML config with hot-reload defaults
├── waved-uart       UART mux for /dev/ttyS1 (BT + MCU frame dispatch)
├── waved-mcu        Battery, motor, button handling
├── waved-bt         Bluetooth command builders + status tracking
├── waved-hid        USB HID gadget (mute/volume reports to host)
├── waved-audio      TEA SDK FFI stubs + callback channel bridge
├── waved-meeting    Opus recording, SQLite DB, HTTP upload with retry
└── waved-wifi       wpa_supplicant wrapper + NTP sync
```

Cross-compiled for `aarch64-unknown-linux-gnu`. Runs on the stock Linux 5.4.242 kernel (Amlogic A5 / AV400).

## Quick Start

### Build

```bash
# Native check (host development)
cd waved
cargo check

# Cross-compile for device (requires Docker + cross-rs)
cargo install cross
cross build --release --target aarch64-unknown-linux-gnu
```

### Deploy

```bash
# Connect device via USB, ensure ADB access
adb devices

# Install (stops stock firmware, pushes binary, sets up init script)
./waved/deploy/install.sh

# Reboot into waved
adb reboot
```

### Revert to Stock

```bash
./waved/deploy/revert.sh
adb reboot
```

### Test

```bash
cd waved

# Layer 1: Unit tests (runs on your Mac, no device needed)
cargo test

# Layer 2: Integration tests (simulate feature swaps hardware for in-memory mocks)
cargo test -p waved --features simulate

# Layer 3: Run simulation binary (full waved on your Mac with fake hardware)
cargo run -p waved --features simulate

# Layer 4A: ADB smoke test (quick deploy + health check on device)
./scripts/smoke.sh --deploy

# Layer 4B: On-device hardware tests (runs on the Wave itself)
cross build --release --target aarch64-unknown-linux-gnu -p waved-device-test
adb push target/aarch64-unknown-linux-gnu/release/waved-device-test /data/waved/
adb shell /data/waved/waved-device-test

# Layer 5: Full HIL run (backup → deploy → test → restore, manual trigger only)
./scripts/hil.sh
```

### Configure

Edit `/data/waved/config.toml` on device (or `waved/config/default.toml` before deploying):

```toml
[device]
name = "Wave Open"

[audio]
mode = "noise_reduction"    # omni, cardioid, supercardioid, figure8, stereo
speaker_volume = 150
mic_volume = 200

[api]
endpoint = "https://your-server.com/meetings/upload"
auth_token = "your-token"
auto_upload = true
retry_max = 5

[ntp]
servers = ["pool.ntp.org", "time.google.com"]
```

## Project Structure

```
├── waved/                    Rust workspace (10 crates)
│   ├── crates/               All service crates + waved-device-test
│   ├── config/               Default device configuration
│   ├── deploy/               ADB deployment scripts + init script + firewall
│   └── scripts/              smoke.sh (Layer 4A) + hil.sh (Layer 5)
├── decompiled/               Ghidra decompiled C output (stock firmware)
├── firmware/                 Firmware update manifests (v1.0.13.6, v1.1.6.6)
├── docs/
│   ├── *.md                  Reverse-engineering reports & guides
│   ├── analysis_reports/     Decompiled firmware analysis
│   └── superpowers/
│       ├── specs/            Firmware design specification
│       └── plans/            Implementation plans (all phases)
├── ghidra_decompile.java     Ghidra headless decompilation script
└── ghidra_decompile.py       Ghidra Python decompilation script
```

## Documentation

### Reverse Engineering

| Document | Description |
|---|---|
| [REPORT.md](docs/REPORT.md) | Main reverse-engineering report |
| [REPORT_AI_MODELS.md](docs/REPORT_AI_MODELS.md) | TEA SDK, ASR, and NPU model analysis |
| [REPORT_DEVICE_DUMP.md](docs/REPORT_DEVICE_DUMP.md) | eMMC partition layout and filesystem analysis |
| [REPORT_NPU.md](docs/REPORT_NPU.md) | Amlogic NPU and neural network analysis |
| [REPORT_UI.md](docs/REPORT_UI.md) | LVGL touchscreen UI reverse engineering |

### Guides

| Document | Description |
|---|---|
| [GUIDE_DEVICE_ACCESS.md](docs/GUIDE_DEVICE_ACCESS.md) | ADB access, root shell, filesystem layout |
| [GUIDE_EXTRACT_MODELS.md](docs/GUIDE_EXTRACT_MODELS.md) | Extracting AI models from device |
| [GUIDE_MODEL_PARTITION_RECOVERY.md](docs/GUIDE_MODEL_PARTITION_RECOVERY.md) | Recovering the 3GB model partition |

### Decompiled Firmware

| File | Description |
|---|---|
| [pspapp.c](decompiled/pspapp.c) | Main application binary |
| [libins_entry.so.c](decompiled/libins_entry.so.c) | Core entry point library |
| [libInstaAudio.so.c](decompiled/libInstaAudio.so.c) | TEA SDK audio pipeline |
| [libcloud_proto_https.so.c](decompiled/libcloud_proto_https.so.c) | Cloud/telemetry protocol |
| [libdatabase.so.c](decompiled/libdatabase.so.c) | SQLite database layer |
| [libui.so.c](decompiled/libui.so.c) | LVGL touchscreen UI |

### Analysis

| Document | Description |
|---|---|
| [Cloud Protocol](docs/analysis_reports/Cloud_Protocol_Decompiled_Analysis.md) | Telemetry and cloud API protocol |
| [Audio Library](docs/analysis_reports/Cloud_and_Audio_Library_Analysis.md) | TEA SDK audio pipeline internals |
| [Init Scripts](docs/analysis_reports/Init_Scripts_and_Config_Analysis.md) | Boot sequence and service startup |
| [UI & Database](docs/analysis_reports/UI_and_Database_Decompiled_Analysis.md) | LVGL UI and SQLite schema |
| [libins_entry](docs/analysis_reports/libins_entry_Decompiled_Analysis.md) | Main application library decompilation |

### Design & Plans

| Document | Description |
|---|---|
| [Firmware Design Spec](docs/superpowers/specs/2026-03-19-waved-open-firmware-design.md) | Full architecture and design specification |
| [Phase 1: Headless MVP](docs/superpowers/plans/2026-03-19-waved-phase1-mvp.md) | This implementation (current) |
| [LVGL UI](docs/superpowers/plans/2026-03-19-waved-lvgl-ui.md) | Touchscreen UI plan |
| [PUC Camera](docs/superpowers/plans/2026-03-19-waved-puc-camera.md) | Auto-framing camera integration |
| [OTA System](docs/superpowers/plans/2026-03-19-waved-ota-system.md) | Secure over-the-air updates |
| [Device Validation](docs/superpowers/plans/2026-03-19-waved-device-validation.md) | On-device hardware validation |

## Roadmap

- [x] **Phase 1: Headless MVP** — UART, BT, MCU, HID, WiFi, recording, upload
- [ ] **Phase 1.5: Device Validation** — On-device testing of TEA SDK FFI, UART byte offsets, HID reports
- [ ] **Phase 2: LVGL UI** — 17 touchscreen screens, status bar, navigation
- [ ] **Phase 3: PUC Camera** — MAVLink protocol, DOA-driven auto-framing
- [ ] **Phase 4: OTA** — RSA-2048 signed SWU packages from custom server
- [ ] **Phase 5: Open Audio** — Replace TEA SDK with WebRTC AEC3 + RNNoise + ODAS

## Hardware

| Component | Details |
|---|---|
| SoC | Amlogic A5 (AV400), quad-core Cortex-A55, 1GB DDR |
| Storage | 32GB eMMC (SquashFS rootfs + ext4 data + exFAT USB disk) |
| Audio | 8-mic array, 2 speakers, TEA SDK 3A processing |
| Display | 1.82" 400x304 touchscreen (LVGL) |
| Connectivity | WiFi (RTL8733BU), Bluetooth 5.0 (BES2600), USB-C (UAC2 + HID) |
| Camera | PUC (PanaCast-style USB camera), MAVLink control |
| Motor | Motorized height-adjustable column (3 positions) |

## License

MIT
