# USB storage (G1)

The computer opens Wave's recordings volume as a drive, through stock's own USB mode switch
(`S89usbgadget uac_hid_msc`, done by waved at runtime). **adb is off while it lasts.** Plan:
[`superpowers/plans/2026-10-13-m3-g1-usb-storage.md`](superpowers/plans/2026-10-13-m3-g1-usb-storage.md);
device-pass checklist below. Code: `crates/waved-system/src/usb_storage/` (service, gadget switch,
adbd), `crates/waved-ui/src/screens_v2/usb_storage.rs` (screen). `scripts/patch-stock-usbgadget.sh`
(the boot gadget's UAC volume and HID) adds nothing for USB storage and removes the old G1 boot step.

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

## Measured on the device (2026-10-13)

Three live tests on the running gadget, with `/etc/init.d/S89usbgadget` left
as it was (a power cycle restores everything), BusyBox 1.32, kernel 5.4.242.
The unit's normal mode is `uac2.0` + `ffs.adb` + `hid.0` (its S89 `start`
enables adb), configuration string `hid`, UDC `fdd00000.crgudc2`.

1. **UAC2 + HID + adb + mass storage** (the first design: the drive linked
   from boot next to everything else): the gadget binds and the Mac enumerates
   it, but when the host sets the mass-storage interface the controller times
   out enabling its endpoint (`crg_issue_command time out, cmd_control: 0x41`
   in `crg_udc_ep_enable`, from `fsg_main_thread` `do_set_interface`), adbd's
   FunctionFS closes, the gadget unregisters and `UDC` reads empty. Unlinking
   the drive and binding again brought adb back (`crg_rewrite_udc_for_error`
   in the log). **The drive cannot sit next to adb on this controller.**
2. **`ffs.adb` removed from a bound configuration and re-added without
   restarting adbd**: the gadget was left with no USB at all, adb included;
   only a power cycle brought it back. Never move `ffs.adb` while adbd runs
   from before.
3. **Stock's own sequence (v3): works.** Storage mode = stock `uac_hid_msc`
   (`all_disable`: `echo none > UDC`, `killall -9 adbd`, `rm -rf` the config
   links `ffs.adb`, `acm.0`, `mass_storage.usb0`, `hid.0`, `uac2.0` stays;
   `msc_enable`: `/dev/udisk` → `lun.0/file`, configuration `msc`, link
   `mass_storage.usb0`; `hid_enable`: configuration `hid`, link `hid.0`;
   `usb_flash`: `sleep 2`, `echo fdd00000.crgudc2 > UDC`). With an empty LUN
   the gadget stayed `configured` for 20 s, no kernel errors, and the Mac kept
   Wave's audio devices. Back to normal: `echo none > UDC`, `killall -9 adbd`,
   remove the `ffs.adb`, `mass_storage.usb0`, `hid.0` links, empty
   `lun.0/file`, **a fresh `/usr/bin/adbd &`** (stock `adbd_enable`),
   configuration `adb`, link `ffs.adb`, configuration `hid`, link `hid.0`,
   restore the configuration string, `sleep 4`, `echo fdd00000.crgudc2 > UDC`.
   adb came back by itself.

So waved now does exactly 3, at runtime. The boot patch's old G1 step (the
drive linked at boot) must never be on a unit: `scripts/patch-stock-usbgadget.sh`
removes it when it finds it (either earlier version) and keeps its other steps
(UAC volume ranges, `hid.0`).

Still to measure (device pass below): v3 with the real volume (an empty LUN
was tested), whether macOS locks the medium, the computer's eject, a pulled
cable, a waved restart in storage mode, and that waved's own USB handles come
back by themselves (below).

## What waved does

