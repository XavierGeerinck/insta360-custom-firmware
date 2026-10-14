#!/usr/bin/env bash
# shellcheck disable=SC2016,SC2034  # check() takes single-quoted expressions and eval's them (they read $rc)
# Runs deploy/install.sh and deploy/revert.sh against a fake adb. No Wave
# needed, and none reachable: the fake maps the Wave's /data, /tmp, /etc, /usr
# and /mnt into a directory ($W) and runs every `adb shell` script there with
# this computer's sh, as the Wave's busybox sh would run it. So the swap and
# its rollback, the gadget patch (the real scripts/patch-stock-usbgadget.sh,
# on an S89usbgadget-shaped fixture), the ADB-at-boot check, the installs and
# the stubs with their read-back are tested as written, not assumed. The
# busybox commands whose effect matters are fakes that log what they were
# asked (ps, kill, killall, pidof, start-stop-daemon, df, sleep, sync) and
# keep the state of a fake watchdog and waved in $W/run. $W/etc and $W/usr are
# the overlay mounts as the Wave shows them; after each script the fake copies
# every file it changed there into $W/data/overlay/upper, as the overlay's
# copy-up does. A script that changes the upper directory itself is logged as
# DIRECT-UPPER-WRITE.
#
# Knobs (environment variables of the fake):
#   FAKE_STATE         device, or anything else: no Wave answers  (device)
#   FAKE_FREE_KB       free KB on /data                           (177876)
#   FAKE_GADGET        the Wave's S89usbgadget: stock | patched |
#                      no_boot_adbd (none live in start_usbgadget) |
#                      missing                                    (stock)
#   FAKE_PATCH         real, or noop: the patch says "already patched"
#                      and changes nothing                        (real)
#   FAKE_CORRUPT       a pushed path whose copy gets one byte too many
#   FAKE_DATA_FULL     1: the new binary does not fit on /data    (0)
#   FAKE_WAVED_STUCK   1: waved never exits, not even on SIGKILL  (0)
#   FAKE_STUBS_STICK   0: writing the two stubs changes nothing   (1)
#   FAKE_NO_COPY_UP    1: writes through /etc and /usr never reach the
#                      upper directory (no overlay there)         (0)
#   FAKE_STUB_WRITE_FAILS  1: writing S60pspapp_daemon fails on the Wave,
#                      while the rest of that script runs on      (0)
#   FAKE_OVERLAY       /proc/mounts: release (/etc and /usr overlays) |
#                      root (debug mode: one overlay on /) | none  (release)
#   FAKE_CUT_SWAP      1: the swap is cut off after it removed the old binary
#                      and wrote half the new one, then Ctrl-C     (0)
#   FAKE_PUSH_FAIL    fail any push whose paths contain this text
#   FAKE_SHELL_FAIL    fail (adb exits 1) any shell command containing this text
#   FAKE_INTERRUPT     Ctrl-C (the signal to install.sh and the adb it waits
#                      for) at the first shell command containing this text
#   FAKE_SIGNAL        the signal FAKE_INTERRUPT sends              (INT)
#   FAKE_INTERRUPT_WAIT  a file to wait for before sending it
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
DEPLOY="$(cd "$HERE/.." && pwd)"
PATCH="$(cd "$DEPLOY/../scripts" && pwd)/patch-stock-usbgadget.sh"
STUB_LIB="$(cd "$DEPLOY/../scripts" && pwd)/libapplogrs.so"
FAILS=0
CHECKS=0
START="-S -b -m -p /tmp/S60waved.pid -x /bin/sh -- /etc/init.d/S60waved start"
OLD_CONFIG='[device]
name = "Board room"'
STOCK_DAEMON='#!/bin/sh
/usr/bin/pspapp_daemon &'

# The parts of stock 1.2.2.6's S89usbgadget that the patch and the ADB check
# read (scripts/test-patch-stock-usbgadget.sh has the same shape).
# start_usbgadget() is the function the boot runs; $1 and $2 are its two
# boot-branch lines. The stock file also has live adbd_enable lines outside
# it, in the modes the stock app switches to, and $3 goes into
# start_usbgadget_ota_mode(), whose name starts the same.
gadget() {
    cat <<EOF
#!/bin/sh
uac_init()
{
	echo -100 > functions/uac2.0/c_volume_min
	echo 1 > functions/uac2.0/c_volume_res
	echo -100 > functions/uac2.0/p_volume_min
	echo 1 > functions/uac2.0/p_volume_res
}

usb_flash()
{
	sleep 2
	echo fdd00000.crgudc2 > /sys/kernel/config/usb_gadget/amlogic/UDC
}

adbd_enable()
{
	/usr/bin/adbd &
}

function start_usbgadget_ota_mode(){
	usb_dir=/sys/kernel/config/usb_gadget/amlogic
	$3
	usb_flash
}

function start_usbgadget(){
    usb_dir=/sys/kernel/config/usb_gadget/amlogic
    if [ ! -d \${usb_dir} ]
    then
    {
		if [ -e \$config_file ]; then
			uac_enable
			$1
		else
			uac_enable
			$2
		fi
        sleep 4
		usb_flash
    }
    fi
}

case "\$1" in
  start)
		start_usbgadget &
		;;
  restart)
		all_disable
		adbd_enable
		;;
esac
EOF
}

