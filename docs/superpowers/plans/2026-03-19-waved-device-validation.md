# Waved Device Validation Plan — Hardware Go/No-Go Gates

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Validate all hardware assumptions and reverse-engineered protocol details on the live Insta360 Wave device before committing to `waved` Phase 1 implementation.
**Architecture:** Each task is a self-contained on-device test run via `adb shell`. Test scripts are pushed to `/data/waved_validation/` on the device and executed as root. Tests are strictly non-destructive — stock firmware remains intact and functional after every test. Results are pulled back to the host for analysis.
**Tech Stack:** Shell scripts + Python 3 (on-device BusyBox/Python), ADB, `strace`, ALSA utils, `xxd`/`hexdump`
**Depends on:** Physical access to Insta360 Wave device with ADB enabled (run `enable_adb.py` first — see `docs/GUIDE_DEVICE_ACCESS.md`)

---

## Preliminary Setup

Before running any task, establish device access and create the working directory:

```bash
# On host: enable ADB
python3 enable_adb.py
sleep 3
adb devices  # Must show PSP_<serial>

# On device: create validation workspace
adb shell mkdir -p /data/waved_validation
adb shell mkdir -p /data/waved_validation/results
```

All test scripts below are created on the host and pushed via `adb push`.

---

## Task 1: Baseline Capture — Document Stock Audio State

Before touching anything, capture the stock system state so we can detect regressions and restore cleanly.

**Files:** `scripts/01_baseline_capture.sh`

- [ ] 1.1 Create and push the baseline capture script:

```bash
cat > /tmp/01_baseline_capture.sh << 'SCRIPT'
#!/bin/sh
# Baseline capture — run BEFORE any other validation test
OUT="/data/waved_validation/results/01_baseline"
mkdir -p "$OUT"

echo "=== Kernel modules ==="
lsmod > "$OUT/lsmod.txt"
cat /proc/modules > "$OUT/proc_modules.txt"

echo "=== Running processes ==="
ps aux > "$OUT/ps_aux.txt"

echo "=== ALSA state ==="
cat /proc/asound/cards > "$OUT/alsa_cards.txt" 2>&1
cat /proc/asound/devices > "$OUT/alsa_devices.txt" 2>&1
cat /proc/asound/pcm > "$OUT/alsa_pcm.txt" 2>&1
amixer contents > "$OUT/amixer_contents.txt" 2>&1

echo "=== USB gadget config ==="
ls -laR /sys/kernel/config/usb_gadget/ > "$OUT/usb_gadget_tree.txt" 2>&1
cat /sys/kernel/config/usb_gadget/*/UDC > "$OUT/usb_gadget_udc.txt" 2>&1

echo "=== Kernel modules detail ==="
cat /proc/modules | grep -i tea > "$OUT/tea_module.txt" 2>&1
modinfo tea_license > "$OUT/tea_license_modinfo.txt" 2>&1

echo "=== UART devices ==="
ls -la /dev/ttyS* > "$OUT/uart_devices.txt" 2>&1
stty -a -F /dev/ttyS1 > "$OUT/ttyS1_settings.txt" 2>&1

echo "=== HID gadget ==="
ls -la /dev/hidg* > "$OUT/hidg_devices.txt" 2>&1

echo "=== TEA SDK files ==="
ls -la /usr/lib/libInstaAudio.so /usr/lib/libTEAProcessSdk.so /usr/lib/libbva_audio.so > "$OUT/tea_libs.txt" 2>&1
ls -la /usr/bin/tea_models/ > "$OUT/tea_models.txt" 2>&1

echo "=== Init scripts ==="
ls -la /etc/init.d/S* > "$OUT/init_scripts.txt" 2>&1
cat /etc/init.d/S50_ko_install > "$OUT/S50_ko_install.txt" 2>&1
cat /etc/init.d/S60pspapp_daemon > "$OUT/S60pspapp_daemon.txt" 2>&1

echo "=== pspapp log (last 200 lines) ==="
tail -200 /opt/logs/psp.log > "$OUT/psp_log_tail.txt" 2>&1

echo "Baseline capture complete: $OUT"
ls -la "$OUT/"
SCRIPT

adb push /tmp/01_baseline_capture.sh /data/waved_validation/01_baseline_capture.sh
adb shell chmod +x /data/waved_validation/01_baseline_capture.sh
adb shell /data/waved_validation/01_baseline_capture.sh
```

- [ ] 1.2 Pull results to host and review:

```bash
adb pull /data/waved_validation/results/01_baseline/ ./validation_results/01_baseline/
```

- [ ] 1.3 Verify `tea_license` module is loaded:

```bash
adb shell lsmod | grep tea
# Expected: tea_license  <size>  0
```

**Pass criteria:** All files captured. `tea_license.ko` is loaded. `pspapp` is running. ALSA cards show at least 2 cards (speaker + USB UAC2). `/dev/hidg0` exists.

**Fail action:** If baseline is incomplete, the device may not be in the expected state. Debug boot issues before proceeding.

---

## Task 2: TEA SDK License Check (CRITICAL Go/No-Go)

This is the single most important test. The entire Phase 1 strategy depends on whether the TEA SDK continues to process audio when `tea_license.ko` is removed.

**Files:** `scripts/02_tea_license_test.sh`