**Storage mode is stock's mode switch.** `crates/waved-system/src/usb_storage/`
runs the v3 sequence through configfs (`device::ConfigfsGadget`) and adbd
(`device::SystemAdbd`), behind the `Gadget` trait with a fake in the tests.
**adb is off while storage mode is on**: this controller cannot run the drive
next to adb. Audio (`uac2.0` stays linked) and HID re-enumerate with the
switch; the computer keeps Wave's microphone and speaker.

1. **Start** (`Command::StartUsbStorage`, System → USB storage). Refused, with
   nothing changed, while a take is recording, paused, finishing or awaiting
   its decision; while a firmware update installs (`OtaPhase::Installing`/
   `Restarting`: it restarts Wave; before that point the update waits for USB
   storage instead); with the gadget not in its normal mode (no gadget or no
   drive function: "USB storage isn't available"); with no configured computer
   (`/sys/class/udc/*/state`); with `/mnt/udisk` not mounted. A medium in the
   unlinked drive (stock's `msc_init` loads `/dev/udisk` at boot; the computer
   never sees it) is emptied first. Then: phase Starting, `StopPlayback`, 1 s
   for an upload or playback that just began to open its file, `sync`, the
   unmount note (`/tmp/waved-usb-storage.unmounted`, the `/proc/mounts` line),
   `umount /mnt/udisk` (no lazy unmount: busy → "Recordings in use, try again",
   note removed, nothing changed, adb untouched).
2. **The switch** (`enter_storage`): the normal mode is read (UDC, the
   configuration string, which of `ffs.adb`, `acm.0`, `hid.0` are linked) and
   written to `/tmp/waved-usb-storage.gadget` before anything changes; then
   stock `all_disable`, `lun.0/removable` = 1 if it is not (the computer's
   eject only empties a removable LUN; it is the kernel default for `lun.0`),
   `/dev/udisk` → `lun.0/file` (read back), configuration `msc`, link
   `mass_storage.usb0`, configuration `hid`, link `hid.0`, 2 s, bind, `UDC`
   read back. Then up to 10 s for the computer to reach `configured`.
   - **The bind fails, or no computer in 10 s**: straight back to normal USB
     (the way back, 6), then the volume is checked and mounted, and the status
     says "USB storage isn't available".
3. **On.** The takeover screen; the meeting service starts no take, deletes
   nothing, the player plays nothing and the upload worker waits. No adb.