setup() {
    T="$(mktemp -d)"
    W="$T/wave"
    mkdir -p "$T/bin" "$T/wave-bin" "$W/data/overlay/upper" "$W/tmp" "$W/etc/init.d" \
        "$W/usr/lib" "$W/mnt/udisk" "$W/run" "$W/proc"
    export FAKE_ADB_LOG="$T/adb.log" FAKE_WAVE="$W" FAKE_WAVE_BIN="$T/wave-bin"
    export FAKE_STATE=device FAKE_FREE_KB=177876 FAKE_GADGET=stock FAKE_PATCH=real FAKE_CORRUPT=''
    export FAKE_DATA_FULL=0 FAKE_WAVED_STUCK=0 FAKE_STUBS_STICK=1 FAKE_PUSH_FAIL='' FAKE_SHELL_FAIL=''
    export FAKE_INTERRUPT='' FAKE_NO_COPY_UP=0 FAKE_STUB_WRITE_FAILS=0 FAKE_OVERLAY=release
    export FAKE_CUT_SWAP=0 FAKE_SIGNAL=INT FAKE_INTERRUPT_WAIT=''
    : > "$FAKE_ADB_LOG"
    gadget "#adbd_enable" "# adbd_enable" "" > "$T/gadget-stock"
    gadget "" "" adbd_enable > "$T/gadget-no_boot_adbd"
    # What an earlier install left: this patch, applied.
    mkdir -p "$T/patched/init.d"
    cp "$T/gadget-stock" "$T/patched/init.d/S89usbgadget"
    INIT="$T/patched/init.d/S89usbgadget" BACKUP_DIR="$T/patched" sh "$PATCH" > /dev/null 2>&1
    cp "$T/patched/init.d/S89usbgadget" "$T/gadget-patched"
    # A stock Wave: the start-up script and library the stubs replace.
    printf '%s\n' "$STOCK_DAEMON" > "$W/etc/init.d/S60pspapp_daemon"
    chmod 755 "$W/etc/init.d/S60pspapp_daemon"
    printf 'ELF' > "$W/usr/lib/libapplogrs.so"
    printf '# stock rsyslog.conf\n' > "$W/etc/rsyslog.conf"

    cat > "$T/bin/adb" <<'ADB'
#!/usr/bin/env bash
echo "$*" >> "$FAKE_ADB_LOG"
W="$FAKE_WAVE"
# The Wave's paths, moved under $W: /data/... and a bare /data alike.
map() {
    printf '%s' "$1" | sed -E \
        -e "s#(^|[^[:alnum:]_.-])/(data|tmp|etc|usr|mnt|proc)/#\\1$W/\\2/#g" \
        -e "s#(^|[^[:alnum:]_.-])/(data|tmp|etc|usr|mnt|proc)([^[:alnum:]_./-]|\$)#\\1$W/\\2\\3#g"
}
# Ctrl-C (or a closed terminal: FAKE_SIGNAL) reaches the whole foreground job:
# install.sh, the subshells it waits in, and this adb. Only processes running
# deploy/install.sh are signalled.
interrupt() {
    local p=$PPID args n=0 chain=""
    if [ -n "$FAKE_INTERRUPT_WAIT" ]; then
        while [ ! -e "$FAKE_INTERRUPT_WAIT" ] && [ $n -lt 200 ]; do sleep 0.05; n=$((n + 1)); done
    fi
    # The whole chain first (a subshell signalled first may be gone before its
    # parent is found), then install.sh itself before the subshells under it.
    while [ "${p:-1}" -gt 1 ]; do
        args="$(ps -o args= -p "$p" 2>/dev/null)" || break
        case "$args" in *deploy/install.sh*) chain="$p $chain" ;; *) break ;; esac
        p="$(ps -o ppid= -p "$p" | tr -d ' ')"
    done
    for p in $chain; do kill -"$FAKE_SIGNAL" "$p"; done
    trap - INT TERM HUP
    kill -"$FAKE_SIGNAL" $$
    exit 130
}
export LC_ALL=C
# Every file under the overlay's upper dir, and every file /etc and /usr show.
upper_state() { (cd "$W/data/overlay/upper" && find . | sort && find . -type f -exec cksum {} + | sort); }
merged_state() { (cd "$W" && find etc usr -type f -exec cksum {} + | sort); }
case "$1" in
  get-state) echo "$FAKE_STATE" ;;
  push)
    [ "$FAKE_STATE" = device ] || { echo "adb: no devices/emulators found" >&2; exit 1; }
    if [ -n "$FAKE_PUSH_FAIL" ]; then
      case "$2 $3" in *"$FAKE_PUSH_FAIL"*) echo "adb: error: failed to copy '$2' to '$3'" >&2; exit 1 ;; esac
    fi
    to="$(map "$3")"
    mkdir -p "$(dirname "$to")"
    cp "$2" "$to" || exit 1
    if [ -n "$FAKE_CORRUPT" ] && [ "$3" = "$FAKE_CORRUPT" ]; then printf x >> "$to"; fi
    echo "$2: 1 file pushed." ;;
  shell)
    shift; cmd="$*"
    [ "$FAKE_STATE" = device ] || { echo "adb: no devices/emulators found" >&2; exit 1; }
    if [ -n "$FAKE_INTERRUPT" ] && [ ! -e "$W/run/interrupted" ]; then
      case "$cmd" in *"$FAKE_INTERRUPT"*) : > "$W/run/interrupted"; interrupt ;; esac
    fi
    if [ -n "$FAKE_SHELL_FAIL" ]; then
      case "$cmd" in *"$FAKE_SHELL_FAIL"*) echo "adb: error: device offline" >&2; exit 1 ;; esac
    fi
    # The swap cut off after it removed the old binary, half-way through
    # copying the new one: then Ctrl-C.
    if [ "$FAKE_CUT_SWAP" = 1 ]; then
      case "$cmd" in *"if cp /tmp/waved.new waved"*)
        m="$(map "$cmd")"
        PATH="$FAKE_WAVE_BIN:$PATH" sh -c "${m%%if cp *}printf 'half a binary' > waved" > /dev/null 2>&1
        interrupt ;;
      esac
    fi
    if [ "$FAKE_STUB_WRITE_FAILS" = 1 ]; then
      cmd="${cmd//> \/etc\/init.d\/S60pspapp_daemon/> /tmp/no-such-dir/S60pspapp_daemon}"
    fi
    if [ "$FAKE_PATCH" = noop ]; then
      case "$cmd" in *"sh /tmp/patch-usbgadget.sh"*)
        printf '#!/bin/sh\necho "already patched (p_volume + c_volume + hid.0 + adb at boot; no USB storage at boot)"\n' \
          > "$W/tmp/patch-usbgadget.sh" ;;
      esac
    fi
    mapped="$(map "$cmd")"
    # Never run a script that still names a path outside $W.
    if printf '%s' "${mapped//"$W"/WAVE}" | grep -Eq "(^|[^[:alnum:]_.-])/(data|tmp|etc|usr|mnt|proc)([^[:alnum:]_.-]|\$)"; then
      echo "UNMAPPED: $cmd" >> "$FAKE_ADB_LOG"; echo "fake adb: unmapped path in: $cmd" >&2; exit 97
    fi
    keep=no
    if [ "$FAKE_STUBS_STICK" = 0 ]; then
      case "$cmd" in *"> /etc/init.d/S60pspapp_daemon"*)
        keep=yes; mkdir -p "$W/run/keep"
        cp -p "$W/etc/init.d/S60pspapp_daemon" "$W/usr/lib/libapplogrs.so" "$W/run/keep/" ;;
      esac
    fi
    upper="$W/data/overlay/upper"
    upper_before="$(upper_state)"
    merged_before="$(merged_state)"
    # What it prints names the Wave's paths, as on the Wave.
    INIT="$W/etc/init.d/S89usbgadget" BACKUP_DIR="$W/data/waved" PATH="$FAKE_WAVE_BIN:$PATH" \
      sh -c 'kill() { [ "$1" != -0 ] || return 1; for p in "$@"; do echo "kill $p" >> "$FAKE_ADB_LOG"; [ "$p" != 4242 ] || rm -f "$FAKE_WAVE/run/watchdog"; done; }
