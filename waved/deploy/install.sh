#!/bin/bash
# Install waved on an Insta360 Wave over adb. Safe to re-run: it upgrades an
# existing install and keeps its config.toml.
#
# Everything it changes lives on /data; the read-only system partition is never
# touched, and deploy/revert.sh removes all of it:
#   /data/waved/                 waved, config.toml, splash.fb, iptables.rules
#   the /etc and /usr overlays   (stored under /data/overlay/upper): S60waved,
#                                S03splash, rsyslog.conf, the patched
#                                S89usbgadget, a stub S60pspapp_daemon and an
#                                empty libapplogrs.so (stock telemetry)
#
# The procedure follows docs/DISPLAY_HARDWARE.md#deploy: /data is small, so
# the new binary goes to /tmp first and replaces the old one only once it has
# arrived intact and waved has exited. The overlay files are written through
# the /etc and /usr mounts, as the over-the-air updater does, never into
# /data/overlay/upper directly.
#
# The order is the safety net. A Wave without the stock app and without ADB
# cannot be reached again, so nothing stock is stopped or stubbed until ADB is
# known to come up at the next boot:
#   1. checks, then the new binary to /tmp, intact
#   2. the USB gadget patch (ADB at boot) and a check that it took
#   3. stop the stock app and waved; swap the binary only once waved has exited
#   4. config, start-up scripts, splash, logging, firewall rules
#   5. last of all, the stubs that switch the stock app and telemetry off,
#      read back to check they took
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WAVED_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
BINARY="${WAVED_BINARY:-${WAVED_DIR}/target/aarch64-unknown-linux-gnu/release/waved}"
MARGIN_KB=20480   # room for logs and the recordings database after the swap

STEP="starting"
trap 'echo "ERROR: install.sh stopped at: ${STEP}" >&2' ERR
trap 'echo "ERROR: install.sh interrupted at: ${STEP}" >&2; exit 130' INT TERM

die() { echo "ERROR: $*" >&2; exit 1; }
# bash does not run the ERR trap inside a function, so an adb shell that fails
# in on_wave would end the install without a word: its callers add || stopped.
stopped() { die "install.sh stopped at: ${STEP}"; }
on_wave() { adb shell "$1" | tr -d '\r'; }

strip_tool() {
    if [ -n "${OBJCOPY:-}" ]; then echo "$OBJCOPY"; return; fi
    local host tool
    host="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')"
    tool="$(rustc --print sysroot 2>/dev/null)/lib/rustlib/${host}/bin/llvm-objcopy"
    [ -x "$tool" ] || die "llvm-objcopy not found. Run: rustup component add llvm-tools"
    echo "$tool"
}

local_md5() {
    if command -v md5sum >/dev/null 2>&1; then md5sum "$1" | cut -d' ' -f1; else md5 -q "$1"; fi
}

# --- Checks. Nothing on the Wave changes until all of them pass. ---
STEP="checking the binary and the adb connection"
[ -f "$BINARY" ] || die "no waved binary at $BINARY
Build it first, from the waved/ directory:
  cross build --release --target aarch64-unknown-linux-gnu --features \"ui,capture-alsa\" -p waved"
[ "$(adb get-state 2>/dev/null || true)" = "device" ] || die "no Wave found on adb. Connect it over USB and enable ADB (Install guide, step 1)."

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

STEP="stripping the binary"
STRIP="$(strip_tool)"
"$STRIP" --strip-all "$BINARY" "$STAGE/waved"
SIZE_KB=$(( ($(wc -c < "$STAGE/waved") + 1023) / 1024 ))

