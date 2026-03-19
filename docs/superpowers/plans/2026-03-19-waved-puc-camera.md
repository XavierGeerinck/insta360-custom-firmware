# `waved-puc` — PTZ Camera Control Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement MAVLink-based PTZ camera control for the Insta360 Wave's PUC camera unit, supporting auto-framing from DOA angles, manual PTZ commands from UI, whiteboard detection toggle, and home position reset.

**Architecture:** The `waved-puc` crate owns exclusive access to `/dev/ttyS3` (separate UART from the BT/MCU mux on `/dev/ttyS1`). It runs a MAVLink v2 serial link using `mavlink` crate, sending gimbal control commands and receiving heartbeats/status. An auto-framing task subscribes to `AudioState` on the state bus for DOA angles and applies a smoothing filter (exponential moving average with configurable gain/damping) to steer the camera without jitter. Manual PTZ commands arrive via the `commands` broadcast channel. PUC connection state and mode are published to the state bus.

**Tech Stack:** Rust, tokio, mavlink (MAVLink v2 protocol), serialport, tokio::sync (watch/broadcast channels), tracing

**Depends on:** waved Phase 1 MVP plan (Task 1 workspace scaffolding, Task 2 state bus)

---

## Errata (from plan review -- apply these fixes during implementation)

1. **`watch::Sender` is `Arc`-wrapped in `StateBus`.** Per the Phase 1 MVP errata, each `watch::Sender` in `StateBus` is wrapped in `Arc`. Use `state_bus.puc.send_modify(|s| ...)` through the `Arc`.

2. **Handle `broadcast::Lagged` errors.** When receiving from the `commands` broadcast channel, log a warning on `Err(RecvError::Lagged(n))` instead of panicking or silently dropping.

3. **MAVLink version selection.** The stock firmware's `puc_server.c` uses MAVLink for gimbal control. Start with MAVLink v2 (`mavlink` crate with `common` dialect). If the PUC camera only speaks v1, fall back -- the `mavlink` crate supports both. Test on-device to confirm.

4. **Serial port baud rate.** The stock firmware uses 115200 baud for `/dev/ttyS3` based on decompilation. Confirm on-device.

5. **Use Rust edition `"2021"`** unless confirmed that edition 2024 cross-compiles for aarch64.

---

## File Structure

```
waved/crates/waved-puc/
├── Cargo.toml
└── src/
    ├── lib.rs          # PucService: public API, run() entry point
    ├── mavlink_link.rs # MAVLink serial connection, heartbeat, send/recv
    ├── auto_frame.rs   # DOA-to-gimbal smoothing algorithm
    └── commands.rs     # PTZ command types and MAVLink message builders
```

---

### Task 1: Crate Scaffolding + MAVLink Serial Link

**Files:**
- Create: `waved/crates/waved-puc/Cargo.toml`
- Create: `waved/crates/waved-puc/src/lib.rs`
- Create: `waved/crates/waved-puc/src/mavlink_link.rs`
- Modify: `waved/Cargo.toml` (add workspace dependency for `mavlink`)

- [ ] **Step 1: Create crate directory**

```bash
mkdir -p waved/crates/waved-puc/src
```

- [ ] **Step 2: Add `mavlink` to workspace dependencies**

Add to `waved/Cargo.toml` under `[workspace.dependencies]`:
```toml
mavlink = { version = "0.14", features = ["common", "tokio-1"] }
serialport = "4"
```

- [ ] **Step 3: Write Cargo.toml**

Create `waved/crates/waved-puc/Cargo.toml`:
```toml
[package]
name = "waved-puc"
version.workspace = true
edition.workspace = true

[dependencies]
waved-state = { path = "../waved-state" }
tokio = { workspace = true, features = ["sync", "time", "macros"] }
mavlink.workspace = true
serialport.workspace = true
tracing.workspace = true
thiserror.workspace = true
```

- [ ] **Step 4: Write MAVLink serial link**

