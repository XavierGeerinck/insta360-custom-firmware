# `waved` Phase 1 MVP — Headless Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a headless `waved` binary that replaces the stock `pspapp`, providing audio pipeline control (TEA SDK FFI), BT/MCU hardware communication, meeting recording with Opus encoding and HTTP upload to a custom API, USB HID mute/volume, WiFi management, and deployment scripts — all without telemetry.

**Architecture:** Single Rust binary using `tokio` async runtime. Modules communicate via typed `watch`/`broadcast` channels (state bus). UART mux owns `/dev/ttyS1` and dispatches frames to BT/MCU modules. TEA SDK FFI runs on a dedicated OS thread with callback-to-channel bridging. Cross-compiled for `aarch64-unknown-linux-gnu`.

**Tech Stack:** Rust, tokio, crossbeam, serialport, alsa-rs, opus, reqwest, sqlx (SQLite), serde, toml, tracing, nix

**Spec:** `docs/superpowers/specs/2026-03-19-waved-open-firmware-design.md`

**Scope:** This plan covers the headless MVP (no touchscreen UI). The LVGL UI, PUC camera, and OTA system are separate follow-on plans. The headless MVP is controlled via `config.toml` + ADB shell and validates all hardware integration before UI work begins.

**Pre-requisites (go/no-go gates from spec section 10):**
Before starting Task 1, these must be validated on-device:
1. `tea_license.ko` behavior — does TEA SDK work without the license module?
2. TEA SDK config override — can we load external JSON?
3. USB gadget behavior — can we reconfigure at runtime?

---

## Errata (from plan review — apply these fixes during implementation)

The code in the tasks below is directionally correct but has compilation and design issues that must be fixed during implementation. Each implementing agent MUST apply these fixes:

### Critical (will not compile without these)

1. **`watch::Sender` is not `Clone`.** In `StateBus`, wrap each `watch::Sender` in `Arc` so multiple modules can share them: `pub audio: Arc<watch::Sender<AudioState>>`. Update all `StateBus::new()` to wrap in `Arc::new()`. The `send_modify` method takes `&self` so `Arc` works.

2. **`crossbeam_channel::Receiver` is not `Clone`.** In `UartMux`, change `bt_receiver()`/`mcu_receiver()` to `take_bt_receiver(&mut self) -> Receiver<Frame>` and `take_mcu_receiver(&mut self) -> Receiver<Frame>` using `Option::take()`. Call these before passing `UartMux` into `Arc`. In `main.rs`, take receivers before wrapping in Arc:
   ```rust
   let mut uart = UartMux::open(UART_PORT, UART_BAUD)?;
   let mcu_rx = uart.take_mcu_receiver();
   let bt_rx = uart.take_bt_receiver();
   let uart = Arc::new(uart);
   ```

3. **UART port type mismatch.** `serialport::new(...).open()` returns `Box<dyn SerialPort>`. Store as `Arc<Mutex<Box<dyn SerialPort>>>` in `UartMux`, not `Arc<Mutex<dyn SerialPort>>`.

4. **Audio frame channel disconnected.** The `_frame_rx` in `main.rs` must be passed to `MeetingService`. Add `audio_rx: mpsc::Receiver<AudioFrame>` parameter to `MeetingService::new()` and consume frames in the recording loop.

### Important (spec requirements not met)

5. **Add SIGTERM handling.** Replace `tokio::signal::ctrl_c()` with:
   ```rust
   use tokio::signal::unix::{signal, SignalKind};
   let mut sigterm = signal(SignalKind::terminate()).unwrap();
   let mut sigint = signal(SignalKind::interrupt()).unwrap();
   tokio::select! {
       _ = sigterm.recv() => info!("SIGTERM received"),
       _ = sigint.recv() => info!("SIGINT received"),
   }
   ```

6. **Add NTP sync to `wifi_srv`.** When WiFi connects, run: `tokio::process::Command::new("ntpd").args(["-q", "-p", server]).status().await`. Iterate config NTP servers.

7. **Add upload retry loop.** Wrap `upload_recording()` in a retry loop using `retry_max` and `retry_backoff_ms` from config. Use `tokio::time::sleep(Duration::from_millis(backoff * attempt))`.

8. **Handle `Command::Shutdown` in `MeetingService::run()`.** If recording is active, stop and save it before returning.

9. **Handle low battery in main.** Subscribe to events in main's async block. On `DeviceEvent::LowBattery(level)`: if level < 10, send `Command::StopRecording` + `Command::SetMotorPosition(Low)`. If level < 5, send `Command::Shutdown`.

10. **Remove duplicate `run_async_only`/`run_async_services`.** Collapse into a single function.

11. **Handle `broadcast::Lagged` errors.** In all `broadcast::Receiver` consumers, log a warning on `Err(Lagged(n))` instead of silently dropping.

12. **Use Rust edition `"2021"` instead of `"2024"`** unless you've confirmed aarch64 cross-compilation works with edition 2024 on your toolchain.

---

## File Structure

```
waved/
├── Cargo.toml                              # Workspace definition
├── Cross.toml                              # cross-rs aarch64 config
├── rust-toolchain.toml                     # Pin Rust toolchain
├── .cargo/config.toml                      # Linker config for aarch64
├── config/
│   └── default.toml                        # Default device config
├── deploy/
│   ├── install.sh                          # ADB push + overlay install
│   ├── revert.sh                           # Revert to stock firmware
│   ├── S60waved                            # Init script with watchdog
│   └── iptables.rules                      # Firewall rules
├── crates/
│   ├── waved/                              # Main binary
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── main.rs                     # Entry: signal handling, module init, tokio runtime
│   ├── waved-state/                        # State bus + event types
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs                      # StateBus, DeviceEvent, state structs
│   ├── waved-config/                       # Config loading + watching
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs                      # WavedConfig, load/save/defaults
│   ├── waved-uart/                         # UART mux for /dev/ttyS1
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                      # Public API: UartMux::new(), send(), subscribe_bt/mcu()
│   │       ├── frame.rs                    # Frame parsing, checksum, header types
│   │       └── mux.rs                      # Reader thread, writer with priority queue
│   ├── waved-mcu/                          # MCU hardware control
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                      # McuService, run()
│   │       ├── battery.rs                  # Battery state parsing
│   │       ├── motor.rs                    # Motor commands
│   │       └── buttons.rs                  # Button event parsing
│   ├── waved-bt/                           # Bluetooth manager
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                      # BtService, run()
│   │       └── commands.rs                 # BT command builders + response parsers
│   ├── waved-hid/                          # USB HID gadget
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs                      # HidService: write mute/volume reports to /dev/hidg0
│   ├── waved-audio/                        # TEA SDK FFI audio control
│   │   ├── Cargo.toml
│   │   ├── build.rs                        # Link against libInstaAudio.so
│   │   └── src/
│   │       ├── lib.rs                      # AudioService, run()
│   │       ├── ffi.rs                      # extern "C" bindings to libInstaAudio.so
│   │       └── callbacks.rs                # C callback → mpsc channel bridge
│   ├── waved-meeting/                      # Recording + upload
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                      # MeetingService, run()
│   │       ├── recorder.rs                 # Opus encoding, file writing
│   │       ├── upload.rs                   # HTTP POST with retry
│   │       └── db.rs                       # SQLite schema + queries
│   └── waved-wifi/                         # WiFi management
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs                      # WifiService, run()
│           └── wpa.rs                      # wpa_supplicant CLI wrapper
```

---

### Task 1: Workspace Scaffolding + Cross-Compilation

**Files:**
- Create: `waved/Cargo.toml`
- Create: `waved/Cross.toml`
- Create: `waved/rust-toolchain.toml`
- Create: `waved/.cargo/config.toml`
- Create: `waved/crates/waved/Cargo.toml`
- Create: `waved/crates/waved/src/main.rs`

- [ ] **Step 1: Create workspace root**

```bash
cd /Users/xaviergeerinck/Projects/insta360
mkdir -p waved/crates/waved/src
```

- [ ] **Step 2: Write workspace Cargo.toml**

Create `waved/Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
thiserror = "2"
uuid = { version = "1", features = ["v4", "serde"] }
crossbeam-channel = "0.5"
nix = { version = "0.29", features = ["signal", "fs"] }
```

- [ ] **Step 3: Write cross-compilation config**

Create `waved/Cross.toml`:
```toml
[target.aarch64-unknown-linux-gnu]
image = "ghcr.io/cross-rs/aarch64-unknown-linux-gnu:main"
```

Create `waved/rust-toolchain.toml`:
```toml
[toolchain]
channel = "stable"
targets = ["aarch64-unknown-linux-gnu"]
```

Create `waved/.cargo/config.toml`:
```toml
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
```

- [ ] **Step 4: Write minimal main.rs**

Create `waved/crates/waved/Cargo.toml`:
```toml
[package]
name = "waved"
version.workspace = true
edition.workspace = true

[[bin]]
name = "waved"
path = "src/main.rs"

[dependencies]
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
nix.workspace = true
```

Create `waved/crates/waved/src/main.rs`:
```rust
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("waved=info".parse().unwrap()))
        .init();

    info!("waved v{} starting", env!("CARGO_PKG_VERSION"));

    // Build tokio runtime
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    rt.block_on(async {
        info!("all modules initialized");

        // Wait for shutdown signal
        match tokio::signal::ctrl_c().await {
            Ok(()) => info!("received SIGINT, shutting down"),
            Err(e) => warn!("signal handler error: {e}"),
        }
    });

    info!("waved shutdown complete");
}
```

- [ ] **Step 5: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check
```
Expected: compiles with no errors.

- [ ] **Step 6: Commit**

```bash
git add waved/
git commit -m "feat: scaffold waved workspace with cross-compilation config"
```

---

### Task 2: State Bus + Event Types

**Files:**
- Create: `waved/crates/waved-state/Cargo.toml`
- Create: `waved/crates/waved-state/src/lib.rs`

- [ ] **Step 1: Create crate**

```bash
mkdir -p waved/crates/waved-state/src
```

- [ ] **Step 2: Write Cargo.toml**

Create `waved/crates/waved-state/Cargo.toml`:
```toml
[package]
name = "waved-state"
version.workspace = true
edition.workspace = true

