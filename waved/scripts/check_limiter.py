#!/usr/bin/env python3
"""Limiter check — full-scale square should produce ≤ -1 dB attenuation."""
import sys, json
data = json.load(sys.stdin)
red = data.get("limiter_reduction_db", 0.0)
if red > -1.0:
    print(f"FAIL: limiter_reduction_db {red:.2f} dB; expected ≤ -1 dB on full-scale square"); sys.exit(1)
print(f"OK: limiter_reduction_db {red:.2f} dB")
