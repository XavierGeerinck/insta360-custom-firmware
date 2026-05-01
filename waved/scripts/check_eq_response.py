#!/usr/bin/env python3
"""Sanity-check /tmp/audio_stats.json: preset==voice and mbagc_active_gain_db is well-shaped."""
import sys, json
data = json.load(sys.stdin)
preset = data.get("playback_preset", "?")
gains = data.get("mbagc_active_gain_db", [])
if preset != "voice":
    print(f"FAIL: expected preset=voice, got {preset}"); sys.exit(1)
if not gains or len(gains) != 4:
    print(f"FAIL: mbagc_active_gain_db missing or wrong shape: {gains}"); sys.exit(1)
print(f"OK: preset={preset} mbagc_gains={gains}")
