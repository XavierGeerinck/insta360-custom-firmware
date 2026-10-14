#!/bin/bash
# Install waved on an Insta360 Wave over adb, or update the waved already on
# it. Safe to run again: an update keeps the Wave's settings.
#
# What it changes on the Wave, all of it on /data (the read-only system
# partition is never touched; deploy/revert.sh removes it all):
#   /data/waved/                waved; config.toml, written only if there is
#                               none (an existing one is never replaced, and is
#                               copied to config.toml.before-<UTC time> first:
#                               a newer build may save keys an older one
#                               rejects, docs/UPDATES.md "Going back over
#                               adb"); splash.fb; iptables.rules
#   the /etc and /usr overlays  (stored under /data/overlay/upper): S60waved,
#                               S03splash, rsyslog.conf (keeps the stock log
#                               from filling /data), the patched S89usbgadget
#                               (ADB at boot; its copy from before the patch
#                               goes to /data/waved), a stub S60pspapp_daemon
#                               and an empty libapplogrs.so (the stock app and
#                               its telemetry, off)
# Overlay files are written through the /etc and /usr mounts, as the
# over-the-air updater writes them, never into /data/overlay/upper directly.
#
# The order is the safety net (docs/DISPLAY_HARDWARE.md#deploy). A Wave
# without the stock app and without ADB can't be reached again, and /data is
# too small for two binaries:
#   1. Checks; nothing changes until they pass: the Wave answers, no
#      over-the-air update is on trial, /data has room.
#   2. Every file to /tmp (RAM), each checked by md5.
#   3. A copy of config.toml.
#   4. The USB gadget patch, then a check that ADB comes up at boot. Nothing
#      is stopped or stubbed before this has passed.
#   5. Stop the stock app, the watchdog and waved; swap the binary only once
#      waved has exited. The old one waits in /tmp/waved.old and goes back if
#      the new one does not fit.
#   6. S60waved, S03splash, rsyslog.conf, splash.fb and iptables.rules, each
#      through a dot-named copy and a rename; config.toml if there was none.
#   7. Last, the stubs that keep the stock app and its telemetry off, read
#      back to check they took.
#   8. An update starts the watchdog again; a first install (stock firmware
#      was running) ends with "adb reboot".
# A failure or Ctrl-C from step 5 on starts the watchdog again after an
# update, and says "adb reboot" after a first install. Every test of the
# Wave's files runs on the Wave (adb shell), never here, and adb shell's exit
# status is not reliable across adb versions, so every step on the Wave
# prints its answer and this script checks it.
#
# Env: WAVED_BINARY  the build to install
#                    (default target/aarch64-unknown-linux-gnu/release/waved)
#      OBJCOPY       llvm-objcopy (default: rustc's own, from
#                    `rustup component add llvm-tools`)
#
# The scripts run on the Wave are single-quoted on purpose: they expand there.
# shellcheck disable=SC2016
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY="${WAVED_BINARY:-$ROOT/target/aarch64-unknown-linux-gnu/release/waved}"
DEFAULT_CONFIG="$ROOT/config/default.toml"
GADGET_PATCH="$ROOT/scripts/patch-stock-usbgadget.sh"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
START_WATCHDOG='start-stop-daemon -S -b -m -p /tmp/S60waved.pid -x /bin/sh -- /etc/init.d/S60waved start'
MARGIN_KB=20480   # room left on /data after the swap: logs, the recordings database
UPDATE=no
STOPPED=no
STEP="starting"

