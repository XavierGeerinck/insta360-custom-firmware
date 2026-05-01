#!/usr/bin/env bash
# SP#6 host-side (Mac) acceptance helpers. Run on the Mac with the Wave
# selected as the default audio device, alongside scripts/test-uac2-routing.sh
# on the workstation that has the device on adb.
#
# Steps:
#   1. Generate 1 kHz tone (10 s WAV) and play via afplay.
#   2. Record 10 s from the Wave via sox.
#   3. Print current Mac output volume (osascript).

set -euo pipefail

# Verify the Wave is selected as an audio device
WAVE_OUT=$(/usr/sbin/system_profiler SPAudioDataType 2>/dev/null | grep -B 2 'UAC2_Gadget' | head -3 || true)
echo "Wave device info:"
echo "$WAVE_OUT"
echo

if ! command -v afplay >/dev/null; then
    echo "ERROR: afplay missing — run on macOS"
    exit 1
fi

if ! command -v sox >/dev/null; then
    echo "ERROR: sox missing — install with 'brew install sox'"
    exit 1
fi

# 1 kHz, 10 s, mono → Mac will up-channel for stereo output
TONE=/tmp/sp6-1k.wav
python3 - <<'PY'
import struct, math, wave
with wave.open('/tmp/sp6-1k.wav', 'wb') as buf:
    buf.setnchannels(1); buf.setsampwidth(2); buf.setframerate(48_000)
    n = 48_000 * 10
    amp = int(0.3 * 32767)
    for i in range(n):
        s = int(amp * math.sin(2*math.pi*1000*i/48_000))
        buf.writeframes(struct.pack('<h', s))
PY

echo "==> playing 1 kHz / 10 s through Wave (afplay)"
afplay "$TONE" &
AFPLAY_PID=$!
sleep 1

echo "==> recording 10 s from Wave (sox)"
sox -d /tmp/sp6-mic.wav trim 0 10

wait $AFPLAY_PID

echo "==> current Mac output volume (osascript)"
osascript -e 'output volume of (get volume settings)'

echo
echo "==> expected: device-side test reports downstream_active=true and upstream_active=true"
echo "    /tmp/sp6-mic.wav should contain audible mic capture from the Wave"