Create `waved/crates/waved-puc/src/mavlink_link.rs`:
```rust
use mavlink::common::MavMessage;
use mavlink::{MavConnection, MavHeader};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time;
use tracing::{debug, error, info, warn};

/// System/component IDs for our side of the MAVLink link.
const SYSTEM_ID: u8 = 1;
const COMPONENT_ID: u8 = 1;

/// Expected heartbeat interval from the PUC camera.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);

/// How often we send our heartbeat to the PUC camera.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Error)]
pub enum LinkError {
    #[error("failed to open serial port: {0}")]
    SerialOpen(String),
    #[error("MAVLink connection error: {0}")]
    MavlinkConnect(String),
    #[error("send error: {0}")]
    Send(String),
    #[error("receive error: {0}")]
    Recv(String),
    #[error("heartbeat timeout — PUC camera not responding")]
    HeartbeatTimeout,
}

/// Incoming messages from the PUC camera.
#[derive(Debug)]
pub struct IncomingMessage {
    pub header: MavHeader,
    pub message: MavMessage,
}

/// A MAVLink serial link to the PUC camera on `/dev/ttyS3`.
pub struct MavlinkLink {
    conn: Arc<Box<dyn MavConnection<MavMessage> + Send + Sync>>,
}

impl MavlinkLink {
    /// Open the MAVLink serial connection.
    ///
    /// `port` is typically `/dev/ttyS3`, `baud` is typically `115200`.
    pub fn open(port: &str, baud: u32) -> Result<Self, LinkError> {
        let connection_string = format!("serial:{}:{}", port, baud);
        let conn = mavlink::connect::<MavMessage>(&connection_string)
            .map_err(|e| LinkError::MavlinkConnect(e.to_string()))?;
        info!(port, baud, "MAVLink link opened to PUC camera");
        Ok(Self {
            conn: Arc::new(conn),
        })
    }

    /// Send a MAVLink message to the PUC camera.
    pub fn send(&self, msg: &MavMessage) -> Result<(), LinkError> {
        let header = MavHeader {
            system_id: SYSTEM_ID,
            component_id: COMPONENT_ID,
            sequence: 0, // mavlink crate auto-increments
        };
        self.conn
            .send(&header, msg)
            .map_err(|e| LinkError::Send(e.to_string()))?;
        debug!(?msg, "sent MAVLink message");
        Ok(())
    }

    /// Spawn a reader task that receives MAVLink messages and forwards them
    /// to an mpsc channel. Also monitors heartbeats.
    ///
    /// Returns the receiver end of the channel and a `JoinHandle`.
    pub fn spawn_reader(
        &self,
    ) -> (
        mpsc::Receiver<IncomingMessage>,
        std::thread::JoinHandle<()>,
    ) {
        let (tx, rx) = mpsc::channel::<IncomingMessage>(64);
        let conn = Arc::clone(&self.conn);

        // MAVLink recv is blocking, so run on a dedicated OS thread.
        let handle = std::thread::Builder::new()
            .name("puc-mavlink-reader".into())
            .spawn(move || {
                loop {
                    match conn.recv() {
                        Ok((header, message)) => {
                            let msg = IncomingMessage { header, message };
                            if tx.blocking_send(msg).is_err() {
                                info!("MAVLink reader shutting down — channel closed");
                                return;
                            }
                        }
                        Err(e) => {
                            warn!("MAVLink recv error: {e}");
                            // Brief sleep to avoid busy-loop on persistent errors
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
            })
            .expect("failed to spawn MAVLink reader thread");

        (rx, handle)
    }

    /// Send a heartbeat message identifying us as an onboard controller.
    pub fn send_heartbeat(&self) -> Result<(), LinkError> {
        use mavlink::common::{MavAutopilot, MavModeFlag, MavState, MavType};

        let msg = MavMessage::HEARTBEAT(mavlink::common::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: MavType::MAV_TYPE_ONBOARD_CONTROLLER,
            autopilot: MavAutopilot::MAV_AUTOPILOT_INVALID,
            base_mode: MavModeFlag::empty(),
            system_status: MavState::MAV_STATE_ACTIVE,
            mavlink_version: 0x03,
        });
        self.send(&msg)
    }

    /// Spawn an async task that sends periodic heartbeats.
    pub fn spawn_heartbeat_sender(&self) -> tokio::task::JoinHandle<()> {
        let conn = Arc::clone(&self.conn);
        tokio::spawn(async move {
            let mut interval = time::interval(HEARTBEAT_INTERVAL);
            loop {
                interval.tick().await;
                let header = MavHeader {
                    system_id: SYSTEM_ID,
                    component_id: COMPONENT_ID,
                    sequence: 0,
                };
                let msg = MavMessage::HEARTBEAT(mavlink::common::HEARTBEAT_DATA {
                    custom_mode: 0,
                    mavtype: mavlink::common::MavType::MAV_TYPE_ONBOARD_CONTROLLER,
                    autopilot: mavlink::common::MavAutopilot::MAV_AUTOPILOT_INVALID,
                    base_mode: mavlink::common::MavModeFlag::empty(),
                    system_status: mavlink::common::MavState::MAV_STATE_ACTIVE,
                    mavlink_version: 0x03,
                });
                if let Err(e) = conn.send(&header, &msg) {
                    warn!("failed to send heartbeat to PUC: {e}");
                }
            }
        })
    }
}
```

- [ ] **Step 5: Write lib.rs stub**

Create `waved/crates/waved-puc/src/lib.rs`:
```rust
pub mod auto_frame;
pub mod commands;
pub mod mavlink_link;

// Re-export for main binary
pub use crate::mavlink_link::MavlinkLink;
```

