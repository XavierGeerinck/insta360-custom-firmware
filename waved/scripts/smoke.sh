#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY="${SCRIPT_DIR}/../target/aarch64-unknown-linux-gnu/release/waved"
PASSED=0
FAILED=0

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }

pass() { green "[PASS] $1"; PASSED=$((PASSED + 1)); }
fail() { red   "[FAIL] $1: $2"; FAILED=$((FAILED + 1)); }

DEPLOY=false
for arg in "$@"; do
    case "$arg" in
        --deploy) DEPLOY=true ;;
        *) echo "Usage: $0 [--deploy]"; exit 1 ;;
    esac
done

echo "=== waved smoke test ==="

if ! adb get-state >/dev/null 2>&1; then
    red "ERROR: No ADB device connected"
    exit 1
fi
pass "ADB connected"

if [ "$DEPLOY" = true ]; then
    if [ ! -f "$BINARY" ]; then
        red "ERROR: Binary not found at $BINARY"
        red "Run: cd waved && cross build --release --target aarch64-unknown-linux-gnu"
        exit 1
    fi
    echo "--- Deploying ---"
    adb shell "killall waved 2>/dev/null || true"
    sleep 1
    adb push "$BINARY" /data/waved/waved
    adb shell "chmod +x /data/waved/waved"
    adb shell "/etc/init.d/S60waved start 2>/dev/null || /data/overlay/upper/etc/init.d/S60waved start"
    pass "Binary deployed"
fi

echo "--- Checking waved process ---"
PID=""
for i in $(seq 1 10); do
    PID=$(adb shell "pidof waved 2>/dev/null" | tr -d '\r\n')
    if [ -n "$PID" ]; then break; fi
    sleep 1
done

if [ -n "$PID" ]; then
    pass "waved running (PID=$PID)"
else
    fail "waved process" "not running after 10s"
fi

if [ -n "$PID" ]; then
    RSS=$(adb shell "cat /proc/$PID/status 2>/dev/null" | grep VmRSS | awk '{print $2}')
    if [ -n "$RSS" ]; then
        pass "Process RSS: ${RSS} kB"
    else
        fail "Process RSS" "could not read /proc/$PID/status"
    fi
fi

PANICS=$(adb shell "logcat -d -t 100 2>/dev/null" | grep -c "panicked\|SIGABRT\|SIGSEGV" || true)
if [ "$PANICS" -eq 0 ]; then
    pass "No panics in recent logs"
else
    fail "Panic check" "$PANICS panic/crash entries found"
fi

if adb shell "test -f /data/waved/meetings.db" 2>/dev/null; then
    pass "meetings.db exists"
else
    fail "meetings.db" "not found"
fi

if adb shell "test -f /data/waved/config.toml" 2>/dev/null; then
    pass "config.toml exists"
else
    fail "config.toml" "not found"
fi

echo ""
echo "=== Results: $PASSED passed, $FAILED failed ==="
if [ "$FAILED" -gt 0 ]; then exit 1; fi