STEP="checking free space on /data"
FREE_KB="$(on_wave "df -k /data | tail -1" | awk '{print $4}')"
OLD_KB="$(on_wave "if [ -f /data/waved/waved ]; then du -k /data/waved/waved | cut -f1; else echo 0; fi")"
NEED_KB=$(( SIZE_KB + MARGIN_KB ))
[ $(( FREE_KB + OLD_KB )) -ge "$NEED_KB" ] || die "/data has $(( (FREE_KB + OLD_KB) / 1024 )) MB available and waved needs $(( NEED_KB / 1024 )) MB. Nothing was changed. See Troubleshooting: \"/data is full\"."

# --- Copy the new binary to /tmp and check it arrived intact. ---
STEP="copying waved to /tmp on the Wave"
echo "=== Copying waved ($(( SIZE_KB / 1024 )) MB) to the Wave ==="
adb push "$STAGE/waved" /tmp/waved.new >/dev/null
[ "$(on_wave "md5sum /tmp/waved.new" | cut -d' ' -f1)" = "$(local_md5 "$STAGE/waved")" ] \
    || die "the copy on the Wave does not match (md5). Nothing was changed; run install.sh again."

# --- ADB at boot first, while the Wave is still fully stock. Without it the
# Wave has no ADB after the reboot (only the stock app turns it on, and it will
# not run), and revert.sh can't reach it. Harmless on stock: ADB comes up earlier.
STEP="patching the USB gadget: ADB at boot, volume ranges, HID controls"
echo "=== Patching the USB gadget: ADB at boot, volume ranges, HID controls ==="
adb push "${WAVED_DIR}/scripts/patch-stock-usbgadget.sh" /tmp/patch-usbgadget.sh >/dev/null
PATCH_OUT="$(on_wave "sh /tmp/patch-usbgadget.sh && echo patched; rm -f /tmp/patch-usbgadget.sh")" || true
if [ "$(echo "$PATCH_OUT" | tail -n 1)" != "patched" ]; then
    echo "$PATCH_OUT" | grep -v -e '^reboot the device' -e 'adb reboot' >&2 || true
    die "the USB gadget patch did not finish. Nothing was changed except possibly /etc/init.d/S89usbgadget (a backup is in /data/waved); the Wave still runs the stock software. See Troubleshooting: \"ADB at boot\"."
fi
# Only what it patched. Its closing "reboot the device ... adb reboot" is for
# running the patch by hand; here the reboot comes once everything is in place.
echo "$PATCH_OUT" | grep -E '^(already )?patched .' || true

# Both must hold: no adbd_enable line in the gadget script is still commented
# out, and at least one live adbd_enable line is inside start_usbgadget(), the
# function the boot runs (from its declaration to the "}" in column 0 that
# closes it; start_usbgadget_ota_mode() does not count). busybox awk on the
# Wave; the answer is read from its output, not from an exit code.
STEP="checking that ADB is enabled at boot"
ADB_AWK='/^[[:space:]]*#[[:space:]]*adbd_enable[[:space:]]*$/ { commented++ }; /^[[:space:]]*(function[[:space:]]+)?start_usbgadget[[:space:]]*[(][)]/ { boot = 1 }; boot && /^}/ { boot = 0 }; boot && /^[[:space:]]*adbd_enable[[:space:]]*$/ { live++ }; END { print "commented=" commented+0 " live=" live+0 }'
ADB_AT_BOOT="$(on_wave "awk '${ADB_AWK}' /etc/init.d/S89usbgadget")" || stopped
case "$ADB_AT_BOOT" in
    "commented=0 live="[1-9]*) ;;
    *) die "ADB is not enabled at boot (/etc/init.d/S89usbgadget: ${ADB_AT_BOOT:-no answer}). Nothing was changed except possibly that file; the Wave still runs the stock software. See Troubleshooting: \"ADB at boot\"." ;;
esac