- [ ] **Step 6: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-puc
```

- [ ] **Step 7: Commit**

```bash
git add waved/crates/waved-puc/ waved/Cargo.toml
git commit -m "feat(waved-puc): scaffold crate with MAVLink serial link to PUC camera"
```

---

### Task 2: PTZ Command Builders

**Files:**
- Create: `waved/crates/waved-puc/src/commands.rs`
- Modify: `waved/crates/waved-state/src/lib.rs` (add PUC-specific commands to `Command` enum)

- [ ] **Step 1: Add PUC commands to the state bus `Command` enum**

Add these variants to the `Command` enum in `waved/crates/waved-state/src/lib.rs`:
```rust
// PUC camera commands
PucPanLeft,
PucPanRight,
PucTiltUp,
PucTiltDown,
PucZoomIn,
PucZoomOut,
PucStop,               // Stop any ongoing PTZ motion
PucHome,               // Reset to home/center position
PucAutoFrameOn,
PucAutoFrameOff,
PucWhiteboardOn,
PucWhiteboardOff,
```

Also add `whiteboard_active: bool` to `PucState`:
```rust
#[derive(Clone, Debug, Default)]
pub struct PucState {
    pub connected: bool,
    pub auto_framing: bool,
    pub whiteboard_active: bool,
    pub current_yaw: f32,    // degrees, for UI display
    pub current_pitch: f32,  // degrees, for UI display
    pub current_zoom: f32,   // 1.0 = no zoom
}
```

- [ ] **Step 2: Write PTZ command builders**

Create `waved/crates/waved-puc/src/commands.rs`:
```rust
//! MAVLink message builders for PTZ gimbal control.
//!
//! The PUC camera accepts standard MAVLink gimbal commands:
//! - `COMMAND_LONG` with `MAV_CMD_DO_MOUNT_CONTROL` for absolute positioning
//! - `COMMAND_LONG` with `MAV_CMD_DO_GIMBAL_MANAGER_PITCHYAW` for rate control
//! - Manual rate control via `MANUAL_CONTROL` or gimbal manager messages
//!
//! Whiteboard mode and home position use vendor-specific or standard commands.

use mavlink::common::{MavMessage, MavCmd, MavFrame, MavResult};

/// Target system/component for the PUC gimbal.
const PUC_TARGET_SYSTEM: u8 = 1;
const PUC_TARGET_COMPONENT: u8 = 154; // MAV_COMP_ID_GIMBAL (typical)

/// PTZ rate in degrees/second for manual pan/tilt.
const PTZ_RATE_DEG_PER_SEC: f32 = 30.0;

/// Zoom step for in/out commands (relative).
const ZOOM_STEP: f32 = 0.1;

/// Build a COMMAND_LONG message for gimbal pitch/yaw rate control.
///
/// Uses `MAV_CMD_DO_GIMBAL_MANAGER_PITCHYAW` (1000):
///   param1 = pitch angle or rate (deg or deg/s)
///   param2 = yaw angle or rate (deg or deg/s)
///   param3 = pitch rate (deg/s) — used in rate mode
///   param4 = yaw rate (deg/s) — used in rate mode
///   param5 = gimbal manager flags
///   param7 = gimbal device ID
pub fn gimbal_rate_command(pitch_rate: f32, yaw_rate: f32) -> MavMessage {
    MavMessage::COMMAND_LONG(mavlink::common::COMMAND_LONG_DATA {
        target_system: PUC_TARGET_SYSTEM,
        target_component: PUC_TARGET_COMPONENT,
        command: MavCmd::MAV_CMD_DO_GIMBAL_MANAGER_PITCHYAW,
        confirmation: 0,
        param1: f32::NAN,       // pitch angle = NaN means "use rate instead"
        param2: f32::NAN,       // yaw angle = NaN means "use rate instead"
        param3: pitch_rate,     // pitch rate deg/s
        param4: yaw_rate,       // yaw rate deg/s
        param5: 0.0,            // flags
        param6: 0.0,            // reserved
        param7: 0.0,            // gimbal device ID (0 = primary)
    })
}

/// Build a COMMAND_LONG for absolute gimbal positioning (pitch, yaw in degrees).
///
/// Uses `MAV_CMD_DO_MOUNT_CONTROL`:
///   param1 = pitch (deg)
///   param2 = roll (deg) — 0 for our 2-axis gimbal
///   param3 = yaw (deg)
///   param7 = mount mode (2 = MAV_MOUNT_MODE_MAVLINK_TARGETING)
pub fn gimbal_absolute_command(pitch_deg: f32, yaw_deg: f32) -> MavMessage {
    MavMessage::COMMAND_LONG(mavlink::common::COMMAND_LONG_DATA {
        target_system: PUC_TARGET_SYSTEM,
        target_component: PUC_TARGET_COMPONENT,
        command: MavCmd::MAV_CMD_DO_MOUNT_CONTROL,
        confirmation: 0,
        param1: pitch_deg,
        param2: 0.0,            // roll
        param3: yaw_deg,
        param4: 0.0,
        param5: 0.0,
        param6: 0.0,
        param7: 2.0,            // MAV_MOUNT_MODE_MAVLINK_TARGETING
    })
}

/// Build a stop command (zero rates).
pub fn gimbal_stop() -> MavMessage {
    gimbal_rate_command(0.0, 0.0)
}

/// Build a home position command (pitch=0, yaw=0 absolute).
pub fn gimbal_home() -> MavMessage {
    gimbal_absolute_command(0.0, 0.0)
}

/// Pan left at default rate.
pub fn pan_left() -> MavMessage {
    gimbal_rate_command(0.0, -PTZ_RATE_DEG_PER_SEC)
}

/// Pan right at default rate.
pub fn pan_right() -> MavMessage {
    gimbal_rate_command(0.0, PTZ_RATE_DEG_PER_SEC)
}