die() {
    # Once here, see it through: a second Ctrl-C must not cut the watchdog
    # restart short (adb, started from here, ignores it too).
    trap - ERR
    trap '' INT TERM
    echo "ERROR: $*" >&2
    # Never leave a Wave that ran waved without its watchdog.
    if [ "$STOPPED" = yes ] && [ "$UPDATE" = yes ]; then
        if wave_ok "$START_WATCHDOG && echo ok"; then
            echo "The watchdog runs again." >&2
        else
            echo "Could not start the watchdog: $START_WATCHDOG" >&2
        fi
    elif [ "$STOPPED" = yes ]; then
        echo "Stock firmware was stopped: adb reboot, then run this again." >&2
    fi
    exit 1
}
# bash runs no ERR trap inside functions (set -E is no cure: grep -c fails on
# a zero count), so every adb call in one ends in an explicit || die.
trap 'die "install.sh stopped during: $STEP"' ERR
trap 'die "install.sh interrupted during: $STEP"' INT TERM

# A section: its heading, and its name for the error messages.
step() {
    STEP="$1"
    echo "=== $1 ==="
}

# What a command printed on the Wave, without adb's carriage returns.
on_wave() {
    adb shell "$1" | tr -d '\r'
}

# Run a script on the Wave that prints "ok" last when every step worked.
# Shows anything else it printed.
wave_ok() {
    local out
    out="$(on_wave "$1")" || true
    [ -z "$out" ] || printf '%s\n' "$out" | grep -vx ok || true
    [ "$(printf '%s\n' "$out" | tail -n 1)" = ok ]
}

# Whether $1 exists on the Wave. No answer stops the install: a wrong "no"
# would make an update look like a first install.
wave_has() {
    local answer
    answer="$(on_wave "if [ -e '$1' ]; then echo yes; else echo no; fi")" || answer=""
    case "$answer" in
        yes) return 0 ;;
        no) return 1 ;;
        *) die "no answer from the Wave about $1 (adb devices); nothing was changed" ;;
    esac
}

local_md5() {
    if command -v md5sum >/dev/null 2>&1; then
        md5sum "$1" | cut -d' ' -f1
    else
        md5 -q "$1"
    fi
}

wave_md5() {
    on_wave "md5sum '$1' 2>/dev/null" | cut -d' ' -f1
}

# Push $1 to $2 (in /tmp on the Wave) and check that it arrived whole.
push_checked() {
    local want got
    adb push "$1" "$2" >/dev/null ||
        die "could not copy $1 to $2 on the Wave (adb push); nothing was changed"
    want="$(local_md5 "$1")" || die "could not read $1"
    got="$(wave_md5 "$2")" || got=""
    [ "$want" = "$got" ] ||
        die "$2 on the Wave is not $1 (md5 ${got:-none}, want $want); nothing was changed"
}

if [ ! -f "$BINARY" ]; then
    die "no waved binary at $BINARY
Build it first, from the waved/ directory (docs/DISPLAY_HARDWARE.md#deploy):
  WAVED_BUILD_ID=\$(git rev-parse --short HEAD) cross build --release \\
    --target aarch64-unknown-linux-gnu --features \"ui,capture-alsa\" -p waved"
fi
if [ -z "${OBJCOPY:-}" ]; then
    host="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')" || host=""
    OBJCOPY="$(rustc --print sysroot 2>/dev/null || true)/lib/rustlib/$host/bin/llvm-objcopy"
fi
command -v "$OBJCOPY" >/dev/null 2>&1 ||
    die "no llvm-objcopy at $OBJCOPY (rustup component add llvm-tools, or set OBJCOPY)"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

step "Stripping waved"
"$OBJCOPY" --strip-all "$BINARY" "$WORK/waved" || die "could not strip $BINARY with $OBJCOPY"
WAVED_MD5="$(local_md5 "$WORK/waved")"
SIZE_KB=$(( ($(wc -c < "$WORK/waved") + 1023) / 1024 ))

step "Checking the Wave"
[ "$(on_wave 'echo ready' || true)" = ready ] ||
    die "no Wave on adb (adb devices). Connect it over USB with ADB on (Install, step 1)."
if wave_has /data/waved/update/trial; then
    die "an over-the-air update is on trial (/data/waved/update/trial): installing now would \
be undone when it commits or goes back. Wait until it has (about a minute after its second \
start; docs/UPDATES.md), then run this again."
fi
if wave_has /data/waved/waved; then UPDATE=yes; fi
HAS_CONFIG=no
if wave_has /data/waved/config.toml; then HAS_CONFIG=yes; fi

# The old binary's space comes back at the swap, so it counts as free.
step "Checking free space on /data"
free_kb="$(on_wave 'df -k /data | tail -n 1' | awk '{print $4}')" || free_kb=""
old_kb="$(on_wave 'if [ -f /data/waved/waved ]; then du -k /data/waved/waved | cut -f1; else echo 0; fi')" ||
    old_kb=""
case "$free_kb" in
    "" | *[!0-9]*) die "could not read the free space on /data (${free_kb:-no answer}); nothing was changed" ;;
