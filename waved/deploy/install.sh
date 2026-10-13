#!/bin/bash
# Install waved on an Insta360 Wave over adb. Safe to re-run: it upgrades an
# existing install and keeps its config.toml.
#
# Everything it changes is on /data; the read-only system partition is never
# touched, and deploy/revert.sh removes all of it:
#   /data/waved/                 waved, config.toml, splash.fb, iptables.rules
#   /data/overlay/upper/etc/     S60waved, S03splash, rsyslog.conf, and the
#                                patched S89usbgadget (the /etc overlay)
#   /data/overlay/upper/usr/lib  an empty libapplogrs.so (stock telemetry)
#
# The procedure follows docs/DISPLAY_HARDWARE.md#deploy: /data is small, so
# the new binary goes to /tmp first and replaces the old one only once it has
# arrived intact and waved has exited.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WAVED_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
BINARY="${WAVED_BINARY:-${WAVED_DIR}/target/aarch64-unknown-linux-gnu/release/waved}"
UPPER=/data/overlay/upper
MARGIN_KB=20480   # room for logs and the recordings database after the swap

die() { echo "ERROR: $*" >&2; exit 1; }
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
[ -f "$BINARY" ] || die "no waved binary at $BINARY
Build it first, from the waved/ directory:
  cross build --release --target aarch64-unknown-linux-gnu --features \"ui,capture-alsa\" -p waved"
[ "$(adb get-state 2>/dev/null || true)" = "device" ] || die "no Wave found on adb. Connect it over USB and enable ADB (Install guide, step 1)."

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
"$(strip_tool)" --strip-all "$BINARY" "$STAGE/waved"
SIZE_KB=$(( ($(wc -c < "$STAGE/waved") + 1023) / 1024 ))

FREE_KB="$(on_wave "df -k /data | tail -1" | awk '{print $4}')"
OLD_KB="$(on_wave "if [ -f /data/waved/waved ]; then du -k /data/waved/waved | cut -f1; else echo 0; fi")"
NEED_KB=$(( SIZE_KB + MARGIN_KB ))
[ $(( FREE_KB + OLD_KB )) -ge "$NEED_KB" ] || die "/data has $(( (FREE_KB + OLD_KB) / 1024 )) MB available and waved needs $(( NEED_KB / 1024 )) MB. Nothing was changed. See Troubleshooting: \"/data is full\"."

# --- Copy the new binary to /tmp and check it arrived intact. ---
echo "=== Copying waved ($(( SIZE_KB / 1024 )) MB) to the Wave ==="
adb push "$STAGE/waved" /tmp/waved.new >/dev/null
[ "$(on_wave "md5sum /tmp/waved.new" | cut -d' ' -f1)" = "$(local_md5 "$STAGE/waved")" ] \
    || die "the copy on the Wave does not match (md5). Nothing was changed; run install.sh again."

# --- Stop the stock app, the watchdog and waved, then swap. ---
echo "=== Stopping the stock app and any running waved ==="
on_wave "killall pspapp pspapp_daemon 2>/dev/null; \
kill \$(ps | awk '/S60waved start/ && !/awk/ {print \$1}') 2>/dev/null; \
killall waved 2>/dev/null; \
n=0; while pidof waved >/dev/null && [ \$n -lt 15 ]; do sleep 1; n=\$((n+1)); done; true" >/dev/null

echo "=== Installing waved ==="
[ "$(on_wave "mkdir -p /data/waved && rm -f /data/waved/waved && cp /tmp/waved.new /data/waved/waved && chmod 755 /data/waved/waved && rm -f /tmp/waved.new && echo ok")" = "ok" ] \
    || die "could not copy waved into /data/waved. The new binary is still in /tmp/waved.new; check 'adb shell df -h /data' and run install.sh again."

if [ "$(on_wave "[ -f /data/waved/config.toml ] && echo yes || echo no")" = "no" ]; then
    echo "=== Writing the default config.toml ==="
    adb push "${WAVED_DIR}/config/default.toml" /data/waved/config.toml >/dev/null
fi

echo "=== Installing start-up scripts, splash, logging and firewall rules ==="
on_wave "mkdir -p ${UPPER}/etc/init.d ${UPPER}/usr/lib /mnt/udisk/recordings" >/dev/null
adb push "${SCRIPT_DIR}/S60waved" "${UPPER}/etc/init.d/S60waved" >/dev/null
adb push "${SCRIPT_DIR}/S03splash" "${UPPER}/etc/init.d/S03splash" >/dev/null
adb push "${SCRIPT_DIR}/splash.fb" /data/waved/splash.fb >/dev/null
adb push "${SCRIPT_DIR}/rsyslog.conf" "${UPPER}/etc/rsyslog.conf" >/dev/null
adb push "${SCRIPT_DIR}/iptables.rules" /data/waved/iptables.rules >/dev/null
on_wave "chmod 0600 /data/waved/config.toml; \
chmod 755 ${UPPER}/etc/init.d/S60waved ${UPPER}/etc/init.d/S03splash; \
printf '#!/bin/sh\n' > ${UPPER}/etc/init.d/S60pspapp_daemon; chmod 755 ${UPPER}/etc/init.d/S60pspapp_daemon; \
: > ${UPPER}/usr/lib/libapplogrs.so" >/dev/null

echo "=== Patching the USB gadget: ADB at boot, volume ranges, HID controls ==="
adb push "${WAVED_DIR}/scripts/patch-stock-usbgadget.sh" /tmp/patch-usbgadget.sh >/dev/null
on_wave "sh /tmp/patch-usbgadget.sh; rm -f /tmp/patch-usbgadget.sh"

# Without this the Wave has no ADB after the reboot, and revert.sh can't reach it.
[ "$(on_wave "grep -c '^[[:space:]]*#[[:space:]]*adbd_enable[[:space:]]*\$' /etc/init.d/S89usbgadget")" = "0" ] \
    || die "ADB is not enabled at boot (/etc/init.d/S89usbgadget). Do not reboot; see Troubleshooting: \"ADB at boot\"."

echo "=== Done. Reboot the Wave to start waved: adb reboot ==="
