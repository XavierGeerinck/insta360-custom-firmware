#!/usr/bin/env bash
# Patch /etc/init.d/S89usbgadget on a Wave so the f_uac2 gadget advertises
# the right UAC2 control surface to USB hosts.
#
# What we change (all in `uac_init()`):
#   1. p_volume_present = 0   (was 1, the default)
#   2. p_mute_present   = 0   (was 1)
#   3. c_mute_present   = 0   (was 1)
#
# Why: kernel 5.4 `f_uac2` in this build claims it has hardware volume +
# mute on the playback Feature Unit, but it doesn't expose a Master
# channel control that macOS's USBAudio.kext recognises. macOS then
# dispatches NEITHER hardware UAC2 SET_CUR(VOLUME) for output volume,
# NOR falls back to software-side attenuation — F11/F12/Sound-prefs
# slider all become inert.
#
# By telling f_uac2 to NOT advertise these controls, macOS knows the
# device has no HW playback volume, applies attenuation in its own
# audio engine before sending samples, and Mac F10/F11/F12 all work
# the way a generic keyboard's volume keys would.
#
# Capture (mic) volume control stays advertised — Mac Sound prefs Input
# slider remains useful for hardware preamp gain. Capture mute is
# disabled so F10 silences system Output (the natural meaning) instead
# of hitting the headset-semantic mic-mute path.
#
# Wave's touch-UI volume + mute are unaffected; they live in AudioState
# and the playback DSP loop, independent of UAC2 mixer.
#
# After the patch, reboot the device:    adb reboot
#
# Idempotent: re-running adds nothing after the first patch. Backs up
# the original to S89usbgadget.bak.<ts> on first run.

set -euo pipefail

INIT=/etc/init.d/S89usbgadget

if [ ! -f "$INIT" ]; then
    echo "FATAL: $INIT not found — wrong device or stock firmware moved it" >&2
    exit 1
fi

if grep -q "echo 0 > functions/uac2.0/p_volume_present" "$INIT"; then
    echo "already patched"
    exit 0
fi

cp "$INIT" "${INIT}.bak.$(date +%s)"

# Add the three new lines just after the p_volume_res block, inside uac_init().
# We anchor on the closing `fi` of the p_volume_res block.
awk '
    /echo .* > functions\/uac2.0\/p_volume_res/ { in_pres=1 }
    { print }
    in_pres && /^[[:space:]]*fi[[:space:]]*$/ {
        print ""
        print "\tif [ -f functions/uac2.0/p_volume_present ]; then"
        print "\t\techo 0 > functions/uac2.0/p_volume_present"
        print "\tfi"
        print "\tif [ -f functions/uac2.0/p_mute_present ]; then"
        print "\t\techo 0 > functions/uac2.0/p_mute_present"
        print "\tfi"
        print "\tif [ -f functions/uac2.0/c_mute_present ]; then"
        print "\t\techo 0 > functions/uac2.0/c_mute_present"
        print "\tfi"
        in_pres=0
    }
' "$INIT" > "${INIT}.new"

mv "${INIT}.new" "$INIT"
chmod +x "$INIT"

echo "patched. uac2.0 control flags now:"
grep -n "_volume_present\|_mute_present" "$INIT" || echo "(none — patch may not have applied)"

echo
echo "reboot the device for the new descriptors to take effect:"
echo "  adb reboot"