esac
case "$old_kb" in
    "" | *[!0-9]*) die "could not read the size of /data/waved/waved (${old_kb:-no answer}); nothing was changed" ;;
esac
need_kb=$(( SIZE_KB + MARGIN_KB ))
[ $(( free_kb + old_kb )) -ge "$need_kb" ] ||
    die "/data has $(( (free_kb + old_kb) / 1024 )) MB available and waved needs $(( need_kb / 1024 )) MB. \
Nothing was changed. See Troubleshooting: \"/data is full\"."
echo "    $(( (free_kb + old_kb) / 1024 )) MB available, waved needs $(( need_kb / 1024 )) MB"

step "Pushing to /tmp on the Wave (md5 checked)"
push_checked "$WORK/waved" /tmp/waved.new
push_checked "$SCRIPT_DIR/S60waved" /tmp/S60waved.new
push_checked "$SCRIPT_DIR/S03splash" /tmp/S03splash.new
push_checked "$SCRIPT_DIR/rsyslog.conf" /tmp/rsyslog.conf.new
push_checked "$SCRIPT_DIR/splash.fb" /tmp/splash.fb.new
push_checked "$SCRIPT_DIR/iptables.rules" /tmp/iptables.rules.new
push_checked "$GADGET_PATCH" /tmp/patch-usbgadget.sh
if [ "$HAS_CONFIG" = no ]; then
    push_checked "$DEFAULT_CONFIG" /tmp/config.toml.new
fi

if [ "$HAS_CONFIG" = yes ]; then
    step "Keeping a copy of config.toml"
    backup="/data/waved/config.toml.before-$STAMP"
    wave_ok "cp /data/waved/config.toml $backup && chmod 600 $backup && echo ok" ||
        die "could not copy config.toml; nothing was changed"
    echo "    $backup"
fi

# ADB at boot first, while nothing is stopped. Stock S89usbgadget keeps adbd
# commented out and leaves ADB to the stock app, which waved replaces: without
# the patch a Wave has no ADB after the next reboot, and revert.sh can't reach
# it. On an update the patch finds it done ("already patched").
step "Patching the USB gadget: ADB at boot, volume ranges, HID controls"
patch_out="$(on_wave 'sh /tmp/patch-usbgadget.sh 2>&1 && echo ok; rm -f /tmp/patch-usbgadget.sh')" ||
    patch_out=""
if [ "$(printf '%s\n' "$patch_out" | tail -n 1)" != ok ]; then
    # Its closing "reboot the device ... adb reboot" is for running it by hand.
    [ -z "$patch_out" ] ||
        printf '%s\n' "$patch_out" | grep -v -e '^reboot the device' -e '^ *adb reboot$' >&2 || true
    die "the USB gadget patch did not finish. Nothing was stopped: the Wave still runs what it ran \
before. Only /etc/init.d/S89usbgadget may have changed; its copy from before the patch is in \
/data/waved (S89usbgadget.bak.<time>). See Troubleshooting: \"ADB at boot\"."
fi
# Only what it did: the reboot comes once everything is in place.
printf '%s\n' "$patch_out" | grep -E '^(already patched|patched|removed) ' | sed 's/^/    /' || true