'"$mapped" | sed "s#$W##g"
    rc=${PIPESTATUS[0]}
    if [ "$keep" = yes ]; then
      cp -p "$W/run/keep/S60pspapp_daemon" "$W/etc/init.d/S60pspapp_daemon"
      cp -p "$W/run/keep/libapplogrs.so" "$W/usr/lib/libapplogrs.so"
    fi
    [ "$(upper_state)" = "$upper_before" ] || echo "DIRECT-UPPER-WRITE: $cmd" >> "$FAKE_ADB_LOG"
    # The overlay's copy-up: what changed under /etc and /usr is in the upper dir.
    if [ "$FAKE_NO_COPY_UP" != 1 ]; then
      comm -13 <(printf '%s\n' "$merged_before") <(merged_state) | while read -r _ _ path; do
        [ -n "$path" ] || continue
        mkdir -p "$upper/$(dirname "$path")"
        cp -p "$W/$path" "$upper/$path"
      done
    fi
    exit $rc ;;
esac
ADB
    # The Wave's busybox commands whose effect matters. The watchdog runs while
    # $W/run/watchdog exists (pid 4242), waved while $W/run/waved does (4243).
    cat > "$T/wave-bin/ps" <<'EOF'
#!/bin/sh
echo "  PID USER       VSZ STAT COMMAND"
[ ! -f "$FAKE_WAVE/run/watchdog" ] || echo " 4242 root      1520 S    /bin/sh /etc/init.d/S60waved start"
[ ! -f "$FAKE_WAVE/run/waved" ] || echo " 4243 root      300m S    /data/waved/waved"
EOF
    # killall waved: waved takes two pidof polls to exit (the lift retracts first).
    cat > "$T/wave-bin/killall" <<'EOF'
#!/bin/sh
echo "killall $*" >> "$FAKE_ADB_LOG"
for name in "$@"; do
    if [ "$name" = waved ] && [ -f "$FAKE_WAVE/run/waved" ] && [ "$FAKE_WAVED_STUCK" != 1 ]; then
        echo 2 > "$FAKE_WAVE/run/waved.exiting"
    fi
done
EOF
    cat > "$T/wave-bin/pidof" <<'EOF'
#!/bin/sh
run="$FAKE_WAVE/run"
[ -f "$run/$1" ] || exit 1
if [ -f "$run/$1.exiting" ]; then
    left=$(cat "$run/$1.exiting")
    if [ "$left" -le 0 ]; then
        rm -f "$run/$1" "$run/$1.exiting"
        echo "pidof $1: exited" >> "$FAKE_ADB_LOG"
        exit 1
    fi
    echo $((left - 1)) > "$run/$1.exiting"
fi
echo 4243
EOF
    cat > "$T/wave-bin/start-stop-daemon" <<'EOF'
#!/bin/sh
printf 'start-stop-daemon %s\n' "$*" | sed "s#$FAKE_WAVE##g" >> "$FAKE_ADB_LOG"
: > "$FAKE_WAVE/run/watchdog"
EOF
    cat > "$T/wave-bin/df" <<'EOF'
#!/bin/sh
echo "Filesystem           1K-blocks      Used Available Use% Mounted on"
echo "/dev/data               450360    237580 $FAKE_FREE_KB  57% /data"
EOF
    # cp runs out of space half-way through copying waved.new over waved.
    cat > "$T/wave-bin/cp" <<'EOF'
#!/bin/sh
for last; do :; done
if [ "$FAKE_DATA_FULL" = 1 ] && [ "${1##*/}" = waved.new ] && [ "${last##*/}" = waved ]; then
    printf 'half a binary' > "$last"
    echo "cp: write error: No space left on device" >&2
    exit 1
fi
exec /bin/cp "$@"
EOF
    cat > "$T/wave-bin/md5sum" <<'EOF'
