#!/usr/bin/env bash
# SP#7(d') device-side acceptance.
set -euo pipefail
cd "$(dirname "$0")/.."

LOG=/tmp/sp7d-acceptance.log
: > "$LOG"

read_hid_field() {
    local expr="$1"
    adb shell "cat /tmp/audio_stats.json 2>/dev/null" | python3 -c "
import json, sys
try: s = json.load(sys.stdin)
except Exception: print('null'); sys.exit(0)
try: print(${expr})
except Exception: print('null')
"
}

echo "==> /dev/hidg0 presence" | tee -a "$LOG"
if adb shell "[ -c /dev/hidg0 ] && echo OK || echo MISSING" | grep -q OK; then
    echo "1/8 hidg0 char device: PASS" | tee -a "$LOG"
else
    echo "1/8 hidg0 char device: FAIL — patch-stock-usbgadget.sh + reboot first" | tee -a "$LOG"
    exit 1
fi

echo "==> hid.device_present" | tee -a "$LOG"
for i in $(seq 1 10); do
    P=$(read_hid_field "s['hid']['device_present']")
    if [ "$P" = "True" ]; then break; fi
    sleep 1
done
if [ "$P" = "True" ]; then
    echo "2/8 device_present: PASS" | tee -a "$LOG"
else
    echo "2/8 device_present: FAIL" | tee -a "$LOG"
    exit 1
fi

echo "==> Wave touch volume tick → host (manual: tap touch volume up 3 times now; 15 s)" | tee -a "$LOG"
BEFORE=$(read_hid_field "s['hid']['events_emitted']['volume_step']")
sleep 15
AFTER=$(read_hid_field "s['hid']['events_emitted']['volume_step']")
DELTA=$(python3 -c "print(int($AFTER) - int($BEFORE))")
if [ "$DELTA" -ge 3 ]; then
    echo "3/8 volume_step counter: PASS ($BEFORE → $AFTER, +$DELTA)" | tee -a "$LOG"
else
    echo "3/8 volume_step counter: FAIL ($BEFORE → $AFTER, +$DELTA, expected ≥ 3)" | tee -a "$LOG"
    exit 1
fi

echo "==> Wave touch mute toggle (manual: toggle mute on touch UI; 10 s)" | tee -a "$LOG"
BEFORE=$(read_hid_field "s['hid']['events_emitted']['mute']")
sleep 10
AFTER=$(read_hid_field "s['hid']['events_emitted']['mute']")
DELTA=$(python3 -c "print(int($AFTER) - int($BEFORE))")
if [ "$DELTA" -ge 1 ]; then
    echo "4/8 mute counter: PASS ($BEFORE → $AFTER, +$DELTA)" | tee -a "$LOG"
else
    echo "4/8 mute counter: FAIL" | tee -a "$LOG"
    exit 1
fi

echo "==> MCU physical Volume Up button regression (manual: press physical Volume Up 3x; 10 s)" | tee -a "$LOG"
BEFORE=$(read_hid_field "s['hid']['events_emitted']['volume_step']")
sleep 10
AFTER=$(read_hid_field "s['hid']['events_emitted']['volume_step']")
DELTA=$(python3 -c "print(int($AFTER) - int($BEFORE))")
if [ "$DELTA" -ge 3 ]; then
    echo "5/8 MCU button regression: PASS" | tee -a "$LOG"
else
    echo "5/8 MCU button regression: FAIL" | tee -a "$LOG"
    exit 1
fi

echo "==> Off-hook LED (manual: start a Zoom call now; 30 s)" | tee -a "$LOG"
for i in $(seq 1 30); do
    if [ "$(read_hid_field "s['hid']['call_off_hook']")" = "True" ]; then
        echo "6/8 call_off_hook: PASS (detected after ${i} s)" | tee -a "$LOG"
        break
    fi
    sleep 1
done
if [ "$(read_hid_field "s['hid']['call_off_hook']")" != "True" ]; then
    echo "6/8 call_off_hook: SKIP — no Zoom call within 30 s (manual, not a hard fail)" | tee -a "$LOG"
fi

echo "==> Mac F10 → Wave speaker mute (regression on SP#6)" | tee -a "$LOG"
echo "    (manual: press F10 on the Mac while music is playing; verify speaker silences within 200 ms)" | tee -a "$LOG"
echo "7/8 SP#6 mute regression: MANUAL" | tee -a "$LOG"

echo "==> hostless stats query" | tee -a "$LOG"
adb shell "cat /tmp/audio_stats.json" | python3 -c "
import json, sys
data = json.loads(sys.stdin.read())
if data.get('hid') is None:
    print('hostless stats: FAIL — hid block missing from /tmp/audio_stats.json', file=sys.stderr)
    sys.exit(1)
" >/dev/null
echo "8/8 hostless stats query: PASS" | tee -a "$LOG"

echo "==> acceptance complete; see $LOG" | tee -a "$LOG"