# Both must hold: no adbd_enable line in the gadget script is still commented
# out, and at least one live adbd_enable line is inside start_usbgadget(), the
# function the boot runs (from its declaration to the "}" in column 0 that
# closes it; start_usbgadget_ota_mode() does not count). busybox awk on the
# Wave; the answer is read from its output, not from an exit code.
step "Checking that ADB comes up at boot"
ADB_AWK='/^[[:space:]]*#[[:space:]]*adbd_enable[[:space:]]*$/ { commented++ }; /^[[:space:]]*(function[[:space:]]+)?start_usbgadget[[:space:]]*[(][)]/ { boot = 1 }; boot && /^}/ { boot = 0 }; boot && /^[[:space:]]*adbd_enable[[:space:]]*$/ { live++ }; END { print "commented=" commented+0 " live=" live+0 }'
adb_at_boot="$(on_wave "awk '$ADB_AWK' /etc/init.d/S89usbgadget")" || adb_at_boot=""
case "$adb_at_boot" in
    "commented=0 live="[1-9]*) echo "    $adb_at_boot" ;;
    *) die "ADB is not enabled at boot (/etc/init.d/S89usbgadget: ${adb_at_boot:-no answer}). Nothing \
was stopped: the Wave still runs what it ran before. Only /etc/init.d/S89usbgadget may have changed; \
its copy from before the patch is in /data/waved. See Troubleshooting: \"ADB at boot\"." ;;
esac

step "Stopping stock firmware, the watchdog and waved"
STOPPED=yes
# The watchdog is the S60waved start loop (started by rcS or start-stop-daemon);
# !/awk/ also skips the shell running this script. waved takes ~2 s to exit
# (the lift retracts first); its binary's space is free only once it has.
wave_ok 'killall pspapp pspapp_daemon 2>/dev/null; \
for pid in $(ps | awk "/S60waved start/ && !/awk/ {print \$1}"); do kill $pid; done; \
rm -f /tmp/S60waved.pid; sleep 1; killall waved 2>/dev/null; n=0; \
while pidof waved >/dev/null; do \
    n=$((n + 1)); \
    if [ $n -eq 30 ]; then killall -9 waved 2>/dev/null; fi; \
    if [ $n -ge 40 ]; then echo "waved does not exit"; exit 1; fi; \
    sleep 1; \
done; echo ok' || die "could not stop waved"

step "Swapping the binary"
# The old binary waits in /tmp/waved.old: if the new one does not fit on
# /data, it goes back.
swap='mkdir -p /data/waved && cd /data/waved || exit 1; \
if [ -f waved ]; then cp waved /tmp/waved.old || exit 1; fi; \
rm -f waved; \
if cp /tmp/waved.new waved && chmod 755 waved && \
   [ "$(md5sum waved | cut -d" " -f1)" = WAVED_MD5 ]; then \
    sync; rm -f /tmp/waved.new; echo ok; \
else \
    rm -f waved; \
    if [ -f /tmp/waved.old ]; then cp /tmp/waved.old waved && chmod 755 waved; fi; \
    echo "the new binary did not fit on /data (df -h /data); the old one is back"; \
fi'
wave_ok "${swap//WAVED_MD5/$WAVED_MD5}" || die "the binary was not replaced"

