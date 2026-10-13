# M3 G1 — USB storage (mass-storage mode)

Root `TODO.md` G1; mockup `docs/mockups/png/usb.png`; stock behaviour and the design in
`waved/docs/USB_STORAGE.md`.

## Goal

The computer opens Wave's recordings volume as a drive. A takeover screen with "Stop USB
storage" stays up while it does. Unplugging the cable ends it cleanly. Wave and the computer
never write the recordings filesystem at the same time, and the meeting database and a
recording in progress stay safe.

## What stock does (summary; details and line refs in USB_STORAGE.md)

- Storage mode re-enumerates the gadget as storage only (`S89usbgadget uac_msc`): the computer
  loses Wave's microphone, speaker and HID while it lasts.
- It loads `/dev/udisk` into `lun.0/file` **without unmounting `/mnt/udisk`**: both sides can
  write the exFAT volume at once.
- It ends on an Exit button (with a "no file transfer in progress" confirmation) or on cable
  unplug, and goes back to `uac_hid`.

## Rework after the device pass (2026-10-13)

The device showed decision 1 cannot work on this controller: the drive next to adb times out
enabling its endpoint, and moving `ffs.adb` at runtime without a fresh adbd left the unit with no
USB at all. Stock's own sequence did work (live test v3, `waved/docs/USB_STORAGE.md`, "Measured
on the device"). The rework:

