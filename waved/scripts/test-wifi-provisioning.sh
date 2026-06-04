#!/usr/bin/env bash
# Acceptance: trigger provisioning, assert AP + page, POST creds, assert STA.
#
# Usage: ./test-wifi-provisioning.sh <test-ssid> <test-psk>
#
# Prereq: device on ADB, `waved` running, and the Task 0 daemon probe passed
# (hostapd/dnsmasq/wpa_passphrase present — see docs/WIFI.md).
set -euo pipefail
SSID="${1:?usage: test-wifi-provisioning.sh <ssid> <psk>}"
PSK="${2:?usage: test-wifi-provisioning.sh <ssid> <psk>}"

echo "[1/4] triggering provisioning (tap the Wi-Fi row in System settings now)…"
# waved enters provisioning on Command::OpenWifiProvisioning. Trigger it via the
# on-device UI (System → Wi-Fi). Allow time for scan (~3s) + AP bring-up.
sleep 10

echo "[2/4] asserting AP at 192.168.2.1 + hostapd…"
adb shell 'ip addr show wlan0 | grep -q 192.168.2.1' \
  && echo "  AP IP up" || { echo "  FAIL: no AP IP on wlan0"; exit 1; }
adb shell 'pgrep hostapd >/dev/null' \
  && echo "  hostapd running" || { echo "  FAIL: hostapd not running"; exit 1; }
adb shell 'pgrep dnsmasq >/dev/null' \
  && echo "  dnsmasq running" || { echo "  FAIL: dnsmasq not running"; exit 1; }

echo "[3/4] asserting form reachable + POST creds…"
# Run curl from the device itself against the loopback-reachable AP_IP.
adb shell "curl -s -o /dev/null -w '%{http_code}' http://192.168.2.1/" | grep -q 200 \
  && echo "  GET / → 200" || { echo "  FAIL: form not served"; exit 1; }
adb shell "curl -s -X POST --data 'ssid=${SSID}&psk=${PSK}' http://192.168.2.1/connect" >/dev/null \
  && echo "  POST /connect accepted"

echo "[4/4] asserting STA reconnect within 30s…"
for _ in $(seq 1 30); do
  if adb shell "wpa_cli status 2>/dev/null | grep -q 'wpa_state=COMPLETED'"; then
    echo "  STA reconnected (wpa_state=COMPLETED)"
    exit 0
  fi
  sleep 1
done
echo "  FAIL: did not reconnect as STA within 30s"
exit 1
