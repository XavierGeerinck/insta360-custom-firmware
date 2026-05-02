#!/usr/bin/env bash
# Patch /etc/init.d/S89usbgadget on a Wave so the f_uac2 gadget advertises
# a useful playback (Output) volume range to USB hosts.
#
# What we change (in `uac_init()`):
#   p_volume_min  -100  →  -16384  (= -64 dB)
#   p_volume_res     1  →  256     (= 1 dB steps)
#
# Why: the stock value `-100..0` in 1/256 dB units gives only ~0.4 dB of
# advertised HW volume range, which macOS treats as "not a real HW
# Output volume" — it dismisses the path entirely and doesn't fall
# back to software attenuation either. With a proper range, macOS at
# least *attempts* the UAC2 SET_CUR(VOLUME) path.
#
# IMPORTANT — known limitation kept here for future work:
# Even with the wider range, kernel 5.4 `f_uac2` does not advertise a
# Master channel control in the playback Feature Unit's `bmaControls`,
# so macOS USBAudio.kext still doesn't dispatch SET_CUR for F11/F12 or
# the Output slider. The realistic fix is patching the kernel module
# itself; we attempted disabling `p_volume_present`/`p_mute_present`
# to coax macOS into software attenuation, but macOS silently no-ops
# instead — and disabling `c_mute_present` removed F10's working path
# (the capture-FU redirect onto speaker mute lives in waved/UacControl).
# So we leave all three `_present` flags at their defaults (=1).
#
# Wave's own touch-UI volume + mute work regardless of this, since they
# live in AudioState and the playback DSP loop, independent of the UAC2
# mixer. F10 silences the speaker via UacControl's host_mute_target=
# Speaker redirect.
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