/// Tilt up at default rate.
pub fn tilt_up() -> MavMessage {
    gimbal_rate_command(PTZ_RATE_DEG_PER_SEC, 0.0)
}

/// Tilt down at default rate.
pub fn tilt_down() -> MavMessage {
    gimbal_rate_command(-PTZ_RATE_DEG_PER_SEC, 0.0)
}

/// Zoom in command via `MAV_CMD_SET_CAMERA_ZOOM`.
///
/// param1 = ZOOM_TYPE_STEP (2)
/// param2 = zoom step value (positive = zoom in)
pub fn zoom_in() -> MavMessage {
    MavMessage::COMMAND_LONG(mavlink::common::COMMAND_LONG_DATA {
        target_system: PUC_TARGET_SYSTEM,
        target_component: PUC_TARGET_COMPONENT,
        command: MavCmd::MAV_CMD_SET_CAMERA_ZOOM,
        confirmation: 0,
        param1: 2.0,            // ZOOM_TYPE_STEP
        param2: ZOOM_STEP,      // positive = zoom in
        param3: 0.0,
        param4: 0.0,
        param5: 0.0,
        param6: 0.0,
        param7: 0.0,
    })
}

/// Zoom out command via `MAV_CMD_SET_CAMERA_ZOOM`.
pub fn zoom_out() -> MavMessage {
    MavMessage::COMMAND_LONG(mavlink::common::COMMAND_LONG_DATA {
        target_system: PUC_TARGET_SYSTEM,
        target_component: PUC_TARGET_COMPONENT,
        command: MavCmd::MAV_CMD_SET_CAMERA_ZOOM,
        confirmation: 0,
        param1: 2.0,            // ZOOM_TYPE_STEP
        param2: -ZOOM_STEP,     // negative = zoom out
        param3: 0.0,
        param4: 0.0,
        param5: 0.0,
        param6: 0.0,
        param7: 0.0,
    })
}

/// Translate a high-level PUC command into a MAVLink message.
pub fn command_to_mavlink(cmd: &waved_state::Command) -> Option<MavMessage> {
    use waved_state::Command;
    match cmd {
        Command::PucPanLeft => Some(pan_left()),
        Command::PucPanRight => Some(pan_right()),
        Command::PucTiltUp => Some(tilt_up()),
        Command::PucTiltDown => Some(tilt_down()),
        Command::PucZoomIn => Some(zoom_in()),
        Command::PucZoomOut => Some(zoom_out()),
        Command::PucStop => Some(gimbal_stop()),
        Command::PucHome => Some(gimbal_home()),
        // Mode toggles are handled by PucService directly, not as MAVLink messages
        Command::PucAutoFrameOn
        | Command::PucAutoFrameOff
        | Command::PucWhiteboardOn
        | Command::PucWhiteboardOff => None,
        _ => None,
    }
}
```

- [ ] **Step 3: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-puc
```

- [ ] **Step 4: Commit**

```bash
git add waved/crates/waved-puc/src/commands.rs waved/crates/waved-state/src/lib.rs
git commit -m "feat(waved-puc): PTZ command builders and PUC state types"
```

---

### Task 3: Auto-Framing Algorithm

**Files:**
- Create: `waved/crates/waved-puc/src/auto_frame.rs`

The auto-framing algorithm subscribes to DOA (Direction of Arrival) angles from `AudioState` and smoothly steers the gimbal to track the active speaker. It uses exponential moving average (EMA) with configurable gain to avoid jitter from momentary noise.

- [ ] **Step 1: Write auto-framing module**