4. **Stop** (the screen's "Stop USB storage", confirmed). First `"\n"` →
   `lun.0/file` **while still bound**: if the computer locked the medium,
   f_mass_storage answers EBUSY: "Eject Wave on the computer first", storage
   stays on. Waved never forces the computer off (the unbind of the way back
   would end the lock unasked, so Stop asks first). Then the way back.
5. **The computer ejects** (Finder ⏏): START STOP UNIT with LoEj closes the
   LUN; the 1 s poll reads `lun.0/file` empty and takes the way back. **Cable
   pulled**: the UDC state is neither `configured` nor `suspended` for 3 polls
   (also a gadget that fell off the bus, as in measurement 1): the way back. A
   sleeping computer (`suspended`) keeps its drive.
6. **The way back** (`leave_storage`), to the noted normal mode: unbind (this
   ends the computer's hold on the drive), `killall -9 adbd` (waits up to 2 s
   for it to go), unlink `ffs.adb`, `acm.0`, `mass_storage.usb0`, `hid.0`,
   empty `lun.0/file` (read back empty), then for what was linked, in stock's
   order: **a fresh adbd** and configuration `adb`, link `ffs.adb`; (`acm`,
   `acm.0`); configuration `hid`, link `hid.0`; the configuration string as it
   was; 4 s (FunctionFS binds only once adbd wrote its descriptors); bind,
   `UDC` read back; the gadget note removed. Tried 3 times, 2 s apart; each try
   stops adbd before it starts one. If adbd does not start, `ffs.adb` stays
   unlinked so audio and HID still bind (adb then needs a reboot).
   - Only what was linked comes back: a unit without adb in its normal mode
     (stock's `start`, release lockdown, `docs/SECURITY.md`) does not get it.
   - **adbd** is started with busybox `start-stop-daemon -S -b -o -x
     /usr/bin/adbd` (cwd `/`, stdio on /dev/null): `-b` forks and detaches it
     (setsid, other descriptors closed), so it outlives waved and holds none of
     waved's files; `-x` refuses a second one, and waved also checks `/proc`
     (`comm` = `adbd`, zombies ignored) before starting. Killing adbd ends
     every `adb shell`: a waved started by hand must be under `setsid`
     (`waved/CLAUDE.md`), or it dies with the shell.
7. **Back on Wave**: only once `lun.0/file` reads empty (even if normal USB did
   not come back: the drive is then unlinked and empty, the computer cannot
   write), `fsck.exfat -p /dev/udisk` (at most 120 s), then mount exactly as
   `/proc/mounts` said before the start, falling back to the boot's
   `-t exfat -o rw,fmask=0077`. Only once `/proc/mounts` shows the volume:
   note removed, phase Off, "USB storage stopped". The meeting service
   reconciles the recordings as at boot (one deleted on the computer shows as
   missing; one marked missing whose files are back is kept again) and
   republishes Files.
   - **The drive still holds the partition** (the way back failed before it
     emptied it): never mounted; phase Stopping, "Recordings unavailable,
     restart Wave".
   - **The mount fails**: the phase stays Stopping (the takeover stays,
     nothing records, uploads, deletes or reconciles on the bare mount point)
     and the status says "Recordings unavailable, restart Wave". The note
     stays, so the next waved checks and mounts it; a reboot mounts it as
     stock does.
   - **The check is stopped at 120 s** (killed part way through its repairs):
     the volume is not mounted at all, as above. A half-repaired exFAT is never
     mounted read-write by waved. A check that could not run at all (no
     `fsck.exfat`) still mounts, as stock's boot does after its read-only
     check.
8. **Waved restarts during USB storage** (crash, watchdog): `reclaim_at_start`
   runs before the meeting service opens anything.
   - The gadget in storage mode (`mass_storage.usb0` linked), or unbound with
     the gadget note there (a waved died part way through a switch): **back to
     normal USB first**, as the note says (without one, Wave's boot mode:
     audio, adb, HID). The unbind ends the computer's hold, so this always
     takes the drive back; the computer may say it was not ejected properly.
   - Then, with the volume unmounted and the LUN empty: if the unmount note is
     there, it checks and mounts the volume as the note says; after a switch it
     mounts it anyway (the boot's mount without a note). **Without either** (a
     fresh boot: `/tmp` is tmpfs) it leaves the volume to the boot, whose
     `udisk_fix` may still be checking it.
   - An unbound gadget without a note is the boot's (`S89usbgadget` binds it
     after waved starts, `S60` < `S89`): left alone. A medium in the unlinked
     drive (stock `msc_init`) is only emptied.
9. **The meeting service reconciles only on a mounted volume** (`/proc/mounts`,
   `waved_system::usb_storage::recordings_mounted`): at start-up, after USB
   storage, and on the 5 s Files tick once a start-up or a return found it
   unmounted. The interrupted deletes finish at the same points. A delete that
   finds the recordings directory itself missing fails and keeps its claim
   (`deleting`) until then, instead of removing the rows.

The two sides never write at once: the LUN is loaded only after the unmount
succeeded, and the volume is mounted only once the LUN reads back empty. The
kernel would not stop either: the LUN opens the partition without `O_EXCL`, and
`mount` does not see that open.

### Decisions (2026-10-13 rework)

- **Stock's mode switch, adb off during it.** Cost if wrong: none known for
  storage itself (v3 worked); adb is unavailable while the computer has the
  drive, so the device pass reads the log afterwards.
- **A waved restart in storage mode ends it** (back to normal USB, then the
  volume). The old design kept the drive with a computer that held it; now the
  unbind always takes it back. Cost if wrong: a crash in the middle of a copy
  cuts the copy (the computer reports an unclean eject); `fsck.exfat -p` runs
  before the mount. Keeping storage on instead would leave the unit without
  adb until someone ejects the drive on the computer.
- **The way back restores what was linked; without a note, the boot mode with
  adb** (a unit must never lose adb, `docs/SECURITY.md`). Cost if wrong: only
  when a waved start finds storage mode without the note (someone ran
  `S89usbgadget uac_hid_msc` by hand): a unit whose normal mode has no adb gets
  adb until the next reboot.
- **adbd via `start-stop-daemon -b`** rather than `/usr/bin/adbd &` from waved:
  a child of waved would inherit waved's open descriptors (ALSA, hidg, the
  framebuffer) and could die with it. Cost if wrong (no `start-stop-daemon`
  on a unit; it is in the stock 1.2.2.6 rootfs as a busybox applet): adbd does
  not start, `ffs.adb` stays unlinked, audio and HID come back, adb needs a
  reboot.
