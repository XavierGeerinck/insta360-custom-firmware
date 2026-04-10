# waved Flash Deployment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the `waved` binary persistently on the Insta360 Wave for the first time, with pspapp suppressed, telemetry neutralized, and a one-command rollback at every step.

**Architecture:** Three sequential checkpoints operating entirely above the partition layer. C1 (foreground smoke test) proves the binary runs on real hardware. C2 (overlay install) makes it persistent via `/data/overlay/upper/`. C3 (reboot survival) validates that `S60waved` actually brings `waved` up at boot. The device screen will go blank after C2 — no LVGL UI in Phase 1, expected.

**Tech Stack:** Rust/Cargo, `cross` (Docker-based cross-compiler), ADB, BusyBox sh, overlayfs on the device.

---

## File map

All existing files — no new source files are created by this plan:

| File | Role |
|---|---|
| `waved/deploy/install.sh` | C2 deploy script — runs on the host, pushes over ADB |
| `waved/deploy/revert.sh` | Rollback script — removes overlay files |
| `waved/deploy/S60waved` | Init script installed onto the device via overlay |
| `waved/config/default.toml` | Default config pushed to `/data/waved/config.toml` |
| `target/aarch64-unknown-linux-gnu/release/waved` | The cross-compiled binary |
| `target/aarch64-unknown-linux-gnu/release/waved-device-test` | On-device hardware FFI test runner |

---

## Task 1: Environment preflight

**Files:** None modified.

- [ ] **Verify cross is installed**

```bash
cross --version
```

Expected: `cross 0.2.x` or similar. If not found or if version is `0.2.5` and you're on macOS/arm64, install from git per the README:

```bash
cargo install cross --git https://github.com/cross-rs/cross
```

- [ ] **Verify Docker is running** (required by cross)

```bash
docker info --format '{{.ServerVersion}}' 2>&1
```

Expected: a version string like `27.x.x`. If you see `Cannot connect to the Docker daemon`, start Docker Desktop and wait for it to be ready.

- [ ] **Verify ADB device is connected**

```bash
adb devices
```

Expected:

```
List of devices attached
PSP_0000000000000000	device
```

If device is offline or not listed: unplug and replug the USB-C cable, wait 5 seconds, re-run.

- [ ] **Snapshot the S89usbgadget overlay md5 as baseline**

```bash
adb shell 'md5sum /data/overlay/upper/etc/init.d/S89usbgadget'
```

Expected: `554342b3eceeda0d5899b84b2a80e345  /data/overlay/upper/etc/init.d/S89usbgadget`

If the md5 is different or the file doesn't exist, **stop**. The boot-time ADB safety net may be missing. Investigate before proceeding.

- [ ] **Check available space on /data**

```bash
adb shell 'df -h /data'
```

Expected: at least 100 MiB free. If `/data` is close to full, clear stale files before proceeding.

---

## Task 2: C1a — Host simulation preflight

**Files:** None modified.

The simulation build exercises all of waved's async task machinery with in-memory mocks for UART, BT, HID, audio, MCU, WiFi, and meeting. If this panics, do not cross-compile.

- [ ] **Build the simulation binary**

```bash
cd waved
cargo build --release -p waved --features simulate 2>&1 | tail -5
```

Expected last line:
```
Finished `release` profile [optimized] target(s) in Xs
```

If you see a compile error, fix it in source before continuing.

- [ ] **Run the simulation and observe startup (Ctrl-C after ~10 seconds)**

```bash
cd waved
cargo run -p waved --features simulate 2>&1 &
WAVEPID=$!
sleep 10
kill $WAVEPID 2>/dev/null
wait $WAVEPID 2>/dev/null
```

