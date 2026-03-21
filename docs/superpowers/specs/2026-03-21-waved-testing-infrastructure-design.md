# waved Testing Infrastructure Design

**Date:** 2026-03-21
**Status:** Approved
**Scope:** All five testing layers for the waved firmware replacement

---

## Overview

A five-layer testing pyramid for waved that enables rapid local development, safe on-device validation, and hardware-in-the-loop regression testing — all without bricking Xavier's daily-driver Wave device.

## Goals

1. **Fast local feedback** — `cargo test` catches logic bugs in seconds on macOS
2. **End-to-end simulation** — full binary runs on Mac with simulated hardware
3. **Safe device testing** — on-device tests with backup/restore safety protocol
4. **No surprise reboots** — HIL is manual-trigger only, never unattended

## Non-Goals

- Automated CI with a dedicated device (no second device available)
- GUI/visual testing (Phase 2 LVGL UI is future work)
- Performance benchmarking (premature at Phase 1 MVP stage)

---

## Layer 1: Unit Tests

Add `#[cfg(test)] mod tests` to each crate. These test pure logic with no hardware dependencies and run with `cargo test` on macOS.

### Per-Crate Test Coverage

#### waved-uart
- Frame parsing from known byte sequences (valid MCU frame, valid BT frame)
- Checksum calculation (correct, off-by-one, zero-length)
- Header-based dispatch (MCU headers → MCU channel, BT headers → BT channel)
- Malformed frames: truncated, wrong checksum, unknown header
- Frame building: construct a frame and verify bytes match expected output

#### waved-mcu
- Battery state transitions: charging → full, discharging → low → critical
- Low-battery thresholds: warnings at <10% and <5%
- Motor position validation: Low/Medium/High, reject out-of-range
- Button event parsing: MuteToggle (0x40), VolumeUp (0x41), VolumeDown (0x42), MuteLongPress (0x43), unknown byte returns None

#### waved-bt
- Command byte builders: get_status, get_mac, get_name, get_version, open, close, disconnect, set_auto_reconnect
- Status response parsing: connected device name, MAC, version
- Malformed response handling

#### waved-config
- Parse valid TOML with all fields
- Parse TOML with missing optional fields (defaults applied)
- Reject invalid TOML (missing required fields, wrong types)
- Default config matches `config/default.toml`

#### waved-meeting
- Metadata JSON serialization round-trip
- Retry backoff calculation (linear: backoff_ms * attempt, max retries from config)
- Queue ordering (oldest first)
- UUID directory naming

#### waved-state
- Watch channel: subscriber sees latest value
- Broadcast channel: multiple subscribers receive events
- State transitions: valid and invalid

#### waved-hid
- HID report byte construction for mute toggle
- HID report byte construction for volume up/down

#### waved-wifi
- WPA supplicant config file generation from SSID/password
- NTP server list (pool.ntp.org, time.google.com, no Chinese servers)

### Running

```bash
cd waved
cargo test
```

---

## Layer 2: Integration Tests with Feature Flags

### Mechanism

A Cargo feature `simulate` on each hardware crate. When enabled, `#[cfg(feature = "simulate")]` swaps the hardware module implementation for an in-memory mock. No traits, no generics — two separate code paths selected at compile time.

The `waved` binary crate (`crates/waved/Cargo.toml` — not the workspace root) aggregates the feature:

```toml
[features]
simulate = [
    "waved-uart/simulate",
    "waved-bt/simulate",
    "waved-hid/simulate",
    "waved-wifi/simulate",
    "waved-audio/simulate",
    "waved-mcu/simulate",
    "waved-meeting/simulate",
]
```

### Simulated Implementations

| Crate | Simulated Behavior |
|-------|-------------------|
| `waved-uart` | In-memory crossbeam channels. A `SimUart` struct holds scripted frame sequences that are returned on read. Writes are captured for assertion. |
| `waved-hid` | All HID report writes go to `tracing::info!` instead of `/dev/hidg*`. |
| `waved-wifi` | `status()` always returns `Connected`. `connect()` is a no-op success. NTP sync returns current system time. |
| `waved-audio` | Generates silence frames (zeroed `[i16]`, 960 samples per 20ms frame at 48kHz, 16-bit, 2-channel stereo) at real-time rate. No TEA SDK dependency. |
| `waved-mcu` | Battery level configurable (default 75%), charging state toggleable. Motor commands logged. Button events injectable via a channel. |
| `waved-bt` | Scripted BT status responses via crossbeam channel. Connection events injectable. Commands captured for assertion. |
| `waved-meeting` | HTTP upload replaced with local file write to `./test_uploads/`. SQLite DB created in `/tmp/`. |

### Integration Test Scenarios

