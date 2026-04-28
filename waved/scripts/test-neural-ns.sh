#!/usr/bin/env bash
# SP#4 neural NS acceptance: confirms DFN3 (CPU) starts cleanly on-device,
# the audio pipeline stays alive with NR engaged, and the CPU delta from
# adding the neural NS stage is under the 5 % budget set in the SP#4 plan.
#
# This script does NOT measure NS *quality* (PESQ/STOI) — that needs paired
# clean/noisy reference recordings and is owned by a separate audio-quality
# eval. The gate here is "DFN3 runs and doesn't blow the CPU budget".
#
# Uses:
#   - /tmp/waved-nr  + SIGHUP   (NR-level switch — added in SP#4; values:
#                                off | low | med | high)
#   - /proc/$pid/stat            (utime+stime ticks, process-wide)
#   - /proc/config.gz            (CONFIG_HZ for ticks → seconds)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WAVED_BIN="$REPO_ROOT/target/aarch64-unknown-linux-gnu/release/waved"
DEVICE_PATH="${WAVED_DEVICE_PATH:-/tmp/waved}"
SAMPLE_S="${SAMPLE_S:-10}"

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
    echo "==> building waved for aarch64..."
    (cd "$REPO_ROOT" && cross build -p waved --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa")
    adb push "$WAVED_BIN" "$DEVICE_PATH"
fi

echo "==> restarting waved..."
adb shell "
    killall S60waved 2>/dev/null
    killall -9 audioserver waved 2>/dev/null
    rm -f /tmp/waved-nr /tmp/waved-mode /tmp/waved-doa-override
    sleep 8
    (setsid $DEVICE_PATH >/tmp/waved.log 2>&1 </dev/null &)
    sleep 6
"

WPID=$(adb shell 'for p in /proc/[0-9]*; do [ "$(cat $p/comm 2>/dev/null)" = "waved" ] && echo "${p#/proc/}" && break; done' | tr -d '\r\n')
if [[ -z "$WPID" ]]; then
    echo "ERROR: waved did not start. Last log lines:"
    adb shell 'tail -30 /tmp/waved.log'
    exit 1
fi
echo "  waved pid=${WPID}"

CLK=$(adb shell 'gunzip -c /proc/config.gz 2>/dev/null | awk -F= "/^CONFIG_HZ=/{print \$2}"' | tr -d '\r')
CLK=${CLK:-100}
echo "  kernel HZ=${CLK}"

set_nr() {
    local lvl="$1"
    adb shell "echo '$lvl' > /tmp/waved-nr && kill -HUP ${WPID}"
    sleep 5  # tract session creation can take ~1.5 s; let things settle
}

sample_cpu_pct() {
    local secs="$1"
    local t0 t1 delta
    t0=$(adb shell "awk '{print \$14+\$15}' /proc/${WPID}/stat" | tr -d '\r')
    sleep "$secs"
    t1=$(adb shell "awk '{print \$14+\$15}' /proc/${WPID}/stat" | tr -d '\r')
    delta=$((t1 - t0))
    awk -v d="$delta" -v hz="$CLK" -v s="$secs" 'BEGIN { printf "%.2f", d / hz / s * 100 }'
}

echo
echo "=== Test 1: DFN3 init engages cleanly ==="
set_nr "low"
INIT_OK=$(adb shell 'grep -c "neural NS active" /tmp/waved.log' | tr -d '\r')
DFN3_LOG=$(adb shell 'grep "neural NS active" /tmp/waved.log | tail -1')
echo "  init log line: $DFN3_LOG"
if [[ "$INIT_OK" -eq 0 ]]; then
    echo "  FAIL — DFN3 did not initialize (check tract cross-build, default-model-ll feature)"
    adb shell 'grep -iE "DFN3|tract|deep_filter" /tmp/waved.log | tail -10'
    exit 1
fi
echo "  PASS"

echo
echo "=== Test 2: still alive after ${SAMPLE_S}s with DFN3 active ==="
sleep "$SAMPLE_S"
ALIVE=$(adb shell "[ -d /proc/${WPID} ] && echo y || echo n" | tr -d '\r')
if [[ "$ALIVE" != "y" ]]; then
    echo "  FAIL — waved died with DFN3 engaged"
    adb shell 'tail -30 /tmp/waved.log'
    exit 1
fi
echo "  PASS"

echo
echo "=== Test 3: CPU budget (DFN3 vs NR Off) ==="
set_nr "off"
echo "  baseline (NR Off) sampling for ${SAMPLE_S}s..."
PCT_OFF=$(sample_cpu_pct "$SAMPLE_S")
set_nr "low"
echo "  DFN3 (NR Low) sampling for ${SAMPLE_S}s..."
PCT_DFN=$(sample_cpu_pct "$SAMPLE_S")
DELTA=$(awk -v a="$PCT_DFN" -v b="$PCT_OFF" 'BEGIN { printf "%.2f", a - b }')
echo "  baseline:  ${PCT_OFF}%"
echo "  DFN3:      ${PCT_DFN}%"
echo "  delta:     ${DELTA}%   (gate: <= 5%)"
PASS=$(awk -v d="$DELTA" 'BEGIN { print (d <= 5.0) ? 1 : 0 }')
if [[ "$PASS" != "1" ]]; then
    echo "  FAIL — DFN3 over CPU budget"
    exit 1
fi
echo "  PASS"

echo
echo "==> all gates passed"
