#!/bin/bash
# Install waved on a Wave over adb, or update the waved already on it.
# docs/DISPLAY_HARDWARE.md#deploy explains every step. In short:
# - Every test of the Wave's files runs on the Wave (adb shell), never here.
# - /data/waved/config.toml on the Wave is never replaced. It is copied to
#   /data/waved/config.toml.before-<UTC time> first: a newer build may save
#   keys an older one rejects (docs/UPDATES.md, "Going back over adb").
# - The binary is stripped, pushed to /tmp (RAM; /data is nearly full) and
#   checked by md5, then swapped in only once the watchdog is stopped and waved
#   has exited. S60waved and iptables.rules go the same way, through /tmp.
# - An update starts the watchdog again; a first install (stock firmware
#   running) ends with "reboot".
# adb shell's exit status is not reliable across adb versions, so every step
# on the Wave prints its answer and this script checks it.
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
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
START_WATCHDOG='start-stop-daemon -S -b -m -p /tmp/S60waved.pid -x /bin/sh -- /etc/init.d/S60waved start'
UPDATE=no
STOPPED=no

die() {
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

wave_has() {
    [ "$(on_wave "if [ -e '$1' ]; then echo yes; else echo no; fi")" = yes ]
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
    adb push "$1" "$2" >/dev/null
    want="$(local_md5 "$1")"
    got="$(wave_md5 "$2")"
    [ "$want" = "$got" ] || die "$2 on the Wave is not $1 (md5 ${got:-none}, want $want)"
}

if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY" >&2
    echo "Build it first (docs/DISPLAY_HARDWARE.md#deploy):" >&2
    echo "  WAVED_BUILD_ID=\$(git rev-parse --short HEAD) cross build --release \\" >&2
    echo "    --target aarch64-unknown-linux-gnu --features \"ui,capture-alsa\" -p waved" >&2
    exit 1
fi
if [ -z "${OBJCOPY:-}" ]; then
    host="$(rustc -vV | sed -n 's/^host: //p')"
    OBJCOPY="$(rustc --print sysroot)/lib/rustlib/$host/bin/llvm-objcopy"
fi
command -v "$OBJCOPY" >/dev/null 2>&1 ||
    die "no llvm-objcopy at $OBJCOPY (rustup component add llvm-tools, or set OBJCOPY)"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "=== Stripping waved ==="
"$OBJCOPY" --strip-all "$BINARY" "$WORK/waved"
WAVED_MD5="$(local_md5 "$WORK/waved")"

echo "=== Checking the Wave ==="
[ "$(on_wave 'echo ready' || true)" = ready ] || die "no Wave on adb (adb devices)"
if wave_has /data/waved/update/trial; then
    die "an over-the-air update is on trial (/data/waved/update/trial): installing now would \
be undone when it commits or goes back. Wait until it has (about a minute after its second \
start; docs/UPDATES.md), then run this again."
fi
if wave_has /data/waved/waved; then UPDATE=yes; fi
HAS_CONFIG=no
if wave_has /data/waved/config.toml; then HAS_CONFIG=yes; fi

echo "=== Pushing to /tmp on the Wave (md5 checked) ==="
push_checked "$WORK/waved" /tmp/waved.new
push_checked "$SCRIPT_DIR/S60waved" /tmp/S60waved.new
push_checked "$SCRIPT_DIR/iptables.rules" /tmp/iptables.rules.new
if [ "$HAS_CONFIG" = no ]; then
    push_checked "$DEFAULT_CONFIG" /tmp/config.toml.new
fi

if [ "$HAS_CONFIG" = yes ]; then
    echo "=== Keeping a copy of config.toml ==="
    backup="/data/waved/config.toml.before-$STAMP"
    wave_ok "cp /data/waved/config.toml $backup && chmod 600 $backup && echo ok" ||
        die "could not copy config.toml; nothing was changed"
    echo "    $backup"
fi

echo "=== Stopping stock firmware, the watchdog and waved ==="
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

echo "=== Swapping the binary ==="
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

echo "=== Installing S60waved and the firewall rules ==="
# A dot name until the rename: rcS runs every /etc/init.d/S??* at boot.
wave_ok 'mkdir -p /etc/init.d && \
cp /tmp/S60waved.new /etc/init.d/.S60waved.new && chmod 755 /etc/init.d/.S60waved.new && \
mv -f /etc/init.d/.S60waved.new /etc/init.d/S60waved && \
cp /tmp/iptables.rules.new /data/waved/.iptables.rules.new && \
chmod 644 /data/waved/.iptables.rules.new && \
mv -f /data/waved/.iptables.rules.new /data/waved/iptables.rules && \
rm -f /tmp/S60waved.new /tmp/iptables.rules.new && sync && echo ok' ||
    die "S60waved or iptables.rules not installed (the new binary is)"

if [ "$HAS_CONFIG" = no ]; then
    echo "=== Writing the default config.toml ==="
    wave_ok 'if [ -e /data/waved/config.toml ]; then echo "config.toml exists; kept it"; \
else cp /tmp/config.toml.new /data/waved/config.toml && chmod 600 /data/waved/config.toml; fi && \
rm -f /tmp/config.toml.new && sync && echo ok' || die "config.toml not written"
fi

echo "=== Stock pspapp off, telemetry library stubbed ==="
# Only where they are missing (a first install): these write the overlay's
# upper directory, as deploy/revert.sh removes them from it.
wave_ok 'u=/data/overlay/upper; mkdir -p $u/etc/init.d $u/usr/lib && \
{ [ -f $u/etc/init.d/S60pspapp_daemon ] || \
  { echo "#!/bin/sh" > $u/etc/init.d/S60pspapp_daemon && chmod 755 $u/etc/init.d/S60pspapp_daemon; }; } && \
{ [ -f $u/usr/lib/libapplogrs.so ] || echo "" > $u/usr/lib/libapplogrs.so; } && \
sync && echo ok' || die "could not stub the stock services"
# S60waved makes it at every start too; it fails while USB storage has the
# volume (/mnt/udisk not mounted), which must not fail the install.
on_wave 'mkdir -p /mnt/udisk/recordings 2>/dev/null' >/dev/null || true

if [ "$UPDATE" = yes ]; then
    echo "=== Starting the watchdog ==="
    wave_ok "$START_WATCHDOG && echo ok" || die "could not start the watchdog"
    STOPPED=no
    echo "=== Done. The previous binary is in /tmp/waved.old until the Wave restarts. ==="
else
    echo "=== Done! Reboot to start waved ==="
    echo "Run: adb reboot"
fi
