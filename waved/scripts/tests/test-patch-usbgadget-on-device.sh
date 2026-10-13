#!/usr/bin/env bash
# shellcheck disable=SC2016  # expect() takes single-quoted expressions and eval's them
# Runs patch-stock-usbgadget.sh on a Wave against a /tmp copy of a stock
# S89usbgadget. Touches nothing outside /tmp on the Wave.
# Usage: test-patch-usbgadget-on-device.sh <stock S89usbgadget>
#   e.g. firmwares/fw_v1.1.6.6/extracted/rootfs/etc/init.d/S89usbgadget
set -euo pipefail
STOCK="${1:?usage: $0 <stock S89usbgadget>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
T="$(mktemp -d)"; trap 'rm -rf "$T"; adb shell "rm -rf /tmp/S89.test /tmp/S89.bak /tmp/patch-test.sh"' EXIT

adb push "$STOCK" /tmp/S89.test >/dev/null
adb push "$HERE/../patch-stock-usbgadget.sh" /tmp/patch-test.sh >/dev/null
adb shell "mkdir -p /tmp/S89.bak && INIT=/tmp/S89.test BACKUP_DIR=/tmp/S89.bak sh /tmp/patch-test.sh"
adb pull /tmp/S89.test "$T/once" >/dev/null
adb shell "INIT=/tmp/S89.test BACKUP_DIR=/tmp/S89.bak sh /tmp/patch-test.sh" | tr -d '\r' > "$T/second-run"
adb pull /tmp/S89.test "$T/twice" >/dev/null

fail=0
expect() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
expect no_commented_adbd_enable '! grep -q "^[[:space:]]*#[[:space:]]*adbd_enable[[:space:]]*$" "$T/once"'
expect adbd_enabled_in_both_boot_branches '[ "$(grep -c "^[[:space:]]*adbd_enable[[:space:]]*$" "$T/once")" -eq "$(( $(grep -c "^[[:space:]]*adbd_enable[[:space:]]*$" "$STOCK") + 2 ))" ]'
expect p_volume_range 'grep -q "echo -16384 > functions/uac2.0/p_volume_min" "$T/once"'
expect hid_inserted_once '[ "$(grep -c "mkdir -p functions/hid.0" "$T/once")" -eq 1 ]'
expect second_run_is_a_no_op 'cmp -s "$T/once" "$T/twice" && grep -q "already patched" "$T/second-run"'
exit $fail
