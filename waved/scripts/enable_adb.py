#!/usr/bin/env python3
"""Enable ADB on Insta360 Wave via USB HID (temporary, byte[5]=0xC0).

Data pipeline (verified from decompiled firmware libins_entry.so and from a
differential probe on a running PSP_1.1.6.6 device):
  Host pyusb SET_REPORT (64 bytes, Report ID 0x03 in data)
  → Linux f_hid gadget places body into /dev/hidg0 FIFO
  → hid_manager_server reads hidg0, prepends 4-byte framing header where
    header[3] = body length (0x40 for vendor), publishes on ZMQ tcp://*:5544
  → pspapp::thread_of_handle_hid_recv_data  (subscribes tcp://127.0.0.1:5544)
  → hid_transfer_message_handler                   (libins_entry.so:164364)
      - frame size MUST be 68 bytes (0x44)
      - frame[3]==0x40 → vendor HID dispatch
      - frame[5]==0xEC/0xED → heartbeat (not us)
      - else → recv_hid_data(frame+4, 0x40)
  → recv_hid_data                                  (libins_entry.so:161724)
      - calls change_carray_to_hid_vendor_package(buf, 0x40, out)
      - parser REQUIRES: length==0x40 AND buf[0]==0x03
      - local_b4 = buf[1]                          (dispatch key)
      - 0xf0 branch: buf[2]==0 && buf[4]==0xF0 && buf[5]==0xC0
  → system_server_swt_usb_mode(5)                  (libins_entry.so:92251)
  → system_server case 0xd                         (libins_entry.so:95794)
      - device_find(&DAT_002d3ee0) lookup
      - if current mode != 5: usb_mode_set(dev, 5)
  → usb_mode_set runs `/etc/init.d/S89usbgadget uac_adb`
  → gadget reconfigure: ffs.adb + uac2.0 active, /usr/bin/adbd started

CAVEATS:
  - macOS IOHIDDeviceSetReport silently drops the data phase for this device.
    This script uses pyusb (libusb) to send raw USB SET_REPORT control
    transfers, bypassing the macOS HID driver. Requires sudo on macOS.
  - Only variant 1 (Output + Report ID 0x03 in data, 64 bytes) can parse.
    The -RID (63-byte) variants are rejected by the parser's length==0x40
    check. Variants 3/4 (Feature reports) may be handled differently by the
    Linux gadget driver depending on version. Variant 1 is the canonical one.
  - On some firmware states, the packet reaches pspapp cleanly (verified via
    diag_hid.py ground-truth probes) but usb_mode_set(5) does not produce a
    visible re-enumeration. Workaround: use /data/enter_factory_mode sentinel
    file path for persistent ADB. See docs/GUIDE_DEVICE_ACCESS.md.

!!! NEVER CHANGE PKT[5] to 0xEE !!!
  byte[5]=0xEE is the "factory devmode" sub-command. It dispatches
  system_setdevmode(0) which chains into system_storage_reset →
  system_factory_reset, wiping /data, /mnt/udisk, cloud binding, and zeroing
  unifykeys[6] (region_code). Verified the hard way 2026-04-08.
  See docs/GUIDE_DEVICE_ACCESS.md → "Other HID Commands" → 0xEE warning.

Usage: sudo python3 enable_adb.py
"""

import sys
import os
import time
import subprocess

VID = 0x18F0  # Insta360
PID = 0x4E40  # Wave (psp)
REPORT_ID = 0x03

# HID packet format — verified against recv_hid_data in decompiled libins_entry.so:
#   byte[0] = 0x03  Report ID (vendor usage page 0xFF00, Report ID 3)
#   byte[1] = 0xF0  HID_MSG_TYPE_OF_SWT_FACTORY  (local_b4 == 0xF0 dispatch)
#   byte[2] = 0x00  must be 0x00  (checked: param_1[2] == '\0')
#   byte[3] = 0x00  not checked (device overwrites to 0xFE as side-effect)
#   byte[4] = 0xF0  ADB sub-command  (checked: param_1[4] == -0x10 signed)
#   byte[5] = 0xC0  ADB sub-command  (checked: param_1[5] == -0x40 signed)
#                   → system_server_swt_usb_mode(5)
#   byte[6..63] = 0x00  padding
PKT = bytearray(64)
PKT[0] = 0x03
PKT[1] = 0xF0
PKT[2] = 0x00
PKT[3] = 0x00
PKT[4] = 0xF0
PKT[5] = 0xC0

# Variant: same packet but without Report ID as first byte (63 bytes).
# Some macOS versions strip the Report ID from the data phase of SET_REPORT,
# so if we send 63 bytes of pure data, the gadget receives exactly 63 bytes.
# hid_manager_server may pad this to 64 bytes for ZMQ framing.
PKT_NO_RID = bytearray(63)
PKT_NO_RID[0] = 0xF0  # HID_MSG_TYPE_OF_SWT_FACTORY
PKT_NO_RID[1] = 0x00  # must be 0x00
PKT_NO_RID[2] = 0x00
PKT_NO_RID[3] = 0xF0  # sub-command
PKT_NO_RID[4] = 0xC0  # ADB enable


def _poll_adb(seconds=15):
    """Return True if an ADB device appears within `seconds`."""
    for _ in range(seconds):
        out = subprocess.run(["adb", "devices"], capture_output=True, text=True).stdout
        lines = [l for l in out.splitlines() if l and not l.startswith("List")]
        if lines:
            print("  ADB device found:")
            for l in lines:
                print(f"    {l}")
            return True
        time.sleep(1)
    return False