- **3 tries at normal USB, then give up.** Cost if wrong: Wave stays off USB
  (adb included) until a reboot; there is no serial console.
- **The start-up `soft_connect` re-plug is gone** (it was off by default and
  never proven on `crgudc2`): the unbind does what it was for.

### Kernel behaviour this relies on (Linux 5.4 `f_mass_storage`)

From the 5.4 source (`drivers/usb/gadget/function/f_mass_storage.c`,
`storage_common.c`); the device pass confirms the parts marked (device).

- `lun.0` is created removable (`fsg_alloc_inst`: `config.removable = true`);
  waved writes `removable` = 1 anyway, while unbound, if it reads otherwise.
- Writing `lun.0/file` (`fsg_store_file`): EBUSY ("eject attempt prevented")
  while the host has PREVENT MEDIUM REMOVAL set and a medium is loaded; an
  empty name (`"\n"`, the newline is dropped) closes the medium; a path opens
  it, dropping any previous one. Reading it gives the open file's path
  (`/dev/mmcblk0p16` for `/dev/udisk`), or nothing.
- The host's eject (device): START STOP UNIT with LoEj on a removable LUN,
  once removal is allowed, closes the medium (`do_start_stop` →
  `fsg_lun_close`): `lun.0/file` reads empty. While the lock is set the eject
  is refused and the LUN stays.
- Unbind (`echo none > UDC`) and a disconnect (cable) raise
  `FSG_STATE_CONFIG_CHANGE`; `handle_exception` clears every LUN's
  `prevent_medium_removal`, and `fsg_unbind` waits for it. The LUN stays open
  across the unbind and the unlink (the function instance owns it): `file`
  still names the partition until it is emptied. This is why the way back can
  always empty it, and why Stop tries the unload while still bound.
- 5.4 has no `forced_eject`.

### waved's own USB handles across the switch (check in the device pass)

