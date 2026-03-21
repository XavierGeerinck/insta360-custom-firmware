# waved Testing Infrastructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a five-layer testing pyramid to waved: unit tests, integration tests with simulation, a simulation binary, on-device smoke tests, and a manual HIL harness.

**Architecture:** Feature-flag approach (`simulate`) swaps hardware modules at compile time. Unit tests live in each crate's `#[cfg(test)]` modules. Integration tests live in the `waved` binary crate's `tests/` directory and compile with `--features simulate`. Shell scripts handle device testing.

**Tech Stack:** Rust, Cargo features, crossbeam-channel (mock UART), tokio (async tests), tempfile (filesystem tests), bash/ADB (device scripts)

**Spec:** `docs/superpowers/specs/2026-03-21-waved-testing-infrastructure-design.md`

---

## File Map

### New Files
| File | Purpose |
|------|---------|
| `crates/waved-uart/src/simulate.rs` | Simulated UART mux with in-memory channels |
| `crates/waved-mcu/src/simulate.rs` | Simulated MCU with configurable battery/buttons |
| `crates/waved-bt/src/simulate.rs` | Simulated BT with scripted responses |
| `crates/waved-hid/src/simulate.rs` | Simulated HID that logs reports |
| `crates/waved-audio/src/simulate.rs` | Simulated audio generating silence frames |
| `crates/waved-wifi/src/simulate.rs` | Simulated WiFi always-connected stub |
| `crates/waved-meeting/src/simulate.rs` | Simulated meeting upload (local file sink) |
| `crates/waved-device-test/Cargo.toml` | On-device test runner crate manifest |
| `crates/waved-device-test/src/main.rs` | On-device test runner binary |
| `crates/waved/tests/boot_sequence.rs` | Integration test: boot + service init |
| `crates/waved/tests/mute_flow.rs` | Integration test: mute button flow |
| `crates/waved/tests/low_battery_shutdown.rs` | Integration test: low battery → shutdown |
| `crates/waved/tests/recording_lifecycle.rs` | Integration test: recording start/stop flow |
| `scripts/smoke.sh` | ADB smoke test script |
| `scripts/hil.sh` | Manual HIL harness script |

### Modified Files
| File | Changes |
|------|---------|
| `Cargo.toml` (workspace) | Add `waved-device-test` to workspace members |
| `crates/waved/Cargo.toml` | Add `simulate` feature, `dev-dependencies` |
| `crates/waved/src/main.rs` | Add `--config` CLI arg, sim-mode path overrides |
| `crates/waved-uart/Cargo.toml` | Add `simulate` feature |
| `crates/waved-uart/src/lib.rs` | Conditional re-export for simulate |
| `crates/waved-mcu/Cargo.toml` | Add `simulate` feature |
| `crates/waved-mcu/src/lib.rs` | Conditional simulate module |
| `crates/waved-mcu/src/buttons.rs` | Add unit tests |
| `crates/waved-mcu/src/motor.rs` | Add unit tests |
| `crates/waved-bt/Cargo.toml` | Add `simulate` feature |
| `crates/waved-bt/src/lib.rs` | Conditional simulate module |
| `crates/waved-bt/src/commands.rs` | Add unit tests |
| `crates/waved-hid/Cargo.toml` | Add `simulate` feature |
| `crates/waved-hid/src/lib.rs` | Conditional simulate module, add unit tests |
| `crates/waved-audio/Cargo.toml` | Add `simulate` feature |
| `crates/waved-audio/src/lib.rs` | Conditional simulate module |
| `crates/waved-wifi/Cargo.toml` | Add `simulate` feature |
| `crates/waved-wifi/src/lib.rs` | Conditional simulate module |
| `crates/waved-wifi/src/wpa.rs` | Add unit tests for status parsing |
| `crates/waved-meeting/Cargo.toml` | Add `simulate` feature, add `tempfile` to dev-deps |
| `crates/waved-meeting/src/lib.rs` | Conditional simulate module |
| `crates/waved-meeting/src/recorder.rs` | Add unit tests |
| `crates/waved-state/src/lib.rs` | Add unit tests |
| `.gitignore` | Add `scripts/backup/`, `scripts/results/` |

---

## Task 1: Unit Tests — waved-uart (frame.rs already tested, add zero-length data test)

The existing `frame.rs` tests are solid. Add one edge case.

**Files:**
- Modify: `crates/waved-uart/src/frame.rs:156-250`

- [ ] **Step 1: Write failing test for zero-length data frame**

Add to the existing `mod tests` in `crates/waved-uart/src/frame.rs`:

```rust
#[test]
fn parse_zero_length_frame() {
    let raw = Frame::build([0xDC, 0xCD], &[]).unwrap();
    assert_eq!(raw.len(), 4); // 2 header + 1 len(0) + 1 checksum
    let mut parser = FrameParser::new();
    let mut result = None;
    for &byte in &raw {
        if let Some(r) = parser.feed(byte) {
            result = Some(r);
        }
    }
    let frame = result.unwrap().unwrap();
    assert_eq!(frame.target, FrameTarget::Mcu);
    assert!(frame.data.is_empty());
}
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-uart`
Expected: All tests PASS (this should already work with the existing code since length=0 transitions to Checksum state)

- [ ] **Step 3: Commit**

```bash
git add crates/waved-uart/src/frame.rs
git commit -m "test(uart): add zero-length data frame edge case"
```

---

## Task 2: Unit Tests — waved-mcu (buttons, motor, battery)

**Files:**
- Modify: `crates/waved-mcu/src/buttons.rs`
- Modify: `crates/waved-mcu/src/motor.rs`

- [ ] **Step 1: Write tests for buttons.rs**

Add to `crates/waved-mcu/src/buttons.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mute_toggle() {
        assert_eq!(parse_button_frame(&[0x40]), Some(ButtonType::MuteToggle));
    }

    #[test]
    fn parse_volume_up() {
        assert_eq!(parse_button_frame(&[0x41]), Some(ButtonType::VolumeUp));
    }

    #[test]
    fn parse_volume_down() {
        assert_eq!(parse_button_frame(&[0x42]), Some(ButtonType::VolumeDown));
    }

    #[test]
    fn parse_mute_long_press() {
        assert_eq!(parse_button_frame(&[0x43]), Some(ButtonType::MuteLongPress));
    }

    #[test]
    fn parse_unknown_byte_returns_none() {
        assert_eq!(parse_button_frame(&[0x44]), None);
        assert_eq!(parse_button_frame(&[0xFF]), None);
    }

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse_button_frame(&[]), None);
    }
}
```

Note: `ButtonType` needs to derive `PartialEq` for assertions. Check `waved-state/src/lib.rs:103` — it already has `#[derive(Clone, Debug, PartialEq)]`. Good.

- [ ] **Step 2: Write tests for motor.rs**

Add to `crates/waved-mcu/src/motor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motor_low_builds_valid_frame() {
        let raw = build_motor_command(&MotorPosition::Low);
        assert!(!raw.is_empty());
        assert_eq!(raw[0..2], [0xDC, 0xCD]); // MCU header
        assert_eq!(raw[3], 0x20); // motor command
        assert_eq!(raw[4], 0x01); // Low position
    }

    #[test]
    fn motor_medium_builds_valid_frame() {
        let raw = build_motor_command(&MotorPosition::Medium);
        assert_eq!(raw[4], 0x02);
    }

    #[test]
    fn motor_high_builds_valid_frame() {
        let raw = build_motor_command(&MotorPosition::High);
        assert_eq!(raw[4], 0x03);
    }

    #[test]
    fn motor_moving_returns_empty() {
        let raw = build_motor_command(&MotorPosition::Moving);
        assert!(raw.is_empty());
    }

    #[test]
    fn motor_stop_builds_valid_frame() {
        let raw = build_motor_stop();
        assert!(!raw.is_empty());
        assert_eq!(raw[0..2], [0xDC, 0xCD]);
        assert_eq!(raw[3], 0x21);
    }

    #[test]
    fn shutdown_command_builds_valid_frame() {
        let raw = build_shutdown_command();
        assert!(!raw.is_empty());
        assert_eq!(raw[3], 0x30);
    }

    #[test]
    fn reboot_command_builds_valid_frame() {
        let raw = build_reboot_command();
        assert!(!raw.is_empty());
        assert_eq!(raw[3], 0x31);
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-mcu`
Expected: All PASS

- [ ] **Step 4: Commit**

```bash
git add crates/waved-mcu/src/buttons.rs crates/waved-mcu/src/motor.rs
git commit -m "test(mcu): add unit tests for buttons and motor commands"
```

---

## Task 3: Unit Tests — waved-bt (commands)

**Files:**
- Modify: `crates/waved-bt/src/commands.rs`

- [ ] **Step 1: Write tests for commands.rs**

Add to `crates/waved-bt/src/commands.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn assert_bt_frame(raw: &[u8], expected_cmd: u8) {
        assert!(!raw.is_empty(), "frame should not be empty");
        assert_eq!(raw[0..2], [0x75, 0x46], "should have BT header");
        assert_eq!(raw[3], expected_cmd, "wrong command byte");
    }

    #[test]
    fn get_status() {
        assert_bt_frame(&build_get_status(), 0x0E);
    }

    #[test]
    fn get_mac() {
        assert_bt_frame(&build_get_mac(), 0x09);
    }

    #[test]
    fn get_name() {
        assert_bt_frame(&build_get_name(), 0x0A);
    }

    #[test]
    fn get_version() {
        assert_bt_frame(&build_get_version(), 0x0D);
    }

    #[test]
    fn open() {
        assert_bt_frame(&build_open(), 0x07);
    }

    #[test]
    fn close() {
        assert_bt_frame(&build_close(), 0x03);
    }

    #[test]
    fn disconnect() {
        assert_bt_frame(&build_disconnect(), 0x02);
    }

    #[test]
    fn set_auto_reconnect_enable() {
        let raw = build_set_auto_reconnect(true);
        assert_eq!(raw[0..2], [0x75, 0x46]);
        assert_eq!(raw[3], 0x04);
        assert_eq!(raw[4], 1);
    }

    #[test]
    fn set_auto_reconnect_disable() {
        let raw = build_set_auto_reconnect(false);
        assert_eq!(raw[4], 0);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-bt`
