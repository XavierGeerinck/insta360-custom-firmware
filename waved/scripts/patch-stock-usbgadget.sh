#!/usr/bin/env bash
# Patch /etc/init.d/S89usbgadget on a Wave so the device exposes:
#   1. A useful playback (Output) volume range to UAC2 hosts.
#   2. A functions/hid.0 control surface advertising the stock-extracted
#      Vendor + Telephony_V4 + Consumer report descriptor, so SP#7(d')
#      (HID Consumer/Telephony control) and future SP#7(a) (Vendor channel)
#      reach /dev/hidg0.
#   3. USB storage (G1, docs/USB_STORAGE.md): mass_storage.usb0 linked into
#      the configuration as a removable drive with no medium. waved loads
#      /dev/udisk into it only after unmounting /mnt/udisk, so the gadget
#      never has to be unbound to switch modes and audio, HID and adb stay.
#      The boot checks the bind (UDC set): if the gadget does not bind with
#      the drive, it unlinks it and binds again without it, so a composite
#      this controller cannot take never leaves Wave off USB (adb included).
#      Why goes to the console and /tmp/waved-usbgadget.log.
#
# Idempotent: re-running adds nothing after the first patch. Backs up
# the original to /data/waved/S89usbgadget.bak.<ts> on each patching run:
# never into /etc/init.d, where rcS runs every S??* at boot
# (docs/AUDIO_HARDWARE.md). Each change is written to a dot-named file next
# to S89 (rcS never runs it), made executable, synced, then renamed over
# S89, so a power cut leaves either the old or the new script, executable.
# INIT and BACKUP_DIR can be overridden to try it on a copy
# (scripts/test-patch-stock-usbgadget.sh does).
#
# After patching, reboot:   adb reboot
# Then check the bind:      adb shell cat /sys/kernel/config/usb_gadget/amlogic/UDC
# Undo: docs/USB_STORAGE.md, "Recovering by hand".

set -euo pipefail

INIT=${INIT:-/etc/init.d/S89usbgadget}
BACKUP_DIR=${BACKUP_DIR:-/data/waved}
TMP="$(dirname "$INIT")/.$(basename "$INIT").waved-tmp"
BLOCK=$(mktemp)
trap 'rm -f "$TMP" "$BLOCK"' EXIT

if [ ! -f "$INIT" ]; then
    echo "FATAL: $INIT not found — wrong device or stock firmware moved it" >&2
    exit 1
fi

# Put $TMP in place of $INIT. It must parse; it is executable before the
# rename (a cut between the two would leave a script rcS cannot run: no USB);
# the rename is atomic, and sync makes both durable.
replace_init() {
    if ! sh -n "$TMP"; then
        echo "FATAL: the patched script does not parse; $INIT left as it was" >&2
        exit 1
    fi
    chmod 755 "$TMP"
    sync
    mv -f "$TMP" "$INIT"
    sync
}

ALREADY_VOL=0
ALREADY_HID=0
ALREADY_CVOL=0
ALREADY_MSC=0
grep -q "echo -16384 > functions/uac2.0/p_volume_min" "$INIT" && ALREADY_VOL=1
grep -q "echo -10240 > functions/uac2.0/c_volume_min" "$INIT" && ALREADY_CVOL=1
grep -q "mkdir -p functions/hid.0" "$INIT" && ALREADY_HID=1
grep -q "^waved_g1_check_bind()" "$INIT" && ALREADY_MSC=1

if [ "$ALREADY_MSC" = "0" ] && grep -q "waved G1: USB storage" "$INIT"; then
    # The first version linked the drive without checking the bind.
    echo "FATAL: $INIT has an older USB storage patch. Restore the newest" >&2
    echo "  $BACKUP_DIR/S89usbgadget.bak.* from before it, then run this again." >&2
    exit 1
fi

if [ "$ALREADY_VOL" = "1" ] && [ "$ALREADY_HID" = "1" ] && [ "$ALREADY_CVOL" = "1" ] \
    && [ "$ALREADY_MSC" = "1" ]; then
    echo "already patched (p_volume + c_volume + hid.0 + mass_storage.usb0)"
    exit 0
fi

mkdir -p "$BACKUP_DIR"
cp "$INIT" "$BACKUP_DIR/S89usbgadget.bak.$(date +%s)"
rm -f "$TMP"

if [ "$ALREADY_VOL" = "0" ]; then
    sed \
        -e 's|echo -100 > functions/uac2.0/p_volume_min|echo -16384 > functions/uac2.0/p_volume_min|' \
        -e 's|echo 1 > functions/uac2.0/p_volume_res|echo 256 > functions/uac2.0/p_volume_res|' \
        "$INIT" > "$TMP"
    replace_init
    echo "patched UAC2 p_volume range (mic, gadget->host)"
fi