[dependencies]
tokio = { workspace = true, features = ["sync"] }
uuid.workspace = true
serde.workspace = true
```

- [ ] **Step 3: Write state bus and types**

Create `waved/crates/waved-state/src/lib.rs`:
```rust
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};
use uuid::Uuid;

// --- Continuous State Types (watch channels) ---

#[derive(Clone, Debug, Default)]
pub struct AudioState {
    pub mode: AudioMode,
    pub mic_muted: bool,
    pub speaker_muted: bool,
    pub speaker_volume: u16,
    pub mic_volume: u16,
    pub doa_horizontal: f32,
    pub doa_pitch: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AudioMode {
    #[default]
    NoiseReduction,
    Omni,
    Cardioid,
    Supercardioid,
    Figure8,
    Stereo,
}

#[derive(Clone, Debug, Default)]
pub struct BatteryState {
    pub level: u8,
    pub voltage: u16,
    pub temperature: i8,
    pub is_charging: bool,
}

#[derive(Clone, Debug, Default)]
pub struct BluetoothState {
    pub connected: bool,
    pub device_name: String,
    pub mac: String,
    pub bt_on: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WifiState {
    pub connected: bool,
    pub ssid: String,
    pub ip_addr: String,
    pub signal_strength: i8,
}

#[derive(Clone, Debug, Default)]
pub struct MeetingState {
    pub recording: bool,
    pub paused: bool,
    pub meeting_id: Option<Uuid>,
    pub duration_secs: u32,
    pub pending_uploads: u32,
    pub failed_uploads: u32,
}

#[derive(Clone, Debug, Default)]
pub struct McuState {
    pub motor_position: MotorPosition,
    pub usb_connected: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum MotorPosition {
    #[default]
    Low,
    Medium,
    High,
    Moving,
}

#[derive(Clone, Debug, Default)]
pub struct PucState {
    pub connected: bool,
    pub auto_framing: bool,
}

// --- Discrete Event Types (broadcast channel) ---

#[derive(Clone, Debug)]
pub enum DeviceEvent {
    ButtonPress(ButtonType),
    UsbConnected,
    UsbDisconnected,
    BtConnected(String),
    BtDisconnected,
    WifiConnected(String),
    WifiDisconnected,
    RecordingStarted(Uuid),
    RecordingStopped(Uuid),
    LowBattery(u8),
    MotorObstruction,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ButtonType {
    MuteToggle,
    VolumeUp,
    VolumeDown,
    MuteLongPress,
}

// --- Command Types (UI/external → modules) ---

#[derive(Clone, Debug)]
pub enum Command {
    SetAudioMode(AudioMode),
    ToggleMicMute,
    ToggleSpeakerMute,
    SetSpeakerVolume(u16),
    SetMicVolume(u16),
    StartRecording,
    StopRecording,
    PauseRecording,
    ResumeRecording,
    AddBookmark,
    RetryUpload(Uuid),
    SetMotorPosition(MotorPosition),
    Shutdown,
}

// --- State Bus ---

pub struct StateBus {
    pub audio: watch::Sender<AudioState>,
    pub battery: watch::Sender<BatteryState>,
    pub bt: watch::Sender<BluetoothState>,
    pub wifi: watch::Sender<WifiState>,
    pub meeting: watch::Sender<MeetingState>,
    pub mcu: watch::Sender<McuState>,
    pub puc: watch::Sender<PucState>,
    pub events: broadcast::Sender<DeviceEvent>,
    pub commands: broadcast::Sender<Command>,
}

impl StateBus {
    pub fn new() -> Self {
        let (audio_tx, _) = watch::channel(AudioState::default());
        let (battery_tx, _) = watch::channel(BatteryState::default());
        let (bt_tx, _) = watch::channel(BluetoothState::default());
        let (wifi_tx, _) = watch::channel(WifiState::default());
        let (meeting_tx, _) = watch::channel(MeetingState::default());
        let (mcu_tx, _) = watch::channel(McuState::default());
        let (puc_tx, _) = watch::channel(PucState::default());
        let (events_tx, _) = broadcast::channel(64);
        let (commands_tx, _) = broadcast::channel(64);

        Self {
            audio: audio_tx,
            battery: battery_tx,
            bt: bt_tx,
            wifi: wifi_tx,
            meeting: meeting_tx,
            mcu: mcu_tx,
            puc: puc_tx,
            events: events_tx,
            commands: commands_tx,
        }
    }
}
```

- [ ] **Step 4: Add to workspace and verify**

Add `"waved-state"` as a dependency to `waved/crates/waved/Cargo.toml`:
```toml
[dependencies]
waved-state = { path = "../waved-state" }
```

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check
```

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved-state/
git commit -m "feat: add state bus with typed watch/broadcast channels"
```

---

### Task 3: Config Loading

**Files:**
- Create: `waved/crates/waved-config/Cargo.toml`
- Create: `waved/crates/waved-config/src/lib.rs`
- Create: `waved/config/default.toml`

- [ ] **Step 1: Create crate**

```bash
mkdir -p waved/crates/waved-config/src waved/config
```

- [ ] **Step 2: Write Cargo.toml**

Create `waved/crates/waved-config/Cargo.toml`:
```toml
[package]
name = "waved-config"
version.workspace = true
edition.workspace = true

[dependencies]
serde.workspace = true
toml.workspace = true
tracing.workspace = true
thiserror.workspace = true
waved-state = { path = "../waved-state" }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Write default config**

Create `waved/config/default.toml`:
```toml
[device]
name = "Wave Open"
language = "en"

[audio]
mode = "noise_reduction"
speaker_volume = 150
mic_volume = 200

[api]
endpoint = "https://localhost:8080/meetings/upload"
auth_token = ""
auto_upload = true
retry_max = 5
retry_backoff_ms = 1000

[security]
device_password = ""
ota_server = ""

[ntp]
servers = ["pool.ntp.org", "time.google.com"]
```

- [ ] **Step 4: Write config module with tests**

Create `waved/crates/waved-config/src/lib.rs`:
```rust
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{info, warn};
use waved_state::AudioMode;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("failed to serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WavedConfig {
    pub device: DeviceConfig,
    pub audio: AudioConfig,
    pub api: ApiConfig,
    pub security: SecurityConfig,
    pub ntp: NtpConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub name: String,
    pub language: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioConfig {
    pub mode: AudioMode,
    pub speaker_volume: u16,
    pub mic_volume: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiConfig {
    pub endpoint: String,
    pub auth_token: String,
    pub auto_upload: bool,
    pub retry_max: u32,
    pub retry_backoff_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub device_password: String,
    pub ota_server: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NtpConfig {
    pub servers: Vec<String>,
}

impl WavedConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&contents)?;
        info!("loaded config from {}", path.display());
        Ok(config)
    }

    pub fn load_or_default(path: &Path) -> Self {
        match Self::load(path) {
            Ok(config) => config,
            Err(e) => {
                warn!("failed to load config ({}), using defaults", e);
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        info!("saved config to {}", path.display());
        Ok(())
    }
}

impl Default for WavedConfig {
    fn default() -> Self {
        toml::from_str(include_str!("../../../config/default.toml"))
            .expect("default.toml must be valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let config = WavedConfig::default();
        assert_eq!(config.device.name, "Wave Open");
        assert_eq!(config.audio.mode, AudioMode::NoiseReduction);
        assert_eq!(config.audio.speaker_volume, 150);
        assert_eq!(config.ntp.servers.len(), 2);
    }

    #[test]
    fn roundtrip_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = WavedConfig::default();
        config.save(&path).unwrap();
        let loaded = WavedConfig::load(&path).unwrap();
        assert_eq!(loaded.device.name, config.device.name);
        assert_eq!(loaded.api.retry_max, config.api.retry_max);
    }
}
```

- [ ] **Step 5: Run tests**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo test -p waved-config
```
Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add waved/crates/waved-config/ waved/config/
git commit -m "feat: add config loading with TOML schema and defaults"
```

---

### Task 4: UART Frame Protocol

**Files:**
- Create: `waved/crates/waved-uart/Cargo.toml`
- Create: `waved/crates/waved-uart/src/lib.rs`
- Create: `waved/crates/waved-uart/src/frame.rs`
- Create: `waved/crates/waved-uart/src/mux.rs`

- [ ] **Step 1: Create crate**

```bash
mkdir -p waved/crates/waved-uart/src
```

- [ ] **Step 2: Write Cargo.toml**

Create `waved/crates/waved-uart/Cargo.toml`:
```toml
[package]
name = "waved-uart"
version.workspace = true
edition.workspace = true

[dependencies]
serialport = "4"
crossbeam-channel.workspace = true
tracing.workspace = true
thiserror.workspace = true
```

- [ ] **Step 3: Write frame parser with tests**

Create `waved/crates/waved-uart/src/frame.rs`:
```rust
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum FrameTarget {
    Bluetooth, // Headers: 0x75 0x46, 0x75 0x45
    Mcu,       // Header: 0xDC 0xCD
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub header: [u8; 2],
    pub target: FrameTarget,
    pub data: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("invalid header: {0:#04x} {1:#04x}")]
    InvalidHeader(u8, u8),
    #[error("checksum mismatch: expected {expected:#04x}, got {actual:#04x}")]
    ChecksumMismatch { expected: u8, actual: u8 },
    #[error("buffer too short")]
    BufferTooShort,
}

impl Frame {
    /// Build a frame ready to send (with checksum appended).
    pub fn build(header: [u8; 2], data: &[u8]) -> Result<Vec<u8>, FrameError> {
        let target = Self::classify_header(header)?;
        let _ = target; // validate header
        let len = data.len() as u8;
        let mut buf = Vec::with_capacity(4 + data.len());
        buf.extend_from_slice(&header);
        buf.push(len);
        buf.extend_from_slice(data);
        let checksum = buf.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        buf.push(checksum);
        Ok(buf)
    }

    fn classify_header(header: [u8; 2]) -> Result<FrameTarget, FrameError> {
        match header {
            [0x75, 0x46] | [0x75, 0x45] => Ok(FrameTarget::Bluetooth),
            [0xDC, 0xCD] => Ok(FrameTarget::Mcu),
            [a, b] => Err(FrameError::InvalidHeader(a, b)),
        }
    }
}

/// State machine parser that processes bytes one at a time.
/// Call `feed()` with each byte; returns `Some(Frame)` when a complete frame is parsed.
pub struct FrameParser {
    state: ParseState,
    header: [u8; 2],
    length: u8,
    data: Vec<u8>,
    checksum_accum: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ParseState {
    Header0,
    Header1,
    Length,
    Data,
    Checksum,
}

impl FrameParser {
    pub fn new() -> Self {
        Self {
            state: ParseState::Header0,
            header: [0; 2],
            length: 0,
            data: Vec::new(),
            checksum_accum: 0,
        }
    }

    pub fn feed(&mut self, byte: u8) -> Option<Result<Frame, FrameError>> {
        match self.state {
            ParseState::Header0 => {
                if byte == 0x75 || byte == 0xDC {
                    self.header[0] = byte;
                    self.checksum_accum = byte;
                    self.state = ParseState::Header1;
                }
                None
            }
            ParseState::Header1 => {
                let valid = match self.header[0] {
                    0x75 => byte == 0x46 || byte == 0x45,
                    0xDC => byte == 0xCD,
                    _ => false,
                };
                if valid {
                    self.header[1] = byte;
                    self.checksum_accum = self.checksum_accum.wrapping_add(byte);
                    self.state = ParseState::Length;
                    None
                } else {
                    self.reset();
                    None
                }
            }
            ParseState::Length => {
                self.length = byte;
                self.checksum_accum = self.checksum_accum.wrapping_add(byte);
                self.data.clear();
                if byte == 0 {
                    self.state = ParseState::Checksum;
                } else {
                    self.state = ParseState::Data;
                }
                None
            }
            ParseState::Data => {
                self.data.push(byte);
                self.checksum_accum = self.checksum_accum.wrapping_add(byte);
                if self.data.len() == self.length as usize {
                    self.state = ParseState::Checksum;
                }
                None
            }
            ParseState::Checksum => {
                let result = if byte == self.checksum_accum {
                    match Frame::classify_header(self.header) {
                        Ok(target) => Ok(Frame {
                            header: self.header,
                            target,
                            data: std::mem::take(&mut self.data),
                        }),
                        Err(e) => Err(e),
                    }
                } else {
                    Err(FrameError::ChecksumMismatch {
                        expected: self.checksum_accum,
                        actual: byte,
                    })
                };
                self.reset();
                Some(result)
            }
        }
    }

    fn reset(&mut self) {
        self.state = ParseState::Header0;
        self.header = [0; 2];
        self.length = 0;
        self.data.clear();
        self.checksum_accum = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_bt_frame() {
        let raw = Frame::build([0x75, 0x46], &[0x01, 0x02]).unwrap();
        assert_eq!(raw.len(), 6); // 2 header + 1 len + 2 data + 1 checksum
        assert_eq!(raw[0..2], [0x75, 0x46]);
        assert_eq!(raw[2], 2); // length
        assert_eq!(raw[3..5], [0x01, 0x02]);
        let expected_checksum = 0x75u8
            .wrapping_add(0x46)
            .wrapping_add(2)
            .wrapping_add(0x01)
            .wrapping_add(0x02);
        assert_eq!(raw[5], expected_checksum);
    }

    #[test]
    fn build_mcu_frame() {
        let raw = Frame::build([0xDC, 0xCD], &[0x10]).unwrap();
        assert_eq!(raw[0..2], [0xDC, 0xCD]);
        assert_eq!(raw[2], 1);
    }

    #[test]
    fn build_invalid_header() {
        assert!(Frame::build([0x00, 0x00], &[0x01]).is_err());
    }

    #[test]
    fn parse_bt_frame() {
        let raw = Frame::build([0x75, 0x46], &[0x0E]).unwrap();
        let mut parser = FrameParser::new();
        let mut result = None;
        for &byte in &raw {
            if let Some(r) = parser.feed(byte) {
                result = Some(r);
            }
        }
        let frame = result.unwrap().unwrap();
        assert_eq!(frame.target, FrameTarget::Bluetooth);
        assert_eq!(frame.data, vec![0x0E]);
    }

    #[test]
    fn parse_mcu_frame() {
        let raw = Frame::build([0xDC, 0xCD], &[0x01, 0x55]).unwrap();
        let mut parser = FrameParser::new();
        let mut result = None;
        for &byte in &raw {
            if let Some(r) = parser.feed(byte) {
                result = Some(r);
            }
        }
        let frame = result.unwrap().unwrap();
        assert_eq!(frame.target, FrameTarget::Mcu);
        assert_eq!(frame.data, vec![0x01, 0x55]);
    }

    #[test]
    fn parse_recovers_from_garbage() {
        let mut parser = FrameParser::new();
        // Feed garbage
        for &byte in &[0xFF, 0x00, 0x13, 0xAB] {
            assert!(parser.feed(byte).is_none());
        }
        // Then a valid frame
        let raw = Frame::build([0x75, 0x45], &[0x09]).unwrap();
        let mut result = None;
        for &byte in &raw {
            if let Some(r) = parser.feed(byte) {
                result = Some(r);
            }
        }
        let frame = result.unwrap().unwrap();
        assert_eq!(frame.target, FrameTarget::Bluetooth);
        assert_eq!(frame.data, vec![0x09]);
    }

    #[test]
    fn checksum_mismatch_detected() {
        let mut raw = Frame::build([0x75, 0x46], &[0x01]).unwrap();
        *raw.last_mut().unwrap() = 0xFF; // corrupt checksum
        let mut parser = FrameParser::new();
        let mut result = None;
        for &byte in &raw {
            if let Some(r) = parser.feed(byte) {
                result = Some(r);
            }
        }
        assert!(result.unwrap().is_err());
    }
}
```

- [ ] **Step 4: Write mux module (reader/writer threads)**

Create `waved/crates/waved-uart/src/mux.rs`:
```rust
use crate::frame::{Frame, FrameParser, FrameTarget};
use crossbeam_channel::{bounded, Receiver, Sender};
use serialport::SerialPort;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{debug, error, trace, warn};

/// A frame with priority for the write queue.
pub struct PrioritizedFrame {
    pub raw: Vec<u8>,
    pub priority: bool, // true = MCU motor-stop etc, sent first
}

pub struct UartMux {
    bt_rx: Receiver<Frame>,
    mcu_rx: Receiver<Frame>,
    write_tx: Sender<PrioritizedFrame>,
    _reader_handle: std::thread::JoinHandle<()>,
    _writer_handle: std::thread::JoinHandle<()>,
}

impl UartMux {
    pub fn open(port_path: &str, baud_rate: u32) -> Result<Self, serialport::Error> {
        let port = serialport::new(port_path, baud_rate)
            .timeout(Duration::from_millis(100))
            .open()?;

        let port = Arc::new(Mutex::new(port));

        let (bt_tx, bt_rx) = bounded::<Frame>(64);
        let (mcu_tx, mcu_rx) = bounded::<Frame>(64);
        let (write_tx, write_rx) = bounded::<PrioritizedFrame>(64);

        let reader_port = Arc::clone(&port);
        let reader_handle = std::thread::Builder::new()
            .name("uart-reader".into())
            .spawn(move || {
                Self::reader_loop(reader_port, bt_tx, mcu_tx);
            })
            .expect("failed to spawn uart reader thread");

        let writer_port = Arc::clone(&port);
        let writer_handle = std::thread::Builder::new()
            .name("uart-writer".into())
            .spawn(move || {
                Self::writer_loop(writer_port, write_rx);
            })
            .expect("failed to spawn uart writer thread");

        Ok(Self {
            bt_rx,
            mcu_rx,
            write_tx,
            _reader_handle: reader_handle,
            _writer_handle: writer_handle,
        })
    }

    pub fn bt_receiver(&self) -> &Receiver<Frame> {
        &self.bt_rx
    }

    pub fn mcu_receiver(&self) -> &Receiver<Frame> {
        &self.mcu_rx
    }

    pub fn send(&self, raw: Vec<u8>, priority: bool) {
        let _ = self.write_tx.send(PrioritizedFrame { raw, priority });
    }

    fn reader_loop(
        port: Arc<Mutex<dyn SerialPort>>,
        bt_tx: Sender<Frame>,
        mcu_tx: Sender<Frame>,
    ) {
        let mut parser = FrameParser::new();
        let mut buf = [0u8; 1];

        loop {
            let read_result = {
                let mut port = port.lock().unwrap();
                port.read(&mut buf)
            };

            match read_result {
                Ok(1) => {
                    if let Some(result) = parser.feed(buf[0]) {
                        match result {
                            Ok(frame) => {
                                trace!("parsed frame: {:?} target={:?}", frame.data, frame.target);
                                let tx = match frame.target {
                                    FrameTarget::Bluetooth => &bt_tx,
                                    FrameTarget::Mcu => &mcu_tx,
                                };
                                if tx.send(frame).is_err() {
                                    debug!("receiver dropped, stopping reader");
                                    return;
                                }
                            }
                            Err(e) => {
                                warn!("frame parse error: {e}");
                            }
                        }
                    }
                }
                Ok(_) => {}
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    // Normal timeout, continue
                }
                Err(e) => {
                    error!("UART read error: {e}");
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    fn writer_loop(
        port: Arc<Mutex<dyn SerialPort>>,
        write_rx: Receiver<PrioritizedFrame>,
    ) {
        use std::io::Write;

        for pframe in write_rx.iter() {
            let mut port = port.lock().unwrap();
            if let Err(e) = port.write_all(&pframe.raw) {
                error!("UART write error: {e}");
            }
            if let Err(e) = port.flush() {
                error!("UART flush error: {e}");
            }
        }
    }
}
```

- [ ] **Step 5: Write lib.rs**

Create `waved/crates/waved-uart/src/lib.rs`:
```rust
pub mod frame;
pub mod mux;

pub use frame::{Frame, FrameError, FrameParser, FrameTarget};
pub use mux::UartMux;
```

- [ ] **Step 6: Run tests**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo test -p waved-uart
```
Expected: 6 tests pass (all frame parsing tests).

- [ ] **Step 7: Commit**

```bash
git add waved/crates/waved-uart/
git commit -m "feat: add UART mux with frame parser and reader/writer threads"
```

---

### Task 5: MCU Service (Battery, Motor, Buttons)

**Files:**
- Create: `waved/crates/waved-mcu/Cargo.toml`
- Create: `waved/crates/waved-mcu/src/lib.rs`
- Create: `waved/crates/waved-mcu/src/battery.rs`
- Create: `waved/crates/waved-mcu/src/motor.rs`
- Create: `waved/crates/waved-mcu/src/buttons.rs`

- [ ] **Step 1: Create crate**

```bash
mkdir -p waved/crates/waved-mcu/src
```

- [ ] **Step 2: Write Cargo.toml**

Create `waved/crates/waved-mcu/Cargo.toml`:
```toml
[package]
name = "waved-mcu"
version.workspace = true
edition.workspace = true

[dependencies]
waved-state = { path = "../waved-state" }
waved-uart = { path = "../waved-uart" }
tokio = { workspace = true, features = ["sync"] }
crossbeam-channel.workspace = true
tracing.workspace = true
thiserror.workspace = true
```

- [ ] **Step 3: Write battery parsing**

Create `waved/crates/waved-mcu/src/battery.rs`:
```rust
use waved_state::BatteryState;

/// Parse battery status from MCU frame data.
/// Frame format is reverse-engineered from decompiled firmware.
/// The exact byte layout will be refined during on-device testing.
pub fn parse_battery_frame(data: &[u8]) -> Option<BatteryState> {
    if data.len() < 5 {
        return None;
    }
    // Placeholder parsing — exact offsets TBD from on-device tracing
    Some(BatteryState {
        level: data[0],
        voltage: u16::from_le_bytes([data[1], data[2]]),
        temperature: data[3] as i8,
        is_charging: data[4] != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_battery() {
        let data = [85, 0x20, 0x0F, 25, 1]; // 85%, 3872mV, 25°C, charging
        let state = parse_battery_frame(&data).unwrap();
        assert_eq!(state.level, 85);
        assert!(state.is_charging);
        assert_eq!(state.temperature, 25);
    }

    #[test]
    fn parse_short_frame_returns_none() {
        assert!(parse_battery_frame(&[1, 2]).is_none());
    }
}
```

- [ ] **Step 4: Write motor commands**

Create `waved/crates/waved-mcu/src/motor.rs`:
```rust
use waved_state::MotorPosition;
use waved_uart::frame::Frame;

/// Motor command IDs (reverse-engineered, refined during testing).
const MCU_HEADER: [u8; 2] = [0xDC, 0xCD];

pub fn build_motor_command(position: &MotorPosition) -> Vec<u8> {
    let cmd_byte = match position {
        MotorPosition::Low => 0x01,
        MotorPosition::Medium => 0x02,
        MotorPosition::High => 0x03,
        MotorPosition::Moving => return vec![], // not a command
    };
    // Motor command frame: [header] [len=2] [cmd_id=0x20] [position] [checksum]
    Frame::build(MCU_HEADER, &[0x20, cmd_byte]).unwrap_or_default()
}

pub fn build_motor_stop() -> Vec<u8> {
    Frame::build(MCU_HEADER, &[0x21]).unwrap_or_default()
}

pub fn build_shutdown_command() -> Vec<u8> {
    Frame::build(MCU_HEADER, &[0x30]).unwrap_or_default()
}

pub fn build_reboot_command() -> Vec<u8> {
    Frame::build(MCU_HEADER, &[0x31]).unwrap_or_default()
}
```

- [ ] **Step 5: Write button parsing**

Create `waved/crates/waved-mcu/src/buttons.rs`:
```rust
use waved_state::ButtonType;

/// Parse button event from MCU frame data.
pub fn parse_button_frame(data: &[u8]) -> Option<ButtonType> {
    if data.is_empty() {
        return None;
    }
    // Button event command IDs (refined during on-device testing)
    match data[0] {
        0x40 => Some(ButtonType::MuteToggle),
        0x41 => Some(ButtonType::VolumeUp),
        0x42 => Some(ButtonType::VolumeDown),
        0x43 => Some(ButtonType::MuteLongPress),
        _ => None,
    }
}
```

- [ ] **Step 6: Write MCU service**

Create `waved/crates/waved-mcu/src/lib.rs`:
```rust
pub mod battery;
pub mod buttons;
pub mod motor;

use crossbeam_channel::Receiver;
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};
use waved_state::{BatteryState, ButtonType, Command, DeviceEvent, McuState, MotorPosition};
use waved_uart::{Frame, UartMux};

pub struct McuService {
    mcu_rx: Receiver<Frame>,
    uart: std::sync::Arc<UartMux>,
    battery_tx: watch::Sender<BatteryState>,
    mcu_tx: watch::Sender<McuState>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl McuService {
    pub fn new(
        mcu_rx: Receiver<Frame>,
        uart: std::sync::Arc<UartMux>,
        battery_tx: watch::Sender<BatteryState>,
        mcu_tx: watch::Sender<McuState>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        Self {
            mcu_rx,
            uart,
            battery_tx,
            mcu_tx,
            events_tx,
        }
    }

    /// Run the MCU service on a dedicated thread (blocking, reads from crossbeam channel).
    pub fn run(self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("mcu_srv started");

        loop {
            crossbeam_channel::select! {
                recv(self.mcu_rx) -> msg => {
                    match msg {
                        Ok(frame) => self.handle_frame(&frame),
                        Err(_) => {
                            warn!("mcu channel closed, stopping");
                            return;
                        }
                    }
                }
                default(std::time::Duration::from_millis(50)) => {
                    // Check for commands
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        self.handle_command(&cmd);
                    }
                }
            }
        }
    }

    fn handle_frame(&self, frame: &Frame) {
        if frame.data.is_empty() {
            return;
        }

        let cmd_id = frame.data[0];
        let payload = &frame.data[1..];

        match cmd_id {
            0x10..=0x1F => {
                // Battery status range
                if let Some(state) = battery::parse_battery_frame(payload) {
                    debug!("battery: {}% {}mV charging={}", state.level, state.voltage, state.is_charging);
                    if state.level < 10 {
                        let _ = self.events_tx.send(DeviceEvent::LowBattery(state.level));
                    }
                    let _ = self.battery_tx.send(state);
                }
            }
            0x40..=0x4F => {
                // Button events
                if let Some(btn) = buttons::parse_button_frame(&frame.data) {
                    debug!("button: {:?}", btn);
                    let _ = self.events_tx.send(DeviceEvent::ButtonPress(btn));
                }
            }
            0x50 => {
                // USB connected
                let _ = self.events_tx.send(DeviceEvent::UsbConnected);
                self.mcu_tx.send_modify(|s| s.usb_connected = true);
            }
            0x51 => {
                // USB disconnected
                let _ = self.events_tx.send(DeviceEvent::UsbDisconnected);
                self.mcu_tx.send_modify(|s| s.usb_connected = false);
            }
            0x60 => {
                // Motor obstruction
                let _ = self.events_tx.send(DeviceEvent::MotorObstruction);
            }
            _ => {
                debug!("unknown MCU command: {:#04x}", cmd_id);
            }
        }
    }

    fn handle_command(&self, cmd: &Command) {
        match cmd {
            Command::SetMotorPosition(pos) => {
                let raw = motor::build_motor_command(pos);
                if !raw.is_empty() {
                    self.uart.send(raw, true); // priority = true for motor
                    self.mcu_tx.send_modify(|s| s.motor_position = MotorPosition::Moving);
                }
            }
            Command::Shutdown => {
                info!("shutdown requested, retracting motor first");
                let retract = motor::build_motor_command(&MotorPosition::Low);
                if !retract.is_empty() {
                    self.uart.send(retract, true);
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
                let shutdown = motor::build_shutdown_command();
                self.uart.send(shutdown, true);
            }
            _ => {} // Not for us
        }
    }
}
```

- [ ] **Step 7: Run tests and verify compilation**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo test -p waved-mcu
cargo check
```

- [ ] **Step 8: Commit**

```bash
git add waved/crates/waved-mcu/
git commit -m "feat: add MCU service with battery, motor, and button handling"
```

---

### Task 6: HID Service (USB Mute/Volume)

**Files:**
- Create: `waved/crates/waved-hid/Cargo.toml`
- Create: `waved/crates/waved-hid/src/lib.rs`

- [ ] **Step 1: Create crate**

```bash
mkdir -p waved/crates/waved-hid/src
```

- [ ] **Step 2: Write HID service**

Create `waved/crates/waved-hid/Cargo.toml`:
```toml
[package]
name = "waved-hid"
version.workspace = true
edition.workspace = true

[dependencies]
waved-state = { path = "../waved-state" }
tokio = { workspace = true, features = ["sync", "io-util", "fs"] }
tracing.workspace = true
thiserror.workspace = true
```

Create `waved/crates/waved-hid/src/lib.rs`:
```rust
use std::path::Path;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
use waved_state::{ButtonType, DeviceEvent};

const HID_GADGET_PATH: &str = "/dev/hidg0";

// HID report IDs for Telephone and Consumer usage pages
// These match the stock USB gadget descriptor in S89usbgadget
const REPORT_ID_TELEPHONE: u8 = 0x01;
const REPORT_ID_CONSUMER: u8 = 0x02;

pub struct HidService;

impl HidService {
    pub async fn run(mut events_rx: broadcast::Receiver<DeviceEvent>) {
        info!("hid_srv started");

        if !Path::new(HID_GADGET_PATH).exists() {
            warn!("{HID_GADGET_PATH} not found, HID disabled (not running on device?)");
            // Keep running but do nothing — allows host development
            loop {
                match events_rx.recv().await {
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("hid_srv lagged by {n} events");
                    }
                }
            }
        }

        loop {
            match events_rx.recv().await {
                Ok(DeviceEvent::ButtonPress(btn)) => {
                    if let Err(e) = Self::send_hid_report(&btn).await {
                        error!("HID write error: {e}");
                    }
                }
                Ok(_) => {} // Ignore non-button events
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("hid_srv lagged by {n} events");
                }
            }
        }
    }

    async fn send_hid_report(button: &ButtonType) -> std::io::Result<()> {
        let report = match button {
            ButtonType::MuteToggle => {
                // Telephone usage page: Hook Switch toggle
                vec![REPORT_ID_TELEPHONE, 0x01] // bit 0 = mute
            }
            ButtonType::VolumeUp => {
                // Consumer usage page: Volume Up
                vec![REPORT_ID_CONSUMER, 0x01]
            }
            ButtonType::VolumeDown => {
                // Consumer usage page: Volume Down
                vec![REPORT_ID_CONSUMER, 0x02]
            }
            ButtonType::MuteLongPress => {
                // Treat as regular mute toggle for now
                vec![REPORT_ID_TELEPHONE, 0x01]
            }
        };

        debug!("sending HID report: {:02x?}", report);

        let mut file = OpenOptions::new()
            .write(true)
            .open(HID_GADGET_PATH)
            .await?;
        file.write_all(&report).await?;

        // Send release report (all zeros) after a short delay
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let release = vec![report[0], 0x00];
        file.write_all(&release).await?;

        Ok(())
    }
}
```

- [ ] **Step 3: Verify compilation**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-hid
```

- [ ] **Step 4: Commit**

```bash
git add waved/crates/waved-hid/
git commit -m "feat: add USB HID service for mute/volume button reports"
```

---

### Task 7: WiFi Service

**Files:**
- Create: `waved/crates/waved-wifi/Cargo.toml`
- Create: `waved/crates/waved-wifi/src/lib.rs`
- Create: `waved/crates/waved-wifi/src/wpa.rs`

- [ ] **Step 1: Create crate and write WiFi service**

```bash
mkdir -p waved/crates/waved-wifi/src
```

Create `waved/crates/waved-wifi/Cargo.toml`:
```toml
[package]
name = "waved-wifi"
version.workspace = true
edition.workspace = true

[dependencies]
waved-state = { path = "../waved-state" }
waved-config = { path = "../waved-config" }
tokio = { workspace = true, features = ["sync", "process", "time"] }
tracing.workspace = true
thiserror.workspace = true
```

Create `waved/crates/waved-wifi/src/wpa.rs`:
```rust
use std::process::Output;
use tokio::process::Command;
use tracing::{debug, warn};

/// Run wpa_cli command and return stdout.
async fn wpa_cli(args: &[&str]) -> Result<String, std::io::Error> {
    let output: Output = Command::new("wpa_cli")
        .args(args)
        .output()
        .await?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    debug!("wpa_cli {:?} → {}", args, stdout.trim());
    Ok(stdout)
}

pub async fn scan() -> Vec<String> {
    if let Err(e) = wpa_cli(&["scan"]).await {
        warn!("wifi scan trigger failed: {e}");
        return vec![];
    }
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    match wpa_cli(&["scan_results"]).await {
        Ok(output) => output
            .lines()
            .skip(1) // header line
            .filter_map(|line| line.split('\t').nth(4).map(String::from))
            .filter(|ssid| !ssid.is_empty())
            .collect(),
        Err(_) => vec![],
    }
}

pub async fn connect(ssid: &str, password: &str) -> bool {
    // Note: never log the password
    let id = match wpa_cli(&["add_network"]).await {
        Ok(s) => s.trim().to_string(),
        Err(_) => return false,
    };
    let _ = wpa_cli(&["set_network", &id, "ssid", &format!("\"{}\"", ssid)]).await;
    let _ = wpa_cli(&["set_network", &id, "psk", &format!("\"{}\"", password)]).await;
    let _ = wpa_cli(&["enable_network", &id]).await;
    let _ = wpa_cli(&["save_config"]).await;
    true
}

pub async fn disconnect() -> bool {
    wpa_cli(&["disconnect"]).await.is_ok()
}

pub async fn status() -> Option<(String, String)> {
    let output = wpa_cli(&["status"]).await.ok()?;
    let mut ssid = None;
    let mut ip = None;
    for line in output.lines() {
        if let Some(val) = line.strip_prefix("ssid=") {
            ssid = Some(val.to_string());
        }
        if let Some(val) = line.strip_prefix("ip_address=") {
            ip = Some(val.to_string());
        }
    }
    match (ssid, ip) {
        (Some(s), Some(i)) => Some((s, i)),
        _ => None,
    }
}
```

Create `waved/crates/waved-wifi/src/lib.rs`:
```rust
pub mod wpa;

use tokio::sync::{broadcast, watch};
use tracing::{debug, info};
use waved_state::{DeviceEvent, WifiState};

pub struct WifiService {
    wifi_tx: watch::Sender<WifiState>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl WifiService {
    pub fn new(
        wifi_tx: watch::Sender<WifiState>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        Self { wifi_tx, events_tx }
    }

    /// Poll WiFi status every 10 seconds and update state bus.
    pub async fn run(self) {
        info!("wifi_srv started");
        let mut was_connected = false;

        loop {
            let state = match wpa::status().await {
                Some((ssid, ip)) => {
                    if !was_connected {
                        info!("wifi connected: {ssid} ({ip})");
                        let _ = self.events_tx.send(DeviceEvent::WifiConnected(ssid.clone()));
                        was_connected = true;
                    }
                    WifiState {
                        connected: true,
                        ssid,
                        ip_addr: ip,
                        signal_strength: 0, // TODO: parse from wpa_cli signal_poll
                    }
                }
                None => {
                    if was_connected {
                        info!("wifi disconnected");
                        let _ = self.events_tx.send(DeviceEvent::WifiDisconnected);
                        was_connected = false;
                    }
                    WifiState::default()
                }
            };
            let _ = self.wifi_tx.send(state);

            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    }
}
```

- [ ] **Step 2: Verify compilation**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-wifi
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-wifi/
git commit -m "feat: add WiFi service with wpa_supplicant CLI wrapper"
```

---

### Task 8: Meeting Service (Recording + Upload)

**Files:**
- Create: `waved/crates/waved-meeting/Cargo.toml`
- Create: `waved/crates/waved-meeting/src/lib.rs`
- Create: `waved/crates/waved-meeting/src/recorder.rs`
- Create: `waved/crates/waved-meeting/src/upload.rs`
- Create: `waved/crates/waved-meeting/src/db.rs`

- [ ] **Step 1: Create crate**

```bash
mkdir -p waved/crates/waved-meeting/src
```

- [ ] **Step 2: Write Cargo.toml**

Create `waved/crates/waved-meeting/Cargo.toml`:
```toml
[package]
name = "waved-meeting"
version.workspace = true
edition.workspace = true

[dependencies]
waved-state = { path = "../waved-state" }
waved-config = { path = "../waved-config" }
tokio = { workspace = true, features = ["sync", "time", "fs"] }
uuid.workspace = true
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
thiserror.workspace = true
reqwest = { version = "0.12", features = ["json", "multipart"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }
chrono = { version = "0.4", features = ["serde"] }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Write SQLite schema and queries**

Create `waved/crates/waved-meeting/src/db.rs`:
```rust
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tracing::info;
use uuid::Uuid;

pub async fn init_pool(path: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite:{path}?mode=rwc"))
        .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS meetings (
            id TEXT PRIMARY KEY,
            start_time TEXT NOT NULL,
            end_time TEXT,
            duration_secs INTEGER DEFAULT 0,
            audio_mode TEXT NOT NULL,
            file_path TEXT NOT NULL,
            upload_status TEXT NOT NULL DEFAULT 'pending',
            upload_attempts INTEGER DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS bookmarks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            meeting_id TEXT NOT NULL,
            timestamp_secs INTEGER NOT NULL,
            FOREIGN KEY (meeting_id) REFERENCES meetings(id)
        )",
    )
    .execute(&pool)
    .await?;

    info!("meeting database initialized at {path}");
    Ok(pool)
}

pub async fn insert_meeting(
    pool: &SqlitePool,
    id: &Uuid,
    audio_mode: &str,
    file_path: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO meetings (id, start_time, audio_mode, file_path, upload_status)
         VALUES (?, datetime('now'), ?, ?, 'pending')",
    )
    .bind(id.to_string())
    .bind(audio_mode)
    .bind(file_path)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn finish_meeting(
    pool: &SqlitePool,
    id: &Uuid,
    duration_secs: u32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE meetings SET end_time = datetime('now'), duration_secs = ? WHERE id = ?",
    )
    .bind(duration_secs)
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_upload_status(
    pool: &SqlitePool,
    id: &Uuid,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE meetings SET upload_status = ?, upload_attempts = upload_attempts + 1 WHERE id = ?")
        .bind(status)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_pending_uploads(pool: &SqlitePool) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, file_path FROM meetings WHERE upload_status = 'pending' OR upload_status = 'upload_failed' ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn add_bookmark(
    pool: &SqlitePool,
    meeting_id: &Uuid,
    timestamp_secs: u32,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO bookmarks (meeting_id, timestamp_secs) VALUES (?, ?)")
        .bind(meeting_id.to_string())
        .bind(timestamp_secs)
        .execute(pool)
        .await?;
    Ok(())
}
```

- [ ] **Step 4: Write upload module**

Create `waved/crates/waved-meeting/src/upload.rs`:
```rust
use reqwest::multipart;
use std::path::Path;
use tracing::{error, info};

pub async fn upload_recording(
    endpoint: &str,
    auth_token: &str,
    meeting_id: &str,
    file_path: &Path,
) -> Result<(), UploadError> {
    let file_bytes = tokio::fs::read(file_path).await.map_err(UploadError::Io)?;
    let file_name = file_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let metadata_path = file_path.with_file_name("metadata.json");
    let metadata_bytes = if metadata_path.exists() {
        tokio::fs::read(&metadata_path).await.ok()
    } else {
        None
    };

    let mut form = multipart::Form::new()
        .text("meeting_id", meeting_id.to_string())
        .part(
            "audio",
            multipart::Part::bytes(file_bytes)
                .file_name(file_name)
                .mime_str("audio/opus")
                .unwrap(),
        );

    if let Some(meta) = metadata_bytes {
        form = form.part(
            "metadata",
            multipart::Part::bytes(meta)
                .file_name("metadata.json")
                .mime_str("application/json")
                .unwrap(),
        );
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(endpoint)
        .bearer_auth(auth_token)
        .multipart(form)
        .send()
        .await
        .map_err(UploadError::Http)?;

    if resp.status().is_success() {
        info!("uploaded meeting {meeting_id} successfully");
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        error!("upload failed: {status} — {body}");
        Err(UploadError::ServerError(status.as_u16(), body))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server error {0}: {1}")]
    ServerError(u16, String),
}
```

- [ ] **Step 5: Write recorder stub and service**

Create `waved/crates/waved-meeting/src/recorder.rs`:
```rust
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::info;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
pub struct MeetingMetadata {
    pub id: String,
    pub start_time: String,
    pub duration_secs: u32,
    pub audio_mode: String,
    pub device_id: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub codec: String,
}

pub struct ActiveRecording {
    pub id: Uuid,
    pub dir: PathBuf,
    pub start: std::time::Instant,
    // TODO: Opus encoder handle will go here once audio_srv delivers frames
}

impl ActiveRecording {
    pub async fn start(base_dir: &Path, audio_mode: &str) -> std::io::Result<Self> {
        let id = Uuid::new_v4();
        let dir = base_dir.join(id.to_string());
        fs::create_dir_all(&dir).await?;

        let metadata = MeetingMetadata {
            id: id.to_string(),
            start_time: chrono::Utc::now().to_rfc3339(),
            duration_secs: 0,
            audio_mode: audio_mode.to_string(),
            device_id: String::new(), // filled from config
            sample_rate: 48000,
            channels: 2,
            codec: "opus".to_string(),
        };

        let meta_json = serde_json::to_string_pretty(&metadata).unwrap();
        fs::write(dir.join("metadata.json"), meta_json).await?;

        // Create empty audio file (Opus frames appended during recording)
        fs::write(dir.join("audio.opus"), []).await?;

        info!("recording started: {} at {}", id, dir.display());
        Ok(Self {
            id,
            dir,
            start: std::time::Instant::now(),
        })
    }

    pub fn duration_secs(&self) -> u32 {
        self.start.elapsed().as_secs() as u32
    }

    pub fn audio_path(&self) -> PathBuf {
        self.dir.join("audio.opus")
    }
}
```

Create `waved/crates/waved-meeting/src/lib.rs`:
```rust
pub mod db;
pub mod recorder;
pub mod upload;

use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tracing::{error, info, warn};
use waved_config::WavedConfig;
use waved_state::{Command, DeviceEvent, MeetingState, WifiState};

pub struct MeetingService {
    config: WavedConfig,
    pool: SqlitePool,
    recordings_dir: PathBuf,
    meeting_tx: watch::Sender<MeetingState>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl MeetingService {
    pub async fn new(
        config: WavedConfig,
        db_path: &str,
        recordings_dir: PathBuf,
        meeting_tx: watch::Sender<MeetingState>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Result<Self, sqlx::Error> {
        let pool = db::init_pool(db_path).await?;
        Ok(Self {
            config,
            pool,
            recordings_dir,
            meeting_tx,
            events_tx,
        })
    }

    pub async fn run(
        self,
        mut cmd_rx: broadcast::Receiver<Command>,
        mut wifi_rx: watch::Receiver<WifiState>,
    ) {
        info!("meeting_srv started");
        let mut active: Option<recorder::ActiveRecording> = None;

        loop {
            tokio::select! {
                Ok(cmd) = cmd_rx.recv() => {
                    match cmd {
                        Command::StartRecording => {
                            if active.is_some() {
                                warn!("already recording, ignoring start");
                                continue;
                            }
                            match recorder::ActiveRecording::start(
                                &self.recordings_dir,
                                &format!("{:?}", self.config.audio.mode),
                            ).await {
                                Ok(rec) => {
                                    let id = rec.id;
                                    let _ = db::insert_meeting(
                                        &self.pool, &id,
                                        &format!("{:?}", self.config.audio.mode),
                                        &rec.dir.to_string_lossy(),
                                    ).await;
                                    let _ = self.events_tx.send(DeviceEvent::RecordingStarted(id));
                                    self.meeting_tx.send_modify(|s| {
                                        s.recording = true;
                                        s.meeting_id = Some(id);
                                        s.duration_secs = 0;
                                    });
                                    active = Some(rec);
                                }
                                Err(e) => error!("failed to start recording: {e}"),
                            }
                        }
                        Command::StopRecording => {
                            if let Some(rec) = active.take() {
                                let duration = rec.duration_secs();
                                let id = rec.id;
                                let _ = db::finish_meeting(&self.pool, &id, duration).await;
                                let _ = self.events_tx.send(DeviceEvent::RecordingStopped(id));
                                self.meeting_tx.send_modify(|s| {
                                    s.recording = false;
                                    s.paused = false;
                                    s.meeting_id = None;
                                    s.duration_secs = duration;
                                });
                                info!("recording stopped: {} ({}s)", id, duration);
                                // Trigger upload if auto_upload and wifi connected
                                if self.config.api.auto_upload && wifi_rx.borrow().connected {
                                    self.try_upload(&id.to_string(), &rec.audio_path()).await;
                                }
                            }
                        }
                        Command::AddBookmark => {
                            if let Some(ref rec) = active {
                                let ts = rec.duration_secs();
                                let _ = db::add_bookmark(&self.pool, &rec.id, ts).await;
                                info!("bookmark at {}s", ts);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(()) = wifi_rx.changed() => {
                    // WiFi just connected — flush pending uploads
                    if wifi_rx.borrow().connected {
                        self.flush_pending_uploads().await;
                    }
                }
            }
        }
    }

    async fn try_upload(&self, meeting_id: &str, audio_path: &std::path::Path) {
        match upload::upload_recording(
            &self.config.api.endpoint,
            &self.config.api.auth_token,
            meeting_id,
            audio_path,
        ).await {
            Ok(()) => {
                if let Ok(uuid) = meeting_id.parse() {
                    let _ = db::set_upload_status(&self.pool, &uuid, "uploaded").await;
                }
            }
            Err(e) => {
                warn!("upload failed for {meeting_id}: {e}");
                if let Ok(uuid) = meeting_id.parse() {
                    let _ = db::set_upload_status(&self.pool, &uuid, "upload_failed").await;
                }
            }
        }
    }

    async fn flush_pending_uploads(&self) {
        match db::get_pending_uploads(&self.pool).await {
            Ok(pending) => {
                for (id, path) in pending {
                    let audio_path = std::path::PathBuf::from(&path).join("audio.opus");
                    self.try_upload(&id, &audio_path).await;
                }
            }
            Err(e) => error!("failed to get pending uploads: {e}"),
        }
    }
}
```

- [ ] **Step 6: Verify compilation**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-meeting
```

- [ ] **Step 7: Commit**

```bash
git add waved/crates/waved-meeting/
git commit -m "feat: add meeting service with recording, SQLite DB, and HTTP upload"
```

---

### Task 9: Bluetooth Service (Basic)

**Files:**
- Create: `waved/crates/waved-bt/Cargo.toml`
- Create: `waved/crates/waved-bt/src/lib.rs`
- Create: `waved/crates/waved-bt/src/commands.rs`

- [ ] **Step 1: Create crate and write BT command builders**

```bash
mkdir -p waved/crates/waved-bt/src
```

Create `waved/crates/waved-bt/Cargo.toml`:
```toml
[package]
name = "waved-bt"
version.workspace = true
edition.workspace = true

[dependencies]
waved-state = { path = "../waved-state" }
waved-uart = { path = "../waved-uart" }
tokio = { workspace = true, features = ["sync"] }
crossbeam-channel.workspace = true
tracing.workspace = true
```

Create `waved/crates/waved-bt/src/commands.rs`:
```rust
use waved_uart::frame::Frame;

const BT_HEADER: [u8; 2] = [0x75, 0x46];

pub fn build_get_status() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x0E]).unwrap_or_default()
}

pub fn build_get_mac() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x09]).unwrap_or_default()
}

pub fn build_get_name() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x0A]).unwrap_or_default()
}

pub fn build_get_version() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x0D]).unwrap_or_default()
}

pub fn build_open() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x07]).unwrap_or_default()
}

pub fn build_close() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x03]).unwrap_or_default()
}

pub fn build_disconnect() -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x02]).unwrap_or_default()
}

pub fn build_set_auto_reconnect(enable: bool) -> Vec<u8> {
    Frame::build(BT_HEADER, &[0x04, if enable { 1 } else { 0 }]).unwrap_or_default()
}
```

Create `waved/crates/waved-bt/src/lib.rs`:
```rust
pub mod commands;

use crossbeam_channel::Receiver;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};
use waved_state::{BluetoothState, DeviceEvent};
use waved_uart::{Frame, UartMux};

pub struct BtService {
    bt_rx: Receiver<Frame>,
    uart: Arc<UartMux>,
    bt_tx: watch::Sender<BluetoothState>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl BtService {
    pub fn new(
        bt_rx: Receiver<Frame>,
        uart: Arc<UartMux>,
        bt_tx: watch::Sender<BluetoothState>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        Self { bt_rx, uart, bt_tx, events_tx }
    }

    pub fn run(self) {
        info!("bt_srv started");

        // Request initial status
        self.uart.send(commands::build_get_status(), false);
        self.uart.send(commands::build_get_mac(), false);
        self.uart.send(commands::build_get_name(), false);

        loop {
            match self.bt_rx.recv() {
                Ok(frame) => self.handle_frame(&frame),
                Err(_) => {
                    warn!("bt channel closed, stopping");
                    return;
                }
            }
        }
    }

    fn handle_frame(&self, frame: &Frame) {
        if frame.data.is_empty() {
            return;
        }

        let cmd_id = frame.data[0];
        let payload = &frame.data[1..];

        match cmd_id {
            0x01 => {
                // Connect reply
                let connected = !payload.is_empty() && payload[0] == 1;
                if connected {
                    let name = String::from_utf8_lossy(&payload[1..]).trim_end_matches('\0').to_string();
                    info!("bluetooth connected: {name}");
                    let _ = self.events_tx.send(DeviceEvent::BtConnected(name.clone()));
                    self.bt_tx.send_modify(|s| {
                        s.connected = true;
                        s.device_name = name;
                    });
                }
            }
            0x02 => {
                // Disconnect
                info!("bluetooth disconnected");
                let _ = self.events_tx.send(DeviceEvent::BtDisconnected);
                self.bt_tx.send_modify(|s| {
                    s.connected = false;
                    s.device_name.clear();
                });
            }
            0x09 => {
                // MAC address response
                let mac = String::from_utf8_lossy(payload).trim_end_matches('\0').to_string();
                debug!("bt mac: {mac}");
                self.bt_tx.send_modify(|s| s.mac = mac);
            }
            0x0E => {
                // Status response
                let connected = !payload.is_empty() && payload[0] != 0;
                self.bt_tx.send_modify(|s| s.connected = connected);
            }
            _ => {
                debug!("unhandled bt response: cmd={:#04x} len={}", cmd_id, payload.len());
            }
        }
    }
}
```

- [ ] **Step 2: Verify compilation**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-bt
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-bt/
git commit -m "feat: add Bluetooth service with command builders and status tracking"
```

---

### Task 10: Audio Service Stub (TEA SDK FFI placeholder)

**Files:**
- Create: `waved/crates/waved-audio/Cargo.toml`
- Create: `waved/crates/waved-audio/src/lib.rs`
- Create: `waved/crates/waved-audio/src/ffi.rs`
- Create: `waved/crates/waved-audio/src/callbacks.rs`

This task creates the audio service structure with FFI stubs. The actual FFI bindings to `libInstaAudio.so` require on-device testing and header extraction. For now, the stubs allow the rest of the system to compile and the interfaces to be validated.

- [ ] **Step 1: Create crate**

```bash
mkdir -p waved/crates/waved-audio/src
```

Create `waved/crates/waved-audio/Cargo.toml`:
```toml
[package]
name = "waved-audio"
version.workspace = true
edition.workspace = true

[dependencies]
waved-state = { path = "../waved-state" }
tokio = { workspace = true, features = ["sync"] }
tracing.workspace = true
thiserror.workspace = true
```

Create `waved/crates/waved-audio/src/ffi.rs`:
```rust
//! FFI stubs for libInstaAudio.so.
//! These will be replaced with real extern "C" declarations once
//! we extract headers from the device and validate on-device.
//!
//! The actual functions live in libInstaAudio.so on the device.
//! They are NOT available on the host during cross-compilation.
//! Use cfg(target_arch = "aarch64") to gate real FFI calls.

use tracing::info;

/// Initialize the TEA SDK audio pipeline.
/// On device: calls Insta360_InitAudio() via FFI.
/// On host: logs and returns success.
pub fn init_audio() -> Result<(), String> {
    #[cfg(target_arch = "aarch64")]
    {
        // TODO: extern "C" { fn Insta360_InitAudio() -> i32; }
        // let ret = unsafe { Insta360_InitAudio() };
        // if ret != 0 { return Err(format!("init failed: {ret}")); }
        info!("audio init: would call Insta360_InitAudio() on device");
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        info!("audio init: stub (not on device)");
    }
    Ok(())
}

/// Set audio processing mode.
/// mode: 1=NoiseReduction, 2=Omni, 3=Cardioid, 4=Supercardioid, 5=Figure8, 6=Stereo
pub fn set_work_mode(mode: u32) -> Result<(), String> {
    info!("set_work_mode({mode}): stub");
    Ok(())
}

/// Enable/disable audio dumps for debugging.
pub fn set_dump_enable(enable: bool) {
    info!("set_dump_enable({enable}): stub");
}
```

Create `waved/crates/waved-audio/src/callbacks.rs`:
```rust
//! Callback bridge: C callbacks from TEA SDK → tokio mpsc channels.
//!
//! On device, we register extern "C" functions that TEA SDK calls
//! when audio frames are ready. These push into mpsc channels
//! that the async meeting_srv consumes.

use tokio::sync::mpsc;

/// A processed audio frame from the TEA SDK 3A output.
#[derive(Clone)]
pub struct AudioFrame {
    pub data: Vec<i16>,       // Interleaved PCM samples
    pub sample_rate: u32,     // 48000
    pub channels: u16,        // 2 (stereo)
    pub timestamp_ms: u64,
}

/// Creates the callback channel pair.
/// Returns (sender for C callback, receiver for async consumer).
pub fn create_audio_channel() -> (mpsc::Sender<AudioFrame>, mpsc::Receiver<AudioFrame>) {
    mpsc::channel(32) // Buffer 32 frames (~640ms at 20ms/frame)
}
```

Create `waved/crates/waved-audio/src/lib.rs`:
```rust
pub mod callbacks;
pub mod ffi;

use tokio::sync::{broadcast, mpsc, watch};
use tracing::{error, info};
use waved_state::{AudioMode, AudioState, Command};

pub struct AudioService {
    audio_tx: watch::Sender<AudioState>,
    frame_tx: mpsc::Sender<callbacks::AudioFrame>,
}

impl AudioService {
    pub fn new(
        audio_tx: watch::Sender<AudioState>,
        frame_tx: mpsc::Sender<callbacks::AudioFrame>,
    ) -> Self {
        Self { audio_tx, frame_tx }
    }

    pub async fn run(self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("audio_srv starting");

        // Initialize TEA SDK on a blocking thread
        let init_result = tokio::task::spawn_blocking(|| ffi::init_audio()).await;
        match init_result {
            Ok(Ok(())) => info!("TEA SDK initialized"),
            Ok(Err(e)) => {
                error!("TEA SDK init failed: {e}");
                return;
            }
            Err(e) => {
                error!("TEA SDK init thread panicked: {e}");
                return;
            }
        }

        loop {
            match cmd_rx.recv().await {
                Ok(Command::SetAudioMode(mode)) => {
                    let mode_num = match mode {
                        AudioMode::NoiseReduction => 1,
                        AudioMode::Omni => 2,
                        AudioMode::Cardioid => 3,
                        AudioMode::Supercardioid => 4,
                        AudioMode::Figure8 => 5,
                        AudioMode::Stereo => 6,
                    };
                    let _ = tokio::task::spawn_blocking(move || ffi::set_work_mode(mode_num)).await;
                    self.audio_tx.send_modify(|s| s.mode = mode);
                }
                Ok(Command::ToggleMicMute) => {
                    self.audio_tx.send_modify(|s| s.mic_muted = !s.mic_muted);
                }
                Ok(Command::ToggleSpeakerMute) => {
                    self.audio_tx.send_modify(|s| s.speaker_muted = !s.speaker_muted);
                }
                Ok(Command::SetSpeakerVolume(v)) => {
                    self.audio_tx.send_modify(|s| s.speaker_volume = v);
                }
                Ok(Command::SetMicVolume(v)) => {
                    self.audio_tx.send_modify(|s| s.mic_volume = v);
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("audio_srv lagged {n} commands");
                }
            }
        }
    }
}
```

- [ ] **Step 2: Verify compilation**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-audio
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-audio/
git commit -m "feat: add audio service with TEA SDK FFI stubs and callback channel"
```

---

### Task 11: Wire Up Main Binary + Deploy Scripts

**Files:**
- Modify: `waved/crates/waved/Cargo.toml`
- Modify: `waved/crates/waved/src/main.rs`
- Create: `waved/deploy/install.sh`
- Create: `waved/deploy/revert.sh`
- Create: `waved/deploy/S60waved`
- Create: `waved/deploy/iptables.rules`

- [ ] **Step 1: Update main binary Cargo.toml with all dependencies**

Update `waved/crates/waved/Cargo.toml`:
```toml
[package]
name = "waved"
version.workspace = true
edition.workspace = true

[[bin]]
name = "waved"
path = "src/main.rs"

[dependencies]
waved-state = { path = "../waved-state" }
waved-config = { path = "../waved-config" }
waved-uart = { path = "../waved-uart" }
waved-mcu = { path = "../waved-mcu" }
waved-bt = { path = "../waved-bt" }
waved-hid = { path = "../waved-hid" }
waved-audio = { path = "../waved-audio" }
waved-meeting = { path = "../waved-meeting" }
waved-wifi = { path = "../waved-wifi" }
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
nix.workspace = true
```

- [ ] **Step 2: Write main.rs wiring all modules together**

Replace `waved/crates/waved/src/main.rs`:
```rust
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use waved_state::StateBus;

const CONFIG_PATH: &str = "/data/waved/config.toml";
const DB_PATH: &str = "/data/waved/meetings.db";
const RECORDINGS_DIR: &str = "/mnt/udisk/recordings";
const UART_PORT: &str = "/dev/ttyS1";
const UART_BAUD: u32 = 115200;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("waved=info".parse().unwrap()),
        )
        .init();

    info!("waved v{} starting", env!("CARGO_PKG_VERSION"));

    // Load config
    let config = waved_config::WavedConfig::load_or_default(CONFIG_PATH.as_ref());
    info!("config loaded: device={} audio_mode={:?}", config.device.name, config.audio.mode);

    // Create state bus
    let bus = Arc::new(StateBus::new());

    // Open UART mux
    let uart = match waved_uart::UartMux::open(UART_PORT, UART_BAUD) {
        Ok(u) => {
            info!("UART opened: {UART_PORT} @ {UART_BAUD}");
            Arc::new(u)
        }
        Err(e) => {
            error!("failed to open UART {UART_PORT}: {e}");
            error!("running without MCU/BT (host development mode)");
            // In host mode, we skip MCU/BT services
            // Build runtime and run async-only services
            run_async_only(config, bus);
            return;
        }
    };

    // Spawn MCU service on dedicated thread
    let mcu_rx = uart.mcu_receiver().clone();
    let mcu_uart = Arc::clone(&uart);
    let mcu_battery_tx = bus.battery.clone();
    let mcu_state_tx = bus.mcu.clone();
    let mcu_events_tx = bus.events.clone();
    let mcu_cmd_rx = bus.commands.subscribe();
    std::thread::Builder::new()
        .name("mcu-srv".into())
        .spawn(move || {
            let svc = waved_mcu::McuService::new(
                mcu_rx, mcu_uart, mcu_battery_tx, mcu_state_tx, mcu_events_tx,
            );
            svc.run(mcu_cmd_rx);
        })
        .expect("failed to spawn mcu thread");

    // Spawn BT service on dedicated thread
    let bt_rx = uart.bt_receiver().clone();
    let bt_uart = Arc::clone(&uart);
    let bt_state_tx = bus.bt.clone();
    let bt_events_tx = bus.events.clone();
    std::thread::Builder::new()
        .name("bt-srv".into())
        .spawn(move || {
            let svc = waved_bt::BtService::new(bt_rx, bt_uart, bt_state_tx, bt_events_tx);
            svc.run();
        })
        .expect("failed to spawn bt thread");

    // Run async services on tokio
    run_async_services(config, bus);
}

fn run_async_only(config: waved_config::WavedConfig, bus: Arc<StateBus>) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async { run_async(config, bus).await });
}

fn run_async_services(config: waved_config::WavedConfig, bus: Arc<StateBus>) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async { run_async(config, bus).await });
}

async fn run_async(config: waved_config::WavedConfig, bus: Arc<StateBus>) {
    // Audio service
    let (frame_tx, _frame_rx) = waved_audio::callbacks::create_audio_channel();
    let audio_svc = waved_audio::AudioService::new(bus.audio.clone(), frame_tx);
    let audio_cmd_rx = bus.commands.subscribe();
    tokio::spawn(async move { audio_svc.run(audio_cmd_rx).await });

    // HID service
    let hid_events_rx = bus.events.subscribe();
    tokio::spawn(async move { waved_hid::HidService::run(hid_events_rx).await });

    // WiFi service
    let wifi_svc = waved_wifi::WifiService::new(bus.wifi.clone(), bus.events.clone());
    tokio::spawn(async move { wifi_svc.run().await });

    // Meeting service
    match waved_meeting::MeetingService::new(
        config.clone(),
        DB_PATH,
        PathBuf::from(RECORDINGS_DIR),
        bus.meeting.clone(),
        bus.events.clone(),
    ).await {
        Ok(meeting_svc) => {
            let meeting_cmd_rx = bus.commands.subscribe();
            let wifi_rx = bus.wifi.subscribe();
            tokio::spawn(async move { meeting_svc.run(meeting_cmd_rx, wifi_rx).await });
        }
        Err(e) => error!("meeting service init failed: {e}"),
    }

    info!("all services started, waiting for shutdown signal");

    // Wait for SIGTERM or SIGINT
    tokio::signal::ctrl_c().await.ok();
    info!("shutdown signal received");

    // Graceful shutdown: send shutdown command
    let _ = bus.commands.send(waved_state::Command::Shutdown);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    info!("waved shutdown complete");
}
```

- [ ] **Step 3: Write deployment scripts**

Create `waved/deploy/S60waved`:
```bash
#!/bin/sh

case "$1" in
    start)
        echo "Starting waved..."

        # Apply firewall rules
        if [ -f /data/waved/iptables.rules ]; then
            iptables-restore < /data/waved/iptables.rules 2>/dev/null || true
        fi

        # Ensure directories exist
        mkdir -p /data/waved
        mkdir -p /mnt/udisk/recordings

        # Set permissions on config
        chmod 0600 /data/waved/config.toml 2>/dev/null || true

        # Start waved with watchdog respawn
        (
            while true; do
                /data/waved/waved 2>&1 | logger -t waved
                echo "waved exited ($?), restarting in 3s..." | logger -t waved
                sleep 3
            done
        ) &
        ;;
    stop)
        echo "Stopping waved..."
        killall waved 2>/dev/null
        ;;
    *)
        echo "Usage: $0 {start|stop}"
        exit 1
        ;;
esac
```

Create `waved/deploy/iptables.rules`:
```
*filter
:INPUT DROP [0:0]
:FORWARD DROP [0:0]
:OUTPUT ACCEPT [0:0]
-A INPUT -i lo -j ACCEPT
-A INPUT -m state --state ESTABLISHED,RELATED -j ACCEPT
-A INPUT -p udp --sport 67 --dport 68 -j ACCEPT
-A INPUT -p udp --dport 5353 -j ACCEPT
COMMIT
```

Create `waved/deploy/install.sh`:
```bash
#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY="${SCRIPT_DIR}/../target/aarch64-unknown-linux-gnu/release/waved"

if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY"
    echo "Run: cd waved && cross build --release --target aarch64-unknown-linux-gnu"
    exit 1
fi

echo "=== Stopping stock firmware ==="
adb shell "killall pspapp pspapp_daemon pspapp_watchdog 2>/dev/null || true"

echo "=== Pushing waved binary ==="
adb shell "mkdir -p /data/waved"
adb push "$BINARY" /data/waved/waved
adb shell "chmod +x /data/waved/waved"

echo "=== Pushing config ==="
if [ ! -f /data/waved/config.toml ]; then
    adb push "${SCRIPT_DIR}/../config/default.toml" /data/waved/config.toml
    adb shell "chmod 0600 /data/waved/config.toml"
fi

echo "=== Installing init script ==="
adb shell "mkdir -p /data/overlay/upper/etc/init.d"
adb push "${SCRIPT_DIR}/S60waved" /data/overlay/upper/etc/init.d/S60waved
adb shell "chmod +x /data/overlay/upper/etc/init.d/S60waved"

echo "=== Removing stock pspapp init ==="
adb shell "mkdir -p /data/overlay/upper/etc/init.d"
adb shell "echo '#!/bin/sh' > /data/overlay/upper/etc/init.d/S60pspapp_daemon"
adb shell "chmod +x /data/overlay/upper/etc/init.d/S60pspapp_daemon"

echo "=== Removing telemetry library ==="
adb shell "mkdir -p /data/overlay/upper/usr/lib"
adb shell "echo '' > /data/overlay/upper/usr/lib/libapplogrs.so"

echo "=== Installing firewall rules ==="
adb push "${SCRIPT_DIR}/iptables.rules" /data/waved/iptables.rules

echo "=== Creating recordings directory ==="
adb shell "mkdir -p /mnt/udisk/recordings"

echo "=== Done! Reboot to start waved ==="
echo "Run: adb reboot"
```

Create `waved/deploy/revert.sh`:
```bash
#!/bin/bash
set -e

echo "=== Reverting to stock firmware ==="
adb shell "rm -rf /data/overlay/upper/etc/init.d/S60waved"
adb shell "rm -rf /data/overlay/upper/etc/init.d/S60pspapp_daemon"
adb shell "rm -rf /data/overlay/upper/usr/lib/libapplogrs.so"
adb shell "rm -rf /data/waved"
echo "=== Done! Reboot to restore stock ==="
echo "Run: adb reboot"
```

- [ ] **Step 4: Make scripts executable**

```bash
chmod +x waved/deploy/install.sh waved/deploy/revert.sh waved/deploy/S60waved
```

- [ ] **Step 5: Verify full workspace compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check
```
Expected: all crates compile successfully.

- [ ] **Step 6: Run all tests**

```bash
cargo test --workspace
```
Expected: all tests pass (frame parser, config, battery parsing).

- [ ] **Step 7: Commit**

```bash
git add waved/crates/waved/src/main.rs waved/crates/waved/Cargo.toml waved/deploy/
git commit -m "feat: wire up all modules in main binary and add deployment scripts"
```

---

### Task 12: Verify Cross-Compilation

- [ ] **Step 1: Install cross**

```bash
cargo install cross
```

- [ ] **Step 2: Cross-compile for aarch64**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cross build --release --target aarch64-unknown-linux-gnu
```
Expected: binary at `target/aarch64-unknown-linux-gnu/release/waved`

- [ ] **Step 3: Check binary size and architecture**

```bash
file target/aarch64-unknown-linux-gnu/release/waved
ls -lh target/aarch64-unknown-linux-gnu/release/waved
```
Expected: `ELF 64-bit LSB ... ARM aarch64`, reasonable size (< 20MB).

- [ ] **Step 4: Commit any build fixes needed**

```bash
git add -A
git commit -m "fix: cross-compilation fixes for aarch64"
```

---

## Next Plans (not in scope)

1. **LVGL UI Plan** — 17 screens, touchscreen input, status bar, navigation
2. **PUC Camera Plan** — MAVLink protocol on `/dev/ttyS3`, DOA-driven auto-framing
3. **OTA System Plan** — RSA-2048 signed SWU packages from custom server
4. **On-Device Testing Plan** — Validate TEA SDK FFI, UART protocol byte offsets, HID reports
5. **Phase 2: Open Audio Plan** — WebRTC AEC3 + RNNoise + ODAS replacement