#!/bin/sh
PATH="${PATH#"$FAKE_WAVE_BIN":}"
if command -v md5sum >/dev/null 2>&1; then exec md5sum "$@"; fi
exec md5 -r "$@"
EOF
    printf '#!/bin/sh\nexit 0\n' > "$T/wave-bin/sleep"
    printf '#!/bin/sh\nexit 0\n' > "$T/wave-bin/sync"
    chmod +x "$T/bin/adb" "$T"/wave-bin/*
    printf '#!/bin/sh\ncp "$2" "$3"\n' > "$T/bin/objcopy"; chmod +x "$T/bin/objcopy"
    head -c 4096 /dev/urandom > "$T/waved"
    export PATH="$T/bin:$PATH" OBJCOPY="$T/bin/objcopy" WAVED_BINARY="$T/waved"
    # Never let a script under test reach a real Wave.
    if [ "$(command -v adb)" != "$T/bin/adb" ]; then
        echo "ABORT: adb does not resolve to the fake ($(command -v adb))" >&2
        exit 2
    fi
}

# A Wave running waved under its watchdog, with its own settings, set up by
# an earlier install: its stubs are in the overlay's upper directory (and so
# in /etc and /usr). $1 is its libapplogrs.so: by default the compiled no-op
# library scripts/libapplogrs.so (8352 bytes), as on the owner's Wave.
wave_running_waved() {
    local lib="${1:-$STUB_LIB}"
    export FAKE_GADGET=patched
    mkdir -p "$W/data/waved" "$W/data/overlay/upper/etc/init.d" "$W/data/overlay/upper/usr/lib"
    printf 'old waved' > "$W/data/waved/waved"; chmod 755 "$W/data/waved/waved"
    printf '%s\n' "$OLD_CONFIG" > "$W/data/waved/config.toml"; chmod 600 "$W/data/waved/config.toml"
    printf '#!/bin/sh\necho old init\n' > "$W/etc/init.d/S60waved"; chmod 755 "$W/etc/init.d/S60waved"
    for d in "$W" "$W/data/overlay/upper"; do
        printf '#!/bin/sh\n' > "$d/etc/init.d/S60pspapp_daemon"; chmod 755 "$d/etc/init.d/S60pspapp_daemon"
        cp "$lib" "$d/usr/lib/libapplogrs.so"
    done
    : > "$W/run/watchdog"
    : > "$W/run/waved"
}

# Both stubs are in the upper dir and show through the mounts as they are there.
stubs_in_upper() {
    local f
    for f in etc/init.d/S60pspapp_daemon usr/lib/libapplogrs.so; do
        [ -f "$W/data/overlay/upper/$f" ] && same "$W/data/overlay/upper/$f" "$W/$f" || return 1
    done
}

# The Wave's /proc/mounts as S02overlayfs leaves it (mount options as its
# fopivot_release / fopivot write them), in the fake's paths.
write_mounts() {
    {
        echo "/dev/data $W/data ext4 rw,noatime 0 0"
        case "$FAKE_OVERLAY" in
            release)
                for d in etc usr; do
                    echo "overlayfs:$W/data/overlay/upper/$d $W/$d overlay rw,noatime,lowerdir=$W/$d,upperdir=$W/data/overlay/upper/$d,workdir=$W/data/overlay/work/$d 0 0"
                done ;;
            root)
                echo "overlayfs:$W/data/overlay/upper / overlay rw,noatime,lowerdir=/,upperdir=$W/data/overlay/upper,workdir=$W/data/overlay/work 0 0" ;;
        esac
    } > "$W/proc/mounts"
}

# Copies what install.sh prints to $T/out until the heading of the install
# step, then stops reading: the pipe of `install.sh 2>&1 | tee log` once tee
# is gone.
reader() {
    local l
    while IFS= read -r l; do
        printf '%s\n' "$l" >> "$T/out"
        case "$l" in "=== Installing S60waved"*) break ;; esac
    done
    : > "$T/reader-gone"
}

# run_install [stderr_closed | pipe_closes]: install.sh against the Wave in $W,
# its S89usbgadget as $FAKE_GADGET says. Output in $T/out, exit status in $rc.
run_install() {
    if [ "$FAKE_GADGET" != missing ]; then
        cp "$T/gadget-$FAKE_GADGET" "$W/etc/init.d/S89usbgadget"
        chmod 755 "$W/etc/init.d/S89usbgadget"
    fi
    write_mounts
    : > "$T/out"
    case "${1:-}" in
        stderr_closed) "$DEPLOY/install.sh" > "$T/out" 2>&- < /dev/null; rc=$? ;;
        pipe_closes) "$DEPLOY/install.sh" 2>&1 < /dev/null | reader; rc=${PIPESTATUS[0]} ;;
        *) "$DEPLOY/install.sh" > "$T/out" 2>&1 < /dev/null; rc=$? ;;
    esac
}
# A Wave whose boot-time files are this checkout's already: an update that
# changes nothing the next boot would pick up.
wave_current() {
    cp "$DEPLOY/S03splash" "$W/etc/init.d/S03splash"
    cp "$DEPLOY/rsyslog.conf" "$W/etc/rsyslog.conf"
    cp "$DEPLOY/splash.fb" "$W/data/waved/splash.fb"
}

# VERBOSE=1 shows the installer's output and the adb log of a failed check.
check() {
    CHECKS=$((CHECKS + 1))
    if eval "$2"; then echo "ok   $1"; return; fi
    echo "FAIL $1"
    FAILS=$((FAILS + 1))
    if [ -n "${VERBOSE:-}" ]; then
        sed 's/^/    out | /' "$T/out"
        sed 's/^/    adb | /' "$FAKE_ADB_LOG"
    fi
}
line_of() { grep -n -F -- "$1" "$FAKE_ADB_LOG" | head -1 | cut -d: -f1; }
# Whether the first log line with $1 comes before the first with $2.
before() {
    local a b
    a="$(line_of "$1")"; b="$(line_of "$2")"
    [ -n "$a" ] && [ -n "$b" ] && [ "$a" -lt "$b" ]
}
same() { cmp -s "$1" "$2"; }
md5sum_of() {
    local f
    f="$(mktemp)"; cat > "$f"
    { md5sum "$f" 2>/dev/null || md5 -r "$f"; } | cut -d' ' -f1
    rm -f "$f"
}
mode() { stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1"; }
logged() { grep -q -F -- "$1" "$FAKE_ADB_LOG"; }
said() { grep -q -F -- "$1" "$T/out"; }
watchdog_started_last() { [ "$(tail -n 1 "$FAKE_ADB_LOG")" = "start-stop-daemon $START" ]; }
# Every file went to /tmp with a push and was checked there by md5.
all_pushed_and_checked() {
    local f
    for f in waved.new S60waved.new S03splash.new rsyslog.conf.new splash.fb.new \
        iptables.rules.new patch-usbgadget.sh "$@"; do
        grep -q "^push .* /tmp/$f\$" "$FAKE_ADB_LOG" && logged "md5sum '/tmp/$f'" || return 1
    done
    ! grep "^push " "$FAKE_ADB_LOG" | grep -v -q " /tmp/[^ ]*\$"
}
# installed <deploy file> <path on the Wave> <dot-named copy>: there, through
# the dot-named copy and a rename.
installed() {
    same "$DEPLOY/$1" "$W$2" && logged "mv -f $3 $2" && [ ! -e "$W$3" ]
}
one_config_backup() {
    local b
    set -- "$W"/data/waved/config.toml.before-*
    [ $# -eq 1 ] && [ -f "$1" ] && [ "$(cat "$1")" = "$OLD_CONFIG" ] && [ "$(mode "$1")" = 600 ]
}
# Nothing of the stock Wave was stopped, swapped, installed or stubbed.
stock_untouched() {
    ! logged killall && ! logged "> /etc/init.d/S60pspapp_daemon" &&
        [ "$(cat "$W/etc/init.d/S60pspapp_daemon")" = "$STOCK_DAEMON" ] &&
        [ "$(cat "$W/usr/lib/libapplogrs.so")" = ELF ] &&
        [ ! -e "$W/data/waved/waved" ] && [ ! -e "$W/etc/init.d/S60waved" ] &&
        [ ! -e "$W/etc/init.d/S03splash" ]
}
# The running waved and its watchdog were left alone.
waved_untouched() {
    ! logged killall && ! logged "kill 4242" && ! logged start-stop-daemon &&
        [ -f "$W/run/watchdog" ] && [ -f "$W/run/waved" ] &&
        [ "$(cat "$W/data/waved/waved")" = "old waved" ]
}

# --- install: a first install on a stock Wave -------------------------------
setup
run_install
check first_install_succeeds '[ $rc -eq 0 ]'
check pushes_every_file_to_tmp_and_checks_it 'all_pushed_and_checked config.toml.new'
check patches_the_gadget_before_stopping_anything 'before "shell sh /tmp/patch-usbgadget.sh" "killall pspapp"'
check checks_adb_at_boot_after_the_patch_and_before_stopping_anything 'before "shell sh /tmp/patch-usbgadget.sh" "commented++" && before "commented++" "killall pspapp"'
check the_gadget_brings_adb_up_at_boot 'grep -q "^[[:space:]]*adbd_enable[[:space:]]*$" "$W/etc/init.d/S89usbgadget" && ! grep -q "#[[:space:]]*adbd_enable" "$W/etc/init.d/S89usbgadget" && [ -x "$W/etc/init.d/S89usbgadget" ]'
check keeps_a_copy_of_the_stock_gadget_in_data_waved 'same "$W"/data/waved/S89usbgadget.bak.* "$T/gadget-stock"'
check shows_what_the_patch_did 'grep -q "^    patched adbd at boot" "$T/out" && said "commented=0 live=2"'
# The patch's own "reboot the device ... adb reboot" is not shown: the only
# reboot hint is the last line, once everything is in place.
check says_adb_reboot_only_at_the_end '[ "$(grep -c "adb reboot" "$T/out")" -eq 1 ] && [ "$(tail -n 1 "$T/out")" = "Run: adb reboot" ]'
check stops_the_stock_app 'grep -q "^killall pspapp pspapp_daemon" "$FAKE_ADB_LOG"'
check swaps_in_the_new_binary 'same "$W/data/waved/waved" "$T/waved" && [ "$(mode "$W/data/waved/waved")" = 755 ]'
check installs_S60waved_through_a_dot_name 'installed S60waved /etc/init.d/S60waved /etc/init.d/.S60waved.new && [ "$(mode "$W/etc/init.d/S60waved")" = 755 ]'
check installs_S03splash_through_a_dot_name 'installed S03splash /etc/init.d/S03splash /etc/init.d/.S03splash.new && [ "$(mode "$W/etc/init.d/S03splash")" = 755 ]'
check installs_rsyslog_conf_through_etc 'installed rsyslog.conf /etc/rsyslog.conf /etc/.rsyslog.conf.new'
check installs_splash_fb 'installed splash.fb /data/waved/splash.fb /data/waved/.splash.fb.new'
check installs_iptables_rules 'installed iptables.rules /data/waved/iptables.rules /data/waved/.iptables.rules.new'
check writes_the_default_config 'same "$W/data/waved/config.toml" "$DEPLOY/../config/default.toml" && [ "$(mode "$W/data/waved/config.toml")" = 600 ] && ! ls "$W"/data/waved/config.toml.before-* > /dev/null 2>&1'
check leaves_nothing_in_tmp '[ -z "$(ls -A "$W/tmp")" ]'
check writes_both_stubs_through_the_mounts 'said "    wrote /etc/init.d/S60pspapp_daemon" && said "    wrote /usr/lib/libapplogrs.so" && [ "$(cat "$W/etc/init.d/S60pspapp_daemon")" = "#!/bin/sh" ] && [ "$(wc -c < "$W/etc/init.d/S60pspapp_daemon" | tr -d " ")" = 10 ] && [ "$(mode "$W/etc/init.d/S60pspapp_daemon")" = 755 ] && [ -f "$W/usr/lib/libapplogrs.so" ] && [ ! -s "$W/usr/lib/libapplogrs.so" ]'
check reads_both_written_stubs_back '[ "$(grep -c -F "\$f(content)" "$FAKE_ADB_LOG")" -eq 2 ]'
check the_stubs_reach_the_upper_dir_by_copy_up 'stubs_in_upper'
check never_writes_the_upper_dir '! logged DIRECT-UPPER-WRITE'
# The stubs are the last change, read back before the recordings folder.
check stubs_come_last 'before "cp /tmp/config.toml.new" "> /etc/init.d/S60pspapp_daemon" && before "> /etc/init.d/S60pspapp_daemon" "for f in /etc/init.d/S60pspapp_daemon /usr/lib/libapplogrs.so" && before "for f in /etc/init.d/S60pspapp_daemon /usr/lib/libapplogrs.so" "mkdir -p /mnt/udisk/recordings"'
check makes_the_recordings_folder '[ -d "$W/mnt/udisk/recordings" ]'
check first_install_leaves_the_watchdog_to_the_reboot '! logged start-stop-daemon'
check firewall_lets_the_wifi_setup_network_in 'grep -q -e "-A INPUT -i wlan0 -p udp --dport 67 -j ACCEPT" "$DEPLOY/iptables.rules" && grep -q -e "-A INPUT -i wlan0 -p udp --dport 53 -j ACCEPT" "$DEPLOY/iptables.rules" && grep -q -e "-A INPUT -i wlan0 -p tcp --dport 80 -j ACCEPT" "$DEPLOY/iptables.rules"'

# --- install: an update of a running waved ----------------------------------
setup; wave_running_waved
run_install
check update_succeeds '[ $rc -eq 0 ]'
check update_pushes_no_default_config 'all_pushed_and_checked && ! logged config.toml.new'
check update_keeps_the_settings '[ "$(cat "$W/data/waved/config.toml")" = "$OLD_CONFIG" ] && [ "$(mode "$W/data/waved/config.toml")" = 600 ]'
check update_backs_up_the_settings_first 'one_config_backup && before "config.toml.before-" "shell sh /tmp/patch-usbgadget.sh"'
check update_patches_before_stopping_waved 'before "commented++" "killall pspapp" && said "    already patched"'
check update_stops_the_watchdog_and_waits_for_waved 'logged "kill 4242" && before "pidof waved: exited" "cp /tmp/waved.new waved"'
check update_keeps_the_old_binary_in_tmp '[ "$(cat "$W/tmp/waved.old")" = "old waved" ] && same "$W/data/waved/waved" "$T/waved" && [ "$(cat "$W/tmp/waved.old.md5")" = "$(printf "old waved" | md5sum_of)" ] && [ ! -e "$W/tmp/waved.swap.pid" ]'
# The swap finishes or rolls back on its own when the adb session goes.
check the_swap_ignores_a_closed_session 'logged "trap \"\" HUP INT TERM PIPE; trap \"rm -f /tmp/waved.swap.pid\" EXIT"'
check update_installs_the_start_up_scripts 'installed S60waved /etc/init.d/S60waved /etc/init.d/.S60waved.new && installed S03splash /etc/init.d/S03splash /etc/init.d/.S03splash.new && installed rsyslog.conf /etc/rsyslog.conf /etc/.rsyslog.conf.new'
# Stubs already in the upper dir stay as they are, whatever they hold: the
# 8352-byte compiled libapplogrs.so is neither rewritten nor size-checked.
check update_keeps_the_stubs_in_the_upper_dir 'said "    kept /etc/init.d/S60pspapp_daemon" && said "    kept /usr/lib/libapplogrs.so" && same "$W/data/overlay/upper/usr/lib/libapplogrs.so" "$STUB_LIB" && same "$W/usr/lib/libapplogrs.so" "$STUB_LIB" && [ "$(cat "$W/data/overlay/upper/etc/init.d/S60pspapp_daemon")" = "#!/bin/sh" ] && stubs_in_upper'
check update_checks_only_that_kept_stubs_are_there '! grep -q -F "\$f(content)" "$FAKE_ADB_LOG" && logged "for f in /etc/init.d/S60pspapp_daemon /usr/lib/libapplogrs.so"'
check update_never_writes_the_upper_dir '! logged DIRECT-UPPER-WRITE'
check update_starts_the_watchdog_last 'watchdog_started_last && before "cp /tmp/waved.new waved" "start-stop-daemon"'
# This Wave had no S03splash and the stock rsyslog.conf: both take effect at the
# next boot, so the update ends with a reboot hint (the gadget was patched).
check update_says_what_needs_a_reboot '[ "$(tail -n 2 "$T/out" | head -n 1)" = "=== Done. The previous binary is in /tmp/waved.old until the Wave restarts. ===" ] && [ "$(tail -n 1 "$T/out")" = "Reboot to apply the new boot splash and logging settings: adb reboot" ] && said "    new /etc/init.d/S03splash" && said "    new /etc/rsyslog.conf"'

# An update that changes nothing the next boot would pick up: no reboot hint.
setup; wave_running_waved; wave_current
run_install
check a_current_update_does_not_ask_for_a_reboot '[ $rc -eq 0 ] && ! said "adb reboot" && ! said "    new /etc/" && [ "$(tail -n 1 "$T/out")" = "=== Done. The previous binary is in /tmp/waved.old until the Wave restarts. ===" ]'
# An update that patches the gadget (a Wave an older install.sh set up).
setup; wave_running_waved; wave_current
FAKE_GADGET=stock run_install
check an_update_that_patches_the_gadget_asks_for_a_reboot '[ $rc -eq 0 ] && said "    patched adbd at boot" && [ "$(tail -n 1 "$T/out")" = "Reboot to apply the new USB gadget settings: adb reboot" ]'

# An older install.sh's one-newline libapplogrs.so is kept too.
setup; printf '\n' > "$T/one-byte.so"; wave_running_waved "$T/one-byte.so"
run_install
check update_keeps_a_one_byte_telemetry_stub '[ $rc -eq 0 ] && said "    kept /usr/lib/libapplogrs.so" && same "$W/data/overlay/upper/usr/lib/libapplogrs.so" "$T/one-byte.so"'

# The installed binary's space comes back at the swap, so it counts as free.
setup; wave_running_waved; head -c 2097152 /dev/zero > "$W/data/waved/waved"
FAKE_FREE_KB=19000 run_install
check counts_the_old_binary_as_free_space '[ $rc -eq 0 ]'

# Only the splash image differs: that takes effect at the next boot too.
setup; wave_running_waved; wave_current; printf 'an older splash' > "$W/data/waved/splash.fb"
run_install
check a_new_splash_image_asks_for_a_reboot '[ $rc -eq 0 ] && said "    new /data/waved/splash.fb" && [ "$(tail -n 1 "$T/out")" = "Reboot to apply the new boot splash: adb reboot" ]'

# The old binary's md5 cannot be written: the swap stops before it removes
# anything, and die() finds nothing to restore.
setup; wave_running_waved; mkdir -p "$W/tmp/waved.old.md5"
run_install
check an_unwritable_old_md5_stops_the_swap_first '[ $rc -eq 1 ] && said "ERROR: the binary was not replaced" && [ "$(cat "$W/data/waved/waved")" = "old waved" ] && ! said "previous binary" && said "The watchdog runs again."'

# Two updates in one boot: A -> B, then B -> C cut before the swap script
# starts (adb fails, or Ctrl-C while it connects). /tmp/waved.old (A) and its
# md5 from the first are still there; the binary B must stay, not go back to A.
for cut in fail interrupt; do
    setup; wave_running_waved
    run_install
    first_rc=$rc
    head -c 4096 /dev/urandom > "$T/waved2"
    : > "$FAKE_ADB_LOG"
    if [ "$cut" = fail ]; then
        WAVED_BINARY="$T/waved2" FAKE_SHELL_FAIL="if cp /tmp/waved.new waved" run_install
    else
        WAVED_BINARY="$T/waved2" FAKE_INTERRUPT="if cp /tmp/waved.new waved" run_install
    fi
    check "second_update_cut_before_the_swap_keeps_the_binary_($cut)" '[ $first_rc -eq 0 ] && [ $rc -eq 1 ] && same "$W/data/waved/waved" "$T/waved" && ! said "previous binary" && [ ! -e "$W/tmp/waved.old.md5" ]'
    check "second_update_cut_before_the_swap_gets_the_watchdog_back_($cut)" 'said "The watchdog runs again." && watchdog_started_last'
done

# --- install: refusals before anything changes ------------------------------
setup; FAKE_FREE_KB=1000 run_install
check low_space_refuses_before_any_push '[ $rc -ne 0 ] && grep -q "^ERROR: /data has 0 MB available and waved needs 20 MB. Nothing was changed. See Troubleshooting: \"/data is full\"." "$T/out" && ! grep -q "^push" "$FAKE_ADB_LOG" && stock_untouched'
setup; FAKE_SHELL_FAIL="df -Pk /data" run_install
check unreadable_free_space_refuses '[ $rc -ne 0 ] && said "ERROR: could not read the free space on /data (no answer)" && ! grep -q "^push" "$FAKE_ADB_LOG"'

# /etc and /usr must be the overlays the stubs and revert.sh assume; checked
# before anything changes. Debug mode (one overlay on /) has the same upper dir.
setup; FAKE_OVERLAY=none run_install
check refuses_without_the_expected_overlays '[ $rc -ne 0 ] && said "ERROR: /etc and /usr are not the overlays install.sh expects" && said "etc=0 usr=0 root=0" && ! grep -q "^push" "$FAKE_ADB_LOG" && stock_untouched'
setup; FAKE_OVERLAY=root run_install
check accepts_the_debug_mode_overlay '[ $rc -eq 0 ]'
setup; run_install
check checks_the_overlay_before_any_push 'before "/proc/mounts" "push " && logged "df -Pk /data"'

setup; wave_running_waved; FAKE_CORRUPT=/tmp/waved.new run_install
check md5_mismatch_refuses '[ $rc -ne 0 ] && said "ERROR: /tmp/waved.new on the Wave is not" && said "nothing was changed"'
check md5_mismatch_changes_nothing 'waved_untouched && ! logged "shell sh /tmp/patch-usbgadget.sh" && ! ls "$W"/data/waved/config.toml.before-* > /dev/null 2>&1 && same "$W/etc/init.d/S89usbgadget" "$T/gadget-patched"'
setup; FAKE_CORRUPT=/tmp/patch-usbgadget.sh run_install
check a_damaged_patch_is_never_run '[ $rc -ne 0 ] && ! logged "shell sh /tmp/patch-usbgadget.sh" && stock_untouched'
setup; FAKE_PUSH_FAIL=/tmp/S03splash.new run_install
check failed_push_refuses '[ $rc -ne 0 ] && said "ERROR: could not copy $DEPLOY/S03splash to /tmp/S03splash.new on the Wave (adb push); nothing was changed" && stock_untouched'

setup; wave_running_waved; mkdir -p "$W/data/waved/update"; printf 'version 0.10.0\n' > "$W/data/waved/update/trial"
run_install
check ota_trial_refuses '[ $rc -ne 0 ] && said "ERROR: an over-the-air update is on trial" && ! grep -q "^push" "$FAKE_ADB_LOG" && waved_untouched'
setup; wave_running_waved; FAKE_SHELL_FAIL="update/trial" run_install
check no_answer_about_a_trial_refuses '[ $rc -ne 0 ] && said "ERROR: no answer from the Wave about /data/waved/update/trial" && ! grep -q "^push" "$FAKE_ADB_LOG" && waved_untouched'

setup; FAKE_STATE=unknown run_install
check no_device_refuses '[ $rc -ne 0 ] && said "ERROR: no Wave on adb" && ! grep -q "^push" "$FAKE_ADB_LOG"'
setup; WAVED_BINARY="$T/missing" run_install
check missing_binary_explains_build '[ $rc -ne 0 ] && said "ERROR: no waved binary at $T/missing" && said "cross build" && [ ! -s "$FAKE_ADB_LOG" ]'

# --- install: no ADB at boot, no further ------------------------------------
# A failed patch, a patch without an answer, a patch that leaves adbd
# commented out, a boot function with no adbd_enable, a check without an
# answer: each stops before the stock app or waved is touched.
setup; FAKE_GADGET=missing run_install
check refuses_when_the_patch_fails '[ $rc -ne 0 ] && said "ERROR: the USB gadget patch did not finish. Nothing was stopped" && said "FATAL:" && ! said "adb reboot"'
check patch_failure_leaves_the_stock_app_alone 'stock_untouched'
setup; FAKE_SHELL_FAIL="sh /tmp/patch-usbgadget.sh" run_install
check refuses_when_the_patch_gives_no_answer '[ $rc -ne 0 ] && said "ERROR: the USB gadget patch did not finish" && stock_untouched'
setup; FAKE_PATCH=noop run_install
check refuses_when_adbd_is_still_commented_out '[ $rc -ne 0 ] && said "ERROR: ADB is not enabled at boot (/etc/init.d/S89usbgadget: commented=2 live=0)" && ! said "adb reboot"'
check commented_adbd_leaves_the_stock_app_alone 'stock_untouched'
setup; FAKE_GADGET=no_boot_adbd run_install
check refuses_when_the_boot_function_has_no_adbd_enable '[ $rc -ne 0 ] && said "ERROR: ADB is not enabled at boot (/etc/init.d/S89usbgadget: commented=0 live=0)" && ! said "adb reboot"'
check missing_adbd_leaves_the_stock_app_alone 'stock_untouched'
setup; FAKE_SHELL_FAIL="commented++" run_install
check refuses_when_the_adb_check_gives_no_answer '[ $rc -ne 0 ] && said "ERROR: ADB is not enabled at boot (/etc/init.d/S89usbgadget: no answer)" && stock_untouched'
setup; wave_running_waved; FAKE_GADGET=missing run_install
check patch_failure_leaves_a_running_waved_alone '[ $rc -ne 0 ] && said "ERROR: the USB gadget patch did not finish" && waved_untouched && ! said "The watchdog runs again."'

# --- install: failures once waved is stopped --------------------------------
setup; wave_running_waved; FAKE_WAVED_STUCK=1 run_install
check refuses_the_swap_while_waved_runs '[ $rc -ne 0 ] && said "waved does not exit" && said "ERROR: could not stop waved" && grep -q "^killall -9 waved" "$FAKE_ADB_LOG" && ! logged "cp /tmp/waved.new waved" && [ "$(cat "$W/data/waved/waved")" = "old waved" ]'
check stuck_waved_gets_its_watchdog_back 'said "The watchdog runs again." && watchdog_started_last'
check stuck_waved_means_nothing_else_is_installed '[ ! -e "$W/etc/init.d/S03splash" ] && ! logged ".S60waved.new" && ! logged "> /etc/init.d/S60pspapp_daemon"'
check no_binary_check_before_the_swap '! logged "kill -0 \"\$p\"" && ! said "previous binary"'

setup; wave_running_waved; FAKE_DATA_FULL=1 run_install
check a_binary_that_does_not_fit_puts_the_old_one_back '[ $rc -ne 0 ] && said "the new binary did not fit on " && said "; the old one is back" && said "ERROR: the binary was not replaced" && [ "$(cat "$W/data/waved/waved")" = "old waved" ] && [ "$(mode "$W/data/waved/waved")" = 755 ]'
check a_binary_that_does_not_fit_gets_the_watchdog_back 'said "The watchdog runs again." && watchdog_started_last && [ "$(cat "$W/data/waved/config.toml")" = "$OLD_CONFIG" ] && logged "kill -0 \"\$p\"" && ! said "previous binary"'
setup; FAKE_DATA_FULL=1 run_install
check first_install_that_does_not_fit_says_reboot '[ $rc -ne 0 ] && said "Stock firmware was stopped: adb reboot, then run this again." && [ ! -e "$W/etc/init.d/S60waved" ] && ! logged "> /etc/init.d/S60pspapp_daemon"'

setup; wave_running_waved; FAKE_SHELL_FAIL=".S03splash.new" run_install
check failed_install_step_stops_before_the_stubs '[ $rc -ne 0 ] && said "ERROR: S60waved, S03splash, rsyslog.conf, splash.fb or iptables.rules not installed (the new binary is)" && ! logged "> /etc/init.d/S60pspapp_daemon" && said "The watchdog runs again." && watchdog_started_last'
check failed_install_step_keeps_the_new_binary 'same "$W/data/waved/waved" "$T/waved" && logged "kill -0 \"\$p\"" && ! said "previous binary"'
setup; FAKE_SHELL_FAIL="> /etc/init.d/S60pspapp_daemon" run_install
check failed_stub_write_refuses '[ $rc -ne 0 ] && said "ERROR: could not stub the stock services" && said "Stock firmware was stopped: adb reboot, then run this again." && ! said "=== Done"'
setup; FAKE_STUB_WRITE_FAILS=1 run_install
check a_stub_that_could_not_be_written_refuses '[ $rc -ne 0 ] && said "ERROR: could not stub the stock services" && ! said "wrote /etc/init.d/S60pspapp_daemon" && ! said "=== Done"'
setup; FAKE_SHELL_FAIL="mkdir -p /mnt/udisk/recordings" run_install
check a_recordings_folder_that_fails_does_not_fail_the_install '[ $rc -eq 0 ] && [ "$(tail -n 1 "$T/out")" = "Run: adb reboot" ]'

# Stubs written now that read back as something else (a write that did not
# take), or that never reach the upper dir (no overlay there): no "Done".
setup; FAKE_STUBS_STICK=0 run_install
check refuses_when_the_stubs_did_not_take '[ $rc -ne 0 ] && said "ERROR: the stock app and telemetry stubs did not take" && said "S60pspapp_daemon(content)" && said "libapplogrs.so(content)" && ! said "=== Done"'
setup; FAKE_NO_COPY_UP=1 run_install
check refuses_when_the_stubs_are_not_in_the_upper_dir '[ $rc -ne 0 ] && said "ERROR: the stock app and telemetry stubs did not take" && said "/data/overlay/upper/etc/init.d/S60pspapp_daemon" && said "/data/overlay/upper/usr/lib/libapplogrs.so" && ! said "(content)" && ! said "=== Done"'
setup; wave_running_waved
rm "$W/data/overlay/upper/etc/init.d/S60pspapp_daemon" "$W/data/overlay/upper/usr/lib/libapplogrs.so"
printf '%s\n' "$STOCK_DAEMON" > "$W/etc/init.d/S60pspapp_daemon"; printf 'ELF' > "$W/usr/lib/libapplogrs.so"
FAKE_STUBS_STICK=0 run_install
check stubs_that_did_not_take_on_an_update_restart_the_watchdog '[ $rc -ne 0 ] && said "ERROR: the stock app and telemetry stubs did not take" && said "The watchdog runs again." && watchdog_started_last'

# --- install: Ctrl-C ----------------------------------------------------------
setup; wave_running_waved; FAKE_INTERRUPT="killall pspapp" run_install
check interrupt_names_the_step '[ $rc -ne 0 ] && said "ERROR: install.sh interrupted during: Stopping stock firmware, the watchdog and waved"'
# Cut before the stop script ran: the watchdog rcS started still runs, so no
# second one is started next to it.
check interrupted_update_starts_no_second_watchdog 'said "the watchdog runs already" && said "The watchdog runs again." && ! grep -q "^start-stop-daemon" "$FAKE_ADB_LOG" && [ -f "$W/run/watchdog" ] && [ "$(cat "$W/data/waved/waved")" = "old waved" ] && ! logged "cp /tmp/waved.new waved"'

# A closed terminal (HUP), and Ctrl-C with stderr closed or its pipe gone
# (`install.sh 2>&1 | tee log`, tee killed first): die still puts the
# watchdog back after an update, and exits 1.
setup; wave_running_waved; FAKE_INTERRUPT=".S60waved.new" FAKE_SIGNAL=HUP run_install
check a_closed_terminal_gets_the_watchdog_back '[ $rc -eq 1 ] && said "ERROR: install.sh interrupted during: Installing S60waved" && said "The watchdog runs again." && watchdog_started_last'
setup; wave_running_waved; FAKE_INTERRUPT=".S60waved.new" FAKE_SIGNAL=TERM run_install stderr_closed
check closed_stderr_still_gets_the_watchdog_back '[ $rc -eq 1 ] && watchdog_started_last'
setup; wave_running_waved
FAKE_INTERRUPT=".S60waved.new" FAKE_SIGNAL=TERM FAKE_INTERRUPT_WAIT="$T/reader-gone" run_install pipe_closes
check a_broken_output_pipe_still_gets_the_watchdog_back '[ -e "$T/reader-gone" ] && [ $rc -eq 1 ] && watchdog_started_last'

# The swap cut off half-way (old binary removed, half the new one written):
# die puts the old one back from /tmp/waved.old before the watchdog starts.
setup; wave_running_waved; FAKE_CUT_SWAP=1 run_install
check a_cut_swap_gets_the_old_binary_back '[ $rc -eq 1 ] && said "ERROR: install.sh interrupted during: Swapping the binary" && said "    put the previous binary back in /data/waved/waved (from /tmp/waved.old)" && [ "$(cat "$W/data/waved/waved")" = "old waved" ] && [ "$(mode "$W/data/waved/waved")" = 755 ]'
check a_cut_swap_then_gets_the_watchdog_back 'said "The watchdog runs again." && watchdog_started_last && before "kill -0 \"\$p\"" "start-stop-daemon -S"'
setup; FAKE_INTERRUPT="sh /tmp/patch-usbgadget.sh" run_install
check interrupt_before_stopping_leaves_the_stock_app_alone '[ $rc -ne 0 ] && said "ERROR: install.sh interrupted during: Patching the USB gadget" && stock_untouched && ! said "adb reboot"'
setup; FAKE_INTERRUPT="cp /tmp/waved.new waved" run_install
check interrupted_first_install_says_reboot '[ $rc -ne 0 ] && said "ERROR: install.sh interrupted during: Swapping the binary" && said "Stock firmware was stopped: adb reboot, then run this again."'

# --- revert ---------------------------------------------------------------------
# revert: deletes in the overlay's upper dir (deleting through the mount would
# leave whiteouts that hide the stock files). Exact non-dot names, so the
# .prev/.new copies alone cannot satisfy them.
UP=/data/overlay/upper
setup; "$DEPLOY/revert.sh" > "$T/out" 2>&1; rc=$?
check revert_succeeds '[ $rc -eq 0 ]'
# The enable_adb.py hint is an absolute path, right from any directory.
check revert_hint_finds_enable_adb 'p="$(sed -n "s/^.*sudo python3 \"\(.*\)\"\$/\1/p" "$T/out")"; [ "${p#/}" != "$p" ] && [ -f "$p" ] && [ "$(basename "$p")" = enable_adb.py ]'
for f in $UP/etc/init.d/S60waved $UP/etc/init.d/S60pspapp_daemon $UP/etc/init.d/S03splash $UP/etc/init.d/S89usbgadget $UP/etc/rsyslog.conf $UP/usr/lib/libapplogrs.so; do
    check "revert_removes_$f" 'grep -q -F -- "$f " "$FAKE_ADB_LOG"'
done
check revert_removes_the_waved_dir 'grep -q "rm -rf /data/waved" "$FAKE_ADB_LOG"'
check revert_deletes_only_in_the_upper_dir '! grep -q -E "rm [^;]* /(etc|usr)/" "$FAKE_ADB_LOG"'
setup; FAKE_STATE=unknown "$DEPLOY/revert.sh" > /dev/null 2>&1; rc=$?
check revert_refuses_without_device '[ $rc -ne 0 ] && ! grep -q "rm " "$FAKE_ADB_LOG"'

echo
if [ "$FAILS" -eq 0 ]; then echo "all $CHECKS passed"; else echo "$FAILS of $CHECKS failed"; exit 1; fi
