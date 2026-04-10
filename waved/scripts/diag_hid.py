#!/usr/bin/env python3
"""Differential HID probe: establish baseline, then send test packets
that should each trigger a specific, observable response from pspapp.

Test packets (all 64B, Output Report, Report ID 0x03 in data):
  T1. type 3 (PC reset)   → device_send_reset_response_to_pc() → immediate reply
  T2. type 5 (sync pack)  → inserts sync resp (type 6) → immediate reply
  T3. type 0xF0 sub 0xC0  → factory/ADB dispatch → USB re-enumerate

T1 and T2 are ground-truth tests: if pspapp is receiving our packets via the
USB→hidg0→zmq→recv_hid_data chain, it MUST respond. If either responds but
T3 doesn't cause re-enumeration, the break is specific to the 0xf0 dispatch
or system_server_swt_usb_mode(5)/usb_mode_set(5) inside pspapp.

If none of T1/T2 get a response, the chain is broken before recv_hid_data
(hid_manager_server dead, zmq sub down, pspapp hid thread stuck, etc.).

!!! WARNING — DO NOT SEND 0xF0 sub 0xEE (factory) !!!
The 0xEE HID sub-command dispatches system_setdevmode(0), which sends
system_server event 2 → system_storage_reset → system_factory_reset, which:
  - wipes /mnt/udisk/* (all recordings)
  - clears /data/* (WiFi creds, bindings, config)
  - unbinds the cloud account
  - zeros unifykeys[6] (region_code)
  - reboots the device into first-run setup
This was verified the hard way on 2026-04-08. The 0xEE phase has been
removed from this script. Keep it removed.

Run: sudo python3 waved/scripts/diag_hid.py
"""
import usb.core, usb.util, time, sys

VID, PID = 0x18F0, 0x4E40
INTF = 3
BM_REQ = 0x21
SET_REPORT = 0x09
W_VALUE = (0x02 << 8) | 0x03  # Output + Report ID 3

dev = usb.core.find(idVendor=VID, idProduct=PID)
if dev is None:
    print("NOT FOUND"); sys.exit(1)
print(f"Found: {dev.manufacturer} {dev.product} (SN: {dev.serial_number})")

try:
    if dev.is_kernel_driver_active(INTF):
        print("Detaching kernel driver...")
        dev.detach_kernel_driver(INTF)
except Exception as e:
    print(f"detach warning: {e}")

try:
    usb.util.claim_interface(dev, INTF)
except Exception as e:
    print(f"claim err: {e}"); sys.exit(1)


def make_pkt(msg_type, sub=0x00):
    """64-byte HID Output Report with Report ID 0x03 in data."""
    p = bytearray(64)
    p[0] = 0x03          # Report ID
    p[1] = msg_type      # message type (→ local_b4 after parse)
    p[2] = 0x00          # strict check in 0xf0 branch
    p[3] = 0x00          # overwritten to 0xfe
    p[4] = 0xF0 if msg_type == 0xF0 else 0x00
    p[5] = sub           # ADB sub-cmd (only used for 0xf0 msg type)
    return bytes(p)


def listen(label, seconds):
    """Drain EP 0x82 for `seconds` and return list of (relative_t, bytes[:16])."""
    print(f"[{label}] {seconds}s window...")
    t0 = time.time()
    t_end = t0 + seconds
    reads = []
    while time.time() < t_end:
        try:
            data = bytes(dev.read(0x82, 64, timeout=200))
            reads.append((time.time() - t0, data))
        except usb.core.USBTimeoutError:
            pass
        except Exception as e:
            print(f"  read err: {e}")
            break
    for t, d in reads:
        print(f"  +{t:5.2f}s  len={len(d)}  b0..b5={d[0]:02x} {d[1]:02x} {d[2]:02x} {d[3]:02x} {d[4]:02x} {d[5]:02x}  b1_dec={d[1]}")
    if not reads:
        print("  (none)")
    return reads


def send(label, pkt):
    print(f"\n[{label}] SEND: {' '.join(f'{b:02x}' for b in pkt[:8])}...")
    try:
        n = dev.ctrl_transfer(BM_REQ, SET_REPORT, W_VALUE, INTF, pkt, timeout=2000)
        print(f"        → sent {n} bytes")
        return True
    except Exception as e:
        print(f"        → err: {e}")
        return False


def check_reenum():
    time.sleep(0.3)
    if usb.core.find(idVendor=VID, idProduct=PID) is None:
        print("\n✓✓✓ DEVICE DISAPPEARED — gadget reconfiguring!")
        return True
    return False


# --- Phase 1: baseline ---
print("\n=== PHASE 1: BASELINE (no packets sent) ===")
baseline = listen("BASELINE", 10)
baseline_rate = len(baseline) / 10

# --- Phase 2: type 3 reset ---
print("\n=== PHASE 2: TYPE 3 (PC RESET) ===")
send("T1", make_pkt(0x03))
t1 = listen("POST-T1", 3)

# --- Phase 3: type 5 sync ---
print("\n=== PHASE 3: TYPE 5 (SYNC PACK) ===")
send("T2", make_pkt(0x05))
t2 = listen("POST-T2", 3)

# --- Phase 4: type 0xF0 ADB enable ---
print("\n=== PHASE 4: TYPE 0xF0 sub 0xC0 (ADB ENABLE) ===")
send("T3", make_pkt(0xF0, 0xC0))
if check_reenum():
    sys.exit(0)
t3 = listen("POST-T3", 3)

# NOTE: There is NO phase 5 (0xEE factory). That HID sub-command triggers
# system_storage_reset → system_factory_reset on the device, wiping /data,
# /mnt/udisk, cloud binding, and region code. Do not re-add it.

# --- Summary ---
print("\n========== SUMMARY ==========")
print(f"Baseline rate:   {baseline_rate:.2f} reads/sec ({len(baseline)} in 10s)")
print(f"POST-T1 (rst):   {len(t1)} reads in 3s  {'← response!' if len(t1) > baseline_rate*3*1.5 else '(at baseline)'}")
print(f"POST-T2 (sync):  {len(t2)} reads in 3s  {'← response!' if len(t2) > baseline_rate*3*1.5 else '(at baseline)'}")
print(f"POST-T3 (ADB):   {len(t3)} reads in 3s  {'← response!' if len(t3) > baseline_rate*3*1.5 else '(at baseline)'}")
print()
print("Interpretation:")
print("  - T1 or T2 response above baseline = pspapp IS receiving our packets;")
print("    break is specific to the 0xf0 factory dispatch or usb_mode_set.")
print("  - T1 AND T2 at baseline = break is upstream (hid_manager_server or ZMQ).")
print("  - Device re-enum in T3 or T4 = mode switch succeeded.")

try:
    usb.util.release_interface(dev, INTF)
    dev.attach_kernel_driver(INTF)
except Exception:
    pass