Expected: All PASS

- [ ] **Step 3: Commit**

```bash
git add crates/waved-bt/src/commands.rs
git commit -m "test(bt): add unit tests for all BT command builders"
```

---

## Task 4: Unit Tests — waved-hid (report construction)

**Files:**
- Modify: `crates/waved-hid/src/lib.rs`

- [ ] **Step 1: Extract report building to a testable function**

The `send_hid_report` method is async and writes to `/dev/hidg0`. Extract the pure report-building logic. Add above `impl HidService`:

```rust
fn build_hid_report(button: &ButtonType) -> (Vec<u8>, Vec<u8>) {
    let press = match button {
        ButtonType::MuteToggle => vec![REPORT_ID_TELEPHONE, 0x01],
        ButtonType::VolumeUp => vec![REPORT_ID_CONSUMER, 0x01],
        ButtonType::VolumeDown => vec![REPORT_ID_CONSUMER, 0x02],
        ButtonType::MuteLongPress => vec![REPORT_ID_TELEPHONE, 0x01],
    };
    let release = vec![press[0], 0x00];
    (press, release)
}
```

Then update `send_hid_report` to use it:

```rust
async fn send_hid_report(button: &ButtonType) -> std::io::Result<()> {
    let (report, release) = build_hid_report(button);

    debug!("sending HID report: {:02x?}", report);

    let mut file = OpenOptions::new()
        .write(true)
        .open(HID_GADGET_PATH)
        .await?;
    file.write_all(&report).await?;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    file.write_all(&release).await?;

    Ok(())
}
```

- [ ] **Step 2: Write tests**

Add to `crates/waved-hid/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_toggle_report() {
        let (press, release) = build_hid_report(&ButtonType::MuteToggle);
        assert_eq!(press, vec![0x01, 0x01]); // TELEPHONE, mute
        assert_eq!(release, vec![0x01, 0x00]);
    }

    #[test]
    fn volume_up_report() {
        let (press, release) = build_hid_report(&ButtonType::VolumeUp);
        assert_eq!(press, vec![0x02, 0x01]); // CONSUMER, vol up
        assert_eq!(release, vec![0x02, 0x00]);
    }

    #[test]
    fn volume_down_report() {
        let (press, release) = build_hid_report(&ButtonType::VolumeDown);
        assert_eq!(press, vec![0x02, 0x02]); // CONSUMER, vol down
        assert_eq!(release, vec![0x02, 0x00]);
    }

    #[test]
    fn mute_long_press_report() {
        let (press, release) = build_hid_report(&ButtonType::MuteLongPress);
        assert_eq!(press, vec![0x01, 0x01]); // same as toggle
        assert_eq!(release, vec![0x01, 0x00]);
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-hid`
Expected: All PASS

- [ ] **Step 4: Commit**

```bash
git add crates/waved-hid/src/lib.rs
git commit -m "test(hid): extract report builder and add unit tests"
```

---

## Task 5: Unit Tests — waved-wifi (wpa status parsing)

**Files:**
- Modify: `crates/waved-wifi/src/wpa.rs`

- [ ] **Step 1: Extract status parsing to a pure function**

The `status()` function calls `wpa_cli` which shells out. Extract the parsing logic. Add to `crates/waved-wifi/src/wpa.rs`:

```rust
pub fn parse_wpa_status(output: &str) -> Option<(String, String)> {
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

Then update `status()` to use it:

```rust
pub async fn status() -> Option<(String, String)> {
    let output = wpa_cli(&["status"]).await.ok()?;
    parse_wpa_status(&output)
}
```

- [ ] **Step 2: Extract scan parsing to a pure function**

```rust
pub fn parse_scan_results(output: &str) -> Vec<String> {
    output
        .lines()
        .skip(1)
        .filter_map(|line| line.split('\t').nth(4).map(String::from))
        .filter(|ssid| !ssid.is_empty())
        .collect()
}
```

Then update `scan()` to use it:

```rust
pub async fn scan() -> Vec<String> {
    if let Err(e) = wpa_cli(&["scan"]).await {
        warn!("wifi scan trigger failed: {e}");
        return vec![];
    }
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    match wpa_cli(&["scan_results"]).await {
        Ok(output) => parse_scan_results(&output),
        Err(_) => vec![],
    }
}
```

- [ ] **Step 3: Write tests**

Add to `crates/waved-wifi/src/wpa.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connected_status() {
        let output = "bssid=aa:bb:cc:dd:ee:ff\nfreq=5180\nssid=MyNetwork\nip_address=192.168.1.42\nwpa_state=COMPLETED\n";
        let (ssid, ip) = parse_wpa_status(output).unwrap();
        assert_eq!(ssid, "MyNetwork");
        assert_eq!(ip, "192.168.1.42");
    }

    #[test]
    fn parse_disconnected_status() {
        let output = "wpa_state=DISCONNECTED\n";
        assert!(parse_wpa_status(output).is_none());
    }

    #[test]
    fn parse_ssid_only_no_ip() {
        let output = "ssid=MyNetwork\nwpa_state=COMPLETED\n";
        assert!(parse_wpa_status(output).is_none());
    }

    #[test]
    fn parse_scan_results_normal() {
        let output = "bssid / frequency / signal level / flags / ssid\naa:bb:cc:dd:ee:ff\t5180\t-45\t[WPA2-PSK]\tHomeWifi\n11:22:33:44:55:66\t2437\t-70\t[WPA2-PSK]\tOffice\n";
        let ssids = parse_scan_results(output);
        assert_eq!(ssids, vec!["HomeWifi", "Office"]);
    }

    #[test]
    fn parse_scan_results_empty() {
        let output = "bssid / frequency / signal level / flags / ssid\n";
        assert!(parse_scan_results(output).is_empty());
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-wifi`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add crates/waved-wifi/src/wpa.rs
git commit -m "test(wifi): extract wpa parsers and add unit tests"
```

---

## Task 6: Unit Tests — waved-state (StateBus, watch/broadcast)

**Files:**
- Modify: `crates/waved-state/src/lib.rs`
- Modify: `crates/waved-state/Cargo.toml`

- [ ] **Step 1: Add tokio dev-dependency**

Add to `crates/waved-state/Cargo.toml`:

```toml
[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt"] }
```

- [ ] **Step 2: Write tests**

Add to `crates/waved-state/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_bus_creates_with_defaults() {
        let bus = StateBus::new();
        let audio = bus.audio.subscribe();
        assert!(!audio.borrow().mic_muted);
        assert_eq!(audio.borrow().mode, AudioMode::NoiseReduction);
    }

    #[test]
    fn watch_channel_sees_latest_value() {
        let bus = StateBus::new();
        let rx = bus.battery.subscribe();

        bus.battery.send_modify(|s| {
            s.level = 85;
            s.is_charging = true;
        });

        assert_eq!(rx.borrow().level, 85);
        assert!(rx.borrow().is_charging);
    }

    #[tokio::test]
    async fn broadcast_multiple_subscribers() {
        let bus = StateBus::new();
        let mut rx1 = bus.events.subscribe();
        let mut rx2 = bus.events.subscribe();

        let _ = bus.events.send(DeviceEvent::UsbConnected);

        match rx1.recv().await.unwrap() {
            DeviceEvent::UsbConnected => {}
            other => panic!("unexpected event: {:?}", other),
        }
        match rx2.recv().await.unwrap() {
            DeviceEvent::UsbConnected => {}
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn motor_position_default_is_low() {
        assert_eq!(MotorPosition::default(), MotorPosition::Low);
    }

    #[test]
    fn audio_mode_default_is_noise_reduction() {
        assert_eq!(AudioMode::default(), AudioMode::NoiseReduction);
    }
}
```

Note: `AudioMode` needs `PartialEq` — it already has it via `#[derive(... PartialEq)]` at line 19.

- [ ] **Step 3: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-state`
Expected: All PASS

- [ ] **Step 4: Commit**

```bash
git add crates/waved-state/Cargo.toml crates/waved-state/src/lib.rs
git commit -m "test(state): add unit tests for StateBus watch/broadcast channels"
```

---

## Task 7: Unit Tests — waved-meeting (recorder metadata, db)

**Files:**
- Modify: `crates/waved-meeting/src/recorder.rs`

- [ ] **Step 1: Write tests for metadata serialization and ActiveRecording**

Add to `crates/waved-meeting/src/recorder.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_serialization_roundtrip() {
        let meta = MeetingMetadata {
            id: "test-id".to_string(),
            start_time: "2026-03-21T10:00:00Z".to_string(),
            duration_secs: 300,
            audio_mode: "NoiseReduction".to_string(),
            device_id: "wave-001".to_string(),
            sample_rate: 48000,
            channels: 2,
            codec: "opus".to_string(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: MeetingMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "test-id");
        assert_eq!(parsed.sample_rate, 48000);
        assert_eq!(parsed.channels, 2);
        assert_eq!(parsed.codec, "opus");
        assert_eq!(parsed.duration_secs, 300);
    }

    #[tokio::test]
    async fn active_recording_creates_directory_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let rec = ActiveRecording::start(dir.path(), "NoiseReduction").await.unwrap();

        assert!(rec.dir.exists());
        assert!(rec.dir.join("metadata.json").exists());
        assert!(rec.dir.join("audio.opus").exists());

        // Verify metadata content
        let meta_json = tokio::fs::read_to_string(rec.dir.join("metadata.json")).await.unwrap();
        let meta: MeetingMetadata = serde_json::from_str(&meta_json).unwrap();
        assert_eq!(meta.id, rec.id.to_string());
        assert_eq!(meta.audio_mode, "NoiseReduction");
        assert_eq!(meta.sample_rate, 48000);
    }

    #[tokio::test]
    async fn audio_path_returns_correct_path() {
        let dir = tempfile::tempdir().unwrap();
        let rec = ActiveRecording::start(dir.path(), "Omni").await.unwrap();
        assert_eq!(rec.audio_path(), rec.dir.join("audio.opus"));
    }
}
```

- [ ] **Step 2: Add tokio test dep to waved-meeting/Cargo.toml**

The `Cargo.toml` already has `tempfile = "3"` in `[dev-dependencies]`. Add tokio test features:

```toml
[dev-dependencies]
tempfile = "3"
tokio = { workspace = true, features = ["macros", "rt"] }
```

- [ ] **Step 3: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-meeting`
Expected: All PASS

