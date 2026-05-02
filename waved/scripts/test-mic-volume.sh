#!/usr/bin/env bash
# On-device acceptance: confirms mic_volume + pdm_gain produce audible
# recordings and that UAC2 mixer events propagate.
#
# Usage on device (via adb):
#   adb push waved/scripts/test-mic-volume.sh /data/test-mic-volume.sh
#   adb shell sh /data/test-mic-volume.sh
#
# Pre-conditions: waved is running with the new mic_volume code; default
# TOML mic_volume=200 and JSON pdm_gain=24 already in place.

set -e

REC_DIR=/tmp/mic-volume-acceptance
mkdir -p "$REC_DIR"

# 1. Boot defaults: record 5 s, measure peak/RMS.
echo "=== test 1: boot defaults (mic_volume=200, pdm_gain=24) ==="
arecord -c 1 -r 48000 -f S16_LE -d 5 "$REC_DIR/boot.wav"
sox "$REC_DIR/boot.wav" -n stat 2>&1 | grep -E "Maximum|RMS"

# 2. Halve mic_volume via the runtime command path so the gain stage
#    sees 100 (unity). 6 dB quieter than test 1.
echo "=== test 2: mic_volume=100 (unity) ==="
amixer -c 1 cset name='PCM Capture Volume' 25%   # 25% of 0..400 = 100
sleep 1
arecord -c 1 -r 48000 -f S16_LE -d 5 "$REC_DIR/unity.wav"
sox "$REC_DIR/unity.wav" -n stat 2>&1 | grep -E "Maximum|RMS"

# 3. Max mic_volume (400). 6 dB louder than test 1.
echo "=== test 3: mic_volume=400 (max) ==="
amixer -c 1 cset name='PCM Capture Volume' 100%
sleep 1
arecord -c 1 -r 48000 -f S16_LE -d 5 "$REC_DIR/max.wav"
sox "$REC_DIR/max.wav" -n stat 2>&1 | grep -E "Maximum|RMS"

# 4. Stats sanity.
echo "=== /tmp/audio_stats.json mic_gain fields ==="
grep -E '"mic_gain_(linear|db)"' /tmp/audio_stats.json || true

echo
echo "Pass criteria:"
echo "  test 1 peak ~ -12 to -6 dBFS at normal speaking distance"
echo "  test 2 peak ~ 6 dB below test 1"
echo "  test 3 peak ~ 6 dB above test 1 (or clipped to 0 dBFS for loud sources)"
echo "  reboot test: re-run script after 'killall waved; reboot' equivalent;"
echo "    test 1 reproduces (TOML default re-applied, runtime change gone)"
