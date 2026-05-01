#!/usr/bin/env python3
"""Soft-input check — sum of per-band MBAGC gains should be ≥ 4 dB."""
import sys, json
data = json.load(sys.stdin)
gains = data.get("mbagc_active_gain_db", [0]*4)
total = sum(gains)
if total < 4.0:
    print(f"FAIL: total MBAGC gain across bands {total:.2f} dB < 4 dB threshold"); sys.exit(1)
print(f"OK: total MBAGC gain {total:.2f} dB")
