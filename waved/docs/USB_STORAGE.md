# USB storage (G1)

The computer opens Wave's recordings volume as a drive. Plan and device-pass checklist:
[`superpowers/plans/2026-10-13-m3-g1-usb-storage.md`](superpowers/plans/2026-10-13-m3-g1-usb-storage.md).
Code: `crates/waved-system/src/usb_storage/` (service), `crates/waved-ui/src/screens_v2/usb_storage.rs`
(screen), `scripts/patch-stock-usbgadget.sh` (gadget).

## The volume

| What | Value | Source |
|---|---|---|
| Partition | `/dev/udisk` = `mmcblk0p16`, about 21.5 GB, exFAT, label "Wave Files" | `../docs/REPORT_DEVICE_DUMP.md:52`, `udisk_fix` strings |
| Boot mount | `udisk_fix /dev/udisk &` from `S02overlayfs` → `etc/datamount` `check_udisk` (lines 57–66). It runs `echo YES \| fsck.exfat -n`, formats with `mkfs.exfat -n "Wave Files"` when `lsblk` does not say exfat, and mounts `mount -t exfat /dev/udisk /mnt/udisk -o rw,fmask=0077` | stock rootfs, `usr/bin/udisk_fix` strings |
| exFAT driver | in the kernel (no `mount.exfat` helper, no module) | stock rootfs `sbin/` |
| Checker | `/sbin/fsck.exfat` → `exfatfsck` (exfat-utils): `[-a \| -n \| -p \| -y]`; `-p` repairs what is safe without asking | `strings sbin/exfatfsck` |
| Under the mount | `/mnt` is on the read-only squashfs root: while `/mnt/udisk` is unmounted, writes there fail with EROFS instead of landing on another filesystem | stock rootfs, `fstab` |
| What else lives there | `recordings/` (waved), stock's `Insta360_Wave_Controller/` installer, `misc/data/DONGLE.bin`, `AIRecord/` | `BLUETOOTH_PROTOCOL.md`, `../docs/GUIDE_DEVICE_ACCESS.md` |

The meeting database is on `/data` (`/data/waved/meetings.db`), not on this volume.

## What stock 1.2.2.6 does

`E` = `firmwares/fw_v1.2.2.6/decompiled/libins_entry.so.c`, `U` = `.../libui.so.c`, `S89` =
`firmwares/fw_v1.2.2.6/extracted/rootfs/etc/init.d/S89usbgadget` (main checkout only, gitignored).
Mode numbers and the status-id callback table were confirmed by disassembling
`rootfs/usr/lib/libins_entry.so` (Ghidra does not show the `.data` tables).

### Gadget modes

`thread_usb_mode_set` (E:144485, the argument table up to E:144825) runs
`/etc/init.d/S89usbgadget <arg>` for `usb_mode_set(dev, n)` (the real body is at E:144957; E:16371 is a
stub):

| n | Name | S89 argument | What the computer gets | Used for |
|---|---|---|---|---|
| 1 | `USB_MODE_PC` | `uac_hid` | audio + HID | normal |
| 2 | `USB_MODE_HID_MSC` | `uac_msc` | **storage only**: `uac_disable`, HID unlinked too | user storage mode |
| 3 | – | `cdc_msc` | storage + serial | only checked, never set |
| 5 | `USB_MODE_SELF` | `uac_adb` | audio + adb | debug, factory |
| 8 | `USB_MODE_UAC_HID_MSC` | `uac_hid_msc` | audio + HID + storage | Wave Controller update |
| 9 | `USB_MODE_STOP` | `stop` | nothing | cloud power off |

Each mode change re-enumerates: `all_disable` writes `none` to `UDC`, unlinks adb, cdc, msc and HID
(never `uac2.0`), the mode links its functions, and `usb_flash` binds `fdd00000.crgudc2` again
(S89:216-229). Mode 8 shows that this controller enumerates `uac2.0` + `hid.0` +
`mass_storage.usb0` together.

### Entering storage mode

