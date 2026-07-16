#!/usr/bin/env bash
# Full on-device Wi-Fi provisioning acceptance.
# Usage: ADB_SERIAL=<optional> ./scripts/test-wifi-provisioning.sh <ssid> <psk>
set -euo pipefail

SSID="${1:?usage: test-wifi-provisioning.sh <ssid> <psk>}"
PSK="${2:?usage: test-wifi-provisioning.sh <ssid> <psk>}"
ADB=(adb)
if [[ -n "${ADB_SERIAL:-}" ]]; then
  ADB+=(-s "$ADB_SERIAL")
fi

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
device_shell() { "${ADB[@]}" shell "$@"; }

SSID_BYTES=$(printf '%s' "$SSID" | LC_ALL=C wc -c | tr -d ' ')
[[ $SSID_BYTES -ge 1 && $SSID_BYTES -le 32 ]] || fail "SSID must be 1..32 bytes"
[[ -z "$PSK" || (${#PSK} -ge 8 && ${#PSK} -le 63) ]] \
  || fail "PSK must be empty or 8..63 characters"

echo "[1/7] preflight"
"${ADB[@]}" get-state >/dev/null
# shellcheck disable=SC2016 # $tool expands on the device, not on the host.
device_shell 'for tool in hostapd wpa_supplicant wpa_cli udhcpc curl; do command -v "$tool" >/dev/null || exit 20; done' \
  || fail "required stock Wi-Fi tool missing"
device_shell 'iw list 2>/dev/null | grep -q "AP/VLAN"' || fail "radio does not advertise AP mode"
device_shell 'pidof waved >/dev/null' || fail "waved is not running"

echo "[2/7] open System → Wi-Fi, verify the live status, then tap Connect to another network"
for _ in $(seq 1 30); do
  if device_shell 'ip -4 addr show wlan0 2>/dev/null | grep -q "192.168.2.1/24"'; then
    break
  fi
  sleep 1
done
device_shell 'ip -4 addr show wlan0 2>/dev/null | grep -q "192.168.2.1/24"' \
  || fail "provisioning AP address did not appear"

echo "[3/7] AP, DHCP, DNS, and captive setup page"
device_shell 'pidof hostapd >/dev/null' || fail "hostapd is not running"
device_shell 'grep -qi ":0043 " /proc/net/udp' || fail "DHCP server is not bound to UDP/67"
device_shell 'grep -qi ":0035 " /proc/net/udp' || fail "DNS server is not bound to UDP/53"
HTTP_CODE=$(device_shell "curl -sS -o /dev/null -w '%{http_code}' http://192.168.2.1/" | tr -d '\r')
[[ "$HTTP_CODE" == 200 ]] || fail "GET / returned HTTP $HTTP_CODE"
CAPTIVE_HEADERS=$(device_shell \
  "curl -sS -D - -o /dev/null http://192.168.2.1/generate_204" | tr -d '\r')
grep -q '^HTTP/1.1 302 Found$' <<<"$CAPTIVE_HEADERS" \
  || fail "captive probe did not return HTTP 302"
grep -qi '^Location: http://192\.168\.2\.1/$' <<<"$CAPTIVE_HEADERS" \
  || fail "captive probe did not redirect to the setup root"
unset CAPTIVE_HEADERS

echo "[4/7] phone gate"
echo "Scan the QR shown on the Wave and accept the Wi-Fi join prompt."
echo "Confirm the phone joins Wave-XXXX and receives 192.168.2.50–200."
echo "Confirm the setup sheet opens, or tap the phone's 'Sign in to network' prompt."
echo "The Wave screen also shows the SSID, password, and IP as a manual fallback."
read -r -p "Press Enter after that succeeds: "

FORM=$(printf '%s\0%s' "$SSID" "$PSK" | python3 -c '
import sys, urllib.parse
ssid, psk = sys.stdin.buffer.read().split(b"\0", 1)
print(urllib.parse.urlencode({"ssid": ssid.decode(), "psk": psk.decode()}), end="")
')
printf '%s' "$FORM" | device_shell \
  "curl -sS -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/x-www-form-urlencoded' --data-binary @- http://192.168.2.1/connect" \
  | grep -q 200 || fail "POST /connect was not accepted"
unset FORM

echo "[5/7] wait for requested SSID and IPv4"
CONNECTED=0
for _ in $(seq 1 50); do
  STATUS=$(device_shell 'wpa_cli status 2>/dev/null' | tr -d '\r')
  ACTUAL_SSID=$(printf '%s\n' "$STATUS" | sed -n 's/^ssid=//p')
  STATE=$(printf '%s\n' "$STATUS" | sed -n 's/^wpa_state=//p')
  IP=$(printf '%s\n' "$STATUS" | sed -n 's/^ip_address=//p')
  if [[ "$STATE" == COMPLETED && "$ACTUAL_SSID" == "$SSID" && -n "$IP" && "$IP" != 0.0.0.0 ]]; then
    CONNECTED=1
    break
  fi
  sleep 1
done
[[ $CONNECTED == 1 ]] || fail "requested network did not reach COMPLETED with IPv4"
printf '  connected: %s (%s)\n' "$ACTUAL_SSID" "$IP"

echo "[6/7] cleanup and secret checks"
device_shell 'if pidof hostapd >/dev/null; then exit 1; fi'
device_shell 'test ! -e /tmp/waved-hostapd.conf'
CONFIG_MODE=$(device_shell 'stat -c %a /etc/wpa_supplicant.conf' | tr -d '\r')
[[ "$CONFIG_MODE" == 600 ]] || fail "wpa_supplicant.conf mode is $CONFIG_MODE, expected 600"
NETWORKS=$(device_shell 'wpa_cli -i wlan0 list_networks' | tr -d '\r')
TARGET_ROWS=$(printf '%s\n' "$NETWORKS" | awk -F '\t' -v ssid="$SSID" '$2 == ssid')
TARGET_COUNT=$(printf '%s\n' "$TARGET_ROWS" | awk 'NF { count++ } END { print count + 0 }')
[[ "$TARGET_COUNT" == 1 ]] || fail "expected exactly one saved profile for requested SSID, found $TARGET_COUNT"
[[ "$TARGET_ROWS" != *"[DISABLED]"* ]] || fail "requested saved profile is disabled"
unset CONFIG_MODE NETWORKS TARGET_ROWS TARGET_COUNT
if [[ -n "$PSK" ]]; then
  REMOTE_TEXT=$("${ADB[@]}" exec-out sh -c \
    'cat /data/waved/waved.log /etc/wpa_supplicant.conf 2>/dev/null')
  [[ "$REMOTE_TEXT" != *"$PSK"* ]] || fail "submitted PSK appears in device log/config"
  unset REMOTE_TEXT
fi

echo "[7/7] process/storage health"
device_shell 'pidof waved >/dev/null' || fail "waved exited"
device_shell 'test ! -e /tmp/mic_capture.pcm' || fail "diagnostic mic capture unexpectedly enabled"
device_shell 'df -h /data /tmp'
echo "Wi-Fi provisioning acceptance: PASS"
