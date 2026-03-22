#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WAVED_DIR="${SCRIPT_DIR}/.."
BINARY="${WAVED_DIR}/target/aarch64-unknown-linux-gnu/release/waved"
DEVICE_TEST="${WAVED_DIR}/target/aarch64-unknown-linux-gnu/release/waved-device-test"
BACKUP_DIR="${SCRIPT_DIR}/backup/$(date +%s)"
RESULTS_DIR="${SCRIPT_DIR}/results"
LOG_FILE="${RESULTS_DIR}/$(date +%s).log"

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }

echo "=== waved HIL test harness ==="
echo ""

if ! adb get-state >/dev/null 2>&1; then
    red "ERROR: No ADB device connected"
    exit 1
fi

yellow "WARNING: This will restart waved on your device."
yellow "Your device will be temporarily unavailable during testing."
read -rp "Continue? [y/N] " confirm
if [[ "$confirm" != "y" && "$confirm" != "Y" ]]; then
    echo "Aborted."
    exit 0
fi

if [ ! -f "$BINARY" ]; then
    red "ERROR: waved binary not found. Run:"
    red "  cd waved && cross build --release --target aarch64-unknown-linux-gnu"
    exit 1
fi

if [ ! -f "$DEVICE_TEST" ]; then
    red "ERROR: waved-device-test binary not found. Run:"
    red "  cd waved && cross build --release --target aarch64-unknown-linux-gnu -p waved-device-test"
    exit 1
fi

echo ""
echo "--- Backing up current state ---"
mkdir -p "$BACKUP_DIR"
adb pull /data/waved/ "$BACKUP_DIR/" 2>/dev/null || true
green "Backup saved to $BACKUP_DIR"

echo ""
echo "--- Deploying test build ---"
adb shell "/etc/init.d/S60waved stop 2>/dev/null || /data/overlay/upper/etc/init.d/S60waved stop 2>/dev/null || killall waved 2>/dev/null || true"
sleep 2
adb push "$BINARY" /data/waved/waved
adb shell "chmod +x /data/waved/waved"
adb push "$DEVICE_TEST" /data/waved/waved-device-test
adb shell "chmod +x /data/waved/waved-device-test"
green "Test build deployed"

echo ""
echo "--- Running tests ---"
mkdir -p "$RESULTS_DIR"

adb shell "/etc/init.d/S60waved start 2>/dev/null || /data/overlay/upper/etc/init.d/S60waved start 2>/dev/null || /data/waved/waved &"
sleep 3

echo ">> Smoke tests" | tee "$LOG_FILE"
"$SCRIPT_DIR/smoke.sh" 2>&1 | tee -a "$LOG_FILE"
SMOKE_EXIT=${PIPESTATUS[0]}

echo "" | tee -a "$LOG_FILE"
echo ">> Device tests" | tee -a "$LOG_FILE"
adb shell "/data/waved/waved-device-test" 2>&1 | tee -a "$LOG_FILE"
DEVICE_EXIT=${PIPESTATUS[0]}

echo ""
echo "--- Restoring previous build ---"
adb shell "killall waved 2>/dev/null || true"
sleep 1

if [ -f "$BACKUP_DIR/waved" ]; then
    adb push "$BACKUP_DIR/waved" /data/waved/waved
    adb shell "chmod +x /data/waved/waved"
fi

adb shell "/etc/init.d/S60waved start 2>/dev/null || /data/overlay/upper/etc/init.d/S60waved start 2>/dev/null || true"
sleep 2

RESTORED_PID=$(adb shell "pidof waved 2>/dev/null" | tr -d '\r\n')
if [ -n "$RESTORED_PID" ]; then
    green "Restored waved running (PID=$RESTORED_PID)"
else
    yellow "WARNING: Could not confirm restored waved is running"
fi

echo ""
echo "=== HIL Test Report ==="
echo "Full log: $LOG_FILE"

if [ "$SMOKE_EXIT" -eq 0 ] && [ "$DEVICE_EXIT" -eq 0 ]; then
    green "ALL TESTS PASSED"
else
    red "SOME TESTS FAILED (smoke=$SMOKE_EXIT, device=$DEVICE_EXIT)"
    exit 1
fi