def _restart_adb():
    subprocess.run(["adb", "kill-server"], capture_output=True)
    time.sleep(1)
    subprocess.run(["adb", "start-server"], capture_output=True)


def _send_pyusb(detach_driver=True):
    """Send SET_REPORT via pyusb/libusb raw control transfer."""
    try:
        import usb.core
    except ImportError:
        print("  pyusb not installed: pip3 install --break-system-packages pyusb")
        return False

    dev = usb.core.find(idVendor=VID, idProduct=PID)
    if dev is None:
        print("  USB device not found")
        return False

    print(f"  Found: {dev.manufacturer} {dev.product} (SN: {dev.serial_number})")

    # Find HID interface
    hid_intf = None
    for cfg in dev:
        for intf in cfg:
            if intf.bInterfaceClass == 3:
                hid_intf = intf.bInterfaceNumber
                break
    if hid_intf is None:
        print("  No HID interface found")
        return False
    print(f"  HID interface: {hid_intf}")

    if detach_driver:
        try:
            if dev.is_kernel_driver_active(hid_intf):
                print("  Detaching kernel driver...")
                dev.detach_kernel_driver(hid_intf)
        except Exception as e:
            print(f"  Cannot detach kernel driver: {e}")
            if "Access denied" in str(e):
                print("  → Run with sudo: sudo python3 enable_adb.py")
                return False

    BM_REQ = 0x21   # Host-to-Device, Class, Interface
    SET_REPORT = 0x09

    # Try multiple variants:
    # 1. Output report with Report ID in data (64 bytes)
    # 2. Output report without Report ID in data (63 bytes)
    # 3. Feature report with Report ID (64 bytes)
    # 4. Feature report without Report ID (63 bytes)
    variants = [
        ("Output+RID(64B)", (0x02 << 8) | REPORT_ID, bytes(PKT)),
        ("Output-RID(63B)", (0x02 << 8) | REPORT_ID, bytes(PKT_NO_RID)),
        ("Feature+RID(64B)", (0x03 << 8) | REPORT_ID, bytes(PKT)),
        ("Feature-RID(63B)", (0x03 << 8) | REPORT_ID, bytes(PKT_NO_RID)),
    ]

    any_sent = False
    for desc, wvalue, data in variants:
        try:
            print(f"  [{desc}] wValue=0x{wvalue:04X} wIndex={hid_intf} len={len(data)}...")
            n = dev.ctrl_transfer(BM_REQ, SET_REPORT, wvalue, hid_intf, data, timeout=2000)
            print(f"    → sent {n} bytes")
            if n > 0:
                any_sent = True
                # Quick check if device disappeared
                time.sleep(0.5)
                if usb.core.find(idVendor=VID, idProduct=PID) is None:
                    print("    ✓ Device disappeared — gadget reconfiguring!")
                    return True
        except Exception as e:
            print(f"    → error: {e}")

    # Reattach driver if we detached
    if detach_driver:
        try:
            dev.attach_kernel_driver(hid_intf)
        except Exception:
            pass

    return any_sent


def _send_hidapi():
    """Fallback: try via hidapi (unreliable on macOS but harmless to try)."""
    try:
        import hid
    except ImportError:
        return False

    devs = hid.enumerate(VID, PID)
    target = next((d for d in devs if d.get("usage_page") == 0xFF00), None)
    if not target:
        return False

    h = hid.device()
    try:
        h.open_path(target["path"])
    except OSError:
        return False

    h.set_nonblocking(1)
    while h.read(64):
        pass

    n = h.write(bytes(PKT))
    h.close()
    return n > 0


# ── main ──────────────────────────────────────────────────────────────────────
print(f"Insta360 Wave ADB Enabler")
print(f"VID=0x{VID:04X} PID=0x{PID:04X}")
print("=" * 50)

if os.geteuid() != 0:
    print("\nWARNING: Not running as root.")
    print("macOS IOHIDDeviceSetReport does NOT reliably deliver data to this device.")
    print("For reliable operation: sudo python3 enable_adb.py\n")

# Method 1: pyusb with kernel driver detach (most reliable, needs sudo)
print("\n[1] pyusb — raw USB SET_REPORT (with kernel driver detach)...")
sent = _send_pyusb(detach_driver=True)

# Method 2: pyusb without detach (in case detach disrupts USB state)
if not sent:
    print("\n[2] pyusb — raw USB SET_REPORT (without kernel driver detach)...")
    sent = _send_pyusb(detach_driver=False)

# Method 3: hidapi fallback
if not sent:
    print("\n[3] hidapi fallback...")
    if _send_hidapi():
        sent = True
        print("  hidapi write accepted (may not work on macOS)")

if not sent:
    print("\nERROR: All methods failed.")
    print("  1. Run with sudo: sudo python3 enable_adb.py")
    print("  2. Unplug/replug USB-C, wait 3s, re-run")
    print("  3. Close Wave Controller if running")
    sys.exit(1)

# ── wait for USB re-enumeration ───────────────────────────────────────────────
print("\nPacket(s) sent. Restarting ADB server...")
_restart_adb()
time.sleep(2)

print("Polling for ADB device (up to 15s)...")
if _poll_adb(15):
    print("\nSuccess! Run:  adb shell")
    sys.exit(0)

print("\nNo ADB device appeared.")
print("  1. Unplug/replug USB-C cable, wait 3s, re-run with sudo")
print("  2. Make sure the device is on the home screen (not File Transfer)")
print("  3. The device screen should be on (not sleeping)")
sys.exit(1)
