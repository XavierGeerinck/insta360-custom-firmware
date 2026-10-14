#!/bin/sh
# Host test for patch-stock-usbgadget.sh. Patches a copy of an
# S89usbgadget-shaped script and checks the result: the UAC volume ranges and
# hid.0, nothing of USB storage at boot (G1 switches the gadget at runtime,
# docs/USB_STORAGE.md), and a G1 step left by an earlier version of the patch
# removed. Run it under busybox, as the Wave runs S89 (the Wave has busybox
# 1.32):
#
#   docker run --rm --network none -v "$PWD/waved/scripts:/w:ro" busybox:1.36 sh /w/test-patch-stock-usbgadget.sh
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

adbd_enable()
{
	/usr/bin/adbd &
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
		if [ -e $config_file ]; then
			uac_enable
			#adbd_enable
		else
			uac_enable
			# adbd_enable
		fi
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

# The first G1 patch (b61d7c1, its awk verbatim): the drive linked right
# before start_usbgadget's usb_flash.
old_g1_v1() {
    awk '
        BEGIN { in_start = 0; inserted = 0 }
        /start_usbgadget\(\)[[:space:]]*\{/ { in_start = 1 }
        in_start && !inserted && /^[[:space:]]*usb_flash[[:space:]]*$/ {
            print "\t\t# waved G1: USB storage (docs/USB_STORAGE.md). A removable drive with no"
            print "\t\t# medium; waved loads /dev/udisk only after unmounting /mnt/udisk."
            print "\t\tif [ -d /sys/kernel/config/usb_gadget/amlogic/functions/mass_storage.usb0 ]; then"
            print "\t\t\techo > /sys/kernel/config/usb_gadget/amlogic/functions/mass_storage.usb0/lun.0/file"
            print "\t\t\techo 1 > /sys/kernel/config/usb_gadget/amlogic/functions/mass_storage.usb0/lun.0/removable"
            print "\t\t\tln -s /sys/kernel/config/usb_gadget/amlogic/functions/mass_storage.usb0 /sys/kernel/config/usb_gadget/amlogic/configs/amlogic.1/mass_storage.usb0"
            print "\t\tfi"
            inserted = 1
        }
        { print }
    ' "$1"
}

# The second G1 patch (1f154d8): its functions in a marked block before
# start_usbgadget (a blank line after it), and link, bind, check around
# start_usbgadget's usb_flash (its awk verbatim, the block shortened).
old_g1_v2() {
    cat > "$WORK/g1-block" <<'EOF'
# >>> waved G1: USB storage (docs/USB_STORAGE.md), by patch-stock-usbgadget.sh.
WAVED_G1_GADGET=${WAVED_G1_GADGET:-/sys/kernel/config/usb_gadget/amlogic}

waved_g1_link_drive()
{
    waved_g1_msc=$WAVED_G1_GADGET/functions/mass_storage.usb0
    [ -d "$waved_g1_msc" ] || return 0
    ln -s "$waved_g1_msc" "$WAVED_G1_GADGET/configs/amlogic.1/mass_storage.usb0"
}

waved_g1_check_bind()
{
    [ -n "$(cat "$WAVED_G1_GADGET/UDC" 2>/dev/null)" ] && return 0
    rm -f "$WAVED_G1_GADGET/configs/amlogic.1/mass_storage.usb0"
    usb_flash
}
# <<< waved G1
EOF
    awk -v block="$WORK/g1-block" '
        BEGIN { in_start = 0; done = 0 }
        /^[[:space:]]*(function[[:space:]]+)?start_usbgadget[[:space:]]*\(\)/ {
            while ((getline line < block) > 0) print line
            close(block)
            print ""
            in_start = 1
        }
        in_start && !done && /^[[:space:]]*usb_flash[[:space:]]*$/ {
            indent = $0
            sub(/usb_flash.*/, "", indent)
            print indent "waved_g1_link_drive"
            print
            print indent "waved_g1_check_bind"
            done = 1
            next
        }
        in_start && /^}/ { in_start = 0 }
        { print }
    ' "$1"
}

# run_patch <dir>: the patch on <dir>/init.d/S89usbgadget, backups in <dir>/backup.
run_patch() {
    INIT=$1/init.d/S89usbgadget BACKUP_DIR=$1/backup sh "$PATCH" > "$1/out" 2>&1
}

# setup <name> <source S89>: a unit with that S89 in its own init.d.
setup() {
    dir=$WORK/$1
    mkdir -p "$dir/init.d" "$dir/backup"
    cp "$2" "$dir/init.d/S89usbgadget"
    chmod 755 "$dir/init.d/S89usbgadget"
}

# check_patched <name>: S89 has the earlier steps and nothing of G1, is
# executable and parses, and nothing else is left in init.d.
check_patched() {
    dir=$WORK/$1
    s89=$dir/init.d/S89usbgadget
    [ -x "$s89" ] || fail "$1: S89 is not executable after the patch"
    sh -n "$s89" || fail "$1: the patched S89 does not parse"
    [ "$(ls -A "$dir/init.d")" = "S89usbgadget" ] \
        || fail "$1: left in init.d: $(ls -A "$dir/init.d" | tr '\n' ' ')"
    grep -q "echo -16384 > functions/uac2.0/p_volume_min" "$s89" || fail "$1: no p_volume range"
    grep -q "echo -10240 > functions/uac2.0/c_volume_min" "$s89" || fail "$1: no c_volume range"
    [ "$(grep -c 'mkdir -p functions/hid.0' "$s89")" -eq 1 ] || fail "$1: hid.0 not added once"
    if grep -q '^[[:space:]]*#[[:space:]]*adbd_enable[[:space:]]*$' "$s89"; then
        fail "$1: adbd_enable is still commented out (no ADB at boot under waved)"
    fi
    [ "$(awk '/start_usbgadget\(\)/ { f = 1 } f && /^}/ { f = 0 } f' "$s89" \
        | grep -c '^[[:space:]]*adbd_enable[[:space:]]*$')" -ge 2 ] \
        || fail "$1: adbd is not enabled in both boot branches of start_usbgadget"
    grep -q '^adbd_enable()' "$s89" || fail "$1: the adbd_enable() definition changed"
    if grep -q "waved G1\|waved_g1_" "$s89"; then
        fail "$1: USB storage is still in the boot: $(grep -n 'waved G1\|waved_g1_' "$s89" | head -3)"
    fi
}

# check_again <name>: a second run says so and changes nothing, no backup.
check_again() {
    dir=$WORK/$1
    s89=$dir/init.d/S89usbgadget
    backups=$(ls "$dir/backup" | wc -l)
    before=$(cksum < "$s89")
    run_patch "$dir" || { cat "$dir/out" >&2; fail "$1: the second run failed"; }
    grep -q "already patched" "$dir/out" || fail "$1: the second run did not say already patched"
    [ "$(cksum < "$s89")" = "$before" ] || fail "$1: the second run changed S89"
    [ "$(ls "$dir/backup" | wc -l)" -eq "$backups" ] || fail "$1: the second run made a backup"
}

# patch_copy <source S89> <name>: patch a stock copy, check it, patch again.
patch_copy() {
    setup "$2" "$1"
    run_patch "$WORK/$2" || { cat "$WORK/$2/out" >&2; fail "$2: the patch failed"; }
    check_patched "$2"
    [ "$(ls "$WORK/$2/backup" | wc -l)" -eq 1 ] || fail "$2: expected one backup"
    grep -n "usb_flash" "$1" > "$WORK/$2.flash.before"
    grep -n "usb_flash" "$WORK/$2/init.d/S89usbgadget" | sed 's/^[0-9]*://' > "$WORK/$2.flash.after"
    sed 's/^[0-9]*://' "$WORK/$2.flash.before" | cmp -s - "$WORK/$2.flash.after" \
        || fail "$2: the binds changed"
    pass "$2: volume ranges, hid.0 and ADB at boot, no USB storage at boot, parses, nothing left in init.d"
    check_again "$2"
    pass "$2: a unit with these steps applied: a second run changes nothing"
}

fixture "$WORK/S89.fixture"
patch_copy "$WORK/S89.fixture" fixture
REFERENCE=$WORK/fixture/init.d/S89usbgadget
BASES=fixture
if [ -n "${STOCK_S89:-}" ]; then
    patch_copy "$STOCK_S89" stock
    BASES="fixture stock"
fi

# A unit patched by an earlier version, G1 included: the G1 step goes, the
# rest stays exactly as this version writes it.
for base in $BASES; do
    reference=$WORK/$base/init.d/S89usbgadget
    for version in v1 v2; do
        name=$base-g1-$version
        "old_g1_$version" "$reference" > "$WORK/S89.$name"
        grep -q "waved G1" "$WORK/S89.$name" || fail "$name: the old patch did not apply"
        setup "$name" "$WORK/S89.$name"
        run_patch "$WORK/$name" || { cat "$WORK/$name/out" >&2; fail "$name: the patch failed"; }
        check_patched "$name"
        cmp -s "$WORK/$name/init.d/S89usbgadget" "$reference" \
            || fail "$name: not the same as a unit never given G1: $(diff "$reference" "$WORK/$name/init.d/S89usbgadget" | head -5)"
        [ "$(ls "$WORK/$name/backup" | wc -l)" -eq 1 ] || fail "$name: expected one backup"
        cmp -s "$WORK/$name/backup/"* "$WORK/S89.$name" || fail "$name: the backup is not the S89 it replaced"
        check_again "$name"
        pass "$base: the $version G1 boot step is removed, the rest left as it was"
    done
done

# Only G1 on an otherwise stock S89 (the earlier steps undone by hand).
old_g1_v2 "$WORK/S89.fixture" > "$WORK/S89.g1-only"
setup g1-only "$WORK/S89.g1-only"
run_patch "$WORK/g1-only" || { cat "$WORK/g1-only/out" >&2; fail "g1-only: the patch failed"; }
check_patched g1-only
cmp -s "$WORK/g1-only/init.d/S89usbgadget" "$REFERENCE" || fail "g1-only: not the same as a stock unit patched"
pass "G1 alone: removed, and the earlier steps applied"

echo "all $PASSES passed"
