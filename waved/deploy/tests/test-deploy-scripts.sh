#!/usr/bin/env bash
# shellcheck disable=SC2016,SC2034  # check() takes single-quoted expressions and eval's them (they read $rc)
# Runs deploy/install.sh and deploy/revert.sh against a fake adb that logs
# every call and answers the few questions the scripts ask. No Wave needed.
#
# The fake runs the two device commands whose logic matters (the gadget patch
# and the binary swap) as real shell with the programs they call replaced by
# loggers, so "swap only if waved has exited" and "patched or die" are tested
# as written, not assumed. Knobs (environment variables of the fake):
#   FAKE_STATE            adb get-state answer              (device)
#   FAKE_FREE_KB          free KB on /data                  (177876)
#   FAKE_CONFIG_EXISTS    config.toml already on the Wave   (no)
#   FAKE_COMMENTED_ADBD   commented adbd_enable lines left  (0)
#   FAKE_MD5              md5 the Wave reports instead of the real one
#   FAKE_PATCH            ok | fail: does patch-usbgadget.sh succeed (ok)
#   FAKE_WAVED_BUSY       1: waved is still running at the swap   (0)
#   FAKE_PUSH_FAIL        fail any push whose paths contain this text
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
DEPLOY="$(cd "$HERE/.." && pwd)"
FAILS=0

setup() {
    T="$(mktemp -d)"
    mkdir -p "$T/bin" "$T/pushed"
    export FAKE_ADB_LOG="$T/adb.log" FAKE_ADB_DIR="$T/pushed"
    export FAKE_STATE=device FAKE_FREE_KB=177876 FAKE_CONFIG_EXISTS=no FAKE_COMMENTED_ADBD=0 FAKE_MD5=
    export FAKE_PATCH=ok FAKE_WAVED_BUSY=0 FAKE_PUSH_FAIL=
    : > "$FAKE_ADB_LOG"
    cat > "$T/bin/adb" <<'ADB'
#!/usr/bin/env bash
echo "$*" >> "$FAKE_ADB_LOG"
case "$1" in
  get-state) echo "$FAKE_STATE" ;;
  push)
    if [ -n "$FAKE_PUSH_FAIL" ]; then
      case "$2 $3" in *"$FAKE_PUSH_FAIL"*) echo "adb: error: failed to copy '$2' to '$3'" >&2; exit 1 ;; esac
    fi
    cp "$2" "$FAKE_ADB_DIR/$(echo "$3" | tr / _)"; echo "1 file pushed" ;;
  shell)
    shift; cmd="$*"
    case "$cmd" in
      *"df -k /data"*) echo "/dev/data 450360 237580 $FAKE_FREE_KB 57% /data" ;;
      *"du -k /data/waved/waved"*) echo 0 ;;
      *"md5sum /tmp/waved.new"*)
        if [ -n "$FAKE_MD5" ]; then echo "$FAKE_MD5  /tmp/waved.new"
        else f="$FAKE_ADB_DIR/_tmp_waved.new"; (md5sum "$f" 2>/dev/null || md5 -r "$f") | awk '{print $1"  /tmp/waved.new"}'; fi ;;
      *"config.toml ] && echo yes"*) echo "$FAKE_CONFIG_EXISTS" ;;
      *"sh /tmp/patch-usbgadget.sh"*)
        ( sh() { echo "patched adbd at boot"; [ "$FAKE_PATCH" = ok ]; }
          rm() { echo "EXEC rm $*" >> "$FAKE_ADB_LOG"; }
          eval "$cmd" ) ;;
      *"adbd_enable"*) echo "$FAKE_COMMENTED_ADBD" ;;
      *"cp /tmp/waved.new /data/waved/waved"*)
        ( pidof() { [ "$FAKE_WAVED_BUSY" = 1 ]; }
          mkdir() { echo "EXEC mkdir $*" >> "$FAKE_ADB_LOG"; }
          rm() { echo "EXEC rm $*" >> "$FAKE_ADB_LOG"; }
          cp() { echo "EXEC cp $*" >> "$FAKE_ADB_LOG"; }
          chmod() { echo "EXEC chmod $*" >> "$FAKE_ADB_LOG"; }
          eval "$cmd" ) ;;
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
check swaps_the_binary 'grep -q "^EXEC cp /tmp/waved.new /data/waved/waved" "$FAKE_ADB_LOG"'
check runs_usb_gadget_patch 'grep -q "^shell sh /tmp/patch-usbgadget.sh" "$FAKE_ADB_LOG"'
check patches_gadget_before_stopping_anything '[ "$(line_of "shell sh /tmp/patch-usbgadget.sh")" -lt "$(line_of "killall pspapp")" ]'
check checks_adb_at_boot_before_stopping_anything '[ "$(line_of "adbd_enable")" -lt "$(line_of "killall pspapp")" ]'
# Overlay files go through the /etc and /usr mounts, never into the upper dir.
check pushes_S60waved_to_etc 'grep -q "^push .*/S60waved /etc/init.d/S60waved$" "$FAKE_ADB_LOG"'
check pushes_S03splash_to_etc 'grep -q "^push .*/S03splash /etc/init.d/S03splash$" "$FAKE_ADB_LOG"'
check pushes_rsyslog_to_etc 'grep -q "^push .*/rsyslog.conf /etc/rsyslog.conf$" "$FAKE_ADB_LOG"'
check pushes_splash_fb 'grep -q "^push .*/splash.fb /data/waved/splash.fb$" "$FAKE_ADB_LOG"'
check pushes_iptables_rules 'grep -q "^push .*/iptables.rules /data/waved/iptables.rules$" "$FAKE_ADB_LOG"'
check pushes_default_config_when_wave_has_none 'grep -q "default.toml /data/waved/config.toml" "$FAKE_ADB_LOG"'
check never_touches_the_upper_dir '! grep -q "/data/overlay/upper" "$FAKE_ADB_LOG"'
check stubs_come_last 'tail -n 1 "$FAKE_ADB_LOG" | grep -q "S60pspapp_daemon" && tail -n 1 "$FAKE_ADB_LOG" | grep -q "/usr/lib/libapplogrs.so"'
check stub_is_written_through_etc 'grep -q "> /etc/init.d/S60pspapp_daemon" "$FAKE_ADB_LOG"'
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

