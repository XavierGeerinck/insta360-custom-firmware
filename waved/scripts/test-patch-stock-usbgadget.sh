#!/bin/sh
# Host test for patch-stock-usbgadget.sh (G1, docs/USB_STORAGE.md). Patches a
# copy of an S89usbgadget-shaped script, then runs the patched boot code's
# USB storage step against a fake configfs tree, with a fake usb_flash that
# can refuse to bind while mass_storage.usb0 is linked. Run it under busybox,
# as the Wave runs S89 (the Wave has busybox 1.32):
#
#   docker run --rm -v "$PWD/waved/scripts:/w:ro" busybox:1.36 sh /w/test-patch-stock-usbgadget.sh
#
# STOCK_S89=<path inside the container> also patches that copy (the stock
# rootfs' script, e.g. mount firmwares/fw_v1.2.2.6/extracted/rootfs/etc/init.d).
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
PATCH=$HERE/patch-stock-usbgadget.sh
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
PASSES=0

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

pass() {
    PASSES=$((PASSES + 1))
    echo "ok - $*"
}

# The parts of stock 1.2.2.6's S89usbgadget the patch touches.
fixture() {
    cat > "$1" <<'EOF'
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

function start_usbgadget_ota_mode(){
	usb_dir=/sys/kernel/config/usb_gadget/amlogic
    if [ ! -d ${usb_dir} ]
    then
    {
        sleep 4
		usb_flash
    }
    fi
}

function start_usbgadget(){

    usb_dir=/sys/kernel/config/usb_gadget/amlogic
    if [ ! -d ${usb_dir} ]
    then
    {
        sleep 4
		usb_flash
    }
    fi
}

case "$1" in
  start)
		start_usbgadget &
		;;
  uac_hid)
		usb_flash
		;;
esac

exit $?
EOF
    chmod 755 "$1"
}