# c_volume = host->gadget = SPEAKER from Mac's perspective. The kernel's
# f_uac2 takes these in 1/256 dB units (not dB), so the original "-100"
# is -0.39 dB — useless range. Set to -10240 (=-40 dB) so macOS sees a
# real volume control and pushes F11/F12 / system slider events into
# PCM Capture Volume (which we then read in uac/control.rs as the
# host-side speaker volume; see docs/AUDIO_HARDWARE.md).
if [ "$ALREADY_CVOL" = "0" ]; then
    sed \
        -e 's|echo -100 > functions/uac2.0/c_volume_min|echo -10240 > functions/uac2.0/c_volume_min|' \
        -e 's|echo 1 > functions/uac2.0/c_volume_res|echo 256 > functions/uac2.0/c_volume_res|' \
        "$INIT" > "$TMP"
    replace_init
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
    ' "$INIT" > "$TMP"
    replace_init
    echo "patched HID gadget (hid.0)"
fi

if [ "$ALREADY_MSC" = "0" ]; then
    # The functions go right before start_usbgadget; its bind (usb_flash, in
    # start_usbgadget only, not the OTA mode) becomes link, bind, check.
    # Stock's msc_init has loaded /dev/udisk into lun.0 by then; the link step
    # empties it first, or the computer would mount the volume Wave has mounted.
    cat > "$BLOCK" <<'EOF'
# >>> waved G1: USB storage (docs/USB_STORAGE.md), by patch-stock-usbgadget.sh.
# A removable drive with no medium, linked next to the other functions; waved
# loads /dev/udisk only after unmounting /mnt/udisk. If the gadget does not
# bind with it, the drive is unlinked and the gadget bound again without it,
# so audio, HID and adb stay. What happened: /tmp/waved-usbgadget.log.
WAVED_G1_GADGET=${WAVED_G1_GADGET:-/sys/kernel/config/usb_gadget/amlogic}
WAVED_G1_LOG=${WAVED_G1_LOG:-/tmp/waved-usbgadget.log}
WAVED_G1_CONSOLE=${WAVED_G1_CONSOLE:-/dev/console}

waved_g1_say()
{
    echo "USB:{waved G1} $*" >> "$WAVED_G1_CONSOLE" 2>/dev/null
    echo "$(date) $*" >> "$WAVED_G1_LOG" 2>/dev/null
}

waved_g1_bound()
{
    [ -n "$(cat "$WAVED_G1_GADGET/UDC" 2>/dev/null)" ]
}

waved_g1_link_drive()
{
    waved_g1_msc=$WAVED_G1_GADGET/functions/mass_storage.usb0
    [ -d "$waved_g1_msc" ] || return 0
    echo > "$waved_g1_msc/lun.0/file"
    echo 1 > "$waved_g1_msc/lun.0/removable"
    ln -s "$waved_g1_msc" "$WAVED_G1_GADGET/configs/amlogic.1/mass_storage.usb0"
}

waved_g1_check_bind()
{
    waved_g1_link=$WAVED_G1_GADGET/configs/amlogic.1/mass_storage.usb0
    waved_g1_bound && return 0
    if [ ! -L "$waved_g1_link" ]; then
        waved_g1_say "the gadget did not bind (no USB storage drive linked)"
        return 0
    fi
    waved_g1_say "the gadget did not bind with mass_storage.usb0; binding again without it"
    dmesg 2>/dev/null | tail -n 20 >> "$WAVED_G1_LOG" 2>/dev/null
    rm -f "$waved_g1_link"
    usb_flash
    if waved_g1_bound; then
        waved_g1_say "bound without USB storage; waved says USB storage isn't available"
    else
        waved_g1_say "the gadget did not bind without mass_storage.usb0 either"
    fi
}
# <<< waved G1
EOF
    awk -v block="$BLOCK" '
        BEGIN { in_start = 0; done = 0 }
        /^[[:space:]]*(function[[:space:]]+)?start_usbgadget[[:space:]]*\(\)/ {
            while ((getline line < block) > 0) print line
            close(block)
            print ""
            in_start = 1
        }
        in_start && !done && /^[[:space:]]*usb_flash[[:space:]]*$/ {
            indent = $0
            sub(/usb_flash.*/, "", indent)
            print indent "waved_g1_link_drive"
            print
            print indent "waved_g1_check_bind"
            done = 1
            next
        }
        in_start && /^}/ { in_start = 0 }
        { print }
        END { if (!done) exit 3 }
    ' "$INIT" > "$TMP" || {
        echo "FATAL: start_usbgadget has no usb_flash line; mass_storage.usb0 not linked" >&2
        exit 1
    }
    replace_init
    echo "patched USB storage drive (mass_storage.usb0, removable, no medium, bind checked)"
fi

echo
grep -n "p_volume_min\|p_volume_max\|p_volume_res\|functions/hid.0\|waved_g1_" "$INIT" || true

echo
echo "reboot the device for the new gadget config to take effect:"
echo "  adb reboot"
