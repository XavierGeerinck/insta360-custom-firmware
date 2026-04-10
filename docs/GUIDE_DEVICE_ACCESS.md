# Insta360 Wave — Device Access Guide

How to gain root access to the Insta360 Wave without opening the device.

---

## Quick Reference

| Method | Access Level | Requirements | Persistence | Status |
|---|---|---|---|---|
| USB ADB via HID `0xC0` | Root shell | USB-C + sudo + `pyusb` | Lost on reboot | Works on fresh boot, **can silently fail on some firmware states** (see Troubleshooting) |
| Factory devmode via `/data/enter_factory_mode` sentinel | Root shell | Pre-existing root shell (UART or momentary ADB) | **Persistent** across reboots | Preferred long-term access method. Non-destructive. |
| Factory devmode via HID `0xEE` | Root shell | USB-C + HID packet | Persistent | ⚠️ **DESTRUCTIVE — wipes `/data`, `/mnt/udisk`, cloud binding, region.** See the warning in "Other HID Commands". |
| Telnet (port 23) | Root shell | Network + password `123456` | Disabled since v1.1.6.6 | Not started by any init script in current firmware |
| Serial UART (`ttyS0`) | Root shell | Physical disassembly to reach pads | Always on | **Confirmed**: `/etc/inittab` respawns `/bin/sh` on `ttyS0` at 115200 8N1. Amlogic `earlycon=aml-uart,0xfe07a000`. |
| ZeroMQ (port 5533) | Read-only monitoring | Network | — | Open but publish-only; cannot inject |
| USB File Transfer | File access only (`/mnt/udisk` only) | USB-C + touchscreen | — | Working (limited) |

**Verified firmware**: `PSP_1.1.6.6` (build `2025-1128-1949`), kernel `Linux buildroot 5.4.242-ab575 aarch64`, bootloader `01.01.251002.224553`, hardware `amlogic`, `androidboot.slot_suffix=_b` currently active.

---

## Method 1: USB ADB (first-time enable via HID `0xC0`)

The historical primary access method: send a USB HID SET_REPORT with `byte[1]=0xF0, byte[5]=0xC0` to the vendor interface (Report ID 3). This triggers `system_server_swt_usb_mode(5)` on the device, which runs `/etc/init.d/S89usbgadget uac_adb`, which reconfigures the USB gadget to expose `ffs.adb` and starts `/usr/bin/adbd`.

**Important caveats before you start:**

- **macOS `IOHIDDeviceSetReport` silently drops data** on this device — the `hidapi` library reports write success but the bytes never reach the device. You **must** use `pyusb` with `sudo` and kernel-driver detach. The repo ships a working implementation at `waved/scripts/enable_adb.py`.
- The ADB mode from `0xC0` is **temporary** — lost on reboot or on any other USB mode switch. For persistent access, set factory devmode via `/data/enter_factory_mode` sentinel (see "Entering Factory Mode Safely" below) after you get a one-time ADB session.
- On some firmware states `0xC0` reaches `pspapp` cleanly but `usb_mode_set(5)` does not visibly reconfigure the gadget. See "ADB Device Not Showing" in Troubleshooting.
- **Never** send `byte[5] = 0xEE` to set factory devmode. See "Other HID Commands" for the destructive call chain.

### Prerequisites

