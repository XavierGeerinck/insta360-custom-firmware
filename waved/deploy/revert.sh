#!/bin/bash
set -e

echo "=== Reverting to stock firmware ==="
adb shell "rm -rf /data/overlay/upper/etc/init.d/S60waved"
adb shell "rm -rf /data/overlay/upper/etc/init.d/S60pspapp_daemon"
adb shell "rm -rf /data/overlay/upper/usr/lib/libapplogrs.so"
adb shell "rm -rf /data/waved"
echo "=== Done! Reboot to restore stock ==="
echo "Run: adb reboot"