- Touch UI: Settings → "USB Mode" (`usbmode_btn_clicked_event_cb`, U:20613) → "File Transfer"
  (`udisk_btn_clicked_event_cb`, U:24228). With no cable: "Please connect the USB-C cable and try
  again." With the device password on, the PIN first. No confirmation and no recording check. It
  opens the storage page (icon `icon/msc/usb_file_transfer.png`, U:24632; "File Transfer", "Files
  can be transferred to PC", `language_EN.json:135-137`) with one Exit button.
- Wave Controller: command 0x35 `SET_SYS_ENABLE_UDISK_MODE` → `slave_set_udisk_mode` (E:24793)
  ignores its enable byte and sets `GUI_USB_MODE` = 1 (E:24873).
- `GUI_USB_MODE` → `_usb_mode_set` (E:105166): stops the firmware download, then `usb_mode_set(2)`.
  Status 0xf2 = 1 makes the audio server move input and output off USB
  (`audio_listen_enter_udsik_event`, E:73435); the mode thread waits up to 20 s for that, runs
  `S89usbgadget uac_msc`, and only then
  `sync; echo 3 > /proc/sys/vm/drop_caches; echo /dev/udisk > .../lun.0/file` (E:144905).
- **`/mnt/udisk` stays mounted read-write the whole time.** Nothing unmounts it or remounts it
  read-only; `lun.0/ro`, `removable` and `nofua` are never written. Recordings are not blocked
  either; only the Wave Controller's "start meeting" is refused in modes 2 and 3 (E:24276-24284).
  The PC and Wave can write the same exFAT volume at once.
- At boot, `msc_init` writes `/dev/udisk` into `lun.0/file` with the function unlinked (S89:131-139),
  so the LUN holds the partition open from boot on.

### Leaving it

- Exit on the storage page: "Confirm exit" / "Please ensure no file transfer is in progress to avoid
  data loss" (`language_EN.json:139-140`), then `GUI_USB_MODE` = 0.
- Cable unplug: the MCU's `MCU_USB_REMOVE` (E:86241), and `usb_configure_check` (E:144304) on
  `/sys/.../android_usb/android0/state` (E:42176, E:109476). The UI says "USB-C cable not detected.
  File transfer has been exited." (`language_EN.json:141`).
- A reboot always comes back in mode 1. There is no timeout.
- On exit: mode 1 (or 5 with the debug switch), `sync`, `drop_caches`, `udisk_recovery()` (E:143385),
  which does nothing while the volume is still mounted read-write exFAT (the normal case: **no
  check after the PC wrote it**), else `umount -f` and `udisk_fix`. `exit_udisk_mode` (E:107500)
  rescans the files and restarts the firmware download; audio comes back to USB after 2 s.

## Measured on the device (2026-10-13): the composite with adb does not work

Two live tests on the running gadget, with `/etc/init.d/S89usbgadget` left
stock (a power cycle restores everything), BusyBox 1.32, kernel 5.4.242:

1. **UAC2 + HID + adb + mass storage** (the design below): the gadget binds and
   the Mac enumerates it, but when the host sets the mass-storage interface the
   controller times out enabling its endpoint (`crg_issue_command time out,
   cmd_control: 0x41` in `crg_udc_ep_enable`, from `fsg_main_thread`
   `do_set_interface`), adbd's FunctionFS closes, the gadget unregisters and
   `UDC` reads empty. Unlinking the drive and binding again brought adb back
   (`crg_rewrite_udc_for_error` in the log).
2. **UAC2 + HID + mass storage, adb unlinked** (stock's `uac_hid_msc` set,
   which stock uses for Wave Controller updates): during the 20 s window the
   Mac's USB and audio lists did not show the Wave at all, and putting
   `ffs.adb` back and binding did not bring the gadget or adb back; only a
   power cycle did. Removing `ffs.adb` from a bound configuration and re-adding
   it while adbd runs is not a safe runtime move on this controller.

So the boot patch (`scripts/patch-stock-usbgadget.sh`, step 3) must **not** be
applied: its bind check would catch case 1 at boot, but the design cannot work.
Until G1 is reworked, waved's USB storage row says "USB storage isn't
available" (no drive linked), which is safe. Next design to try: stock's own
mode switch (`S89usbgadget uac_msc` / `uac_hid_msc`, which re-runs stock's
whole gadget setup including adbd on the way back), tested live first with an
automatic return to the normal mode, as above.

## What waved does instead

**Swap the medium, never the gadget.** `scripts/patch-stock-usbgadget.sh` links
`mass_storage.usb0` into `configs/amlogic.1` at boot as a removable drive with no medium (it empties
the `lun.0/file` that `msc_init` set, then links, then `usb_flash` binds). The computer sees an empty
card reader next to Wave's audio, HID and adb. Storage mode only writes `lun.0/file`.

**The bind is checked at boot.** This controller has never been bound with `mass_storage.usb0`
next to `uac2.0`, `hid.0`, `ffs.adb` and `acm.0` (stock's mode 8 has no adb or acm), and stock S89
ignores a failed bind: Wave would boot with no USB, adb included. The patched `start_usbgadget`
runs `waved_g1_link_drive`, `usb_flash`, `waved_g1_check_bind`: if `$G/UDC` reads empty after the
bind, it removes the link, binds again without the drive, and says so on the console and in
`/tmp/waved-usbgadget.log` (with the `dmesg` tail). waved then finds no linked drive and answers
"USB storage isn't available". The script writes each change to a dot-named file next to S89 (rcS
runs `S??*` only), checks it with `sh -n`, makes it executable, syncs and renames it over S89; the
backups go to `/data/waved/S89usbgadget.bak.<ts>`. `scripts/test-patch-stock-usbgadget.sh` runs
the patch and the bind check under busybox against a fake configfs tree.

1. **Start** (`Command::StartUsbStorage`, System → USB storage). Refused, with nothing changed, while
   a take is recording, paused, finishing or awaiting its decision; while a firmware update
   installs (`OtaPhase::Installing`/`Restarting`: it restarts Wave; before that point the update
   waits for USB storage instead); with no configured computer (`/sys/class/udc/*/state`); with no
   linked removable drive; with `/mnt/udisk` not mounted. Then: phase Starting, `StopPlayback`, 1 s
   for an upload or playback that just began to open its file, `sync`, the unmount note
   (`/tmp/waved-usb-storage.unmounted`, the `/proc/mounts` line), `umount /mnt/udisk` (no lazy
   unmount: busy → "Recordings in use, try again", note removed, nothing changed), `/dev/udisk` →
   `lun.0/file`, read back, phase On.
2. **On.** The takeover screen; the meeting service starts no take, deletes nothing, the player
   plays nothing and the upload worker waits (an upload would find the file gone and mark it
   missing; a delete would remove the rows and leave the files, which would come back). Audio,
   HID and adb keep working.
3. **Stop** (the screen's "Stop USB storage", confirmed). `"\n"` → `lun.0/file`. If the computer
   locked the medium, f_mass_storage answers EBUSY: "Eject Wave on the computer first", storage
   stays on. Waved never forces the computer off.
4. **The computer ejects** (Finder ⏏): START STOP UNIT with LoEj closes the LUN; the 1 s poll reads
   `lun.0/file` empty and ends the mode.
5. **Cable pulled**: the UDC state is neither `configured` nor `suspended` for 3 polls. The disconnect
   clears the computer's lock, so the unload succeeds. A sleeping computer (`suspended`) keeps its drive.
6. **Back on Wave**: `lun.0/file` reads empty, then `fsck.exfat -p /dev/udisk` (at most 120 s), then
   mount exactly as `/proc/mounts` said before the start, falling back to the boot's
   `-t exfat -o rw,fmask=0077`. Only once `/proc/mounts` shows the volume: note removed, phase Off,
   "USB storage stopped". The meeting service reconciles the recordings as at boot (one deleted on
   the computer shows as missing; one marked missing whose files are back is kept again) and
   republishes Files.
   - **The mount fails**: the phase stays Stopping (the takeover stays, nothing records, uploads,
     deletes or reconciles on the bare mount point) and the status says "Recordings unavailable,
     restart Wave". The note stays, so the next waved checks and mounts it; a reboot mounts it
     as stock does.
   - **The check is stopped at 120 s** (killed part way through its repairs): the volume is not
     mounted at all, as above. A half-repaired exFAT is never mounted read-write by waved. A check
     that could not run at all (no `fsck.exfat`) still mounts, as stock's boot does after its
     read-only check.
7. **Waved restarts during USB storage** (crash, watchdog): `reclaim_at_start` runs before the
   meeting service opens anything.
   - A drive still loaded is unloaded. If the computer holds it (EBUSY), storage stays On with the
     takeover until the computer ejects it or the cable is pulled; the start-up **re-plug is off**
     (decision below). With `WAVED_USB_REPLUG=1` waved re-plugs USB (`/sys/class/udc/*/soft_connect`
     disconnect, 1 s, connect), writes connect up to 3 times until the UDC state shows a computer
     again (5 s each), then retries the unload; a re-plug that does not bring the computer back is
     logged as an error ("Wave may be off USB, adb included") and not tried again.
   - Then, with the volume unmounted and the LUN empty: if the unmount note is there (waved died
     between the unmount and the load, or between emptying the drive and the mount), it checks and
     mounts it as the note says. **Without a note** (a fresh boot: `/tmp` is tmpfs) it leaves the
     volume to the boot, whose `udisk_fix` may still be checking it.
   - A medium in an *unlinked* drive (stock `msc_init` at boot, before the patch empties it) is
     only emptied.
8. **The meeting service reconciles only on a mounted volume** (`/proc/mounts`,
   `waved_system::usb_storage::recordings_mounted`): at start-up, after USB storage, and on the
   5 s Files tick once a start-up or a return found it unmounted. Without that, a restart with the
   volume away marked every recording missing, for good. The interrupted deletes finish at the same
   points. A delete that finds the recordings directory itself missing fails and keeps its claim
   (`deleting`) until then, instead of removing the rows.

The two sides never write at once: the LUN is loaded only after the unmount succeeded, and the volume
is mounted only once the LUN reads back empty. The kernel would not stop either: the LUN opens the
partition without `O_EXCL`, and `mount` does not see that open.

**Decision: the start-up re-plug is off** until the device pass shows that `soft_connect`
disconnect/connect on `fdd00000.crgudc2` brings the computer, audio and adb back. `soft_connect` is
write-only and the kernel accepts "connect" even when the controller does not start, so a bad
re-plug can leave Wave off USB with nothing but the log to show it. Cost if wrong (it works and stays
off): after a crash during USB storage, the computer keeps its drive until it is ejected there or
the cable is pulled; nothing is lost, the takeover says what is going on.

Still open: an update that reaches Installing does not itself wait for USB storage (the update agent
is adding that check); a start during Installing/Restarting is refused here. The upload worker does
not look at `/proc/mounts`: at boot, before stock's `udisk_fix` mounts the volume, an upload could
find its file missing and mark it so; the next reconciliation turns it back (kept on Wave, not
queued again).

### Kernel behaviour this relies on (Linux 5.4 `f_mass_storage`; verify in the device pass)

- `lun.0/removable` defaults to 1 for `lun.0`; waved and the patch check and set it anyway.
- Writing `lun.0/file` while bound works for a removable LUN; an empty name closes the medium;
  EBUSY ("eject attempt prevented") while the host has PREVENT MEDIUM REMOVAL set. 5.4 has no
  `forced_eject`.
- A host eject (START STOP UNIT, LoEj) closes the medium: `lun.0/file` reads empty.
- A disconnect or bus reset clears PREVENT MEDIUM REMOVAL.
- `lun.0/ro` can only change with no medium loaded (waved never sets it).
- `soft_connect` keeps the functions bound (no unbind, so adb's functionfs and the UAC2 card stay).

Measured on the device: _(fill in at the device pass: does the composite with `mass_storage.usb0`
bind (`$G/UDC` after the patch and reboot, `/tmp/waved-usbgadget.log`); does macOS lock the medium;
mount options in `/proc/mounts`; `soft_connect` on `crgudc2` (does the UDC state go to `not
attached` and back, do audio and adb come back); xruns during a 500 MB copy)_.

## Recovering by hand

```sh
G=/sys/kernel/config/usb_gadget/amlogic
L=$G/functions/mass_storage.usb0/lun.0
cat $G/UDC; cat $L/file; cat /sys/class/udc/*/state; grep udisk /proc/mounts
cat /tmp/waved-usbgadget.log /tmp/waved-usb-storage.unmounted 2>/dev/null
# Take the drive back (fails with EBUSY while the computer holds it: eject there or unplug):
echo > $L/file && cat $L/file && fsck.exfat -p /dev/udisk && mount -t exfat /dev/udisk /mnt/udisk -o rw,fmask=0077 && rm -f /tmp/waved-usb-storage.unmounted
# Re-plug without touching the cable (unproven on crgudc2: it may leave Wave off USB):
echo disconnect > /sys/class/udc/fdd00000.crgudc2/soft_connect; sleep 1; echo connect > /sys/class/udc/fdd00000.crgudc2/soft_connect
# Waved never unbinds the gadget; if it is unbound anyway ($G/UDC empty):
echo fdd00000.crgudc2 > $G/UDC
```

Never `echo /dev/udisk > $L/file` while `/mnt/udisk` is mounted.

**Undo the gadget patch** (over adb). Each patching run saved the S89 it replaced; the newest
backup is the script from just before the last patch (the volume and HID patches only, if that run
added USB storage). Restore it the way the patch writes (dot-named file, executable, synced, then
renamed), check it, and reboot:

```sh
B=$(ls -t /data/waved/S89usbgadget.bak.* | head -1); echo "$B"
cp "$B" /etc/init.d/.S89usbgadget.restore && sh -n /etc/init.d/.S89usbgadget.restore \
  && chmod 755 /etc/init.d/.S89usbgadget.restore && sync \
  && mv -f /etc/init.d/.S89usbgadget.restore /etc/init.d/S89usbgadget && sync && echo restored
grep -c waved_g1_ /etc/init.d/S89usbgadget   # 0: the USB storage patch is gone
ls -A /etc/init.d                            # no .S89usbgadget.restore left
reboot
```

Never copy a backup into `/etc/init.d` under
an `S??*` name: rcS runs it at boot. If adb is gone after a reboot (the bind check failed too), only
the UART console is left (`../docs/GUIDE_DEVICE_ACCESS.md`): there, `echo fdd00000.crgudc2 > $G/UDC`
after `rm $G/configs/amlogic.1/mass_storage.usb0`, then the restore above.