Create `waved/crates/waved-puc/src/auto_frame.rs`:
```rust
//! DOA-to-gimbal auto-framing with smooth tracking.
//!
//! The audio pipeline provides DOA angles (horizontal 0..360, pitch -90..90)
//! via `AudioState` on the state bus. This module:
//!
//! 1. Reads DOA from the watch channel on each tick
//! 2. Applies an EMA (exponential moving average) filter to smooth jitter
//! 3. Applies a deadband so small DOA changes don't cause constant micro-adjustments
//! 4. Emits absolute gimbal position commands at a controlled rate
//!
//! Tuning parameters:
//! - `gain`: EMA alpha (0.0 = ignore new samples, 1.0 = no smoothing). Default 0.15.
//! - `deadband_deg`: Ignore DOA changes smaller than this. Default 5.0 degrees.
//! - `tick_interval_ms`: How often to update. Default 100ms (10 Hz).

use std::time::Duration;

/// Auto-framing configuration.
#[derive(Debug, Clone)]
pub struct AutoFrameConfig {
    /// EMA gain (alpha). Range 0.0..1.0.
    /// Lower = smoother but slower response. Higher = more responsive but jittery.
    pub gain: f32,

    /// Ignore DOA changes smaller than this many degrees.
    pub deadband_deg: f32,

    /// Update interval.
    pub tick_interval: Duration,

    /// Maximum slew rate in degrees per second.
    /// Prevents jarring fast movements when DOA jumps suddenly (e.g., new speaker).
    pub max_slew_rate: f32,
}

impl Default for AutoFrameConfig {
    fn default() -> Self {
        Self {
            gain: 0.15,
            deadband_deg: 5.0,
            tick_interval: Duration::from_millis(100),
            max_slew_rate: 45.0, // degrees per second
        }
    }
}

/// Smoothed gimbal target state.
#[derive(Debug, Clone)]
pub struct AutoFrameState {
    /// Current smoothed yaw target (degrees, -180..180).
    pub target_yaw: f32,
    /// Current smoothed pitch target (degrees, -90..90).
    pub target_pitch: f32,
    /// Last commanded yaw (for deadband comparison).
    pub last_commanded_yaw: f32,
    /// Last commanded pitch (for deadband comparison).
    pub last_commanded_pitch: f32,
    /// Whether we have received at least one valid DOA sample.
    pub initialized: bool,
}

impl Default for AutoFrameState {
    fn default() -> Self {
        Self {
            target_yaw: 0.0,
            target_pitch: 0.0,
            last_commanded_yaw: 0.0,
            last_commanded_pitch: 0.0,
            initialized: false,
        }
    }
}

/// Result of an auto-frame tick.
#[derive(Debug)]
pub enum AutoFrameAction {
    /// Send an absolute gimbal command to these angles.
    MoveTo { pitch_deg: f32, yaw_deg: f32 },
    /// No movement needed (within deadband).
    Hold,
}

impl AutoFrameState {
    /// Process a new DOA sample and compute the gimbal action.
    ///
    /// `doa_horizontal`: 0..360 degrees from the microphone array.
    ///   We remap to -180..180 for gimbal yaw.
    /// `doa_pitch`: -90..90 degrees from the microphone array.
    pub fn update(
        &mut self,
        doa_horizontal: f32,
        doa_pitch: f32,
        config: &AutoFrameConfig,
    ) -> AutoFrameAction {
        // Remap DOA horizontal (0..360) to gimbal yaw (-180..180).
        // DOA 0 = front of device = gimbal 0.
        // DOA 90 = right = gimbal 90, DOA 270 = left = gimbal -90.
        let raw_yaw = if doa_horizontal > 180.0 {
            doa_horizontal - 360.0
        } else {
            doa_horizontal
        };
        let raw_pitch = doa_pitch;

        if !self.initialized {
            // First sample: jump directly to the DOA position.
            self.target_yaw = raw_yaw;
            self.target_pitch = raw_pitch;
            self.last_commanded_yaw = raw_yaw;
            self.last_commanded_pitch = raw_pitch;
            self.initialized = true;
            return AutoFrameAction::MoveTo {
                pitch_deg: raw_pitch,
                yaw_deg: raw_yaw,
            };
        }

        // Apply EMA smoothing.
        let alpha = config.gain;
        self.target_yaw = self.target_yaw * (1.0 - alpha) + raw_yaw * alpha;
        self.target_pitch = self.target_pitch * (1.0 - alpha) + raw_pitch * alpha;

        // Apply max slew rate limiting.
        let max_delta = config.max_slew_rate * config.tick_interval.as_secs_f32();
        let yaw_delta = (self.target_yaw - self.last_commanded_yaw).clamp(-max_delta, max_delta);
        let pitch_delta =
            (self.target_pitch - self.last_commanded_pitch).clamp(-max_delta, max_delta);

        let new_yaw = self.last_commanded_yaw + yaw_delta;
        let new_pitch = self.last_commanded_pitch + pitch_delta;

        // Check deadband: only command if the delta from last commanded position
        // exceeds the threshold.
        let yaw_change = (new_yaw - self.last_commanded_yaw).abs();
        let pitch_change = (new_pitch - self.last_commanded_pitch).abs();

        if yaw_change > config.deadband_deg || pitch_change > config.deadband_deg {
            self.last_commanded_yaw = new_yaw;
            self.last_commanded_pitch = new_pitch;
            AutoFrameAction::MoveTo {
                pitch_deg: new_pitch,
                yaw_deg: new_yaw,
            }
        } else {
            AutoFrameAction::Hold
        }
    }

    /// Reset state (e.g., when auto-framing is toggled off and back on).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sample_jumps_to_position() {
        let mut state = AutoFrameState::default();
        let config = AutoFrameConfig::default();

        match state.update(90.0, 10.0, &config) {
            AutoFrameAction::MoveTo { yaw_deg, pitch_deg } => {
                assert!((yaw_deg - 90.0).abs() < 0.01);
                assert!((pitch_deg - 10.0).abs() < 0.01);
            }
            _ => panic!("expected MoveTo on first sample"),
        }
    }

    #[test]
    fn small_changes_within_deadband_hold() {
        let mut state = AutoFrameState::default();
        let config = AutoFrameConfig {
            deadband_deg: 5.0,
            gain: 0.15,
            ..Default::default()
        };

        // Initialize at (0, 0).
        state.update(0.0, 0.0, &config);

        // Small change: 1 degree should stay within deadband after EMA.
        let action = state.update(1.0, 0.5, &config);
        assert!(matches!(action, AutoFrameAction::Hold));
    }

    #[test]
    fn large_change_triggers_move() {
        let mut state = AutoFrameState::default();
        let config = AutoFrameConfig {
            deadband_deg: 5.0,
            gain: 1.0, // No smoothing for this test
            max_slew_rate: 360.0, // No slew limit for this test
            ..Default::default()
        };

        // Initialize at (0, 0).
        state.update(0.0, 0.0, &config);

        // Large change: 45 degrees should trigger a move.
        let action = state.update(45.0, 0.0, &config);
        assert!(matches!(action, AutoFrameAction::MoveTo { .. }));
    }

    #[test]
    fn doa_wraps_around_correctly() {
        let mut state = AutoFrameState::default();
        let config = AutoFrameConfig {
            gain: 1.0,
            max_slew_rate: 360.0,
            ..Default::default()
        };

        // DOA 270 should map to yaw -90.
        match state.update(270.0, 0.0, &config) {
            AutoFrameAction::MoveTo { yaw_deg, .. } => {
                assert!((yaw_deg - (-90.0)).abs() < 0.01);
            }
            _ => panic!("expected MoveTo"),
        }
    }

    #[test]
    fn slew_rate_limits_large_jumps() {
        let mut state = AutoFrameState::default();
        let config = AutoFrameConfig {
            gain: 1.0,              // No EMA smoothing
            deadband_deg: 0.0,      // No deadband
            max_slew_rate: 10.0,    // 10 deg/s
            tick_interval: Duration::from_millis(100), // 0.1s per tick
        };

        // Initialize at (0, 0).
        state.update(0.0, 0.0, &config);

        // Jump to 90 degrees — should be limited to 1 deg per tick (10 deg/s * 0.1s).
        match state.update(90.0, 0.0, &config) {
            AutoFrameAction::MoveTo { yaw_deg, .. } => {
                assert!(
                    (yaw_deg - 1.0).abs() < 0.01,
                    "slew rate should limit to 1 deg/tick, got {yaw_deg}"
                );
            }
            _ => panic!("expected MoveTo"),
        }
    }
}
```