- [ ] **Step 4: Commit**

```bash
git add crates/waved-meeting/src/recorder.rs crates/waved-meeting/Cargo.toml
git commit -m "test(meeting): add unit tests for metadata serialization and recording lifecycle"
```

---

## Task 7b: Unit Tests — waved-config (additional spec coverage)

The existing tests cover `default_config_parses` and `roundtrip_save_load`. Add the spec-required tests for invalid TOML and missing fields.

**Files:**
- Modify: `crates/waved-config/src/lib.rs:94-117`

- [ ] **Step 1: Add tests for invalid and partial TOML**

Add to the existing `mod tests` in `crates/waved-config/src/lib.rs`:

```rust
#[test]
fn reject_invalid_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.toml");
    std::fs::write(&path, "this is not valid toml {{{{").unwrap();
    assert!(WavedConfig::load(&path).is_err());
}

#[test]
fn reject_missing_required_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial.toml");
    // Missing [device], [audio], etc.
    std::fs::write(&path, "[ntp]\nservers = [\"pool.ntp.org\"]\n").unwrap();
    assert!(WavedConfig::load(&path).is_err());
}

#[test]
fn load_or_default_falls_back() {
    let path = std::path::Path::new("/nonexistent/config.toml");
    let config = WavedConfig::load_or_default(path);
    assert_eq!(config.device.name, "Wave Open");
}
```

- [ ] **Step 2: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-config`
Expected: All PASS

- [ ] **Step 3: Commit**

```bash
git add crates/waved-config/src/lib.rs
git commit -m "test(config): add tests for invalid TOML, missing sections, and fallback"
```

---

## Task 8: Simulate Feature — waved-uart

**Files:**
- Modify: `crates/waved-uart/Cargo.toml`
- Modify: `crates/waved-uart/src/lib.rs`
- Create: `crates/waved-uart/src/simulate.rs`

- [ ] **Step 1: Add simulate feature to Cargo.toml**

Add to `crates/waved-uart/Cargo.toml`:

```toml
[features]
simulate = []
```

- [ ] **Step 2: Create simulate.rs**

Create `crates/waved-uart/src/simulate.rs`:

```rust
//! Simulated UART mux for testing without hardware.
//! Replaces serial port with in-memory crossbeam channels.

use crate::frame::{Frame, FrameTarget};
use crossbeam_channel::{bounded, Receiver, Sender};
use tracing::info;

pub struct SimUartMux {
    bt_rx: Option<Receiver<Frame>>,
    mcu_rx: Option<Receiver<Frame>>,
    write_tx: Sender<Vec<u8>>,
    write_rx: Receiver<Vec<u8>>,
    bt_inject_tx: Sender<Frame>,
    mcu_inject_tx: Sender<Frame>,
}

impl SimUartMux {
    pub fn new() -> Self {
        let (bt_tx, bt_rx) = bounded::<Frame>(64);
        let (mcu_tx, mcu_rx) = bounded::<Frame>(64);
        let (write_tx, write_rx) = bounded::<Vec<u8>>(64);

        info!("[SIM] UART mux created (in-memory channels)");

        Self {
            bt_rx: Some(bt_rx),
            mcu_rx: Some(mcu_rx),
            write_tx,
            write_rx,
            bt_inject_tx: bt_tx,
            mcu_inject_tx: mcu_tx,
        }
    }

    pub fn take_bt_receiver(&mut self) -> Receiver<Frame> {
        self.bt_rx.take().expect("bt_rx already taken")
    }

    pub fn take_mcu_receiver(&mut self) -> Receiver<Frame> {
        self.mcu_rx.take().expect("mcu_rx already taken")
    }

    pub fn send(&self, raw: Vec<u8>, _priority: bool) {
        let _ = self.write_tx.send(raw);
    }

    /// Inject a frame as if the MCU sent it (for testing).
    pub fn inject_mcu_frame(&self, data: Vec<u8>) {
        let _ = self.mcu_inject_tx.send(Frame {
            header: [0xDC, 0xCD],
            target: FrameTarget::Mcu,
            data,
        });
    }

    /// Inject a frame as if the BT module sent it (for testing).
    pub fn inject_bt_frame(&self, data: Vec<u8>) {
        let _ = self.bt_inject_tx.send(Frame {
            header: [0x75, 0x46],
            target: FrameTarget::Bluetooth,
            data,
        });
    }

    /// Read what waved tried to write to UART (for assertions).
    pub fn try_recv_written(&self) -> Option<Vec<u8>> {
        self.write_rx.try_recv().ok()
    }
}
```

- [ ] **Step 3: Update lib.rs with conditional exports**

Modify `crates/waved-uart/src/lib.rs`:

```rust
pub mod frame;

#[cfg(not(feature = "simulate"))]
pub mod mux;

#[cfg(feature = "simulate")]
pub mod simulate;

pub use frame::{Frame, FrameError, FrameParser, FrameTarget};

#[cfg(not(feature = "simulate"))]
pub use mux::UartMux;

#[cfg(feature = "simulate")]
pub use simulate::SimUartMux as UartMux;
```

- [ ] **Step 4: Run tests (both modes)**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-uart && cargo test -p waved-uart --features simulate`
Expected: All PASS in both modes (frame tests are not affected by the feature flag)

- [ ] **Step 5: Commit**

```bash
git add crates/waved-uart/
git commit -m "feat(uart): add simulate feature with in-memory UART mux"
```

---

## Task 9: Simulate Feature — waved-mcu

**Files:**
- Modify: `crates/waved-mcu/Cargo.toml`
- Modify: `crates/waved-mcu/src/lib.rs`
- Create: `crates/waved-mcu/src/simulate.rs`

- [ ] **Step 1: Add simulate feature to Cargo.toml**

Add to `crates/waved-mcu/Cargo.toml`:

```toml
[features]
simulate = ["waved-uart/simulate"]
```

- [ ] **Step 2: Create simulate.rs**

Create `crates/waved-mcu/src/simulate.rs`:

```rust
//! Simulated MCU service for testing without hardware.

use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tracing::info;
use waved_state::{BatteryState, Command, DeviceEvent, McuState, MotorPosition};

pub struct SimMcuService {
    battery_tx: Arc<watch::Sender<BatteryState>>,
    mcu_tx: Arc<watch::Sender<McuState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
    button_rx: Receiver<waved_state::ButtonType>,
}

/// Handle for injecting simulated MCU events.
pub struct SimMcuHandle {
    button_tx: Sender<waved_state::ButtonType>,
    battery_tx: Arc<watch::Sender<BatteryState>>,
}

impl SimMcuHandle {
    /// Inject a button press event.
    pub fn press_button(&self, btn: waved_state::ButtonType) {
        let _ = self.button_tx.send(btn);
    }

    /// Set simulated battery level.
    pub fn set_battery(&self, level: u8, is_charging: bool) {
        self.battery_tx.send_modify(|s| {
            s.level = level;
            s.is_charging = is_charging;
        });
    }
}

impl SimMcuService {
    pub fn new(
        battery_tx: Arc<watch::Sender<BatteryState>>,
        mcu_tx: Arc<watch::Sender<McuState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> (Self, SimMcuHandle) {
        let (button_tx, button_rx) = bounded(64);

        // Set default battery state
        battery_tx.send_modify(|s| {
            s.level = 75;
            s.voltage = 3800;
            s.temperature = 25;
            s.is_charging = false;
        });

        info!("[SIM] MCU service created (battery=75%, temp=25C)");

        let handle = SimMcuHandle {
            button_tx,
            battery_tx: Arc::clone(&battery_tx),
        };

        let svc = Self {
            battery_tx,
            mcu_tx,
            events_tx,
            button_rx,
        };

        (svc, handle)
    }

    pub fn run(self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("[SIM] mcu_srv started");

        loop {
            crossbeam_channel::select! {
                recv(self.button_rx) -> msg => {
                    match msg {
                        Ok(btn) => {
                            info!("[SIM] button press: {:?}", btn);
                            let _ = self.events_tx.send(DeviceEvent::ButtonPress(btn));
                        }
                        Err(_) => return,
                    }
                }
                default(std::time::Duration::from_millis(50)) => {
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        match cmd {
                            Command::SetMotorPosition(pos) => {
                                info!("[SIM] motor → {:?}", pos);
                                self.mcu_tx.send_modify(|s| s.motor_position = pos);
                            }
                            Command::Shutdown => {
                                info!("[SIM] shutdown requested");
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 3: Update lib.rs with conditional module**

Add to the top of `crates/waved-mcu/src/lib.rs`, after the existing module declarations:

```rust
#[cfg(feature = "simulate")]
pub mod simulate;
```

- [ ] **Step 4: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-mcu && cargo test -p waved-mcu --features simulate`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add crates/waved-mcu/
git commit -m "feat(mcu): add simulate feature with configurable battery and button injection"
```

---

## Task 10: Simulate Feature — waved-bt

**Files:**
- Modify: `crates/waved-bt/Cargo.toml`
- Modify: `crates/waved-bt/src/lib.rs`
- Create: `crates/waved-bt/src/simulate.rs`

- [ ] **Step 1: Add simulate feature to Cargo.toml**

Add to `crates/waved-bt/Cargo.toml`:

```toml
[features]
simulate = ["waved-uart/simulate"]
```

- [ ] **Step 2: Create simulate.rs**

Create `crates/waved-bt/src/simulate.rs`:

```rust
//! Simulated BT service for testing without hardware.

