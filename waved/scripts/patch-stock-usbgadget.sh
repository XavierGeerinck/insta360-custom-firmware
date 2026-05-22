#!/usr/bin/env bash
# Patch /etc/init.d/S89usbgadget on a Wave so the device exposes:
#   1. A useful playback (Output) volume range to UAC2 hosts.
#   2. A functions/hid.0 control surface advertising the stock-extracted
#      Vendor + Telephony_V4 + Consumer report descriptor, so SP#7(d')
#      (HID Consumer/Telephony control) and future SP#7(a) (Vendor channel)
#      reach /dev/hidg0.
#
# Idempotent: re-running adds nothing after the first patch. Backs up
# the original to S89usbgadget.bak.<ts> on first run.
#
# After patching, reboot:   adb reboot

set -euo pipefail

INIT=/etc/init.d/S89usbgadget

if [ ! -f "$INIT" ]; then
    echo "FATAL: $INIT not found — wrong device or stock firmware moved it" >&2
    exit 1
fi

ALREADY_VOL=0
ALREADY_HID=0
ALREADY_CVOL=0
grep -q "echo -16384 > functions/uac2.0/p_volume_min" "$INIT" && ALREADY_VOL=1
grep -q "echo -10240 > functions/uac2.0/c_volume_min" "$INIT" && ALREADY_CVOL=1
grep -q "mkdir -p functions/hid.0" "$INIT" && ALREADY_HID=1

if [ "$ALREADY_VOL" = "1" ] && [ "$ALREADY_HID" = "1" ] && [ "$ALREADY_CVOL" = "1" ]; then
    echo "already patched (p_volume + c_volume + hid.0)"
    exit 0
fi

cp "$INIT" "${INIT}.bak.$(date +%s)"

if [ "$ALREADY_VOL" = "0" ]; then
    sed -i \
        -e 's|echo -100 > functions/uac2.0/p_volume_min|echo -16384 > functions/uac2.0/p_volume_min|' \
        -e 's|echo 1 > functions/uac2.0/p_volume_res|echo 256 > functions/uac2.0/p_volume_res|' \
        "$INIT"
    echo "patched UAC2 p_volume range (mic, gadget->host)"
fi

# c_volume = host->gadget = SPEAKER from Mac's perspective. The kernel's
# f_uac2 takes these in 1/256 dB units (not dB), so the original "-100"
# is -0.39 dB — useless range. Set to -10240 (=-40 dB) so macOS sees a
# real volume control and pushes F11/F12 / system slider events into
# PCM Capture Volume (which we then read in uac/control.rs as the
# host-side speaker volume; see docs/AUDIO_HARDWARE.md).
if [ "$ALREADY_CVOL" = "0" ]; then
    sed -i \
        -e 's|echo -100 > functions/uac2.0/c_volume_min|echo -10240 > functions/uac2.0/c_volume_min|' \
        -e 's|echo 1 > functions/uac2.0/c_volume_res|echo 256 > functions/uac2.0/c_volume_res|' \
        "$INIT"
    echo "patched UAC2 c_volume range (speaker, host->gadget)"
fi

if [ "$ALREADY_HID" = "0" ]; then
    DESCRIPTOR='\\x06\\x00\\xff\\x09\\x01\\xa1\\x01\\x85\\x03\\x09\\x01\\x15\\x00\\x26\\xff\\x00\\x95\\x3f\\x75\\x08\\x81\\x02\\x09\\x01\\x15\\x00\\x26\\xff\\x00\\x95\\x3f\\x75\\x08\\x91\\x22\\xc0\\x05\\x0b\\x09\\x05\\xa1\\x01\\x85\\x02\\x15\\x00\\x25\\x01\\x75\\x01\\x95\\x01\\x09\\x20\\x81\\x22\\x09\\x2f\\x81\\x06\\x09\\x21\\x81\\x02\\x09\\x24\\x81\\x06\\x09\\x07\\x05\\x09\\x09\\x01\\x81\\x06\\x75\\x03\\x81\\x01\\x85\\x05\\x05\\x08\\x09\\x17\\x75\\x01\\x91\\x02\\x75\\x07\\x91\\x03\\x85\\x06\\x09\\x09\\x75\\x01\\x91\\x02\\x75\\x07\\x91\\x03\\x85\\x07\\x09\\x18\\x75\\x01\\x91\\x02\\x75\\x07\\x91\\x03\\xc0\\x05\\x0c\\x09\\x01\\xa1\\x01\\x85\\x01\\x05\\x0c\\x15\\x00\\x25\\x01\\x09\\xe9\\x09\\xea\\x09\\xe2\\x09\\xcd\\x09\\xb5\\x09\\xb6\\x09\\xb0\\x09\\xb1\\x75\\x01\\x95\\x08\\x81\\x02\\xc0'

    awk -v desc="$DESCRIPTOR" '
        BEGIN { inserted = 0 }
        /^[[:space:]]*usb_flash[[:space:]]*$/ && !inserted {
            print "\t# SP#7d HID control surface (Vendor + Telephony_V4 + Consumer)"
            print "\tmkdir -p functions/hid.0"
            print "\techo 1  > functions/hid.0/protocol"
            print "\techo 1  > functions/hid.0/subclass"
            print "\techo 64 > functions/hid.0/report_length"
            print "\techo -ne \"" desc "\" > functions/hid.0/report_desc"
            print "\tln -sf functions/hid.0 configs/amlogic.1/hid.0"
            inserted = 1
        }
        { print }
    ' "$INIT" > "${INIT}.tmp" && mv "${INIT}.tmp" "$INIT"
    chmod +x "$INIT"
    echo "patched HID gadget (hid.0)"
fi

echo
grep -n "p_volume_min\|p_volume_max\|p_volume_res\|functions/hid.0" "$INIT" || true

echo
echo "reboot the device for the new gadget config to take effect:"
echo "  adb reboot"