```bash
# macOS
brew install android-platform-tools          # adb
pip3 install --break-system-packages pyusb   # required (not hidapi — see caveats above)
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

Use the script in this repo:

```bash
sudo python3 waved/scripts/enable_adb.py
```

The script uses `pyusb` + kernel-driver detach to do a raw USB `SET_REPORT` control transfer, which bypasses the macOS HID stack. It tries four variants (Output/Feature × with/without Report ID in data). **Only the 64-byte Output report with Report ID `0x03` at byte[0] can possibly parse** on the device side — the other three variants are guaranteed to be rejected by `change_carray_to_hid_vendor_package` (`libins_entry.so:159752`), which requires both `length == 0x40` and `buf[0] == 0x03`. The script sends all four for historical reasons; the one that matters is variant 1.

**The 64-byte HID Output Report it sends** (Report ID in data):

```
byte[0] = 0x03   Report ID 3 (vendor usage page 0xff00)
byte[1] = 0xF0   HID_MSG_TYPE_OF_SWT_FACTORY (dispatch key → local_b4)
byte[2] = 0x00   strict check in 0xf0 branch
byte[3] = 0x00   (device overwrites to 0xFE as a side effect — not checked)
byte[4] = 0xF0   signed -0x10 check
byte[5] = 0xC0   signed -0x40 = ADB enable sub-command
byte[6..63] = 0x00
```

**Do not copy-paste your own `hidapi`-based version.** `hidapi` on macOS uses `IOHIDDeviceSetReport` which returns success but does not deliver the bytes to this device. This was verified experimentally — the script includes a `hidapi` fallback only as a last resort and it does not work on current macOS.

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

The `0xF0` HID message type supports three sub-commands via `byte[5]`. **Only `0xC0` is safe to send casually** — the other two have destructive side effects. All three are dispatched from `recv_hid_data` in `libins_entry.so:161903`.

| `byte[5]` | Action | Internal path | Safety |
|---|---|---|---|
| `0xC0` (-0x40) | **ADB enable** | `system_server_swt_usb_mode(5)` → system_server event `0xd` → `usb_mode_set(5)` → `/etc/init.d/S89usbgadget uac_adb` | **Safe.** Temporary (lost on reboot). Can silently fail on some firmware states — see Troubleshooting. |
| `0xE0` (-0x20) | User devmode | system_server event `6`, data=1 → `system_setdevmode(1)` → writes `unifykeys[3] = 0` | **Safe.** Clears factory devmode. Disables persistent ADB. |
| `0xEE` (-0x12) | Factory devmode | system_server event `6`, data=0 → `system_setdevmode(0)` → writes `unifykeys[3] = 0x9257` + `gui_set_debug_switch(1)` + **dispatches system_server event `2`** | **DESTRUCTIVE.** See warning below. |

> ### ⚠️ DO NOT SEND `byte[5] = 0xEE` unless you have accepted the consequences
>
> The `0xEE` sub-command is documented in the old reverse-engineering notes as "Factory mode" and it is — but `system_setdevmode(0)` does not just flip a devmode flag. After writing the persistent magic to `unifykeys[3]`, it synchronously dispatches **system_server event 2**, which is handled by `system_storage_reset()` at `libins_entry.so:92960`. Despite the name, `system_storage_reset` is a **full factory reset**:
>
> 1. `clear_mask_point_of_device_activation`
> 2. `device_bind_of_force_unbind_and_clear_userdata` — unbinds the Insta360 cloud account
> 3. `rm -rf /opt/logs/*`
> 4. `system_udisk_reset()` — `rm -rf /mnt/udisk/*` (**wipes all meeting recordings**)
> 5. `system_udisk_format()` — `mkfs.exfat` on `/dev/udisk`
> 6. `rm -rf /data/common_settings/asr/speaker_named/*` and `hotword*`
> 7. `unifykeys_write(dev, 6, 0, 1)` — **zeros `unifykeys[6]` `region_code`**, which is why the device then asks for region selection on next boot
> 8. `system_cleardevpassword()`
> 9. `reset_system_config()`
> 10. Chain-calls `system_factory_reset()` — `rm -rf /data/*` (except overlay), `rm -rf /data/overlay/upper/*` (except `/opt`), `system_reboot()`
>
> **Verified the hard way on 2026-04-08** during a HID protocol probe that thought `0xEE` was a benign devmode flag set. Do not repeat this. If you want persistent factory-mode ADB without losing user data, use the `/data/enter_factory_mode` sentinel file path (see "Entering Factory Mode Safely" below) — it writes the same `unifykeys[3] = 0x9257` magic without calling the reset chain.

To restore user (non-factory) devmode:

```python
pkt[5] = 0xE0  # User devmode. Clears unifykeys[3] factory magic.
               # Safe — does NOT trigger the reset chain.
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

If `adb devices` shows empty after sending the `0xC0` HID command:

1. Check USB connection: `ioreg -p IOUSB -l | grep "psp"`
2. Restart ADB server: `adb kill-server && adb start-server`
3. Make sure the device screen is awake and on the home screen (not File Transfer, not Settings menu). `pspapp`'s HID handling can stall while the display sleeps.
4. Close Wave Controller.app on macOS if running — it claims the vendor HID interface.
5. Run with sudo and let `enable_adb.py` use the pyusb path with kernel driver detach. macOS `IOHIDDeviceSetReport` can silently drop SET_REPORT data; libusb bypasses that.
6. Power-cycle the Wave if none of the above work (hold power ~8s), then retry.
7. **If even a fresh boot doesn't respond to `0xC0`**: the dispatch chain is reaching `pspapp` but `usb_mode_set(5)` is not producing a gadget reconfigure. This has been observed on `PSP_1.1.6.6` despite the decompilation saying it should work. In this case use the `/data/enter_factory_mode` sentinel path below (requires one-time root shell via UART or by surviving the `0xEE` factory-reset chain) — once in factory mode, `pspapp` calls `S89usbgadget uac_adb` at boot, which reliably works.

You can verify whether `pspapp` is receiving your HID packets at all with `waved/scripts/diag_hid.py` (no longer sends `0xEE`): it sends a type-3 PC reset and a type-5 sync-pack and listens for the expected type-4/type-6 responses on EP 0x82. If those respond but `0xC0` doesn't re-enumerate, the break is specifically in `system_server`'s `case 0xd` handler or the `usb_mode_set(5)` mechanism — not in the USB→pspapp delivery chain.

### HID Device Not Found

If `hid.enumerate(0x18F0, 0x4E40)` returns empty:

1. Make sure the device isn't in File Transfer mode (exit via touchscreen)
2. Unplug and replug the USB-C cable
3. Check if another app has claimed the HID interface (close Wave Controller)
4. If the device is currently in `uac_adb` mode (ADB already enabled), the HID function is **not** in the active gadget config. In that case `hid.enumerate` will return nothing for the vendor usage page even though ADB works. This is expected.

### ADB Access Lost After Reboot

The `0xC0` (temporary) ADB is **not persistent** — each boot puts the gadget back in the `start` configuration which does not enable ADB. Options:

1. **Re-send `0xC0` after each reboot** — same flow as first-time enable.
2. **Enter factory devmode via the `/data/enter_factory_mode` sentinel** (non-destructive) — see "Entering Factory Mode Safely" below. `pspapp` will then call `S89usbgadget uac_adb` at every boot. Requires one-time root shell to create the sentinel file.
3. **Modify `/etc/init.d/S89usbgadget` via the writable `/etc` overlay** (independent of factory mode and `pspapp`) — see "Making ADB Survive a pspapp Failure" below.
4. Enable telnet/SSH as a secondary access path before your primary fails.

### Entering Factory Mode Safely (without the 0xEE reset chain)

`system_devmode_init` (libins_entry.so:95120) runs at every boot and checks `/data/enter_factory_mode`. If the file exists:

1. It writes `0x9257` to `unifykeys[3]`
2. Sets the runtime devmode cache (`database[0xec] = 0`)
3. Removes the sentinel file (one-shot)

This is the **same persistent state** that the `0xEE` HID command produces, without calling `system_setdevmode(0)` or the downstream reset chain. Steps:

```bash
# You need a root shell via ANY means first (UART, momentary 0xC0 ADB, etc.)
adb shell 'touch /data/enter_factory_mode && sync && reboot'
```

On next boot the device comes up in factory devmode. `pspapp` will call `S89usbgadget uac_adb` during its USB init, and `adb devices` should show the device automatically without any HID poking. To verify after boot:

```bash
adb shell "echo -n devmode > /sys/class/unifykeys/name && cat /sys/class/unifykeys/read | xxd -p"
# Expect: 57920000 (little-endian 0x9257)
```

To **leave** factory devmode:

```bash
adb shell 'echo -n devmode > /sys/class/unifykeys/name && printf "\x00\x00\x00\x00" > /sys/class/unifykeys/write && sync && reboot'
```

This is equivalent to the `0xE0` HID user-devmode command but does not go through `system_setdevmode(1)` at all.

### Making ADB Survive a `pspapp` Failure

Since `adbd` depends on `pspapp` detecting factory devmode and explicitly calling `S89usbgadget uac_adb`, any `pspapp` crash/hang on boot kills your ADB access even when `unifykeys[3] == 0x9257`. For a hardening plan that does NOT depend on `pspapp`:

`/etc` is writable via overlayfs from `/data/overlay/upper/etc`. You can modify `/etc/init.d/S89usbgadget` so the `start` branch enables ADB directly:

```bash
adb shell '
# The start branch in start_usbgadget() has adbd_enable commented out.
# Enable it by editing /etc/init.d/S89usbgadget (which lives on the overlay).
sed -i "s|^			#adbd_enable|			adbd_enable|" /etc/init.d/S89usbgadget
# Verify the change
grep -n "adbd_enable" /etc/init.d/S89usbgadget | head -6
sync
'
```

After this change, even `start` mode enables ADB, so you get ADB at every boot regardless of `pspapp` state. **Keep a copy of the original script before editing** (`cp /etc/init.d/S89usbgadget /etc/init.d/S89usbgadget.orig`) — if your edit breaks boot, you'll need UART to recover.

Note: this method does not survive a firmware OTA (SWUpdate will overwrite the rootfs, and the overlay will be re-applied to the new base).

---

## Technical Details

### How the ADB Enable Works (full chain)

From decompiled `libins_entry.so` and verified against a running device:

1. Host sends USB HID SET_REPORT (64 bytes, Report ID 0x03) on interface 3 (the HID interface) via EP0 control transfer. There is **no OUT interrupt endpoint** on the HID interface — SET_REPORT is the only host→device path.
2. Linux f_hid gadget on the device places the data phase into a FIFO readable from `/dev/hidg0`.
3. `hid_manager_server` (separate userspace process, PID distinct from pspapp) reads `/dev/hidg0`, prepends a **4-byte framing header** where `header[3]` = length of the HID body (`0x40` = 64 for vendor reports, `0x02` for telephone/consumer status), and publishes on `tcp://*:5544` via ZeroMQ.
4. `pspapp::thread_of_handle_hid_recv_data` (libins_entry.so:164730) subscribes to `tcp://127.0.0.1:5544`. It drops any frame that is not exactly `0x44` = 68 bytes (4-byte header + 64-byte body), logging `"Failed recv buf_size:%d != %d"`.
5. `hid_transfer_message_handler(frame)` (libins_entry.so:164364) dispatches on `frame[3]`:
   - `0x02` → Ring/Meeting/Mute status from Wave Controller (byte[4] selects sub-type)
   - `0x40` → vendor HID payload; if `frame[5]` is `0xEC` or `0xED` it's a heartbeat (`send_hb_response_to_hid`), otherwise it calls `recv_hid_data(frame + 4, 0x40)` passing the 64-byte HID body
   - anything else → logged as `"unknow hid context!!!"` and dropped
6. `recv_hid_data(buf, 0x40)` (libins_entry.so:161724) calls `change_carray_to_hid_vendor_package(buf, 0x40, out)`:
   - Requires `length == 0x40` (exactly 64 bytes)
   - Requires `buf[0] == 0x03` (Report ID literal in the data buffer — without this the parser returns 0 and the packet is silently dropped)
   - Sets `local_b4 = buf[1]` (this is the message-type dispatch key)
   - Copies bytes 2..9, 10, and 11..62 into the parsed struct
7. The `local_b4` dispatch has many cases (0, 1, 2, 3=reset, 4=reset-ack, 5=sync-pack, 6=sync-resp, 0xef=heartbeat, **0xf0=factory/SWT**). For `0xf0`:
   - Requires `buf[2] == 0x00` (strict)
   - Writes `buf[3] = 0xFE` as a side effect (not checked)
   - Requires `buf[4] == 0xF0`
   - Dispatches on `buf[5]`: `0xEE` → factory devmode (destructive, see warning above), `0xE0` → user devmode, `0xC0` → `system_server_swt_usb_mode(5)`
8. `system_server_swt_usb_mode(5)` (libins_entry.so:92251) sends IPC event `0x0d` with data=5 to `system_server`.
9. `system_server_request` case `0xd` (libins_entry.so:95794) looks up the USB device with `device_find(&DAT_002d3ee0)`, reads the current mode, and if different from the requested mode, calls `usb_mode_set(dev, 5)`.
10. `usb_mode_set`/`thread_usb_mode_set` runs `/etc/init.d/S89usbgadget uac_adb` which is a shell script that: disables all current gadget functions, enables `uac2.0` and `ffs.adb`, starts `/usr/bin/adbd &`, and re-attaches the gadget to the UDC (`fdd00000.crgudc2`).
11. The device re-enumerates with `ffs.adb` + `uac2.0` active. `adbd` listens on the ADB endpoint.

### USB Gadget Modes

The USB gadget is driven by `/etc/init.d/S89usbgadget` (a shell script) using Linux configfs under `/sys/kernel/config/usb_gadget/amlogic/`. The script accepts a **mode name** as its first argument, not a numeric mode. `pspapp` calls it via `system()` when it wants to switch modes.

Available functions that can be enabled (symlinked into `configs/amlogic.1/`):

| configfs function | Purpose |
|---|---|
| `uac2.0` | USB Audio Class 2.0 (microphone/speaker) |
| `hid.0` | HID vendor+telephone+consumer (multi-collection, Report IDs 1/2/3) |
| `mass_storage.usb0` | Mass storage exposing `/dev/udisk` (the `/mnt/udisk` partition) |
| `ffs.adb` | functionfs ADB interface; requires `/usr/bin/adbd` running |
| `acm.0` | CDC-ACM serial (rarely used in production) |

Mode names accepted by `S89usbgadget <mode>`:

| Mode arg | Functions enabled | Notes |
|---|---|---|
| `start` | `uac2.0` + `hid.0` | Default on boot. `adbd_enable` is **commented out** in the `start` branch, so ADB is NOT enabled by default. |
| `uac_adb` | `uac2.0` + `ffs.adb` | ADB mode. `adbd` process is started. |
| `uac_hid` | `uac2.0` + `hid.0` | Same as `start` minus `mass_storage`. |
| `uac_msc` | `mass_storage.usb0` | File Transfer touchscreen option. |
| `uac_hid_adb` | `hid.0` + `ffs.adb` | HID + ADB, no audio. |
| `uac_hid_msc` | `hid.0` + `mass_storage.usb0` | HID + File Transfer. |
| `uac_adb_msc` | `ffs.adb` + `mass_storage.usb0` | ADB + File Transfer. |
| `adb_cdc` | `ffs.adb` + `acm.0` | ADB + CDC-ACM. |
| `uac_cdc` | `uac2.0` + `acm.0` | |
| `cdc_msc` | `acm.0` + `mass_storage.usb0` | |
| `restart` | `ffs.adb` + `hid.0` | Emergency ADB restart. |
| `stop` | (none) | Detaches gadget from UDC. |

**Key observation**: `pspapp`'s `usb_mode_set(N)` maps numeric modes to these shell commands. Mode 5 runs `S89usbgadget uac_adb` (confirmed from psp.log: `[CLIENT_OTA]usb mode set cmd: /etc/init.d/S89usbgadget  uac_adb`). The ADB mode is NOT a special gadget state — it's just the userspace script being called with a particular argument.

**Second key observation**: Since `adbd_enable` is commented out in the `start` branch, the device boots **without** ADB even in factory devmode. It's `pspapp` that, detecting factory devmode at startup, calls `S89usbgadget uac_adb` explicitly to switch modes. If `pspapp` ever fails to do that call (crash, hang, race), factory mode alone doesn't give you ADB. For a hardening plan that doesn't depend on `pspapp`'s good behavior, see "Making ADB Survive a pspapp Failure" below.

### The `/etc/debug_mode` sentinel (unused)

`S89usbgadget`'s `start_usbgadget()` function checks `/etc/debug_mode` but both branches are currently identical. Chinese comment in the script says `uac 常驻 其他选2个使能` ("uac is resident, others select 2 to enable"). This appears to be intended as a debug toggle that was never wired up. Creating `/etc/debug_mode` alone does nothing — to get ADB at boot, you still need `pspapp` to trigger the mode switch, or you need to modify the script (see overlayfs writable `/etc` below).

---

## eMMC Partition Layout Reference

Confirmed on a running device (`adb shell`, `/proc/partitions`, `ls /dev/`):

| # | Major:Minor | `/dev/` alias | Size | Filesystem | Mount | Purpose |
|---|---|---|---|---|---|---|
| 1 | 179:1 | **`/dev/bootloader`** | 4 MB | raw | — | U-Boot (also mirrored in `/dev/mmcblk0boot0`/`boot1` eMMC HW boot partitions) |
| 2 | 179:2 | (no alias — use `mknod 179 2`) | 64 MB | raw | — | Reserved |
| 4 | 179:4 | **`/dev/env`** | 8 MB | raw | — | U-Boot environment |
| 5 | 179:5 | (no alias) | 8 MB | raw | — | Env backup (likely) |
| 6 | 179:6 | (no alias) | 8 MB | raw | — | Unknown |
| 7 | 179:7 | **`/dev/misc`** | 8 MB | raw | — | A/B slot state |
| 8 | 179:8 | (no alias) | 8 MB | raw | — | Unknown |
| 9 | 179:9 | **`/dev/boot_a`** | 64 MB | `boot.img` | — | Kernel slot A |
| 10 | 179:10 | **`/dev/boot_b`** | 64 MB | `boot.img` | — | Kernel slot B (active per `androidboot.slot_suffix=_b`) |
| 11 | 179:11 | **`/dev/system_a`** | 512 MB | SquashFS | — | Rootfs slot A |
| 12 | 179:12 | **`/dev/system_b`** → `/dev/root` | 512 MB | SquashFS | `/` (ro) | Rootfs slot B (active). ~203 MB used. |
| 13 | 179:13 | **`/dev/data`** | 512 MB | ext4 | `/data` | User data + overlayfs upper dirs for `/etc`, `/opt`, `/usr` |
| 14 | 179:14 | **`/dev/model`** | 4.6 GB | ext4 (ro) | `/model` | AI models (4.0 GB used) |
| **15** | **179:15** | **`/dev/no_use`** | **64 MB** | **ext4** | **`/unifykey`** | **Hardware-backed secrets: serial, MACs, devmode magic, bindkey, etc. Name is misleading — it IS in use.** |
| 16 | 179:16 | **`/dev/udisk`** | 21 GB | exFAT | `/mnt/udisk` | User recordings + cached `Insta360_Wave_Controller` installer |
| 17 | 179:17 | **`/dev/psp_udisk_s`** | 512 MB | exFAT | `/udisk_s` | System temp files (145 MB used) |
| 18 | 179:18 | **`/dev/psp_model_s`** | 1.2 GB | ext4 | `/tmp/psp_model` | Model staging area (OTA buffer) |

Also present:
- `/dev/mmcblk0boot0` (179:32) and `/dev/mmcblk0boot1` (179:64) — eMMC hardware boot partitions, typically holding a second bootloader copy
- `/dev/mmcblk0rpmb` (240:0) — Replay Protected Memory Block (secure)
- `/dev/mmcblk0` (179:0) — whole disk (30.6 GB)

Overlay layout: `/etc`, `/opt`, and `/usr` are overlayfs mounts on top of the read-only SquashFS rootfs, with upper dirs at `/data/overlay/upper/etc`, `/data/overlay/upper/opt`, `/data/overlay/upper/usr`. Any change you make to `/etc/*` is persisted at `/data/overlay/upper/etc/*` and survives reboots (but not an OTA of the system partition, which rewrites `/dev/system_<slot>`).

### Backing Up Critical Partitions

Once you have ADB, back up at minimum `/dev/no_use` (the unifykey partition — irreplaceable hardware state) and `/dev/bootloader` + `/dev/env` + `/dev/misc`. These are small and cannot be reconstructed if lost.

```bash
for spec in \
  "p15_unifykey:/dev/no_use" \
  "p01_bootloader:/dev/bootloader" \
  "p04_env:/dev/env" \
  "p07_misc:/dev/misc" \
  "p09_boot_a:/dev/boot_a" \
  "p10_boot_b:/dev/boot_b"; do
  name="${spec%%:*}"; dev="${spec##*:}"
  adb shell "dd if=$dev of=/tmp/part.img bs=1M && md5sum /tmp/part.img"
  adb pull /tmp/part.img "$name.img"
  adb shell 'rm -f /tmp/part.img'
done
```

Also back up the active system slot (`/dev/system_b`, 512 MB) and `/dev/data` (512 MB, holds your overlay customizations and wifi/cloud state).

---

## Known Firmware Quirks and Incidents

### 2026-04-08: `0xEE` HID sub-command is destructive (not just a flag set)

During a HID protocol probe, the `0xEE` "factory mode" sub-command was sent based on the earlier assumption that it simply sets a persistent devmode flag. It does — **and also** triggers `system_storage_reset` → `system_factory_reset`, which wiped `/data`, `/mnt/udisk` (all meeting recordings), the WiFi config, the Insta360 cloud binding, `unifykeys[6]` (region code), and the device password. The device rebooted into first-run setup.

Silver lining: because `unifykeys[3] = 0x9257` is written *before* the reset chain fires, and because `/unifykey` is on a separate partition not touched by `rm -rf /data`, the device came up in factory devmode after setup and **`pspapp` automatically called `S89usbgadget uac_adb` at startup**, giving persistent ADB without any HID packet. This is useful to know as a recovery path, but the cost is unacceptable — use `/data/enter_factory_mode` sentinel instead if you want factory devmode.

Full call chain documented in "Other HID Commands" above.

### `0xC0` HID ADB enable can silently fail

On the pre-incident firmware state of the verified device (`PSP_1.1.6.6`), sending `0xC0` reached `recv_hid_data` (confirmed by differential probe with types 3/5 returning expected responses on EP 0x82) but did not produce a USB re-enumeration. `system_server_swt_usb_mode(5)` was called, but `usb_mode_set(5)` did not visibly reconfigure the gadget. Root cause not yet pinned down — likely either `device_find(&DAT_002d3ee0)` returning null or `usb_mode_get` returning a mode that equals 5 (causing the `!=` guard in `case 0xd` at libins_entry.so:95794 to skip the set). Workaround is to use the `/data/enter_factory_mode` sentinel path for persistent ADB, or to modify `/etc/init.d/S89usbgadget` via the overlayfs (see "Making ADB Survive a pspapp Failure").

### `hid_manager_server` is a separate process

`hid_manager_server` (not `pspapp`) reads `/dev/hidg0`, frames the data with a 4-byte header (`header[3] = body_length`), and publishes on ZMQ `tcp://*:5544`. If it crashes, HID commands from the host are silently dropped at the `/dev/hidg0` → ZMQ boundary. It is **not** managed by `pspapp_daemon` — check `ps | grep hid_manager_server` to confirm it's running. No auto-restart mechanism is in place.

### Factory devmode does not auto-enable ADB at boot (strictly)

Factory devmode (`unifykeys[3] = 0x9257`, `database[0xec] = 0`) does NOT by itself cause `adbd` to start at boot. The `start` branch of `S89usbgadget` has `adbd_enable` commented out. What actually enables ADB in factory mode is `pspapp`'s `thread_usb_mode_set` detecting factory mode at startup and invoking `S89usbgadget uac_adb`. This happens several seconds after `S89usbgadget start` runs. A `pspapp` hang or crash at the wrong moment leaves the device in `start` (uac+hid) with no ADB despite the devmode flag being set. For a failure-independent ADB path, edit `S89usbgadget` on the `/etc` overlay as described in "Making ADB Survive a `pspapp` Failure".

---

## Security Disclosure

These findings are documented for security research and device interoperability purposes. The root access methods described here exploit:

1. **Unauthenticated USB HID command** — No encryption, no authentication. Any software with USB HID access can send the ADB enable packet.
2. **Default root password** — `123456` across all devices and firmware versions.
3. **ADB runs as root** — No user separation, no authorization prompt on the device.

These should be reported to Insta360 for remediation.