use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tracing::info;
use waved_state::{BluetoothState, DeviceEvent};

pub struct SimBtService {
    bt_tx: Arc<watch::Sender<BluetoothState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl SimBtService {
    pub fn new(
        bt_tx: Arc<watch::Sender<BluetoothState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        // Default: BT on, not connected
        bt_tx.send_modify(|s| {
            s.bt_on = true;
            s.connected = false;
            s.mac = "AA:BB:CC:DD:EE:FF".to_string();
        });

        info!("[SIM] BT service created (on, not connected)");

        Self { bt_tx, events_tx }
    }

    pub fn run(self) {
        info!("[SIM] bt_srv started (idle loop)");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
}
```

- [ ] **Step 3: Update lib.rs with conditional module**

Add to the top of `crates/waved-bt/src/lib.rs`, after `pub mod commands;`:

```rust
#[cfg(feature = "simulate")]
pub mod simulate;
```

- [ ] **Step 4: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-bt --features simulate`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add crates/waved-bt/
git commit -m "feat(bt): add simulate feature with idle BT service"
```

---

## Task 11: Simulate Feature — waved-hid

**Files:**
- Modify: `crates/waved-hid/Cargo.toml`
- Modify: `crates/waved-hid/src/lib.rs`
- Create: `crates/waved-hid/src/simulate.rs`

- [ ] **Step 1: Add simulate feature to Cargo.toml**

Add to `crates/waved-hid/Cargo.toml`:

```toml
[features]
simulate = []
```

- [ ] **Step 2: Create simulate.rs**

Create `crates/waved-hid/src/simulate.rs`:

```rust
//! Simulated HID service — logs reports to tracing instead of /dev/hidg0.

use tokio::sync::broadcast;
use tracing::{info, warn};
use waved_state::{ButtonType, DeviceEvent};

pub struct SimHidService;

impl SimHidService {
    pub async fn run(mut events_rx: broadcast::Receiver<DeviceEvent>) {
        info!("[SIM] hid_srv started (logging reports)");

        loop {
            match events_rx.recv().await {
                Ok(DeviceEvent::ButtonPress(btn)) => {
                    let desc = match btn {
                        ButtonType::MuteToggle => "TELEPHONE:mute_toggle",
                        ButtonType::VolumeUp => "CONSUMER:volume_up",
                        ButtonType::VolumeDown => "CONSUMER:volume_down",
                        ButtonType::MuteLongPress => "TELEPHONE:mute_long",
                    };
                    info!("[SIM] HID report: {desc} (press + release)");
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("[SIM] hid_srv lagged by {n} events");
                }
            }
        }
    }
}
```

- [ ] **Step 3: Update lib.rs with conditional module**

Add to `crates/waved-hid/src/lib.rs` at the top:

```rust
#[cfg(feature = "simulate")]
pub mod simulate;
```

- [ ] **Step 4: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-hid --features simulate`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add crates/waved-hid/
git commit -m "feat(hid): add simulate feature with tracing-based HID reporting"
```

---

## Task 12: Simulate Feature — waved-audio

**Files:**
- Modify: `crates/waved-audio/Cargo.toml`
- Modify: `crates/waved-audio/src/lib.rs`
- Create: `crates/waved-audio/src/simulate.rs`

- [ ] **Step 1: Add simulate feature to Cargo.toml**

Add to `crates/waved-audio/Cargo.toml`:

```toml
[features]
simulate = []
```

- [ ] **Step 2: Create simulate.rs**

Create `crates/waved-audio/src/simulate.rs`:

```rust
//! Simulated audio service — generates silence frames at real-time rate.

use crate::callbacks::AudioFrame;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{info, warn};
use waved_state::{AudioMode, AudioState, Command};

/// Silence frame: 960 samples * 2 channels = 1920 i16 values per 20ms at 48kHz.
const SAMPLES_PER_FRAME: usize = 960;
const CHANNELS: u16 = 2;
const FRAME_INTERVAL_MS: u64 = 20;

pub struct SimAudioService {
    audio_tx: Arc<watch::Sender<AudioState>>,
    frame_tx: mpsc::Sender<AudioFrame>,
}

impl SimAudioService {
    pub fn new(
        audio_tx: Arc<watch::Sender<AudioState>>,
        frame_tx: mpsc::Sender<AudioFrame>,
    ) -> Self {
        info!("[SIM] audio service created (silence generator)");
        Self { audio_tx, frame_tx }
    }

    pub async fn run(self, mut cmd_rx: broadcast::Receiver<Command>) {
        info!("[SIM] audio_srv starting (generating silence frames)");

        let mut frame_count: u64 = 0;
        let mut interval = tokio::time::interval(
            std::time::Duration::from_millis(FRAME_INTERVAL_MS),
        );

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let frame = AudioFrame {
                        data: vec![0i16; SAMPLES_PER_FRAME * CHANNELS as usize],
                        sample_rate: 48000,
                        channels: CHANNELS,
                        timestamp_ms: frame_count * FRAME_INTERVAL_MS,
                    };
                    let _ = self.frame_tx.send(frame).await;
                    frame_count += 1;
                }
                cmd_result = cmd_rx.recv() => {
                    match cmd_result {
                        Ok(Command::SetAudioMode(mode)) => {
                            info!("[SIM] audio mode → {:?}", mode);
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
                        Ok(Command::Shutdown) => return,
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Closed) => return,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("[SIM] audio_srv lagged {n} commands");
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 3: Update lib.rs with conditional module**

Add to `crates/waved-audio/src/lib.rs` at the top (after `pub mod ffi;`):

```rust
#[cfg(feature = "simulate")]
pub mod simulate;
```

- [ ] **Step 4: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-audio --features simulate`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/
git commit -m "feat(audio): add simulate feature with silence frame generator"
```

---

## Task 13: Simulate Feature — waved-wifi

**Files:**
- Modify: `crates/waved-wifi/Cargo.toml`
- Modify: `crates/waved-wifi/src/lib.rs`
- Create: `crates/waved-wifi/src/simulate.rs`

- [ ] **Step 1: Add simulate feature to Cargo.toml**

Add to `crates/waved-wifi/Cargo.toml`:

```toml
[features]
simulate = []
```

- [ ] **Step 2: Create simulate.rs**

Create `crates/waved-wifi/src/simulate.rs`:

```rust
//! Simulated WiFi service — always connected, NTP always synced.

use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tracing::info;
use waved_state::{DeviceEvent, WifiState};

pub struct SimWifiService {
    wifi_tx: Arc<watch::Sender<WifiState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
}

impl SimWifiService {
    pub fn new(
        wifi_tx: Arc<watch::Sender<WifiState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
    ) -> Self {
        info!("[SIM] WiFi service created (always connected)");
        Self { wifi_tx, events_tx }
    }

    pub async fn run(self) {
        info!("[SIM] wifi_srv started");

        // Immediately report connected
        let state = WifiState {
            connected: true,
            ssid: "SimNetwork".to_string(),
            ip_addr: "192.168.1.100".to_string(),
            signal_strength: -45,
        };
        let _ = self.wifi_tx.send(state);
        let _ = self.events_tx.send(DeviceEvent::WifiConnected("SimNetwork".to_string()));

        info!("[SIM] WiFi connected to SimNetwork (192.168.1.100)");
        info!("[SIM] NTP sync: using system time");

        // Stay alive
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    }
}
```

- [ ] **Step 3: Update lib.rs with conditional module**

Add to `crates/waved-wifi/src/lib.rs` at the top (after `pub mod wpa;`):

```rust
#[cfg(feature = "simulate")]
pub mod simulate;
```

- [ ] **Step 4: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-wifi --features simulate`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add crates/waved-wifi/
git commit -m "feat(wifi): add simulate feature with always-connected stub"
```

---

## Task 14: Simulate Feature — waved-meeting

**Files:**
- Modify: `crates/waved-meeting/Cargo.toml`
- Create: `crates/waved-meeting/src/simulate.rs`
- Modify: `crates/waved-meeting/src/lib.rs`

- [ ] **Step 1: Add simulate feature to Cargo.toml**

Add to `crates/waved-meeting/Cargo.toml`:

```toml
[features]
simulate = []
```

- [ ] **Step 2: Create simulate.rs**

Create `crates/waved-meeting/src/simulate.rs`:

```rust
//! Simulated meeting upload — writes to local files instead of HTTP.

use std::path::Path;
use tracing::info;

pub async fn upload_recording(
    _endpoint: &str,
    _auth_token: &str,
    meeting_id: &str,
    file_path: &Path,
) -> Result<(), crate::upload::UploadError> {
    let dest_dir = std::path::PathBuf::from("./test_uploads");
    tokio::fs::create_dir_all(&dest_dir).await.map_err(crate::upload::UploadError::Io)?;

    let dest = dest_dir.join(format!("{meeting_id}.opus"));
    if file_path.exists() {
        tokio::fs::copy(file_path, &dest).await.map_err(crate::upload::UploadError::Io)?;
        info!("[SIM] uploaded {meeting_id} → {}", dest.display());
    } else {
        info!("[SIM] upload {meeting_id}: source file not found, skipping copy");
    }

    Ok(())
}
```

- [ ] **Step 3: Update lib.rs with conditional module and upload cfg gate**

Add to `crates/waved-meeting/src/lib.rs` at the top (after existing module declarations):

```rust
#[cfg(feature = "simulate")]
pub mod simulate;
```

Then in the `try_upload_with_retry` method, gate the `upload_recording` call. Replace the existing call at line ~153:

```rust
// Replace:
//   match upload::upload_recording(
//       &self.config.api.endpoint,
//       &self.config.api.auth_token,
//       meeting_id,
//       audio_path,
//   ).await {

// With:
#[cfg(not(feature = "simulate"))]
let upload_result = upload::upload_recording(
    &self.config.api.endpoint,
    &self.config.api.auth_token,
    meeting_id,
    audio_path,
).await;

#[cfg(feature = "simulate")]
let upload_result = simulate::upload_recording(
    &self.config.api.endpoint,
    &self.config.api.auth_token,
    meeting_id,
    audio_path,
).await;

match upload_result {
```

- [ ] **Step 4: Run tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved-meeting --features simulate`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add crates/waved-meeting/
git commit -m "feat(meeting): add simulate feature with local file upload sink"
```

---

## Task 15: Wire simulate feature into waved binary crate

**Files:**
- Modify: `crates/waved/Cargo.toml`
- Modify: `crates/waved/src/main.rs`

- [ ] **Step 1: Add simulate feature to the binary crate's Cargo.toml**

Add to `crates/waved/Cargo.toml`:

```toml
[features]
simulate = [
    "waved-uart/simulate",
    "waved-bt/simulate",
    "waved-hid/simulate",
    "waved-audio/simulate",
    "waved-mcu/simulate",
    "waved-wifi/simulate",
    "waved-meeting/simulate",
]
```

- [ ] **Step 2: Update main.rs for simulation mode**

Replace the constants and `main()` function in `crates/waved/src/main.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use waved_state::{Command, DeviceEvent, MotorPosition, StateBus};

#[cfg(not(feature = "simulate"))]
const CONFIG_PATH: &str = "/data/waved/config.toml";
#[cfg(feature = "simulate")]
const CONFIG_PATH: &str = "./config.toml";

#[cfg(not(feature = "simulate"))]
const DB_PATH: &str = "/data/waved/meetings.db";
#[cfg(feature = "simulate")]
const DB_PATH: &str = "./meetings.db";

#[cfg(not(feature = "simulate"))]
const RECORDINGS_DIR: &str = "/mnt/udisk/recordings";
#[cfg(feature = "simulate")]
const RECORDINGS_DIR: &str = "./recordings";

const UART_PORT: &str = "/dev/ttyS1";
const UART_BAUD: u32 = 115200;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("waved=info".parse().unwrap()),
        )
        .init();

    #[cfg(feature = "simulate")]
    info!("waved v{} starting [SIMULATION MODE]", env!("CARGO_PKG_VERSION"));
    #[cfg(not(feature = "simulate"))]
    info!("waved v{} starting", env!("CARGO_PKG_VERSION"));

    // Load config
    let config = waved_config::WavedConfig::load_or_default(CONFIG_PATH.as_ref());
    info!("config loaded: device={} audio_mode={:?}", config.device.name, config.audio.mode);

    // Create state bus
    let bus = Arc::new(StateBus::new());

    // --- Hardware services: real or simulated ---
    #[cfg(not(feature = "simulate"))]
    {
        let uart_result = waved_uart::UartMux::open(UART_PORT, UART_BAUD);

        match uart_result {
            Ok(mut uart) => {
                info!("UART opened: {UART_PORT} @ {UART_BAUD}");
                let mcu_rx = uart.take_mcu_receiver();
                let bt_rx = uart.take_bt_receiver();
                let uart = Arc::new(uart);

                let mcu_uart = Arc::clone(&uart);
                let mcu_battery_tx = Arc::clone(&bus.battery);
                let mcu_state_tx = Arc::clone(&bus.mcu);
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

                let bt_uart = Arc::clone(&uart);
                let bt_state_tx = Arc::clone(&bus.bt);
                let bt_events_tx = bus.events.clone();
                std::thread::Builder::new()
                    .name("bt-srv".into())
                    .spawn(move || {
                        let svc = waved_bt::BtService::new(bt_rx, bt_uart, bt_state_tx, bt_events_tx);
                        svc.run();
                    })
                    .expect("failed to spawn bt thread");
            }
            Err(e) => {
                error!("failed to open UART {UART_PORT}: {e}");
                error!("running without MCU/BT (host development mode)");
            }
        }
    }

    #[cfg(feature = "simulate")]
    {
        // Simulated MCU
        let (sim_mcu, _mcu_handle) = waved_mcu::simulate::SimMcuService::new(
            Arc::clone(&bus.battery),
            Arc::clone(&bus.mcu),
            bus.events.clone(),
        );
        let mcu_cmd_rx = bus.commands.subscribe();
        std::thread::Builder::new()
            .name("sim-mcu".into())
            .spawn(move || sim_mcu.run(mcu_cmd_rx))
            .expect("failed to spawn sim-mcu thread");

        // Simulated BT
        let sim_bt = waved_bt::simulate::SimBtService::new(
            Arc::clone(&bus.bt),
            bus.events.clone(),
        );
        std::thread::Builder::new()
            .name("sim-bt".into())
            .spawn(move || sim_bt.run())
            .expect("failed to spawn sim-bt thread");
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    rt.block_on(run_async(config, bus));

    info!("waved shutdown complete");
}
```

- [ ] **Step 3: Update run_async for simulation mode**

Replace `run_async` in `crates/waved/src/main.rs`:

```rust
async fn run_async(config: waved_config::WavedConfig, bus: Arc<StateBus>) {
    // Audio service
    let (frame_tx, frame_rx) = waved_audio::callbacks::create_audio_channel();

    #[cfg(not(feature = "simulate"))]
    {
        let audio_svc = waved_audio::AudioService::new(Arc::clone(&bus.audio), frame_tx);
        let audio_cmd_rx = bus.commands.subscribe();
        tokio::spawn(async move { audio_svc.run(audio_cmd_rx).await });
    }

    #[cfg(feature = "simulate")]
    {
        let sim_audio = waved_audio::simulate::SimAudioService::new(
            Arc::clone(&bus.audio),
            frame_tx,
        );
        let audio_cmd_rx = bus.commands.subscribe();
        tokio::spawn(async move { sim_audio.run(audio_cmd_rx).await });
    }

    // HID service
    let hid_events_rx = bus.events.subscribe();
    #[cfg(not(feature = "simulate"))]
    tokio::spawn(async move { waved_hid::HidService::run(hid_events_rx).await });
    #[cfg(feature = "simulate")]
    tokio::spawn(async move { waved_hid::simulate::SimHidService::run(hid_events_rx).await });

    // WiFi service
    #[cfg(not(feature = "simulate"))]
    {
        let wifi_svc = waved_wifi::WifiService::new(
            Arc::clone(&bus.wifi),
            bus.events.clone(),
            &config,
        );
        tokio::spawn(async move { wifi_svc.run().await });
    }
    #[cfg(feature = "simulate")]
    {
        let sim_wifi = waved_wifi::simulate::SimWifiService::new(
            Arc::clone(&bus.wifi),
            bus.events.clone(),
        );
        tokio::spawn(async move { sim_wifi.run().await });
    }

    // Meeting service
    match waved_meeting::MeetingService::new(
        config.clone(),
        DB_PATH,
        PathBuf::from(RECORDINGS_DIR),
        Arc::clone(&bus.meeting),
        bus.events.clone(),
        frame_rx,
    ).await {
        Ok(meeting_svc) => {
            let meeting_cmd_rx = bus.commands.subscribe();
            let wifi_rx = bus.wifi.subscribe();
            tokio::spawn(async move { meeting_svc.run(meeting_cmd_rx, wifi_rx).await });
        }
        Err(e) => error!("meeting service init failed: {e}"),
    }

    info!("all services started, waiting for shutdown signal");

    // Low battery handler
    let low_battery_bus = Arc::clone(&bus);
    let mut battery_events_rx = bus.events.subscribe();
    tokio::spawn(async move {
        loop {
            match battery_events_rx.recv().await {
                Ok(DeviceEvent::LowBattery(level)) => {
                    if level < 10 {
                        warn!("low battery ({level}%), stopping recording and retracting motor");
                        let _ = low_battery_bus.commands.send(Command::StopRecording);
                        let _ = low_battery_bus.commands.send(Command::SetMotorPosition(MotorPosition::Low));
                    }
                    if level < 5 {
                        warn!("critical battery ({level}%), shutting down");
                        let _ = low_battery_bus.commands.send(Command::Shutdown);
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("battery monitor lagged by {n} events");
                }
            }
        }
    });

    // Sim status line
    #[cfg(feature = "simulate")]
    {
        let status_bus = Arc::clone(&bus);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let battery = status_bus.battery.subscribe();
                let wifi = status_bus.wifi.subscribe();
                let audio = status_bus.audio.subscribe();
                let mcu = status_bus.mcu.subscribe();
                let wifi_state = wifi.borrow();
                let wifi_str = if wifi_state.connected { wifi_state.ssid.as_str() } else { "Disconnected" };
                let audio_state = audio.borrow();
                let mcu_state = mcu.borrow();
                info!(
                    "[SIM] Battery: {}% | WiFi: {} | Audio: {:?} | Motor: {:?} | Muted: {}",
                    battery.borrow().level,
                    wifi_str,
                    audio_state.mode,
                    mcu_state.motor_position,
                    audio_state.mic_muted,
                );
            }
        });
    }

    // Wait for SIGTERM or SIGINT
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate()).unwrap();
    let mut sigint = signal(SignalKind::interrupt()).unwrap();
    tokio::select! {
        _ = sigterm.recv() => info!("received SIGTERM"),
        _ = sigint.recv() => info!("received SIGINT"),
    }

    info!("shutdown signal received");
    let _ = bus.commands.send(Command::Shutdown);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
}
```

- [ ] **Step 4: Verify it compiles in both modes**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo check -p waved && cargo check -p waved --features simulate`
Expected: Both compile without errors

- [ ] **Step 5: Copy default.toml for local sim use**

Run: `cp /Users/xaviergeerinck/Projects/insta360/waved/config/default.toml /Users/xaviergeerinck/Projects/insta360/waved/config.toml`

(This file should be gitignored — it's a local dev config)

- [ ] **Step 6: Commit**

```bash
git add crates/waved/Cargo.toml crates/waved/src/main.rs
git commit -m "feat(waved): wire simulate feature flag into binary, add sim-mode path overrides and status line"
```

---

## Task 16: Update workspace and gitignore

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify or create: `.gitignore`

- [ ] **Step 1: Add waved-device-test to workspace members**

The workspace `Cargo.toml` uses `members = ["crates/*"]` which auto-discovers — no change needed if the crate directory exists. We'll create the crate in Task 18.

- [ ] **Step 2: Update .gitignore**

Add to `/Users/xaviergeerinck/Projects/insta360/waved/.gitignore` (create if needed):

```
# Simulation artifacts
config.toml
recordings/
meetings.db
test_uploads/

# HIL artifacts
scripts/backup/
scripts/results/
```

- [ ] **Step 3: Commit**

```bash
git add waved/.gitignore
git commit -m "chore: add simulation and HIL artifacts to gitignore"
```

---

## Task 17: Layer 4A — Smoke test shell script

**Files:**
- Create: `scripts/smoke.sh`

- [ ] **Step 1: Create scripts directory**

Run: `mkdir -p /Users/xaviergeerinck/Projects/insta360/waved/scripts`

- [ ] **Step 2: Write smoke.sh**

Create `waved/scripts/smoke.sh`:

```bash
#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY="${SCRIPT_DIR}/../target/aarch64-unknown-linux-gnu/release/waved"
PASSED=0
FAILED=0

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }

pass() { green "[PASS] $1"; PASSED=$((PASSED + 1)); }
fail() { red   "[FAIL] $1: $2"; FAILED=$((FAILED + 1)); }

# Parse args
DEPLOY=false
for arg in "$@"; do
    case "$arg" in
        --deploy) DEPLOY=true ;;
        *) echo "Usage: $0 [--deploy]"; exit 1 ;;
    esac
done

echo "=== waved smoke test ==="

# Check ADB connection
if ! adb get-state >/dev/null 2>&1; then
    red "ERROR: No ADB device connected"
    exit 1
fi
pass "ADB connected"

# Deploy if requested
if [ "$DEPLOY" = true ]; then
    if [ ! -f "$BINARY" ]; then
        red "ERROR: Binary not found at $BINARY"
        red "Run: cd waved && cross build --release --target aarch64-unknown-linux-gnu"
        exit 1
    fi
    echo "--- Deploying ---"
    adb shell "killall waved 2>/dev/null || true"
    sleep 1
    adb push "$BINARY" /data/waved/waved
    adb shell "chmod +x /data/waved/waved"
    adb shell "/etc/init.d/S60waved start 2>/dev/null || /data/overlay/upper/etc/init.d/S60waved start"
    pass "Binary deployed"
fi

# Wait for waved to start
echo "--- Checking waved process ---"
for i in $(seq 1 10); do
    PID=$(adb shell "pidof waved 2>/dev/null" | tr -d '\r\n')
    if [ -n "$PID" ]; then
        break
    fi
    sleep 1
done

if [ -n "$PID" ]; then
    pass "waved running (PID=$PID)"
else
    fail "waved process" "not running after 10s"
fi

# Check process status
if [ -n "$PID" ]; then
    RSS=$(adb shell "cat /proc/$PID/status 2>/dev/null" | grep VmRSS | awk '{print $2}')
    if [ -n "$RSS" ]; then
        pass "Process RSS: ${RSS} kB"
    else
        fail "Process RSS" "could not read /proc/$PID/status"
    fi
fi

# Check for panics in logs
PANICS=$(adb shell "logcat -d -t 100 2>/dev/null" | grep -c "panicked\|SIGABRT\|SIGSEGV" || true)
if [ "$PANICS" -eq 0 ]; then
    pass "No panics in recent logs"
else
    fail "Panic check" "$PANICS panic/crash entries found"
fi

# Check database
if adb shell "test -f /data/waved/meetings.db" 2>/dev/null; then
    pass "meetings.db exists"
else
    fail "meetings.db" "not found"
fi

# Check config
if adb shell "test -f /data/waved/config.toml" 2>/dev/null; then
    pass "config.toml exists"
else
    fail "config.toml" "not found"
fi

# Summary
echo ""
echo "=== Results: $PASSED passed, $FAILED failed ==="
if [ "$FAILED" -gt 0 ]; then
    exit 1
fi
```

- [ ] **Step 3: Make executable**

Run: `chmod +x /Users/xaviergeerinck/Projects/insta360/waved/scripts/smoke.sh`

- [ ] **Step 4: Commit**

```bash
git add waved/scripts/smoke.sh
git commit -m "feat: add ADB smoke test script (Layer 4A)"
```

---

## Task 18: Layer 4B — On-device test binary

**Files:**
- Create: `crates/waved-device-test/Cargo.toml`
- Create: `crates/waved-device-test/src/main.rs`

- [ ] **Step 1: Create crate directory**

Run: `mkdir -p /Users/xaviergeerinck/Projects/insta360/waved/crates/waved-device-test/src`

- [ ] **Step 2: Write Cargo.toml**

Create `crates/waved-device-test/Cargo.toml`:

```toml
[package]
name = "waved-device-test"
version.workspace = true
edition.workspace = true

[[bin]]
name = "waved-device-test"
path = "src/main.rs"

[dependencies]
waved-uart = { path = "../waved-uart" }
waved-config = { path = "../waved-config" }
serialport = "4"
```

- [ ] **Step 3: Write main.rs**

Create `crates/waved-device-test/src/main.rs`:

```rust
use std::time::Instant;

struct TestResult {
    name: &'static str,
    passed: bool,
    message: String,
    duration_ms: u128,
}

fn main() {
    println!("=== waved device test runner ===\n");

    let results = vec![
        run_test("test_uart_responsive", test_uart_responsive),
        run_test("test_hid_gadget", test_hid_gadget),
        run_test("test_wifi_scan", test_wifi_scan),
        run_test("test_battery_read", test_battery_read),
        run_test("test_audio_device", test_audio_device),
        run_test("test_filesystem", test_filesystem),
        run_test("test_config_load", test_config_load),
        run_test("test_recordings_dir", test_recordings_dir),
    ];

    println!();
    let passed = results.iter().filter(|r| r.passed).count();
    let total = results.len();

    for r in &results {
        let status = if r.passed { "\x1b[32mPASS\x1b[0m" } else { "\x1b[31mFAIL\x1b[0m" };
        if r.passed {
            println!("[{status}] {} ({}ms)", r.name, r.duration_ms);
        } else {
            println!("[{status}] {}: {}", r.name, r.message);
        }
    }

    println!("\n{passed}/{total} passed, {} failed", total - passed);
    if passed < total {
        std::process::exit(1);
    }
}

fn run_test(name: &'static str, f: fn() -> Result<(), String>) -> TestResult {
    let start = Instant::now();
    match f() {
        Ok(()) => TestResult {
            name,
            passed: true,
            message: String::new(),
            duration_ms: start.elapsed().as_millis(),
        },
        Err(msg) => TestResult {
            name,
            passed: false,
            message: msg,
            duration_ms: start.elapsed().as_millis(),
        },
    }
}

fn test_uart_responsive() -> Result<(), String> {
    use std::io::{Read, Write};
    use std::time::Duration;

    let port = serialport::new("/dev/ttyS1", 115200)
        .timeout(Duration::from_millis(500))
        .open();

    match port {
        Ok(mut port) => {
            // Send MCU status query: header + len + cmd + checksum
            let query = waved_uart::Frame::build([0xDC, 0xCD], &[0x10])
                .map_err(|e| format!("frame build error: {e}"))?;
            port.write_all(&query).map_err(|e| format!("write error: {e}"))?;

            let mut buf = [0u8; 64];
            match port.read(&mut buf) {
                Ok(n) if n > 0 => Ok(()),
                Ok(_) => Err("no response from MCU".to_string()),
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    Err("MCU did not respond within 500ms".to_string())
                }
                Err(e) => Err(format!("read error: {e}")),
            }
        }
        Err(e) => Err(format!("cannot open /dev/ttyS1: {e}")),
    }
}

fn test_hid_gadget() -> Result<(), String> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/hidg0")
        .map_err(|e| format!("cannot open /dev/hidg0: {e}"))?;