# --- Stop the stock app, the watchdog and waved, then swap. ---
STEP="stopping the stock app and waved"
echo "=== Stopping the stock app and any running waved ==="
on_wave "killall pspapp pspapp_daemon 2>/dev/null; \
kill \$(ps | awk '/S60waved start/ && !/awk/ {print \$1}') 2>/dev/null; \
killall waved 2>/dev/null; \
n=0; while pidof waved >/dev/null && [ \$n -lt 15 ]; do sleep 1; n=\$((n+1)); done; \
if pidof waved >/dev/null; then killall -9 waved 2>/dev/null; n=0; while pidof waved >/dev/null && [ \$n -lt 5 ]; do sleep 1; n=\$((n+1)); done; fi; \
true" >/dev/null || stopped

# The old binary's space is only freed once waved has exited: copying over a
# live one fails with ENOSPC over a partial file. Swap only if it is gone.
STEP="installing the waved binary"
echo "=== Installing waved ==="
SWAP="$(on_wave "if pidof waved >/dev/null; then echo busy; else mkdir -p /data/waved && rm -f /data/waved/waved && cp /tmp/waved.new /data/waved/waved && chmod 755 /data/waved/waved && rm -f /tmp/waved.new && echo ok; fi")" || true
case "$SWAP" in
    ok) ;;
    busy) die "waved did not stop; nothing was replaced (the new binary is in /tmp/waved.new). Run install.sh again." ;;
    *) die "could not copy waved into /data/waved. The new binary is still in /tmp/waved.new; check 'adb shell df -h /data' and run install.sh again." ;;
esac

STEP="writing the config"
HAS_CONFIG="$(on_wave "[ -f /data/waved/config.toml ] && echo yes || echo no")" || stopped
case "$HAS_CONFIG" in
    yes) ;;
    no)
        echo "=== Writing the default config.toml ==="
        adb push "${WAVED_DIR}/config/default.toml" /data/waved/config.toml >/dev/null ;;
    *) stopped ;;
esac

# The /etc overlay: write through the mount, not into /data/overlay/upper.
STEP="installing start-up scripts, splash, logging and firewall rules"
echo "=== Installing start-up scripts, splash, logging and firewall rules ==="
on_wave "mkdir -p /mnt/udisk/recordings" >/dev/null || stopped
adb push "${SCRIPT_DIR}/S60waved" /etc/init.d/S60waved >/dev/null
adb push "${SCRIPT_DIR}/S03splash" /etc/init.d/S03splash >/dev/null
adb push "${SCRIPT_DIR}/splash.fb" /data/waved/splash.fb >/dev/null
adb push "${SCRIPT_DIR}/rsyslog.conf" /etc/rsyslog.conf >/dev/null
adb push "${SCRIPT_DIR}/iptables.rules" /data/waved/iptables.rules >/dev/null
on_wave "chmod 0600 /data/waved/config.toml; chmod 755 /etc/init.d/S60waved /etc/init.d/S03splash" >/dev/null || stopped

# Last: these switch the stock app and its telemetry off for good.
STEP="switching the stock app and telemetry off"
echo "=== Switching the stock app and its telemetry off ==="
on_wave "printf '#!/bin/sh\n' > /etc/init.d/S60pspapp_daemon; chmod 755 /etc/init.d/S60pspapp_daemon; \
: > /usr/lib/libapplogrs.so" >/dev/null || stopped

# Read them back. A stub that did not take leaves the stock app starting next
# to waved at the next boot.
STEP="checking the stubs"
STUBS="$(on_wave "cat /etc/init.d/S60pspapp_daemon; if [ -f /usr/lib/libapplogrs.so ] && [ ! -s /usr/lib/libapplogrs.so ]; then echo 'libapplogrs.so: empty'; fi")" || stopped
[ "$STUBS" = "$(printf '#!/bin/sh\nlibapplogrs.so: empty')" ] \
    || die "the stock app and telemetry stubs did not take (/etc/init.d/S60pspapp_daemon, /usr/lib/libapplogrs.so). waved is installed, but the stock app would start next to it. Run install.sh again before you reboot. See Troubleshooting: \"The installer stopped part-way\"."

echo "=== Done. Reboot the Wave to start waved: adb reboot ==="