**CRITICAL DECISION POINT:**
- **If audio still works after `rmmod`:** Phase 1 can use TEA SDK as-is. Proceed to Task 3.
- **If audio breaks:** Phase 1 must either (a) keep `tea_license.ko` loaded (if it doesn't interfere with `waved`), or (b) fall back to open-source audio immediately (Phase 2 becomes Phase 1). This changes the entire project timeline.

- [ ] 2.1 Verify audio works BEFORE the test (stock state):

```bash
# Confirm pspapp is running and audio is active
adb shell ps aux | grep pspapp
# Play a test tone or speak near the device and verify:
#   - Sound comes out of speaker (loopback or BT)
#   - USB audio works on host (if connected in audio mode)
```

- [ ] 2.2 Create and push the license removal test script:

```bash
cat > /tmp/02_tea_license_test.sh << 'SCRIPT'
#!/bin/sh
# TEA SDK license removal test
# SAFETY: This is reversible — modprobe tea_license restores it
OUT="/data/waved_validation/results/02_tea_license"
mkdir -p "$OUT"

echo "=== Phase A: Pre-removal state ==="
lsmod | grep tea > "$OUT/pre_lsmod.txt"
dmesg | tail -50 > "$OUT/pre_dmesg.txt"

echo "=== Phase B: Remove tea_license.ko ==="
rmmod tea_license 2>&1 | tee "$OUT/rmmod_output.txt"
RMMOD_RC=$?
echo "rmmod exit code: $RMMOD_RC" >> "$OUT/rmmod_output.txt"

echo "=== Phase C: Post-removal state ==="
lsmod | grep tea > "$OUT/post_lsmod.txt"
dmesg | tail -50 > "$OUT/post_dmesg.txt"

echo "=== Phase D: Check if pspapp crashed ==="
sleep 2
ps aux | grep pspapp > "$OUT/post_pspapp.txt"

echo "=== Phase E: Check audio pipeline ==="
# Try to capture a short audio sample from the microphone array
# If TEA SDK is blocked, ALSA capture may fail or return silence
arecord -D hw:0,0 -f S16_LE -r 48000 -c 2 -d 3 "$OUT/test_capture.wav" 2>&1 | tee "$OUT/arecord_output.txt"
ARECORD_RC=$?
echo "arecord exit code: $ARECORD_RC" >> "$OUT/arecord_output.txt"

echo "=== Phase F: Check pspapp logs for license errors ==="
tail -50 /opt/logs/psp.log > "$OUT/post_psp_log.txt" 2>&1
dmesg | grep -i -E "tea|license|audio" > "$OUT/post_dmesg_audio.txt" 2>&1

echo "=== Phase G: Restore tea_license.ko ==="
modprobe tea_license 2>&1 | tee "$OUT/modprobe_output.txt"
# If modprobe fails (module not in standard path), try insmod with full path:
if [ $? -ne 0 ]; then
    # Find the module
    find / -name "tea_license.ko" 2>/dev/null > "$OUT/tea_license_path.txt"
    KO_PATH=$(head -1 "$OUT/tea_license_path.txt")
    if [ -n "$KO_PATH" ]; then
        insmod "$KO_PATH" 2>&1 | tee -a "$OUT/modprobe_output.txt"
    fi
fi
lsmod | grep tea > "$OUT/restored_lsmod.txt"

echo "License test complete. Results in $OUT"
ls -la "$OUT/"
SCRIPT

adb push /tmp/02_tea_license_test.sh /data/waved_validation/02_tea_license_test.sh
adb shell chmod +x /data/waved_validation/02_tea_license_test.sh
```

- [ ] 2.3 Stop pspapp, run the test, observe behavior:

```bash
# Stop stock app so we control the audio pipeline
adb shell killall pspapp pspapp_daemon pspapp_watchdog 2>/dev/null

# Run the test
adb shell /data/waved_validation/02_tea_license_test.sh
```

- [ ] 2.4 Pull results and analyze:

```bash
adb pull /data/waved_validation/results/02_tea_license/ ./validation_results/02_tea_license/

# Check if rmmod succeeded
cat ./validation_results/02_tea_license/rmmod_output.txt

# Check if audio capture produced non-silent data
# A file of all zeros = TEA SDK blocked the pipeline
ls -la ./validation_results/02_tea_license/test_capture.wav
# On macOS, play the file:
afplay ./validation_results/02_tea_license/test_capture.wav 2>/dev/null
```

- [ ] 2.5 Test the alternative scenario — run pspapp WITHOUT tea_license loaded:

```bash
# Ensure tea_license is still removed
adb shell lsmod | grep tea
# If present, remove it again:
adb shell rmmod tea_license

# Restart pspapp and watch for crashes/errors
adb shell /usr/bin/pspapp &
sleep 5
adb shell ps aux | grep pspapp
adb shell tail -30 /opt/logs/psp.log
# Speak near the device — does audio come through USB/speaker?
```

- [ ] 2.6 Restore stock state:

```bash
adb shell killall pspapp 2>/dev/null
# Reboot to fully restore stock state (tea_license.ko loaded by S50_ko_install)
adb shell reboot
# Re-enable ADB after reboot
sleep 30
python3 enable_adb.py
sleep 3
adb devices
```

**Pass criteria:** `rmmod` succeeds AND audio still works (capture file contains non-silent audio, pspapp does not crash). This means the license check is a soft check or only runs at init time.

**Fail criteria:** Audio goes silent, pspapp crashes, or ALSA returns errors. Record the exact error messages.

**If FAIL — secondary test:** Check if `tea_license.ko` can coexist with `waved` (i.e., leave it loaded, never call `rmmod`). If yes, Phase 1 still works — we just keep the module loaded. Only if `tea_license.ko` actively interferes with a non-pspapp caller is this a true blocker.

---

## Task 3: TEA SDK Config Override

Test whether `libInstaAudio.so` can load an external JSON config to tune audio parameters (AEC aggressiveness, AGC, latency) without binary-patching the TEA SDK.

**Files:** `scripts/03_tea_config_override.sh`, `scripts/tea_config_test.json`

- [ ] 3.1 First, find how the stock config is loaded:

```bash
# Search for config-related strings in the TEA libraries
adb shell strings /usr/lib/libInstaAudio.so | grep -i -E "config|json|\.json|path|load|file"
adb shell strings /usr/lib/libTEAProcessSdk.so | grep -i -E "config|json|\.json|path|load|file"

# Check if there's already a config file anywhere
adb shell find / -name "*tea*config*" -o -name "*audio*config*" -o -name "*insta*audio*" 2>/dev/null | grep -i -E "json|cfg|conf|ini"

# Check /data for any audio configs
adb shell ls -laR /data/ 2>/dev/null | grep -i -E "audio|tea|config"

# Check environment variables that pspapp uses
adb shell cat /proc/$(adb shell pidof pspapp)/environ 2>/dev/null | tr '\0' '\n' | grep -i audio
```

- [ ] 3.2 Create a test config JSON (modified from stock defaults with a clearly detectable change):

```bash
cat > /tmp/tea_config_test.json << 'JSON'
{
    "version": "3.10.1.1153",
    "pipeline": {
        "aec": {
            "ec_aggr": 0.8,
            "tail_length_ms": 500,
            "nlms_tail_ms": 180
        },
        "agc": {
            "agc_alpha": 0.3,
            "agc_attack": 0.1,
            "target_level_dbfs": -3
        },
        "ns": {
            "level": 2
        }
    },
    "test_marker": "waved_validation_config_v1"
}
JSON

adb push /tmp/tea_config_test.json /data/waved_validation/tea_config_test.json
```

- [ ] 3.3 Create and run the config override test:

```bash
cat > /tmp/03_tea_config_override.sh << 'SCRIPT'
#!/bin/sh
# TEA SDK config override test — try multiple candidate paths
OUT="/data/waved_validation/results/03_tea_config"
mkdir -p "$OUT"

CONFIG_SRC="/data/waved_validation/tea_config_test.json"

# Candidate paths (based on firmware analysis and common patterns)
CANDIDATES="
/data/tea_config.json
/data/waved/tea_config.json
/data/__qci/config.json
/data/__qci/tea_config.json
/tmp/tea_config.json
/userdata/roomix/config.json
/etc/tea_config.json
/usr/bin/tea_models/config.json
"

echo "=== Phase A: Check if libInstaAudio.so references config paths ==="
strings /usr/lib/libInstaAudio.so | grep -i "\.json" > "$OUT/libInstaAudio_json_strings.txt" 2>&1
strings /usr/lib/libTEAProcessSdk.so | grep -i "\.json" > "$OUT/libTEAProcess_json_strings.txt" 2>&1
strings /usr/lib/libInstaAudio.so | grep -i "config" > "$OUT/libInstaAudio_config_strings.txt" 2>&1
strings /usr/lib/libTEAProcessSdk.so | grep -i "config" > "$OUT/libTEAProcess_config_strings.txt" 2>&1

echo "=== Phase B: Use strace to find config file opens ==="
# Kill stock app, then restart with strace to see what files it opens
killall pspapp pspapp_daemon pspapp_watchdog 2>/dev/null
sleep 2

# Run pspapp under strace, filtering for open/openat calls
# Timeout after 15 seconds (enough for init)
timeout 15 strace -f -e trace=openat -o "$OUT/strace_openat.txt" /usr/bin/pspapp 2>&1 &
STRACE_PID=$!
sleep 15
kill $STRACE_PID 2>/dev/null
killall pspapp strace 2>/dev/null
sleep 2

# Filter for audio/tea/config related opens
grep -i -E "tea|audio|config|json|roomix|qci" "$OUT/strace_openat.txt" > "$OUT/strace_filtered.txt" 2>&1

echo "=== Phase C: Try each candidate path ==="
for CANDIDATE in $CANDIDATES; do
    DIR=$(dirname "$CANDIDATE")
    mkdir -p "$DIR" 2>/dev/null
    cp "$CONFIG_SRC" "$CANDIDATE"
    echo "Placed config at: $CANDIDATE" >> "$OUT/candidate_log.txt"
done

# Restart pspapp briefly and check logs for config loading
timeout 15 /usr/bin/pspapp > "$OUT/pspapp_with_configs.txt" 2>&1 &
sleep 15
killall pspapp 2>/dev/null

# Check logs
tail -100 /opt/logs/psp.log > "$OUT/psp_log_with_configs.txt" 2>&1
grep -i -E "config|json|tea_config|load|parse" /opt/logs/psp.log > "$OUT/psp_log_config_lines.txt" 2>&1

echo "=== Phase D: Clean up candidate configs ==="
for CANDIDATE in $CANDIDATES; do
    rm -f "$CANDIDATE" 2>/dev/null
done

echo "Config override test complete. Results in $OUT"
ls -la "$OUT/"
SCRIPT

adb push /tmp/03_tea_config_override.sh /data/waved_validation/03_tea_config_override.sh
adb shell chmod +x /data/waved_validation/03_tea_config_override.sh
adb shell /data/waved_validation/03_tea_config_override.sh
```

- [ ] 3.4 Pull and analyze results:

```bash
adb pull /data/waved_validation/results/03_tea_config/ ./validation_results/03_tea_config/

# Key files to check:
# 1. strace_filtered.txt — shows what files TEA SDK tries to open
# 2. libInstaAudio_json_strings.txt — embedded path references
# 3. psp_log_config_lines.txt — log evidence of config loading
cat ./validation_results/03_tea_config/strace_filtered.txt
```

- [ ] 3.5 Restore stock state:

```bash
adb shell reboot
sleep 30
python3 enable_adb.py && sleep 3 && adb devices
```

**Pass criteria:** `strace` reveals a file path that `libInstaAudio.so` or `libTEAProcessSdk.so` attempts to open for config. Placing our JSON at that path causes a detectable change in behavior (log message, audio parameter change).

**Fail criteria:** No external config path found. The config is embedded in the `.so` binary.

**If FAIL — fallback:** Binary-patch `libTEAProcessSdk.so` to change embedded config values. Use `hexdump` + `dd` or a Python script to locate the JSON blob inside the binary and overwrite specific values (e.g., `ec_aggr`). Place the patched `.so` on the overlay at `/data/overlay/upper/usr/lib/libTEAProcessSdk.so`. This is more fragile but workable.

---

## Task 4: UART Protocol Validation

Capture live UART traffic on `/dev/ttyS1` to validate the frame format, command IDs, and byte offsets for battery, motor, and button frames. The `waved-uart` frame parser uses placeholder byte offsets from static analysis that need on-device confirmation.

**Files:** `scripts/04_uart_capture.sh`, `scripts/04_uart_parser.py`

- [ ] 4.1 Create the UART capture script:

```bash
cat > /tmp/04_uart_capture.sh << 'SCRIPT'
#!/bin/sh
# UART traffic capture on /dev/ttyS1
# SAFETY: We only READ from the UART — no writes. pspapp continues to run
# and communicate normally. We just snoop on the traffic.
OUT="/data/waved_validation/results/04_uart"
mkdir -p "$OUT"

echo "=== Phase A: UART device info ==="
stty -a -F /dev/ttyS1 > "$OUT/ttyS1_stty.txt" 2>&1
ls -la /dev/ttyS1 > "$OUT/ttyS1_dev.txt" 2>&1

echo "=== Phase B: Capture raw UART bytes ==="
echo "Capturing 30 seconds of UART traffic..."
echo "During capture: press mute button, extend/retract motor, check battery"

# Use cat + timeout to capture raw bytes (pspapp has ttyS1 open, but we
# can open it read-only in parallel on Linux)
# Alternative: use strace on pspapp to see UART reads/writes
PSPAPP_PID=$(pidof pspapp)
if [ -n "$PSPAPP_PID" ]; then
    echo "pspapp PID: $PSPAPP_PID"

    # Capture 30 seconds of UART I/O via strace on the pspapp process
    # -e trace=read,write filters to I/O calls
    # -e read=<fd>,write=<fd> would be ideal but we need the fd number first

    # First, find which fd is ttyS1
    ls -la /proc/$PSPAPP_PID/fd/ 2>/dev/null | grep ttyS1 > "$OUT/pspapp_ttyS1_fd.txt"
    UART_FD=$(ls -la /proc/$PSPAPP_PID/fd/ 2>/dev/null | grep ttyS1 | awk '{print $(NF-2)}')
    echo "ttyS1 fd in pspapp: $UART_FD" >> "$OUT/pspapp_ttyS1_fd.txt"

    # Capture all read/write calls (we'll filter by fd later)
    echo "Starting strace capture for 30 seconds..."
    timeout 30 strace -f -p $PSPAPP_PID -e trace=read,write \
        -e read=$UART_FD -e write=$UART_FD \
        -x -o "$OUT/strace_uart_raw.txt" 2>&1

    # Also capture with -xx for full hex dump
    echo "Starting hex capture for 15 seconds..."
    timeout 15 strace -f -p $PSPAPP_PID -e trace=read,write \
        -xx -o "$OUT/strace_uart_hex.txt" 2>&1
else
    echo "ERROR: pspapp not running. UART traffic requires pspapp." > "$OUT/error.txt"
fi

echo "=== Phase C: Parse captured frames ==="
# Count frame headers in raw capture
grep -c "\\\\x75\\\\x46" "$OUT/strace_uart_raw.txt" > "$OUT/frame_count_7546.txt" 2>&1
grep -c "\\\\x75\\\\x45" "$OUT/strace_uart_raw.txt" > "$OUT/frame_count_7545.txt" 2>&1
grep -c "\\\\xdc\\\\xcd" "$OUT/strace_uart_raw.txt" > "$OUT/frame_count_dccd.txt" 2>&1

echo "UART capture complete. Results in $OUT"
ls -la "$OUT/"
SCRIPT

adb push /tmp/04_uart_capture.sh /data/waved_validation/04_uart_capture.sh
adb shell chmod +x /data/waved_validation/04_uart_capture.sh
```

- [ ] 4.2 Create the Python frame parser (pushed to device):

```bash
cat > /tmp/04_uart_parser.py << 'PYEOF'
#!/usr/bin/env python3
"""Parse UART frames from strace output.

Frame format (from firmware analysis):
  BT frames:  [0x75] [0x46 or 0x45] [length] [data...] [checksum]
  MCU frames: [0xDC] [0xCD] [length] [data...] [checksum]
  Checksum = additive sum of all bytes (mod 256)

This script parses strace -xx output to extract and decode UART frames.
"""
import sys
import re
import os

def parse_strace_hex(filepath):
    """Extract hex byte sequences from strace -xx output."""
    frames = []
    with open(filepath, 'r') as f:
        for line in f:
            # Match read/write calls with hex data
            m = re.search(r'(?:read|write)\(\d+,\s*"((?:\\x[0-9a-f]{2})+)"', line)
            if m:
                hex_str = m.group(1)
                # Convert \xNN sequences to bytes
                data = bytes.fromhex(hex_str.replace('\\x', ''))
                frames.append(('R' if 'read(' in line else 'W', data))
    return frames

def find_frames(raw_bytes):
    """Find UART frames in a byte stream."""
    results = []
    i = 0
    while i < len(raw_bytes) - 3:
        # BT frame type 1: 0x75 0x46
        if raw_bytes[i] == 0x75 and raw_bytes[i+1] == 0x46:
            length = raw_bytes[i+2] if i+2 < len(raw_bytes) else 0
            frame_end = i + 3 + length + 1  # header(2) + len(1) + data(N) + checksum(1)
            if frame_end <= len(raw_bytes):
                frame = raw_bytes[i:frame_end]
                results.append(('BT_46', frame))
            i = max(i + 1, frame_end)
        # BT frame type 2: 0x75 0x45
        elif raw_bytes[i] == 0x75 and raw_bytes[i+1] == 0x45:
            length = raw_bytes[i+2] if i+2 < len(raw_bytes) else 0
            frame_end = i + 3 + length + 1
            if frame_end <= len(raw_bytes):
                frame = raw_bytes[i:frame_end]
                results.append(('BT_45', frame))
            i = max(i + 1, frame_end)
        # MCU frame: 0xDC 0xCD
        elif raw_bytes[i] == 0xDC and raw_bytes[i+1] == 0xCD:
            length = raw_bytes[i+2] if i+2 < len(raw_bytes) else 0
            frame_end = i + 3 + length + 1
            if frame_end <= len(raw_bytes):
                frame = raw_bytes[i:frame_end]
                results.append(('MCU', frame))
            i = max(i + 1, frame_end)
        else:
            i += 1
    return results

def verify_checksum(frame):
    """Verify additive checksum (sum of all bytes mod 256 should equal last byte)."""
    if len(frame) < 4:
        return False
    payload = frame[:-1]
    expected = frame[-1]
    actual = sum(payload) & 0xFF
    return actual == expected

def decode_mcu_frame(frame):
    """Attempt to decode MCU frame data fields."""
    if len(frame) < 4:
        return "too short"
    cmd = frame[3] if len(frame) > 3 else None
    data = frame[3:-1] if len(frame) > 4 else b''

    info = f"cmd=0x{cmd:02X}" if cmd else "no cmd"

    # Known MCU command IDs (from firmware analysis — NEED VALIDATION)
    MCU_CMDS = {
        0x01: "battery_level",
        0x02: "battery_voltage",
        0x03: "battery_temperature",
        0x04: "battery_current",
        0x05: "charging_state",
        0x10: "motor_extend",
        0x11: "motor_retract",
        0x12: "motor_stop",
        0x20: "button_event",
        0x30: "led_control",
        0x40: "power_shutdown",
        0x41: "power_reboot",
    }

    if cmd in MCU_CMDS:
        info += f" ({MCU_CMDS[cmd]})"
    info += f" data={data.hex()}"
    return info

def main():
    results_dir = "/data/waved_validation/results/04_uart"
    strace_file = os.path.join(results_dir, "strace_uart_hex.txt")
    output_file = os.path.join(results_dir, "parsed_frames.txt")

    if not os.path.exists(strace_file):
        print(f"ERROR: {strace_file} not found. Run capture first.")
        sys.exit(1)

    io_ops = parse_strace_hex(strace_file)

    # Concatenate all bytes into a stream for frame detection
    all_read_bytes = b''.join(data for direction, data in io_ops if direction == 'R')
    all_write_bytes = b''.join(data for direction, data in io_ops if direction == 'W')

    with open(output_file, 'w') as out:
        out.write(f"=== UART Frame Analysis ===\n")
        out.write(f"Total I/O operations: {len(io_ops)}\n")
        out.write(f"Total read bytes: {len(all_read_bytes)}\n")
        out.write(f"Total write bytes: {len(all_write_bytes)}\n\n")

        for label, stream in [("READ (device->SoC)", all_read_bytes),
                               ("WRITE (SoC->device)", all_write_bytes)]:
            frames = find_frames(stream)
            out.write(f"--- {label}: {len(frames)} frames ---\n")

            for frame_type, frame in frames:
                checksum_ok = verify_checksum(frame)
                hex_repr = frame.hex()
                line = f"  [{frame_type}] {hex_repr} checksum={'OK' if checksum_ok else 'FAIL'}"

                if frame_type == 'MCU':
                    line += f" | {decode_mcu_frame(frame)}"
                elif frame_type.startswith('BT'):
                    cmd_id = frame[3] if len(frame) > 3 else None
                    if cmd_id:
                        line += f" | cmd=0x{cmd_id:02X}"

                out.write(line + "\n")
            out.write("\n")

    print(f"Parsed frames written to {output_file}")
    # Also print summary to stdout
    with open(output_file, 'r') as f:
        print(f.read())

if __name__ == '__main__':
    main()
PYEOF

adb push /tmp/04_uart_parser.py /data/waved_validation/04_uart_parser.py
adb shell chmod +x /data/waved_validation/04_uart_parser.py
```

- [ ] 4.3 Run the capture (interact with the device during the 30-second window):

```bash
# Run capture — during the 30 seconds, physically:
# 1. Press the mute button (single press, double press, long press)
# 2. Extend the motor column via touchscreen
# 3. Retract the motor column
# 4. Note battery level shown on screen
adb shell /data/waved_validation/04_uart_capture.sh
```

- [ ] 4.4 Run the parser:

```bash
# Check if Python3 is available on device
adb shell which python3 || adb shell which python
# If no Python on device, pull strace output and parse on host:
adb pull /data/waved_validation/results/04_uart/ ./validation_results/04_uart/
python3 /tmp/04_uart_parser.py  # (modify paths to point to local files)
```

- [ ] 4.5 Validate specific frame fields against the design spec:

```bash
# Key questions to answer from the parsed data:
# 1. Are BT frames really [0x75][0x46/0x45][len][data][checksum]?
# 2. Are MCU frames really [0xDC][0xCD][len][data][checksum]?
# 3. What command ID is used for battery level reports?
# 4. What command ID is used for button press events?
# 5. What command ID is used for motor control?
# 6. Is the checksum really additive sum of all preceding bytes?
# 7. What is the baud rate confirmed by stty output?
```

**Pass criteria:** Captured frames match the documented format: 2-byte header, 1-byte length, N-byte data, 1-byte additive checksum. At least one of each frame type (BT 0x7546, BT 0x7545, MCU 0xDCCD) is captured. Battery level, button events, and motor commands have identifiable command IDs and data layouts.

**Fail criteria:** Frame format does not match spec. Headers or checksum algorithm differ.

**If FAIL:** Update `waved-uart` frame parser with the correct format before proceeding. This is a data-correction issue, not a blocker.

---

## Task 5: USB HID Report Validation

Test the HID report format for mute and volume controls by writing to `/dev/hidg0` and observing macOS behavior. This validates the `waved-hid` module's report format.

**Files:** `scripts/05_hid_test.sh`

- [ ] 5.1 First, dump the HID descriptor to understand the report format:

```bash
# Get the HID report descriptor from sysfs
adb shell cat /sys/kernel/config/usb_gadget/*/functions/hid.*/report_desc | xxd > ./validation_results/05_hid/report_desc.hex

# Also check the HID gadget configuration
adb shell ls -la /sys/kernel/config/usb_gadget/*/functions/hid.*/
adb shell cat /sys/kernel/config/usb_gadget/*/functions/hid.*/protocol
adb shell cat /sys/kernel/config/usb_gadget/*/functions/hid.*/subclass
adb shell cat /sys/kernel/config/usb_gadget/*/functions/hid.*/report_length
```

- [ ] 5.2 Create and push the HID test script:

```bash
cat > /tmp/05_hid_test.sh << 'SCRIPT'
#!/bin/sh
# USB HID report test — write mute/volume reports to /dev/hidg0
# SAFETY: This sends HID reports to the host. Worst case: host sees
# a phantom mute/volume press. Easily reversed.
OUT="/data/waved_validation/results/05_hid"
mkdir -p "$OUT"

echo "=== Phase A: HID gadget info ==="
ls -la /dev/hidg* > "$OUT/hidg_devices.txt" 2>&1
ls -la /sys/kernel/config/usb_gadget/*/functions/hid.* > "$OUT/hidg_functions.txt" 2>&1

# Read report descriptor
for DESC in /sys/kernel/config/usb_gadget/*/functions/hid.*/report_desc; do
    if [ -f "$DESC" ]; then
        hexdump -C "$DESC" > "$OUT/report_desc_$(basename $(dirname $DESC)).hex" 2>&1
    fi
done

# Read report length
for RL in /sys/kernel/config/usb_gadget/*/functions/hid.*/report_length; do
    if [ -f "$RL" ]; then
        echo "$(dirname $RL): $(cat $RL)" >> "$OUT/report_lengths.txt"
    fi
done

echo "=== Phase B: Test Telephone Mute HID report ==="
# HID Telephone usage page (0x0B):
#   Usage: Mute (0x2F) — Usage Page: Telephony (0x0B)
# Typical report: [report_id] [button_bitmap]
# The exact format depends on the descriptor — check Phase A output

# Attempt 1: Simple 1-byte mute press + release
# This is a guess — must be validated against the report descriptor
echo "Sending mute press (attempt 1: simple bitmap)..."

# NOTE: The exact bytes depend on the HID report descriptor.
# From S89usbgadget analysis, the gadget has:
#   - Report ID 1: Vendor (64 bytes, usage page 0xFF00)
#   - Report ID 2: Telephone (usage page 0x0B) — Phone Mute
#   - Report ID 3: Consumer (usage page 0x0C) — Volume Up/Down/Mute
#
# Telephone Mute (Report ID 2):
#   Press:   \x02\x01  (report_id=2, mute=1)
#   Release: \x02\x00  (report_id=2, mute=0)
printf '\x02\x01' > /dev/hidg0 2>"$OUT/mute_press_err.txt"
echo "mute press rc=$?" >> "$OUT/hid_test_log.txt"
sleep 0.2
printf '\x02\x00' > /dev/hidg0 2>"$OUT/mute_release_err.txt"
echo "mute release rc=$?" >> "$OUT/hid_test_log.txt"
sleep 1

echo "=== Phase C: Test Consumer Volume HID report ==="
# Consumer Volume (Report ID 3):
#   Volume Up:   \x03\x01  (report_id=3, vol_up=1)
#   Release:     \x03\x00
#   Volume Down: \x03\x02  (report_id=3, vol_down=1)
#   Release:     \x03\x00
printf '\x03\x01' > /dev/hidg0 2>"$OUT/volup_err.txt"
echo "vol up rc=$?" >> "$OUT/hid_test_log.txt"
sleep 0.2
printf '\x03\x00' > /dev/hidg0 2>"$OUT/volup_release_err.txt"
echo "vol up release rc=$?" >> "$OUT/hid_test_log.txt"
sleep 1

printf '\x03\x02' > /dev/hidg0 2>"$OUT/voldown_err.txt"
echo "vol down rc=$?" >> "$OUT/hid_test_log.txt"
sleep 0.2
printf '\x03\x00' > /dev/hidg0 2>"$OUT/voldown_release_err.txt"
echo "vol down release rc=$?" >> "$OUT/hid_test_log.txt"

echo "HID test complete. Check macOS for volume/mute changes."
echo "Results in $OUT"
ls -la "$OUT/"
SCRIPT

adb push /tmp/05_hid_test.sh /data/waved_validation/05_hid_test.sh
adb shell chmod +x /data/waved_validation/05_hid_test.sh
```

- [ ] 5.3 Switch device back to audio mode (out of ADB mode) to test HID:

This is the tricky part — ADB mode replaces the UAC+HID gadget. We need to either:
- (a) Test from within the stock firmware (let pspapp handle it and just observe), or
- (b) Modify the USB gadget to include both ADB + HID simultaneously.

**Option A — Observe stock HID behavior from macOS:**

```bash
# On macOS: monitor HID events while pressing the physical mute button on the device
# Use hidutil or IOKit to observe incoming reports
# This validates that the HID descriptor works natively with macOS
ioreg -p IOUSB -l | grep -A 20 "psp"
```

**Option B — Push script, switch to audio mode, let it run on a timer:**

```bash
# Push the script, set a delayed execution, then switch USB mode
adb push /tmp/05_hid_test.sh /data/waved_validation/05_hid_test.sh
adb shell chmod +x /data/waved_validation/05_hid_test.sh

# Schedule the test to run in 10 seconds (gives time to switch USB mode)
adb shell "nohup sh -c 'sleep 10 && /data/waved_validation/05_hid_test.sh' > /tmp/hid_test_bg.log 2>&1 &"

# Switch back to normal USB mode (UAC + HID)
# This will drop ADB — the test runs on the timer
adb shell reboot
# After reboot, device is back in UAC+HID mode
# Wait 60 seconds for boot + 10 second delay + test execution
# Then observe macOS: did volume/mute change?
```

- [ ] 5.4 After test, re-enable ADB and pull results:

```bash
python3 enable_adb.py
sleep 3
adb pull /data/waved_validation/results/05_hid/ ./validation_results/05_hid/
```

- [ ] 5.5 Parse the HID report descriptor to confirm report format:

```bash
# On host: decode the HID descriptor using a parser
# pip3 install hid-parser  (or use https://eleccelerator.com/usbdescreqparser/)
# Feed the hex bytes from report_desc.hex
```

**Pass criteria:** macOS recognizes mute/volume HID events from the device. The report descriptor confirms Telephone (0x0B) and Consumer (0x0C) usage pages. Writing the correct report bytes to `/dev/hidg0` triggers the expected host-side behavior.

**Fail criteria:** Reports are silently dropped, macOS does not react, or the report descriptor uses a different format than assumed.

**If FAIL:** Decode the full HID report descriptor and adjust the report byte format in `waved-hid`. This is a format correction, not a blocker.

---

## Task 6: USB Gadget Reconfiguration

Test whether the USB composite gadget can be modified at runtime (e.g., switch between speakerphone and file-transfer modes) without causing a full USB re-enumeration that would disrupt the macOS audio session.

**Files:** `scripts/06_usb_gadget_test.sh`

- [ ] 6.1 Document the current USB gadget layout:

```bash
adb shell ls -laR /sys/kernel/config/usb_gadget/ > ./validation_results/06_usb/gadget_full_tree.txt
adb shell cat /sys/kernel/config/usb_gadget/*/idVendor
adb shell cat /sys/kernel/config/usb_gadget/*/idProduct
adb shell cat /sys/kernel/config/usb_gadget/*/strings/0x409/manufacturer
adb shell cat /sys/kernel/config/usb_gadget/*/strings/0x409/product
adb shell cat /sys/kernel/config/usb_gadget/*/configs/*/strings/0x409/configuration
```

- [ ] 6.2 Create and push the gadget test script:

```bash
cat > /tmp/06_usb_gadget_test.sh << 'SCRIPT'
#!/bin/sh
# USB gadget reconfiguration test
# SAFETY: We only READ sysfs state and attempt soft config changes.
# If the gadget breaks, a reboot restores it.
OUT="/data/waved_validation/results/06_usb"
mkdir -p "$OUT"

GADGET_BASE=$(ls -d /sys/kernel/config/usb_gadget/* 2>/dev/null | head -1)
if [ -z "$GADGET_BASE" ]; then
    echo "ERROR: No USB gadget found in configfs" > "$OUT/error.txt"
    exit 1
fi

echo "=== Phase A: Current gadget configuration ==="
echo "Gadget base: $GADGET_BASE" > "$OUT/gadget_info.txt"
cat "$GADGET_BASE/UDC" >> "$OUT/gadget_info.txt" 2>&1
cat "$GADGET_BASE/idVendor" >> "$OUT/gadget_info.txt" 2>&1
cat "$GADGET_BASE/idProduct" >> "$OUT/gadget_info.txt" 2>&1

# List all functions
echo "=== Functions ===" >> "$OUT/gadget_info.txt"
ls -la "$GADGET_BASE/functions/" >> "$OUT/gadget_info.txt" 2>&1

# List all configs and their linked functions
echo "=== Configs ===" >> "$OUT/gadget_info.txt"
for CONFIG in "$GADGET_BASE"/configs/*/; do
    echo "Config: $CONFIG" >> "$OUT/gadget_info.txt"
    ls -la "$CONFIG" >> "$OUT/gadget_info.txt" 2>&1
done

echo "=== Phase B: Check if gadget supports runtime config switch ==="
# The stock firmware switches USB modes via system_server_swt_usb_mode()
# which likely does: unbind UDC → reconfigure → bind UDC
# Test: can we read/modify configs without unbinding?

# Count available configs
NUM_CONFIGS=$(ls -d "$GADGET_BASE"/configs/*/ 2>/dev/null | wc -l)
echo "Number of configs: $NUM_CONFIGS" > "$OUT/config_count.txt"

# List functions in each config
for CONFIG in "$GADGET_BASE"/configs/*/; do
    CONFIG_NAME=$(basename "$CONFIG")
    echo "=== $CONFIG_NAME ===" >> "$OUT/config_functions.txt"
    ls -la "$CONFIG" 2>/dev/null | grep "^l" >> "$OUT/config_functions.txt"
done

echo "=== Phase C: Test UDC unbind/rebind timing ==="
# This tests how fast we can reconfigure the gadget
# WARNING: This will briefly disconnect USB from the host
UDC=$(cat "$GADGET_BASE/UDC" 2>/dev/null)
echo "Current UDC: $UDC" > "$OUT/udc_test.txt"

if [ -n "$UDC" ]; then
    echo "Unbinding UDC..."
    START=$(date +%s%N 2>/dev/null || date +%s)
    echo "" > "$GADGET_BASE/UDC"
    sleep 0.5
    echo "Rebinding UDC..."
    echo "$UDC" > "$GADGET_BASE/UDC"
    END=$(date +%s%N 2>/dev/null || date +%s)
    echo "UDC rebind complete" >> "$OUT/udc_test.txt"
    echo "Start: $START, End: $END" >> "$OUT/udc_test.txt"
else
    echo "No UDC bound — cannot test rebind" >> "$OUT/udc_test.txt"
fi

echo "=== Phase D: Examine S89usbgadget for mode switch logic ==="
# Copy the init script for reference
cat /etc/init.d/S89usbgadget > "$OUT/S89usbgadget.txt" 2>&1

echo "USB gadget test complete. Results in $OUT"
ls -la "$OUT/"
SCRIPT

adb push /tmp/06_usb_gadget_test.sh /data/waved_validation/06_usb_gadget_test.sh
adb shell chmod +x /data/waved_validation/06_usb_gadget_test.sh
```

- [ ] 6.3 Run the test (note: Phase C will briefly disconnect USB — ADB session may drop):

```bash
# Create results dir
adb shell mkdir -p /data/waved_validation/results/06_usb

# Run phases A and B first (non-destructive)
adb shell /data/waved_validation/06_usb_gadget_test.sh

# If ADB drops during Phase C, re-enable:
python3 enable_adb.py && sleep 3 && adb devices
```

- [ ] 6.4 Pull results:

```bash
adb pull /data/waved_validation/results/06_usb/ ./validation_results/06_usb/
```

**Pass criteria:** The gadget supports multiple configs in configfs. UDC unbind/rebind completes in under 1 second. The S89usbgadget script reveals the mode-switching mechanism. macOS re-enumerates the device after rebind (this is expected — the question is how fast).

**Fail criteria:** Gadget reconfiguration crashes the kernel or takes more than 5 seconds.

**Key finding to document:** Does mode switching require full UDC unbind/rebind (which causes macOS re-enumeration), or can function symlinks be changed while bound? The answer determines whether `waved` can switch between speakerphone and file-transfer modes seamlessly.

---

## Task 7: Audio Frame Capture — TEA SDK 3A Output Validation

Validate the TEA SDK's audio output callback mechanism. Capture processed audio frames and verify format (sample rate, bit depth, channels). This confirms the `audio_srv` → `meeting_srv` data path.

**Files:** `scripts/07_audio_frame_capture.sh`

- [ ] 7.1 Enable audio dump flags (stock firmware feature):

```bash
# These magic files enable audio dumping in the stock firmware
adb shell touch /tmp/AUDIO_VERBOSE
adb shell touch /tmp/DUMP_ASR_TRANS_AUDIO
adb shell touch /data/DOA_ENABLE

# Give pspapp a moment to pick up the flags
sleep 5

# Check if dump directories are created
adb shell ls -la /mnt/udisk/audio_dump/ 2>/dev/null
adb shell ls -la /mnt/udisk/asr_dump/ 2>/dev/null
```

- [ ] 7.2 Generate audio (speak near the device for 10 seconds):

```bash
# Monitor logs while speaking
adb shell tail -f /opt/logs/psp.log | grep -i -E "audio|dump|doa|3a" &
# Wait 10 seconds while speaking...
sleep 10
kill %1 2>/dev/null
```

- [ ] 7.3 Capture and analyze the dumped audio:

```bash
cat > /tmp/07_audio_frame_capture.sh << 'SCRIPT'
#!/bin/sh
OUT="/data/waved_validation/results/07_audio"
mkdir -p "$OUT"

echo "=== Phase A: List dump files ==="
ls -laR /mnt/udisk/audio_dump/ > "$OUT/audio_dump_listing.txt" 2>&1
ls -laR /mnt/udisk/asr_dump/ > "$OUT/asr_dump_listing.txt" 2>&1
ls -laR /userdata/roomix/dump/ > "$OUT/roomix_dump_listing.txt" 2>&1

echo "=== Phase B: Copy dump files (first 1MB of each) ==="
for DUMP in /mnt/udisk/audio_dump/*; do
    if [ -f "$DUMP" ]; then
        BASENAME=$(basename "$DUMP")
        dd if="$DUMP" of="$OUT/audio_$BASENAME" bs=1M count=1 2>/dev/null
        echo "Copied: $BASENAME ($(wc -c < "$DUMP") bytes total)" >> "$OUT/dump_sizes.txt"
    fi
done

for DUMP in /mnt/udisk/asr_dump/*; do
    if [ -f "$DUMP" ]; then
        BASENAME=$(basename "$DUMP")
        dd if="$DUMP" of="$OUT/asr_$BASENAME" bs=1M count=1 2>/dev/null
        echo "Copied: $BASENAME ($(wc -c < "$DUMP") bytes total)" >> "$OUT/dump_sizes.txt"
    fi
done

echo "=== Phase C: Analyze ALSA device parameters ==="
# Check what formats the ALSA devices support
for CARD in 0 1 2; do
    for DEV in 0 1; do
        echo "--- Card $CARD, Device $DEV ---" >> "$OUT/alsa_params.txt"
        cat /proc/asound/card${CARD}/pcm${DEV}p/sub0/hw_params >> "$OUT/alsa_params.txt" 2>&1
        cat /proc/asound/card${CARD}/pcm${DEV}c/sub0/hw_params >> "$OUT/alsa_params.txt" 2>&1
    done
done

echo "=== Phase D: Check TEA SDK output format from logs ==="
grep -i -E "3a|output|callback|frame|sample|rate|channel|bit" /opt/logs/psp.log | tail -50 > "$OUT/psp_log_audio.txt" 2>&1

echo "=== Phase E: Direct ALSA capture test ==="
# Capture from the USB UAC2 device (typically card 1)
# 48kHz, 16-bit, stereo — the expected 3A output format
arecord -D hw:0,0 -f S16_LE -r 48000 -c 2 -d 5 "$OUT/direct_capture_48k_s16_2ch.wav" 2>"$OUT/arecord_stderr.txt"
echo "arecord exit code: $?" >> "$OUT/arecord_result.txt"

# Also try 32kHz mono (ASR output format)
arecord -D hw:0,0 -f S16_LE -r 32000 -c 1 -d 5 "$OUT/direct_capture_32k_s16_1ch.wav" 2>"$OUT/arecord_32k_stderr.txt"
echo "arecord 32k exit code: $?" >> "$OUT/arecord_result.txt"

echo "=== Phase F: Check DOA output ==="
grep -i "doa" /opt/logs/psp.log | tail -20 > "$OUT/doa_log.txt" 2>&1

echo "Audio frame capture complete. Results in $OUT"
ls -la "$OUT/"
SCRIPT

adb push /tmp/07_audio_frame_capture.sh /data/waved_validation/07_audio_frame_capture.sh
adb shell chmod +x /data/waved_validation/07_audio_frame_capture.sh
adb shell /data/waved_validation/07_audio_frame_capture.sh
```

- [ ] 7.4 Pull and analyze on host:

```bash
adb pull /data/waved_validation/results/07_audio/ ./validation_results/07_audio/

# Check the captured WAV file
file ./validation_results/07_audio/direct_capture_48k_s16_2ch.wav
# Should show: RIFF (little-endian) data, WAVE audio, Microsoft PCM, 16 bit, stereo 48000 Hz

# Play to verify it contains actual speech (not silence)
afplay ./validation_results/07_audio/direct_capture_48k_s16_2ch.wav

# Check raw dump files — these are PCM, need to know format to play:
# sox -r 48000 -b 16 -c 2 -e signed-integer ./validation_results/07_audio/audio_*.pcm output.wav
```

- [ ] 7.5 Clean up dump flags:

```bash
adb shell rm -f /tmp/AUDIO_VERBOSE /tmp/DUMP_ASR_TRANS_AUDIO /data/DOA_ENABLE
# Clean up dump files to save device storage
adb shell rm -rf /mnt/udisk/audio_dump/ /mnt/udisk/asr_dump/
```

**Pass criteria:** Audio dump files are generated. Direct ALSA capture produces a valid WAV file with audible speech at 48kHz/16-bit/stereo. The 3A output format matches the spec expectation (48kHz/16-bit/2ch for speaker/USB, 32kHz/16-bit/1ch for ASR).

**Fail criteria:** No dump files generated (AUDIO_VERBOSE flag not supported in this firmware version). ALSA capture fails or produces silence.

**If FAIL:** Try alternative dump methods — `arecord` from different ALSA devices, or use `strace` on pspapp to find the ALSA write calls and identify the correct device/format.

---

## Task 8: Cleanup and Results Summary

Collect all results, clean up the device, and produce a go/no-go summary.

**Files:** `scripts/08_cleanup_summary.sh`

- [ ] 8.1 Collect all results to host:

```bash
adb pull /data/waved_validation/results/ ./validation_results/
```

- [ ] 8.2 Clean up device (remove test artifacts, preserve stock state):

```bash
adb shell rm -rf /data/waved_validation/
adb shell rm -f /tmp/AUDIO_VERBOSE /tmp/DUMP_ASR_TRANS_AUDIO /data/DOA_ENABLE
adb shell rm -rf /mnt/udisk/audio_dump/ /mnt/udisk/asr_dump/
```

- [ ] 8.3 Reboot to confirm stock firmware is intact:

```bash
adb shell reboot
sleep 30
# Device should boot normally with all stock features working
```

- [ ] 8.4 Write the go/no-go decision document based on results:

```
Create: validation_results/GO_NO_GO_SUMMARY.md

Contents:
| Gate | Result | Notes |
|------|--------|-------|
| TEA SDK license removal | PASS/FAIL | [details] |
| TEA SDK config override | PASS/FAIL | [details, fallback used?] |
| UART frame format | PASS/FAIL | [byte offsets confirmed/corrected] |
| USB HID reports | PASS/FAIL | [report format confirmed/corrected] |
| USB gadget reconfig | PASS/FAIL | [re-enumeration behavior] |
| Audio frame capture | PASS/FAIL | [format confirmed: rate/bits/channels] |

Overall: GO / NO-GO for Phase 1
Blockers: [list any]
Corrections needed: [list byte offset fixes, format changes, etc.]
```

**Pass criteria for overall GO:** Tasks 2 (TEA license) and 7 (audio capture) must pass. Tasks 3-6 may have fallbacks.

**If overall NO-GO:** The primary blocker is Task 2. If TEA SDK requires the license module AND the license module is incompatible with non-pspapp callers, Phase 1 must be redesigned around open-source audio (pull Phase 2 forward).

---

## Decision Tree Summary

```
Task 2: tea_license.ko removal
  ├── Audio still works → GO (best case)
  ├── Audio breaks, but module can stay loaded → GO (keep module loaded)
  └── Audio breaks AND module blocks non-pspapp callers → NO-GO for Phase 1
       └── Fallback: Phase 2 open-source audio becomes Phase 1

Task 3: TEA config override
  ├── External JSON path found → Use it
  └── No external path → Binary-patch libTEAProcessSdk.so (acceptable)

Task 4: UART protocol
  ├── Frames match spec → Confirm byte offsets in waved-uart
  └── Frames differ → Update waved-uart parser (not a blocker)

Task 5: USB HID
  ├── Reports work as expected → Confirm format in waved-hid
  └── Reports differ → Update report format (not a blocker)

Task 6: USB gadget
  ├── Runtime switch without re-enum → Seamless mode switching
  └── Re-enumeration required → Accept brief USB disconnect on mode switch

Task 7: Audio capture
  ├── 48kHz/16bit/stereo confirmed → Proceed with Opus encoding plan
  └── Different format → Adjust meeting_srv encoder parameters
```
