#!/bin/bash
# Put a Wave back on its stock software: remove everything deploy/install.sh
# and over-the-air updates put on /data. The system partition was never
# changed, so after a reboot the stock app runs again. Deeper paths back
# (stock updater, raw restore) are in docs/UPDATES.md, "Paths back".
set -euo pipefail
UPPER=/data/overlay/upper

[ "$(adb get-state 2>/dev/null || true)" = "device" ] || { echo "ERROR: no Wave found on adb." >&2; exit 1; }

echo "=== Removing waved ==="
adb shell "rm -f \
${UPPER}/etc/init.d/S60waved ${UPPER}/etc/init.d/S60pspapp_daemon \
${UPPER}/etc/init.d/S03splash ${UPPER}/etc/init.d/S89usbgadget \
${UPPER}/etc/rsyslog.conf ${UPPER}/usr/lib/libapplogrs.so \
${UPPER}/etc/init.d/.S60waved.prev ${UPPER}/etc/init.d/.S60waved.new \
${UPPER}/etc/init.d/.S03splash.prev ${UPPER}/etc/init.d/.S03splash.new \
${UPPER}/etc/.rsyslog.conf.prev ${UPPER}/etc/.rsyslog.conf.new; \
rm -rf /data/waved"
echo "=== Done. Reboot the Wave to start the stock app: adb reboot ==="
echo "After the reboot ADB is off again, as on stock. To turn it on: sudo python3 waved/scripts/enable_adb.py"