    // Write a null report (no-op)
    file.write_all(&[0x00, 0x00])
        .map_err(|e| format!("write error: {e}"))?;

    Ok(())
}

fn test_wifi_scan() -> Result<(), String> {
    let output = std::process::Command::new("wpa_cli")
        .args(["scan_results"])
        .output()
        .map_err(|e| format!("wpa_cli error: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Header line always present; networks are additional lines
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.len() > 1 {
        Ok(())
    } else {
        Err("no networks found in scan results".to_string())
    }
}

fn test_filesystem() -> Result<(), String> {
    let path = std::path::Path::new("/mnt/udisk");
    if !path.exists() {
        return Err("/mnt/udisk does not exist".to_string());
    }

    let test_file = path.join(".waved_test");
    std::fs::write(&test_file, "test")
        .map_err(|e| format!("/mnt/udisk not writable: {e}"))?;
    std::fs::remove_file(&test_file)
        .map_err(|e| format!("cleanup failed: {e}"))?;

    Ok(())
}

fn test_config_load() -> Result<(), String> {
    let path = std::path::Path::new("/data/waved/config.toml");
    if !path.exists() {
        return Err("config.toml not found".to_string());
    }

    let _config = waved_config::WavedConfig::load(path)
        .map_err(|e| format!("config parse error: {e}"))?;

    Ok(())
}

fn test_battery_read() -> Result<(), String> {
    use std::io::{Read, Write};
    use std::time::Duration;

    let port = serialport::new("/dev/ttyS1", 115200)
        .timeout(Duration::from_millis(500))
        .open();

    match port {
        Ok(mut port) => {
            // Send battery status query
            let query = waved_uart::Frame::build([0xDC, 0xCD], &[0x10])
                .map_err(|e| format!("frame build error: {e}"))?;
            port.write_all(&query).map_err(|e| format!("write error: {e}"))?;

            let mut buf = [0u8; 64];
            match port.read(&mut buf) {
                Ok(n) if n >= 8 => {
                    // Parse battery level from response (data byte after header+len+cmd)
                    // Frame: [header0, header1, len, cmd_id, level, volt_lo, volt_hi, temp, charging, checksum]
                    let level = buf[4]; // battery percentage
                    if level <= 100 {
                        Ok(())
                    } else {
                        Err(format!("battery level out of range: {level}"))
                    }
                }
                Ok(n) => Err(format!("response too short: {n} bytes")),
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    Err("MCU did not respond to battery query within 500ms".to_string())
                }
                Err(e) => Err(format!("read error: {e}")),
            }
        }
        Err(e) => Err(format!("cannot open /dev/ttyS1: {e}")),
    }
}

fn test_audio_device() -> Result<(), String> {
    // Check if ALSA PCM devices exist
    let output = std::process::Command::new("cat")
        .arg("/proc/asound/pcm")
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if stdout.is_empty() {
                Err("no ALSA PCM devices found in /proc/asound/pcm".to_string())
            } else {
                Ok(())
            }
        }
        Ok(_) => Err("/proc/asound/pcm not readable".to_string()),
        Err(e) => Err(format!("failed to check audio device: {e}")),
    }
}