Located in `waved/crates/waved/tests/` (Rust integration test directory for the binary crate):

1. **Boot sequence** — waved starts, all services initialize, state bus reports `Ready`
2. **Mute button flow** — inject mute button event → state updates to muted → HID report logged → audio callback receives mute state
3. **Recording lifecycle** — start meeting → audio frames generated → recording file created → stop meeting → upload queued
4. **Low battery shutdown** — set battery to 4% → warning event emitted → motor retracts → shutdown initiated
5. **Config reload** — modify config file → waved picks up changes (when hot-reload is implemented)
6. **WiFi → upload** — WiFi connects → queued recordings upload → upload directory populated

### Running

```bash
cd waved
cargo test -p waved --features simulate
```

---

## Layer 3: Simulation Binary

The same `waved` binary compiled with `--features simulate` runs on macOS. No cross-compilation needed.

### Behavior Differences in Simulation Mode

- **Config path:** `./config.toml` (current directory) instead of `/data/waved/config.toml`
- **Recording path:** `./recordings/` instead of `/mnt/udisk/recordings/`
- **Database path:** `./meetings.db` instead of `/data/waved/meetings.db`
- **Logging:** `tracing` output to stdout with `RUST_LOG` filtering
- **Audio:** Silence frames generated at real-time rate (20ms intervals)
- **Console output:** Periodic status line showing simulated state:
  ```
  [SIM] Battery: 75% | WiFi: Connected | Audio: Silence | Motor: Low | Muted: false
  ```

### Running

```bash
cd waved
cargo run -p waved --features simulate
```

Or with custom config:

```bash
RUST_LOG=debug cargo run -p waved --features simulate -- --config ./my-test-config.toml
```

### Use Cases

- Smoke test the full boot sequence without a device
- Test HTTP upload pipeline against a local server
- Demo the state machine to collaborators
- Debug meeting recording flow with breakpoints in IDE

---

## Layer 4: On-Device Tests

### 4A: Shell Smoke Script

**Location:** `waved/scripts/smoke.sh`

A bash script that runs from macOS over ADB. Designed for the fast feedback loop after `cross build`.

```
Usage: ./waved/scripts/smoke.sh [--deploy]

  --deploy    Push the latest release build before testing
  (default)   Test against whatever is currently on the device
```

**Test sequence:**

1. `adb shell pidof waved` — check if already running
2. If `--deploy`: push binary, restart via init script
3. Wait for startup (poll PID, max 10 seconds)
4. `adb shell cat /proc/$(pidof waved)/status` — verify running, check RSS
5. `adb shell logcat -d | grep waved` — check for panic/SIGABRT
6. `adb shell ls /data/waved/meetings.db` — verify DB created
7. `adb shell ls /data/waved/config.toml` — verify config present
8. Report pass/fail summary

**Expected runtime:** < 30 seconds including deploy.

### 4B: Rust Device Test Binary

**Location:** `waved/crates/waved-device-test/`

A separate binary crate that runs on the Wave device. Not part of the main `waved` binary — it's a standalone test runner.

**Tests:**

| Test | What it validates |
|------|-------------------|
| `test_uart_loopback` | Open `/dev/ttyS1`, send a status query frame, expect MCU response within 500ms |
| `test_hid_gadget` | Open `/dev/hidg0`, write a mute report, verify no error |
| `test_wifi_scan` | Run `wpa_cli scan_results`, expect at least one network |
| `test_battery_read` | Query MCU for battery level, verify 0-100% range |
| `test_filesystem` | Verify `/mnt/udisk/` is mounted and writable |
| `test_audio_device` | Verify ALSA device exists (if applicable to this platform) |
| `test_config_load` | Load `/data/waved/config.toml`, verify all sections parse |

**Running:**

```bash
cross build --release --target aarch64-unknown-linux-gnu -p waved-device-test
adb push target/aarch64-unknown-linux-gnu/release/waved-device-test /data/waved/
adb shell /data/waved/waved-device-test
```

**Output format:**

```
[PASS] test_uart_loopback (23ms)
[PASS] test_hid_gadget (5ms)
[PASS] test_wifi_scan (1204ms)
[PASS] test_battery_read (45ms)
[PASS] test_filesystem (2ms)
[FAIL] test_audio_device: ALSA device not found
[PASS] test_config_load (8ms)

6/7 passed, 1 failed
```

---

## Layer 5: Hardware-in-the-Loop (HIL) Harness

### Safety Protocol

This is Xavier's daily-driver device. The HIL harness is **manual-trigger only** and includes a full backup/restore cycle.

**Location:** `waved/scripts/hil.sh`

### Execution Flow