The unbind takes the gadget's functions down and the bind brings them up new,
on the way in and again on the way back: f_uac2's ALSA card (`hw:1`) is freed
and a new one created, and `/dev/hidg0` belongs to a new `hid.0` (the old one
went with its link). Every waved handle on them comes back by itself, tried
once a second, each outage logged once (its first failure and its end): the
UAC bridges reopen `hw:1,0` on error; the mixer poll (`waved-audio`
`mixer_poll`, Mac volume and mute → Wave) drops the old card's mixer when its
events fail (ENODEV), opens `hw:1` again, writes Wave's volume and mute into
the new card (it starts at the kernel's defaults) and only then reads it, so
the defaults are never taken for the Mac's; the HID key writer, the reader
(call LEDs, the Wave Controller's requests) and the PC link's writer
(`waved-hid` `node`) check before using their handle that `/dev/hidg0` is
still the node they opened (device and inode; a read waits at most 1 s), open
it again when not or after an error, and keep keys and replies that wait for
it (bounded, oldest dropped first, none sent more than 10 s late). The check
comes before the error because Linux 5.4 `f_hid` frees `hid.0` under open
files: a read or write on the old node may never return.

Still open: the upload worker does not look at `/proc/mounts`: at boot, before
stock's `udisk_fix` mounts the volume, an upload could find its file missing
and mark it so; the next reconciliation turns it back (kept on Wave, not
queued again). Installs wait while USB storage is on (waved-update's gate,
`docs/UPDATES.md` U10).

## Device-pass checklist

Run from the Mac. `G=/sys/kernel/config/usb_gadget/amlogic`,
`L=$G/functions/mass_storage.usb0/lun.0` (expand them by hand inside single
quotes). **adb is gone while storage mode is on**: read
`/data/waved/waved.log` (`usb storage:` lines) and `dmesg` afterwards. waved
must run under the watchdog (or `setsid`), never from an `adb shell` the
switch kills.

0. **Before.** Deploy waved (`docs/DISPLAY_HARDWARE.md#deploy`). If the unit
   ever got the old G1 boot step (`adb shell grep -c waved_g1_
   /etc/init.d/S89usbgadget` not 0), run the new
   `scripts/patch-stock-usbgadget.sh` once (it removes it; on the Mac first:
   `docker run --rm --network none -v "$PWD/waved/scripts:/w:ro" busybox:1.36
   sh /w/test-patch-stock-usbgadget.sh`), check the count is 0, reboot. Record
   the normal mode: `adb shell 'cat $G/UDC; ls -l $G/configs/amlogic.1/; cat
   $G/configs/amlogic.1/strings/0x409/configuration; cat $L/file $L/removable;
   pidof adbd; grep udisk /proc/mounts; cat /sys/class/udc/*/state'`.
1. **The real volume, ejected in Finder.** System → USB storage. Expect the
   takeover ("Starting", then "USB storage is on" within ~10 s), `adb devices`
   empty, "Wave Files" on the Mac desktop, Wave still in Audio MIDI Setup.
   Copy a recording from the drive to the Mac and a small file (`g1.txt`)
   onto it. Eject in Finder. Expect the takeover to leave by itself within
   ~15 s and "USB storage stopped"; `adb devices` lists Wave again. Then:
   `adb shell 'ls /mnt/udisk/g1.txt; grep udisk /proc/mounts; cat $G/UDC; ls
   $G/configs/amlogic.1/; cat $G/configs/amlogic.1/strings/0x409/configuration;
   cat $L/file; pidof adbd; ls /tmp/waved-usb-storage.* 2>&1; grep "usb
   storage:" /data/waved/waved.log | tail -30; dmesg | tail -60'`: the file is
   there, the mount options as before, the links and configuration string as
   in 0, the LUN empty, one adbd, no notes, no `crg_issue_command time out`.
   Audio: play music on the Mac through Wave and talk into it. Then waved's
   USB handles (above), without restarting waved:
   - the Mac's volume keys (or its menu-bar slider) change Wave's speaker
     volume and F10 mutes it, as they did before the session;
   - Wave's Louder/Quieter move the Mac's volume (one Mac step per press, no
     ping-pong back);
   - the Wave Controller app connects and answers (PC link), e.g. it shows
     Wave's volume and a change there reaches Wave;
   - `grep -E "uac-control|hid|pclink" /data/waved/waved.log | tail -30`: per
     switch at most one failure line and one "is back"/"again"/"new node" line
     each for the mixer, the key writer, the reader and the PC link, no line
     repeating every second.
   If one stays dead, note it here with its log lines; `killall waved` (the
   watchdog restarts it) is the workaround.
2. **Stop without ejecting.** Start, wait for the drive, Stop USB storage →
   Stop. If macOS locked the medium: "Eject Wave on the computer first" and the
   screen stays; eject in Finder and it leaves. If not: storage stops and the
   Mac says "Disk Not Ejected Properly". Note which one here. adb comes back.
3. **Cable pull.** Start, then unplug the cable. Wait 15 s (3 s to notice, the
   way back, the check), plug back in: `adb devices` lists Wave, no drive in
   Finder, audio works, and the log has "no computer for 3 s", "normal USB is
   back", "checked /dev/udisk", "off, recordings mounted on Wave".
4. **waved restart in storage mode.** adb is down during it, so arm the kill
   first: `adb shell 'setsid sh -c "sleep 40; killall waved" </dev/null
   >/dev/null 2>&1 &'`, then start storage within 40 s and wait. After the
   watchdog restart (~3 s) the Mac loses the drive (maybe "not ejected
   properly"), adb comes back, and the log has "USB was left in storage mode;
   back to normal USB first" and "reclaimed, recordings mounted". Files shows
   every recording (not Damaged). Optional: a kill timed into a way back (eject,
   then the kill a few seconds later) shows the same.
5. **Large copy.** ≥ 500 MB from the drive while music plays on the Mac
   through Wave; `/tmp/audio_stats.json` (`xruns`, `uac`) afterwards.
6. **Mac sleep.** Start, close the lid 30 s, open: the drive is still there.
7. **Refusals.** While recording: "Stop the recording first". Cable out:
   "Connect a computer first". With an `adb shell` sitting in `cd /mnt/udisk`:
   "Recordings in use, try again", nothing changed (adb stays, `/mnt/udisk`
   mounted, `$L/file` empty).
8. **PIN.** With a PIN set, System asks for it before USB storage; Stop does
   not.

## Recovering by hand

Over adb, once it is back (normal mode, or after a reboot):

```sh
G=/sys/kernel/config/usb_gadget/amlogic
L=$G/functions/mass_storage.usb0/lun.0
cat $G/UDC; ls -l $G/configs/amlogic.1/; cat $L/file; cat /sys/class/udc/*/state; pidof adbd
grep udisk /proc/mounts
cat /tmp/waved-usb-storage.gadget /tmp/waved-usb-storage.unmounted 2>/dev/null
# The recordings are not mounted ("Recordings unavailable, restart Wave") and $L/file is empty:
fsck.exfat -p /dev/udisk && mount -t exfat /dev/udisk /mnt/udisk -o rw,fmask=0077 && rm -f /tmp/waved-usb-storage.unmounted
```

Never `echo /dev/udisk > $L/file` while `/mnt/udisk` is mounted, and never
link `mass_storage.usb0` while `ffs.adb` is linked (measurement 1).

**Wave off USB** (no adb, no audio): pull the cable for 5 s and plug it back
(during storage mode waved goes back to normal USB on a pull); if that does not
bring adb back (waved gave up after 3 tries, see the log afterwards), restart
Wave: the boot builds the gadget from scratch. There is no serial console.

**Undo the gadget patch** (over adb). Each patching run saved the S89 it
replaced; the newest backup is the script from just before the last patching
run. Restore it the way the patch writes (dot-named file, executable, synced,
then renamed), check it, and reboot:

```sh
B=$(ls -t /data/waved/S89usbgadget.bak.* | head -1); echo "$B"
cp "$B" /etc/init.d/.S89usbgadget.restore && sh -n /etc/init.d/.S89usbgadget.restore \
  && chmod 755 /etc/init.d/.S89usbgadget.restore && sync \
  && mv -f /etc/init.d/.S89usbgadget.restore /etc/init.d/S89usbgadget && sync && echo restored
grep -c waved_g1_ /etc/init.d/S89usbgadget   # must be 0: never the old USB storage boot step
ls -A /etc/init.d                            # no .S89usbgadget.restore left
reboot
```

Never restore a backup that has the old G1 step (`grep -c waved_g1_` or `grep
-c "waved G1"` not 0), and never copy a backup into `/etc/init.d` under an
`S??*` name: rcS runs it at boot.