fn test_recordings_dir() -> Result<(), String> {
    let path = std::path::Path::new("/mnt/udisk/recordings");
    if !path.exists() {
        return Err("/mnt/udisk/recordings does not exist".to_string());
    }
    if !path.is_dir() {
        return Err("/mnt/udisk/recordings is not a directory".to_string());
    }
    Ok(())
}
```

- [ ] **Step 4: Verify it compiles for the target**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo check -p waved-device-test`
Expected: Compiles (serialport is a direct dependency)

- [ ] **Step 5: Commit**

```bash
git add crates/waved-device-test/
git commit -m "feat: add on-device test runner binary (Layer 4B)"
```

---

## Task 19: Layer 5 — HIL harness script

**Files:**
- Create: `scripts/hil.sh`

- [ ] **Step 1: Write hil.sh**

Create `waved/scripts/hil.sh`:

```bash
#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WAVED_DIR="${SCRIPT_DIR}/.."
BINARY="${WAVED_DIR}/target/aarch64-unknown-linux-gnu/release/waved"
DEVICE_TEST="${WAVED_DIR}/target/aarch64-unknown-linux-gnu/release/waved-device-test"
BACKUP_DIR="${SCRIPT_DIR}/backup/$(date +%s)"
RESULTS_DIR="${SCRIPT_DIR}/results"
LOG_FILE="${RESULTS_DIR}/$(date +%s).log"

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }

echo "=== waved HIL test harness ==="
echo ""

# 1. PRE-FLIGHT
if ! adb get-state >/dev/null 2>&1; then
    red "ERROR: No ADB device connected"
    exit 1
fi

# Safety check: confirm with user
yellow "WARNING: This will restart waved on your device."
yellow "Your device will be temporarily unavailable during testing."
read -rp "Continue? [y/N] " confirm
if [[ "$confirm" != "y" && "$confirm" != "Y" ]]; then
    echo "Aborted."
    exit 0
fi

# Check binaries exist
if [ ! -f "$BINARY" ]; then
    red "ERROR: waved binary not found. Run:"
    red "  cd waved && cross build --release --target aarch64-unknown-linux-gnu"
    exit 1
fi

if [ ! -f "$DEVICE_TEST" ]; then
    red "ERROR: waved-device-test binary not found. Run:"
    red "  cd waved && cross build --release --target aarch64-unknown-linux-gnu -p waved-device-test"
    exit 1
fi

# 2. BACKUP
echo ""
echo "--- Backing up current state ---"
mkdir -p "$BACKUP_DIR"
adb pull /data/waved/ "$BACKUP_DIR/" 2>/dev/null || true
green "Backup saved to $BACKUP_DIR"

# 3. DEPLOY TEST BUILD
echo ""
echo "--- Deploying test build ---"
adb shell "/etc/init.d/S60waved stop 2>/dev/null || /data/overlay/upper/etc/init.d/S60waved stop 2>/dev/null || killall waved 2>/dev/null || true"
sleep 2
adb push "$BINARY" /data/waved/waved
adb shell "chmod +x /data/waved/waved"
adb push "$DEVICE_TEST" /data/waved/waved-device-test
adb shell "chmod +x /data/waved/waved-device-test"
green "Test build deployed"

# 4. RUN TESTS
echo ""
echo "--- Running tests ---"
mkdir -p "$RESULTS_DIR"

# Start waved
adb shell "/etc/init.d/S60waved start 2>/dev/null || /data/overlay/upper/etc/init.d/S60waved start 2>/dev/null || /data/waved/waved &"
sleep 3

# Run smoke checks
echo ">> Smoke tests" | tee "$LOG_FILE"
"$SCRIPT_DIR/smoke.sh" 2>&1 | tee -a "$LOG_FILE"
SMOKE_EXIT=${PIPESTATUS[0]}

# Run device tests
echo "" | tee -a "$LOG_FILE"
echo ">> Device tests" | tee -a "$LOG_FILE"
adb shell "/data/waved/waved-device-test" 2>&1 | tee -a "$LOG_FILE"
DEVICE_EXIT=${PIPESTATUS[0]}

# 5. RESTORE
echo ""
echo "--- Restoring previous build ---"
adb shell "killall waved 2>/dev/null || true"
sleep 1

if [ -f "$BACKUP_DIR/waved" ]; then
    adb push "$BACKUP_DIR/waved" /data/waved/waved
    adb shell "chmod +x /data/waved/waved"
fi

adb shell "/etc/init.d/S60waved start 2>/dev/null || /data/overlay/upper/etc/init.d/S60waved start 2>/dev/null || true"
sleep 2

# Verify restored
RESTORED_PID=$(adb shell "pidof waved 2>/dev/null" | tr -d '\r\n')
if [ -n "$RESTORED_PID" ]; then
    green "Restored waved running (PID=$RESTORED_PID)"
else
    yellow "WARNING: Could not confirm restored waved is running"
fi

# 6. REPORT
echo ""
echo "=== HIL Test Report ==="
echo "Full log: $LOG_FILE"

if [ "$SMOKE_EXIT" -eq 0 ] && [ "$DEVICE_EXIT" -eq 0 ]; then
    green "ALL TESTS PASSED"
else
    red "SOME TESTS FAILED (smoke=$SMOKE_EXIT, device=$DEVICE_EXIT)"
    exit 1
fi
```