- [ ] **Step 2: Verify tests pass**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo test -p waved-puc
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-puc/src/auto_frame.rs
git commit -m "feat(waved-puc): auto-framing algorithm with EMA smoothing, deadband, slew rate"
```

---

### Task 4: PucService — Main Service Loop

**Files:**
- Modify: `waved/crates/waved-puc/src/lib.rs`

This is the main service that ties everything together: opens the MAVLink link, spawns the reader and heartbeat tasks, listens for commands on the broadcast channel, subscribes to `AudioState` for DOA angles, and runs the auto-framing loop.

- [ ] **Step 1: Write the full PucService**

Replace `waved/crates/waved-puc/src/lib.rs`:
```rust
//! `waved-puc` — PTZ camera control for the Insta360 Wave PUC camera.
//!
//! The PUC (Pan-tilt Unit Camera) is connected via MAVLink over `/dev/ttyS3`.
//! This module provides:
//! - Auto-framing: track active speaker using DOA angles from audio pipeline
//! - Manual PTZ: pan/tilt/zoom commands from UI
//! - Whiteboard detection toggle
//! - Home position reset

pub mod auto_frame;
pub mod commands;
pub mod mavlink_link;

use auto_frame::{AutoFrameAction, AutoFrameConfig, AutoFrameState};
use commands::command_to_mavlink;
use mavlink::common::MavMessage;
use mavlink_link::{LinkError, MavlinkLink};
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tokio::time;
use tracing::{debug, error, info, warn};
use waved_state::{AudioState, Command, PucState, StateBus};

/// Default serial port for the PUC camera.
const PUC_SERIAL_PORT: &str = "/dev/ttyS3";

/// Default baud rate for the PUC serial link.
const PUC_BAUD_RATE: u32 = 115200;

/// How long to wait before retrying a failed connection.
const RECONNECT_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

pub struct PucService {
    state_bus: Arc<StateBus>,
    auto_frame_config: AutoFrameConfig,
    serial_port: String,
    baud_rate: u32,
}

impl PucService {
    pub fn new(state_bus: Arc<StateBus>) -> Self {
        Self {
            state_bus,
            auto_frame_config: AutoFrameConfig::default(),
            serial_port: PUC_SERIAL_PORT.to_string(),
            baud_rate: PUC_BAUD_RATE,
        }
    }

    /// Override serial port (useful for testing).
    pub fn with_serial_port(mut self, port: impl Into<String>) -> Self {
        self.serial_port = port.into();
        self
    }

    /// Override baud rate.
    pub fn with_baud_rate(mut self, baud: u32) -> Self {
        self.baud_rate = baud;
        self
    }

    /// Override auto-frame config.
    pub fn with_auto_frame_config(mut self, config: AutoFrameConfig) -> Self {
        self.auto_frame_config = config;
        self
    }

    /// Run the PUC service. This will retry the connection on failure.
    ///
    /// This function runs forever until the task is cancelled.
    pub async fn run(self) {
        loop {
            info!(
                port = %self.serial_port,
                baud = self.baud_rate,
                "connecting to PUC camera"
            );

            match self.run_connected().await {
                Ok(()) => {
                    info!("PUC service stopped normally");
                    return;
                }
                Err(e) => {
                    error!("PUC connection lost: {e}");
                    self.state_bus.puc.send_modify(|s| {
                        s.connected = false;
                    });
                    warn!(
                        delay_secs = RECONNECT_DELAY.as_secs(),
                        "reconnecting to PUC camera"
                    );
                    time::sleep(RECONNECT_DELAY).await;
                }
            }
        }
    }

