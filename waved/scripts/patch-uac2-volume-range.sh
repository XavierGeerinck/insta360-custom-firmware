#!/usr/bin/env bash
# Patch /etc/init.d/S89usbgadget on a Wave so the f_uac2 gadget advertises
# a useful playback (Output) volume range to USB hosts. Without this, macOS
# sees the kernel default range (-100..0 in 1/256 dB units = ~0.4 dB total)
# and dismisses the device's HW Output volume — Mac volume keys / mute then
# go nowhere because Mac neither dispatches UAC2 SET_CUR for Output nor
# falls back to software attenuation. Mic gain is unaffected; macOS sends
# UAC2 for Input regardless of range.
#
# After the patch, reboot the device. Volume keys on a Mac with Wave
# selected as Output will then dispatch UAC2 SET_CUR(VOLUME) which our
# `run_mixer_poll` picks up and `run_playback_loop` applies as scalar gain.
#
# Idempotent: re-running has no effect after the first patch. Backs up the
# original to S89usbgadget.bak.<ts> on first run.

set -euo pipefail

INIT=/etc/init.d/S89usbgadget

if [ ! -f "$INIT" ]; then
    echo "FATAL: $INIT not found — wrong device or stock firmware moved it" >&2
    exit 1
fi

if grep -q "echo -16384 > functions/uac2.0/p_volume_min" "$INIT"; then
    echo "already patched"
    exit 0
fi

cp "$INIT" "${INIT}.bak.$(date +%s)"

sed -i \
    -e 's|echo -100 > functions/uac2.0/p_volume_min|echo -16384 > functions/uac2.0/p_volume_min|' \
    -e 's|echo 1 > functions/uac2.0/p_volume_res|echo 256 > functions/uac2.0/p_volume_res|' \
    "$INIT"

echo "patched. relevant lines:"
grep -n "p_volume_min\|p_volume_max\|p_volume_res" "$INIT"

echo
echo "reboot the device for the new range to take effect:"
echo "  adb reboot"