```
1. PRE-FLIGHT
   - Verify ADB connection
   - Verify device is not in a meeting (check waved state)
   - Ask user confirmation: "This will restart waved on your device. Continue? [y/N]"

2. BACKUP
   - adb pull /data/waved/ → ./scripts/backup/$(date +%s)/
   - Record current waved PID and uptime

3. DEPLOY TEST BUILD
   - adb shell /etc/init.d/S60waved stop  (graceful shutdown)
   - adb push release binary → /data/waved/waved
   - adb push waved-device-test → /data/waved/waved-device-test

4. RUN TESTS
   - Start waved, wait for PID
   - Run smoke checks (from smoke.sh logic)
   - Run waved-device-test binary
   - Capture all output to ./scripts/results/$(date +%s).log

5. RESTORE
   - adb shell /etc/init.d/S60waved stop
   - adb push backup binary → /data/waved/waved
   - adb shell /etc/init.d/S60waved start
   - Verify restored waved is running

6. REPORT
   - Print pass/fail summary
   - Print path to full log
```

### Running

```bash
./waved/scripts/hil.sh
```

No flags, no automation, no CI integration. Just a script you run when you want to validate a build on real hardware.

### Future: Second Device

When a dedicated test device becomes available:
- Point `hil.sh` at a specific ADB serial (`-s <serial>`)
- Wire into GitHub Actions as a self-hosted runner
- Remove the confirmation prompt
- Add to PR checks

No code changes needed — just configuration.

---

## Directory Structure

Note: `waved-config` and `waved-state` do not need `simulate.rs` files because they have no hardware dependencies — they operate on pure data structures and tokio channels.

```
waved/
├── Cargo.toml                          # Workspace manifest (unchanged)
├── crates/
│   ├── waved/
│   │   ├── Cargo.toml                  # Add simulate feature (aggregates sub-crate features)
│   │   ├── src/main.rs                 # Add --config flag for sim mode
│   │   └── tests/                      # Integration tests (Layer 2)
│   │       ├── boot_sequence.rs
│   │       ├── mute_flow.rs
│   │       ├── recording_lifecycle.rs
│   │       ├── low_battery_shutdown.rs
│   │       └── wifi_upload.rs
│   ├── waved-uart/
│   │   ├── src/lib.rs                  # Add #[cfg(test)] mod tests
│   │   └── src/simulate.rs             # #[cfg(feature = "simulate")] module
│   ├── waved-mcu/
│   │   ├── src/lib.rs                  # Add #[cfg(test)] mod tests
│   │   └── src/simulate.rs
│   ├── waved-bt/
│   │   ├── src/lib.rs                  # Add #[cfg(test)] mod tests
│   │   └── src/simulate.rs
│   ├── waved-hid/
│   │   ├── src/lib.rs                  # Add #[cfg(test)] mod tests
│   │   └── src/simulate.rs
│   ├── waved-audio/
│   │   ├── src/lib.rs                  # Add #[cfg(test)] mod tests
│   │   └── src/simulate.rs
│   ├── waved-wifi/
│   │   ├── src/lib.rs                  # Add #[cfg(test)] mod tests
│   │   └── src/simulate.rs
│   ├── waved-config/
│   │   └── src/lib.rs                  # Add #[cfg(test)] mod tests (no simulate needed)
│   ├── waved-meeting/
│   │   ├── src/lib.rs                  # Add #[cfg(test)] mod tests
│   │   └── src/simulate.rs
│   ├── waved-state/
│   │   └── src/lib.rs                  # Add #[cfg(test)] mod tests (no simulate needed)
│   └── waved-device-test/
│       ├── Cargo.toml                  # New crate
│       └── src/main.rs                 # On-device test runner
└── scripts/
    ├── smoke.sh                        # Layer 4A: ADB smoke script
    ├── hil.sh                          # Layer 5: HIL harness
    ├── backup/                         # HIL backup storage (gitignored)
    └── results/                        # HIL test results (gitignored)
```

---

## Build Commands Summary

| Purpose | Command |
|---------|---------|
| Unit tests (Layer 1) | `cargo test` |
| Integration tests (Layer 2) | `cargo test -p waved --features simulate` |
| Simulation binary (Layer 3) | `cargo run -p waved --features simulate` |
| Cross-compile for device | `cross build --release --target aarch64-unknown-linux-gnu` |
| Cross-compile device tests | `cross build --release --target aarch64-unknown-linux-gnu -p waved-device-test` |
| Deploy + smoke test (Layer 4A) | `./waved/scripts/smoke.sh --deploy` |
| Device test binary (Layer 4B) | `adb shell /data/waved/waved-device-test` |
| Full HIL run (Layer 5) | `./waved/scripts/hil.sh` |