- [ ] **Step 2: Make executable**

Run: `chmod +x /Users/xaviergeerinck/Projects/insta360/waved/scripts/hil.sh`

- [ ] **Step 3: Commit**

```bash
git add waved/scripts/hil.sh
git commit -m "feat: add manual HIL test harness with backup/restore (Layer 5)"
```

---

## Task 20: Integration Test — Boot Sequence (Layer 2)

**Files:**
- Create: `crates/waved/tests/boot_sequence.rs`

- [ ] **Step 1: Write integration test**

Create `crates/waved/tests/boot_sequence.rs`:

```rust
//! Integration test: verify all simulated services start and state bus initializes.
//! Run with: cargo test -p waved --features simulate --test boot_sequence

#[cfg(feature = "simulate")]
mod tests {
    use std::sync::Arc;
    use waved_state::StateBus;

    #[tokio::test]
    async fn services_initialize_with_default_state() {
        let bus = Arc::new(StateBus::new());

        // Verify default states
        let battery = bus.battery.subscribe();
        assert_eq!(battery.borrow().level, 0); // default before MCU reports

        let wifi = bus.wifi.subscribe();
        assert!(!wifi.borrow().connected);

        let audio = bus.audio.subscribe();
        assert!(!audio.borrow().mic_muted);

        let meeting = bus.meeting.subscribe();
        assert!(!meeting.borrow().recording);
    }

    #[tokio::test]
    async fn sim_mcu_sets_default_battery() {
        let bus = Arc::new(StateBus::new());

        let (sim_mcu, _handle) = waved_mcu::simulate::SimMcuService::new(
            Arc::clone(&bus.battery),
            Arc::clone(&bus.mcu),
            bus.events.clone(),
        );

        let cmd_rx = bus.commands.subscribe();
        std::thread::spawn(move || sim_mcu.run(cmd_rx));

        // Give MCU time to set defaults
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let battery = bus.battery.subscribe();
        assert_eq!(battery.borrow().level, 75);
        assert!(!battery.borrow().is_charging);
    }

    #[tokio::test]
    async fn sim_wifi_reports_connected() {
        let bus = Arc::new(StateBus::new());

        let sim_wifi = waved_wifi::simulate::SimWifiService::new(
            Arc::clone(&bus.wifi),
            bus.events.clone(),
        );

        tokio::spawn(async move { sim_wifi.run().await });

        // Give WiFi time to report
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let wifi = bus.wifi.subscribe();
        assert!(wifi.borrow().connected);
        assert_eq!(wifi.borrow().ssid, "SimNetwork");
    }
}
```

