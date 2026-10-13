#!/usr/bin/env python3
"""Soft-input check — the MBAGC stays off.

Stock's dynamic_eq_config_2.json has mbagc_switch 0 and stock runs no MBAGC on
the speaker (docs/AUDIO_HARDWARE.md "Stock speaker tuning"), so soft input
must not be lifted: every band's gain is 0 dB.
"""
import sys, json
data = json.load(sys.stdin)
gains = data.get("mbagc_active_gain_db", [0]*4)
if any(abs(g) > 0.01 for g in gains):
    print(f"FAIL: MBAGC is active: {gains} (expected all 0 dB)"); sys.exit(1)
print(f"OK: MBAGC off, gains {gains}")
