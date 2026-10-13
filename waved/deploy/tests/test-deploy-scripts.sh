#!/usr/bin/env bash
# shellcheck disable=SC2016,SC2034  # check() takes single-quoted expressions and eval's them (they read $rc)
# Runs deploy/install.sh and deploy/revert.sh against a fake adb that logs
# every call and answers the few questions the scripts ask. No Wave needed.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
DEPLOY="$(cd "$HERE/.." && pwd)"
FAILS=0

setup() {
    T="$(mktemp -d)"
    mkdir -p "$T/bin" "$T/pushed"
    export FAKE_ADB_LOG="$T/adb.log" FAKE_ADB_DIR="$T/pushed"
    export FAKE_STATE=device FAKE_FREE_KB=177876 FAKE_CONFIG_EXISTS=no FAKE_COMMENTED_ADBD=0 FAKE_MD5=
    : > "$FAKE_ADB_LOG"
    cat > "$T/bin/adb" <<'ADB'
#!/usr/bin/env bash
echo "$*" >> "$FAKE_ADB_LOG"
case "$1" in
  get-state) echo "$FAKE_STATE" ;;
  push) cp "$2" "$FAKE_ADB_DIR/$(echo "$3" | tr / _)"; echo "1 file pushed" ;;
  shell)
    shift; cmd="$*"
    case "$cmd" in
      *"df -k /data"*) echo "/dev/data 450360 237580 $FAKE_FREE_KB 57% /data" ;;
      *"du -k /data/waved/waved"*) echo 0 ;;
      *"md5sum /tmp/waved.new"*)
        if [ -n "$FAKE_MD5" ]; then echo "$FAKE_MD5  /tmp/waved.new"
        else f="$FAKE_ADB_DIR/_tmp_waved.new"; (md5sum "$f" 2>/dev/null || md5 -r "$f") | awk '{print $1"  /tmp/waved.new"}'; fi ;;
      *"config.toml ] && echo yes"*) echo "$FAKE_CONFIG_EXISTS" ;;
      *"adbd_enable"*) echo "$FAKE_COMMENTED_ADBD" ;;
      *"cp /tmp/waved.new /data/waved/waved"*) echo ok ;;
    esac ;;
esac
ADB
    chmod +x "$T/bin/adb"
    printf '#!/bin/sh\ncp "$2" "$3"\n' > "$T/bin/objcopy"; chmod +x "$T/bin/objcopy"
    head -c 4096 /dev/urandom > "$T/waved"
    export PATH="$T/bin:$PATH" OBJCOPY="$T/bin/objcopy" WAVED_BINARY="$T/waved"
    # Never let a script under test reach a real Wave.
    if [ "$(command -v adb)" != "$T/bin/adb" ]; then
        echo "ABORT: adb does not resolve to the fake ($(command -v adb))" >&2
        exit 2
    fi
}

check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; FAILS=$((FAILS+1)); fi; }
line_of() { grep -n -F -- "$1" "$FAKE_ADB_LOG" | head -1 | cut -d: -f1; }

# install: happy path
setup
"$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check installs_and_succeeds '[ $rc -eq 0 ]'
check pushes_to_tmp_before_swapping 'grep -n -F "/tmp/waved.new" "$FAKE_ADB_LOG" | head -1 | grep -q ":push " && [ "$(line_of "/tmp/waved.new")" -lt "$(line_of "cp /tmp/waved.new /data/waved/waved")" ]'
check stops_stock_before_swapping '[ "$(line_of "killall pspapp")" -lt "$(line_of "cp /tmp/waved.new /data/waved/waved")" ]'
for f in S60waved S03splash splash.fb rsyslog.conf iptables.rules; do
    check "pushes_$f" 'grep -q -- "$f" "$FAKE_ADB_LOG"'
done
check pushes_default_config_when_wave_has_none 'grep -q "default.toml /data/waved/config.toml" "$FAKE_ADB_LOG"'
check runs_usb_gadget_patch 'grep -q "sh /tmp/patch-usbgadget.sh" "$FAKE_ADB_LOG"'
check says_reboot 'grep -q "adb reboot" "$T/out"'

# install: keeps an existing config
setup; FAKE_CONFIG_EXISTS=yes "$DEPLOY/install.sh" > /dev/null 2>&1
check keeps_existing_config '! grep -q "default.toml" "$FAKE_ADB_LOG"'

# install: refusals
setup; FAKE_MD5=deadbeef "$DEPLOY/install.sh" > /dev/null 2>&1; rc=$?
check md5_mismatch_refuses '[ $rc -ne 0 ] && ! grep -q "cp /tmp/waved.new /data/waved/waved" "$FAKE_ADB_LOG"'

setup; FAKE_FREE_KB=1000 "$DEPLOY/install.sh" > /dev/null 2>&1; rc=$?
check low_space_refuses_before_touching_the_wave '[ $rc -ne 0 ] && ! grep -q -e "^push" -e "killall" "$FAKE_ADB_LOG"'

setup; FAKE_STATE=unknown "$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check no_device_refuses '[ $rc -ne 0 ] && grep -q "adb" "$T/out"'

setup; FAKE_COMMENTED_ADBD=2 "$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check refuses_when_adb_is_not_enabled_at_boot '[ $rc -ne 0 ] && ! grep -q "adb reboot" "$T/out"'

setup; WAVED_BINARY="$T/missing" "$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check missing_binary_explains_build '[ $rc -ne 0 ] && grep -q "cross build" "$T/out"'

# revert
setup; "$DEPLOY/revert.sh" > "$T/out" 2>&1; rc=$?
check revert_succeeds '[ $rc -eq 0 ]'
for f in S60waved S60pspapp_daemon S03splash S89usbgadget rsyslog.conf libapplogrs.so "rm -rf /data/waved"; do
    check "revert_removes_$f" 'grep -q -- "$f" "$FAKE_ADB_LOG"'
done
setup; FAKE_STATE=unknown "$DEPLOY/revert.sh" > /dev/null 2>&1; rc=$?
check revert_refuses_without_device '[ $rc -ne 0 ] && ! grep -q "rm " "$FAKE_ADB_LOG"'

echo
if [ "$FAILS" -eq 0 ]; then echo "all passed"; else echo "$FAILS failed"; exit 1; fi
