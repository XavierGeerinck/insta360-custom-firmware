---
title: Revert to stock
description: Remove waved with one script and a reboot, and what that keeps and removes.
---

waved never writes the Wave's system partition, so the stock system is still
there. Removing waved's files from `/data` and rebooting brings the stock app
back.

## Revert

With the Wave connected over USB (waved keeps ADB on at every boot), from the
repository root:

```bash
cd waved
./deploy/revert.sh
adb reboot
```

The stock app starts after the reboot.

## What it removes

From the `/etc` and `/usr` overlays, stored under `/data/overlay/upper`.
Where stock has its own version of a file, that version is back:

- `/etc/init.d/S60waved`, waved's start-up script,
- `/etc/init.d/S60pspapp_daemon`, the stub that kept the stock app from
  starting,
- `/etc/init.d/S03splash`, the boot splash script,
- `/etc/init.d/S89usbgadget`, the patched USB gadget script,
- `/etc/rsyslog.conf`, the logging fix,
- `/usr/lib/libapplogrs.so`, the empty stand-in for the telemetry library,
- the `.prev` and `.new` copies that updates leave next to `S60waved`,
  `S03splash` and `rsyslog.conf`.

And all of `/data/waved`: waved, `config.toml`, the log, the splash image, the
firewall rules, the recordings list with each recording's upload state
(`meetings.db`), and the update state.

## After the reboot

ADB is off again, as on stock. To turn it on, still in `waved/`:

```bash
sudo python3 scripts/enable_adb.py
```

`revert.sh` ends by printing the same command with the script's full path.

## What it keeps

- **Your recordings.** `revert.sh` does not touch `/mnt/udisk`, so the
  folders in `/mnt/udisk/recordings` (each with `audio.wav` and
  `metadata.json`) stay. To copy them to your computer first:

  ```bash
  adb pull /mnt/udisk/recordings ./wave-recordings
  ```

- **Saved Wi-Fi networks.** `/etc/wpa_supplicant.conf` is not in the list
  above, so it stays.
- **The stock app's own data** elsewhere on `/data`.

## Going further back

If the soft revert above is not enough, the Updates guide lists deeper paths:
the stock updater, a manual `swupdate` over ADB, and a raw partition restore.
They are research notes and none of them has been tested.
[Paths back, least invasive first](/insta360-custom-firmware/guides/updates/#paths-back-least-invasive-first).

## Installing again

Turn ADB on as above, then, still in `waved/`, run the installer again:

```bash
./deploy/install.sh
```

[Install](/insta360-custom-firmware/start/install/) has the full steps.