step "Installing S60waved, S03splash, rsyslog.conf, the splash and the firewall rules"
# A dot name until the rename: rcS runs every /etc/init.d/S??* at boot. The
# /etc files go through the overlay mount, like everything else here.
wave_ok 'mkdir -p /etc/init.d && \
cp /tmp/S60waved.new /etc/init.d/.S60waved.new && chmod 755 /etc/init.d/.S60waved.new && \
mv -f /etc/init.d/.S60waved.new /etc/init.d/S60waved && \
cp /tmp/S03splash.new /etc/init.d/.S03splash.new && chmod 755 /etc/init.d/.S03splash.new && \
mv -f /etc/init.d/.S03splash.new /etc/init.d/S03splash && \
cp /tmp/rsyslog.conf.new /etc/.rsyslog.conf.new && chmod 644 /etc/.rsyslog.conf.new && \
mv -f /etc/.rsyslog.conf.new /etc/rsyslog.conf && \
cp /tmp/splash.fb.new /data/waved/.splash.fb.new && chmod 644 /data/waved/.splash.fb.new && \
mv -f /data/waved/.splash.fb.new /data/waved/splash.fb && \
cp /tmp/iptables.rules.new /data/waved/.iptables.rules.new && \
chmod 644 /data/waved/.iptables.rules.new && \
mv -f /data/waved/.iptables.rules.new /data/waved/iptables.rules && \
rm -f /tmp/S60waved.new /tmp/S03splash.new /tmp/rsyslog.conf.new /tmp/splash.fb.new \
    /tmp/iptables.rules.new && sync && echo ok' ||
    die "S60waved, S03splash, rsyslog.conf, splash.fb or iptables.rules not installed (the new binary is)"

if [ "$HAS_CONFIG" = no ]; then
    step "Writing the default config.toml"
    wave_ok 'if [ -e /data/waved/config.toml ]; then echo "config.toml exists; kept it"; \
else cp /tmp/config.toml.new /data/waved/config.toml && chmod 600 /data/waved/config.toml; fi && \
rm -f /tmp/config.toml.new && sync && echo ok' || die "config.toml not written"
fi

step "Stock pspapp off, telemetry library stubbed"
# Only where the overlay's upper directory has none yet (deploy/revert.sh
# deletes them there), but written through the /etc and /usr mounts: never
# into the upper directory under a mounted overlay.
wave_ok 'u=/data/overlay/upper; \
{ [ -f $u/etc/init.d/S60pspapp_daemon ] || \
  { echo "#!/bin/sh" > /etc/init.d/S60pspapp_daemon && chmod 755 /etc/init.d/S60pspapp_daemon; }; } && \
{ [ -f $u/usr/lib/libapplogrs.so ] || : > /usr/lib/libapplogrs.so; } && \
sync && echo ok' || die "could not stub the stock services"
# Read both back through the mounts: a stub that did not take leaves the stock
# app starting next to waved at the next boot. libapplogrs.so is empty, or
# one newline on a Wave installed by an older install.sh.
wave_ok '[ "$(cat /etc/init.d/S60pspapp_daemon 2>/dev/null)" = "#!/bin/sh" ] && \
[ -f /usr/lib/libapplogrs.so ] && n=$(wc -c < /usr/lib/libapplogrs.so) && [ $((n)) -le 1 ] && \
echo ok || ls -l /etc/init.d/S60pspapp_daemon /usr/lib/libapplogrs.so 2>&1' ||
    die "the stock app and telemetry stubs did not take (/etc/init.d/S60pspapp_daemon, \
/usr/lib/libapplogrs.so): waved is installed, but the stock app would start next to it at the \
next boot. Run install.sh again; see Troubleshooting: \"The installer stopped part-way\"."
# S60waved makes it at every start too; it fails while USB storage has the
# volume (/mnt/udisk not mounted), which must not fail the install.
on_wave 'mkdir -p /mnt/udisk/recordings 2>/dev/null' >/dev/null || true

if [ "$UPDATE" = yes ]; then
    step "Starting the watchdog"
    wave_ok "$START_WATCHDOG && echo ok" || die "could not start the watchdog"
    STOPPED=no
    echo "=== Done. The previous binary is in /tmp/waved.old until the Wave restarts. ==="
else
    echo "=== Done! Reboot to start waved ==="
    echo "Run: adb reboot"
fi
