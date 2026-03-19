#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY="${SCRIPT_DIR}/../target/aarch64-unknown-linux-gnu/release/waved"

if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY"
    echo "Run: cd waved && cross build --release --target aarch64-unknown-linux-gnu"
    exit 1
fi

echo "=== Stopping stock firmware ==="
adb shell "killall pspapp pspapp_daemon pspapp_watchdog 2>/dev/null || true"

echo "=== Pushing waved binary ==="
adb shell "mkdir -p /data/waved"
adb push "$BINARY" /data/waved/waved
adb shell "chmod +x /data/waved/waved"

echo "=== Pushing config ==="
if [ ! -f /data/waved/config.toml ]; then
    adb push "${SCRIPT_DIR}/../config/default.toml" /data/waved/config.toml
    adb shell "chmod 0600 /data/waved/config.toml"
fi

echo "=== Installing init script ==="
adb shell "mkdir -p /data/overlay/upper/etc/init.d"
adb push "${SCRIPT_DIR}/S60waved" /data/overlay/upper/etc/init.d/S60waved
adb shell "chmod +x /data/overlay/upper/etc/init.d/S60waved"

echo "=== Removing stock pspapp init ==="
adb shell "mkdir -p /data/overlay/upper/etc/init.d"
adb shell "echo '#!/bin/sh' > /data/overlay/upper/etc/init.d/S60pspapp_daemon"
adb shell "chmod +x /data/overlay/upper/etc/init.d/S60pspapp_daemon"

echo "=== Removing telemetry library ==="
adb shell "mkdir -p /data/overlay/upper/usr/lib"
adb shell "echo '' > /data/overlay/upper/usr/lib/libapplogrs.so"

echo "=== Installing firewall rules ==="
adb push "${SCRIPT_DIR}/iptables.rules" /data/waved/iptables.rules

echo "=== Creating recordings directory ==="
adb shell "mkdir -p /mnt/udisk/recordings"

echo "=== Done! Reboot to start waved ==="
echo "Run: adb reboot"