    /// Inner loop: connect, run, return error on disconnect.
    async fn run_connected(&self) -> Result<(), LinkError> {
        let link = MavlinkLink::open(&self.serial_port, self.baud_rate)?;
        let (mut mavlink_rx, _reader_handle) = link.spawn_reader();
        let _heartbeat_handle = link.spawn_heartbeat_sender();

        // Mark as connected.
        self.state_bus.puc.send_modify(|s| {
            s.connected = true;
        });
        info!("PUC camera connected");

        // Send initial heartbeat and request home position.
        link.send_heartbeat()?;

        // Subscribe to commands.
        let mut cmd_rx = self.state_bus.commands.subscribe();

        // Subscribe to audio state for DOA.
        let audio_rx = self.state_bus.audio.subscribe();

        // Auto-framing state.
        let mut af_state = AutoFrameState::default();
        let mut auto_framing_enabled = false;
        let mut whiteboard_active = false;

        // Auto-frame tick interval.
        let mut af_ticker = time::interval(self.auto_frame_config.tick_interval);

        loop {
            tokio::select! {
                // Process incoming MAVLink messages from the PUC camera.
                msg = mavlink_rx.recv() => {
                    match msg {
                        Some(incoming) => {
                            self.handle_incoming(&incoming.message);
                        }
                        None => {
                            // Reader channel closed — connection lost.
                            return Err(LinkError::HeartbeatTimeout);
                        }
                    }
                }

                // Process commands from UI / other modules.
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Ok(command) => {
                            self.handle_command(
                                &command,
                                &link,
                                &mut auto_framing_enabled,
                                &mut whiteboard_active,
                                &mut af_state,
                            )?;
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(missed = n, "PUC command receiver lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("command channel closed, shutting down PUC service");
                            return Ok(());
                        }
                    }
                }

                // Auto-framing tick: read latest DOA and steer gimbal.
                _ = af_ticker.tick(), if auto_framing_enabled => {
                    let audio = audio_rx.borrow().clone();
                    let action = af_state.update(
                        audio.doa_horizontal,
                        audio.doa_pitch,
                        &self.auto_frame_config,
                    );
                    match action {
                        AutoFrameAction::MoveTo { pitch_deg, yaw_deg } => {
                            let msg = commands::gimbal_absolute_command(pitch_deg, yaw_deg);
                            if let Err(e) = link.send(&msg) {
                                warn!("failed to send auto-frame command: {e}");
                            }
                            // Update state bus with current position.
                            self.state_bus.puc.send_modify(|s| {
                                s.current_yaw = yaw_deg;
                                s.current_pitch = pitch_deg;
                            });
                            debug!(yaw = yaw_deg, pitch = pitch_deg, "auto-frame move");
                        }
                        AutoFrameAction::Hold => {
                            // Within deadband, no command needed.
                        }
                    }
                }
            }
        }
    }

    /// Handle an incoming MAVLink message from the PUC camera.
    fn handle_incoming(&self, msg: &MavMessage) {
        match msg {
            MavMessage::HEARTBEAT(hb) => {
                debug!(
                    mavtype = ?hb.mavtype,
                    status = ?hb.system_status,
                    "PUC heartbeat received"
                );
                self.state_bus.puc.send_modify(|s| {
                    s.connected = true;
                });
            }
            MavMessage::COMMAND_ACK(ack) => {
                debug!(
                    command = ?ack.command,
                    result = ?ack.result,
                    "PUC command ACK"
                );
                if ack.result != mavlink::common::MavResult::MAV_RESULT_ACCEPTED {
                    warn!(
                        command = ?ack.command,
                        result = ?ack.result,
                        "PUC rejected command"
                    );
                }
            }
            MavMessage::GIMBAL_DEVICE_ATTITUDE_STATUS(status) => {
                // Parse quaternion to euler angles for state bus.
                // The quaternion is [w, x, y, z] in the status message.
                let q = &status.q;
                let (yaw, pitch) = quaternion_to_yaw_pitch(q[0], q[1], q[2], q[3]);
                self.state_bus.puc.send_modify(|s| {
                    s.current_yaw = yaw.to_degrees();
                    s.current_pitch = pitch.to_degrees();
                });
                debug!(
                    yaw_deg = yaw.to_degrees(),
                    pitch_deg = pitch.to_degrees(),
                    "PUC attitude update"
                );
            }
            _ => {
                debug!(?msg, "unhandled PUC message");
            }
        }
    }

    /// Handle a command from the UI or other modules.
    fn handle_command(
        &self,
        cmd: &Command,
        link: &MavlinkLink,
        auto_framing: &mut bool,
        whiteboard: &mut bool,
        af_state: &mut AutoFrameState,
    ) -> Result<(), LinkError> {
        match cmd {
            Command::PucAutoFrameOn => {
                info!("auto-framing enabled");
                *auto_framing = true;
                af_state.reset();
                self.state_bus.puc.send_modify(|s| {
                    s.auto_framing = true;
                });
            }
            Command::PucAutoFrameOff => {
                info!("auto-framing disabled");
                *auto_framing = false;
                // Stop any ongoing motion.
                link.send(&commands::gimbal_stop())?;
                self.state_bus.puc.send_modify(|s| {
                    s.auto_framing = false;
                });
            }
            Command::PucWhiteboardOn => {
                info!("whiteboard mode enabled");
                *whiteboard = true;
                self.state_bus.puc.send_modify(|s| {
                    s.whiteboard_active = true;
                });
                // Whiteboard mode: disable auto-framing (camera stays fixed on board).
                if *auto_framing {
                    *auto_framing = false;
                    self.state_bus.puc.send_modify(|s| {
                        s.auto_framing = false;
                    });
                    info!("auto-framing disabled for whiteboard mode");
                }
            }
            Command::PucWhiteboardOff => {
                info!("whiteboard mode disabled");
                *whiteboard = false;
                self.state_bus.puc.send_modify(|s| {
                    s.whiteboard_active = false;
                });
            }
            Command::PucHome => {
                info!("resetting PUC to home position");
                link.send(&commands::gimbal_home())?;
                af_state.reset();
                self.state_bus.puc.send_modify(|s| {
                    s.current_yaw = 0.0;
                    s.current_pitch = 0.0;
                });
            }
            other => {
                // Check if it's a PTZ motion command.
                if let Some(mavlink_msg) = command_to_mavlink(other) {
                    // Manual PTZ overrides auto-framing temporarily.
                    if *auto_framing {
                        debug!("manual PTZ command while auto-framing — sending directly");
                    }
                    link.send(&mavlink_msg)?;
                }
                // Ignore non-PUC commands silently.
            }
        }
        Ok(())
    }
}

