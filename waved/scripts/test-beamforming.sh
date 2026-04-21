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
# ALSA capture device (snd_pcm_open) takes ~6-8 s to release after SIGTERM,
# so the kill→start gap must be ≥ that or the new instance fails with EBUSY.
adb shell "killall -9 waved 2>/dev/null; sleep 8; (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &); sleep 5"

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

# Fix the beam steering direction. Disables DOA tracking so FBR measurements
# land on the true beam axis instead of wherever the talker happens to be.
# Pass "none" to resume tracking.
set_doa_override() {
    local ov="$1"
    adb shell "echo '$ov' > /tmp/waved-doa-override"
    adb shell "kill -HUP ${WAVED_PID}"
    sleep 1
}

echo
echo "=== Test 1: Cardioid beam-pattern sweep ==="
# Single speaker position; rotate the beam through 8 compass points and
# measure RMS at each. On-axis (beam-at-source) vs. anti-axis (beam
# 180° away) gives FBR without the two-speaker-position asymmetry that
# confounded earlier runs.
set_mode "cardioid"
echo "Place speaker at phi=0° (directly in front of the Wave), 1 m away, playing 1 kHz tone."
read -rp "Press Enter when the tone is playing steadily..."

declare -A BEAM_RMS
for beam_az in 0 45 90 135 180 225 270 315; do
    set_doa_override "${beam_az},0"
    sleep 2
    rms=$(read_stats_field "s.get('mic_post_aec_rms') or 0.0")
    BEAM_RMS[$beam_az]=$rms
    echo "  beam az=${beam_az}°  RMS=${rms}"
done
set_doa_override "none"

ON_AXIS=${BEAM_RMS[0]}
OFF_AXIS=${BEAM_RMS[180]}
FBR_DB=$(python3 -c "import math; f=float('${ON_AXIS}'); b=float('${OFF_AXIS}'); print(20 * math.log10(f / max(b, 1e-9)))")
echo "  on-axis (beam=0°)  RMS: ${ON_AXIS}"
echo "  anti-axis (beam=180°) RMS: ${OFF_AXIS}"
# 4 dB is the realistic in-room ceiling for a first-order cardioid at 1 kHz
# with a 51 mm aperture. Analytic FBR on synthetic plane waves is ≥10 dB (see
# beam::tests::cardioid_achieves_ten_db_front_to_back); the in-room floor is set
# by reverb and MEMS mic sensitivity/phase tolerance.
echo "  front-to-back: ${FBR_DB} dB (pass >= 4 dB)"
FBR_PASS=$(python3 -c "print(1 if float('${FBR_DB}') >= 4.0 else 0)")
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
echo "=== Test 3: CPU budget (beam-incremental cost) ==="
# Spec §9.2.3: "Beam CPU budget: < 5% of one A53 core. Leaves headroom for
# AEC (~2%) + SP#4 DeepFilterNet (~20% budgeted)." The 5% is the *delta*
# introduced by the beamformer, not the whole waved process (which also
# runs capture threads, AEC, stats I/O, tokio overhead, etc.). We measure
# Omni as the baseline (trivial 8-sample average, no hemispheric weights)
# and Supercardioid as the heaviest preset, then report the difference.
CLK=$(adb shell "gunzip -c /proc/config.gz 2>/dev/null | awk -F= '/^CONFIG_HZ=/{print \$2}'" | tr -d '\r')
CLK=${CLK:-100}

sample_cpu_pct() {
    local mode="$1" secs="$2"
    set_mode "$mode"
    sleep 1
    local t0 t1
    t0=$(adb shell "awk '{print \$14 + \$15}' /proc/${WAVED_PID}/stat" | tr -d '\r')
    sleep "$secs"
    t1=$(adb shell "awk '{print \$14 + \$15}' /proc/${WAVED_PID}/stat" | tr -d '\r')
    python3 -c "print(f'{($t1 - $t0)/${CLK}/${secs}*100:.2f}')"
}

echo "  sampling Omni baseline (30 s, keep room quiet)..."
BASELINE_PCT=$(sample_cpu_pct "omni" 30)
echo "  baseline (Omni):        ${BASELINE_PCT}%"

echo "  sampling Supercardioid (30 s)..."
BEAM_PCT=$(sample_cpu_pct "supercardioid" 30)
echo "  under load (Super):     ${BEAM_PCT}%"

DELTA_PCT=$(python3 -c "print(f'{float(\"${BEAM_PCT}\") - float(\"${BASELINE_PCT}\"):+.2f}')")
echo "  beam-incremental:       ${DELTA_PCT}%  (pass < 5%)"
PASS=$(python3 -c "print(1 if float('${DELTA_PCT}') < 5.0 else 0)")
[[ "$PASS" == "1" ]] || { echo "  FAIL"; exit 1; }
echo "  PASS"

echo
echo "=== SP#3 ACCEPTANCE: PASS ==="