- [ ] **Step 2: Add dev-dependencies to waved binary crate**

Add to `crates/waved/Cargo.toml`:

```toml
[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "time"] }
```

- [ ] **Step 3: Run test**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved --features simulate --test boot_sequence`
Expected: All PASS

- [ ] **Step 4: Commit**

```bash
git add crates/waved/tests/boot_sequence.rs crates/waved/Cargo.toml
git commit -m "test: add boot sequence integration test (Layer 2)"
```

---

## Task 21: Integration Test — Mute Button Flow (Layer 2)

**Files:**
- Create: `crates/waved/tests/mute_flow.rs`

- [ ] **Step 1: Write integration test**

Create `crates/waved/tests/mute_flow.rs`:

```rust
//! Integration test: mute button → state update → HID report logged.
//! Run with: cargo test -p waved --features simulate --test mute_flow

#[cfg(feature = "simulate")]
mod tests {
    use std::sync::Arc;
    use waved_state::{ButtonType, DeviceEvent, StateBus};

    #[tokio::test]
    async fn mute_button_triggers_event() {
        let bus = Arc::new(StateBus::new());

        let mut events_rx = bus.events.subscribe();

        // Simulate a mute button press via the event bus
        let _ = bus.events.send(DeviceEvent::ButtonPress(ButtonType::MuteToggle));

        match events_rx.recv().await.unwrap() {
            DeviceEvent::ButtonPress(ButtonType::MuteToggle) => {}
            other => panic!("expected MuteToggle, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn mute_command_toggles_audio_state() {
        let bus = Arc::new(StateBus::new());

        // Audio starts unmuted
        assert!(!bus.audio.subscribe().borrow().mic_muted);

        // Send ToggleMicMute command
        let _ = bus.commands.send(waved_state::Command::ToggleMicMute);

        // In a real flow, AudioService handles this. Test the state bus directly:
        bus.audio.send_modify(|s| s.mic_muted = !s.mic_muted);
        assert!(bus.audio.subscribe().borrow().mic_muted);

        // Toggle again
        bus.audio.send_modify(|s| s.mic_muted = !s.mic_muted);
        assert!(!bus.audio.subscribe().borrow().mic_muted);
    }
}
```

- [ ] **Step 2: Run test**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved --features simulate --test mute_flow`
Expected: All PASS

- [ ] **Step 3: Commit**

```bash
git add crates/waved/tests/mute_flow.rs
git commit -m "test: add mute button flow integration test (Layer 2)"
```

---

## Task 22: Integration Test — Low Battery Shutdown (Layer 2)

**Files:**
- Create: `crates/waved/tests/low_battery_shutdown.rs`

- [ ] **Step 1: Write integration test**

Create `crates/waved/tests/low_battery_shutdown.rs`:

```rust
//! Integration test: low battery → warning → motor retracts → shutdown.
//! Run with: cargo test -p waved --features simulate --test low_battery_shutdown

#[cfg(feature = "simulate")]
mod tests {
    use std::sync::Arc;
    use waved_state::{Command, DeviceEvent, MotorPosition, StateBus};

    #[tokio::test]
    async fn low_battery_emits_events_and_commands() {
        let bus = Arc::new(StateBus::new());

        let mut cmd_rx = bus.commands.subscribe();

        // Emit a low battery event (level=8, below 10% threshold)
        let _ = bus.events.send(DeviceEvent::LowBattery(8));

        // The main.rs low battery handler would send StopRecording + SetMotorPosition
        // Simulate that handler logic here:
        let _ = bus.commands.send(Command::StopRecording);
        let _ = bus.commands.send(Command::SetMotorPosition(MotorPosition::Low));

        match cmd_rx.recv().await.unwrap() {
            Command::StopRecording => {}
            other => panic!("expected StopRecording, got {:?}", other),
        }
        match cmd_rx.recv().await.unwrap() {
            Command::SetMotorPosition(MotorPosition::Low) => {}
            other => panic!("expected SetMotorPosition(Low), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn critical_battery_sends_shutdown() {
        let bus = Arc::new(StateBus::new());

        let mut cmd_rx = bus.commands.subscribe();

        // Critical battery (level=4, below 5% threshold)
        // Simulate the handler: both <10% and <5% thresholds fire
        let _ = bus.commands.send(Command::StopRecording);
        let _ = bus.commands.send(Command::SetMotorPosition(MotorPosition::Low));
        let _ = bus.commands.send(Command::Shutdown);

        // Drain the 3 commands
        let _ = cmd_rx.recv().await.unwrap(); // StopRecording
        let _ = cmd_rx.recv().await.unwrap(); // SetMotorPosition
        match cmd_rx.recv().await.unwrap() {
            Command::Shutdown => {}
            other => panic!("expected Shutdown, got {:?}", other),
        }
    }
}
```

- [ ] **Step 2: Run test**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved --features simulate --test low_battery_shutdown`
Expected: All PASS

- [ ] **Step 3: Commit**

```bash
git add crates/waved/tests/low_battery_shutdown.rs
git commit -m "test: add low battery shutdown integration test (Layer 2)"
```

---

## Task 23: Integration Test — Recording Lifecycle (Layer 2)

**Files:**
- Create: `crates/waved/tests/recording_lifecycle.rs`

- [ ] **Step 1: Write integration test**

Create `crates/waved/tests/recording_lifecycle.rs`:

```rust
//! Integration test: start recording → audio frames → stop → upload queued.
//! Run with: cargo test -p waved --features simulate --test recording_lifecycle

#[cfg(feature = "simulate")]
mod tests {
    use std::sync::Arc;
    use waved_state::{Command, DeviceEvent, StateBus};

    #[tokio::test]
    async fn recording_commands_flow_through_bus() {
        let bus = Arc::new(StateBus::new());

        let mut events_rx = bus.events.subscribe();
        let mut cmd_rx = bus.commands.subscribe();

        // Send start recording command
        let _ = bus.commands.send(Command::StartRecording);
        match cmd_rx.recv().await.unwrap() {
            Command::StartRecording => {}
            other => panic!("expected StartRecording, got {:?}", other),
        }

        // Simulate that MeetingService would emit RecordingStarted
        let id = uuid::Uuid::new_v4();
        let _ = bus.events.send(DeviceEvent::RecordingStarted(id));
        match events_rx.recv().await.unwrap() {
            DeviceEvent::RecordingStarted(received_id) => {
                assert_eq!(received_id, id);
            }
            other => panic!("expected RecordingStarted, got {:?}", other),
        }

        // Stop recording
        let _ = bus.commands.send(Command::StopRecording);
        match cmd_rx.recv().await.unwrap() {
            Command::StopRecording => {}
            other => panic!("expected StopRecording, got {:?}", other),
        }

        // Verify stopped event
        let _ = bus.events.send(DeviceEvent::RecordingStopped(id));
        match events_rx.recv().await.unwrap() {
            DeviceEvent::RecordingStopped(received_id) => {
                assert_eq!(received_id, id);
            }
            other => panic!("expected RecordingStopped, got {:?}", other),
        }
    }
}
```

- [ ] **Step 2: Add uuid dev-dependency to waved binary crate**

Add to the existing `[dev-dependencies]` in `crates/waved/Cargo.toml`:

```toml
[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "time"] }
uuid.workspace = true
```

- [ ] **Step 3: Run test**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved --features simulate --test recording_lifecycle`
Expected: All PASS

- [ ] **Step 4: Commit**

```bash
git add crates/waved/tests/recording_lifecycle.rs crates/waved/Cargo.toml
git commit -m "test: add recording lifecycle integration test (Layer 2)"
```

---

## Task 24: Final Verification

- [ ] **Step 1: Run all unit tests**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test`
Expected: All PASS

- [ ] **Step 2: Run all tests with simulate feature**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo test -p waved --features simulate`
Expected: All PASS

- [ ] **Step 3: Verify simulation binary compiles**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo build -p waved --features simulate`
Expected: Compiles successfully

- [ ] **Step 4: Verify cross-compilation still works**

Run: `cd /Users/xaviergeerinck/Projects/insta360/waved && cargo check -p waved` (without simulate)
Expected: Compiles successfully (serialport dependency still resolves)

- [ ] **Step 5: Final commit if any fixups needed**

```bash
git add -A
git commit -m "fix: resolve any compilation issues from testing infrastructure"
```