Expected: log output appears within 10 seconds (any startup announcements from waved's async tasks — service names, state transitions). No `panic`, no `SIGSEGV`, no `thread main panicked`.

If the process exits on its own before 10 seconds, check the exit code: `echo $?`. Anything non-zero is a failure.

- [ ] **Confirm exit was clean**

The `kill` above will exit the process. Check that no panic log was produced:

```bash
cargo run -p waved --features simulate 2>&1 | grep -E "panic|thread.*panicked|SIGSEGV" | head
```

Expected: no output (grep returns nothing).

---

## Task 3: C1b — Cross-build for device

**Files:** Produces `target/aarch64-unknown-linux-gnu/release/waved` and `target/aarch64-unknown-linux-gnu/release/waved-device-test`.

A fresh build is required — the last binary was built March 23 and `waved-meeting/Cargo.toml`, `waved-uart/Cargo.toml`, `waved-device-test/Cargo.toml` have changed since then.

- [ ] **Cross-build waved**

```bash
cd waved
cross build --release --target aarch64-unknown-linux-gnu -p waved 2>&1 | tail -5
```

Expected last line:
```
Finished `release` profile [optimized] target(s) in Xs
```

This takes several minutes on first run (Docker image pull + full compile). If `cross` exits with `error: No such file or directory (os error 2)` or a rustup detection error, install cross from git:

```bash
cargo install cross --git https://github.com/cross-rs/cross
```

Then re-run the build command.

- [ ] **Cross-build waved-device-test**

```bash
cross build --release --target aarch64-unknown-linux-gnu -p waved-device-test 2>&1 | tail -5
```

Expected: `Finished ...` (same as above).

- [ ] **Verify both binaries are present and recent**

```bash
ls -lh target/aarch64-unknown-linux-gnu/release/waved \
       target/aarch64-unknown-linux-gnu/release/waved-device-test
```

Expected: both files present, sizes in the ~5–15 MiB range, timestamps within the last few minutes.

---

## Task 4: C1c — On-device hardware self-test

**Files:** Pushes `waved-device-test` to `/data/waved/` on device.

This is the highest-information step — it exercises UART/MCU, HID gadget, WiFi, battery, audio, filesystem, config parsing, and recordings dir without running the full daemon. Any failure here means the daemon will fail too; fix the failing subsystem before continuing.

- [ ] **Pre-check: which binaries link against libapplogrs.so (R4 mitigation)**

```bash
adb shell 'grep -rl libapplogrs /usr/bin /usr/sbin /usr/lib 2>/dev/null | head -20'
```

Expected: a list of files (likely `pspapp`, `libins_entry.so`, etc.). Capture the output. These are the binaries that will break if `libapplogrs.so` is emptied. Since `install.sh` stubs pspapp out entirely, this should not matter — but it's good to know which binaries we're affecting.

If `waved` appears in the list, **stop** — the waved binary should not link against libapplogrs.so. That would be a compile-time error to fix.

- [ ] **Push waved-device-test to device**

```bash
cd waved
adb shell mkdir -p /data/waved
adb push target/aarch64-unknown-linux-gnu/release/waved-device-test /data/waved/waved-device-test
adb shell chmod +x /data/waved/waved-device-test
```

Expected: `adb push` prints transfer speed line, no errors.

- [ ] **Stop pspapp (so waved-device-test can claim UART and HID)**

```bash
adb shell 'killall pspapp_daemon pspapp_watchdog pspapp 2>/dev/null || true ; sleep 1 ; ps | grep pspapp | grep -v grep'
```

Expected: the grep returns nothing (no pspapp processes). The `|| true` suppresses "no process found" errors.

- [ ] **Run waved-device-test and capture output**

```bash
adb shell /data/waved/waved-device-test
```

Expected output format:
```
=== waved device test runner ===

[PASS] test_uart_responsive (Xms)
[PASS] test_hid_gadget (Xms)
[PASS] test_wifi_scan (Xms)
[PASS] test_battery_read (Xms)
[PASS] test_audio_device (Xms)
[PASS] test_filesystem (Xms)
[PASS] test_config_load (Xms)
[PASS] test_recordings_dir (Xms)

8/8 passed, 0 failed
```

Exit code 0 (ADB will not print an error). Any `[FAIL]` line is a blocker — **stop here**, fix the failing subsystem, rebuild, re-push, re-run before proceeding.

---

## Task 5: C1d — On-device foreground daemon smoke test

**Files:** Pushes `waved` and `config.toml` to `/data/waved/` on device.

- [ ] **Push waved binary and default config**

```bash
cd waved
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell chmod +x /data/waved/waved
# Only push config if it doesn't already exist on device
adb shell 'test -f /data/waved/config.toml && echo "config exists" || echo "config missing"'
```

If config is missing:
```bash
adb push waved/config/default.toml /data/waved/config.toml
adb shell chmod 0600 /data/waved/config.toml
```

- [ ] **Verify pspapp is still stopped (respawn guard)**

```bash
adb shell 'ps | grep pspapp | grep -v grep'
```

Expected: no output. If pspapp_daemon respawned pspapp, kill again:
```bash
adb shell 'killall pspapp_daemon pspapp_watchdog pspapp 2>/dev/null || true'
```

- [ ] **Run waved in the foreground with a 60-second timeout**

```bash
adb shell 'timeout 60 /data/waved/waved 2>&1' | tee /tmp/waved_c1d.log
```

This captures output to `/tmp/waved_c1d.log` on the host for inspection. The command returns automatically after 60 seconds.

- [ ] **Inspect the output for pass criteria**

```bash
echo "--- first 20 lines ---"
head -20 /tmp/waved_c1d.log
echo "--- any panics? ---"
grep -E "panic|thread.*panicked|SIGSEGV|Segmentation fault" /tmp/waved_c1d.log || echo "no panics found ✓"
echo "--- any output within 30s marker? ---"
wc -l /tmp/waved_c1d.log
```

Pass criteria:
- `wc -l` shows at least one line (binary produced output)
- `grep` for panics/segfaults returns nothing
- No crash loops visible (e.g., the same error repeating every few lines)

If the binary produced no output at all within 60 seconds, or crashed immediately: **stop**. Investigate with `adb shell dmesg | tail -30` for kernel-level messages.

- [ ] **Check dmesg for any segfaults or kernel errors from waved**

```bash
adb shell 'dmesg | grep -E "waved|segfault|oops|killed" | tail -10'
```

Expected: no output (grep returns nothing). If you see a segfault line referencing `waved`, that's a critical failure — stop.

> **GATE 1:** All of Tasks 1–5 must pass cleanly. If any step failed, do not proceed to Task 6 (C2 overlay install).

---

## Task 6: C2 — Overlay install and validation

**Files:** Creates/modifies on device (via overlay):
- `/data/overlay/upper/etc/init.d/S60waved` (created)
- `/data/overlay/upper/etc/init.d/S60pspapp_daemon` (created — stub)
- `/data/overlay/upper/usr/lib/libapplogrs.so` (created — empty)
- `/data/waved/iptables.rules` (pushed)
- `/mnt/udisk/recordings/` (created)

**Do not reboot after this task** — validation must pass first (Task 7 Gate check).

- [ ] **Run install.sh**

```bash
cd /Users/xaviergeerinck/Projects/insta360
./waved/deploy/install.sh
```

Expected output:
```
=== Stopping stock firmware ===
=== Pushing waved binary ===
<adb push transfer stats>
=== Pushing config ===
=== Installing init script ===
<adb push transfer stats>
=== Removing stock pspapp init ===
=== Removing telemetry library ===
=== Installing firewall rules ===
<adb push transfer stats>
=== Creating recordings directory ===
=== Done! Reboot to start waved ===
Run: adb reboot
```

If the script exits with a non-zero code, **do not reboot**. Inspect the error and re-run manually.

- [ ] **Validate all overlay targets are present**

```bash
adb shell '
set -e
echo "=== overlay init.d files ==="
ls -la /data/overlay/upper/etc/init.d/S60waved \
       /data/overlay/upper/etc/init.d/S60pspapp_daemon
echo "=== S89usbgadget overlay md5 (must not change) ==="
md5sum /data/overlay/upper/etc/init.d/S89usbgadget
echo "=== libapplogrs stub ==="
ls -la /data/overlay/upper/usr/lib/libapplogrs.so
echo "=== binary ==="
test -x /data/waved/waved && echo "binary present and executable ✓"
echo "=== config and iptables ==="
md5sum /data/waved/config.toml /data/waved/iptables.rules
echo "=== recordings dir ==="
ls -la /mnt/udisk/ | grep recordings
'
```

Expected:
- Both init.d files listed with correct timestamps
- S89usbgadget md5 = `554342b3eceeda0d5899b84b2a80e345` (must be unchanged — if different, **stop**)
- libapplogrs.so is 0 bytes or 1 byte (the empty stub written by install.sh)
- binary present and executable
- config.toml and iptables.rules both present with non-zero md5
- recordings dir exists

If any check fails: **do not reboot**. Fix install.sh or apply the missing step manually, then re-run this validation block.

- [ ] **Verify S60pspapp_daemon is actually a no-op stub**

```bash
adb shell 'cat /etc/init.d/S60pspapp_daemon'
```

Expected: a single line `#!/bin/sh` (or similar minimal content). The file should not contain any reference to `pspapp_daemon`.

> **GATE 2:** Both validation steps in Task 6 must pass, including the S89usbgadget md5 check. Only proceed to Task 7 (the reboot) if everything is green.

---

## Task 7: C3 — Reboot survival validation

**Files:** Nothing modified — this task only reads state.

- [ ] **Reboot the device**

```bash
adb reboot
```

Start a timer. Expected: ADB comes back within ~15 seconds wall-clock.

- [ ] **Wait for device and record wall-clock time**

```bash
time adb wait-for-device
```

Expected: `real    0m12.xxx` or similar (under 20s). If ADB hasn't come back after 30s: unplug and replug USB-C, wait another 30s. If still nothing after 60s total, **do not revert yet** — first check if factory devmode could still be active. Since pspapp is now stubbed, boot-time ADB via S89usbgadget is the only path. If that failed (R6), you need UART access.

- [ ] **Run the full C3 verification block**

```bash
adb shell '
echo "=== UPTIME ==="
cat /proc/uptime | cut -d" " -f1

echo "=== waved process ==="
ps -o pid,ppid,user,comm | grep -E "waved" | grep -v grep

echo "=== pspapp process (must be EMPTY) ==="
ps -o pid,user,comm | grep -E "pspapp" | grep -v grep || echo "no pspapp ✓"

echo "=== adbd start time in dmesg ==="
dmesg | grep -E "adbd" | head -3

echo "=== S89usbgadget overlay file still intact? ==="
md5sum /data/overlay/upper/etc/init.d/S89usbgadget

echo "=== waved logs ==="
logread 2>/dev/null | grep waved | tail -20 || dmesg | grep waved | tail -20

echo "=== libapplogrs neutralized? ==="
ls -la /usr/lib/libapplogrs.so

echo "=== USB gadget config ==="
cat /sys/kernel/config/usb_gadget/amlogic/configs/amlogic.1/strings/0x409/configuration
ls /sys/kernel/config/usb_gadget/amlogic/configs/amlogic.1/ | grep -v -E "MaxPower|bmAttributes|strings"

echo "=== disk usage check ==="
df -h /data /var/log 2>/dev/null || df -h /data
'
```

- [ ] **Check all 7 pass criteria against the output above**

| # | Criterion | Where to look |
|---|---|---|
| 1 | ADB back within ~15s wall-clock | `time adb wait-for-device` output |
| 2 | `waved` process is running | `ps` block — must show waved |
| 3 | Zero `pspapp*` processes | `ps` block — must show `no pspapp ✓` |
| 4 | `dmesg` shows adbd starting around uptime ~8s | `adbd start time` block — check the timestamp in brackets |
| 5 | USB gadget: both `ffs.adb` and `hid.0` symlinked | `USB gadget config` block — both should be listed |
| 6 | `/usr/lib/libapplogrs.so` is 0 or 1 bytes (stub) | `libapplogrs` block — `ls -la` shows size |
| 7 | waved logs contain startup output, no crash loop | `waved logs` block — look for repeated `waved exited` in tight succession |

- [ ] **Additional: check S89usbgadget md5 survived the reboot (R6 guard)**

Expected: `554342b3eceeda0d5899b84b2a80e345  /data/overlay/upper/etc/init.d/S89usbgadget`

If any criterion fails:

```bash
./waved/deploy/revert.sh
adb reboot
```

Then diagnose from the collected logs before re-attempting.

- [ ] **Confirm C3 passed — record the device state**

```bash
adb shell '
echo "=== Final state snapshot ==="
echo "waved PID/comm: $(ps -o pid,comm,user | grep waved | grep -v grep | head -1)"
echo "adbd PID: $(pidof adbd)"
echo "USB mode: $(cat /sys/kernel/config/usb_gadget/amlogic/configs/amlogic.1/strings/0x409/configuration)"
echo "S89 overlay md5: $(md5sum /data/overlay/upper/etc/init.d/S89usbgadget | cut -d" " -f1)"
echo "Data used: $(df -h /data | tail -1)"
'
```

Save this output. It is the new "what normal looks like" baseline with `waved` running.

> **GATE 3:** All 7 pass criteria met and S89usbgadget md5 confirmed. The deployment is done. C4 (slot A partition flash) can be planned separately once this state has been stable for a period.

---

## Rollback reference

At any point:

```bash
./waved/deploy/revert.sh  # removes overlay files
adb reboot                # device boots back to pure stock pspapp
```

`revert.sh` removes: `S60waved`, `S60pspapp_daemon` stub, `libapplogrs.so` stub. It does **not** touch the `S89usbgadget` overlay edit — boot-time ADB is preserved through a revert.

If ADB is unavailable after a bad reboot: factory devmode is no longer a fallback (pspapp is stubbed in C2). Recovery requires UART (solder the pads on the board per `docs/GUIDE_DEVICE_ACCESS.md`) or Amlogic MaskROM via `pyamlboot` (`boot-g12.py`, toolchain already installed and verified).

---

## C4 investigation tasks (deferred — do not execute yet)

These are pre-work tasks to resolve the C4 TBDs when ready. Do not run until C3 has been stable for a period.

- [ ] **Inspect SquashFS parameters of the active rootfs**

```bash
unsquashfs -s backups/partitions/p11_system_a.img
```

Note the compression algorithm and block size — `mksquashfs` must match exactly.

- [ ] **Inspect the misc partition A/B state struct**

```bash
xxd backups/partitions/p07_misc.img | head -32
```

Look for magic bytes (`boot-recovery`, `BAK`, or Amlogic-specific headers). Compare against AOSP `bootable/recovery/bootloader_message/` struct layout.

- [ ] **Check current active slot from on-device bootloader perspective**

```bash
adb shell 'cat /proc/cmdline | grep -o "slot_suffix=_[ab]"'
```

Expected: `slot_suffix=_b` (current active slot). When we write slot A and switch, this should change to `slot_suffix=_a` after the next reboot.