- **R1. Storage mode is stock's `uac_hid_msc` switch, at runtime** (replaces 1 and 13): after the
  unmount, unbind, kill adbd, unlink `ffs.adb`/`acm.0`/`mass_storage.usb0`/`hid.0` (`uac2.0`
  stays), `/dev/udisk` into the LUN, link drive and HID, bind, up to 10 s for the computer. A
  switch that does not come up goes straight back to normal USB and remounts ("USB storage isn't
  available"). adb is off while storage is on; audio and HID re-enumerate.
- **R2. The way back** (Stop, the computer's eject, a pulled cable): unbind, kill adbd, unlink,
  empty the LUN, a fresh detached adbd (`start-stop-daemon -S -b -o -x /usr/bin/adbd`, never a
  second one), link what was linked before (noted in `/tmp/waved-usb-storage.gadget`), restore the
  configuration string, 4 s, bind; 3 tries. Then fsck and mount as before (2, 11, 12 unchanged),
  only with the LUN read back empty. Stop still tries the unload while bound first, so a locked
  medium still says "Eject Wave on the computer first" (4 unchanged).
- **R3. A waved restart in storage mode goes back to normal USB first** (replaces 6's "storage
  stays on" and 10): the gadget in storage mode, or unbound with the gadget note, is restored
  (without a note: Wave's boot mode, audio + adb + HID), which ends the computer's hold; then the
  existing reclaim and mount. An unbound gadget without a note is the boot's and is left alone.
  The `soft_connect` re-plug and `WAVED_USB_REPLUG` are gone.
- **R4. The boot patch has no G1 step** (replaces task 6): `scripts/patch-stock-usbgadget.sh`
  keeps the UAC volume ranges and `hid.0`, and removes a G1 step left by either earlier version.
- Open: waved's own `/dev/hidg0` and `hw:1` mixer handles do not reopen after the switch
  (USB_STORAGE.md, "waved's own USB handles"); the device pass checks it.

Decisions 1, 6 (restart part), 10 and 13 below are superseded; the rest stand.

## Decisions

1. *(Superseded by R1.)* **Swap the medium, never the gadget.** `scripts/patch-stock-usbgadget.sh` links
   `mass_storage.usb0` into `configs/amlogic.1` at boot as an empty, removable drive. Storage
   mode writes `/dev/udisk` into `lun.0/file`; stopping writes an empty string. The UDC is never
   unbound, so UAC2 audio, HID and adb keep working through the whole mode, and no path can leave
   the gadget unbound.
   - The gadget can do both: stock's own Wave Controller update mode (8, `uac_hid_msc`) runs
     `uac2.0` + `hid.0` + `mass_storage.usb0` in one configuration.
   - Cost if wrong: if the composite does not enumerate with storage linked, or audio
     glitches during copies, the device pass shows it. The fallback is to pause the UAC
     bridges in software during the mode, which needs no re-enumeration.
2. **Unmount first, export second; unload first, mount second.** Start: `sync`, unmount
   `/mnt/udisk` (refuse when busy), then load the LUN. Back: unload the LUN, read it back
   empty, check the volume (`fsck.exfat -p`), then mount it exactly as it was mounted
   (`/proc/mounts`), falling back to stock's `mount -t exfat /dev/udisk /mnt/udisk -o
   rw,fmask=0077`. The kernel does not stop either double writer itself: the LUN's open is not
   exclusive, and mount does not see it.
3. **Refuse to start** while recording, paused, finalizing a take, or a decision is pending ("Stop
   the recording first"); with no computer configured on USB ("Connect a computer first"); or
   when the gadget has no removable drive linked ("USB storage isn't available").
4. **Stop never forces.** If the computer still holds the drive (it locked the medium, so the
   unload fails with EBUSY), Stop says "Eject Wave on the computer first" and storage stays on.
   The computer's own eject ends the mode by itself: the LUN reads empty.
5. **Cable pull** ends the mode: the UDC state stays neither `configured` nor `suspended` for
   3 s. A disconnect clears the computer's lock in `f_mass_storage`, so the unload succeeds. A
   sleeping computer (`suspended`) keeps its drive.
6. *(Restart part superseded by R3.)* **A waved restart while exported** (crash and watchdog): before the meeting service opens
   the database, `reclaim_at_start` unloads the LUN, then checks and mounts the volume. If the
   computer holds the drive, storage stays on until it ejects it or the cable is pulled (the
   start-up re-plug is opt-in, decision 10). A volume a previous waved unmounted (the tmpfs
   unmount note, written before the unmount) is mounted again even with an empty LUN; without a
   note an unmounted volume is the boot's (`udisk_fix` may still be checking it).
   *Review 2026-10-13:* the meeting service also reconciles only while `/proc/mounts` shows the
   volume, and `MissingArtifact` turns back into `LocalOnly` (`Uploaded` if delivered) when both
   files are there again: a restart or a failed mount marked every recording missing for good.
7. **The meeting service waits while the volume is away** (`UsbStorageState::recordings_away`):
   no new recording, no uploads (an upload would mark the file missing), and reconciliation
   runs again when the volume is back, so recordings deleted on the computer show as missing.
8. **Stopping is not PIN-locked.** The takeover and its confirmation are no longer guarded by
   System's PIN. Pulling the cable stops it anyway; a PIN would only stand in the way. Starting
   still needs the PIN (System).
9. **Copy:** the mockup's "Audio is off until you stop" is wrong under decision 1. The screen says
   "Your computer can open the recordings. Recording is off until you stop." Still right under R1
   (`uac2.0` stays linked); adb being off is not user-facing, so it is only in the docs.

Added after the review (2026-10-13):

10. *(Superseded by R3: removed.)* **The start-up re-plug is off** (`WAVED_USB_REPLUG=1` turns it on) until the device pass shows
    `soft_connect` disconnect/connect on `crgudc2` brings the computer, audio and adb back. When on,
    `connect` is written up to 3 times until the UDC state shows a computer (5 s each); a re-plug
    that fails is logged as an error and not tried again. Cost if wrong (it works and stays off):
    after a crash during USB storage the computer keeps the drive until it ejects it or the cable
    is pulled; nothing is lost.
11. **Off only once mounted.** `mount_back` publishes Off only once `/proc/mounts` shows the
    volume; otherwise the phase stays Stopping (the takeover stays) with "Recordings unavailable,
    restart Wave". Cost if wrong (a transient mount failure): the user restarts Wave; there is no
    retry in place.
12. **No mount after a stopped check.** `fsck.exfat -p` killed at 120 s may have left the volume
    half repaired: it stays unmounted (as in 11); the next waved checks it again, a reboot mounts
    it as stock does. Cost if wrong (the check was only slow): the recordings are away until a
    restart.
13. *(Superseded by R1/R4: nothing of G1 at boot.)* **The boot checks the bind.** The patched `start_usbgadget` unlinks the drive and binds again
    if the gadget did not bind with it (`$G/UDC` empty), logging why in
    `/tmp/waved-usbgadget.log`. Cost if wrong (the fallback misfires on an adb timing failure):
    USB storage is unavailable until the next boot; audio and adb stay.
14. **No delete, play or start during an install.** Delete and Play are refused while the volume
    is away; a delete that finds the recordings directory missing keeps its claim instead of
    removing the rows. `StartUsbStorage` is refused while an update installs (Installing,
    Restarting); the update agent adds the reverse check.

## State and commands (waved-state, additive)

- `Command::StartUsbStorage`, `Command::StopUsbStorage`
- `UsbStorageState { phase: UsbStoragePhase }`, `UsbStoragePhase::{Off, Starting, On, Stopping}`,
  `StateBus::usb_storage`
- `DeviceEvent::UsbStorageProblem(UsbStorageProblem)`:
  `Recording | NoComputer | Unavailable | Busy | EjectFirst | NotMounted`

## Tasks (TDD: failing test first)

1. waved-state: types, commands, event, bus channel.
2. waved-system `usb_storage`: `Gadget` (mode, enter/leave storage mode, LUN file, UDC state;
   reworked per R1–R3: `device::ConfigfsGadget` runs stock's switch, `device::Adbd` /
   `SystemAdbd` stops and starts adbd) and `Volume` (sync, mounted spec, unmount, check, mount)
   traits, configfs and system implementations, fakes. The `UsbStorage` core (start, stop, poll,
   reclaim) and the `run` thread (commands plus a 1 s poll).
3. waved main.rs (additive): `reclaim_at_start` before the meeting service; spawn the service;
   hand the meeting service the storage watch.
4. waved-meeting (additive, Linux-only tests): gate StartRecording and the upload worker, and
   reconcile on return.
5. waved-ui: `UiSnapshot::usb_storage`; router takeover; route guards; the confirmation sends
   `StopUsbStorage`; the `usb_storage` screen and its golden; the System row; message copy for
   problems and "USB storage stopped"; registry; README.
6. *(R4)* `scripts/patch-stock-usbgadget.sh`: no USB storage step; removes the one an earlier
   version added; `scripts/test-patch-stock-usbgadget.sh` checks both old variants come out
   byte-identical to a unit never given G1.
7. Docs: `waved/docs/USB_STORAGE.md`, `waved/CLAUDE.md` table, root `TODO.md` G1.

## Device-pass checklist

The checklist for the rework (R1–R4) is in `waved/docs/USB_STORAGE.md`, "Device-pass checklist":
first a run with the real volume (copy a file, eject in Finder, adb and audio come back), then
Stop without eject, a cable pull, and a waved restart in storage mode; then a large copy, Mac
sleep, the refusals and the PIN. Recovery commands: the same doc, "Recovering by hand". The
first design's checklist (boot patch, `soft_connect` re-plug) is obsolete: never apply the old
G1 boot step.
