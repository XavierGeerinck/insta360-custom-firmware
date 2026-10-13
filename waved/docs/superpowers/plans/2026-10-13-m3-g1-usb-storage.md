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

## Decisions

1. **Swap the medium, never the gadget.** `scripts/patch-stock-usbgadget.sh` links
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
6. **A waved restart while exported** (crash and watchdog): before the meeting service opens
   the database, `reclaim_at_start` unloads the LUN. If the computer holds it, it re-plugs the
   USB connection (`soft_connect`) to release it, then checks and mounts the volume. Without
   that, reconciliation would mark every recording missing.
7. **The meeting service waits while the volume is away** (`UsbStorageState::recordings_away`):
   no new recording, no uploads (an upload would mark the file missing), and reconciliation
   runs again when the volume is back, so recordings deleted on the computer show as missing.
8. **Stopping is not PIN-locked.** The takeover and its confirmation are no longer guarded by
   System's PIN. Pulling the cable stops it anyway; a PIN would only stand in the way. Starting
   still needs the PIN (System).
9. **Copy:** the mockup's "Audio is off until you stop" is wrong under decision 1. The screen says
   "Your computer can open the recordings. Recording is off until you stop."

## State and commands (waved-state, additive)

- `Command::StartUsbStorage`, `Command::StopUsbStorage`
- `UsbStorageState { phase: UsbStoragePhase }`, `UsbStoragePhase::{Off, Starting, On, Stopping}`,
  `StateBus::usb_storage`
- `DeviceEvent::UsbStorageProblem(UsbStorageProblem)`:
  `Recording | NoComputer | Unavailable | Busy | EjectFirst | NotMounted`

## Tasks (TDD: failing test first)

1. waved-state: types, commands, event, bus channel.
2. waved-system `usb_storage`: `Gadget` (LUN file, removable and link check, UDC state,
   re-plug) and `Volume` (sync, mounted spec, unmount, check, mount) traits, configfs and
   system implementations, fakes. The `UsbStorage` core (start, stop, poll, reclaim) and the
   `run` thread (commands plus a 1 s poll).
3. waved main.rs (additive): `reclaim_at_start` before the meeting service; spawn the service;
   hand the meeting service the storage watch.
4. waved-meeting (additive, Linux-only tests): gate StartRecording and the upload worker, and
   reconcile on return.
5. waved-ui: `UiSnapshot::usb_storage`; router takeover; route guards; the confirmation sends
   `StopUsbStorage`; the `usb_storage` screen and its golden; the System row; message copy for
   problems and "USB storage stopped"; registry; README.
6. `scripts/patch-stock-usbgadget.sh`: idempotent MSC link step in `start_usbgadget`.
7. Docs: `waved/docs/USB_STORAGE.md`, `waved/CLAUDE.md` table, root `TODO.md` G1.

## Device-pass checklist

Run from the Mac with adb. `G=/sys/kernel/config/usb_gadget/amlogic`,
`L=$G/functions/mass_storage.usb0/lun.0`. Watch `tail -f /data/waved/waved.log` (look for
`usb storage:` lines) and Finder / `diskutil list` on the Mac.

1. **Before anything, record the gadget:**
   `adb shell 'ls -l $G/configs/amlogic.1/; for f in file removable ro nofua; do echo $f=$(cat $L/$f); done; cat /sys/class/udc/*/state; ls /sys/class/udc/*/; grep udisk /proc/mounts; ls -l /dev/udisk'`
   (expand `$G`/`$L` by hand inside the single quotes). Expect no `mass_storage.usb0` link yet
   and `file=/dev/...` (stock `msc_init` holds the partition open, unlinked).
2. **Patch and reboot:** `adb push waved/scripts/patch-stock-usbgadget.sh /tmp/ && adb shell
   'sh /tmp/patch-stock-usbgadget.sh' && adb reboot`. After boot, repeat step 1: expect the
   `mass_storage.usb0` link, `removable=1` and `file=` empty. On the Mac: Wave's mic and speaker
   still listed in Audio MIDI Setup, `adb devices` still lists Wave, nothing new in Finder, and
   `system_profiler SPUSBDataType` shows a mass-storage interface. In Windows (if at hand):
   no empty drive letter in Explorer.
3. **Deploy waved** (`docs/DISPLAY_HARDWARE.md#deploy`).
4. **Start:** System → USB storage. Expect the takeover screen within 2 s, `grep udisk
   /proc/mounts` empty, `cat $L/file` = the partition, the volume ("Wave Files") on the Mac
   desktop, and `adb devices` still connected. Play music on the Mac through Wave and talk
   into it: audio keeps working.
5. **Copy a large recording to the Mac** (≥ 500 MB) while a call or music runs. Watch
   `/tmp/audio_stats.json` (`xruns`, `uac`) for glitches during the copy.
6. **Eject on the Mac** (Finder ⏏). Expect the takeover to leave by itself, "USB storage
   stopped", `cat $L/file` empty, `/mnt/udisk` mounted again, Files lists the recordings.
7. **Stop without ejecting:** start again, then Stop USB storage → Stop. If macOS locked the
   medium: "Eject Wave on the computer first" and the screen stays; eject, and it leaves.
   If not locked: storage stops and the Mac warns "Disk Not Ejected Properly". Note which one
   happens in USB_STORAGE.md.
8. **Cable pull:** start, unplug the cable. Within ~4 s the takeover leaves, the volume is
   mounted again (`grep udisk /proc/mounts`), and `fsck.exfat -p` ran (log). Plug back in:
   no drive appears, audio works.
9. **Mac sleep:** start, close the lid 30 s, open. The drive is still there; Wave still in
   storage mode.
10. **Refusals:** start while recording ("Stop the recording first"); with the cable out
    ("Connect a computer first"); while a file plays in Files (start succeeds, playback
    stops first). With an `adb shell` sitting in `cd /mnt/udisk`: "Recordings in use, try
    again", and nothing changed (`/mnt/udisk` still mounted, `file` empty).
11. **Restart while exported:** start, then `adb shell killall waved`. After the watchdog
    restart (~3 s), the log shows `usb storage: reclaimed`, the Mac loses the drive (re-plug),
    `/mnt/udisk` is mounted, and Files shows every recording (not Damaged).
12. **PIN:** with a PIN set, System asks for it before USB storage; Stop does not.

### Recovering by hand

- Take the drive back from the computer and mount it here:
  `adb shell 'echo > $L/file; cat $L/file; fsck.exfat -p /dev/udisk; mount -t exfat /dev/udisk /mnt/udisk -o rw,fmask=0077'`
  (`echo > file` fails with "Device or resource busy" while the computer holds the drive: eject
  it on the computer or unplug the cable first.)
- The gadget is never unbound by waved. If it is anyway (`cat $G/UDC` empty):
  `adb shell 'echo fdd00000.crgudc2 > $G/UDC'` (over UART if adb is gone).
- Force a re-plug without touching the cable:
  `echo disconnect > /sys/class/udc/fdd00000.crgudc2/soft_connect; sleep 1; echo connect > /sys/class/udc/fdd00000.crgudc2/soft_connect`.
- Undo the gadget patch: restore `/data/waved/S89usbgadget.bak.<ts>` over
  `/etc/init.d/S89usbgadget` and reboot.
