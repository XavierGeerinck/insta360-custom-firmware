#!/bin/bash
set -e

echo "=== Reverting to stock firmware ==="
adb shell "rm -rf /data/overlay/upper/etc/init.d/S60waved"
adb shell "rm -rf /data/overlay/upper/etc/init.d/S60pspapp_daemon"
adb shell "rm -rf /data/overlay/upper/usr/lib/libapplogrs.so"
# Old copies an over-the-air update kept (docs/UPDATES.md).
adb shell "rm -f /data/overlay/upper/etc/init.d/S60waved.prev /data/overlay/upper/etc/init.d/S03splash.prev /data/overlay/upper/etc/rsyslog.conf.prev"
adb shell "rm -rf /data/waved"
echo "=== Done! Reboot to restore stock ==="
echo "Run: adb reboot"