/// Convert a quaternion (w, x, y, z) to yaw and pitch angles in radians.
///
/// Uses the standard aerospace convention:
/// - yaw = atan2(2(wz + xy), 1 - 2(yy + zz))
/// - pitch = asin(2(wx - yz))
fn quaternion_to_yaw_pitch(w: f32, x: f32, y: f32, z: f32) -> (f32, f32) {
    let yaw = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z));
    let pitch_sin = (2.0 * (w * x - y * z)).clamp(-1.0, 1.0);
    let pitch = pitch_sin.asin();
    (yaw, pitch)
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-puc
```

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-puc/src/lib.rs
git commit -m "feat(waved-puc): PucService with auto-framing, manual PTZ, whiteboard, and home reset"
```

---

### Task 5: Integration into Main Binary

**Files:**
- Modify: `waved/crates/waved/Cargo.toml` (add `waved-puc` dependency)
- Modify: `waved/crates/waved/src/main.rs` (spawn `PucService`)

- [ ] **Step 1: Add dependency**

Add to `waved/crates/waved/Cargo.toml` under `[dependencies]`:
```toml
waved-puc = { path = "../waved-puc" }
```

- [ ] **Step 2: Spawn PucService in main.rs**

Add the following to the async block in `main.rs`, after `StateBus` creation and alongside other service spawns:

```rust
// PUC camera service
let puc_service = waved_puc::PucService::new(Arc::clone(&state_bus));
let puc_handle = tokio::spawn(async move {
    puc_service.run().await;
});
```

Add `puc_handle` to the shutdown/select logic so it is cancelled on SIGTERM:

```rust
tokio::select! {
    _ = sigterm.recv() => info!("SIGTERM received"),
    _ = sigint.recv() => info!("SIGINT received"),
}

// Cancel PUC service (and all other service handles).
puc_handle.abort();
```

- [ ] **Step 3: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved
```

- [ ] **Step 4: Run full test suite**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo test --workspace
```

All `auto_frame` tests from Task 3 should pass. No hardware-dependent tests run in CI (MAVLink serial requires `/dev/ttyS3`).

- [ ] **Step 5: Commit**

```bash
git add waved/crates/waved/Cargo.toml waved/crates/waved/src/main.rs
git commit -m "feat(waved): integrate PucService into main binary"
```

---

## On-Device Validation Checklist

After deploying to the Insta360 Wave, verify these manually:

1. **Serial link**: `waved` log shows `"MAVLink link opened to PUC camera"` and `"PUC heartbeat received"` within 5 seconds of boot.
2. **Manual PTZ**: Send commands via ADB shell or UI. Camera pans left/right, tilts up/down, zooms in/out. `PucStop` halts motion.
3. **Home reset**: `PucHome` returns camera to center position (pitch=0, yaw=0).
4. **Auto-framing**: Enable auto-framing, speak from different positions around the device. Camera should smoothly track to the active speaker within 1-2 seconds.
5. **Deadband**: Small ambient noise should not cause constant micro-adjustments.
6. **Whiteboard mode**: Enable whiteboard, verify auto-framing is disabled and camera holds position. Disable whiteboard, verify manual PTZ and auto-framing work again.
7. **Reconnection**: Physically disconnect/reconnect the PUC camera. Service should recover within `RECONNECT_DELAY` (3 seconds).
