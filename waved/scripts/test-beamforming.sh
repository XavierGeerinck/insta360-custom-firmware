#!/usr/bin/env bash
# SP#3 beamforming acceptance: front-to-back ratio, DOA accuracy, CPU budget.
#
# Requires: Bluetooth speaker (or wired speaker) placed at known angular
# positions 1 m from the device. Operator moves the speaker between prompts.
# Source material: a 1 kHz tone (Test 1), then speech-band audio at each
# compass point (Test 2), then silence (Test 3).
#
# Uses:
#   - /tmp/audio_stats.json    (written by stats_collector; has rms_per_channel,
#                               mic_post_aec_rms, doa_horizontal, mode)
#   - /tmp/waved-mode  + SIGHUP (mode switch mechanism; see mode_switch_handler
#                                in crates/waved/src/main.rs)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WAVED_BIN="$REPO_ROOT/target/aarch64-unknown-linux-gnu/release/waved"

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
    echo "==> building waved for aarch64..."
    (cd "$REPO_ROOT" && cross build -p waved --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa")
    adb push "$WAVED_BIN" /data/waved/waved
fi

echo "==> restarting waved..."
adb shell "killall waved 2>/dev/null; sleep 2; (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &); sleep 5"

WAVED_PID=$(adb shell "for p in /proc/[0-9]*; do if [ -f \$p/comm ]; then n=\$(cat \$p/comm 2>/dev/null); if [ \"\$n\" = \"waved\" ]; then echo \${p#/proc/}; break; fi; fi; done" | tr -d '\r\n')
if [[ -z "$WAVED_PID" ]]; then
    echo "ERROR: waved did not start. Last log lines:"
    adb shell "tail -30 /tmp/waved.log 2>/dev/null" || true
    exit 1
fi
echo "  waved pid=${WAVED_PID}"

read_stats_field() {
    local expr="$1"
    adb shell "cat /tmp/audio_stats.json 2>/dev/null" | python3 -c "
import json, sys
try:
    s = json.load(sys.stdin)
except Exception:
    print('null'); sys.exit(0)
try:
    print(${expr})
except Exception:
    print('null')
"
}

set_mode() {
    local m="$1"
    adb shell "echo '$m' > /tmp/waved-mode"
    adb shell "kill -HUP ${WAVED_PID}"
    # Give AudioService one run-loop tick + next stats write (500 ms) to settle.
    sleep 1
}

echo
echo "=== Test 1: Cardioid front-to-back ratio ==="
set_mode "cardioid"
echo "Place speaker at phi=0° (directly in front of the Wave), 1 m away, playing 1 kHz tone."
read -rp "Press Enter when the tone is playing..."
sleep 3
RMS_FRONT=$(read_stats_field "s.get('mic_post_aec_rms') or 0.0")
echo "  front RMS: ${RMS_FRONT}"

echo "Now move the speaker to phi=180° (directly behind), same distance, same tone."
read -rp "Press Enter when in position..."
sleep 3
RMS_BACK=$(read_stats_field "s.get('mic_post_aec_rms') or 0.0")
echo "  back  RMS: ${RMS_BACK}"

FBR_DB=$(python3 -c "import math; f=float('${RMS_FRONT}'); b=float('${RMS_BACK}'); print(20 * math.log10(f / max(b, 1e-9)))")
echo "  front-to-back: ${FBR_DB} dB (pass >= 10 dB)"
FBR_PASS=$(python3 -c "print(1 if float('${FBR_DB}') >= 10.0 else 0)")
[[ "$FBR_PASS" == "1" ]] || { echo "  FAIL"; exit 1; }
echo "  PASS"

echo
echo "=== Test 2: DOA accuracy ==="
set_mode "omni"
for angle in 0 90 180 270; do
    echo "Place speaker at phi=${angle}°, 1 m. Start speech-band signal (voice or pink noise)."
    read -rp "Press Enter when ready..."
    sleep 3
    MEASURED=$(read_stats_field "s.get('doa_horizontal') or 0.0")
    DIFF=$(python3 -c "
m = float('${MEASURED}')
a = float('${angle}')
# AudioState DOA is in [-180, 180]; ground truth is in [0, 360). Normalize both
# to [0, 360) and take the circular-shortest-arc distance.
m_wrap = (m + 360) % 360
d = abs(m_wrap - a)
print(min(d, 360 - d))
")
    echo "  ground truth: ${angle}°  measured: ${MEASURED}°  error: ${DIFF}°"
    PASS=$(python3 -c "print(1 if float('${DIFF}') <= 10.0 else 0)")
    [[ "$PASS" == "1" ]] || { echo "  FAIL"; exit 1; }
    echo "  PASS"
done

echo
echo "=== Test 3: CPU budget (Supercardioid, 30 s silence) ==="
set_mode "supercardioid"
echo "  sampling /proc/${WAVED_PID}/stat for 30 s (keep room quiet)..."
T0=$(adb shell "awk '{print \$14 + \$15}' /proc/${WAVED_PID}/stat" | tr -d '\r')
sleep 30
T1=$(adb shell "awk '{print \$14 + \$15}' /proc/${WAVED_PID}/stat" | tr -d '\r')
CLK=$(adb shell 'getconf CLK_TCK' | tr -d '\r')
CPU_SEC=$(python3 -c "print((${T1} - ${T0}) / ${CLK})")
CPU_PCT=$(python3 -c "print(${CPU_SEC} / 30.0 * 100)")
echo "  CPU: ${CPU_PCT}% over 30 s (pass < 5%)"
PASS=$(python3 -c "print(1 if float('${CPU_PCT}') < 5.0 else 0)")
[[ "$PASS" == "1" ]] || { echo "  FAIL"; exit 1; }
echo "  PASS"

echo
echo "=== SP#3 ACCEPTANCE: PASS ==="