# ADB not enabled at boot: stop before the stock app is touched.
setup; FAKE_COMMENTED_ADBD=2 "$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check refuses_when_adb_is_not_enabled_at_boot '[ $rc -ne 0 ] && grep -q "ERROR: ADB is not enabled at boot" "$T/out" && ! grep -q "adb reboot" "$T/out"'
check adb_refusal_leaves_the_stock_app_alone '! grep -q -e "killall" -e "S60pspapp_daemon" -e "libapplogrs.so" -e "/etc/init.d/S60waved" "$FAKE_ADB_LOG"'

# The gadget patch must say "patched" as its last word, or install stops.
setup; FAKE_PATCH=fail "$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check refuses_when_the_patch_does_not_finish '[ $rc -ne 0 ] && grep -q "ERROR: the USB gadget patch did not finish" "$T/out" && ! grep -q "adb reboot" "$T/out"'
check patch_refusal_leaves_the_stock_app_alone '! grep -q -e "killall" -e "S60pspapp_daemon" -e "libapplogrs.so" -e "/etc/init.d/S60waved" -e "^EXEC cp" "$FAKE_ADB_LOG"'

# waved still alive at the swap: do not touch the binary, push nothing after.
setup; FAKE_WAVED_BUSY=1 "$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check refuses_the_swap_while_waved_runs '[ $rc -ne 0 ] && grep -q "ERROR: waved did not stop; nothing was replaced" "$T/out"'
check busy_waved_binary_is_not_replaced '! grep -q -e "^EXEC rm -f /data/waved/waved" -e "^EXEC cp" "$FAKE_ADB_LOG"'
check busy_waved_means_no_later_pushes '! grep -q -e "/etc/init.d/S60waved" -e S03splash -e rsyslog -e splash.fb -e iptables -e default.toml -e S60pspapp_daemon -e libapplogrs.so "$FAKE_ADB_LOG"'

# An adb failure under set -e names the step; the stubs are never reached.
setup; FAKE_PUSH_FAIL=/tmp/waved.new "$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check early_adb_failure_names_the_step '[ $rc -ne 0 ] && grep -q "ERROR: install.sh stopped at: copying waved to /tmp" "$T/out"'
setup; FAKE_PUSH_FAIL=/etc/init.d/S03splash "$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check late_adb_failure_names_the_step '[ $rc -ne 0 ] && grep -q "ERROR: install.sh stopped at: installing start-up scripts" "$T/out"'
check late_adb_failure_never_stubs_the_stock_app '! grep -q -e "S60pspapp_daemon" -e "libapplogrs.so" "$FAKE_ADB_LOG"'

setup; WAVED_BINARY="$T/missing" "$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check missing_binary_explains_build '[ $rc -ne 0 ] && grep -q "cross build" "$T/out"'

# revert: deletes in the overlay's upper dir (deleting through the mount would
# leave whiteouts that hide the stock files). Exact non-dot names, so the
# .prev/.new copies alone cannot satisfy them.
UP=/data/overlay/upper
setup; "$DEPLOY/revert.sh" > "$T/out" 2>&1; rc=$?
check revert_succeeds '[ $rc -eq 0 ]'
for f in $UP/etc/init.d/S60waved $UP/etc/init.d/S60pspapp_daemon $UP/etc/init.d/S03splash $UP/etc/init.d/S89usbgadget $UP/etc/rsyslog.conf $UP/usr/lib/libapplogrs.so; do
    check "revert_removes_$f" 'grep -q -F -- "$f " "$FAKE_ADB_LOG"'
done
check revert_removes_the_waved_dir 'grep -q "rm -rf /data/waved" "$FAKE_ADB_LOG"'
check revert_deletes_only_in_the_upper_dir '! grep -q -E "rm [^;]* /(etc|usr)/" "$FAKE_ADB_LOG"'
setup; FAKE_STATE=unknown "$DEPLOY/revert.sh" > /dev/null 2>&1; rc=$?
check revert_refuses_without_device '[ $rc -ne 0 ] && ! grep -q "rm " "$FAKE_ADB_LOG"'

echo
if [ "$FAILS" -eq 0 ]; then echo "all passed"; else echo "$FAILS failed"; exit 1; fi
