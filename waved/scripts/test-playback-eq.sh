#!/usr/bin/env bash
# SP#5 on-device acceptance: playback EQ + gain.
#   1. Voice EQ — sanity check that `playback_preset` and `mbagc_active_gain_db`
#      surface in /tmp/audio_stats.json.
#   2. Soft talk — the MBAGC stays off (stock mbagc_switch 0): 0 dB on every band.
#   3. Limiter ceiling — full-scale square produces ≤ −1 dB attenuation.
#   4. Preset switch artefact — manual listening (rapid voice↔media flip; the
#      switch fades out and back in over 5 ms each way).
#   5. CPU delta — waved CPU% within budget.
#
# Pre-conditions: device booted, waved running.
# Outputs: /tmp/sp5-acceptance.log on the host. Exit 0 on full pass.

set -euo pipefail
cd "$(dirname "$0")/.."

LOG=/tmp/sp5-acceptance.log
: > "$LOG"

run_step() {
    local name="$1"; shift
    echo "=== $name ===" | tee -a "$LOG"
    "$@" 2>&1 | tee -a "$LOG"
}

WAVED_PID=$(adb shell "pidof waved" | tr -d '\r')
if [ -z "$WAVED_PID" ]; then
    echo "ERROR: waved is not running"
    exit 1
fi
echo "waved pid=${WAVED_PID}" | tee -a "$LOG"

# Test 1: Voice EQ sanity (engages SIGUSR2 with pink noise, reads stats).
run_step "1/5 voice EQ sanity" bash -c '
python3 scripts/gen_pink_noise.py 6 > /tmp/sp5-pink.s16le
adb push /tmp/sp5-pink.s16le /data/waved/playback.s16le >/dev/null
adb shell "echo voice > /tmp/waved-playback-preset"
adb shell "kill -HUP '"$WAVED_PID"'"
sleep 0.5
adb shell "kill -USR2 '"$WAVED_PID"'"
sleep 7
adb shell "cat /tmp/audio_stats.json" | python3 scripts/check_eq_response.py
'

# Test 2: Soft talk is not lifted (MBAGC off, as stock).
run_step "2/5 soft talk, MBAGC off" bash -c '
python3 scripts/gen_soft_speech.py 6 > /tmp/sp5-soft.s16le
adb push /tmp/sp5-soft.s16le /data/waved/playback.s16le >/dev/null
adb shell "kill -USR2 '"$WAVED_PID"'"
sleep 7
adb shell "cat /tmp/audio_stats.json" | python3 scripts/check_mbagc_gain.py
'

# Test 3: Limiter true-peak ceiling.
run_step "3/5 limiter ceiling" bash -c '
python3 scripts/gen_square.py 4 > /tmp/sp5-square.s16le
adb push /tmp/sp5-square.s16le /data/waved/playback.s16le >/dev/null
adb shell "kill -USR2 '"$WAVED_PID"'"
sleep 5
adb shell "cat /tmp/audio_stats.json" | python3 scripts/check_limiter.py
'

# Test 4: Preset switch click test (manual listening).
run_step "4/5 preset switch artefact (manual)" bash -c '
python3 scripts/gen_pink_noise.py 4 > /tmp/sp5-pink-short.s16le
adb push /tmp/sp5-pink-short.s16le /data/waved/playback.s16le >/dev/null
adb shell "kill -USR2 '"$WAVED_PID"'" &
for p in voice media voice media voice; do
    adb shell "echo $p > /tmp/waved-playback-preset; kill -HUP '"$WAVED_PID"'"
    sleep 0.3
done
sleep 1
echo "  manual listening: pink noise must remain steady; any audible click fails this test"
'

# Test 5: CPU delta vs pre-SP#5 baseline.
run_step "5/5 CPU delta" bash -c '
adb shell "cat /proc/'"$WAVED_PID"'/stat" > /tmp/cpu-before
sleep 15
adb shell "cat /proc/'"$WAVED_PID"'/stat" > /tmp/cpu-after
python3 scripts/cpu_delta.py /tmp/cpu-before /tmp/cpu-after 1.5
'

# Back to the automatic speaker scene (docs/AUDIO_HARDWARE.md "Speaker scenes").
adb shell "echo auto > /tmp/waved-playback-preset; kill -HUP $WAVED_PID"

echo "=== ALL PASS ===" | tee -a "$LOG"