# patch_copy <source S89> <name>: patch a copy in its own init.d, check the
# result, patch it again (nothing changes), and leave it in $WORK/<name>.
patch_copy() {
    dir=$WORK/$2
    mkdir -p "$dir/init.d" "$dir/backup"
    cp "$1" "$dir/init.d/S89usbgadget"
    chmod 755 "$dir/init.d/S89usbgadget"
    s89=$dir/init.d/S89usbgadget

    INIT=$s89 BACKUP_DIR=$dir/backup sh "$PATCH" > "$dir/out" 2>&1 \
        || { cat "$dir/out" >&2; fail "$2: the patch failed"; }
    [ -x "$s89" ] || fail "$2: S89 is not executable after the patch"
    sh -n "$s89" || fail "$2: the patched S89 does not parse"
    [ "$(ls -A "$dir/init.d")" = "S89usbgadget" ] \
        || fail "$2: left in init.d: $(ls -A "$dir/init.d" | tr '\n' ' ')"
    [ "$(ls "$dir/backup" | wc -l)" -eq 1 ] || fail "$2: expected one backup"
    [ "$(grep -c '^waved_g1_check_bind()' "$s89")" -eq 1 ] || fail "$2: the bind check is not defined once"
    # In start_usbgadget only: link, bind, check, in that order.
    steps=$(awk '/start_usbgadget[[:space:]]*\(\)/ { on = 1 } on && /^}/ { on = 0 }
        on && /^[[:space:]]*(waved_g1_link_drive|usb_flash|waved_g1_check_bind)[[:space:]]*$/ { gsub(/[[:space:]]/, ""); printf "%s ", $0 }' "$s89")
    [ "$steps" = "waved_g1_link_drive usb_flash waved_g1_check_bind " ] \
        || fail "$2: start_usbgadget runs: $steps"
    [ "$(grep -c 'waved_g1_link_drive$' "$s89")" -eq 1 ] || fail "$2: linked outside start_usbgadget"
    pass "$2: patched, executable, parses, nothing left in init.d"

    before=$(cksum < "$s89")
    INIT=$s89 BACKUP_DIR=$dir/backup sh "$PATCH" > "$dir/out" 2>&1 || fail "$2: the second run failed"
    grep -q "already patched" "$dir/out" || fail "$2: the second run did not say already patched"
    [ "$(cksum < "$s89")" = "$before" ] || fail "$2: the second run changed S89"
    [ "$(ls "$dir/backup" | wc -l)" -eq 1 ] || fail "$2: the second run made a backup"
    pass "$2: a second run changes nothing"
}

fixture "$WORK/S89.fixture"
patch_copy "$WORK/S89.fixture" fixture
if [ -n "${STOCK_S89:-}" ]; then
    patch_copy "$STOCK_S89" stock
fi

# An older G1 patch (no bind check) is refused, untouched.
mkdir -p "$WORK/old/init.d" "$WORK/old/backup"
sed 's|^\(\t*\)usb_flash$|\1# waved G1: USB storage (docs/USB_STORAGE.md).\n&|' "$WORK/S89.fixture" > "$WORK/old/init.d/S89usbgadget"
grep -q "waved G1: USB storage" "$WORK/old/init.d/S89usbgadget" || fail "old fixture"
before=$(cksum < "$WORK/old/init.d/S89usbgadget")
if INIT=$WORK/old/init.d/S89usbgadget BACKUP_DIR=$WORK/old/backup sh "$PATCH" > "$WORK/old/out" 2>&1; then
    fail "an older G1 patch was patched over"
fi
[ "$(cksum < "$WORK/old/init.d/S89usbgadget")" = "$before" ] || fail "the older patch was changed"
pass "an older G1 patch is refused and left alone"

# The patched boot code, against a fake gadget.
sed -n '/^# >>> waved G1/,/^# <<< waved G1/p' "$WORK/fixture/init.d/S89usbgadget" > "$WORK/g1.sh"
[ -s "$WORK/g1.sh" ] || fail "no waved G1 block in the patched S89"

# boot <case> <reject> <drive>: a fresh fake configfs tree, stock's msc_init
# state, then the patched start_usbgadget's three steps. <reject>: "msc"
# refuses the bind while the drive is linked, "all" refuses every bind,
# "none". <drive>: "no" leaves out the drive function.
boot() {
    G=$WORK/$1/gadget
    reject=$2
    rm -rf "$WORK/$1"
    mkdir -p "$G/configs/amlogic.1"
    : > "$G/UDC"
    if [ "$3" != no ]; then
        mkdir -p "$G/functions/mass_storage.usb0/lun.0"
        echo /dev/udisk > "$G/functions/mass_storage.usb0/lun.0/file"
        echo 0 > "$G/functions/mass_storage.usb0/lun.0/removable"
    fi
    (
        WAVED_G1_GADGET=$G
        WAVED_G1_LOG=$WORK/$1/log
        WAVED_G1_CONSOLE=$WORK/$1/console
        . "$WORK/g1.sh"
        usb_flash() {
            if [ "$reject" = all ] \
                || { [ "$reject" = msc ] && [ -L "$G/configs/amlogic.1/mass_storage.usb0" ]; }; then
                : > "$G/UDC"
            else
                echo fdd00000.crgudc2 > "$G/UDC"
            fi
        }
        waved_g1_link_drive
        usb_flash
        waved_g1_check_bind
    )
}

boot binds none yes
G=$WORK/binds/gadget
[ -L "$G/configs/amlogic.1/mass_storage.usb0" ] || fail "binds: the drive is not linked"
[ "$(cat "$G/functions/mass_storage.usb0/lun.0/file")" = "" ] || fail "binds: the drive holds a medium"
[ "$(cat "$G/functions/mass_storage.usb0/lun.0/removable")" = 1 ] || fail "binds: not removable"
[ "$(cat "$G/UDC")" = fdd00000.crgudc2 ] || fail "binds: not bound"
[ ! -e "$WORK/binds/log" ] || fail "binds: logged $(cat "$WORK/binds/log")"
pass "a gadget that binds with the drive keeps it, empty and removable"

boot rejects msc yes
G=$WORK/rejects/gadget
[ "$(cat "$G/UDC")" = fdd00000.crgudc2 ] || fail "rejects: USB is not bound again"
[ ! -e "$G/configs/amlogic.1/mass_storage.usb0" ] || fail "rejects: the drive is still linked"
grep -q "did not bind with mass_storage.usb0" "$WORK/rejects/log" || fail "rejects: the log does not say why"
grep -q "bound without USB storage" "$WORK/rejects/log" || fail "rejects: the log does not say it bound"
grep -q "did not bind with mass_storage.usb0" "$WORK/rejects/console" || fail "rejects: nothing on the console"
pass "a gadget that will not bind with the drive binds again without it, and says why"

boot dead all yes
grep -q "did not bind without mass_storage.usb0 either" "$WORK/dead/log" || fail "dead: the log does not say so"
pass "a bind that fails without the drive too is logged as such"

boot plain none no
G=$WORK/plain/gadget
[ ! -e "$G/configs/amlogic.1/mass_storage.usb0" ] || fail "plain: linked a drive that does not exist"
[ "$(cat "$G/UDC")" = fdd00000.crgudc2 ] || fail "plain: not bound"
pass "no drive function: nothing linked, bound as stock"

echo "all $PASSES passed"
