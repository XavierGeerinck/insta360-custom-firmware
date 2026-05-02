#!/usr/bin/env bash
# On-device acceptance: confirms the mic_volume pipeline is wired end-to-end.
# Drives the UAC2 capture mixer, then reads /tmp/audio_stats.json to confirm
# the gain stage actually saw the change.
#
# Usage on device (via adb):
#   adb push waved/scripts/test-mic-volume.sh /data/test-mic-volume.sh
#   adb shell sh /data/test-mic-volume.sh
#
# Pre-conditions:
#   1. waved is running with the new mic_volume code; default
#      TOML mic_volume=200 and JSON pdm_gain=24 already in place.
#
# Audio-level verification (separate manual procedure):
#   waved holds hw:1,0 capture exclusively, so on-device `arecord -D hw:1,0`
#   fails with "Device or resource busy". To verify actual gained bytes flow
#   to the host, plug into a Mac and `arecord` from the device's USB audio
#   gadget (Audio MIDI Setup → "Insta360 Wave" → record), or use any host
#   audio app. This stats-based script verifies the runtime control path.

set -euo pipefail

# Preflight: verify waved is running.
if ! pidof waved > /dev/null 2>&1; then
    echo "ERROR: waved is not running" >&2
    exit 1
fi
if [ ! -f /tmp/audio_stats.json ]; then
    echo "ERROR: /tmp/audio_stats.json not found (stats_collector should write it every 500ms)" >&2
    exit 1
fi

# Read mic_gain_linear (numeric) from the stats JSON.
read_gain_linear() {
    grep -oE '"mic_gain_linear":[^,}]*' /tmp/audio_stats.json \
        | sed -E 's/.*: *([0-9.-]+).*/\1/'
}
read_gain_db() {
    grep -oE '"mic_gain_db":[^,}]*' /tmp/audio_stats.json \
        | sed -E 's/.*: *([0-9.-]+).*/\1/'
}

# Compare two floats within tolerance. Returns 0 on match, 1 otherwise.
near() {
    local actual="$1" expected="$2" tol="$3"
    awk -v a="$actual" -v e="$expected" -v t="$tol" \
        'BEGIN { d = (a - e); if (d < 0) d = -d; exit (d > t) }'
}

# Check one stage: set capture to PERCENT, expect EXPECT_LIN linear and EXPECT_DB dB.
check_stage() {
    local label="$1" capture_pct="$2" expect_lin="$3" expect_db="$4"
    amixer -c 1 sset 'PCM' "${capture_pct}%" capture > /dev/null
    sleep 1
    local got_lin got_db
    got_lin="$(read_gain_linear)"
    got_db="$(read_gain_db)"
    printf '  %-50s linear=%s dB=%s' "$label" "$got_lin" "$got_db"
    if near "$got_lin" "$expect_lin" 0.01 && near "$got_db" "$expect_db" 0.05; then
        echo '  PASS'
    else
        echo "  FAIL (expected linear≈$expect_lin dB≈$expect_db)"
        return 1
    fi
}

PASS=0
FAIL=0
run() {
    if "$@"; then PASS=$((PASS+1)); else FAIL=$((FAIL+1)); fi
}

echo "=== mic_volume runtime control path ==="
run check_stage "host capture 50% (boot default → mic_volume=200)"   50 2.0  6.02
run check_stage "host capture 25% (→ mic_volume=100, unity)"          25 1.0  0.0
run check_stage "host capture 100% (→ mic_volume=800, +18 dB)"       100 8.0 18.06
run check_stage "host capture 0%  (→ mic_volume=0,   −120 dB sentinel)" 0 0.0 -120.0

echo
echo "=== recording limiter (idle) ==="
amixer -c 1 sset 'PCM' 50% capture > /dev/null
sleep 1
RED_IDLE=$(grep -oE '"recording_limiter_reduction_db":[^,}]*' /tmp/audio_stats.json | sed -E 's/.*: *([0-9.-]+).*/\1/')
echo "  idle reduction: $RED_IDLE dB (expected ~0.0 with no input)"
if awk -v v="$RED_IDLE" 'BEGIN { exit (v > -0.1 ? 0 : 1) }'; then
    echo "  PASS"
    PASS=$((PASS+1))
else
    echo "  FAIL (limiter is reducing at idle — investigate)"
    FAIL=$((FAIL+1))
fi

echo
echo "=== recording limiter (loud — manual: speak loudly close to the mic for 3s) ==="
amixer -c 1 sset 'PCM' 100% capture > /dev/null
echo "  *** speak loudly now, 3 second window ***"
sleep 3
RED_LOUD=$(grep -oE '"recording_limiter_reduction_db":[^,}]*' /tmp/audio_stats.json | sed -E 's/.*: *([0-9.-]+).*/\1/')
echo "  loud reduction: $RED_LOUD dB (expected < 0 if limiter engaged)"
if awk -v v="$RED_LOUD" 'BEGIN { exit (v < -0.1 ? 0 : 1) }'; then
    echo "  PASS"
    PASS=$((PASS+1))
else
    echo "  WARN (limiter not engaged — your input may not have been loud enough; not a hard fail)"
fi

echo
echo "=== reset to boot default ==="
amixer -c 1 sset 'PCM' 50% capture > /dev/null
sleep 1
echo "  current: linear=$(read_gain_linear) dB=$(read_gain_db)"

echo
echo "=== PDM Gain hardware preamp ==="
PDM=$(amixer -c 0 cget numid=9 | grep -oE ': values=[0-9]+' | sed 's/: values=//')
if [ "$PDM" = "24" ]; then
    echo "  PASS (numid=9 = 24, matches audio_config_default.json)"
    PASS=$((PASS+1))
else
    echo "  FAIL (numid=9 = $PDM, expected 24)"
    FAIL=$((FAIL+1))
fi

echo
echo "=== summary ==="
echo "  passed: $PASS"
echo "  failed: $FAIL"
if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
echo
echo "Reboot test (manual): 'killall waved' to trigger respawn or full reboot;"
echo "re-run this script and confirm boot default reproduces (TOML 200 → 2.0×)."
