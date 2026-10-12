#!/bin/bash
set -e

echo "=== Reverting to stock firmware ==="
adb shell "rm -rf /data/overlay/upper/etc/init.d/S60waved"
adb shell "rm -rf /data/overlay/upper/etc/init.d/S60pspapp_daemon"
adb shell "rm -rf /data/overlay/upper/usr/lib/libapplogrs.so"
# Old and half-written copies an over-the-air update kept: dot names, so rcS's
# S??* glob never runs them (docs/UPDATES.md).
adb shell "rm -f /data/overlay/upper/etc/init.d/.S60waved.prev /data/overlay/upper/etc/init.d/.S60waved.new"
adb shell "rm -f /data/overlay/upper/etc/init.d/.S03splash.prev /data/overlay/upper/etc/init.d/.S03splash.new"
adb shell "rm -f /data/overlay/upper/etc/.rsyslog.conf.prev /data/overlay/upper/etc/.rsyslog.conf.new"
adb shell "rm -rf /data/waved"
echo "=== Done! Reboot to restore stock ==="
echo "Run: adb reboot"
